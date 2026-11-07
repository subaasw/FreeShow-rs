// SPDX-License-Identifier: GPL-3.0-only

//! Proportional text for slide items. The face is DejaVu Sans, bundled under
//! its license in `assets/`. Glyph coverage is blended, so edges are not the
//! hard 8×8 blocks of the debug font.

use std::collections::HashMap;
use std::path::Path;
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
    let path = family_file(family, bold)?;
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec(bytes).ok()
}

fn family_file(family: &str, bold: bool) -> Option<&'static str> {
    let files: &[&str] = match (family.to_ascii_lowercase().as_str(), bold) {
        ("arial", false) => &[
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Arial.ttf",
        ],
        ("arial", true) => &[
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/Library/Fonts/Arial Bold.ttf",
            "C:\\Windows\\Fonts\\arialbd.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Arial_Bold.ttf",
        ],
        _ => &[],
    };
    files.iter().copied().find(|path| Path::new(path).is_file())
}

fn factor(font: &FontVec, font_px: f32) -> f32 {
    let units = font.height_unscaled();
    if units <= 0.0 {
        return 1.0;
    }
    font_px.max(1.0) / units
}
