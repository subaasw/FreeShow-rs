// SPDX-License-Identifier: GPL-3.0-only

//! On-disk FreeShow library: `.show` files and `Config/projects.json`.
//!
//! Show files are a JSON array `[id, show]`, which is what FreeShow writes.
//! Unknown fields stay in the JSON value so a save does not drop data this
//! crate does not understand yet.

mod audio_routing;
mod export;
mod import;
mod portable;
pub use audio_routing::{AudioRoute, audio_routes, network_audio_routes};
pub use export::{
    write_portable_project, write_portable_template, write_project_package, write_show_pdf,
};
mod item_edit;
mod item_text;
mod media_library;
mod scripture;
mod show_draft;
mod show_edit;
pub use show_draft::ShowDraft;
mod stage_layout;

pub use item_edit::{
    ItemBox, ItemFilters, ItemTypography, TextShadow, css_px, css_set, css_value, item_hidden,
    item_locked,
};
pub use media_library::{
    MediaEntry, MediaEntryKind, image_dimensions, media_duration, media_entry_kind,
};
pub use stage_layout::{
    ChartItem, ChartSample, QrItem, StageClockOptions, StageItemKind, StageItemSpec, StageLayout,
    StageTrackerOptions, default_stage, load_stage_file, serialize_stage, write_stage_layout,
};

/// FreeShow `settings.json` text transition. `kind` is the stored type
/// (`fade`, `none`, `slide`, and the rest). `direction` is the slide axis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextTransition {
    pub kind: String,
    pub direction: String,
    pub duration_ms: u64,
    pub easing: String,
}

impl Default for TextTransition {
    fn default() -> Self {
        Self {
            kind: "fade".into(),
            direction: "left_right".into(),
            duration_ms: 500,
            easing: "sine".into(),
        }
    }
}

/// The parts of FreeShow's library `settings.json` this app understands.
///
/// Missing or malformed values stay `None`; callers keep their own defaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FreeShowPreferences {
    /// Theme key, for example `default`, `dark`, or `blue`.
    pub theme: Option<String>,
    /// Portable `Config/themes.json`, keyed by the theme ID in settings.
    pub themes: Value,
    /// `serverData.remote.password`. An empty value means no password.
    pub remote_password: Option<String>,
    /// `ports.remote`, the phone remote port.
    pub remote_port: Option<u16>,
    /// `ports.stage`, the stage page port.
    pub stage_port: Option<u16>,
    /// `language`, for example `en` or `nb`.
    pub language: Option<String>,
    /// `timeFormat`: `24` or `12`.
    pub time_format: Option<String>,
    /// `autosave`: `never`, or a value such as `15min`.
    pub autosave: Option<String>,
    pub labels_disabled: Option<bool>,
    pub group_numbers: Option<bool>,
    pub format_new_show: Option<bool>,
    pub auto_output: Option<bool>,
    pub max_connections: Option<u32>,
    /// Audience outputs stored in `settings.json`, in file order.
    pub outputs: Vec<OutputPreference>,
    /// `outLocked`: when true, the operator cannot change what is on the output.
    pub out_locked: Option<bool>,
    /// `splitLines`: max lyric lines per imported slide, or 0 to leave lines as they are.
    pub split_lines: Option<u32>,
    pub full_colors: Option<bool>,
    pub alert_updates: Option<bool>,
    pub custom_fonts: Value,
    /// `lockedOverlays`: map of output id to overlay ids that stay on that output.
    pub locked_overlays: Value,
    pub active_scenes: Value,
    pub resized: Value,
    pub slides_options: Value,
    pub drawer: Value,
    pub metronome: Value,
    pub audio_effects: Value,
    pub effects_library: Value,
    pub timeline: Value,
    pub timecode: Value,
    pub cloud_sync_data: Value,
    pub drive_data: Value,
    pub content_provider_data: Value,
    pub obs_data: Value,
    pub ai: Value,
    /// Custom keyboard shortcuts: map of action name to chord, e.g. `"undo": "Ctrl+Z"`.
    pub shortcuts: Value,
    /// `special` bag, including `guideSeen`.
    pub special: Value,
    /// From `settings_synced.json`.
    pub draw_settings: Value,
    pub profiles: Value,
    pub scenes: Value,
    pub interactions: Value,
}

/// One output from FreeShow's library `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPreference {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub active: bool,
    pub style: String,
    /// Stored `bounds.width` / `bounds.height` of this output, when present.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// FreeShow defaults this to on for audience windows.
    pub always_on_top: bool,
    pub kiosk: bool,
    /// A stage display (`stageOutput` names a layout) rather than an audience window.
    pub stage: bool,
}

/// A named output scene from `settings_synced.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSpec {
    pub id: String,
    pub name: String,
    pub overlays: Vec<String>,
    pub hidden_overlays: Vec<String>,
    pub media: Option<String>,
    pub action: Option<String>,
}

/// A stored interaction: either a quiz prompt or a simple action trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionSpec {
    pub id: String,
    pub name: String,
    pub questions: Vec<String>,
    pub action: Option<String>,
}

/// One point or lift in a draw-on-output stroke, from `drawSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawStroke {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: String,
    /// When true, this point ends the current stroke.
    pub up: bool,
}

/// A stored Player-tab video (local path or a YouTube/Vimeo id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerVideo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub path: Option<String>,
}

/// A named live effect from `effectsLibrary` or synced `effects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSpec {
    pub id: String,
    pub name: String,
    pub color: String,
    pub background: String,
    pub place_under: bool,
    pub item_types: Vec<String>,
    pub display_duration_ms: Option<u64>,
}

/// One cue on a show timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineCue {
    pub time_ms: u64,
    pub name: String,
    pub kind: String,
    pub data: Option<String>,
    pub slide_index: Option<usize>,
}

impl FreeShowPreferences {
    /// FreeShow's default clock is 24-hour.
    pub fn uses_12_hour_clock(&self) -> bool {
        self.time_format.as_deref() == Some("12")
    }

    /// `None` means autosave is turned off. Missing settings use 15 minutes.
    pub fn autosave_interval(&self) -> Option<std::time::Duration> {
        let key = self.autosave.as_deref().unwrap_or("15min");
        if key.eq_ignore_ascii_case("never") {
            return None;
        }
        let minutes = key
            .strip_suffix("min")
            .or_else(|| key.strip_suffix("m"))
            .unwrap_or(key)
            .parse::<u64>()
            .ok()
            .filter(|minutes| *minutes > 0 && *minutes <= 24 * 60)?;
        Some(std::time::Duration::from_secs(minutes.saturating_mul(60)))
    }

    /// FreeShow numbers repeated groups unless this is stored as false.
    pub fn group_numbers(&self) -> bool {
        self.group_numbers.unwrap_or(true)
    }

    pub fn labels_disabled(&self) -> bool {
        self.labels_disabled.unwrap_or(false)
    }

    pub fn format_new_show(&self) -> bool {
        self.format_new_show.unwrap_or(false)
    }

    pub fn auto_output(&self) -> bool {
        self.auto_output.unwrap_or(false)
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections.unwrap_or(10)
    }

    /// Picks an enabled audience output first, keeping the disabled active one if no other is enabled.
    pub fn active_output(&self) -> Option<&OutputPreference> {
        let mut audience = self.outputs.iter().filter(|output| !output.stage);
        audience
            .clone()
            .find(|output| output.enabled && output.active)
            .or_else(|| audience.clone().find(|output| output.enabled))
            .or_else(|| audience.find(|output| output.active))
    }

    /// The output is locked unless this is stored as true.
    pub fn out_locked(&self) -> bool {
        self.out_locked.unwrap_or(false)
    }

    /// 0 means imported text is not split into fixed-size pages.
    pub fn split_lines(&self) -> u32 {
        self.split_lines.unwrap_or(0)
    }

    pub fn full_colors(&self) -> bool {
        self.full_colors.unwrap_or(false)
    }

    pub fn guide_seen(&self) -> bool {
        self.special.get("guideSeen").and_then(Value::as_bool) == Some(true)
    }

    pub fn focus_mode_setting(&self) -> bool {
        self.special.get("focusMode").and_then(Value::as_bool) == Some(true)
    }

    pub fn output_always_on_top(&self) -> bool {
        self.active_output()
            .map(|output| output.always_on_top)
            .unwrap_or(true)
    }

    pub fn output_kiosk(&self) -> bool {
        self.active_output()
            .map(|output| output.kiosk)
            .unwrap_or(false)
    }

    pub fn shortcut(&self, action: &str) -> String {
        self.shortcuts
            .get(action)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|chord| !chord.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_shortcut(action).to_string())
    }

    pub fn shortcut_list(&self) -> Vec<(String, String)> {
        default_shortcuts()
            .into_iter()
            .map(|(action, _)| (action.to_string(), self.shortcut(action)))
            .collect()
    }

    /// FreeShow alerts the operator about updates unless this is stored as false.
    pub fn alert_updates(&self) -> bool {
        self.alert_updates.unwrap_or(true)
    }

    /// Overlay ids locked onto `output_id`, falling back to the `default` output list.
    pub fn locked_overlay_ids(&self, output_id: &str) -> Vec<String> {
        locked_overlay_ids(&self.locked_overlays, output_id)
    }

    pub fn overlay_is_locked(&self, output_id: &str, overlay_id: &str) -> bool {
        self.locked_overlay_ids(output_id)
            .iter()
            .any(|id| id == overlay_id)
    }

    /// Bottom drawer height in pixels. FreeShow stores 300 by default.
    pub fn drawer_height(&self) -> u32 {
        stored_size(&self.drawer, "height", 40, 800)
            .or_else(|| stored_size(&self.resized, "drawer", 40, 800))
            .unwrap_or(300)
    }

    /// Left show-list width. This operator window uses 240 when unset.
    pub fn left_panel_width(&self) -> u32 {
        stored_size(&self.resized, "leftPanel", 80, 800).unwrap_or(240)
    }

    /// Right project-list width. This operator window uses 260 when unset.
    pub fn right_panel_width(&self) -> u32 {
        stored_size(&self.resized, "rightPanel", 80, 800).unwrap_or(260)
    }

    /// Slide-list columns in grid mode (1–12, default 4).
    pub fn slides_columns(&self) -> u32 {
        stored_size(&self.slides_options, "columns", 1, 12).unwrap_or(4)
    }

    /// `grid`, `simple`, `groups`, `list`, or `lyrics`. Missing settings use `grid`.
    pub fn slides_mode(&self) -> &str {
        self.slides_options
            .get("mode")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|mode| matches!(*mode, "grid" | "simple" | "groups" | "list" | "lyrics"))
            .unwrap_or("grid")
    }

    /// Local custom fonts as `(family, path)` pairs. Google-only entries have no path.
    pub fn custom_font_files(&self) -> Vec<(String, std::path::PathBuf)> {
        let Some(list) = self.custom_fonts.as_array() else {
            return Vec::new();
        };
        let mut fonts = Vec::new();
        for entry in list {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty());
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(std::path::PathBuf::from);
            if let (Some(name), Some(path)) = (name, path) {
                fonts.push((name.to_string(), path));
            }
        }
        fonts
    }

    /// Metronome tempo in BPM. Accepts `tempo` or `bpm`. Missing or 0 means 120.
    pub fn metronome_bpm(&self) -> u32 {
        stored_size(&self.metronome, "tempo", 20, 400)
            .or_else(|| stored_size(&self.metronome, "bpm", 20, 400))
            .unwrap_or(120)
    }

    pub fn metronome_beats(&self) -> u32 {
        stored_size(&self.metronome, "beats", 1, 16).unwrap_or(4)
    }

    /// `send` or `receive` when a timecode mode was stored.
    pub fn timecode_kind(&self) -> Option<&str> {
        self.timecode
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|kind| *kind == "send" || *kind == "receive")
    }

    pub fn timecode_framerate(&self) -> f64 {
        self.timecode
            .get("framerate")
            .and_then(Value::as_f64)
            .filter(|rate| *rate > 0.0 && *rate <= 120.0)
            .unwrap_or(30.0)
    }

    /// Pixel size of the active or first enabled audience output.
    pub fn configured_output_size(&self) -> Option<(u32, u32)> {
        let output = self.active_output().filter(|output| output.enabled)?;
        Some((output.width?, output.height?))
    }

    /// Pixel size of the first configured stage output, which supplies the native stage layout.
    pub fn configured_stage_size(&self) -> Option<(u32, u32)> {
        let output = self.outputs.iter().find(|output| output.stage)?;
        Some((output.width?, output.height?))
    }

    /// Formats a 24-hour `HH:MM:SS` clock, honoring `timeFormat` and seconds.
    pub fn format_clock(&self, clock_24: &str, seconds: bool) -> String {
        format_clock(clock_24, self.uses_12_hour_clock(), seconds)
    }

    /// The watermark overlay is shown when `special.watermark` is true.
    pub fn watermark_enabled(&self) -> bool {
        self.special.get("watermark").and_then(Value::as_bool) == Some(true)
    }

    /// Scene by id or case-insensitive name.
    pub fn scene(&self, id_or_name: &str) -> Option<SceneSpec> {
        let query = id_or_name.trim();
        if query.is_empty() {
            return None;
        }
        let map = self.scenes.as_object()?;
        if let Some(entry) = map.get(query) {
            return Some(scene_spec(query, entry));
        }
        let wanted = query.to_ascii_lowercase();
        map.iter().find_map(|(id, entry)| {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or(id);
            (name.eq_ignore_ascii_case(&wanted) || id.eq_ignore_ascii_case(&wanted))
                .then(|| scene_spec(id, entry))
        })
    }

    pub fn scenes(&self) -> Vec<SceneSpec> {
        let Some(map) = self.scenes.as_object() else {
            return Vec::new();
        };
        map.iter()
            .map(|(id, entry)| scene_spec(id, entry))
            .collect()
    }

    /// Scene ids recorded for `output_id`, then `default`, then a bare list.
    pub fn active_scene_ids(&self, output_id: &str) -> Vec<String> {
        if let Some(map) = self.active_scenes.as_object() {
            for key in [output_id, "default"] {
                if key.is_empty() {
                    continue;
                }
                if let Some(ids) = scene_id_list(map.get(key)) {
                    return ids;
                }
            }
            return Vec::new();
        }
        scene_id_list(Some(&self.active_scenes)).unwrap_or_default()
    }

    /// Interaction by id or case-insensitive name.
    pub fn interaction(&self, id_or_name: &str) -> Option<InteractionSpec> {
        let query = id_or_name.trim();
        if query.is_empty() {
            return None;
        }
        let map = self.interactions.as_object()?;
        if let Some(entry) = map.get(query) {
            return Some(interaction_spec(query, entry));
        }
        let wanted = query.to_ascii_lowercase();
        map.iter().find_map(|(id, entry)| {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or(id);
            (name.eq_ignore_ascii_case(&wanted) || id.eq_ignore_ascii_case(&wanted))
                .then(|| interaction_spec(id, entry))
        })
    }

    pub fn interactions(&self) -> Vec<InteractionSpec> {
        let Some(map) = self.interactions.as_object() else {
            return Vec::new();
        };
        map.iter()
            .map(|(id, entry)| interaction_spec(id, entry))
            .collect()
    }

    /// Brush strokes stored under `drawSettings.paint.lines` or `drawSettings.lines`.
    pub fn draw_strokes(&self) -> Vec<DrawStroke> {
        let paint = self
            .draw_settings
            .get("paint")
            .unwrap_or(&self.draw_settings);
        let lines = paint
            .get("lines")
            .or_else(|| self.draw_settings.get("lines"))
            .or_else(|| self.draw_settings.get("paintCache"));
        parse_draw_strokes(lines)
    }

    pub fn draw_color(&self) -> String {
        let paint = self
            .draw_settings
            .get("paint")
            .unwrap_or(&self.draw_settings);
        paint
            .get("color")
            .and_then(Value::as_str)
            .or_else(|| self.draw_settings.get("color").and_then(Value::as_str))
            .map(str::trim)
            .filter(|color| !color.is_empty())
            .unwrap_or("#ffff00")
            .to_string()
    }

    pub fn draw_size(&self) -> f32 {
        let paint = self
            .draw_settings
            .get("paint")
            .unwrap_or(&self.draw_settings);
        paint
            .get("size")
            .and_then(json_f32)
            .or_else(|| self.draw_settings.get("size").and_then(json_f32))
            .filter(|size| *size > 0.0)
            .unwrap_or(12.0)
    }

    /// Live output delay in milliseconds, from `special.outputDelayMs`.
    pub fn output_delay_ms(&self) -> u32 {
        self.special
            .get("outputDelayMs")
            .and_then(Value::as_u64)
            .or_else(|| self.special.get("outputDelay").and_then(Value::as_u64))
            .and_then(|ms| u32::try_from(ms).ok())
            .unwrap_or(0)
    }

    pub fn effects_library(&self) -> Vec<EffectSpec> {
        effects_from(&self.effects_library)
    }

    pub fn effect(&self, id_or_name: &str) -> Option<EffectSpec> {
        let query = id_or_name.trim();
        if query.is_empty() {
            return None;
        }
        let wanted = query.to_ascii_lowercase();
        self.effects_library().into_iter().find(|effect| {
            effect.id.eq_ignore_ascii_case(&wanted) || effect.name.eq_ignore_ascii_case(&wanted)
        })
    }

    /// Combined linear gain from stored `audioEffects` stacks (default 1.0).
    pub fn audio_effect_gain(&self) -> f32 {
        audio_effect_gain(&self.audio_effects)
    }

    /// Enabled processor names in `audioEffects`.
    pub fn audio_effect_names(&self) -> Vec<String> {
        audio_effect_names(&self.audio_effects)
    }

    /// Timeline editor zoom from settings, when stored.
    pub fn timeline_zoom(&self) -> f32 {
        self.timeline
            .get("zoom")
            .and_then(json_f32)
            .filter(|zoom| *zoom > 0.0)
            .unwrap_or(1.0)
    }
}

/// Formats `HH:MM:SS` or `HH:MM` as 24-hour or 12-hour text.
pub fn format_clock(clock_24: &str, twelve_hour: bool, seconds: bool) -> String {
    let mut parts = clock_24.split(':');
    let Some(hour) = parts
        .next()
        .and_then(|value| value.trim().parse::<u32>().ok())
    else {
        return if seconds {
            clock_24.to_string()
        } else {
            clock_24.get(..5).unwrap_or(clock_24).to_string()
        };
    };
    let minute = parts
        .next()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let second = parts
        .next()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if twelve_hour {
        let period = if hour >= 12 { "PM" } else { "AM" };
        let hour12 = match hour % 12 {
            0 => 12,
            other => other,
        };
        if seconds {
            format!("{hour12}:{minute:02}:{second:02} {period}")
        } else {
            format!("{hour12}:{minute:02} {period}")
        }
    } else if seconds {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

pub const SHOWS_DIR: &str = "Shows";
pub const CONFIG_DIR: &str = "Config";
pub const BIBLES_DIR: &str = "Bibles";
pub const MEDIA_DIR: &str = "Media";
pub const AUDIO_DIR: &str = "Audio";
pub const IMPORTS_DIR: &str = "Imports";
pub const BACKUPS_DIR: &str = "Backups";
pub const EXPORTS_DIR: &str = "Exports";
pub const PROJECTS_FILE: &str = "projects.json";
pub const TEMPLATES_FILE: &str = "templates.json";
pub const OVERLAYS_FILE: &str = "overlays.json";
pub const EVENTS_FILE: &str = "events.json";
pub const STAGE_FILE: &str = "stage.json";
pub const SYNCED_SETTINGS_FILE: &str = "settings_synced.json";

#[derive(Debug, Error)]
pub enum DataError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("show file must be a JSON array of [id, show]")]
    ShowShape,
    #[error("{0}")]
    Scripture(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutView {
    pub id: String,
    pub name: String,
    pub slides: Vec<SlideView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideView {
    pub id: String,
    pub group: Option<String>,
    pub color: Option<String>,
    pub notes: String,
    pub lines: Vec<String>,
}

/// The picture a slide contributes to the audience background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlideBackdrop {
    /// This slide does not change the picture already on screen.
    Unchanged,
    /// A clear-background action removes the picture.
    Clear,
    /// A picture that sits behind this slide.
    Picture { path: String, foreground: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimedCueTarget {
    Next,
    First,
    Slide(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowFile {
    pub id: String,
    pub show: Value,
    pub path: Option<PathBuf>,
}

/// Lessons.church OLP/OLF documents are imported by the ChurchApps integration,
/// which downloads their media before publishing a show.
pub fn is_lesson_document(value: &Value) -> bool {
    value.get("lessonName").and_then(Value::as_str).is_some()
        && (value.get("messages").is_some_and(Value::is_array)
            || value.get("sections").is_some_and(Value::is_array))
}

fn active_output_key(outputs: &serde_json::Map<String, Value>) -> Option<String> {
    let mut audience = outputs.iter().filter(|(_, output)| {
        output.is_object() && output["stageOutput"].as_str().is_none_or(str::is_empty)
    });
    audience
        .clone()
        .find(|(_, output)| {
            output["active"].as_bool() == Some(true) && output["enabled"].as_bool() != Some(false)
        })
        .or_else(|| {
            audience
                .clone()
                .find(|(_, output)| output["enabled"].as_bool() != Some(false))
        })
        .or_else(|| audience.find(|(_, output)| output["active"].as_bool() == Some(true)))
        .map(|(key, _)| key.clone())
}

fn json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn json_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|number| number as u64))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .map(|number| number as usize)
}

fn timer_node<'a>(
    entry: &'a mut Value,
    child: Option<&str>,
) -> Option<&'a mut serde_json::Map<String, Value>> {
    if let Some(child) = child {
        if entry.pointer(&format!("/children/{child}")).is_none() {
            let object = entry.as_object_mut()?;
            let children = object
                .entry("children")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            let children = children.as_object_mut()?;
            children
                .entry(child.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
        }
        entry
            .pointer_mut(&format!("/children/{child}"))?
            .as_object_mut()
    } else {
        entry.as_object_mut()
    }
}

fn write_slide_actions(node: &mut Value, actions: &[Value]) -> bool {
    if !node.is_object() {
        *node = Value::Object(serde_json::Map::new());
    }
    let Some(object) = node.as_object_mut() else {
        return false;
    };
    let slot = object
        .entry("actions")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(serde_json::Map::new());
    }
    let Some(actions_object) = slot.as_object_mut() else {
        return false;
    };
    actions_object.insert("slideActions".to_string(), Value::Array(actions.to_vec()));
    true
}

fn action_trigger(node: &Value, name: &str) -> bool {
    node.pointer("/actions/slideActions")
        .and_then(Value::as_array)
        .is_some_and(|actions| {
            actions.iter().any(|action| {
                action
                    .get("triggers")
                    .and_then(Value::as_array)
                    .is_some_and(|triggers| {
                        triggers
                            .iter()
                            .any(|trigger| trigger.as_str() == Some(name))
                    })
            })
        })
}

fn clears_background(node: &Value) -> bool {
    action_trigger(node, "clear_background") || action_trigger(node, "clear_all")
}

/// Group label used when a slide action names a group: lower case, spaces as underscores.
fn label_id(label: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = label.to_lowercase().chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == 'x'
            && chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_digit())
        {
            index += 2;
            continue;
        }
        match chars[index] {
            '[' | ']' | '\'' | '"' | ':' => {}
            ' ' | '-' => out.push('_'),
            other => out.push(other),
        }
        index += 1;
    }
    out.trim_matches('_').to_string()
}

/// Where a slide action sends the presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlideJump {
    /// A layout index, optionally in another show.
    Index {
        show_id: Option<String>,
        index: usize,
    },
    /// A group label, such as Chorus or Verse_2.
    Name(String),
    /// A global group id.
    Group(String),
    /// The next slide that can show on this output.
    Next,
    /// The previous slide that can show on this output.
    Previous,
}

/// A slide action that replaces the text or media transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionChange {
    /// `text` or `media`.
    pub slot: String,
    pub kind: String,
    pub duration_ms: u64,
    pub easing: String,
}

/// A slide action that sets or mutes an audio channel.
#[derive(Clone, Debug, PartialEq)]
pub enum MixStep {
    /// `volume` is missing when the action toggles between silence and the previous level.
    Volume {
        channel: String,
        volume: Option<f64>,
        use_db: bool,
    },
    /// `muted` is missing when the action toggles the channel.
    Mute {
        channel: String,
        muted: Option<bool>,
    },
}

/// Actions before a wait, the wait in seconds, and the actions after it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionPause {
    pub before: Vec<Value>,
    pub seconds: f64,
    pub after: Vec<Value>,
}

/// Splits a slide's actions at the first wait. No wait returns nothing.
pub fn pause_in_actions(actions: &[Value]) -> Option<ActionPause> {
    let wait_at = actions.iter().position(action_is_wait)?;
    let seconds = wait_seconds(&actions[wait_at]);
    Some(ActionPause {
        before: actions[..wait_at].to_vec(),
        seconds,
        after: actions[wait_at + 1..].to_vec(),
    })
}

fn action_is_wait(action: &Value) -> bool {
    action
        .get("triggers")
        .and_then(Value::as_array)
        .is_some_and(|triggers| {
            triggers
                .iter()
                .any(|trigger| trigger.as_str() == Some("wait"))
        })
}

fn wait_seconds(action: &Value) -> f64 {
    let value = action.pointer("/actionValues/wait");
    value
        .and_then(|value| value.get("number"))
        .and_then(json_f64)
        .or_else(|| value.and_then(json_f64))
        .unwrap_or(0.0)
        .max(0.0)
}

/// One playlist action on a slide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistStep {
    /// Start this playlist. `id` is used when it is set, otherwise `name`.
    Start { id: String, name: String },
    /// Play the next song in the playlist that is already running.
    Next,
}

/// A saved list of audio files.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioList {
    pub name: String,
    pub songs: Vec<String>,
    pub shuffle: bool,
    /// Missing means the list starts again after the last song.
    pub loops: bool,
    pub volume: f32,
}

/// A slide action that changes the auto-advance countdown.
#[derive(Clone, Debug, PartialEq)]
pub enum TimerStep {
    /// This showing does not advance on its own.
    Clear,
    /// Write this duration onto every slide. The slide that sets it keeps its current countdown.
    Set(f64),
}

impl ShowFile {
    pub fn parse(text: &str) -> Result<Self, DataError> {
        let value: Value = serde_json::from_str(text)?;
        let Some(pair) = value.as_array() else {
            return Err(DataError::ShowShape);
        };
        if pair.len() != 2 {
            return Err(DataError::ShowShape);
        }
        let Some(id) = pair[0].as_str() else {
            return Err(DataError::ShowShape);
        };
        if !pair[1].is_object() {
            return Err(DataError::ShowShape);
        }
        Ok(Self {
            id: id.to_string(),
            show: pair[1].clone(),
            path: None,
        })
    }

    pub fn load(path: &Path) -> Result<Self, DataError> {
        let mut show = Self::parse(&fs::read_to_string(path)?)?;
        show.path = Some(path.to_path_buf());
        Ok(show)
    }

