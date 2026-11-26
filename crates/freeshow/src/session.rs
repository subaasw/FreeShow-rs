// SPDX-License-Identifier: GPL-3.0-only

use freeshow_core::{NetworkRoute, SpeakerRoute};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use freeshow_data::{
    AUDIO_DIR, BibleFile, Catalog, DrawStroke, EffectSpec, FreeShowPreferences, Library, MEDIA_DIR,
    MediaEntry, MediaEntryKind, MixStep, OutlinedBook, PlayerVideo, ProjectsFile, ShowFile,
    SlideBackdrop, SlideJump, StageLayout, TimedCueTarget, TimelineCue, TimerStep, VerseHit,
    default_stage, new_id, parse_timeline_text, timeline_cues_from_show, timeline_for_show,
    timeline_text_from,
};
use freeshow_media::{MediaKind, cameras, media_kind};

use crate::slide_countdown::SlideCountdown;

mod drawer_edit;
mod editor_text;
mod layer_builds;
mod library_edit;
mod panels;
mod recovery;
mod slide_layers;
use freeshow_output::{
    Fit, OUTPUT_HEIGHT, OUTPUT_WIDTH, Raster, aligned_output_size, crop_raster, decode_image,
};
pub(crate) use layer_builds::LayerBuilds;
pub use library_edit::ProjectPick;
pub use panels::{
    AUTOSAVE_CHOICES, STAGE_ADD_KINDS, STAGE_ADD_NAMES, STAGE_DATE_FORMATS, TRANSITION_KINDS,
    lan_host, setting_label,
};
pub use slide_layers::LayerDrag;

/// Network feeds due for a refresh: weather locations and RSS URLs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedRequest {
    pub weather: Vec<(f64, f64)>,
    pub rss: Vec<String>,
}

/// What a background worker fetched for a [`FeedRequest`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedResults {
    pub weather: Vec<crate::feeds::WeatherReport>,
    /// `None` when RSS was not requested or every feed failed.
    pub rss: Option<Vec<crate::feeds::RssItem>>,
}

impl FeedRequest {
    /// Fetch everything in this request. Blocking; run it off the UI thread.
    pub fn fetch(
        &self,
        weather: impl Fn(f64, f64) -> Result<crate::feeds::WeatherReport, String>,
        rss: impl Fn(&str) -> Result<Vec<crate::feeds::RssItem>, String>,
    ) -> FeedResults {
        let reports = self
            .weather
            .iter()
            .filter_map(|(lat, lon)| weather(*lat, *lon).ok())
            .collect();
        let mut fetched = false;
        let mut items = Vec::new();
        for url in &self.rss {
            if let Ok(feed) = rss(url) {
                fetched = true;
                items.extend(feed);
            }
        }
        FeedResults {
            weather: reports,
            rss: fetched.then_some(items),
        }
    }
}

/// Linear gain from a volume action. A value above 5 is a percent. Decibels convert to gain.
#[derive(Clone)]
struct PendingActions {
    due: Instant,
    show_id: String,
    slide_index: usize,
    actions: Vec<serde_json::Value>,
}

struct PendingCueCompletion {
    due: Instant,
    show_id: String,
    layout_id: String,
    index: usize,
}

struct PendingCueActions {
    started: Instant,
    show_id: String,
    slide_index: usize,
    cues: VecDeque<TimelineCue>,
}

/// An index in `0..count` that is not `current`.
fn other_slide(count: usize, current: usize) -> usize {
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    state ^= (current as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    loop {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let pick = (state as usize) % count;
        if pick != current {
            return pick;
        }
    }
}

fn mix_gain(volume: Option<f64>, use_db: bool, current: f32, unmuted: &mut f32) -> f32 {
    let Some(volume) = volume else {
        if current > 0.0 {
            *unmuted = current;
            return 0.0;
        }
        return *unmuted;
    };
    let mut gain = volume as f32;
    if use_db {
        if gain <= -60.0 {
            return 0.0;
        }
        gain = 10f32.powf(gain / 20.0);
    } else if gain > 5.0 {
        gain /= 100.0;
    }
    (gain * 1000.0).round() / 1000.0
}

#[derive(Debug)]
pub enum PendingPlay {
    Audio(PathBuf),
    Video(PathBuf),
    Camera(String),
    StopAudio,
    StopVideo,
    ToggleMedia,
    PauseAudio,
    ResumeAudio,
    SeekAudio(u64),
    SeekVideo(u64),
    SetLoop,
    SetMute,
}

pub enum DrawerAction {
    None,
    Present,
    /// Repaint the output as it is (an overlay or template changed) without going live.
    Refresh,
    PlayAudio(PathBuf),
    PlayVideo(PathBuf),
    PlayCamera(String),
}

struct TimelineSoundtrack {
    path: PathBuf,
    transport: Option<MediaTransport>,
    delay_ms: u64,
    restart: bool,
    owned: bool,
}

#[derive(Clone, Copy, Default)]
enum VideoEnd {
    #[default]
    Hold,
    Clear {
        fade: bool,
    },
    Black {
        fade: bool,
    },
}

impl VideoEnd {
    fn from_media(media: &serde_json::Value) -> Self {
        match media["endBehavior"].as_str() {
            Some("clear") => Self::Clear { fade: false },
            Some("fade_clear") => Self::Clear { fade: true },
            Some("black") => Self::Black { fade: false },
            Some("fade_black") => Self::Black { fade: true },
            _ => Self::Hold,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct MediaTransport {
    from: f64,
    to: f64,
    rate: f64,
}

impl MediaTransport {
    fn from_value(transport: &serde_json::Value) -> Option<Self> {
        Some(Self {
            from: transport["fromTime"].as_f64()?,
            to: transport["toTime"].as_f64()?,
            rate: transport["speed"].as_f64()?,
        })
    }
}

pub struct Session {
    pub root: Option<PathBuf>,
    pub icecast: Option<(freeshow_core::IcecastDestination, bool)>,
    /// Settings read from the FreeShow library, not this app's own settings file.
    pub preferences: FreeShowPreferences,
    /// Audience framebuffer size: 1920×1080, or the active output's stored bounds.
    pub output_width: u32,
    pub output_height: u32,
    configured_stage_size: Option<(u32, u32)>,
    pub audio_device_map: std::collections::BTreeMap<String, String>,
    pub shows: Vec<ShowFile>,
    pub projects: ProjectsFile,
    pub project_id: Option<String>,
    pub project_item: usize,
    /// True once the operator picked `project_item` in the project list (or it was opened
    /// from there); a show opened from the library leaves the project list unselected.
    pub project_picked: bool,
    folder_paths: Vec<PathBuf>,
    folder_index: usize,
    pub tab: String,
    /// Quick-search text that filters the current drawer list.
    pub search_query: String,
    pub metronome_on: bool,
    metronome_started: Option<std::time::Instant>,
    pub metronome_beat: u32,
    history: Vec<HistoryEntry>,
    layer_selection: Option<slide_layers::LayerSelection>,
    drafts: Vec<recovery::PendingDraft>,
    pending_editor_text: Vec<editor_text::PendingEditorText>,
    timer_save_bases: HashMap<String, serde_json::Value>,
    /// Last library action result, shown in the status bar.
    pub notice: String,
    /// Shows deleted this session, newest last, for Restore.
    deleted_shows: Vec<ShowFile>,
    redo: Vec<HistoryEntry>,
    pub focus_mode: bool,
    pub guide_visible: bool,
    pub kiosk: bool,
    pub always_on_top: bool,
    pub active_scene: Option<String>,
    pub active_interaction: Option<String>,
    pub draw_strokes: Vec<DrawStroke>,
    pub player_now: Option<String>,
    pub player_videos: Vec<PlayerVideo>,
    /// A YouTube or Vimeo page being turned into a playable address off the UI thread.
    pub stream_lookup: Option<std::sync::mpsc::Receiver<(String, Option<String>)>>,
    pub active_effects: Vec<String>,
    pub timeline_cues: Vec<TimelineCue>,
    pub timeline_playing: bool,
    timeline_started: Option<std::time::Instant>,
    timeline_fired: usize,
    timeline_loop: bool,
    timeline_end_ms: Option<u64>,
    timeline_soundtrack: Option<TimelineSoundtrack>,
    pub show_index: Option<usize>,
    pub slide_index: Option<usize>,
    pub(crate) program_content: std::cell::RefCell<Option<crate::live::ProgramContent>>,
    pub(crate) layer_builds: Option<LayerBuilds>,
    pub(crate) slide_countdown: Option<SlideCountdown>,
    layer_build_stepped: bool,
    /// The selected text box on the current slide.
    pub item_index: usize,
    pub templates: Vec<String>,
    pub template_catalog: Catalog,
    template_pick: Option<String>,
    template_favorites: BTreeSet<String>,
    pub(crate) template_favorites_only: bool,
    pub overlays: Vec<String>,
    pub overlay_catalog: Catalog,
    pub active_overlays: Vec<String>,
    pub bibles: Vec<BibleFile>,
    pub scripture: Arc<Vec<freeshow_remote::RemoteBible>>,
    pub media: Vec<MediaEntry>,
    pub audio: Vec<String>,
    /// Calendar labels, soonest first, with the matching ids in `event_ids`.
    pub events: Vec<String>,
    pub event_ids: Vec<String>,
    /// The Calendar event or Player video picked for editing in the drawer form.
    pub drawer_pick: Option<usize>,
    pub event_catalog: Catalog,
    pub fired_events: Vec<String>,
    pub verse_hits: Vec<VerseHit>,
    pub verse_index: Option<usize>,
    pub bible_index: Option<usize>,
    pub media_index: Option<usize>,
    pub audio_index: Option<usize>,
    pub underlay: Option<Raster>,
    pub network_background: bool,
    /// Path the current still underlay was decoded from, if any.
    underlay_from: Option<PathBuf>,
    pub background_fit: Fit,
    pub background_flip_x: bool,
    pub background_flip_y: bool,
    pub background_filter: String,
    pub background_blend: String,
    pub background_blur_px: f32,
    pub background_blur_opacity: f32,
    /// Background colour of the active output style; black when the style names none.
    pub style_background: Option<freeshow_output::Rgba>,
    /// The template the active output style restyles every slide with on the output.
    pub style_template: Option<String>,
    /// The next output change is a background image, so it uses the media transition.
    pub background_transition: bool,
    /// Path of the picture currently behind the slide, when one is showing.
    background_path: Option<PathBuf>,
    video_end: VideoEnd,
    video_end_fade: Option<bool>,
    video_transport: Option<MediaTransport>,
    imported_foreground_active: bool,
    pub volume: f32,
    /// The main channel is muted. Playback stays silent until a slide unmutes it.
    pub audio_muted: bool,
    /// Volume restored when a slide toggles the main channel back on.
    unmuted_volumes: std::collections::BTreeMap<String, f32>,
    pub pending_play: Option<PendingPlay>,
    /// A second playback for the same slide, so a video and an audio file can both start.
    pub pending_also: Option<PendingPlay>,
    pub playing_audio: Option<PathBuf>,
    audio_loop_override: Option<bool>,
    audio_transport: Option<MediaTransport>,
    transition_sound_playing: bool,
    /// The audio playlist started by a slide, when one is running.
    playlist_id: Option<String>,
    playlist_index: usize,
    playlist_songs: Vec<PathBuf>,
    playlist_loop: bool,
    playlist_volume: f32,
    /// A sound effect is playing in place of the playlist song.
    effect_playing: Option<PathBuf>,
    /// Playlist song to start again when the sound effect ends.
    effect_return: Option<PathBuf>,
    /// The last file end that already moved the show.
    media_finished: Option<(String, bool)>,
    /// The show moved because media ended, so the output windows need a new frame.
    pub refresh_output: bool,
    /// The operator is looking at another show, slide or tab; the output keeps what is live.
    pub program_held: bool,
    /// An edit changed the show that is on air, so the output repaints without a transition.
    pub live_edited: bool,
    /// Live commands from the operator window, run with the remote batch.
    pub ui_commands: Vec<freeshow_remote::Command>,
    pub audio_paused: bool,
    pub media_paused: bool,
    pub media_loop: bool,
    pub media_muted: bool,
    pub video_rate: f64,
    pub video_volume: f64,
    pub video_from: f64,
    pub video_to: f64,
    pub video_time: f64,
    pub video_duration: f64,
    pub audio_time: f64,
    pub audio_duration: f64,
    pub blanked: bool,
    /// The audience slide is cleared because the current media is the whole output.
    pub slide_hidden: bool,
    /// How many lines of a line-reveal item are on the audience. Zero hides them.
    pub reveal_count: u32,
    /// A click-reveal item is on the audience. It stays hidden until Next.
    pub click_revealed: bool,
    /// First lyric line shown when the output style pages the slide.
    pub line_index: u32,
    /// How many random-slide hops are already in progress.
    random_hops: u8,
    /// Slide actions waiting out a wait action.
    pending_actions: Option<PendingActions>,
    /// Countdown captured when this slide was shown. A later timer action does not change it.
    timed_seconds: Option<f64>,
    pending_cue_completion: Option<PendingCueCompletion>,
    pending_cue_actions: Option<PendingCueActions>,
    pub stage: StageLayout,
    pub(crate) stage_layout_id: Option<String>,
    pub stage_media: std::cell::RefCell<crate::stage::StageMedia>,
    pub timer_left: u64,
    pub timer_deadline: Option<Instant>,
    pub timers: Vec<NamedTimer>,
    pub running: Option<RunningTimer>,
    pub recording: bool,
    pub chrome: crate::operator::SessionChrome,
    pub warnings: Vec<String>,
}

/// An NDI picture the library asked to broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NdiTarget {
    pub id: String,
    pub name: String,
    pub stage: bool,
    /// NDI discovery group. Members of the same group share one sender.
    pub group: Option<String>,
}

impl NdiTarget {
    pub fn grouped_name(&self) -> String {
        match &self.group {
            Some(group) => format!("{} [{group}]", self.name),
            None => self.name.clone(),
        }
    }
}

/// An OMT picture the library asked to broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmtTarget {
    pub id: String,
    pub name: String,
    pub stage: bool,
}

/// A WHIP WebRTC picture the library asked to broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebrtcTarget {
    pub id: String,
    pub url: String,
    pub token: Option<String>,
    pub stage: bool,
}

/// An RTMP destination the library asked to start automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtmpTarget {
    pub id: String,
    pub url: String,
    pub file: bool,
}

fn is_stage_output(output: &serde_json::Value) -> bool {
    output["stageOutput"]
        .as_str()
        .is_some_and(|layout| !layout.is_empty())
}

fn named_source(output: &serde_json::Value, data_key: &str, fallback: &str) -> String {
    let valid = |value: &serde_json::Value| {
        value
            .as_str()
            .filter(|name| !name.is_empty() && !name.contains('\0'))
            .map(str::to_string)
    };
    valid(&output[data_key]["name"]).unwrap_or_else(|| {
        valid(&output["name"])
            .map(|name| format!("{fallback} - {name}"))
            .unwrap_or_else(|| fallback.into())
    })
}

fn export_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "show".to_string()
    } else {
        trimmed.to_string()
    }
}

impl Session {
    pub fn stage_output_size(&self) -> (u32, u32) {
        self.configured_stage_size
            .unwrap_or((self.output_width, self.output_height))
    }

    pub fn empty() -> Self {
        Self {
            root: None,
            icecast: None,
            preferences: FreeShowPreferences::default(),
            output_width: OUTPUT_WIDTH,
            output_height: OUTPUT_HEIGHT,
            configured_stage_size: None,
            audio_device_map: Default::default(),
            shows: Vec::new(),
            projects: ProjectsFile::empty(),
            project_id: None,
            project_item: 0,
            project_picked: false,
            folder_paths: Vec::new(),
            folder_index: 0,
            tab: "shows".to_string(),
            search_query: String::new(),
            metronome_on: false,
            metronome_started: None,
            metronome_beat: 0,
            history: Vec::new(),
            layer_selection: None,
            drafts: Vec::new(),
            pending_editor_text: Vec::new(),
            timer_save_bases: HashMap::new(),
            notice: String::new(),
            deleted_shows: Vec::new(),
            redo: Vec::new(),
            focus_mode: false,
            guide_visible: false,
            kiosk: false,
            always_on_top: true,
            active_scene: None,
            active_interaction: None,
            draw_strokes: Vec::new(),
            player_now: None,
            player_videos: Vec::new(),
            stream_lookup: None,
            active_effects: Vec::new(),
            timeline_cues: Vec::new(),
            timeline_playing: false,
            timeline_started: None,
            timeline_fired: 0,
            timeline_loop: false,
            timeline_end_ms: None,
            timeline_soundtrack: None,
            show_index: None,
            slide_index: None,
            program_content: Default::default(),
            layer_builds: None,
            slide_countdown: None,
            layer_build_stepped: false,
            item_index: 0,
            templates: Vec::new(),
            template_catalog: Catalog::empty(),
            template_pick: None,
            template_favorites: BTreeSet::new(),
            template_favorites_only: false,
            overlays: Vec::new(),
            overlay_catalog: Catalog::empty(),
            active_overlays: Vec::new(),
            bibles: Vec::new(),
            scripture: Arc::new(Vec::new()),
            media: Vec::new(),
            audio: Vec::new(),
            events: Vec::new(),
            event_ids: Vec::new(),
            drawer_pick: None,
            event_catalog: Catalog::empty(),
            fired_events: Vec::new(),
            verse_hits: Vec::new(),
            verse_index: None,
            bible_index: None,
            media_index: None,
            audio_index: None,
            underlay: None,
            network_background: false,
            underlay_from: None,
            background_fit: Fit::Contain,
            background_flip_x: false,
            background_flip_y: false,
            background_filter: String::new(),
            background_blend: String::new(),
            background_blur_px: 6.0,
            background_blur_opacity: 0.3,
            style_background: None,
            style_template: None,
            background_transition: false,
            background_path: None,
            video_end: VideoEnd::Hold,
            video_end_fade: None,
            video_transport: None,
            imported_foreground_active: false,
            volume: 1.0,
            audio_muted: false,
            unmuted_volumes: Default::default(),
            pending_play: None,
            pending_also: None,
            playing_audio: None,
            audio_loop_override: None,
            audio_transport: None,
            transition_sound_playing: false,
            playlist_id: None,
            playlist_index: 0,
            playlist_songs: Vec::new(),
            playlist_loop: true,
            playlist_volume: 1.0,
            effect_playing: None,
            effect_return: None,
            media_finished: None,
            refresh_output: false,
            program_held: false,
            live_edited: false,
            ui_commands: Vec::new(),
            audio_paused: false,
            media_paused: false,
            media_loop: true,
            media_muted: true,
            video_rate: 1.0,
            video_volume: 1.0,
            video_from: 0.0,
            video_to: 0.0,
            video_time: 0.0,
            video_duration: 0.0,
            audio_time: 0.0,
            audio_duration: 0.0,
            blanked: false,
            slide_hidden: false,
            reveal_count: 0,
            click_revealed: false,
            line_index: 0,
            random_hops: 0,
            pending_actions: None,
            timed_seconds: None,
            pending_cue_completion: None,
            pending_cue_actions: None,
            stage: default_stage(),
            stage_layout_id: None,
            stage_media: Default::default(),
            timer_left: 300,
            timer_deadline: None,
            timers: vec![NamedTimer::default_countdown()],
            running: None,
            recording: false,
            chrome: crate::operator::SessionChrome::default(),
            warnings: Vec::new(),
        }
    }

    pub fn open(root: PathBuf) -> Self {
        let library = Library::open(&root);
        let preferences = library.preferences();
        let mut warnings = Vec::new();
        if let Some(notes) = take(library.import_scripture_sources(), &mut warnings) {
            warnings.extend(notes);
        }
        if let Some(notes) = take(library.import_show_sources(), &mut warnings) {
            warnings.extend(notes);
        }
        let projects =
            take(library.load_projects(), &mut warnings).unwrap_or_else(ProjectsFile::empty);
        let project_id = library_edit::startup_project(&projects, &root);
        let template_catalog =
            take(library.load_templates(), &mut warnings).unwrap_or_else(Catalog::empty);
        let templates = template_catalog.names();
        let template_favorites =
            take(library.load_template_favorites(), &mut warnings).unwrap_or_default();
        let overlay_catalog =
            take(library.load_overlays(), &mut warnings).unwrap_or_else(Catalog::empty);
        let overlays = overlay_catalog.names();
        let (drafts, draft_warnings) = library.load_show_drafts();
        warnings.extend(draft_warnings);
        let shows = take(library.list_shows(), &mut warnings).unwrap_or_default();
        let bibles = take(library.list_bibles(), &mut warnings).unwrap_or_default();
        let scripture = scripture_catalog(&bibles);
        let mut media = take(library.list_media_entries(), &mut warnings).unwrap_or_default();
        for camera in cameras() {
            media.push(MediaEntry::camera(&camera));
        }
        let audio = take(
            library.list_media_files(&library.audio_dir()),
            &mut warnings,
        )
        .unwrap_or_default();
        let (stage_layout_id, stage) = library.load_stage_layout_with_id();
        let event_catalog =
            take(library.load_events(), &mut warnings).unwrap_or_else(Catalog::empty);
        let (event_ids, events) = drawer_edit::event_rows(&event_catalog, &shows);
        let synced =
            take(library.load_synced_settings(), &mut warnings).unwrap_or_else(Catalog::empty);
        let timers = named_timers(&synced);
        let (volume, audio_muted) = library.main_audio();
        let (output_width, output_height) = preferences
            .configured_output_size()
            .map(|(width, height)| aligned_output_size(width, height))
            .unwrap_or((OUTPUT_WIDTH, OUTPUT_HEIGHT));
        let configured_stage_size = preferences
            .configured_stage_size()
            .map(|(width, height)| aligned_output_size(width, height));
        let active_overlays = preferences.locked_overlay_ids(&library.audience_output().0);
        freeshow_output::register_custom_fonts(&preferences.custom_font_files());
        let focus_mode = preferences.focus_mode_setting();
        let guide_visible = !preferences.guide_seen();
        let kiosk = preferences.output_kiosk();
        let always_on_top = preferences.output_always_on_top();
        let draw_strokes = preferences.draw_strokes();
        let chrome = crate::operator::SessionChrome {
            profile_locked: crate::operator::profiles_from(&preferences.profiles)
                .iter()
                .any(|profile| profile.password.is_some()),
            ..crate::operator::SessionChrome::default()
        };
        let mut session = Self {
            root: Some(root),
            icecast: library.icecast(),
            preferences,
            output_width,
            output_height,
            configured_stage_size,
            audio_device_map: Default::default(),
            shows,
            projects,
            project_id,
            project_item: 0,
            project_picked: false,
            folder_paths: Vec::new(),
            folder_index: 0,
            tab: "shows".to_string(),
            search_query: String::new(),
            metronome_on: false,
            metronome_started: None,
            metronome_beat: 0,
            history: Vec::new(),
            layer_selection: None,
            drafts: drafts
                .into_iter()
                .map(recovery::PendingDraft::recorded)
                .collect(),
            pending_editor_text: Vec::new(),
            notice: String::new(),
            deleted_shows: Vec::new(),
            timer_save_bases: HashMap::new(),
            redo: Vec::new(),
            focus_mode,
            guide_visible,
            kiosk,
            always_on_top,
            active_scene: None,
            active_interaction: None,
            draw_strokes,
            player_now: None,
            player_videos: Vec::new(),
            stream_lookup: None,
            active_effects: Vec::new(),
            timeline_cues: Vec::new(),
            timeline_playing: false,
            timeline_started: None,
            timeline_fired: 0,
            timeline_loop: false,
            timeline_end_ms: None,
            timeline_soundtrack: None,
            show_index: None,
            slide_index: None,
            program_content: Default::default(),
            layer_builds: None,
            slide_countdown: None,
            layer_build_stepped: false,
            item_index: 0,
            templates,
            template_catalog,
            template_pick: None,
            template_favorites,
            template_favorites_only: false,
            overlays,
            overlay_catalog,
            active_overlays,
            bibles,
            scripture,
            media,
            audio,
            events,
            event_ids,
            drawer_pick: None,
            event_catalog,
            fired_events: Vec::new(),
            verse_hits: Vec::new(),
            verse_index: None,
            bible_index: None,
            media_index: None,
            audio_index: None,
            underlay: None,
            network_background: false,
            underlay_from: None,
            background_fit: Fit::Contain,
            background_flip_x: false,
            background_flip_y: false,
            background_filter: String::new(),
            background_blend: String::new(),
            background_blur_px: 6.0,
            background_blur_opacity: 0.3,
            style_background: None,
            style_template: None,
            background_transition: false,
            background_path: None,
            video_end: VideoEnd::Hold,
            video_end_fade: None,
            video_transport: None,
            imported_foreground_active: false,
            volume,
            audio_muted,
            unmuted_volumes: Default::default(),
            pending_play: None,
            pending_also: None,
            playing_audio: None,
            audio_loop_override: None,
            audio_transport: None,
            transition_sound_playing: false,
            playlist_id: None,
            playlist_index: 0,
            playlist_songs: Vec::new(),
            playlist_loop: true,
            playlist_volume: 1.0,
            effect_playing: None,
            effect_return: None,
            media_finished: None,
            refresh_output: false,
            program_held: false,
            live_edited: false,
            ui_commands: Vec::new(),
            audio_paused: false,
            media_paused: false,
            media_loop: true,
            media_muted: true,
            video_rate: 1.0,
            video_volume: 1.0,
            video_from: 0.0,
            video_to: 0.0,
            video_time: 0.0,
            video_duration: 0.0,
            audio_time: 0.0,
            audio_duration: 0.0,
            blanked: false,
            slide_hidden: false,
            reveal_count: 0,
            click_revealed: false,
            line_index: 0,
            random_hops: 0,
            pending_actions: None,
            timed_seconds: None,
            pending_cue_completion: None,
            pending_cue_actions: None,
            stage,
            stage_layout_id,
            stage_media: Default::default(),
            timer_left: 300,
            timer_deadline: None,
            timers,
            running: None,
            recording: false,
            chrome,
            warnings,
        };
        session.restore_watermark();
        session.apply_active_scenes();
        session.load_player_videos();
        session.refresh_style_look();
        session
    }

    /// Re-reads the active output style's background colour after a style or output change.
    pub fn refresh_style_look(&mut self) {
        self.style_background = self
            .root
            .as_deref()
            .and_then(|root| Library::open(root).style_background())
            .and_then(|color| freeshow_output::parse_color(&color));
        self.style_template = self
            .root
            .as_deref()
            .and_then(|root| Library::open(root).style_template());
    }

    pub fn timer_label(&self) -> String {
        let seconds = self
            .running
            .as_ref()
            .map(RunningTimer::shown)
            .unwrap_or_else(|| self.timer_remaining());
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }

    /// The label of every named timer, for stage boxes that point at one id.
    pub fn timer_labels(&self) -> Vec<(String, String)> {
        self.timers
            .iter()
            .map(|timer| {
                let seconds = self
                    .running
                    .as_ref()
                    .filter(|running| running.id == timer.id)
                    .map(RunningTimer::shown)
                    .unwrap_or(timer.start);
                (
                    timer.id.clone(),
                    format!("{:02}:{:02}", seconds / 60, seconds % 60),
                )
            })
            .collect()
    }

    pub fn start_named_timer(&mut self, id: &str) -> bool {
        let Some(timer) = self.timers.iter().find(|timer| timer.id == id) else {
            return false;
        };
        if let Some(running) = self.running.as_mut().filter(|running| running.id == id) {
            if running.since.is_none() {
                running.since = Some(Instant::now());
            }
            return true;
        }
        self.running = Some(RunningTimer {
            id: id.to_string(),
            start: timer.start,
            end: timer.end,
            mark: timer.start,
            since: Some(Instant::now()),
        });
        true
    }

    pub fn pause_named_timer(&mut self, id: &str) -> bool {
        let shown = self
            .running
            .as_ref()
            .filter(|running| running.id == id)
            .map(RunningTimer::shown);
        let Some(shown) = shown else {
            return false;
        };
        let Some(running) = self.running.as_mut() else {
            return false;
        };
        running.mark = shown;
        running.since = None;
        true
    }

    pub fn stop_named_timer(&mut self, id: &str) -> bool {
        if self
            .running
            .as_ref()
            .is_some_and(|running| running.id == id)
        {
            self.running = None;
            return true;
        }
        false
    }

    /// Starts a show when a calendar action's `from` time is within the last second.
    /// A legacy event of type `show` is treated as `start_show`. Each event starts once.
    pub fn start_due_events(&mut self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let due: Vec<(String, String)> = self
            .event_catalog
            .entries()
            .into_iter()
            .filter_map(|(id, event)| {
                if self.fired_events.iter().any(|fired| fired == &id) {
                    return None;
                }
                let show_id = scheduled_show(&event)?;
                let from = event_time(&event)?;
                let left = from.signed_duration_since(now).num_milliseconds();
                (left <= 0 && left > -1000).then_some((id, show_id))
            })
            .collect();
        let mut started = false;
        for (id, show_id) in due {
            if self.present_show_id(&show_id) {
                self.fired_events.push(id);
                started = true;
            }
        }
        started
    }

    fn present_show_id(&mut self, id: &str) -> bool {
        let Some(index) = self.shows.iter().position(|show| show.id == id) else {
            return false;
        };
        self.tab = "shows".to_string();
        self.go_live();
        self.slide_hidden = false;
        self.show_index = Some(index);
        self.reveal_count = 0;
        self.click_revealed = false;
        self.line_index = 0;
        self.slide_index = self.shows.get(index).and_then(|show| {
            if show.ordered_slides().is_empty() {
                None
            } else {
                Some(0)
            }
        });
        self.load_timeline();
        self.apply_slide_actions();
        self.follow_started_show(0);
        self.follow_slide_jump(0);
        true
    }

    /// Opens the show named by this slide, then that show's own start action, up to eight hops.
    fn follow_started_show(&mut self, depth: u8) {
        if depth >= 8 {
            return;
        }
        let Some(next) = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.started_show_at(index))
        }) else {
            return;
        };
        if self.current_show().is_some_and(|show| show.id == next) {
            return;
        }
        let (output_id, output_name) = self.audience();
        let Some(slide) = self
            .shows
            .iter()
            .find(|show| show.id == next)
            .and_then(|show| show.first_output(&output_id, &output_name))
        else {
            return;
        };
        if self.open_show_at(&next, slide) {
            self.follow_started_show(depth + 1);
        }
    }

    fn open_show_at(&mut self, id: &str, slide: usize) -> bool {
        let Some(index) = self.shows.iter().position(|show| show.id == id) else {
            return false;
        };
        let len = self
            .shows
            .get(index)
            .map(|show| show.ordered_slides().len())
            .unwrap_or(0);
        if len == 0 {
            return false;
        }
        self.tab = "shows".to_string();
        self.go_live();
        self.slide_hidden = false;
        self.show_index = Some(index);
        self.slide_index = Some(slide.min(len - 1));
        self.reveal_count = 0;
        self.click_revealed = false;
        self.line_index = 0;
        self.load_timeline();
        self.apply_slide_actions();
        true
    }

    pub fn toggle_timer(&mut self) {
        if self.timer_deadline.is_some() {
            self.timer_left = self.timer_remaining();
            self.timer_deadline = None;
            return;
        }
        let seconds = if self.timer_left == 0 {
            300
        } else {
            self.timer_left
        };
        self.timer_left = seconds;
        self.timer_deadline = Some(Instant::now() + Duration::from_secs(seconds));
    }

    fn timer_remaining(&self) -> u64 {
        if let Some(deadline) = self.timer_deadline {
            deadline.saturating_duration_since(Instant::now()).as_secs()
        } else {
            self.timer_left
        }
    }

    pub fn set_search(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
    }

    pub fn toggle_metronome(&mut self) {
        self.metronome_on = !self.metronome_on;
        if self.metronome_on {
            self.metronome_started = Some(std::time::Instant::now());
            self.metronome_beat = 0;
        } else {
            self.metronome_started = None;
            self.metronome_beat = 0;
        }
    }

    pub fn metronome_beat_at(&self, now: std::time::Instant) -> u32 {
        let Some(started) = self.metronome_started else {
            return 0;
        };
        let bpm = f64::from(self.preferences.metronome_bpm());
        let beats = self.preferences.metronome_beats().max(1);
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        let index = (elapsed * bpm / 60.0).floor() as u32;
        index % beats
    }

    pub fn update_metronome(&mut self, now: std::time::Instant) {
        if self.metronome_on {
            self.metronome_beat = self.metronome_beat_at(now);
        }
    }

    /// Footer text for the metronome and a configured timecode mode.
    pub fn chrome_status(&self) -> String {
        let mut parts = Vec::new();
        if self.metronome_on {
            parts.push(format!(
                "Metronome {} BPM {}/{}",
                self.preferences.metronome_bpm(),
                self.metronome_beat + 1,
                self.preferences.metronome_beats()
            ));
        }
        if let Some(kind) = self.preferences.timecode_kind() {
            let clock = freeshow_timecode::latest_clock()
                .unwrap_or_else(|| format!("{:.0} fps", self.preferences.timecode_framerate()));
            parts.push(format!("Timecode {kind} {clock}"));
        }
        if let Some(name) = &self.player_now {
            parts.push(format!("Player {name}"));
        }
        if !self.active_effects.is_empty() {
            parts.push(format!("Effect {}", self.active_effects.join(", ")));
        }
        let audio_fx = self.preferences.audio_effect_names();
        if !audio_fx.is_empty() {
            parts.push(format!("Audio FX {}", audio_fx.join(", ")));
        }
        if self.timeline_playing {
            parts.push("Timeline".to_string());
        }
        let delay = self.preferences.output_delay_ms();
        if delay > 0 {
            parts.push(format!("Delay {delay} ms"));
        }
        if let Some(alert) = &self.chrome.update_alert {
            parts.push(alert.clone());
        }
        let ticker = self.chrome.ticker();
        if !ticker.is_empty() {
            parts.push(ticker);
        }
        parts.join("  ·  ")
    }

    fn drawer_names(&self) -> Vec<String> {
        match self.tab.as_str() {
            "shows" => self.shows.iter().map(ShowFile::name).collect(),
            "media" => self.media.iter().map(MediaEntry::label).collect(),
            "player" => self
                .player_videos
                .iter()
                .map(|video| match video.kind.as_str() {
                    "youtube" => format!("{} (YouTube)", video.name),
                    "vimeo" => format!("{} (Vimeo)", video.name),
                    _ => video.name.clone(),
                })
                .collect(),
            "audio" => self.audio.clone(),
            "overlays" => self.overlays.clone(),
            "templates" => self.templates.clone(),
            "scripture" => self.bibles.iter().map(BibleFile::name).collect(),
            "calendar" => self.events.clone(),
            "rss" => self
                .chrome
                .rss_items
                .iter()
                .map(|item| {
                    // The site name is enough to tell feeds apart; a full link only crowds the row.
                    let site = item
                        .link
                        .split("://")
                        .nth(1)
                        .and_then(|rest| rest.split('/').next())
                        .map(|host| host.trim_start_matches("www."))
                        .filter(|host| !host.is_empty());
                    match site {
                        Some(site) => format!("{} · {site}", item.title),
                        None => item.title.clone(),
                    }
                })
                .collect(),
            "functions" => vec![
                "Clear".to_string(),
                "Next".to_string(),
                "Previous".to_string(),
                // The countdown on stage displays: what a click does, and how much is left.
                if self.timer_deadline.is_some() {
                    format!("Pause timer ({} left)", self.timer_label())
                } else if self.timer_left == 0 || self.timer_left == 300 {
                    "Start 5-minute timer".to_string()
                } else {
                    format!("Resume timer ({} left)", self.timer_label())
                },
                if self.recording {
                    "Stop".to_string()
                } else {
                    "Record".to_string()
                },
                "Planning Center".to_string(),
                "ChurchApps".to_string(),
                "Backup".to_string(),
                "Restore".to_string(),
                "OBS scene".to_string(),
                "OBS stream".to_string(),
                "OBS record".to_string(),
                "AI".to_string(),
                match self.icecast.as_ref() {
                    Some((_, true)) => "Disable Icecast",
                    Some((_, false)) => "Enable Icecast",
                    None => "Icecast (not routed)",
                }
                .to_string(),
                "Export text".to_string(),
                "Export PDF".to_string(),
                "Export package".to_string(),
                "Screenshot".to_string(),
                if self.metronome_on {
                    "Stop metronome".to_string()
                } else {
                    "Metronome".to_string()
                },
                "Undo".to_string(),
                "Redo".to_string(),
                if self.focus_mode {
                    "Exit focus mode".to_string()
                } else {
                    "Focus mode".to_string()
                },
                "Guide".to_string(),
                if self.kiosk {
                    "Exit kiosk".to_string()
                } else {
                    "Kiosk".to_string()
                },
                if self.always_on_top {
                    "Windowed top".to_string()
                } else {
                    "Always on top".to_string()
                },
                "Shortcuts".to_string(),
                "Save layout".to_string(),
                "Window layouts".to_string(),
                if self.preferences.watermark_enabled() {
                    "Hide watermark".to_string()
                } else {
                    "Watermark".to_string()
                },
                "Variables".to_string(),
                "Clear drawing".to_string(),
                if self.active_interaction.is_some() {
                    "End interaction".to_string()
                } else {
                    "Ask".to_string()
                },
                if self.timeline_playing {
                    "Stop timeline".to_string()
                } else {
                    "Timeline".to_string()
                },
                if self.active_effects.is_empty() {
                    "Effects".to_string()
                } else {
                    "Clear effects".to_string()
                },
            ],
            _ => Vec::new(),
        }
    }

    fn drawer_entries(&self) -> Vec<(usize, String)> {
        if self.tab == "templates" {
            return self
                .visible_template_entries()
                .map(|(index, (_, name, _))| (index, name.to_owned()))
                .collect();
        }
        let query = self.search_query.trim().to_ascii_lowercase();
        self.drawer_names()
            .into_iter()
            .enumerate()
            .filter(|(_, name)| query.is_empty() || name.to_ascii_lowercase().contains(&query))
            .collect()
    }

    pub fn drawer(&self) -> Vec<String> {
        self.drawer_entries()
            .into_iter()
            .map(|(_, name)| name)
            .collect()
    }

    pub fn visible_media_entries(&self) -> impl Iterator<Item = &MediaEntry> {
        self.drawer_entries()
            .into_iter()
            .filter_map(|(index, _)| self.media.get(index))
    }

    pub fn media_preview_entry(&self, path: &str) -> Option<&MediaEntry> {
        if self.tab != "media" {
            return None;
        }
        self.visible_media_entries().find(|entry| {
            entry.path == Path::new(path)
                && matches!(entry.kind, MediaEntryKind::Image | MediaEntryKind::Video)
                && entry.path.is_file()
        })
    }

    /// Which visible drawer rows are on air: showing overlays and the playing audio file.
    pub fn drawer_on(&self) -> Vec<bool> {
        self.drawer_entries()
            .into_iter()
            .map(|(index, name)| match self.tab.as_str() {
                "overlays" => self
                    .overlay_catalog
                    .id_named(&name)
                    .is_some_and(|id| self.active_overlays.contains(&id)),
                "audio" => {
                    self.playing_audio.is_some()
                        && self
                            .audio
                            .get(index)
                            .and_then(|file| self.file_in(AUDIO_DIR, file))
                            == self.playing_audio
                }
                _ => false,
            })
            .collect()
    }

    /// Visible row for a real drawer index, after the current search filter.
    pub fn visible_drawer_index(&self, real: Option<usize>) -> Option<usize> {
        let real = real?;
        self.drawer_entries()
            .iter()
            .position(|(index, _)| *index == real)
    }

    #[cfg(test)]
    pub fn slide_labels(&self) -> Vec<String> {
        self.slide_rows().into_iter().map(|row| row.text).collect()
    }

    pub fn slide_rows(&self) -> Vec<SlideLabel> {
        if self.tab == "scripture" {
            return self
                .verse_hits
                .iter()
                .map(|hit| SlideLabel {
                    text: format!("{}  {}", hit.reference, hit.text),
                    color: None,
                })
                .collect();
        }
        // The open show stays in view while the operator browses media, audio and the rest.
        self.current_show()
            .map(|show| slide_rows_for(&show.ordered_slides(), self.preferences.group_numbers()))
            .unwrap_or_default()
    }

    pub fn slides_list_columns(&self) -> usize {
        if self.tab == "scripture" {
            return 1;
        }
        match self.preferences.slides_mode() {
            "list" | "lyrics" => 1,
            _ => self.preferences.slides_columns().max(1) as usize,
        }
    }

    pub fn project_title(&self) -> String {
        self.projects
            .projects()
            .iter()
            .find(|project| Some(&project.id) == self.project_id.as_ref())
            .map(|project| project.name.clone())
            .unwrap_or_else(|| "Project".to_string())
    }

    pub fn project_labels(&self) -> Vec<String> {
        let Some(project) = self.current_project() else {
            return Vec::new();
        };
        project
            .shows
            .iter()
            .map(|item| {
                self.shows
                    .iter()
                    .find(|show| show.id == item.id)
                    .map(ShowFile::name)
                    .filter(|name| !name.is_empty())
                    .or_else(|| {
                        if item.label.is_empty() {
                            None
                        } else {
                            Some(item.label.clone())
                        }
                    })
                    .unwrap_or_else(|| item.id.clone())
            })
            .collect()
    }

    /// The tag each project row shows: empty for a song, `Section` for a header row, else the
    /// media kind ("Video", "Image" and so on).
    pub fn project_kinds(&self) -> Vec<String> {
        let Some(project) = self.current_project() else {
            return Vec::new();
        };
        project
            .shows
            .iter()
            .map(|item| match item.kind.as_str() {
                "show" | "" => String::new(),
                "section" => "Section".to_string(),
                "pdf" => "PDF".to_string(),
                "ppt" => "PowerPoint".to_string(),
                other => {
                    let mut chars = other.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                }
            })
            .collect()
    }

    /// The project row whose song is on the output now: the picked row when it is that song,
    /// else its first row.
    pub fn live_project_item(&self) -> Option<usize> {
        if self.blanked || self.slide_hidden {
            return None;
        }
        let live_show = match self.program_content.borrow().as_ref() {
            Some(crate::live::ProgramContent::Slide { show_id, .. }) => show_id.clone(),
            _ => return None,
        };
        let project = self.current_project()?;
        let is_live = |item: &freeshow_data::ProjectShowRef| {
            matches!(item.kind.as_str(), "show" | "") && item.id == live_show
        };
        if project.shows.get(self.project_item).is_some_and(is_live) {
            return Some(self.project_item);
        }
        project.shows.iter().position(is_live)
    }

    pub fn group(&self) -> String {
        if self.tab != "shows" {
            return String::new();
        }
        let Some(show) = self.current_show() else {
            return String::new();
        };
        self.slide_index
            .and_then(|index| show.ordered_slides().get(index).cloned())
            .and_then(|slide| slide.group)
            .unwrap_or_default()
    }

    pub fn notes(&self) -> String {
        if self.tab != "shows" {
            return String::new();
        }
        let Some(show) = self.current_show() else {
            return String::new();
        };
        self.slide_index
            .and_then(|index| show.ordered_slides().get(index).cloned())
            .map(|slide| slide.notes)
            .unwrap_or_default()
    }

    pub fn lyrics(&self) -> String {
        if self.tab == "scripture" {
            return self
                .verse_index
                .and_then(|index| self.verse_hits.get(index))
                .map(|hit| format!("{}\n{}", hit.reference, hit.text))
                .unwrap_or_default();
        }
        let Some(show) = self.current_show() else {
            return String::new();
        };
        let Some(slide) = self
            .slide_index
            .and_then(|index| show.ordered_slides().get(index).cloned())
        else {
            return String::new();
        };
        show.item_text(&slide.id, self.current_item(show))
            .unwrap_or_else(|| show.plain_text(&slide.id))
    }

    /// Real drawer index for a visible (search-filtered) row.
    pub fn drawer_real_index(&self, visible: usize) -> usize {
        self.drawer_entries()
            .get(visible)
            .map(|(real, _)| *real)
            .unwrap_or(visible)
    }

    pub fn select_drawer(&mut self, index: usize) -> DrawerAction {
        if self.tab == "templates" {
            return self.choose_template(index);
        }
        let index = self.drawer_real_index(index);
        match self.tab.as_str() {
            "shows" => {
                if index < self.shows.len() {
                    self.hold_program();
                    let changed = self.show_index != Some(index);
                    self.show_index = Some(index);
                    // Opening a show picks no slide; the first Next or a click presents one.
                    if changed {
                        self.slide_index = None;
                        self.layer_selection = None;
                    }
                    self.project_picked = false;
                    if changed {
                        self.load_timeline();
                    }
                }
                DrawerAction::None
            }
            "scripture" => {
                if index < self.bibles.len() {
                    self.hold_program();
                    self.bible_index = Some(index);
                    self.verse_hits.clear();
                    self.verse_index = None;
                }
                DrawerAction::None
            }
            "media" => self.choose_media(index),
            "player" => {
                self.drawer_pick = Some(index);
                self.choose_player(index)
            }
            "calendar" => {
                self.drawer_pick = (index < self.event_ids.len()).then_some(index);
                DrawerAction::None
            }
            "audio" => self.choose_audio(index),
            "overlays" => self.toggle_overlay(index),
            _ => DrawerAction::None,
        }
    }

    pub fn replace_underlay(&mut self, raster: Option<Raster>) {
        self.underlay = raster;
        self.underlay_from = None;
        self.network_background = false;
    }

    pub fn begin_network_background(&mut self) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        if self.imported_foreground_active {
            self.slide_hidden = false;
        }
        self.reset_background_look();
        self.underlay_from = None;
        self.network_background = true;
        self.blanked = false;
        self.video_time = 0.0;
        self.video_duration = 0.0;
        self.discard_queued_video();
        true
    }

    pub fn update_network_background(&mut self, raster: Raster) -> bool {
        if !self.network_background || self.blanked {
            return false;
        }
        self.underlay = Some(raster);
        true
    }

    fn set_still_underlay(&mut self, path: PathBuf, raster: Option<Raster>) {
        self.replace_underlay(raster);
        self.underlay_from = self.underlay.is_some().then_some(path);
    }

    pub fn clear_output(&mut self) {
        if self.preferences.out_locked() {
            return;
        }
        self.program_content.get_mut().take();
        self.layer_builds = None;
        self.slide_countdown = None;
        self.layer_build_stepped = false;
        self.pending_cue_completion = None;
        self.pending_cue_actions = None;
        self.replace_underlay(None);
        self.reset_background_look();
        self.playing_audio = None;
        self.audio_transport = None;
        self.transition_sound_playing = false;
        self.audio_paused = false;
        self.video_time = 0.0;
        self.video_duration = 0.0;
        self.audio_time = 0.0;
        self.audio_duration = 0.0;
    }

    pub fn clear_all_layers(&mut self) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        self.clear_output();
        self.blanked = true;
        self.slide_hidden = true;
        self.background_transition = false;
        self.timed_seconds = None;
        self.pending_actions = None;
        self.active_overlays.clear();
        self.restore_locked_overlays();
        self.clear_effects();
        self.playlist_id = None;
        self.playlist_songs.clear();
        self.effect_playing = None;
        self.effect_return = None;
        if self.metronome_on {
            self.toggle_metronome();
        }
        self.pending_play = Some(PendingPlay::StopVideo);
        self.pending_also = Some(PendingPlay::StopAudio);
        true
    }

    fn output_id(&self) -> String {
        self.audience().0
    }

    fn locked_overlay_ids(&self) -> Vec<String> {
        self.preferences.locked_overlay_ids(&self.output_id())
    }

    fn overlay_is_locked(&self, id: &str) -> bool {
        self.preferences.overlay_is_locked(&self.output_id(), id)
    }

    fn restore_locked_overlays(&mut self) {
        for id in self.locked_overlay_ids() {
            if !self.active_overlays.iter().any(|active| active == &id) {
                self.active_overlays.push(id);
            }
        }
    }

    fn watermark_overlay_id(&self) -> Option<String> {
        if self.overlay_catalog.get("watermark").is_some() {
            return Some("watermark".to_string());
        }
        self.overlay_catalog
            .id_named("Watermark")
            .or_else(|| self.overlay_catalog.id_named("watermark"))
    }

    fn restore_watermark(&mut self) {
        let Some(id) = self.watermark_overlay_id() else {
            return;
        };
        if !self.preferences.watermark_enabled() {
            self.active_overlays.retain(|active| active != &id);
            return;
        }
        if !self.active_overlays.iter().any(|active| active == &id) {
            self.active_overlays.push(id);
        }
    }

    /// Turns the catalog watermark overlay on or off and stores the choice.
    pub fn toggle_watermark(&mut self) -> bool {
        let enabled = !self.preferences.watermark_enabled();
        self.patch_special("watermark", serde_json::Value::Bool(enabled));
        self.restore_watermark();
        true
    }

    fn resolve_overlay_id(&self, id_or_name: &str) -> Option<String> {
        if self.overlay_catalog.get(id_or_name).is_some() {
            return Some(id_or_name.to_string());
        }
        self.overlay_catalog.id_named(id_or_name)
    }

    /// Applies a named or id-addressed scene: overlays, hidden overlays, and media.
    pub fn start_scene(&mut self, id_or_name: &str) -> bool {
        let Some(scene) = self.preferences.scene(id_or_name) else {
            return false;
        };
        for hidden in &scene.hidden_overlays {
            if let Some(id) = self.resolve_overlay_id(hidden) {
                self.active_overlays.retain(|active| active != &id);
            }
        }
        for overlay in &scene.overlays {
            if let Some(id) = self.resolve_overlay_id(overlay)
                && !self.active_overlays.iter().any(|active| active == &id)
            {
                self.active_overlays.push(id);
            }
        }
        self.restore_locked_overlays();
        self.restore_watermark();
        if let Some(media) = &scene.media {
            self.play_scene_media(media);
        }
        self.active_scene = Some(scene.id);
        true
    }

    fn apply_active_scenes(&mut self) {
        let output_id = self.output_id();
        let ids = self.preferences.active_scene_ids(&output_id);
        for id in ids {
            let _ = self.start_scene(&id);
        }
    }

    fn play_scene_media(&mut self, media: &str) {
        let direct = PathBuf::from(media);
        let path = if direct.is_file() {
            Some(direct)
        } else {
            self.media.iter().find_map(|entry| {
                let name = entry
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                (entry.name == media
                    || entry.label() == media
                    || name == media
                    || entry.path.ends_with(media))
                .then(|| entry.path.clone())
            })
        };
        let Some(path) = path else {
            return;
        };
        let kind = media_kind(&path);
        if matches!(kind, MediaKind::Image | MediaKind::Video) {
            self.video_end = VideoEnd::Hold;
            self.video_end_fade = None;
        }
        match kind {
            MediaKind::Image => {
                let raster = self.load_still(&path);
                self.set_still_underlay(path, raster);
                self.background_transition = true;
            }
            MediaKind::Video => {
                self.remember_video_playback(&path);
                self.background_path = Some(path.clone());
                self.background_transition = true;
                self.pending_play = Some(PendingPlay::Video(path));
            }
            MediaKind::Audio => {
                self.pending_play = Some(PendingPlay::Audio(path));
            }
            MediaKind::Other => {}
        }
    }

    /// Starts a stored interaction: runs its action and/or shows its questions.
    pub fn start_interaction(&mut self, id_or_name: &str) -> bool {
        let Some(spec) = self.preferences.interaction(id_or_name) else {
            return false;
        };
        if let Some(action) = &spec.action {
            self.run_interaction_action(action);
        }
        if spec.questions.is_empty() {
            return true;
        }
        self.active_interaction = Some(spec.id);
        true
    }

    pub fn stop_interaction(&mut self) {
        self.active_interaction = None;
    }

    /// Starts the first stored interaction, or stops the one that is already on.
    pub fn toggle_first_interaction(&mut self) -> bool {
        if self.active_interaction.is_some() {
            self.stop_interaction();
            return true;
        }
        let Some(spec) = self.preferences.interactions().into_iter().next() else {
            return false;
        };
        self.start_interaction(&spec.id)
    }

    fn run_interaction_action(&mut self, action: &str) {
        match action.trim().to_ascii_lowercase().as_str() {
            "next" | "next_slide" => {
                let _ = self.advance_show();
            }
            "previous" | "previous_slide" => {
                let _ = self.retreat_show();
            }
            "clear" => self.clear_output(),
            other if other.starts_with("overlay:") => {
                if let Some(id) = self.resolve_overlay_id(other.trim_start_matches("overlay:"))
                    && !self.active_overlays.iter().any(|active| active == &id)
                {
                    self.active_overlays.push(id);
                }
            }
            _ => {}
        }
    }

    pub fn interaction_lines(&self) -> Vec<String> {
        let Some(id) = &self.active_interaction else {
            return Vec::new();
        };
        self.preferences
            .interaction(id)
            .map(|spec| {
                let mut lines = Vec::new();
                if !spec.name.is_empty() {
                    lines.push(spec.name);
                }
                lines.extend(spec.questions);
                lines
            })
            .unwrap_or_default()
    }

    pub fn variable_text(&self) -> String {
        let Some(root) = &self.root else {
            return String::new();
        };
        Library::open(root).variable_editor_text()
    }

    pub fn save_variables(&mut self, text: &str) -> bool {
        let Some(root) = &self.root else {
            return false;
        };
        Library::open(root).save_variables_text(text)
    }

    pub fn clear_drawing(&mut self) {
        self.draw_strokes.clear();
    }

    #[cfg(test)]
    pub fn add_draw_point(&mut self, x: f32, y: f32, size: f32, color: &str) {
        self.draw_strokes.push(DrawStroke {
            x,
            y,
            size,
            color: color.to_string(),
            up: false,
        });
    }

    fn load_player_videos(&mut self) {
        let Some(root) = &self.root else {
            self.player_videos.clear();
            return;
        };
        self.player_videos = Library::open(root).player_videos();
    }

    fn choose_player(&mut self, index: usize) -> DrawerAction {
        let Some(video) = self.player_videos.get(index).cloned() else {
            return DrawerAction::None;
        };
        self.player_now = Some(video.name.clone());
        let Some(link) = video.path.as_deref() else {
            self.notice = format!("{} has no link to play", video.name);
            return DrawerAction::None;
        };
        let web = link.starts_with("http://") || link.starts_with("https://");
        if web && freeshow_media::is_stream_page(link) {
            if !freeshow_media::ytdlp_available() {
                self.notice = "Install yt-dlp to play YouTube and Vimeo links".to_string();
                return DrawerAction::None;
            }
            // yt-dlp takes seconds, so the operator window keeps running meanwhile.
            let (sender, receiver) = std::sync::mpsc::channel();
            let (name, page) = (video.name.clone(), link.to_string());
            std::thread::spawn(move || {
                let _ = sender.send((name, freeshow_media::resolve_stream_page(&page)));
            });
            self.stream_lookup = Some(receiver);
            self.notice = format!("Loading {}…", video.name);
            return DrawerAction::None;
        }
        if !web && !Path::new(link).is_file() {
            self.notice = format!("{} is missing: no file at {link}", video.name);
            return DrawerAction::None;
        }
        if !self.play_path(PathBuf::from(link)) {
            self.notice = format!("{} is not a video this app can play", video.name);
            return DrawerAction::None;
        }
        // Start it now, the same way as clicking a video in the Media tab.
        match self.pending_play.take() {
            Some(PendingPlay::Video(path)) => DrawerAction::PlayVideo(path),
            Some(PendingPlay::Audio(path)) => DrawerAction::PlayAudio(path),
            Some(other) => {
                self.pending_play = Some(other);
                DrawerAction::None
            }
            None => DrawerAction::Present,
        }
    }

    /// Plays a looked-up YouTube or Vimeo video once yt-dlp answers: `Some(true)` when the
    /// output should start it, `Some(false)` when only the notice changed, `None` while waiting.
    pub fn poll_stream_lookup(&mut self) -> Option<bool> {
        let receiver = self.stream_lookup.as_ref()?;
        let (name, url) = match receiver.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.stream_lookup = None;
                return Some(false);
            }
        };
        self.stream_lookup = None;
        // A different video picked meanwhile wins.
        if self.player_now.as_deref() != Some(name.as_str()) {
            return Some(false);
        }
        let Some(url) = url else {
            self.notice = format!("Could not load {name}: check the link, the network and yt-dlp");
            return Some(false);
        };
        self.notice.clear();
        self.play_video(PathBuf::from(url));
        self.refresh_output = true;
        Some(true)
    }

    pub fn start_effect(&mut self, id_or_name: &str) -> bool {
        let Some(effect) = self.preferences.effect(id_or_name) else {
            return false;
        };
        if !self.active_effects.iter().any(|id| id == &effect.id) {
            self.active_effects.push(effect.id);
        }
        true
    }

    pub fn clear_effects(&mut self) {
        self.active_effects.clear();
    }

    pub fn toggle_effects(&mut self) -> bool {
        if !self.active_effects.is_empty() {
            self.clear_effects();
            return true;
        }
        let Some(effect) = self.preferences.effects_library().into_iter().next() else {
            return false;
        };
        self.start_effect(&effect.id)
    }

    pub fn live_effects(&self) -> Vec<EffectSpec> {
        self.active_effects
            .iter()
            .filter_map(|id| self.preferences.effect(id))
            .collect()
    }

    fn load_timeline(&mut self) {
        self.stop_timeline();
        self.timeline_cues = self
            .current_show()
            .map(|show| timeline_cues_from_show(&show.show))
            .unwrap_or_default();
        let (looping, end, soundtrack) = self
            .current_show()
            .and_then(|show| {
                let (timeline, _) = timeline_for_show(&show.show)?;
                let end = timeline["duration"]
                    .as_f64()
                    .filter(|duration| duration.is_finite() && *duration > 0.0)
                    .map(|duration| (duration * 1000.0).round() as u64);
                let soundtrack = timeline["soundtrack"]
                    .as_str()
                    .and_then(|id| show.show["media"].get(id))
                    .and_then(|media| {
                        let path = PathBuf::from(media["path"].as_str()?);
                        (media_kind(&path) == MediaKind::Audio && path.is_file()).then(|| {
                            TimelineSoundtrack {
                                path,
                                transport: MediaTransport::from_value(&media["audioTransport"]),
                                delay_ms: timeline["soundtrackDelayMs"]
                                    .as_u64()
                                    .unwrap_or_default(),
                                restart: true,
                                owned: false,
                            }
                        })
                    });
                Some((timeline["loop"] == true, end, soundtrack))
            })
            .unwrap_or_default();
        self.timeline_loop = looping;
        let last_cue = self.timeline_cues.last().map_or(0, |cue| cue.time_ms);
        self.timeline_end_ms = end.map(|end| end.max(last_cue).max(1)).or_else(|| {
            soundtrack
                .is_none()
                .then_some(last_cue.max(u64::from(looping)))
        });
        self.timeline_soundtrack = soundtrack;
    }

    pub fn timeline_text(&self) -> String {
        timeline_text_from(&self.timeline_cues)
    }

    pub fn save_timeline(&mut self, text: &str) -> bool {
        let mut cues = parse_timeline_text(text);
        let Some(index) = self.show_index else {
            self.stop_timeline();
            self.timeline_loop = false;
            self.timeline_end_ms = None;
            self.timeline_soundtrack = None;
            self.timeline_cues = cues;
            return !self.timeline_cues.is_empty() || text.trim().is_empty();
        };
        let Some(snapshot) = self.edit_snapshot("Edit timeline") else {
            return false;
        };
        let Some(show) = self.shows.get_mut(index) else {
            return false;
        };
        let mut original = timeline_cues_from_show(&show.show).into_iter().peekable();
        for cue in &mut cues {
            while original
                .peek()
                .is_some_and(|saved| saved.time_ms < cue.time_ms)
            {
                original.next();
            }
            if original.peek().is_some_and(|saved| {
                saved.time_ms == cue.time_ms && saved.kind == cue.kind && saved.data == cue.data
            }) {
                let saved = original.next().unwrap();
                cue.name = saved.name;
                cue.slide_index = saved.slide_index;
            }
        }
        let actions: Vec<serde_json::Value> = cues
            .iter()
            .map(|cue| {
                let mut data = serde_json::json!({});
                if let Some(value) = &cue.data {
                    if let Ok(index) = value.parse::<u64>() {
                        data["index"] = serde_json::json!(index);
                    } else {
                        data["id"] = serde_json::json!(value);
                    }
                }
                if let Some(index) = cue.slide_index {
                    data["index"] = serde_json::json!(index);
                }
                serde_json::json!({
                    "id": cue.name,
                    "time": cue.time_ms,
                    "name": cue.name,
                    "type": cue.kind,
                    "data": data
                })
            })
            .collect();
        let (mut timeline, layout) = timeline_for_show(&show.show)
            .filter(|(timeline, _)| timeline.is_object())
            .map(|(timeline, layout)| (timeline.clone(), layout.map(str::to_owned)))
            .unwrap_or_else(|| (serde_json::json!({ "loop": false }), None));
        timeline["actions"] = serde_json::json!(actions);
        if let Some(layout) = layout {
            show.show["layouts"][layout]["timeline"] = timeline;
        } else {
            show.show["timeline"] = timeline;
        }
        if let Err(error) = self.persist_show_edit(index, snapshot) {
            self.notice = error;
            return false;
        }
        self.load_timeline();
        true
    }

    pub fn start_timeline(&mut self) -> bool {
        if self.current_show().is_some() {
            self.load_timeline();
        }
        if self.timeline_cues.is_empty() && self.timeline_soundtrack.is_none() {
            return false;
        }
        self.timeline_playing = true;
        self.timeline_started = Some(std::time::Instant::now());
        self.timeline_fired = 0;
        true
    }

    pub fn stop_timeline(&mut self) -> bool {
        self.timeline_playing = false;
        self.timeline_started = None;
        self.timeline_fired = 0;
        self.stop_timeline_soundtrack()
    }

    fn stop_timeline_soundtrack(&mut self) -> bool {
        let stop_audio = self.timeline_soundtrack.as_mut().is_some_and(|track| {
            std::mem::take(&mut track.owned) && self.playing_audio.as_ref() == Some(&track.path)
        });
        if stop_audio {
            self.stop_audio();
            self.refresh_output = true;
        }
        stop_audio
    }

    /// Fires due timeline cues. Returns true when the output should refresh.
    pub fn tick_timeline(&mut self, now: std::time::Instant) -> bool {
        if !self.timeline_playing {
            return false;
        }
        let Some(started) = self.timeline_started else {
            return false;
        };
        let elapsed = now.saturating_duration_since(started).as_millis() as u64;
        let mut changed = false;
        if let Some(track) = &mut self.timeline_soundtrack
            && elapsed >= track.delay_ms
            && self
                .timeline_end_ms
                .is_none_or(|end| elapsed < end || track.delay_ms == 0)
            && std::mem::take(&mut track.restart)
        {
            let path = track.path.clone();
            let transport = track.transport;
            self.queue_audio(path);
            self.audio_transport = transport;
            if let Some(track) = &mut self.timeline_soundtrack {
                track.owned = true;
            }
            changed = true;
        }
        while self.timeline_playing && self.timeline_fired < self.timeline_cues.len() {
            let cue = self.timeline_cues[self.timeline_fired].clone();
            if cue.time_ms > elapsed {
                break;
            }
            self.timeline_fired += 1;
            changed |= self.run_timeline_cue(&cue, self.show_index);
        }
        if self.timeline_playing
            && self.timeline_fired >= self.timeline_cues.len()
            && self
                .timeline_end_ms
                .map_or_else(|| !self.timeline_soundtrack_active(), |end| elapsed >= end)
        {
            if self.timeline_loop {
                let period = self.timeline_end_ms.unwrap_or(elapsed).max(1);
                // Keep the original phase and skip entirely missed repetitions.
                self.timeline_started = Some(now - Duration::from_millis(elapsed % period));
                self.timeline_fired = 0;
                if self
                    .timeline_soundtrack
                    .as_ref()
                    .is_some_and(|track| track.delay_ms > 0)
                {
                    changed |= self.stop_timeline_soundtrack();
                }
                if let Some(track) = &mut self.timeline_soundtrack {
                    track.restart = true;
                }
            } else {
                changed |= self.stop_timeline();
            }
        }
        changed
    }

    fn timeline_soundtrack_active(&self) -> bool {
        self.timeline_soundtrack.as_ref().is_some_and(|track| {
            track.restart
                || (self.timeline_owns_audio(&track.path)
                    && !self
                        .media_finished
                        .as_ref()
                        .is_some_and(|(path, audio)| *audio && Path::new(path) == track.path))
        })
    }

    fn timeline_owns_audio(&self, path: &Path) -> bool {
        self.timeline_soundtrack.as_ref().is_some_and(|track| {
            track.owned && track.path == path && self.playing_audio.as_deref() == Some(path)
        })
    }

    pub fn timeline_audio_failed(&mut self, path: &Path) {
        if self.timeline_owns_audio(path) {
            self.playing_audio = None;
            self.audio_transport = None;
            self.audio_paused = false;
            // Let the slides finish even if the audio file could not be opened.
            self.timeline_soundtrack = None;
        }
    }

    fn run_timeline_cue(&mut self, cue: &TimelineCue, source: Option<usize>) -> bool {
        match cue.kind.to_ascii_lowercase().as_str() {
            "next" | "next_slide" | "go" => self.advance_show(),
            "previous" | "previous_slide" => self.retreat_show(),
            "slide" => self.run_timeline_slide(cue),
            "audio" if !self.preferences.out_locked() => cue
                .data
                .as_deref()
                .is_some_and(|id| self.run_timeline_audio(source, id)),
            "background" | "foreground" if !self.preferences.out_locked() => {
                cue.data.as_deref().is_some_and(|id| {
                    self.run_timeline_picture(
                        source,
                        id,
                        cue.kind.eq_ignore_ascii_case("foreground"),
                    )
                })
            }
            "overlay" => {
                if let Some(id) = cue
                    .data
                    .as_deref()
                    .and_then(|name| self.resolve_overlay_id(name))
                    && !self.active_overlays.iter().any(|active| active == &id)
                {
                    self.active_overlays.push(id);
                    return true;
                }
                false
            }
            "scene" => cue.data.as_deref().is_some_and(|id| self.start_scene(id)),
            "effect" => cue.data.as_deref().is_some_and(|id| self.start_effect(id)),
            "clear_background" if !self.preferences.out_locked() => {
                let slide_hidden = self.slide_hidden;
                self.clear_background();
                self.slide_hidden = slide_hidden;
                true
            }
            "clear_audio" if !self.preferences.out_locked() => {
                self.stop_audio();
                true
            }
            "clear_slide" => self.clear_slide_layer(),
            "clear" | "clear_all" => self.clear_all_layers(),
            _ => false,
        }
    }

    fn run_timeline_audio(&mut self, source: Option<usize>, id: &str) -> bool {
        let Some(media) = source
            .and_then(|index| self.shows.get(index))
            .and_then(|show| show.show["media"].get(id))
        else {
            return false;
        };
        let Some(path) = media["path"]
            .as_str()
            .map(PathBuf::from)
            .filter(|path| media_kind(path) == MediaKind::Audio && path.is_file())
        else {
            return false;
        };
        let looping = media["loop"].as_bool();
        let retrigger = media["retrigger"] == true;
        let transport = MediaTransport::from_value(&media["audioTransport"]);
        self.queue_cue_audio(path, looping, retrigger, transport)
    }

    fn run_timeline_picture(&mut self, source: Option<usize>, id: &str, foreground: bool) -> bool {
        let Some(media) = source
            .and_then(|index| self.shows.get(index))
            .and_then(|show| show.show["media"].get(id))
        else {
            return false;
        };
        let Some(path) = media["path"]
            .as_str()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
        else {
            return false;
        };
        let retrigger = media["retrigger"] == true;
        let hide_slide = foreground || self.slide_hidden;
        let shown = match media_kind(&path) {
            MediaKind::Image => self.show_background_still(&path, hide_slide, retrigger),
            MediaKind::Video if path.is_file() => {
                let end = VideoEnd::from_media(media);
                let transport = MediaTransport::from_value(&media["videoTransport"]);
                let loops = media["loop"] != false;
                let muted = media["muted"] != false;
                let ended = self
                    .media_finished
                    .as_ref()
                    .is_some_and(|(file, audio)| !audio && Path::new(file) == path);
                if retrigger
                    || ended
                    || self.background_path.as_ref() != Some(&path)
                    || self.media_loop != loops
                    || self.media_muted != muted
                    || self.video_transport != transport
                {
                    self.queue_background_video(&path, Some((loops, muted)), hide_slide, transport);
                } else {
                    self.slide_hidden = hide_slide;
                }
                self.video_end = end;
                true
            }
            _ => false,
        };
        if shown {
            self.imported_foreground_active = foreground;
        }
        shown
    }

    fn run_timeline_slide(&mut self, cue: &TimelineCue) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        let Some(show) = self.current_show() else {
            return false;
        };
        let (output_id, output_name) = self.audience();
        let eligible = |index| show.can_output(index, &output_id, &output_name);
        let numbered = cue.slide_index.filter(|&index| eligible(index));
        let next = match cue.data.as_deref() {
            Some(id) if show.show["slides"].get(id).is_some() => numbered
                .filter(|&index| {
                    show.ordered_slides()
                        .get(index)
                        .is_some_and(|slide| slide.id == id)
                })
                .or_else(|| show.output_index_for_slide(id, &output_id, &output_name)),
            Some(index) => index.parse::<usize>().ok().filter(|&index| eligible(index)),
            None => numbered,
        };
        let Some(next) = next else {
            return false;
        };
        self.tab = "shows".to_string();
        self.select_slide(next);
        true
    }

    /// Selects the first show and first slide and applies that slide.
    pub fn open_first_show(&mut self) -> bool {
        let Some(id) = self.shows.first().map(|show| show.id.clone()) else {
            return false;
        };
        self.open_show_at(&id, 0)
    }

    /// Selects a show by id and presents its first slide.
    pub fn present_show(&mut self, id: &str) -> bool {
        self.open_show_at(id, 0)
    }

    /// Writes the open show as plain text under the library `Exports/` folder.
    pub fn export_current_show_text(&self) -> Result<PathBuf, String> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| "no library is open".to_string())?;
        let show = self
            .current_show()
            .ok_or_else(|| "no show is open".to_string())?;
        let stem = export_stem(&show.name());
        let path = root
            .join(freeshow_data::EXPORTS_DIR)
            .join(format!("{stem}.txt"));
        show.write_plain_text(&path)
            .map_err(|err| err.to_string())?;
        Ok(path)
    }

    fn exports_dir(&self) -> Result<PathBuf, String> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| "no library is open".to_string())?;
        let path = root.join(freeshow_data::EXPORTS_DIR);
        std::fs::create_dir_all(&path).map_err(|err| err.to_string())?;
        Ok(path)
    }

    /// Writes the open show as a lyrics PDF under the library `Exports/` folder.
    pub fn export_current_show_pdf(&self) -> Result<PathBuf, String> {
        let show = self
            .current_show()
            .ok_or_else(|| "no show is open".to_string())?;
        let path = self
            .exports_dir()?
            .join(format!("{}.pdf", export_stem(&show.name())));
        freeshow_data::write_show_pdf(show, &path).map_err(|err| err.to_string())?;
        Ok(path)
    }

    /// Writes the open project and its shows as a `.project` zip under `Exports/`.
    pub fn export_current_project_package(&self) -> Result<(PathBuf, Vec<String>), String> {
        let project_id = self
            .project_id
            .as_deref()
            .ok_or_else(|| "no project is open".to_string())?;
        let project = self
            .projects
            .projects()
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| "no project is open".to_string())?;
        let body = self
            .projects
            .project_body(project_id)
            .ok_or_else(|| "no project is open".to_string())?;
        let path = self
            .exports_dir()?
            .join(format!("{}.project", export_stem(&project.name)));
        let root = self
            .root
            .as_deref()
            .ok_or("Choose a library folder first")?;
        let warnings = freeshow_data::write_portable_project(
            &project,
            &body,
            &self.shows,
            &Library::open(root),
            &path,
        )
        .map_err(|err| err.to_string())?;
        Ok((path, warnings))
    }

    /// Writes an audience frame as a PNG under `Exports/`.
    pub fn export_screenshot(&self, frame: &freeshow_output::Frame) -> Result<PathBuf, String> {
        let path = self.exports_dir()?.join("screenshot.png");
        let bytes = frame.encode_png().map_err(|err| err.to_string())?;
        std::fs::write(&path, bytes).map_err(|err| err.to_string())?;
        Ok(path)
    }

    pub fn template_entries(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.visible_template_entries()
            .map(|(_, entry)| entry)
            .collect()
    }

    fn visible_template_entries(
        &self,
    ) -> impl Iterator<Item = (usize, (&str, &str, &serde_json::Value))> {
        let query = self.search_query.trim().to_lowercase();
        self.template_catalog
            .named_entries()
            .into_iter()
            .chain(
                Catalog::starter_templates()
                    .named_entries()
                    .into_iter()
                    .filter(|(id, _, _)| self.template_catalog.get(id).is_none()),
            )
            .enumerate()
            .filter(move |(_, (id, name, _))| {
                (!self.template_favorites_only || self.is_template_favorite(id))
                    && (query.is_empty() || name.to_lowercase().contains(&query))
            })
    }

    pub fn is_template_favorite(&self, id: &str) -> bool {
        self.template_favorites.contains(id)
    }

    pub fn is_starter_template(&self, id: &str) -> bool {
        self.template_catalog.get(id).is_none() && Catalog::starter_templates().get(id).is_some()
    }

    pub fn selected_template(&self) -> Option<(&str, &str, &serde_json::Value)> {
        if self.tab != "templates" {
            return None;
        }
        let key = self.template_pick.as_deref()?;
        self.template_entries()
            .into_iter()
            .find(|(id, _, _)| *id == key)
    }

    pub fn template_target_show(&self) -> Option<String> {
        self.current_show().map(ShowFile::name)
    }

    pub fn browse_template(&mut self, id: &str) -> bool {
        if self.tab != "templates" || !self.template_entries().iter().any(|(key, _, _)| *key == id)
        {
            return false;
        }
        self.template_pick = Some(id.into());
        true
    }

    fn choose_template(&mut self, index: usize) -> DrawerAction {
        let picked = self
            .visible_template_entries()
            .nth(index)
            .map(|(_, (id, _, _))| id.to_owned());
        if picked.is_some() {
            self.template_pick = picked;
        }
        DrawerAction::None
    }

    fn apply_template_id(&mut self, id: &str) -> bool {
        let Some(template) = self.template_catalog.get_template(id).cloned() else {
            return false;
        };
        let Some(show_id) = self.current_show().map(|show| show.id.clone()) else {
            return false;
        };
        self.edit_show(&show_id, |show| show.apply_template(id, &template))
    }

    fn toggle_overlay(&mut self, index: usize) -> DrawerAction {
        let Some(name) = self.overlays.get(index).cloned() else {
            return DrawerAction::None;
        };
        let Some(id) = self.overlay_catalog.id_named(&name) else {
            return DrawerAction::None;
        };
        if let Some(pos) = self.active_overlays.iter().position(|active| active == &id) {
            if !self.overlay_is_locked(&id) {
                self.active_overlays.remove(pos);
            }
        } else {
            self.active_overlays.push(id);
        }
        DrawerAction::Refresh
    }

    fn choose_media(&mut self, index: usize) -> DrawerAction {
        let Some(entry) = self.media.get(index).cloned() else {
            return DrawerAction::None;
        };
        self.media_index = Some(index);
        if let Some(camera) = entry.name.strip_prefix("Camera: ") {
            self.replace_underlay(None);
            self.reset_background_look();
            self.background_transition = true;
            return DrawerAction::PlayCamera(camera.to_string());
        }
        if !entry.exists {
            return DrawerAction::None;
        }
        let path = entry.path;
        match media_kind(&path) {
            MediaKind::Image => {
                self.remember_background(&path);
                self.set_still_underlay(path.clone(), self.load_still(&path));
                self.background_transition = true;
                self.note_drawer_media_layer(&path);
                DrawerAction::Present
            }
            MediaKind::Video => {
                self.media_finished = None;
                self.remember_background(&path);
                self.remember_video_playback(&path);
                self.replace_underlay(None);
                let foreground = self.note_drawer_media_layer(&path);
                // Like FreeShow's media drawer, a picked video loops unless it is a foreground clip.
                self.media_loop = !foreground;
                self.background_transition = true;
                DrawerAction::PlayVideo(path)
            }
            MediaKind::Audio => DrawerAction::PlayAudio(path),
            MediaKind::Other => DrawerAction::None,
        }
    }

    fn choose_audio(&mut self, index: usize) -> DrawerAction {
        let Some(name) = self.audio.get(index).cloned() else {
            return DrawerAction::None;
        };
        self.audio_index = Some(index);
        self.file_in(AUDIO_DIR, &name)
            .map(DrawerAction::PlayAudio)
            .unwrap_or(DrawerAction::None)
    }

    fn edit_show(&mut self, id: &str, edit: impl FnOnce(&mut ShowFile) -> bool) -> bool {
        let Some(index) = self.shows.iter().position(|show| show.id == id) else {
            return false;
        };
        let snapshot = self.show_edit_snapshot(index, "Edit show");
        let layout = self.shows[index].active_layout();
        if !edit(&mut self.shows[index]) {
            return false;
        }
        if let Err(error) = self.persist_show_edit_then(index, snapshot, |session| {
            if session.shows[index].active_layout() != layout {
                session.slide_index = Some(0);
                session.item_index = 0;
            } else if session.show_index != Some(index) {
                session.slide_index = session.shows[index].first_enabled_slide();
                session.item_index = 0;
            }
            session.layer_selection = None;
            session.tab = "shows".to_string();
            session.show_index = Some(index);
        }) {
            self.notice = error;
            return false;
        }
        self.hold_program();
        true
    }

    pub fn play_named(&mut self, name: &str) -> bool {
        if let Some(camera) = name.strip_prefix("Camera: ") {
            if self.network_background {
                self.replace_underlay(None);
            }
            self.blanked = false;
            self.media_paused = false;
            self.background_transition = true;
            self.pending_play = Some(PendingPlay::Camera(camera.to_string()));
            return true;
        }
        if let Some(path) = self
            .file_in(MEDIA_DIR, name)
            .or_else(|| self.file_in(AUDIO_DIR, name))
            .or_else(|| {
                self.media.iter().find_map(|entry| {
                    (entry.exists
                        && (entry.name == name
                            || entry.path.to_string_lossy() == name
                            || entry.label() == name))
                        .then(|| entry.path.clone())
                })
            })
        {
            return self.play_path(path);
        }
        if let Some(path) = crate::live::resolve_media(self.root.as_deref(), name) {
            return self.play_path(path);
        }
        false
    }

    fn play_path(&mut self, path: PathBuf) -> bool {
        self.media_paused = false;
        match media_kind(&path) {
            MediaKind::Image => {
                self.remember_background(&path);
                self.set_still_underlay(path.clone(), self.load_still(&path));
                self.blanked = false;
                self.background_transition = true;
                if !matches!(self.pending_play, Some(PendingPlay::StopVideo)) {
                    self.pending_play = None;
                }
                self.note_media_layer(&path, MediaKind::Image);
                true
            }
            MediaKind::Video => {
                self.play_video(path);
                true
            }
            MediaKind::Audio => {
                if self.audio_paused && self.playing_audio.as_ref() == Some(&path) {
                    self.audio_paused = false;
                    self.keep_video_then(PendingPlay::ResumeAudio);
                } else {
                    self.queue_audio(path);
                }
                true
            }
            MediaKind::Other => false,
        }
    }

    fn play_video(&mut self, path: PathBuf) {
        self.media_paused = false;
        self.media_finished = None;
        self.remember_background(&path);
        self.remember_video_playback(&path);
        self.replace_underlay(None);
        self.blanked = false;
        self.note_media_layer(&path, MediaKind::Video);
        self.background_transition = true;
        self.pending_play = Some(PendingPlay::Video(path));
    }

    fn remember_background(&mut self, path: &Path) {
        self.imported_foreground_active = false;
        self.video_end = VideoEnd::Hold;
        self.video_end_fade = None;
        let Some(root) = self.root.clone() else {
            self.reset_background_look();
            return;
        };
        let library = Library::open(&root);
        let (fit, flip_x, flip_y) = library.media_look(path);
        let fit = if fit.is_empty() {
            library.style_fit()
        } else {
            fit
        };
        self.background_fit = Fit::named(&fit);
        self.background_flip_x = flip_x;
        self.background_flip_y = flip_y;
        let (filter, blend) = library.media_effect(path);
        self.background_filter = filter;
        self.background_blend = blend;
        let (blur_px, blur_opacity) = library.style_blur();
        self.background_blur_px = blur_px;
        self.background_blur_opacity = blur_opacity;
    }

    fn remember_video_playback(&mut self, path: &Path) {
        self.video_transport = None;
        let (rate, kind, volume, from, to) = self
            .root
            .as_deref()
            .map(|root| {
                let library = Library::open(root);
                let (from, to) = library.media_span(path);
                (
                    library.media_speed(path),
                    library.media_video_type(path),
                    f64::from(library.media_volume(path)),
                    from,
                    to,
                )
            })
            .unwrap_or((1.0, String::new(), 1.0, 0.0, 0.0));
        self.video_rate = rate;
        self.video_volume = volume;
        self.video_from = from;
        self.video_to = to;
        let background = kind == "background";
        let (row_loop, row_muted) = self.project_video_flags(path);
        self.media_loop = row_loop.unwrap_or(background);
        self.media_muted = row_muted.unwrap_or(background);
    }

    /// Loop and mute stored on the current project row when that row is this video.
    fn project_video_flags(&self, path: &Path) -> (Option<bool>, Option<bool>) {
        let Some(item) = self
            .current_project()
            .and_then(|project| project.shows.get(self.project_item).cloned())
        else {
            return (None, None);
        };
        if item.kind != "video" || item.id != path.to_string_lossy() {
            return (None, None);
        }
        (item.loops, item.muted)
    }

    /// A foreground picture or a one-shot video replaces the song. A background video keeps it.
    /// Media picked in the drawer goes behind the song; only a foreground clip replaces it.
    /// Returns whether it is a foreground clip.
    fn note_drawer_media_layer(&mut self, path: &Path) -> bool {
        let foreground = self
            .root
            .as_deref()
            .map(|root| Library::open(root).media_video_type(path))
            .unwrap_or_default()
            == "foreground";
        self.slide_hidden = foreground;
        foreground
    }

    fn note_media_layer(&mut self, path: &Path, kind: MediaKind) {
        let video_type = self
            .root
            .as_deref()
            .map(|root| Library::open(root).media_video_type(path))
            .unwrap_or_default();
        let looping = kind == MediaKind::Video && self.media_loop;
        self.slide_hidden = video_type == "foreground"
            || (video_type != "background" && (kind == MediaKind::Image || !looping));
    }

    /// Starts the video behind this slide. The same clip is left running.
    fn play_slide_video(&mut self, path: &Path, foreground: bool) {
        let entry = self.current_show().and_then(|show| {
            show.backdrop_media_entry_at(self.slide_index?, &path.to_string_lossy())
        });
        let flags = entry.map(ShowFile::media_entry_flags);
        let end = entry.map(VideoEnd::from_media).unwrap_or_default();
        let transport =
            entry.and_then(|media| MediaTransport::from_value(&media["videoTransport"]));
        let retrigger = entry.is_some_and(|media| media["retrigger"] == true);
        if !retrigger
            && self.background_path.as_deref() == Some(path)
            && self.video_transport == transport
        {
            self.video_end = end;
            if !self.timeline_playing {
                self.background_transition = false;
            }
            if foreground {
                self.slide_hidden = true;
            } else if let Some((_, _, video_type)) = &flags
                && video_type.is_some()
            {
                self.slide_hidden = video_type.as_deref() == Some("foreground");
            }
            return;
        }
        let foreground = foreground
            || flags
                .as_ref()
                .and_then(|(_, _, kind)| kind.as_deref())
                .map_or_else(
                    || {
                        self.root.as_deref().is_some_and(|root| {
                            Library::open(root).media_video_type(path) == "foreground"
                        })
                    },
                    |kind| kind == "foreground",
                );
        self.queue_background_video(
            path,
            flags.map(|(loops, muted, _)| (loops, muted)),
            foreground,
            transport,
        );
        self.video_end = end;
    }

    fn queue_background_video(
        &mut self,
        path: &Path,
        audio_flags: Option<(bool, bool)>,
        foreground: bool,
        transport: Option<MediaTransport>,
    ) {
        self.media_finished = None;
        self.remember_background(path);
        self.remember_video_playback(path);
        self.video_transport = transport;
        if let Some(transport) = transport {
            self.video_from = transport.from;
            self.video_to = transport.to;
            self.video_rate = transport.rate;
        }
        if let Some((loops, muted)) = audio_flags {
            self.media_loop = loops;
            self.media_muted = muted;
        }
        self.replace_underlay(None);
        self.blanked = false;
        self.background_path = Some(path.to_path_buf());
        self.background_transition = true;
        self.slide_hidden = foreground;
        self.pending_also = self
            .pending_also
            .take()
            .or_else(|| self.pending_play.take())
            .filter(|play| matches!(play, PendingPlay::StopAudio | PendingPlay::Audio(_)));
        self.pending_play = Some(PendingPlay::Video(path.to_path_buf()));
    }

    fn load_still(&self, path: &Path) -> Option<Raster> {
        let image = std::fs::read(path)
            .ok()
            .and_then(|bytes| decode_image(&bytes))?;
        let (left, right, top, bottom) = self
            .root
            .as_deref()
            .map(|root| Library::open(root).media_crop(path))
            .unwrap_or((0, 0, 0, 0));
        Some(crop_raster(&image, left, right, top, bottom))
    }

    fn show_background_still(&mut self, path: &Path, foreground: bool, retrigger: bool) -> bool {
        if !retrigger
            && self.background_path.as_deref() == Some(path)
            && self.underlay_from.as_deref() == Some(path)
            && self.underlay.is_some()
        {
            self.slide_hidden = foreground;
            if !self.timeline_playing {
                self.background_transition = false;
            }
            return true;
        }
        let Some(still) = self.load_still(path) else {
            return false;
        };
        self.discard_queued_video();
        self.remember_background(path);
        self.set_still_underlay(path.to_path_buf(), Some(still));
        self.background_path = Some(path.to_path_buf());
        self.background_transition = true;
        self.blanked = false;
        self.slide_hidden = foreground;
        true
    }

    fn reset_background_look(&mut self) {
        self.imported_foreground_active = false;
        self.video_end = VideoEnd::Hold;
        self.video_end_fade = None;
        self.background_fit = Fit::Contain;
        self.background_flip_x = false;
        self.background_flip_y = false;
        self.background_filter.clear();
        self.background_blend.clear();
        self.background_blur_px = 6.0;
        self.background_blur_opacity = 0.3;
        self.background_path = None;
    }

    fn clear_background(&mut self) {
        self.replace_underlay(None);
        self.reset_background_look();
        self.slide_hidden = false;
        self.background_transition = true;
        self.discard_queued_video();
    }

    fn clear_slide_layer(&mut self) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        self.slide_hidden = true;
        self.layer_builds = None;
        self.slide_countdown = None;
        self.layer_build_stepped = false;
        self.pending_cue_completion = None;
        self.timed_seconds = None;
        true
    }

    fn discard_queued_video(&mut self) {
        if matches!(
            self.pending_play,
            Some(PendingPlay::Video(_) | PendingPlay::Camera(_))
        ) {
            self.pending_play = self.pending_also.take();
        }
    }

    /// Puts this slide's picture behind the lyrics, carries an earlier picture, or clears it.
    fn apply_slide_background(&mut self) {
        self.apply_slide_actions();
        self.follow_started_show(0);
        self.follow_slide_jump(0);
    }

    /// Opens the slide this one names, then that slide's own jump, up to eight hops.
    fn follow_slide_jump(&mut self, depth: u8) {
        if depth >= 8 {
            return;
        }
        let jump = self
            .current_show()
            .and_then(|show| self.slide_index.and_then(|index| show.slide_jump_at(index)));
        let Some(jump) = jump else {
            return;
        };
        match jump {
            SlideJump::Index { show_id, index } => {
                if let Some(show_id) = show_id
                    && self.current_show().is_some_and(|show| show.id != show_id)
                {
                    if self.open_show_at(&show_id, index) {
                        self.follow_started_show(0);
                        self.follow_slide_jump(depth + 1);
                    }
                    return;
                }
                self.jump_to_slide(index, depth);
            }
            SlideJump::Name(name) => {
                let Some(index) = self.current_show().and_then(|show| show.group_index(&name))
                else {
                    return;
                };
                self.jump_to_slide(index, depth);
            }
            SlideJump::Group(id) => {
                let current = self.slide_index.unwrap_or(0);
                let Some(index) = self
                    .current_show()
                    .and_then(|show| show.next_global_group(&id, current))
                else {
                    return;
                };
                self.jump_to_slide(index, depth);
            }
            SlideJump::Next => self.jump_output(1, depth),
            SlideJump::Previous => self.jump_output(-1, depth),
        }
    }

    fn jump_output(&mut self, delta: isize, depth: u8) {
        let current = self.slide_index.unwrap_or(0);
        let (output_id, output_name) = self.audience();
        let Some(next) = self
            .current_show()
            .and_then(|show| show.step_output(current, delta, &output_id, &output_name))
        else {
            return;
        };
        self.jump_to_slide(next, depth);
    }

    fn jump_to_slide(&mut self, index: usize, depth: u8) {
        if self.slide_index == Some(index) {
            return;
        }
        let len = self
            .current_show()
            .map(|show| show.ordered_slides().len())
            .unwrap_or(0);
        if index >= len {
            return;
        }
        self.go_live();
        self.slide_hidden = false;
        self.slide_index = Some(index);
        self.reveal_count = 0;
        self.click_revealed = false;
        self.line_index = 0;
        self.apply_slide_actions();
        self.follow_started_show(0);
        self.follow_slide_jump(depth + 1);
    }

    fn apply_slide_actions(&mut self) {
        if self.tab == "scripture" {
            return;
        }
        self.layer_build_stepped = false;
        let slide = self.current_show().and_then(|show| {
            let ordered = show.ordered_slides();
            show.show["slides"].get(&ordered.get(self.slide_index?)?.id)
        });
        // Action-only cues can change media while retaining the composed slide.
        if !slide.is_some_and(|slide| slide["settings"]["actionOnly"].as_bool() == Some(true)) {
            let now = Instant::now();
            let countdown = slide.and_then(|slide| SlideCountdown::on_take(slide, now));
            self.layer_builds = slide.and_then(|slide| LayerBuilds::on_take(slide, now));
            self.slide_countdown = countdown;
        }
        self.pending_actions = None;
        self.pending_cue_actions = self.current_show().and_then(|show| {
            let slide_index = self.slide_index?;
            let cues = show.delayed_actions_at(slide_index);
            (!cues.is_empty()).then(|| PendingCueActions {
                started: Instant::now(),
                show_id: show.id.clone(),
                slide_index,
                cues: cues.into(),
            })
        });
        self.apply_action_slice(None);
    }

    pub fn run_due_cue_actions(&mut self, now: Instant) -> bool {
        let mut changed = false;
        while let Some(pending) = &mut self.pending_cue_actions {
            let Some(next) = pending.cues.front() else {
                self.pending_cue_actions = None;
                break;
            };
            let elapsed = now.saturating_duration_since(pending.started);
            if elapsed.as_millis() < u128::from(next.time_ms) {
                break;
            }
            let cue = pending.cues.pop_front().unwrap();
            let source = self
                .shows
                .iter()
                .position(|show| show.id == pending.show_id);
            if source.is_some() {
                changed |= self.run_timeline_cue(&cue, source);
            }
        }
        changed
    }

    /// Runs one slice of the slide actions. A wait stores the rest until it is due.
    fn apply_action_slice(&mut self, replacement: Option<Vec<serde_json::Value>>) {
        let show_index = self.show_index;
        let slide_index = self.slide_index;
        let real = self
            .current_show()
            .and_then(|show| slide_index.map(|index| show.actions_at(index)));
        let using = replacement.unwrap_or_else(|| real.clone().unwrap_or_default());
        let pause = freeshow_data::pause_in_actions(&using);
        let run = pause
            .as_ref()
            .map(|pause| pause.before.clone())
            .unwrap_or(using);
        let holding = pause.is_some();
        if holding
            && let (Some(show_index), Some(slide_index)) = (show_index, slide_index)
            && let Some(show) = self.shows.get_mut(show_index)
        {
            let _ = show.set_actions_at(slide_index, &run);
        }
        self.apply_slide_actions_now(false, real.as_deref().filter(|_| holding));
        if holding
            && let (Some(show_index), Some(slide_index), Some(real)) =
                (show_index, slide_index, real)
            && let Some(show) = self.shows.get_mut(show_index)
        {
            let _ = show.set_actions_at(slide_index, &real);
        }
        let still_here = self.show_index == show_index && self.slide_index == slide_index;
        if still_here
            && let Some(pause) = pause
            && !pause.after.is_empty()
        {
            let show_id = self
                .current_show()
                .map(|show| show.id.clone())
                .unwrap_or_default();
            self.pending_actions = Some(PendingActions {
                due: Instant::now() + Duration::from_secs_f64(pause.seconds),
                show_id,
                slide_index: slide_index.unwrap_or(0),
                actions: pause.after,
            });
        }
    }

    /// Runs the actions stored for later once their wait has elapsed.
    pub fn run_due_actions(&mut self) -> bool {
        let Some(pending) = self.pending_actions.clone() else {
            return false;
        };
        if Instant::now() < pending.due {
            return false;
        }
        self.pending_actions = None;
        let same = self
            .current_show()
            .is_some_and(|show| show.id == pending.show_id)
            && self.slide_index == Some(pending.slide_index);
        if !same {
            return false;
        }
        self.apply_saved_actions(pending.actions);
        true
    }

    /// Runs a waited action list without treating it as a new slide.
    fn apply_saved_actions(&mut self, actions: Vec<serde_json::Value>) {
        let show_index = self.show_index;
        let slide_index = self.slide_index;
        let real = self
            .current_show()
            .and_then(|show| slide_index.map(|index| show.actions_at(index)));
        let pause = freeshow_data::pause_in_actions(&actions);
        let run = pause
            .as_ref()
            .map(|pause| pause.before.clone())
            .unwrap_or(actions);
        if let (Some(show_index), Some(slide_index)) = (show_index, slide_index)
            && let Some(show) = self.shows.get_mut(show_index)
        {
            let _ = show.set_actions_at(slide_index, &run);
        }
        self.apply_slide_actions_now(true, real.as_deref());
        if let (Some(show_index), Some(slide_index), Some(real)) = (show_index, slide_index, real)
            && let Some(show) = self.shows.get_mut(show_index)
        {
            let _ = show.set_actions_at(slide_index, &real);
        }
        let still_here = self.show_index == show_index && self.slide_index == slide_index;
        if still_here
            && let Some(pause) = pause
            && !pause.after.is_empty()
        {
            let show_id = self
                .current_show()
                .map(|show| show.id.clone())
                .unwrap_or_default();
            self.pending_actions = Some(PendingActions {
                due: Instant::now() + Duration::from_secs_f64(pause.seconds),
                show_id,
                slide_index: slide_index.unwrap_or(0),
                actions: pause.after,
            });
        }
    }

    #[cfg(test)]
    /// Makes a stored wait due, then runs it. Used when a test does not sleep.
    pub fn finish_waits(&mut self) -> bool {
        if let Some(pending) = &mut self.pending_actions {
            pending.due = Instant::now();
        }
        self.run_due_actions()
    }

    fn apply_slide_actions_now(
        &mut self,
        continuation: bool,
        original_actions: Option<&[serde_json::Value]>,
    ) {
        if self.tab == "scripture" {
            return;
        }
        if self.current_show().is_some_and(|show| {
            show.show["origin"] == "propresenter"
                && self
                    .slide_index
                    .is_some_and(|index| show.clears_all_at(index))
        }) {
            self.clear_all_layers();
            return;
        }
        if !continuation {
            if self.imported_foreground_active {
                self.clear_background();
            }
            self.effect_playing = None;
            self.effect_return = None;
            self.timed_seconds = self
                .current_show()
                .and_then(|show| self.slide_index.and_then(|index| show.next_timer_at(index)));
            self.pending_cue_completion = self.current_show().and_then(|show| {
                let index = self.slide_index?;
                show.timed_cue_target_at(index)?;
                let duration = Duration::try_from_secs_f64(self.timed_seconds?).ok()?;
                Some(PendingCueCompletion {
                    due: Instant::now().checked_add(duration)?,
                    show_id: show.id.clone(),
                    layout_id: show.active_layout(),
                    index,
                })
            });
            let backdrop = self
                .current_show()
                .and_then(|show| self.slide_index.map(|index| show.backdrop_at(index)));
            if let Some(backdrop) = backdrop {
                match backdrop {
                    SlideBackdrop::Unchanged => {}
                    SlideBackdrop::Clear => self.clear_background(),
                    SlideBackdrop::Picture { path, foreground } => {
                        let path = PathBuf::from(path);
                        if media_kind(&path) == MediaKind::Video {
                            self.play_slide_video(&path, foreground);
                        } else if media_kind(&path) == MediaKind::Image {
                            let file_foreground = self.root.as_deref().is_some_and(|root| {
                                Library::open(root).media_video_type(&path) == "foreground"
                            });
                            self.show_background_still(&path, foreground || file_foreground, false);
                        }
                        self.imported_foreground_active = foreground
                            && self.background_path.as_ref() == Some(&path)
                            && self
                                .current_show()
                                .is_some_and(|show| show.show["origin"] == "propresenter");
                    }
                }
            }
        }
        self.stop_slide_audio();
        self.start_slide_audio(continuation);
        self.start_slide_stream();
        self.apply_camera();
        self.apply_media_toggle();
        self.apply_template_action(original_actions);
        self.apply_slide_overlays();
        self.apply_cleared_overlays();
        self.apply_action_overlays();
        self.apply_slide_timers();
        let hide_lyrics = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.clears_slide_at(index)))
            .unwrap_or(false);
        if hide_lyrics {
            self.slide_hidden = true;
        }
        self.apply_output_style();
        self.apply_transition_change();
        self.apply_timer_steps(original_actions);
        self.apply_mix_steps();
        self.apply_icecast_actions();
        self.apply_variable_actions();
        self.apply_playlist_steps();
        self.apply_audio_effects();
        self.apply_stage_layouts();
        self.apply_random_slide();
    }

    /// Opens a different slide in this show. A chain stops after eight hops.
    fn apply_random_slide(&mut self) {
        if self.random_hops >= 8 {
            return;
        }
        let wants = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.random_slide_at(index)))
            .unwrap_or(false);
        if !wants {
            return;
        }
        let count = self
            .current_show()
            .map(|show| show.ordered_slides().len())
            .unwrap_or(0);
        let current = self.slide_index.unwrap_or(0);
        if count < 2 || current >= count {
            return;
        }
        let pick = other_slide(count, current);
        let previous = self.random_hops;
        self.random_hops = previous + 1;
        self.jump_to_slide(pick, previous);
        self.random_hops = previous;
    }

    /// Switches the stage window to the layout this slide names.
    fn apply_stage_layouts(&mut self) {
        let steps = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.stage_layouts_at(index)))
            .unwrap_or_default();
        for (output, layout_id) in steps {
            let _ = self.select_stage_layout(output.as_deref(), &layout_id);
        }
    }

    pub fn select_primary_stage_layout(&mut self, id: &str) -> Result<(), String> {
        let output = self
            .preferences
            .outputs
            .iter()
            .find(|output| output.stage)
            .map(|output| output.id.clone());
        self.select_stage_layout(output.as_deref(), id)
    }

    pub fn create_stage_layout(&mut self, duplicate: bool) -> Result<String, String> {
        let root = self.root.as_deref().ok_or("open a library first")?;
        let library = Library::open(root);
        let id = if duplicate {
            library.duplicate_stage_layout(self.stage_layout_id.as_deref(), &self.stage)
        } else {
            library.create_stage_layout(&StageLayout {
                name: "New layout".into(),
                background: default_stage().background,
                items: Vec::new(),
            })
        }
        .map_err(|err| format!("could not create stage layout: {err}"))?;
        if self.select_primary_stage_layout(&id).is_ok() {
            return Ok(format!("Created {}", self.stage.name));
        }
        self.stage = library
            .stage_layout(&id)
            .ok_or("created stage layout is missing")?;
        self.stage_layout_id = Some(id);
        Ok("Created layout. Could not save its selection".into())
    }

    fn select_stage_layout(&mut self, output: Option<&str>, id: &str) -> Result<(), String> {
        let root = self.root.as_deref().ok_or("open a library first")?;
        let library = Library::open(root);
        let layout = library
            .stage_layout(id)
            .ok_or_else(|| format!("no stage layout {id:?}"))?;
        if !library.assign_stage_layout(output, id) {
            return Err("could not save the stage layout selection".into());
        }
        self.stage = layout;
        self.stage_layout_id = Some(id.into());
        Ok(())
    }

    /// Starts or steps the audio playlist named by this slide.
    fn apply_playlist_steps(&mut self) {
        let steps = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.playlist_steps_at(index)))
            .unwrap_or_default();
        for step in steps {
            match step {
                freeshow_data::PlaylistStep::Start { id, name } => {
                    let resolved = if !id.is_empty() {
                        Some(id)
                    } else if let Some(root) = self.root.clone() {
                        Library::open(&root).playlist_id_named(&name)
                    } else {
                        None
                    };
                    if let Some(id) = resolved {
                        self.start_playlist(&id);
                    }
                }
                freeshow_data::PlaylistStep::Next => self.step_playlist(),
            }
        }
    }

    /// Plays the first song. The same playlist already running is left alone.
    fn start_playlist(&mut self, id: &str) {
        if self.playlist_id.as_deref() == Some(id) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(list) = Library::open(&root).playlist(id) else {
            return;
        };
        if list.songs.is_empty() {
            return;
        }
        let songs = if list.shuffle {
            freeshow_data::shuffled_songs(&list.songs, id)
        } else {
            list.songs
        };
        self.playlist_id = Some(id.to_string());
        self.playlist_songs = songs.into_iter().map(PathBuf::from).collect();
        self.playlist_index = 0;
        self.playlist_loop = list.loops;
        self.playlist_volume = list.volume;
        self.media_finished = None;
        self.queue_playlist_song();
    }

    /// Moves to the next song. The list wraps when it is set to loop.
    fn step_playlist(&mut self) {
        if self.playlist_id.is_none() || self.playlist_songs.is_empty() {
            return;
        }
        let next = self.playlist_index + 1;
        if next >= self.playlist_songs.len() {
            if !self.playlist_loop {
                self.playlist_id = None;
                self.playlist_songs.clear();
                return;
            }
            self.playlist_index = 0;
        } else {
            self.playlist_index = next;
        }
        self.queue_playlist_song();
    }

    fn queue_playlist_song(&mut self) {
        let Some(path) = self.playlist_songs.get(self.playlist_index).cloned() else {
            return;
        };
        self.queue_audio(path);
    }

    /// The song that just ended belongs to the running playlist, so the next one is queued.
    fn advance_playlist(&mut self, path: &str, audio: bool) {
        if !audio || self.playlist_id.is_none() {
            return;
        }
        let current = self
            .playlist_songs
            .get(self.playlist_index)
            .map(|song| song.to_string_lossy().to_string());
        if current.as_deref() != Some(path) {
            return;
        }
        self.step_playlist();
    }

    /// Writes the variable changes stored on this slide.
    fn apply_variable_actions(&mut self) {
        let actions = self
            .current_show()
            .and_then(|show| {
                self.slide_index
                    .map(|index| show.variable_actions_at(index))
            })
            .unwrap_or_default();
        let Some(root) = self.root.clone() else {
            return;
        };
        let library = Library::open(&root);
        for action in actions {
            let _ = library.apply_variable(&action);
        }
    }

    fn apply_icecast_actions(&mut self) {
        let actions = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.actions_at(index)))
            .unwrap_or_default();
        for action in actions {
            if action["triggers"].as_array().is_some_and(|triggers| {
                triggers
                    .iter()
                    .any(|trigger| trigger.as_str() == Some("toggle_icecast"))
            }) {
                match action.pointer("/actionValues/toggle_icecast/value") {
                    None | Some(serde_json::Value::Null) => self.toggle_icecast(None),
                    Some(serde_json::Value::Bool(value)) => self.toggle_icecast(Some(*value)),
                    Some(serde_json::Value::String(value))
                        if value == "true" || value == "false" =>
                    {
                        self.toggle_icecast(Some(value == "true"));
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn toggle_icecast(&mut self, enabled: Option<bool>) {
        let (Some(root), Some((_, current))) = (&self.root, &mut self.icecast) else {
            return;
        };
        let enabled = enabled.unwrap_or(!*current);
        if enabled != *current {
            match Library::open(root).set_icecast_enabled(enabled) {
                Ok(()) => *current = enabled,
                Err(err) => tracing::error!("could not save Icecast enabled state: {err}"),
            }
        }
    }

    /// Sets the main channel volume or mute from this slide. Other channels are stored only.
    fn apply_mix_steps(&mut self) {
        let steps = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.mix_steps_at(index)))
            .unwrap_or_default();
        if steps.is_empty() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let library = Library::open(&root);
        for step in steps {
            match step {
                MixStep::Volume {
                    channel,
                    volume,
                    use_db,
                } => {
                    let current = if channel == "main" {
                        self.volume
                    } else {
                        library.channel_audio(&channel).0
                    };
                    let remembered = self.unmuted_volumes.entry(channel.clone()).or_insert(1.0);
                    let gain = mix_gain(volume, use_db, current, remembered);
                    if channel == "main" {
                        self.volume = gain.clamp(0.0, 1.0);
                    }
                    let _ = library.set_channel_volume(&channel, gain);
                }
                MixStep::Mute { channel, muted } => {
                    let muted = muted.unwrap_or_else(|| {
                        if channel == "main" {
                            !self.audio_muted
                        } else {
                            !library.channel_muted(&channel)
                        }
                    });
                    if channel == "main" {
                        self.audio_muted = muted;
                    }
                    let _ = library.set_channel_muted(&channel, muted);
                }
            }
        }
    }

    /// Removes one named overlay and leaves the others showing.
    fn apply_cleared_overlays(&mut self) {
        let ids = self
            .current_show()
            .and_then(|show| {
                self.slide_index
                    .map(|index| show.cleared_overlays_at(index))
            })
            .unwrap_or_default();
        for id in ids {
            if !self.overlay_is_locked(&id) {
                self.active_overlays.retain(|active| active != &id);
            }
        }
        self.restore_locked_overlays();
    }

    /// Stores the transition this slide names so the next frame uses it.
    fn apply_transition_change(&mut self) {
        let change = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.transition_change_at(index))
        });
        let Some(change) = change else {
            return;
        };
        let Some(root) = self.root.clone() else {
            return;
        };
        let _ = Library::open(&root).set_transition(
            &change.slot,
            &change.kind,
            change.duration_ms,
            &change.easing,
            "",
        );
    }

    /// A clear action drops this slide's countdown. A set action rewrites every slide's timer.
    fn apply_timer_steps(&mut self, original_actions: Option<&[serde_json::Value]>) {
        let steps = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.timer_steps_at(index)))
            .unwrap_or_default();
        let show_index = self.show_index;
        for step in steps {
            match step {
                TimerStep::Clear => {
                    self.timed_seconds = None;
                    self.pending_cue_completion = None;
                }
                TimerStep::Set(seconds) => {
                    self.pending_cue_completion = None;
                    if let Some(index) = show_index
                        && let Some(show) = self.shows.get_mut(index)
                    {
                        self.timer_save_bases
                            .entry(show.id.clone())
                            .or_insert_with(|| {
                                let mut original = show.clone();
                                if let Some((slide, actions)) =
                                    self.slide_index.zip(original_actions)
                                {
                                    original.set_actions_at(slide, actions);
                                }
                                original.show
                            });
                        show.set_next_timers(seconds);
                    }
                }
            }
        }
    }

    /// Lines per page from the active output style. Zero shows the whole slide.
    fn style_line_count(&self) -> u32 {
        self.root
            .as_deref()
            .map(|root| Library::open(root).style_lines())
            .unwrap_or(0)
    }

    /// The lyric window the audience should paint, when the style pages the slide.
    pub fn style_page(&self) -> Option<(usize, usize)> {
        let page = self.style_line_count();
        if page == 0 {
            return None;
        }
        let start = self.line_index as usize;
        Some((start, start + page as usize))
    }

    /// Id and name of the audience output this show is playing on.
    pub(crate) fn audience(&self) -> (String, String) {
        self.root
            .as_deref()
            .map(|root| Library::open(root).audience_output())
            .unwrap_or_else(|| ("default".into(), String::new()))
    }

    /// Stores the output style this slide names, so the next frame uses it.
    fn apply_output_style(&mut self) {
        let style = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.output_style_at(index))
        });
        let Some(style) = style else {
            return;
        };
        let Some(root) = self.root.clone() else {
            return;
        };
        let library = Library::open(&root);
        if library.output_style_id() != style && library.set_output_style(&style) {
            self.preferences = library.preferences();
            self.refresh_style_look();
        }
    }

    /// Starts the timers named by this slide. A stop action clears the one that is running.
    fn apply_slide_timers(&mut self) {
        let (stop, pause, ids) = self
            .current_show()
            .and_then(|show| {
                self.slide_index.map(|index| {
                    (
                        show.stops_timers_at(index),
                        show.pauses_timers_at(index),
                        show.timer_ids_at(index),
                    )
                })
            })
            .unwrap_or((false, false, Vec::new()));
        if stop {
            self.running = None;
        } else if pause && let Some(running) = self.running.as_mut() {
            running.mark = running.shown();
            running.since = None;
        }
        for id in ids {
            let _ = self.start_named_timer(&id);
        }
    }

    /// Turns on the overlays listed on this slide. A clear action removes the ones already showing.
    fn apply_slide_overlays(&mut self) {
        let (clear, ids) = self
            .current_show()
            .and_then(|show| {
                self.slide_index
                    .map(|index| (show.clears_overlays_at(index), show.overlays_at(index)))
            })
            .unwrap_or((false, Vec::new()));
        if clear {
            self.active_overlays.clear();
            self.restore_locked_overlays();
        }
        for id in ids {
            if self.overlay_catalog.get(&id).is_some()
                && !self.active_overlays.iter().any(|active| active == &id)
            {
                self.active_overlays.push(id);
            }
        }
        self.restore_locked_overlays();
    }

    /// Stops the playing audio when this slide asks for that.
    fn stop_slide_audio(&mut self) {
        let clear = self
            .current_show()
            .and_then(|show| {
                self.slide_index.map(|index| {
                    show.clears_audio_at(index)
                        || (self.transition_sound_playing && show.stops_transition_sound_at(index))
                })
            })
            .unwrap_or(false);
        if clear {
            self.stop_audio();
        }
    }

    fn stop_audio(&mut self) {
        self.playing_audio = None;
        self.transition_sound_playing = false;
        self.audio_loop_override = None;
        self.audio_transport = None;
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::StopAudio);
    }

    /// Keeps a slide video queued and stores the audio command beside it.
    fn keep_video_then(&mut self, play: PendingPlay) {
        if matches!(
            self.pending_play,
            Some(PendingPlay::Video(_) | PendingPlay::StopVideo)
        ) {
            self.pending_also = Some(play);
        } else {
            self.pending_play = Some(play);
        }
    }

    pub fn take_plays(&mut self) -> [Option<PendingPlay>; 2] {
        [self.pending_play.take(), self.pending_also.take()]
    }

    pub fn audio_change_pending(&self) -> bool {
        [&self.pending_play, &self.pending_also]
            .into_iter()
            .any(|play| matches!(play, Some(PendingPlay::Audio(_) | PendingPlay::StopAudio)))
    }

    /// Starts the first audio file listed on this slide, unless that file is already playing.
    fn start_slide_audio(&mut self, continuation: bool) {
        let audio = self.current_show().and_then(|show| {
            self.slide_index.and_then(|index| {
                show.audio_media_at(index).into_iter().find_map(|media| {
                    let transition_sound = media["transitionSound"] == true;
                    if transition_sound && continuation {
                        return None;
                    }
                    let path = PathBuf::from(media["path"].as_str()?);
                    (media_kind(&path) == MediaKind::Audio).then_some((
                        path,
                        media["loop"].as_bool(),
                        media["retrigger"] == true,
                        MediaTransport::from_value(&media["audioTransport"]),
                        transition_sound,
                    ))
                })
            })
        });
        if let Some((path, looping, retrigger, transport, transition_sound)) = audio
            && self.queue_cue_audio(path, looping, retrigger, transport)
        {
            self.transition_sound_playing = transition_sound;
        }
    }

    fn queue_cue_audio(
        &mut self,
        path: PathBuf,
        looping: Option<bool>,
        retrigger: bool,
        transport: Option<MediaTransport>,
    ) -> bool {
        if !retrigger
            && self.playing_audio.as_ref() == Some(&path)
            && !self.audio_paused
            && self.audio_loop_override == looping
            && self.audio_transport == transport
            && !self.timeline_owns_audio(&path)
        {
            return false;
        }
        self.queue_audio(path);
        self.audio_loop_override = looping;
        self.audio_transport = transport;
        true
    }

    fn queue_audio(&mut self, path: PathBuf) {
        self.remember_audio_playback(&path);
        self.keep_video_then(PendingPlay::Audio(path));
    }

    pub fn remember_audio_playback(&mut self, path: &Path) {
        self.media_finished = None;
        self.audio_loop_override = None;
        self.audio_transport = None;
        self.transition_sound_playing = false;
        if let Some(track) = &mut self.timeline_soundtrack {
            track.owned = false;
        }
        self.playing_audio = Some(path.to_path_buf());
        self.audio_paused = false;
    }

    /// Turns on overlays named by an action. Unknown ids are skipped.
    fn apply_action_overlays(&mut self) {
        let ids = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.action_overlays_at(index)))
            .unwrap_or_default();
        for id in ids {
            if self.overlay_catalog.get(&id).is_some()
                && !self.active_overlays.iter().any(|active| active == &id)
            {
                self.active_overlays.push(id);
            }
        }
    }

    /// Pauses or resumes the playing media when this slide asks for that.
    fn apply_media_toggle(&mut self) {
        let toggles = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.toggles_media_at(index)))
            .unwrap_or(false);
        if !toggles {
            return;
        }
        if self.pending_play.is_none() {
            self.pending_play = Some(PendingPlay::ToggleMedia);
        } else if self.pending_also.is_none() {
            self.pending_also = Some(PendingPlay::ToggleMedia);
        }
    }

    /// Copies the named template onto this show's lyric boxes.
    fn apply_template_action(&mut self, original_actions: Option<&[serde_json::Value]>) {
        let Some(id) = self
            .current_show()
            .and_then(|show| self.slide_index.and_then(|index| show.template_at(index)))
        else {
            return;
        };
        // A wait exposes only part of the action list while it runs. Save the
        // complete list with the template and restore the running slice afterward.
        let state = self.show_index.zip(self.slide_index).zip(original_actions);
        if let Some(((show_index, slide_index), original)) = state {
            let show = &mut self.shows[show_index];
            let running = show.actions_at(slide_index);
            show.set_actions_at(slide_index, original);
            let _ = self.apply_template_id(&id);
            self.shows[show_index].set_actions_at(slide_index, &running);
        } else {
            let _ = self.apply_template_id(&id);
        }
    }

    /// Starts the camera this slide names, and keeps a queued audio file beside it.
    fn apply_camera(&mut self) {
        let camera = self
            .current_show()
            .and_then(|show| self.slide_index.and_then(|index| show.camera_at(index)));
        let Some(camera) = camera else {
            return;
        };
        if matches!(&self.pending_play, Some(PendingPlay::Camera(current)) if current == &camera) {
            return;
        }
        let audio = match self.pending_play.take() {
            Some(PendingPlay::Audio(path)) => Some(PendingPlay::Audio(path)),
            other => {
                self.pending_play = other;
                None
            }
        };
        let _ = self.play_named(&format!("Camera: {camera}"));
        if let Some(audio) = audio {
            self.pending_also = Some(audio);
        }
    }

    /// Starts a saved audio stream when it points at an audio file.
    fn start_slide_stream(&mut self) {
        if matches!(self.pending_play, Some(PendingPlay::Audio(_)))
            || matches!(self.pending_also, Some(PendingPlay::Audio(_)))
        {
            return;
        }
        let raw = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.audio_stream_at(index))
        });
        let Some(raw) = raw else {
            return;
        };
        let resolved = self
            .root
            .as_deref()
            .and_then(|root| Library::open(root).audio_stream_value(&raw))
            .unwrap_or(raw);
        if resolved.starts_with("http://") || resolved.starts_with("https://") {
            return;
        }
        let path = PathBuf::from(&resolved);
        if media_kind(&path) != MediaKind::Audio {
            return;
        }
        if self.playing_audio.as_ref() == Some(&path) && !self.audio_paused {
            return;
        }
        self.queue_audio(path);
    }

    /// Plays the first sound effect on this slide. A running playlist resumes when it ends.
    fn apply_audio_effects(&mut self) {
        let path = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.effect_audio_at(index).into_iter().next())
        });
        let Some(path) = path else {
            return;
        };
        let path = PathBuf::from(path);
        self.effect_return = if self.playlist_id.is_some() {
            self.playlist_songs.get(self.playlist_index).cloned()
        } else {
            None
        };
        self.effect_playing = Some(path.clone());
        self.queue_audio(path);
    }

    /// The file that ended was a sound effect, so the playlist song starts again.
    fn resume_after_effect(&mut self, path: &str, audio: bool) -> bool {
        if !audio {
            return false;
        }
        let Some(effect) = self.effect_playing.clone() else {
            return false;
        };
        if effect.to_string_lossy() != path {
            return false;
        }
        self.effect_playing = None;
        let Some(back) = self.effect_return.take() else {
            return true;
        };
        self.queue_audio(back);
        true
    }

    /// Goes to the next slide when the file that just ended belongs to this slide.
    pub fn finish_media(&mut self, path: &str, audio: bool) -> bool {
        if !audio
            && matches!(
                self.pending_play,
                Some(PendingPlay::Video(_) | PendingPlay::Camera(_))
            )
        {
            return false;
        }
        let key = (path.to_string(), audio);
        if self.media_finished.as_ref() == Some(&key) {
            return false;
        }
        self.media_finished = Some(key.clone());
        if audio && self.timeline_owns_audio(Path::new(path)) {
            return false;
        }
        let ended_video = !audio && self.finish_background_video(Path::new(path));
        if !self.resume_after_effect(path, audio) {
            self.advance_playlist(path, audio);
        }
        let restarted = [&self.pending_play, &self.pending_also]
            .iter()
            .any(|play| matches!(play, Some(PendingPlay::Audio(_))));
        if audio
            && !restarted
            && self
                .playing_audio
                .as_ref()
                .is_some_and(|playing| playing.to_string_lossy() == path)
        {
            self.stop_audio();
        }
        let waits = self
            .current_show()
            .and_then(|show| {
                self.slide_index
                    .map(|index| show.waits_for_ended(index, path, audio))
            })
            .unwrap_or(false);
        if !waits {
            self.refresh_output |= ended_video;
            return ended_video;
        }
        // An automatic media completion cannot stand in for an operator's build click.
        let advanced = !self.waiting_for_layer_build() && self.advance_show();
        self.media_finished = Some(key);
        self.refresh_output |= advanced || ended_video;
        advanced || ended_video
    }

    fn finish_background_video(&mut self, path: &Path) -> bool {
        if self.background_path.as_deref() != Some(path) {
            return false;
        }
        let (fade, black) = match self.video_end {
            VideoEnd::Hold => return false,
            VideoEnd::Clear { fade } => (fade, false),
            VideoEnd::Black { fade } => (fade, true),
        };
        self.reset_background_look();
        self.replace_underlay(black.then(|| Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        }));
        if black {
            self.background_fit = Fit::Fill;
        }
        self.background_transition = true;
        self.video_end_fade = Some(fade);
        true
    }

    pub fn set_channel_volume(&mut self, id: &str, gain: f32) {
        if id == "main" {
            self.volume = gain.clamp(0.0, 1.0);
        }
        if let Some(root) = &self.root {
            Library::open(root).set_channel_volume(id, gain);
        }
    }

    fn audio_mixer(&self) -> serde_json::Value {
        let mut mixer = self.root.as_ref().map(|root| Library::open(root).audio_mixer())
            .unwrap_or_else(|| serde_json::json!({"audioRouting":{"channels":[{"id":"main", "name":"Main"}]}, "audioChannelsData":{}}));
        let main = mixer["audioChannelsData"]
            .as_object_mut()
            .unwrap()
            .entry("main")
            .or_insert_with(|| serde_json::json!({}));
        if !main.is_object() {
            *main = serde_json::json!({});
        }
        main["volume"] = serde_json::json!(self.volume);
        main["isMuted"] = serde_json::json!(self.audio_muted);
        mixer
    }

    /// Physical destinations for drawer/slide audio, or the active playlist.
    pub fn audio_routes(&self) -> Vec<SpeakerRoute> {
        let sources = self.audio_sources();
        self.routes_for_sources(
            &sources.iter().map(String::as_str).collect::<Vec<_>>(),
            self.audio_file_gain(),
        )
    }

    fn audio_sources(&self) -> Vec<String> {
        if let Some(id) = self
            .playlist_id
            .as_ref()
            .filter(|_| self.effect_playing.is_none())
        {
            vec![format!("playlist_sub_{id}"), "playlists_default".into()]
        } else {
            vec!["drawer_audio".into()]
        }
    }

    pub fn video_audio_routes(&self) -> Vec<SpeakerRoute> {
        let (output, _) = self.audience();
        self.routes_for_sources(
            &["output_window", &format!("output_win_sub_{output}")],
            self.video_volume as f32,
        )
    }

    pub fn audio_network_routes(&self) -> Vec<NetworkRoute> {
        let sources = self.audio_sources();
        self.network_routes(
            &sources.iter().map(String::as_str).collect::<Vec<_>>(),
            self.audio_file_gain(),
        )
    }

    pub fn video_network_routes(&self) -> Vec<NetworkRoute> {
        let (output, _) = self.audience();
        self.network_routes(
            &["output_window", &format!("output_win_sub_{output}")],
            self.video_volume as f32,
        )
    }

    fn network_routes(&self, sources: &[&str], file_gain: f32) -> Vec<NetworkRoute> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let main = if self.audio_muted { 0.0 } else { self.volume };
        Library::open(root)
            .network_audio_routes(sources, main)
            .into_iter()
            .map(|mut route| {
                let gain = route.gain * file_gain;
                route.gain = if gain.is_finite() {
                    gain.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                route
            })
            .collect()
    }

    pub fn network_output(&self, kind: &str) -> Option<String> {
        self.root
            .as_ref()
            .and_then(|root| Library::open(root).network_output(kind))
    }

    #[cfg(test)]
    pub fn ndi_output(&self) -> Option<(String, String)> {
        self.ndi_outputs()
            .into_iter()
            .find(|target| !target.stage)
            .map(|target| (target.id, target.name))
    }

    /// Enabled NDI pictures, including stage. Outputs that share an NDI group
    /// use one sender named `{name} [{group}]`.
    pub fn ndi_outputs(&self) -> Vec<NdiTarget> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        Library::open(root)
            .network_outputs("ndi")
            .into_iter()
            .map(|(id, output)| {
                let group = ndi_group(&output["ndiData"]["groups"]);
                let valid = |value: &serde_json::Value| {
                    value
                        .as_str()
                        .filter(|name| !name.is_empty() && !name.contains('\0'))
                        .map(str::to_string)
                };
                let name = valid(&output["ndiData"]["name"]).unwrap_or_else(|| {
                    valid(&output["name"])
                        .map(|name| format!("FreeShow NDI - {name}"))
                        .unwrap_or_else(|| "FreeShow NDI".into())
                });
                let stage = output["stageOutput"]
                    .as_str()
                    .is_some_and(|layout| !layout.is_empty());
                NdiTarget {
                    id,
                    name,
                    stage,
                    group,
                }
            })
            .collect()
    }

    pub fn omt_outputs(&self) -> Vec<OmtTarget> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        Library::open(root)
            .network_outputs("omt")
            .into_iter()
            .map(|(id, output)| {
                let name = named_source(&output, "omtData", "FreeShow OMT");
                let stage = is_stage_output(&output);
                OmtTarget { id, name, stage }
            })
            .collect()
    }

    pub fn webrtc_outputs(&self) -> Vec<WebrtcTarget> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        Library::open(root)
            .network_outputs("webrtc")
            .into_iter()
            .filter_map(|(id, output)| {
                let url = output["webrtcData"]["url"]
                    .as_str()
                    .map(str::trim)
                    .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
                    .map(str::to_string)?;
                let token = output["webrtcData"]["token"]
                    .as_str()
                    .map(str::trim)
                    .filter(|token| !token.is_empty())
                    .map(str::to_string);
                let stage = is_stage_output(&output);
                Some(WebrtcTarget {
                    id,
                    url,
                    token,
                    stage,
                })
            })
            .collect()
    }

    /// RTMP destinations that should start when the operator opens the library.
    pub fn rtmp_destinations(&self) -> Vec<RtmpTarget> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let auto = self
            .preferences
            .special
            .get("autoRtmp")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        Library::open(root)
            .network_outputs("rtmp")
            .into_iter()
            .flat_map(|(id, output)| rtmp_targets_for(&id, &output, auto))
            .collect()
    }

    /// Linux v4l2 loopback device for a virtual camera, when enabled.
    pub fn virtual_camera_device(&self) -> Option<String> {
        if let Some(device) = env_nonempty("FREESHOW_V4L2") {
            return Some(device);
        }
        if !(special_flag(&self.preferences.special, "virtualCamera")
            || special_flag(&self.preferences.special, "virtualOutput")
            || self.output_flag("virtual")
            || self.output_flag("virtualCamera"))
        {
            return None;
        }
        Some(
            special_text(
                &self.preferences.special,
                &["v4l2Device", "virtualCameraDevice"],
            )
            .unwrap_or_else(|| "/dev/video10".to_string()),
        )
    }

    /// Blackmagic Decklink device number, when the library asked to send to it.
    pub fn blackmagic_device(&self) -> Option<i32> {
        if let Some(value) = env_nonempty("FREESHOW_DECKLINK") {
            return value.parse().ok();
        }
        if !(special_flag(&self.preferences.special, "blackmagic")
            || special_flag(&self.preferences.special, "autoBlackmagic")
            || self.output_flag("blackmagic")
            || self.output_flag("decklink"))
        {
            return None;
        }
        special_text(
            &self.preferences.special,
            &["decklinkDevice", "blackmagicDevice"],
        )
        .and_then(|value| value.parse().ok())
        .or(Some(0))
    }

    /// Spout or Syphon share name, when the operator enabled a texture share.
    pub fn capture_share_name(&self) -> Option<String> {
        if let Some(name) =
            env_nonempty("FREESHOW_SPOUT").or_else(|| env_nonempty("FREESHOW_SYPHON"))
        {
            return Some(name);
        }
        if !(special_flag(&self.preferences.special, "spout")
            || special_flag(&self.preferences.special, "syphon")
            || self.output_flag("spout")
            || self.output_flag("syphon"))
        {
            return None;
        }
        Some(
            special_text(&self.preferences.special, &["spoutName", "syphonName"])
                .unwrap_or_else(|| "FreeShow".to_string()),
        )
    }

    /// NDI or OMT source the library asked to receive as a background, if any.
    pub fn network_receive(&self) -> Option<(freeshow_ndi::SourceKind, String)> {
        if let Some(name) = env_nonempty("FREESHOW_NDI_IN") {
            return Some((freeshow_ndi::SourceKind::Ndi, name));
        }
        if let Some(name) = env_nonempty("FREESHOW_OMT_IN") {
            return Some((freeshow_ndi::SourceKind::Omt, name));
        }
        let special = &self.preferences.special;
        special_text(special, &["ndiInput", "ndiSource"])
            .map(|name| (freeshow_ndi::SourceKind::Ndi, name))
            .or_else(|| {
                special_text(special, &["omtInput", "omtSource"])
                    .map(|name| (freeshow_ndi::SourceKind::Omt, name))
            })
    }

    fn output_flag(&self, kind: &str) -> bool {
        self.root.as_ref().is_some_and(|root| {
            Library::open(root)
                .network_outputs(kind)
                .into_iter()
                .next()
                .is_some()
        })
    }

    fn routes_for_sources(&self, sources: &[&str], file_gain: f32) -> Vec<SpeakerRoute> {
        let main = if self.audio_muted { 0.0 } else { self.volume };
        let Some(root) = &self.root else {
            return vec![SpeakerRoute::stereo(
                None,
                (main * file_gain).clamp(0.0, 1.0),
            )];
        };
        let mut destinations =
            std::collections::BTreeMap::<(Option<String>, Option<u16>, Option<u16>), f32>::new();
        for route in Library::open(root).audio_routes(sources, main) {
            let device = route
                .device
                .map(|id| self.audio_device_map.get(&id).cloned().unwrap_or(id));
            *destinations
                .entry((device, route.input_channel, route.output_channel))
                .or_default() += route.gain * file_gain;
        }
        destinations
            .into_iter()
            .map(
                |((device, input_channel, output_channel), gain)| SpeakerRoute {
                    device,
                    gain: gain.clamp(0.0, 1.0),
                    input_channel,
                    output_channel,
                },
            )
            .collect()
    }
    fn audio_file_gain(&self) -> f32 {
        let file = self
            .playing_audio
            .as_ref()
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_volume(path))
            })
            .unwrap_or(1.0);
        let playlist = if self.playlist_id.is_some() && self.effect_playing.is_none() {
            self.playlist_volume
        } else {
            1.0
        };
        (file * playlist).clamp(0.0, 1.0)
    }

    /// The file that is playing repeats when `media.json` says so.
    pub fn audio_should_loop(&self) -> bool {
        if let Some(looping) = self.audio_loop_override {
            return self.playing_audio.is_some() && looping;
        }
        self.playing_audio
            .as_ref()
            .filter(|path| !self.timeline_owns_audio(path))
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_loops(path))
            })
            .unwrap_or(false)
    }

    /// Stored start and end for the file that is playing. Zero is the file start or the file end.
    pub fn audio_span(&self) -> (f64, f64) {
        if let Some(transport) = self.audio_transport {
            return (transport.from, transport.to);
        }
        self.playing_audio
            .as_ref()
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_span(path))
            })
            .unwrap_or((0.0, 0.0))
    }

    /// Stored playback speed for the file that is playing. Missing values stay at normal speed.
    pub fn audio_tempo(&self) -> f64 {
        if let Some(transport) = self.audio_transport {
            return transport.rate;
        }
        self.playing_audio
            .as_ref()
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_tempo(path))
            })
            .unwrap_or(1.0)
    }

    fn file_in(&self, folder: &str, name: &str) -> Option<PathBuf> {
        let path = self.root.as_ref()?.join(folder).join(name);
        path.is_file().then_some(path)
    }

    /// Browsing without presenting: the output keeps showing the live slide or verse.
    pub fn hold_program(&mut self) {
        self.program_held = true;
    }

    /// The selection becomes the program again.
    pub fn go_live(&mut self) {
        if self.tab == "scripture" {
            self.layer_builds = None;
            self.slide_countdown = None;
            self.layer_build_stepped = false;
        }
        if self.tab == "scripture"
            || (self.program_held
                && self.pending_cue_actions.as_ref().is_some_and(|pending| {
                    self.slide_index != Some(pending.slide_index)
                        || self
                            .current_show()
                            .is_none_or(|show| show.id != pending.show_id)
                }))
        {
            self.pending_cue_actions = None;
        }
        self.blanked = false;
        self.program_held = false;
    }

    /// A drawer tab change from the operator, which never changes the output.
    pub fn switch_tab(&mut self, tab: &str) {
        if self.tab != tab {
            self.hold_program();
            self.tab = tab.to_string();
            self.drawer_pick = None;
        }
    }

    /// Before stepping, the open show and slide jump back to what is live, as FreeShow steps the output.
    fn resume_program(&mut self) {
        if !self.program_held {
            return;
        }
        let live = match self.program_content.borrow().as_ref() {
            Some(crate::live::ProgramContent::Slide { show_id, index, .. }) => {
                Some((show_id.clone(), *index))
            }
            Some(crate::live::ProgramContent::Scripture(lines)) => {
                let verse = self
                    .verse_index
                    .and_then(|index| self.verse_hits.get(index));
                if verse.is_some_and(|hit| lines.first() == Some(&hit.reference)) {
                    self.tab = "scripture".to_string();
                }
                None
            }
            _ => None,
        };
        if let Some((show_id, index)) = live
            && let Some(show) = self.shows.iter().position(|show| show.id == show_id)
        {
            if self.show_index != Some(show) {
                self.show_index = Some(show);
                self.load_timeline();
            }
            self.tab = "shows".to_string();
            self.slide_index = Some(index);
        }
        self.program_held = false;
    }

    pub fn select_slide(&mut self, index: usize) {
        if self.preferences.out_locked() {
            return;
        }
        self.go_live();
        self.slide_hidden = false;
        if self.tab == "scripture" {
            if index < self.verse_hits.len() {
                self.verse_index = Some(index);
            }
            return;
        }
        if self
            .current_show()
            .is_some_and(|show| index < show.ordered_slides().len())
        {
            self.slide_index = Some(index);
            self.reveal_count = 0;
            self.click_revealed = false;
            self.line_index = 0;
            self.apply_slide_background();
        }
    }

    /// Picks the slide to edit without changing anything on the output.
    pub fn select_slide_for_edit(&mut self, index: usize) {
        self.hold_program();
        if self.tab != "scripture"
            && self
                .current_show()
                .is_some_and(|show| index < show.ordered_slides().len())
        {
            if self.slide_index != Some(index) {
                self.layer_selection = None;
            }
            self.slide_index = Some(index);
            self.line_index = 0;
        }
    }

    pub fn current_slide_text(&self) -> Option<String> {
        let show = self.show_index.and_then(|index| self.shows.get(index))?;
        let slides = show.ordered_slides();
        let slide = slides.get(self.slide_index?)?;
        Some(slide.lines.join("\n"))
    }

    /// Writes the selected slide even when another drawer tab is open.
    pub fn replace_slide_text(&mut self, text: &str) -> bool {
        self.edit_selected_slide("Edit lyrics", |show, slide, item| {
            show.set_item_text(slide, item, text)
        })
    }

    pub fn save_lyrics(&mut self, text: &str) -> bool {
        self.tab == "shows" && self.replace_slide_text(text)
    }

    pub fn save_group(&mut self, group: &str) -> bool {
        self.tab == "shows"
            && self.edit_selected_slide("Edit group", |show, slide, _| show.set_group(slide, group))
    }

    pub fn save_notes(&mut self, notes: &str) -> bool {
        self.tab == "shows"
            && self.edit_selected_slide("Edit notes", |show, slide, _| show.set_notes(slide, notes))
    }

    fn current_item(&self, show: &freeshow_data::ShowFile) -> usize {
        let Some(slide_index) = self.slide_index else {
            return 0;
        };
        let count = show
            .ordered_slides()
            .get(slide_index)
            .map(|slide| show.item_count(&slide.id))
            .unwrap_or(0);
        if count == 0 {
            0
        } else {
            self.item_index.min(count - 1)
        }
    }

    pub fn item_position(&self) -> (usize, usize) {
        let Some(show) = self.current_show() else {
            return (0, 0);
        };
        let count = self
            .slide_index
            .and_then(|index| {
                show.ordered_slides()
                    .get(index)
                    .map(|slide| slide.id.clone())
            })
            .map(|id| show.item_count(&id))
            .unwrap_or(0);
        (self.item_index.min(count.saturating_sub(1)), count)
    }

    pub fn item_label(&self) -> String {
        let (index, count) = self.item_position();
        format!("Box {} of {}", index + 1, count.max(1))
    }

    pub fn item_typography(&self) -> freeshow_data::ItemTypography {
        let Some(show) = self.current_show() else {
            return freeshow_data::ItemTypography {
                font_px: 100,
                ..Default::default()
            };
        };
        let Some(slide_id) = self.slide_index.and_then(|index| {
            show.ordered_slides()
                .get(index)
                .map(|slide| slide.id.clone())
        }) else {
            return show.item_typography("", 0);
        };
        show.item_typography(&slide_id, self.current_item(show))
    }

    pub fn item_filters(&self) -> freeshow_data::ItemFilters {
        let Some(show) = self.current_show() else {
            return freeshow_data::ItemFilters::default();
        };
        let Some(slide_id) = self.slide_index.and_then(|index| {
            show.ordered_slides()
                .get(index)
                .map(|slide| slide.id.clone())
        }) else {
            return freeshow_data::ItemFilters::default();
        };
        show.item_filters(&slide_id, self.current_item(show))
    }

    fn edit_current_item(&mut self, edit: impl FnOnce(&mut ShowFile, &str, usize) -> bool) -> bool {
        self.tab == "shows" && self.edit_selected_slide("Edit item", edit)
    }

    fn edit_selected_slide(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut ShowFile, &str, usize) -> bool,
    ) -> bool {
        self.edit_selected_slide_then(
            label,
            |show, slide, item| edit(show, slide, item).then_some(()),
            |_, ()| {},
        )
    }

    fn edit_selected_slide_then<T>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut ShowFile, &str, usize) -> Option<T>,
        select: impl FnOnce(&mut Self, T),
    ) -> bool {
        let Some(snapshot) = self.edit_snapshot(label) else {
            return false;
        };
        let Some((index, slide_index)) = self.show_index.zip(self.slide_index) else {
            return false;
        };
        let show = &mut self.shows[index];
        let Some(slide_id) = show
            .ordered_slides()
            .get(slide_index)
            .map(|slide| slide.id.clone())
        else {
            return false;
        };
        let item = self
            .item_index
            .min(show.item_count(&slide_id).saturating_sub(1));
        let Some(selection) = edit(show, &slide_id, item) else {
            show.show = snapshot.show;
            return false;
        };
        match self.persist_show_edit_then(index, snapshot, |session| select(session, selection)) {
            Ok(()) => true,
            Err(error) => {
                self.notice = error;
                false
            }
        }
    }

    pub fn add_item_box(&mut self) -> bool {
        self.edit_and_select_item("Edit item", |show, slide, _| show.add_item_box(slide))
    }

    pub fn add_shape_box(&mut self, preset: &str) -> bool {
        let size = self.edit_source_size();
        self.edit_and_select_item("Add shape", |show, slide, _| {
            show.add_shape_box(slide, preset, size)
        })
    }

    pub fn slide_pictures(&self) -> impl Iterator<Item = &MediaEntry> {
        self.media
            .iter()
            .filter(|entry| entry.kind == MediaEntryKind::Image && entry.exists)
    }

    /// Library pictures and videos for the stage media picker: `(label, path)`.
    pub fn stage_media_files(&self) -> Vec<(String, String)> {
        self.media
            .iter()
            .filter(|entry| {
                entry.exists && matches!(entry.kind, MediaEntryKind::Image | MediaEntryKind::Video)
            })
            .map(|entry| (entry.label(), entry.path.display().to_string()))
            .collect()
    }

    /// Library stills for stage web preview cards: `(label, path)`.
    pub fn stage_still_files(&self) -> Vec<(String, String)> {
        self.slide_pictures()
            .map(|entry| (entry.label(), entry.path.display().to_string()))
            .collect()
    }

    pub fn add_picture_box(&mut self, index: usize) -> Result<(), String> {
        let (_, src) = self.slide_picture_source(index)?;
        if !self.edit_and_select_item("Edit item", |show, slide, _| {
            show.add_picture_box(slide, &src)
        }) {
            return Err("Could not save the picture box".to_string());
        }
        Ok(())
    }

    pub fn can_replace_picture_box(&self) -> bool {
        self.layer_indices().len() == 1
            && self.selected_layer_slide().is_some_and(|(show, slide)| {
                show.can_replace_item_picture(&slide, self.current_item(show))
            })
    }

    pub fn replace_picture_box(&mut self, index: usize) -> Result<(), String> {
        if !self.can_replace_picture_box() {
            return Err("Select one unlocked picture layer".into());
        }
        let (path, src) = self.slide_picture_source(index)?;
        let bytes =
            std::fs::read(path).map_err(|error| format!("Could not read the picture. {error}"))?;
        if decode_image(&bytes).is_none() {
            return Err("FreeShow could not open the replacement picture".into());
        }
        if !self.edit_selected_slide("Replace picture", |show, slide, item| {
            show.replace_item_picture(slide, item, &src)
        }) {
            return Err("Could not replace the picture".into());
        }
        Ok(())
    }

    fn slide_picture_source(&self, index: usize) -> Result<(&Path, String), String> {
        let picture = self
            .slide_pictures()
            .nth(index)
            .filter(|picture| picture.path.is_file())
            .ok_or_else(|| "The selected picture is no longer available".to_string())?;
        let relative = self
            .root
            .as_ref()
            .and_then(|root| picture.path.strip_prefix(root).ok());
        let src = relative
            .unwrap_or(&picture.path)
            .to_str()
            .ok_or_else(|| "The picture path contains unsupported characters".to_string())?;
        let src = if relative.is_some() && std::path::MAIN_SEPARATOR != '/' {
            src.replace(std::path::MAIN_SEPARATOR, "/")
        } else {
            src.to_string()
        };
        Ok((&picture.path, src))
    }

    fn edit_and_select_item(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut ShowFile, &str, usize) -> Option<usize>,
    ) -> bool {
        self.tab == "shows"
            && self.edit_selected_slide_then(label, edit, |session, next| {
                session.item_index = next;
                session.layer_selection = None;
            })
    }

    pub fn set_font_family(&mut self, family: &str) -> bool {
        self.edit_current_item(|show, slide_id, item| {
            show.set_item_font_family(slide_id, item, family)
        })
    }

    pub fn output_lines(&self) -> Vec<String> {
        if self.tab == "scripture" {
            return self
                .verse_index
                .and_then(|index| self.verse_hits.get(index))
                .map(|hit| vec![hit.reference.clone(), hit.text.clone()])
                .unwrap_or_default();
        }
        let Some(show) = self.current_show() else {
            return Vec::new();
        };
        let Some(index) = self.slide_index else {
            return Vec::new();
        };
        show.ordered_slides()
            .get(index)
            .map(|slide| show.visible_lines_for_slide(&slide.id))
            .unwrap_or_default()
    }

    pub fn to_snapshot(&self) -> freeshow_remote::ShowSnapshot {
        let program = self.program_content.borrow();
        let mut show_index = self.show_index;
        let mut slide_index = self.slide_index;
        let mut taken_slide = None;
        if !self.blanked
            && !self.slide_hidden
            && let Some(crate::live::ProgramContent::Slide {
                show_id,
                slide_id,
                slide,
                index,
                ..
            }) = program.as_ref()
        {
            show_index = self.shows.iter().position(|show| &show.id == show_id);
            slide_index = Some(*index);
            taken_slide = Some(freeshow_remote::RemoteSlide {
                id: slide_id.clone(),
                label: String::new(),
                lines: freeshow_data::visible_slide_lines(&crate::live::program_slide_readback(
                    self, slide,
                )),
                notes: slide["notes"].as_str().unwrap_or_default().to_string(),
                group: String::new(),
            });
        }
        freeshow_remote::ShowSnapshot {
            shows: self
                .shows
                .iter()
                .map(|show| {
                    let layouts = show
                        .layout_views()
                        .into_iter()
                        .map(|layout| freeshow_remote::RemoteLayout {
                            id: layout.id,
                            name: layout.name,
                            slides: remote_slides(
                                show,
                                layout.slides,
                                self.preferences.group_numbers(),
                            ),
                        })
                        .collect::<Vec<_>>();
                    let active = show.active_layout();
                    let slides = layouts
                        .iter()
                        .find(|layout| layout.id == active)
                        .map(|layout| layout.slides.clone())
                        .unwrap_or_else(|| {
                            remote_slides(
                                show,
                                show.ordered_slides(),
                                self.preferences.group_numbers(),
                            )
                        });
                    freeshow_remote::RemoteShow {
                        id: show.id.clone(),
                        name: show.name(),
                        slides,
                        layouts,
                        active_layout: active,
                    }
                })
                .collect(),
            show_index,
            slide_index,
            cleared: self.blanked || self.slide_hidden,
            taken_slide,
            project: self.project_labels(),
            projects: self
                .projects
                .projects()
                .into_iter()
                .map(|project| freeshow_remote::RemoteProject {
                    id: project.id,
                    name: project.name,
                    parent: project.parent,
                    created: project.created,
                    shows: project
                        .shows
                        .into_iter()
                        .map(|item| {
                            let name = if item.label.is_empty() {
                                self.shows
                                    .iter()
                                    .find(|show| show.id == item.id)
                                    .map(ShowFile::name)
                                    .unwrap_or_default()
                            } else {
                                item.label
                            };
                            freeshow_remote::RemoteProjectItem {
                                id: item.id,
                                name,
                                kind: item.kind,
                            }
                        })
                        .collect(),
                })
                .collect(),
            project_id: self.project_id.clone(),
            folders: self
                .projects
                .folders()
                .into_iter()
                .map(|folder| freeshow_remote::RemoteFolder {
                    id: folder.id,
                    name: folder.name,
                    parent: folder.parent,
                })
                .collect(),
            bibles: Arc::clone(&self.scripture),
            scripture_lines: match program.as_ref() {
                Some(crate::live::ProgramContent::Scripture(lines))
                    if !self.blanked && !self.slide_hidden =>
                {
                    lines.clone()
                }
                _ if self.tab == "scripture" && !self.blanked && !self.program_held => {
                    self.output_lines()
                }
                _ => Vec::new(),
            },
            overlays: overlay_views(&self.overlay_catalog),
            active_overlays: self.active_overlays.clone(),
            media: self
                .media
                .iter()
                .map(|entry| file_view(&entry.name))
                .collect(),
            audio: self.audio.iter().map(|name| file_view(name)).collect(),
            timers: self
                .timers
                .iter()
                .map(|timer| freeshow_remote::RemoteTimer {
                    id: timer.id.clone(),
                    name: timer.name.clone(),
                    start: timer.start,
                    end: timer.end,
                })
                .collect(),
            active_timer: self.running.as_ref().map(|timer| timer.id.clone()),
            timer_paused: self
                .running
                .as_ref()
                .is_some_and(|timer| timer.since.is_none()),
            media_loop: self.media_loop,
            media_muted: self.media_muted,
            media_paused: self.media_paused,
            video_time: self.video_time,
            video_duration: self.video_duration,
            playing_audio: self
                .playing_audio
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            audio_time: self.audio_time,
            audio_duration: self.audio_duration,
            audio_paused: self.audio_paused,
            templates: self
                .template_catalog
                .entries()
                .into_iter()
                .map(|(id, body)| freeshow_remote::RemoteTemplate {
                    name: body
                        .get("name")
                        .and_then(|name| name.as_str())
                        .unwrap_or(&id)
                        .to_string(),
                    id,
                    body,
                })
                .collect(),
            volume: self.volume,
            audio_mixer: self.audio_mixer(),
            project_item: self.project_item,
            next_item_on_last_slide: self
                .root
                .as_deref()
                .is_none_or(|root| Library::open(root).next_item_on_last_slide()),
        }
    }

    pub fn apply_remote(&mut self, command: freeshow_remote::Command) {
        match command {
            freeshow_remote::Command::SetShow(index) => {
                self.tab = "shows".to_string();
                self.select_drawer(index);
                if self.slide_index.is_none() {
                    self.slide_index = self.first_slide();
                    self.apply_slide_background();
                }
                self.go_live();
            }
            freeshow_remote::Command::SetSlide(index) => {
                self.tab = "shows".to_string();
                self.blanked = false;
                self.select_slide(index);
            }
            freeshow_remote::Command::OpenProjectItem { index, slide } => {
                let _ = self.open_project_item(index, slide);
            }
            freeshow_remote::Command::Clear => {
                self.clear_all_layers();
            }
            freeshow_remote::Command::ClearSlide => {
                self.clear_slide_layer();
            }
            freeshow_remote::Command::ClearBackground => {
                if self.preferences.out_locked() {
                    return;
                }
                self.replace_underlay(None);
                self.reset_background_look();
                self.video_time = 0.0;
                self.video_duration = 0.0;
                self.pending_play = Some(PendingPlay::StopVideo);
            }
            freeshow_remote::Command::ShowOverlay { id } => {
                if self.overlay_catalog.get(&id).is_some()
                    && !self.active_overlays.iter().any(|active| active == &id)
                {
                    self.active_overlays.push(id);
                }
            }
            freeshow_remote::Command::ClearOverlay { id } => {
                if self.preferences.out_locked() || self.overlay_is_locked(&id) {
                    return;
                }
                self.active_overlays.retain(|active| active != &id);
            }
            freeshow_remote::Command::ClearOverlays => {
                if self.preferences.out_locked() {
                    return;
                }
                self.active_overlays.clear();
                self.restore_locked_overlays();
            }
            freeshow_remote::Command::Play { name } => {
                let _ = self.play_named(&name);
            }
            freeshow_remote::Command::SetVolume { thousandths } => {
                self.set_channel_volume("main", f32::from(thousandths) / 1000.0);
            }
            freeshow_remote::Command::SetChannelVolume { id, thousandths } => {
                self.set_channel_volume(&id, f32::from(thousandths) / 1000.0);
            }
            freeshow_remote::Command::MuteChannel { id, muted } => {
                if id == "main" {
                    self.audio_muted = muted;
                }
                if let Some(root) = &self.root {
                    Library::open(root).set_channel_muted(&id, muted);
                }
            }
            freeshow_remote::Command::StopAudio => {
                self.stop_audio();
            }
            freeshow_remote::Command::SeekAudio { millis } => {
                if self.playing_audio.is_some() {
                    self.pending_play = Some(PendingPlay::SeekAudio(millis));
                }
            }
            freeshow_remote::Command::SeekVideo { millis } => {
                self.pending_play = Some(PendingPlay::SeekVideo(millis));
            }
            freeshow_remote::Command::SetMediaLoop { on } => {
                self.media_loop = on;
                self.pending_play = Some(PendingPlay::SetLoop);
            }
            freeshow_remote::Command::SetMediaMute { on } => {
                self.media_muted = on;
                self.pending_play = Some(PendingPlay::SetMute);
            }
            freeshow_remote::Command::SetTemplate { id } => {
                let _ = self.apply_template_id(&id);
            }
            freeshow_remote::Command::StartTimer { id } => {
                let _ = self.start_named_timer(&id);
            }
            freeshow_remote::Command::PauseTimer { id } => {
                let _ = self.pause_named_timer(&id);
            }
            freeshow_remote::Command::StopTimer { id } => {
                let _ = self.stop_named_timer(&id);
            }
            freeshow_remote::Command::PauseAudio { name } => {
                let matches = self
                    .playing_audio
                    .as_ref()
                    .is_some_and(|path| names_the_file(path, &name));
                if matches {
                    self.audio_paused = true;
                    self.pending_play = Some(PendingPlay::PauseAudio);
                }
            }
            freeshow_remote::Command::ToggleIcecast(enabled) => self.toggle_icecast(enabled),
            freeshow_remote::Command::ToggleMedia => {
                self.pending_play = Some(PendingPlay::ToggleMedia);
            }
            freeshow_remote::Command::RearrangeGroups { show_id, from, to } => {
                let _ = self.edit_show(&show_id, |show| show.reorder_groups(from, to));
            }
            freeshow_remote::Command::AddGroup { show_id, group_id } => {
                let _ = self.edit_show(&show_id, |show| show.duplicate_slide(&group_id).is_some());
            }
            freeshow_remote::Command::PresentVerse { reference, text } => {
                self.tab = "scripture".to_string();
                self.go_live();
                self.slide_hidden = false;
                self.verse_hits = vec![VerseHit { reference, text }];
                self.verse_index = Some(0);
            }
            freeshow_remote::Command::SetShowText { id, text } => {
                let _ = self.set_show_text(&id, &text);
            }
            freeshow_remote::Command::CreateShow { id, name, text } => {
                let _ = self.create_show_with(&id, &name, &text);
            }
            freeshow_remote::Command::AddToProject {
                project_id,
                show_id,
            } => {
                if self.projects.append_show(&project_id, &show_id) {
                    let _ = self.save_projects();
                }
            }
            freeshow_remote::Command::CreateProject { id, name, parent } => {
                if self.projects.add_project_in(&id, &name, &parent) {
                    let _ = self.save_projects();
                    self.project_id = Some(id);
                    self.project_item = 0;
                }
            }
            freeshow_remote::Command::RenameProject { project_id, name } => {
                if self.projects.set_project_name(&project_id, &name) {
                    let _ = self.save_projects();
                }
            }
            freeshow_remote::Command::DeleteProject { project_id } => {
                if self.projects.delete_project(&project_id) {
                    if self.project_id.as_deref() == Some(project_id.as_str()) {
                        self.project_id = self
                            .projects
                            .projects()
                            .first()
                            .map(|project| project.id.clone());
                        self.project_item = 0;
                    }
                    let _ = self.save_projects();
                }
            }
            freeshow_remote::Command::ChangeLayout { show_id, layout_id } => {
                let _ = self.edit_show(&show_id, |show| show.set_active_layout(&layout_id));
                if self
                    .shows
                    .iter()
                    .any(|show| show.id == show_id && show.active_layout() == layout_id)
                {
                    self.slide_index = Some(0);
                    self.go_live();
                    self.slide_hidden = false;
                }
            }
            freeshow_remote::Command::RemoveProjectItem { project_id, index } => {
                if self.projects.remove_show_at(&project_id, index) {
                    if self.project_id.as_deref() == Some(project_id.as_str())
                        && index < self.project_item
                    {
                        self.project_item -= 1;
                    }
                    let _ = self.save_projects();
                }
            }
        }
    }

    pub fn step_slide(&mut self, delta: isize) {
        self.go_live();
        self.slide_hidden = false;
        let len = self
            .current_show()
            .map(|show| show.ordered_slides().len())
            .unwrap_or(0);
        if len == 0 {
            return;
        }
        let current = self.slide_index.unwrap_or(0);
        let (output_id, output_name) = self.audience();
        let Some(next) = self
            .current_show()
            .and_then(|show| show.step_output(current, delta, &output_id, &output_name))
        else {
            return;
        };
        self.slide_index = Some(next);
        self.reveal_count = 0;
        self.click_revealed = false;
        self.line_index = 0;
        self.apply_slide_background();
    }

    pub fn move_project(&mut self, delta: isize) -> bool {
        let Some(project_id) = self.project_id.clone() else {
            return false;
        };
        if self.highlighted_project_item().is_none() {
            self.notice = "Click the project item to move first".to_string();
            return false;
        }
        if !self.projects.reorder(&project_id, self.project_item, delta) {
            return false;
        }
        if let Some(next) = self.project_item.checked_add_signed(delta) {
            self.project_item = next;
        }
        self.save_projects()
    }

    /// Moves the project item at `from` to `to`, as when it is dragged in the project list.
    pub fn drop_project_item(&mut self, from: usize, to: usize) -> bool {
        let Some(project_id) = self.project_id.clone() else {
            return false;
        };
        if from == to
            || !self
                .projects
                .reorder(&project_id, from, to as isize - from as isize)
        {
            return false;
        }
        // The picked item keeps its highlight wherever it now sits.
        let picked = self.project_item;
        self.project_item = if picked == from {
            to
        } else if from < picked && picked <= to {
            picked - 1
        } else if to <= picked && picked < from {
            picked + 1
        } else {
            picked
        };
        self.save_projects()
    }

    pub fn create_show(&mut self) -> bool {
        let name = unique_name("Untitled", self.shows.iter().map(ShowFile::name).collect());
        self.create_library_show(ShowFile::blank(&new_id(), &name))
            .is_ok()
    }

    fn create_library_show(&mut self, mut show: ShowFile) -> Result<(), String> {
        let root = self
            .root
            .as_deref()
            .ok_or("Choose a library folder first")?;
        let directory = Library::open(root).shows_dir();
        let stem = library_edit::show_file_name(&show.name());
        let mut path = directory.join(format!("{stem}.show"));
        if path.try_exists().map_err(|err| err.to_string())? {
            path = directory.join(format!("{stem} ({}).show", show.id));
        }
        show.save(&path)
            .map_err(|err| format!("Could not save the new show: {err}"))?;
        show.path = Some(path);
        let id = show.id.clone();
        let pos = self
            .shows
            .iter()
            .position(|existing| existing.name() > show.name())
            .unwrap_or(self.shows.len());
        self.shows.insert(pos, show);
        self.tab = "shows".to_string();
        self.hold_program();
        self.show_index = Some(pos);
        self.layer_selection = None;
        self.item_index = 0;
        self.project_picked = false;
        self.load_timeline();
        self.slide_index = self.first_slide();
        if let Some(project_id) = self.project_id.clone()
            && self.projects.append_show(&project_id, &id)
        {
            let _ = self.save_projects();
            if let Some(len) = self.current_project().map(|project| project.shows.len()) {
                self.project_item = len.saturating_sub(1);
            }
        }
        Ok(())
    }

    pub fn create_show_with(&mut self, id: &str, name: &str, text: &str) -> bool {
        if self.shows.iter().any(|show| show.id == id) {
            return self.set_show_text(id, text);
        }
        let Some(root) = self.root.clone() else {
            return false;
        };
        let name = if name.trim().is_empty() {
            unique_name("Untitled", self.shows.iter().map(ShowFile::name).collect())
        } else {
            unique_name(name, self.shows.iter().map(ShowFile::name).collect())
        };
        let mut show = ShowFile::blank(id, &name);
        let _ = show.replace_lyrics_with_split(
            text,
            self.preferences.format_new_show(),
            self.preferences.split_lines(),
        );
        let path = Library::open(&root)
            .shows_dir()
            .join(format!("{}.show", library_edit::show_file_name(&name)));
        if show.save(&path).is_err() {
            return false;
        }
        show.path = Some(path);
        let pos = self
            .shows
            .iter()
            .position(|existing| existing.name() > show.name())
            .unwrap_or(self.shows.len());
        self.shows.insert(pos, show);
        self.tab = "shows".to_string();
        self.hold_program();
        // The insert shifted the list, so the open show's index no longer names it.
        self.show_index = None;
        self.select_drawer(pos);
        // A new show opens into editing on its first slide.
        self.slide_index = self.first_slide();
        if let Some(project_id) = self.project_id.clone()
            && self.projects.append_show(&project_id, id)
        {
            let _ = self.save_projects();
        }
        true
    }

    pub fn set_show_text(&mut self, id: &str, text: &str) -> bool {
        let Some(index) = self.shows.iter().position(|show| show.id == id) else {
            return false;
        };
        if self.shows[index].has_locked_layers() {
            self.notice = "Unlock the show’s layers before replacing its text".into();
            return false;
        }
        let snapshot = self.show_edit_snapshot(index, "Edit show text");
        if !self.shows[index].replace_lyrics_with_split(
            text,
            self.preferences.format_new_show(),
            self.preferences.split_lines(),
        ) {
            return false;
        }
        if let Err(error) = self.persist_show_edit_then(index, snapshot, |session| {
            session.layer_selection = None;
            session.tab = "shows".to_string();
            session.show_index = Some(index);
            session.slide_index = Some(0);
            session.item_index = 0;
        }) {
            self.notice = error;
            return false;
        }
        self.hold_program();
        self.slide_hidden = false;
        true
    }

    pub fn add_slide(&mut self) -> bool {
        if self.tab != "shows" {
            return false;
        }
        let Some(index) = self.show_index else {
            return false;
        };
        let Some(snapshot) = self.edit_snapshot("Add slide") else {
            return false;
        };
        let show = &mut self.shows[index];
        if show.add_slide().is_none() {
            return false;
        }
        let slide = show.ordered_slides().len().saturating_sub(1);
        if let Err(error) = self.persist_show_edit_then(index, snapshot, |session| {
            session.slide_index = Some(slide);
            session.layer_selection = None;
            session.item_index = 0;
        }) {
            self.notice = error;
            return false;
        }
        self.hold_program();
        self.slide_hidden = false;
        true
    }

    pub fn create_project(&mut self) -> bool {
        let existing = self
            .projects
            .projects()
            .into_iter()
            .map(|project| project.name)
            .collect();
        let name = dated_project_name(existing);
        let id = new_id();
        if !self.projects.add_project(&id, &name) {
            return false;
        }
        if !self.save_projects() {
            return false;
        }
        self.project_id = Some(id);
        self.project_item = 0;
        self.project_picked = false;
        self.remember_project();
        true
    }

    pub fn add_current_show(&mut self) -> bool {
        let Some(project_id) = self.project_id.clone() else {
            return false;
        };
        let Some(show_id) = self.current_show().map(|show| show.id.clone()) else {
            return false;
        };
        if !self.projects.append_show(&project_id, &show_id) {
            return false;
        }
        // The new item is the one the operator is looking at.
        self.project_item = self.project_labels().len().saturating_sub(1);
        self.project_picked = true;
        self.save_projects()
    }

    /// The project row to highlight: the picked item, unless the operator has since opened
    /// a different show from the library.
    pub fn highlighted_project_item(&self) -> Option<usize> {
        if !self.project_picked {
            return None;
        }
        let item = self
            .current_project()?
            .shows
            .get(self.project_item)?
            .clone();
        match item.kind.as_str() {
            "show" | "" => self
                .current_show()
                .is_some_and(|show| show.id == item.id)
                .then_some(self.project_item),
            _ => Some(self.project_item),
        }
    }

    pub fn reload_import_catalog(&mut self, imported_project: Option<String>) {
        let Some(root) = &self.root else {
            return;
        };
        let library = Library::open(root);
        if let Ok(mut shows) = library.list_shows() {
            let previous: HashMap<_, _> = self
                .shows
                .iter()
                .map(|show| (show.id.as_str(), &show.show))
                .collect();
            let mut unchanged = BTreeSet::new();
            for fresh in &mut shows {
                let Some(&loaded) = previous.get(fresh.id.as_str()) else {
                    continue;
                };
                if loaded != &fresh.show {
                    if self.timer_save_bases.get(&fresh.id) != Some(&fresh.show) {
                        continue;
                    }
                    // An unrelated import must not discard pending timer edits.
                    fresh.show.clone_from(loaded);
                }
                unchanged.insert(fresh.id.clone());
            }
            // History from a different on-disk revision could overwrite external edits.
            self.history
                .retain(|entry| unchanged.contains(&entry.snapshot.show_id));
            self.redo
                .retain(|entry| unchanged.contains(&entry.snapshot.show_id));
            self.timer_save_bases.retain(|id, _| unchanged.contains(id));
            if let Some(show) = self.current_show() {
                let previous_slide = self.slide_index.and_then(|index| {
                    show.ordered_slides()
                        .get(index)
                        .map(|slide| slide.id.clone())
                });
                let index = shows
                    .iter()
                    .position(|fresh| fresh.id == show.id)
                    .unwrap_or_else(|| {
                        shows.push(show.clone());
                        shows.len() - 1
                    });
                self.show_index = Some(index);
                if let Some(previous) = previous_slide {
                    let ordered = shows[index].ordered_slides();
                    self.slide_index = ordered
                        .iter()
                        .position(|slide| slide.id == previous)
                        .or_else(|| (!ordered.is_empty()).then_some(0));
                }
            }
            self.shows = shows;
        } else {
            tracing::error!("could not reload shows after ChurchApps import");
        }
        if let Ok(catalog) = library.load_templates() {
            self.templates = catalog.names();
            self.template_catalog = catalog;
        }
        if let Ok(catalog) = library.load_overlays() {
            self.overlays = catalog.names();
            self.overlay_catalog = catalog;
        }
        if let Ok(projects) = library.load_projects() {
            let active_item = self.current_project().and_then(|project| {
                project
                    .shows
                    .get(self.project_item)
                    .map(|item| item.id.clone())
            });
            self.projects = projects;
            match self.current_project() {
                Some(project) => {
                    let previous_index = self.project_item;
                    self.project_item = active_item
                        .and_then(|id| {
                            if project
                                .shows
                                .get(previous_index)
                                .is_some_and(|item| item.id == id)
                            {
                                Some(previous_index)
                            } else {
                                project.shows.iter().position(|item| item.id == id)
                            }
                        })
                        .unwrap_or_else(|| {
                            previous_index.min(project.shows.len().saturating_sub(1))
                        });
                }
                None => {
                    self.project_id = imported_project;
                    self.project_item = 0;
                }
            }
        } else {
            tracing::error!("could not reload projects after ChurchApps import");
        }
    }

    /// Persists pending timer edits and the project file.
    pub fn autosave(&mut self) -> usize {
        let mut wrote = 0;
        for show in &self.shows {
            let Some(base) = self.timer_save_bases.get(&show.id) else {
                continue;
            };
            let Some(path) = &show.path else {
                continue;
            };
            if self.has_pending_show_draft(path) {
                continue;
            }
            let Ok(saved) = ShowFile::load(path) else {
                continue;
            };
            if saved.id != show.id {
                continue;
            }
            if saved.show != show.show {
                if saved.show != *base || show.save(path).is_err() {
                    continue;
                }
                wrote += 1;
            }
            self.timer_save_bases.remove(&show.id);
        }
        let Some(root) = &self.root else {
            return wrote;
        };
        let path = Library::open(root).projects_path();
        if let Ok(json) = self.projects.to_json()
            && std::fs::read(&path).ok().as_deref() != Some(json.as_bytes())
            && self.projects.save(&path).is_ok()
        {
            wrote += 1;
        }
        wrote
    }

    /// Moves to the next or previous verse, past the search results into the Bible.
    fn step_verse(&mut self, delta: i64) -> bool {
        let Some(index) = self.verse_index else {
            return false;
        };
        let target = index as i64 + delta;
        if target < 0 || target as usize >= self.verse_hits.len() {
            let neighbour = self.verse_hits.get(index).and_then(|hit| {
                let (book_chapter, verse) = hit.reference.rsplit_once(':')?;
                let verse = verse.parse::<i64>().ok()? + delta;
                (verse > 0).then(|| format!("{book_chapter}:{verse}"))
            });
            let bible = self.bibles.get(self.bible_index.unwrap_or(0));
            let Some(hit) = neighbour
                .zip(bible)
                .and_then(|(query, bible)| bible.search(&query).into_iter().next())
            else {
                return false;
            };
            if delta > 0 {
                self.verse_hits.push(hit);
                self.verse_index = Some(self.verse_hits.len() - 1);
            } else {
                self.verse_hits.insert(0, hit);
                self.verse_index = Some(0);
            }
        } else {
            self.verse_index = Some(target as usize);
        }
        self.go_live();
        self.slide_hidden = false;
        true
    }

    /// The playing audio file for the operator's now-playing bar: name, time, progress and paused.
    pub fn now_playing(&self) -> Option<(String, String, f32, bool)> {
        let path = self.playing_audio.as_ref()?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let clock = |seconds: f64| {
            let seconds = seconds.max(0.0) as u64;
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        let time = if self.audio_duration > 0.0 {
            format!(
                "{} / {}",
                clock(self.audio_time),
                clock(self.audio_duration)
            )
        } else {
            clock(self.audio_time)
        };
        let progress = if self.audio_duration > 0.0 {
            (self.audio_time / self.audio_duration) as f32
        } else {
            0.0
        };
        Some((name, time, progress, self.audio_paused))
    }

    /// Pause, resume or stop the playing audio from the now-playing bar.
    pub fn audio_control(&mut self, action: &str) -> bool {
        let Some(path) = self.playing_audio.clone() else {
            return false;
        };
        let name = path.to_string_lossy().into_owned();
        let command = match action {
            "stop" => freeshow_remote::Command::StopAudio,
            "toggle" if self.audio_paused => freeshow_remote::Command::Play { name },
            "toggle" => freeshow_remote::Command::PauseAudio { name },
            _ => return false,
        };
        self.ui_commands.push(command);
        true
    }

    pub fn search(&mut self, query: &str) {
        self.hold_program();
        let index = self.bible_index.unwrap_or(0);
        let Some(bible) = self.bibles.get(index) else {
            self.verse_hits.clear();
            self.verse_index = None;
            return;
        };
        self.verse_hits = bible.search(query);
        self.verse_index = if self.verse_hits.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    fn save_projects(&self) -> bool {
        let Some(root) = &self.root else {
            return false;
        };
        self.projects
            .save(&Library::open(root).projects_path())
            .is_ok()
    }

    /// The slide of the open show that is on the output now, if any.
    pub fn live_slide_index(&self) -> Option<usize> {
        if self.blanked || self.slide_hidden {
            return None;
        }
        if self.tab == "scripture" {
            return match self.program_content.borrow().as_ref() {
                Some(crate::live::ProgramContent::Scripture(lines))
                    if *lines == self.output_lines() =>
                {
                    self.verse_index
                }
                _ => None,
            };
        }
        let show = self.current_show()?;
        match self.program_content.borrow().as_ref() {
            Some(crate::live::ProgramContent::Slide { show_id, index, .. })
                if show_id == &show.id =>
            {
                Some(*index)
            }
            _ => None,
        }
    }

    fn current_show(&self) -> Option<&ShowFile> {
        self.show_index.and_then(|index| self.shows.get(index))
    }

    /// The slide being shown wins. A new background image uses the media transition instead.
    pub fn output_motion(&mut self) -> (freeshow_data::TextTransition, bool) {
        let background = self.background_transition;
        self.background_transition = false;
        let build = std::mem::take(&mut self.layer_build_stepped);
        if let Some(fade) = self.video_end_fade.take() {
            let mut motion = self
                .style_motion("media")
                .unwrap_or_else(|| self.library_motion(true));
            if fade && motion.duration_ms > 0 {
                motion.kind = "fade".into();
            } else if fade {
                motion = freeshow_data::TextTransition::default();
            } else {
                motion.kind = "none".into();
            }
            return (motion, true);
        }
        if self.blanked || (build && !background) {
            return (
                freeshow_data::TextTransition {
                    kind: "none".into(),
                    ..Default::default()
                },
                false,
            );
        }
        if background {
            let from_slide = if self.tab == "shows" {
                self.current_show().and_then(|show| {
                    self.slide_index
                        .and_then(|index| show.media_transition_at(index))
                })
            } else {
                None
            };
            let motion = from_slide
                .or_else(|| self.style_motion("media"))
                .unwrap_or_else(|| self.library_motion(true));
            return (motion, true);
        }
        (self.selected_slide_motion().0, false)
    }

    /// The text transition and its source, shared by Take and the editor.
    pub fn selected_slide_motion(&self) -> (freeshow_data::TextTransition, &'static str) {
        if self.tab == "shows"
            && let Some((show, index)) = self.current_show().zip(self.slide_index)
            && let Some(motion) = show.transition_at(index)
        {
            return (motion, "This slide");
        }
        if let Some(motion) = self.style_motion("text") {
            return (motion, "Output style");
        }
        (self.library_motion(false), "Library default")
    }

    fn style_motion(&self, kind: &str) -> Option<freeshow_data::TextTransition> {
        self.root
            .as_deref()
            .and_then(|root| Library::open(root).style_transition(kind))
    }

    pub fn library_transition(&self, media: bool) -> freeshow_data::TextTransition {
        self.library_motion(media)
    }

    fn library_motion(&self, media: bool) -> freeshow_data::TextTransition {
        let Some(root) = self.root.as_deref() else {
            return if media {
                freeshow_data::TextTransition {
                    duration_ms: 800,
                    ..freeshow_data::TextTransition::default()
                }
            } else {
                freeshow_data::TextTransition::default()
            };
        };
        let library = Library::open(root);
        if media {
            library.media_transition()
        } else {
            library.text_transition()
        }
    }

    /// Seconds until this slide should advance on its own.
    pub fn next_timer_seconds(&self) -> Option<f64> {
        if self.blanked || self.waiting_for_layer_build() {
            return None;
        }
        if let Some(seconds) = self.folder_timer_seconds() {
            return Some(seconds);
        }
        if self.tab != "shows" {
            return None;
        }
        self.timed_seconds
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| {
                seconds.max(self.automatic_build_remaining(Instant::now()).as_secs_f64())
            })
    }

    pub fn has_pending_cue_completion(&self) -> bool {
        self.pending_cue_completion.is_some()
    }

    pub(crate) fn waiting_for_layer_build(&self) -> bool {
        !self.blanked
            && !self.slide_hidden
            && self
                .layer_builds
                .as_ref()
                .is_some_and(LayerBuilds::waiting_for_next)
    }

    pub(crate) fn automatic_build_remaining(&self, now: Instant) -> Duration {
        if self.blanked || self.slide_hidden {
            return Duration::ZERO;
        }
        self.layer_builds.as_ref().map_or(Duration::ZERO, |builds| {
            builds.automatic_entrance_remaining(now)
        })
    }

    fn step_layer_build(&mut self, forward: bool) -> bool {
        if self.blanked || self.slide_hidden || (self.tab != "shows" && !self.program_held) {
            return false;
        }
        let changed = self.layer_builds.as_mut().is_some_and(|builds| {
            if forward {
                builds.next(Instant::now())
            } else {
                builds.previous(Instant::now())
            }
        });
        self.layer_build_stepped |= changed;
        changed
    }

    pub(crate) fn tick_layer_builds(&mut self, now: Instant) -> bool {
        let delay = self.preferences.output_delay_ms();
        !self.blanked
            && !self.slide_hidden
            && self
                .layer_builds
                .as_mut()
                .is_some_and(|builds| builds.tick(now, delay))
    }

    pub(crate) fn tick_live_graphics(&mut self, now: Instant) -> bool {
        if self.blanked || self.slide_hidden {
            return false;
        }
        let delay = self.preferences.output_delay_ms();
        let countdown_changed = self
            .slide_countdown
            .as_mut()
            .is_some_and(|countdown| countdown.tick(now, delay));
        self.tick_layer_builds(now) || countdown_changed
    }

    pub fn run_due_cue_completion(&mut self, now: Instant) -> bool {
        let Some(pending) = &self.pending_cue_completion else {
            return false;
        };
        let matching_show = self.current_show().filter(|show| {
            self.tab == "shows"
                && !self.blanked
                && self.slide_index == Some(pending.index)
                && show.id == pending.show_id
                && show.active_layout() == pending.layout_id
        });
        let Some(show) = matching_show else {
            self.pending_cue_completion = None;
            return false;
        };
        if now < pending.due
            || self.waiting_for_layer_build()
            || !self.automatic_build_remaining(now).is_zero()
        {
            return false;
        }
        let (output_id, output_name) = self.audience();
        let next = match show.timed_cue_target_at(pending.index) {
            Some(TimedCueTarget::First) => show.first_output(&output_id, &output_name),
            Some(TimedCueTarget::Next) => {
                show.step_output(pending.index, 1, &output_id, &output_name)
            }
            Some(TimedCueTarget::Slide(id)) => {
                show.output_index_for_slide(&id, &output_id, &output_name)
            }
            None => None,
        };
        self.pending_cue_completion = None;
        let Some(next) = next.filter(|_| !self.preferences.out_locked()) else {
            return false;
        };
        self.select_slide(next);
        true
    }

    /// The next slide in this song, the next picture in a folder, or the next project item.
    pub fn advance_show(&mut self) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        if self.step_layer_build(true) {
            return true;
        }
        self.resume_program();
        if self.tab == "scripture" {
            return self.step_verse(1);
        }
        if self.step_folder(1) {
            return true;
        }
        let page = self.style_line_count();
        if page > 0 {
            let most = self
                .current_show()
                .and_then(|show| self.slide_index.map(|index| show.most_lines(index)))
                .unwrap_or(0) as u32;
            if self.line_index + page < most {
                self.line_index += page;
                if self.current_show().is_some_and(|show| {
                    self.slide_index
                        .is_some_and(|index| show.has_click_reveal(index))
                }) && !self.click_revealed
                {
                    self.click_revealed = true;
                }
                return true;
            }
        }
        if self.current_show().is_some_and(|show| {
            self.slide_index
                .is_some_and(|index| show.has_click_reveal(index))
        }) && !self.click_revealed
        {
            self.click_revealed = true;
            return true;
        }
        if let Some(total) = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.line_reveal_lines(index))
        }) && (self.reveal_count as usize) < total
        {
            self.reveal_count += 1;
            return true;
        }
        if self.slide_index.is_none() && self.current_show().is_some() {
            let Some(first) = self.first_slide() else {
                return self.open_next_project_show();
            };
            self.go_live();
            self.slide_hidden = false;
            self.slide_index = Some(first);
            self.reveal_count = 0;
            self.click_revealed = false;
            self.line_index = 0;
            self.apply_slide_background();
            return true;
        }
        let (output_id, output_name) = self.audience();
        let restart = self.current_show().and_then(|show| {
            let index = self.slide_index.unwrap_or(0);
            show.ends_at(index)
                .then(|| show.first_output(&output_id, &output_name))
                .flatten()
        });
        if let Some(first) = restart {
            if self.slide_index != Some(first) {
                self.go_live();
                self.slide_hidden = false;
                self.slide_index = Some(first);
                self.reveal_count = 0;
                self.click_revealed = false;
                self.line_index = 0;
                self.apply_slide_background();
            }
            return true;
        }
        let index = self.slide_index.unwrap_or(0);
        let (output_id, output_name) = self.audience();
        let next = self
            .current_show()
            .and_then(|show| show.step_output(index, 1, &output_id, &output_name));
        if let Some(next) = next {
            self.step_slide(1);
            return self.slide_index == Some(next);
        }
        self.open_next_project_show()
    }

    /// The slide Next presents first in the open show: the first one this output shows.
    pub fn first_slide(&self) -> Option<usize> {
        let show = self.current_show()?;
        let (output_id, output_name) = self.audience();
        show.first_output(&output_id, &output_name)
            .or_else(|| show.first_enabled_slide())
            .or_else(|| (!show.ordered_slides().is_empty()).then_some(0))
    }

    /// Picks the first slide when none is, so editing has something to edit.
    pub fn pick_first_slide(&mut self) -> bool {
        if self.slide_index.is_some() || self.tab != "shows" {
            return false;
        }
        self.slide_index = self.first_slide();
        self.slide_index.is_some()
    }

    fn open_next_project_show(&mut self) -> bool {
        if self
            .root
            .as_deref()
            .is_some_and(|root| !Library::open(root).next_item_on_last_slide())
        {
            return false;
        }
        let items: Vec<(String, String)> = self
            .current_project()
            .map(|project| {
                project
                    .shows
                    .iter()
                    .map(|item| (item.id.clone(), item.kind.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut cursor = self.project_item;
        loop {
            let Some(next) = Self::next_project_index(
                &items
                    .iter()
                    .map(|(_, kind)| kind.as_str())
                    .collect::<Vec<_>>(),
                cursor,
            ) else {
                return false;
            };
            if self.open_project_item(next, Some(0)) {
                return true;
            }
            cursor = next;
        }
    }

    fn open_project_item(&mut self, index: usize, slide: Option<usize>) -> bool {
        let Some((id, kind)) = self.current_project().and_then(|project| {
            project
                .shows
                .get(index)
                .map(|item| (item.id.clone(), item.kind.clone()))
        }) else {
            return false;
        };
        self.folder_paths.clear();
        self.folder_index = 0;
        let opened = match kind.as_str() {
            "image" | "video" | "player" | "audio" => self.play_named(&id),
            "folder" => self.open_folder(Path::new(&id)),
            "overlay" => self.show_overlay(&id),
            "show" | "" => {
                if !self.present_show_id(&id) {
                    false
                } else if let Some(slide) = slide {
                    let len = self
                        .current_show()
                        .map(|show| show.ordered_slides().len())
                        .unwrap_or(0);
                    if len > 0 {
                        self.slide_index = Some(slide.min(len - 1));
                        self.apply_slide_background();
                    }
                    true
                } else {
                    true
                }
            }
            _ => false,
        };
        if opened {
            self.project_item = index;
            self.project_picked = true;
        }
        opened
    }

    fn open_folder(&mut self, path: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|file| matches!(media_kind(file), MediaKind::Image | MediaKind::Video))
            .collect();
        files.sort_by(|left, right| Self::folder_file_order(left, right));
        let Some(first) = files.first().cloned() else {
            return false;
        };
        self.pending_cue_completion = None;
        self.folder_paths = files;
        self.folder_index = 0;
        self.show_folder_file(first)
    }

    fn step_folder(&mut self, delta: isize) -> bool {
        if self.folder_paths.is_empty() || delta == 0 {
            return false;
        }
        let folder = self.current_project().is_some_and(|project| {
            project
                .shows
                .get(self.project_item)
                .is_some_and(|item| item.kind == "folder")
        });
        if !folder {
            return false;
        }
        let len = self.folder_paths.len() as isize;
        let current = self.folder_index as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.folder_index = next;
        let path = self.folder_paths[next].clone();
        self.show_folder_file(path)
    }

    fn show_folder_file(&mut self, path: PathBuf) -> bool {
        let kind = media_kind(&path);
        let soundtrack = if kind == MediaKind::Image {
            self.soundtrack_before(&path)
        } else {
            None
        };
        let shown = self.play_path(path.clone());
        if kind == MediaKind::Video {
            self.media_loop = false;
            self.media_muted = false;
        }
        self.slide_hidden = false;
        if let Some(audio) = soundtrack {
            self.queue_audio(audio);
        }
        shown
    }

    /// The audio file that FreeShow starts when it sits immediately before this picture.
    fn soundtrack_before(&self, visual: &Path) -> Option<PathBuf> {
        let parent = visual.parent()?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(parent)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|file| {
                matches!(
                    media_kind(file),
                    MediaKind::Image | MediaKind::Video | MediaKind::Audio
                )
            })
            .collect();
        files.sort_by(|left, right| Self::folder_file_order(left, right));
        let index = files.iter().position(|file| file == visual)?;
        let previous = files.get(index.checked_sub(1)?)?;
        (media_kind(previous) == MediaKind::Audio).then(|| previous.clone())
    }

    fn folder_timer_seconds(&self) -> Option<f64> {
        let path = self.folder_paths.get(self.folder_index)?;
        if media_kind(path) != MediaKind::Image {
            return None;
        }
        let stored = self.current_project().and_then(|project| {
            project
                .shows
                .get(self.project_item)
                .and_then(|item| item.timer_ms)
        });
        Some(stored.map(|millis| millis as f64 / 1000.0).unwrap_or(10.0))
    }

    fn show_overlay(&mut self, id: &str) -> bool {
        if self.overlay_catalog.get(id).is_none() {
            return false;
        }
        if !self.active_overlays.iter().any(|active| active == id) {
            self.active_overlays.push(id.to_string());
        }
        true
    }

    /// The previous slide in this song, the previous picture in a folder, or the previous project item.
    pub fn retreat_show(&mut self) -> bool {
        if self.preferences.out_locked() {
            return false;
        }
        let was_held = self.program_held;
        self.resume_program();
        if self.tab == "scripture" {
            return self.step_verse(-1);
        }
        if self.step_folder(-1) {
            return true;
        }
        let page = self.style_line_count();
        if page > 0 && self.line_index > 0 {
            self.line_index = self.line_index.saturating_sub(page);
            return true;
        }
        if self.reveal_count > 0 {
            self.reveal_count -= 1;
            return true;
        }
        if self.click_revealed {
            self.click_revealed = false;
            return true;
        }
        if self.step_layer_build(false) {
            if was_held {
                self.hold_program();
            }
            return true;
        }
        let index = self.slide_index.unwrap_or(0);
        let (output_id, output_name) = self.audience();
        let previous = self
            .current_show()
            .and_then(|show| show.step_output(index, -1, &output_id, &output_name));
        if let Some(previous) = previous {
            self.step_slide(-1);
            if self.slide_index == Some(previous) {
                if let Some(builds) = &mut self.layer_builds {
                    builds.finish();
                }
                if let Some(total) = self
                    .current_show()
                    .and_then(|show| show.line_reveal_lines(previous))
                {
                    self.reveal_count = total as u32;
                }
                if self
                    .current_show()
                    .is_some_and(|show| show.has_click_reveal(previous))
                {
                    self.click_revealed = true;
                }
                let page = self.style_line_count();
                if page > 0 {
                    let most = self
                        .current_show()
                        .map(|show| show.most_lines(previous))
                        .unwrap_or(0);
                    if most > 0 {
                        let page = page as usize;
                        let pages = most.div_ceil(page);
                        self.line_index = ((pages - 1) * page) as u32;
                    }
                }
                return true;
            }
            return false;
        }
        self.open_previous_project_show()
    }

    fn open_previous_project_show(&mut self) -> bool {
        if self
            .root
            .as_deref()
            .is_some_and(|root| !Library::open(root).next_item_on_last_slide())
        {
            return false;
        }
        let items: Vec<String> = self
            .current_project()
            .map(|project| project.shows.iter().map(|item| item.kind.clone()).collect())
            .unwrap_or_default();
        let mut cursor = self.project_item;
        loop {
            let Some(previous) = Self::previous_project_index(
                &items.iter().map(String::as_str).collect::<Vec<_>>(),
                cursor,
            ) else {
                return false;
            };
            if self.open_project_item(previous, Some(usize::MAX)) {
                return true;
            }
            cursor = previous;
        }
    }

    fn folder_file_order(left: &Path, right: &Path) -> std::cmp::Ordering {
        let name = |file: &Path| {
            file.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        name(left).cmp(&name(right))
    }

    fn previous_project_index(kinds: &[&str], index: usize) -> Option<usize> {
        kinds
            .iter()
            .enumerate()
            .take(index)
            .rev()
            .find(|(_, kind)| **kind != "section")
            .map(|(previous, _)| previous)
    }

    fn next_project_index(kinds: &[&str], index: usize) -> Option<usize> {
        kinds
            .iter()
            .enumerate()
            .skip(index.saturating_add(1))
            .find(|(_, kind)| **kind != "section")
            .map(|(next, _)| next)
    }

    fn current_project(&self) -> Option<freeshow_data::ProjectEntry> {
        let project_id = self.project_id.as_ref()?;
        self.projects
            .projects()
            .into_iter()
            .find(|project| &project.id == project_id)
    }
}

pub(crate) struct NamedTimer {
    id: String,
    name: String,
    start: u64,
    end: u64,
}

impl NamedTimer {
    fn default_countdown() -> Self {
        Self {
            id: "default".into(),
            name: "05:00".into(),
            start: 300,
            end: 0,
        }
    }
}

pub(crate) struct RunningTimer {
    id: String,
    start: u64,
    end: u64,
    mark: u64,
    since: Option<Instant>,
}

impl RunningTimer {
    fn shown(&self) -> u64 {
        let elapsed = self
            .since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        if self.start >= self.end {
            self.mark.saturating_sub(elapsed).max(self.end)
        } else {
            self.mark.saturating_add(elapsed).min(self.end)
        }
    }
}

fn scheduled_show(event: &serde_json::Value) -> Option<String> {
    let kind = event
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if kind == "show" {
        return event
            .get("show")
            .and_then(|value| value.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_string);
    }
    if kind != "action" {
        return None;
    }
    let action = event.get("action")?;
    if action.get("id").and_then(|value| value.as_str()) != Some("start_show") {
        return None;
    }
    action
        .pointer("/data/id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn event_time(event: &serde_json::Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = event.get("from").and_then(|value| value.as_str())?;
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|time| time.and_utc())
}

fn named_timers(settings: &Catalog) -> Vec<NamedTimer> {
    let Some(timers) = settings.get("timers").and_then(|value| value.as_object()) else {
        return vec![NamedTimer::default_countdown()];
    };
    let mut timers: Vec<NamedTimer> = timers
        .iter()
        .filter(|(_, value)| {
            value
                .get("type")
                .and_then(|kind| kind.as_str())
                .unwrap_or("counter")
                == "counter"
        })
        .map(|(id, value)| NamedTimer {
            id: id.clone(),
            name: value
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or(id)
                .to_string(),
            start: value
                .get("start")
                .and_then(|start| start.as_u64())
                .unwrap_or(0),
            end: value.get("end").and_then(|end| end.as_u64()).unwrap_or(0),
        })
        .collect();
    if timers.is_empty() {
        return vec![NamedTimer::default_countdown()];
    }
    timers.sort_by(|left, right| left.name.cmp(&right.name));
    timers
}

fn names_the_file(path: &Path, name: &str) -> bool {
    !name.is_empty()
        && (path == Path::new(name)
            || path.file_name().and_then(|file| file.to_str()) == Some(name)
            || path.ends_with(name))
}

fn unique_name(base: &str, existing: Vec<String>) -> String {
    let taken: Vec<String> = existing.iter().map(|name| name.to_lowercase()).collect();
    if !taken.iter().any(|name| name == &base.to_lowercase()) {
        return base.to_string();
    }
    let mut number = 2;
    loop {
        let candidate = format!("{base} {number}");
        if !taken.iter().any(|name| name == &candidate.to_lowercase()) {
            return candidate;
        }
        number += 1;
    }
}

fn dated_project_name(existing: Vec<String>) -> String {
    let today = chrono::Local::now().format("%d.%m.%y").to_string();
    if !existing.iter().any(|name| name == &today) {
        return today;
    }
    let mut number = 2;
    loop {
        let candidate = format!("{today} ({number})");
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
        number += 1;
    }
}

fn remote_slides(
    show: &ShowFile,
    mut slides: Vec<freeshow_data::SlideView>,
    group_numbers: bool,
) -> Vec<freeshow_remote::RemoteSlide> {
    for slide in &mut slides {
        slide.lines = show.visible_lines_for_slide(&slide.id);
    }
    let labels = slide_rows_for(&slides, group_numbers);
    slides
        .into_iter()
        .zip(labels)
        .map(|(slide, label)| freeshow_remote::RemoteSlide {
            label: label.text,
            lines: slide.lines,
            notes: slide.notes,
            id: slide.id,
            group: slide.group.unwrap_or_default(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideLabel {
    pub text: String,
    pub color: Option<String>,
}

fn slide_rows_for(slides: &[freeshow_data::SlideView], group_numbers: bool) -> Vec<SlideLabel> {
    // Numbers tell apart different slides that share a group name. A chorus sung twice is the
    // same slide, so it keeps one label, as in FreeShow.
    let mut ids = HashMap::<&str, Vec<&str>>::new();
    if group_numbers {
        for slide in slides {
            if let Some(group) = slide.group.as_deref().filter(|group| !group.is_empty()) {
                let seen = ids.entry(group).or_default();
                if !seen.contains(&slide.id.as_str()) {
                    seen.push(&slide.id);
                }
            }
        }
    }
    slides
        .iter()
        .map(|slide| {
            let group_name = slide
                .group
                .as_deref()
                .filter(|group| !group.is_empty())
                .map(|group| match ids.get(group) {
                    Some(seen) if seen.len() > 1 => {
                        let number = seen.iter().position(|id| *id == slide.id).unwrap_or(0) + 1;
                        format!("{group} {number}")
                    }
                    _ => group.to_string(),
                });
            SlideLabel {
                text: slide_label_with_group(slide, group_name.as_deref()),
                color: slide.color.clone(),
            }
        })
        .collect()
}

fn slide_label_with_group(slide: &freeshow_data::SlideView, group: Option<&str>) -> String {
    let text = slide
        .lines
        .iter()
        .find(|line| !line.is_empty())
        .map(String::as_str)
        .unwrap_or("");
    match group {
        Some(group) if !group.is_empty() => {
            if text.is_empty() {
                group.to_string()
            } else {
                format!("{group}: {text}")
            }
        }
        _ if text.is_empty() => "(empty slide)".to_string(),
        _ => text.to_string(),
    }
}

fn take<T>(result: Result<T, freeshow_data::DataError>, warnings: &mut Vec<String>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            warnings.push(err.to_string());
            None
        }
    }
}

pub fn load_show_file(path: &Path) -> Option<ShowFile> {
    ShowFile::load(path).ok()
}

fn file_view(name: &str) -> freeshow_remote::RemoteFile {
    let kind = if name.starts_with("Camera: ") {
        "camera"
    } else {
        match media_kind(Path::new(name)) {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Other => "file",
        }
    };
    freeshow_remote::RemoteFile {
        name: name.to_string(),
        kind: kind.to_string(),
    }
}

fn overlay_views(catalog: &Catalog) -> Vec<freeshow_remote::RemoteOverlay> {
    catalog
        .entries()
        .into_iter()
        .map(|(id, value)| freeshow_remote::RemoteOverlay {
            id,
            name: value
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or("")
                .to_string(),
            category: value
                .get("category")
                .and_then(|category| category.as_str())
                .map(str::to_string),
            items: value
                .get("items")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        })
        .collect()
}

#[derive(Debug, Clone)]
struct EditSnapshot {
    label: String,
    show_id: String,
    show: serde_json::Value,
    selection: EditSelection,
}

struct HistoryEntry {
    snapshot: EditSnapshot,
    opposite_selection: EditSelection,
}

#[derive(Debug, Clone, Default)]
struct EditSelection {
    slide_index: Option<usize>,
    item_index: usize,
    layers: Option<slide_layers::LayerSelection>,
}

impl Session {
    fn edit_snapshot(&self, label: &str) -> Option<EditSnapshot> {
        let index = self.show_index.filter(|&index| index < self.shows.len())?;
        Some(self.show_edit_snapshot(index, label))
    }

    fn show_edit_snapshot(&self, index: usize, label: &str) -> EditSnapshot {
        let show = &self.shows[index];
        EditSnapshot {
            label: label.to_string(),
            show_id: show.id.clone(),
            show: show.show.clone(),
            selection: self.edit_selection(index),
        }
    }

    fn edit_selection(&self, index: usize) -> EditSelection {
        if self.show_index == Some(index) {
            EditSelection {
                slide_index: self.slide_index,
                item_index: self.item_index,
                layers: self.layer_selection.clone(),
            }
        } else {
            EditSelection {
                slide_index: self.shows[index].first_enabled_slide(),
                ..Default::default()
            }
        }
    }

    fn commit_edit(&mut self, index: usize, snapshot: EditSnapshot) {
        self.hold_edited_program(&snapshot.show_id);
        self.history.push(HistoryEntry {
            snapshot,
            opposite_selection: self.edit_selection(index),
        });
        self.redo.clear();
        if self.history.len() > 40 {
            self.history.remove(0);
        }
    }

    fn hold_edited_program(&mut self, show_id: &str) {
        let on_air = matches!(
            self.program_content.borrow().as_ref(),
            Some(crate::live::ProgramContent::Slide { show_id: live, .. }) if live == show_id
        );
        if on_air {
            self.hold_program();
        }
    }

    fn selected_live_slide(&self) -> Option<&serde_json::Value> {
        if self.blanked || self.slide_hidden || self.tab != "shows" || self.preferences.out_locked()
        {
            return None;
        }
        let show = self.current_show()?;
        let index = self.slide_index?;
        let slides = show.ordered_slides();
        let slide_id = &slides.get(index)?.id;
        match self.program_content.borrow().as_ref() {
            Some(crate::live::ProgramContent::Slide {
                show_id,
                slide_id: live_id,
                index: live_index,
                ..
            }) if show_id == &show.id && live_id == slide_id && *live_index == index => {
                show.show.get("slides")?.get(slide_id)
            }
            _ => None,
        }
    }

    pub fn can_update_live_slide(&self) -> bool {
        self.selected_live_slide().is_some_and(|selected| {
            matches!(
                self.program_content.borrow().as_ref(),
                Some(crate::live::ProgramContent::Slide { slide, .. }) if slide.as_ref() != selected
            )
        })
    }

    pub fn update_live_slide(&mut self) -> bool {
        if !self.can_update_live_slide() {
            return false;
        }
        let Some(replacement) = self.selected_live_slide().cloned() else {
            return false;
        };
        let Some(crate::live::ProgramContent::Slide { slide, .. }) = self.program_content.get_mut()
        else {
            return false;
        };
        let now = Instant::now();
        self.slide_countdown =
            SlideCountdown::update(&replacement, self.slide_countdown.as_ref(), now);
        self.layer_builds = self
            .layer_builds
            .as_ref()
            .and_then(|builds| builds.updated(&replacement, now));
        *slide = std::rc::Rc::new(replacement);
        self.hold_program();
        self.live_edited = true;
        true
    }

    fn restore_snapshot(&mut self, entry: &HistoryEntry) -> Result<HistoryEntry, String> {
        let snap = &entry.snapshot;
        let mut targets = self
            .shows
            .iter()
            .enumerate()
            .filter(|(_, show)| show.id == snap.show_id);
        let (index, _) = targets
            .next()
            .ok_or_else(|| "The edited show is no longer in the library".to_string())?;
        if targets.next().is_some() {
            return Err("More than one show has the edited show’s ID".into());
        }
        self.check_pending_show_draft(index)?;
        let show = &self.shows[index];
        let restored = ShowFile {
            id: show.id.clone(),
            show: snap.show.clone(),
            path: show.path.clone(),
        };
        if let Some(path) = &restored.path {
            let saved = ShowFile::load(path).map_err(|err| err.to_string())?;
            let base = self.timer_save_bases.get(&show.id).unwrap_or(&show.show);
            if saved.id != show.id || &saved.show != base {
                return Err("The saved show changed on disk".into());
            }
            restored.save(path).map_err(|err| err.to_string())?;
        }

        let current = std::mem::replace(&mut self.shows[index], restored);
        self.timer_save_bases.remove(&current.id);
        // Each selection is paired with its exact saved layout, including repeated slides.
        let inverse = HistoryEntry {
            snapshot: EditSnapshot {
                label: snap.label.clone(),
                show_id: current.id,
                show: current.show,
                selection: entry.opposite_selection.clone(),
            },
            opposite_selection: snap.selection.clone(),
        };
        self.hold_edited_program(&snap.show_id);
        self.hold_program();
        self.tab = "shows".to_string();
        self.show_index = Some(index);
        self.slide_index = snap.selection.slide_index;
        self.item_index = snap.selection.item_index;
        self.layer_selection.clone_from(&snap.selection.layers);
        Ok(inverse)
    }

    fn replay_edit(&mut self, undo: bool) -> bool {
        let source = if undo {
            &mut self.history
        } else {
            &mut self.redo
        };
        let Some(snapshot) = source.pop() else {
            return false;
        };
        match self.restore_snapshot(&snapshot) {
            Ok(inverse) => {
                let destination = if undo {
                    &mut self.redo
                } else {
                    &mut self.history
                };
                destination.push(inverse);
                self.notice.clear();
                true
            }
            Err(error) => {
                let source = if undo {
                    &mut self.history
                } else {
                    &mut self.redo
                };
                source.push(snapshot);
                let operation = if undo { "Undo" } else { "Redo" };
                self.notice = format!("{operation} failed: {error}");
                false
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.replay_edit(true)
    }

    pub fn redo(&mut self) -> bool {
        self.replay_edit(false)
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[cfg(test)]
    pub fn last_edit_label(&self) -> Option<&str> {
        self.history
            .last()
            .map(|entry| entry.snapshot.label.as_str())
    }

    pub fn toggle_focus_mode(&mut self) -> bool {
        self.focus_mode = !self.focus_mode;
        self.guide_visible = false;
        self.patch_special("focusMode", serde_json::Value::Bool(self.focus_mode));
        true
    }

    pub fn show_guide(&mut self) {
        self.guide_visible = true;
    }

    pub fn dismiss_guide(&mut self) {
        self.guide_visible = false;
        self.patch_special("guideSeen", serde_json::json!(true));
    }

    pub fn guide_text(&self) -> String {
        let mut text = String::from(
            "Welcome to FreeShow\n1. Pick a show from the drawer or your project.\n2. Click a slide to put it on the output. Arrow keys and Page Up/Down step through.\n3. Open Output for the audience and Stage for the foldback. Esc or Clear all blanks the screen; F1 to F4 clear the background, slide, overlays and audio.\nCtrl+F1 opens this guide again. Focus mode (Ctrl+Shift+F) hides the drawer.",
        );
        if let Some(alert) = self.alert_banner() {
            text = format!("{alert}\n\n{text}");
        }
        if self.chrome.profile_locked {
            text.push_str("\n\nProfile login required.");
        }
        text.push_str(
            "\n\nEdit opens the slide editor: clicks then pick slides without going live, and Go shows the one you are editing.",
        );
        text
    }

    pub fn toggle_kiosk(&mut self) -> bool {
        self.kiosk = !self.kiosk;
        true
    }

    pub fn toggle_always_on_top(&mut self) -> bool {
        self.always_on_top = !self.always_on_top;
        true
    }

    pub fn shortcut_text(&self) -> String {
        self.preferences
            .shortcut_list()
            .into_iter()
            .map(|(action, chord)| format!("{action}={chord}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn save_shortcuts(&mut self, text: &str) -> bool {
        let mut map = serde_json::Map::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((action, chord)) = line.split_once('=') else {
                continue;
            };
            let action = action.trim();
            let chord = freeshow_data::normalize_chord(chord);
            if action.is_empty() || chord.is_empty() {
                continue;
            }
            map.insert(action.to_string(), serde_json::Value::String(chord));
        }
        if map.is_empty() {
            return false;
        }
        self.patch_settings(|settings| {
            settings["shortcuts"] = serde_json::Value::Object(map);
        })
    }

    #[cfg(test)]
    pub fn chord_for(&self, action: &str) -> String {
        freeshow_data::normalize_chord(&self.preferences.shortcut(action))
    }

    pub fn perform_shortcut(&mut self, chord: &str) -> bool {
        let chord = freeshow_data::normalize_chord(chord);
        if chord.is_empty() {
            return false;
        }
        let action = self
            .preferences
            .shortcut_list()
            .into_iter()
            .find(|(_, bound)| freeshow_data::normalize_chord(bound) == chord)
            .map(|(action, _)| action)
            .or_else(|| match chord.as_str() {
                // Presentation clickers send page keys.
                "PageDown" | "F5" => Some("next".to_string()),
                "PageUp" => Some("previous".to_string()),
                // FreeShow's fixed clear keys.
                "." => Some("clear".to_string()),
                "F1" => Some("clear_background".to_string()),
                "F2" => Some("clear_slide".to_string()),
                "F3" => Some("clear_overlays".to_string()),
                "F4" => Some("clear_audio".to_string()),
                _ => None,
            });
        let Some(action) = action else {
            return false;
        };
        let clear = match action.as_str() {
            "clear" => Some(freeshow_remote::Command::Clear),
            "clear_background" => Some(freeshow_remote::Command::ClearBackground),
            "clear_slide" => Some(freeshow_remote::Command::ClearSlide),
            "clear_overlays" => Some(freeshow_remote::Command::ClearOverlays),
            "clear_audio" => Some(freeshow_remote::Command::StopAudio),
            _ => None,
        };
        if let Some(command) = clear {
            self.ui_commands.push(command);
            return true;
        }
        let live = match action.as_str() {
            "next" => self.advance_show(),
            "previous" => self.retreat_show(),
            "go" => self.slide_index.is_some() || self.open_first_show(),
            _ => false,
        };
        if live {
            self.go_live();
            self.refresh_output = true;
            return true;
        }
        match action.as_str() {
            "undo" => self.undo(),
            "redo" => self.redo(),
            "focus" => self.toggle_focus_mode(),
            "kiosk" => self.toggle_kiosk(),
            "guide" => {
                if self.guide_visible {
                    self.dismiss_guide();
                } else {
                    self.show_guide();
                }
                true
            }
            "always_on_top" => self.toggle_always_on_top(),
            _ => false,
        }
    }

    fn patch_special(&mut self, key: &str, value: serde_json::Value) -> bool {
        self.patch_settings(|settings| {
            let special = settings.as_object_mut().map(|root| {
                root.entry("special")
                    .or_insert_with(|| serde_json::json!({}))
            });
            if let Some(special) = special.and_then(serde_json::Value::as_object_mut) {
                special.insert(key.to_string(), value);
            }
        })
    }

    /// Edits the library `settings.json` in place, keeping every key this app does not know.
    ///
    /// A file that exists but does not parse is left untouched (returns false) so a
    /// hand-edited or newer-format file is never replaced by a near-empty one. The
    /// write goes through a temp file and rename so a crash cannot truncate it.
    fn patch_settings(&mut self, edit: impl FnOnce(&mut serde_json::Value)) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        let path = root.join("settings.json");
        let mut settings = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value) if value.is_object() => value,
                _ => {
                    tracing::error!("{} is not a JSON object; not changing it", path.display());
                    return false;
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(err) => {
                tracing::error!("could not read {}: {err}", path.display());
                return false;
            }
        };
        edit(&mut settings);
        let Ok(bytes) = serde_json::to_vec_pretty(&settings) else {
            return false;
        };
        let staging = path.with_extension("json.tmp");
        if std::fs::write(&staging, bytes).is_err() || std::fs::rename(&staging, &path).is_err() {
            let _ = std::fs::remove_file(&staging);
            return false;
        }
        if let Some(preferences) = self.reload_preferences() {
            self.preferences = preferences;
        }
        self.refresh_style_look();
        true
    }

    fn reload_preferences(&self) -> Option<FreeShowPreferences> {
        let root = self.root.as_ref()?;
        Some(freeshow_data::Library::open(root).preferences())
    }
}

fn ndi_group(value: &serde_json::Value) -> Option<String> {
    if let Some(name) = value
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return Some(name.to_string());
    }
    value.as_array().and_then(|items| {
        items.iter().find_map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
    })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn special_flag(special: &serde_json::Value, key: &str) -> bool {
    special.get(key).and_then(serde_json::Value::as_bool) == Some(true)
}

fn special_text(special: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        special
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

impl Session {
    pub fn settings_text(&self) -> String {
        crate::operator::settings_text(&self.preferences, &self.chrome)
    }

    pub fn login_profile(&mut self, id: &str, password: &str) -> bool {
        let profiles = crate::operator::profiles_from(&self.preferences.profiles);
        let Some(profile) = profiles
            .iter()
            .find(|profile| profile.id == id || profile.name.eq_ignore_ascii_case(id))
        else {
            return false;
        };
        if !profile.allows_login(password) {
            return false;
        }
        self.chrome.active_profile = Some(profile.id.clone());
        self.chrome.profile_locked = false;
        true
    }

    pub fn logout_profile(&mut self) {
        self.chrome.active_profile = None;
        self.chrome.profile_locked = crate::operator::profiles_from(&self.preferences.profiles)
            .iter()
            .any(|profile| profile.password.is_some());
    }

    pub fn apply_update_alert(&mut self, update: &crate::update::ReleaseUpdate) {
        crate::operator::apply_update_alert(&mut self.chrome, &self.preferences, update);
    }

    pub fn alert_banner(&self) -> Option<&str> {
        self.chrome.update_alert.as_deref()
    }

    /// Weather and RSS fetches that are due, for a background worker. Marks them fetched now
    /// so a slow or failing network waits a full interval instead of retrying every tick.
    pub fn take_feed_request(&mut self, now: std::time::Instant) -> Option<FeedRequest> {
        let mut request = FeedRequest::default();
        if crate::operator::weather_due(
            self.chrome.weather_fetched_at,
            now,
            crate::operator::weather_interval(&self.preferences),
        ) {
            self.chrome.weather_fetched_at = Some(now);
            let items = self.current_weather_items();
            request.weather =
                crate::operator::weather_locations_from(&self.preferences.special, &items)
                    .into_iter()
                    .map(|(lat, lon, _)| (lat, lon))
                    .collect();
        }
        if crate::operator::weather_due(
            self.chrome.rss_fetched_at,
            now,
            crate::operator::rss_interval(&self.preferences),
        ) {
            self.chrome.rss_fetched_at = Some(now);
            request.rss = crate::operator::rss_urls_from(&self.preferences);
            if request.rss.is_empty() {
                self.chrome.rss_items.clear();
            }
        }
        (!request.weather.is_empty() || !request.rss.is_empty()).then_some(request)
    }

    /// Store what a background [`FeedRequest`] fetched. Failed fetches keep the old data.
    pub fn apply_feed_results(&mut self, results: FeedResults) {
        for report in results.weather {
            self.chrome.store_weather(report);
        }
        if let Some(items) = results.rss {
            self.chrome.rss_items = items;
        }
    }

    fn current_weather_items(&self) -> Vec<serde_json::Value> {
        let Some(show) = self.current_show() else {
            return Vec::new();
        };
        let Some(index) = self.slide_index else {
            return Vec::new();
        };
        let ordered = show.ordered_slides();
        let Some(slide) = ordered.get(index) else {
            return Vec::new();
        };
        show.show
            .pointer(&format!("/slides/{}/items", slide.id))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    pub fn inspector_text(&self) -> String {
        let Some(show) = self.current_show() else {
            return "inspector=(no show)".into();
        };
        let Some(index) = self.slide_index else {
            return "inspector=(no slide)".into();
        };
        let ordered = show.ordered_slides();
        let Some(slide) = ordered.get(index) else {
            return "inspector=(no slide)".into();
        };
        let typography = show.item_typography(&slide.id, self.item_index);
        format!(
            "item={}\nfontSize={}px\nfontFamily={}\nbold={}\ncolor={}\nalign={}\nboxes={}\nslide={}",
            self.item_index,
            typography.font_px,
            typography.family,
            typography.bold,
            typography.color,
            typography.align,
            show.item_count(&slide.id),
            slide.id
        )
    }

    pub fn set_stage_item_style(&mut self, id: &str, style: &str) -> bool {
        let Some(index) = self.stage.items.iter().position(|item| item.id == id) else {
            return false;
        };
        let previous = std::mem::replace(&mut self.stage.items[index].style, style.into());
        if self.save_stage_layout() {
            return true;
        }
        self.stage.items[index].style = previous;
        false
    }

    pub fn add_stage_item(&mut self, kind: &str, style: &str) -> Option<String> {
        let (kind_name, offset) = match kind {
            "next" => ("slide", 1),
            other => (other, 0),
        };
        let id = (1..)
            .map(|_| freeshow_data::new_id())
            .find(|id| self.stage.items.iter().all(|item| &item.id != id))?;
        let kind = match kind_name {
            "notes" => freeshow_data::StageItemKind::Notes,
            "clock" => freeshow_data::StageItemKind::Clock,
            "timer" => freeshow_data::StageItemKind::Timer,
            "text" => freeshow_data::StageItemKind::Text,
            "media" => freeshow_data::StageItemKind::Media,
            "tracker" => freeshow_data::StageItemKind::SlideTracker,
            "variable" => freeshow_data::StageItemKind::Variable,
            "qr" => freeshow_data::StageItemKind::QrCode,
            "output" => freeshow_data::StageItemKind::CurrentOutput,
            "camera" => freeshow_data::StageItemKind::Camera,
            "chart" => freeshow_data::StageItemKind::Chart,
            "web" => freeshow_data::StageItemKind::Web,
            _ => freeshow_data::StageItemKind::Slide,
        };
        self.stage.items.push(freeshow_data::StageItemSpec {
            kind,
            enabled: true,
            id: id.clone(),
            style: style.to_string(),
            align: String::new(),
            offset,
            line_count: None,
            text_box: None,
            invert_items: false,
            label: String::new(),
            lines: if kind == freeshow_data::StageItemKind::Text {
                vec!["Text".into()]
            } else {
                Vec::new()
            },
            variable_id: None,
            timer_id: None,
            src: None,
            device_name: None,
            device_type: None,
            output_source: None,
            show_output_label: false,
            include_media: false,
            fit: None,
            flip_x: false,
            flip_y: false,
            qr: (kind == freeshow_data::StageItemKind::QrCode).then(freeshow_data::QrItem::default),
            url: None,
            chart: (kind == freeshow_data::StageItemKind::Chart)
                .then(freeshow_data::ChartItem::default),
            clock: freeshow_data::StageClockOptions::default(),
            tracker: freeshow_data::StageTrackerOptions::default(),
        });
        if self.save_stage_layout() {
            return Some(id);
        }
        self.stage.items.pop();
        None
    }

    fn save_stage_layout(&self) -> bool {
        let Some(root) = &self.root else {
            return true;
        };
        Library::open(root)
            .save_stage_layout(self.stage_layout_id.as_deref(), &self.stage)
            .is_ok()
    }

    pub fn network_sources(&self) -> Vec<freeshow_ndi::NetworkSource> {
        freeshow_ndi::network_sources()
    }
}

fn rtmp_targets_for(id: &str, output: &serde_json::Value, auto: bool) -> Vec<RtmpTarget> {
    let data = &output["rtmpData"];
    let streaming = data["streaming"].as_bool() == Some(true);
    let mut targets = Vec::new();
    if let Some(list) = data["destinations"].as_array() {
        for dest in list {
            if dest["enabled"].as_bool() == Some(false) {
                continue;
            }
            let Some(url) = rtmp_url(dest["url"].as_str(), dest["key"].as_str()) else {
                continue;
            };
            if streaming || auto || dest["file"].as_bool() == Some(true) {
                targets.push(RtmpTarget {
                    id: format!("{id}-{}", dest["id"].as_str().unwrap_or("dest")),
                    url,
                    file: dest["file"].as_bool() == Some(true),
                });
            }
        }
    }
    if targets.is_empty()
        && let Some(url) = rtmp_url(data["url"].as_str(), data["key"].as_str())
        && (streaming || auto)
    {
        targets.push(RtmpTarget {
            id: id.to_string(),
            url,
            file: false,
        });
    }
    targets
}

fn rtmp_url(base: Option<&str>, key: Option<&str>) -> Option<String> {
    let base = base.map(str::trim).filter(|url| !url.is_empty())?;
    let key = key.map(str::trim).filter(|key| !key.is_empty());
    let url = match key {
        Some(key) if !base.contains(key) && !base.contains('?') => {
            let slash = if base.ends_with('/') { "" } else { "/" };
            format!("{base}{slash}{key}")
        }
        _ => base.to_string(),
    };
    (url.starts_with("rtmp://") || url.starts_with("rtmps://") || PathBuf::from(&url).is_absolute())
        .then_some(url)
}

fn scripture_catalog(bibles: &[BibleFile]) -> Arc<Vec<freeshow_remote::RemoteBible>> {
    Arc::new(
        bibles
            .iter()
            .map(|bible| freeshow_remote::RemoteBible {
                id: bible.id.clone(),
                name: bible.name(),
                books: bible.outline().into_iter().map(remote_book).collect(),
            })
            .collect(),
    )
}

fn remote_book(book: OutlinedBook) -> freeshow_remote::RemoteBook {
    freeshow_remote::RemoteBook {
        number: book.number,
        name: book.name,
        chapters: book
            .chapters
            .into_iter()
            .map(|chapter| freeshow_remote::RemoteChapter {
                number: chapter.number,
                verses: chapter
                    .verses
                    .into_iter()
                    .map(|verse| freeshow_remote::RemoteVerse {
                        number: verse.number,
                        text: verse.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloaded_lesson_media_presents_from_the_local_library() {
        use std::io::{Read, Write};
        let root =
            std::env::temp_dir().join(format!("freeshow-present-lesson-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            // Generated green PNG and silent H.264 clip; no external network.
            for body in [
                include_bytes!("../../freeshow-data/tests/fixtures/lesson-cover.png").as_slice(),
                include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4").as_slice(),
            ] {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("lesson test listener: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let lesson = serde_json::json!({"lessonName":"Sunday Lesson", "lessonImage":format!("http://{address}/cover.png"), "messages":[{"files":[{"name":"Story","url":format!("http://{address}/story.mp4")}]}]});
        freeshow_churchapps::import_lesson_document(&root, &lesson).unwrap();
        server.join().unwrap();
        let mut session = Session::open(root.clone());
        let id = session.shows[0].id.clone();
        assert!(session.present_show_id(&id));
        let raster = session.underlay.as_ref().expect("lesson image must decode");
        assert_eq!(raster.rgba, [0, 255, 0, 255]);
        assert_eq!(session.projects.projects()[0].name, "Lessons.church");
        assert!(
            session
                .background_path
                .as_ref()
                .unwrap()
                .starts_with(root.join("Media").canonicalize().unwrap())
        );
        session.select_slide(1);
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        let Some(PendingPlay::Video(path)) = session.pending_play.take() else {
            panic!("lesson must queue the downloaded video");
        };
        let mut player = freeshow_media::VideoPlayer::open_with_network(&path, &[], &[]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let frame = loop {
            if let Some(frame) = player.next_frame() {
                break frame;
            }
            assert!(Instant::now() < deadline, "downloaded video did not decode");
        };
        let center = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
        assert!(
            frame.rgba[center] < 15 && frame.rgba[center + 1] > 240 && frame.rgba[center + 2] < 15
        );
        drop(player);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_propresenter_bundle_presents_lyrics_over_its_copied_background() {
        let root =
            std::env::temp_dir().join(format!("freeshow-present-probundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Imports")).unwrap();
        // Hand-authored Pro6 XML and a generated green PNG, zipped together.
        std::fs::write(
            root.join("Imports/Sunday.proBundle"),
            include_bytes!("../../freeshow-data/tests/fixtures/pro6-background.proBundle"),
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.warnings.is_empty(), "{:?}", session.warnings);
        assert_eq!(session.shows.len(), 1);
        let id = session.shows[0].id.clone();
        assert!(session.present_show_id(&id));
        assert_eq!(
            session.current_show().unwrap().ordered_slides()[0].lines,
            ["Imported words"]
        );
        assert!(!session.slide_hidden);
        let raster = session
            .underlay
            .as_ref()
            .expect("bundled PNG must decode for presentation");
        assert_eq!((raster.width, raster.height), (1, 1));
        assert_eq!(raster.rgba, [0, 255, 0, 255]);
        assert!(
            session
                .background_path
                .as_ref()
                .unwrap()
                .starts_with(root.join("Media").canonicalize().unwrap())
        );
        session.select_slide(1);
        assert_eq!(
            session.current_show().unwrap().ordered_slides()[1].lines,
            ["Second slide"]
        );
        assert!(session.underlay.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_routes_keep_channel_gain_file_volume_and_output_identity() {
        let root =
            std::env::temp_dir().join(format!("freeshow-session-network-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("settings.json"),serde_json::json!({
            "outputs":{"screen":{"active":true,"enabled":true},"ndi":{"ndi":true,"enabled":true},"stream":{"rtmp":true,"enabled":true}},
            "audioChannelsData":{"streaming":{"volume":0.5}},
            "audioRouting":{"connections":[
                {"from":"drawer_audio","to":"main"},{"from":"main","to":"speaker_default"},
                {"from":"drawer_audio","to":"streaming"},{"from":"output_win_sub_screen","to":"streaming"},
                {"from":"streaming","to":"network_sub_stream"},{"from":"streaming","to":"network_sub_ndi"}
            ]}}).to_string()).unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.network_output("ndi").as_deref(), Some("ndi"));
        assert_eq!(
            session.ndi_output(),
            Some(("ndi".into(), "FreeShow NDI".into()))
        );
        assert_eq!(session.network_output("rtmp").as_deref(), Some("stream"));
        session.volume = 0.0;
        session.video_volume = 0.4;
        assert!(
            session
                .audio_network_routes()
                .iter()
                .all(|route| route.gain == 0.5)
        );
        assert!(
            session
                .video_network_routes()
                .iter()
                .all(|route| route.gain == 0.2)
        );
        session.apply_remote(freeshow_remote::Command::MuteChannel {
            id: "streaming".into(),
            muted: true,
        });
        assert!(
            session
                .audio_network_routes()
                .iter()
                .all(|route| route.gain == 0.0)
        );
        assert!(
            session
                .video_network_routes()
                .iter()
                .all(|route| route.gain == 0.0)
        );
        assert_eq!(session.audio_routes()[0].gain, 0.0);
        let path = root.join("settings.json");
        let mut settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        settings["outputs"]["ndi"]["ndiData"] =
            serde_json::json!({"name":"My NDI","groups":"Private"});
        std::fs::write(&path, settings.to_string()).unwrap();
        assert_eq!(session.ndi_output(), Some(("ndi".into(), "My NDI".into())));
        assert_eq!(session.ndi_outputs()[0].grouped_name(), "My NDI [Private]");
        settings["outputs"]["ndi"]["ndiData"]["groups"] = serde_json::json!(["Private"]);
        std::fs::write(&path, settings.to_string()).unwrap();
        assert_eq!(session.ndi_outputs()[0].group.as_deref(), Some("Private"));
        settings["outputs"]["ndi"]["ndiData"]["groups"] = serde_json::json!("");
        std::fs::write(&path, settings.to_string()).unwrap();
        assert_eq!(session.ndi_output(), Some(("ndi".into(), "My NDI".into())));
        assert!(session.ndi_outputs()[0].group.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ndi_outputs_include_grouped_and_stage_pictures() {
        let root =
            std::env::temp_dir().join(format!("freeshow-session-ndi-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            serde_json::json!({
                "outputs": {
                    "main": {
                        "name": "Main",
                        "ndi": true,
                        "enabled": true,
                        "ndiData": {"name": "Sanctuary"}
                    },
                    "overflow": {
                        "name": "Overflow",
                        "ndi": true,
                        "enabled": true
                    },
                    "grouped": {
                        "name": "Private",
                        "ndi": true,
                        "enabled": true,
                        "ndiData": {"name": "Private NDI", "groups": "Staff"}
                    },
                    "off": {"name": "Off", "ndi": true, "enabled": false},
                    "stage": {
                        "name": "Stage",
                        "ndi": true,
                        "enabled": true,
                        "stageOutput": "default",
                        "ndiData": {"name": "Stage NDI"}
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let session = Session::open(root.clone());
        assert_eq!(
            session.ndi_outputs(),
            vec![
                NdiTarget {
                    id: "grouped".into(),
                    name: "Private NDI".into(),
                    stage: false,
                    group: Some("Staff".into()),
                },
                NdiTarget {
                    id: "main".into(),
                    name: "Sanctuary".into(),
                    stage: false,
                    group: None,
                },
                NdiTarget {
                    id: "overflow".into(),
                    name: "FreeShow NDI - Overflow".into(),
                    stage: false,
                    group: None,
                },
                NdiTarget {
                    id: "stage".into(),
                    name: "Stage NDI".into(),
                    stage: true,
                    group: None,
                },
            ]
        );
        assert_eq!(
            session.ndi_output(),
            Some(("grouped".into(), "Private NDI".into()))
        );
        assert_eq!(
            session.ndi_outputs()[0].grouped_name(),
            "Private NDI [Staff]"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rtmp_destinations_start_from_auto_flag_and_legacy_url() {
        let root =
            std::env::temp_dir().join(format!("freeshow-session-rtmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            serde_json::json!({
                "special": {
                    "autoRtmp": true,
                    "virtualCamera": true,
                    "v4l2Device": "/dev/video42",
                    "blackmagic": true,
                    "decklinkDevice": "1",
                    "spout": true,
                    "spoutName": "Lobby",
                    "ndiInput": "Cam 1"
                },
                "outputs": {
                    "main": {
                        "name": "Main",
                        "rtmp": true,
                        "enabled": true,
                        "rtmpData": {
                            "destinations": [
                                {
                                    "id": "yt",
                                    "url": "rtmp://live.example/app",
                                    "key": "secret"
                                },
                                {
                                    "id": "off",
                                    "enabled": false,
                                    "url": "rtmp://ignored/app",
                                    "key": "no"
                                }
                            ]
                        }
                    },
                    "legacy": {
                        "name": "Legacy",
                        "rtmp": true,
                        "enabled": true,
                        "rtmpData": {
                            "streaming": true,
                            "url": "rtmps://backup.example/live",
                            "key": "old"
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let session = Session::open(root.clone());
        assert_eq!(
            session.rtmp_destinations(),
            vec![
                RtmpTarget {
                    id: "legacy".into(),
                    url: "rtmps://backup.example/live/old".into(),
                    file: false,
                },
                RtmpTarget {
                    id: "main-yt".into(),
                    url: "rtmp://live.example/app/secret".into(),
                    file: false,
                },
            ]
        );
        assert_eq!(
            session.virtual_camera_device().as_deref(),
            Some("/dev/video42")
        );
        assert_eq!(session.blackmagic_device(), Some(1));
        assert_eq!(session.capture_share_name().as_deref(), Some("Lobby"));
        assert_eq!(
            session.network_receive(),
            Some((freeshow_ndi::SourceKind::Ndi, "Cam 1".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn omt_and_webrtc_outputs_use_stored_names_and_whip_urls() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-session-omt-webrtc-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            serde_json::json!({
                "outputs": {
                    "main": {
                        "name": "Main",
                        "omt": true,
                        "webrtc": true,
                        "enabled": true,
                        "omtData": {"name": "Sanctuary OMT"},
                        "webrtcData": {
                            "url": "https://whip.example/main",
                            "token": "secret"
                        }
                    },
                    "stage": {
                        "name": "Stage",
                        "omt": true,
                        "webrtc": true,
                        "enabled": true,
                        "stageOutput": "default",
                        "webrtcData": {"url": " http://127.0.0.1:8080/whip "}
                    },
                    "empty": {
                        "name": "Empty",
                        "webrtc": true,
                        "enabled": true,
                        "webrtcData": {"url": ""}
                    },
                    "off": {"name": "Off", "omt": true, "webrtc": true, "enabled": false}
                }
            })
            .to_string(),
        )
        .unwrap();
        let session = Session::open(root.clone());
        assert_eq!(
            session.omt_outputs(),
            vec![
                OmtTarget {
                    id: "main".into(),
                    name: "Sanctuary OMT".into(),
                    stage: false
                },
                OmtTarget {
                    id: "stage".into(),
                    name: "FreeShow OMT - Stage".into(),
                    stage: true
                },
            ]
        );
        assert_eq!(
            session.webrtc_outputs(),
            vec![
                WebrtcTarget {
                    id: "main".into(),
                    url: "https://whip.example/main".into(),
                    token: Some("secret".into()),
                    stage: false
                },
                WebrtcTarget {
                    id: "stage".into(),
                    url: "http://127.0.0.1:8080/whip".into(),
                    token: None,
                    stage: true
                },
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn video_routes_use_the_audience_output_and_keep_file_gain_when_main_changes() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-video-routing-session-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings = serde_json::json!({
            "outputs":{"screen":{"name":"Audience", "active":true, "enabled":true}},
            "audioRouting":{"channels":[{"id":"main"},{"id":"cue"}], "connections":[
                {"from":"drawer_audio", "to":"main"},
                {"from":"main", "to":"speaker_default"},
                {"from":"output_win_sub_screen", "to":"cue"},
                {"from":"cue", "to":"speaker_sub_stage"}
            ]},
            "audioChannelsData":{"main":{"volume":0.8}, "cue":{"volume":0.5}}
        });
        std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
        let mut session = Session::open(root.clone());
        session
            .audio_device_map
            .insert("speaker_sub_stage".into(), "Stage Speakers".into());
        session.video_volume = 0.4;
        assert_eq!(
            session.audio_routes(),
            vec![SpeakerRoute::stereo(None, 0.8)]
        );
        assert_eq!(
            session.video_audio_routes(),
            vec![SpeakerRoute {
                output_channel: Some(0),
                ..SpeakerRoute::stereo(Some("Stage Speakers".into()), 0.2)
            }]
        );
        session.apply_remote(freeshow_remote::Command::SetVolume { thousandths: 0 });
        assert_eq!(session.video_audio_routes()[0].gain, 0.2);
        session.apply_remote(freeshow_remote::Command::MuteChannel {
            id: "cue".into(),
            muted: true,
        });
        assert_eq!(session.video_audio_routes()[0].gain, 0.0);
        session.apply_remote(freeshow_remote::Command::MuteChannel {
            id: "cue".into(),
            muted: false,
        });
        assert_eq!(session.video_audio_routes()[0].gain, 0.2);
        let mut settings = settings;
        settings["outputs"]["screen"]["enabled"] = serde_json::json!(false);
        std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
        assert_eq!(session.video_audio_routes()[0].gain, 0.0);
        let mut no_library = Session::empty();
        no_library.volume = 0.5;
        no_library.video_volume = 0.4;
        assert_eq!(
            no_library.video_audio_routes(),
            vec![SpeakerRoute::stereo(None, 0.2)]
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn routed_audio_obeys_remote_channel_gain_without_leaking_to_main() {
        let root =
            std::env::temp_dir().join(format!("freeshow-routing-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            serde_json::json!({
                "keep":"unchanged",
                "audioRouting":{"channels":[{"id":"main"},{"id":"cue"}], "connections":[
                    {"from":"drawer_audio", "to":"cue"}, {"from":"cue", "to":"speaker_sub_hash"}
                ]},
                "audioChannelsData":{"main":{"volume":0.2}, "cue":{"volume":0.5}}
            })
            .to_string(),
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        session
            .audio_device_map
            .insert("speaker_sub_hash".into(), "Stage Speakers".into());
        assert_eq!(
            session.audio_routes(),
            vec![SpeakerRoute {
                output_channel: Some(0),
                ..SpeakerRoute::stereo(Some("Stage Speakers".into()), 0.5)
            }]
        );
        session.apply_remote(freeshow_remote::Command::SetVolume { thousandths: 0 });
        assert_eq!(session.audio_routes()[0].gain, 0.5);
        session.apply_remote(freeshow_remote::Command::SetChannelVolume {
            id: "cue".into(),
            thousandths: 250,
        });
        assert_eq!(session.audio_routes()[0].gain, 0.25);
        session.apply_remote(freeshow_remote::Command::MuteChannel {
            id: "cue".into(),
            muted: true,
        });
        assert_eq!(session.audio_routes()[0].gain, 0.0);
        session.apply_remote(freeshow_remote::Command::MuteChannel {
            id: "cue".into(),
            muted: false,
        });
        assert_eq!(session.audio_routes()[0].gain, 0.25);
        let mixer = session.to_snapshot().audio_mixer;
        assert_eq!(mixer["audioChannelsData"]["cue"]["volume"], 0.25);
        assert_eq!(mixer["audioRouting"]["channels"][1]["id"], "cue");
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(stored["keep"], "unchanged");
        assert_eq!(
            Session::open(root.clone()).to_snapshot().audio_mixer["audioChannelsData"]["cue"]["isMuted"],
            false
        );
        std::fs::write(root.join("settings.json"), "broken json").unwrap();
        assert!(!Library::open(&root).set_channel_volume("cue", 1.0));
        assert_eq!(
            std::fs::read_to_string(root.join("settings.json")).unwrap(),
            "broken json"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_show_and_project_land_in_the_library() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        assert_eq!(session.shows.len(), 1);
        assert_eq!(session.shows[0].name(), "Untitled");
        assert_eq!(session.project_labels(), vec!["Untitled".to_string()]);
        assert!(session.add_slide());
        assert_eq!(session.slide_labels().len(), 2);

        let again = Session::open(root.clone());
        assert_eq!(again.shows.len(), 1);
        assert_eq!(again.shows[0].ordered_slides().len(), 2);
        assert_eq!(again.projects.projects().len(), 1);
        assert!(again.project_title().contains('.'));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn slide_labels_number_repeated_groups_from_library_settings() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-group-numbers-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_group("Verse"));
        assert!(session.add_slide());
        session.slide_index = Some(1);
        assert!(session.save_group("Verse"));
        assert!(session.save_lyrics("second"));
        session.slide_index = Some(0);
        assert!(session.save_lyrics("first"));
        assert_eq!(
            session.slide_labels(),
            vec!["Verse 1: first".to_string(), "Verse 2: second".to_string()]
        );
        std::fs::write(root.join("settings.json"), r#"{"groupNumbers":false}"#).unwrap();
        let mut muted = Session::open(root.clone());
        muted.select_drawer(0);
        assert_eq!(
            muted.slide_labels(),
            vec!["Verse: first".to_string(), "Verse: second".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn slide_grid_uses_group_colors_and_column_count() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-grid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"fullColors":true,"slidesOptions":{"columns":3,"mode":"grid"}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("first"));
        assert!(session.save_group("Verse"));
        assert!(session.add_slide());
        session.slide_index = Some(1);
        assert!(session.save_lyrics("second"));
        assert!(session.save_group("Chorus"));
        let ids: Vec<String> = session.shows[0]
            .ordered_slides()
            .iter()
            .map(|slide| slide.id.clone())
            .collect();
        session.shows[0].show["slides"][&ids[0]]["color"] = serde_json::json!("#ff0000");
        session.shows[0].show["slides"][&ids[1]]["color"] = serde_json::json!("#00ff00");
        assert!(session.preferences.full_colors());
        assert_eq!(session.slides_list_columns(), 3);
        assert_eq!(
            session.slide_rows(),
            vec![
                SlideLabel {
                    text: "Verse: first".to_string(),
                    color: Some("#ff0000".to_string()),
                },
                SlideLabel {
                    text: "Chorus: second".to_string(),
                    color: Some("#00ff00".to_string()),
                },
            ]
        );
        std::fs::write(
            root.join("settings.json"),
            r#"{"fullColors":false,"slidesOptions":{"columns":4,"mode":"list"}}"#,
        )
        .unwrap();
        let listed = Session::open(root.clone());
        assert_eq!(listed.slides_list_columns(), 1);
        assert!(!listed.preferences.full_colors());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn undo_and_redo_restore_edited_lyrics() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-undo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(session.save_lyrics("first"));
        assert!(session.save_lyrics("second"));
        assert!(session.can_undo());
        assert_eq!(session.last_edit_label(), Some("Edit lyrics"));
        assert!(session.undo());
        assert_eq!(session.lyrics(), "first");
        assert!(session.can_redo());
        assert!(session.redo());
        assert_eq!(session.lyrics(), "second");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn undo_and_redo_target_the_edited_show_and_preserve_history_on_write_failure() {
        let root = std::env::temp_dir().join(format!("freeshow-cross-show-undo-{}", new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("a", "Alpha", "First"));
        assert!(session.create_show_with("b", "Bravo", "Other show"));
        let first = session
            .shows
            .iter()
            .position(|show| show.id == "a")
            .unwrap();
        let second = session
            .shows
            .iter()
            .position(|show| show.id == "b")
            .unwrap();
        let first_path = session.shows[first].path.clone().unwrap();
        let second_path = session.shows[second].path.clone().unwrap();
        session.select_drawer(first);
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        assert!(session.save_lyrics("Second"));
        let edited = session.shows[first].show.clone();
        let other = session.shows[second].show.clone();
        session.select_drawer(second);

        let mut duplicate = session.shows[second].clone();
        duplicate.id = "a".into();
        duplicate.path = None;
        session.shows.insert(0, duplicate);
        session.show_index = Some(second + 1);
        assert!(!session.undo());
        assert!(session.notice.contains("More than one show"));
        session.shows.remove(0);
        session.show_index = Some(second);

        let blocked = first_path.with_extension("show.tmp");
        std::fs::create_dir(&blocked).unwrap();
        assert!(!session.undo());
        assert!(session.notice.contains("Undo failed"));
        assert_eq!(session.show_index, Some(second));
        assert_eq!(session.shows[first].show, edited);
        assert_eq!(ShowFile::load(&first_path).unwrap().show, edited);
        assert_eq!((session.history.len(), session.redo.len()), (1, 0));

        std::fs::remove_dir(&blocked).unwrap();
        session.shows.swap(first, second);
        session.show_index = Some(first);
        assert!(session.undo());
        assert_eq!(session.current_show().unwrap().id, "a");
        assert_eq!(session.lyrics(), "First");
        let undone = session.current_show().unwrap().show.clone();
        session.select_drawer(first);

        std::fs::create_dir(&blocked).unwrap();
        assert!(!session.redo());
        assert!(session.notice.contains("Redo failed"));
        assert_eq!(session.current_show().unwrap().id, "b");
        assert_eq!(session.shows[second].show, undone);
        assert_eq!(ShowFile::load(&first_path).unwrap().show, undone);
        assert_eq!((session.history.len(), session.redo.len()), (0, 1));

        std::fs::remove_dir(&blocked).unwrap();
        assert!(session.redo());
        assert_eq!(session.current_show().unwrap().id, "a");
        assert_eq!(session.lyrics(), "Second");
        assert_eq!(ShowFile::load(&first_path).unwrap().show, edited);
        assert_eq!(ShowFile::load(&second_path).unwrap().show, other);
        assert_eq!(crate::live::audience_frame(&session), on_air);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clear_layers_one_at_a_time_and_mark_the_live_slide() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-clear-layers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "One\n\nTwo"));
        session.select_slide(1);
        session.replace_underlay(Some(Raster {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
        }));
        let _ = crate::live::audience_frame(&session);
        assert_eq!(session.live_slide_index(), Some(1));

        session.apply_remote(freeshow_remote::Command::ClearBackground);
        assert!(session.underlay.is_none());
        assert!(matches!(session.pending_play, Some(PendingPlay::StopVideo)));
        assert_eq!(session.live_slide_index(), Some(1), "the slide stays up");

        session.apply_remote(freeshow_remote::Command::ClearSlide);
        assert_eq!(session.live_slide_index(), None);
        assert!(!session.blanked, "only the slide layer went away");

        // Picking a slide to edit leaves the cleared output alone.
        session.select_slide_for_edit(0);
        assert_eq!(session.slide_index, Some(0));
        assert!(session.slide_hidden);

        // The open show stays listed while another drawer tab is browsed.
        session.tab = "media".into();
        assert_eq!(session.slide_rows().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn live_shortcuts_ask_for_an_output_refresh() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-live-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "One\n\nTwo\n\nThree"));
        session.select_slide(0);
        session.refresh_output = false;
        assert!(session.perform_shortcut("Right"));
        assert_eq!(session.slide_index, Some(1));
        assert!(session.refresh_output);
        session.refresh_output = false;
        assert!(session.perform_shortcut("PageDown"));
        assert_eq!(session.slide_index, Some(2));
        assert!(session.perform_shortcut("PageUp"));
        assert_eq!(session.slide_index, Some(1));
        assert!(session.perform_shortcut("Escape"));
        assert_eq!(session.ui_commands, vec![freeshow_remote::Command::Clear]);
        for command in std::mem::take(&mut session.ui_commands) {
            session.apply_remote(command);
        }
        assert!(session.blanked);
        assert!(session.perform_shortcut(" "));
        assert!(!session.blanked);
        assert!(session.refresh_output);
        // FreeShow's fixed keys clear one layer each, and F5 moves on.
        for key in ["F1", "F2", "F3", "F4", "."] {
            assert!(session.perform_shortcut(key), "{key}");
        }
        assert_eq!(
            std::mem::take(&mut session.ui_commands),
            vec![
                freeshow_remote::Command::ClearBackground,
                freeshow_remote::Command::ClearSlide,
                freeshow_remote::Command::ClearOverlays,
                freeshow_remote::Command::StopAudio,
                freeshow_remote::Command::Clear,
            ]
        );
        let before = session.slide_index;
        assert!(session.perform_shortcut("F5"));
        assert_ne!(session.slide_index, before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shortcuts_focus_guide_and_kiosk_toggle() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-shortcuts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.guide_visible);
        assert!(session.guide_text().contains("Welcome"));
        assert!(session.shortcut_text().contains("undo="));
        session.dismiss_guide();
        assert!(!session.guide_visible);
        assert!(session.preferences.guide_seen());
        assert!(session.toggle_focus_mode());
        assert!(session.focus_mode);
        assert!(session.save_shortcuts("undo=Ctrl+Shift+Z\ngo=Enter\n"));
        assert_eq!(session.chord_for("undo"), "Ctrl+Shift+Z");
        assert_eq!(session.chord_for("go"), "Enter");
        assert!(session.perform_shortcut("Ctrl+Shift+F"));
        assert!(!session.focus_mode);
        assert!(session.perform_shortcut("F11"));
        assert!(session.kiosk);
        assert!(session.always_on_top);
        assert!(session.perform_shortcut("Ctrl+Shift+A"));
        assert!(!session.always_on_top);
        session.tab = "functions".to_string();
        let functions = session.drawer();
        assert!(functions.contains(&"Save layout".to_string()));
        assert!(functions.contains(&"Window layouts".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scenes_watermark_variables_and_draw_apply_on_output() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-scenes-draw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("Config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{
                "special": {"guideSeen": true, "watermark": true},
                "activeScenes": {"default": ["warm"]}
            }"#,
        )
        .unwrap();
        std::fs::write(
            config.join("overlays.json"),
            r#"{
                "watermark": {
                    "name": "Watermark",
                    "items": [{
                        "style": "top:0px;left:0px;height:80px;width:200px;",
                        "lines": [{"text": [{"value": "WM", "style": "font-size:20px;color:#F0008C;"}]}]
                    }]
                },
                "logo": {"name": "Logo"}
            }"#,
        )
        .unwrap();
        std::fs::write(
            config.join("settings_synced.json"),
            r##"{
                "variables": {"speaker": {"name": "speaker", "text": "Alex"}},
                "scenes": {
                    "warm": {
                        "name": "Warm",
                        "content": {"overlays": ["logo"]}
                    }
                },
                "interactions": {
                    "quiz": {
                        "name": "Welcome",
                        "inputs": [{"type": "text", "question": "What is your name?"}]
                    }
                },
                "drawSettings": {
                    "paint": {
                        "color": "#ffff00",
                        "size": 20,
                        "lines": [{"x": 100, "y": 100, "size": 20, "color": "#ffff00"}]
                    }
                }
            }"##,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.active_scene.as_deref(), Some("warm"));
        assert!(session.active_overlays.contains(&"watermark".to_string()));
        assert!(session.active_overlays.contains(&"logo".to_string()));
        assert_eq!(session.draw_strokes.len(), 1);
        let frame = crate::live::audience_frame(&session);
        let dot = frame.pixel(100, 100);
        assert_eq!((dot.r, dot.g, dot.b), (255, 255, 0));
        assert!(session.variable_text().contains("speaker=Alex"));
        assert!(session.save_variables("speaker=Jordan\n"));
        assert!(
            freeshow_data::Library::open(&root)
                .variables()
                .iter()
                .any(|(name, value)| name.contains("speaker") && value == "Jordan")
        );
        assert!(session.start_interaction("quiz"));
        let lines = session.interaction_lines();
        assert!(lines.iter().any(|line| line.contains("What is your name?")));
        assert!(session.toggle_watermark());
        assert!(!session.preferences.watermark_enabled());
        assert!(!session.active_overlays.contains(&"watermark".to_string()));
        session.clear_drawing();
        assert!(session.draw_strokes.is_empty());
        session.add_draw_point(200.0, 80.0, 16.0, "#00ff00");
        let painted = crate::live::audience_frame(&session);
        let dot = painted.pixel(200, 80);
        assert_eq!((dot.r, dot.g, dot.b), (0, 255, 0));
        session.tab = "functions".to_string();
        let functions = session.drawer();
        assert!(functions.contains(&"Watermark".to_string()));
        assert!(functions.contains(&"Variables".to_string()));
        assert!(functions.contains(&"Clear drawing".to_string()));
        assert!(functions.contains(&"End interaction".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn player_effects_audio_and_timeline_drive_the_output() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-player-timeline-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("Config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r##"{
                "special": {"guideSeen": true, "outputDelayMs": 120},
                "timeline": {"zoom": 2},
                "effectsLibrary": {
                    "snow": {
                        "name": "Snow",
                        "color": "white",
                        "items": [{"type": "snow"}]
                    }
                },
                "audioEffects": {
                    "master": {
                        "stack": [{
                            "id": "gain1",
                            "type": "filter",
                            "enabled": true,
                            "config": {"gain": 0.5}
                        }]
                    }
                }
            }"##,
        )
        .unwrap();
        std::fs::write(config.join("overlays.json"), r#"{"logo":{"name":"Logo"}}"#).unwrap();
        std::fs::write(
            config.join("settings_synced.json"),
            r#"{
                "playerVideos": {
                    "intro": {"name": "Intro Loop", "type": "youtube", "id": "abc"}
                }
            }"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.preferences.timeline_zoom(), 2.0);
        assert_eq!(session.preferences.output_delay_ms(), 120);
        assert!((session.preferences.audio_effect_gain() - 0.5).abs() < f32::EPSILON);
        assert!(session.chrome_status().contains("Audio FX master:filter"));
        assert!(session.chrome_status().contains("Delay 120 ms"));
        session.tab = "player".to_string();
        assert!(
            session
                .drawer()
                .iter()
                .any(|row| row.contains("Intro Loop"))
        );
        let _ = session.select_drawer(0);
        assert_eq!(session.player_now.as_deref(), Some("Intro Loop"));
        assert!(session.start_effect("Snow"));
        assert_eq!(session.active_effects, vec!["snow".to_string()]);
        let _ = crate::live::audience_frame(&session);
        assert!(session.create_project());
        assert!(session.create_show_with(
            "timeline",
            "Cues",
            "[Verse]
First line

Chorus:
Second line"
        ));
        assert!(session.save_timeline("0.000 overlay:logo\n0.050 next\n"));
        assert_eq!(session.timeline_cues.len(), 2);
        assert!(session.start_timeline());
        let started = session.timeline_started.unwrap();
        assert!(session.tick_timeline(started + std::time::Duration::from_millis(1)));
        assert!(session.active_overlays.contains(&"logo".to_string()));
        assert!(session.tick_timeline(started + std::time::Duration::from_millis(80)));
        assert_eq!(session.slide_index, Some(1));
        assert!(!session.timeline_playing);
        session.tab = "functions".to_string();
        let functions = session.drawer();
        assert!(functions.contains(&"Timeline".to_string()));
        assert!(functions.contains(&"Clear effects".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn imported_timeline_plays_the_recorded_arrangement_position() {
        let mut show = ShowFile::blank("recording", "Recording");
        let chorus = show.add_slide().unwrap();
        show.add_slide().unwrap();
        let mut arrangement = show.show["layouts"]["main"].clone();
        arrangement["slides"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id":chorus}));
        arrangement["timeline"]["actions"] = serde_json::json!([{
            "time":1500.0,
            "name":"Second chorus",
            "type":"slide",
            "data":{"id":chorus,"index":3}
        }]);
        show.show["layouts"]["arranged"] = arrangement;
        show.show["origin"] = serde_json::json!("propresenter");
        assert!(show.set_active_layout("arranged"));
        let mut session = Session::empty();
        session.shows = vec![show];
        assert!(session.present_show_id("recording"));
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(!session.tick_timeline(start + Duration::from_millis(1499)));
        assert!(session.tick_timeline(start + Duration::from_millis(1500)));
        assert_eq!(session.slide_index, Some(3));
        assert!(!session.timeline_playing);
    }

    #[test]
    fn imported_timeline_clears_layers_at_their_own_times() {
        let mut show = ShowFile::blank("clear", "Clear");
        let slide = show.ordered_slides()[0].id.clone();
        show.show["slides"][&slide]["items"] = serde_json::json!([
            {"type":"shape","style":"left:0px;top:0px;width:200px;height:200px;background:#ff0000;"},
            {"type":"text","lines":[{"text":[{"value":"On screen"}]}]}
        ]);
        show.show["origin"] = serde_json::json!("propresenter");
        show.show["layouts"]["main"]["timeline"] = serde_json::json!({"actions": [
            {"time": 1000, "name": "Background off", "type": "clear_background"},
            {"time": 1500, "name": "Audio off", "type": "clear_audio"},
            {"time": 1750, "name": "Slide off", "type": "clear_slide"},
            {"time": 1900, "name": "Background off again", "type": "clear_background"},
            {"time": 2000, "name": "All off", "type": "clear_all"}
        ]});
        let mut session = Session::empty();
        session.shows = vec![show];
        session
            .stage
            .items
            .retain(|item| item.kind == freeshow_data::StageItemKind::Slide && item.offset == 0);
        assert!(session.present_show_id("clear"));
        assert!(session.save_timeline(&session.timeline_text()));
        assert_eq!(session.timeline_cues[0].name, "Background off");
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        let video = PathBuf::from("background.mp4");
        let audio = PathBuf::from("song.mp3");
        session.background_path = Some(video.clone());
        session.underlay = Some(Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 255, 255],
        });
        let before = crate::live::audience_frame(&session);
        let stage = crate::live::stage_frame(&session);
        session.pending_play = Some(PendingPlay::Video(video));
        session.pending_also = Some(PendingPlay::Audio(audio.clone()));
        session.playing_audio = Some(audio.clone());
        assert!(!session.tick_timeline(start + Duration::from_millis(999)));
        assert!(session.underlay.is_some());
        assert!(session.tick_timeline(start + Duration::from_millis(1000)));
        assert!(session.underlay.is_none());
        assert!(session.background_path.is_none());
        assert!(session.background_transition);
        let after = crate::live::audience_frame(&session);
        assert_eq!(
            before.pixel(0, 0),
            freeshow_output::parse_color("#ff0000").unwrap()
        );
        assert_eq!(after.pixel(0, 0), before.pixel(0, 0));
        assert_ne!(after.pixel(800, 300), before.pixel(800, 300));
        assert_eq!(crate::live::stage_frame(&session).pixels, stage.pixels);
        assert_eq!(session.playing_audio.as_ref(), Some(&audio));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(path)), None] if path == audio)
        );
        assert!(!session.tick_timeline(start + Duration::from_millis(1499)));
        assert!(session.tick_timeline(start + Duration::from_millis(1500)));
        assert!(session.playing_audio.is_none());
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopAudio), None]
        ));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.program_content.borrow().is_some());
        session.replace_underlay(Some(Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 255, 255],
        }));
        session.playing_audio = Some(audio.clone());
        session.active_overlays.push("watermark".to_string());
        session.timed_seconds = Some(3.0);
        assert!(!session.tick_timeline(start + Duration::from_millis(1749)));
        assert!(!session.slide_hidden);
        assert!(session.tick_timeline(start + Duration::from_millis(1750)));
        let cleared = crate::live::audience_frame(&session);
        assert_ne!(cleared.pixel(0, 0), before.pixel(0, 0));
        assert!(session.underlay.is_some());
        assert_ne!(crate::live::stage_frame(&session).pixels, stage.pixels);
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.playing_audio.as_ref(), Some(&audio));
        assert_eq!(session.active_overlays, ["watermark"]);
        assert!(session.next_timer_seconds().is_none());
        assert!(session.tick_timeline(start + Duration::from_millis(1900)));
        assert!(session.underlay.is_none());
        assert!(session.slide_hidden);
        session.active_effects.push("sparkles".into());
        session.playlist_id = Some("music".into());
        session.toggle_metronome();
        assert!(session.tick_timeline(start + Duration::from_millis(2000)));
        assert!(session.blanked);
        assert!(session.slide_hidden);
        assert!(session.active_overlays.is_empty());
        assert!(session.active_effects.is_empty());
        assert!(session.playlist_id.is_none());
        assert!(!session.metronome_on);
        assert!(session.playing_audio.is_none());
        assert_eq!(session.output_motion().0.kind, "none");
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopVideo), Some(PendingPlay::StopAudio)]
        ));
        assert!(!session.timeline_playing);

        session.shows[0].show["layouts"]["main"]["slides"][0]["actions"]["slideActions"] =
            serde_json::json!([{"triggers": ["clear_all"]}]);
        session.select_slide(0);
        assert!(session.blanked);
        assert!(session.play_path(audio.clone()));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::StopVideo), Some(PendingPlay::Audio(path))] if path == audio)
        );
    }

    #[test]
    fn imported_timeline_loops_at_the_saved_duration_after_an_edit() {
        let mut show = ShowFile::blank("loop", "Loop");
        let chorus = show.add_slide().unwrap();
        let mut arrangement = show.show["layouts"]["main"].clone();
        arrangement["slides"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id": chorus}));
        arrangement["timeline"] = serde_json::json!({
            "loop": true, "duration": 2.5, "actions": [
                {"time": 0, "type": "slide", "data": {"index": 0}},
                {"time": 1000, "type": "slide", "data": {"id": chorus, "index": 2}}
            ]
        });
        show.show["layouts"]["arranged"] = arrangement;
        show.show["origin"] = serde_json::json!("propresenter");
        assert!(show.set_active_layout("arranged"));
        let mut session = Session::empty();
        session.shows = vec![show];
        assert!(session.present_show_id("loop"));
        assert!(session.save_timeline(&session.timeline_text()));
        let show = &session.current_show().unwrap().show;
        assert!(show["timeline"].is_null());
        assert_eq!(show["layouts"]["arranged"]["timeline"]["duration"], 2.5);
        assert!(session.start_timeline());

        let start = session.timeline_started.unwrap();
        assert!(session.tick_timeline(start));
        assert!(session.tick_timeline(start + Duration::from_secs(1)));
        assert_eq!(session.slide_index, Some(2));
        assert!(!session.tick_timeline(start + Duration::from_millis(2499)));
        assert!(session.timeline_playing);
        assert!(!session.tick_timeline(start + Duration::from_millis(2500)));
        assert_eq!(session.slide_index, Some(2));
        assert!(session.tick_timeline(start + Duration::from_millis(2550)));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.tick_timeline(start + Duration::from_millis(3500)));
        assert_eq!(session.slide_index, Some(2));
        session.stop_timeline();
        assert!(!session.tick_timeline(start + Duration::from_secs(8)));
    }

    #[test]
    fn imported_timeline_switches_backgrounds_without_reselecting_the_slide() {
        let root = std::env::temp_dir().join(new_id());
        std::fs::create_dir_all(&root).unwrap();
        let still = root.join("sky.png");
        let video = root.join("motion.mp4");
        std::fs::write(
            &still,
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-cover.png"),
        )
        .unwrap();
        std::fs::write(
            &video,
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4"),
        )
        .unwrap();
        let mut show = ShowFile::blank("media", "Media");
        show.show["origin"] = serde_json::json!("propresenter");
        show.show["media"] = serde_json::json!({
            "still": {"path": still},
            "clip": {"path": video, "loop": false, "videoTransport": {"fromTime": 0.25, "toTime": 0.75, "speed":1.5}},
            "trim": {"path": video, "loop": false, "videoTransport": {"fromTime": 0.5, "toTime": 1.0, "speed":1.25}},
            "restart": {"path": video, "loop": false, "retrigger": true, "videoTransport": {"fromTime": 0.5, "toTime": 1.0, "speed":1.25}},
            "looping": {"path": video, "loop": true},
        });
        show.show["layouts"]["main"]["timeline"]["actions"] = serde_json::json!([
            {"time": 0, "type": "background", "data": {"id": "still"}},
            {"time": 500, "type": "background", "data": {"id": "clip"}},
            {"time": 600, "type": "background", "data": {"id": "clip"}},
            {"time": 650, "type": "background", "data": {"id": "trim"}},
            {"time": 700, "type": "background", "data": {"id": "restart"}},
            {"time": 800, "type": "background", "data": {"id": "looping"}},
            {"time": 900, "type": "background", "data": {"id": "still"}},
        ]);
        let mut session = Session::empty();
        session.shows = vec![show];
        assert!(session.present_show_id("media"));
        assert!(session.save_timeline(&session.timeline_text()));
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(session.tick_timeline(start));
        assert_eq!(session.background_path.as_ref(), Some(&still));
        assert!(!session.slide_hidden);
        assert!(session.underlay.is_some());
        assert!(!session.tick_timeline(start + Duration::from_millis(499)));
        assert!(session.tick_timeline(start + Duration::from_millis(500)));
        assert!(!session.media_loop);
        assert_eq!(
            (session.video_from, session.video_to, session.video_rate),
            (0.25, 0.75, 1.5)
        );
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Video(path)), None] if path == video)
        );
        session.tick_timeline(start + Duration::from_millis(600));
        assert!(matches!(session.take_plays(), [None, None]));
        assert!(session.tick_timeline(start + Duration::from_millis(650)));
        assert_eq!(
            (session.video_from, session.video_to, session.video_rate),
            (0.5, 1.0, 1.25)
        );
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Video(path)), None] if path == video)
        );
        assert!(session.tick_timeline(start + Duration::from_millis(700)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Video(path)), None] if path == video)
        );
        assert!(session.clear_slide_layer());
        assert!(session.tick_timeline(start + Duration::from_millis(900)));
        assert!(session.media_loop);
        assert_eq!(
            (session.video_from, session.video_to, session.video_rate),
            (0.0, 0.0, 1.0)
        );
        assert!(matches!(session.take_plays(), [None, None]));
        assert_eq!(session.background_path.as_ref(), Some(&still));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.slide_hidden);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_foreground_cues_hide_the_slide_until_the_next_slide() {
        let root = std::env::temp_dir().join(new_id());
        std::fs::create_dir_all(&root).unwrap();
        let still = root.join("announcement.png");
        std::fs::write(
            &still,
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-cover.png"),
        )
        .unwrap();
        let mut show = ShowFile::blank("foreground", "Foreground");
        let first = show.ordered_slides()[0].id.clone();
        let second = show.add_slide().unwrap();
        for id in [&first, &second] {
            show.show["slides"][id]["items"] = serde_json::json!([
                {"type":"shape","style":"left:0px;top:0px;width:1920px;height:1080px;background:#ff0000;"}
            ]);
        }
        show.show["origin"] = serde_json::json!("propresenter");
        show.show["media"]["notice"] =
            serde_json::json!({"path": still, "videoType": "foreground"});
        show.show["layouts"]["main"]["timeline"]["actions"] = serde_json::json!([
            {"time": 100, "type": "foreground", "data": {"id": "notice"}},
            {"time": 200, "type": "slide", "data": {"id": second, "index": 1}}
        ]);
        let mut session = Session::empty();
        session.output_width = 160;
        session.output_height = 90;
        session.shows = vec![show];
        assert!(session.present_show_id("foreground"));
        assert!(session.save_timeline(&session.timeline_text()));
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        let before = crate::live::audience_frame(&session);
        assert_eq!(
            before.pixel(0, 0),
            freeshow_output::parse_color("#ff0000").unwrap()
        );
        assert!(session.tick_timeline(start + Duration::from_millis(100)));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.slide_hidden);
        assert!(session.underlay.is_some());
        let foreground = crate::live::audience_frame(&session);
        assert_ne!(foreground.pixel(0, 0), before.pixel(0, 0));
        assert!(session.tick_timeline(start + Duration::from_millis(200)));
        assert_eq!(session.slide_index, Some(1));
        assert!(!session.slide_hidden);
        assert!(session.underlay.is_none());
        assert_eq!(
            crate::live::audience_frame(&session).pixel(0, 0),
            before.pixel(0, 0)
        );
        session.shows[0].show["layouts"]["main"]["slides"][0]["background"] =
            serde_json::json!("notice");
        session.select_slide(0);
        assert!(session.slide_hidden);
        assert!(session.underlay.is_some());
        session.select_slide(1);
        assert!(!session.slide_hidden);
        assert!(session.underlay.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scene_video_replaces_the_previous_cues_end_behavior() {
        let video = std::env::temp_dir().join(format!("{}.mp4", new_id()));
        std::fs::write(
            &video,
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4"),
        )
        .unwrap();
        let mut session = Session::empty();
        session.video_end = VideoEnd::Clear { fade: true };
        session.video_end_fade = Some(true);
        session.play_scene_media(&video.to_string_lossy());
        assert!(session.video_end_fade.is_none());
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::Video(_)), None]
        ));
        session.replace_underlay(Some(Raster {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        }));
        assert!(!session.finish_media(&video.to_string_lossy(), false));
        assert_eq!(session.underlay.unwrap().rgba, [255, 0, 0, 255]);
        std::fs::remove_file(video).unwrap();
    }

    #[test]
    fn imported_timeline_video_end_actions_keep_the_slide_and_audio() {
        let video = std::env::temp_dir().join(format!("{}.mp4", new_id()));
        std::fs::write(
            &video,
            include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4"),
        )
        .unwrap();
        for (behavior, motion, black) in [
            ("clear", "none", false),
            ("black", "none", true),
            ("fade_clear", "fade", false),
            ("fade_black", "fade", true),
            ("hold", "", false),
        ] {
            let mut show = ShowFile::blank("ending", "Ending");
            show.show["origin"] = serde_json::json!("propresenter");
            show.show["media"]["clip"] =
                serde_json::json!({"path": video, "loop": false, "endBehavior": behavior});
            show.show["layouts"]["main"]["timeline"]["actions"] = serde_json::json!([
                {"time": 0, "type": "background", "data": {"id": "clip"}}
            ]);
            let mut session = Session::empty();
            session.shows = vec![show];
            assert!(session.present_show_id("ending"));
            assert!(session.start_timeline());
            assert!(session.tick_timeline(session.timeline_started.unwrap()));
            let video_path = video.to_string_lossy();
            assert!(
                !session.finish_media(&video_path, false),
                "ignore an old completion while a video is queued"
            );
            assert!(matches!(
                session.take_plays(),
                [Some(PendingPlay::Video(_)), None]
            ));
            session.replace_underlay(Some(Raster {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 0, 255],
            }));
            session.playing_audio = Some(PathBuf::from("score.wav"));
            assert_eq!(session.finish_media(&video_path, false), behavior != "hold");
            assert!(!session.finish_media(&video_path, false));
            if behavior == "hold" {
                assert_eq!(session.underlay.unwrap().rgba, [255, 0, 0, 255]);
                continue;
            }
            assert_eq!(session.slide_index, Some(0));
            assert!(!session.slide_hidden);
            assert_eq!(
                session.playing_audio.as_deref(),
                Some(Path::new("score.wav"))
            );
            assert!(session.background_path.is_none());
            assert!(session.refresh_output);
            let (transition, media) = session.output_motion();
            assert!(media);
            assert_eq!(transition.kind, motion);
            assert_eq!(
                session.underlay.as_ref().map(|image| image.rgba.as_slice()),
                black.then_some([0, 0, 0, 255].as_slice())
            );
        }
        std::fs::remove_file(video).unwrap();
    }

    #[test]
    fn delayed_slide_actions_follow_the_triggered_cue_while_browsing() {
        let root = std::env::temp_dir().join(new_id());
        std::fs::create_dir_all(&root).unwrap();
        let sound = root.join("sound.wav");
        std::fs::write(&sound, b"fixture").unwrap();
        let mut show = ShowFile::blank("delayed", "Delayed");
        show.add_slide().unwrap();
        show.show["media"]["sound"] = serde_json::json!({"path": sound});
        show.show["layouts"]["main"]["slides"][0]["delayedActions"] = serde_json::json!([
            {"time": 250, "type": "audio", "data": {"id": "sound"}},
            {"time": 500, "type": "clear_audio"}
        ]);
        let mut session = Session::empty();
        session.shows = vec![show, ShowFile::blank("browse", "Browse")];
        assert!(session.present_show_id("delayed"));
        let started = session.pending_cue_actions.as_ref().unwrap().started;
        session.go_live();
        session.hold_program();
        session.show_index = Some(1);

        assert!(!session.run_due_cue_actions(started + Duration::from_millis(249)));
        assert!(session.run_due_cue_actions(started + Duration::from_millis(250)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(path)), None] if path == sound)
        );
        assert!(session.run_due_cue_actions(started + Duration::from_millis(500)));
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopAudio), None]
        ));
        assert!(session.pending_cue_actions.is_none());
        assert_eq!(session.show_index, Some(1));

        assert!(session.present_show_id("delayed"));
        let started = session.pending_cue_actions.as_ref().unwrap().started;
        session.select_slide(1);
        assert!(!session.run_due_cue_actions(started + Duration::from_secs(1)));
        assert!(matches!(session.take_plays(), [None, None]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_timeline_audio_cues_keep_their_settings_beside_a_video_cue() {
        let root = std::env::temp_dir().join(new_id());
        std::fs::create_dir_all(&root).unwrap();
        let song = root.join("song.wav");
        let cue = root.join("cue.wav");
        let video = root.join("video.mp4");
        for path in [&song, &cue, &video] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let mut show = ShowFile::blank("audio-cues", "Audio cues");
        show.show["origin"] = serde_json::json!("propresenter");
        show.show["media"] = serde_json::json!({
            "bed": {"path": song, "audioTransport": {"fromTime": 0.25, "toTime": 2.25, "speed": 0.75}},
            "cue": {"path": cue, "loop": true, "audioTransport": {"fromTime": 1.0, "toTime": 2.0, "speed": 1.5}},
            "alternate": {"path": cue, "loop": true, "audioTransport": {"fromTime": 3.0, "toTime": 4.0, "speed": 2.0}},
            "restart": {"path": cue, "loop": false, "retrigger": true}, "video": {"path": video},
        });
        show.show["layouts"]["main"]["timeline"] = serde_json::json!({"soundtrack": "bed", "actions": [
            {"time": 500, "type": "audio", "data": {"id": "cue"}},
            {"time": 500, "type": "background", "data": {"id": "video"}},
            {"time": 700, "type": "audio", "data": {"id": "cue"}},
            {"time": 850, "type": "audio", "data": {"id": "alternate"}},
            {"time": 1000, "type": "audio", "data": {"id": "restart"}}
        ]});
        let mut session = Session::empty();
        session.shows = vec![show];
        assert!(session.present_show_id("audio-cues"));
        assert!(session.save_timeline(&session.timeline_text()));
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(session.tick_timeline(start));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(path)), None] if path == song)
        );
        assert_eq!(session.audio_span(), (0.25, 2.25));
        assert_eq!(session.audio_tempo(), 0.75);
        assert!(session.tick_timeline(start + Duration::from_millis(500)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Video(path)), Some(PendingPlay::Audio(audio))] if path == video && audio == cue)
        );
        assert!(session.audio_should_loop());
        assert_eq!(session.audio_span(), (1.0, 2.0));
        assert_eq!(session.audio_tempo(), 1.5);
        assert!(!session.tick_timeline(start + Duration::from_millis(700)));
        assert!(matches!(session.take_plays(), [None, None]));
        assert!(session.tick_timeline(start + Duration::from_millis(850)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(audio)), None] if audio == cue)
        );
        assert!(session.audio_should_loop());
        assert_eq!(session.audio_span(), (3.0, 4.0));
        assert_eq!(session.audio_tempo(), 2.0);
        assert!(session.tick_timeline(start + Duration::from_millis(1000)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(audio)), None] if audio == cue)
        );
        assert!(!session.audio_should_loop());
        assert_eq!(session.audio_span(), (0.0, 0.0));
        assert_eq!(session.audio_tempo(), 1.0);
        assert_eq!(session.playing_audio.as_ref(), Some(&cue));
        assert_eq!(session.slide_index, Some(0));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drawer_audio_discards_imported_slide_audio_settings_for_the_same_file() {
        let mut show = ShowFile::blank("audios", "Audio");
        show.show["media"]["clip"] = serde_json::json!({
            "path": "clip.wav", "loop": true,
            "audioTransport": {"fromTime": 1.0, "toTime": 2.0, "speed": 1.5}
        });
        show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["clip"]);
        let mut session = Session::empty();
        session.shows.push(show);
        session.show_index = Some(0);
        session.select_slide(0);
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(path)), None] if path == Path::new("clip.wav"))
        );
        assert_eq!(session.audio_span(), (1.0, 2.0));
        assert_eq!(session.audio_tempo(), 1.5);
        assert!(session.audio_should_loop());

        session.remember_audio_playback(Path::new("clip.wav"));
        assert_eq!(session.audio_span(), (0.0, 0.0));
        assert_eq!(session.audio_tempo(), 1.0);
        assert!(!session.audio_should_loop());
    }

    #[test]
    fn imported_timeline_soundtrack_preserves_cues_and_obeys_stop_and_loop() {
        let mut show = ShowFile::blank("with-track", "With track");
        let path = std::env::temp_dir().join(format!("{}.wav", new_id()));
        std::fs::write(&path, b"RIFF").unwrap();
        let verse = show.add_slide().unwrap();
        show.show["origin"] = serde_json::json!("propresenter");
        show.show["media"] = serde_json::json!({
            "backing": {"path": path},
            "motion": {"path": "motion.mp4"}
        });
        show.show["layouts"]["main"]["slides"][1]["background"] = serde_json::json!("motion");
        show.show["layouts"]["main"]["timeline"] = serde_json::json!({
            "soundtrack": "backing", "duration": 3.0, "loop": true,
            "actions": [{"time": 0, "type": "slide", "data": {"id": verse, "index": 1}}]
        });
        let mut session = Session::empty();
        session.shows = vec![show];
        assert!(session.present_show_id("with-track"));
        assert!(session.save_timeline(&session.timeline_text()));
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(session.tick_timeline(start));
        assert_eq!(session.slide_index, Some(1));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Video(_)), Some(PendingPlay::Audio(audio))] if audio == path)
        );
        assert!(!session.tick_timeline(start + Duration::from_millis(2999)));
        assert!(!session.tick_timeline(start + Duration::from_secs(3)));
        assert!(session.tick_timeline(start + Duration::from_millis(3020)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(audio)), None] if audio == path)
        );
        assert!(session.stop_timeline());
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopAudio), None]
        ));

        // An audio-only timeline remains active until the track ends when duration is absent.
        let timeline = &mut session.shows[0].show["layouts"]["main"]["timeline"];
        *timeline =
            serde_json::json!({"soundtrack": "backing", "soundtrackDelayMs": 100, "actions": []});
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(!session.tick_timeline(start + Duration::from_millis(99)));
        assert!(session.timeline_playing);
        assert!(matches!(session.take_plays(), [None, None]));
        assert!(session.tick_timeline(start + Duration::from_millis(100)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(audio)), None] if audio == path)
        );
        assert!(!session.finish_media(&path.to_string_lossy(), true));
        assert!(session.tick_timeline(start + Duration::from_secs(1)));
        assert!(!session.timeline_playing);
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopAudio), None]
        ));
        let timeline = &mut session.shows[0].show["layouts"]["main"]["timeline"];
        timeline["duration"] = serde_json::json!(1.0);
        timeline["loop"] = serde_json::json!(true);
        assert!(session.start_timeline());
        let start = session.timeline_started.unwrap();
        assert!(session.tick_timeline(start + Duration::from_millis(100)));
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::Audio(_)), None]
        ));
        assert!(session.tick_timeline(start + Duration::from_secs(1)));
        assert!(matches!(
            session.take_plays(),
            [Some(PendingPlay::StopAudio), None]
        ));
        assert!(!session.tick_timeline(start + Duration::from_millis(1099)));
        assert!(session.tick_timeline(start + Duration::from_millis(1100)));
        assert!(
            matches!(session.take_plays(), [Some(PendingPlay::Audio(audio)), None] if audio == path)
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn format_new_show_splits_headers_into_groups() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-format-show-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("settings.json"), r#"{"formatNewShow":true}"#).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with(
            "song1",
            "Song",
            "[Verse]\nIn the beginning\n\nChorus:\nPraise the Lord"
        ));
        let show = session.current_show().unwrap();
        let slides = show.ordered_slides();
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(slides[1].group.as_deref(), Some("Chorus"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn drawer_search_filters_shows_and_maps_the_visible_row() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        session.shows[0].show["name"] = serde_json::json!("Amazing Grace");
        assert!(session.create_show());
        session.shows[1].show["name"] = serde_json::json!("Holy Holy");
        session.tab = "shows".to_string();
        assert_eq!(
            session.drawer(),
            vec!["Amazing Grace".to_string(), "Holy Holy".to_string()]
        );
        session.set_search("holy");
        assert_eq!(session.drawer(), vec!["Holy Holy".to_string()]);
        assert_eq!(session.drawer_real_index(0), 1);
        assert_eq!(session.visible_drawer_index(Some(1)), Some(0));
        session.select_drawer(0);
        assert_eq!(session.show_index, Some(1));
        session.set_search("");
        assert_eq!(session.drawer().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn operator_exports_pdf_package_and_screenshot() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-operator-export-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let path = session.export_current_show_pdf().unwrap();
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("pdf"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("%PDF-1.4"));
        let (package, warnings) = session.export_current_project_package().unwrap();
        assert!(warnings.is_empty());
        assert!(package.extension().and_then(|ext| ext.to_str()) == Some("project"));
        let bytes = std::fs::read(&package).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let frame = crate::live::audience_frame(&session);
        let shot = session.export_screenshot(&frame).unwrap();
        assert_eq!(
            shot.file_name().and_then(|name| name.to_str()),
            Some("screenshot.png")
        );
        assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_package_import_plays_its_bundled_picture_and_video_offline() {
        let source = std::env::temp_dir().join(format!("freeshow-package-source-{}", new_id()));
        let incoming = std::env::temp_dir().join(format!("freeshow-package-incoming-{}", new_id()));
        let mut session = Session::open(source.clone());
        assert!(session.create_project());
        assert!(session.create_show());

        let relative = "Media/Backdrop.png";
        let image = freeshow_output::Frame {
            width: 1,
            height: 1,
            pixels: vec![255, 80, 20, 255],
        };
        std::fs::create_dir_all(source.join("Media")).unwrap();
        std::fs::write(source.join(relative), image.encode_png().unwrap()).unwrap();
        let clip = "Media/Clip.mp4";
        let clip_bytes = include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4");
        std::fs::write(source.join(clip), clip_bytes).unwrap();
        let project_id = session.project_id.clone().unwrap();
        let mut project = session.projects.project_body(&project_id).unwrap();
        project["shows"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": clip, "type": "video", "name": "Closing clip",
            }));
        assert!(session.projects.put_project(&project_id, project));
        assert!(session.save_projects());
        assert!(
            session.edit_selected_slide("Attach a picture", |show, _, _| {
                show.show["media"]["b"] = serde_json::json!({"path": relative});
                show.show["layouts"]["main"]["slides"][0]["background"] = "b".into();
                true
            })
        );
        let (package, warnings) = session.export_current_project_package().unwrap();
        assert!(warnings.is_empty());
        std::fs::create_dir_all(incoming.join("Imports")).unwrap();
        std::fs::copy(package, incoming.join("Imports/Share.project")).unwrap();
        std::fs::remove_dir_all(&source).unwrap();

        let mut reopened = Session::open(incoming.clone());
        assert!(reopened.warnings.is_empty(), "{:?}", reopened.warnings);
        assert_eq!(
            reopened
                .media
                .iter()
                .filter(|entry| entry.path.starts_with(incoming.join("Media")))
                .count(),
            2
        );
        let project = reopened.projects.projects().into_iter().next().unwrap();
        reopened.project_id = Some(project.id);
        assert!(reopened.open_project_item(0, Some(0)));
        let frame = crate::live::audience_frame(&reopened);
        assert_eq!(
            frame.pixel(frame.width / 2, frame.height / 2),
            image.pixel(0, 0)
        );
        assert!(
            reopened
                .background_path
                .as_ref()
                .is_some_and(|path| path.starts_with(incoming.join("Media")))
        );
        assert!(reopened.open_project_item(1, None));
        let [Some(PendingPlay::Video(copied)), _] = reopened.take_plays() else {
            panic!("The imported project's second cue should play the bundled video");
        };
        assert!(copied.starts_with(incoming.join("Media")));
        assert_eq!(std::fs::read(&copied).unwrap(), clip_bytes);
        assert!(freeshow_media::video_thumbnail(&copied, 96, 54).is_some());
        std::fs::remove_dir_all(incoming).unwrap();
    }

    #[test]
    fn metronome_advances_beats_from_the_stored_tempo() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-metronome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"metronome":{"tempo":120,"beats":4}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.preferences.metronome_bpm(), 120);
        assert!(session.chrome_status().is_empty());
        session.toggle_metronome();
        assert!(session.metronome_on);
        assert!(session.chrome_status().contains("120 BPM"));
        let started = session.metronome_started.unwrap();
        session.update_metronome(started + std::time::Duration::from_millis(510));
        assert_eq!(session.metronome_beat, 1);
        session.update_metronome(started + std::time::Duration::from_secs(2));
        assert_eq!(session.metronome_beat, 0);
        session.toggle_metronome();
        assert!(!session.metronome_on);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn edit_preview_hit_test_and_geometry_follow_the_selected_box() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-edit-hit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Front"));
        assert!(session.add_item_box());
        assert!(session.save_lyrics("Back"));
        session.inspector_set("left", "100").unwrap();
        session.inspector_set("top", "100").unwrap();
        session.inspector_set("width", "200").unwrap();
        session.inspector_set("height", "200").unwrap();
        // Default lyric box is left:50 top:88; the second box sits on top at 100,100.
        assert_eq!(session.item_box_at(150.0, 150.0), Some(1));
        assert_eq!(session.item_box_at(80.0, 200.0), Some(0));
        assert_eq!(session.item_box_at(5.0, 5.0), None);
        assert!(session.select_item(0));
        let area = session.inspector_form().item_box;
        let drag = session.begin_layer_drag().unwrap();
        assert!(session.finish_layer_move(drag, 40 - area.left, 50 - area.top));
        let area = session.inspector_form().item_box;
        assert_eq!((area.left, area.top), (40, 50));
        let drag = session.begin_layer_drag().unwrap();
        assert!(session.finish_layer_resize(
            drag,
            300.0 - area.width as f32,
            180.0 - area.height as f32,
            false,
        ));
        let area = session.inspector_form().item_box;
        assert_eq!((area.width, area.height), (300, 180));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lyric_boxes_can_be_selected_styled_and_removed() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-lyric-boxes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Hello"));
        assert!(session.add_item_box());
        assert_eq!(session.item_label(), "Box 2 of 2");
        assert!(session.save_lyrics("Caption"));
        session.inspector_set("font-size", "120").unwrap();
        session.inspector_set("bold", "true").unwrap();
        session.inspector_set("italic", "true").unwrap();
        session.inspector_set("font-family", "Georgia").unwrap();
        session.inspector_set("color", "#ff0000").unwrap();
        session.inspector_set("left", "300").unwrap();
        session.inspector_set("width", "900").unwrap();
        session.inspector_set("border-radius", "24").unwrap();
        session
            .inspector_set("background-color", "#00000080")
            .unwrap();
        let box_shadow = "0 4px 12px rgba(0, 0, 0, 0.5), inset 1px 2px #ffffff";
        session.inspector_set("box-shadow", box_shadow).unwrap();
        for invalid in [
            "0 0 #fff, broken",
            "0 0 -5px #fff",
            "0 0 unknown",
            "0 0 #fff;top:0",
            "1e20 0",
        ] {
            assert!(
                session.inspector_set("box-shadow", invalid).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(session.inspector_form().item_box.box_shadow, box_shadow);
        session.inspector_set("box-shadow", "").unwrap();
        assert!(session.inspector_form().item_box.box_shadow.is_empty());
        let backdrop = "blur(8px) saturate(125%) hue-rotate(-0.5turn)";
        session.inspector_set("backdrop-filter", backdrop).unwrap();
        for invalid in [
            "blur(-5px)",
            "opacity(NaN)",
            "drop-shadow(1px 2px)",
            "blur(4px);top:0",
            "contrast(2",
        ] {
            assert!(
                session.inspector_set("backdrop-filter", invalid).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(session.inspector_form().item_box.backdrop_filter, backdrop);
        session.inspector_set("backdrop-filter", "none").unwrap();
        assert!(session.inspector_form().item_box.backdrop_filter.is_empty());
        session.inspector_set("blur", "3").unwrap();
        session.inspector_set("grayscale", "100").unwrap();
        session.inspector_set("opacity", "60").unwrap();
        session.inspector_set("blend", "screen").unwrap();
        assert!(session.inspector_set("blend", "glow").is_err());
        session.inspector_set("valign", "bottom").unwrap();
        session.inspector_set("rotation", "-90").unwrap();
        session.inspector_set("text-case", "uppercase").unwrap();
        assert!(session.inspector_set("valign", "middle").is_err());
        let form = session.inspector_form();
        assert_eq!(
            (form.item_box.valign.as_str(), form.item_box.rotation),
            ("bottom", 270)
        );
        assert_eq!(form.typography.text_case, "uppercase");
        let item = session.current_show().unwrap().show["slides"]["s1"]["items"][1].clone();
        assert_eq!(item["align"], "align-items: flex-end;");
        session.inspector_set("rotation", "0").unwrap();
        session.inspector_set("valign", "center").unwrap();
        let form = session.inspector_form();
        assert_eq!(
            (form.item_box.valign.as_str(), form.item_box.rotation),
            ("center", 0)
        );
        session.inspector_set("slide-transition", "spin").unwrap();
        session.inspector_set("slide-transition-ms", "900").unwrap();
        let (motion, source) = session.selected_slide_motion();
        assert_eq!(
            (motion.kind.as_str(), motion.duration_ms, source),
            ("spin", 900, "This slide")
        );
        assert!(session.inspector_set("slide-transition", "warp").is_err());
        session
            .inspector_set("slide-transition", "default")
            .unwrap();
        assert_eq!(session.selected_slide_motion().1, "Library default");
        assert_eq!(session.inspector_form().filters.blur, 3);
        let item = session.current_show().unwrap().show["slides"]["s1"]["items"][1].clone();
        assert_eq!(item["filter"], "blur(3px) grayscale(1) opacity(0.6)");
        assert_eq!(item["blend"], "screen");
        session.inspector_set("opacity", "100").unwrap();
        session.inspector_set("blur", "0").unwrap();
        session.inspector_set("grayscale", "0").unwrap();
        session.inspector_set("blend", "normal").unwrap();
        let item = session.current_show().unwrap().show["slides"]["s1"]["items"][1].clone();
        assert!(item.get("filter").is_none() && item.get("blend").is_none());
        assert!(session.inspector_set("color", "red;top:0").is_err());
        assert!(session.inspector_set("width", "0").is_err());
        let form = session.inspector_form();
        assert!(form.has_item);
        assert_eq!(form.typography.font_px, 120);
        assert!(form.typography.bold && form.typography.italic);
        assert_eq!(form.typography.family, "Georgia");
        assert_eq!(form.typography.color, "#ff0000");
        assert_eq!((form.item_box.left, form.item_box.width), (300, 900));
        assert_eq!(form.item_box.radius, "24px");
        assert_eq!(form.item_box.background, "#00000080");
        assert!(session.select_item(0));
        assert_eq!(session.lyrics(), "Hello");
        assert!(session.select_item(1));
        assert_eq!(session.lyrics(), "Caption");
        assert!(session.remove_item_box());
        assert_eq!(session.item_label(), "Box 1 of 1");
        assert_eq!(session.lyrics(), "Hello");
        let mut again = Session::open(root.clone());
        again.select_drawer(0);
        assert_eq!(again.lyrics(), "", "opening a show picks no slide");
        assert!(again.pick_first_slide());
        assert_eq!(again.lyrics(), "Hello");
        assert_eq!(again.current_show().unwrap().item_count("s1"), 1);
        let style = again
            .current_show()
            .unwrap()
            .show
            .pointer("/slides/s1/items/0/lines/0/text/0/style")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        assert!(!style.contains("Georgia"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_last_slide_opens_the_next_project_item() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-advance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.add_slide());
        let first = session.current_show().unwrap().id.clone();
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.current_show().unwrap().id, first);
        assert!(session.create_show());
        let second = session.current_show().unwrap().id.clone();
        assert!(session.present_show_id(&first));
        session.project_item = 0;
        session.slide_index = Some(1);
        assert!(session.advance_show());
        assert_eq!(session.current_show().unwrap().id, second);
        assert_eq!(session.project_item, 1);
        assert_eq!(session.slide_index, Some(0));
        assert!(!session.advance_show());
        assert_eq!(
            Session::next_project_index(&["show", "section", "show"], 0),
            Some(2)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_disabled_setting_keeps_the_last_slide() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-stay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        assert_eq!(session.project_labels().len(), 2);
        let first = session.shows[0].id.clone();
        std::fs::write(
            root.join("settings.json"),
            r#"{"special":{"nextItemOnLastSlide":false}}"#,
        )
        .unwrap();
        assert!(session.present_show_id(&first));
        session.project_item = 0;
        assert!(!session.advance_show());
        assert_eq!(session.current_show().unwrap().id, first);
        assert_eq!(session.project_item, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_picture_stays_behind_the_lyrics() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-bg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let picture = root.join("chapel.png");
        let click = root.join("click.mp3");
        std::fs::write(&click, b"not a real song").unwrap();
        std::fs::write(
            &picture,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99,
                248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ],
        )
        .unwrap();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["media"] = serde_json::json!({
                "bg": { "path": picture },
                "click": { "path": click }
            });
            show.show["layouts"]["main"]["slides"][0]["background"] = serde_json::json!("bg");
            show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["click"]);
            assert!(show.add_slide().is_some());
        }
        assert!(session.present_show_id(&show_id));
        assert!(session.underlay.is_some());
        assert!(!session.slide_hidden);
        assert!(session.background_transition);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Audio(ref path)) if path.ends_with("click.mp3")
        ));
        session.pending_play = None;
        session.select_slide(0);
        assert!(session.pending_play.is_none());
        session.select_slide(1);
        assert!(session.underlay.is_some());
        assert!(!session.slide_hidden);
        assert!(!session.background_transition);
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_background"] }]
            });
        }
        session.select_slide(1);
        assert!(session.underlay.is_none());
        assert!(session.background_transition);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_video_starts_behind_the_lyrics() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let clip = root.join("sermon.mp4");
        let bed = root.join("bed.mp3");
        std::fs::write(&clip, b"not a real video").unwrap();
        std::fs::write(&bed, b"not a real song").unwrap();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["media"]["clip"] = serde_json::json!({ "path": clip });
            show.show["media"]["bed"] = serde_json::json!({ "path": bed });
            show.show["layouts"]["main"]["slides"][0]["background"] = serde_json::json!("clip");
            show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["bed"]);
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_background"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert!(session.underlay.is_none());
        assert!(!session.slide_hidden);
        assert!(session.media_loop);
        assert!(session.media_muted);
        assert!(session.background_transition);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Video(ref path)) if path == &clip
        ));
        assert!(matches!(
            session.pending_also,
            Some(PendingPlay::Audio(ref path)) if path == &bed
        ));
        session.pending_play = None;
        session.pending_also = None;
        session.select_slide(0);
        assert!(session.pending_play.is_none());
        assert!(!session.background_transition);
        session.select_slide(1);
        assert!(session.underlay.is_none());
        {
            let show = &mut session.shows[show_index];
            show.show["media"]["clip"]["loop"] = serde_json::json!(false);
            show.show["media"]["clip"]["muted"] = serde_json::json!(false);
            show.show["media"]["clip"]["videoType"] = serde_json::json!("foreground");
        }
        session.select_slide(0);
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        assert!(session.slide_hidden);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Video(ref path)) if path == &clip
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_turns_its_overlays_on() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-overlay-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        session.overlay_catalog = Catalog::parse(r#"{"logo":{"name":"Logo"}}"#).unwrap();
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["overlays"] =
                serde_json::json!(["logo", "missing"]);
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_overlays"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        session.select_slide(0);
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        session.select_slide(1);
        assert!(session.active_overlays.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locked_overlays_stay_on_and_out_locked_blocks_output() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-locked-settings-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{
                "lockedOverlays": {"default": ["logo"]},
                "outputs": {"default": {"enabled": true, "active": true, "name": "Main"}}
            }"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        session.overlay_catalog = Catalog::parse(r#"{"logo":{"name":"Logo"}}"#).unwrap();
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        {
            let show_index = session.show_index.unwrap();
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        session.preferences.out_locked = Some(true);
        session.video_time = 9.0;
        session.clear_output();
        assert_eq!(session.video_time, 9.0);
        assert!(!session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(!session.retreat_show());
        session.select_slide(1);
        assert_eq!(session.slide_index, Some(0));
        session.apply_remote(freeshow_remote::Command::ClearOverlays);
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        session.preferences.out_locked = Some(false);
        session.apply_remote(freeshow_remote::Command::ClearOverlays);
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_can_stop_the_playing_audio() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let click = root.join("click.mp3");
        std::fs::write(&click, b"not a real song").unwrap();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["media"] = serde_json::json!({
                "click": { "path": click }
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_audio"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        session.playing_audio = Some(click.clone());
        session.select_slide(1);
        assert!(session.playing_audio.is_none());
        assert!(matches!(session.pending_play, Some(PendingPlay::StopAudio)));
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "clearAudio": true
            });
            show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["click"]);
        }
        session.select_slide(0);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Audio(ref path)) if path == &click
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn transition_sound_repeats_on_entry_and_stop_leaves_manual_audio_alone() {
        let mut show = ShowFile::blank("sounds", "Sounds");
        assert!(show.add_slide().is_some());
        show.show["media"]["tone"] = serde_json::json!({
            "path": "transition.wav", "loop": true, "retrigger": true, "transitionSound": true
        });
        show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["tone"]);
        show.show["layouts"]["main"]["slides"][1]["actions"]["stopTransitionSound"] =
            serde_json::json!(true);
        let mut session = Session::empty();
        session.shows.push(show);
        session.show_index = Some(0);

        for _ in 0..2 {
            session.select_slide(0);
            assert!(session.take_plays().into_iter().flatten().any(|play| {
                matches!(play, PendingPlay::Audio(path) if path == Path::new("transition.wav"))
            }));
            assert_eq!(session.audio_loop_override, Some(true));
        }
        session.apply_slide_actions_now(true, None);
        assert!(!session.audio_change_pending());
        session.select_slide(1);
        assert!(
            session
                .take_plays()
                .into_iter()
                .flatten()
                .any(|play| { matches!(play, PendingPlay::StopAudio) })
        );
        assert!(session.playing_audio.is_none());

        let manual = PathBuf::from("manual.wav");
        session.queue_audio(manual.clone());
        session.take_plays();
        session.select_slide(1);
        assert!(!session.audio_change_pending());
        assert_eq!(session.playing_audio, Some(manual));
    }

    #[test]
    fn a_slide_starts_its_countdown() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-timer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["id_start_timer"],
                    "actionValues": { "id_start_timer": { "id": "default" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["stop_timers"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(
            session.running.as_ref().map(|timer| timer.id.as_str()),
            Some("default")
        );
        assert!(
            session
                .running
                .as_ref()
                .is_some_and(|timer| timer.since.is_some())
        );
        session.select_slide(1);
        assert!(session.running.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_pauses_its_countdown() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-pause-timer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["id_start_timer"],
                    "actionValues": { "id_start_timer": { "id": "default" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["pause_timers"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert!(
            session
                .running
                .as_ref()
                .is_some_and(|timer| timer.since.is_some())
        );
        session.select_slide(1);
        assert!(
            session
                .running
                .as_ref()
                .is_some_and(|timer| timer.since.is_none())
        );
        assert_eq!(session.running.as_ref().map(|timer| timer.mark), Some(300));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_moves_on_when_its_audio_ends() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-after-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            show.show["media"]["bed"] = serde_json::json!({ "path": "/chapel/bed.mp3" });
            show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["bed"]);
            show.show["layouts"]["main"]["slides"][0]["actions"] =
                serde_json::json!({ "nextAfterMedia": true });
            show.show["layouts"]["main"]["slides"][0]["background"] = serde_json::json!("clip");
            show.show["media"]["clip"] = serde_json::json!({ "path": "/chapel/sermon.mp4" });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        session.take_plays();
        assert!(!session.finish_media("/chapel/other.mp3", true));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.finish_media("/chapel/bed.mp3", true));
        assert_eq!(session.slide_index, Some(1));
        assert!(session.playing_audio.is_none());
        assert!(session.now_playing().is_none());
        assert!(!session.finish_media("/chapel/bed.mp3", true));
        assert_eq!(session.slide_index, Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_opens_another_song() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-start-show-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let first = session.current_show().unwrap().id.clone();
        assert!(session.create_show());
        let second = session.current_show().unwrap().id.clone();
        let second_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[second_index];
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][0]["disabled"] = serde_json::json!(true);
        }
        let first_index = session
            .shows
            .iter()
            .position(|show| show.id == first)
            .unwrap();
        session.shows[first_index].show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
            "slideActions": [{ "actionValues": { "start_show": { "id": second } } }]
        });
        assert!(session.present_show_id(&first));
        assert_eq!(session.current_show().unwrap().id, second);
        assert_eq!(session.slide_index, Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_clear_slide_hides_the_lyrics_and_keeps_the_picture() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-clear-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        session.overlay_catalog = Catalog::parse(r#"{"logo":{"name":"Logo"}}"#).unwrap();
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let picture = root.join("chapel.png");
        std::fs::write(
            &picture,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99,
                248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ],
        )
        .unwrap();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["media"] = serde_json::json!({ "bg": { "path": picture } });
            show.show["layouts"]["main"]["slides"][0]["background"] = serde_json::json!("bg");
            show.show["layouts"]["main"]["slides"][0]["overlays"] = serde_json::json!(["logo"]);
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_slide"] }]
            });
            show.show["layouts"]["main"]["slides"][2]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_all"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        session.playing_audio = Some(picture.clone());
        session.select_slide(1);
        assert!(session.underlay.is_some());
        assert!(session.slide_hidden);
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        assert!(session.playing_audio.is_some());
        session.select_slide(2);
        assert!(session.underlay.is_none());
        assert!(session.slide_hidden);
        assert!(session.active_overlays.is_empty());
        assert!(session.playing_audio.is_none());
        assert!(matches!(session.pending_play, Some(PendingPlay::StopAudio)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_image_project_item_becomes_the_background() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-image-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let picture = root.join("bg.png");
        std::fs::write(
            &picture,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99,
                248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ],
        )
        .unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(
            session
                .projects
                .append_item(&project_id, &picture.to_string_lossy(), "image")
        );
        assert!(session.present_show_id(&show_id));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert!(session.underlay.is_some());
        assert!(session.slide_hidden);
        assert!(session.background_transition);
        assert_eq!(session.project_item, 1);
        assert_eq!(session.current_show().unwrap().id, show_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_project_item_opens_the_following_show() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-skip-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let first = session.current_show().unwrap().id.clone();
        let project_id = session.project_id.clone().unwrap();
        assert!(session.projects.append_item(
            &project_id,
            &root.join("missing.png").to_string_lossy(),
            "image"
        ));
        assert!(session.create_show());
        let second = session.current_show().unwrap().id.clone();
        assert!(session.present_show_id(&first));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert_eq!(session.current_show().unwrap().id, second);
        assert_eq!(session.project_item, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_video_project_item_is_ready_to_play() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-video-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let clip = root.join("clip.mp4");
        std::fs::write(&clip, b"not a real movie").unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(
            session
                .projects
                .append_item(&project_id, &clip.to_string_lossy(), "video")
        );
        assert!(session.present_show_id(&show_id));
        let store = serde_json::json!({
            clip.to_string_lossy().as_ref(): { "speed": 2 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Video(ref path)) if path == &clip
        ));
        assert_eq!(session.project_item, 1);
        assert!((session.video_rate - 2.0).abs() < 0.001);
        assert!(session.slide_hidden);
        assert!(session.background_transition);
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        assert!((session.video_volume - 1.0).abs() < 0.001);
        let store = serde_json::json!({
            clip.to_string_lossy().as_ref(): { "speed": 2, "videoType": "background", "volume": 0.4 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&clip.to_string_lossy()));
        assert!(session.media_loop);
        assert!(session.media_muted);
        assert!(!session.slide_hidden);
        assert!((session.video_volume - 0.4).abs() < 0.001);
        assert_eq!(session.video_from, 0.0);
        assert_eq!(session.video_to, 0.0);
        let store = serde_json::json!({
            clip.to_string_lossy().as_ref(): { "fromTime": 1.5, "toTime": 9 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&clip.to_string_lossy()));
        assert!((session.video_from - 1.5).abs() < 0.001);
        assert!((session.video_to - 9.0).abs() < 0.001);
        let store = serde_json::json!({
            clip.to_string_lossy().as_ref(): { "videoType": "background" }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(
            session
                .projects
                .set_item_playback(&project_id, 1, false, false)
        );
        assert!(session.play_named(&clip.to_string_lossy()));
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        assert!(!session.slide_hidden);
        assert_eq!(session.current_show().unwrap().id, show_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_timer_restarts_an_imported_loop_cue() {
        let mut show = ShowFile::blank("loop", "Loop");
        for _ in 0..3 {
            show.add_slide().unwrap();
        }
        let order = &mut show.show["layouts"]["main"]["slides"];
        order[0]["disabled"] = serde_json::json!(true);
        order[1]["nextTimer"] = serde_json::json!(2.0);
        order[1]["nextTimerTarget"] = serde_json::json!("next");
        order[2]["nextTimer"] = serde_json::json!(1.25);
        order[2]["nextTimerTarget"] = serde_json::json!("first");
        order[2]["background"] = serde_json::json!("video");
        show.show["media"]["video"] = serde_json::json!({"path":"loop.mp4"});
        let mut session = Session::empty();
        session.shows = vec![show];
        session.show_index = Some(0);
        session.select_slide(2);
        assert_eq!(session.next_timer_seconds(), Some(1.25));
        assert!(session.background_transition);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert!(!session.run_due_cue_completion(due - Duration::from_millis(1)));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(3));
        assert!(!session.run_due_cue_completion(due));

        session.select_slide(2);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert!(session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.next_timer_seconds(), Some(2.0));
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert!(session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(2));
    }

    #[test]
    fn a_timed_jump_finds_the_first_eligible_occurrence_of_its_slide() {
        let mut show = ShowFile::blank("jump", "Jump");
        let target = show.add_slide().unwrap();
        show.add_slide().unwrap();
        let order = show.show["layouts"]["main"]["slides"]
            .as_array_mut()
            .unwrap();
        order[1]["bindings"] = serde_json::json!(["other"]);
        order[2]["nextTimer"] = serde_json::json!(1);
        order[2]["nextTimerTarget"] = serde_json::json!({"slide": target});
        order.push(serde_json::json!({"id":target}));
        let mut session = Session::empty();
        session.shows = vec![show];
        session.show_index = Some(0);
        session.select_slide(2);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert!(session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(3));

        session.shows[0].show["layouts"]["main"]["slides"][3]["disabled"] = serde_json::json!(true);
        session.select_slide(2);
        let due = session.pending_cue_completion.as_ref().unwrap().due;
        assert!(!session.run_due_cue_completion(due));
        assert_eq!(session.slide_index, Some(2));
        assert!(!session.has_pending_cue_completion());
    }

    #[test]
    fn a_folder_project_item_shows_its_pictures_in_order() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-folder-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let folder = root.join("slides");
        std::fs::create_dir_all(&folder).unwrap();
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207,
            192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];
        std::fs::write(folder.join("b.png"), png).unwrap();
        std::fs::write(folder.join("a.png"), png).unwrap();
        std::fs::write(folder.join("notes.txt"), b"skip").unwrap();
        std::fs::write(folder.join("a.mp3"), b"not a real song").unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(
            session
                .projects
                .append_item(&project_id, &folder.to_string_lossy(), "folder")
        );
        assert!(session.present_show_id(&show_id));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert_eq!(session.project_item, 1);
        assert_eq!(session.folder_index, 0);
        assert_eq!(session.folder_paths.len(), 2);
        assert!(session.folder_paths[0].ends_with("a.png"));
        assert!(!session.slide_hidden);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Audio(ref path)) if path.ends_with("a.mp3")
        ));
        assert_eq!(session.next_timer_seconds(), Some(10.0));
        assert!(session.underlay.is_some());
        assert!(session.advance_show());
        assert_eq!(session.folder_index, 1);
        assert!(session.folder_paths[1].ends_with("b.png"));
        assert!(session.pending_play.is_none());
        assert_eq!(session.project_item, 1);
        assert!(session.advance_show());
        assert_eq!(session.folder_index, 0);
        assert!(session.retreat_show());
        assert_eq!(session.folder_index, 1);
        assert_eq!(session.project_item, 1);
        assert!(session.folder_paths[1].ends_with("b.png"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_video_plays_once_with_sound() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-folder-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let folder = root.join("clips");
        std::fs::create_dir_all(&folder).unwrap();
        let clip = folder.join("clip.mp4");
        std::fs::write(&clip, b"not a real movie").unwrap();
        let store = serde_json::json!({
            clip.to_string_lossy().as_ref(): { "videoType": "background" }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(
            session
                .projects
                .append_item(&project_id, &folder.to_string_lossy(), "folder")
        );
        assert!(session.present_show_id(&show_id));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Video(ref path)) if path == &clip
        ));
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_audio_project_item_is_ready_to_play() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-audio-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let song = root.join("song.mp3");
        std::fs::write(&song, b"not a real song").unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(
            session
                .projects
                .append_item(&project_id, &song.to_string_lossy(), "audio")
        );
        assert!(session.present_show_id(&show_id));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Audio(ref path)) if path == &song
        ));
        assert_eq!(session.playing_audio.as_ref(), Some(&song));
        assert_eq!(session.project_item, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_command_opens_the_project_row() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-remote-row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let first = session.current_show().unwrap().id.clone();
        assert!(session.create_show());
        let second = session.current_show().unwrap().id.clone();
        assert!(session.present_show_id(&first));
        session.project_item = 0;
        session.apply_remote(freeshow_remote::Command::OpenProjectItem {
            index: 1,
            slide: Some(0),
        });
        assert_eq!(session.current_show().unwrap().id, second);
        assert_eq!(session.project_item, 1);
        assert_eq!(session.slide_index, Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_overlay_project_item_stays_on_the_song() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-overlay-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        session.overlay_catalog = Catalog::parse(r#"{"logo":{"name":"Logo"}}"#).unwrap();
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let project_id = session.project_id.clone().unwrap();
        assert!(session.projects.append_item(&project_id, "logo", "overlay"));
        assert!(session.present_show_id(&show_id));
        session.project_item = 0;
        session.slide_index = Some(0);
        assert!(session.advance_show());
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        assert_eq!(session.current_show().unwrap().id, show_id);
        assert_eq!(session.project_item, 1);
        session.apply_remote(freeshow_remote::Command::OpenProjectItem {
            index: 1,
            slide: None,
        });
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn previous_on_the_first_slide_opens_the_earlier_song() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-retreat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.add_slide());
        let first = session.current_show().unwrap().id.clone();
        assert!(session.create_show());
        let second = session.current_show().unwrap().id.clone();
        assert!(session.present_show_id(&second));
        session.project_item = 1;
        session.slide_index = Some(0);
        assert!(session.retreat_show());
        assert_eq!(session.current_show().unwrap().id, first);
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.project_item, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_plays_a_sound_effect() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-effect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"audioPlaylists":{"bed":{"name":"Bed","songs":["one.mp3","two.mp3"],"loop":false}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [
                    {
                        "triggers": ["start_playlist"],
                        "actionValues": { "start_playlist": { "id": "bed" } }
                    },
                    {
                        "triggers": ["start_audio_effect"],
                        "actionValues": { "start_audio_effect": { "path": "hit.mp3" } }
                    }
                ]
            });
        }
        assert!(session.present_show_id(&show_id));
        let plays = session.take_plays();
        assert!(
            matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("hit.mp3"))
        );
        assert!(!session.finish_media("hit.mp3", true));
        let plays = session.take_plays();
        assert!(
            matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("one.mp3"))
        );
        assert!(!session.finish_media("one.mp3", true));
        let plays = session.take_plays();
        assert!(
            matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("two.mp3"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_waits_before_its_later_actions() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"variables":{"mark":{"name":"Mark","type":"text","text":""}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [
                    {
                        "triggers": ["change_variable"],
                        "actionValues": { "change_variable": { "id": "mark", "key": "text", "value": "now" } }
                    },
                    { "triggers": ["wait"], "actionValues": { "wait": { "number": 5 } } },
                    {
                        "triggers": ["change_variable"],
                        "actionValues": { "change_variable": { "id": "mark", "key": "text", "value": "after" } }
                    }
                ]
            });
            assert!(show.add_slide().is_some());
        }
        assert!(session.present_show_id(&show_id));
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "mark" && value == "now")
        );
        assert!(!session.run_due_actions());
        assert_eq!(session.current_show().unwrap().actions_at(0).len(), 3);
        session.select_slide(1);
        assert!(!session.finish_waits());
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "mark" && value == "now")
        );
        session.select_slide(0);
        assert!(session.finish_waits());
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "mark" && value == "after")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_opens_a_random_other_slide() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-random-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"variables":{"mark":{"name":"Mark","type":"text","text":""}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["random_slide"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            let landed = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_variable"],
                    "actionValues": { "change_variable": { "id": "mark", "key": "text", "value": "landed" } }
                }]
            });
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = landed.clone();
            show.show["layouts"]["main"]["slides"][2]["actions"] = landed;
        }
        assert!(session.present_show_id(&show_id));
        let index = session.slide_index.unwrap();
        assert!(index == 1 || index == 2);
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "mark" && value == "landed")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_switches_the_stage_layout() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-stage-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"stage":{"name":"Stage","stageOutput":"default"},"audience":{"name":"Audience","active":true}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Config").join("stage.json"),
            r##"{"default":{"name":"Main","settings":{"color":"#111111"},"items":{"clock":{"type":"clock","style":"left:1px;width:200px;height:80px;"}}},"notes":{"name":"Notes","settings":{"color":"#abcdef"},"items":{"notes":{"type":"slide_notes","style":"left:1px;width:800px;height:200px;"}}}}"##,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.stage.name, "Main");
        let library = Library::open(&root);
        let original = library.stage_layout("default").unwrap();
        let before = crate::live::stage_frame(&session);
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_stage_output_layout"],
                    "actionValues": { "change_stage_output_layout": { "stageLayoutId": "notes" } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_stage_output_layout"],
                    "actionValues": {
                        "change_stage_output_layout": { "outputId": "audience", "stageLayoutId": "default" }
                    }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.stage.name, "Notes");
        assert_eq!(session.stage.background, "#abcdef");
        let after = crate::live::stage_frame(&session);
        assert_ne!(before.pixels, after.pixels);
        let stored = std::fs::read_to_string(root.join("settings.json")).unwrap();
        assert!(stored.contains("\"stageOutput\":\"notes\""));
        session.select_slide(1);
        assert_eq!(session.stage.name, "Notes");
        session.stage_set("notes", "left", "77").unwrap();
        assert_eq!(library.stage_layout("default").unwrap(), original);
        let mut reopened = Session::open(root.clone());
        assert_eq!(reopened.stage.name, "Notes");
        assert_eq!(reopened.stage_rows()[0].left, 77);
        assert!(reopened.remove_stage_item("notes"));
        assert!(Session::open(root.clone()).stage.items.is_empty());
        assert_eq!(library.stage_layout("default").unwrap(), original);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_plays_its_audio_playlist() {
        let songs = vec![
            "one.mp3".to_string(),
            "two.mp3".to_string(),
            "three.mp3".to_string(),
        ];
        let shuffled = freeshow_data::shuffled_songs(&songs, "bed");
        assert_eq!(shuffled.len(), 3);
        assert!(shuffled.iter().all(|song| songs.contains(song)));
        assert_ne!(shuffled, songs);

        let root =
            std::env::temp_dir().join(format!("freeshow-rust-playlist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"audioPlaylists":{"bed":{"name":"Sunday Bed","songs":["one.mp3","two.mp3"],"volume":0.5},"walk":{"name":"Walk In","songs":["a.mp3","b.mp3","c.mp3"],"mode":"shuffle","loop":false}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["start_playlist"],
                    "actionValues": { "start_playlist": { "id": "bed" } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["name_start_playlist", "playlist_next"],
                    "actionValues": { "name_start_playlist": { "value": "Walk" } }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        let plays = session.take_plays();
        assert!(
            matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("one.mp3"))
        );
        assert!((session.audio_routes()[0].gain - 0.5).abs() < 0.001);
        assert!(!session.finish_media("other.mp3", true));
        assert!(session.take_plays()[0].is_none());
        assert!(!session.finish_media("one.mp3", true));
        let plays = session.take_plays();
        assert!(
            matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("two.mp3"))
        );
        session.select_slide(0);
        assert!(session.take_plays()[0].is_none());
        session.select_slide(1);
        let plays = session.take_plays();
        let Some(PendingPlay::Audio(ref path)) = plays[0] else {
            panic!("walk playlist did not start");
        };
        let started = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(["a.mp3", "b.mp3", "c.mp3"].contains(&started.as_str()));
        assert!(!session.finish_media(&path.to_string_lossy(), true));
        let plays = session.take_plays();
        let Some(PendingPlay::Audio(ref next)) = plays[0] else {
            panic!("shuffle playlist did not step");
        };
        assert_ne!(next, path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_changes_its_variable() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-change-variable-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"variables":{"count":{"name":"Count","type":"number","number":1,"step":2},"speaker":{"name":"Speaker","type":"text","text":"Ada","enabled":true}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_variable"],
                    "actionValues": { "change_variable": { "name": "Count", "key": "increment" } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_variable"],
                    "actionValues": { "change_variable": { "id": "speaker", "key": "text", "value": "Bo" } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][2]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_variable"],
                    "actionValues": { "change_variable": { "name": "Speaker", "key": "enabled", "value": false } }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        let library = Library::open(&root);
        let values = library.variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "count" && value == "3")
        );
        session.select_slide(1);
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "speaker" && value == "Bo")
        );
        session.select_slide(2);
        let values = Library::open(&root).variables();
        assert!(
            values
                .iter()
                .any(|(name, value)| name == "speaker" && value.is_empty())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lyric_shows_its_variable() {
        let speaker = [("speaker".into(), "Ada".into())];
        assert_eq!(
            freeshow_data::fill_variables("Hello {variable_speaker}", &speaker),
            "Hello Ada"
        );
        assert_eq!(
            freeshow_data::fill_variables("Hello {$speaker|Guest}", &speaker),
            "Hello Ada"
        );
        assert_eq!(
            freeshow_data::fill_variables(
                "Hello {$speaker|Guest}",
                &[("speaker".into(), String::new())]
            ),
            "Hello Guest"
        );
        assert_eq!(
            freeshow_data::fill_variables("{variable_count}", &[("count".into(), "3".into())]),
            "3"
        );

        let root =
            std::env::temp_dir().join(format!("freeshow-rust-variable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"variables":{"v1":{"name":"Speaker","type":"text","text":"Ada"},"v2":{"name":"Count","type":"number","number":3}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_index = session.show_index.unwrap();
        session.shows[show_index].show["slides"]["s1"]["items"][0]["lines"] = serde_json::json!([{
            "text": [{
                "value": "Hello {variable_speaker} {$count}",
                "style": "font-size:80px;color:#FFFFFF;"
            }]
        }]);
        let show_id = session.shows[show_index].id.clone();
        assert!(session.present_show_id(&show_id));
        let filled = crate::live::audience_frame(&session);
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"variables":{}}"#,
        )
        .unwrap();
        let empty = crate::live::audience_frame(&session);
        assert_ne!(filled.pixels, empty.pixels);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_stage_follows_the_audience_reveal() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-stage-reveal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["slides"]["s1"]["items"] = serde_json::json!([{
                "lineReveal": true,
                "lines": [
                    { "text": [{ "value": "One" }] },
                    { "text": [{ "value": "Two" }] }
                ]
            }]);
            assert!(show.add_slide().is_some());
            show.show["slides"]["s2"]["items"] = serde_json::json!([
                {
                    "clickReveal": true,
                    "lines": [{ "text": [{ "value": "Secret" }] }]
                },
                {
                    "lines": [{ "text": [{ "value": "Visible" }] }]
                }
            ]);
            assert_eq!(
                show.stage_text_boxes(0, true, 0, false, None),
                vec![Vec::<String>::new()]
            );
            assert_eq!(
                show.stage_text_boxes(0, true, 1, false, None),
                vec![vec!["One".to_string()]]
            );
            assert_eq!(
                show.stage_text_boxes(1, false, 0, false, None),
                vec![vec![], vec!["Visible".to_string()]]
            );
        }
        assert!(session.present_show_id(&show_id));
        let hidden = crate::live::stage_frame(&session);
        assert!(session.advance_show());
        assert_eq!(session.reveal_count, 1);
        let shown = crate::live::stage_frame(&session);
        assert_ne!(hidden.pixels, shown.pixels);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_applies_its_template_overlay_and_pause() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-template-action-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        session.template_catalog = Catalog::parse(
            r#"{"bold":{"name":"Bold","items":[{"style":"color:red;","lines":[{"align":"","text":[{"style":"font-size:80px;","value":""}]}]}]}}"#,
        )
        .unwrap();
        session.overlay_catalog = Catalog::parse(r#"{"logo":{"name":"Logo"}}"#).unwrap();
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [
                    {
                        "triggers": ["id_select_overlay"],
                        "actionValues": { "id_select_overlay": { "id": "logo" } }
                    },
                    {
                        "triggers": ["id_select_overlay"],
                        "actionValues": { "id_select_overlay": { "id": "missing" } }
                    },
                    {
                        "triggers": ["set_template"],
                        "actionValues": { "set_template": { "id": "bold" } }
                    },
                    { "triggers": ["toggle_playing_media"] },
                    { "triggers": ["wait"], "actionValues": { "wait": { "number": 5 } } },
                    { "triggers": ["toggle_playing_media"] }
                ]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["set_template"],
                    "actionValues": { "set_template": { "id": "missing" } }
                }]
            });
            show.save(show.path.as_deref().unwrap()).unwrap();
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.active_overlays, vec!["logo".to_string()]);
        assert_eq!(
            session.current_show().unwrap().show["settings"]["template"],
            "bold"
        );
        assert_eq!(
            session.current_show().unwrap().show["slides"]["s1"]["items"][0]["style"],
            "color:red;"
        );
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::ToggleMedia)
        ));
        let saved = ShowFile::load(session.shows[show_index].path.as_deref().unwrap()).unwrap();
        assert_eq!(saved.actions_at(0).len(), 6);
        assert!(session.show_draft_status().is_empty());
        let _ = session.take_plays();
        session.select_slide(1);
        assert_eq!(
            session.current_show().unwrap().show["settings"]["template"],
            "bold"
        );
        assert!(session.pending_play.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_starts_its_camera() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-camera-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let bed = root.join("bed.mp3");
        std::fs::write(&bed, b"not a real song").unwrap();
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["media"] = serde_json::json!({
                "song": { "path": bed.to_string_lossy(), "name": "Bed" }
            });
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["start_camera"],
                    "actionValues": { "start_camera": { "id": "front-id", "name": "Front" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][0]["audio"] = serde_json::json!(["song"]);
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["start_camera"],
                    "actionValues": { "start_camera": { "id": "Side" } }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Camera(ref name)) if name == "Front"
        ));
        assert!(session.background_transition);
        assert!(matches!(
            session.pending_also,
            Some(PendingPlay::Audio(ref path)) if path.ends_with("bed.mp3")
        ));
        let _ = session.take_plays();
        session.select_slide(1);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Camera(ref name)) if name == "Side"
        ));
        assert!(session.pending_also.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_sets_the_main_volume_and_mute() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-volume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_volume"],
                    "actionValues": { "change_volume": { "channelId": "main", "volume": 0.25 } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["mute"],
                    "actionValues": { "mute": { "id": "main", "value": true } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][2]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_volume"],
                    "actionValues": { "change_volume": { "volume": 40 } }
                }]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][3]["actions"] = serde_json::json!({
                "slideActions": [
                    {
                        "triggers": ["mute"],
                        "actionValues": { "mute": { "value": "false" } }
                    },
                    {
                        "triggers": ["change_volume"],
                        "actionValues": { "change_volume": { "volume": -6, "useDB": true } }
                    }
                ]
            });
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][4]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["change_volume"],
                    "actionValues": { "change_volume": {} }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert!((session.volume - 0.25).abs() < 0.001);
        assert!(!session.audio_muted);
        assert!((session.audio_routes()[0].gain - 0.25).abs() < 0.001);
        session.select_slide(1);
        assert!(session.audio_muted);
        assert_eq!(session.audio_routes()[0].gain, 0.0);
        assert!((session.volume - 0.25).abs() < 0.001);
        session.select_slide(2);
        assert!((session.volume - 0.4).abs() < 0.001);
        assert!(session.audio_muted);
        session.select_slide(3);
        assert!(!session.audio_muted);
        assert!((session.volume - 0.501).abs() < 0.001);
        assert!((session.audio_routes()[0].gain - 0.501).abs() < 0.001);
        session.select_slide(4);
        assert_eq!(session.volume, 0.0);
        session.select_slide(4);
        assert!((session.volume - 0.501).abs() < 0.001);
        let again = Session::open(root.clone());
        assert!((again.volume - 0.501).abs() < 0.001);
        assert!(!again.audio_muted);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_clears_one_overlay_and_sets_the_transition() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-overlay-transition-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        session.overlay_catalog =
            Catalog::parse(r#"{"logo":{"name":"Logo"},"title":{"name":"Title"}}"#).unwrap();
        assert!(session.create_show());
        std::fs::write(
            root.join("settings.json"),
            r#"{"transitionData":{"text":{"type":"fade","duration":500,"easing":"sine"}}}"#,
        )
        .unwrap();
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["layouts"]["main"]["slides"][0]["overlays"] =
                serde_json::json!(["logo", "title"]);
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [
                    {
                        "triggers": ["clear_overlay"],
                        "actionValues": { "clear_overlay": { "id": "logo" } }
                    },
                    {
                        "triggers": ["change_transition"],
                        "actionValues": {
                            "change_transition": {
                                "id": "media",
                                "type": "slide",
                                "duration": 400,
                                "easing": "quad"
                            }
                        }
                    }
                ]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(
            session.active_overlays,
            vec!["logo".to_string(), "title".to_string()]
        );
        session.select_slide(1);
        assert_eq!(session.active_overlays, vec!["title".to_string()]);
        let library = Library::open(&root);
        let media = library.media_transition();
        assert_eq!(media.kind, "slide");
        assert_eq!(media.duration_ms, 400);
        assert_eq!(media.easing, "quad");
        let text = library.text_transition();
        assert_eq!(text.kind, "fade");
        assert_eq!(text.duration_ms, 500);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_can_hold_or_rewrite_its_countdown() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-next-timer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][0]["nextTimer"] = serde_json::json!(10);
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["clear_next_timer"] }]
            });
            show.show["layouts"]["main"]["slides"][1]["nextTimer"] = serde_json::json!(8);
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["set_next_slide_timer"],
                    "actionValues": { "set_next_slide_timer": { "value": 3 } }
                }]
            });
            show.show["layouts"]["main"]["slides"][2]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["previous_slide"] }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.next_timer_seconds(), None);
        session.select_slide(1);
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.next_timer_seconds(), Some(8.0));
        let show = session.current_show().unwrap();
        assert_eq!(show.next_timer_at(0), Some(3.0));
        assert_eq!(show.next_timer_at(1), Some(3.0));
        assert!(show.ends_at(2));
        assert!(!show.ends_at(1));
        session.select_slide(0);
        assert_eq!(session.next_timer_seconds(), None);
        session.select_slide(2);
        assert_eq!(session.slide_index, Some(1));
        {
            let show = &mut session.shows[session.show_index.unwrap()];
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][2]["actions"] = serde_json::json!({
                "slideActions": [{ "triggers": ["next_slide"] }]
            });
        }
        session.select_slide(2);
        assert_eq!(session.slide_index, Some(3));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_opens_its_named_group() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-group-jump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.set_group("s1", "Intro"));
            let chorus = show.add_slide().unwrap();
            let verse = show.add_slide().unwrap();
            let verse_again = show.add_slide().unwrap();
            let bridge = show.add_slide().unwrap();
            let tag = show.add_slide().unwrap();
            assert!(show.set_group(&chorus, "Chorus"));
            assert!(show.set_group(&verse, "Verse"));
            assert!(show.set_group(&verse_again, "Verse"));
            assert!(show.set_group(&bridge, "Bridge"));
            assert!(show.set_group(&tag, "Tag"));
            show.show["slides"][&verse]["globalGroup"] = serde_json::json!("verse");
            show.show["slides"][&verse_again]["globalGroup"] = serde_json::json!("verse");
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["name_select_slide"],
                    "actionValues": { "name_select_slide": { "value": "Chorus" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][4]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["id_select_group"],
                    "actionValues": { "id_select_group": { "id": "verse" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][5]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["index_select_slide"],
                    "actionValues": { "index_select_slide": { "index": 2 } }
                }]
            });
            assert_eq!(show.group_index("Chorus"), Some(1));
            assert_eq!(show.group_index("Verse"), Some(2));
            assert_eq!(show.group_index("Verse_2"), Some(3));
            assert_eq!(show.group_index("Cho"), Some(1));
            assert_eq!(show.next_global_group("verse", 0), Some(2));
            assert_eq!(show.next_global_group("verse", 2), Some(3));
            assert_eq!(show.next_global_group("verse", 4), Some(2));
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(1));
        session.select_slide(5);
        assert_eq!(session.slide_index, Some(2));
        session.select_slide(4);
        assert_eq!(session.slide_index, Some(2));
        assert!(session.create_show_with("other-song", "Other Song", "Hello"));
        let first = session
            .shows
            .iter()
            .position(|show| show.id == show_id)
            .unwrap();
        session.shows[first].show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
            "slideActions": [{
                "triggers": ["index_select_slide"],
                "actionValues": { "index_select_slide": { "showId": "other-song", "index": 0 } }
            }]
        });
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.current_show().unwrap().id, "other-song");
        assert_eq!(session.slide_index, Some(0));
        {
            let show = &mut session.shows[first];
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["name_select_slide"],
                    "actionValues": { "name_select_slide": { "value": "Chorus" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["name_select_slide"],
                    "actionValues": { "name_select_slide": { "value": "Intro" } }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_turns_the_style_line_page() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-style-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"default":{"active":true,"style":"main"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"styles":{"main":{"lines":2}}}"#,
        )
        .unwrap();
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["slides"]["s1"]["items"] = serde_json::json!([{
                "style": "left:40px;top:40px;width:800px;height:800px;",
                "lines": [
                    { "text": [{ "value": "One", "style": "font-size:40px;color:#FFFFFF;" }] },
                    { "text": [{ "value": "Two", "style": "font-size:40px;color:#FFFFFF;" }] },
                    { "text": [{ "value": "Three", "style": "font-size:40px;color:#FFFFFF;" }] },
                    { "text": [{ "value": "Four", "style": "font-size:40px;color:#FFFFFF;" }] }
                ]
            }]);
            assert!(show.add_slide().is_some());
            assert_eq!(show.most_lines(0), 4);
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.line_index, 0);
        let first = crate::live::audience_frame(&session);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.line_index, 2);
        let second = crate::live::audience_frame(&session);
        assert_ne!(first.pixels, second.pixels);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.line_index, 0);
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.line_index, 2);
        assert!(session.retreat_show());
        assert_eq!(session.line_index, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_shows_a_click_reveal_item() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-click-reveal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["slides"]["s1"]["items"] = serde_json::json!([{
                "clickReveal": true,
                "style": "left:40px;top:40px;width:800px;height:200px;",
                "lines": [{ "text": [{ "value": "Amen", "style": "font-size:80px;color:#FFFFFF;" }] }]
            }]);
            assert!(show.add_slide().is_some());
            assert!(show.has_click_reveal(0));
        }
        assert!(session.present_show_id(&show_id));
        assert!(!session.click_revealed);
        let hidden = crate::live::audience_frame(&session);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(session.click_revealed);
        let shown = crate::live::audience_frame(&session);
        assert_ne!(hidden.pixels, shown.pixels);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(1));
        assert!(!session.click_revealed);
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(session.click_revealed);
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(!session.click_revealed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_reveals_one_more_line() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-line-reveal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            show.show["slides"]["s1"]["items"] = serde_json::json!([{
                "lineReveal": true,
                "lines": [
                    { "text": [{ "value": "Holy" }] },
                    { "text": [{ "value": "holy" }] }
                ]
            }]);
            assert!(show.add_slide().is_some());
        }
        assert_eq!(session.shows[show_index].line_reveal_lines(0), Some(2));
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.reveal_count, 0);
        assert_eq!(session.slide_index, Some(0));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.reveal_count, 1);
        assert!(session.advance_show());
        assert_eq!(session.reveal_count, 2);
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.reveal_count, 0);
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.reveal_count, 2);
        assert!(session.retreat_show());
        assert_eq!(session.reveal_count, 1);
        let hidden = crate::live::frame_for_slide(
            session.current_show().unwrap(),
            0,
            None,
            true,
            Some(0),
            true,
            None,
            &[],
        );
        let one = crate::live::frame_for_slide(
            session.current_show().unwrap(),
            0,
            None,
            true,
            Some(1),
            true,
            None,
            &[],
        );
        let both = crate::live::frame_for_slide(
            session.current_show().unwrap(),
            0,
            None,
            true,
            Some(2),
            true,
            None,
            &[],
        );
        assert_ne!(hidden.pixels, one.pixels);
        assert_ne!(one.pixels, both.pixels);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_skips_a_slide_bound_to_another_output() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-bound-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["bindings"] = serde_json::json!(["stage"]);
        }
        assert!(session.present_show_id(&show_id));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(2));
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_end_slide_returns_to_the_first() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-end-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["end"] = serde_json::json!(true);
        }
        assert!(session.present_show_id(&show_id));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(1));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_skips_a_disabled_slide() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-disabled-slide-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][1]["disabled"] = serde_json::json!(true);
        }
        assert!(session.present_show_id(&show_id));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(2));
        assert!(session.retreat_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(2));
        assert!(!session.advance_show());
        assert_eq!(session.slide_index, Some(2));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_starts_a_saved_audio_stream() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-audio-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let bed = root.join("bed.mp3");
        std::fs::write(&bed, b"not a real song").unwrap();
        std::fs::create_dir_all(root.join("Config")).unwrap();
        let synced = serde_json::json!({
            "audioStreams": {
                "bed": { "name": "Bed", "value": bed },
                "live": { "name": "Live", "value": "https://example.com/live" }
            }
        });
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            synced.to_string(),
        )
        .unwrap();
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        {
            let show = &mut session.shows[show_index];
            assert!(show.add_slide().is_some());
            show.show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["start_audio_stream"],
                    "actionValues": { "start_audio_stream": { "id": "bed" } }
                }]
            });
            show.show["layouts"]["main"]["slides"][1]["actions"] = serde_json::json!({
                "slideActions": [{
                    "triggers": ["start_audio_stream"],
                    "actionValues": { "start_audio_stream": { "id": "live" } }
                }]
            });
        }
        assert!(session.present_show_id(&show_id));
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::Audio(ref path)) if path == &bed
        ));
        session.pending_play = None;
        session.playing_audio = None;
        session.select_slide(1);
        assert!(session.pending_play.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_switches_the_output_style() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-slide-style-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"default":{"active":true,"style":"main"}},"transitionData":{"text":{"type":"fade","duration":500,"easing":"sine"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"styles":{"main":{"transition":{"text":{"type":"fade","duration":500,"easing":"sine"}}},"night":{"transition":{"text":{"type":"scale","duration":250,"easing":"back"}}}}}"#,
        )
        .unwrap();
        let show_id = session.current_show().unwrap().id.clone();
        let show_index = session.show_index.unwrap();
        session.shows[show_index].show["layouts"]["main"]["slides"][0]["actions"] = serde_json::json!({
            "slideActions": [{
                "triggers": ["change_output_style"],
                "actionValues": { "change_output_style": { "outputStyle": "night" } }
            }]
        });
        assert!(session.present_show_id(&show_id));
        assert_eq!(
            freeshow_data::Library::open(&root).output_style_id(),
            "night"
        );
        let (motion, background) = session.output_motion();
        assert!(!background);
        assert_eq!(motion.kind, "scale");
        assert_eq!(motion.easing, "back");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_output_style_transition_fills_a_plain_slide() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-style-motion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        session.slide_index = Some(0);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"default":{"active":true,"style":"main"}},"transitionData":{"text":{"type":"fade","duration":500,"easing":"sine"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"styles":{"main":{"transition":{"text":{"type":"scale","duration":250,"easing":"back"},"media":{"type":"spin","duration":1100,"easing":"bounce"}}}}}"#,
        )
        .unwrap();
        let (motion, background) = session.output_motion();
        assert!(!background);
        assert_eq!(motion.kind, "scale");
        assert_eq!(motion.easing, "back");
        assert_eq!(session.selected_slide_motion(), (motion, "Output style"));
        let index = session.show_index.unwrap();
        session.shows[index].show["layouts"]["main"]["slides"][0]["transition"] =
            serde_json::json!({"type":"none","duration":0});
        let (motion, _) = session.output_motion();
        assert_eq!(motion.kind, "none");
        session.background_transition = true;
        let (motion, background) = session.output_motion();
        assert!(background);
        assert_eq!(motion.kind, "spin");
        assert_eq!(motion.duration_ms, 1100);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn transition_audition_and_range_edits_leave_the_program_on_air() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-transition-audition-{}",
            freeshow_data::new_id()
        ));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.edit_selected_slide("Set fixture", |show, _, _| {
            assert!(show.replace_lyrics_with_split("Red\n\nDisabled\n\nBlue\n\nAgain", false, 0));
            let slides = show.ordered_slides();
            for (index, color) in ["#ff0000", "#00ff00", "#0000ff"].into_iter().enumerate() {
                show.show["slides"][&slides[index].id]["settings"]["color"] = color.into();
            }
            let positions = &mut show.show["layouts"]["main"]["slides"];
            positions[1]["disabled"] = true.into();
            positions[2]["transition"] = serde_json::json!({
                "type": "wipe", "duration": 780, "easing": "back",
                "custom": {"direction": "bottom_top", "pluginData": {"keep": true}}
            });
            positions[3]["id"] = slides[0].id.as_str().into();
            true
        }));
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        session.select_slide_for_edit(2);
        session.background_transition = true;

        session
            .inspector_set("slide-transition-easing", "linear")
            .unwrap();
        let audition = crate::live::TransitionAudition::prepare(&session).unwrap();
        assert_eq!((audition.from_slide, audition.to_slide), (Some(0), 2));
        assert_eq!(audition.source, "This slide");
        assert_eq!(audition.duration().as_millis(), 780);
        assert_eq!(audition.motion.direction, "bottom_top");
        assert_eq!(audition.motion.easing, "linear");
        assert_eq!(
            audition.frame(0.0).pixel(5, 5),
            freeshow_output::parse_color("#ff0000").unwrap()
        );
        assert_eq!(
            audition.frame(1.0).pixel(5, 5),
            freeshow_output::parse_color("#0000ff").unwrap()
        );
        assert_ne!(audition.frame(0.5).pixels, audition.frame(1.0).pixels);
        assert!(session.background_transition);

        let history = session.history.len();
        assert!(session.apply_slide_motion_range(2, 5).is_err());
        assert_eq!(session.history.len(), history);
        session.apply_slide_motion_range(2, 4).unwrap();
        assert_eq!(session.history.len(), history + 1);
        let show = session.current_show().unwrap();
        let persisted = ShowFile::load(show.path.as_deref().unwrap()).unwrap();
        assert!(persisted.transition_at(0).is_none());
        for index in 1..4 {
            assert_eq!(
                persisted.transition_at(index),
                Some(audition.motion.clone())
            );
        }
        assert_eq!(
            persisted.show["layouts"]["main"]["slides"][2]["transition"]["custom"]["pluginData"]["keep"],
            true
        );
        assert!(session.undo());
        assert!(session.current_show().unwrap().transition_at(3).is_none());
        assert!(session.redo());
        session
            .inspector_set("slide-transition", "default")
            .unwrap();
        assert_eq!(session.selected_slide_motion().1, "Library default");
        session.apply_slide_motion_range(1, 4).unwrap();
        assert!((0..4).all(|index| {
            session
                .current_show()
                .unwrap()
                .transition_at(index)
                .is_none()
        }));
        assert_eq!(crate::live::audience_frame(&session), on_air);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_background_uses_the_stored_fit_and_flip() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-media-fit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        std::fs::create_dir_all(root.join("Config")).unwrap();
        let picture = root.join("bg.png");
        std::fs::write(
            &picture,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99,
                248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ],
        )
        .unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"outputs":{"default":{"active":true,"style":"main"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"styles":{"main":{"fit":"fill"}}}"#,
        )
        .unwrap();
        assert!(session.play_named(&picture.to_string_lossy()));
        assert_eq!(session.background_fit, Fit::Fill);
        assert!(!session.background_flip_x);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "fit": "cover", "flipped": true, "flippedY": true }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&picture.to_string_lossy()));
        assert_eq!(session.background_fit, Fit::Cover);
        assert!(session.background_flip_x);
        assert!(session.background_flip_y);
        assert!(session.background_filter.is_empty());
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "filter": "grayscale(1)", "blend": "multiply" }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&picture.to_string_lossy()));
        assert_eq!(session.background_filter, "grayscale(1)");
        assert_eq!(session.background_blend, "multiply");
        std::fs::write(
            root.join("Config").join("settings_synced.json"),
            r#"{"styles":{"main":{"fit":"fill","blurAmount":10,"blurOpacity":0.5}}}"#,
        )
        .unwrap();
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "fit": "blur" }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&picture.to_string_lossy()));
        assert_eq!(session.background_fit, Fit::Blur);
        assert!((session.background_blur_px - 10.0).abs() < 0.001);
        assert!((session.background_blur_opacity - 0.5).abs() < 0.001);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "cropping": { "left": 1 } }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.play_named(&picture.to_string_lossy()));
        assert_eq!(session.underlay.as_ref().map(|image| image.width), Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_audio_file_uses_its_stored_volume() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-audio-gain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        let song = root.join("song.mp3");
        std::fs::write(&song, b"not a real song").unwrap();
        assert!(session.play_named(&song.to_string_lossy()));
        assert!((session.audio_routes()[0].gain - 1.0).abs() < 0.001);
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        session.volume = 0.5;
        assert!((session.audio_routes()[0].gain - 0.2).abs() < 0.001);
        assert!(!session.audio_should_loop());
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4, "loop": true }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.audio_should_loop());
        assert_eq!(session.audio_span(), (0.0, 0.0));
        assert!((session.audio_routes()[0].gain - 0.2).abs() < 0.001);
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4, "loop": true, "fromTime": 2, "toTime": 9 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert_eq!(session.audio_span(), (2.0, 9.0));
        assert!((session.audio_tempo() - 1.0).abs() < 0.001);
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4, "tempo": 1.5 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!((session.audio_tempo() - 1.5).abs() < 0.001);
        assert!((session.audio_routes()[0].gain - 0.2).abs() < 0.001);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn icecast_slide_and_remote_toggles_keep_the_saved_connection_settings() {
        let root = std::env::temp_dir().join(freeshow_data::new_id());
        let library = Library::open(&root);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(library.settings_path(), r#"{
            "audioRouting":{"connections":[{"from":"main","to":"icecast"}]},
            "special":{"icecast":{"host":"stream.local","port":"8123","mount":"live.opus","password":"test","tls":true}}
        }"#).unwrap();
        let mut session = Session::open(root.clone());
        let (configured, enabled) = session.icecast.as_ref().unwrap();
        assert!(*enabled);
        assert_eq!(configured.tls, Some(true));
        assert_eq!(
            (configured.port, configured.mount.as_str()),
            (8123, "/live.opus")
        );
        let mut show = ShowFile::blank("test", "Test");
        show.set_actions_at(
            0,
            &[serde_json::json!({
                "triggers": ["toggle_icecast"],
                "actionValues": {"toggle_icecast":{"value":"false"}}
            })],
        );
        session.shows = vec![show];
        session.show_index = Some(0);
        session.slide_index = Some(0);
        session.apply_icecast_actions();
        assert!(!library.icecast().unwrap().1);
        session.apply_remote(freeshow_remote::Command::ToggleIcecast(Some(true)));
        let (saved, enabled) = library.icecast().unwrap();
        assert!(enabled);
        assert_eq!(saved.tls, Some(true));
        assert_eq!(
            (saved.host.as_str(), saved.password.as_str()),
            ("stream.local", "test")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_import_refresh_keeps_the_open_show_and_project_cursor() {
        let root = std::env::temp_dir().join(freeshow_data::new_id());
        let library = Library::open(&root);
        ShowFile::blank("live", "Z Live")
            .save(&library.shows_dir().join("live.show"))
            .unwrap();
        let mut projects = ProjectsFile::empty();
        projects.add_project("active", "Active service");
        projects.append_show("active", "live");
        projects.append_show("active", "live");
        projects.save(&library.projects_path()).unwrap();
        let mut session = Session::open(root.clone());
        session.show_index = Some(0);
        session.slide_index = Some(0);
        session.project_item = 1;

        ShowFile::blank("lesson", "A Lesson")
            .save(&library.shows_dir().join("lesson.show"))
            .unwrap();
        projects.add_project("lessons", "Lessons.church");
        projects.append_show("lessons", "lesson");
        projects.save(&library.projects_path()).unwrap();

        session.reload_import_catalog(Some("lessons".into()));
        assert_eq!(session.shows[0].id, "lesson");
        assert_eq!(session.current_show().unwrap().id, "live");
        assert_eq!(session.slide_index, Some(0));
        assert_eq!(session.project_id.as_deref(), Some("active"));
        assert_eq!(session.project_item, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_refresh_preserves_external_changes_and_keeps_unrelated_undo() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("a", "Alpha", "Alpha original"));
        assert!(session.save_lyrics("Alpha edit"));
        let alpha = session.current_show().unwrap().path.clone().unwrap();
        assert!(session.create_show_with("b", "Bravo", "Bravo original"));
        assert!(session.save_lyrics("Bravo first edit"));
        assert!(session.save_lyrics("Bravo second edit"));
        assert!(session.undo());
        assert_eq!(session.history.len(), 2);
        assert_eq!(session.redo.len(), 1);
        let mut external = session.current_show().unwrap().clone();
        let bravo = external.path.clone().unwrap();
        external.show["externalField"] = serde_json::json!({"preserve": true});
        external.save(&bravo).unwrap();

        session.reload_import_catalog(None);
        assert_eq!(session.current_show().unwrap().show, external.show);
        assert_eq!(session.history.len(), 1);
        assert!(!session.can_redo());
        assert!(session.undo());
        assert_eq!(session.current_show().unwrap().id, "a");
        assert_eq!(
            ShowFile::load(&alpha).unwrap().lyrics_text(),
            "Alpha original"
        );
        assert_eq!(ShowFile::load(&bravo).unwrap().show, external.show);
    }

    #[test]
    fn autosave_writes_a_changed_show_once() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let path = session.shows[0].path.clone().unwrap();
        let show = &mut session.shows[0];
        session
            .timer_save_bases
            .insert(show.id.clone(), show.show.clone());
        show.show["name"] = serde_json::json!("Changed");
        session.reload_import_catalog(None);
        assert_eq!(session.shows[0].name(), "Changed");
        let backups = root.join("Backups");
        let before = std::fs::read_dir(&backups)
            .map(|dir| dir.count())
            .unwrap_or(0);
        assert!(session.autosave() >= 1);
        assert_eq!(ShowFile::load(&path).unwrap().name(), "Changed");
        let after = std::fs::read_dir(&backups).unwrap().count();
        assert!(after > before);
        assert_eq!(session.autosave(), 0);
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), after);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_verse_becomes_the_output() {
        let mut session = Session::empty();
        session.apply_remote(freeshow_remote::Command::PresentVerse {
            reference: "John 3:16".into(),
            text: "For God so loved the world".into(),
        });
        assert_eq!(
            session.output_lines(),
            vec![
                "John 3:16".to_string(),
                "For God so loved the world".to_string()
            ]
        );
        assert_eq!(
            session.to_snapshot().scripture_lines,
            session.output_lines()
        );
    }

    #[test]
    fn an_imported_bible_is_on_the_remote_snapshot() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-scripture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Bibles")).unwrap();
        let bible = BibleFile::from_document(
            "kjv",
            serde_json::json!({
                "name": "KJV",
                "books": [{
                    "name": "John",
                    "number": 43,
                    "chapters": [{
                        "number": 3,
                        "verses": [{ "number": 16, "text": "For God so loved the world" }]
                    }]
                }]
            }),
        );
        std::fs::write(root.join("Bibles/KJV.fsb"), bible.to_json().unwrap()).unwrap();
        let session = Session::open(root.clone());
        let snapshot = session.to_snapshot();
        assert_eq!(snapshot.bibles[0].id, "kjv");
        assert_eq!(snapshot.bibles[0].books[0].name, "John");
        assert_eq!(
            snapshot.bibles[0].books[0].chapters[0].verses[0].text,
            "For God so loved the world"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_repeated_chorus_keeps_one_label_and_different_verses_are_numbered() {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.set_lyrics_text(
            "[Verse]\nOne\n\n[Chorus]\nSing\n\n[Verse]\nTwo\n\n[Chorus]\nSing",
            0
        ));
        let labels: Vec<_> = slide_rows_for(&show.ordered_slides(), true)
            .into_iter()
            .map(|row| row.text)
            .collect();
        assert!(labels[0].starts_with("Verse 1"), "{labels:?}");
        assert!(labels[1].starts_with("Chorus") && !labels[1].starts_with("Chorus 2"));
        assert!(labels[2].starts_with("Verse 2"));
        assert_eq!(labels[1], labels[3]);
    }

    #[test]
    fn the_now_playing_bar_pauses_resumes_and_stops_the_audio() {
        let mut session = Session::empty();
        assert!(session.now_playing().is_none());
        assert!(!session.audio_control("toggle"));
        session.playing_audio = Some(PathBuf::from("/tmp/Audio/prelude.mp3"));
        session.audio_time = 65.4;
        session.audio_duration = 130.0;
        let (name, time, progress, paused) = session.now_playing().unwrap();
        assert_eq!(
            (name.as_str(), time.as_str(), paused),
            ("prelude", "1:05 / 2:10", false)
        );
        assert!((progress - 0.503).abs() < 0.01);
        assert!(session.audio_control("toggle"));
        let pause = session.ui_commands.pop().unwrap();
        session.apply_remote(pause);
        assert!(session.audio_paused);
        assert!(session.audio_control("toggle"));
        assert!(matches!(
            session.ui_commands.pop(),
            Some(freeshow_remote::Command::Play { .. })
        ));
        assert!(session.audio_control("stop"));
        assert!(matches!(
            session.ui_commands.pop(),
            Some(freeshow_remote::Command::StopAudio)
        ));
    }

    #[test]
    fn arrow_keys_step_through_verses_past_the_search_results() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-verse-step-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Bibles")).unwrap();
        let verses = (15..=18)
            .map(
                |number| serde_json::json!({ "number": number, "text": format!("verse {number}") }),
            )
            .collect::<Vec<_>>();
        let bible = BibleFile::from_document(
            "kjv",
            serde_json::json!({
                "name": "KJV",
                "books": [{ "name": "John", "number": 43, "chapters": [{ "number": 3, "verses": verses }] }]
            }),
        );
        std::fs::write(root.join("Bibles/KJV.fsb"), bible.to_json().unwrap()).unwrap();
        let mut session = Session::open(root.clone());
        session.tab = "scripture".into();
        session.search("John 3:16");
        assert_eq!(session.verse_hits.len(), 1);
        session.select_slide(0);
        assert!(session.perform_shortcut("Right"));
        assert_eq!(session.output_lines()[0], "John 3:17");
        assert!(session.perform_shortcut("Right"));
        assert!(!session.perform_shortcut("Right"), "John 3 ends at 18");
        assert_eq!(session.output_lines()[0], "John 3:18");
        assert_eq!(session.tab, "scripture", "the project did not take over");
        for _ in 0..3 {
            assert!(session.perform_shortcut("Left"));
        }
        assert_eq!(session.output_lines()[0], "John 3:15");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_rows_name_their_kind_and_mark_the_song_on_air() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-project-kinds-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Config")).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show_with("song1", "Call to worship", "Come\n\nSing"));
        let project_id = session.project_id.clone().unwrap();
        let mut json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("Config/projects.json")).unwrap(),
        )
        .unwrap();
        let rows = json["projects"][&project_id]["shows"]
            .as_array_mut()
            .unwrap();
        rows.insert(
            0,
            serde_json::json!({ "id": "s", "type": "section", "name": "Worship" }),
        );
        rows.push(serde_json::json!({ "id": "/a/b.mp4", "type": "video", "name": "b.mp4" }));
        std::fs::write(root.join("Config/projects.json"), json.to_string()).unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.project_kinds(), vec!["Section", "", "Video"]);
        assert_eq!(session.live_project_item(), None);
        assert!(session.open_project_item(1, Some(0)));
        let _ = crate::live::audience_frame(&session);
        assert_eq!(session.live_project_item(), Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_lyric_edit_is_saved_in_the_project() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-remote-show-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show_with("song1", "Call to worship", "Come\n\nSing"));
        assert_eq!(session.shows[0].ordered_slides().len(), 2);
        assert_eq!(session.project_labels().last().unwrap(), "Call to worship");
        assert!(session.set_show_text("song1", "Verse one\n\nChorus"));
        let again = Session::open(root.clone());
        assert_eq!(
            again.shows[0].ordered_slides()[1].lines,
            vec!["Chorus".to_string()]
        );
        assert_eq!(again.to_snapshot().projects[0].shows[0].kind, "show");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_play_finds_the_library_file() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-remote-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Media")).unwrap();
        std::fs::write(root.join("Media/clip.mp4"), b"not a real video").unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.play_named("clip.mp4"));
        match session.pending_play {
            Some(PendingPlay::Video(ref path)) => {
                assert_eq!(path, &root.join("Media/clip.mp4"));
            }
            other => panic!("expected a video, got {other:?}"),
        }
        session.apply_remote(freeshow_remote::Command::SetVolume { thousandths: 250 });
        assert_eq!(session.volume, 0.25);
        assert_eq!(session.to_snapshot().media[0].kind, "video");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_picture_from_the_media_drawer_goes_behind_the_live_song() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-media-behind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Media")).unwrap();
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207,
            192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];
        std::fs::write(root.join("Media/bg.png"), png).unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "One\n\nTwo"));
        session.select_slide(0);
        let _ = crate::live::audience_frame(&session);
        session.tab = "media".into();
        assert!(matches!(session.select_drawer(0), DrawerAction::Present));
        assert!(session.underlay.is_some());
        assert!(!session.slide_hidden, "the lyrics stay over the picture");
        let _ = crate::live::audience_frame(&session);
        assert_eq!(session.live_slide_index(), Some(0));
        std::fs::write(
            root.join("media.json"),
            serde_json::json!({ root.join("Media/bg.png").to_string_lossy(): { "videoType": "foreground" } })
                .to_string(),
        )
        .unwrap();
        session.select_drawer(0);
        assert!(
            session.slide_hidden,
            "a foreground picture replaces the song"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_media_drawer_labels_library_pictures() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-media-drawer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Media")).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&48u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(root.join("Media/bg.png"), png).unwrap();
        std::fs::write(root.join("Media/clip.mp4"), b"not a real video").unwrap();
        let mut session = Session::open(root.clone());
        session.tab = "media".into();
        assert_eq!(&session.drawer()[..2], ["bg.png  64x48", "clip.mp4"]);
        assert_eq!(session.to_snapshot().media[0].kind, "image");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pausing_audio_keeps_the_file_so_play_can_resume() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-remote-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Audio")).unwrap();
        std::fs::write(root.join("Audio/song.wav"), b"RIFF").unwrap();
        let mut session = Session::open(root.clone());
        assert!(session.play_named("song.wav"));
        assert!(matches!(session.pending_play, Some(PendingPlay::Audio(_))));
        session.apply_remote(freeshow_remote::Command::PauseAudio {
            name: "other.wav".into(),
        });
        assert!(!session.audio_paused);
        session.apply_remote(freeshow_remote::Command::PauseAudio {
            name: "song.wav".into(),
        });
        assert!(session.audio_paused);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::PauseAudio)
        ));
        assert!(session.play_named("song.wav"));
        assert!(!session.audio_paused);
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::ResumeAudio)
        ));
        session.apply_remote(freeshow_remote::Command::SeekAudio { millis: 1500 });
        assert!(matches!(
            session.pending_play,
            Some(PendingPlay::SeekAudio(1500))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_template_from_the_library_restyles_the_open_show() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let library = freeshow_data::Library::open(&root);
        std::fs::create_dir_all(library.templates_path().parent().unwrap()).unwrap();
        std::fs::write(
            library.templates_path(),
            r#"{"big":{"name":"Big","items":[{"style":"top:10px;","align":"center","lines":[{"align":"center","text":[{"value":"","style":"font-size:120px;"}]}]}]}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.templates, vec!["Big".to_string()]);
        assert!(session.create_show_with("song1", "Call", "Hello"));
        session.apply_remote(freeshow_remote::Command::SetTemplate { id: "big".into() });
        assert_eq!(
            session.shows[0]
                .show
                .pointer("/slides/s1/items/0/lines/0/text/0/style")
                .and_then(|value| value.as_str()),
            Some("font-size:120px;")
        );
        assert_eq!(
            session.shows[0]
                .show
                .pointer("/slides/s1/items/0/lines/0/text/0/value")
                .and_then(|value| value.as_str()),
            Some("Hello")
        );
        let again = Session::open(root.clone());
        assert_eq!(
            again.shows[0]
                .show
                .pointer("/settings/template")
                .and_then(|value| value.as_str()),
            Some("big")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_named_timer_from_the_library_drives_the_stage_clock() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-timer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let library = freeshow_data::Library::open(&root);
        std::fs::create_dir_all(library.synced_settings_path().parent().unwrap()).unwrap();
        std::fs::write(
            library.synced_settings_path(),
            r#"{"timers":{"sermon":{"name":"Sermon","type":"counter","start":90,"end":0}}}"#,
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        assert_eq!(session.timers.len(), 1);
        assert_eq!(session.timers[0].id, "sermon");
        assert!(session.start_named_timer("sermon"));
        assert!(session.timer_label().starts_with("01:"));
        assert!(session.pause_named_timer("sermon"));
        let mark = session.running.as_ref().unwrap().mark;
        assert!(mark == 90 || mark == 89);
        assert!(session.running.as_ref().unwrap().since.is_none());
        assert!(session.stop_named_timer("sermon"));
        assert!(session.running.is_none());
        assert_eq!(session.timer_label(), "05:00");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_calendar_action_starts_its_show_when_the_time_arrives() {
        let mut session = Session::empty();
        session
            .shows
            .push(ShowFile::blank("grace", "Amazing Grace"));
        let now = chrono::Utc::now();
        session.event_catalog = Catalog::parse(&format!(
            r#"{{"sunday":{{"name":"Sunday","type":"action","from":"{}","action":{{"id":"start_show","data":{{"id":"grace"}}}}}},"later":{{"name":"Later","type":"show","from":"{}","show":"grace"}},"old":{{"name":"Old","type":"action","from":"{}","action":{{"id":"start_show","data":{{"id":"grace"}}}}}}}}"#,
            now.to_rfc3339(),
            (now + chrono::Duration::seconds(30)).to_rfc3339(),
            (now - chrono::Duration::seconds(5)).to_rfc3339(),
        ))
        .unwrap();
        assert!(session.start_due_events(now));
        assert_eq!(session.show_index, Some(0));
        assert_eq!(session.slide_index, Some(0));
        assert!(!session.blanked);
        assert!(!session.start_due_events(now));
        session.fired_events.clear();
        session.show_index = None;
        assert!(!session.start_due_events(now + chrono::Duration::seconds(5)));
    }
}
