// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use freeshow_data::{
    AUDIO_DIR, BibleFile, Catalog, Library, MEDIA_DIR, OutlinedBook, ProjectsFile,     MixStep, ShowFile, SlideBackdrop, SlideJump, TimerStep,
    StageLayout, VerseHit, default_stage, new_id,
};
use freeshow_media::{MediaKind, cameras, media_kind};
use freeshow_output::{Fit, Raster, crop_raster, decode_image};

/// Linear gain from a volume action. A value above 5 is a percent. Decibels convert to gain.
#[derive(Clone)]
struct PendingActions {
    due: Instant,
    show_id: String,
    slide_index: usize,
    actions: Vec<serde_json::Value>,
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
    PlayAudio(PathBuf),
    PlayVideo(PathBuf),
    PlayCamera(String),
}

pub struct Session {
    pub root: Option<PathBuf>,
    pub shows: Vec<ShowFile>,
    pub projects: ProjectsFile,
    pub project_id: Option<String>,
    pub project_item: usize,
    folder_paths: Vec<PathBuf>,
    folder_index: usize,
    pub tab: String,
    pub show_index: Option<usize>,
    pub slide_index: Option<usize>,
    pub templates: Vec<String>,
    pub template_catalog: Catalog,
    pub overlays: Vec<String>,
    pub overlay_catalog: Catalog,
    pub active_overlays: Vec<String>,
    pub bibles: Vec<BibleFile>,
    pub scripture: Arc<Vec<freeshow_remote::RemoteBible>>,
    pub media: Vec<String>,
    pub audio: Vec<String>,
    pub events: Vec<String>,
    pub event_catalog: Catalog,
    pub fired_events: Vec<String>,
    pub verse_hits: Vec<VerseHit>,
    pub verse_index: Option<usize>,
    pub bible_index: Option<usize>,
    pub media_index: Option<usize>,
    pub audio_index: Option<usize>,
    pub underlay: Option<Raster>,
    pub background_fit: Fit,
    pub background_flip_x: bool,
    pub background_flip_y: bool,
    pub background_filter: String,
    pub background_blend: String,
    pub background_blur_px: f32,
    pub background_blur_opacity: f32,
    /// The next output change is a background image, so it uses the media transition.
    pub background_transition: bool,
    /// Path of the picture currently behind the slide, when one is showing.
    background_path: Option<PathBuf>,
    pub volume: f32,
    /// The main channel is muted. Playback stays silent until a slide unmutes it.
    pub audio_muted: bool,
    /// Volume restored when a slide toggles the main channel back on.
    unmuted_volume: f32,
    pub pending_play: Option<PendingPlay>,
    /// A second playback for the same slide, so a video and an audio file can both start.
    pub pending_also: Option<PendingPlay>,
    pub playing_audio: Option<PathBuf>,
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
    pub stage: StageLayout,
    pub timer_left: u64,
    pub timer_deadline: Option<Instant>,
    pub timers: Vec<NamedTimer>,
    pub running: Option<RunningTimer>,
    pub recording: bool,
    pub warnings: Vec<String>,
}

impl Session {
    pub fn empty() -> Self {
        Self {
            root: None,
            shows: Vec::new(),
            projects: ProjectsFile::empty(),
            project_id: None,
            project_item: 0,
            folder_paths: Vec::new(),
            folder_index: 0,
            tab: "shows".to_string(),
            show_index: None,
            slide_index: None,
            templates: Vec::new(),
            template_catalog: Catalog::empty(),
            overlays: Vec::new(),
            overlay_catalog: Catalog::empty(),
            active_overlays: Vec::new(),
            bibles: Vec::new(),
            scripture: Arc::new(Vec::new()),
            media: Vec::new(),
            audio: Vec::new(),
            events: Vec::new(),
            event_catalog: Catalog::empty(),
            fired_events: Vec::new(),
            verse_hits: Vec::new(),
            verse_index: None,
            bible_index: None,
            media_index: None,
            audio_index: None,
            underlay: None,
            background_fit: Fit::Contain,
            background_flip_x: false,
            background_flip_y: false,
            background_filter: String::new(),
            background_blend: String::new(),
            background_blur_px: 6.0,
            background_blur_opacity: 0.3,
            background_transition: false,
            background_path: None,
            volume: 1.0,
            audio_muted: false,
            unmuted_volume: 1.0,
            pending_play: None,
            pending_also: None,
            playing_audio: None,
            playlist_id: None,
            playlist_index: 0,
            playlist_songs: Vec::new(),
            playlist_loop: true,
            playlist_volume: 1.0,
            effect_playing: None,
            effect_return: None,
            media_finished: None,
            refresh_output: false,
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
            stage: default_stage(),
            timer_left: 300,
            timer_deadline: None,
            timers: vec![NamedTimer::default_countdown()],
            running: None,
            recording: false,
            warnings: Vec::new(),
        }
    }

    pub fn open(root: PathBuf) -> Self {
        let library = Library::open(&root);
        let mut warnings = Vec::new();
        let shows = take(library.list_shows(), &mut warnings).unwrap_or_default();
        let projects =
            take(library.load_projects(), &mut warnings).unwrap_or_else(ProjectsFile::empty);
        let project_id = projects
            .projects()
            .first()
            .map(|project| project.id.clone());
        let template_catalog =
            take(library.load_templates(), &mut warnings).unwrap_or_else(Catalog::empty);
        let templates = template_catalog.names();
        let overlay_catalog =
            take(library.load_overlays(), &mut warnings).unwrap_or_else(Catalog::empty);
        let overlays = overlay_catalog.names();
        if let Ok(notes) = library.import_scripture_sources() {
            warnings.extend(notes);
        }
        if let Ok(notes) = library.import_show_sources() {
            warnings.extend(notes);
        }
        let bibles = take(library.list_bibles(), &mut warnings).unwrap_or_default();
        let scripture = scripture_catalog(&bibles);
        let mut media = take(
            library.list_media_files(&library.media_dir()),
            &mut warnings,
        )
        .unwrap_or_default();
        for camera in cameras() {
            media.push(format!("Camera: {camera}"));
        }
        let audio = take(
            library.list_media_files(&library.audio_dir()),
            &mut warnings,
        )
        .unwrap_or_default();
        let stage = library.load_stage_layout();
        let event_catalog =
            take(library.load_events(), &mut warnings).unwrap_or_else(Catalog::empty);
        let events = event_catalog.names();
        let synced =
            take(library.load_synced_settings(), &mut warnings).unwrap_or_else(Catalog::empty);
        let timers = named_timers(&synced);
        let (volume, audio_muted) = library.main_audio();
        Self {
            root: Some(root),
            shows,
            projects,
            project_id,
            project_item: 0,
            folder_paths: Vec::new(),
            folder_index: 0,
            tab: "shows".to_string(),
            show_index: None,
            slide_index: None,
            templates,
            template_catalog,
            overlays,
            overlay_catalog,
            active_overlays: Vec::new(),
            bibles,
            scripture,
            media,
            audio,
            events,
            event_catalog,
            fired_events: Vec::new(),
            verse_hits: Vec::new(),
            verse_index: None,
            bible_index: None,
            media_index: None,
            audio_index: None,
            underlay: None,
            background_fit: Fit::Contain,
            background_flip_x: false,
            background_flip_y: false,
            background_filter: String::new(),
            background_blend: String::new(),
            background_blur_px: 6.0,
            background_blur_opacity: 0.3,
            background_transition: false,
            background_path: None,
            volume,
            audio_muted,
            unmuted_volume: if volume > 0.0 { volume } else { 1.0 },
            pending_play: None,
            pending_also: None,
            playing_audio: None,
            playlist_id: None,
            playlist_index: 0,
            playlist_songs: Vec::new(),
            playlist_loop: true,
            playlist_volume: 1.0,
            effect_playing: None,
            effect_return: None,
            media_finished: None,
            refresh_output: false,
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
            stage,
            timer_left: 300,
            timer_deadline: None,
            timers,
            running: None,
            recording: false,
            warnings,
        }
    }