    pub fn to_json(&self) -> Result<String, DataError> {
        Ok(serde_json::to_string(&Value::Array(vec![
            Value::String(self.id.clone()),
            self.show.clone(),
        ]))?)
    }

    pub fn save(&self, path: &Path) -> Result<(), DataError> {
        atomic_write(path, self.to_json()?.as_bytes())
    }

    /// A new FreeShow show with one empty slide on the Default layout.
    pub fn blank(id: &str, name: &str) -> Self {
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let show = serde_json::json!({
            "name": name,
            "category": null,
            "settings": { "activeLayout": "main", "template": null },
            "timestamps": { "created": created, "modified": null, "used": null },
            "meta": {},
            "slides": {
                "s1": {
                    "group": "",
                    "color": null,
                    "settings": {},
                    "notes": "",
                    "items": [{
                        "style": "top:88px;left:50px;height:904px;width:1820px;",
                        "lines": [{ "align": "", "text": [{ "value": "", "style": "" }] }]
                    }]
                }
            },
            "layouts": {
                "main": { "name": "Default", "notes": "", "slides": [{ "id": "s1" }] }
            },
            "media": {}
        });
        Self {
            id: id.to_string(),
            show,
            path: None,
        }
    }

    /// A one-slide editable show with its own copy of a template's design.
    pub fn from_template(id: &str, name: &str, template: &Value) -> Option<Self> {
        let items = template.get("items")?.as_array()?;
        if items.is_empty() || items.iter().any(|item| !item.is_object()) {
            return None;
        }
        let mut show = Self::blank(id, name);
        let slide = &mut show.show["slides"]["s1"];
        slide["items"] = Value::Array(items.clone());
        if let Some(color) = template
            .pointer("/settings/backgroundColor")
            .filter(|value| value.is_string())
        {
            slide["settings"]["color"] = color.clone();
        }
        if let Some(size) = template
            .pointer("/settings/resolution")
            .filter(|value| value.is_object())
        {
            slide["settings"]["resolution"] = size.clone();
        }
        Some(show)
    }

