// SPDX-License-Identifier: GPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Pixel placement of one operator or output window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowPlacement {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// Parses `x,y,width,height`.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<_> = text.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return None;
        }
        Some(Self::new(
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
            parts[3].parse().ok()?,
        ))
    }

    pub fn as_text(&self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

/// Named placements of the control, audience, and stage windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WindowProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<WindowPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<WindowPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<WindowPlacement>,
}

impl WindowProfile {
    pub fn is_empty(&self) -> bool {
        self.control.is_none() && self.output.is_none() && self.stage.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    /// An explicit theme choice for this app. `None` uses the FreeShow library
    /// theme, then the default theme.
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub data_path: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub show_output: bool,
    #[serde(default = "default_true")]
    pub show_stage: bool,
    /// Monitor index for the audience window. Empty means the second monitor when one is connected.
    #[serde(default)]
    pub output_screen: Option<usize>,
    /// FreeShow speaker node IDs mapped to native output device names.
    #[serde(default)]
    pub audio_devices: std::collections::BTreeMap<String, String>,
    /// Named window-layout profiles (control, output, and stage geometry).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub window_profiles: std::collections::BTreeMap<String, WindowProfile>,
    /// Profile applied when the operator windows open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_window_profile: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: None,
            data_path: None,
            show_output: true,
            show_stage: true,
            output_screen: None,
            audio_devices: Default::default(),
            window_profiles: Default::default(),
            active_window_profile: None,
        }
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl AppSettings {
    /// Loads settings. A missing file is the default configuration, not an error.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(SettingsError::Json),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(SettingsError::Io(err)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(SettingsError::Io)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(SettingsError::Json)?;
        fs::write(path, text).map_err(SettingsError::Io)
    }

    pub fn window_profile(&self, name: &str) -> Option<&WindowProfile> {
        self.window_profiles.get(name)
    }

    /// Stores a named layout. The first saved profile becomes the active one.
    pub fn save_window_profile(&mut self, name: impl Into<String>, profile: WindowProfile) {
        let name = name.into();
        if self.active_window_profile.is_none() {
            self.active_window_profile = Some(name.clone());
        }
        self.window_profiles.insert(name, profile);
    }

    /// Chooses the profile applied on the next open. `None` clears the choice.
    pub fn set_active_window_profile(&mut self, name: Option<String>) -> bool {
        match name {
            None => {
                self.active_window_profile = None;
                true
            }
            Some(name) if self.window_profiles.contains_key(&name) => {
                self.active_window_profile = Some(name);
                true
            }
            Some(_) => false,
        }
    }

    pub fn active_profile(&self) -> Option<&WindowProfile> {
        self.active_window_profile
            .as_deref()
            .and_then(|name| self.window_profiles.get(name))
    }

    /// Editor text: `active=name` plus `name.control=x,y,w,h` lines.
    pub fn profile_text(&self) -> String {
        let mut lines = vec![format!(
            "active={}",
            self.active_window_profile.clone().unwrap_or_default()
        )];
        for (name, profile) in &self.window_profiles {
            if let Some(place) = &profile.control {
                lines.push(format!("{name}.control={}", place.as_text()));
            }
            if let Some(place) = &profile.output {
                lines.push(format!("{name}.output={}", place.as_text()));
            }
            if let Some(place) = &profile.stage {
                lines.push(format!("{name}.stage={}", place.as_text()));
            }
        }
        lines.join("\n")
    }

    /// Replaces window profiles from [`Self::profile_text`] format.
    pub fn apply_profile_text(&mut self, text: &str) {
        let mut profiles = std::collections::BTreeMap::new();
        let mut active = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key.eq_ignore_ascii_case("active") {
                active = (!value.is_empty()).then(|| value.to_string());
                continue;
            }
            let Some((name, slot)) = key.split_once('.') else {
                continue;
            };
            let Some(place) = WindowPlacement::parse(value) else {
                continue;
            };
            let entry = profiles
                .entry(name.to_string())
                .or_insert_with(WindowProfile::default);
            match slot.trim().to_ascii_lowercase().as_str() {
                "control" => entry.control = Some(place),
                "output" | "audience" => entry.output = Some(place),
                "stage" => entry.stage = Some(place),
                _ => {}
            }
        }
        profiles.retain(|_, profile| !profile.is_empty());
        if let Some(name) = active.as_ref()
            && !profiles.contains_key(name)
        {
            active = None;
        }
        self.window_profiles = profiles;
        self.active_window_profile = active;
    }
}

/// Directory for this app's own settings and log.
///
/// `FREESHOW_CONFIG_DIR` overrides the platform default. This is separate from
/// the FreeShow library folder stored in [`AppSettings::data_path`].
/// Monitor that should receive the audience window.
///
/// A saved index is used while that monitor is connected. Otherwise the second
/// monitor is used when there is one. `None` keeps the window on the operator display.
pub fn output_monitor_index(saved: Option<usize>, monitor_count: usize) -> Option<usize> {
    if monitor_count == 0 {
        return None;
    }
    if let Some(index) = saved
        && index < monitor_count
    {
        return Some(index);
    }
    if monitor_count > 1 { Some(1) } else { None }
}

/// The next monitor after the one currently chosen, wrapping around the list.
pub fn next_output_monitor(saved: Option<usize>, monitor_count: usize) -> Option<usize> {
    if monitor_count == 0 {
        return None;
    }
    let current = output_monitor_index(saved, monitor_count).unwrap_or(0);
    Some((current + 1) % monitor_count)
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FREESHOW_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    platform_config_dir().unwrap_or_else(|| PathBuf::from("freeshow-rust"))
}

fn platform_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/freeshow-rust"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("freeshow-rust"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("freeshow-rust"));
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/freeshow-rust"))
    }
}
