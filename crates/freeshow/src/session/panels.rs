// SPDX-License-Identifier: GPL-3.0-only

//! Data behind the Settings, Inspector and Stage editor panels.
//!
//! Each panel reads a plain snapshot and writes back one validated key at a
//! time, so the Slint side never builds JSON or CSS itself.

use super::Session;

/// FreeShow's autosave choices (Settings → General).
pub const AUTOSAVE_CHOICES: [&str; 6] = ["never", "2min", "5min", "10min", "15min", "30min"];
/// Stage item kinds the editor can add without extra data.
pub const STAGE_ADD_KINDS: [&str; 7] = [
    "slide", "next", "notes", "clock", "timer", "text", "tracker",
];

/// Library `settings.json` values shown in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsForm {
    pub language: String,
    pub time_format: String,
    pub autosave: String,
    pub alert_updates: bool,
    pub auto_output: bool,
    pub out_locked: bool,
    pub full_colors: bool,
    pub labels_disabled: bool,
    pub group_numbers: bool,
    pub split_lines: i32,
    pub remote_port: i32,
    pub stage_port: i32,
    pub max_connections: i32,
    pub theme: String,
    pub themes: Vec<String>,
    pub profiles: String,
    /// Name of the audience output the window flags apply to, or empty when none is stored.
    pub output_name: String,
    pub output_always_on_top: bool,
    pub output_kiosk: bool,
    /// The library-wide slide text and media transitions: kind and milliseconds.
    pub text_transition: String,
    pub text_duration: i32,
    pub media_transition: String,
    pub media_duration: i32,
    /// Where phones and tablets on this network open the remote and stage pages.
    pub addresses: String,
}

/// The friendly name of a Settings key, for the "Saved …" line.
pub fn setting_label(key: &str) -> &str {
    match key {
        "language" => "language",
        "timeFormat" => "time format",
        "autosave" => "autosave",
        "alertUpdates" => "update alerts",
        "autoOutput" => "open output on startup",
        "outLocked" => "lock output",
        "fullColors" => "full group colors",
        "labelsDisabled" => "hide button labels",
        "groupNumbers" => "group numbers",
        "splitLines" => "split lines",
        "maxConnections" => "connections",
        "remotePort" => "remote port (applies after a restart)",
        "stagePort" => "stage port (applies after a restart)",
        "textTransition" | "textDuration" => "slide transition",
        "mediaTransition" | "mediaDuration" => "media transition",
        "outputAlwaysOnTop" => "always on top",
        "outputKiosk" => "kiosk",
        "theme" => "theme",
        other => other,
    }
}

/// This computer's address on the local network, or `localhost` when it has none.
pub fn lan_host() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        // Connecting a UDP socket picks the outgoing interface without sending anything.
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| {
                socket.connect("192.0.2.1:80")?;
                socket.local_addr()
            })
            .ok()
            .map(|addr| addr.ip())
            .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
            .map_or_else(|| "localhost".to_string(), |ip| ip.to_string())
    })
    .clone()
}

/// Transition kinds the output can draw, in the order Settings lists them.
pub const TRANSITION_KINDS: [&str; 6] = ["none", "fade", "blur", "scale", "slide", "spin"];

/// The selected slide item as the Inspector shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorForm {
    pub has_item: bool,
    pub label: String,
    pub typography: freeshow_data::ItemTypography,
    pub item_box: freeshow_data::ItemBox,
}

/// One stage item row in the Stage editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRow {
    pub id: String,
    pub kind: String,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub font_px: i32,
    pub color: String,
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim() {
        "true" | "1" | "on" => Ok(true),
        "false" | "0" | "off" => Ok(false),
        other => Err(format!("expected true or false, got {other:?}")),
    }
}

fn parse_int(value: &str, min: i64, max: i64) -> Result<i64, String> {
    let number = value
        .trim()
        .trim_end_matches("px")
        .parse::<i64>()
        .map_err(|_| format!("expected a number, got {value:?}"))?;
    if !(min..=max).contains(&number) {
        return Err(format!("{number} is outside {min}–{max}"));
    }
    Ok(number)
}

/// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(...)`/`rgba(...)`, or a CSS color name.
pub fn valid_css_color(value: &str) -> bool {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    if (value.starts_with("rgb(") || value.starts_with("rgba(")) && value.ends_with(')') {
        return true;
    }
    !value.is_empty() && value.len() <= 24 && value.chars().all(|c| c.is_ascii_alphabetic())
}