    /// Appends an empty slide to the active layout and returns its id.
    pub fn add_slide(&mut self) -> Option<String> {
        let id = {
            let slides = self.show.get_mut("slides").and_then(Value::as_object_mut)?;
            let mut number = slides.len() + 1;
            let mut id = format!("s{number}");
            while slides.contains_key(&id) {
                number += 1;
                id = format!("s{number}");
            }
            slides.insert(
                id.clone(),
                serde_json::json!({
                    "group": "",
                    "color": null,
                    "settings": {},
                    "notes": "",
                    "items": [{
                        "style": "top:88px;left:50px;height:904px;width:1820px;",
                        "lines": [{ "align": "", "text": [{ "value": "", "style": "" }] }]
                    }]
                }),
            );
            id
        };
        let layout_id = self
            .show
            .pointer("/settings/activeLayout")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string();
        if let Some(order) = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array_mut)
        {
            order.push(serde_json::json!({ "id": id }));
        }
        Some(id)
    }

    /// Replaces the active layout with one slide per blank-line block.
    /// This is the text the phone remote edits.
    pub fn replace_lyrics(&mut self, text: &str) -> bool {
        self.replace_lyrics_with(text, false)
    }

    /// When `format` is true, group headers such as `[Verse]` or `Chorus:` become slide groups.
    pub fn replace_lyrics_with(&mut self, text: &str, format: bool) -> bool {
        self.replace_lyrics_with_split(text, format, 0)
    }

    /// Like `replace_lyrics_with`, then splits any stanza longer than `split_lines`.
    pub fn replace_lyrics_with_split(
        &mut self,
        text: &str,
        format: bool,
        split_lines: u32,
    ) -> bool {
        if self.has_locked_layers() {
            return false;
        }
        let layout_id = self
            .show
            .pointer("/settings/activeLayout")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string();
        let previous_ids: Vec<String> = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)
            .map(|order| {
                order
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let previous_groups: Vec<String> = previous_ids
            .iter()
            .map(|id| {
                self.show
                    .pointer(&format!("/slides/{id}/group"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        let stanzas: Vec<(String, String)> = if format {
            formatted_stanzas(text)
        } else {
            let chunks: Vec<&str> = text
                .split("\n\n")
                .map(str::trim)
                .filter(|chunk| !chunk.is_empty())
                .collect();
            let chunks: Vec<&str> = if chunks.is_empty() { vec![""] } else { chunks };
            chunks
                .into_iter()
                .enumerate()
                .map(|(index, chunk)| {
                    // "[Chorus]" on its own first line names the group, as in FreeShow's text editor.
                    let (first, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
                    let bracketed = first
                        .trim()
                        .strip_prefix('[')
                        .and_then(|line| line.strip_suffix(']'))
                        .map(str::trim)
                        .filter(|name| !name.is_empty());
                    match bracketed {
                        Some(name) => (name.to_string(), rest.trim().to_string()),
                        None => (
                            previous_groups.get(index).cloned().unwrap_or_default(),
                            chunk.to_string(),
                        ),
                    }
                })
                .collect()
        };
        let stanzas = split_stanzas(stanzas, split_lines);
        let mut slides = serde_json::Map::new();
        let mut order = Vec::new();
        let mut seen: Vec<(&str, &str, String)> = Vec::new();
        for (group, chunk) in &stanzas {
            // A chorus sung again is the same slide shown twice in the layout, as in FreeShow.
            // A bare "[Chorus]" with no lines under it repeats the chorus written earlier.
            if let Some((_, _, id)) = seen.iter().find(|(seen_group, seen_chunk, _)| {
                !group.is_empty()
                    && *seen_group == group
                    && (*seen_chunk == chunk || (chunk.is_empty() && !seen_chunk.is_empty()))
            }) {
                order.push(serde_json::json!({ "id": id }));
                continue;
            }
            let id = format!("s{}", slides.len() + 1);
            seen.push((group, chunk, id.clone()));
            let (global_group, color) = default_group(group)
                .map(|(global, color)| (Value::from(global), Value::from(color)))
                .unwrap_or((Value::Null, Value::Null));
            let lines: Vec<Value> = if chunk.is_empty() {
                vec![serde_json::json!({
                    "align": "",
                    "text": [{ "value": "", "style": "" }]
                })]
            } else {
                chunk
                    .split('\n')
                    .map(|line| {
                        serde_json::json!({
                            "align": "",
                            "text": [{ "value": line, "style": "" }]
                        })
                    })
                    .collect()
            };
            slides.insert(
                id.clone(),
                serde_json::json!({
                    "group": group,
                    "globalGroup": global_group,
                    "color": color,
                    "settings": {},
                    "notes": "",
                    "items": [{
                        "style": "top:88px;left:50px;height:904px;width:1820px;",
                        "lines": lines
                    }]
                }),
            );
            order.push(serde_json::json!({ "id": id }));
        }
        let Some(show) = self.show.as_object_mut() else {
            return false;
        };
        show.insert("slides".to_string(), Value::Object(slides));
        let layouts = show
            .entry("layouts")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let Some(layouts) = layouts.as_object_mut() else {
            return false;
        };
        let layout = layouts
            .entry(layout_id)
            .or_insert_with(|| serde_json::json!({ "name": "Default", "notes": "", "slides": [] }));
        let Some(layout) = layout.as_object_mut() else {
            return false;
        };
        layout.insert("slides".to_string(), Value::Array(order));
        true
    }

    pub fn name(&self) -> String {
        self.show
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&self.id)
            .to_string()
    }

    /// Layout-ordered lyrics in the same `[Group]` / blank-line shape the plain-text importer reads.
    pub fn export_plain_text(&self) -> String {
        let mut out = String::new();
        let name = self.name();
        if !name.trim().is_empty() {
            out.push_str("Title: ");
            out.push_str(&name);
            out.push_str("\n\n");
        }
        for (index, slide) in self.ordered_slides().iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            if let Some(group) = slide
                .group
                .as_deref()
                .map(str::trim)
                .filter(|group| !group.is_empty())
            {
                out.push('[');
                out.push_str(group);
                out.push_str("]\n");
            }
            for line in &slide.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Writes `export_plain_text` through the crash-safe library save path.
    pub fn write_plain_text(&self, path: &Path) -> Result<(), DataError> {
        atomic_write(path, self.export_plain_text().as_bytes())
    }

    /// The raw items of one slide, for saving it as an overlay or template.
    pub fn slide_items(&self, slide_id: &str) -> Vec<Value> {
        self.show
            .get("slides")
            .and_then(|slides| slides.get(slide_id))
            .and_then(|slide| slide.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    pub fn has_locked_layers(&self) -> bool {
        self.show["slides"].as_object().is_some_and(|slides| {
            slides.values().any(|slide| {
                slide["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(item_locked))
            })
        })
    }

    pub fn visible_lines_for_slide(&self, id: &str) -> Vec<String> {
        visible_slide_lines(&self.show["slides"][id])
    }

    /// Plain text of each slide, in the same order FreeShow stores slide objects.
    pub fn slides(&self) -> Vec<SlideView> {
        let Some(slides) = self.show.get("slides").and_then(Value::as_object) else {
            return Vec::new();
        };
        slides
            .iter()
            .map(|(id, slide)| SlideView {
                id: id.clone(),
                group: slide
                    .get("group")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                color: slide
                    .get("color")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|color| !color.is_empty())
                    .map(str::to_string),
                notes: slide
                    .get("notes")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                lines: slide_lines(slide),
            })
            .collect()
    }

    pub fn active_layout(&self) -> String {
        self.show
            .pointer("/settings/activeLayout")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string()
    }

    /// Points the show at a layout that already exists.
    pub fn set_active_layout(&mut self, layout_id: &str) -> bool {
        if layout_id.is_empty()
            || self
                .show
                .pointer(&format!("/layouts/{layout_id}"))
                .is_none()
        {
            return false;
        }
        let Some(settings) = self
            .show
            .pointer_mut("/settings")
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        settings.insert(
            "activeLayout".to_string(),
            Value::String(layout_id.to_string()),
        );
        true
    }

    /// Copies a template's text box onto every lyric item and remembers the template id.
    /// Line text stays. Locked slides and non-text items stay as they are.
    pub fn apply_template(&mut self, template_id: &str, template: &Value) -> bool {
        if template_id.is_empty() {
            return false;
        }
        let has_text_box = template_text_item(template).is_some();
        let Some(settings) = self
            .show
            .pointer_mut("/settings")
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        settings.insert(
            "template".to_string(),
            Value::String(template_id.to_string()),
        );
        if !has_text_box {
            return true;
        }
        let Some(slides) = self.show.get_mut("slides").and_then(Value::as_object_mut) else {
            return true;
        };
        for slide in slides.values_mut() {
            restyle_with_template(slide, template);
        }
        true
    }

    /// Every layout, with slides in that layout's order.
    pub fn layout_views(&self) -> Vec<LayoutView> {
        let Some(layouts) = self.show.get("layouts").and_then(Value::as_object) else {
            return Vec::new();
        };
        layouts
            .iter()
            .map(|(id, layout)| LayoutView {
                id: id.clone(),
                name: layout
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                slides: self.slides_in_layout(id),
            })
            .collect()
    }

    /// Slides in the active layout's order. Child slides follow their parent.
    /// An empty layout falls back to file order.
    pub fn ordered_slides(&self) -> Vec<SlideView> {
        self.slides_in_layout(&self.active_layout())
    }

    /// The transition stored on this layout position. A child uses its own
    /// entry under the parent layout slide. An empty value means the library
    /// transition still applies.
    pub fn transition_at(&self, index: usize) -> Option<TextTransition> {
        read_transition(self.layout_node_at(index)?.get("transition"))
    }

    /// Stores this layout position's own slide transition. An empty kind removes it so the
    /// style transition applies again. FreeShow keeps a child's under its parent's entry.
    pub fn set_transition_at(&mut self, index: usize, kind: &str, duration_ms: u64) -> bool {
        self.set_transition_at_dir(index, kind, duration_ms, "")
    }

    /// Like [`Self::set_transition_at`], and stores `custom.direction` when `direction` is set.
    pub fn set_transition_at_dir(
        &mut self,
        index: usize,
        kind: &str,
        duration_ms: u64,
        direction: &str,
    ) -> bool {
        self.write_slide_transition(index, kind, duration_ms, direction, None)
    }

    /// Writes a complete override. None restores the output style or library transition.
    pub fn set_slide_transition(&mut self, index: usize, motion: Option<&TextTransition>) -> bool {
        match motion {
            Some(motion) => self.write_slide_transition(
                index,
                &motion.kind,
                motion.duration_ms,
                &motion.direction,
                Some(&motion.easing),
            ),
            None => self.write_slide_transition(index, "", 0, "", None),
        }
    }

    fn write_slide_transition(
        &mut self,
        index: usize,
        kind: &str,
        duration_ms: u64,
        direction: &str,
        easing: Option<&str>,
    ) -> bool {
        let Some((position, child)) = self.layout_position(index) else {
            return false;
        };
        let layout_id = self.active_layout();
        let Some(entry) = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides/{position}"))
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        let kind = kind.trim();
        if kind.is_empty() {
            let node = match &child {
                None => Some(entry),
                Some(id) => entry
                    .get_mut("children")
                    .and_then(|children| children.get_mut(id))
                    .and_then(Value::as_object_mut),
            };
            if let Some(node) = node {
                node.remove("transition");
            }
            return true;
        }
        let node = match child {
            None => entry,
            Some(id) => {
                let children = entry
                    .entry("children")
                    .or_insert_with(|| serde_json::json!({}));
                if !children.is_object() {
                    *children = serde_json::json!({});
                }
                let slot = children
                    .as_object_mut()
                    .unwrap()
                    .entry(id)
                    .or_insert_with(|| serde_json::json!({}));
                if !slot.is_object() {
                    *slot = serde_json::json!({});
                }
                slot.as_object_mut().unwrap()
            }
        };
        let transition = node
            .entry("transition")
            .or_insert_with(|| serde_json::json!({}));
        if !transition.is_object()
            || (easing.is_none() && transition["type"].as_str() != Some(kind))
        {
            *transition = serde_json::json!({});
        }
        transition["type"] = kind.into();
        transition["duration"] = duration_ms.min(10_000).into();
        if let Some(easing) = easing {
            transition["easing"] = easing.into();
        }
        let direction = direction.trim();
        if !direction.is_empty() {
            if !transition["custom"].is_object() {
                transition["custom"] = serde_json::json!({});
            }
            transition["custom"]["direction"] = direction.into();
        }
        true
    }

    /// The layout array position holding `index`, and the child slide id when it is a child.
    fn layout_position(&self, index: usize) -> Option<(usize, Option<String>)> {
        let layout_id = self.active_layout();
        let order = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)?;
        let mut cursor = 0usize;
        for (position, entry) in order.iter().enumerate() {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if self.show.pointer(&format!("/slides/{id}")).is_none() {
                continue;
            }
            if cursor == index {
                return Some((position, None));
            }
            cursor += 1;
            let children = self
                .show
                .pointer(&format!("/slides/{id}/children"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for child in children {
                let Some(child_id) = child.as_str() else {
                    continue;
                };
                if self.show.pointer(&format!("/slides/{child_id}")).is_none() {
                    continue;
                }
                if cursor == index {
                    return Some((position, Some(child_id.to_string())));
                }
                cursor += 1;
            }
        }
        None
    }

    /// The background transition stored on this layout position.
    pub fn media_transition_at(&self, index: usize) -> Option<TextTransition> {
        read_transition(self.layout_node_at(index)?.get("mediaTransition"))
    }

    /// Seconds until FreeShow advances, from the layout slide's `nextTimer`.
    pub fn next_timer_at(&self, index: usize) -> Option<f64> {
        let node = self.layout_node_at(index)?;
        let seconds = node.get("nextTimer").and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_u64().map(|number| number as f64))
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })?;
        (seconds > 0.0).then_some(seconds)
    }

    /// Explicit timed completion. Ordinary FreeShow next-slide timers omit a target.
    pub fn timed_cue_target_at(&self, index: usize) -> Option<TimedCueTarget> {
        let node = self.layout_node_at(index)?;
        match node["nextTimerTarget"].as_str() {
            Some("next") => Some(TimedCueTarget::Next),
            Some("first") => Some(TimedCueTarget::First),
            _ => node["nextTimerTarget"]["slide"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| TimedCueTarget::Slide(id.to_owned())),
        }
    }

    /// The picture behind this slide: its own background, a picture carried from
    /// an earlier slide, or a clear-background action.
    pub fn backdrop_at(&self, index: usize) -> SlideBackdrop {
        let steps = self.layout_steps();
        let Some((slide_id, node)) = steps.get(index) else {
            return SlideBackdrop::Unchanged;
        };
        if clears_background(node) {
            return SlideBackdrop::Clear;
        }
        if let Some(id) = node
            .get("background")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            if let Some((path, foreground, _)) = self.media_picture(id) {
                return SlideBackdrop::Picture { path, foreground };
            }
            return SlideBackdrop::Unchanged;
        }
        if let Some(path) = self
            .show
            .pointer(&format!("/slides/{slide_id}/settings/backgroundImage"))
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
        {
            return SlideBackdrop::Picture {
                path: path.to_string(),
                foreground: false,
            };
        }
        let mut picture = None;
        let mut cleared = false;
        for (_, earlier) in steps.iter().take(index) {
            if earlier.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if clears_background(earlier) {
                picture = None;
                cleared = true;
                continue;
            }
            let Some(media_id) = earlier
                .get("background")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let Some((path, foreground, once)) = self.media_picture(media_id) else {
                continue;
            };
            if foreground || once {
                picture = None;
                cleared = false;
                continue;
            }
            picture = Some((path, foreground));
            cleared = false;
        }
        if let Some((path, foreground)) = picture {
            SlideBackdrop::Picture { path, foreground }
        } else if cleared {
            SlideBackdrop::Clear
        } else {
            SlideBackdrop::Unchanged
        }
    }

    /// Audio files this slide starts, from layout `audio` ids in `media`.
    pub fn audio_at(&self, index: usize) -> Vec<String> {
        self.audio_media_at(index)
            .into_iter()
            .filter_map(|media| {
                media["path"]
                    .as_str()
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
            })
            .collect()
    }

    pub fn audio_media_at(&self, index: usize) -> Vec<&Value> {
        let Some(node) = self.layout_node_at(index) else {
            return Vec::new();
        };
        node.get("audio")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| {
                        let id = id.as_str()?;
                        self.show["media"].get(id)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn delayed_actions_at(&self, index: usize) -> Vec<TimelineCue> {
        self.layout_node_at(index)
            .map(|node| timeline_cues_from_actions(&node["delayedActions"], true))
            .unwrap_or_default()
    }

    /// Sound-effect files this slide plays, in order.
    pub fn effect_audio_at(&self, index: usize) -> Vec<String> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        actions
            .iter()
            .filter(|action| {
                action
                    .get("triggers")
                    .and_then(Value::as_array)
                    .is_some_and(|triggers| {
                        triggers
                            .iter()
                            .any(|trigger| trigger.as_str() == Some("start_audio_effect"))
                    })
            })
            .filter_map(|action| {
                let value = action.pointer("/actionValues/start_audio_effect");
                value
                    .and_then(|value| value.get("path"))
                    .and_then(Value::as_str)
                    .or_else(|| value.and_then(Value::as_str))
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
            })
            .collect()
    }

    /// Overlay ids this slide turns on.
    pub fn overlays_at(&self, index: usize) -> Vec<String> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        node.get("overlays")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The slide actions stored on this step.
    pub fn actions_at(&self, index: usize) -> Vec<Value> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        node.pointer("/actions/slideActions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Replaces the slide actions on this step. The rest of the step stays.
    pub fn set_actions_at(&mut self, index: usize, actions: &[Value]) -> bool {
        let Some(target) = self.layout_steps().get(index).map(|(id, _)| id.clone()) else {
            return false;
        };
        let layout_id = self.active_layout();
        let Some(order) = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        for entry in order {
            if entry.get("id").and_then(Value::as_str) == Some(target.as_str()) {
                return write_slide_actions(entry, actions);
            }
            if let Some(child) = entry.pointer_mut(&format!("/children/{target}")) {
                return write_slide_actions(child, actions);
            }
        }
        false
    }

    /// This slide asks to open some other slide in the show.
    pub fn random_slide_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        action_trigger(node, "random_slide")
    }

    /// A slide action that opens another slide: by index, by group name, or by global group.
    pub fn slide_jump_at(&self, index: usize) -> Option<SlideJump> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        let actions = node.pointer("/actions/slideActions")?.as_array()?;
        for action in actions {
            let triggers = action
                .get("triggers")
                .and_then(Value::as_array)
                .map(|triggers| {
                    triggers
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if triggers.contains(&"index_select_slide") {
                let value = action.pointer("/actionValues/index_select_slide");
                let index = value.and_then(|value| {
                    value
                        .get("index")
                        .and_then(json_usize)
                        .or_else(|| json_usize(value))
                });
                if let Some(index) = index {
                    let show_id = value
                        .and_then(|value| value.get("showId"))
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty() && *id != "active")
                        .map(str::to_string);
                    return Some(SlideJump::Index { show_id, index });
                }
            }
            if triggers.contains(&"name_select_slide") {
                let value = action.pointer("/actionValues/name_select_slide");
                let name = value.and_then(|value| {
                    value
                        .get("value")
                        .and_then(Value::as_str)
                        .or_else(|| value.as_str())
                });
                if let Some(name) = name.filter(|name| !name.is_empty()) {
                    return Some(SlideJump::Name(name.to_string()));
                }
            }
            if triggers.contains(&"id_select_group") {
                let value = action.pointer("/actionValues/id_select_group");
                let id = value.and_then(|value| {
                    value
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| value.as_str())
                });
                if let Some(id) = id.filter(|id| !id.is_empty()) {
                    return Some(SlideJump::Group(id.to_string()));
                }
            }
            if triggers.contains(&"next_slide") {
                return Some(SlideJump::Next);
            }
            if triggers.contains(&"previous_slide") {
                return Some(SlideJump::Previous);
            }
        }
        None
    }

    /// Overlay ids a clear-overlay action removes. Other overlays stay.
    pub fn cleared_overlays_at(&self, index: usize) -> Vec<String> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("clear_overlay"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/clear_overlay");
            let id = value.and_then(|value| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            });
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                ids.push(id.to_string());
            }
        }
        ids
    }

    /// The last transition this slide stores. Missing pieces use fade, 500ms, and sine.
    pub fn transition_change_at(&self, index: usize) -> Option<TransitionChange> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        let actions = node.pointer("/actions/slideActions")?.as_array()?;
        let mut change = None;
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("change_transition"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/change_transition");
            let slot = value
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .filter(|slot| !slot.is_empty())
                .unwrap_or("text");
            let kind = value
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
                .unwrap_or("fade");
            let duration_ms = value
                .and_then(|value| value.get("duration"))
                .and_then(json_f64)
                .unwrap_or(500.0)
                .max(0.0) as u64;
            let easing = value
                .and_then(|value| value.get("easing"))
                .and_then(Value::as_str)
                .filter(|easing| !easing.is_empty())
                .unwrap_or("sine");
            change = Some(TransitionChange {
                slot: slot.to_string(),
                kind: kind.to_string(),
                duration_ms,
                easing: easing.to_string(),
            });
        }
        change
    }

    /// Template id this slide applies to the whole show.
    pub fn template_at(&self, index: usize) -> Option<String> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        let actions = node.pointer("/actions/slideActions")?.as_array()?;
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("set_template"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/set_template");
            let id = value
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.and_then(Value::as_str))
                .filter(|id| !id.is_empty());
            if let Some(id) = id {
                return Some(id.to_string());
            }
        }
        None
    }

    /// Overlay ids this slide turns on through an action, in stored order.
    pub fn action_overlays_at(&self, index: usize) -> Vec<String> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("id_select_overlay"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/id_select_overlay");
            let id = value
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.and_then(Value::as_str))
                .filter(|id| !id.is_empty());
            if let Some(id) = id {
                ids.push(id.to_string());
            }
        }
        ids
    }

    /// This slide pauses or resumes whatever is playing.
    pub fn toggles_media_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        action_trigger(node, "toggle_playing_media")
    }

    /// The camera this slide starts. A stored name wins, then the id.
    pub fn camera_at(&self, index: usize) -> Option<String> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        let actions = node.pointer("/actions/slideActions")?.as_array()?;
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("start_camera"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/start_camera");
            let name = value
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty());
            let id = value
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.and_then(Value::as_str))
                .filter(|id| !id.is_empty());
            if let Some(camera) = name.or(id) {
                return Some(camera.to_string());
            }
        }
        None
    }

    /// Stage layouts this slide selects, in order. The output id is set when the action names one window.
    pub fn stage_layouts_at(&self, index: usize) -> Vec<(Option<String>, String)> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for action in actions {
            let chosen = action
                .get("triggers")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    triggers
                        .iter()
                        .any(|trigger| trigger.as_str() == Some("change_stage_output_layout"))
                });
            if !chosen {
                continue;
            }
            let value = action.pointer("/actionValues/change_stage_output_layout");
            let layout = value
                .and_then(|value| value.get("stageLayoutId"))
                .and_then(Value::as_str)
                .or_else(|| value.and_then(Value::as_str))
                .filter(|id| !id.is_empty());
            let Some(layout) = layout else {
                continue;
            };
            let output = value
                .and_then(|value| value.get("outputId"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string);
            found.push((output, layout.to_string()));
        }
        found
    }

    /// Playlist actions on this slide, in the order they are stored.
    pub fn playlist_steps_at(&self, index: usize) -> Vec<PlaylistStep> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for action in actions {
            let triggers = action
                .get("triggers")
                .and_then(Value::as_array)
                .map(|triggers| {
                    triggers
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if triggers.iter().any(|trigger| trigger == "playlist_next") {
                found.push(PlaylistStep::Next);
            }
            let start = triggers.iter().any(|trigger| trigger == "start_playlist");
            let named = triggers
                .iter()
                .any(|trigger| trigger == "name_start_playlist");
            if !start && !named {
                continue;
            }
            let key = if named {
                "name_start_playlist"
            } else {
                "start_playlist"
            };
            let value = action.pointer(&format!("/actionValues/{key}"));
            let id = value
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .unwrap_or("")
                .to_string();
            let name = value
                .and_then(|value| value.get("value").or_else(|| value.get("name")))
                .and_then(Value::as_str)
                .or_else(|| value.and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            if start && !id.is_empty() {
                found.push(PlaylistStep::Start {
                    id,
                    name: String::new(),
                });
            } else if !name.is_empty() || !id.is_empty() {
                found.push(PlaylistStep::Start { id, name });
            }
        }
        found
    }

    /// Variable edits stored on this slide, in order.
    pub fn variable_actions_at(&self, index: usize) -> Vec<Value> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        actions
            .iter()
            .filter(|action| {
                action
                    .get("triggers")
                    .and_then(Value::as_array)
                    .is_some_and(|triggers| {
                        triggers
                            .iter()
                            .any(|trigger| trigger.as_str() == Some("change_variable"))
                    })
            })
            .filter_map(|action| action.pointer("/actionValues/change_variable").cloned())
            .collect()
    }

    /// Volume and mute actions on this slide, in the order they are stored.
    pub fn mix_steps_at(&self, index: usize) -> Vec<MixStep> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        for action in actions {
            let triggers = action
                .get("triggers")
                .and_then(Value::as_array)
                .map(|triggers| {
                    triggers
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if triggers.contains(&"change_volume") {
                let value = action.pointer("/actionValues/change_volume");
                let channel = value
                    .and_then(|value| value.get("channelId"))
                    .and_then(Value::as_str)
                    .filter(|channel| !channel.is_empty())
                    .unwrap_or("main");
                let volume = value
                    .and_then(|value| value.get("volume"))
                    .and_then(json_f64);
                let use_db = value
                    .and_then(|value| value.get("useDB"))
                    .and_then(Value::as_bool)
                    == Some(true);
                steps.push(MixStep::Volume {
                    channel: channel.to_string(),
                    volume,
                    use_db,
                });
            }
            if triggers.contains(&"mute") {
                let value = action.pointer("/actionValues/mute");
                let channel = value
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .filter(|channel| !channel.is_empty())
                    .unwrap_or("main");
                let muted = value.and_then(|value| match value.get("value") {
                    Some(flag) if flag.as_bool().is_some() => flag.as_bool(),
                    Some(flag) if flag.as_str() == Some("true") => Some(true),
                    Some(flag) if flag.as_str() == Some("false") => Some(false),
                    _ => None,
                });
                steps.push(MixStep::Mute {
                    channel: channel.to_string(),
                    muted,
                });
            }
        }
        steps
    }

    /// Auto-advance actions on this slide, in the order they are stored.
    pub fn timer_steps_at(&self, index: usize) -> Vec<TimerStep> {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return Vec::new();
        };
        let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        for action in actions {
            let triggers = action
                .get("triggers")
                .and_then(Value::as_array)
                .map(|triggers| {
                    triggers
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if triggers.contains(&"clear_next_timer") {
                steps.push(TimerStep::Clear);
            }
            if triggers.contains(&"set_next_slide_timer") {
                let value = action.pointer("/actionValues/set_next_slide_timer");
                let seconds = value.and_then(|value| {
                    value
                        .get("value")
                        .and_then(json_f64)
                        .or_else(|| json_f64(value))
                });
                if let Some(seconds) = seconds {
                    steps.push(TimerStep::Set(seconds));
                }
            }
        }
        steps
    }

    /// Writes `nextTimer` on every slide in the active layout.
    /// A positive duration also marks the last slide as the end of the loop.
    pub fn set_next_timers(&mut self, seconds: f64) {
        let layout_id = self.active_layout();
        let snapshot = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some(order) = snapshot.as_array() else {
            return;
        };
        struct Step {
            parent: usize,
            child: Option<String>,
            ended: bool,
        }
        let mut steps = Vec::new();
        for (parent_index, entry) in order.iter().enumerate() {
            let Some(parent_id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if self.show.pointer(&format!("/slides/{parent_id}")).is_none() {
                continue;
            }
            steps.push(Step {
                parent: parent_index,
                child: None,
                ended: entry.get("end").and_then(Value::as_bool) == Some(true),
            });
            let Some(children) = self
                .show
                .pointer(&format!("/slides/{parent_id}/children"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for child in children {
                let Some(child_id) = child.as_str() else {
                    continue;
                };
                if self.show.pointer(&format!("/slides/{child_id}")).is_none() {
                    continue;
                }
                steps.push(Step {
                    parent: parent_index,
                    child: Some(child_id.to_string()),
                    ended: entry
                        .pointer(&format!("/children/{child_id}/end"))
                        .and_then(Value::as_bool)
                        == Some(true),
                });
            }
        }
        if steps.is_empty() {
            return;
        }
        let lone_end = steps.iter().filter(|step| step.ended).count() == 1;
        let last = steps.len() - 1;
        let mark_last = seconds > 0.0;
        let Some(order) = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array_mut)
        else {
            return;
        };
        for (index, step) in steps.iter().enumerate() {
            let Some(entry) = order.get_mut(step.parent) else {
                continue;
            };
            let Some(target) = timer_node(entry, step.child.as_deref()) else {
                continue;
            };
            target.insert("nextTimer".to_string(), serde_json::json!(seconds));
            target.remove("nextTimerTarget");
            if lone_end && step.ended {
                target.remove("end");
            }
            if index == last {
                target.insert("end".to_string(), Value::Bool(mark_last));
            }
        }
    }

    /// Layout index of the group this name points at. Verse_2 is the second Verse.
    pub fn group_index(&self, name: &str) -> Option<usize> {
        let query = label_id(name);
        if query.is_empty() {
            return None;
        }
        let steps = self.layout_steps();
        let mut counts = std::collections::HashMap::<String, u32>::new();
        let mut contains = None;
        for (index, (slide_id, _)) in steps.iter().enumerate() {
            let group = self
                .show
                .pointer(&format!("/slides/{slide_id}/group"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if group.is_empty() {
                continue;
            }
            let base = label_id(group);
            if base.is_empty() {
                continue;
            }
            let count = counts.entry(base.clone()).or_insert(0);
            *count += 1;
            let labeled = if *count > 1 {
                format!("{base}_{count}")
            } else {
                base.clone()
            };
            if labeled == query {
                return Some(index);
            }
            if contains.is_none() && labeled.contains(&query) {
                contains = Some(index);
            }
        }
        contains
    }

    /// The next slide with this global group after `current`, or the first one if none follows.
    pub fn next_global_group(&self, group_id: &str, current: usize) -> Option<usize> {
        if group_id.is_empty() {
            return None;
        }
        let steps = self.layout_steps();
        let mut first = None;
        let mut after = None;
        for (index, (slide_id, _)) in steps.iter().enumerate() {
            let id = self
                .show
                .pointer(&format!("/slides/{slide_id}/globalGroup"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if id != group_id {
                continue;
            }
            if first.is_none() {
                first = Some(index);
            }
            if index > current && after.is_none() {
                after = Some(index);
            }
        }
        after.or(first).filter(|index| *index != current)
    }

    /// Another show this slide opens instead of displaying itself.
    pub fn started_show_at(&self, index: usize) -> Option<String> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        if let Some(id) = node
            .pointer("/actions/startShow/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            return Some(id.to_string());
        }
        node.pointer("/actions/slideActions")
            .and_then(Value::as_array)
            .and_then(|actions| {
                actions.iter().find_map(|action| {
                    action
                        .pointer("/actionValues/start_show/id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                })
            })
    }

    /// This slide can show on the audience output. A disabled slide cannot.
    /// A slide with bindings shows only when one binding is that output's id or name.
    pub fn can_output(&self, index: usize, output_id: &str, output_name: &str) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        Self::can_output_node(node, output_id, output_name)
    }

    /// First eligible occurrence of a slide in the active arrangement.
    pub fn output_index_for_slide(
        &self,
        slide_id: &str,
        output_id: &str,
        output_name: &str,
    ) -> Option<usize> {
        self.layout_steps().iter().position(|(id, node)| {
            id == slide_id && Self::can_output_node(node, output_id, output_name)
        })
    }

    fn can_output_node(node: &Value, output_id: &str, output_name: &str) -> bool {
        if node.get("disabled").and_then(Value::as_bool) == Some(true) {
            return false;
        }
        let Some(bindings) = node.get("bindings").and_then(Value::as_array) else {
            return true;
        };
        let bindings: Vec<&str> = bindings
            .iter()
            .filter_map(Value::as_str)
            .filter(|binding| !binding.is_empty())
            .collect();
        if bindings.is_empty() {
            return true;
        }
        if output_id.is_empty() {
            return false;
        }
        bindings.iter().any(|binding| {
            *binding == output_id
                || (!output_name.is_empty() && binding.eq_ignore_ascii_case(output_name))
        })
    }

    /// The next slide that can show on this output. `delta` is `1` or `-1`.
    pub fn step_output(
        &self,
        from: usize,
        delta: isize,
        output_id: &str,
        output_name: &str,
    ) -> Option<usize> {
        if delta == 0 {
            return None;
        }
        let len = self.layout_steps().len() as isize;
        let mut index = from as isize;
        loop {
            index += delta;
            if index < 0 || index >= len {
                return None;
            }
            if self.can_output(index as usize, output_id, output_name) {
                return Some(index as usize);
            }
        }
    }

    /// The first slide that can show on this output.
    pub fn first_output(&self, output_id: &str, output_name: &str) -> Option<usize> {
        (0..self.layout_steps().len()).find(|index| self.can_output(*index, output_id, output_name))
    }

    /// The most lyric lines on any item of this slide.
    pub fn most_lines(&self, index: usize) -> usize {
        let steps = self.layout_steps();
        let Some((slide_id, _)) = steps.get(index) else {
            return 0;
        };
        self.show
            .pointer(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| !item_hidden(item))
                    .map(|item| {
                        item.get("lines")
                            .and_then(Value::as_array)
                            .map(|lines| lines.len())
                            .unwrap_or(0)
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// This slide has an item that stays hidden until the next click.
    pub fn has_click_reveal(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((slide_id, _)) = steps.get(index) else {
            return false;
        };
        self.show
            .pointer(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    !item_hidden(item)
                        && item.get("clickReveal").and_then(Value::as_bool) == Some(true)
                })
            })
    }

    /// How many lines the line-reveal items on this slide have. `None` when none reveal.
    pub fn line_reveal_lines(&self, index: usize) -> Option<usize> {
        let steps = self.layout_steps();
        let (slide_id, _) = steps.get(index)?;
        let items = self
            .show
            .pointer(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array)?;
        let mut most = 0usize;
        let mut any = false;
        for item in items {
            if item_hidden(item) || item.get("lineReveal").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            any = true;
            let count = item
                .get("lines")
                .and_then(Value::as_array)
                .map(|lines| lines.len())
                .unwrap_or(0);
            most = most.max(count);
        }
        any.then_some(most)
    }

    /// The slide's stage-eligible text boxes in storage order.
    /// Empty groups retain their box number until a reveal makes the text visible.
    pub fn stage_text_boxes(
        &self,
        index: usize,
        current: bool,
        reveal: u32,
        clicks_shown: bool,
        page: Option<(usize, usize)>,
    ) -> Vec<Vec<String>> {
        let steps = self.layout_steps();
        let Some((slide_id, _)) = steps.get(index) else {
            return Vec::new();
        };
        self.stage_text_boxes_for_slide(slide_id, current, reveal, clicks_shown, page)
    }

    pub fn stage_text_boxes_for_slide(
        &self,
        slide_id: &str,
        current: bool,
        reveal: u32,
        clicks_shown: bool,
        page: Option<(usize, usize)>,
    ) -> Vec<Vec<String>> {
        Self::stage_text_boxes_from_slide(
            &self.show["slides"][slide_id],
            current,
            reveal,
            clicks_shown,
            page,
        )
    }

    pub fn stage_text_boxes_from_slide(
        slide: &Value,
        current: bool,
        reveal: u32,
        clicks_shown: bool,
        page: Option<(usize, usize)>,
    ) -> Vec<Vec<String>> {
        let Some(items) = slide.get("items").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut boxes = Vec::new();
        for item in items {
            if item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| !kind.is_empty() && kind != "text")
                || item
                    .get("bindings")
                    .and_then(Value::as_array)
                    .is_some_and(|bindings| {
                        !bindings.is_empty()
                            && !bindings
                                .iter()
                                .any(|target| target.as_str() == Some("stage"))
                    })
            {
                continue;
            }
            let click = item.get("clickReveal").and_then(Value::as_bool) == Some(true);
            let line_reveal = item.get("lineReveal").and_then(Value::as_bool) == Some(true);
            boxes.push(Vec::new());
            if item_hidden(item)
                || (click && !(current && clicks_shown))
                || (line_reveal && !current)
            {
                continue;
            }
            let Some(item_lines) = item.get("lines").and_then(Value::as_array) else {
                continue;
            };
            let lines = boxes.last_mut().unwrap();
            for (line_index, line) in item_lines.iter().enumerate() {
                if line_reveal && current && line_index >= reveal as usize {
                    continue;
                }
                if current
                    && !line_reveal
                    && let Some((start, end)) = page
                    && (line_index < start || line_index >= end)
                {
                    continue;
                }
                lines.push(line_value(line));
            }
        }
        boxes
    }

    /// This slide is marked as the end, so the next step returns to the start.
    pub fn ends_at(&self, index: usize) -> bool {
        self.layout_steps()
            .get(index)
            .and_then(|(_, node)| node.get("end").and_then(Value::as_bool))
            == Some(true)
    }

    /// The next layout slide that is not disabled. `delta` is `1` or `-1`.
    pub fn step_enabled(&self, from: usize, delta: isize) -> Option<usize> {
        if delta == 0 {
            return None;
        }
        let len = self.layout_steps().len() as isize;
        let mut index = from as isize;
        loop {
            index += delta;
            if index < 0 || index >= len {
                return None;
            }
            let disabled = self
                .layout_steps()
                .get(index as usize)
                .and_then(|(_, node)| node.get("disabled").and_then(Value::as_bool))
                == Some(true);
            if !disabled {
                return Some(index as usize);
            }
        }
    }

    /// The first layout slide that is not disabled.
    pub fn first_enabled_slide(&self) -> Option<usize> {
        self.layout_steps()
            .iter()
            .position(|(_, node)| node.get("disabled").and_then(Value::as_bool) != Some(true))
    }

    /// Timer ids this slide starts, from `id_start_timer` and timer items.
    pub fn timer_ids_at(&self, index: usize) -> Vec<String> {
        let steps = self.layout_steps();
        let Some((slide_id, node)) = steps.get(index) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        if let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        {
            for action in actions {
                let id = action
                    .pointer("/actionValues/id_start_timer/id")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        action
                            .pointer("/actionValues/id_start_timer")
                            .and_then(Value::as_str)
                    })
                    .filter(|id| !id.is_empty());
                if let Some(id) = id {
                    ids.push(id.to_string());
                }
            }
        }
        let scan_items = node.pointer("/actions/startTimer").and_then(Value::as_bool) == Some(true)
            || action_trigger(node, "start_slide_timers");
        if scan_items
            && let Some(items) = self
                .show
                .pointer(&format!("/slides/{slide_id}/items"))
                .and_then(Value::as_array)
        {
            for item in items {
                if item.get("type").and_then(Value::as_str) != Some("timer") {
                    continue;
                }
                let id = item
                    .pointer("/timer/id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("timerId").and_then(Value::as_str))
                    .filter(|id| !id.is_empty());
                if let Some(id) = id {
                    ids.push(id.to_string());
                }
            }
        }
        ids
    }

    /// The output style this slide turns on, from `change_output_style` or `outputStyle`.
    pub fn output_style_at(&self, index: usize) -> Option<String> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        if let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        {
            for action in actions {
                let chosen = action
                    .get("triggers")
                    .and_then(Value::as_array)
                    .is_some_and(|triggers| {
                        triggers
                            .iter()
                            .any(|trigger| trigger.as_str() == Some("change_output_style"))
                    });
                if !chosen {
                    continue;
                }
                let value = action.pointer("/actionValues/change_output_style");
                let id = value
                    .and_then(|value| value.get("outputStyle").or_else(|| value.get("styleId")))
                    .and_then(Value::as_str)
                    .or_else(|| value.and_then(Value::as_str))
                    .unwrap_or("");
                return Some(id.to_string());
            }
        }
        node.pointer("/actions/outputStyle")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Audio stream this slide starts: a saved id, a file path, or a web address.
    pub fn audio_stream_at(&self, index: usize) -> Option<String> {
        let steps = self.layout_steps();
        let (_, node) = steps.get(index)?;
        if let Some(actions) = node
            .pointer("/actions/slideActions")
            .and_then(Value::as_array)
        {
            for action in actions {
                let chosen = action
                    .get("triggers")
                    .and_then(Value::as_array)
                    .is_some_and(|triggers| {
                        triggers
                            .iter()
                            .any(|trigger| trigger.as_str() == Some("start_audio_stream"))
                    });
                if !chosen {
                    continue;
                }
                let value = action.pointer("/actionValues/start_audio_stream");
                let target = value
                    .and_then(|value| {
                        value
                            .get("value")
                            .and_then(Value::as_str)
                            .filter(|target| !target.is_empty())
                            .or_else(|| {
                                value
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .filter(|id| !id.is_empty())
                            })
                    })
                    .or_else(|| value.and_then(Value::as_str).filter(|id| !id.is_empty()));
                if let Some(target) = target {
                    return Some(target.to_string());
                }
            }
        }
        node.pointer("/actions/audioStream")
            .and_then(Value::as_str)
            .filter(|target| !target.is_empty())
            .map(str::to_string)
    }

    /// This slide goes on when its own video or audio finishes.
    pub fn nexts_after_media_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        node.pointer("/actions/nextAfterMedia")
            .and_then(Value::as_bool)
            == Some(true)
    }

    /// The file that just ended belongs to this slide, and the slide is waiting for it.
    pub fn waits_for_ended(&self, index: usize, ended: &str, audio: bool) -> bool {
        if ended.is_empty() || !self.nexts_after_media_at(index) {
            return false;
        }
        if audio {
            return self.audio_at(index).iter().any(|path| path == ended);
        }
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        let Some(id) = node.get("background").and_then(Value::as_str) else {
            return false;
        };
        if id == ended {
            return true;
        }
        self.show
            .pointer(&format!("/media/{id}/path"))
            .and_then(Value::as_str)
            == Some(ended)
    }

    /// This slide pauses the countdown that is running.
    pub fn pauses_timers_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        action_trigger(node, "pause_timers")
    }

    /// This slide stops the countdown that is running.
    pub fn stops_timers_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        node.pointer("/actions/stopTimers").and_then(Value::as_bool) == Some(true)
            || action_trigger(node, "stop_timers")
    }

    /// This slide stops the audio that is playing.
    pub fn clears_audio_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        node.pointer("/actions/clearAudio").and_then(Value::as_bool) == Some(true)
            || action_trigger(node, "clear_audio")
            || action_trigger(node, "clear_all")
    }

    pub fn stops_transition_sound_at(&self, index: usize) -> bool {
        self.layout_node_at(index)
            .is_some_and(|node| node["actions"]["stopTransitionSound"] == true)
    }

    /// This slide hides the lyrics and leaves the picture.
    pub fn clears_slide_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        action_trigger(node, "clear_slide") || action_trigger(node, "clear_all")
    }

    /// This slide explicitly requests a full output clear.
    pub fn clears_all_at(&self, index: usize) -> bool {
        self.layout_node_at(index)
            .is_some_and(|node| action_trigger(&node, "clear_all"))
    }

    /// This slide removes the overlays already on screen.
    pub fn clears_overlays_at(&self, index: usize) -> bool {
        let steps = self.layout_steps();
        let Some((_, node)) = steps.get(index) else {
            return false;
        };
        node.pointer("/actions/clearOverlays")
            .and_then(Value::as_bool)
            == Some(true)
            || action_trigger(node, "clear_overlays")
            || action_trigger(node, "clear_all")
    }

    fn layout_steps(&self) -> Vec<(String, Value)> {
        let layout_id = self.active_layout();
        let Some(order) = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        for entry in order {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if self.show.pointer(&format!("/slides/{id}")).is_none() {
                continue;
            }
            steps.push((id.to_string(), entry.clone()));
            let Some(children) = self
                .show
                .pointer(&format!("/slides/{id}/children"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for child in children {
                let Some(child_id) = child.as_str() else {
                    continue;
                };
                if self.show.pointer(&format!("/slides/{child_id}")).is_none() {
                    continue;
                }
                let node = entry
                    .pointer(&format!("/children/{child_id}"))
                    .cloned()
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                steps.push((child_id.to_string(), node));
            }
        }
        steps
    }

    /// Loop, mute, and video type stored on the show media entry for this path.
    /// Loop and mute default to on. The video type is only set when the file stores one.
    pub fn media_flags(&self, path: &str) -> Option<(bool, bool, Option<String>)> {
        self.media_entry(path).map(Self::media_entry_flags)
    }

    fn media_entry(&self, path: &str) -> Option<&Value> {
        self.show
            .get("media")?
            .as_object()?
            .values()
            .find(|value| value["path"].as_str() == Some(path))
    }

    /// Prefers this slide's own media entry when the same path is used in both layers.
    pub fn backdrop_media_flags_at(
        &self,
        index: usize,
        path: &str,
    ) -> Option<(bool, bool, Option<String>)> {
        self.backdrop_media_entry_at(index, path)
            .map(Self::media_entry_flags)
    }

    pub fn backdrop_media_entry_at(&self, index: usize, path: &str) -> Option<&Value> {
        self.layout_node_at(index)
            .and_then(|node| {
                node["background"]
                    .as_str()
                    .and_then(|id| self.show["media"].get(id))
            })
            .filter(|entry| entry["path"].as_str() == Some(path))
            .or_else(|| self.media_entry(path))
    }

    /// Reads the loop, mute, and layer settings from a show media entry.
    pub fn media_entry_flags(value: &Value) -> (bool, bool, Option<String>) {
        let loops = value["loop"] != false;
        let muted = value["muted"] != false;
        let video_type = value["videoType"]
            .as_str()
            .filter(|kind| !kind.is_empty())
            .map(str::to_string);
        (loops, muted, video_type)
    }

    fn media_picture(&self, id: &str) -> Option<(String, bool, bool)> {
        let media = self.show.pointer(&format!("/media/{id}"))?;
        let path = media
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())?;
        let foreground = media.get("videoType").and_then(Value::as_str) == Some("foreground");
        let once = media.get("loop").and_then(Value::as_bool) == Some(false);
        Some((path.to_string(), foreground, once))
    }

    fn layout_node_at(&self, index: usize) -> Option<Value> {
        let layout_id = self.active_layout();
        let order = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)?;
        if order.is_empty() {
            return None;
        }
        let mut cursor = 0usize;
        for entry in order {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if self.show.pointer(&format!("/slides/{id}")).is_none() {
                continue;
            }
            if cursor == index {
                return Some(entry.clone());
            }
            cursor += 1;
            let Some(children) = self
                .show
                .pointer(&format!("/slides/{id}/children"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for child in children {
                let Some(child_id) = child.as_str() else {
                    continue;
                };
                if self.show.pointer(&format!("/slides/{child_id}")).is_none() {
                    continue;
                }
                if cursor == index {
                    return entry
                        .pointer(&format!("/children/{child_id}"))
                        .cloned()
                        .or_else(|| Some(Value::Object(serde_json::Map::new())));
                }
                cursor += 1;
            }
        }
        None
    }

    fn slides_in_layout(&self, layout_id: &str) -> Vec<SlideView> {
        let stored = self.slides();
        let Some(order) = self
            .show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)
        else {
            return stored;
        };
        if order.is_empty() {
            return stored;
        }
        let by_id: std::collections::HashMap<String, SlideView> = stored
            .into_iter()
            .map(|slide| (slide.id.clone(), slide))
            .collect();
        let mut slides = Vec::new();
        for entry in order {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(slide) = by_id.get(id) {
                slides.push(slide.clone());
            }
            let Some(children) = self
                .show
                .pointer(&format!("/slides/{id}/children"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for child in children {
                let Some(child_id) = child.as_str() else {
                    continue;
                };
                if let Some(slide) = by_id.get(child_id) {
                    slides.push(slide.clone());
                }
            }
        }
        slides
    }

    pub fn plain_text(&self, slide_id: &str) -> String {
        self.slides()
            .into_iter()
            .find(|slide| slide.id == slide_id)
            .map(|slide| slide.lines.join("\n"))
            .unwrap_or_default()
    }

    /// Replaces a slide's lyric lines. Other item fields, and extra show fields, stay.
    pub fn set_plain_text(&mut self, slide_id: &str, text: &str) -> bool {
        let Some(slide) = self
            .show
            .get_mut("slides")
            .and_then(Value::as_object_mut)
            .and_then(|slides| slides.get_mut(slide_id))
        else {
            return false;
        };
        let Some(slide) = slide.as_object_mut() else {
            return false;
        };
        let items = slide
            .entry("items")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(items) = items.as_array_mut() else {
            return false;
        };
        if items.is_empty() {
            items.push(serde_json::json!({ "style": "", "lines": [] }));
        }
        let Some(item) = items[0].as_object_mut() else {
            return false;
        };
        let lines_value = item
            .entry("lines")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(lines) = lines_value.as_array_mut() else {
            return false;
        };
        let new_lines: Vec<&str> = if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').collect()
        };
        lines.truncate(new_lines.len());
        while lines.len() < new_lines.len() {
            lines.push(serde_json::json!({
                "align": "",
                "text": [{ "value": "", "style": "" }]
            }));
        }
        for (line, text) in lines.iter_mut().zip(new_lines) {
            let Some(line) = line.as_object_mut() else {
                continue;
            };
            let parts = line
                .entry("text")
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(parts) = parts.as_array_mut() else {
                continue;
            };
            if parts.is_empty() {
                parts.push(serde_json::json!({ "value": text, "style": "" }));
            } else if let Some(first) = parts[0].as_object_mut() {
                first.insert("value".to_string(), Value::String(text.to_string()));
                parts.truncate(1);
            }
        }
        true
    }

    /// Sets the slide group label. An empty group is stored as an empty string.
    pub fn set_group(&mut self, slide_id: &str, group: &str) -> bool {
        let Some(slide) = self
            .show
            .get_mut("slides")
            .and_then(Value::as_object_mut)
            .and_then(|slides| slides.get_mut(slide_id))
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        slide.insert("group".to_string(), Value::String(group.to_string()));
        true
    }

    /// Sets the operator/stage notes text for a slide.
    pub fn set_notes(&mut self, slide_id: &str, notes: &str) -> bool {
        let Some(slide) = self
            .show
            .get_mut("slides")
            .and_then(Value::as_object_mut)
            .and_then(|slides| slides.get_mut(slide_id))
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        slide.insert("notes".to_string(), Value::String(notes.to_string()));
        true
    }

    /// Moves one named group within the active layout. Indexes skip slides with no group.
    pub fn reorder_groups(&mut self, from: usize, to: usize) -> bool {
        if from == to {
            return false;
        }
        let layout_id = self
            .show
            .pointer("/settings/activeLayout")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string();
        let ids: Vec<(usize, String)> = {
            let Some(order) = self
                .show
                .pointer(&format!("/layouts/{layout_id}/slides"))
                .and_then(Value::as_array)
            else {
                return false;
            };
            order
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|id| (index, id.to_string()))
                })
                .collect()
        };
        let positions: Vec<usize> = ids
            .into_iter()
            .filter(|(_, id)| {
                self.show
                    .pointer(&format!("/slides/{id}/group"))
                    .and_then(Value::as_str)
                    .is_some_and(|group| !group.is_empty())
            })
            .map(|(index, _)| index)
            .collect();
        if from >= positions.len() || to >= positions.len() {
            return false;
        }
        let Some(order) = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let mut grouped: Vec<Value> = positions
            .iter()
            .map(|index| order[*index].clone())
            .collect();
        let item = grouped.remove(from);
        grouped.insert(to, item);
        for (index, value) in positions.into_iter().zip(grouped) {
            order[index] = value;
        }
        true
    }

    /// Appends a copy of a group slide to the active layout.
    pub fn duplicate_slide(&mut self, slide_id: &str) -> Option<String> {
        let mut slide = self.show.pointer(&format!("/slides/{slide_id}"))?.clone();
        if let Some(object) = slide.as_object_mut() {
            object.remove("children");
        }
        let new_id = {
            let slides = self.show.get("slides")?.as_object()?;
            let mut number = slides.len() + 1;
            let mut id = format!("s{number}");
            while slides.contains_key(&id) {
                number += 1;
                id = format!("s{number}");
            }
            id
        };
        {
            let slides = self.show.get_mut("slides")?.as_object_mut()?;
            slides.insert(new_id.clone(), slide);
        }
        let layout_id = self
            .show
            .pointer("/settings/activeLayout")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string();
        let order = self
            .show
            .pointer_mut(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array_mut)?;
        order.push(serde_json::json!({ "id": new_id }));
        Some(new_id)
    }

    /// Moves or resizes the first text box. Other CSS declarations stay in the style string.
    /// Distances are in the 1920×1080 slide space FreeShow stores.
    pub fn nudge_item_box(&mut self, slide_id: &str, dx: i32, dy: i32, dw: i32, dh: i32) -> bool {
        let Some(style) = self
            .show
            .pointer(&format!("/slides/{slide_id}/items/0/style"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return false;
        };
        let next = adjust_box(&style, dx, dy, dw, dh);
        let Some(value) = self
            .show
            .pointer_mut(&format!("/slides/{slide_id}/items/0/style"))
        else {
            return false;
        };
        *value = Value::String(next);
        true
    }
}

fn split_stanzas(stanzas: Vec<(String, String)>, split_lines: u32) -> Vec<(String, String)> {
    if split_lines == 0 {
        return stanzas;
    }
    let mut split = Vec::new();
    for (group, chunk) in stanzas {
        let lines: Vec<&str> = chunk.split('\n').collect();
        if lines.len() as u32 <= split_lines {
            split.push((group, chunk));
            continue;
        }
        for part in lines.chunks(split_lines as usize) {
            split.push((group.clone(), part.join("\n")));
        }
    }
    split
}

fn formatted_stanzas(text: &str) -> Vec<(String, String)> {
    let chunks: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect();
    if chunks.is_empty() {
        return vec![(String::new(), String::new())];
    }
    chunks
        .into_iter()
        .map(|chunk| {
            let mut lines: Vec<&str> = chunk.lines().collect();
            let group = lines
                .first()
                .and_then(|line| group_header(line))
                .unwrap_or_default();
            if !group.is_empty() {
                lines.remove(0);
            }
            (group, lines.join("\n"))
        })
        .collect()
}

/// FreeShow's built-in group for a slide group name like "Verse 2" or "Pre-Chorus": its id and colour.
pub fn default_group(group: &str) -> Option<(&'static str, &'static str)> {
    let key = group
        .trim()
        .trim_end_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace())
        .to_ascii_lowercase()
        .replace(['-', ' ', '_'], "");
    Some(match key.as_str() {
        "break" | "interlude" | "instrumental" => ("break", "#f5255e"),
        "bridge" => ("bridge", "#f52598"),
        "chorus" | "refrain" => ("chorus", "#f525d2"),
        "intro" => ("intro", "#d525f5"),
        "outro" | "ending" | "coda" => ("outro", "#a525f5"),
        "prechorus" => ("pre_chorus", "#8825f5"),
        "tag" | "vamp" => ("tag", "#7525f5"),
        "verse" => ("verse", "#5825f5"),
        _ => return None,
    })
}

fn group_header(line: &str) -> Option<String> {
    let line = line.trim();
    let line = line
        .strip_prefix('[')
        .and_then(|line| line.strip_suffix(']'))
        .unwrap_or(line);
    let line = line.strip_suffix(':').unwrap_or(line).trim();
    if line.is_empty() {
        return None;
    }
    const NAMES: &[&str] = &[
        "verse",
        "chorus",
        "bridge",
        "pre-chorus",
        "prechorus",
        "intro",
        "outro",
        "tag",
        "ending",
        "interlude",
        "vamp",
        "instrumental",
        "break",
        "coda",
    ];
    let lowered = line.to_ascii_lowercase();
    let key = lowered
        .trim_end_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace())
        .trim();
    if NAMES
        .iter()
        .any(|name| key == *name || (key.starts_with(name) && key.len() <= name.len() + 3))
    {
        Some(title_group(line))
    } else {
        None
    }
}

