// SPDX-License-Identifier: GPL-3.0-only

use freeshow_core::{Rgb, Rgba, ThemePalette, theme_by_name};
use freeshow_data::FreeShowPreferences;
use freeshow_output::parse_color;

pub struct OperatorTheme {
    pub palette: ThemePalette,
    pub font_family: String,
    pub font_scale: f32,
    /// Corner radius for chrome controls, in pixels. Built-in themes use 4.
    pub border_radius: f32,
}

pub fn operator_theme(explicit: Option<&str>, preferences: &FreeShowPreferences) -> OperatorTheme {
    let name = explicit
        .or(preferences.theme.as_deref())
        .unwrap_or("default");
    let mut palette = theme_by_name(name);
    let (family, scale) = match palette.name.to_ascii_lowercase().as_str() {
        "dark" => ("monospace", 1.1),
        "white" | "blue" => ("Trebuchet MS", 1.0),
        "sea" => ("Georgia", 1.0),
        "papyrus" => ("Papyrus", 0.9),
        "terminal" => ("monospace", 1.0),
        _ => ("", 1.0),
    };
    let custom = &preferences.themes[name];
    for (key, target) in [
        ("primary", &mut palette.primary),
        ("primary-lighter", &mut palette.primary_lighter),
        ("primary-darker", &mut palette.primary_darker),
        ("primary-darkest", &mut palette.primary_darkest),
        ("text", &mut palette.text),
        ("textInvert", &mut palette.text_invert),
        ("secondary-text", &mut palette.secondary_text),
        ("secondary", &mut palette.secondary),
    ] {
        if let Some(color) = custom["colors"][key].as_str().and_then(parse_color) {
            *target = Rgb {
                r: color.r,
                g: color.g,
                b: color.b,
            };
        }
    }
    for (key, target) in [("hover", &mut palette.hover), ("focus", &mut palette.focus)] {
        if let Some(color) = custom["colors"][key].as_str().and_then(parse_color) {
            *target = Rgba {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            };
        }
    }
    let font_family = custom["font"]["family"].as_str().unwrap_or(family).trim();
    let font_family = if font_family == "sans-serif" {
        ""
    } else {
        font_family
    };
    let font_scale = custom["font"]["size"]
        .as_str()
        .and_then(|size| size.trim().strip_suffix("em"))
        .and_then(|size| size.trim().parse::<f32>().ok())
        .filter(|size| size.is_finite() && *size > 0.0)
        .unwrap_or(scale);
    let border_radius = custom
        .pointer("/border/radius")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_radius)
        .unwrap_or(4.0);
    OperatorTheme {
        palette,
        font_family: font_family.into(),
        font_scale,
        border_radius,
    }
}

fn parse_radius(value: &str) -> Option<f32> {
    let text = value.trim();
    let text = text.strip_suffix("px").unwrap_or(text).trim();
    let size: f32 = text.parse().ok()?;
    (size.is_finite() && size >= 0.0).then_some(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn library_theme_uses_stored_colors_and_font_with_explicit_override() {
        let preferences = FreeShowPreferences {
            theme: Some("custom-id".into()),
            themes: json!({"custom-id": {
                "name": "Sunday",
                "colors": {"primary": "#123456", "secondary": "rgb(10 20 30)",
                    "text": "invalid", "primary-darker": null,
                    "hover": "rgb(255 0 0 / 0.2)", "focus": "rgba(0, 255, 0, 0.4)"},
                "font": {"family": "Georgia", "size": "1.4em"},
                "border": {"radius": "8px"}
            }}),
            ..Default::default()
        };
        let theme = operator_theme(None, &preferences);
        assert_eq!(theme.palette.primary, Rgb::from_hex(0x123456));
        assert_eq!(theme.palette.secondary, Rgb::from_hex(0x0a141e));
        assert_eq!(theme.palette.text, theme_by_name("default").text);
        assert_eq!(theme.font_family, "Georgia");
        assert_eq!(theme.font_scale, 1.4);
        assert_eq!(theme.border_radius, 8.0);
        assert_eq!(
            theme.palette.hover,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 51
            }
        );
        assert_eq!(
            theme.palette.focus,
            Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 102
            }
        );
        let override_theme = operator_theme(Some("sea"), &preferences);
        assert_eq!(override_theme.palette, theme_by_name("sea"));
        assert_eq!(override_theme.font_family, "Georgia");
        assert_eq!(override_theme.font_scale, 1.0);
        assert_eq!(override_theme.border_radius, 4.0);
    }

    #[test]
    fn malformed_fonts_and_missing_themes_keep_defaults() {
        for size in ["NaNem", "infem", "-1em", "0em", "100px", "bad"] {
            let preferences = FreeShowPreferences {
                themes: json!({"dark": {"font": {"family": 5, "size": size}}}),
                ..Default::default()
            };
            let theme = operator_theme(Some("dark"), &preferences);
            assert_eq!(theme.font_scale, 1.1);
            assert_eq!(theme.font_family, "monospace");
        }
        let theme = operator_theme(Some("missing"), &FreeShowPreferences::default());
        assert_eq!(theme.palette, theme_by_name("default"));
        assert_eq!(theme.font_scale, 1.0);
        assert_eq!(theme.border_radius, 4.0);
        let zero = FreeShowPreferences {
            themes: json!({"default": {"border": {"radius": "0"}}}),
            ..Default::default()
        };
        assert_eq!(operator_theme(None, &zero).border_radius, 0.0);
        assert_eq!(parse_radius("12px"), Some(12.0));
        assert_eq!(parse_radius("bad"), None);
    }
}