fn stage_kind_name(kind: &freeshow_data::StageItemKind) -> &'static str {
    use freeshow_data::StageItemKind as Kind;
    match kind {
        Kind::Slide => "slide",
        Kind::Notes => "notes",
        Kind::Clock => "clock",
        Kind::Timer => "timer",
        Kind::Text => "text",
        Kind::Variable => "variable",
        Kind::SlideTracker => "tracker",
        Kind::Media => "media",
        Kind::Camera => "camera",
        Kind::CurrentOutput => "output",
        Kind::QrCode => "qr",
        Kind::Web => "web",
        Kind::Chart => "chart",
        Kind::Custom => "custom",
    }
}

impl Session {
    pub fn settings_form(&self) -> SettingsForm {
        let prefs = &self.preferences;
        let mut themes: Vec<String> = freeshow_core::BUILT_IN_THEMES
            .iter()
            .map(|theme| theme.name.to_ascii_lowercase())
            .collect();
        if let Some(custom) = prefs.themes.as_object() {
            for name in custom.keys() {
                if !themes.contains(name) {
                    themes.push(name.clone());
                }
            }
        }
        let profiles = crate::operator::profiles_from(&prefs.profiles)
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>();
        SettingsForm {
            language: prefs.language.clone().unwrap_or_else(|| "en".into()),
            time_format: prefs.time_format.clone().unwrap_or_else(|| "24".into()),
            autosave: prefs.autosave.clone().unwrap_or_else(|| "15min".into()),
            alert_updates: prefs.alert_updates(),
            auto_output: prefs.auto_output(),
            out_locked: prefs.out_locked(),
            full_colors: prefs.full_colors(),
            labels_disabled: prefs.labels_disabled(),
            group_numbers: prefs.group_numbers(),
            split_lines: prefs.split_lines() as i32,
            remote_port: i32::from(prefs.remote_port.unwrap_or(freeshow_remote::REMOTE_PORT)),
            stage_port: i32::from(prefs.stage_port.unwrap_or(freeshow_remote::STAGE_PORT)),
            addresses: {
                let host = lan_host();
                format!(
                    "On a phone or tablet on this network, open the remote at http://{host}:{} and the stage display at http://{host}:{}",
                    prefs.remote_port.unwrap_or(freeshow_remote::REMOTE_PORT),
                    prefs.stage_port.unwrap_or(freeshow_remote::STAGE_PORT)
                )
            },
            max_connections: prefs.max_connections.unwrap_or(10) as i32,
            theme: prefs.theme.clone().unwrap_or_else(|| "default".into()),
            themes,
            profiles: if profiles.is_empty() {
                "No profiles".into()
            } else {
                profiles.join(", ")
            },
            output_name: prefs
                .active_output()
                .map(|output| {
                    if output.name.is_empty() {
                        output.id.clone()
                    } else {
                        output.name.clone()
                    }
                })
                .unwrap_or_default(),
            output_always_on_top: self.output_window_flags().0,
            output_kiosk: self.output_window_flags().1,
            text_transition: self.library_transition(false).kind,
            text_duration: self.library_transition(false).duration_ms as i32,
            media_transition: self.library_transition(true).kind,
            media_duration: self.library_transition(true).duration_ms as i32,
        }
    }

