// SPDX-License-Identifier: GPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: None,
            data_path: None,
            show_output: true,
            show_stage: true,
            output_screen: None,
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
