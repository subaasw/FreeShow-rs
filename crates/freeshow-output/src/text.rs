// SPDX-License-Identifier: GPL-3.0-only

//! Proportional text for slide items.
//!
//! Face lookup, in order: the hardcoded Arial paths when the family is Arial;
//! fontconfig (`fc-match`) on Unix, rejecting a fallback family that does not
//! match; a scan of the font folders DirectWrite and Core Text use
//! (`%WINDIR%\\Fonts`, macOS Library and System fonts, Linux
//! `/usr/share/fonts` and user font dirs); then the DejaVu Sans face bundled
//! under its license in `assets/`. Glyph coverage is blended, so edges are
//! not the hard 8×8 blocks of the debug font.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontVec, PxScale, point};

use crate::{Rgba, paint};

const FACE: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

pub struct Clip {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy)]
pub struct Typeface<'a> {
    pub family: &'a str,
    pub bold: bool,
    pub italic: bool,
}

fn face() -> &'static FontVec {
    static FONT: OnceLock<FontVec> = OnceLock::new();
    FONT.get_or_init(|| {
        FontVec::try_from_vec(FACE.to_vec()).expect("DejaVu Sans is a valid TrueType font")
    })
}

pub fn line_height(font_px: f32, face: Typeface<'_>) -> i32 {
    with_face(face, |font| {
        let factor = factor(font, font_px);
        ((font.ascent_unscaled() - font.descent_unscaled() + font.line_gap_unscaled()) * factor)
            .ceil()
            .max(1.0) as i32
    })
}

pub fn measure(text: &str, font_px: f32, face: Typeface<'_>, tracking: f32, word_gap: f32) -> i32 {
    with_face(face, |font| {
        let factor = factor(font, font_px);
        let mut width = 0.0;
        let mut previous = None;
        let count = text.chars().count();
        for (index, ch) in text.chars().enumerate() {
            let id = font.glyph_id(ch);
            if let Some(previous) = previous {
                width += font.kern_unscaled(previous, id) * factor;
            }
            width += font.h_advance_unscaled(id) * factor;
            if index + 1 < count {
                width += tracking;
            }
            if ch.is_whitespace() {
                width += word_gap;
            }
            previous = Some(id);
        }
        width.ceil().max(0.0) as i32
    })
}

pub fn ascent(font_px: f32, face: Typeface<'_>) -> f32 {
    with_face(face, |font| font.ascent_unscaled() * factor(font, font_px))
}

pub fn draw(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    text: &str,
    color: Rgba,
    font_px: f32,
    mut cursor_x: f32,
    baseline: f32,
    clip: &Clip,
    typeface: Typeface<'_>,
    tracking: f32,
    word_gap: f32,
) {
    with_face(typeface, |font| {
        let factor = factor(font, font_px);
        let scale = PxScale::from(font_px.max(1.0));
        let mut previous = None;
        let count = text.chars().count();
        for (index, ch) in text.chars().enumerate() {
            let id = font.glyph_id(ch);
            if let Some(previous) = previous {
                cursor_x += font.kern_unscaled(previous, id) * factor;
            }
            let glyph = id.with_scale_and_position(scale, point(cursor_x, baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    if coverage < 0.04 {
                        return;
                    }
                    let y_px = bounds.min.y + gy as f32;
                    let shear = if typeface.italic {
                        (baseline - y_px) * 0.2
                    } else {
                        0.0
                    };
                    let x = (bounds.min.x + gx as f32 + shear).round() as i32;
                    let y = y_px.round() as i32;
                    if x < clip.x || y < clip.y || x >= clip.x + clip.w || y >= clip.y + clip.h {
                        return;
                    }
                    let alpha = (coverage.clamp(0.0, 1.0) * f32::from(color.a)).round() as u8;
                    paint(
                        pixels,
                        frame_w,
                        frame_h,
                        x,
                        y,
                        Rgba {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                            a: alpha,
                        },
                    );
                });
            }
            cursor_x += font.h_advance_unscaled(id) * factor;
            if index + 1 < count {
                cursor_x += tracking;
            }
            if ch.is_whitespace() {
                cursor_x += word_gap;
            }
            previous = Some(id);
        }
    });
}