fn title_group(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub(crate) fn adjust_box(style: &str, dx: i32, dy: i32, dw: i32, dh: i32) -> String {
    let mut left = 50;
    let mut top = 88;
    let mut width = 1820;
    let mut height = 904;
    let mut rest = Vec::new();
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((key, value)) = decl.split_once(':') else {
            rest.push(decl.to_string());
            continue;
        };
        match key.trim() {
            "left" => left = px_number(value).unwrap_or(left),
            "top" => top = px_number(value).unwrap_or(top),
            "width" => width = px_number(value).unwrap_or(width),
            "height" => height = px_number(value).unwrap_or(height),
            _ => rest.push(format!("{}:{}", key.trim(), value.trim())),
        }
    }
    left = (left + dx).clamp(0, 1920);
    top = (top + dy).clamp(0, 1080);
    width = (width + dw).clamp(40, 1920);
    height = (height + dh).clamp(40, 1080);
    let mut parts = vec![
        format!("left:{left}px"),
        format!("top:{top}px"),
        format!("width:{width}px"),
        format!("height:{height}px"),
    ];
    parts.extend(rest);
    let mut written = parts.join(";");
    written.push(';');
    written
}

fn px_number(value: &str) -> Option<i32> {
    let value = value.trim().trim_end_matches("px").trim();
    value
        .parse::<f32>()
        .ok()
        .map(|number| number.round() as i32)
}

pub(crate) fn json_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    value.as_str().and_then(|text| text.parse().ok())
}

/// Replaces `{variable_name}` and `{$name}` with the stored value. An empty value uses the `|` or `?` fallback.
pub fn fill_variables(text: &str, variables: &[(String, String)]) -> String {
    if !text.contains('{') || variables.is_empty() {
        return text.to_string();
    }
    let mut result = text.to_string();
    for (name, value) in variables {
        result = replace_variable_token(&result, &format!("variable_{name}"), value);
        result = replace_variable_token(&result, &format!("${name}"), value);
    }
    result
}

fn replace_variable_token(text: &str, id: &str, value: &str) -> String {
    let needle = format!("{{{id}");
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(&needle) {
        out.push_str(&rest[..start]);
        let after = &rest[start + needle.len()..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inside = &after[..end];
        if inside.is_empty() {
            out.push_str(value);
            rest = &after[end + 1..];
            continue;
        }
        if let Some(fallback) = inside
            .strip_prefix('|')
            .or_else(|| inside.strip_prefix('?'))
        {
            if value.is_empty() {
                out.push_str(fallback);
            } else {
                out.push_str(value);
            }
            rest = &after[end + 1..];
            continue;
        }
        out.push('{');
        rest = &rest[start + 1..];
    }
    out.push_str(rest);
    out
}

fn variable_key(
    json: &Value,
    id: Option<&str>,
    name: Option<&str>,
    index: Option<usize>,
) -> Option<String> {
    let map = json.get("variables")?.as_object()?;
    if let Some(id) = id.filter(|id| !id.is_empty())
        && map.contains_key(id)
    {
        return Some(id.to_string());
    }
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        let query = variable_name_id(name);
        let mut exact = None;
        let mut contains = None;
        for (key, variable) in map {
            let stored =
                variable_name_id(variable.get("name").and_then(Value::as_str).unwrap_or(""));
            if stored == query && exact.is_none() {
                exact = Some(key.clone());
            }
            if contains.is_none() && !query.is_empty() && stored.contains(&query) {
                contains = Some(key.clone());
            }
        }
        return exact.or(contains);
    }
    let index = index.filter(|index| *index > 0)?;
    let mut named: Vec<(String, String)> = map
        .iter()
        .filter_map(|(key, variable)| {
            let name = variable.get("name").and_then(Value::as_str)?;
            Some((variable_name_id(name), key.clone()))
        })
        .collect();
    named.sort_by(|left, right| left.0.cmp(&right.0));
    named.get(index - 1).map(|(_, key)| key.clone())
}

fn audio_list(entry: &Value) -> Option<AudioList> {
    let songs = entry
        .get("songs")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|song| song.as_str().map(str::to_string))
        .filter(|song| !song.is_empty())
        .collect::<Vec<_>>();
    if songs.is_empty() {
        return None;
    }
    let volume = entry
        .get("volume")
        .and_then(json_f64)
        .filter(|volume| *volume > 0.0)
        .unwrap_or(1.0) as f32;
    Some(AudioList {
        name: entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        songs,
        shuffle: entry.get("mode").and_then(Value::as_str) == Some("shuffle"),
        loops: entry.get("loop").and_then(Value::as_bool) != Some(false),
        volume,
    })
}

/// Plays the songs in a shuffled order. The same playlist id keeps the same order.
pub fn shuffled_songs(songs: &[String], playlist_id: &str) -> Vec<String> {
    let mut items = songs.to_vec();
    let mut state = playlist_id.bytes().fold(0u64, |state, byte| {
        state.wrapping_mul(131).wrapping_add(u64::from(byte))
    }) | 1;
    for index in (1..items.len()).rev() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let pick = (state as usize) % (index + 1);
        items.swap(index, pick);
    }
    items
}

fn variable_name_id(name: &str) -> String {
    name.to_lowercase().trim().replace(' ', "_")
}

fn variable_text(variable: &Value) -> String {
    let kind = variable
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("text");
    match kind {
        "number" | "random_number" => {
            let number = variable
                .get("number")
                .and_then(json_f64)
                .or_else(|| variable.get("default").and_then(json_f64))
                .unwrap_or(0.0);
            if number.fract() == 0.0 {
                format!("{}", number as i64)
            } else {
                format!("{number}")
            }
        }
        _ => {
            if variable.get("enabled").and_then(Value::as_bool) == Some(false) {
                return String::new();
            }
            variable
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        }
    }
}

