// SPDX-License-Identifier: GPL-3.0-only

//! Shared settings, FreeShow theme colors, and logging.

mod audio;
pub use audio::{ICECAST_BUS, IcecastDestination, NetworkRoute, SpeakerRoute, speaker_matrix};

mod log;
mod settings;
mod theme;

pub use log::{init_logging, log_file_path};
pub use settings::{
    AppSettings, SettingsError, config_dir, next_output_monitor, output_monitor_index,
};
pub use theme::{Rgb, Rgba, ThemePalette, theme_by_name};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn default_theme_matches_freeshow() {
        let theme = theme_by_name("default");
        assert_eq!(theme.name, "default");
        assert_eq!(theme.primary, Rgb::from_hex(0x242832));
        assert_eq!(theme.primary_lighter, Rgb::from_hex(0x2f3542));
        assert_eq!(theme.primary_darker, Rgb::from_hex(0x191923));
        assert_eq!(theme.primary_darkest, Rgb::from_hex(0x12121c));
        assert_eq!(theme.text, Rgb::from_hex(0xf0f0ff));
        assert_eq!(theme.text_invert, Rgb::from_hex(0x131313));
        assert_eq!(theme.secondary_text, Rgb::from_hex(0xf0f0ff));
        assert_eq!(theme.secondary, Rgb::from_hex(0xf0008c));
        assert_eq!(theme.hover, Rgba::from_rgb_frac(255, 255, 255, 50));
        assert_eq!(theme.focus, Rgba::from_rgb_frac(255, 255, 255, 100));
    }

    #[test]
    fn built_in_hover_and_focus_washes_match_freeshow() {
        use crate::theme::Rgba;
        assert_eq!(
            theme_by_name("light").hover,
            Rgba::from_rgb_frac(0, 0, 0, 50)
        );
        assert_eq!(
            theme_by_name("light").focus,
            Rgba::from_rgb_frac(0, 0, 0, 100)
        );
        assert_eq!(
            theme_by_name("black").hover,
            Rgba::from_rgb_frac(255, 255, 255, 200)
        );
        assert_eq!(
            theme_by_name("black").focus,
            Rgba::from_rgb_frac(255, 255, 255, 300)
        );
        assert_eq!(
            theme_by_name("charcoal").hover,
            Rgba::from_rgb_frac(0xd0, 0xd0, 0xd0, 50)
        );
        assert_eq!(
            theme_by_name("papyrus").focus,
            Rgba::from_rgb_frac(0xea, 0xc6, 0xa4, 300)
        );
        assert_eq!(theme_by_name("sea").hover, Rgba::from_rgb_frac(0, 0, 0, 50));
        assert_eq!(
            theme_by_name("aqua").focus,
            Rgba::from_rgb_frac(255, 255, 255, 300)
        );
    }

    #[test]
    fn named_themes_match_freeshow_defaults() {
        assert_eq!(theme_by_name("dark").secondary, Rgb::from_hex(0xe64934));
        assert_eq!(theme_by_name("light").primary, Rgb::from_hex(0xDADDE2));
        assert_eq!(theme_by_name("light").text, Rgb::from_hex(0x333748));
        assert_eq!(theme_by_name("blue").primary, Rgb::from_hex(0x323743));
        assert_eq!(theme_by_name("sea").secondary, Rgb::from_hex(0x007db3));
        assert_eq!(theme_by_name("aqua").secondary, Rgb::from_hex(0x00ffbe));
        assert_eq!(theme_by_name("charcoal").text, Rgb::from_hex(0xd0d0d0));
        assert_eq!(theme_by_name("papyrus").primary, Rgb::from_hex(0xeac6a4));
        assert_eq!(theme_by_name("terminal").secondary, Rgb::from_hex(0x00ff00));
        assert_eq!(theme_by_name("white").secondary, Rgb::from_hex(0xfbe1fe));
        assert_eq!(theme_by_name("black").primary, Rgb::from_hex(0x000000));
        assert_eq!(theme_by_name("no-such-theme").name, "default");
    }

    #[test]
    fn every_built_in_theme_is_reachable_by_key_and_display_name() {
        use crate::theme::BUILT_IN_THEMES;

        assert_eq!(BUILT_IN_THEMES.len(), 11);
        for theme in BUILT_IN_THEMES {
            let by_display = theme_by_name(theme.name);
            assert_eq!(by_display, theme, "{} by display name", theme.name);
            let by_lower = theme_by_name(&theme.name.to_lowercase());
            assert_eq!(by_lower, theme, "{} by lower-case key", theme.name);
        }
    }

    #[test]
    fn missing_settings_file_uses_defaults() {
        let dir =
            std::env::temp_dir().join(format!("freeshow-rust-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let loaded = AppSettings::load(&dir.join("settings.json")).unwrap();
        assert_eq!(loaded.theme, None);
        assert_eq!(loaded.data_path, None);
        assert!(loaded.show_output);
        assert!(loaded.show_stage);
        assert_eq!(loaded.output_screen, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("freeshow-rust-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let settings = AppSettings {
            theme: Some("dark".to_string()),
            data_path: Some(std::path::PathBuf::from("/tmp/FreeShow")),
            show_output: true,
            show_stage: false,
            output_screen: Some(1),
            audio_devices: [("speaker_sub_id".into(), "Stage Speakers".into())].into(),
        };
        settings.save(&path).unwrap();
        let loaded = AppSettings::load(&path).unwrap();
        assert_eq!(loaded, settings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_screen_uses_the_saved_monitor_or_the_second_one() {
        assert_eq!(output_monitor_index(None, 0), None);
        assert_eq!(output_monitor_index(None, 1), None);
        assert_eq!(output_monitor_index(None, 2), Some(1));
        assert_eq!(output_monitor_index(Some(0), 2), Some(0));
        assert_eq!(output_monitor_index(Some(4), 2), Some(1));
        assert_eq!(next_output_monitor(None, 2), Some(0));
        assert_eq!(next_output_monitor(Some(0), 2), Some(1));
        assert_eq!(next_output_monitor(Some(1), 3), Some(2));
        assert_eq!(next_output_monitor(Some(2), 3), Some(0));
    }

    #[test]
    fn corrupt_settings_are_an_error() {
        let dir = std::env::temp_dir().join(format!("freeshow-rust-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, "not json").unwrap();
        assert!(AppSettings::load(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_dir_honors_override_and_log_name() {
        let override_dir = std::env::temp_dir().join("freeshow-rust-config-override");
        // SAFETY: this test is the only one that reads or writes the variable.
        unsafe { std::env::set_var("FREESHOW_CONFIG_DIR", &override_dir) };
        assert_eq!(config_dir(), override_dir);
        assert_eq!(
            log_file_path(&override_dir),
            override_dir.join("freeshow-rust.log")
        );
        unsafe { std::env::remove_var("FREESHOW_CONFIG_DIR") };
    }

    #[test]
    fn init_logging_creates_a_log_file() {
        let dir = std::env::temp_dir().join(format!("freeshow-rust-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = init_logging(&dir).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("freeshow-rust"));
        let _ = fs::remove_dir_all(&dir);
    }
}
