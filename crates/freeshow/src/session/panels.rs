// SPDX-License-Identifier: GPL-3.0-only

//! Data behind the Settings, Inspector and Stage editor panels.
//!
//! Each panel reads a plain snapshot and writes back one validated key at a
//! time, so the Slint side never builds JSON or CSS itself.

use super::Session;

/// FreeShow's autosave choices (Settings → General).
pub const AUTOSAVE_CHOICES: [&str; 6] = ["never", "2min", "5min", "10min", "15min", "30min"];
/// Stage item kinds the editor can add without extra data.
pub const STAGE_ADD_KINDS: [&str; 14] = [
    "slide", "next", "notes", "clock", "timer", "text", "media", "tracker", "variable", "qr",
    "output", "camera", "chart", "web",
];

/// How the stage editor's add menu names each of [`STAGE_ADD_KINDS`].
pub const STAGE_ADD_NAMES: [&str; 14] = [
    "Current slide",
    "Next slide",
    "Slide notes",
    "Clock",
    "Timer",
    "Text",
    "Picture or video",
    "Slide number",
    "Variable",
    "QR code",
    "Output picture",
    "Camera",
    "Chart",
    "Web preview",
];

/// Library `settings.json` values shown in Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// Picture size of the active output, as `WIDTHxHEIGHT`.
    pub output_resolution: String,
    /// The library-wide slide text and media transitions: kind and milliseconds.
    pub text_transition: String,
    pub text_duration: i32,
    pub media_transition: String,
    pub media_duration: i32,
    /// Where phones and tablets on this network open the remote and stage pages.
    pub addresses: String,
    /// Output styles by name, and the look of the one the active output uses.
    pub style_names: Vec<String>,
    pub style_name: String,
    pub style_is_default: bool,
    pub style_background: String,
    pub style_fit: String,
    pub style_blur_amount: i32,
    pub style_blur_opacity: i32,
    pub style_lines: i32,
    /// The template the style lays slides out with, or "None".
    pub style_template: String,
    /// "None" then every template name, for the Styles tab.
    pub template_choices: Vec<String>,
    /// Audience outputs by name; `output_name` is the one the output window shows.
    pub output_names: Vec<String>,
    /// The theme editor: `(key, label, #hex)` per colour, font, size (%) and corner radius.
    pub theme_colors: Vec<(String, String, String)>,
    pub theme_font: String,
    pub theme_font_size: i32,
    pub theme_radius: i32,
    /// The theme has edits stored in `Config/themes.json` that Reset would drop.
    pub theme_edited: bool,
}

/// Theme colours the editor shows, as FreeShow's `themes.json` keys and plain labels.
pub const THEME_COLORS: [(&str, &str); 8] = [
    ("primary", "Panels"),
    ("primary-lighter", "Buttons"),
    ("primary-darker", "Header"),
    ("primary-darkest", "Deepest"),
    ("text", "Text"),
    ("textInvert", "Text on light"),
    ("secondary", "Accent"),
    ("secondary-text", "Text on accent"),
];

/// How a style fits background pictures and videos (Settings → Styles).
pub const STYLE_FITS: [&str; 4] = ["contain", "cover", "fill", "blur"];

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
        "outputStyle" => "output style",
        "styleNew" => "new style",
        "styleDelete" => "style removal",
        "styleName" => "style name",
        "styleBackground" => "style background",
        "styleFit" => "media fit",
        "styleBlurAmount" | "styleBlurOpacity" => "blur",
        "styleLines" => "max lines",
        "styleTemplate" => "style template",
        "outputPick" => "active output",
        "outputNew" => "new output",
        "outputDelete" => "output removal",
        "outputName" => "output name",
        "outputResolution" => "output resolution",
        "themeFont" => "theme font",
        "themeFontSize" => "theme text size",
        "themeRadius" => "corner radius",
        "themeDuplicate" => "theme copy",
        "themeReset" => "theme reset",
        key if key.starts_with("themeColor.") => "theme colour",
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

/// CSS blend modes the output can draw, in the order the Inspector lists them.
pub const BLEND_MODES: [&str; 9] = [
    "normal",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "difference",
    "exclusion",
    "color-dodge",
];

/// Transition kinds the output can draw, in the order Settings lists them.
pub const TRANSITION_KINDS: [&str; 6] = ["none", "fade", "blur", "scale", "slide", "spin"];

/// The selected slide item as the Inspector shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorForm {
    pub has_item: bool,
    pub label: String,
    pub typography: freeshow_data::ItemTypography,
    pub item_box: freeshow_data::ItemBox,
    pub filters: freeshow_data::ItemFilters,
    /// The slide's own transition kind, or "default" when the style transition applies.
    pub slide_transition: String,
    pub slide_transition_ms: i32,
}

/// One stage item row in the Stage editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRow {
    pub enabled: bool,
    pub id: String,
    pub kind: String,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub font_px: i32,
    pub color: String,
    /// What the operator calls this item, such as "Next slide" or "Clock".
    pub title: String,
    pub label: String,
    pub align: String,
    /// Slides ahead of the live one: 0 is the current slide, 1 the next.
    pub offset: i32,
    /// Lines shown at most; 0 shows the whole slide.
    pub line_count: i32,
    pub include_media: bool,
    pub text_box: i32,
    pub invert_items: bool,
    pub qr_text: String,
    pub qr_color: String,
    pub qr_background: String,
    pub output_source: String,
    pub show_output_label: bool,
    pub device_name: String,
    pub flip_x: bool,
    pub flip_y: bool,
    pub chart_kind: String,
    pub chart_data: String,
    pub chart_hole_size: i32,
    pub web_url: String,
    /// A text item's words, one line per line.
    pub text: String,
    pub seconds: bool,
    pub date_format: String,
    /// The timer a timer item follows; empty follows whichever timer runs.
    pub timer: String,
    /// The variable a variable item shows; empty shows nothing.
    pub variable: String,
    /// A media item's picture or video path.
    pub src: String,
    /// How a media item fills its box: contain, cover or fill.
    pub fit: String,
    /// A slide tracker's look: `number` or `bar`.
    pub tracker_type: String,
    /// A slide tracker bar's colour; empty uses the theme accent.
    pub tracker_accent: String,
}

/// Date formats a stage clock can show, as FreeShow stores them; `none` hides the date.
pub const STAGE_DATE_FORMATS: [&str; 5] = [
    "none",
    "DD/MM/YYYY",
    "MM/DD/YYYY",
    "YYYY-MM-DD",
    "DD.MM.YYYY",
];