fn with_face<T>(typeface: Typeface<'_>, draw: impl FnOnce(&FontVec) -> T) -> T {
    if typeface.family.is_empty() {
        return draw(face());
    }
    let key = format!(
        "{}:{}",
        typeface.family.to_ascii_lowercase(),
        if typeface.bold { "bold" } else { "regular" }
    );
    let cache = named_faces();
    let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
    if !cache.contains_key(&key)
        && let Some(font) = load_family(typeface.family, typeface.bold)
    {
        cache.insert(key.clone(), font);
    }
    if let Some(font) = cache.get(&key) {
        draw(font)
    } else {
        drop(cache);
        draw(face())
    }
}

fn named_faces() -> &'static Mutex<HashMap<String, FontVec>> {
    static FACES: OnceLock<Mutex<HashMap<String, FontVec>>> = OnceLock::new();
    FACES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_family(family: &str, bold: bool) -> Option<FontVec> {
    let path = resolve_family(family, bold)?;
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec(bytes).ok()
}

fn resolve_family(family: &str, bold: bool) -> Option<PathBuf> {
    let key = format!(
        "{}:{}",
        family.to_ascii_lowercase(),
        if bold { "bold" } else { "regular" }
    );
    let cache = resolved_paths();
    let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(path) = cache.get(&key) {
        return path.clone();
    }
    let path = family_file(family, bold)
        .map(PathBuf::from)
        .or_else(|| fontconfig_match(family, bold))
        .or_else(|| scan_system_fonts(family, bold));
    cache.insert(key, path.clone());
    path
}

fn resolved_paths() -> &'static Mutex<HashMap<String, Option<PathBuf>>> {
    static PATHS: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    PATHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn family_file(family: &str, bold: bool) -> Option<&'static str> {
    let files: &[&str] = match (family.to_ascii_lowercase().as_str(), bold) {
        ("arial", false) => &[
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Arial.ttf",
        ],
        ("arial", true) => &[
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/Library/Fonts/Arial Bold.ttf",
            r"C:\Windows\Fonts\arialbd.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold.ttf",
        ],
        _ => &[],
    };
    files.iter().copied().find(|path| Path::new(path).is_file())
}

fn fontconfig_match(family: &str, bold: bool) -> Option<PathBuf> {
    let pattern = if bold {
        format!("{family}:weight=bold")
    } else {
        family.to_owned()
    };
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{family}\n", &pattern])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next()?.trim());
    let matched = lines.next()?.split(',').next()?.trim();
    if !family_matches(matched, family) || !path.is_file() {
        return None;
    }
    Some(path)
}

fn family_matches(matched: &str, requested: &str) -> bool {
    normalize_family(matched) == normalize_family(requested)
}

