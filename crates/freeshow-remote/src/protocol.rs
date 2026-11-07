// SPDX-License-Identifier: GPL-3.0-only

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the operator window should do after a remote message.
/// Indexes are absolute so a second click cannot be applied twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SetShow(usize),
    SetSlide(usize),
    /// Open one row of the current project. `slide` is set for a song and empty for media.
    OpenProjectItem { index: usize, slide: Option<usize> },
    Clear,
    PresentVerse { reference: String, text: String },
    SetShowText { id: String, text: String },
    CreateShow { id: String, name: String, text: String },
    AddToProject { project_id: String, show_id: String },
    ShowOverlay { id: String },
    ClearOverlay { id: String },
    ClearOverlays,
    Play { name: String },
    /// Output gain in thousandths, so 1000 is full volume.
    SetVolume { thousandths: u16 },
    StopAudio,
    ToggleMedia,
    PauseAudio { name: String },
    SeekAudio { millis: u64 },
    SeekVideo { millis: u64 },
    SetMediaLoop { on: bool },
    SetMediaMute { on: bool },
    SetTemplate { id: String },
    StartTimer { id: String },
    PauseTimer { id: String },
    StopTimer { id: String },
    RearrangeGroups {
        show_id: String,
        from: usize,
        to: usize,
    },
    AddGroup { show_id: String, group_id: String },
    ChangeLayout { show_id: String, layout_id: String },
    RenameProject { project_id: String, name: String },
    DeleteProject { project_id: String },
    RemoveProjectItem { project_id: String, index: usize },
    CreateProject {
        id: String,
        name: String,
        parent: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShowSnapshot {
    pub shows: Vec<RemoteShow>,
    pub show_index: Option<usize>,
    pub slide_index: Option<usize>,
    pub cleared: bool,
    pub project: Vec<String>,
    #[serde(default)]
    pub projects: Vec<RemoteProject>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub folders: Vec<RemoteFolder>,
    /// Shared so the 100ms snapshot refresh does not copy every verse.
    #[serde(skip, default = "empty_bibles")]
    pub bibles: Arc<Vec<RemoteBible>>,
    /// When set, the audience is showing a verse instead of the current song slide.
    pub scripture_lines: Vec<String>,
    #[serde(default)]
    pub overlays: Vec<RemoteOverlay>,
    #[serde(default)]
    pub active_overlays: Vec<String>,
    #[serde(default)]
    pub media: Vec<RemoteFile>,
    #[serde(default)]
    pub audio: Vec<RemoteFile>,
    #[serde(default)]
    pub templates: Vec<RemoteTemplate>,
    #[serde(default)]
    pub timers: Vec<RemoteTimer>,
    #[serde(default)]
    pub active_timer: Option<String>,
    #[serde(default)]
    pub timer_paused: bool,
    /// Background video restarts at the end. FreeShow starts a clip looping.
    #[serde(default = "looping_media")]
    pub media_loop: bool,
    #[serde(default)]
    pub media_paused: bool,
    /// Background video starts silent. FreeShow treats a missing mute flag as muted.
    #[serde(default = "muted_media")]
    pub media_muted: bool,
    #[serde(default)]
    pub video_time: f64,
    #[serde(default)]
    pub video_duration: f64,
    /// File name of the clip the phone started, empty when nothing is playing.
    #[serde(default)]
    pub playing_audio: String,
    #[serde(default)]
    pub audio_time: f64,
    #[serde(default)]
    pub audio_duration: f64,
    #[serde(default)]
    pub audio_paused: bool,
    /// Linear output gain, 1 is full volume.
    #[serde(default = "full_volume")]
    pub volume: f32,
    /// Row in the current project that the audience is playing.
    #[serde(default)]
    pub project_item: usize,
    /// When this is false, the last slide stays up instead of opening the next project item.
    #[serde(default = "next_item_default")]
    pub next_item_on_last_slide: bool,
}

fn full_volume() -> f32 {
    1.0
}

fn next_item_default() -> bool {
    true
}

fn looping_media() -> bool {
    true
}

fn muted_media() -> bool {
    true
}

fn empty_bibles() -> Arc<Vec<RemoteBible>> {
    Arc::new(Vec::new())
}

impl Default for ShowSnapshot {
    fn default() -> Self {
        Self {
            shows: Vec::new(),
            show_index: None,
            slide_index: None,
            cleared: false,
            project: Vec::new(),
            projects: Vec::new(),
            project_id: None,
            folders: Vec::new(),
            bibles: Arc::new(Vec::new()),
            scripture_lines: Vec::new(),
            overlays: Vec::new(),
            active_overlays: Vec::new(),
            media: Vec::new(),
            audio: Vec::new(),
            templates: Vec::new(),
            timers: Vec::new(),
            active_timer: None,
            timer_paused: false,
            media_loop: true,
            media_paused: false,
            media_muted: true,
            video_time: 0.0,
            video_duration: 0.0,
            playing_audio: String::new(),
            audio_time: 0.0,
            audio_duration: 0.0,
            audio_paused: false,
            volume: 1.0,
            project_item: 0,
            next_item_on_last_slide: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteBible {
    pub id: String,
    pub name: String,
    pub books: Vec<RemoteBook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteBook {
    pub number: u64,
    pub name: String,
    pub chapters: Vec<RemoteChapter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteChapter {
    pub number: u64,
    pub verses: Vec<RemoteVerse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteVerse {
    pub number: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteOverlay {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub items: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteFile {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteTemplate {
    pub id: String,
    pub name: String,
    pub body: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteTimer {
    pub id: String,
    pub name: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteProject {
    pub id: String,
    pub name: String,
    pub parent: String,
    pub created: u64,
    pub shows: Vec<RemoteProjectItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteProjectItem {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteFolder {
    pub id: String,
    pub name: String,
    pub parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteLayout {
    pub id: String,
    pub name: String,
    pub slides: Vec<RemoteSlide>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteShow {
    pub id: String,
    pub name: String,
    pub slides: Vec<RemoteSlide>,
    #[serde(default)]
    pub layouts: Vec<RemoteLayout>,
    #[serde(default)]
    pub active_layout: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSlide {
    pub label: String,
    pub lines: Vec<String>,
    pub notes: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub group: String,
}

impl ShowSnapshot {
    pub fn view(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "lines".into(),
                json!(if self.scripture_lines.is_empty() {
                    self.slide_lines()
                } else {
                    self.scripture_lines.clone()
                }),
            );
            object.insert("next_lines".into(), json!(self.next_lines()));
            object.insert("notes".into(), json!(self.notes()));
            object.insert("show_name".into(), json!(self.show_name()));
        }
        value
    }

    pub fn show_name(&self) -> String {
        self.current_show()
            .map(|show| show.name.clone())
            .unwrap_or_default()
    }

    pub fn slide_lines(&self) -> Vec<String> {
        self.current_slide()
            .map(|slide| slide.lines.clone())
            .unwrap_or_default()
    }

    pub fn next_lines(&self) -> Vec<String> {
        let Some(index) = self.slide_index else {
            return Vec::new();
        };
        self.current_show()
            .and_then(|show| show.slides.get(index + 1))
            .map(|slide| slide.lines.clone())
            .unwrap_or_default()
    }

    pub fn notes(&self) -> String {
        self.current_slide()
            .map(|slide| slide.notes.clone())
            .unwrap_or_default()
    }

    fn current_show(&self) -> Option<&RemoteShow> {
        self.show_index.and_then(|index| self.shows.get(index))
    }

    fn current_slide(&self) -> Option<&RemoteSlide> {
        let index = self.slide_index?;
        self.current_show()?.slides.get(index)
    }
}

/// Replies sent when a remote connection is accepted.
fn access_granted(snapshot: &ShowSnapshot) -> Vec<Value> {
    vec![
        json!({ "channel": "ACCESS", "data": null }),
        shows_message(snapshot),
        out_message(snapshot),
        scripture_catalog(snapshot),
        projects_message(snapshot),
        folders_message(snapshot),
        project_message(snapshot),
        out_data_message(snapshot),
        media_message(snapshot),
    ]
}

/// Handle one FreeShow remote or stage message.
///
/// The body is `{ "channel", "data" }`, the same shape the Electron app
/// receives on the `REMOTE` and `STAGE` socket.io events.
pub fn handle_message(snapshot: &mut ShowSnapshot, message: &Value) -> (Vec<Value>, Vec<Command>) {
    let mut authenticated = true;
    handle_message_authenticated(snapshot, message, None, &mut authenticated)
}

/// Handle one message with the connection's password policy and state.
///
/// `password` is the configured remote password, if any. `authenticated` is
/// this connection's state and is updated by the `PASSWORD` and `ACCESS`
/// handshake. A connection that has not been accepted receives no replies and
/// cannot change the show.
pub fn handle_message_authenticated(
    snapshot: &mut ShowSnapshot,
    message: &Value,
    password: Option<&str>,
    authenticated: &mut bool,
) -> (Vec<Value>, Vec<Command>) {
    let Some(channel) = message.get("channel").and_then(Value::as_str) else {
        return (vec![unknown()], Vec::new());
    };
    let password = password.map(str::trim).filter(|value| !value.is_empty());
    let data = message.get("data").cloned().unwrap_or(Value::Null);
    let mut commands = Vec::new();
    let replies = match channel {
        "PASSWORD" => {
            if password.is_some() {
                vec![json!({
                    "channel": "PASSWORD",
                    "data": { "password": true, "dictionary": {} }
                })]
            } else {
                *authenticated = true;
                let mut replies = vec![json!({
                    "channel": "PASSWORD",
                    "data": { "password": false, "dictionary": {} }
                })];
                replies.extend(access_granted(snapshot));
                replies
            }
        }
        "ACCESS" => {
            if password.is_none() || data.as_str() == password {
                *authenticated = true;
                access_granted(snapshot)
            } else {
                *authenticated = false;
                vec![json!({ "channel": "ERROR", "data": { "key": "wrongPass" } })]
            }
        }
        _ if password.is_some() && !*authenticated => return (Vec::new(), Vec::new()),
        "API:get_output" => vec![out_message(snapshot)],
        "API:next_slide" => {
            if let Some(command) = step(snapshot, 1) {
                commands.push(command);
            }
            vec![out_message(snapshot)]
        }
        "API:previous_slide" => {
            if let Some(command) = step(snapshot, -1) {
                commands.push(command);
            }
            vec![out_message(snapshot)]
        }
        "API:index_select_slide" => {
            if let Some(id) = data.get("showId").and_then(Value::as_str)
                && let Some(command) = select_show(snapshot, id)
            {
                commands.push(command);
            }
            if let Some(index) = index_of(&data)
                && let Some(command) = set_slide(snapshot, index)
            {
                commands.push(command);
            }
            vec![out_message(snapshot)]
        }
        "SHOW" => {
            let id = match &data {
                Value::String(id) => id.as_str(),
                other => other.get("id").and_then(Value::as_str).unwrap_or(""),
            };
            if let Some(command) = select_show(snapshot, id) {
                commands.push(command);
            }
            vec![show_message(snapshot), out_message(snapshot)]
        }
        "API:clear" | "API:clear_all" => {
            snapshot.cleared = true;
            snapshot.scripture_lines.clear();
            snapshot.active_overlays.clear();
            commands.push(Command::Clear);
            vec![out_message(snapshot), out_data_message(snapshot)]
        }
        "API:clear_slide" | "API:clear_background" => {
            snapshot.cleared = true;
            snapshot.scripture_lines.clear();
            commands.push(Command::Clear);
            vec![out_message(snapshot), out_data_message(snapshot)]
        }
        "GET_OVERLAYS" => vec![overlays_message(snapshot)],
        "GET_TEMPLATES" => vec![templates_message(snapshot)],
        "GET_FUNCTIONS" => vec![functions_message(snapshot)],
        "API:id_start_timer" => timer_command(snapshot, &data, TimerAct::Start, &mut commands),
        "API:id_pause_timer" => timer_command(snapshot, &data, TimerAct::Pause, &mut commands),
        "API:id_stop_timer" => timer_command(snapshot, &data, TimerAct::Stop, &mut commands),
        "API:set_template" => set_template(snapshot, &data, &mut commands),
        "API:id_select_overlay" => show_overlay(snapshot, &data, &mut commands),
        "API:clear_overlay" => clear_one_overlay(snapshot, &data, &mut commands),
        "API:clear_overlays" => {
            snapshot.active_overlays.clear();
            commands.push(Command::ClearOverlays);
            vec![out_data_message(snapshot), cleared_message(snapshot)]
        }
        "API:play_media" => play_named(snapshot, &data, true, &mut commands),
        "API:play_audio" => play_named(snapshot, &data, false, &mut commands),
        "API:toggle_media_loop" => toggle_media_loop(snapshot, &mut commands),
        "API:toggle_media_mute" => toggle_media_mute(snapshot, &mut commands),
        "API:get_media_loop_state" => vec![json!({
            "channel": "API:get_media_loop_state",
            "data": snapshot.media_loop
        })],
        "API:get_playing_video_state" => vec![video_state_message(snapshot)],
        "API:get_playing_audio_data" => vec![audio_data_message(snapshot)],
        "API:get_playing_audio_time" => vec![json!({
            "channel": "API:get_playing_audio_time",
            "data": snapshot.audio_time
        })],
        "API:pause_audio" => pause_audio(&data, &mut commands),
        "API:audio_seekto" => seek_to(true, &data, &mut commands),
        "API:video_seekto" => seek_to(false, &data, &mut commands),
        "API:stop_audio" => {
            commands.push(Command::StopAudio);
            vec![cleared_message(snapshot)]
        }
        "API:toggle_playing_media" => {
            snapshot.media_paused = !snapshot.media_paused;
            commands.push(Command::ToggleMedia);
            vec![out_message(snapshot)]
        }
        "API:change_volume" => change_volume(snapshot, &data, &mut commands),
        "API:mute" => mute_output(snapshot, &data, &mut commands),
        "GET_AUDIO_MIXER" => vec![mixer_message(snapshot)],
        "API:get_groups" => vec![groups_message(snapshot, &data)],
        "API:rearrange_groups" => rearrange_groups(snapshot, &data, &mut commands),
        "API:add_group" => add_group(snapshot, &data, &mut commands),
        "API:get_cleared" => vec![cleared_message(snapshot)],
        "SEARCH_SHOWS" => vec![search_shows_message(snapshot, &data)],
        "API:get_plain_text" => vec![plain_text_message(snapshot, &data)],
        "API:set_plain_text" => set_plain_text(snapshot, &data, &mut commands),
        "API:create_show" => create_show(snapshot, &data, &mut commands),
        "API:add_to_project" => add_to_project(snapshot, &data, &mut commands),
        "API:rename_project" => rename_project(snapshot, &data, &mut commands),
        "API:delete_project" => delete_project(snapshot, &data, &mut commands),
        "API:remove_project_item" => remove_project_item(snapshot, &data, &mut commands),
        "API:change_layout" => change_layout(snapshot, &data, &mut commands),
        "API:create_project" => create_project(snapshot, &data, &mut commands),
        "API:scripture_next" => step_scripture_message(snapshot, 1, &mut commands),
        "API:scripture_previous" => step_scripture_message(snapshot, -1, &mut commands),
        "GET_SCRIPTURE" => vec![scripture_message(snapshot, &data)],
        "SEARCH_SCRIPTURE" => vec![search_message(snapshot, &data)],
        "API:start_scripture" => {
            if let Some((reference, text)) = present_verse(snapshot, &data) {
                snapshot.cleared = false;
                snapshot.scripture_lines = vec![reference.clone(), text.clone()];
                commands.push(Command::PresentVerse { reference, text });
            }
            vec![out_message(snapshot)]
        }
        "LAYOUTS" => vec![json!({
            "channel": "LAYOUTS",
            "data": [{ "id": "default", "name": "Stage", "password": false }]
        })],
        "LAYOUT" => vec![
            json!({
                "channel": "LAYOUT",
                "data": {
                    "id": "default",
                    "name": "Stage",
                    "disabled": false,
                    "settings": { "color": "#12121c" },
                    "lines": display_lines(snapshot),
                    "nextLines": display_next(snapshot),
                    "notes": display_notes(snapshot)
                }
            }),
            out_message(snapshot),
        ],
        _ => vec![unknown()],
    };
    (replies, commands)
}

fn step(snapshot: &mut ShowSnapshot, delta: isize) -> Option<Command> {
    let len = snapshot.current_show()?.slides.len();
    if len == 0 {
        return None;
    }
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    let current = snapshot.slide_index.unwrap_or(0) as isize;
    let target = current + delta;
    if target >= 0 && (target as usize) < len {
        let next = target as usize;
        snapshot.slide_index = Some(next);
        return Some(Command::SetSlide(next));
    }
    if snapshot.next_item_on_last_slide
        && let Some(command) = open_project_neighbor(snapshot, delta)
    {
        return Some(command);
    }
    let next = target.clamp(0, len as isize - 1) as usize;
    snapshot.slide_index = Some(next);
    Some(Command::SetSlide(next))
}

fn open_project_neighbor(snapshot: &mut ShowSnapshot, delta: isize) -> Option<Command> {
    let project_id = snapshot.project_id.clone()?;
    let items = snapshot
        .projects
        .iter()
        .find(|project| project.id == project_id)?
        .shows
        .clone();
    if delta == 0 {
        return None;
    }
    let mut cursor = snapshot.project_item;
    loop {
        let next = if delta > 0 {
            items
                .iter()
                .enumerate()
                .skip(cursor.saturating_add(1))
                .find(|(_, item)| item.kind != "section")
                .map(|(index, _)| index)
        } else {
            items
                .iter()
                .enumerate()
                .take(cursor)
                .rev()
                .find(|(_, item)| item.kind != "section")
                .map(|(index, _)| index)
        };
        let index = next?;
        let item = items[index].clone();
        let opened = match item.kind.as_str() {
            "show" | "" => select_show(snapshot, &item.id).is_some(),
            "image" | "video" | "player" | "audio" | "folder" => true,
            "overlay" => snapshot.overlays.iter().any(|overlay| overlay.id == item.id),
            _ => false,
        };
        if opened && item.kind == "overlay" && !snapshot.active_overlays.iter().any(|active| active == &item.id)
        {
            snapshot.active_overlays.push(item.id.clone());
        }
        if opened {
            snapshot.project_item = index;
            let slide = match item.kind.as_str() {
                "show" | "" if delta < 0 => {
                    let last = snapshot
                        .current_show()
                        .map(|show| show.slides.len().saturating_sub(1))
                        .unwrap_or(0);
                    snapshot.slide_index = Some(last);
                    Some(last)
                }
                "show" | "" => snapshot.slide_index,
                _ => None,
            };
            return Some(Command::OpenProjectItem { index, slide });
        }
        if index == cursor {
            return None;
        }
        cursor = index;
    }
}

fn select_show(snapshot: &mut ShowSnapshot, id: &str) -> Option<Command> {
    let index = snapshot.shows.iter().position(|show| show.id == id)?;
    snapshot.show_index = Some(index);
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    snapshot.slide_index = if snapshot.shows[index].slides.is_empty() {
        None
    } else {
        Some(0)
    };
    Some(Command::SetShow(index))
}

fn set_slide(snapshot: &mut ShowSnapshot, index: usize) -> Option<Command> {
    let show = snapshot.current_show()?;
    if index >= show.slides.len() {
        return None;
    }
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    snapshot.slide_index = Some(index);
    Some(Command::SetSlide(index))
}

fn showing_scripture(snapshot: &ShowSnapshot) -> bool {
    !snapshot.scripture_lines.is_empty()
}

fn display_lines(snapshot: &ShowSnapshot) -> Vec<String> {
    if showing_scripture(snapshot) {
        snapshot.scripture_lines.clone()
    } else {
        snapshot.slide_lines()
    }
}

fn display_next(snapshot: &ShowSnapshot) -> Vec<String> {
    if showing_scripture(snapshot) {
        Vec::new()
    } else {
        snapshot.next_lines()
    }
}

fn display_notes(snapshot: &ShowSnapshot) -> String {
    if showing_scripture(snapshot) {
        String::new()
    } else {
        snapshot.notes()
    }
}

fn bible_by_id<'a>(snapshot: &'a ShowSnapshot, id: &str) -> Option<&'a RemoteBible> {
    snapshot
        .bibles
        .iter()
        .find(|bible| bible.id == id)
        .or_else(|| snapshot.bibles.first())
}

fn scripture_catalog(snapshot: &ShowSnapshot) -> Value {
    let mut data = serde_json::Map::new();
    for bible in snapshot.bibles.iter() {
        data.insert(
            bible.id.clone(),
            json!({
                "id": bible.id,
                "name": bible.name,
                "api": false
            }),
        );
    }
    json!({ "channel": "SCRIPTURE", "data": Value::Object(data) })
}

fn projects_message(snapshot: &ShowSnapshot) -> Value {
    let projects = snapshot
        .projects
        .iter()
        .map(|project| {
            json!({
                "id": project.id,
                "name": project.name,
                "parent": project.parent,
                "created": project.created,
                "shows": project.shows.iter().map(|item| json!({
                    "id": item.id,
                    "name": item.name,
                    "type": item.kind
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    json!({ "channel": "PROJECTS", "data": projects })
}

fn folders_message(snapshot: &ShowSnapshot) -> Value {
    let mut folders = serde_json::Map::new();
    for folder in &snapshot.folders {
        folders.insert(
            folder.id.clone(),
            json!({
                "name": folder.name,
                "parent": folder.parent
            }),
        );
    }
    json!({
        "channel": "FOLDERS",
        "data": { "folders": Value::Object(folders), "opened": [] }
    })
}

fn project_message(snapshot: &ShowSnapshot) -> Value {
    json!({ "channel": "PROJECT", "data": snapshot.project_id })
}

fn cleared_message(snapshot: &ShowSnapshot) -> Value {
    let cleared = snapshot.cleared;
    let overlays_cleared = snapshot.active_overlays.is_empty();
    json!({
        "channel": "API:get_cleared",
        "data": {
            "all": cleared && overlays_cleared,
            "background": cleared,
            "slide": cleared,
            "overlays": overlays_cleared,
            "audio": true,
            "slideTimers": true
        }
    })
}

fn out_data_message(snapshot: &ShowSnapshot) -> Value {
    json!({
        "channel": "OUT_DATA",
        "data": {
            "overlays": snapshot.active_overlays,
            "slide": if snapshot.cleared { Value::Null } else { json!(snapshot.slide_index) }
        }
    })
}

fn overlays_message(snapshot: &ShowSnapshot) -> Value {
    let mut overlays = serde_json::Map::new();
    for overlay in &snapshot.overlays {
        overlays.insert(
            overlay.id.clone(),
            json!({
                "name": overlay.name,
                "color": null,
                "category": overlay.category,
                "items": overlay.items
            }),
        );
    }
    json!({
        "channel": "GET_OVERLAYS",
        "data": { "overlays": Value::Object(overlays), "categories": {} }
    })
}

enum TimerAct {
    Start,
    Pause,
    Stop,
}

fn functions_message(snapshot: &ShowSnapshot) -> Value {
    let mut timers = serde_json::Map::new();
    for timer in &snapshot.timers {
        timers.insert(
            timer.id.clone(),
            json!({
                "name": timer.name,
                "type": "counter",
                "start": timer.start,
                "end": timer.end
            }),
        );
    }
    let active = snapshot
        .active_timer
        .as_ref()
        .map(|id| json!([{ "id": id, "paused": snapshot.timer_paused }]))
        .unwrap_or_else(|| json!([]));
    json!({
        "channel": "GET_FUNCTIONS",
        "data": {
            "timers": Value::Object(timers),
            "actions": {},
            "activeTimers": active
        }
    })
}

fn timer_command(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    act: TimerAct,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    if snapshot.timers.iter().any(|timer| timer.id == id) {
        match act {
            TimerAct::Start => {
                snapshot.active_timer = Some(id.to_string());
                snapshot.timer_paused = false;
                commands.push(Command::StartTimer { id: id.to_string() });
            }
            TimerAct::Pause => {
                if snapshot.active_timer.as_deref() == Some(id) {
                    snapshot.timer_paused = true;
                    commands.push(Command::PauseTimer { id: id.to_string() });
                }
            }
            TimerAct::Stop => {
                if snapshot.active_timer.as_deref() == Some(id) {
                    snapshot.active_timer = None;
                    snapshot.timer_paused = false;
                    commands.push(Command::StopTimer { id: id.to_string() });
                }
            }
        }
    }
    vec![functions_message(snapshot)]
}

fn templates_message(snapshot: &ShowSnapshot) -> Value {
    let mut templates = serde_json::Map::new();
    for template in &snapshot.templates {
        templates.insert(template.id.clone(), template.body.clone());
    }
    json!({
        "channel": "GET_TEMPLATES",
        "data": { "templates": Value::Object(templates), "categories": {} }
    })
}

fn set_template(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    if snapshot.templates.iter().any(|template| template.id == id) {
        commands.push(Command::SetTemplate {
            id: id.to_string(),
        });
    }
    vec![templates_message(snapshot)]
}

fn show_overlay(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    if snapshot.overlays.iter().any(|overlay| overlay.id == id)
        && !snapshot.active_overlays.iter().any(|active| active == id)
    {
        snapshot.active_overlays.push(id.to_string());
        commands.push(Command::ShowOverlay { id: id.to_string() });
    }
    vec![out_data_message(snapshot), cleared_message(snapshot)]
}

fn clear_one_overlay(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let before = snapshot.active_overlays.len();
    snapshot.active_overlays.retain(|active| active != id);
    if snapshot.active_overlays.len() != before {
        commands.push(Command::ClearOverlay { id: id.to_string() });
    }
    vec![out_data_message(snapshot), cleared_message(snapshot)]
}

fn media_message(snapshot: &ShowSnapshot) -> Value {
    let mut data = serde_json::Map::new();
    for file in snapshot.media.iter().chain(snapshot.audio.iter()) {
        data.insert(
            file.name.clone(),
            json!({
                "name": file.name,
                "path": file.name,
                "type": file.kind
            }),
        );
    }
    json!({ "channel": "MEDIA", "data": Value::Object(data) })
}

fn mixer_message(snapshot: &ShowSnapshot) -> Value {
    json!({
        "channel": "GET_AUDIO_MIXER",
        "data": {
            "audioRouting": { "channels": [{ "id": "main", "name": "Main" }] },
            "audioChannelsData": {
                "main": { "volume": snapshot.volume, "muted": snapshot.volume == 0.0 }
            }
        }
    })
}

fn seek_to(audio: bool, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let seconds = data
        .get("seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0);
    let millis = (seconds * 1000.0).round() as u64;
    if audio {
        commands.push(Command::SeekAudio { millis });
    } else {
        commands.push(Command::SeekVideo { millis });
    }
    Vec::new()
}

fn pause_audio(data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let name = data.get("path").and_then(Value::as_str).unwrap_or("");
    if !name.is_empty() {
        commands.push(Command::PauseAudio {
            name: name.to_string(),
        });
    }
    Vec::new()
}

fn play_named(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    background: bool,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let name = data.get("path").and_then(Value::as_str).unwrap_or("");
    if !name.is_empty() {
        snapshot.cleared = false;
        if background {
            let looping = data
                .pointer("/data/loop")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let muted = data
                .pointer("/data/muted")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            snapshot.media_loop = looping;
            snapshot.media_muted = muted;
            snapshot.media_paused = false;
            commands.push(Command::SetMediaMute { on: muted });
            commands.push(Command::SetMediaLoop { on: looping });
        }
        commands.push(Command::Play {
            name: name.to_string(),
        });
    }
    vec![out_message(snapshot), out_data_message(snapshot)]
}

fn toggle_media_mute(snapshot: &mut ShowSnapshot, commands: &mut Vec<Command>) -> Vec<Value> {
    snapshot.media_muted = !snapshot.media_muted;
    commands.push(Command::SetMediaMute {
        on: snapshot.media_muted,
    });
    vec![video_state_message(snapshot)]
}

fn toggle_media_loop(snapshot: &mut ShowSnapshot, commands: &mut Vec<Command>) -> Vec<Value> {
    snapshot.media_loop = !snapshot.media_loop;
    commands.push(Command::SetMediaLoop {
        on: snapshot.media_loop,
    });
    vec![video_state_message(snapshot)]
}

fn audio_data_message(snapshot: &ShowSnapshot) -> Value {
    let data = if snapshot.playing_audio.is_empty() {
        Value::Null
    } else {
        json!({
            "id": snapshot.playing_audio,
            "name": snapshot.playing_audio,
            "paused": snapshot.audio_paused,
            "isMic": false,
            "duration": snapshot.audio_duration
        })
    };
    json!({ "channel": "API:get_playing_audio_data", "data": data })
}

fn video_state_message(snapshot: &ShowSnapshot) -> Value {
    json!({
        "channel": "API:get_playing_video_state",
        "data": {
            "loop": snapshot.media_loop,
            "paused": snapshot.media_paused,
            "muted": snapshot.media_muted,
            "time": snapshot.video_time,
            "duration": snapshot.video_duration
        }
    })
}

fn change_volume(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let thousandths = gain_thousandths(data.get("volume").and_then(Value::as_f64).unwrap_or(1.0));
    snapshot.volume = f32::from(thousandths) / 1000.0;
    commands.push(Command::SetVolume { thousandths });
    vec![mixer_message(snapshot)]
}

fn mute_output(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let muted = data.get("value").and_then(Value::as_bool).unwrap_or(true);
    let thousandths = if muted { 0 } else { 1000 };
    snapshot.volume = f32::from(thousandths) / 1000.0;
    commands.push(Command::SetVolume { thousandths });
    vec![mixer_message(snapshot)]
}

fn gain_thousandths(volume: f64) -> u16 {
    let volume = if volume > 5.0 { volume / 100.0 } else { volume };
    (volume.clamp(0.0, 1.0) * 1000.0).round() as u16
}

fn move_grouped_slides(slides: &mut [RemoteSlide], from: usize, to: usize) -> bool {
    let positions: Vec<usize> = slides
        .iter()
        .enumerate()
        .filter(|(_, slide)| !slide.group.is_empty())
        .map(|(index, _)| index)
        .collect();
    if from >= positions.len() || to >= positions.len() || from == to {
        return false;
    }
    let mut grouped: Vec<RemoteSlide> = positions.iter().map(|index| slides[*index].clone()).collect();
    let slide = grouped.remove(from);
    grouped.insert(to, slide);
    for (index, slide) in positions.into_iter().zip(grouped) {
        slides[index] = slide;
    }
    true
}

fn groups_message(snapshot: &ShowSnapshot, data: &Value) -> Value {
    let requested = data.get("id").and_then(Value::as_str).unwrap_or("");
    let show = snapshot
        .shows
        .iter()
        .find(|show| show.id == requested)
        .or_else(|| snapshot.current_show());
    let mut groups: Vec<Value> = show
        .map(|show| {
            show.slides
                .iter()
                .filter(|slide| !slide.id.is_empty() && !slide.group.is_empty())
                .map(|slide| {
                    json!({
                        "id": slide.id,
                        "group": if slide.group.is_empty() { "—" } else { slide.group.as_str() },
                        "color": null
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    groups.sort_by(|left, right| {
        left["group"]
            .as_str()
            .unwrap_or("")
            .cmp(right["group"].as_str().unwrap_or(""))
    });
    json!({
        "channel": "API:get_groups",
        "data": {
            "id": show.map(|show| show.id.clone()).unwrap_or_default(),
            "value": groups
        }
    })
}

fn rearrange_groups(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let show_id = data
        .get("showId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let from = data.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
    let to = data.get("to").and_then(Value::as_u64).unwrap_or(0) as usize;
    if let Some(show) = snapshot.shows.iter_mut().find(|show| show.id == show_id)
        && move_grouped_slides(&mut show.slides, from, to)
    {
        commands.push(Command::RearrangeGroups {
            show_id: show_id.clone(),
            from,
            to,
        });
    }
    vec![groups_message(snapshot, &json!({ "id": show_id }))]
}

fn add_group(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let show_id = data
        .get("showId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let group_id = data
        .get("groupId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(show) = snapshot.shows.iter_mut().find(|show| show.id == show_id)
        && let Some(index) = show.slides.iter().position(|slide| slide.id == group_id)
    {
        let mut copy = show.slides[index].clone();
        copy.id = format!("{group_id}-copy");
        show.slides.push(copy);
        commands.push(Command::AddGroup {
            show_id: show_id.clone(),
            group_id,
        });
    }
    vec![groups_message(snapshot, &json!({ "id": show_id }))]
}

fn search_shows_message(snapshot: &ShowSnapshot, data: &Value) -> Value {
    let query = data.get("query").and_then(Value::as_str).unwrap_or("");
    let limit = data
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(500) as usize;
    let needle = query.trim().to_lowercase();
    let results = if needle.chars().count() < 2 {
        Vec::new()
    } else {
        snapshot
            .shows
            .iter()
            .filter(|show| {
                show.name.to_lowercase().contains(&needle)
                    || show.slides.iter().any(|slide| {
                        slide
                            .lines
                            .iter()
                            .any(|line| line.to_lowercase().contains(&needle))
                    })
            })
            .take(limit)
            .map(|show| json!({ "id": show.id, "match": 1 }))
            .collect()
    };
    json!({
        "channel": "SEARCH_SHOWS",
        "data": { "query": query, "results": results }
    })
}

fn plain_text_message(snapshot: &ShowSnapshot, data: &Value) -> Value {
    let requested = data.get("id").and_then(Value::as_str).unwrap_or("");
    let show = snapshot
        .shows
        .iter()
        .find(|show| show.id == requested)
        .or_else(|| snapshot.current_show());
    let id = show.map(|show| show.id.clone()).unwrap_or_default();
    let value = show.map(show_plain_text).unwrap_or_default();
    json!({
        "channel": "API:get_plain_text",
        "data": { "id": id, "value": value }
    })
}

fn show_plain_text(show: &RemoteShow) -> String {
    show.slides
        .iter()
        .map(|slide| slide.lines.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn slides_from_text(text: &str) -> Vec<RemoteSlide> {
    let chunks: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect();
    let chunks: Vec<&str> = if chunks.is_empty() { vec![""] } else { chunks };
    chunks
        .into_iter()
        .map(|chunk| {
            let lines: Vec<String> = if chunk.is_empty() {
                Vec::new()
            } else {
                chunk.split('\n').map(str::to_string).collect()
            };
            let label = lines.first().cloned().unwrap_or_default();
            RemoteSlide {
                label,
                lines,
                notes: String::new(),
                id: String::new(),
                group: String::new(),
            }
        })
        .collect()
}

fn set_plain_text(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let requested = data.get("id").and_then(Value::as_str).unwrap_or("");
    let text = data.get("value").and_then(Value::as_str).unwrap_or("");
    let index = snapshot
        .shows
        .iter()
        .position(|show| show.id == requested)
        .or(snapshot.show_index);
    let Some(index) = index else {
        return vec![plain_text_message(snapshot, data)];
    };
    let id = snapshot.shows[index].id.clone();
    let previous: Vec<(String, String)> = snapshot.shows[index]
        .slides
        .iter()
        .map(|slide| (slide.id.clone(), slide.group.clone()))
        .collect();
    let mut slides = slides_from_text(text);
    for (slide, (old_id, group)) in slides.iter_mut().zip(previous) {
        slide.id = old_id;
        slide.group = group;
    }
    let active = snapshot.shows[index].active_layout.clone();
    let layout_at = snapshot.shows[index]
        .layouts
        .iter()
        .position(|layout| layout.id == active)
        .or_else(|| {
            if snapshot.shows[index].layouts.is_empty() {
                None
            } else {
                Some(0)
            }
        });
    if let Some(at) = layout_at {
        snapshot.shows[index].layouts[at].slides.clone_from(&slides);
    }
    snapshot.shows[index].slides = slides;
    snapshot.show_index = Some(index);
    snapshot.slide_index = Some(0);
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    commands.push(Command::SetShowText {
        id,
        text: text.to_string(),
    });
    vec![show_message(snapshot), out_message(snapshot)]
}

fn create_show(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Untitled")
        .to_string();
    let text = data.get("text").and_then(Value::as_str).unwrap_or("");
    let id = new_remote_id();
    let slides = slides_from_text(text);
    snapshot.shows.push(RemoteShow {
        id: id.clone(),
        name: name.clone(),
        layouts: vec![RemoteLayout {
            id: "main".into(),
            name: "Default".into(),
            slides: slides.clone(),
        }],
        active_layout: "main".into(),
        slides,
    });
    snapshot.show_index = Some(snapshot.shows.len() - 1);
    snapshot.slide_index = Some(0);
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    if let Some(project_id) = snapshot.project_id.clone()
        && let Some(project) = snapshot.projects.iter_mut().find(|project| project.id == project_id)
    {
        project.shows.push(RemoteProjectItem {
            id: id.clone(),
            name: name.clone(),
            kind: "show".into(),
        });
    }
    commands.push(Command::CreateShow {
        id: id.clone(),
        name: name.clone(),
        text: text.to_string(),
    });
    vec![
        json!({
            "channel": "API:create_show",
            "data": {
                "id": id.clone(),
                "show": { "id": id, "name": name }
            }
        }),
        show_message(snapshot),
        out_message(snapshot),
    ]
}

fn add_to_project(snapshot: &mut ShowSnapshot, data: &Value, commands: &mut Vec<Command>) -> Vec<Value> {
    let project_id = data
        .get("projectId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| snapshot.project_id.clone())
        .unwrap_or_default();
    let show_id = data.get("id").and_then(Value::as_str).unwrap_or("");
    if project_id.is_empty() || show_id.is_empty() {
        return vec![projects_message(snapshot)];
    }
    let name = snapshot
        .shows
        .iter()
        .find(|show| show.id == show_id)
        .map(|show| show.name.clone())
        .unwrap_or_default();
    if let Some(project) = snapshot
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
    {
        project.shows.push(RemoteProjectItem {
            id: show_id.to_string(),
            name,
            kind: "show".into(),
        });
        commands.push(Command::AddToProject {
            project_id,
            show_id: show_id.to_string(),
        });
    }
    vec![projects_message(snapshot)]
}

fn rename_project(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let project_id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let name = data.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if !project_id.is_empty()
        && !name.is_empty()
        && let Some(project) = snapshot
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
    {
        project.name = name.to_string();
        commands.push(Command::RenameProject {
            project_id: project_id.to_string(),
            name: name.to_string(),
        });
    }
    vec![projects_message(snapshot)]
}

fn delete_project(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let project_id = data.get("id").and_then(Value::as_str).unwrap_or("");
    if let Some(index) = snapshot
        .projects
        .iter()
        .position(|project| project.id == project_id)
    {
        snapshot.projects.remove(index);
        if snapshot.project_id.as_deref() == Some(project_id) {
            snapshot.project_id = snapshot.projects.first().map(|project| project.id.clone());
        }
        commands.push(Command::DeleteProject {
            project_id: project_id.to_string(),
        });
    }
    vec![projects_message(snapshot), project_message(snapshot)]
}

fn remove_project_item(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let project_id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let Some(index) = data.get("index").and_then(Value::as_u64) else {
        return vec![projects_message(snapshot)];
    };
    let index = index as usize;
    if let Some(project) = snapshot
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
        && index < project.shows.len()
    {
        project.shows.remove(index);
        commands.push(Command::RemoveProjectItem {
            project_id: project_id.to_string(),
            index,
        });
    }
    vec![projects_message(snapshot)]
}

fn step_scripture_message(
    snapshot: &mut ShowSnapshot,
    delta: isize,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    if let Some((reference, text)) = adjacent_verse(snapshot, delta) {
        snapshot.cleared = false;
        snapshot.scripture_lines = vec![reference.clone(), text.clone()];
        commands.push(Command::PresentVerse { reference, text });
    }
    vec![out_message(snapshot)]
}

fn adjacent_verse(snapshot: &ShowSnapshot, delta: isize) -> Option<(String, String)> {
    let current = snapshot.scripture_lines.first().cloned().unwrap_or_default();
    let mut flat = Vec::new();
    for bible in snapshot.bibles.iter() {
        for book in &bible.books {
            for chapter in &book.chapters {
                for verse in &chapter.verses {
                    flat.push((
                        format!("{} {}:{}", book.name, chapter.number, verse.number),
                        verse.text.clone(),
                    ));
                }
            }
        }
    }
    if flat.is_empty() {
        return None;
    }
    let next = match flat.iter().position(|(reference, _)| reference == &current) {
        Some(index) => {
            let next = index as isize + delta;
            if next < 0 || next >= flat.len() as isize {
                return None;
            }
            next as usize
        }
        None if delta > 0 => 0,
        None => return None,
    };
    Some(flat[next].clone())
}

fn new_remote_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let tick = NEXT.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("remote{millis:x}{tick:x}")
}

fn scripture_message(snapshot: &ShowSnapshot, data: &Value) -> Value {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let Some(bible) = bible_by_id(snapshot, id) else {
        return json!({ "channel": "GET_SCRIPTURE", "data": { "id": id } });
    };
    let book_index = data.get("bookIndex").and_then(Value::as_u64);
    let chapter_index = data.get("chapterIndex").and_then(Value::as_u64);
    if let (Some(book_index), Some(chapter_index)) = (book_index, chapter_index) {
        let verses = bible
            .books
            .get(book_index as usize)
            .and_then(|book| book.chapters.get(chapter_index as usize))
            .map(|chapter| {
                chapter
                    .verses
                    .iter()
                    .map(|verse| {
                        json!({
                            "number": verse.number,
                            "text": verse.text,
                            "keyName": verse.number.to_string()
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return json!({
            "channel": "GET_SCRIPTURE",
            "data": {
                "id": bible.id,
                "bibleUpdate": {
                    "kind": "verses",
                    "id": bible.id,
                    "bookIndex": book_index,
                    "chapterIndex": chapter_index,
                    "verses": verses
                }
            }
        });
    }
    if let Some(book_index) = book_index {
        let chapters = bible
            .books
            .get(book_index as usize)
            .map(|book| {
                book.chapters
                    .iter()
                    .map(|chapter| {
                        json!({
                            "number": chapter.number,
                            "keyName": chapter.number.to_string()
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return json!({
            "channel": "GET_SCRIPTURE",
            "data": {
                "id": bible.id,
                "bibleUpdate": {
                    "kind": "chapters",
                    "id": bible.id,
                    "bookIndex": book_index,
                    "chapters": chapters
                }
            }
        });
    }
    let books = bible
        .books
        .iter()
        .map(|book| {
            json!({
                "name": book.name,
                "number": book.number,
                "keyName": book.number.to_string(),
                "chapters": book.chapters.iter().map(|chapter| json!({
                    "number": chapter.number,
                    "keyName": chapter.number.to_string(),
                    "verses": chapter.verses.iter().map(|verse| json!({
                        "number": verse.number,
                        "text": verse.text
                    })).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "channel": "GET_SCRIPTURE",
        "data": {
            "id": bible.id,
            "bible": { "name": bible.name, "books": books }
        }
    })
}

fn search_message(snapshot: &ShowSnapshot, data: &Value) -> Value {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let term = data.get("searchTerm").and_then(Value::as_str).unwrap_or("");
    let results = bible_by_id(snapshot, id)
        .map(|bible| scripture_hits(bible, term))
        .unwrap_or_default()
        .into_iter()
        .map(|(reference, text)| {
            json!({
                "reference": reference,
                "referenceFull": reference,
                "verseText": text
            })
        })
        .collect::<Vec<_>>();
    json!({
        "channel": "SEARCH_SCRIPTURE",
        "data": {
            "id": id,
            "searchTerm": term,
            "searchResults": { "type": "text", "results": results }
        }
    })
}

fn present_verse(snapshot: &ShowSnapshot, data: &Value) -> Option<(String, String)> {
    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let reference = data.get("reference").and_then(Value::as_str)?;
    let bible = bible_by_id(snapshot, id)?;
    dotted_hit(bible, reference).or_else(|| named_hit(bible, reference))
}

fn dotted_hit(bible: &RemoteBible, reference: &str) -> Option<(String, String)> {
    let mut parts = reference.split('.');
    let book_number: u64 = parts.next()?.parse().ok()?;
    let chapter_number: u64 = parts.next()?.parse().ok()?;
    let verse_number: u64 = parts.next()?.split('-').next()?.parse().ok()?;
    let book = bible
        .books
        .iter()
        .find(|book| book.number == book_number)?;
    verse_text(book, chapter_number, verse_number)
}

fn named_hit(bible: &RemoteBible, reference: &str) -> Option<(String, String)> {
    let (book_name, rest) = reference.trim().rsplit_once(' ')?;
    let (chapter, verse) = rest.split_once(':')?;
    let chapter_number: u64 = chapter.parse().ok()?;
    let verse_number: u64 = verse.split('-').next()?.parse().ok()?;
    let book = bible.books.iter().find(|book| {
        book.name.eq_ignore_ascii_case(book_name.trim())
    })?;
    verse_text(book, chapter_number, verse_number)
}

fn verse_text(book: &RemoteBook, chapter_number: u64, verse_number: u64) -> Option<(String, String)> {
    let chapter = book
        .chapters
        .iter()
        .find(|chapter| chapter.number == chapter_number)?;
    let verse = chapter
        .verses
        .iter()
        .find(|verse| verse.number == verse_number)?;
    Some((
        format!("{} {chapter_number}:{verse_number}", book.name),
        verse.text.clone(),
    ))
}

fn scripture_hits(bible: &RemoteBible, term: &str) -> Vec<(String, String)> {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(hit) = named_hit(bible, trimmed) {
        return vec![hit];
    }
    let needle = trimmed.to_lowercase();
    let mut hits = Vec::new();
    for book in &bible.books {
        for chapter in &book.chapters {
            for verse in &chapter.verses {
                if verse.text.to_lowercase().contains(&needle) {
                    hits.push((
                        format!("{} {}:{}", book.name, chapter.number, verse.number),
                        verse.text.clone(),
                    ));
                    if hits.len() == 40 {
                        return hits;
                    }
                }
            }
        }
    }
    hits
}

fn shows_message(snapshot: &ShowSnapshot) -> Value {
    let mut shows = serde_json::Map::new();
    for show in &snapshot.shows {
        shows.insert(show.id.clone(), json!({ "name": show.name }));
    }
    json!({ "channel": "SHOWS", "data": Value::Object(shows) })
}

fn show_message(snapshot: &ShowSnapshot) -> Value {
    let data = snapshot.current_show().map(show_document);
    json!({ "channel": "SHOW", "data": data })
}

fn show_document(show: &RemoteShow) -> Value {
    let layouts = if show.layouts.is_empty() {
        vec![RemoteLayout {
            id: active_layout_id(show),
            name: "Default".into(),
            slides: show.slides.clone(),
        }]
    } else {
        show.layouts.clone()
    };
    let mut slides = serde_json::Map::new();
    let mut layout_map = serde_json::Map::new();
    for layout in &layouts {
        let mut order = Vec::new();
        for (index, slide) in layout.slides.iter().enumerate() {
            let id = if slide.id.is_empty() {
                format!("s{}", index + 1)
            } else {
                slide.id.clone()
            };
            slides.entry(id.clone()).or_insert_with(|| {
                json!({
                    "group": slide.group,
                    "color": null,
                    "settings": {},
                    "notes": slide.notes,
                    "items": [{
                        "lines": slide.lines.iter().map(|line| json!({
                            "align": "",
                            "text": [{ "value": line, "style": "" }]
                        })).collect::<Vec<_>>()
                    }]
                })
            });
            order.push(json!({ "id": id }));
        }
        layout_map.insert(
            layout.id.clone(),
            json!({
                "name": layout.name,
                "notes": "",
                "slides": order
            }),
        );
    }
    json!({
        "id": show.id,
        "name": show.name,
        "settings": { "activeLayout": active_layout_id(show), "template": null },
        "slides": slides,
        "layouts": layout_map
    })
}

fn active_layout_id(show: &RemoteShow) -> String {
    if !show.active_layout.is_empty() {
        show.active_layout.clone()
    } else {
        show.layouts
            .first()
            .map(|layout| layout.id.clone())
            .unwrap_or_else(|| "main".into())
    }
}

fn change_layout(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let show_id = data.get("showId").and_then(Value::as_str).unwrap_or("");
    let layout_id = data.get("layoutId").and_then(Value::as_str).unwrap_or("");
    let Some(index) = snapshot.shows.iter().position(|show| show.id == show_id) else {
        return vec![show_message(snapshot), out_message(snapshot)];
    };
    let Some(layout_at) = snapshot.shows[index]
        .layouts
        .iter()
        .position(|layout| layout.id == layout_id)
    else {
        return vec![show_message(snapshot), out_message(snapshot)];
    };
    let slides = snapshot.shows[index].layouts[layout_at].slides.clone();
    snapshot.shows[index].slides = slides;
    snapshot.shows[index].active_layout = layout_id.to_string();
    snapshot.show_index = Some(index);
    snapshot.slide_index = if snapshot.shows[index].slides.is_empty() {
        None
    } else {
        Some(0)
    };
    snapshot.cleared = false;
    snapshot.scripture_lines.clear();
    commands.push(Command::ChangeLayout {
        show_id: show_id.to_string(),
        layout_id: layout_id.to_string(),
    });
    vec![show_message(snapshot), out_message(snapshot)]
}

fn create_project(
    snapshot: &mut ShowSnapshot,
    data: &Value,
    commands: &mut Vec<Command>,
) -> Vec<Value> {
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return vec![projects_message(snapshot), project_message(snapshot)];
    }
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(new_remote_id);
    if snapshot.projects.iter().any(|project| project.id == id) {
        return vec![projects_message(snapshot), project_message(snapshot)];
    }
    let parent = snapshot
        .project_id
        .as_ref()
        .and_then(|active| snapshot.projects.iter().find(|project| &project.id == active))
        .map(|project| project.parent.clone())
        .filter(|parent| parent != "/" && snapshot.folders.iter().any(|folder| &folder.id == parent))
        .unwrap_or_else(|| "/".into());
    snapshot.projects.push(RemoteProject {
        id: id.clone(),
        name: name.clone(),
        parent: parent.clone(),
        created: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
        shows: Vec::new(),
    });
    snapshot.project_id = Some(id.clone());
    commands.push(Command::CreateProject { id, name, parent });
    vec![projects_message(snapshot), project_message(snapshot)]
}

fn out_message(snapshot: &ShowSnapshot) -> Value {
    if snapshot.cleared {
        return json!({
            "channel": "OUT",
            "data": {
                "slide": null,
                "layout": "main",
                "lines": [],
                "nextLines": [],
                "notes": "",
                "show": null
            }
        });
    }
    let layout = snapshot
        .current_show()
        .map(active_layout_id)
        .unwrap_or_else(|| "main".into());
    let show = snapshot.current_show().map(show_document);
    json!({
        "channel": "OUT",
        "data": {
            "slide": snapshot.slide_index,
            "layout": layout,
            "lines": display_lines(snapshot),
            "nextLines": display_next(snapshot),
            "notes": display_notes(snapshot),
            "show": show
        }
    })
}

fn unknown() -> Value {
    json!({ "channel": "ERROR", "data": { "key": "unknown" } })
}

fn index_of(data: &Value) -> Option<usize> {
    data.get("index")
        .and_then(Value::as_u64)
        .or_else(|| data.as_u64())
        .map(|index| index as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ShowSnapshot {
        ShowSnapshot {
            shows: vec![
                RemoteShow {
                    id: "grace".into(),
                    name: "Amazing Grace".into(),
                    layouts: Vec::new(),
                    active_layout: String::new(),
                    slides: vec![
                        RemoteSlide {
                            label: "Amazing grace".into(),
                            lines: vec!["Amazing grace".into()],
                            notes: "Stand".into(),
                            id: String::new(),
                            group: String::new(),
                        },
                        RemoteSlide {
                            label: "how sweet".into(),
                            lines: vec!["how sweet".into()],
                            notes: String::new(),
                id: String::new(),
                group: String::new(),
                        },
                        RemoteSlide {
                            label: "the sound".into(),
                            lines: vec!["the sound".into()],
                            notes: String::new(),
                id: String::new(),
                group: String::new(),
                        },
                    ],
                },
                RemoteShow {
                    id: "other".into(),
                    name: "Other Song".into(),
                    layouts: Vec::new(),
                    active_layout: String::new(),
                    slides: vec![RemoteSlide {
                        label: "Hello".into(),
                        lines: vec!["Hello".into()],
                        notes: String::new(),
                id: String::new(),
                group: String::new(),
                    }],
                },
            ],
            show_index: Some(0),
            slide_index: Some(0),
            cleared: false,
            project: vec!["Amazing Grace".into()],
            projects: Vec::new(),
            project_id: None,
            folders: Vec::new(),
            bibles: Arc::new(Vec::new()),
            scripture_lines: Vec::new(),
            overlays: Vec::new(),
            active_overlays: Vec::new(),
            media: Vec::new(),
            audio: Vec::new(),
            templates: Vec::new(),
            timers: Vec::new(),
            active_timer: None,
            timer_paused: false,
            media_loop: true,
            media_paused: false,
            media_muted: true,
            video_time: 0.0,
            video_duration: 0.0,
            playing_audio: String::new(),
            audio_time: 0.0,
            audio_duration: 0.0,
            audio_paused: false,
            volume: 1.0,
            project_item: 0,
            next_item_on_last_slide: true,
        }
    }

    fn john() -> RemoteBible {
        RemoteBible {
            id: "kjv".into(),
            name: "KJV".into(),
            books: vec![RemoteBook {
                number: 43,
                name: "John".into(),
                chapters: vec![RemoteChapter {
                    number: 3,
                    verses: vec![
                        RemoteVerse {
                            number: 16,
                            text: "For God so loved the world".into(),
                        },
                        RemoteVerse {
                            number: 17,
                            text: "For God sent not his Son".into(),
                        },
                    ],
                }],
            }],
        }
    }

    #[test]
    fn the_remote_can_open_a_bible_and_present_a_verse() {
        let mut snapshot = sample();
        snapshot.bibles = Arc::new(vec![john()]);
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "GET_SCRIPTURE",
                "data": { "id": "kjv" }
            }),
        );
        assert_eq!(replies[0]["data"]["bible"]["books"][0]["name"], "John");
        assert_eq!(
            replies[0]["data"]["bible"]["books"][0]["chapters"][0]["verses"][0]["text"],
            "For God so loved the world"
        );

        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "GET_SCRIPTURE",
                "data": { "id": "kjv", "bookIndex": 0, "chapterIndex": 0 }
            }),
        );
        assert_eq!(replies[0]["data"]["bibleUpdate"]["kind"], "verses");
        assert_eq!(
            replies[0]["data"]["bibleUpdate"]["verses"][1]["text"],
            "For God sent not his Son"
        );

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:start_scripture",
                "data": { "id": "kjv", "reference": "43.3.16" }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::PresentVerse {
                reference: "John 3:16".into(),
                text: "For God so loved the world".into(),
            }]
        );
        assert_eq!(replies[0]["channel"], "OUT");
        assert_eq!(replies[0]["data"]["lines"][1], "For God so loved the world");

        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "SEARCH_SCRIPTURE",
                "data": { "id": "kjv", "searchTerm": "John 3:17" }
            }),
        );
        assert_eq!(
            replies[0]["data"]["searchResults"]["results"][0]["verseText"],
            "For God sent not his Son"
        );

        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "PASSWORD" }),
        );
        assert_eq!(replies[4]["channel"], "SCRIPTURE");
        assert_eq!(replies[4]["data"]["kjv"]["name"], "KJV");
    }

    #[test]
    fn the_remote_can_run_the_service_order() {
        let mut snapshot = sample();
        snapshot.projects = vec![RemoteProject {
            id: "sunday".into(),
            name: "Sunday".into(),
            parent: "/".into(),
            created: 10,
            shows: vec![
                RemoteProjectItem {
                    id: "welcome".into(),
                    name: "Welcome".into(),
                    kind: "section".into(),
                },
                RemoteProjectItem {
                    id: "grace".into(),
                    name: "Amazing Grace".into(),
                    kind: "show".into(),
                },
            ],
        }];
        snapshot.project_id = Some("sunday".into());
        snapshot.folders = vec![RemoteFolder {
            id: "songs".into(),
            name: "Songs".into(),
            parent: "/".into(),
        }];
        snapshot.bibles = Arc::new(vec![john()]);
        snapshot.scripture_lines = vec!["John 3:16".into(), "For God so loved the world".into()];

        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "ACCESS" }));
        assert_eq!(replies[3]["channel"], "SCRIPTURE");
        assert_eq!(replies[4]["channel"], "PROJECTS");
        assert_eq!(replies[4]["data"][0]["shows"][0]["type"], "section");
        assert_eq!(replies[4]["data"][0]["shows"][1]["id"], "grace");
        assert_eq!(replies[5]["data"]["folders"]["songs"]["name"], "Songs");
        assert_eq!(replies[6]["data"], "sunday");

        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "SEARCH_SHOWS", "data": { "query": "sweet" } }),
        );
        assert_eq!(replies[0]["data"]["results"][0]["id"], "grace");

        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_plain_text", "data": { "id": "grace" } }),
        );
        assert_eq!(
            replies[0]["data"]["value"],
            "Amazing grace\n\nhow sweet\n\nthe sound"
        );

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:set_plain_text",
                "data": { "id": "grace", "value": "Verse one\n\nChorus" }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::SetShowText {
                id: "grace".into(),
                text: "Verse one\n\nChorus".into(),
            }]
        );
        assert_eq!(replies[1]["data"]["lines"][0], "Verse one");

        snapshot.scripture_lines = vec!["John 3:16".into(), "For God so loved the world".into()];
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:scripture_next" }),
        );
        assert_eq!(replies[0]["data"]["lines"][0], "John 3:17");
        assert!(matches!(commands[0], Command::PresentVerse { .. }));

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:create_show",
                "data": { "name": "Call to worship", "text": "Come\n\nSing" }
            }),
        );
        assert_eq!(replies[0]["channel"], "API:create_show");
        assert_eq!(replies[0]["data"]["show"]["name"], "Call to worship");
        assert!(matches!(commands[0], Command::CreateShow { .. }));
        assert_eq!(snapshot.projects[0].shows.last().unwrap().name, "Call to worship");

        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "API:get_cleared" }));
        assert_eq!(replies[0]["data"]["slide"], false);
    }

    #[test]
    fn the_remote_puts_an_overlay_on_the_output() {
        let mut snapshot = sample();
        snapshot.overlays = vec![RemoteOverlay {
            id: "logo".into(),
            name: "Logo".into(),
            category: None,
            items: json!([{ "lines": [{ "text": [{ "value": "LIVE" }] }] }]),
        }];
        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "GET_OVERLAYS" }));
        assert_eq!(replies[0]["data"]["overlays"]["logo"]["name"], "Logo");
        assert_eq!(
            replies[0]["data"]["overlays"]["logo"]["items"][0]["lines"][0]["text"][0]["value"],
            "LIVE"
        );

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:id_select_overlay", "data": { "id": "logo" } }),
        );
        assert_eq!(
            commands,
            vec![Command::ShowOverlay { id: "logo".into() }]
        );
        assert_eq!(replies[0]["channel"], "OUT_DATA");
        assert_eq!(replies[0]["data"]["overlays"][0], "logo");
        assert_eq!(replies[1]["data"]["overlays"], false);

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:clear_overlay", "data": { "id": "logo" } }),
        );
        assert_eq!(
            commands,
            vec![Command::ClearOverlay { id: "logo".into() }]
        );
        assert_eq!(replies[1]["data"]["overlays"], true);

        snapshot.active_overlays.push("logo".into());
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:clear_all" }));
        assert_eq!(commands, vec![Command::Clear]);
        assert!(snapshot.active_overlays.is_empty());
    }

    #[test]
    fn the_remote_can_play_a_file_and_set_the_volume() {
        let mut snapshot = sample();
        snapshot.media = vec![RemoteFile {
            name: "clip.mp4".into(),
            kind: "video".into(),
        }];
        snapshot.audio = vec![RemoteFile {
            name: "song.mp3".into(),
            kind: "audio".into(),
        }];
        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "ACCESS" }));
        assert_eq!(replies.last().unwrap()["channel"], "MEDIA");
        assert_eq!(replies.last().unwrap()["data"]["clip.mp4"]["type"], "video");

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:play_media", "data": { "path": "clip.mp4" } }),
        );
        assert_eq!(
            commands,
            vec![
                Command::SetMediaMute { on: true },
                Command::SetMediaLoop { on: true },
                Command::Play {
                    name: "clip.mp4".into()
                }
            ]
        );
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:play_media",
                "data": { "path": "clip.mp4", "data": { "loop": false } }
            }),
        );
        assert_eq!(commands[0], Command::SetMediaMute { on: true });
        assert_eq!(commands[1], Command::SetMediaLoop { on: false });
        assert!(!snapshot.media_loop);
        assert!(snapshot.media_muted);
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:toggle_media_mute" }),
        );
        assert_eq!(commands, vec![Command::SetMediaMute { on: false }]);
        assert_eq!(replies[0]["data"]["muted"], false);
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:toggle_media_loop" }),
        );
        assert_eq!(commands, vec![Command::SetMediaLoop { on: true }]);
        assert_eq!(replies[0]["data"]["loop"], true);
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_media_loop_state" }),
        );
        assert_eq!(replies[0]["data"], true);
        snapshot.video_time = 4.5;
        snapshot.video_duration = 12.0;
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_playing_video_state" }),
        );
        assert_eq!(replies[0]["data"]["time"], 4.5);
        assert_eq!(replies[0]["data"]["duration"], 12.0);
        snapshot.playing_audio = "song.mp3".into();
        snapshot.audio_paused = true;
        snapshot.audio_time = 2.5;
        snapshot.audio_duration = 8.0;
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_playing_audio_data" }),
        );
        assert_eq!(replies[0]["data"]["id"], "song.mp3");
        assert_eq!(replies[0]["data"]["paused"], true);
        assert_eq!(replies[0]["data"]["duration"], 8.0);
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_playing_audio_time" }),
        );
        assert_eq!(replies[0]["data"], 2.5);
        snapshot.playing_audio.clear();
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_playing_audio_data" }),
        );
        assert!(replies[0]["data"].is_null());

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:change_volume", "data": { "volume": 0.5 } }),
        );
        assert_eq!(commands, vec![Command::SetVolume { thousandths: 500 }]);
        assert_eq!(replies[0]["data"]["audioChannelsData"]["main"]["volume"], 0.5);

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:change_volume", "data": { "volume": 40.0 } }),
        );
        assert_eq!(commands, vec![Command::SetVolume { thousandths: 400 }]);
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:pause_audio", "data": { "path": "song.wav" } }),
        );
        assert_eq!(
            commands,
            vec![Command::PauseAudio {
                name: "song.wav".into()
            }]
        );
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:audio_seekto", "data": { "seconds": 12.5 } }),
        );
        assert_eq!(commands, vec![Command::SeekAudio { millis: 12_500 }]);
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:video_seekto", "data": { "seconds": 3.0 } }),
        );
        assert_eq!(commands, vec![Command::SeekVideo { millis: 3_000 }]);
    }

    #[test]
    fn the_remote_can_list_and_apply_a_template() {
        let mut snapshot = sample();
        snapshot.templates = vec![RemoteTemplate {
            id: "big".into(),
            name: "Big".into(),
            body: json!({ "name": "Big", "items": [] }),
        }];
        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "GET_TEMPLATES" }));
        assert_eq!(replies[0]["data"]["templates"]["big"]["name"], "Big");
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:set_template", "data": { "id": "big" } }),
        );
        assert_eq!(
            commands,
            vec![Command::SetTemplate { id: "big".into() }]
        );
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:set_template", "data": { "id": "missing" } }),
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn the_remote_can_start_pause_and_stop_a_timer() {
        let mut snapshot = sample();
        snapshot.timers = vec![RemoteTimer {
            id: "default".into(),
            name: "05:00".into(),
            start: 300,
            end: 0,
        }];
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:id_start_timer", "data": { "id": "default" } }),
        );
        assert_eq!(
            commands,
            vec![Command::StartTimer {
                id: "default".into()
            }]
        );
        assert_eq!(replies[0]["data"]["timers"]["default"]["start"], 300);
        assert_eq!(replies[0]["data"]["activeTimers"][0]["paused"], false);

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:id_pause_timer", "data": { "id": "default" } }),
        );
        assert_eq!(
            commands,
            vec![Command::PauseTimer {
                id: "default".into()
            }]
        );
        assert!(snapshot.timer_paused);

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:id_stop_timer", "data": { "id": "default" } }),
        );
        assert_eq!(
            commands,
            vec![Command::StopTimer {
                id: "default".into()
            }]
        );
        assert!(snapshot.active_timer.is_none());
    }

    #[test]
    fn the_remote_can_list_and_reorder_groups() {
        let mut snapshot = sample();
        snapshot.shows[0].slides[0].id = "s1".into();
        snapshot.shows[0].slides[0].group = "Verse".into();
        snapshot.shows[0].slides[1].id = "s2".into();
        snapshot.shows[0].slides[1].group = "Chorus".into();
        snapshot.shows[0].slides[2].id = "s3".into();
        let (replies, _) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:get_groups", "data": { "id": "grace" } }),
        );
        assert_eq!(replies[0]["data"]["value"][0]["group"], "Chorus");
        assert_eq!(replies[0]["data"]["value"][1]["group"], "Verse");

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:rearrange_groups",
                "data": { "showId": "grace", "from": 0, "to": 1 }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::RearrangeGroups {
                show_id: "grace".into(),
                from: 0,
                to: 1
            }]
        );
        assert_eq!(snapshot.shows[0].slides[0].group, "Chorus");
        assert_eq!(snapshot.shows[0].slides[1].group, "Verse");

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:add_group",
                "data": { "showId": "grace", "groupId": "s2" }
            }),
        );
        assert!(matches!(
            commands[0],
            Command::AddGroup { .. }
        ));
        assert_eq!(snapshot.shows[0].slides.last().unwrap().group, "Chorus");
    }

    #[test]
    fn the_remote_can_rename_and_delete_a_project() {
        let mut snapshot = sample();
        snapshot.project_id = Some("sunday".into());
        snapshot.projects = vec![
            RemoteProject {
                id: "sunday".into(),
                name: "Sunday".into(),
                parent: "/".into(),
                created: 1,
                shows: vec![
                    RemoteProjectItem {
                        id: "grace".into(),
                        name: "Amazing Grace".into(),
                        kind: "show".into(),
                    },
                    RemoteProjectItem {
                        id: "other".into(),
                        name: "Other Song".into(),
                        kind: "show".into(),
                    },
                ],
            },
            RemoteProject {
                id: "monday".into(),
                name: "Monday".into(),
                parent: "/".into(),
                created: 2,
                shows: Vec::new(),
            },
        ];
        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:rename_project",
                "data": { "id": "sunday", "name": " Easter " }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::RenameProject {
                project_id: "sunday".into(),
                name: "Easter".into()
            }]
        );
        assert_eq!(snapshot.projects[0].name, "Easter");

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:remove_project_item",
                "data": { "id": "sunday", "index": 0 }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::RemoveProjectItem {
                project_id: "sunday".into(),
                index: 0
            }]
        );
        assert_eq!(snapshot.projects[0].shows[0].id, "other");

        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:delete_project", "data": { "id": "sunday" } }),
        );
        assert_eq!(
            commands,
            vec![Command::DeleteProject {
                project_id: "sunday".into()
            }]
        );
        assert_eq!(snapshot.project_id.as_deref(), Some("monday"));
        assert_eq!(replies[1]["channel"], "PROJECT");
        assert_eq!(replies[1]["data"], "monday");
    }

    #[test]
    fn the_remote_can_switch_a_show_layout() {
        let mut snapshot = sample();
        let verse = snapshot.shows[0].slides[0].clone();
        let chorus = snapshot.shows[0].slides[1].clone();
        snapshot.shows[0].active_layout = "main".into();
        snapshot.shows[0].layouts = vec![
            RemoteLayout {
                id: "main".into(),
                name: "Default".into(),
                slides: vec![verse.clone(), chorus.clone()],
            },
            RemoteLayout {
                id: "short".into(),
                name: "Short".into(),
                slides: vec![chorus],
            },
        ];
        snapshot.shows[0].slides = vec![verse];
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:change_layout",
                "data": { "showId": "grace", "layoutId": "short" }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::ChangeLayout {
                show_id: "grace".into(),
                layout_id: "short".into()
            }]
        );
        assert_eq!(snapshot.shows[0].active_layout, "short");
        assert_eq!(snapshot.shows[0].slides.len(), 1);
        assert_eq!(snapshot.slide_index, Some(0));
        assert_eq!(replies[0]["data"]["settings"]["activeLayout"], "short");
        assert_eq!(replies[0]["data"]["layouts"]["short"]["name"], "Short");
        assert_eq!(replies[1]["data"]["layout"], "short");
    }

    #[test]
    fn the_remote_can_create_a_project_in_the_open_folder() {
        let mut snapshot = sample();
        snapshot.folders = vec![RemoteFolder {
            id: "sundays".into(),
            name: "Sundays".into(),
            parent: "/".into(),
        }];
        snapshot.projects = vec![RemoteProject {
            id: "sunday".into(),
            name: "Sunday".into(),
            parent: "sundays".into(),
            created: 1,
            shows: Vec::new(),
        }];
        snapshot.project_id = Some("sunday".into());
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:create_project",
                "data": { "id": "easter", "name": " Easter " }
            }),
        );
        assert_eq!(
            commands,
            vec![Command::CreateProject {
                id: "easter".into(),
                name: "Easter".into(),
                parent: "sundays".into()
            }]
        );
        assert_eq!(snapshot.project_id.as_deref(), Some("easter"));
        assert_eq!(snapshot.projects[1].parent, "sundays");
        assert_eq!(replies[1]["channel"], "PROJECT");
        assert_eq!(replies[1]["data"], "easter");

        let (_, commands) = handle_message(
            &mut snapshot,
            &json!({ "channel": "API:create_project", "data": { "name": "" } }),
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn password_opens_the_show_list_without_a_password() {
        let mut snapshot = sample();
        let (replies, commands) = handle_message(&mut snapshot, &json!({ "channel": "PASSWORD" }));
        assert!(commands.is_empty());
        assert_eq!(replies[0]["data"]["password"], false);
        assert_eq!(replies[1]["channel"], "ACCESS");
        assert_eq!(replies[2]["channel"], "SHOWS");
        assert_eq!(replies[2]["data"]["grace"]["name"], "Amazing Grace");
        assert_eq!(replies[3]["data"]["lines"][0], "Amazing grace");
    }

    #[test]
    fn a_password_blocks_the_show_until_access_is_accepted() {
        let mut snapshot = sample();
        let mut authenticated = false;

        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "PASSWORD" }),
            Some("1234"),
            &mut authenticated,
        );
        assert!(commands.is_empty());
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["data"]["password"], true);
        assert!(!authenticated);

        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "API:next_slide" }),
            Some("1234"),
            &mut authenticated,
        );
        assert!(replies.is_empty(), "an unauthenticated channel is ignored");
        assert!(commands.is_empty());
        assert_eq!(snapshot.slide_index, Some(0));

        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "ACCESS", "data": "0000" }),
            Some("1234"),
            &mut authenticated,
        );
        assert!(commands.is_empty());
        assert_eq!(replies[0]["channel"], "ERROR");
        assert_eq!(replies[0]["data"]["key"], "wrongPass");
        assert!(!authenticated);

        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "ACCESS", "data": "1234" }),
            Some("1234"),
            &mut authenticated,
        );
        assert!(commands.is_empty());
        assert_eq!(replies[0]["channel"], "ACCESS");
        assert_eq!(replies[1]["channel"], "SHOWS");
        assert!(authenticated);

        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "API:next_slide" }),
            Some("1234"),
            &mut authenticated,
        );
        assert_eq!(commands, vec![Command::SetSlide(1)]);
        assert_eq!(replies[0]["data"]["lines"][0], "how sweet");
    }

    #[test]
    fn an_empty_password_does_not_gate_the_remote() {
        let mut snapshot = sample();
        let mut authenticated = false;
        let (replies, commands) = handle_message_authenticated(
            &mut snapshot,
            &json!({ "channel": "PASSWORD" }),
            Some("  "),
            &mut authenticated,
        );
        assert!(commands.is_empty());
        assert_eq!(replies[0]["data"]["password"], false);
        assert!(authenticated);
    }

    #[test]
    fn next_and_previous_move_by_one_slide() {
        let mut snapshot = sample();
        let (replies, commands) =
            handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(commands, vec![Command::SetSlide(1)]);
        assert_eq!(replies[0]["data"]["lines"][0], "how sweet");
        assert_eq!(replies[0]["data"]["nextLines"][0], "the sound");

        let (replies, commands) =
            handle_message(&mut snapshot, &json!({ "channel": "API:previous_slide" }));
        assert_eq!(commands, vec![Command::SetSlide(0)]);
        assert_eq!(replies[0]["data"]["lines"][0], "Amazing grace");
    }

    #[test]
    fn next_stops_on_the_last_slide() {
        let mut snapshot = sample();
        snapshot.slide_index = Some(2);
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(commands, vec![Command::SetSlide(2)]);
        assert_eq!(snapshot.slide_index, Some(2));
    }

    fn sunday_project(snapshot: &mut ShowSnapshot) {
        snapshot.project_id = Some("sunday".into());
        snapshot.project_item = 0;
        snapshot.projects = vec![RemoteProject {
            id: "sunday".into(),
            name: "Sunday".into(),
            parent: "/".into(),
            created: 0,
            shows: vec![
                RemoteProjectItem {
                    id: "grace".into(),
                    name: "Amazing Grace".into(),
                    kind: "show".into(),
                },
                RemoteProjectItem {
                    id: "welcome".into(),
                    name: "Welcome".into(),
                    kind: "section".into(),
                },
                RemoteProjectItem {
                    id: "other".into(),
                    name: "Other Song".into(),
                    kind: "show".into(),
                },
                RemoteProjectItem {
                    id: "/tmp/bg.png".into(),
                    name: "Background".into(),
                    kind: "image".into(),
                },
            ],
        }];
    }

    #[test]
    fn next_on_the_last_slide_opens_the_next_song() {
        let mut snapshot = sample();
        sunday_project(&mut snapshot);
        snapshot.slide_index = Some(2);
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(
            commands,
            vec![Command::OpenProjectItem {
                index: 2,
                slide: Some(0)
            }]
        );
        assert_eq!(snapshot.show_index, Some(1));
        assert_eq!(snapshot.slide_index, Some(0));
        assert_eq!(snapshot.project_item, 2);
        let lines = snapshot.current_show().unwrap().slides[0].lines.clone();
        assert_eq!(lines, vec!["Hello".to_string()]);
    }

    #[test]
    fn next_on_the_last_slide_opens_an_image() {
        let mut snapshot = sample();
        sunday_project(&mut snapshot);
        snapshot.show_index = Some(1);
        snapshot.project_item = 2;
        snapshot.slide_index = Some(0);
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(
            commands,
            vec![Command::OpenProjectItem {
                index: 3,
                slide: None
            }]
        );
        assert_eq!(snapshot.show_index, Some(1));
        assert_eq!(snapshot.slide_index, Some(0));
    }

    #[test]
    fn previous_on_the_first_slide_opens_the_earlier_song() {
        let mut snapshot = sample();
        sunday_project(&mut snapshot);
        snapshot.show_index = Some(1);
        snapshot.project_item = 2;
        snapshot.slide_index = Some(0);
        let (_, commands) =
            handle_message(&mut snapshot, &json!({ "channel": "API:previous_slide" }));
        assert_eq!(
            commands,
            vec![Command::OpenProjectItem {
                index: 0,
                slide: Some(2)
            }]
        );
        assert_eq!(snapshot.show_index, Some(0));
        assert_eq!(snapshot.slide_index, Some(2));
    }

    #[test]
    fn a_disabled_setting_keeps_the_remote_on_the_last_slide() {
        let mut snapshot = sample();
        sunday_project(&mut snapshot);
        snapshot.slide_index = Some(2);
        snapshot.next_item_on_last_slide = false;
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(commands, vec![Command::SetSlide(2)]);
        assert_eq!(snapshot.project_item, 0);
    }

    #[test]
    fn next_on_the_last_slide_shows_the_overlay() {
        let mut snapshot = sample();
        snapshot.project_id = Some("sunday".into());
        snapshot.overlays = vec![RemoteOverlay {
            id: "logo".into(),
            name: "Logo".into(),
            category: None,
            items: json!([]),
        }];
        snapshot.projects = vec![RemoteProject {
            id: "sunday".into(),
            name: "Sunday".into(),
            parent: "/".into(),
            created: 0,
            shows: vec![
                RemoteProjectItem {
                    id: "grace".into(),
                    name: "Amazing Grace".into(),
                    kind: "show".into(),
                },
                RemoteProjectItem {
                    id: "missing".into(),
                    name: "Missing".into(),
                    kind: "overlay".into(),
                },
                RemoteProjectItem {
                    id: "logo".into(),
                    name: "Logo".into(),
                    kind: "overlay".into(),
                },
            ],
        }];
        snapshot.slide_index = Some(2);
        let (_, commands) = handle_message(&mut snapshot, &json!({ "channel": "API:next_slide" }));
        assert_eq!(
            commands,
            vec![Command::OpenProjectItem {
                index: 2,
                slide: None
            }]
        );
        assert_eq!(snapshot.show_index, Some(0));
        assert_eq!(snapshot.slide_index, Some(2));
        assert_eq!(snapshot.active_overlays, vec!["logo".to_string()]);
    }

    #[test]
    fn selecting_a_slide_by_index_can_switch_show() {
        let mut snapshot = sample();
        let (replies, commands) = handle_message(
            &mut snapshot,
            &json!({
                "channel": "API:index_select_slide",
                "data": { "showId": "other", "index": 0 }
            }),
        );
        assert_eq!(commands, vec![Command::SetShow(1), Command::SetSlide(0)]);
        assert_eq!(replies[0]["data"]["lines"][0], "Hello");
        assert_eq!(replies[0]["data"]["show"]["id"], "other");
    }

    #[test]
    fn clear_blanks_the_output_message() {
        let mut snapshot = sample();
        let (replies, commands) =
            handle_message(&mut snapshot, &json!({ "channel": "API:clear_all" }));
        assert_eq!(commands, vec![Command::Clear]);
        assert!(snapshot.cleared);
        assert!(replies[0]["data"]["slide"].is_null());
        assert!(replies[0]["data"]["lines"].as_array().unwrap().is_empty());
        assert_eq!(snapshot.slide_lines()[0], "Amazing grace");
    }

    #[test]
    fn stage_layouts_reply_with_one_open_layout() {
        let mut snapshot = sample();
        let (replies, commands) = handle_message(&mut snapshot, &json!({ "channel": "LAYOUTS" }));
        assert!(commands.is_empty());
        assert_eq!(replies[0]["data"][0]["id"], "default");
        assert_eq!(replies[0]["data"][0]["password"], false);

        let (replies, _) = handle_message(&mut snapshot, &json!({ "channel": "LAYOUT" }));
        assert_eq!(replies[0]["channel"], "LAYOUT");
        assert_eq!(replies[0]["data"]["notes"], "Stand");
        assert_eq!(replies[1]["channel"], "OUT");
    }
}