    pub fn timer_label(&self) -> String {
        let seconds = self
            .running
            .as_ref()
            .map(RunningTimer::shown)
            .unwrap_or_else(|| self.timer_remaining());
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
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
        if self.running.as_ref().is_some_and(|running| running.id == id) {
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
        self.blanked = false;
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
        self.blanked = false;
        self.slide_hidden = false;
        self.show_index = Some(index);
        self.slide_index = Some(slide.min(len - 1));
        self.reveal_count = 0;
        self.click_revealed = false;
        self.line_index = 0;
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

    pub fn drawer(&self) -> Vec<String> {
        match self.tab.as_str() {
            "shows" => self.shows.iter().map(ShowFile::name).collect(),
            "media" => self.media.clone(),
            "audio" => self.audio.clone(),
            "overlays" => self.overlays.clone(),
            "templates" => self.templates.clone(),
            "scripture" => self.bibles.iter().map(BibleFile::name).collect(),
            "calendar" => self.events.clone(),
            "functions" => vec![
                "Clear".to_string(),
                "Next".to_string(),
                "Previous".to_string(),
                "Timer".to_string(),
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
            ],
            _ => Vec::new(),
        }
    }

    pub fn slide_labels(&self) -> Vec<String> {
        if self.tab == "scripture" {
            return self
                .verse_hits
                .iter()
                .map(|hit| format!("{}  {}", hit.reference, hit.text))
                .collect();
        }
        if self.tab != "shows" {
            return Vec::new();
        }
        self.current_show()
            .map(|show| show.ordered_slides().iter().map(slide_label).collect())
            .unwrap_or_default()
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
        show.plain_text(&slide.id)
    }

    pub fn select_drawer(&mut self, index: usize) -> DrawerAction {
        match self.tab.as_str() {
            "shows" => {
                if index < self.shows.len() {
                    self.show_index = Some(index);
                    self.slide_index = self.current_show().and_then(|show| {
                        if show.ordered_slides().is_empty() {
                            None
                        } else {
                            Some(0)
                        }
                    });
                }
                DrawerAction::None
            }
            "scripture" => {
                if index < self.bibles.len() {
                    self.bible_index = Some(index);
                    self.verse_hits.clear();
                    self.verse_index = None;
                }
                DrawerAction::None
            }
            "media" => self.choose_media(index),
            "audio" => self.choose_audio(index),
            "overlays" => self.toggle_overlay(index),
            "templates" => self.choose_template(index),
            _ => DrawerAction::None,
        }
    }

    pub fn clear_output(&mut self) {
        self.underlay = None;
        self.reset_background_look();
        self.playing_audio = None;
        self.audio_paused = false;
        self.video_time = 0.0;
        self.video_duration = 0.0;
        self.audio_time = 0.0;
        self.audio_duration = 0.0;
    }

    fn choose_template(&mut self, index: usize) -> DrawerAction {
        let Some(name) = self.templates.get(index).cloned() else {
            return DrawerAction::None;
        };
        let Some(id) = self.template_catalog.id_named(&name) else {
            return DrawerAction::None;
        };
        if self.apply_template_id(&id) {
            DrawerAction::Present
        } else {
            DrawerAction::None
        }
    }

    fn apply_template_id(&mut self, id: &str) -> bool {
        let Some(template) = self.template_catalog.get(id).cloned() else {
            return false;
        };
        let Some(show_id) = self.current_show().map(|show| show.id.clone()) else {
            return false;
        };
        let mut applied = false;
        self.edit_show(&show_id, |show| {
            applied = show.apply_template(id, &template);
            applied
        });
        applied
    }

    fn toggle_overlay(&mut self, index: usize) -> DrawerAction {
        let Some(name) = self.overlays.get(index).cloned() else {
            return DrawerAction::None;
        };
        let Some(id) = self.overlay_catalog.id_named(&name) else {
            return DrawerAction::None;
        };
        if let Some(pos) = self.active_overlays.iter().position(|active| active == &id) {
            self.active_overlays.remove(pos);
        } else {
            self.active_overlays.push(id);
        }
        DrawerAction::Present
    }

    fn choose_media(&mut self, index: usize) -> DrawerAction {
        let Some(name) = self.media.get(index).cloned() else {
            return DrawerAction::None;
        };
        self.media_index = Some(index);
        if let Some(camera) = name.strip_prefix("Camera: ") {
            self.underlay = None;
            self.reset_background_look();
            self.background_transition = true;
            return DrawerAction::PlayCamera(camera.to_string());
        }
        let Some(path) = self.file_in(MEDIA_DIR, &name) else {
            return DrawerAction::None;
        };
        match media_kind(&path) {
            MediaKind::Image => {
                self.remember_background(&path);
                self.underlay = self.load_still(&path);
                self.background_transition = true;
                self.note_media_layer(&path, MediaKind::Image);
                DrawerAction::Present
            }
            MediaKind::Video => {
                self.media_finished = None;
                self.remember_background(&path);
                self.remember_video_playback(&path);
                self.underlay = None;
                self.note_media_layer(&path, MediaKind::Video);
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

    fn edit_show(&mut self, id: &str, edit: impl FnOnce(&mut ShowFile) -> bool) {
        let Some(index) = self.shows.iter().position(|show| show.id == id) else {
            return;
        };
        let saved = {
            let Some(show) = self.shows.get_mut(index) else {
                return;
            };
            if !edit(show) {
                return;
            }
            match show.path.clone() {
                Some(path) => show.save(&path).is_ok(),
                None => true,
            }
        };
        if saved {
            self.tab = "shows".to_string();
            self.show_index = Some(index);
            self.blanked = false;
        }
    }

    pub fn play_named(&mut self, name: &str) -> bool {
        if let Some(camera) = name.strip_prefix("Camera: ") {
            self.blanked = false;
            self.media_paused = false;
            self.background_transition = true;
            self.pending_play = Some(PendingPlay::Camera(camera.to_string()));
            return true;
        }
        if let Some(path) = self
            .file_in(MEDIA_DIR, name)
            .or_else(|| self.file_in(AUDIO_DIR, name))
        {
            return self.play_path(path);
        }
        let path = PathBuf::from(name);
        if path.is_file() {
            return self.play_path(path);
        }
        false
    }

    fn play_path(&mut self, path: PathBuf) -> bool {
        self.media_paused = false;
        match media_kind(&path) {
            MediaKind::Image => {
                self.remember_background(&path);
                self.underlay = self.load_still(&path);
                self.blanked = false;
                self.background_transition = true;
                self.pending_play = None;
                self.note_media_layer(&path, MediaKind::Image);
                true
            }
            MediaKind::Video => {
                self.media_finished = None;
                self.remember_background(&path);
                self.remember_video_playback(&path);
                self.underlay = None;
                self.blanked = false;
                self.note_media_layer(&path, MediaKind::Video);
                self.background_transition = true;
                self.pending_play = Some(PendingPlay::Video(path));
                true
            }
            MediaKind::Audio => {
                if self.audio_paused && self.playing_audio.as_ref() == Some(&path) {
                    self.audio_paused = false;
                    self.pending_play = Some(PendingPlay::ResumeAudio);
                } else {
                    self.media_finished = None;
                    self.playing_audio = Some(path.clone());
                    self.audio_paused = false;
                    self.pending_play = Some(PendingPlay::Audio(path));
                }
                true
            }
            MediaKind::Other => false,
        }
    }

    fn remember_background(&mut self, path: &Path) {
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
        let Some(item) = self.current_project().and_then(|project| {
            project.shows.get(self.project_item).cloned()
        }) else {
            return (None, None);
        };
        if item.kind != "video" || item.id != path.to_string_lossy() {
            return (None, None);
        }
        (item.loops, item.muted)
    }

    /// A foreground picture or a one-shot video replaces the song. A background video keeps it.
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
    fn play_slide_video(&mut self, path: &Path) {
        let flags = self
            .current_show()
            .and_then(|show| show.media_flags(&path.to_string_lossy()));
        if self.background_path.as_deref() == Some(path) {
            self.background_transition = false;
            if let Some((_, _, video_type)) = &flags {
                if video_type.is_some() {
                    self.slide_hidden = video_type.as_deref() == Some("foreground");
                }
            }
            return;
        }
        self.media_finished = None;
        self.remember_background(path);
        self.remember_video_playback(path);
        if let Some((loops, muted, _)) = flags {
            self.media_loop = loops;
            self.media_muted = muted;
        }
        self.underlay = None;
        self.blanked = false;
        self.background_path = Some(path.to_path_buf());
        self.background_transition = true;
        let show_type = self
            .current_show()
            .and_then(|show| show.media_flags(&path.to_string_lossy()))
            .and_then(|(_, _, video_type)| video_type);
        if show_type.is_some() {
            self.slide_hidden = show_type.as_deref() == Some("foreground");
        } else {
            self.slide_hidden = self.root.as_deref().is_some_and(|root| {
                Library::open(root).media_video_type(path) == "foreground"
            });
        }
        self.pending_also = None;
        self.pending_play = Some(PendingPlay::Video(path.to_path_buf()));
    }

    fn load_still(&self, path: &Path) -> Option<Raster> {
        let image = std::fs::read(path).ok().and_then(|bytes| decode_image(&bytes))?;
        let (left, right, top, bottom) = self
            .root
            .as_deref()
            .map(|root| Library::open(root).media_crop(path))
            .unwrap_or((0, 0, 0, 0));
        Some(crop_raster(&image, left, right, top, bottom))
    }

    fn reset_background_look(&mut self) {
        self.background_fit = Fit::Contain;
        self.background_flip_x = false;
        self.background_flip_y = false;
        self.background_filter.clear();
        self.background_blend.clear();
        self.background_blur_px = 6.0;
        self.background_blur_opacity = 0.3;
        self.background_path = None;
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
        let jump = self.current_show().and_then(|show| {
            self.slide_index.and_then(|index| show.slide_jump_at(index))
        });
        let Some(jump) = jump else {
            return;
        };
        match jump {
            SlideJump::Index { show_id, index } => {
                if let Some(show_id) = show_id {
                    if self.current_show().is_some_and(|show| show.id != show_id) {
                        if self.open_show_at(&show_id, index) {
                            self.follow_started_show(0);
                            self.follow_slide_jump(depth + 1);
                        }
                        return;
                    }
                }
                self.jump_to_slide(index, depth);
            }
            SlideJump::Name(name) => {
                let Some(index) = self
                    .current_show()
                    .and_then(|show| show.group_index(&name))
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
        let Some(next) = self.current_show().and_then(|show| {
            show.step_output(current, delta, &output_id, &output_name)
        }) else {
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
        self.blanked = false;
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
        self.pending_actions = None;
        self.apply_action_slice(None);
    }

    /// Runs one slice of the slide actions. A wait stores the rest until it is due.
    fn apply_action_slice(&mut self, replacement: Option<Vec<serde_json::Value>>) {
        let show_index = self.show_index;
        let slide_index = self.slide_index;
        let real = self.current_show().and_then(|show| {
            slide_index.map(|index| show.actions_at(index))
        });
        let using = replacement.unwrap_or_else(|| real.clone().unwrap_or_default());
        let pause = freeshow_data::pause_in_actions(&using);
        let run = pause
            .as_ref()
            .map(|pause| pause.before.clone())
            .unwrap_or(using);
        let holding = pause.is_some();
        if holding {
            if let (Some(show_index), Some(slide_index)) = (show_index, slide_index) {
                if let Some(show) = self.shows.get_mut(show_index) {
                    let _ = show.set_actions_at(slide_index, &run);
                }
            }
        }
        self.apply_slide_actions_now(false);
        if holding {
            if let (Some(show_index), Some(slide_index), Some(real)) =
                (show_index, slide_index, real)
            {
                if let Some(show) = self.shows.get_mut(show_index) {
                    let _ = show.set_actions_at(slide_index, &real);
                }
            }
        }
        let still_here = self.show_index == show_index && self.slide_index == slide_index;
        if still_here {
            if let Some(pause) = pause {
                if !pause.after.is_empty() {
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
        let same = self.current_show().is_some_and(|show| show.id == pending.show_id)
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
        let real = self.current_show().and_then(|show| {
            slide_index.map(|index| show.actions_at(index))
        });
        let pause = freeshow_data::pause_in_actions(&actions);
        let run = pause
            .as_ref()
            .map(|pause| pause.before.clone())
            .unwrap_or(actions);
        if let (Some(show_index), Some(slide_index)) = (show_index, slide_index) {
            if let Some(show) = self.shows.get_mut(show_index) {
                let _ = show.set_actions_at(slide_index, &run);
            }
        }
        self.apply_slide_actions_now(true);
        if let (Some(show_index), Some(slide_index), Some(real)) = (show_index, slide_index, real)
        {
            if let Some(show) = self.shows.get_mut(show_index) {
                let _ = show.set_actions_at(slide_index, &real);
            }
        }
        let still_here = self.show_index == show_index && self.slide_index == slide_index;
        if still_here {
            if let Some(pause) = pause {
                if !pause.after.is_empty() {
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
        }
    }

    /// Makes a stored wait due, then runs it. Used when a test does not sleep.
    pub fn finish_waits(&mut self) -> bool {
        if let Some(pending) = &mut self.pending_actions {
            pending.due = Instant::now();
        }
        self.run_due_actions()
    }

    fn apply_slide_actions_now(&mut self, continuation: bool) {
        if self.tab == "scripture" {
            return;
        }
        if !continuation {
        self.effect_playing = None;
        self.effect_return = None;
        self.timed_seconds = self.current_show().and_then(|show| {
            self.slide_index
                .and_then(|index| show.next_timer_at(index))
        });
        let backdrop = self.current_show().and_then(|show| {
            self.slide_index.map(|index| show.backdrop_at(index))
        });
        if let Some(backdrop) = backdrop {
            match backdrop {
                SlideBackdrop::Unchanged => {}
                SlideBackdrop::Clear => {
                    self.underlay = None;
                    self.reset_background_look();
                    self.slide_hidden = false;
                    self.background_transition = true;
                }
                SlideBackdrop::Picture { path, foreground } => {
                    let path = PathBuf::from(path);
                    if media_kind(&path) == MediaKind::Video {
                        self.play_slide_video(&path);
                    } else if media_kind(&path) == MediaKind::Image {
                        let file_foreground = self.root.as_deref().is_some_and(|root| {
                            Library::open(root).media_video_type(&path) == "foreground"
                        });
                        let foreground = foreground || file_foreground;
                        if self.background_path.as_ref() == Some(&path) && self.underlay.is_some() {
                            self.slide_hidden = foreground;
                            self.background_transition = false;
                        } else {
                            self.remember_background(&path);
                            self.underlay = self.load_still(&path);
                            if self.underlay.is_some() {
                                self.background_path = Some(path);
                                self.background_transition = true;
                                self.blanked = false;
                                self.slide_hidden = foreground;
                            }
                        }
                    }
                }
            }
        }
        }
        self.stop_slide_audio();
        self.start_slide_audio();
        self.start_slide_stream();
        self.apply_camera();
        self.apply_media_toggle();
        self.apply_template_action();
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
        self.apply_timer_steps();
        self.apply_mix_steps();
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
        let Some(root) = self.root.clone() else {
            return;
        };
        let library = Library::open(&root);
        for (output, layout_id) in steps {
            if !library.assign_stage_layout(output.as_deref(), &layout_id) {
                continue;
            }
            if let Some(layout) = library.stage_layout(&layout_id) {
                self.stage = layout;
            }
        }
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
        self.playing_audio = Some(path.clone());
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::Audio(path));
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
                    let gain = mix_gain(volume, use_db, self.volume, &mut self.unmuted_volume);
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
            .and_then(|show| self.slide_index.map(|index| show.cleared_overlays_at(index)))
            .unwrap_or_default();
        for id in ids {
            self.active_overlays.retain(|active| active != &id);
        }
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
        );
    }

    /// A clear action drops this slide's countdown. A set action rewrites every slide's timer.
    fn apply_timer_steps(&mut self) {
        let steps = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.timer_steps_at(index)))
            .unwrap_or_default();
        let show_index = self.show_index;
        for step in steps {
            match step {
                TimerStep::Clear => self.timed_seconds = None,
                TimerStep::Set(seconds) => {
                    if let Some(index) = show_index
                        && let Some(show) = self.shows.get_mut(index)
                    {
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
    fn audience(&self) -> (String, String) {
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
        let _ = Library::open(&root).set_output_style(&style);
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
        } else if pause {
            if let Some(running) = self.running.as_mut() {
                running.mark = running.shown();
                running.since = None;
            }
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
                self.slide_index.map(|index| {
                    (show.clears_overlays_at(index), show.overlays_at(index))
                })
            })
            .unwrap_or((false, Vec::new()));
        if clear {
            self.active_overlays.clear();
        }
        for id in ids {
            if self.overlay_catalog.get(&id).is_some()
                && !self.active_overlays.iter().any(|active| active == &id)
            {
                self.active_overlays.push(id);
            }
        }
    }

    /// Stops the playing audio when this slide asks for that.
    fn stop_slide_audio(&mut self) {
        let clear = self
            .current_show()
            .and_then(|show| self.slide_index.map(|index| show.clears_audio_at(index)))
            .unwrap_or(false);
        if !clear {
            return;
        }
        self.playing_audio = None;
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::StopAudio);
    }

    /// Keeps a slide video queued and stores the audio command beside it.
    fn keep_video_then(&mut self, play: PendingPlay) {
        if matches!(self.pending_play, Some(PendingPlay::Video(_))) {
            self.pending_also = Some(play);
        } else {
            self.pending_play = Some(play);
        }
    }

    pub fn take_plays(&mut self) -> [Option<PendingPlay>; 2] {
        [self.pending_play.take(), self.pending_also.take()]
    }

    /// Starts the first audio file listed on this slide, unless that file is already playing.
    fn start_slide_audio(&mut self) {
        let path = self.current_show().and_then(|show| {
            self.slide_index.and_then(|index| {
                show.audio_at(index).into_iter().find(|path| {
                    media_kind(Path::new(path)) == MediaKind::Audio
                })
            })
        });
        let Some(path) = path else {
            return;
        };
        let path = PathBuf::from(path);
        if self.playing_audio.as_ref() == Some(&path) && !self.audio_paused {
            return;
        }
        self.media_finished = None;
        self.playing_audio = Some(path.clone());
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::Audio(path));
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
    fn apply_template_action(&mut self) {
        let Some(id) = self.current_show().and_then(|show| {
            self.slide_index.and_then(|index| show.template_at(index))
        }) else {
            return;
        };
        let _ = self.apply_template_id(&id);
    }

    /// Starts the camera this slide names, and keeps a queued audio file beside it.
    fn apply_camera(&mut self) {
        let camera = self.current_show().and_then(|show| {
            self.slide_index.and_then(|index| show.camera_at(index))
        });
        let Some(camera) = camera else {
            return;
        };
        if matches!(&self.pending_play, Some(PendingPlay::Camera(current)) if current == &camera)
        {
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
        self.media_finished = None;
        self.playing_audio = Some(path.clone());
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::Audio(path));
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
        self.playing_audio = Some(path.clone());
        self.audio_paused = false;
        self.media_finished = None;
        self.keep_video_then(PendingPlay::Audio(path));
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
        self.playing_audio = Some(back.clone());
        self.audio_paused = false;
        self.keep_video_then(PendingPlay::Audio(back));
        true
    }

    /// Goes to the next slide when the file that just ended belongs to this slide.
    pub fn finish_media(&mut self, path: &str, audio: bool) -> bool {
        let key = (path.to_string(), audio);
        if self.media_finished.as_ref() == Some(&key) {
            return false;
        }
        self.media_finished = Some(key.clone());
        if !self.resume_after_effect(path, audio) {
            self.advance_playlist(path, audio);
        }
        let waits = self
            .current_show()
            .and_then(|show| {
                self.slide_index
                    .map(|index| show.waits_for_ended(index, path, audio))
            })
            .unwrap_or(false);
        if !waits {
            return false;
        }
        let advanced = self.advance_show();
        self.media_finished = Some(key);
        self.refresh_output = advanced;
        advanced
    }

    /// Main output gain times the volume stored for the file that is playing.
    pub fn playback_gain(&self) -> f32 {
        let file = self
            .playing_audio
            .as_ref()
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_volume(path))
            })
            .unwrap_or(1.0);
        if self.audio_muted {
            return 0.0;
        }
        let playlist = if self.playlist_id.is_some() {
            self.playlist_volume
        } else {
            1.0
        };
        (self.volume * file * playlist).clamp(0.0, 1.0)
    }

    /// The file that is playing repeats when `media.json` says so.
    pub fn audio_should_loop(&self) -> bool {
        self.playing_audio
            .as_ref()
            .and_then(|path| {
                self.root
                    .as_deref()
                    .map(|root| Library::open(root).media_loops(path))
            })
            .unwrap_or(false)
    }

    /// Stored start and end for the file that is playing. Zero is the file start or the file end.
    pub fn audio_span(&self) -> (f64, f64) {
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

    pub fn select_slide(&mut self, index: usize) {
        self.blanked = false;
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

    pub fn current_slide_text(&self) -> Option<String> {
        let show = self.show_index.and_then(|index| self.shows.get(index))?;
        let slides = show.ordered_slides();
        let slide = slides.get(self.slide_index?)?;
        Some(slide.lines.join("\n"))
    }

    /// Writes the selected slide even when another drawer tab is open.
    pub fn replace_slide_text(&mut self, text: &str) -> bool {
        let Some(slide_index) = self.slide_index else {
            return false;
        };
        let Some(show_index) = self.show_index else {
            return false;
        };
        let Some(show) = self.shows.get_mut(show_index) else {
            return false;
        };
        let Some(slide_id) = show
            .ordered_slides()
            .get(slide_index)
            .map(|slide| slide.id.clone())
        else {
            return false;
        };
        if !show.set_plain_text(&slide_id, text) {
            return false;
        }
        let Some(path) = show.path.clone() else {
            return true;
        };
        show.save(&path).is_ok()
    }

    pub fn save_lyrics(&mut self, text: &str) -> bool {
        if self.tab != "shows" {
            return false;
        }
        self.replace_slide_text(text)
    }

    pub fn save_group(&mut self, group: &str) -> bool {
        if self.tab != "shows" {
            return false;
        }
        let Some(slide_index) = self.slide_index else {
            return false;
        };
        let Some(show_index) = self.show_index else {
            return false;
        };
        let Some(show) = self.shows.get_mut(show_index) else {
            return false;
        };
        let Some(slide_id) = show
            .ordered_slides()
            .get(slide_index)
            .map(|slide| slide.id.clone())
        else {
            return false;
        };
        if !show.set_group(&slide_id, group) {
            return false;
        }
        let Some(path) = show.path.clone() else {
            return true;
        };
        show.save(&path).is_ok()
    }

    pub fn nudge_box(&mut self, dx: i32, dy: i32, dw: i32, dh: i32) -> bool {
        if self.tab != "shows" {
            return false;
        }
        let Some(slide_index) = self.slide_index else {
            return false;
        };
        let Some(show_index) = self.show_index else {
            return false;
        };
        let Some(show) = self.shows.get_mut(show_index) else {
            return false;
        };
        let Some(slide_id) = show
            .ordered_slides()
            .get(slide_index)
            .map(|slide| slide.id.clone())
        else {
            return false;
        };
        if !show.nudge_item_box(&slide_id, dx, dy, dw, dh) {
            return false;
        }
        let Some(path) = show.path.clone() else {
            return true;
        };
        show.save(&path).is_ok()
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
            .map(|slide| slide.lines.clone())
            .unwrap_or_default()
    }

    pub fn to_snapshot(&self) -> freeshow_remote::ShowSnapshot {
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
                            slides: layout
                                .slides
                                .into_iter()
                                .map(|slide| freeshow_remote::RemoteSlide {
                                    label: slide_label(&slide),
                                    lines: slide.lines,
                                    notes: slide.notes,
                                    id: slide.id,
                                    group: slide.group.unwrap_or_default(),
                                })
                                .collect(),
                        })
                        .collect::<Vec<_>>();
                    let active = show.active_layout();
                    let slides = layouts
                        .iter()
                        .find(|layout| layout.id == active)
                        .map(|layout| layout.slides.clone())
                        .unwrap_or_else(|| {
                            show.ordered_slides()
                                .into_iter()
                                .map(|slide| freeshow_remote::RemoteSlide {
                                    label: slide_label(&slide),
                                    lines: slide.lines,
                                    notes: slide.notes,
                                    id: slide.id,
                                    group: slide.group.unwrap_or_default(),
                                })
                                .collect()
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
            show_index: self.show_index,
            slide_index: self.slide_index,
            cleared: self.blanked,
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
            scripture_lines: if self.tab == "scripture" && !self.blanked {
                self.output_lines()
            } else {
                Vec::new()
            },
            overlays: overlay_views(&self.overlay_catalog),
            active_overlays: self.active_overlays.clone(),
            media: self
                .media
                .iter()
                .map(|name| file_view(name))
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
                self.blanked = false;
                self.select_drawer(index);
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
                self.blanked = true;
                self.active_overlays.clear();
                self.clear_output();
            }
            freeshow_remote::Command::ShowOverlay { id } => {
                if self.overlay_catalog.get(&id).is_some()
                    && !self.active_overlays.iter().any(|active| active == &id)
                {
                    self.active_overlays.push(id);
                }
            }
            freeshow_remote::Command::ClearOverlay { id } => {
                self.active_overlays.retain(|active| active != &id);
            }
            freeshow_remote::Command::ClearOverlays => {
                self.active_overlays.clear();
            }
            freeshow_remote::Command::Play { name } => {
                let _ = self.play_named(&name);
            }
            freeshow_remote::Command::SetVolume { thousandths } => {
                self.volume = f32::from(thousandths) / 1000.0;
            }
            freeshow_remote::Command::StopAudio => {
                self.playing_audio = None;
                self.audio_paused = false;
                self.pending_play = Some(PendingPlay::StopAudio);
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
            freeshow_remote::Command::ToggleMedia => {
                self.pending_play = Some(PendingPlay::ToggleMedia);
            }
            freeshow_remote::Command::RearrangeGroups {
                show_id,
                from,
                to,
            } => {
                self.edit_show(&show_id, |show| show.reorder_groups(from, to));
            }
            freeshow_remote::Command::AddGroup {
                show_id,
                group_id,
            } => {
                self.edit_show(&show_id, |show| show.duplicate_slide(&group_id).is_some());
            }
            freeshow_remote::Command::PresentVerse { reference, text } => {
                self.tab = "scripture".to_string();
                self.blanked = false;
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
            freeshow_remote::Command::ChangeLayout {
                show_id,
                layout_id,
            } => {
                self.edit_show(&show_id, |show| show.set_active_layout(&layout_id));
                if self
                    .shows
                    .iter()
                    .any(|show| show.id == show_id && show.active_layout() == layout_id)
                {
                    self.slide_index = Some(0);
                    self.blanked = false;
                    self.slide_hidden = false;
                }
            }
            freeshow_remote::Command::RemoveProjectItem { project_id, index } => {
                if self.projects.remove_show_at(&project_id, index) {
                    if self.project_id.as_deref() == Some(project_id.as_str()) && index < self.project_item
                    {
                        self.project_item -= 1;
                    }
                    let _ = self.save_projects();
                }
            }
        }
    }

    pub fn step_slide(&mut self, delta: isize) {
        self.blanked = false;
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
        let Some(next) = self.current_show().and_then(|show| {
            show.step_output(current, delta, &output_id, &output_name)
        }) else {
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
        if !self.projects.reorder(&project_id, self.project_item, delta) {
            return false;
        }
        if let Some(next) = self.project_item.checked_add_signed(delta) {
            self.project_item = next;
        }
        self.save_projects()
    }

    pub fn create_show(&mut self) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        let name = unique_name("Untitled", self.shows.iter().map(ShowFile::name).collect());
        let id = new_id();
        let mut show = ShowFile::blank(&id, &name);
        let path = Library::open(&root)
            .shows_dir()
            .join(format!("{name}.show"));
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
        self.blanked = false;
        self.select_drawer(pos);
        if let Some(project_id) = self.project_id.clone()
            && self.projects.append_show(&project_id, &id)
        {
            let _ = self.save_projects();
            if let Some(len) = self.current_project().map(|project| project.shows.len()) {
                self.project_item = len.saturating_sub(1);
            }
        }
        true
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
        let _ = show.replace_lyrics(text);
        let path = Library::open(&root)
            .shows_dir()
            .join(format!("{name}.show"));
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
        self.blanked = false;
        self.select_drawer(pos);
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
        self.tab = "shows".to_string();
        self.blanked = false;
        self.slide_hidden = false;
        self.show_index = Some(index);
        let saved = {
            let Some(show) = self.shows.get_mut(index) else {
                return false;
            };
            if !show.replace_lyrics(text) {
                return false;
            }
            match show.path.clone() {
                Some(path) => show.save(&path).is_ok(),
                None => true,
            }
        };
        if saved {
            self.slide_index = Some(0);
        }
        saved
    }

    pub fn add_slide(&mut self) -> bool {
        if self.tab != "shows" {
            return false;
        }
        let Some(show_index) = self.show_index else {
            return false;
        };
        let (saved, index) = {
            let Some(show) = self.shows.get_mut(show_index) else {
                return false;
            };
            if show.add_slide().is_none() {
                return false;
            }
            let index = show.ordered_slides().len().saturating_sub(1);
            let saved = match show.path.clone() {
                Some(path) => show.save(&path).is_ok(),
                None => true,
            };
            (saved, index)
        };
        if saved {
            self.slide_index = Some(index);
            self.blanked = false;
            self.slide_hidden = false;
        }
        saved
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
        self.save_projects()
    }

    /// Writes shows and the project file when their bytes differ. Unchanged files are left alone.
    pub fn autosave(&self) -> usize {
        let mut wrote = 0;
        for show in &self.shows {
            let Some(path) = &show.path else {
                continue;
            };
            let Ok(json) = show.to_json() else {
                continue;
            };
            if std::fs::read(path).ok().as_deref() == Some(json.as_bytes()) {
                continue;
            }
            if show.save(path).is_ok() {
                wrote += 1;
            }
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

    pub fn search(&mut self, query: &str) {
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

    fn current_show(&self) -> Option<&ShowFile> {
        self.show_index.and_then(|index| self.shows.get(index))
    }

    /// The slide being shown wins. A new background image uses the media transition instead.
    pub fn output_motion(&mut self) -> (freeshow_data::TextTransition, bool) {
        let background = self.background_transition;
        self.background_transition = false;
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
        let from_slide = if self.tab == "shows" {
            self.current_show()
                .and_then(|show| self.slide_index.and_then(|index| show.transition_at(index)))
        } else {
            None
        };
        let motion = from_slide
            .or_else(|| self.style_motion("text"))
            .unwrap_or_else(|| self.library_motion(false));
        (motion, false)
    }

    fn style_motion(&self, kind: &str) -> Option<freeshow_data::TextTransition> {
        self.root
            .as_deref()
            .and_then(|root| Library::open(root).style_transition(kind))
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
        if self.blanked {
            return None;
        }
        if let Some(seconds) = self.folder_timer_seconds() {
            return Some(seconds);
        }
        if self.tab != "shows" {
            return None;
        }
        self.timed_seconds.filter(|seconds| *seconds > 0.0)
    }

    /// The next slide in this song, the next picture in a folder, or the next project item.
    pub fn advance_show(&mut self) -> bool {
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
        }) {
            if (self.reveal_count as usize) < total {
                self.reveal_count += 1;
                return true;
            }
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
                self.blanked = false;
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
        let next = self.current_show().and_then(|show| {
            show.step_output(index, 1, &output_id, &output_name)
        });
        if let Some(next) = next {
            self.step_slide(1);
            return self.slide_index == Some(next);
        }
        self.open_next_project_show()
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
                &items.iter().map(|(_, kind)| kind.as_str()).collect::<Vec<_>>(),
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
            self.playing_audio = Some(audio.clone());
            self.audio_paused = false;
            self.pending_play = Some(PendingPlay::Audio(audio));
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
        let index = self.slide_index.unwrap_or(0);
        let (output_id, output_name) = self.audience();
        let previous = self.current_show().and_then(|show| {
            show.step_output(index, -1, &output_id, &output_name)
        });
        if let Some(previous) = previous {
            self.step_slide(-1);
            if self.slide_index == Some(previous) {
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
    let kind = event.get("type").and_then(|value| value.as_str()).unwrap_or("");
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
        .filter(|(_, value)| value.get("type").and_then(|kind| kind.as_str()).unwrap_or("counter") == "counter")
        .map(|(id, value)| NamedTimer {
            id: id.clone(),
            name: value
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or(id)
                .to_string(),
            start: value.get("start").and_then(|start| start.as_u64()).unwrap_or(0),
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

fn slide_label(slide: &freeshow_data::SlideView) -> String {
    let text = slide
        .lines
        .iter()
        .find(|line| !line.is_empty())
        .map(String::as_str)
        .unwrap_or("");
    match &slide.group {
        Some(group) if !group.is_empty() => format!("{group}: {text}"),
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
    fn the_last_slide_opens_the_next_project_item() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-advance-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-stay-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-bg-{}",
            std::process::id()
        ));
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
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248,
                207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
                96, 130,
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-video-{}",
            std::process::id()
        ));
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
    fn a_slide_can_stop_the_playing_audio() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-audio-{}",
            std::process::id()
        ));
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
    fn a_slide_starts_its_countdown() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-timer-{}",
            std::process::id()
        ));
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
        assert_eq!(session.running.as_ref().map(|timer| timer.id.as_str()), Some("default"));
        assert!(session.running.as_ref().is_some_and(|timer| timer.since.is_some()));
        session.select_slide(1);
        assert!(session.running.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_pauses_its_countdown() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-pause-timer-{}",
            std::process::id()
        ));
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
        assert!(session.running.as_ref().is_some_and(|timer| timer.since.is_some()));
        session.select_slide(1);
        assert!(session.running.as_ref().is_some_and(|timer| timer.since.is_none()));
        assert_eq!(session.running.as_ref().map(|timer| timer.mark), Some(300));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_moves_on_when_its_audio_ends() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-after-media-{}",
            std::process::id()
        ));
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
        assert!(!session.finish_media("/chapel/other.mp3", true));
        assert_eq!(session.slide_index, Some(0));
        assert!(session.finish_media("/chapel/bed.mp3", true));
        assert_eq!(session.slide_index, Some(1));
        assert!(!session.finish_media("/chapel/bed.mp3", true));
        assert_eq!(session.slide_index, Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_opens_another_song() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-start-show-{}",
            std::process::id()
        ));
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
        session.shows[first_index].show["layouts"]["main"]["slides"][0]["actions"] =
            serde_json::json!({
                "slideActions": [{ "actionValues": { "start_show": { "id": second } } }]
            });
        assert!(session.present_show_id(&first));
        assert_eq!(session.current_show().unwrap().id, second);
        assert_eq!(session.slide_index, Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_clear_slide_hides_the_lyrics_and_keeps_the_picture() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-clear-slide-{}",
            std::process::id()
        ));
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
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248,
                207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
                96, 130,
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-image-item-{}",
            std::process::id()
        ));
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
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248,
                207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
                96, 130,
            ],
        )
        .unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(session.projects.append_item(
            &project_id,
            &picture.to_string_lossy(),
            "image"
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-skip-item-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-video-item-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let clip = root.join("clip.mp4");
        std::fs::write(&clip, b"not a real movie").unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(session.projects.append_item(
            &project_id,
            &clip.to_string_lossy(),
            "video"
        ));
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
        assert!(session.projects.set_item_playback(&project_id, 1, false, false));
        assert!(session.play_named(&clip.to_string_lossy()));
        assert!(!session.media_loop);
        assert!(!session.media_muted);
        assert!(!session.slide_hidden);
        assert_eq!(session.current_show().unwrap().id, show_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_project_item_shows_its_pictures_in_order() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-folder-item-{}",
            std::process::id()
        ));
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
        assert!(session.projects.append_item(
            &project_id,
            &folder.to_string_lossy(),
            "folder"
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-folder-video-{}",
            std::process::id()
        ));
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
        assert!(session.projects.append_item(
            &project_id,
            &folder.to_string_lossy(),
            "folder"
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-audio-item-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show());
        let show_id = session.current_show().unwrap().id.clone();
        let song = root.join("song.mp3");
        std::fs::write(&song, b"not a real song").unwrap();
        let project_id = session.project_id.clone().unwrap();
        assert!(session.projects.append_item(
            &project_id,
            &song.to_string_lossy(),
            "audio"
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-remote-row-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-overlay-item-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-retreat-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-effect-{}",
            std::process::id()
        ));
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
        assert!(matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("hit.mp3")));
        assert!(session.finish_media("hit.mp3", true) == false);
        let plays = session.take_plays();
        assert!(matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("one.mp3")));
        assert!(session.finish_media("one.mp3", true) == false);
        let plays = session.take_plays();
        assert!(matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("two.mp3")));
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
        assert!(values.iter().any(|(name, value)| name == "mark" && value == "now"));
        assert!(!session.run_due_actions());
        assert_eq!(
            session.current_show().unwrap().actions_at(0).len(),
            3
        );
        session.select_slide(1);
        assert!(!session.finish_waits());
        let values = Library::open(&root).variables();
        assert!(values.iter().any(|(name, value)| name == "mark" && value == "now"));
        session.select_slide(0);
        assert!(session.finish_waits());
        let values = Library::open(&root).variables();
        assert!(values.iter().any(|(name, value)| name == "mark" && value == "after"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_opens_a_random_other_slide() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-random-slide-{}",
            std::process::id()
        ));
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
        assert!(values.iter().any(|(name, value)| name == "mark" && value == "landed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_switches_the_stage_layout() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-stage-layout-{}",
            std::process::id()
        ));
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
        let reopened = Session::open(root.clone());
        assert_eq!(reopened.stage.name, "Notes");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_plays_its_audio_playlist() {
        let songs = vec!["one.mp3".to_string(), "two.mp3".to_string(), "three.mp3".to_string()];
        let shuffled = freeshow_data::shuffled_songs(&songs, "bed");
        assert_eq!(shuffled.len(), 3);
        assert!(shuffled.iter().all(|song| songs.contains(song)));
        assert_ne!(shuffled, songs);

        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-playlist-{}",
            std::process::id()
        ));
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
        assert!(matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("one.mp3")));
        assert!((session.playback_gain() - 0.5).abs() < 0.001);
        assert!(!session.finish_media("other.mp3", true));
        assert!(session.take_plays()[0].is_none());
        assert!(session.finish_media("one.mp3", true) == false);
        let plays = session.take_plays();
        assert!(matches!(plays[0], Some(PendingPlay::Audio(ref path)) if path.ends_with("two.mp3")));
        session.select_slide(0);
        assert!(session.take_plays()[0].is_none());
        session.select_slide(1);
        let plays = session.take_plays();
        let Some(PendingPlay::Audio(ref path)) = plays[0] else {
            panic!("walk playlist did not start");
        };
        let started = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(["a.mp3", "b.mp3", "c.mp3"].contains(&started.as_str()));
        assert!(session.finish_media(&path.to_string_lossy(), true) == false);
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
        assert!(values.iter().any(|(name, value)| name == "count" && value == "3"));
        session.select_slide(1);
        let values = Library::open(&root).variables();
        assert!(values.iter().any(|(name, value)| name == "speaker" && value == "Bo"));
        session.select_slide(2);
        let values = Library::open(&root).variables();
        assert!(values.iter().any(|(name, value)| name == "speaker" && value.is_empty()));
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
            freeshow_data::fill_variables("Hello {$speaker|Guest}", &[("speaker".into(), String::new())]),
            "Hello Guest"
        );
        assert_eq!(
            freeshow_data::fill_variables("{variable_count}", &[("count".into(), "3".into())]),
            "3"
        );

        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-variable-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-stage-reveal-{}",
            std::process::id()
        ));
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
                show.stage_lines(0, true, 0, false, None),
                Vec::<String>::new()
            );
            assert_eq!(
                show.stage_lines(0, true, 1, false, None),
                vec!["One".to_string()]
            );
            assert_eq!(
                show.stage_lines(1, false, 0, false, None),
                vec!["Visible".to_string()]
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
        assert!(matches!(session.pending_play, Some(PendingPlay::ToggleMedia)));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-camera-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-volume-{}",
            std::process::id()
        ));
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
        assert!((session.playback_gain() - 0.25).abs() < 0.001);
        session.select_slide(1);
        assert!(session.audio_muted);
        assert_eq!(session.playback_gain(), 0.0);
        assert!((session.volume - 0.25).abs() < 0.001);
        session.select_slide(2);
        assert!((session.volume - 0.4).abs() < 0.001);
        assert!(session.audio_muted);
        session.select_slide(3);
        assert!(!session.audio_muted);
        assert!((session.volume - 0.501).abs() < 0.001);
        assert!((session.playback_gain() - 0.501).abs() < 0.001);
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-next-timer-{}",
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-group-jump-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-style-lines-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-click-reveal-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-line-reveal-{}",
            std::process::id()
        ));
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
        assert_eq!(
            session.shows[show_index].line_reveal_lines(0),
            Some(2)
        );
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-bound-slide-{}",
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
            show.show["layouts"]["main"]["slides"][1]["bindings"] =
                serde_json::json!(["stage"]);
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-end-slide-{}",
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-audio-stream-{}",
            std::process::id()
        ));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-slide-style-{}",
            std::process::id()
        ));
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
        session.shows[show_index].show["layouts"]["main"]["slides"][0]["actions"] =
            serde_json::json!({
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-style-motion-{}",
            std::process::id()
        ));
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
    fn a_background_uses_the_stored_fit_and_flip() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-media-fit-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        std::fs::create_dir_all(root.join("Config")).unwrap();
        let picture = root.join("bg.png");
        std::fs::write(
            &picture,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248,
                207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
                96, 130,
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-audio-gain-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::open(root.clone());
        let song = root.join("song.mp3");
        std::fs::write(&song, b"not a real song").unwrap();
        assert!(session.play_named(&song.to_string_lossy()));
        assert!((session.playback_gain() - 1.0).abs() < 0.001);
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4 }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        session.volume = 0.5;
        assert!((session.playback_gain() - 0.2).abs() < 0.001);
        assert!(!session.audio_should_loop());
        let store = serde_json::json!({
            song.to_string_lossy().as_ref(): { "volume": 0.4, "loop": true }
        });
        std::fs::write(root.join("media.json"), store.to_string()).unwrap();
        assert!(session.audio_should_loop());
        assert_eq!(session.audio_span(), (0.0, 0.0));
        assert!((session.playback_gain() - 0.2).abs() < 0.001);
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
        assert!((session.playback_gain() - 0.2).abs() < 0.001);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autosave_writes_a_changed_show_once() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-autosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        let path = session.shows[0].path.clone().unwrap();
        session.shows[0].show["name"] = serde_json::json!("Changed");
        let backups = root.join("Backups");
        let before = std::fs::read_dir(&backups).map(|dir| dir.count()).unwrap_or(0);
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
        assert_eq!(session.to_snapshot().scripture_lines, session.output_lines());
    }

    #[test]
    fn an_imported_bible_is_on_the_remote_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-scripture-{}",
            std::process::id()
        ));
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
    fn a_remote_lyric_edit_is_saved_in_the_project() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-remote-show-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = Session::open(root.clone());
        assert!(session.create_project());
        assert!(session.create_show_with("song1", "Call to worship", "Come\n\nSing"));
        assert_eq!(session.shows[0].ordered_slides().len(), 2);
        assert_eq!(session.project_labels().last().unwrap(), "Call to worship");
        assert!(session.set_show_text("song1", "Verse one\n\nChorus"));
        let again = Session::open(root.clone());
        assert_eq!(again.shows[0].ordered_slides()[1].lines, vec!["Chorus".to_string()]);
        assert_eq!(again.to_snapshot().projects[0].shows[0].kind, "show");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_play_finds_the_library_file() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-remote-media-{}",
            std::process::id()
        ));
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
    fn pausing_audio_keeps_the_file_so_play_can_resume() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-remote-audio-{}",
            std::process::id()
        ));
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
        assert!(matches!(session.pending_play, Some(PendingPlay::PauseAudio)));
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
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-template-{}",
            std::process::id()
        ));
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
        session.apply_remote(freeshow_remote::Command::SetTemplate {
            id: "big".into(),
        });
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
        session.shows.push(ShowFile::blank("grace", "Amazing Grace"));
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