fn normalize_family(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn scan_system_fonts(family: &str, bold: bool) -> Option<PathBuf> {
    scan_font_dirs(system_font_dirs(), family, bold)
}

fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/System/Library/Fonts/Supplemental"),
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join("Library/Fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    dirs.push(PathBuf::from(windir).join("Fonts"));
    dirs
}

fn scan_font_dirs(
    dirs: impl IntoIterator<Item = PathBuf>,
    family: &str,
    bold: bool,
) -> Option<PathBuf> {
    let needle = normalize_family(family);
    if needle.is_empty() {
        return None;
    }
    let mut best: Option<(i32, PathBuf)> = None;
    for dir in dirs {
        let mut files = Vec::new();
        collect_font_files(&dir, 0, &mut files);
        for path in files {
            let Some(score) = font_file_score(&path, &needle, bold) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best = Some((score, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn collect_font_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() > 8_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else if is_font_file(&path) {
            out.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("ttf" | "otf" | "ttc")
    )
}

fn font_file_score(path: &Path, family: &str, bold: bool) -> Option<i32> {
    let stem = normalize_family(path.file_stem()?.to_str()?);
    let style = stem.strip_prefix(family)?;
    if !is_style_suffix(style) {
        return None;
    }
    let is_bold = style.contains("bold")
        || style == "bd"
        || style.contains("heavy")
        || style.contains("black")
        || style.contains("semibold")
        || style.contains("demibold");
    let is_italic = style.contains("italic") || style.contains("oblique");
    if is_italic || bold != is_bold {
        return None;
    }
    let mut score = 200 - style.len() as i32;
    if style.is_empty() || style == "regular" || style == "roman" || style == "r" {
        score += 20;
    }
    Some(score)
}

fn is_style_suffix(style: &str) -> bool {
    if style.is_empty() {
        return true;
    }
    const STYLES: &[&str] = &[
        "regular", "roman", "book", "medium", "semibold", "demibold", "light", "thin", "bold",
        "black", "heavy", "italic", "oblique", "bd", "r",
    ];
    let mut rest = style;
    let mut progressed = true;
    while !rest.is_empty() && progressed {
        progressed = false;
        for token in STYLES {
            if let Some(stripped) = rest.strip_prefix(token) {
                rest = stripped;
                progressed = true;
                break;
            }
        }
    }
    rest.is_empty()
}

fn factor(font: &FontVec, font_px: f32) -> f32 {
    let units = font.height_unscaled();
    if units <= 0.0 {
        return 1.0;
    }
    font_px.max(1.0) / units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fontconfig_or_scan_finds_installed_families() {
        let path = resolve_family("Liberation Sans", false)
            .or_else(|| resolve_family("DejaVu Sans", false))
            .expect("a common system sans should be installed");
        assert!(path.is_file());
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        assert!(
            name.contains("liberation") || name.contains("dejavu"),
            "{path:?}"
        );
        if let Some(bold) = resolve_family("Liberation Sans", true) {
            let name = bold
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase();
            assert!(name.contains("bold"), "{bold:?}");
        }
        assert!(resolve_family("NotARealFontFamilyXYZ_123", false).is_none());
    }

    #[test]
    fn named_family_measures_with_the_system_face() {
        if resolve_family("Liberation Sans", false).is_none() {
            return;
        }
        let liberation = Typeface {
            family: "Liberation Sans",
            bold: false,
            italic: false,
        };
        let bundled = Typeface {
            family: "",
            bold: false,
            italic: false,
        };
        let named = measure("Hamburgefonstiv", 48.0, liberation, 0.0, 0.0);
        let fallback = measure("Hamburgefonstiv", 48.0, bundled, 0.0, 0.0);
        assert!(named > 0 && fallback > 0);
    }

    #[test]
    fn missing_family_falls_back_to_dejavu() {
        let missing = Typeface {
            family: "NotARealFontFamilyXYZ_123",
            bold: false,
            italic: false,
        };
        let bundled = Typeface {
            family: "",
            bold: false,
            italic: false,
        };
        assert_eq!(
            measure("Hello", 32.0, missing, 0.0, 0.0),
            measure("Hello", 32.0, bundled, 0.0, 0.0)
        );
    }

    #[test]
    fn font_scan_picks_bold_and_regular_from_a_folder() {
        let dir = std::env::temp_dir().join(format!(
            "freeshow-fonts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let regular = dir.join("Cool Family-Regular.ttf");
        let bold = dir.join("CoolFamily-Bold.ttf");
        let other = dir.join("CoolerFamily-Regular.ttf");
        std::fs::write(&regular, b"x").unwrap();
        std::fs::write(&bold, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(
            scan_font_dirs([dir.clone()], "Cool Family", false).as_deref(),
            Some(regular.as_path())
        );
        assert_eq!(
            scan_font_dirs([dir.clone()], "Cool Family", true).as_deref(),
            Some(bold.as_path())
        );
        assert!(scan_font_dirs([dir.clone()], "Cool", false).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn family_name_comparison_ignores_spaces_and_case() {
        assert!(family_matches("DejaVu Sans", "dejavu sans"));
        assert!(family_matches("Liberation Sans", "LiberationSans"));
        assert!(!family_matches("Noto Sans", "Noto"));
    }
}