/// The name an item has in the stage editor list.
pub(crate) fn stage_item_title(item: &freeshow_data::StageItemSpec) -> String {
    use freeshow_data::StageItemKind as Kind;
    if !item.label.trim().is_empty() {
        return item.label.trim().to_string();
    }
    match item.kind {
        Kind::Slide => match item.offset {
            0 => "Current slide".to_string(),
            1 => "Next slide".to_string(),
            n if n > 1 => format!("Slide +{n}"),
            n => format!("Slide {n}"),
        },
        Kind::Notes => match item.offset {
            0 => "Slide notes".into(),
            1 => "Next slide notes".into(),
            n => format!("Slide {n:+} notes"),
        },
        Kind::Clock => "Clock".to_string(),
        Kind::Timer => "Timer".to_string(),
        Kind::Text => match item.lines.first().map(|line| line.trim()) {
            Some(first) if !first.is_empty() => format!("Text: {first}"),
            _ => "Text".to_string(),
        },
        Kind::Variable => "Variable".to_string(),
        Kind::SlideTracker => "Slide number".to_string(),
        Kind::Media => "Picture or video".to_string(),
        Kind::Camera => item
            .device_name
            .as_deref()
            .map_or_else(|| "Camera".to_string(), |name| format!("Camera: {name}")),
        Kind::CurrentOutput => "Output picture".to_string(),
        Kind::QrCode => "QR code".to_string(),
        Kind::Web => item
            .url
            .as_deref()
            .map_or_else(|| "Web preview".into(), freeshow_output::display_url),
        Kind::Chart => "Chart".to_string(),
        Kind::Custom => item.id.clone(),
    }
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
                .unwrap_or_else(|| "Primary".into()),
            output_always_on_top: self.output_window_flags().0,
            output_kiosk: self.output_window_flags().1,
            output_resolution: prefs
                .active_output()
                .and_then(|output| Some(format!("{}x{}", output.width?, output.height?)))
                .unwrap_or_else(|| "1920x1080".into()),
            text_transition: self.library_transition(false).kind,
            text_duration: self.library_transition(false).duration_ms as i32,
            media_transition: self.library_transition(true).kind,
            media_duration: self.library_transition(true).duration_ms as i32,
            output_names: self
                .audience_outputs()
                .into_iter()
                .map(|(_, name)| name)
                .collect(),
            ..self.style_and_theme_form()
        }
    }

    fn style_and_theme_form(&self) -> SettingsForm {
        let theme = self.theme_form();
        SettingsForm {
            theme_colors: theme.theme_colors,
            theme_font: theme.theme_font,
            theme_font_size: theme.theme_font_size,
            theme_radius: theme.theme_radius,
            theme_edited: theme.theme_edited,
            ..self.style_form()
        }
    }

    /// The Styles tab: every style's name and the active output style's look.
    fn style_form(&self) -> SettingsForm {
        use serde_json::Value;
        let mut form = SettingsForm::default();
        let Some(root) = self.root.as_deref() else {
            return form;
        };
        let library = freeshow_data::Library::open(root);
        let id = library.output_style_id();
        let styles = library.styles();
        form.style_names = styles.iter().map(|(_, name)| name.clone()).collect();
        form.style_name = styles
            .iter()
            .find(|(style, _)| *style == id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| id.clone());
        form.style_is_default = id == "default";
        let style = library.style_value(&id);
        let text = |key: &str| {
            style
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        form.style_background = text("background");
        form.style_fit = Some(text("fit"))
            .filter(|fit| STYLE_FITS.contains(&fit.as_str()))
            .unwrap_or_else(|| "contain".into());
        let (amount, opacity) = library.style_blur();
        form.style_blur_amount = amount.round() as i32;
        form.style_blur_opacity = (opacity * 100.0).round() as i32;
        form.style_lines = library.style_lines() as i32;
        form.style_template = library
            .style_template()
            .and_then(|template| {
                self.template_catalog
                    .get(&template)?
                    .get("name")?
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "None".into());
        form.template_choices = std::iter::once("None".to_string())
            .chain(self.template_catalog.names())
            .collect();
        form
    }

    /// Writes one Styles-tab value to the active output's style and repaints the output.
    fn apply_style_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        use serde_json::{Value, json};
        let root = self.root.clone().ok_or("choose a library folder first")?;
        let library = freeshow_data::Library::open(&root);
        let id = library.output_style_id();
        let wrote = match key {
            "outputStyle" => {
                let (style, _) = library
                    .styles()
                    .into_iter()
                    .find(|(_, name)| name == value)
                    .ok_or_else(|| format!("no style named {value:?}"))?;
                library.set_output_style(&style)
            }
            "styleNew" => {
                let count = library.styles().len();
                let style = library
                    .create_style(&format!("Style {}", count + 1))
                    .ok_or("could not add a style")?;
                library.set_output_style(&style)
            }
            "styleDelete" => {
                if id == "default" {
                    return Err("The Primary style stays; pick another style to remove it".into());
                }
                library.delete_style(&id)
            }
            "styleName" => {
                if value.is_empty() || value.chars().count() > 60 {
                    return Err("a style name is 1–60 characters".into());
                }
                library.patch_style(&id, "name", json!(value))
            }
            "styleBackground" => {
                if value.is_empty() {
                    library.patch_style(&id, "background", Value::Null)
                } else if freeshow_output::parse_color(value).is_some() {
                    library.patch_style(&id, "background", json!(value))
                } else {
                    return Err(format!("{value:?} is not a colour like #000000"));
                }
            }
            "styleFit" => {
                if !STYLE_FITS.contains(&value) {
                    return Err(format!("media fit must be one of {STYLE_FITS:?}"));
                }
                library.patch_style(&id, "fit", json!(value))
            }
            "styleBlurAmount" => {
                library.patch_style(&id, "blurAmount", json!(parse_int(value, 0, 100)?))
            }
            "styleBlurOpacity" => {
                let percent = parse_int(value, 1, 100)?;
                library.patch_style(&id, "blurOpacity", json!(percent as f64 / 100.0))
            }
            "styleTemplate" => {
                if value == "None" {
                    library.patch_style(&id, "template", Value::Null)
                } else {
                    let template = self
                        .template_catalog
                        .id_named(value)
                        .ok_or_else(|| format!("no template named {value:?}"))?;
                    library.patch_style(&id, "template", json!(template))
                }
            }
            "styleLines" => {
                let lines = parse_int(value, 0, 20)?;
                library.patch_style(
                    &id,
                    "lines",
                    if lines == 0 {
                        Value::Null
                    } else {
                        json!(lines)
                    },
                )
            }
            other => return Err(format!("unknown setting {other:?}")),
        };
        if !wrote {
            return Err("could not write the style".into());
        }
        self.refresh_style_look();
        if let Some(path) = self.background_path.clone() {
            self.remember_background(&path);
        }
        if !self.blanked {
            self.live_edited = true;
        }
        Ok(())
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
            "outputStyle" | "styleNew" | "styleDelete" | "styleName" | "styleBackground"
            | "styleFit" | "styleBlurAmount" | "styleBlurOpacity" | "styleLines"
            | "styleTemplate" => {
                return self.apply_style_setting(key, value);
            }
            "outputPick" | "outputNew" | "outputDelete" | "outputName" | "outputResolution" => {
                return self.apply_output_setting(key, value);
            }
            "themeFont" | "themeFontSize" | "themeRadius" | "themeDuplicate" | "themeReset" => {
                return self.apply_theme_setting(key, value);
            }
            key if key.starts_with("themeColor.") => {
                return self.apply_theme_setting(key, value);
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

    fn theme_id(&self) -> String {
        self.preferences
            .theme
            .clone()
            .unwrap_or_else(|| "default".into())
    }

    /// The theme editor's view of the library theme, with built-in values where none is stored.
    fn theme_form(&self) -> SettingsForm {
        let theme = crate::theme::operator_theme(None, &self.preferences);
        let palette = &theme.palette;
        let hex = |c: freeshow_core::Rgb| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
        let value = |key: &str| match key {
            "primary" => hex(palette.primary),
            "primary-lighter" => hex(palette.primary_lighter),
            "primary-darker" => hex(palette.primary_darker),
            "primary-darkest" => hex(palette.primary_darkest),
            "text" => hex(palette.text),
            "textInvert" => hex(palette.text_invert),
            "secondary" => hex(palette.secondary),
            _ => hex(palette.secondary_text),
        };
        SettingsForm {
            theme_colors: THEME_COLORS
                .iter()
                .map(|(key, label)| (key.to_string(), label.to_string(), value(key)))
                .collect(),
            theme_font: theme.font_family.clone(),
            theme_font_size: (theme.font_scale * 100.0).round() as i32,
            theme_radius: theme.border_radius.round() as i32,
            theme_edited: self.preferences.themes.get(self.theme_id()).is_some(),
            ..SettingsForm::default()
        }
    }

    /// Edits the library theme in `Config/themes.json`, or copies or resets it.
    fn apply_theme_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        use serde_json::{Value, json};
        let root = self.root.clone().ok_or("choose a library folder first")?;
        let id = self.theme_id();
        let path = root.join("Config").join("themes.json");
        let mut themes: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let built_in = freeshow_core::BUILT_IN_THEMES
            .iter()
            .any(|theme| theme.name.eq_ignore_ascii_case(&id));
        let mut switch_to = None;
        let entry = |themes: &mut Value, id: &str| -> Value {
            if !themes[id].is_object() {
                themes[id] = json!({ "name": id });
            }
            themes[id].clone()
        };
        if let Some(color_key) = key.strip_prefix("themeColor.") {
            if !THEME_COLORS.iter().any(|(known, _)| *known == color_key) {
                return Err(format!("unknown theme colour {color_key:?}"));
            }
            if freeshow_output::parse_color(value).is_none() {
                return Err(format!("{value:?} is not a colour like #242832"));
            }
            entry(&mut themes, &id);
            if !themes[&id]["colors"].is_object() {
                themes[&id]["colors"] = json!({});
            }
            themes[&id]["colors"][color_key] = json!(value);
        } else {
            match key {
                "themeFont" => {
                    entry(&mut themes, &id);
                    if !themes[&id]["font"].is_object() {
                        themes[&id]["font"] = json!({});
                    }
                    themes[&id]["font"]["family"] = if value.is_empty() {
                        json!("sans-serif")
                    } else {
                        json!(value)
                    };
                }
                "themeFontSize" => {
                    let percent = parse_int(value, 50, 200)?;
                    entry(&mut themes, &id);
                    if !themes[&id]["font"].is_object() {
                        themes[&id]["font"] = json!({});
                    }
                    themes[&id]["font"]["size"] = json!(format!("{}em", percent as f64 / 100.0));
                }
                "themeRadius" => {
                    let radius = parse_int(value, 0, 24)?;
                    entry(&mut themes, &id);
                    themes[&id]["border"] = json!({ "radius": format!("{radius}px") });
                }
                "themeDuplicate" => {
                    let form = self.theme_form();
                    let base = format!("{id}-copy");
                    let taken = self.settings_form().themes;
                    let copy = std::iter::once(base.clone())
                        .chain((2..).map(|n| format!("{base}-{n}")))
                        .find(|name| !taken.contains(name))
                        .ok_or("could not name the copy")?;
                    let colors: serde_json::Map<String, Value> = form
                        .theme_colors
                        .iter()
                        .map(|(key, _, hex)| (key.clone(), json!(hex)))
                        .collect();
                    themes[&copy] = json!({
                        "name": copy,
                        "colors": colors,
                        "font": {
                            "family": if form.theme_font.is_empty() { "sans-serif".to_string() } else { form.theme_font.clone() },
                            "size": format!("{}em", form.theme_font_size as f64 / 100.0),
                        },
                        "border": { "radius": format!("{}px", form.theme_radius) },
                    });
                    switch_to = Some(copy);
                }
                "themeReset" => {
                    if let Some(all) = themes.as_object_mut() {
                        all.remove(&id);
                    }
                    if !built_in {
                        switch_to = Some("default".to_string());
                    }
                }
                other => return Err(format!("unknown setting {other:?}")),
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let bytes = serde_json::to_vec_pretty(&themes).map_err(|err| err.to_string())?;
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write themes.json: {err}"))?;
        if let Some(theme) = switch_to {
            if !self.patch_settings(|settings| settings["theme"] = json!(theme)) {
                return Err("could not write settings.json".into());
            }
        } else if let Some(preferences) = self.reload_preferences() {
            self.preferences = preferences;
        }
        Ok(())
    }

    /// Audience outputs (not stage displays) as `(id, shown name)`.
    pub(crate) fn audience_outputs(&self) -> Vec<(String, String)> {
        let outputs: Vec<(String, String)> = self
            .preferences
            .outputs
            .iter()
            .filter(|output| !output.stage)
            .map(|output| {
                let name = if output.name.trim().is_empty() {
                    output.id.clone()
                } else {
                    output.name.trim().to_string()
                };
                (output.id.clone(), name)
            })
            .collect();
        if outputs.is_empty() {
            // A library that never stored an output plays FreeShow's default "Primary" one.
            vec![("default".into(), "Primary".into())]
        } else {
            outputs
        }
    }

    /// Picks, adds, removes or renames the audience output the output window plays.
    fn apply_output_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        use serde_json::json;
        let outputs = self.audience_outputs();
        let active = match self.preferences.active_output() {
            Some(output) => output.id.clone(),
            None => self.add_default_output()?,
        };
        let wrote = match key {
            "outputPick" => {
                let (id, _) = outputs
                    .iter()
                    .find(|(_, name)| name == value)
                    .ok_or_else(|| format!("no output named {value:?}"))?
                    .clone();
                self.patch_settings(|settings| {
                    if let Some(all) = settings.get_mut("outputs").and_then(|o| o.as_object_mut()) {
                        for (key, output) in all.iter_mut() {
                            let is_stage = output
                                .get("stageOutput")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|layout| !layout.is_empty());
                            if !is_stage && let Some(output) = output.as_object_mut() {
                                output.insert("active".into(), json!(*key == id));
                            }
                        }
                    }
                })
            }
            "outputNew" => {
                let taken: Vec<String> = self
                    .preferences
                    .outputs
                    .iter()
                    .map(|output| output.id.clone())
                    .collect();
                let id = (2..)
                    .map(|n| format!("output{n}"))
                    .find(|id| !taken.contains(id))
                    .ok_or("could not name the output")?;
                let name = format!("Output {}", outputs.len().max(1) + 1);
                self.patch_settings(|settings| {
                    if let Some(all) = settings.get_mut("outputs").and_then(|o| o.as_object_mut()) {
                        for output in all.values_mut() {
                            if let Some(output) = output.as_object_mut()
                                && output.get("stageOutput").is_none()
                            {
                                output.insert("active".into(), json!(false));
                            }
                        }
                        all.insert(
                            id.clone(),
                            json!({
                                "enabled": true,
                                "active": true,
                                "name": name,
                                "color": "#F0008C",
                                "style": "default",
                                "bounds": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                                "alwaysOnTop": true,
                            }),
                        );
                    }
                })
            }
            "outputDelete" => {
                if outputs.len() < 2 {
                    return Err("FreeShow needs at least one output".into());
                }
                let next = outputs
                    .iter()
                    .find(|(id, _)| *id != active)
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default();
                self.patch_settings(|settings| {
                    if let Some(all) = settings.get_mut("outputs").and_then(|o| o.as_object_mut()) {
                        all.remove(&active);
                        if let Some(output) = all.get_mut(&next).and_then(|o| o.as_object_mut()) {
                            output.insert("active".into(), json!(true));
                        }
                    }
                })
            }
            "outputName" => {
                if value.is_empty() || value.chars().count() > 60 {
                    return Err("an output name is 1–60 characters".into());
                }
                return self.patch_output(&active, "name", json!(value));
            }
            "outputResolution" => {
                let (width, height) = value
                    .split_once(['x', '×'])
                    .and_then(|(w, h)| {
                        Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
                    })
                    .filter(|(w, h)| (320..=7680).contains(w) && (240..=4320).contains(h))
                    .ok_or("a resolution is WIDTHxHEIGHT, such as 1280x720")?;
                // Only the size changes; the window keeps its stored position.
                self.patch_settings(|settings| {
                    if let Some(output) = settings
                        .get_mut("outputs")
                        .and_then(|outputs| outputs.get_mut(&active))
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        let bounds = output
                            .entry("bounds")
                            .or_insert_with(|| json!({ "x": 0, "y": 0 }));
                        if !bounds.is_object() {
                            *bounds = json!({ "x": 0, "y": 0 });
                        }
                        bounds["width"] = json!(width);
                        bounds["height"] = json!(height);
                    }
                })
            }
            other => return Err(format!("unknown setting {other:?}")),
        };
        if !wrote {
            return Err("could not write settings.json".into());
        }
        self.refresh_style_look();
        if let Some(path) = self.background_path.clone() {
            self.remember_background(&path);
        }
        if !self.blanked {
            self.live_edited = true;
        }
        Ok(())
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
        let own_transition = self
            .current_show()
            .zip(self.slide_index)
            .and_then(|(show, index)| show.transition_at(index));
        InspectorForm {
            has_item: self.tab == "shows"
                && self.current_show().is_some()
                && self.slide_index.is_some(),
            label: self.item_label(),
            typography: self.item_typography(),
            item_box,
            filters: self.item_filters(),
            slide_transition: own_transition
                .as_ref()
                .map_or_else(|| "default".to_string(), |t| t.kind.clone()),
            slide_transition_ms: own_transition.map_or(500, |t| t.duration_ms.min(10_000) as i32),
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
            "valign" => {
                if !["top", "center", "bottom"].contains(&value.as_str()) {
                    return Err(format!("invalid vertical alignment {value:?}"));
                }
                self.edit_current_item(|show, slide, item| {
                    show.set_item_valign(slide, item, &value)
                })
            }
            "rotation" => {
                let degrees = parse_int(&value, -360, 360)?.rem_euclid(360);
                let css = if degrees == 0 {
                    String::new()
                } else {
                    format!("rotate({degrees}deg)")
                };
                self.edit_current_item(|show, slide, item| {
                    show.set_item_box_css(slide, item, "transform", &css)
                })
            }
            "text-case" => {
                if !["none", "uppercase", "lowercase", "capitalize"].contains(&value.as_str()) {
                    return Err(format!("invalid text case {value:?}"));
                }
                self.edit_current_item(|show, slide, item| {
                    show.set_item_text_case(slide, item, &value)
                })
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
            "blur" | "grayscale" | "sepia" | "invert" | "brightness" | "contrast" | "saturate"
            | "hue-rotate" | "opacity" | "blend" => {
                let mut filters = self.item_filters();
                match key {
                    "blur" => filters.blur = parse_int(&value, 0, 100)? as i32,
                    "grayscale" => filters.grayscale = parse_int(&value, 0, 100)? as i32,
                    "sepia" => filters.sepia = parse_int(&value, 0, 100)? as i32,
                    "invert" => filters.invert = parse_int(&value, 0, 100)? as i32,
                    "brightness" => filters.brightness = parse_int(&value, 0, 500)? as i32,
                    "contrast" => filters.contrast = parse_int(&value, 0, 500)? as i32,
                    "saturate" => filters.saturate = parse_int(&value, 0, 500)? as i32,
                    "hue-rotate" => filters.hue_rotate = parse_int(&value, 0, 360)? as i32,
                    "opacity" => filters.opacity = parse_int(&value, 0, 100)? as i32,
                    _ => {
                        if !BLEND_MODES.contains(&value.as_str()) {
                            return Err(format!("unknown blend mode {value:?}"));
                        }
                        filters.blend = value.clone();
                    }
                }
                self.edit_current_item(|show, slide, item| {
                    show.set_item_filters(slide, item, &filters)
                })
            }
            "slide-transition" | "slide-transition-ms" => {
                let Some(index) = self.slide_index else {
                    return Err("Pick a slide first".to_string());
                };
                let form = self.inspector_form();
                let (kind, duration) = if key == "slide-transition" {
                    if value != "default" && !TRANSITION_KINDS.contains(&value.as_str()) {
                        return Err(format!("unknown transition {value:?}"));
                    }
                    (value.clone(), form.slide_transition_ms as u64)
                } else {
                    let ms = parse_int(&value, 0, 10_000)?;
                    // A duration on a slide still using the style transition gives it a fade.
                    let kind = if form.slide_transition == "default" {
                        "fade".to_string()
                    } else {
                        form.slide_transition
                    };
                    (kind, ms as u64)
                };
                let kind = if kind == "default" {
                    String::new()
                } else {
                    kind
                };
                self.edit_current_item(|show, _, _| show.set_transition_at(index, &kind, duration))
            }
            "line-height" => {
                let percent = parse_int(&value, 0, 500)? as i32;
                self.edit_current_item(|show, slide, item| {
                    show.set_item_line_height(slide, item, percent)
                })
            }
            "letter-spacing" => {
                let px = parse_int(&value, -300, 100)? as i32;
                self.edit_current_item(|show, slide, item| {
                    show.set_item_letter_spacing(slide, item, px)
                })
            }
            "outline-width" | "outline-color" => {
                let current = self.item_typography();
                let (width, color) = if key == "outline-width" {
                    let width = parse_int(&value, 0, 100)? as i32;
                    (width, current.outline_color)
                } else {
                    if !value.is_empty() && !valid_css_color(&value) {
                        return Err(format!("invalid color {value:?}"));
                    }
                    // Picking a colour with no outline yet gives it a visible width.
                    (current.outline_width.max(4), value.clone())
                };
                self.edit_current_item(|show, slide, item| {
                    show.set_item_outline(slide, item, width, &color)
                })
            }
            "shadow-x" | "shadow-y" | "shadow-blur" | "shadow-color" => {
                let mut shadow = self.item_typography().shadow;
                match key {
                    "shadow-x" => shadow.x = parse_int(&value, -1000, 1000)? as i32,
                    "shadow-y" => shadow.y = parse_int(&value, -1000, 1000)? as i32,
                    "shadow-blur" => shadow.blur = parse_int(&value, 0, 500)? as i32,
                    _ => {
                        if !value.is_empty() && !valid_css_color(&value) {
                            return Err(format!("invalid color {value:?}"));
                        }
                        shadow.color = value.clone();
                        if shadow.x == 0 && shadow.y == 0 && shadow.blur == 0 {
                            shadow = freeshow_data::TextShadow {
                                x: 2,
                                y: 2,
                                blur: 10,
                                color: value.clone(),
                            };
                        }
                    }
                }
                self.edit_current_item(|show, slide, item| {
                    show.set_item_shadow(slide, item, &shadow)
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
                    enabled: item.enabled,
                    id: item.id.clone(),
                    kind: stage_kind_name(&item.kind).to_string(),
                    left: px("left", 0),
                    top: px("top", 0),
                    width: px("width", 1920),
                    height: px("height", 1080),
                    font_px: px("font-size", 100),
                    color: freeshow_data::css_value(&item.style, "color").unwrap_or_default(),
                    title: stage_item_title(item),
                    label: item.label.clone(),
                    align: if item.align.is_empty() {
                        "center".to_string()
                    } else {
                        item.align.clone()
                    },
                    offset: item.offset.clamp(-99, 99) as i32,
                    line_count: item.line_count.unwrap_or(0).min(99) as i32,
                    include_media: item.include_media,
                    text_box: item
                        .text_box
                        .map_or(0, |index| index.saturating_add(1).min(99) as i32),
                    invert_items: item.invert_items,
                    qr_text: item
                        .qr
                        .as_ref()
                        .map_or_else(String::new, |qr| qr.text.clone()),
                    qr_color: item
                        .qr
                        .as_ref()
                        .map_or_else(String::new, |qr| qr.color.clone()),
                    qr_background: item
                        .qr
                        .as_ref()
                        .map_or_else(String::new, |qr| qr.background.clone()),
                    output_source: item.output_source.clone().unwrap_or_default(),
                    show_output_label: item.show_output_label,
                    device_name: item.device_name.clone().unwrap_or_default(),
                    flip_x: item.flip_x,
                    flip_y: item.flip_y,
                    chart_kind: item
                        .chart
                        .as_ref()
                        .map_or_else(|| "bar".into(), |chart| chart.kind.to_ascii_lowercase()),
                    chart_data: item
                        .chart
                        .as_ref()
                        .map_or_else(String::new, |chart| chart.data.clone()),
                    chart_hole_size: item
                        .chart
                        .as_ref()
                        .map_or(0, |chart| i32::from(chart.hole_size)),
                    web_url: item.url.clone().unwrap_or_default(),
                    text: item.lines.join("\n"),
                    seconds: item.clock.seconds,
                    date_format: item.clock.date_format.clone(),
                    timer: item.timer_id.clone().unwrap_or_default(),
                    variable: item.variable_id.clone().unwrap_or_default(),
                    src: item.src.clone().unwrap_or_default(),
                    fit: item.fit.clone().unwrap_or_else(|| "contain".into()),
                    tracker_type: item.tracker.kind.clone(),
                    tracker_accent: item.tracker.accent.clone(),
                }
            })
            .collect()
    }

    /// Sets one geometry or text field on a stage item and saves `Config/stage.json`.
    pub fn stage_set(&mut self, id: &str, key: &str, value: &str) -> Result<(), String> {
        if id.is_empty() {
            return self.stage_set_layout(key, value.trim());
        }
        if matches!(
            key,
            "align"
                | "enabled"
                | "label"
                | "offset"
                | "line-count"
                | "include-media"
                | "text-box"
                | "invert-items"
                | "qr-text"
                | "qr-color"
                | "qr-background"
                | "output-source"
                | "show-output-label"
                | "camera-device"
                | "flip-x"
                | "flip-y"
                | "chart-kind"
                | "chart-data"
                | "chart-hole-size"
                | "web-url"
                | "text"
                | "seconds"
                | "date-format"
                | "timer"
                | "variable"
                | "src"
                | "fit"
                | "tracker-type"
                | "tracker-accent"
        ) {
            return self.stage_set_option(id, key, value);
        }
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

    pub fn stage_box_at(&self, x: f32, y: f32) -> Option<usize> {
        self.stage.items.iter().rposition(|item| {
            let px =
                |key, default| freeshow_data::css_px(&item.style, key).unwrap_or(default) as f32;
            let (left, top) = (px("left", 0), px("top", 0));
            item.enabled
                && x >= left
                && x < left + px("width", 1920)
                && y >= top
                && y < top + px("height", 1080)
        })
    }

    pub fn set_stage_box_geometry(
        &mut self,
        id: &str,
        first: i32,
        second: i32,
        resize: bool,
    ) -> bool {
        let Some(item) = self.stage.items.iter().find(|item| item.id == id) else {
            return false;
        };
        let (horizontal, vertical, minimum) = if resize {
            ("width", "height", 1)
        } else {
            ("left", "top", -8000)
        };
        let style = freeshow_data::css_set(
            &item.style,
            horizontal,
            &format!("{}px", first.clamp(minimum, 8000)),
        );
        let style = freeshow_data::css_set(
            &style,
            vertical,
            &format!("{}px", second.clamp(minimum, 8000)),
        );
        self.set_stage_item_style(id, &style)
    }

    fn stage_set_layout(&mut self, key: &str, value: &str) -> Result<(), String> {
        let field = match key {
            "layout-name" if !value.is_empty() => &mut self.stage.name,
            "layout-name" => return Err("enter a stage layout name".into()),
            "layout-background" if freeshow_output::parse_color(value).is_some() => {
                &mut self.stage.background
            }
            "layout-background" => return Err(format!("invalid stage background {value:?}")),
            _ => return Err(format!("unknown stage layout field {key:?}")),
        };
        let previous = std::mem::replace(field, value.into());
        if self.save_stage_layout() {
            return Ok(());
        }
        if key == "layout-name" {
            self.stage.name = previous;
        } else {
            self.stage.background = previous;
        }
        Err("could not save stage.json".into())
    }

    /// The editor name of the stage item `id`, or the id when it is gone.
    pub fn stage_item_name(&self, id: &str) -> String {
        self.stage
            .items
            .iter()
            .find(|item| item.id == id)
            .map_or_else(|| id.to_string(), stage_item_title)
    }

    /// Named timers a stage timer item can follow, as `(id, name)`.
    pub fn timer_choices(&self) -> Vec<(String, String)> {
        self.timers
            .iter()
            .map(|timer| (timer.id.clone(), timer.name.clone()))
            .collect()
    }

    /// Variables a stage variable item can show, as `(id, name)`.
    pub fn variable_choices(&self) -> Vec<(String, String)> {
        self.root
            .as_deref()
            .map(|root| freeshow_data::Library::open(root).variable_names())
            .unwrap_or_default()
    }

    /// Sets one per-kind option (alignment, slide offset, clock format, timer, text).
    fn stage_set_option(&mut self, id: &str, key: &str, value: &str) -> Result<(), String> {
        let timers: Vec<String> = self.timers.iter().map(|timer| timer.id.clone()).collect();
        let variables: Vec<String> = self
            .variable_choices()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if key == "output-source"
            && !value.is_empty()
            && !self.audience_outputs().iter().any(|(id, _)| id == value)
        {
            return Err(format!("no audience output {value:?}"));
        }
        let index = self
            .stage
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| format!("no stage item {id:?}"))?;
        let item = &mut self.stage.items[index];
        let previous = item.clone();
        match key {
            "enabled" => item.enabled = parse_bool(value)?,
            "label" => item.label = value.trim().into(),
            "align" => {
                let align = value.trim();
                if !matches!(align, "left" | "center" | "right") {
                    return Err(format!("invalid alignment {align:?}"));
                }
                item.align = if align == "center" {
                    String::new()
                } else {
                    align.to_string()
                };
            }
            "offset" => item.offset = parse_int(value, -99, 99)?,
            "line-count" => {
                let count = parse_int(value, 0, 99)?;
                item.line_count = (count > 0).then_some(count as usize);
            }
            "include-media" => item.include_media = parse_bool(value)?,
            "text-box" => {
                let number = parse_int(value, 0, 99)? as usize;
                item.text_box = number.checked_sub(1);
            }
            "invert-items" => item.invert_items = parse_bool(value)?,
            "output-source" if item.kind == freeshow_data::StageItemKind::CurrentOutput => {
                item.output_source = (!value.is_empty()).then(|| value.to_string());
            }
            "show-output-label" if item.kind == freeshow_data::StageItemKind::CurrentOutput => {
                item.show_output_label = parse_bool(value)?;
            }
            "camera-device" if item.kind == freeshow_data::StageItemKind::Camera => {
                item.device_name = (!value.is_empty()).then(|| value.to_string());
                item.device_type = item.device_name.as_ref().map(|_| "camera".into());
            }
            "flip-x" if item.kind == freeshow_data::StageItemKind::Camera => {
                item.flip_x = parse_bool(value)?;
            }
            "flip-y" if item.kind == freeshow_data::StageItemKind::Camera => {
                item.flip_y = parse_bool(value)?;
            }
            "chart-kind" if item.kind == freeshow_data::StageItemKind::Chart => {
                if !matches!(value, "bar" | "line" | "pie") {
                    return Err(format!("unknown chart type {value:?}"));
                }
                item.chart.get_or_insert_with(Default::default).kind = value.into();
            }
            "chart-data" if item.kind == freeshow_data::StageItemKind::Chart => {
                item.chart.get_or_insert_with(Default::default).data = value.into();
            }
            "chart-hole-size" if item.kind == freeshow_data::StageItemKind::Chart => {
                let size = parse_int(value, 0, 90)? as u8;
                item.chart.get_or_insert_with(Default::default).hole_size = size;
            }
            "web-url" if item.kind == freeshow_data::StageItemKind::Web => {
                let url = value.trim();
                item.url = (!url.is_empty()).then(|| url.into());
            }
            "qr-text" | "qr-color" | "qr-background"
                if item.kind == freeshow_data::StageItemKind::QrCode =>
            {
                let value = if key == "qr-text" {
                    value
                } else {
                    value.trim()
                };
                if key != "qr-text"
                    && !(key == "qr-background" && value.is_empty())
                    && freeshow_output::parse_color(value).is_none()
                {
                    return Err(format!("invalid QR color {value:?}"));
                }
                let qr = item.qr.get_or_insert_with(Default::default);
                match key {
                    "qr-text" => qr.text = value.into(),
                    "qr-color" => qr.color = value.into(),
                    _ => qr.background = value.into(),
                }
            }
            "text" => {
                item.lines = value
                    .lines()
                    .map(|line| line.trim_end().to_string())
                    .collect();
            }
            "seconds" => item.clock.seconds = parse_bool(value)?,
            "date-format" => {
                if !STAGE_DATE_FORMATS.contains(&value.trim()) {
                    return Err(format!("unknown date format {value:?}"));
                }
                item.clock.date_format = value.trim().to_string();
            }
            "src" => {
                let src = value.trim();
                if item.kind == freeshow_data::StageItemKind::Web
                    && (src.contains("://") || src.starts_with("www."))
                {
                    return Err("enter an image file path for the still".into());
                }
                item.src = (!src.is_empty()).then(|| src.to_string());
            }
            "tracker-type" => {
                if !matches!(value.trim(), "number" | "bar") {
                    return Err(format!("unknown tracker look {value:?}"));
                }
                item.tracker.kind = value.trim().to_string();
            }
            "tracker-accent" => {
                let accent = value.trim();
                if !accent.is_empty() && !valid_css_color(accent) {
                    return Err(format!("invalid color {accent:?}"));
                }
                item.tracker.accent = accent.to_string();
            }
            "fit" => {
                if !matches!(value.trim(), "contain" | "cover" | "fill") {
                    return Err(format!("unknown fit {value:?}"));
                }
                item.fit = Some(value.trim().to_string());
            }
            "timer" => {
                let timer = value.trim();
                if !timer.is_empty() && !timers.iter().any(|known| known == timer) {
                    return Err(format!("no timer {timer:?}"));
                }
                item.timer_id = (!timer.is_empty()).then(|| timer.to_string());
            }
            "variable" => {
                let variable = value.trim();
                if !variable.is_empty() && !variables.iter().any(|known| known == variable) {
                    return Err(format!("no variable {variable:?}"));
                }
                item.variable_id = (!variable.is_empty()).then(|| variable.to_string());
            }
            other => return Err(format!("unknown stage field {other:?}")),
        }
        if self.save_stage_layout() {
            Ok(())
        } else {
            self.stage.items[index] = previous;
            Err("could not save stage.json".into())
        }
    }

    pub fn move_stage_item(&mut self, id: &str, forward: bool) -> Result<usize, String> {
        let index = self
            .stage
            .items
            .iter()
            .position(|item| item.id == id)
            .ok_or_else(|| format!("no stage item {id:?}"))?;
        let next = index
            .checked_add_signed(if forward { 1 } else { -1 })
            .filter(|next| *next < self.stage.items.len())
            .ok_or("stage item is already at the edge of the stack")?;
        self.stage.items.swap(index, next);
        if self.save_stage_layout() {
            return Ok(next);
        }
        self.stage.items.swap(index, next);
        Err("could not save stage.json".into())
    }

    pub fn remove_stage_item(&mut self, id: &str) -> bool {
        let Some(index) = self.stage.items.iter().position(|item| item.id == id) else {
            return false;
        };
        let item = self.stage.items.remove(index);
        if self.save_stage_layout() {
            return true;
        }
        self.stage.items.insert(index, item);
        false
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
    fn styles_and_outputs_are_edited_from_settings() {
        let (_dir, mut session) = library("styles-outputs");
        let form = session.settings_form();
        assert_eq!(form.style_names, vec!["Primary".to_string()]);
        assert_eq!(form.output_names, vec!["Primary".to_string()]);
        session.apply_setting("styleBackground", "#00b140").unwrap();
        assert!(session.style_background.is_some());
        assert!(
            session
                .apply_setting("styleBackground", "greenish")
                .is_err()
        );
        assert!(session.apply_setting("styleDelete", "").is_err());
        session.apply_setting("styleNew", "").unwrap();
        session.apply_setting("styleName", "Night").unwrap();
        session.apply_setting("styleFit", "blur").unwrap();
        session.apply_setting("styleLines", "3").unwrap();
        assert!(session.apply_setting("styleFit", "stretch").is_err());
        let form = session.settings_form();
        assert_eq!(form.style_name, "Night");
        assert_eq!(form.style_fit, "blur");
        assert_eq!(form.style_lines, 3);
        assert!(
            session.style_background.is_none(),
            "a new style starts black"
        );
        session.apply_setting("outputStyle", "Primary").unwrap();
        assert!(session.style_background.is_some());

        session.apply_setting("outputNew", "").unwrap();
        session.apply_setting("outputName", "Lobby").unwrap();
        let form = session.settings_form();
        assert_eq!(
            form.output_names,
            vec!["Primary".to_string(), "Lobby".to_string()]
        );
        assert_eq!(form.output_name, "Lobby");
        session.apply_setting("outputPick", "Primary").unwrap();
        assert_eq!(session.settings_form().output_name, "Primary");
        assert_eq!(session.settings_form().output_resolution, "1920x1080");
        session
            .apply_setting("outputResolution", "1280x720")
            .unwrap();
        assert_eq!(session.settings_form().output_resolution, "1280x720");
        assert_eq!(
            session.preferences.configured_output_size(),
            Some((1280, 720))
        );
        assert!(session.apply_setting("outputResolution", "huge").is_err());
        assert!(session.apply_setting("outputResolution", "10x10").is_err());
        session.apply_setting("outputDelete", "").unwrap();
        assert_eq!(
            session.settings_form().output_names,
            vec!["Lobby".to_string()]
        );
        assert!(session.apply_setting("outputDelete", "").is_err());
    }

    #[test]
    fn an_output_style_can_lay_slides_out_with_a_template() {
        let (_dir, mut session) = library("style-template");
        session.template_catalog.insert(
            "lower",
            serde_json::json!({"name":"Lower third","items":[{"style":"top:900px;","lines":[{"align":"","text":[{"value":"","style":"font-size:40px;"}]}]}]}),
        );
        let form = session.settings_form();
        assert_eq!(form.style_template, "None");
        assert_eq!(form.template_choices, ["None", "Lower third"]);
        session
            .apply_setting("styleTemplate", "Lower third")
            .unwrap();
        assert_eq!(session.style_template.as_deref(), Some("lower"));
        assert_eq!(session.settings_form().style_template, "Lower third");
        assert!(session.apply_setting("styleTemplate", "Missing").is_err());
        session.apply_setting("styleTemplate", "None").unwrap();
        assert_eq!(session.style_template, None);
    }

    #[test]
    fn the_theme_editor_edits_copies_and_resets_the_library_theme() {
        let (dir, mut session) = library("theme-editor");
        let form = session.settings_form();
        assert_eq!(form.theme, "dark");
        assert_eq!(form.theme_colors.len(), THEME_COLORS.len());
        assert!(!form.theme_edited);
        session
            .apply_setting("themeColor.secondary", "#2ecc71")
            .unwrap();
        assert!(
            session
                .apply_setting("themeColor.secondary", "green-ish")
                .is_err()
        );
        assert!(
            session
                .apply_setting("themeColor.bogus", "#000000")
                .is_err()
        );
        session.apply_setting("themeFontSize", "120").unwrap();
        session.apply_setting("themeRadius", "8").unwrap();
        let form = session.settings_form();
        assert!(form.theme_edited);
        assert_eq!(form.theme_font_size, 120);
        assert_eq!(form.theme_radius, 8);
        let accent = form
            .theme_colors
            .iter()
            .find(|(key, _, _)| key == "secondary")
            .unwrap();
        assert_eq!(accent.2, "#2ecc71");
        session.apply_setting("themeDuplicate", "").unwrap();
        let form = session.settings_form();
        assert_eq!(form.theme, "dark-copy");
        assert!(form.themes.contains(&"dark-copy".to_string()));
        session.apply_setting("themeReset", "").unwrap();
        assert_eq!(session.settings_form().theme, "default");
        session.apply_setting("theme", "dark").unwrap();
        session.apply_setting("themeReset", "").unwrap();
        let form = session.settings_form();
        assert_eq!(form.theme, "dark");
        assert!(!form.theme_edited);
        let saved = std::fs::read_to_string(dir.path().join("Config/themes.json")).unwrap();
        assert!(!saved.contains("dark-copy") && !saved.contains("\"dark\""));
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
        assert!(session.set_stage_box_geometry(&id, 410, -90, false));
        assert!(session.set_stage_box_geometry(&id, 450, 180, true));
        let mut session = Session::open(dir.path().to_path_buf());
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!((row.kind.as_str(), row.left, row.top), ("clock", 410, -90));
        assert_eq!((row.width, row.height), (450, 180));
        assert_eq!(row.color, "#ff0000");
        assert!(session.remove_stage_item(&id));
        assert!(session.stage_rows().iter().all(|row| row.id != id));
    }

    #[test]
    fn reordering_overlapping_stage_items_preserves_hidden_and_unordered_items() {
        let (dir, _) = library("stage-order");
        for (name, pixel) in [
            ("red.png", [200, 0, 0, 255]),
            ("blue.png", [0, 0, 200, 255]),
        ] {
            let image = freeshow_output::Frame {
                width: 16,
                height: 9,
                pixels: pixel.repeat(16 * 9),
            };
            std::fs::write(dir.path().join(name), image.encode_png().unwrap()).unwrap();
        }
        let path = freeshow_data::Library::open(dir.path()).stage_path();
        let style = "left:0px;top:0px;width:160px;height:90px;";
        let hidden = serde_json::json!({"type":"clock","enabled":false,"custom":true});
        let unlisted = serde_json::json!({"type":"media","src":"other.png"});
        std::fs::write(
            &path,
            serde_json::json!({"stage":{
                "items":{
                    "red":{"type":"media","style":style,"src":"red.png"},
                    "blue":{"type":"media","style":style,"src":"blue.png"},
                    "clock1":hidden,
                    "unlisted":unlisted,
                },
                "itemOrder":["red","clock1","blue"]
            }})
            .to_string(),
        )
        .unwrap();

        let mut session = Session::open(dir.path().to_path_buf());
        let pixel = |session: &Session| crate::live::stage_frame(session).pixel(10, 10);
        assert_eq!(session.stage_box_at(10.5, 10.5), Some(2));
        assert_eq!(session.stage_box_at(160.0, 10.0), None);
        assert_eq!(
            pixel(&session),
            freeshow_output::parse_color("#0000c8").unwrap()
        );
        assert!(session.move_stage_item("red", false).is_err());
        assert_eq!(session.move_stage_item("red", true).unwrap(), 1);
        assert_eq!(session.move_stage_item("red", true).unwrap(), 2);
        assert_eq!(
            session.stage.items[session.stage_box_at(10.0, 10.0).unwrap()].id,
            "red"
        );
        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(
            pixel(&session),
            freeshow_output::parse_color("#c80000").unwrap()
        );
        assert!(
            !session
                .stage_rows()
                .iter()
                .find(|row| row.id == "clock1")
                .unwrap()
                .enabled
        );
        session.stage_set("red", "enabled", "false").unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(
            session.stage.items[session.stage_box_at(10.0, 10.0).unwrap()].id,
            "blue"
        );
        assert_eq!(
            pixel(&session),
            freeshow_output::parse_color("#0000c8").unwrap()
        );
        assert!(
            !session
                .stage_rows()
                .iter()
                .find(|row| row.id == "red")
                .unwrap()
                .enabled
        );
        session.stage_set("red", "enabled", "true").unwrap();
        assert_eq!(
            pixel(&session),
            freeshow_output::parse_color("#c80000").unwrap()
        );

        let id = session.add_stage_item("clock", "left:500px;").unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["stage"]["itemOrder"],
            serde_json::json!(["clock1", "blue", "red", id])
        );
        assert_eq!(saved["stage"]["items"]["clock1"], hidden);
        assert_eq!(saved["stage"]["items"]["unlisted"], unlisted);
        std::fs::write(&path, "{bad json").unwrap();
        assert!(session.move_stage_item("red", false).is_err());
        assert_eq!(session.stage.items[2].id, "red");
    }

    #[test]
    fn duplicating_a_stage_layout_keeps_the_full_saved_layout_and_isolates_edits() {
        let (dir, _) = library("stage-duplicate");
        let library = freeshow_data::Library::open(dir.path());
        let path = library.stage_path();
        let original = serde_json::json!({
            "name": "Original",
            "settings":{"color":"#2468ac","custom":[1,2]},
            "items":{
                "notes":{"type":"slide_notes","style":"left:7px;"},
                "hidden":{"type":"clock","enabled":false,"custom":true}
            },
            "itemOrder":["hidden","notes"]
        });
        std::fs::write(&path, serde_json::json!({"source":original}).to_string()).unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        session.create_stage_layout(true).unwrap();
        let id = session.stage_layout_id.clone().unwrap();
        assert_ne!(id, "source");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut expected = original.clone();
        expected["name"] = serde_json::json!("Original copy");
        assert_eq!(saved[&id], expected);
        assert_eq!(saved["source"], original);

        session.stage_set("notes", "left", "42").unwrap();
        let reopened = Session::open(dir.path().to_path_buf());
        assert_eq!(reopened.stage.name, "Original copy");
        assert_eq!(
            reopened
                .stage_rows()
                .iter()
                .find(|row| row.id == "notes")
                .unwrap()
                .left,
            42
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["source"], original);
    }

    #[test]
    fn creating_and_editing_a_stage_layout_preserves_a_legacy_layout() {
        let (dir, _) = library("stage-create");
        let library = freeshow_data::Library::open(dir.path());
        let path = library.stage_path();
        let legacy = serde_json::json!({
            "name": "Existing",
            "settings": {"color": "#111111", "custom": true},
            "items": {"clock": {"type": "clock"}},
        });
        std::fs::write(&path, legacy.to_string()).unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        session.create_stage_layout(false).unwrap();
        let id = session.stage_layout_id.clone().unwrap();
        assert!(session.stage.items.is_empty());
        assert_eq!(session.stage.name, "New layout");
        session
            .stage_set("", "layout-name", "  Rehearsal  ")
            .unwrap();
        session
            .stage_set("", "layout-background", "#2468ac")
            .unwrap();
        assert!(
            session
                .stage_set("", "layout-background", "invalid")
                .is_err()
        );

        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(session.stage_layout_id.as_deref(), Some(id.as_str()));
        assert_eq!(session.stage.name, "Rehearsal");
        assert_eq!(
            crate::live::stage_frame(&session).pixel(0, 0),
            freeshow_output::parse_color("#2468ac").unwrap()
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["default"], legacy);

        std::fs::write(&path, "{bad json").unwrap();
        assert!(session.create_stage_layout(false).is_err());
        assert_eq!(session.stage.name, "Rehearsal");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{bad json");
    }

    #[test]
    fn stage_layout_selection_persists_and_only_changes_the_primary_stage_output() {
        let (dir, _) = library("stage-picker");
        let library = freeshow_data::Library::open(dir.path());
        std::fs::write(
            library.stage_path(),
            r#"{"a":{"name":"Clock","items":{"box":{"type":"clock"}}},"b":{"name":"Notes","items":{"box":{"type":"slide_notes"}}},"c":{"name":"Hidden","disabled":true,"items":{}}}"#,
        )
        .unwrap();
        assert_eq!(
            library.stage_layout_choices(),
            vec![("a".into(), "Clock".into()), ("b".into(), "Notes".into())]
        );
        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(session.stage.name, "Clock");
        assert!(session.select_stage_layout(Some("audience"), "b").is_err());
        assert_eq!(session.stage.name, "Clock");

        session.select_primary_stage_layout("b").unwrap();
        session.stage_set("box", "left", "42").unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        assert_eq!(session.stage.name, "Notes");
        assert_eq!(session.stage_rows()[0].left, 42);
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(library.settings_path()).unwrap())
                .unwrap();
        assert_eq!(settings["nativeStageLayout"], "b");
        assert_eq!(settings["unknownKey"]["keep"], 1);

        std::fs::write(library.settings_path(), "{bad json").unwrap();
        assert!(session.select_primary_stage_layout("a").is_err());
        assert_eq!(session.stage.name, "Notes");
        assert_eq!(
            std::fs::read_to_string(library.settings_path()).unwrap(),
            "{bad json"
        );

        std::fs::write(
            library.settings_path(),
            r#"{"outputs":{"first":{"stageOutput":"a"},"second":{"stageOutput":"a"}}}"#,
        )
        .unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        session.select_primary_stage_layout("b").unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(library.settings_path()).unwrap())
                .unwrap();
        assert_eq!(settings["outputs"]["first"]["stageOutput"], "b");
        assert_eq!(settings["outputs"]["second"]["stageOutput"], "a");
    }

    #[test]
    fn a_missing_stage_layout_does_not_redirect_or_apply_edits() {
        let (dir, _) = library("stage-missing");
        let path = freeshow_data::Library::open(dir.path()).stage_path();
        std::fs::write(
            &path,
            r#"{"a":{"items":{"text":{"type":"text","lines":["Before"]}}},"b":{"items":{}}}"#,
        )
        .unwrap();
        let mut session = Session::open(dir.path().to_path_buf());
        let stage = session.stage.clone();
        let shortened = r#"{"b":{"items":{}}}"#;
        std::fs::write(&path, shortened).unwrap();

        assert!(session.stage_set("text", "text", "After").is_err());
        assert!(session.stage_set("text", "left", "20").is_err());
        assert!(!session.set_stage_box_geometry("text", 20, 30, false));
        assert!(!session.set_stage_box_geometry("text", 300, 200, true));
        assert!(session.add_stage_item("text", "").is_none());
        assert!(!session.remove_stage_item("text"));
        assert_eq!(session.stage, stage);
        assert_eq!(std::fs::read_to_string(path).unwrap(), shortened);
    }

    #[test]
    fn stage_box_labels_and_notes_offsets_survive_reopening() {
        let (dir, mut session) = library("stage-box-label");
        let id = session.add_stage_item("notes", "left:10px;").unwrap();
        session.stage_set(&id, "offset", "1").unwrap();
        assert_eq!(session.stage_item_name(&id), "Next slide notes");
        session.stage_set(&id, "label", "  Speaker cue  ").unwrap();

        let mut reopened = Session::open(dir.path().to_path_buf());
        let row = reopened
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(row.offset, 1);
        assert_eq!(row.title, "Speaker cue");
        assert_eq!(row.label, "Speaker cue");
        reopened.stage_set(&id, "label", "").unwrap();
        let reopened = Session::open(dir.path().to_path_buf());
        assert_eq!(reopened.stage_item_name(&id), "Next slide notes");
    }

    #[test]
    fn stage_slide_options_reopen_with_the_saved_values() {
        let (dir, mut session) = library("stage-slide-options");
        let id = session.add_stage_item("slide", "left:10px;").unwrap();
        session.stage_set(&id, "include-media", "true").unwrap();
        session.stage_set(&id, "text-box", "2").unwrap();
        session.stage_set(&id, "invert-items", "true").unwrap();

        let mut reopened = Session::open(dir.path().to_path_buf());
        let row = reopened
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert!(row.include_media && row.invert_items);
        assert_eq!(row.text_box, 2);
        reopened.stage_set(&id, "include-media", "false").unwrap();
        reopened.stage_set(&id, "text-box", "0").unwrap();
        reopened.stage_set(&id, "invert-items", "false").unwrap();
        let saved = freeshow_data::Library::open(dir.path()).load_stage_layout();
        let item = saved.items.iter().find(|item| item.id == id).unwrap();
        assert!(!item.include_media && !item.invert_items);
        assert_eq!(item.text_box, None);
    }

    #[test]
    fn stage_qr_options_reopen_and_render() {
        let (dir, mut session) = library("stage-qr");
        let id = session
            .add_stage_item("qr", "left:0px;top:0px;width:320px;height:160px;")
            .unwrap();
        session.stage_set(&id, "qr-text", "  Welcome  ").unwrap();
        assert!(session.stage_set(&id, "qr-color", "invalid").is_err());
        session.stage_set(&id, "qr-color", "#d02483").unwrap();
        session.stage_set(&id, "qr-background", "#f3e9dc").unwrap();

        let path = freeshow_data::Library::open(dir.path()).stage_path();
        let mut saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let layout_id = saved.as_object().unwrap().keys().next().unwrap().clone();
        saved[&layout_id]["items"][&id]["qr_code"]["custom"] = serde_json::json!(true);
        std::fs::write(&path, saved.to_string()).unwrap();

        let mut reopened = Session::open(dir.path().to_path_buf());
        let row = reopened
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(row.kind, "qr");
        assert_eq!(row.qr_text, "  Welcome  ");
        assert_eq!(row.qr_color, "#d02483");
        assert_eq!(row.qr_background, "#f3e9dc");
        let frame = crate::live::stage_frame(&reopened);
        assert!(
            frame
                .pixels
                .chunks_exact(4)
                .any(|pixel| pixel == [208, 36, 131, 255])
        );
        assert_eq!(
            frame.pixel(80, 0),
            freeshow_output::parse_color("#f3e9dc").unwrap()
        );

        reopened.stage_set(&id, "qr-background", "").unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let qr = &saved[&layout_id]["items"][&id]["qr_code"];
        assert_eq!(qr["background"], "");
        assert_eq!(qr["custom"], true);
    }

    #[test]
    fn stage_camera_edits_keep_the_device_and_orientation_after_reopening() {
        let (dir, mut session) = library("stage-camera");
        let id = session.add_stage_item("camera", "left:10px;").unwrap();
        session
            .stage_set(&id, "camera-device", "Front Camera")
            .unwrap();
        session.stage_set(&id, "fit", "cover").unwrap();
        session.stage_set(&id, "flip-x", "true").unwrap();
        session.stage_set(&id, "flip-y", "true").unwrap();

        let library = freeshow_data::Library::open(dir.path());
        let path = library.stage_path();
        let mut saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let layout = saved.as_object().unwrap().keys().next().unwrap().clone();
        assert_eq!(saved[&layout]["items"][&id]["device"]["type"], "camera");
        saved[&layout]["items"][&id]["device"]["extension"] = serde_json::json!(1);
        std::fs::write(&path, saved.to_string()).unwrap();

        let mut reopened = Session::open(dir.path().to_path_buf());
        let row = reopened
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(row.kind, "camera");
        assert_eq!(row.device_name, "Front Camera");
        assert_eq!(row.fit, "cover");
        assert!(row.flip_x && row.flip_y);
        reopened.stage_set(&id, "camera-device", "").unwrap();
        reopened.stage_set(&id, "flip-x", "false").unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let item = &saved[&layout]["items"][&id];
        assert_eq!(item["device"], serde_json::json!({"extension": 1}));
        assert_eq!(item["flipped"], false);
        assert_eq!(item["flippedY"], true);
    }

    #[test]
    fn stage_chart_edits_reopen_and_render() {
        let (dir, mut session) = library("stage-chart");
        let id = session
            .add_stage_item("chart", "left:0px;top:0px;width:400px;height:240px;")
            .unwrap();
        let values = r##"[["A",10,"#c01054"],["B",20,"#365686"]]"##;
        session.stage_set(&id, "chart-kind", "pie").unwrap();
        session.stage_set(&id, "chart-data", values).unwrap();
        session.stage_set(&id, "chart-hole-size", "40").unwrap();
        assert!(session.stage_set(&id, "chart-hole-size", "91").is_err());
        assert!(session.stage_set(&id, "chart-kind", "unknown").is_err());

        let path = freeshow_data::Library::open(dir.path()).stage_path();
        let mut saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let layout = saved.as_object().unwrap().keys().next().unwrap().clone();
        saved[&layout]["items"][&id]["chart"]["extension"] = serde_json::json!(true);
        std::fs::write(&path, saved.to_string()).unwrap();

        let mut reopened = Session::open(dir.path().to_path_buf());
        let row = reopened
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(
            (row.kind.as_str(), row.chart_kind.as_str()),
            ("chart", "pie")
        );
        assert_eq!(row.chart_data, values);
        assert_eq!(row.chart_hole_size, 40);
        let frame = crate::live::stage_frame(&reopened);
        assert!(
            frame
                .pixels
                .chunks_exact(4)
                .any(|pixel| pixel == [192, 16, 84, 255])
        );

        reopened.stage_set(&id, "chart-data", "A: 5\nB: 6").unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let chart = &saved[&layout]["items"][&id]["chart"];
        assert_eq!(chart["data"], "A: 5\nB: 6");
        assert_eq!(chart["extension"], true);
    }

    #[test]
    fn stage_web_previews_keep_url_and_still_edits_after_reopening() {
        let (dir, mut session) = library("stage-web");
        let id = session
            .add_stage_item("web", "left:0px;top:0px;width:320px;height:180px;")
            .unwrap();
        let path = freeshow_data::Library::open(dir.path()).stage_path();
        let mut saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let layout = saved.as_object().unwrap().keys().next().unwrap().clone();
        let item = &mut saved[&layout]["items"][&id];
        item["web"] = serde_json::json!({"url": "https://old.example", "extension": true});
        item["src"] = serde_json::json!("https://old.example");
        std::fs::write(&path, saved.to_string()).unwrap();

        let mut session = Session::open(dir.path().to_path_buf());
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(row.web_url, "https://old.example");
        assert!(row.src.is_empty());

        let image = freeshow_output::Frame {
            width: 16,
            height: 9,
            pixels: [192, 16, 84, 255].repeat(16 * 9),
        };
        std::fs::write(dir.path().join("preview.png"), image.encode_png().unwrap()).unwrap();
        let address = "https://example.test/schedule";
        session.stage_set(&id, "web-url", address).unwrap();
        session.stage_set(&id, "src", "preview.png").unwrap();

        let mut session = Session::open(dir.path().to_path_buf());
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(row.kind, "web");
        assert_eq!(row.web_url, address);
        assert_eq!(row.src, "preview.png");
        assert_eq!(
            crate::live::stage_frame(&session).pixel(310, 165),
            image.pixel(0, 0)
        );

        session.stage_set(&id, "src", "").unwrap();
        assert_eq!(
            crate::live::stage_frame(&session).pixel(310, 165),
            freeshow_output::parse_color("#1a1a24").unwrap()
        );
        session.stage_set(&id, "web-url", "").unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved[&layout]["items"][&id]["web"],
            serde_json::json!({"extension": true})
        );
        let reloaded = freeshow_data::Library::open(dir.path()).load_stage_layout();
        let item = reloaded.items.iter().find(|item| item.id == id).unwrap();
        assert!(item.url.is_none() && item.src.is_none());
    }

    #[test]
    fn a_stage_variable_item_shows_the_variable_it_is_given() {
        let (dir, mut session) = library("stage-variable");
        std::fs::write(
            dir.path().join("Config").join("settings_synced.json"),
            r#"{"variables":{"v1":{"name":"Speaker","type":"text","text":"Pastor Ram"}}}"#,
        )
        .unwrap();
        let id = session
            .add_stage_item("variable", "left:10px;top:20px;width:600px;height:100px;")
            .unwrap();
        assert_eq!(session.stage_item_name(&id), "Variable");
        assert_eq!(
            session.variable_choices(),
            vec![("v1".to_string(), "Speaker".to_string())]
        );
        assert!(session.stage_set(&id, "variable", "v9").is_err());
        session.stage_set(&id, "variable", "v1").unwrap();
        let row = session
            .stage_rows()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(
            (row.kind.as_str(), row.variable.as_str()),
            ("variable", "v1")
        );
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("Config").join("stage.json")).unwrap(),
        )
        .unwrap();
        let layout = saved.as_object().unwrap().values().next().unwrap();
        assert_eq!(layout["items"][&id]["variable"]["id"], "v1");
        assert_eq!(layout["items"][&id]["type"], "variable");
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