    /// Writes one Settings value into the library `settings.json`.
    pub fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        use serde_json::{Value, json};
        let value = value.trim();
        let entry: (Vec<&str>, Value) = match key {
            "language" => {
                if value.is_empty()
                    || value.len() > 8
                    || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return Err(format!("invalid language code {value:?}"));
                }
                (vec!["language"], json!(value))
            }
            "timeFormat" => {
                if value != "12" && value != "24" {
                    return Err("time format is 12 or 24".into());
                }
                (vec!["timeFormat"], json!(value))
            }
            "autosave" => {
                if !AUTOSAVE_CHOICES.contains(&value) {
                    return Err(format!("autosave must be one of {AUTOSAVE_CHOICES:?}"));
                }
                (vec!["autosave"], json!(value))
            }
            "alertUpdates" | "autoOutput" | "outLocked" | "fullColors" | "labelsDisabled"
            | "groupNumbers" => (vec![key], json!(parse_bool(value)?)),
            "splitLines" => (vec!["splitLines"], json!(parse_int(value, 0, 20)?)),
            "maxConnections" => (vec!["maxConnections"], json!(parse_int(value, 0, 10_000)?)),
            "remotePort" => (
                vec!["ports", "remote"],
                json!(parse_int(value, 1024, 65535)?),
            ),
            "stagePort" => (
                vec!["ports", "stage"],
                json!(parse_int(value, 1024, 65535)?),
            ),
            "textTransition" | "mediaTransition" | "textDuration" | "mediaDuration" => {
                let media = key.starts_with("media");
                let mut motion = self.library_transition(media);
                if key.ends_with("Duration") {
                    motion.duration_ms = parse_int(value, 0, 10_000)? as u64;
                } else if TRANSITION_KINDS.contains(&value) {
                    motion.kind = value.to_string();
                } else {
                    return Err(format!("transition must be one of {TRANSITION_KINDS:?}"));
                }
                let root = self.root.clone().ok_or("choose a library folder first")?;
                let slot = if media { "media" } else { "text" };
                return if freeshow_data::Library::open(&root).set_transition(
                    slot,
                    &motion.kind,
                    motion.duration_ms,
                    &motion.easing,
                ) {
                    Ok(())
                } else {
                    Err("could not write settings.json".into())
                };
            }
            "outputAlwaysOnTop" | "outputKiosk" => {
                let id = match self.preferences.active_output() {
                    Some(output) => output.id.clone(),
                    None => self.add_default_output()?,
                };
                let field = if key == "outputKiosk" {
                    "kiosk"
                } else {
                    "alwaysOnTop"
                };
                return self.patch_output(&id, field, json!(parse_bool(value)?));
            }
            "theme" => {
                let Some(theme) = self
                    .settings_form()
                    .themes
                    .into_iter()
                    .find(|theme| theme.eq_ignore_ascii_case(value))
                else {
                    return Err(format!("unknown theme {value:?}"));
                };
                (vec!["theme"], json!(theme))
            }
            other => return Err(format!("unknown setting {other:?}")),
        };
        let (path, value) = entry;
        let wrote = self.patch_settings(|settings| {
            let mut node = settings;
            for part in &path[..path.len() - 1] {
                if !node.get(*part).is_some_and(Value::is_object) {
                    node[*part] = json!({});
                }
                node = &mut node[*part];
            }
            node[path[path.len() - 1]] = value;
        });
        if wrote {
            Ok(())
        } else {
            Err("could not write settings.json".into())
        }
    }

    /// Always-on-top and kiosk for the audience output window, from its stored output.
    /// FreeShow keeps outputs on top unless `alwaysOnTop` is false.
    pub fn output_window_flags(&self) -> (bool, bool) {
        self.preferences
            .active_output()
            .map(|output| (output.always_on_top, output.kiosk))
            .unwrap_or((true, false))
    }

    /// A library that never had an output stored gets FreeShow's default "Primary" output.
    fn add_default_output(&mut self) -> Result<String, String> {
        let wrote = self.patch_settings(|settings| {
            if !settings
                .get("outputs")
                .is_some_and(serde_json::Value::is_object)
            {
                settings["outputs"] = serde_json::json!({});
            }
            settings["outputs"]["default"] = serde_json::json!({
                "enabled": true,
                "active": true,
                "name": "Primary",
                "color": "#F0008C",
                "bounds": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "alwaysOnTop": true,
            });
        });
        if wrote && self.preferences.active_output().is_some() {
            Ok("default".into())
        } else {
            Err("could not write settings.json".into())
        }
    }

    fn patch_output(
        &mut self,
        id: &str,
        field: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        let wrote = self.patch_settings(|settings| {
            if let Some(output) = settings
                .get_mut("outputs")
                .and_then(|outputs| outputs.get_mut(id))
                .and_then(serde_json::Value::as_object_mut)
            {
                output.insert(field.to_string(), value);
            }
        });
        if wrote {
            Ok(())
        } else {
            Err("could not write settings.json".into())
        }
    }

    pub fn inspector_form(&self) -> InspectorForm {
        let item_box = self
            .current_show()
            .and_then(|show| {
                let slide = show.ordered_slides().get(self.slide_index?)?.id.clone();
                Some(show.item_box(&slide, self.current_item(show)))
            })
            .unwrap_or_default();
        InspectorForm {
            has_item: self.tab == "shows"
                && self.current_show().is_some()
                && self.slide_index.is_some(),
            label: self.item_label(),
            typography: self.item_typography(),
            item_box,
        }
    }

    /// Applies one Inspector field to the selected item and saves the show.
    pub fn inspector_set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let value = value.trim().to_string();
        let ok = match key {
            "font-family" => self.set_font_family(&value),
            "font-size" => {
                let px = parse_int(&value, 12, 400)? as i32;
                self.edit_current_item(|show, slide, item| show.set_item_font_size(slide, item, px))
            }
            "bold" => {
                let on = parse_bool(&value)?;
                self.edit_current_item(|show, slide, item| show.set_item_bold(slide, item, on))
            }
            "italic" => {
                let on = parse_bool(&value)?;
                self.edit_current_item(|show, slide, item| show.set_item_italic(slide, item, on))
            }
            "color" => {
                if !valid_css_color(&value) {
                    return Err(format!("invalid color {value:?}"));
                }
                self.edit_current_item(|show, slide, item| show.set_item_color(slide, item, &value))
            }
            "align" => {
                if !["left", "center", "right", "justify"].contains(&value.as_str()) {
                    return Err(format!("invalid alignment {value:?}"));
                }
                self.edit_current_item(|show, slide, item| show.set_item_align(slide, item, &value))
            }
            "left" | "top" | "width" | "height" | "border-radius" => {
                let (min, max) = match key {
                    "width" | "height" => (1, 8000),
                    "border-radius" => (0, 2000),
                    _ => (-8000, 8000),
                };
                let px = parse_int(&value, min, max)?;
                let css = format!("{px}px");
                self.edit_current_item(|show, slide, item| {
                    show.set_item_box_css(slide, item, key, &css)
                })
            }
            "background-color" => {
                if !value.is_empty() && !valid_css_color(&value) {
                    return Err(format!("invalid color {value:?}"));
                }
                self.edit_current_item(|show, slide, item| {
                    show.set_item_box_css(slide, item, "background-color", &value)
                })
            }
            other => return Err(format!("unknown inspector field {other:?}")),
        };
        if ok {
            Ok(())
        } else {
            Err("select a show slide first".into())
        }
    }

    pub fn stage_rows(&self) -> Vec<StageRow> {
        self.stage
            .items
            .iter()
            .map(|item| {
                let px = |key: &str, default: i32| {
                    freeshow_data::css_px(&item.style, key).unwrap_or(default)
                };
                StageRow {
                    id: item.id.clone(),
                    kind: stage_kind_name(&item.kind).to_string(),
                    left: px("left", 0),
                    top: px("top", 0),
                    width: px("width", 1920),
                    height: px("height", 1080),
                    font_px: px("font-size", 100),
                    color: freeshow_data::css_value(&item.style, "color").unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Sets one geometry or text field on a stage item and saves `Config/stage.json`.
    pub fn stage_set(&mut self, id: &str, key: &str, value: &str) -> Result<(), String> {
        let value = value.trim();
        let css = match key {
            "left" | "top" => format!("{}px", parse_int(value, -8000, 8000)?),
            "width" | "height" => format!("{}px", parse_int(value, 1, 8000)?),
            "font-size" => format!("{}px", parse_int(value, 8, 800)?),
            "color" => {
                if !valid_css_color(value) {
                    return Err(format!("invalid color {value:?}"));
                }
                value.to_string()
            }
            other => return Err(format!("unknown stage field {other:?}")),
        };
        let style = self
            .stage
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| freeshow_data::css_set(&item.style, key, &css))
            .ok_or_else(|| format!("no stage item {id:?}"))?;
        if self.set_stage_item_style(id, &style) {
            Ok(())
        } else {
            Err("could not save stage.json".into())
        }
    }

    pub fn remove_stage_item(&mut self, id: &str) -> bool {
        let before = self.stage.items.len();
        self.stage.items.retain(|item| item.id != id);
        self.stage.items.len() != before && self.save_stage_layout()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Dir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn library(name: &str) -> (Dir, Session) {
        let dir = Dir(
            std::env::temp_dir().join(format!("freeshow-panels-{name}-{}", std::process::id()))
        );
        let _ = std::fs::remove_dir_all(dir.path());
        std::fs::create_dir_all(dir.path().join("Shows")).unwrap();
        std::fs::create_dir_all(dir.path().join("Config")).unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"theme":"dark","unknownKey":{"keep":1},"ports":{"remote":5510}}"#,
        )
        .unwrap();
        let session = Session::open(dir.path().to_path_buf());
        (dir, session)
    }

    #[test]
    fn settings_write_back_validated_values_and_keep_unknown_keys() {
        let (dir, mut session) = library("settings");
        assert_eq!(session.settings_form().theme, "dark");
        assert!(
            session.settings_form().group_numbers,
            "on by default, as the slides show"
        );
        session.apply_setting("outLocked", "true").unwrap();
        session.apply_setting("stagePort", "5600").unwrap();
        session.apply_setting("autosave", "5min").unwrap();
        session.apply_setting("theme", "sea").unwrap();
        assert!(session.apply_setting("remotePort", "80").is_err());
        assert!(session.apply_setting("autosave", "soon").is_err());
        assert!(session.apply_setting("theme", "nope").is_err());
        assert!(session.apply_setting("bogus", "1").is_err());
        let form = session.settings_form();
        assert!(form.out_locked);
        assert_eq!(form.stage_port, 5600);
        assert_eq!(form.remote_port, 5510);
        assert_eq!(form.autosave, "5min");
        assert_eq!(form.theme, "sea");
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["unknownKey"]["keep"], 1);
        assert_eq!(saved["ports"]["stage"], 5600);
    }

    #[test]
    fn slide_and_media_transitions_are_set_from_settings() {
        let (dir, mut session) = library("transitions");
        let form = session.settings_form();
        assert_eq!(
            (form.text_transition.as_str(), form.text_duration),
            ("fade", 500)
        );
        assert_eq!(
            (form.media_transition.as_str(), form.media_duration),
            ("fade", 800)
        );
        session.apply_setting("textTransition", "none").unwrap();
        session.apply_setting("mediaDuration", "1200").unwrap();
        assert!(session.apply_setting("textTransition", "wobble").is_err());
        assert!(session.apply_setting("textDuration", "-5").is_err());
        let form = session.settings_form();
        assert_eq!(form.text_transition, "none");
        assert_eq!(form.media_duration, 1200);
        assert_eq!(session.output_motion().0.kind, "none");
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["transitionData"]["media"]["duration"], 1200);
        assert_eq!(saved["unknownKey"]["keep"], 1);
    }

    #[test]
    fn output_window_flags_follow_the_active_output() {
        let (dir, mut session) = library("outputs");
        assert_eq!(session.output_window_flags(), (true, false));
        session.apply_setting("outputKiosk", "true").unwrap();
        assert_eq!(
            session.output_window_flags(),
            (true, true),
            "a Primary output was added"
        );
        assert_eq!(session.settings_form().output_name, "Primary");
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"outputs":{"a":{"name":"Side","enabled":true},"b":{"name":"Main","active":true,"alwaysOnTop":false}}}"#,
        )
        .unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(session.output_window_flags(), (false, false));
        assert_eq!(session.settings_form().output_name, "Main");
        session.apply_setting("outputKiosk", "true").unwrap();
        session.apply_setting("outputAlwaysOnTop", "true").unwrap();
        assert_eq!(session.output_window_flags(), (true, true));
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["outputs"]["b"]["kiosk"], true);
        assert!(saved["outputs"]["a"].get("kiosk").is_none());
    }

    #[test]
    fn a_malformed_settings_file_is_never_overwritten() {
        let (dir, mut session) = library("malformed");
        std::fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        assert!(session.apply_setting("outLocked", "true").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn stage_items_can_be_added_moved_and_removed() {
        let (dir, mut session) = library("stage");
        let id = session
            .add_stage_item("clock", "left:10px;top:20px;width:300px;height:100px;")
            .unwrap();
        session.stage_set(&id, "left", "400").unwrap();
        session.stage_set(&id, "color", "#ff0000").unwrap();
        assert!(session.stage_set(&id, "color", "not a color!").is_err());
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!((row.kind.as_str(), row.left, row.top), ("clock", 400, 20));
        assert_eq!(row.color, "#ff0000");
        let saved = std::fs::read_to_string(dir.path().join("Config").join("stage.json")).unwrap();
        assert!(saved.contains("400px"), "{saved}");
        assert!(session.remove_stage_item(&id));
        assert!(session.stage_rows().iter().all(|row| row.id != id));
    }

    #[test]
    fn css_colors_are_validated() {
        for ok in [
            "#fff",
            "#ff000080",
            "rgb(1, 2, 3)",
            "rgba(0,0,0,0.5)",
            "white",
        ] {
            assert!(valid_css_color(ok), "{ok}");
        }
        for bad in ["", "#ggg", "#12345", "red;top:0", "url(x)"] {
            assert!(!valid_css_color(bad), "{bad}");
        }
    }
}