fn line_value(line: &Value) -> String {
    line.get("text")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("value").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Extracts unstyled text from a stored slide, including a taken slide snapshot.
pub fn slide_lines(slide: &Value) -> Vec<String> {
    collect_slide_lines(slide, false)
}

pub fn visible_slide_lines(slide: &Value) -> Vec<String> {
    collect_slide_lines(slide, true)
}

fn collect_slide_lines(slide: &Value, visible_only: bool) -> Vec<String> {
    let Some(items) = slide.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for item in items {
        if visible_only && item_hidden(item) {
            continue;
        }
        let Some(item_lines) = item.get("lines").and_then(Value::as_array) else {
            continue;
        };
        for line in item_lines {
            lines.push(line_value(line));
        }
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectShowRef {
    pub id: String,
    pub label: String,
    /// FreeShow show type, usually `show` or `section`.
    pub kind: String,
    /// Slideshow delay stored on a folder or PDF row, in milliseconds.
    pub timer_ms: Option<u64>,
    /// Project-row override. Missing means the video type decides.
    pub loops: Option<bool>,
    /// Project-row override. Missing means the video type decides.
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    pub parent: String,
    pub created: u64,
    pub shows: Vec<ProjectShowRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFolder {
    pub id: String,
    pub name: String,
    pub parent: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectsFile {
    document: Value,
}

impl ProjectsFile {
    pub fn empty() -> Self {
        Self {
            document: Value::Object(Map::from_iter([
                ("projects".to_string(), Value::Object(Map::new())),
                ("folders".to_string(), Value::Object(Map::new())),
                ("projectTemplates".to_string(), Value::Object(Map::new())),
            ])),
        }
    }

    pub fn parse(text: &str) -> Result<Self, DataError> {
        let document: Value = serde_json::from_str(text)?;
        if !document.is_object() {
            return Err(DataError::ShowShape);
        }
        Ok(Self { document })
    }

    pub fn load(path: &Path) -> Result<Self, DataError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn project_matches(&self, other: &Self, id: &str) -> bool {
        self.document
            .get("projects")
            .and_then(|projects| projects.get(id))
            == other
                .document
                .get("projects")
                .and_then(|projects| projects.get(id))
    }

    pub fn projects(&self) -> Vec<ProjectEntry> {
        let Some(projects) = self.document.get("projects").and_then(Value::as_object) else {
            return Vec::new();
        };
        projects
            .iter()
            .map(|(id, project)| ProjectEntry {
                id: id.clone(),
                name: project
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                parent: project
                    .get("parent")
                    .and_then(Value::as_str)
                    .unwrap_or("/")
                    .to_string(),
                created: json_u64(project.get("created")).unwrap_or(0),
                shows: project
                    .get("shows")
                    .and_then(Value::as_array)
                    .map(|shows| {
                        shows
                            .iter()
                            .filter_map(|show| {
                                show.get("id")
                                    .and_then(Value::as_str)
                                    .map(|id| ProjectShowRef {
                                        id: id.to_string(),
                                        label: show
                                            .get("name")
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                            .to_string(),
                                        kind: show
                                            .get("type")
                                            .and_then(Value::as_str)
                                            .unwrap_or("show")
                                            .to_string(),
                                        timer_ms: show
                                            .pointer("/data/timer")
                                            .and_then(Value::as_f64)
                                            .filter(|seconds| *seconds > 0.0)
                                            .map(|seconds| (seconds * 1000.0).round() as u64),
                                        loops: show.get("loop").and_then(Value::as_bool),
                                        muted: show.get("muted").and_then(Value::as_bool),
                                    })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    pub fn folders(&self) -> Vec<ProjectFolder> {
        let Some(folders) = self.document.get("folders").and_then(Value::as_object) else {
            return Vec::new();
        };
        folders
            .iter()
            .map(|(id, folder)| ProjectFolder {
                id: id.clone(),
                name: folder
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                parent: folder
                    .get("parent")
                    .and_then(Value::as_str)
                    .unwrap_or("/")
                    .to_string(),
            })
            .collect()
    }

    pub fn to_json(&self) -> Result<String, DataError> {
        Ok(serde_json::to_string(&self.document)?)
    }

    pub fn save(&self, path: &Path) -> Result<(), DataError> {
        atomic_write(path, self.to_json()?.as_bytes())
    }

    pub fn reorder(&mut self, project_id: &str, index: usize, delta: isize) -> bool {
        let Some(next) = index.checked_add_signed(delta) else {
            return false;
        };
        let Some(shows) = self
            .document
            .pointer_mut(&format!("/projects/{project_id}/shows"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        if index >= shows.len() || next >= shows.len() {
            return false;
        }
        let item = shows.remove(index);
        shows.insert(next, item);
        true
    }

    pub fn append_show(&mut self, project_id: &str, show_id: &str) -> bool {
        let Some(project) = self
            .document
            .pointer_mut(&format!("/projects/{project_id}"))
        else {
            return false;
        };
        let Some(project) = project.as_object_mut() else {
            return false;
        };
        let shows = project
            .entry("shows")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(shows) = shows.as_array_mut() else {
            return false;
        };
        shows.push(serde_json::json!({ "id": show_id, "type": "show" }));
        true
    }

    /// Adds a service-plan show with its planned duration in seconds.
    pub fn append_scheduled_show(&mut self, project_id: &str, show_id: &str, seconds: f64) -> bool {
        if !seconds.is_finite() || seconds < 0.0 {
            return false;
        }
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        shows.push(serde_json::json!({"id":show_id,"type":"show","scheduleLength":seconds}));
        true
    }

    pub fn append_item(&mut self, project_id: &str, id: &str, kind: &str) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        shows.push(serde_json::json!({ "id": id, "type": kind }));
        true
    }

    /// Stores the project-row loop and mute flags FreeShow reads for a video.
    pub fn set_item_playback(
        &mut self,
        project_id: &str,
        index: usize,
        loops: bool,
        muted: bool,
    ) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        let Some(show) = shows.get_mut(index).and_then(Value::as_object_mut) else {
            return false;
        };
        show.insert("loop".to_string(), Value::Bool(loops));
        show.insert("muted".to_string(), Value::Bool(muted));
        true
    }

    pub fn append_section(&mut self, project_id: &str, id: &str, name: &str, notes: &str) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        shows
            .push(serde_json::json!({ "id": id, "type": "section", "name": name, "notes": notes }));
        true
    }

    /// Inserts a section header at `index` (clamped to the end of the project).
    pub fn insert_section(&mut self, project_id: &str, index: usize, id: &str, name: &str) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        let index = index.min(shows.len());
        shows.insert(
            index,
            serde_json::json!({ "id": id, "type": "section", "name": name, "notes": "" }),
        );
        true
    }

    /// Renames the section header at `index`. Other rows are left alone.
    pub fn rename_section(&mut self, project_id: &str, index: usize, name: &str) -> bool {
        let Some(row) = self
            .project_shows_mut(project_id)
            .and_then(|shows| shows.get_mut(index))
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        if row.get("type").and_then(Value::as_str) != Some("section") {
            return false;
        }
        row.insert("name".to_string(), Value::String(name.to_string()));
        true
    }

    pub fn clear_shows(&mut self, project_id: &str) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        shows.clear();
        true
    }

    pub fn set_project_name(&mut self, project_id: &str, name: &str) -> bool {
        let Some(slot) = self
            .document
            .pointer_mut(&format!("/projects/{project_id}/name"))
        else {
            return false;
        };
        *slot = Value::String(name.to_string());
        true
    }

    pub fn delete_project(&mut self, project_id: &str) -> bool {
        let Some(projects) = self
            .document
            .get_mut("projects")
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        projects.remove(project_id).is_some()
    }

    pub fn remove_show_at(&mut self, project_id: &str, index: usize) -> bool {
        let Some(shows) = self.project_shows_mut(project_id) else {
            return false;
        };
        if index >= shows.len() {
            return false;
        }
        shows.remove(index);
        true
    }

    fn project_shows_mut(&mut self, project_id: &str) -> Option<&mut Vec<Value>> {
        let project = self
            .document
            .pointer_mut(&format!("/projects/{project_id}"))?
            .as_object_mut()?;
        project
            .entry("shows")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
    }

    /// Inserts or replaces a project document under `id`.
    pub fn put_project(&mut self, id: &str, project: Value) -> bool {
        if id.is_empty() || !project.is_object() {
            return false;
        }
        let projects = self
            .document
            .as_object_mut()
            .and_then(|document| {
                document
                    .entry("projects")
                    .or_insert_with(|| Value::Object(Map::new()));
                document.get_mut("projects")
            })
            .and_then(Value::as_object_mut);
        let Some(projects) = projects else {
            return false;
        };
        projects.insert(id.to_string(), project);
        true
    }

    pub fn add_project(&mut self, id: &str, name: &str) -> bool {
        self.add_project_in(id, name, "/")
    }

    pub fn add_project_in(&mut self, id: &str, name: &str, parent: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let parent = if parent.is_empty() { "/" } else { parent };
        let projects = self
            .document
            .as_object_mut()
            .and_then(|document| {
                document
                    .entry("projects")
                    .or_insert_with(|| Value::Object(Map::new()));
                document.get_mut("projects")
            })
            .and_then(Value::as_object_mut);
        let Some(projects) = projects else {
            return false;
        };
        if projects.contains_key(id) {
            return false;
        }
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        projects.insert(
            id.to_string(),
            serde_json::json!({
                "name": name,
                "created": created,
                "modified": created,
                "parent": parent,
                "shows": []
            }),
        );
        true
    }
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tick = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}{tick:x}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    document: Value,
}

impl Catalog {
    pub fn starter_templates() -> &'static Self {
        static STARTERS: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
        STARTERS.get_or_init(|| {
            Self::parse(include_str!("../assets/starter-templates.json"))
                .expect("the bundled template catalog must be valid")
        })
    }

    /// A library copy wins if a bundled template has been imported or customized.
    pub fn get_template(&self, id: &str) -> Option<&Value> {
        self.get(id).or_else(|| Self::starter_templates().get(id))
    }

    pub fn save(&self, path: &Path) -> Result<(), DataError> {
        atomic_write(path, self.to_json()?.as_bytes())
    }

    pub fn empty() -> Self {
        Self {
            document: Value::Object(Map::new()),
        }
    }

    pub fn load(path: &Path) -> Result<Self, DataError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn parse(text: &str) -> Result<Self, DataError> {
        let document: Value = serde_json::from_str(text)?;
        if !document.is_object() {
            return Err(DataError::ShowShape);
        }
        Ok(Self { document })
    }

    pub fn names(&self) -> Vec<String> {
        let Some(entries) = self.document.as_object() else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .values()
            .filter_map(|value| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.document.get(id)
    }

    pub fn insert(&mut self, id: &str, value: Value) {
        if let Some(document) = self.document.as_object_mut() {
            document.insert(id.to_string(), value);
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.document
            .as_object_mut()
            .is_some_and(|document| document.remove(id).is_some())
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        self.document
            .as_object()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, value)| (id.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Named entries in display order, with stable IDs when names match.
    pub fn named_entries(&self) -> Vec<(&str, &str, &Value)> {
        let Some(entries) = self.document.as_object() else {
            return Vec::new();
        };
        let mut named = entries
            .iter()
            .filter_map(|(id, entry)| Some((id.as_str(), entry.get("name")?.as_str()?, entry)))
            .collect::<Vec<_>>();
        named.sort_by_key(|(id, name, _)| (*name, *id));
        named
    }

    pub fn id_named(&self, name: &str) -> Option<String> {
        let entries = self.document.as_object()?;
        entries.iter().find_map(|(id, value)| {
            let entry_name = value.get("name").and_then(Value::as_str)?;
            (entry_name == name).then(|| id.clone())
        })
    }

    pub fn to_json(&self) -> Result<String, DataError> {
        Ok(serde_json::to_string(&self.document)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerseHit {
    pub reference: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlinedBook {
    pub number: u64,
    pub name: String,
    pub chapters: Vec<OutlinedChapter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlinedChapter {
    pub number: u64,
    pub verses: Vec<OutlinedVerse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlinedVerse {
    pub number: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BibleFile {
    pub id: String,
    document: Value,
}

impl BibleFile {
    pub fn parse(text: &str) -> Result<Self, DataError> {
        let value: Value = serde_json::from_str(text)?;
        let Some(pair) = value.as_array() else {
            return Err(DataError::ShowShape);
        };
        if pair.len() != 2 {
            return Err(DataError::ShowShape);
        }
        let Some(id) = pair[0].as_str() else {
            return Err(DataError::ShowShape);
        };
        if !pair[1].is_object() {
            return Err(DataError::ShowShape);
        }
        Ok(Self {
            id: id.to_string(),
            document: pair[1].clone(),
        })
    }

    pub fn load(path: &Path) -> Result<Self, DataError> {
        Self::parse(&fs::read_to_string(path)?)
    }

    pub fn name(&self) -> String {
        self.document
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&self.id)
            .to_string()
    }

    pub fn to_json(&self) -> Result<String, DataError> {
        Ok(serde_json::to_string(&Value::Array(vec![
            Value::String(self.id.clone()),
            self.document.clone(),
        ]))?)
    }

    pub fn from_document(id: impl Into<String>, document: Value) -> Self {
        Self {
            id: id.into(),
            document,
        }
    }

    pub fn search(&self, query: &str) -> Vec<VerseHit> {
        if let Some(hits) = scripture::reference_hits(&self.document, query) {
            return hits;
        }
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let Some(books) = self.document.get("books").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for book in books {
            let book_name = book.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(chapters) = book.get("chapters").and_then(Value::as_array) else {
                continue;
            };
            for chapter in chapters {
                let chapter_number = json_u64(chapter.get("number")).unwrap_or(0);
                let Some(verses) = chapter.get("verses").and_then(Value::as_array) else {
                    continue;
                };
                for verse in verses {
                    let text = verse.get("text").and_then(Value::as_str).unwrap_or("");
                    if !text.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let verse_number = json_u64(verse.get("number")).unwrap_or(0);
                    hits.push(VerseHit {
                        reference: format!("{book_name} {chapter_number}:{verse_number}"),
                        text: text.to_string(),
                    });
                    if hits.len() == 40 {
                        return hits;
                    }
                }
            }
        }
        hits
    }

    /// Books, chapters, and verse text in FreeShow order, for the phone remote.
    pub fn outline(&self) -> Vec<OutlinedBook> {
        let Some(books) = self.document.get("books").and_then(Value::as_array) else {
            return Vec::new();
        };
        books
            .iter()
            .filter_map(|book| {
                let name = book
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    return None;
                }
                let chapters = book
                    .get("chapters")
                    .and_then(Value::as_array)
                    .map(|chapters| {
                        chapters
                            .iter()
                            .map(|chapter| OutlinedChapter {
                                number: json_u64(chapter.get("number")).unwrap_or(0),
                                verses: chapter
                                    .get("verses")
                                    .and_then(Value::as_array)
                                    .map(|verses| {
                                        verses
                                            .iter()
                                            .filter_map(|verse| {
                                                let text = verse
                                                    .get("text")
                                                    .and_then(Value::as_str)?
                                                    .to_string();
                                                Some(OutlinedVerse {
                                                    number: json_u64(verse.get("number"))
                                                        .unwrap_or(0),
                                                    text,
                                                })
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(OutlinedBook {
                    number: json_u64(book.get("number")).unwrap_or(0),
                    name,
                    chapters,
                })
            })
            .collect()
    }
}

fn valid_icecast_host(host: &str) -> bool {
    !host.is_empty()
        && !host.contains(['/', '@', '?', '#', '\\'])
        && !host.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn valid_icecast_mount(mount: &str) -> bool {
    !mount.is_empty()
        && !mount.contains(['?', '#'])
        && !mount
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
}

#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn shows_dir(&self) -> PathBuf {
        self.root.join(SHOWS_DIR)
    }

    pub fn projects_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(PROJECTS_FILE)
    }

    pub fn templates_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(TEMPLATES_FILE)
    }

    pub fn overlays_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(OVERLAYS_FILE)
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(EVENTS_FILE)
    }

    pub fn synced_settings_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(SYNCED_SETTINGS_FILE)
    }

    pub fn bibles_dir(&self) -> PathBuf {
        self.root.join(BIBLES_DIR)
    }

    pub fn media_dir(&self) -> PathBuf {
        self.root.join(MEDIA_DIR)
    }

    pub fn audio_dir(&self) -> PathBuf {
        self.root.join(AUDIO_DIR)
    }

    pub fn list_shows(&self) -> Result<Vec<ShowFile>, DataError> {
        let dir = self.shows_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut shows = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("show") {
                continue;
            }
            shows.push(ShowFile::load(&path)?);
        }
        shows.sort_by_key(|a| a.name());
        Ok(shows)
    }

    pub fn load_projects(&self) -> Result<ProjectsFile, DataError> {
        ProjectsFile::load(&self.projects_path())
    }

    pub fn load_templates(&self) -> Result<Catalog, DataError> {
        Catalog::load(&self.templates_path())
    }

    fn template_favorites_path(&self) -> PathBuf {
        self.root
            .join(CONFIG_DIR)
            .join("freeshow-rust-template-favorites.json")
    }

    pub fn load_template_favorites(&self) -> Result<BTreeSet<String>, DataError> {
        match fs::read_to_string(self.template_favorites_path()) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save_template_favorites(&self, ids: &BTreeSet<String>) -> Result<(), DataError> {
        atomic_write(
            &self.template_favorites_path(),
            &serde_json::to_vec_pretty(ids)?,
        )
    }

    pub fn load_overlays(&self) -> Result<Catalog, DataError> {
        Catalog::load(&self.overlays_path())
    }

    pub fn stage_path(&self) -> PathBuf {
        self.root.join(CONFIG_DIR).join(STAGE_FILE)
    }

    pub fn load_stage_layout(&self) -> StageLayout {
        self.load_stage_layout_with_id().1
    }

    /// The active stage layout and its map key when stored by id.
    pub fn load_stage_layout_with_id(&self) -> (Option<String>, StageLayout) {
        if let Some(id) = self.assigned_stage_id()
            && let Some(layout) = self.stage_layout(&id)
        {
            return (Some(id), layout);
        }
        stage_layout::load_stage_with_id(&self.stage_path())
    }

    pub fn save_stage_layout(&self, id: Option<&str>, layout: &StageLayout) -> std::io::Result<()> {
        stage_layout::write_stage_layout_at(&self.stage_path(), id, layout)
    }

    pub fn create_stage_layout(&self, layout: &StageLayout) -> std::io::Result<String> {
        stage_layout::create_stage_layout(&self.stage_path(), layout)
    }

    pub fn duplicate_stage_layout(
        &self,
        source: Option<&str>,
        layout: &StageLayout,
    ) -> std::io::Result<String> {
        stage_layout::duplicate_stage_layout(&self.stage_path(), source, layout)
    }

    /// One layout from `Config/stage.json`.
    pub fn stage_layout(&self, id: &str) -> Option<stage_layout::StageLayout> {
        let text = fs::read_to_string(self.stage_path()).ok()?;
        stage_layout::stage_by_id(&text, id)
    }

    /// Enabled saved stage layouts as (id, name).
    pub fn stage_layout_choices(&self) -> Vec<(String, String)> {
        let Some(Value::Object(layouts)) = fs::read_to_string(self.stage_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        else {
            return Vec::new();
        };
        if layouts.get("items").is_some_and(Value::is_object) {
            return Vec::new();
        }
        layouts
            .into_iter()
            .filter(|(_, layout)| {
                layout.get("items").is_some_and(Value::is_object)
                    && layout["disabled"].as_bool() != Some(true)
            })
            .map(|(id, layout)| {
                let name = layout["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&id);
                (id.clone(), name.to_owned())
            })
            .collect()
    }

    /// The layout id stored on the first stage output, or for the native stage alone.
    pub fn assigned_stage_id(&self) -> Option<String> {
        let text = fs::read_to_string(self.settings_path()).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        json.get("outputs")
            .and_then(Value::as_object)
            .and_then(|outputs| {
                outputs.values().find_map(|output| {
                    output
                        .get("stageOutput")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                })
            })
            .or_else(|| {
                json["nativeStageLayout"]
                    .as_str()
                    .filter(|id| !id.is_empty())
            })
            .map(str::to_owned)
    }

    /// Points stage outputs at this layout. An output id limits the change to that window.
    pub fn assign_stage_layout(&self, output_id: Option<&str>, layout_id: &str) -> bool {
        if layout_id.is_empty() || self.stage_layout(layout_id).is_none() {
            return false;
        }
        let path = self.settings_path();
        let mut json = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value @ Value::Object(_)) => value,
                _ => return false,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(_) => return false,
        };
        let mut assigned = false;
        if let Some(outputs) = json.get_mut("outputs").and_then(Value::as_object_mut) {
            for (key, output) in outputs {
                if output_id.is_none_or(|wanted| wanted == key.as_str())
                    && output["stageOutput"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty())
                {
                    output["stageOutput"] = Value::String(layout_id.into());
                    assigned = true;
                }
            }
        }
        if !assigned {
            if output_id.is_some() {
                return false;
            }
            json["nativeStageLayout"] = Value::String(layout_id.into());
        }
        atomic_write(&path, json.to_string().as_bytes()).is_ok()
    }

    pub fn load_events(&self) -> Result<Catalog, DataError> {
        Catalog::load(&self.events_path())
    }

    pub fn load_synced_settings(&self) -> Result<Catalog, DataError> {
        Catalog::load(&self.synced_settings_path())
    }

    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Reads the FreeShow preferences this app honors from the library settings.
    ///
    /// A missing file, malformed JSON, or an unexpected value is ignored.
    pub fn preferences(&self) -> FreeShowPreferences {
        let json: Value = fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let theme = json
            .get("theme")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|theme| !theme.is_empty())
            .map(str::to_string);
        let remote_password = json
            .pointer("/serverData/remote/password")
            .and_then(|value| match value {
                Value::String(password) => Some(password.clone()),
                Value::Number(password) => Some(password.to_string()),
                _ => None,
            })
            .map(|password| password.trim().to_string())
            .filter(|password| !password.is_empty());
        let port = |name: &str| {
            json.pointer(&format!("/ports/{name}"))
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
        };
        let flag = |name: &str| json.get(name).and_then(Value::as_bool);
        let text_setting = |name: &str| {
            json.get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let synced: Value = fs::read_to_string(self.synced_settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        FreeShowPreferences {
            theme,
            themes: fs::read_to_string(self.root.join(CONFIG_DIR).join("themes.json"))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default(),
            remote_password,
            remote_port: port("remote"),
            stage_port: port("stage"),
            language: text_setting("language"),
            time_format: text_setting("timeFormat").filter(|value| value == "12" || value == "24"),
            autosave: text_setting("autosave").filter(|value| {
                value.eq_ignore_ascii_case("never")
                    || value
                        .strip_suffix("min")
                        .or_else(|| value.strip_suffix("m"))
                        .and_then(|minutes| minutes.parse::<u64>().ok())
                        .is_some_and(|minutes| minutes > 0 && minutes <= 24 * 60)
            }),
            labels_disabled: flag("labelsDisabled"),
            group_numbers: flag("groupNumbers"),
            format_new_show: flag("formatNewShow"),
            auto_output: flag("autoOutput"),
            max_connections: json
                .get("maxConnections")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value <= 10_000),
            outputs: output_preferences(&json),
            out_locked: flag("outLocked"),
            split_lines: json
                .get("splitLines")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
            full_colors: flag("fullColors"),
            alert_updates: flag("alertUpdates"),
            custom_fonts: stored_value(&json, "customFonts"),
            locked_overlays: stored_value(&json, "lockedOverlays"),
            active_scenes: stored_value(&json, "activeScenes"),
            resized: stored_value(&json, "resized"),
            slides_options: stored_value(&json, "slidesOptions"),
            drawer: stored_value(&json, "drawer"),
            metronome: stored_value(&json, "metronome"),
            audio_effects: stored_value(&json, "audioEffects"),
            effects_library: stored_value(&json, "effectsLibrary"),
            timeline: stored_value(&json, "timeline"),
            timecode: stored_value(&json, "timecode"),
            cloud_sync_data: stored_value(&json, "cloudSyncData"),
            drive_data: stored_value(&json, "driveData"),
            content_provider_data: stored_value(&json, "contentProviderData"),
            obs_data: stored_value(&json, "obsData"),
            ai: stored_value(&json, "ai"),
            shortcuts: stored_value(&json, "shortcuts"),
            special: stored_value(&json, "special"),
            draw_settings: stored_value(&synced, "drawSettings"),
            profiles: stored_value(&synced, "profiles"),
            scenes: stored_value(&synced, "scenes"),
            interactions: stored_value(&synced, "interactions"),
        }
    }

    /// FreeShow's general toggle. Missing settings keep the default, which advances.
    pub fn next_item_on_last_slide(&self) -> bool {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return true;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return true;
        };
        json.pointer("/special/nextItemOnLastSlide")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn text_transition(&self) -> TextTransition {
        self.stored_transition("/transitionData/text", 500)
    }

    /// The media transition. FreeShow's default is a fade of 800ms.
    pub fn media_transition(&self) -> TextTransition {
        self.stored_transition("/transitionData/media", 800)
    }

    /// Id and name of the active audience output. Missing settings use `default`.
    pub fn audience_output(&self) -> (String, String) {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return ("default".into(), String::new());
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return ("default".into(), String::new());
        };
        let Some(outputs) = json.get("outputs").and_then(Value::as_object) else {
            return ("default".into(), String::new());
        };
        let Some(key) = active_output_key(outputs) else {
            return (
                available_output_id(outputs.keys().map(String::as_str)),
                String::new(),
            );
        };
        let name = outputs
            .get(&key)
            .and_then(|output| output.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        (key, name)
    }

    /// The address stored for an audio stream id in `settings_synced.json`.
    pub fn audio_stream_value(&self, id: &str) -> Option<String> {
        if id.is_empty() {
            return None;
        }
        let text = fs::read_to_string(self.synced_settings_path()).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        json.pointer(&format!("/audioStreams/{id}/value"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    /// Style id of the active output. FreeShow's primary output uses `default`.
    pub fn output_style_id(&self) -> String {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return "default".into();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return "default".into();
        };
        let Some(outputs) = json.get("outputs").and_then(Value::as_object) else {
            return "default".into();
        };
        active_output_key(outputs)
            .and_then(|key| outputs.get(&key))
            .and_then(|output| output["style"].as_str())
            .filter(|style| !style.is_empty())
            .unwrap_or("default")
            .to_string()
    }

    /// Named values from `settings_synced.json`. Each value is returned under
    /// its raw store id and under its lower-case name with spaces as
    /// underscores, so both FreeShow tokens and stage items can find it.
    /// Variables an operator can pick, as `(id, name)` in FreeShow's order of ids.
    pub fn variable_names(&self) -> Vec<(String, String)> {
        let Ok(text) = fs::read_to_string(self.synced_settings_path()) else {
            return Vec::new();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let Some(map) = json.get("variables").and_then(Value::as_object) else {
            return Vec::new();
        };
        map.iter()
            .filter_map(|(id, variable)| {
                let name = variable.get("name").and_then(Value::as_str)?.trim();
                (!name.is_empty()).then(|| (id.clone(), name.to_string()))
            })
            .collect()
    }

    pub fn variables(&self) -> Vec<(String, String)> {
        let Ok(text) = fs::read_to_string(self.synced_settings_path()) else {
            return Vec::new();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let Some(map) = json.get("variables").and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut pairs = Vec::new();
        for (raw_id, variable) in map {
            let Some(name) = variable.get("name").and_then(Value::as_str) else {
                continue;
            };
            let id = variable_name_id(name);
            if id.is_empty() {
                continue;
            }
            let value = variable_text(variable);
            pairs.push((raw_id.clone(), value.clone()));
            if id != *raw_id {
                pairs.push((id, value));
            }
        }
        pairs
    }

    /// The saved audio list with this id.
    pub fn playlist(&self, id: &str) -> Option<AudioList> {
        let json = self.synced_json()?;
        let entry = json.pointer(&format!("/audioPlaylists/{id}"))?;
        audio_list(entry)
    }

    /// The id of the playlist whose name matches, then the first name that contains the query.
    pub fn playlist_id_named(&self, name: &str) -> Option<String> {
        let query = variable_name_id(name);
        if query.is_empty() {
            return None;
        }
        let json = self.synced_json()?;
        let map = json.get("audioPlaylists")?.as_object()?;
        let mut exact = None;
        let mut contains = None;
        for (id, entry) in map {
            let stored = variable_name_id(entry.get("name").and_then(Value::as_str).unwrap_or(""));
            if stored == query && exact.is_none() {
                exact = Some(id.clone());
            }
            if contains.is_none() && stored.contains(&query) {
                contains = Some(id.clone());
            }
        }
        exact.or(contains)
    }

    fn synced_json(&self) -> Option<Value> {
        let text = fs::read_to_string(self.synced_settings_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Increments, sets, or turns a variable from a slide action. Other fields stay.
    pub fn apply_variable(&self, action: &Value) -> bool {
        let path = self.synced_settings_path();
        let Ok(text) = fs::read_to_string(&path) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        let Some(key) = variable_key(
            &json,
            action.get("id").and_then(Value::as_str),
            action.get("name").and_then(Value::as_str),
            action.get("index").and_then(json_usize),
        ) else {
            return false;
        };
        let mut json = json;
        let Some(entry) = json
            .pointer_mut(&format!("/variables/{key}"))
            .and_then(Value::as_object_mut)
        else {
            return false;
        };
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string();
        let command = action
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .or_else(|| action.get("variableAction").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let value = action.get("value");
        if kind == "number"
            || command == "increment"
            || command == "decrement"
            || command == "number"
        {
            let current = entry
                .get("number")
                .and_then(json_f64)
                .or_else(|| entry.get("default").and_then(json_f64))
                .unwrap_or(0.0);
            let step = entry
                .get("step")
                .and_then(json_f64)
                .filter(|step| *step != 0.0)
                .unwrap_or(1.0);
            let given = value.and_then(json_f64).filter(|number| *number != 0.0);
            let next = if command == "increment" {
                current + given.unwrap_or(step)
            } else if command == "decrement" {
                current - given.unwrap_or(step)
            } else {
                given.unwrap_or_else(|| entry.get("default").and_then(json_f64).unwrap_or(0.0))
            };
            let stored = if next.fract() == 0.0 {
                serde_json::json!(next as i64)
            } else {
                serde_json::json!(next)
            };
            entry.insert("number".to_string(), stored);
        } else if command == "text"
            || value.and_then(Value::as_str).is_some() && command != "enabled"
        {
            let text = value.and_then(Value::as_str).unwrap_or("");
            entry.insert("text".to_string(), Value::String(text.to_string()));
        } else {
            let enabled = value
                .and_then(Value::as_bool)
                .or_else(|| value.and_then(Value::as_str).map(|flag| flag != "false"))
                .unwrap_or_else(|| entry.get("enabled").and_then(Value::as_bool) != Some(true));
            entry.insert("enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, json.to_string()).is_ok()
    }

    /// Editor text of stored variables as `name=value` lines.
    /// One `name=value` line per stored variable, without the lowercase id aliases.
    pub fn variable_editor_text(&self) -> String {
        let Some(json) = self.synced_json() else {
            return String::new();
        };
        let Some(map) = json.get("variables").and_then(Value::as_object) else {
            return String::new();
        };
        map.values()
            .filter_map(|variable| {
                let name = variable.get("name").and_then(Value::as_str)?.trim();
                (!variable_name_id(name).is_empty())
                    .then(|| format!("{name}={}", variable_text(variable)))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Creates or updates variables from `name=value` editor lines.
    pub fn save_variables_text(&self, text: &str) -> bool {
        let path = self.synced_settings_path();
        let mut json = fs::read_to_string(&path)
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !json.is_object() {
            json = Value::Object(Map::new());
        }
        let variables = json
            .as_object_mut()
            .unwrap()
            .entry("variables")
            .or_insert_with(|| Value::Object(Map::new()));
        if !variables.is_object() {
            *variables = Value::Object(Map::new());
        }
        let map = variables.as_object_mut().unwrap();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let value = value.trim();
            let id = variable_name_id(name);
            let key = map
                .iter()
                .find_map(|(existing, entry)| {
                    let stored = entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(existing);
                    (stored == name || existing == &id || variable_name_id(stored) == id)
                        .then(|| existing.clone())
                })
                .unwrap_or(id);
            let entry = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
            if let Some(entry) = entry.as_object_mut() {
                entry
                    .entry("name")
                    .or_insert_with(|| Value::String(name.to_string()));
                entry.insert("text".to_string(), Value::String(value.to_string()));
            }
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, json.to_string()).is_ok()
    }

    /// Edits `settings_synced.json` in place, keeping every other key. A file that
    /// exists but does not parse is left alone (returns false).
    pub fn edit_synced_settings(&self, edit: impl FnOnce(&mut Map<String, Value>)) -> bool {
        let path = self.synced_settings_path();
        let mut json = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) if value.is_object() => value,
                _ => return false,
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
            Err(_) => return false,
        };
        edit(json.as_object_mut().expect("synced settings object"));
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        atomic_write(&path, json.to_string().as_bytes()).is_ok()
    }

    /// Player-tab videos from `settings_synced.json`.
    pub fn player_videos(&self) -> Vec<PlayerVideo> {
        self.synced_json()
            .and_then(|json| json.get("playerVideos").cloned())
            .map(|value| player_videos_from(&value))
            .unwrap_or_default()
    }

    /// Volume and mute of the main audio channel. Missing settings are full volume, unmuted.
    pub fn main_audio(&self) -> (f32, bool) {
        self.channel_audio("main")
    }

    pub fn channel_audio(&self, id: &str) -> (f32, bool) {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return (1.0, false);
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (1.0, false);
        };
        let channel = json.get("audioChannelsData").and_then(|data| data.get(id));
        let volume = channel
            .and_then(|channel| channel.get("volume"))
            .and_then(json_f64)
            .unwrap_or(1.0) as f32;
        let muted = channel
            .and_then(|channel| channel.get("isMuted"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        (volume, muted)
    }

    pub fn audio_routes(&self, sources: &[&str], main_gain: f32) -> Vec<AudioRoute> {
        let settings = fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        audio_routes(&settings, sources, main_gain)
    }

    pub fn network_audio_routes(
        &self,
        sources: &[&str],
        main_gain: f32,
    ) -> Vec<freeshow_core::NetworkRoute> {
        let settings = fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        network_audio_routes(&settings, sources, main_gain)
    }

    /// Icecast uses a standalone mixer node instead of a video output.
    pub fn icecast(&self) -> Option<(freeshow_core::IcecastDestination, bool)> {
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(self.settings_path()).ok()?).ok()?;
        let saved_route = audio_routing::has_icecast_route(&settings);
        if !saved_route && !audio_routing::uses_icecast_main_mix(&settings) {
            return None;
        }
        let value = &settings["special"]["icecast"];
        let host = value["host"]
            .as_str()
            .filter(|host| !host.is_empty())
            .unwrap_or("localhost");
        let mount = value["mount"]
            .as_str()
            .filter(|mount| !mount.is_empty())
            .unwrap_or("/stream.opus");
        let password = value["password"].as_str().unwrap_or("hackme");
        let tls = match value.get("tls") {
            Some(value) => Some(value.as_bool()?),
            None => None,
        };
        let port = if value["port"].is_null() {
            8000
        } else {
            u16::try_from(json_u64(value.get("port"))?)
                .ok()
                .filter(|port| *port != 0)?
        };
        if !valid_icecast_host(host) || !valid_icecast_mount(mount) || password.contains('\0') {
            return None;
        }
        let mount = if mount.starts_with('/') {
            mount.to_owned()
        } else {
            format!("/{mount}")
        };
        let enabled = match value["enabled"].as_bool() {
            Some(enabled) => enabled,
            None => saved_route,
        };
        Some((
            freeshow_core::IcecastDestination {
                host: host.to_owned(),
                port,
                mount,
                password: password.to_owned(),
                tls,
            },
            enabled,
        ))
    }

    pub fn set_icecast_enabled(&self, enabled: bool) -> Result<(), DataError> {
        self.patch_icecast(false, |icecast, _| {
            icecast.insert("enabled".into(), Value::Bool(enabled));
        })
    }

    pub fn save_icecast_connection(
        &self,
        host: &str,
        port: u16,
        mount: &str,
        password: Option<&str>,
        tls: Option<bool>,
        main_mix: bool,
    ) -> Result<(), DataError> {
        let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        if !valid_icecast_host(host) {
            return Err(invalid("Enter an Icecast hostname or IP address.").into());
        }
        if port == 0 {
            return Err(invalid("Enter an Icecast port from 1 to 65535.").into());
        }
        if !valid_icecast_mount(mount) {
            return Err(invalid("Enter a mount without spaces, ? or #.").into());
        }
        if password.is_some_and(|password| password.contains('\0')) {
            return Err(invalid("The Icecast password contains an invalid character.").into());
        }
        self.patch_icecast(true, |icecast, saved_route| {
            use serde_json::json;
            icecast.insert("host".into(), json!(host));
            icecast.insert("port".into(), json!(port));
            let mount = if mount.starts_with('/') {
                mount.to_owned()
            } else {
                format!("/{mount}")
            };
            icecast.insert("mount".into(), json!(mount));
            if main_mix {
                if !saved_route && icecast.get("mainMix").and_then(Value::as_bool) != Some(true) {
                    icecast.insert("enabled".into(), json!(false));
                }
                icecast.insert("mainMix".into(), json!(true));
            } else {
                icecast.remove("mainMix");
            }
            if let Some(password) = password {
                icecast.insert("password".into(), json!(password));
            }
            if let Some(tls) = tls {
                icecast.insert("tls".into(), json!(tls));
            } else {
                icecast.remove("tls");
            }
        })
    }

    fn patch_icecast(
        &self,
        create: bool,
        edit: impl FnOnce(&mut serde_json::Map<String, Value>, bool),
    ) -> Result<(), DataError> {
        let path = self.settings_path();
        let mut settings: Value = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                serde_json::json!({})
            }
            Err(error) => return Err(error.into()),
        };
        let saved_route = audio_routing::has_icecast_route(&settings);
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Icecast settings must be stored in JSON objects.",
            )
        };
        let root = settings.as_object_mut().ok_or_else(invalid)?;
        let special = root
            .entry("special")
            .or_insert_with(|| serde_json::json!({}));
        let special = special.as_object_mut().ok_or_else(invalid)?;
        let icecast = special
            .entry("icecast")
            .or_insert_with(|| serde_json::json!({}));
        let icecast = icecast.as_object_mut().ok_or_else(invalid)?;
        edit(icecast, saved_route);
        atomic_write(&path, &serde_json::to_vec_pretty(&settings)?)
    }

    /// First enabled audience output of this transport.
    pub fn network_output(&self, kind: &str) -> Option<String> {
        self.network_output_settings(kind).map(|(id, _)| id)
    }

    pub fn network_output_settings(&self, kind: &str) -> Option<(String, Value)> {
        self.network_outputs(kind)
            .into_iter()
            .find(|(_, output)| output["stageOutput"].as_str().is_none_or(str::is_empty))
    }

    /// Every enabled output of this transport, including stage pictures.
    pub fn network_outputs(&self, kind: &str) -> Vec<(String, Value)> {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return Vec::new();
        };
        let Ok(settings) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        settings["outputs"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, output)| {
                output["enabled"].as_bool() != Some(false) && output[kind].as_bool() == Some(true)
            })
            .map(|(id, body)| (id.clone(), body.clone()))
            .collect()
    }
    pub fn audio_mixer(&self) -> Value {
        let settings: Value = fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let mut routing = settings["audioRouting"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut channels = routing
            .get("channels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !channels
            .iter()
            .any(|channel| channel["id"].as_str() == Some("main"))
        {
            channels.insert(0, serde_json::json!({"id":"main", "name":"Main"}));
        }
        routing.insert("channels".into(), Value::Array(channels));
        serde_json::json!({
            "audioRouting": routing,
            "audioChannelsData": settings["audioChannelsData"].as_object().cloned().unwrap_or_default()
        })
    }

    pub fn channel_muted(&self, channel: &str) -> bool {
        if channel.is_empty() {
            return false;
        }
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        json.pointer(&format!("/audioChannelsData/{channel}/isMuted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Stores a channel volume in `settings.json` without dropping mute or other keys.
    pub fn set_channel_volume(&self, channel: &str, volume: f32) -> bool {
        self.edit_channel(channel, |entry| {
            entry.insert("volume".into(), serde_json::json!(volume));
        })
    }

    /// Stores whether a channel is muted.
    pub fn set_channel_muted(&self, channel: &str, muted: bool) -> bool {
        self.edit_channel(channel, |entry| {
            entry.insert("isMuted".into(), Value::Bool(muted));
        })
    }

    fn edit_channel(&self, channel: &str, edit: impl FnOnce(&mut Map<String, Value>)) -> bool {
        if channel.is_empty() {
            return false;
        }
        let path = self.settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut json = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(json) if json.is_object() => json,
                _ => return false,
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
            Err(_) => return false,
        };
        let root = json.as_object_mut().expect("settings object");
        let channels = root
            .entry("audioChannelsData")
            .or_insert_with(|| Value::Object(Map::new()));
        if !channels.is_object() {
            *channels = Value::Object(Map::new());
        }
        let channel_value = channels
            .as_object_mut()
            .expect("channels")
            .entry(channel.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !channel_value.is_object() {
            *channel_value = Value::Object(Map::new());
        }
        edit(channel_value.as_object_mut().expect("channel"));
        atomic_write(&path, json.to_string().as_bytes()).is_ok()
    }

    /// Replaces the text or media transition in `settings.json`.
    /// Empty `direction` keeps a stored `custom.direction`, or defaults to `left_right`.
    pub fn set_transition(
        &self,
        slot: &str,
        kind: &str,
        duration_ms: u64,
        easing: &str,
        direction: &str,
    ) -> bool {
        if slot.is_empty() {
            return false;
        }
        let path = self.settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut json = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let root = json.as_object_mut().expect("settings object");
        let data = root
            .entry("transitionData")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !data.is_object() {
            *data = Value::Object(serde_json::Map::new());
        }
        let kept = data
            .pointer(&format!("/{slot}/custom/direction"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("left_right")
            .to_string();
        let direction = if direction.trim().is_empty() {
            kept
        } else {
            direction.trim().to_string()
        };
        data.as_object_mut().expect("transition data").insert(
            slot.to_string(),
            serde_json::json!({
                "type": kind,
                "duration": duration_ms,
                "easing": easing,
                "custom": { "direction": direction }
            }),
        );
        fs::write(path, json.to_string()).is_ok()
    }

    /// Stores `style_id` on the active output in `settings.json`.
    pub fn set_output_style(&self, style_id: &str) -> bool {
        let path = self.settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut json = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let outputs = json
            .as_object_mut()
            .expect("settings object")
            .entry("outputs")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !outputs.is_object() {
            *outputs = Value::Object(serde_json::Map::new());
        }
        let outputs = outputs.as_object_mut().expect("outputs object");
        let key = active_output_key(outputs)
            .unwrap_or_else(|| available_output_id(outputs.keys().map(String::as_str)));
        let output = outputs
            .entry(key)
            .or_insert_with(|| serde_json::json!({"name":"Primary","active":true,"enabled":true}));
        if !output.is_object() {
            *output = Value::Object(serde_json::Map::new());
        }
        output
            .as_object_mut()
            .expect("output object")
            .insert("style".into(), Value::String(style_id.to_string()));
        fs::write(path, json.to_string()).is_ok()
    }

    /// Fit named on the active output style. FreeShow uses contain when it is missing.
    pub fn style_fit(&self) -> String {
        let id = self.output_style_id();
        let Ok(text) = fs::read_to_string(self.synced_settings_path()) else {
            return "contain".into();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return "contain".into();
        };
        json.pointer(&format!("/styles/{id}/fit"))
            .and_then(Value::as_str)
            .filter(|fit| !fit.is_empty())
            .unwrap_or("contain")
            .to_string()
    }

    /// Blur radius and opacity for `fit: "blur"`. A missing radius is 6. A missing or zero opacity is 0.3.
    pub fn style_blur(&self) -> (f32, f32) {
        let id = self.output_style_id();
        let Ok(text) = fs::read_to_string(self.synced_settings_path()) else {
            return (6.0, 0.3);
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (6.0, 0.3);
        };
        let style = json.pointer(&format!("/styles/{id}"));
        let amount = style
            .and_then(|entry| entry.get("blurAmount"))
            .and_then(Value::as_f64)
            .or_else(|| {
                style
                    .and_then(|entry| entry.get("blurAmount"))
                    .and_then(Value::as_u64)
                    .map(|value| value as f64)
            });
        let opacity = style
            .and_then(|entry| entry.get("blurOpacity"))
            .and_then(Value::as_f64)
            .or_else(|| {
                style
                    .and_then(|entry| entry.get("blurOpacity"))
                    .and_then(Value::as_u64)
                    .map(|value| value as f64)
            });
        let amount = amount.filter(|value| *value >= 0.0).unwrap_or(6.0) as f32;
        let opacity = match opacity {
            Some(value) if value > 0.0 => (value as f32).min(1.0),
            _ => 0.3,
        };
        (amount, opacity)
    }

    pub fn media_store_path(&self) -> PathBuf {
        self.root.join("media.json")
    }

    /// Fit and flips stored for one file. An empty fit means the style fit still applies.
    pub fn media_look(&self, path: &Path) -> (String, bool, bool) {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return (String::new(), false, false);
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (String::new(), false, false);
        };
        let Some(entry) = json.get(path.to_string_lossy().as_ref()) else {
            return (String::new(), false, false);
        };
        (
            entry
                .get("fit")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            entry.get("flipped").and_then(Value::as_bool) == Some(true),
            entry.get("flippedY").and_then(Value::as_bool) == Some(true),
        )
    }

    /// CSS filter and blend stored for one file. Empty strings leave the picture unchanged.
    pub fn media_effect(&self, path: &Path) -> (String, String) {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return (String::new(), String::new());
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (String::new(), String::new());
        };
        let Some(entry) = json.get(path.to_string_lossy().as_ref()) else {
            return (String::new(), String::new());
        };
        let text_at = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        (text_at("filter"), text_at("blend"))
    }

    /// Pixels cut from the left, right, top, and bottom of one still. Missing sides stay at zero.
    pub fn media_crop(&self, path: &Path) -> (u32, u32, u32, u32) {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return (0, 0, 0, 0);
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (0, 0, 0, 0);
        };
        let Some(crop) = json
            .get(path.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("cropping"))
        else {
            return (0, 0, 0, 0);
        };
        (
            crop_pixels(crop, "left"),
            crop_pixels(crop, "right"),
            crop_pixels(crop, "top"),
            crop_pixels(crop, "bottom"),
        )
    }

    /// Per-file audio gain from `media.json`. Missing values stay at full, and the result is clamped to 0..=1.
    pub fn media_volume(&self, path: &Path) -> f32 {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return 1.0;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return 1.0;
        };
        let Some(volume) = json
            .get(path.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("volume"))
        else {
            return 1.0;
        };
        let Some(number) = volume
            .as_f64()
            .or_else(|| volume.as_u64().map(|value| value as f64))
            .or_else(|| volume.as_i64().map(|value| value as f64))
        else {
            return 1.0;
        };
        (number as f32).clamp(0.0, 1.0)
    }

    /// FreeShow repeats one audio file when its `media.json` entry sets `loop` to true.
    pub fn media_loops(&self, path: &Path) -> bool {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        json.get(path.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("loop"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    /// Start and end, in seconds, stored for one file. Zero means the file start or the file end.
    pub fn media_span(&self, path: &Path) -> (f64, f64) {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return (0.0, 0.0);
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return (0.0, 0.0);
        };
        let Some(entry) = json.get(path.to_string_lossy().as_ref()) else {
            return (0.0, 0.0);
        };
        (
            stored_seconds(entry, "fromTime"),
            stored_seconds(entry, "toTime"),
        )
    }

    /// Playback speed stored for one audio file. A missing or non-positive tempo stays at 1.
    pub fn media_tempo(&self, path: &Path) -> f64 {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return 1.0;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return 1.0;
        };
        let Some(entry) = json.get(path.to_string_lossy().as_ref()) else {
            return 1.0;
        };
        let tempo = stored_seconds(entry, "tempo");
        if tempo > 0.0 { tempo } else { 1.0 }
    }

    /// Playback speed stored for one video. A missing, zero, or negative speed stays at 1.
    pub fn media_speed(&self, path: &Path) -> f64 {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return 1.0;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return 1.0;
        };
        let Some(speed) = json
            .get(path.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("speed"))
        else {
            return 1.0;
        };
        let number = speed
            .as_f64()
            .or_else(|| speed.as_str().and_then(|text| text.parse::<f64>().ok()));
        match number {
            Some(value) if value > 0.0 => value,
            _ => 1.0,
        }
    }

    /// `background`, `foreground`, or an empty string when the file does not set one.
    pub fn media_video_type(&self, path: &Path) -> String {
        let Ok(text) = fs::read_to_string(self.media_store_path()) else {
            return String::new();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return String::new();
        };
        json.get(path.to_string_lossy().as_ref())
            .and_then(|entry| entry.get("videoType"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    /// How many lyric lines the active output style shows at once. Zero means the whole slide.
    /// The template id the active output style lays every slide out with, if any.
    pub fn style_template(&self) -> Option<String> {
        let id = self.output_style_id();
        self.style_value(&id)
            .get("template")
            .and_then(Value::as_str)
            .filter(|template| !template.is_empty())
            .map(str::to_string)
    }

    pub fn style_lines(&self) -> u32 {
        let id = self.output_style_id();
        if id.is_empty() {
            return 0;
        }
        let Ok(text) = fs::read_to_string(self.synced_settings_path()) else {
            return 0;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return 0;
        };
        let Some(style) = json.pointer(&format!("/styles/{id}")) else {
            return 0;
        };
        if let Some(layers) = style.get("layers").and_then(Value::as_array)
            && !layers.iter().any(|layer| layer.as_str() == Some("slide"))
        {
            return 0;
        }
        style
            .get("lines")
            .and_then(Value::as_u64)
            .or_else(|| {
                style
                    .get("lines")
                    .and_then(Value::as_f64)
                    .map(|lines| lines as u64)
            })
            .unwrap_or(0) as u32
    }

    fn synced_settings(&self) -> Value {
        fs::read_to_string(self.synced_settings_path())
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
    }

    /// Output styles as `(id, name)`, sorted by name. FreeShow's `default` style ("Primary")
    /// is always listed, even before anything was stored for it.
    pub fn styles(&self) -> Vec<(String, String)> {
        let json = self.synced_settings();
        let mut styles: Vec<(String, String)> = json
            .get("styles")
            .and_then(Value::as_object)
            .map(|styles| {
                styles
                    .iter()
                    .map(|(id, style)| {
                        let name = style
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if id == "default" {
                                    "Primary".into()
                                } else {
                                    id.clone()
                                }
                            });
                        (id.clone(), name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !styles.iter().any(|(id, _)| id == "default") {
            styles.push(("default".into(), "Primary".into()));
        }
        styles.sort_by_key(|style| style.1.to_lowercase());
        styles
    }

    /// The stored JSON of one style, or an empty object.
    pub fn style_value(&self, id: &str) -> Value {
        self.synced_settings()
            .pointer(&format!("/styles/{id}"))
            .filter(|style| style.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
    }

    /// Sets one key on a style in `Config/settings_synced.json`, creating the style if needed.
    /// A `null` value removes the key so FreeShow's default applies again.
    pub fn patch_style(&self, id: &str, key: &str, value: Value) -> bool {
        if id.is_empty() || key.is_empty() {
            return false;
        }
        let path = self.synced_settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut json = self.synced_settings();
        let root = json.as_object_mut().expect("settings object");
        let styles = root
            .entry("styles")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !styles.is_object() {
            *styles = Value::Object(serde_json::Map::new());
        }
        let style = styles
            .as_object_mut()
            .expect("styles object")
            .entry(id.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !style.is_object() {
            *style = Value::Object(serde_json::Map::new());
        }
        let style = style.as_object_mut().expect("style object");
        if value.is_null() {
            style.remove(key);
        } else {
            style.insert(key.to_string(), value);
        }
        fs::write(
            path,
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        )
        .is_ok()
    }

    /// Adds a style named `name` and returns its id.
    pub fn create_style(&self, name: &str) -> Option<String> {
        let name = name.trim();
        let name = if name.is_empty() { "Style" } else { name };
        let taken: Vec<String> = self.styles().into_iter().map(|(id, _)| id).collect();
        let id = (1..)
            .map(|n| format!("style{n}"))
            .find(|id| !taken.contains(id))?;
        self.patch_style(&id, "name", Value::String(name.to_string()))
            .then_some(id)
    }

    /// Removes a style. The `default` style stays, and outputs using the removed one fall back to it.
    pub fn delete_style(&self, id: &str) -> bool {
        if id.is_empty() || id == "default" {
            return false;
        }
        let path = self.synced_settings_path();
        let mut json = self.synced_settings();
        let removed = json
            .get_mut("styles")
            .and_then(Value::as_object_mut)
            .and_then(|styles| styles.remove(id))
            .is_some();
        if !removed
            || fs::write(
                &path,
                serde_json::to_string_pretty(&json).unwrap_or_default(),
            )
            .is_err()
        {
            return false;
        }
        if self.output_style_id() == id {
            let _ = self.set_output_style("default");
        }
        true
    }

    /// Background colour of the active output style, shown wherever nothing else is drawn.
    pub fn style_background(&self) -> Option<String> {
        let id = self.output_style_id();
        self.style_value(&id)
            .get("background")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|color| !color.is_empty())
            .map(str::to_string)
    }

    /// Text or media transition stored on the active output style.
    pub fn style_transition(&self, kind: &str) -> Option<TextTransition> {
        let id = self.output_style_id();
        if id.is_empty() {
            return None;
        }
        let text = fs::read_to_string(self.synced_settings_path()).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        read_transition(json.pointer(&format!("/styles/{id}/transition/{kind}")))
    }

    fn stored_transition(&self, pointer: &str, default_duration: u64) -> TextTransition {
        let Ok(text) = fs::read_to_string(self.settings_path()) else {
            return TextTransition {
                duration_ms: default_duration,
                ..TextTransition::default()
            };
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return TextTransition {
                duration_ms: default_duration,
                ..TextTransition::default()
            };
        };
        let Some(value) = json.pointer(pointer) else {
            return TextTransition {
                duration_ms: default_duration,
                ..TextTransition::default()
            };
        };
        TextTransition {
            kind: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("fade")
                .to_string(),
            direction: value
                .pointer("/custom/direction")
                .and_then(Value::as_str)
                .unwrap_or("left_right")
                .to_string(),
            duration_ms: value
                .get("duration")
                .and_then(Value::as_u64)
                .unwrap_or(default_duration),
            easing: stored_easing(value.get("easing")),
        }
    }

    /// Converts USFM, OSIS, and Zefania files in `Bibles/` into FreeShow `.fsb` files.
    pub fn import_scripture_sources(&self) -> Result<Vec<String>, DataError> {
        scripture::import_sources(&self.bibles_dir())
    }

    pub fn imports_dir(&self) -> PathBuf {
        self.root.join(IMPORTS_DIR)
    }

    /// Converts song files in `Imports/` into `Shows/*.show`.
    pub fn import_show_sources(&self) -> Result<Vec<String>, DataError> {
        import::import_sources_split(
            &self.imports_dir(),
            &self.shows_dir(),
            &self.media_dir(),
            self.preferences().split_lines(),
        )
    }

    pub fn list_bibles(&self) -> Result<Vec<BibleFile>, DataError> {
        let dir = self.bibles_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut bibles = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("fsb") {
                continue;
            }
            bibles.push(BibleFile::load(&path)?);
        }
        bibles.sort_by_key(|a| a.name());
        Ok(bibles)
    }

    pub fn list_media_files(&self, dir: &Path) -> Result<Vec<String>, DataError> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
                && !name.starts_with(".freeshow-copy-")
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }
}

fn output_preferences(settings: &Value) -> Vec<OutputPreference> {
    let Some(outputs) = settings.get("outputs").and_then(Value::as_object) else {
        return Vec::new();
    };
    outputs
        .iter()
        .filter(|(_, output)| output.is_object())
        .map(|(id, output)| OutputPreference {
            id: id.clone(),
            name: output
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            enabled: output.get("enabled").and_then(Value::as_bool) != Some(false),
            active: output.get("active").and_then(Value::as_bool) == Some(true),
            style: output
                .get("style")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string(),
            width: output
                .pointer("/bounds/width")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0),
            height: output
                .pointer("/bounds/height")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0),
            always_on_top: output.get("alwaysOnTop").and_then(Value::as_bool) != Some(false),
            kiosk: output.get("kiosk").and_then(Value::as_bool) == Some(true),
            stage: output
                .get("stageOutput")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
        })
        .collect()
}

/// An unused id for the first audience output in a library.
pub fn available_output_id<'a>(taken: impl Iterator<Item = &'a str> + Clone) -> String {
    if !taken.clone().any(|id| id == "default") {
        return "default".into();
    }
    (2..)
        .map(|index| format!("output{index}"))
        .find(|candidate| !taken.clone().any(|id| id == candidate))
        .expect("an unused output id")
}

fn stored_value(json: &Value, name: &str) -> Value {
    json.get(name)
        .cloned()
        .filter(|value| !value.is_null())
        .unwrap_or(Value::Null)
}

pub fn default_shortcuts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("undo", "Ctrl+Z"),
        ("redo", "Ctrl+Y"),
        ("focus", "Ctrl+Shift+F"),
        ("next", "Right"),
        ("previous", "Left"),
        ("go", "Space"),
        ("kiosk", "F11"),
        ("guide", "Ctrl+F1"),
        ("always_on_top", "Ctrl+Shift+A"),
        ("clear", "Escape"),
    ]
}

fn default_shortcut(action: &str) -> &'static str {
    default_shortcuts()
        .into_iter()
        .find(|(name, _)| *name == action)
        .map(|(_, chord)| chord)
        .unwrap_or("")
}

pub fn normalize_chord(text: &str) -> String {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key = String::new();
    for part in text.split('+') {
        let part = if part == " " { "space" } else { part.trim() };
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "meta" | "super" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            other => key = titled_key(other),
        }
    }
    let mut chord = String::new();
    if ctrl {
        chord.push_str("Ctrl+");
    }
    if alt {
        chord.push_str("Alt+");
    }
    if shift {
        chord.push_str("Shift+");
    }
    chord.push_str(&key);
    chord
}

fn titled_key(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        " " | "space" | "spacebar" => "Space".into(),
        "right" | "arrowright" => "Right".into(),
        "left" | "arrowleft" => "Left".into(),
        "up" | "arrowup" => "Up".into(),
        "down" | "arrowdown" => "Down".into(),
        "esc" | "escape" | "\u{1b}" => "Escape".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        other if other.len() == 1 => other.to_ascii_uppercase(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

fn stored_size(json: &Value, name: &str, min: u32, max: u32) -> Option<u32> {
    let value = json.get(name)?;
    let number = value
        .as_u64()
        .or_else(|| value.as_f64().map(|number| number.round() as u64))?;
    let number = u32::try_from(number).ok()?;
    (min..=max).contains(&number).then_some(number)
}

fn locked_overlay_ids(locked: &Value, output_id: &str) -> Vec<String> {
    if let Some(list) = locked.as_array() {
        return string_ids(list);
    }
    let Some(map) = locked.as_object() else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for key in [output_id, "default"] {
        if key.is_empty() {
            continue;
        }
        if let Some(Value::Array(list)) = map.get(key) {
            for id in string_ids(list) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        if key == output_id && !ids.is_empty() {
            return ids;
        }
    }
    ids
}

fn string_ids(list: &[Value]) -> Vec<String> {
    list.iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

fn scene_spec(id: &str, entry: &Value) -> SceneSpec {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .trim()
        .to_string();
    let content = entry.get("content").unwrap_or(entry);
    let media = content
        .get("media")
        .and_then(media_ref)
        .or_else(|| entry.get("media").and_then(media_ref));
    let action = entry
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    SceneSpec {
        id: id.to_string(),
        name: if name.is_empty() {
            id.to_string()
        } else {
            name
        },
        overlays: string_list(content.get("overlays")),
        hidden_overlays: string_list(content.get("hiddenOverlays")),
        media,
        action,
    }
}

fn scene_id_list(value: Option<&Value>) -> Option<Vec<String>> {
    let value = value?;
    if let Some(list) = value.as_array() {
        let ids = string_ids(list);
        return (!ids.is_empty()).then_some(ids);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| vec![id.to_string()])
}

fn media_ref(value: &Value) -> Option<String> {
    if let Some(path) = value
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        return Some(path.to_string());
    }
    for key in ["path", "id", "name"] {
        if let Some(path) = value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return Some(path.to_string());
        }
    }
    None
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    scene_id_list(value).unwrap_or_default()
}

fn interaction_spec(id: &str, entry: &Value) -> InteractionSpec {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .trim()
        .to_string();
    let action = entry
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut questions = Vec::new();
    if let Some(inputs) = entry.get("inputs").and_then(Value::as_array) {
        for input in inputs {
            if let Some(question) = input
                .get("question")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|question| !question.is_empty())
            {
                questions.push(question.to_string());
            }
        }
    }
    InteractionSpec {
        id: id.to_string(),
        name: if name.is_empty() {
            id.to_string()
        } else {
            name
        },
        questions,
        action,
    }
}

fn parse_draw_strokes(value: Option<&Value>) -> Vec<DrawStroke> {
    let Some(list) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            if entry
                .as_str()
                .is_some_and(|flag| flag.eq_ignore_ascii_case("mouseup"))
            {
                return Some(DrawStroke {
                    x: 0.0,
                    y: 0.0,
                    size: 0.0,
                    color: String::new(),
                    up: true,
                });
            }
            let x = entry.get("x").and_then(json_f32)?;
            let y = entry.get("y").and_then(json_f32)?;
            Some(DrawStroke {
                x,
                y,
                size: entry.get("size").and_then(json_f32).unwrap_or(12.0),
                color: entry
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                up: false,
            })
        })
        .collect()
}

fn json_f32(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|number| number as f32)
        .or_else(|| value.as_i64().map(|number| number as f32))
        .or_else(|| value.as_u64().map(|number| number as f32))
        .or_else(|| value.as_str()?.parse().ok())
}

fn effects_from(value: &Value) -> Vec<EffectSpec> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    map.iter()
        .map(|(id, entry)| {
            let items = entry
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            if item.get("hidden").and_then(Value::as_bool) == Some(true) {
                                return None;
                            }
                            item.get("type")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|kind| !kind.is_empty())
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            EffectSpec {
                id: id.clone(),
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                color: entry
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                background: entry
                    .get("background")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                place_under: entry.get("placeUnderSlide").and_then(Value::as_bool) == Some(true),
                item_types: items,
                display_duration_ms: entry
                    .get("displayDuration")
                    .and_then(Value::as_u64)
                    .map(|secs| secs.saturating_mul(1000))
                    .or_else(|| entry.get("displayDurationMs").and_then(Value::as_u64)),
            }
        })
        .collect()
}

fn audio_effect_names(value: &Value) -> Vec<String> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for (channel, config) in map {
        let Some(stack) = config.get("stack").and_then(Value::as_array) else {
            continue;
        };
        for item in stack {
            if item.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("effect");
            names.push(format!("{channel}:{kind}"));
        }
    }
    names
}

fn audio_effect_gain(value: &Value) -> f32 {
    let Some(map) = value.as_object() else {
        return 1.0;
    };
    let mut gain = 1.0_f32;
    for config in map.values() {
        let Some(stack) = config.get("stack").and_then(Value::as_array) else {
            continue;
        };
        for item in stack {
            if item.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(stored) = item
                .get("config")
                .and_then(|config| {
                    config
                        .get("gain")
                        .or_else(|| config.get("volume"))
                        .or_else(|| config.get("outputGain"))
                })
                .and_then(json_f32)
            {
                gain *= stored;
            }
        }
    }
    gain.clamp(0.0, 4.0)
}

/// Player videos stored in `settings_synced.json`.
pub fn player_videos_from(value: &Value) -> Vec<PlayerVideo> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut videos = Vec::new();
    for (id, entry) in map {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .trim();
        if name.is_empty() {
            continue;
        }
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| entry.get("mediaType").and_then(Value::as_str))
            .unwrap_or("file")
            .to_string();
        let path = entry
            .get("path")
            .or_else(|| entry.get("url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .or_else(|| {
                // FreeShow stores only the YouTube or Vimeo video id.
                let video_id = entry.get("id").and_then(Value::as_str)?.trim();
                player_link(&kind, video_id)
            });
        videos.push(PlayerVideo {
            id: id.clone(),
            name: name.to_string(),
            kind,
            path,
        });
    }
    videos
}

/// The page to play for a FreeShow Player video id.
pub fn player_link(kind: &str, video_id: &str) -> Option<String> {
    if video_id.is_empty() {
        return None;
    }
    match kind {
        "youtube" => Some(format!("https://www.youtube.com/watch?v={video_id}")),
        "vimeo" => Some(format!("https://vimeo.com/{video_id}")),
        _ => None,
    }
}

/// The FreeShow Player type and video id of a YouTube or Vimeo address.
pub fn player_video_id(link: &str) -> Option<(&'static str, String)> {
    let link = link.trim();
    let rest = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
        .unwrap_or(link);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.").trim_start_matches("m.");
    let clean = |id: &str| {
        let id: String = id
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        (!id.is_empty()).then_some(id)
    };
    match host {
        "youtu.be" => clean(path).map(|id| ("youtube", id)),
        "youtube.com" | "music.youtube.com" => {
            if let Some(query) = path.strip_prefix("watch?") {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("v="))
                    .and_then(clean)
                    .map(|id| ("youtube", id))
            } else {
                ["shorts/", "embed/", "live/"]
                    .iter()
                    .find_map(|prefix| path.strip_prefix(prefix))
                    .and_then(clean)
                    .map(|id| ("youtube", id))
            }
        }
        "vimeo.com" | "player.vimeo.com" => path
            .trim_start_matches("video/")
            .split('/')
            .find(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            .map(|id| ("vimeo", id.to_string())),
        _ => None,
    }
}

/// The show-level timeline takes precedence over the active layout's timeline.
pub fn timeline_for_show(show: &Value) -> Option<(&Value, Option<&str>)> {
    let active = show
        .pointer("/settings/activeLayout")
        .and_then(Value::as_str)
        .unwrap_or("main");
    show.get("timeline")
        .filter(|timeline| timeline["actions"].is_array())
        .map(|timeline| (timeline, None))
        .or_else(|| {
            Some((
                show.get("layouts")?.get(active)?.get("timeline")?,
                Some(active),
            ))
        })
}

/// Timeline cues stored on a show document.
pub fn timeline_cues_from_show(show: &Value) -> Vec<TimelineCue> {
    let Some((timeline, layout)) = timeline_for_show(show) else {
        return Vec::new();
    };
    let imported_ms = layout.is_some() && show["origin"] == "propresenter";
    timeline_cues_from_actions(&timeline["actions"], imported_ms)
}

fn timeline_cues_from_actions(actions: &Value, imported_ms: bool) -> Vec<TimelineCue> {
    let Some(actions) = actions.as_array() else {
        return Vec::new();
    };
    let mut cues = Vec::new();
    for action in actions {
        let time_ms = action
            .get("time")
            .and_then(Value::as_u64)
            .or_else(|| {
                action.get("time").and_then(json_f32).map(|time| {
                    // Earlier ProPresenter imports saved millisecond positions as JSON floats.
                    (if imported_ms { time } else { time * 1000.0 }).round() as u64
                })
            })
            .unwrap_or(0);
        let kind = action
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("action")
            .to_string();
        let slide_index = (kind == "slide")
            .then(|| action.pointer("/data/index").and_then(json_usize))
            .flatten();
        let data = action
            .pointer("/data/id")
            .and_then(Value::as_str)
            .or_else(|| action.pointer("/data/path").and_then(Value::as_str))
            .or_else(|| match kind.as_str() {
                "slide" | "clear_background" | "clear_audio" | "clear_slide" | "clear_all" => None,
                _ => action["name"].as_str(),
            })
            .map(str::to_string)
            .or_else(|| {
                action
                    .pointer("/data/index")
                    .and_then(Value::as_u64)
                    .map(|index| index.to_string())
            });
        cues.push(TimelineCue {
            time_ms,
            name: action
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&kind)
                .to_string(),
            kind,
            data,
            slide_index,
        });
    }
    cues.sort_by_key(|cue| cue.time_ms);
    cues
}

pub fn timeline_text_from(cues: &[TimelineCue]) -> String {
    cues.iter()
        .map(|cue| {
            let seconds = cue.time_ms as f32 / 1000.0;
            match &cue.data {
                Some(data) if !data.is_empty() && data != &cue.kind => {
                    format!("{seconds:.3} {}:{data}", cue.kind)
                }
                _ => format!("{seconds:.3} {}", cue.kind),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_timeline_text(text: &str) -> Vec<TimelineCue> {
    let mut cues = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(time) = parts.next() else {
            continue;
        };
        let rest = parts.next().unwrap_or("next").trim();
        let seconds: f32 = if let Some(ms) = time.strip_suffix("ms") {
            ms.parse::<f32>().unwrap_or(0.0) / 1000.0
        } else {
            time.parse().unwrap_or(0.0)
        };
        let (kind, data) = rest
            .split_once(':')
            .map(|(kind, data)| {
                (
                    kind.trim(),
                    Some(data.trim().to_string()).filter(|s| !s.is_empty()),
                )
            })
            .unwrap_or((rest, None));
        let kind = if kind.is_empty() { "next" } else { kind };
        cues.push(TimelineCue {
            time_ms: (seconds.max(0.0) * 1000.0).round() as u64,
            name: kind.to_string(),
            kind: kind.to_string(),
            data,
            slide_index: None,
        });
    }
    cues.sort_by_key(|cue| cue.time_ms);
    cues
}

/// FreeShow's default library folder, when it already exists on this machine.
pub fn default_library_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }?;
    let path = home.join("Documents").join("FreeShow");
    path.is_dir().then_some(path)
}

/// Writes `bytes` by flushing a temporary file and renaming it into place.
/// The previous file, when there was one, is copied into the library `Backups` folder first.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), DataError> {
    if path.is_file() && fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let _ = keep_previous(path);
    atomic_replace(path, bytes)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), DataError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    replace_file(&tmp, path)?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn keep_previous(path: &Path) -> Result<(), DataError> {
    if !path.is_file() {
        return Ok(());
    }
    let root = if path.file_name().is_some_and(|name| name == "settings.json") {
        path.parent()
    } else {
        path.parent().and_then(Path::parent)
    };
    let Some(root) = root else {
        return Ok(());
    };
    if root.as_os_str().is_empty() {
        return Ok(());
    }
    let backups = root.join(BACKUPS_DIR);
    fs::create_dir_all(&backups)?;
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let dest = backups.join(format!("{file_name}.{stamp}"));
    fs::copy(path, &dest)?;
    prune_backups(&backups, &file_name)?;
    Ok(())
}

fn prune_backups(dir: &Path, file_name: &str) -> Result<(), DataError> {
    let prefix = format!("{file_name}.");
    let mut copies = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) {
            copies.push(entry.path());
        }
    }
    copies.sort();
    let keep = 8;
    if copies.len() > keep {
        let extra = copies.len() - keep;
        for path in copies.into_iter().take(extra) {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn read_transition(value: Option<&Value>) -> Option<TextTransition> {
    let value = value?;
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    if kind.is_empty() {
        return None;
    }
    Some(TextTransition {
        kind: kind.to_string(),
        direction: value
            .pointer("/custom/direction")
            .and_then(Value::as_str)
            .unwrap_or("left_right")
            .to_string(),
        duration_ms: value.get("duration").and_then(Value::as_u64).unwrap_or(500),
        easing: stored_easing(value.get("easing")),
    })
}

fn crop_pixels(crop: &Value, key: &str) -> u32 {
    let Some(value) = crop.get(key) else {
        return 0;
    };
    let number = value
        .as_f64()
        .or_else(|| value.as_u64().map(|pixels| pixels as f64))
        .or_else(|| value.as_i64().map(|pixels| pixels as f64))
        .unwrap_or(0.0);
    if number <= 0.0 {
        0
    } else {
        number.round().min(1_000_000.0) as u32
    }
}

fn stored_seconds(entry: &Value, key: &str) -> f64 {
    let Some(value) = entry.get(key) else {
        return 0.0;
    };
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_i64().map(|number| number as f64))
        .unwrap_or(0.0)
        .max(0.0)
}

fn stored_easing(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("sine")
        .to_string()
}

fn replace_file(from: &Path, to: &Path) -> Result<(), DataError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_err) if to.exists() => {
            let aside = temp_path(to).with_extension("replacing");
            fs::rename(to, &aside)?;
            if let Err(err) = fs::rename(from, to) {
                let _ = fs::rename(&aside, to);
                return Err(err.into());
            }
            let _ = fs::remove_file(aside);
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// The first text box supplies the text style for an applied template.
pub fn template_text_item(template: &Value) -> Option<&Value> {
    template.get("items")?.as_array()?.iter().find(|item| {
        item.is_object() && item.get("type").and_then(Value::as_str).unwrap_or("text") == "text"
    })
}

/// Gives every lyric box on `slide` the first text box look of `template`, keeping its words.
/// Locked slides and non-text items stay as they are. Returns whether anything changed.
pub fn restyle_with_template(slide: &mut Value, template: &Value) -> bool {
    let Some(template_item) = template_text_item(template) else {
        return false;
    };
    if slide.get("locked").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let style = template_item
        .get("style")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let align = template_item
        .get("align")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let line_align = template_item
        .pointer("/lines/0/align")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let text_style = template_item
        .pointer("/lines/0/text/0/style")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(items) = slide.get_mut("items").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for item in items {
        if item_locked(item) || item.get("type").and_then(Value::as_str).unwrap_or("text") != "text"
        {
            continue;
        }
        let hidden = item_hidden(item);
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let next_style = if hidden {
            Value::String(item_edit::hide_item_style(
                style.as_str().unwrap_or_default(),
            ))
        } else {
            style.clone()
        };
        object.insert("style".to_string(), next_style);
        object.insert("align".to_string(), align.clone());
        changed = true;
        let Some(lines) = object.get_mut("lines").and_then(Value::as_array_mut) else {
            continue;
        };
        for line in lines {
            let Some(line) = line.as_object_mut() else {
                continue;
            };
            line.insert("align".to_string(), Value::String(line_align.clone()));
            let Some(texts) = line.get_mut("text").and_then(Value::as_array_mut) else {
                continue;
            };
            for text in texts {
                if let Some(text) = text.as_object_mut() {
                    text.insert("style".to_string(), Value::String(text_style.clone()));
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMAZING: &str = r#"[
  "id-amazing",
  {
    "name": "Amazing Grace",
    "category": null,
    "settings": { "activeLayout": "layout-1", "template": null },
    "timestamps": { "created": 1, "modified": null, "used": null },
    "meta": { "title": "Amazing Grace", "author": "John Newton" },
    "slides": {
      "slide-1": {
        "group": "Verse",
        "color": null,
        "settings": {},
        "notes": "slow",
        "items": [
          {
            "style": "left:10px;",
            "lines": [
              {
                "align": "",
                "text": [
                  { "value": "Amazing grace, ", "style": "" },
                  { "value": "how sweet the sound", "style": "" }
                ]
              }
            ]
          }
        ]
      }
    },
    "layouts": { "layout-1": { "name": "Default", "slides": [] } },
    "media": {},
    "extraField": { "keep": true }
  }
]"#;

    #[test]
    fn show_file_reads_name_slides_and_keeps_unknown_fields() {
        let show = ShowFile::parse(AMAZING).unwrap();
        assert_eq!(show.id, "id-amazing");
        assert_eq!(show.name(), "Amazing Grace");
        let slides = show.slides();
        assert_eq!(slides.len(), 1);
        assert_eq!(slides[0].id, "slide-1");
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(slides[0].color, None);
        assert_eq!(
            slides[0].lines,
            vec!["Amazing grace, how sweet the sound".to_string()]
        );
        assert_eq!(
            show.show
                .pointer("/extraField/keep")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn nudge_item_box_moves_the_text_and_keeps_other_style() {
        let mut show = ShowFile::parse(AMAZING).unwrap();
        *show
            .show
            .pointer_mut("/slides/slide-1/items/0/style")
            .unwrap() = Value::String("left:10px;color:#fff;".into());
        assert!(show.nudge_item_box("slide-1", 20, -100, 0, 0));
        let style = show
            .show
            .pointer("/slides/slide-1/items/0/style")
            .and_then(Value::as_str)
            .unwrap();
        assert!(style.contains("left:30px"));
        assert!(style.contains("top:0px"));
        assert!(style.contains("color:#fff"));
        assert!(!show.nudge_item_box("missing", 1, 0, 0, 0));
    }

    #[test]
    fn blank_show_and_new_project_round_trip() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let library = Library::open(&root);
        let mut show = ShowFile::blank("show-1", "Untitled");
        assert_eq!(show.name(), "Untitled");
        assert_eq!(show.ordered_slides().len(), 1);
        assert_eq!(show.add_slide().as_deref(), Some("s2"));
        let path = library.shows_dir().join("Untitled.show");
        show.save(&path).unwrap();
        let loaded = ShowFile::load(&path).unwrap();
        assert_eq!(loaded.id, "show-1");
        assert_eq!(loaded.ordered_slides().len(), 2);
        assert_eq!(
            loaded
                .show
                .pointer("/layouts/main/name")
                .and_then(Value::as_str),
            Some("Default")
        );

        let mut projects = ProjectsFile::empty();
        assert!(projects.add_project("p1", "07.11.26"));
        assert!(!projects.add_project("p1", "other"));
        assert!(projects.append_show("p1", "show-1"));
        projects.save(&library.projects_path()).unwrap();
        let again = ProjectsFile::load(&library.projects_path()).unwrap();
        let entries = again.projects();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "07.11.26");
        assert_eq!(entries[0].shows[0].id, "show-1");
        let stored = fs::read_to_string(library.projects_path()).unwrap();
        assert!(stored.contains("\"parent\":\"/\""));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn show_file_round_trip_preserves_json() {
        let show = ShowFile::parse(AMAZING).unwrap();
        let again = ShowFile::parse(&show.to_json().unwrap()).unwrap();
        assert_eq!(again, show);
    }

    #[test]
    fn show_save_keeps_the_previous_copy_in_backups() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-backup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let library = Library::open(&root);
        let mut show = ShowFile::parse(AMAZING).unwrap();
        let path = library.shows_dir().join("Amazing Grace.show");
        show.save(&path).unwrap();
        show.save(&path).unwrap();
        assert!(
            fs::read_dir(root.join(BACKUPS_DIR)).is_err() || {
                fs::read_dir(root.join(BACKUPS_DIR))
                    .unwrap()
                    .flatten()
                    .count()
                    == 0
            }
        );

        assert!(show.set_plain_text("slide-1", "changed line"));
        show.save(&path).unwrap();
        let loaded = ShowFile::load(&path).unwrap();
        assert_eq!(loaded.plain_text("slide-1"), "changed line");
        assert!(!temp_path(&path).exists());

        let backup = fs::read_dir(root.join(BACKUPS_DIR))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .next()
            .expect("previous show");
        let previous = fs::read_to_string(backup).unwrap();
        assert!(previous.contains("Amazing grace, "));
        assert!(previous.contains("how sweet the sound"));
        assert!(!previous.contains("changed line"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_show_is_rejected() {
        assert!(ShowFile::parse("{").is_err());
        assert!(ShowFile::parse("{}").is_err());
        assert!(ShowFile::parse("[\"only-id\"]").is_err());
    }

    #[test]
    fn library_lists_shows_and_projects() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-lib-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let library = Library::open(&root);
        let show = ShowFile::parse(AMAZING).unwrap();
        show.save(&library.shows_dir().join("Amazing Grace.show"))
            .unwrap();
        fs::create_dir_all(library.projects_path().parent().unwrap()).unwrap();
        fs::write(
            library.projects_path(),
            r#"{"projects":{"p1":{"name":"Sunday","created":1,"parent":"/","shows":[{"id":"id-amazing","type":"show"}]}},"folders":{},"kept":true}"#,
        )
        .unwrap();

        let listed = library.list_shows().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name(), "Amazing Grace");

        let projects = library.load_projects().unwrap();
        let entries = projects.projects();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Sunday");
        assert_eq!(entries[0].shows[0].id, "id-amazing");
        assert!(projects.to_json().unwrap().contains("\"kept\":true"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_library_folders_are_empty() {
        let root = std::env::temp_dir().join(format!("freeshow-rust-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let library = Library::open(&root);
        assert!(library.list_shows().unwrap().is_empty());
        assert!(library.load_projects().unwrap().projects().is_empty());
        assert!(library.load_templates().unwrap().names().is_empty());
        assert!(library.load_overlays().unwrap().names().is_empty());
        assert!(library.list_bibles().unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn groups_can_be_reordered_and_duplicated() {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.set_group("s1", "Verse"));
        assert!(show.set_notes("s1", "cue soft"));
        assert_eq!(show.show["slides"]["s1"]["notes"], "cue soft");
        assert!(show.add_slide().is_some());
        assert!(show.set_group("s2", "Chorus"));
        assert!(show.reorder_groups(0, 1));
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Chorus"));
        assert!(show.duplicate_slide("s2").is_some());
        assert_eq!(show.ordered_slides().len(), 3);
        assert!(show.replace_lyrics("one\n\ntwo"));
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[1].group.as_deref(), Some("Verse"));
        assert_eq!(slides[1].lines, vec!["two".to_string()]);
    }

    #[test]
    fn formatted_lyrics_turn_headers_into_groups() {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.replace_lyrics_with(
            "[Verse]\nIn the beginning\n\nChorus:\nPraise the Lord",
            true
        ));
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(slides[0].lines, vec!["In the beginning".to_string()]);
        assert_eq!(slides[1].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[1].lines, vec!["Praise the Lord".to_string()]);
        let mut show = ShowFile::blank("sunday", "Sunday");
        assert!(show.replace_lyrics_with_split("One\nTwo\nThree\nFour\nFive", false, 2));
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].lines, vec!["One".to_string(), "Two".to_string()]);
        assert_eq!(
            slides[1].lines,
            vec!["Three".to_string(), "Four".to_string()]
        );
        assert_eq!(slides[2].lines, vec!["Five".to_string()]);
    }

    #[test]
    fn player_links_round_trip_freeshow_video_ids() {
        assert_eq!(
            player_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=4"),
            Some(("youtube", "dQw4w9WgXcQ".to_string()))
        );
        assert_eq!(
            player_video_id("youtu.be/dQw4w9WgXcQ?si=x"),
            Some(("youtube", "dQw4w9WgXcQ".to_string()))
        );
        assert_eq!(
            player_video_id("https://youtube.com/shorts/abc_-1"),
            Some(("youtube", "abc_-1".to_string()))
        );
        assert_eq!(
            player_video_id("https://vimeo.com/channels/staff/144160599"),
            Some(("vimeo", "144160599".to_string()))
        );
        assert_eq!(player_video_id("https://cdn.example/clip.mp4"), None);
        let videos = player_videos_from(&serde_json::json!({
            "a": {"name": "Intro", "type": "youtube", "id": "abc"},
            "b": {"name": "Story", "type": "vimeo", "id": "123"},
        }));
        assert_eq!(
            videos[0].path.as_deref(),
            Some("https://www.youtube.com/watch?v=abc")
        );
        assert_eq!(videos[1].path.as_deref(), Some("https://vimeo.com/123"));
    }

    #[test]
    fn a_bracketed_first_line_names_the_group_even_without_formatting() {
        let mut show = ShowFile::blank("grace", "Grace");
        assert!(show.replace_lyrics_with_split(
            "[Chorus]\nMy chains are gone\n\nPlain verse",
            false,
            0
        ));
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[0].lines, vec!["My chains are gone".to_string()]);
        assert_eq!(slides[1].lines, vec!["Plain verse".to_string()]);
    }

    #[test]
    fn the_variable_editor_lists_each_variable_once_by_its_name() {
        let root = std::env::temp_dir().join(format!("fs-var-editor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(CONFIG_DIR)).unwrap();
        fs::write(
            root.join(CONFIG_DIR).join(SYNCED_SETTINGS_FILE),
            r#"{"variables":{"v1":{"name":"Speaker","type":"text","text":"Pastor Ram"}}}"#,
        )
        .unwrap();
        let library = Library::open(&root);
        assert_eq!(library.variable_editor_text(), "Speaker=Pastor Ram");
        assert!(library.save_variables_text("Speaker=Sita"));
        assert_eq!(library.variable_editor_text(), "Speaker=Sita");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_keeps_its_own_transition_until_it_is_cleared() {
        let mut show = ShowFile::blank("grace", "Grace");
        assert!(show.replace_lyrics_with_split("One\n\nTwo", false, 0));
        assert!(show.transition_at(1).is_none());
        assert!(show.set_transition_at(1, "spin", 800));
        let transition = show.transition_at(1).unwrap();
        assert_eq!(
            (transition.kind.as_str(), transition.duration_ms),
            ("spin", 800)
        );
        assert!(show.set_transition_at_dir(1, "slide", 600, "top_bottom"));
        let transition = show.transition_at(1).unwrap();
        assert_eq!(
            (
                transition.kind.as_str(),
                transition.duration_ms,
                transition.direction.as_str()
            ),
            ("slide", 600, "top_bottom")
        );
        assert!(show.transition_at(0).is_none());
        assert!(show.set_transition_at(1, "", 0));
        assert!(show.transition_at(1).is_none());
        assert!(!show.set_transition_at(9, "fade", 500));
    }

    #[test]
    fn typed_groups_get_freeshow_colours_and_a_repeated_chorus_is_one_slide() {
        let mut show = ShowFile::blank("grace", "Grace");
        let text = "[Verse 1]\nAmazing grace\n\n[Chorus]\nMy chains are gone\n\n[Verse 2]\nTwas grace\n\n[Chorus]\nMy chains are gone\n\n[Bridge]\nAnd like a flood";
        assert!(show.replace_lyrics_with_split(text, true, 0));
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 5, "the layout still plays the chorus twice");
        assert_eq!(slides[1].id, slides[3].id);
        assert_eq!(show.show["slides"].as_object().unwrap().len(), 4);
        let colour = |index: usize| show.show["slides"][&slides[index].id]["color"].clone();
        assert_eq!(colour(0), "#5825f5");
        assert_eq!(colour(1), "#f525d2");
        assert_eq!(colour(4), "#f52598");
        assert_eq!(show.show["slides"][&slides[1].id]["globalGroup"], "chorus");
        assert_eq!(
            default_group("Pre-Chorus 2"),
            Some(("pre_chorus", "#8825f5"))
        );
        assert_eq!(default_group("Welcome"), None);
        for format in [true, false] {
            let mut show = ShowFile::blank("bare", "Bare");
            let text = "[Verse 1]\nBlessed assurance\n\n[Chorus]\nThis is my story\n\n[Verse 2]\nPerfect submission\n\n[Chorus]";
            assert!(show.replace_lyrics_with_split(text, format, 0));
            let slides = show.ordered_slides();
            assert_eq!(slides.len(), 4, "format {format}");
            assert_eq!(slides[1].id, slides[3].id, "a bare [Chorus] repeats it");
        }
    }

    #[test]
    fn plain_text_export_round_trips_layout_lyrics() {
        let imported = crate::import::import_show(
            "Grace.txt",
            b"Title: Amazing Grace\n\n[Verse]\nAmazing grace, how sweet the sound\n\n[Chorus]\nPraise God\n",
        )
        .unwrap();
        let text = imported.export_plain_text();
        assert!(text.starts_with("Title: Amazing Grace"));
        assert!(text.contains("[Verse]"));
        assert!(text.contains("Amazing grace, how sweet the sound"));
        assert!(text.contains("[Chorus]"));
        assert!(text.contains("Praise God"));
        let again = crate::import::import_show("Grace.txt", text.as_bytes()).unwrap();
        assert_eq!(again.name(), "Amazing Grace");
        let slides = again.ordered_slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(slides[0].lines, ["Amazing grace, how sweet the sound"]);
        assert_eq!(slides[1].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[1].lines, ["Praise God"]);
        let path = std::env::temp_dir().join("freeshow-grace-export.txt");
        again.write_plain_text(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, text);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_active_layout_selects_which_slides_play() {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.set_plain_text("s1", "verse"));
        assert!(show.add_slide().is_some());
        assert!(show.set_plain_text("s2", "chorus"));
        let layouts = show
            .show
            .get_mut("layouts")
            .and_then(Value::as_object_mut)
            .unwrap();
        layouts.insert(
            "short".into(),
            serde_json::json!({
                "name": "Short",
                "notes": "",
                "slides": [{ "id": "s2" }]
            }),
        );
        assert_eq!(show.ordered_slides()[0].lines, vec!["verse".to_string()]);
        assert!(show.set_active_layout("short"));
        assert_eq!(show.active_layout(), "short");
        assert_eq!(show.ordered_slides()[0].lines, vec!["chorus".to_string()]);
        assert!(!show.set_active_layout("missing"));
        let views = show.layout_views();
        assert_eq!(views.len(), 2);
        assert_eq!(views[1].name, "Short");
        assert!(show.set_active_layout("main"));
        let layouts = show
            .show
            .get_mut("layouts")
            .and_then(Value::as_object_mut)
            .unwrap();
        layouts.insert(
            "main".into(),
            serde_json::json!({
                "name": "Default",
                "notes": "",
                "slides": [{
                    "id": "s1",
                    "transition": { "type": "slide", "duration": 200, "custom": { "direction": "right_left" } },
                    "nextTimer": 4,
                    "children": { "s2": { "transition": { "type": "none", "duration": 0 } } }
                }]
            }),
        );
        show.show
            .pointer_mut("/slides/s1")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert("children".into(), serde_json::json!(["s2"]));
        assert_eq!(show.ordered_slides().len(), 2);
        assert_eq!(
            show.transition_at(0),
            Some(TextTransition {
                kind: "slide".into(),
                direction: "right_left".into(),
                duration_ms: 200,
                easing: "sine".into(),
            })
        );
        assert_eq!(
            show.transition_at(1),
            Some(TextTransition {
                kind: "none".into(),
                direction: "left_right".into(),
                duration_ms: 0,
                easing: "sine".into(),
            })
        );
        assert_eq!(show.transition_at(2), None);
        assert_eq!(show.next_timer_at(0), Some(4.0));
        assert_eq!(show.next_timer_at(1), None);
    }

    #[test]
    fn one_slide_can_be_restyled_for_an_output_alone() {
        let template = serde_json::json!({"items":[{"style":"top:900px;","align":"","lines":[{"align":"left","text":[{"value":"","style":"font-size:40px;"}]}]}]});
        let mut slide = serde_json::json!({"items":[
            {"style":"top:0px;","lines":[{"align":"","text":[{"value":"Words","style":"font-size:90px;"}]}]},
            {"type":"media","src":"a.png"}
        ]});
        assert!(restyle_with_template(&mut slide, &template));
        assert_eq!(slide["items"][0]["style"], "top:900px;");
        assert_eq!(slide["items"][0]["lines"][0]["text"][0]["value"], "Words");
        assert_eq!(
            slide["items"][0]["lines"][0]["text"][0]["style"],
            "font-size:40px;"
        );
        assert_eq!(slide["items"][1]["src"], "a.png");
        let mut locked = serde_json::json!({"locked":true,"items":[{"style":"top:0px;"}]});
        assert!(!restyle_with_template(&mut locked, &template));
        assert!(!restyle_with_template(
            &mut slide,
            &serde_json::json!({"items":[]})
        ));
    }

    #[test]
    fn a_template_restyles_lyrics_without_replacing_them() {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.set_plain_text("s1", "Hello"));
        let template = serde_json::json!({
            "name": "Big",
            "items": [{
                "style": "top:10px;left:10px;width:100px;height:100px;",
                "align": "center",
                "lines": [{ "align": "center", "text": [{ "value": "", "style": "font-size:120px;" }] }]
            }]
        });
        assert!(show.apply_template("big", &template));
        assert_eq!(
            show.show
                .pointer("/settings/template")
                .and_then(Value::as_str),
            Some("big")
        );
        assert_eq!(
            show.show
                .pointer("/slides/s1/items/0/style")
                .and_then(Value::as_str),
            Some("top:10px;left:10px;width:100px;height:100px;")
        );
        assert_eq!(
            show.show
                .pointer("/slides/s1/items/0/lines/0/text/0/style")
                .and_then(Value::as_str),
            Some("font-size:120px;")
        );
        assert_eq!(
            show.show
                .pointer("/slides/s1/items/0/lines/0/text/0/value")
                .and_then(Value::as_str),
            Some("Hello")
        );
        assert!(!show.apply_template("", &template));
    }

    #[test]
    fn a_project_can_be_renamed_trimmed_and_deleted() {
        let mut projects = ProjectsFile::empty();
        assert!(projects.add_project("sunday", "Sunday"));
        assert!(projects.add_project("monday", "Monday"));
        assert!(projects.append_show("sunday", "grace"));
        assert!(projects.append_show("sunday", "other"));
        assert!(projects.set_project_name("sunday", "Easter"));
        assert!(projects.remove_show_at("sunday", 0));
        assert!(!projects.remove_show_at("sunday", 4));
        let sunday = projects
            .projects()
            .into_iter()
            .find(|project| project.id == "sunday")
            .unwrap();
        assert_eq!(sunday.name, "Easter");
        assert_eq!(sunday.shows.len(), 1);
        assert_eq!(sunday.shows[0].id, "other");
        assert!(projects.delete_project("sunday"));
        assert!(!projects.delete_project("sunday"));
        let left = projects.projects();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "monday");
    }

    #[test]
    fn a_project_keeps_the_folder_it_was_created_in() {
        let mut projects = ProjectsFile::empty();
        assert!(projects.add_project_in("easter", "Easter", "sundays"));
        assert!(!projects.add_project_in("easter", "Again", "sundays"));
        let easter = projects
            .projects()
            .into_iter()
            .find(|project| project.id == "easter")
            .unwrap();
        assert_eq!(easter.name, "Easter");
        assert_eq!(easter.parent, "sundays");
        assert!(easter.shows.is_empty());
    }

    #[test]
    fn layout_order_edit_and_project_reorder_round_trip() {
        let mut show = ShowFile::parse(
            r#"[
            "song",
            {
              "name": "Song",
              "settings": { "activeLayout": "main" },
              "slides": {
                "b": { "group": "Chorus", "items": [{ "style": "keep", "lines": [{ "align": "", "text": [{ "value": "chorus", "style": "bold" }] }] }] },
                "a": { "group": "Verse", "items": [{ "style": "keep", "lines": [{ "align": "", "text": [{ "value": "line", "style": "" }] }] }] }
              },
              "layouts": { "main": { "name": "Default", "slides": [{ "id": "a" }, { "id": "b" }] } },
              "extraField": { "keep": true }
            }
            ]"#,
        )
        .unwrap();
        let ordered = show.ordered_slides();
        assert_eq!(ordered[0].id, "a");
        assert_eq!(ordered[1].id, "b");
        assert!(show.set_plain_text("a", "new line\nsecond"));
        assert_eq!(show.plain_text("a"), "new line\nsecond");
        assert!(show.set_group("a", "Bridge"));
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Bridge"));
        assert_eq!(
            show.show
                .pointer("/slides/a/items/0/style")
                .and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(
            show.show
                .pointer("/extraField/keep")
                .and_then(Value::as_bool),
            Some(true)
        );
        let again = ShowFile::parse(&show.to_json().unwrap()).unwrap();
        assert_eq!(again.plain_text("a"), "new line\nsecond");

        let mut projects = ProjectsFile::parse(
            r#"{"projects":{"p1":{"name":"Sunday","shows":[{"id":"a"},{"id":"b"}]}},"folders":{},"kept":true}"#,
        )
        .unwrap();
        assert!(projects.reorder("p1", 0, 1));
        assert_eq!(projects.projects()[0].shows[0].id, "b");
        assert!(projects.append_show("p1", "c"));
        assert_eq!(projects.projects()[0].shows.len(), 3);
        assert!(projects.to_json().unwrap().contains("\"kept\":true"));
    }

    #[test]
    fn templates_overlays_settings_and_bibles_round_trip() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-stores-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let library = Library::open(&root);
        fs::create_dir_all(library.templates_path().parent().unwrap()).unwrap();
        fs::write(
            library.templates_path(),
            r#"{"t1":{"name":"Lyrics","items":[]},"note":"kept"}"#,
        )
        .unwrap();
        fs::write(
            library.overlays_path(),
            r#"{"o1":{"name":"Logo","items":[]}}"#,
        )
        .unwrap();
        fs::write(
            library.synced_settings_path(),
            r#"{"general":{"language":"en"}}"#,
        )
        .unwrap();
        fs::write(library.events_path(), r#"{"e1":{"name":"Sunday"}}"#).unwrap();
        fs::create_dir_all(library.bibles_dir()).unwrap();
        let bible = r#"["kjv",{"name":"KJV","books":[{"number":43,"name":"John","chapters":[{"number":3,"verses":[{"number":16,"text":"For God so loved the world"}]}]}]}]"#;
        fs::write(library.bibles_dir().join("KJV.fsb"), bible).unwrap();
        fs::create_dir_all(library.media_dir()).unwrap();
        fs::write(library.media_dir().join("bg.jpg"), b"img").unwrap();

        let templates = library.load_templates().unwrap();
        assert_eq!(templates.names(), vec!["Lyrics".to_string()]);
        assert!(templates.to_json().unwrap().contains("kept"));
        assert_eq!(
            library.load_overlays().unwrap().names(),
            vec!["Logo".to_string()]
        );
        assert!(
            library
                .load_synced_settings()
                .unwrap()
                .to_json()
                .unwrap()
                .contains("en")
        );
        assert_eq!(
            library.load_events().unwrap().names(),
            vec!["Sunday".to_string()]
        );
        let bibles = library.list_bibles().unwrap();
        assert_eq!(bibles[0].name(), "KJV");
        let hits = bibles[0].search("loved");
        assert_eq!(hits[0].reference, "John 3:16");
        assert!(hits[0].text.contains("loved"));
        let again = BibleFile::parse(&bibles[0].to_json().unwrap()).unwrap();
        assert_eq!(again, bibles[0]);
        assert_eq!(
            library.list_media_files(&library.media_dir()).unwrap(),
            vec!["bg.jpg".to_string()]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn text_transition_reads_the_stored_slide() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-transition-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let library = Library::open(&root);
        assert_eq!(library.text_transition(), TextTransition::default());
        assert_eq!(library.media_transition().duration_ms, 800);
        fs::write(
            library.settings_path(),
            r#"{"transitionData":{"text":{"type":"slide","duration":700,"easing":"sine","custom":{"direction":"top_bottom"}},"media":{"type":"blur","duration":900,"easing":"linear"}}}"#,
        )
        .unwrap();
        assert_eq!(
            library.text_transition(),
            TextTransition {
                kind: "slide".into(),
                direction: "top_bottom".into(),
                duration_ms: 700,
                easing: "sine".into(),
            }
        );
        assert_eq!(
            library.media_transition(),
            TextTransition {
                kind: "blur".into(),
                direction: "left_right".into(),
                duration_ms: 900,
                easing: "linear".into(),
            }
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn styles_can_be_listed_created_patched_and_deleted() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-style-edit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Config")).unwrap();
        let library = Library::open(&root);
        assert_eq!(
            library.styles(),
            vec![("default".to_string(), "Primary".to_string())]
        );
        assert_eq!(library.style_background(), None);
        assert!(library.patch_style("default", "background", Value::from("#102030")));
        assert_eq!(library.style_background().as_deref(), Some("#102030"));
        let id = library.create_style("Night").unwrap();
        assert_eq!(id, "style1");
        assert_eq!(library.styles().len(), 2);
        assert!(library.set_output_style(&id));
        assert_eq!(library.style_background(), None);
        assert!(library.patch_style(&id, "lines", Value::from(2)));
        assert_eq!(library.style_lines(), 2);
        assert!(library.patch_style(&id, "lines", Value::Null));
        assert_eq!(library.style_lines(), 0);
        assert!(!library.delete_style("default"));
        assert!(library.delete_style(&id));
        assert_eq!(library.output_style_id(), "default");
        assert_eq!(library.style_background().as_deref(), Some("#102030"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn style_transition_uses_the_enabled_audience_output() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-style-transition-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Config")).unwrap();
        let library = Library::open(&root);
        fs::write(
            library.settings_path(),
            r#"{"outputs":{
                "a-stage":{"active":true,"stageOutput":"default","name":"Stage","style":"stage"},
                "b-disabled":{"active":true,"enabled":false,"style":"disabled"},
                "c-room":{"name":"Room","style":"main"}
            }}"#,
        )
        .unwrap();
        fs::write(
            library.synced_settings_path(),
            r#"{"styles":{"main":{"name":"Main","transition":{"text":{"type":"scale","duration":250,"easing":"back"},"media":{"type":"spin","duration":1100,"easing":"bounce"}}}}}"#,
        )
        .unwrap();
        assert_eq!(library.audience_output(), ("c-room".into(), "Room".into()));
        assert_eq!(library.output_style_id(), "main");
        assert_eq!(
            library.style_transition("text"),
            Some(TextTransition {
                kind: "scale".into(),
                direction: "left_right".into(),
                duration_ms: 250,
                easing: "back".into(),
            })
        );
        assert_eq!(library.style_transition("media").unwrap().kind, "spin");
        assert!(library.set_output_style("next"));
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(library.settings_path()).unwrap()).unwrap();
        assert_eq!(saved["outputs"]["a-stage"]["style"], "stage");
        assert_eq!(saved["outputs"]["b-disabled"]["style"], "disabled");
        assert_eq!(saved["outputs"]["c-room"]["style"], "next");
        fs::write(
            library.settings_path(),
            r#"{"outputs":{"default":{"active":true,"style":"missing"}}}"#,
        )
        .unwrap();
        assert_eq!(library.style_transition("text"), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn media_look_reads_the_file_fit_and_flip() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-media-look-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Config")).unwrap();
        let library = Library::open(&root);
        let picture = root.join("bg.png");
        fs::write(
            library.settings_path(),
            r#"{"outputs":{"default":{"active":true,"style":"main"}}}"#,
        )
        .unwrap();
        fs::write(
            library.synced_settings_path(),
            r#"{"styles":{"main":{"fit":"fill"}}}"#,
        )
        .unwrap();
        assert_eq!(library.style_fit(), "fill");
        assert_eq!(library.media_look(&picture), (String::new(), false, false));
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "fit": "cover", "flipped": true, "flippedY": true }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_look(&picture), ("cover".into(), true, true));
        assert_eq!(library.media_volume(&picture), 1.0);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "volume": 0.4 }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_volume(&picture), 0.4);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "volume": 0 }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_volume(&picture), 0.0);
        assert!(!library.media_loops(&picture));
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "loop": true }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert!(library.media_loops(&picture));
        assert_eq!(library.media_span(&picture), (0.0, 0.0));
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "fromTime": 1.5, "toTime": 12 }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_span(&picture), (1.5, 12.0));
        assert_eq!(library.media_tempo(&picture), 1.0);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "tempo": 1.5 }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert!((library.media_tempo(&picture) - 1.5).abs() < 0.001);
        assert_eq!(
            library.media_effect(&picture),
            (String::new(), String::new())
        );
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "filter": "grayscale(1)", "blend": "multiply" }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(
            library.media_effect(&picture),
            ("grayscale(1)".into(), "multiply".into())
        );
        assert_eq!(library.media_crop(&picture), (0, 0, 0, 0));
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "cropping": { "left": 12, "top": 3 } }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_crop(&picture), (12, 0, 3, 0));
        assert_eq!(library.style_blur(), (6.0, 0.3));
        fs::write(
            library.synced_settings_path(),
            r#"{"styles":{"main":{"fit":"blur","blurAmount":10,"blurOpacity":0.5}}}"#,
        )
        .unwrap();
        assert_eq!(library.style_blur(), (10.0, 0.5));
        assert_eq!(library.media_speed(&picture), 1.0);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "speed": "1.5" }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert!((library.media_speed(&picture) - 1.5).abs() < 0.001);
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "speed": 0 }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_speed(&picture), 1.0);
        assert_eq!(library.media_video_type(&picture), "");
        let store = serde_json::json!({
            picture.to_string_lossy().as_ref(): { "videoType": "foreground" }
        });
        fs::write(library.media_store_path(), store.to_string()).unwrap();
        assert_eq!(library.media_video_type(&picture), "foreground");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slide_keeps_its_picture_until_a_later_slide_clears_it() {
        let show = ShowFile {
            id: "song".into(),
            path: None,
            show: serde_json::json!({
                "settings": { "activeLayout": "main" },
                "slides": {
                    "s1": { "settings": {} },
                    "s2": { "settings": {} },
                    "s3": { "settings": { "backgroundImage": "/plain.png" } },
                    "s4": { "settings": {} }
                },
                "layouts": { "main": { "slides": [
                    { "id": "s1", "background": "bg", "audio": ["click", "missing"], "overlays": ["logo", ""] },
                    { "id": "s2" },
                    { "id": "s3" },
                    { "id": "s4", "actions": { "clearAudio": true, "slideActions": [{ "triggers": ["clear_background", "clear_overlays", "clear_audio"] }] } }
                ] } },
                "media": { "bg": { "path": "/chapel.png" }, "click": { "path": "/click.mp3" } }
            }),
        };
        assert_eq!(
            show.backdrop_at(0),
            SlideBackdrop::Picture {
                path: "/chapel.png".into(),
                foreground: false
            }
        );
        assert_eq!(show.backdrop_at(1), show.backdrop_at(0));
        assert_eq!(show.audio_at(0), vec!["/click.mp3".to_string()]);
        assert!(show.audio_at(1).is_empty());
        assert_eq!(show.overlays_at(0), vec!["logo".to_string()]);
        assert!(!show.clears_overlays_at(0));
        assert!(show.clears_overlays_at(3));
        assert!(!show.clears_audio_at(0));
        assert!(show.clears_audio_at(3));
        assert_eq!(
            show.backdrop_at(2),
            SlideBackdrop::Picture {
                path: "/plain.png".into(),
                foreground: false
            }
        );
        assert_eq!(show.backdrop_at(3), SlideBackdrop::Clear);
        let once = ShowFile {
            id: "once".into(),
            path: None,
            show: serde_json::json!({
                "settings": { "activeLayout": "main" },
                "slides": { "s1": { "settings": {} }, "s2": { "settings": {} } },
                "layouts": { "main": { "slides": [
                    { "id": "s1", "background": "clip" },
                    { "id": "s2" }
                ] } },
                "media": { "clip": { "path": "/once.png", "loop": false } }
            }),
        };
        assert_eq!(
            once.backdrop_at(0),
            SlideBackdrop::Picture {
                path: "/once.png".into(),
                foreground: false
            }
        );
        assert_eq!(once.backdrop_at(1), SlideBackdrop::Unchanged);
        let cleared = ShowFile {
            id: "cleared".into(),
            path: None,
            show: serde_json::json!({
                "settings": { "activeLayout": "main" },
                "slides": { "s1": { "settings": {} }, "s2": { "settings": {} } },
                "layouts": { "main": { "slides": [
                    { "id": "s1", "actions": { "slideActions": [{ "triggers": ["clear_slide"] }] } },
                    { "id": "s2", "actions": { "slideActions": [{ "triggers": ["clear_all"] }] } }
                ] } },
                "media": {}
            }),
        };
        assert!(cleared.clears_slide_at(0));
        assert!(!cleared.clears_audio_at(0));
        assert_eq!(cleared.backdrop_at(0), SlideBackdrop::Unchanged);
        assert!(cleared.clears_slide_at(1));
        assert!(cleared.clears_audio_at(1));
        assert!(cleared.clears_overlays_at(1));
        assert_eq!(cleared.backdrop_at(1), SlideBackdrop::Clear);
        let linked = ShowFile {
            id: "linked".into(),
            path: None,
            show: serde_json::json!({
                "settings": { "activeLayout": "main" },
                "slides": { "s1": { "settings": {} }, "s2": { "settings": {} } },
                "layouts": { "main": { "slides": [
                    { "id": "s1", "disabled": true, "actions": { "startShow": { "id": "other" } } },
                    { "id": "s2", "end": true, "actions": { "slideActions": [{ "actionValues": { "start_show": { "id": "next" } } }] } }
                ] } },
                "media": {}
            }),
        };
        assert_eq!(linked.started_show_at(0).as_deref(), Some("other"));
        assert_eq!(linked.started_show_at(1).as_deref(), Some("next"));
        assert_eq!(linked.first_enabled_slide(), Some(1));
        assert_eq!(linked.step_enabled(0, 1), Some(1));
        assert_eq!(linked.step_enabled(1, -1), None);
        assert_eq!(linked.step_enabled(1, 1), None);
        assert!(!linked.ends_at(0));
        assert!(linked.ends_at(1));
        let timed = ShowFile {
            id: "timed".into(),
            path: None,
            show: serde_json::json!({
                "settings": { "activeLayout": "main" },
                "slides": {
                    "s1": { "settings": {}, "items": [
                        { "type": "timer", "timer": { "id": "sermon" } },
                        { "type": "text", "timerId": "ignored" }
                    ] },
                    "s2": { "settings": {} },
                    "s3": { "settings": {} }
                },
                "layouts": { "main": { "slides": [
                    { "id": "s1", "background": "clip", "audio": ["song"], "actions": {
                        "nextAfterMedia": true,
                        "startTimer": true,
                        "slideActions": [
                            { "triggers": ["id_start_timer"], "actionValues": { "id_start_timer": { "id": "welcome" } } },
                            { "triggers": ["change_output_style"], "actionValues": { "change_output_style": { "outputStyle": "night" } } }
                        ]
                    } },
                    { "id": "s2", "actions": { "stopTimers": true, "outputStyle": "day", "audioStream": "/chapel/bed.mp3" } },
                    { "id": "s3", "bindings": ["stage"], "actions": { "slideActions": [
                        { "triggers": ["pause_timers"] },
                        { "triggers": ["start_audio_stream"], "actionValues": { "start_audio_stream": { "id": "bed", "value": "https://example.com/live" } } }
                    ] } }
                ] } },
                "media": {
                    "clip": { "path": "/films/sermon.mp4" },
                    "song": { "path": "/audio/bed.mp3" }
                }
            }),
        };
        assert_eq!(
            timed.media_flags("/films/sermon.mp4"),
            Some((true, true, None))
        );
        assert!(timed.can_output(0, "default", ""));
        assert!(!timed.can_output(2, "default", ""));
        assert!(timed.can_output(2, "other", "Stage"));
        assert_eq!(timed.step_output(0, 1, "default", ""), Some(1));
        assert_eq!(timed.audio_stream_at(1).as_deref(), Some("/chapel/bed.mp3"));
        assert_eq!(
            timed.audio_stream_at(2).as_deref(),
            Some("https://example.com/live")
        );
        assert_eq!(timed.output_style_at(0).as_deref(), Some("night"));
        assert_eq!(timed.output_style_at(1).as_deref(), Some("day"));
        assert_eq!(timed.output_style_at(2), None);
        assert!(timed.nexts_after_media_at(0));
        assert!(timed.waits_for_ended(0, "/films/sermon.mp4", false));
        assert!(timed.waits_for_ended(0, "/audio/bed.mp3", true));
        assert!(!timed.waits_for_ended(0, "/other.mp4", false));
        assert!(!timed.nexts_after_media_at(1));
        assert_eq!(
            timed.timer_ids_at(0),
            vec!["welcome".to_string(), "sermon".to_string()]
        );
        assert!(!timed.stops_timers_at(0));
        assert!(timed.timer_ids_at(1).is_empty());
        assert!(timed.stops_timers_at(1));
        assert!(timed.pauses_timers_at(2));
        assert!(!timed.stops_timers_at(2));
    }

    #[test]
    fn a_folder_row_keeps_its_timer() {
        let projects = ProjectsFile::parse(
            r#"{"projects":{"sunday":{"name":"Sunday","shows":[{"id":"/slides","type":"folder","data":{"timer":4}}]}}}"#,
        )
        .unwrap();
        let row = &projects.projects()[0].shows[0];
        assert_eq!(row.kind, "folder");
        assert_eq!(row.timer_ms, Some(4_000));
        assert_eq!(row.loops, None);
        assert_eq!(row.muted, None);
        let projects = ProjectsFile::parse(
            r#"{"projects":{"sunday":{"name":"Sunday","shows":[{"id":"/clip.mp4","type":"video","loop":false,"muted":true}]}}}"#,
        )
        .unwrap();
        let row = &projects.projects()[0].shows[0];
        assert_eq!(row.loops, Some(false));
        assert_eq!(row.muted, Some(true));
    }

    #[test]
    fn next_item_on_last_slide_defaults_on() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-next-item-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let library = Library::open(&root);
        assert!(library.next_item_on_last_slide());
        fs::write(
            library.settings_path(),
            r#"{"special":{"nextItemOnLastSlide":false}}"#,
        )
        .unwrap();
        assert!(!library.next_item_on_last_slide());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn library_preferences_read_theme_and_remote_password() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-preferences-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let library = Library::open(&root);
        assert_eq!(
            library.preferences(),
            FreeShowPreferences::default(),
            "a missing settings file has no preferences"
        );
        fs::write(
            library.settings_path(),
            r#"{"theme":"sea","serverData":{"remote":{"password":" 1234 "}},"ports":{"remote":5600,"stage":5601}}"#,
        )
        .unwrap();
        let preferences = library.preferences();
        assert_eq!(preferences.theme.as_deref(), Some("sea"));
        assert_eq!(preferences.remote_password.as_deref(), Some("1234"));
        assert_eq!(preferences.remote_port, Some(5600));
        assert_eq!(preferences.stage_port, Some(5601));
        fs::create_dir_all(root.join(CONFIG_DIR)).unwrap();
        let themes_path = root.join(CONFIG_DIR).join("themes.json");
        let themes = serde_json::json!({"sea": {
            "colors": {"primary": "#123456"},
            "font": {"family": "Georgia", "size": "1.2em"}
        }});
        fs::write(&themes_path, themes.to_string()).unwrap();
        assert_eq!(library.preferences().themes, themes);
        // Reading compatibility settings must not rewrite the Electron files.
        assert_eq!(
            fs::read_to_string(&themes_path).unwrap(),
            themes.to_string()
        );
        fs::remove_file(library.settings_path()).unwrap();
        assert_eq!(library.preferences().theme, None);
        assert_eq!(library.preferences().themes, themes);
        fs::write(&themes_path, "invalid json").unwrap();
        assert_eq!(library.preferences().themes, Value::Null);
        fs::write(
            library.settings_path(),
            r#"{"theme":"","serverData":{"remote":{"password":""}},"ports":{"remote":0,"stage":"x"}}"#,
        )
        .unwrap();
        assert_eq!(library.preferences(), FreeShowPreferences::default());
        fs::write(library.settings_path(), "not json").unwrap();
        assert_eq!(library.preferences(), FreeShowPreferences::default());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn library_preferences_read_operator_settings_and_outputs() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-operator-settings-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let library = Library::open(&root);
        fs::write(
            library.settings_path(),
            r#"{
                "language":" nb ",
                "timeFormat":"12",
                "autosave":"5min",
                "labelsDisabled":true,
                "groupNumbers":false,
                "formatNewShow":true,
                "autoOutput":true,
                "maxConnections":3,
                "outputs":{
                    "default":{"enabled":true,"active":true,"name":"Primary","style":"wide"},
                    "stage":{"enabled":false,"name":"Side","style":""}
                }
            }"#,
        )
        .unwrap();
        let preferences = library.preferences();
        assert_eq!(preferences.language.as_deref(), Some("nb"));
        assert!(preferences.uses_12_hour_clock());
        assert_eq!(
            preferences.autosave_interval(),
            Some(std::time::Duration::from_secs(5 * 60))
        );
        assert!(preferences.labels_disabled());
        assert!(!preferences.group_numbers());
        assert!(preferences.format_new_show());
        assert_eq!(preferences.auto_output, Some(true));
        assert_eq!(preferences.max_connections(), 3);
        assert_eq!(
            preferences.outputs,
            vec![
                OutputPreference {
                    id: "default".into(),
                    name: "Primary".into(),
                    enabled: true,
                    active: true,
                    style: "wide".into(),
                    width: None,
                    height: None,
                    always_on_top: true,
                    kiosk: false,
                    stage: false,
                },
                OutputPreference {
                    id: "stage".into(),
                    name: "Side".into(),
                    enabled: false,
                    active: false,
                    style: "".into(),
                    width: None,
                    height: None,
                    always_on_top: true,
                    kiosk: false,
                    stage: false,
                },
            ]
        );
        assert_eq!(format_clock("00:05:09", true, true), "12:05:09 AM");
        assert_eq!(format_clock("13:05:09", true, false), "1:05 PM");
        assert_eq!(format_clock("13:05:09", false, false), "13:05");
        let defaults = FreeShowPreferences::default();
        assert!(!defaults.uses_12_hour_clock());
        assert_eq!(
            defaults.autosave_interval(),
            Some(std::time::Duration::from_secs(15 * 60))
        );
        assert!(defaults.group_numbers());
        assert_eq!(defaults.max_connections(), 10);
        fs::write(
            library.settings_path(),
            r#"{"timeFormat":"26","autosave":"soon","maxConnections":-1,"language":""}"#,
        )
        .unwrap();
        assert_eq!(library.preferences(), FreeShowPreferences::default());
        fs::write(library.settings_path(), r#"{"autosave":"never"}"#).unwrap();
        assert_eq!(library.preferences().autosave.as_deref(), Some("never"));
        assert_eq!(library.preferences().autosave_interval(), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn library_preferences_read_remaining_settings_keys() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-rust-remaining-settings-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(CONFIG_DIR)).unwrap();
        let library = Library::open(&root);
        fs::write(
            library.settings_path(),
            r#"{
                "outLocked": true,
                "splitLines": 4,
                "fullColors": true,
                "alertUpdates": false,
                "customFonts": [{"name": "Display", "path": "fonts/Display.ttf"}],
                "lockedOverlays": {"default": ["logo", "lower"]},
                "activeScenes": {"default": ["warm"]},
                "resized": {"drawer": 320},
                "slidesOptions": {"columns": 4, "mode": "grid"},
                "drawer": {"height": 220},
                "metronome": {"bpm": 80},
                "audioEffects": {"main": {"reverb": 0.2}},
                "effectsLibrary": [{"path": "fx/glow.json", "name": "Glow"}],
                "timeline": {"zoom": 1.5},
                "timecode": {"enabled": true},
                "cloudSyncData": {"enabled": false},
                "driveData": {"method": "disable"},
                "contentProviderData": {"planningcenter": {"churchId": "c1"}},
                "obsData": {"enabled": true, "port": 4455},
                "ai": {"language": "en"},
                "shortcuts": {"undo": "Ctrl+Shift+Z", "go": "Enter"},
                "special": {"guideSeen": true, "focusMode": true}
            }"#,
        )
        .unwrap();
        fs::write(
            library.synced_settings_path(),
            r#"{"drawSettings":{"color":"yellow"},"profiles":{"house":{"name":"House"}},"scenes":{"warm":{"name":"Warm"}},"interactions":{"go":{"action":"next"}}}"#,
        )
        .unwrap();
        let preferences = library.preferences();
        assert!(preferences.out_locked());
        assert_eq!(preferences.split_lines(), 4);
        assert!(preferences.full_colors());
        assert_eq!(preferences.drawer_height(), 220);
        assert!(preferences.guide_seen());
        assert!(preferences.focus_mode_setting());
        assert_eq!(preferences.shortcut("undo"), "Ctrl+Shift+Z");
        assert_eq!(preferences.shortcut("redo"), "Ctrl+Y");
        assert_eq!(preferences.shortcut("go"), "Enter");
        assert_eq!(preferences.left_panel_width(), 240);
        assert_eq!(preferences.right_panel_width(), 260);
        assert_eq!(preferences.slides_columns(), 4);
        assert_eq!(preferences.slides_mode(), "grid");
        assert_eq!(
            preferences.custom_font_files(),
            vec![(
                "Display".to_string(),
                std::path::PathBuf::from("fonts/Display.ttf")
            )]
        );
        assert_eq!(preferences.metronome_bpm(), 80);
        assert_eq!(preferences.metronome_beats(), 4);
        assert_eq!(preferences.timecode_kind(), None);
        // resized.drawer is 320 in this fixture, but drawer.height (220) wins.
        assert!(!preferences.alert_updates());
        assert_eq!(
            preferences.locked_overlay_ids("default"),
            vec!["logo".to_string(), "lower".to_string()]
        );
        assert!(preferences.overlay_is_locked("default", "logo"));
        assert_eq!(
            preferences.custom_fonts,
            serde_json::json!([{"name": "Display", "path": "fonts/Display.ttf"}])
        );
        assert_eq!(
            preferences.active_scenes,
            serde_json::json!({"default": ["warm"]})
        );
        assert_eq!(preferences.resized, serde_json::json!({"drawer": 320}));
        assert_eq!(
            preferences.slides_options,
            serde_json::json!({"columns": 4, "mode": "grid"})
        );
        assert_eq!(preferences.drawer, serde_json::json!({"height": 220}));
        assert_eq!(preferences.metronome, serde_json::json!({"bpm": 80}));
        assert_eq!(
            preferences.audio_effects,
            serde_json::json!({"main": {"reverb": 0.2}})
        );
        assert_eq!(
            preferences.effects_library,
            serde_json::json!([{"path": "fx/glow.json", "name": "Glow"}])
        );
        assert_eq!(preferences.timeline, serde_json::json!({"zoom": 1.5}));
        assert_eq!(preferences.timecode, serde_json::json!({"enabled": true}));
        assert_eq!(
            preferences.cloud_sync_data,
            serde_json::json!({"enabled": false})
        );
        assert_eq!(
            preferences.drive_data,
            serde_json::json!({"method": "disable"})
        );
        assert_eq!(
            preferences.content_provider_data,
            serde_json::json!({"planningcenter": {"churchId": "c1"}})
        );
        assert_eq!(
            preferences.obs_data,
            serde_json::json!({"enabled": true, "port": 4455})
        );
        assert_eq!(preferences.ai, serde_json::json!({"language": "en"}));
        assert_eq!(
            preferences.draw_settings,
            serde_json::json!({"color": "yellow"})
        );
        assert_eq!(
            preferences.profiles,
            serde_json::json!({"house": {"name": "House"}})
        );
        assert_eq!(
            preferences.scenes,
            serde_json::json!({"warm": {"name": "Warm"}})
        );
        assert_eq!(
            preferences.interactions,
            serde_json::json!({"go": {"action": "next"}})
        );
        let defaults = FreeShowPreferences::default();
        assert!(!defaults.out_locked());
        assert_eq!(defaults.split_lines(), 0);
        assert!(!defaults.full_colors());
        assert!(defaults.alert_updates());
        assert!(defaults.locked_overlay_ids("default").is_empty());
        fs::write(
            library.settings_path(),
            r#"{"outLocked":"yes","splitLines":-1,"alertUpdates":"no"}"#,
        )
        .unwrap();
        fs::write(library.synced_settings_path(), "not json").unwrap();
        let ignored = library.preferences();
        assert!(!ignored.out_locked());
        assert_eq!(ignored.split_lines(), 0);
        assert!(ignored.alert_updates());
        assert_eq!(ignored.draw_settings, Value::Null);
        let _ = fs::remove_dir_all(&root);
    }
}
