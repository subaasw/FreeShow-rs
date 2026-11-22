// SPDX-License-Identifier: GPL-3.0-only

#![allow(clippy::too_many_arguments)]

use crate::{Rgba, paint, text};

/// FreeShow lays items out in a 1920×1080 slide. The compositor scales that
/// space onto the output frame.
pub const SOURCE_WIDTH: f32 = 1920.0;
pub const SOURCE_HEIGHT: f32 = 1080.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxStyle {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub color: Rgba,
    pub background: Option<Rgba>,
    pub elliptical_background: bool,
    pub font_px: f32,
    pub align: Align,
    pub font_family: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_case: TextCase,
    pub stroke_px: f32,
    pub stroke_color: Option<Rgba>,
    pub shadow: Option<TextShadow>,
    /// CSS `border-radius` in slide pixels. `50%` still uses `elliptical_background`.
    pub radius: f32,
    pub box_shadow: Option<BoxShadow>,
    /// CSS `backdrop-filter` from the item style, or empty.
    pub backdrop: String,
    /// CSS gradient from `background`.
    pub gradient: Option<Gradient>,
    pptx_radial_focus: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextCase {
    #[default]
    AsWritten,
    Upper,
    Lower,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub color: Rgba,
}

/// CSS `box-shadow` on an item box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// CSS linear or centered circular radial gradient on an item box.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    linear_angle_deg: Option<f32>,
    pub stops: Vec<(f32, Rgba)>,
}

impl Default for BoxStyle {
    fn default() -> Self {
        Self {
            left: 50.0,
            top: 88.0,
            width: 1820.0,
            height: 904.0,
            color: Rgba::WHITE,
            background: None,
            elliptical_background: false,
            font_px: 100.0,
            align: Align::Center,
            font_family: String::new(),
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_case: TextCase::AsWritten,
            stroke_px: 0.0,
            stroke_color: None,
            shadow: None,
            radius: 0.0,
            box_shadow: None,
            backdrop: String::new(),
            gradient: None,
            pptx_radial_focus: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun {
    pub text: String,
    pub style: String,
    /// Measure this lyric prefix at the line's main font and draw the run there.
    pub place_on: Option<String>,
    pub paint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledLine {
    pub align: String,
    pub runs: Vec<TextRun>,
    /// Lyric lines take the item line background. Chord lines do not.
    pub line_bar: bool,
    /// A later visual row of a lyric that wrapped. It keeps the line background
    /// and does not open the gap that sits between lyrics.
    pub wrapped: bool,
    /// Source pixels to lift the painted glyphs. The line box stays put.
    pub shift_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    #[default]
    Fill,
    Contain,
    Cover,
    /// The picture is repeated at its source size to fill the box.
    Tile,
    /// The picture stays contained, with a blurred cover copy behind it.
    Blur,
}

impl Fit {
    /// FreeShow media fit. A missing value uses contain. `blur` keeps the picture contained and adds the backdrop.
    pub fn named(name: &str) -> Self {
        match name {
            "cover" => Self::Cover,
            "fill" => Self::Fill,
            "tile" => Self::Tile,
            "blur" => Self::Blur,
            _ => Self::Contain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledItem {
    pub style: String,
    pub align: String,
    pub lines: Vec<StyledLine>,
    pub image: Option<Raster>,
    pub fit: Fit,
    pub line_bg: Option<Rgba>,
    pub line_gap: f32,
    pub line_radius: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// CSS `filter: opacity()`, from 0 to 1. `1` leaves the item unchanged.
    pub opacity: f32,
    /// CSS `filter: grayscale()`, from 0 to 1.
    pub grayscale: f32,
    /// CSS `filter: invert()`, from 0 to 1.
    pub invert: f32,
    /// CSS `filter: sepia()`, from 0 to 1.
    pub sepia: f32,
    /// CSS `filter: brightness()`. `1` leaves the color unchanged. `0` is black.
    pub brightness: f32,
    /// CSS `filter: contrast()`. `1` leaves the color unchanged. `0` is flat gray.
    pub contrast: f32,
    /// CSS `filter: saturate()`. `1` leaves the color unchanged. `0` is gray.
    pub saturate: f32,
    /// CSS `filter: hue-rotate()`, in degrees. `0` leaves the color unchanged.
    pub hue_rotate: f32,
    /// CSS `filter: blur()`, in slide pixels. `0` leaves the item sharp.
    pub blur: f32,
    /// CSS `mix-blend-mode`. `Normal` paints the item over the slide.
    pub blend: Blend,
    /// Percentage edges hidden from an item picture. `clip` is false for a PowerPoint crop.
    pub crop: Crop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub clip: bool,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
            clip: true,
        }
    }
}

impl Crop {
    fn clamped(self) -> Self {
        if !self.clip {
            return Self::default();
        }
        let mut top = self.top.clamp(0.0, 99.0);
        let mut right = self.right.clamp(0.0, 99.0);
        let mut bottom = self.bottom.clamp(0.0, 99.0);
        let mut left = self.left.clamp(0.0, 99.0);
        let horizontal = left + right;
        if horizontal > 99.0 {
            let factor = 99.0 / horizontal;
            left *= factor;
            right *= factor;
        }
        let vertical = top + bottom;
        if vertical > 99.0 {
            let factor = 99.0 / vertical;
            top *= factor;
            bottom *= factor;
        }
        Self {
            top,
            right,
            bottom,
            left,
            clip: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    ColorDodge,
    ColorBurn,
    Lighten,
    Darken,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledSlide {
    pub output_width: u32,
    pub output_height: u32,
    pub source_width: f32,
    pub source_height: f32,
    pub background: Rgba,
    pub items: Vec<StyledItem>,
}

pub fn parse_style(css: &str) -> BoxStyle {
    let mut style = BoxStyle::default();
    let mut spacing = None;
    let mut words = None;
    for part in css.split(';') {
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "left" => style.left = px(value).unwrap_or(style.left),
            "top" => style.top = px(value).unwrap_or(style.top),
            "width" => style.width = px(value).unwrap_or(style.width),
            "height" => style.height = px(value).unwrap_or(style.height),
            "font-size" => style.font_px = px(value).unwrap_or(style.font_px),
            "color" => {
                if let Some(color) = parse_color(value) {
                    style.color = color;
                }
            }
            "background-color" => {
                if let Some(color) = parse_color(value) {
                    style.background = Some(color);
                }
            }
            "background" => {
                if let Some(gradient) = parse_gradient(value) {
                    style.background = Some(gradient.stops[0].1);
                    style.gradient = Some(gradient);
                } else if let Some(color) = parse_color(value) {
                    style.background = Some(color);
                }
            }
            "--pptx-radial-focus" => style.pptx_radial_focus = parse_radial_focus(value),
            "border-radius" => {
                if value == "50%" {
                    style.elliptical_background = true;
                } else if let Some(radius) = px(value.split_whitespace().next().unwrap_or(value)) {
                    style.radius = radius;
                }
            }
            "box-shadow" => style.box_shadow = parse_box_shadow(value),
            "backdrop-filter" => style.backdrop = value.to_string(),
            "text-align" => style.align = parse_align(value),
            "font-family" => style.font_family = font_family(value),
            "font-weight" => style.bold = value.trim() == "bold" || value.trim() == "700",
            "font-style" => {
                style.italic = value.trim().eq_ignore_ascii_case("italic")
                    || value.trim().eq_ignore_ascii_case("oblique")
            }
            "text-decoration" | "text-decoration-line" => {
                style.underline = value.split_whitespace().any(|part| part == "underline");
                style.strike = value.split_whitespace().any(|part| part == "line-through");
            }
            "text-shadow" => style.shadow = parse_shadow(value),
            "letter-spacing" => spacing = Some(value.to_string()),
            "word-spacing" => words = Some(value.to_string()),
            "text-transform" => style.text_case = text_case(value),
            "-webkit-text-stroke-width" | "text-stroke-width" => {
                style.stroke_px = px(value).unwrap_or(0.0)
            }
            "-webkit-text-stroke-color" | "text-stroke-color" => {
                style.stroke_color = parse_color(value)
            }
            "-webkit-text-stroke" | "text-stroke" => {
                if let Some((width, color)) = stroke_shorthand(value) {
                    style.stroke_px = width;
                    style.stroke_color = Some(color);
                }
            }
            _ => {}
        }
    }
    if let Some(value) = spacing {
        style.letter_spacing = spacing_px(&value, style.font_px);
    }
    if let Some(value) = words {
        style.word_spacing = spacing_px(&value, style.font_px);
    }
    style
}

fn output_spacing(spacing: f32, slide: &StyledSlide) -> f32 {
    if slide.source_height <= 0.0 {
        spacing
    } else {
        spacing * slide.output_height as f32 / slide.source_height
    }
}

fn stroke_shorthand(value: &str) -> Option<(f32, Rgba)> {
    let mut rest = value.trim();
    let token = rest.split_whitespace().next()?;
    let width = px(token)?;
    rest = rest[token.len()..].trim();
    Some((width, parse_color(rest).unwrap_or(Rgba::BLACK)))
}

fn stroke_offsets(radius: i32) -> Vec<(f32, f32)> {
    let radius = radius.clamp(1, 16);
    let mut offsets = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy <= radius * radius {
                offsets.push((dx as f32, dy as f32));
            }
        }
    }
    offsets
}

fn text_case(value: &str) -> TextCase {
    match value.trim() {
        "uppercase" => TextCase::Upper,
        "lowercase" => TextCase::Lower,
        "capitalize" => TextCase::Capitalize,
        _ => TextCase::AsWritten,
    }
}

fn cased(text: &str, case: TextCase) -> std::borrow::Cow<'_, str> {
    match case {
        TextCase::AsWritten => std::borrow::Cow::Borrowed(text),
        TextCase::Upper => std::borrow::Cow::Owned(text.to_uppercase()),
        TextCase::Lower => std::borrow::Cow::Owned(text.to_lowercase()),
        TextCase::Capitalize => {
            let mut out = String::new();
            let mut word = true;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    word = true;
                    out.push(ch);
                } else if word {
                    out.extend(ch.to_uppercase());
                    word = false;
                } else {
                    out.push(ch);
                }
            }
            std::borrow::Cow::Owned(out)
        }
    }
}

fn spacing_px(value: &str, font_px: f32) -> f32 {
    let value = value.trim();
    if let Some(em) = value.strip_suffix("em") {
        return em.trim().parse::<f32>().unwrap_or(0.0) * font_px;
    }
    px(value).unwrap_or(0.0)
}

pub fn parse_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let (channels, alpha) = inner
        .split_once('/')
        .map(|(body, alpha)| (body, Some(alpha.trim())))
        .unwrap_or((inner, None));
    let nums: Vec<f32> = channels
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    if nums.len() < 3 {
        return None;
    }
    let alpha = if let Some(alpha) = alpha {
        parse_alpha(alpha)?
    } else if nums.len() >= 4 {
        alpha_byte(nums[3])
    } else {
        255
    };
    Some(Rgba {
        r: nums[0].clamp(0.0, 255.0) as u8,
        g: nums[1].clamp(0.0, 255.0) as u8,
        b: nums[2].clamp(0.0, 255.0) as u8,
        a: alpha,
    })
}

pub fn render_styled(slide: &StyledSlide) -> crate::Frame {
    let width = slide.output_width;
    let height = slide.output_height;
    let mut frame = crate::Frame {
        width,
        height,
        pixels: vec![0u8; (width * height * 4) as usize],
    };
    paint_styled(&mut frame.pixels, slide);
    frame
}

/// Draws `slide` onto an existing framebuffer. Items are painted directly when
/// the sizes match, so a lyric slide does not need a second full-frame `over`.
pub fn composite_styled(base: &mut crate::Frame, slide: &StyledSlide) {
    if base.width != slide.output_width || base.height != slide.output_height {
        *base = over(base, &render_styled(slide));
        return;
    }
    paint_styled(&mut base.pixels, slide);
}

fn paint_styled(pixels: &mut [u8], slide: &StyledSlide) {
    if slide.background.a > 0 {
        fill(
            pixels,
            slide.output_width,
            slide.output_height,
            slide.background,
        );
    }
    for item in &slide.items {
        draw_item(pixels, slide, item);
    }
}

pub fn clear_frame(width: u32, height: u32) -> crate::Frame {
    render_styled(&StyledSlide {
        output_width: width,
        output_height: height,
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        background: Rgba::BLACK,
        items: Vec::new(),
    })
}

/// FreeShow slide directions. `LeftRight` moves both layers toward the left,
/// which is `translate(-pos%)` in the output transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideWay {
    LeftRight,
    RightLeft,
    TopBottom,
    BottomTop,
}

pub fn slide_way(direction: &str) -> SlideWay {
    match direction {
        "right_left" => SlideWay::RightLeft,
        "top_bottom" => SlideWay::TopBottom,
        "bottom_top" => SlideWay::BottomTop,
        _ => SlideWay::LeftRight,
    }
}

pub fn slide_frames(from: &crate::Frame, to: &crate::Frame, t: f32, way: SlideWay) -> crate::Frame {
    let t = t.clamp(0.0, 1.0);
    let width = from.width as i32;
    let height = from.height as i32;
    let shift_x = (width as f32 * t).round() as i32;
    let shift_y = (height as f32 * t).round() as i32;
    let remain_x = (width as f32 * (1.0 - t)).round() as i32;
    let remain_y = (height as f32 * (1.0 - t)).round() as i32;
    let (out_x, out_y, in_x, in_y) = match way {
        SlideWay::LeftRight => (-shift_x, 0, -remain_x, 0),
        SlideWay::RightLeft => (shift_x, 0, remain_x, 0),
        SlideWay::TopBottom => (0, -shift_y, 0, -remain_y),
        SlideWay::BottomTop => (0, shift_y, 0, remain_y),
    };
    let mut pixels = vec![0u8; from.pixels.len()];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    blit_frame(&mut pixels, width, height, from, out_x, out_y);
    blit_frame(&mut pixels, width, height, to, in_x, in_y);
    crate::Frame {
        width: from.width,
        height: from.height,
        pixels,
    }
}

/// FreeShow's scale transition. The current slide shrinks and fades, and the
/// next slide grows from the center.
pub fn scale_frames(from: &crate::Frame, to: &crate::Frame, t: f32) -> crate::Frame {
    let t = t.clamp(0.0, 1.0);
    let mut pixels = vec![0u8; from.pixels.len()];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    paint_scaled(&mut pixels, from, 1.0 - t);
    paint_scaled(&mut pixels, to, t);
    crate::Frame {
        width: from.width,
        height: from.height,
        pixels,
    }
}

fn paint_scaled(dest: &mut [u8], source: &crate::Frame, amount: f32) {
    if amount < 0.001 {
        return;
    }
    let width = source.width as f32;
    let height = source.height as f32;
    let source_w = source.width as i32;
    let source_h = source.height as i32;
    for dy in 0..source_h {
        for dx in 0..source_w {
            let sx = ((dx as f32 + 0.5 - width / 2.0) / amount + width / 2.0 - 0.5).round();
            let sy = ((dy as f32 + 0.5 - height / 2.0) / amount + height / 2.0 - 0.5).round();
            if sx < 0.0 || sy < 0.0 || sx >= width || sy >= height {
                continue;
            }
            let from = ((sy as i32 * source_w + sx as i32) * 4) as usize;
            let into = ((dy * source_w + dx) * 4) as usize;
            for channel in 0..4 {
                let under = f32::from(dest[into + channel]);
                let over = f32::from(source.pixels[from + channel]);
                dest[into + channel] = (under + (over - under) * amount).round() as u8;
            }
        }
    }
}

/// Svelte's in-out easings that FreeShow offers. An unknown name stays linear,
/// which is what FreeShow does when the name is not in its list. Sine is the
/// stored default.
pub fn ease_progress(name: &str, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match name {
        "sine" => sine_in_out(t),
        "back" => back_in_out(t),
        "circ" => circ_in_out(t),
        "cubic" => cubic_in_out(t),
        "elastic" => elastic_in_out(t),
        "bounce" => bounce_in_out(t),
        _ => t,
    }
}

fn sine_in_out(t: f32) -> f32 {
    (1.0 - (std::f32::consts::PI * t).cos()) / 2.0
}

fn back_in_out(mut t: f32) -> f32 {
    let s = 1.70158 * 1.525;
    t *= 2.0;
    if t < 1.0 {
        0.5 * (t * t * ((s + 1.0) * t - s))
    } else {
        t -= 2.0;
        0.5 * (t * t * ((s + 1.0) * t + s) + 2.0)
    }
}

fn circ_in_out(mut t: f32) -> f32 {
    t *= 2.0;
    if t < 1.0 {
        -0.5 * ((1.0 - t * t).sqrt() - 1.0)
    } else {
        t -= 2.0;
        0.5 * ((1.0 - t * t).sqrt() + 1.0)
    }
}

fn cubic_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        0.5 * (2.0 * t - 2.0).powi(3) + 1.0
    }
}

fn elastic_in_out(t: f32) -> f32 {
    let pi = std::f32::consts::PI;
    if t < 0.5 {
        0.5 * ((13.0 * pi / 2.0) * 2.0 * t).sin() * 2.0_f32.powf(10.0 * (2.0 * t - 1.0))
    } else {
        0.5 * ((-13.0 * pi / 2.0) * (2.0 * t)).sin() * 2.0_f32.powf(-10.0 * (2.0 * t - 1.0)) + 1.0
    }
}

fn bounce_out(t: f32) -> f32 {
    let a = 4.0 / 11.0;
    let b = 8.0 / 11.0;
    let c = 9.0 / 10.0;
    let ca = 4356.0 / 361.0;
    let cb = 35442.0 / 1805.0;
    let cc = 16061.0 / 1805.0;
    let t2 = t * t;
    if t < a {
        7.5625 * t2
    } else if t < b {
        9.075 * t2 - 9.9 * t + 3.4
    } else if t < c {
        ca * t2 - cb * t + cc
    } else {
        10.8 * t * t - 20.52 * t + 10.72
    }
}

fn bounce_in_out(t: f32) -> f32 {
    if t < 0.5 {
        0.5 * (1.0 - bounce_out(1.0 - t * 2.0))
    } else {
        0.5 * bounce_out(t * 2.0 - 1.0) + 0.5
    }
}

/// FreeShow's blur transition. The current slide fades while a 5px blur grows,
/// and the next slide fades in as its blur clears.
pub fn blur_frames(from: &crate::Frame, to: &crate::Frame, t: f32) -> crate::Frame {
    let t = t.clamp(0.0, 1.0);
    let mut pixels = vec![0u8; from.pixels.len()];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    paint_blurred(&mut pixels, from, t, 1.0 - t);
    paint_blurred(&mut pixels, to, 1.0 - t, t);
    crate::Frame {
        width: from.width,
        height: from.height,
        pixels,
    }
}

/// FreeShow's spin transition. Each layer fades while it turns one full circle.
pub fn spin_frames(from: &crate::Frame, to: &crate::Frame, t: f32) -> crate::Frame {
    let t = t.clamp(0.0, 1.0);
    let mut pixels = vec![0u8; from.pixels.len()];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    paint_spin(&mut pixels, from, 1.0 - t);
    paint_spin(&mut pixels, to, t);
    crate::Frame {
        width: from.width,
        height: from.height,
        pixels,
    }
}

fn paint_spin(dest: &mut [u8], source: &crate::Frame, amount: f32) {
    if amount < 0.001 {
        return;
    }
    let (sin, cos) = (amount * std::f32::consts::TAU).sin_cos();
    let width = source.width as i32;
    let height = source.height as i32;
    let cx = source.width as f32 / 2.0;
    let cy = source.height as f32 / 2.0;
    for dy in 0..height {
        for dx in 0..width {
            let ox = dx as f32 + 0.5 - cx;
            let oy = dy as f32 + 0.5 - cy;
            let sx = cx + ox * cos + oy * sin - 0.5;
            let sy = cy - ox * sin + oy * cos - 0.5;
            let sx = sx.round();
            let sy = sy.round();
            if sx < 0.0 || sy < 0.0 || sx >= source.width as f32 || sy >= source.height as f32 {
                continue;
            }
            let from = ((sy as i32 * width + sx as i32) * 4) as usize;
            let into = ((dy * width + dx) * 4) as usize;
            for channel in 0..4 {
                let under = f32::from(dest[into + channel]);
                let over = f32::from(source.pixels[from + channel]);
                dest[into + channel] = (under + (over - under) * amount).round() as u8;
            }
        }
    }
}

fn paint_blurred(dest: &mut [u8], source: &crate::Frame, blur_amount: f32, opacity: f32) {
    if opacity < 0.001 {
        return;
    }
    let radius = (blur_amount * 5.0).round() as i32;
    let blurred = if radius >= 1 {
        let width = source.width as i32;
        let height = source.height as i32;
        let clamped = radius.clamp(1, 16);
        let mut horizontal = vec![0u8; source.pixels.len()];
        box_blur_axis(
            &source.pixels,
            &mut horizontal,
            width,
            height,
            clamped,
            true,
        );
        let mut vertical = vec![0u8; source.pixels.len()];
        box_blur_axis(&horizontal, &mut vertical, width, height, clamped, false);
        vertical
    } else {
        source.pixels.clone()
    };
    for index in (0..dest.len()).step_by(4) {
        for channel in 0..4 {
            let under = f32::from(dest[index + channel]);
            let over = f32::from(blurred.get(index + channel).copied().unwrap_or(0));
            dest[index + channel] = (under + (over - under) * opacity).round() as u8;
        }
    }
}

fn blit_frame(dest: &mut [u8], width: i32, height: i32, source: &crate::Frame, ox: i32, oy: i32) {
    let source_w = source.width as i32;
    let source_h = source.height as i32;
    for sy in 0..source_h {
        let dy = sy + oy;
        if dy < 0 || dy >= height {
            continue;
        }
        for sx in 0..source_w {
            let dx = sx + ox;
            if dx < 0 || dx >= width {
                continue;
            }
            let from = ((sy * source_w + sx) * 4) as usize;
            let into = ((dy * width + dx) * 4) as usize;
            dest[into..into + 4].copy_from_slice(&source.pixels[from..from + 4]);
        }
    }
}

pub fn crossfade(from: &crate::Frame, to: &crate::Frame, t: f32) -> crate::Frame {
    let t = t.clamp(0.0, 1.0);
    let width = from.width;
    let height = from.height;
    let mut pixels = vec![0u8; from.pixels.len()];
    for index in (0..pixels.len()).step_by(4) {
        for channel in 0..4 {
            let a = f32::from(from.pixels[index + channel]);
            let b = f32::from(to.pixels.get(index + channel).copied().unwrap_or(0));
            pixels[index + channel] = (a + (b - a) * t).round() as u8;
        }
    }
    crate::Frame {
        width,
        height,
        pixels,
    }
}

pub fn image_frame(image: &Raster, width: u32, height: u32) -> crate::Frame {
    placed_image(
        image,
        width,
        height,
        Fit::Contain,
        false,
        false,
        "",
        "",
        6.0,
        0.3,
    )
}

pub fn placed_image(
    image: &Raster,
    width: u32,
    height: u32,
    fit: Fit,
    flip_x: bool,
    flip_y: bool,
    filter: &str,
    blend: &str,
    blur_px: f32,
    blur_opacity: f32,
) -> crate::Frame {
    let foreground = if fit == Fit::Blur { Fit::Contain } else { fit };
    let mut items = Vec::new();
    if fit == Fit::Blur {
        let backdrop = format!("{filter} blur({blur_px}px) opacity({blur_opacity})");
        items.push(still_item(image, Fit::Cover, flip_x, flip_y, &backdrop, ""));
    }
    items.push(still_item(image, foreground, flip_x, flip_y, filter, blend));
    render_styled(&StyledSlide {
        output_width: width,
        output_height: height,
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        background: crate::Rgba::BLACK,
        items,
    })
}

fn still_item(
    image: &Raster,
    fit: Fit,
    flip_x: bool,
    flip_y: bool,
    filter: &str,
    blend: &str,
) -> StyledItem {
    StyledItem {
        style: format!("left:0px;top:0px;width:{SOURCE_WIDTH}px;height:{SOURCE_HEIGHT}px;"),
        align: String::new(),
        lines: Vec::new(),
        image: Some(image.clone()),
        fit,
        line_bg: None,
        line_gap: 0.0,
        line_radius: 0.0,
        flip_x,
        flip_y,
        opacity: filter_opacity(filter),
        grayscale: filter_grayscale(filter),
        invert: filter_invert(filter),
        sepia: filter_sepia(filter),
        brightness: filter_brightness(filter),
        contrast: filter_contrast(filter),
        saturate: filter_saturate(filter),
        hue_rotate: filter_hue_rotate(filter),
        blur: filter_blur(filter),
        blend: blend_mode(blend),
        crop: Crop::default(),
    }
}

/// Cuts pixels off each side of a still, matching FreeShow's media crop before the picture is fitted.
pub fn crop_raster(image: &Raster, left: u32, right: u32, top: u32, bottom: u32) -> Raster {
    if left == 0 && right == 0 && top == 0 && bottom == 0 {
        return image.clone();
    }
    let width = image.width as i64;
    let height = image.height as i64;
    let left = left as i64;
    let right = right as i64;
    let top = top as i64;
    let bottom = bottom as i64;
    let cropped_w = width - left - right;
    let cropped_h = height - top - bottom;
    if cropped_w <= 0 || cropped_h <= 0 || image.rgba.len() < (width * height * 4) as usize {
        return Raster {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
    }
    let mut rgba = Vec::with_capacity((cropped_w * cropped_h * 4) as usize);
    for row in top..top + cropped_h {
        let start = ((row * width + left) * 4) as usize;
        let end = start + (cropped_w * 4) as usize;
        rgba.extend_from_slice(&image.rgba[start..end]);
    }
    Raster {
        width: cropped_w as u32,
        height: cropped_h as u32,
        rgba,
    }
}

/// Paints `top` over `base`. Pixels in `top` with alpha 0 leave `base` visible.
pub fn over(base: &crate::Frame, top: &crate::Frame) -> crate::Frame {
    let mut pixels = base.pixels.clone();
    let len = pixels.len().min(top.pixels.len());
    let mut index = 0;
    while index + 3 < len {
        let alpha = top.pixels[index + 3];
        if alpha == 0 {
            index += 4;
            while index + 3 < len && top.pixels[index + 3] == 0 {
                index += 4;
            }
            continue;
        }
        if alpha == 255 {
            pixels[index..index + 4].copy_from_slice(&top.pixels[index..index + 4]);
        } else {
            let cover = u16::from(alpha);
            let remain = 255 - cover;
            for channel in 0..3 {
                let below = u16::from(pixels[index + channel]);
                let above = u16::from(top.pixels[index + channel]);
                pixels[index + channel] = ((above * cover + below * remain) / 255) as u8;
            }
            pixels[index + 3] = 255;
        }
        index += 4;
    }
    crate::Frame {
        width: base.width,
        height: base.height,
        pixels,
    }
}

pub fn with_underlay(
    slide: crate::Frame,
    underlay: Option<&Raster>,
    width: u32,
    height: u32,
) -> crate::Frame {
    let Some(image) = underlay else {
        return slide;
    };
    over(&image_frame(image, width, height), &slide)
}

/// Paint one pixel per QR module. The compositor scales the raster with
/// nearest-neighbor sampling and keeps the four-module quiet zone.
pub fn qr_image(text: &str, foreground: &str, background: &str) -> Option<Raster> {
    use qrcode::{Color, EcLevel, QrCode};
    if text.is_empty() {
        return None;
    }
    let code = QrCode::new(text)
        .or_else(|_| QrCode::with_error_correction_level(text, EcLevel::L))
        .ok()?;
    let dark = parse_color(foreground).unwrap_or(Rgba::BLACK);
    let light = if background.is_empty() {
        Rgba::TRANSPARENT
    } else {
        parse_color(background).unwrap_or(Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        })
    };
    let width = code.width();
    let side = width + 8;
    let modules = code.into_colors();
    let mut rgba = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let color = if (4..width + 4).contains(&x)
                && (4..width + 4).contains(&y)
                && modules[(y - 4) * width + x - 4] == Color::Dark
            {
                dark
            } else {
                light
            };
            rgba.extend_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }
    Some(Raster {
        width: side as u32,
        height: side as u32,
        rgba,
    })
}

pub fn fit_raster(image: &Raster, max_width: u32, max_height: u32) -> Raster {
    if image.width == 0 || image.height == 0 || max_width == 0 || max_height == 0 {
        return Raster {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
    }
    if image.width <= max_width && image.height <= max_height {
        return image.clone();
    }
    let scale =
        (max_width as f32 / image.width as f32).min(max_height as f32 / image.height as f32);
    let width = (image.width as f32 * scale).round().max(1.0) as u32;
    let height = (image.height as f32 * scale).round().max(1.0) as u32;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        let src_y = (y as u64 * image.height as u64 / height as u64) as u32;
        for x in 0..width {
            let src_x = (x as u64 * image.width as u64 / width as u64) as u32;
            let src = ((src_y * image.width + src_x) * 4) as usize;
            let dest = ((y * width + x) * 4) as usize;
            rgba[dest..dest + 4].copy_from_slice(&image.rgba[src..src + 4]);
        }
    }
    Raster {
        width,
        height,
        rgba,
    }
}

pub fn decode_image(bytes: &[u8]) -> Option<Raster> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some(Raster {
        width,
        height,
        rgba: image.into_raw(),
    })
}

fn draw_item(pixels: &mut [u8], slide: &StyledSlide, item: &StyledItem) {
    let mut style = parse_style(&item.style);
    if let Some(align) = align_in(&item.align) {
        style.align = align;
    }
    let x = scale(style.left, slide.output_width, slide.source_width);
    let y = scale(style.top, slide.output_height, slide.source_height);
    let w = scale(style.width, slide.output_width, slide.source_width).max(1);
    let h = scale(style.height, slide.output_height, slide.source_height).max(1);
    let radius = if style.elliptical_background {
        0
    } else {
        scale(style.radius, slide.output_height, slide.source_height).max(0)
    };
    if !style.backdrop.trim().is_empty() {
        apply_backdrop(
            pixels,
            slide,
            x,
            y,
            w,
            h,
            radius,
            style.elliptical_background,
            &style.backdrop,
        );
    }
    if let Some(shadow) = style.box_shadow.filter(|shadow| !shadow.inset) {
        paint_box_shadow(
            pixels,
            slide,
            x,
            y,
            w,
            h,
            radius,
            style.elliptical_background,
            shadow,
        );
    }
    let opacity = item.opacity.clamp(0.0, 1.0);
    let grayscale = item.grayscale.clamp(0.0, 1.0);
    let invert = item.invert.clamp(0.0, 1.0);
    let sepia = item.sepia.clamp(0.0, 1.0);
    let brightness = item.brightness.clamp(0.0, 10.0);
    let contrast = item.contrast.clamp(0.0, 10.0);
    let saturate = item.saturate.clamp(0.0, 10.0);
    let hue = item.hue_rotate;
    let blur = output_spacing(item.blur, slide).clamp(0.0, 24.0);
    let blend = item.blend;
    let color_filter = opacity < 0.999
        || grayscale > 0.001
        || invert > 0.001
        || sepia > 0.001
        || (brightness - 1.0).abs() > 0.001
        || (contrast - 1.0).abs() > 0.001
        || (saturate - 1.0).abs() > 0.001
        || hue_changes(hue);
    let before = (color_filter || blur >= 0.5 || blend != Blend::Normal)
        .then(|| copy_rect(pixels, slide.output_width, slide.output_height, x, y, w, h));
    fill_item_background(pixels, slide, &style, x, y, w, h, radius);
    if let Some(image) = &item.image {
        blit(
            pixels,
            slide,
            x,
            y,
            w,
            h,
            image,
            item.fit,
            item.flip_x,
            item.flip_y,
            item.crop,
        );
    }
    let clip = text::Clip { x, y, w, h };
    let gap = scale(item.line_gap, slide.output_height, slide.source_height).max(0);
    let visual = wrap_lines(&item.lines, &item.style, slide, w);
    let mut cursor_y = y + vertical_offset(item, &visual, &style, slide, h, gap);
    for (index, line) in visual.iter().enumerate() {
        let mut line_style = style.clone();
        if let Some(align) = align_in(&line.align) {
            line_style.align = align;
        }
        let runs: Vec<(TextRun, BoxStyle)> = line
            .runs
            .iter()
            .map(|run| {
                let mut run_style = line_style.clone();
                apply_run_style(&mut run_style, &run.style);
                (run.clone(), run_style)
            })
            .collect();
        if runs.is_empty() {
            continue;
        }
        let (height_family, height_bold, height_px, height_css) = {
            let (run, height_style) = runs
                .iter()
                .find(|(run, _)| run.paint && !run.text.is_empty())
                .unwrap_or(&runs[0]);
            (
                height_style.font_family.clone(),
                height_style.bold,
                height_style.font_px,
                run.style.clone(),
            )
        };
        let font_px = scale(height_px, slide.output_height, slide.source_height).max(1) as f32;
        let line_face = text::Typeface {
            family: height_family.as_str(),
            bold: height_bold,
            italic: false,
        };
        let bar_h = line_box(&height_css, &item.style, font_px, line_face, slide);
        if (line.line_bar || line.wrapped)
            && let Some(color) = item.line_bg
        {
            let radius = scale(item.line_radius, slide.output_height, slide.source_height).max(0);
            fill_round_rect(
                pixels,
                slide.output_width,
                slide.output_height,
                x,
                cursor_y,
                w,
                bar_h,
                radius,
                color,
            );
        }
        let text_width: i32 = runs
            .iter()
            .filter(|(run, _)| run.place_on.is_none())
            .map(|(run, run_style)| {
                let size = scale(run_style.font_px, slide.output_height, slide.source_height).max(1)
                    as f32;
                let shown = cased(&run.text, run_style.text_case);
                text::measure(
                    &shown,
                    size,
                    text::Typeface {
                        family: run_style.font_family.as_str(),
                        bold: run_style.bold,
                        italic: run_style.italic,
                    },
                    output_spacing(run_style.letter_spacing, slide),
                    output_spacing(run_style.word_spacing, slide),
                )
            })
            .sum();
        let spaces: usize = runs
            .iter()
            .filter(|(run, _)| run.place_on.is_none())
            .map(|(run, run_style)| {
                cased(&run.text, run_style.text_case)
                    .chars()
                    .filter(|ch| ch.is_whitespace())
                    .count()
            })
            .sum();
        let justify = line_style.align == Align::Justify
            && spaces > 0
            && visual.get(index + 1).is_some_and(|next| !next.line_bar);
        let justify_extra = if justify {
            (w - text_width).max(0) as f32 / spaces as f32
        } else {
            0.0
        };
        let mut cursor_x = match line_style.align {
            Align::Left | Align::Justify => x as f32,
            Align::Center => x as f32 + (w - text_width) as f32 / 2.0,
            Align::Right => (x + w - text_width) as f32,
        };
        let origin = cursor_x;
        let spacer = runs.iter().find(|(run, _)| run.place_on.is_none());
        let spacer_size = spacer
            .map(|(_, run_style)| {
                scale(run_style.font_px, slide.output_height, slide.source_height).max(1) as f32
            })
            .unwrap_or(font_px);
        let spacer_family = spacer
            .map(|(_, run_style)| run_style.font_family.clone())
            .unwrap_or_default();
        let spacer_bold = spacer.map(|(_, run_style)| run_style.bold).unwrap_or(false);
        let spacer_spacing = spacer
            .map(|(_, run_style)| output_spacing(run_style.letter_spacing, slide))
            .unwrap_or(0.0);
        let spacer_words = spacer
            .map(|(_, run_style)| output_spacing(run_style.word_spacing, slide))
            .unwrap_or(0.0);
        let spacer_case = spacer
            .map(|(_, run_style)| run_style.text_case)
            .unwrap_or(TextCase::AsWritten);
        let font_line = text::line_height(font_px, line_face) as f32;
        let leading = (bar_h as f32 - font_line).max(0.0) / 2.0;
        let baseline = cursor_y as f32 + leading + text::ascent(font_px, line_face)
            - scale(
                line.shift_y as f32,
                slide.output_height,
                slide.source_height,
            ) as f32;
        for (run, run_style) in &runs {
            let size =
                scale(run_style.font_px, slide.output_height, slide.source_height).max(1) as f32;
            let typeface = text::Typeface {
                family: run_style.font_family.as_str(),
                bold: run_style.bold,
                italic: run_style.italic,
            };
            let tracking = output_spacing(run_style.letter_spacing, slide);
            let word_gap = output_spacing(run_style.word_spacing, slide) + justify_extra;
            let shown = cased(&run.text, run_style.text_case);
            let draw_x = if let Some(prefix) = &run.place_on {
                let prefix = cased(prefix, spacer_case);
                origin
                    + text::measure(
                        &prefix,
                        spacer_size,
                        text::Typeface {
                            family: spacer_family.as_str(),
                            bold: spacer_bold,
                            italic: false,
                        },
                        spacer_spacing,
                        spacer_words,
                    ) as f32
            } else {
                cursor_x
            };
            if run.paint {
                if let Some(shadow) = run_style.shadow {
                    for (dx, dy) in shadow_offsets(shadow, slide) {
                        text::draw(
                            pixels,
                            slide.output_width,
                            slide.output_height,
                            &shown,
                            shadow.color,
                            size,
                            draw_x + dx,
                            baseline + dy,
                            &clip,
                            typeface,
                            tracking,
                            word_gap,
                        );
                    }
                }
                if run_style.stroke_px > 0.0 {
                    let radius = output_spacing(run_style.stroke_px, slide).round() as i32;
                    let stroke = run_style.stroke_color.unwrap_or(run_style.color);
                    if radius > 0 {
                        for (dx, dy) in stroke_offsets(radius) {
                            text::draw(
                                pixels,
                                slide.output_width,
                                slide.output_height,
                                &shown,
                                stroke,
                                size,
                                draw_x + dx,
                                baseline + dy,
                                &clip,
                                typeface,
                                tracking,
                                word_gap,
                            );
                        }
                    }
                }
                text::draw(
                    pixels,
                    slide.output_width,
                    slide.output_height,
                    &shown,
                    run_style.color,
                    size,
                    draw_x,
                    baseline,
                    &clip,
                    typeface,
                    tracking,
                    word_gap,
                );
                if run_style.underline {
                    paint_underline(
                        pixels,
                        slide.output_width,
                        slide.output_height,
                        draw_x,
                        baseline,
                        text::measure(&shown, size, typeface, tracking, word_gap),
                        size,
                        run_style.color,
                        &clip,
                    );
                }
                if run_style.strike {
                    paint_strike(
                        pixels,
                        slide.output_width,
                        slide.output_height,
                        draw_x,
                        baseline,
                        text::measure(&shown, size, typeface, tracking, word_gap),
                        size,
                        run_style.color,
                        &clip,
                    );
                }
            }
            if run.place_on.is_none() {
                cursor_x += text::measure(&shown, size, typeface, tracking, word_gap) as f32;
            }
        }
        cursor_y += bar_h;
        let later_bar = visual[index + 1..]
            .iter()
            .any(|next| next.line_bar && !next.runs.is_empty());
        if line.line_bar && later_bar {
            cursor_y += gap;
        }
        if cursor_y >= y + h {
            break;
        }
    }
    if let Some(before) = before {
        if color_filter {
            blend_rect(
                pixels,
                slide.output_width,
                slide.output_height,
                x,
                y,
                w,
                h,
                &before,
                opacity,
                grayscale,
                invert,
                sepia,
                brightness,
                contrast,
                saturate,
                hue,
            );
        }
        if blur >= 0.5 {
            blur_item(
                pixels,
                slide.output_width,
                slide.output_height,
                x,
                y,
                w,
                h,
                &before,
                blur.round() as i32,
            );
        }
        if blend != Blend::Normal {
            apply_blend(
                pixels,
                slide.output_width,
                slide.output_height,
                x,
                y,
                w,
                h,
                &before,
                blend,
            );
        }
    }
    if let Some(shadow) = style.box_shadow.filter(|shadow| shadow.inset) {
        paint_inset_shadow(
            pixels,
            slide,
            x,
            y,
            w,
            h,
            radius,
            style.elliptical_background,
            shadow,
        );
    }
}

/// FreeShow's blend dropdown. An unknown name stays `normal`.
pub fn blend_mode(name: &str) -> Blend {
    match name.trim() {
        "multiply" => Blend::Multiply,
        "screen" => Blend::Screen,
        "overlay" => Blend::Overlay,
        "soft-light" => Blend::SoftLight,
        "hard-light" => Blend::HardLight,
        "color-dodge" => Blend::ColorDodge,
        "color-burn" => Blend::ColorBurn,
        "lighten" => Blend::Lighten,
        "darken" => Blend::Darken,
        "difference" => Blend::Difference,
        "exclusion" => Blend::Exclusion,
        "hue" => Blend::Hue,
        "saturation" => Blend::Saturation,
        "color" => Blend::Color,
        "luminosity" => Blend::Luminosity,
        _ => Blend::Normal,
    }
}

fn apply_blend(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    before: &[u8],
    mode: Blend,
) {
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    let row_bytes = ((x1 - x0) * 4) as usize;
    let separable = !matches!(
        mode,
        Blend::Hue | Blend::Saturation | Blend::Color | Blend::Luminosity
    );
    for (row_index, row) in (y0..y1).enumerate() {
        for col in 0..(x1 - x0) {
            let dst = ((row * frame_w as i32 + x0 + col) * 4) as usize;
            let src = row_index * row_bytes + (col as usize) * 4;
            if pixels[dst..dst + 4] == before[src..src + 4] {
                continue;
            }
            if separable {
                for channel in 0..3 {
                    pixels[dst + channel] =
                        blend_byte(mode, pixels[dst + channel], before[src + channel]);
                }
            } else {
                let mixed = blend_hsl(
                    mode,
                    [pixels[dst], pixels[dst + 1], pixels[dst + 2]],
                    [before[src], before[src + 1], before[src + 2]],
                );
                pixels[dst..dst + 3].copy_from_slice(&mixed);
            }
        }
    }
}

fn blend_byte(mode: Blend, src: u8, dst: u8) -> u8 {
    let source = f32::from(src) / 255.0;
    let backdrop = f32::from(dst) / 255.0;
    let mixed = match mode {
        Blend::Multiply => source * backdrop,
        Blend::Screen => 1.0 - (1.0 - source) * (1.0 - backdrop),
        Blend::Overlay => {
            if backdrop < 0.5 {
                2.0 * source * backdrop
            } else {
                1.0 - 2.0 * (1.0 - source) * (1.0 - backdrop)
            }
        }
        Blend::HardLight => {
            if source < 0.5 {
                2.0 * source * backdrop
            } else {
                1.0 - 2.0 * (1.0 - source) * (1.0 - backdrop)
            }
        }
        Blend::SoftLight => {
            if source < 0.5 {
                backdrop - (1.0 - 2.0 * source) * backdrop * (1.0 - backdrop)
            } else {
                let dodge = if backdrop < 0.25 {
                    ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop
                } else {
                    backdrop.sqrt()
                };
                backdrop + (2.0 * source - 1.0) * (dodge - backdrop)
            }
        }
        Blend::ColorDodge => {
            if source >= 1.0 {
                1.0
            } else {
                (backdrop / (1.0 - source)).min(1.0)
            }
        }
        Blend::ColorBurn => {
            if source <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - backdrop) / source).min(1.0)
            }
        }
        Blend::Lighten => source.max(backdrop),
        Blend::Darken => source.min(backdrop),
        Blend::Difference => (source - backdrop).abs(),
        Blend::Exclusion => source + backdrop - 2.0 * source * backdrop,
        Blend::Normal | Blend::Hue | Blend::Saturation | Blend::Color | Blend::Luminosity => source,
    };
    (mixed * 255.0).round().clamp(0.0, 255.0) as u8
}

fn blend_hsl(mode: Blend, src: [u8; 3], dst: [u8; 3]) -> [u8; 3] {
    let source = rgb_to_hsl(src);
    let backdrop = rgb_to_hsl(dst);
    let (hue, sat, light) = match mode {
        Blend::Hue => (source.0, backdrop.1, backdrop.2),
        Blend::Saturation => (backdrop.0, source.1, backdrop.2),
        Blend::Color => (source.0, source.1, backdrop.2),
        _ => (backdrop.0, backdrop.1, source.2),
    };
    hsl_to_rgb(hue, sat, light)
}

fn rgb_to_hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let red = f32::from(rgb[0]) / 255.0;
    let green = f32::from(rgb[1]) / 255.0;
    let blue = f32::from(rgb[2]) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let light = (max + min) / 2.0;
    if (max - min) < 1e-6 {
        return (0.0, 0.0, light);
    }
    let delta = max - min;
    let sat = if light > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == red {
        ((green - blue) / delta).rem_euclid(6.0)
    } else if max == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    } / 6.0;
    (hue, sat, light)
}

fn hsl_to_rgb(hue: f32, sat: f32, light: f32) -> [u8; 3] {
    if sat <= 0.0 {
        let gray = (light * 255.0).round().clamp(0.0, 255.0) as u8;
        return [gray, gray, gray];
    }
    let q = if light < 0.5 {
        light * (1.0 + sat)
    } else {
        light + sat - light * sat
    };
    let p = 2.0 * light - q;
    let channel = |offset: f32| {
        let mut t = hue + offset;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let value = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [channel(1.0 / 3.0), channel(0.0), channel(-1.0 / 3.0)]
}

fn copy_rect(pixels: &[u8], frame_w: u32, frame_h: u32, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    let mut copied = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
    for row in y0..y1 {
        let start = ((row * frame_w as i32 + x0) * 4) as usize;
        let end = start + ((x1 - x0) * 4) as usize;
        copied.extend_from_slice(&pixels[start..end]);
    }
    copied
}

fn blend_rect(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    before: &[u8],
    opacity: f32,
    grayscale: f32,
    invert: f32,
    sepia: f32,
    brightness: f32,
    contrast: f32,
    saturate: f32,
    hue: f32,
) {
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    let row_bytes = ((x1 - x0) * 4) as usize;
    for (row_index, row) in (y0..y1).enumerate() {
        for col in 0..(x1 - x0) {
            let dst = ((row * frame_w as i32 + x0 + col) * 4) as usize;
            let src = row_index * row_bytes + (col as usize) * 4;
            if pixels[dst..dst + 4] == before[src..src + 4] {
                continue;
            }
            let mut painted = [
                pixels[dst],
                pixels[dst + 1],
                pixels[dst + 2],
                pixels[dst + 3],
            ];
            if grayscale > 0.0 {
                let luma = 0.2126 * f32::from(painted[0])
                    + 0.7152 * f32::from(painted[1])
                    + 0.0722 * f32::from(painted[2]);
                for sample in &mut painted[..3] {
                    let color = f32::from(*sample);
                    *sample = (color * (1.0 - grayscale) + luma * grayscale).round() as u8;
                }
            }
            if invert > 0.0 {
                for sample in &mut painted[..3] {
                    let color = f32::from(*sample);
                    *sample = (color * (1.0 - invert) + (255.0 - color) * invert).round() as u8;
                }
            }
            if sepia > 0.0 {
                let red = f32::from(painted[0]);
                let green = f32::from(painted[1]);
                let blue = f32::from(painted[2]);
                let toned = [
                    (red * 0.393 + green * 0.769 + blue * 0.189).clamp(0.0, 255.0),
                    (red * 0.349 + green * 0.686 + blue * 0.168).clamp(0.0, 255.0),
                    (red * 0.272 + green * 0.534 + blue * 0.131).clamp(0.0, 255.0),
                ];
                for (sample, tone) in painted[..3].iter_mut().zip(&toned) {
                    let color = f32::from(*sample);
                    *sample = (color * (1.0 - sepia) + tone * sepia).round() as u8;
                }
            }
            if (brightness - 1.0).abs() > 0.001 {
                for sample in &mut painted[..3] {
                    *sample = (f32::from(*sample) * brightness).round().clamp(0.0, 255.0) as u8;
                }
            }
            if (contrast - 1.0).abs() > 0.001 {
                for sample in &mut painted[..3] {
                    let color = f32::from(*sample);
                    *sample = ((color - 128.0) * contrast + 128.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
            if (saturate - 1.0).abs() > 0.001 {
                let red = f32::from(painted[0]);
                let green = f32::from(painted[1]);
                let blue = f32::from(painted[2]);
                let amount = saturate;
                painted[0] = (red * (0.213 + 0.787 * amount)
                    + green * (0.715 - 0.715 * amount)
                    + blue * (0.072 - 0.072 * amount))
                    .round()
                    .clamp(0.0, 255.0) as u8;
                painted[1] = (red * (0.213 - 0.213 * amount)
                    + green * (0.715 + 0.285 * amount)
                    + blue * (0.072 - 0.072 * amount))
                    .round()
                    .clamp(0.0, 255.0) as u8;
                painted[2] = (red * (0.213 - 0.213 * amount)
                    + green * (0.715 - 0.715 * amount)
                    + blue * (0.072 + 0.928 * amount))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            if hue_changes(hue) {
                let (red, green, blue) = hue_spin(
                    f32::from(painted[0]),
                    f32::from(painted[1]),
                    f32::from(painted[2]),
                    hue,
                );
                painted[0] = red.round().clamp(0.0, 255.0) as u8;
                painted[1] = green.round().clamp(0.0, 255.0) as u8;
                painted[2] = blue.round().clamp(0.0, 255.0) as u8;
            }
            for channel in 0..4 {
                let old = f32::from(before[src + channel]);
                let new = f32::from(painted[channel]);
                pixels[dst + channel] = (old * (1.0 - opacity) + new * opacity).round() as u8;
            }
        }
    }
}

fn clipped_rect(
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> (i32, i32, i32, i32) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = x.saturating_add(w).min(frame_w as i32).max(x0);
    let y1 = y.saturating_add(h).min(frame_h as i32).max(y0);
    (x0, y0, x1, y1)
}

/// The last `opacity()` in a CSS filter string. Missing means fully visible.
pub fn filter_opacity(filter: &str) -> f32 {
    filter_amount(filter, "opacity", 1.0).clamp(0.0, 1.0)
}

/// The last `grayscale()` in a CSS filter string. Missing leaves the color unchanged.
pub fn filter_grayscale(filter: &str) -> f32 {
    filter_amount(filter, "grayscale", 0.0).clamp(0.0, 1.0)
}

/// The last `invert()` in a CSS filter string. Missing leaves the color unchanged.
pub fn filter_invert(filter: &str) -> f32 {
    filter_amount(filter, "invert", 0.0).clamp(0.0, 1.0)
}

/// The last `sepia()` in a CSS filter string. Missing leaves the color unchanged.
pub fn filter_sepia(filter: &str) -> f32 {
    filter_amount(filter, "sepia", 0.0).clamp(0.0, 1.0)
}

/// The last `brightness()` in a CSS filter string. Missing means unchanged (`1`).
pub fn filter_brightness(filter: &str) -> f32 {
    filter_amount(filter, "brightness", 1.0).clamp(0.0, 10.0)
}

/// The last `contrast()` in a CSS filter string. Missing means unchanged (`1`).
pub fn filter_contrast(filter: &str) -> f32 {
    filter_amount(filter, "contrast", 1.0).clamp(0.0, 10.0)
}

/// The last `saturate()` in a CSS filter string. Missing means unchanged (`1`).
pub fn filter_saturate(filter: &str) -> f32 {
    filter_amount(filter, "saturate", 1.0).clamp(0.0, 10.0)
}

/// The last `hue-rotate()` in a CSS filter string, in degrees. Missing means unchanged.
pub fn filter_hue_rotate(filter: &str) -> f32 {
    let needle = "hue-rotate(";
    let mut degrees = 0.0;
    let mut rest = filter;
    while let Some(start) = rest.find(needle) {
        rest = &rest[start + needle.len()..];
        let Some(end) = rest.find(')') else {
            break;
        };
        degrees = angle_degrees(rest[..end].trim());
        rest = &rest[end + 1..];
    }
    degrees
}

fn angle_degrees(token: &str) -> f32 {
    if let Some(number) = token.strip_suffix("deg") {
        number.trim().parse().unwrap_or(0.0)
    } else if let Some(number) = token.strip_suffix("turn") {
        number.trim().parse::<f32>().unwrap_or(0.0) * 360.0
    } else if let Some(number) = token.strip_suffix("rad") {
        number.trim().parse::<f32>().unwrap_or(0.0).to_degrees()
    } else if let Some(number) = token.strip_suffix("grad") {
        number.trim().parse::<f32>().unwrap_or(0.0) * 0.9
    } else {
        token.parse().unwrap_or(0.0)
    }
}

fn hue_changes(degrees: f32) -> bool {
    let wrapped = degrees.rem_euclid(360.0);
    wrapped > 0.05 && wrapped < 359.95
}

/// SVG `hueRotate` matrix. A half turn of red lands near cyan.
fn hue_spin(red: f32, green: f32, blue: f32, degrees: f32) -> (f32, f32, f32) {
    let radians = degrees.to_radians();
    let cos = radians.cos();
    let sin = radians.sin();
    let next_red = red * (0.213 + cos * 0.787 - sin * 0.213)
        + green * (0.715 - cos * 0.715 - sin * 0.715)
        + blue * (0.072 - cos * 0.072 + sin * 0.928);
    let next_green = red * (0.213 - cos * 0.213 + sin * 0.143)
        + green * (0.715 + cos * 0.285 + sin * 0.140)
        + blue * (0.072 - cos * 0.072 - sin * 0.283);
    let next_blue = red * (0.213 - cos * 0.213 - sin * 0.787)
        + green * (0.715 - cos * 0.715 + sin * 0.715)
        + blue * (0.072 + cos * 0.928 + sin * 0.072);
    (next_red, next_green, next_blue)
}

/// The last `blur()` in a CSS filter string, in pixels. Missing means sharp.
pub fn filter_blur(filter: &str) -> f32 {
    let needle = "blur(";
    let mut radius: f32 = 0.0;
    let mut rest = filter;
    while let Some(start) = rest.find(needle) {
        rest = &rest[start + needle.len()..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let token = rest[..end]
            .trim()
            .strip_suffix("px")
            .unwrap_or(rest[..end].trim());
        radius = token.trim().parse().unwrap_or(0.0);
        rest = &rest[end + 1..];
    }
    radius.clamp(0.0, 100.0)
}

fn blur_item(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    before: &[u8],
    radius: i32,
) {
    let radius = radius.clamp(1, 16);
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    let width = x1 - x0;
    let height = y1 - y0;
    if width <= 0 || height <= 0 {
        return;
    }
    let row_bytes = (width * 4) as usize;
    let mut layer = vec![0u8; (width * height * 4) as usize];
    for (row_index, row) in (y0..y1).enumerate() {
        for col in 0..width {
            let dst = ((row * frame_w as i32 + x0 + col) * 4) as usize;
            let src = row_index * row_bytes + (col as usize) * 4;
            if pixels[dst..dst + 4] != before[src..src + 4] {
                layer[src..src + 4].copy_from_slice(&pixels[dst..dst + 4]);
                layer[src + 3] = 255;
            }
        }
    }
    let mut tmp = layer.clone();
    box_blur_axis(&layer, &mut tmp, width, height, radius, true);
    box_blur_axis(&tmp, &mut layer, width, height, radius, false);
    for (row_index, row) in (y0..y1).enumerate() {
        for col in 0..width {
            let dst = ((row * frame_w as i32 + x0 + col) * 4) as usize;
            let src = row_index * row_bytes + (col as usize) * 4;
            let alpha = f32::from(layer[src + 3]) / 255.0;
            if alpha <= 0.0 {
                pixels[dst..dst + 4].copy_from_slice(&before[src..src + 4]);
                continue;
            }
            for channel in 0..3 {
                let old = f32::from(before[src + channel]);
                let new = f32::from(layer[src + channel]);
                pixels[dst + channel] = (old * (1.0 - alpha) + new * alpha).round() as u8;
            }
            pixels[dst + 3] = 255;
        }
    }
}

fn box_blur_axis(
    src: &[u8],
    dst: &mut [u8],
    width: i32,
    height: i32,
    radius: i32,
    horizontal: bool,
) {
    for y in 0..height {
        for x in 0..width {
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            let mut alpha = 0u32;
            let mut samples = 0u32;
            for offset in -radius..=radius {
                let (sample_x, sample_y) = if horizontal {
                    (x + offset, y)
                } else {
                    (x, y + offset)
                };
                if sample_x < 0 || sample_y < 0 || sample_x >= width || sample_y >= height {
                    continue;
                }
                let index = ((sample_y * width + sample_x) * 4) as usize;
                let cover = u32::from(src[index + 3]);
                red += u32::from(src[index]) * cover;
                green += u32::from(src[index + 1]) * cover;
                blue += u32::from(src[index + 2]) * cover;
                alpha += cover;
                samples += 1;
            }
            let index = ((y * width + x) * 4) as usize;
            if alpha == 0 || samples == 0 {
                dst[index..index + 4].fill(0);
                continue;
            }
            dst[index] = (red / alpha) as u8;
            dst[index + 1] = (green / alpha) as u8;
            dst[index + 2] = (blue / alpha) as u8;
            dst[index + 3] = (alpha / samples) as u8;
        }
    }
}

fn filter_amount(filter: &str, name: &str, fallback: f32) -> f32 {
    let needle = format!("{name}(");
    let mut amount = fallback;
    let mut rest = filter;
    while let Some(start) = rest.find(&needle) {
        rest = &rest[start + needle.len()..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let token = rest[..end].trim();
        amount = if let Some(percent) = token.strip_suffix('%') {
            percent.trim().parse::<f32>().unwrap_or(fallback * 100.0) / 100.0
        } else {
            token.parse::<f32>().unwrap_or(fallback)
        };
        rest = &rest[end + 1..];
    }
    amount
}

#[derive(Clone, Copy)]
enum VAlign {
    Start,
    Center,
    End,
}

fn vertical_align(css: &str) -> VAlign {
    let Some(value) = css_value(css, "align-items") else {
        return VAlign::Center;
    };
    match value {
        "flex-start" | "start" | "top" => VAlign::Start,
        "flex-end" | "end" | "bottom" => VAlign::End,
        _ => VAlign::Center,
    }
}

fn css_value<'a>(css: &'a str, key: &str) -> Option<&'a str> {
    css.split(';').find_map(|part| {
        let (name, value) = part.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

fn wrap_lines(
    lines: &[StyledLine],
    item_style: &str,
    slide: &StyledSlide,
    max_width: i32,
) -> Vec<StyledLine> {
    lines
        .iter()
        .flat_map(|line| split_line(line, item_style, slide, max_width))
        .collect()
}

fn split_line(
    line: &StyledLine,
    item_style: &str,
    slide: &StyledSlide,
    max_width: i32,
) -> Vec<StyledLine> {
    if max_width <= 8
        || item_style.contains("nowrap")
        || line
            .runs
            .iter()
            .any(|run| run.place_on.is_some() || !run.paint || run.style.contains("nowrap"))
    {
        return vec![line.clone()];
    }
    let words = line_words(line);
    if words.is_empty() {
        return vec![line.clone()];
    }
    let total: i32 = words
        .iter()
        .map(|(text, css)| piece_width(text, css, item_style, slide))
        .sum();
    if total <= max_width {
        return vec![line.clone()];
    }
    let mut rows: Vec<Vec<(String, String)>> = vec![Vec::new()];
    let mut width = 0;
    for (text, css) in words {
        let word = piece_width(&text, &css, item_style, slide);
        if word > max_width {
            for chunk in break_piece(&text, &css, item_style, slide, max_width) {
                if rows.last().is_some_and(|row| !row.is_empty()) {
                    rows.push(Vec::new());
                }
                width = piece_width(&chunk, &css, item_style, slide);
                rows.last_mut().unwrap().push((chunk, css.clone()));
            }
            continue;
        }
        let row_has_words = rows.last().is_some_and(|row| !row.is_empty());
        if width > 0 && width + word > max_width && row_has_words {
            rows.push(Vec::new());
            width = 0;
        }
        rows.last_mut().unwrap().push((text, css));
        width += word;
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| StyledLine {
            align: line.align.clone(),
            line_bar: line.line_bar && index == 0,
            wrapped: index > 0,
            shift_y: line.shift_y,
            runs: merge_words(row),
        })
        .collect()
}

fn line_words(line: &StyledLine) -> Vec<(String, String)> {
    let mut words = Vec::new();
    for run in &line.runs {
        if run.text.is_empty() {
            continue;
        }
        let mut current = String::new();
        for ch in run.text.chars() {
            current.push(ch);
            if ch.is_whitespace() {
                words.push((std::mem::take(&mut current), run.style.clone()));
            }
        }
        if !current.is_empty() {
            words.push((current, run.style.clone()));
        }
    }
    words
}

fn break_piece(
    text: &str,
    css: &str,
    item_style: &str,
    slide: &StyledSlide,
    max_width: i32,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let mut trial = current.clone();
        trial.push(ch);
        if !current.is_empty() && piece_width(&trial, css, item_style, slide) > max_width {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn merge_words(words: Vec<(String, String)>) -> Vec<TextRun> {
    let mut runs = Vec::new();
    for (text, style) in words {
        if let Some(last) = runs.last_mut() {
            let TextRun {
                style: last_style,
                text: last_text,
                ..
            } = last;
            if last_style == &style {
                last_text.push_str(&text);
                continue;
            }
        }
        runs.push(TextRun {
            text,
            style,
            place_on: None,
            paint: true,
        });
    }
    runs
}

fn piece_width(text: &str, css: &str, item_style: &str, slide: &StyledSlide) -> i32 {
    let mut style = parse_style(item_style);
    apply_run_style(&mut style, css);
    let size = scale(style.font_px, slide.output_height, slide.source_height).max(1) as f32;
    let shown = cased(text, style.text_case);
    text::measure(
        &shown,
        size,
        text::Typeface {
            family: style.font_family.as_str(),
            bold: style.bold,
            italic: style.italic,
        },
        output_spacing(style.letter_spacing, slide),
        output_spacing(style.word_spacing, slide),
    )
}

fn vertical_offset(
    item: &StyledItem,
    lines: &[StyledLine],
    style: &BoxStyle,
    slide: &StyledSlide,
    box_h: i32,
    gap: i32,
) -> i32 {
    let mut content = 0;
    let mut counted = 0;
    for line in lines {
        let Some(bar_h) = line_bar_height(line, style, slide, &item.style) else {
            continue;
        };
        content += bar_h;
        counted += 1;
        if line.line_bar {
            content += gap;
        }
    }
    if counted > 0 {
        let bars = lines
            .iter()
            .filter(|line| line.line_bar && !line.runs.is_empty())
            .count();
        if bars > 0 {
            content -= gap;
        }
    }
    let spare = (box_h - content).max(0);
    match vertical_align(&item.align) {
        VAlign::Start => 0,
        VAlign::End => spare,
        VAlign::Center => spare / 2,
    }
}

fn line_bar_height(
    line: &StyledLine,
    style: &BoxStyle,
    slide: &StyledSlide,
    item_css: &str,
) -> Option<i32> {
    let run = line
        .runs
        .iter()
        .find(|run| run.paint && !run.text.is_empty())
        .or_else(|| line.runs.first())?;
    if run.text.is_empty() && line.runs.iter().all(|run| run.text.is_empty()) {
        return None;
    }
    let mut run_style = style.clone();
    apply_run_style(&mut run_style, &run.style);
    let font_px = scale(run_style.font_px, slide.output_height, slide.source_height).max(1) as f32;
    let face = text::Typeface {
        family: run_style.font_family.as_str(),
        bold: run_style.bold,
        italic: run_style.italic,
    };
    Some(line_box(&run.style, item_css, font_px, face, slide))
}

/// `line-height` is a multiplier of the font size, a percent, or a source pixel size.
fn line_box(
    run_css: &str,
    item_css: &str,
    font_px: f32,
    face: text::Typeface<'_>,
    slide: &StyledSlide,
) -> i32 {
    let font_line = text::line_height(font_px, face);
    let Some(value) =
        css_value(run_css, "line-height").or_else(|| css_value(item_css, "line-height"))
    else {
        return font_line;
    };
    if value.eq_ignore_ascii_case("normal") {
        return font_line;
    }
    if let Some(px) = value.strip_suffix("px") {
        return scale(
            px.trim().parse().unwrap_or(0.0),
            slide.output_height,
            slide.source_height,
        )
        .max(1);
    }
    if let Some(percent) = value.strip_suffix('%') {
        let number: f32 = percent.trim().parse().unwrap_or(100.0);
        return (font_px * number / 100.0).round().max(1.0) as i32;
    }
    if let Ok(number) = value.parse::<f32>() {
        return (font_px * number).round().max(1.0) as i32;
    }
    font_line
}

fn paint_underline(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    origin_x: f32,
    baseline: f32,
    width: i32,
    font_px: f32,
    color: Rgba,
    clip: &text::Clip,
) {
    let thick = (font_px / 16.0).round().max(1.0) as i32;
    let top = baseline.round() as i32 + 1;
    let left = origin_x.round() as i32;
    for dy in 0..thick {
        for dx in 0..width {
            let x = left + dx;
            let y = top + dy;
            if x < clip.x || y < clip.y || x >= clip.x + clip.w || y >= clip.y + clip.h {
                continue;
            }
            paint(pixels, frame_w, frame_h, x, y, color);
        }
    }
}

fn paint_strike(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    origin_x: f32,
    baseline: f32,
    width: i32,
    font_px: f32,
    color: Rgba,
    clip: &text::Clip,
) {
    let thick = (font_px / 16.0).round().max(1.0) as i32;
    let mid = (baseline - font_px * 0.3).round() as i32;
    let top = mid - thick / 2;
    let left = origin_x.round() as i32;
    for dy in 0..thick {
        for dx in 0..width {
            let x = left + dx;
            let y = top + dy;
            if x < clip.x || y < clip.y || x >= clip.x + clip.w || y >= clip.y + clip.h {
                continue;
            }
            paint(pixels, frame_w, frame_h, x, y, color);
        }
    }
}

fn apply_run_style(style: &mut BoxStyle, css: &str) {
    let parsed = parse_style(css);
    if css.contains("font-size") {
        style.font_px = parsed.font_px;
    }
    if css.split(';').any(|part| {
        part.split_once(':')
            .is_some_and(|(key, _)| key.trim() == "color")
    }) {
        style.color = parsed.color;
    }
    if css.contains("text-align") {
        style.align = parsed.align;
    }
    if css.contains("font-family") {
        style.font_family = parsed.font_family;
    }
    if css.contains("font-weight") {
        style.bold = parsed.bold;
    }
    if css.contains("font-style") {
        style.italic = parsed.italic;
    }
    if css.contains("text-decoration") {
        style.underline = parsed.underline;
        style.strike = parsed.strike;
    }
    if css.contains("letter-spacing") {
        style.letter_spacing = parsed.letter_spacing;
    }
    if css.contains("word-spacing") {
        style.word_spacing = parsed.word_spacing;
    }
    if css.contains("text-transform") {
        style.text_case = parsed.text_case;
    }
    if css.contains("text-stroke") {
        style.stroke_px = parsed.stroke_px;
        if parsed.stroke_color.is_some() {
            style.stroke_color = parsed.stroke_color;
        }
    }
    if css.contains("text-shadow") {
        style.shadow = parsed.shadow;
    }
}

fn parse_shadow(value: &str) -> Option<TextShadow> {
    let mut rest = value.trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return None;
    }
    let mut lengths = Vec::new();
    while lengths.len() < 3 {
        let trimmed = rest.trim_start();
        let Some(token) = trimmed.split_whitespace().next() else {
            break;
        };
        if px(token).is_none() {
            break;
        }
        lengths.push(px(token).unwrap_or(0.0));
        rest = &trimmed[token.len()..];
    }
    if lengths.is_empty() {
        return None;
    }
    Some(TextShadow {
        x: lengths.first().copied().unwrap_or(0.0),
        y: lengths.get(1).copied().unwrap_or(0.0),
        blur: lengths.get(2).copied().unwrap_or(0.0),
        color: parse_color(rest).unwrap_or(Rgba::BLACK),
    })
}

fn parse_box_shadow(value: &str) -> Option<BoxShadow> {
    let mut rest = value.trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return None;
    }
    let inset = rest
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("inset"));
    let cleaned = rest
        .split_whitespace()
        .filter(|token| !token.eq_ignore_ascii_case("inset"))
        .collect::<Vec<_>>()
        .join(" ");
    rest = cleaned.trim();
    let mut lengths = Vec::new();
    while lengths.len() < 4 {
        let trimmed = rest.trim_start();
        let Some(token) = trimmed.split_whitespace().next() else {
            break;
        };
        if px(token).is_none() {
            break;
        }
        lengths.push(px(token).unwrap_or(0.0));
        rest = &trimmed[token.len()..];
    }
    if lengths.len() < 2 {
        return None;
    }
    Some(BoxShadow {
        x: lengths[0],
        y: lengths[1],
        blur: lengths.get(2).copied().unwrap_or(0.0),
        spread: lengths.get(3).copied().unwrap_or(0.0),
        color: parse_color(rest).unwrap_or(Rgba::BLACK),
        inset,
    })
}

fn in_item_shape(
    px: i32,
    py: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    ellipse: bool,
) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    if ellipse {
        let dx = (px - x) as f32 + 0.5;
        let dy = (py - y) as f32 + 0.5;
        let nx = 2.0 * dx / w as f32 - 1.0;
        let ny = 2.0 * dy / h as f32 - 1.0;
        return nx * nx + ny * ny <= 1.0;
    }
    !outside_round_corner(px - x, py - y, w, h, radius)
}

fn apply_backdrop(
    pixels: &mut [u8],
    slide: &StyledSlide,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    ellipse: bool,
    filter: &str,
) {
    let pad = output_spacing(filter_blur(filter), slide)
        .round()
        .clamp(0.0, 24.0) as i32;
    let bx = x - pad;
    let by = y - pad;
    let bw = (w + pad * 2).max(1);
    let bh = (h + pad * 2).max(1);
    let (cx0, cy0, cx1, cy1) =
        clipped_rect(slide.output_width, slide.output_height, bx, by, bw, bh);
    let cw = cx1 - cx0;
    let ch = cy1 - cy0;
    if cw <= 0 || ch <= 0 {
        return;
    }
    let mut filtered = copy_rect(
        pixels,
        slide.output_width,
        slide.output_height,
        bx,
        by,
        bw,
        bh,
    );
    if pad > 0 {
        box_blur_buffer(&mut filtered, cw, ch, pad);
    }
    let opacity = filter_opacity(filter);
    let grayscale = filter_grayscale(filter);
    let invert = filter_invert(filter);
    let sepia = filter_sepia(filter);
    let brightness = filter_brightness(filter);
    let contrast = filter_contrast(filter);
    let saturate = filter_saturate(filter);
    let hue = filter_hue_rotate(filter);
    let (x0, y0, x1, y1) = clipped_rect(slide.output_width, slide.output_height, x, y, w, h);
    for py in y0..y1 {
        for px in x0..x1 {
            if !in_item_shape(px, py, x, y, w, h, radius, ellipse) {
                continue;
            }
            let sx = (px - cx0).clamp(0, cw - 1) as usize;
            let sy = (py - cy0).clamp(0, ch - 1) as usize;
            let src = (sy * cw as usize + sx) * 4;
            if src + 3 >= filtered.len() {
                continue;
            }
            let color = filter_pixel(
                [
                    filtered[src],
                    filtered[src + 1],
                    filtered[src + 2],
                    filtered[src + 3],
                ],
                opacity,
                grayscale,
                invert,
                sepia,
                brightness,
                contrast,
                saturate,
                hue,
            );
            let dest = ((py as u32 * slide.output_width + px as u32) * 4) as usize;
            if dest + 3 < pixels.len() {
                pixels[dest..dest + 4].copy_from_slice(&color);
            }
        }
    }
}

fn box_blur_buffer(pixels: &mut [u8], width: i32, height: i32, radius: i32) {
    if radius <= 0 || width <= 0 || height <= 0 {
        return;
    }
    let source = pixels.to_vec();
    let radius = radius.max(1);
    let width = width as usize;
    let height = height as usize;
    let r = radius as usize;
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            let y0 = y.saturating_sub(r);
            let y1 = (y + r + 1).min(height);
            let x0 = x.saturating_sub(r);
            let x1 = (x + r + 1).min(width);
            for py in y0..y1 {
                for px in x0..x1 {
                    let i = (py * width + px) * 4;
                    for channel in 0..4 {
                        sum[channel] += u32::from(source[i + channel]);
                    }
                    count += 1;
                }
            }
            let dest = (y * width + x) * 4;
            for channel in 0..4 {
                pixels[dest + channel] = (sum[channel] / count.max(1)) as u8;
            }
        }
    }
}

fn filter_pixel(
    mut painted: [u8; 4],
    opacity: f32,
    grayscale: f32,
    invert: f32,
    sepia: f32,
    brightness: f32,
    contrast: f32,
    saturate: f32,
    hue: f32,
) -> [u8; 4] {
    if grayscale > 0.0 {
        let luma = 0.2126 * f32::from(painted[0])
            + 0.7152 * f32::from(painted[1])
            + 0.0722 * f32::from(painted[2]);
        for sample in &mut painted[..3] {
            let color = f32::from(*sample);
            *sample = (color * (1.0 - grayscale) + luma * grayscale).round() as u8;
        }
    }
    if invert > 0.0 {
        for sample in &mut painted[..3] {
            let color = f32::from(*sample);
            *sample = (color * (1.0 - invert) + (255.0 - color) * invert).round() as u8;
        }
    }
    if sepia > 0.0 {
        let red = f32::from(painted[0]);
        let green = f32::from(painted[1]);
        let blue = f32::from(painted[2]);
        let toned = [
            (red * 0.393 + green * 0.769 + blue * 0.189).clamp(0.0, 255.0),
            (red * 0.349 + green * 0.686 + blue * 0.168).clamp(0.0, 255.0),
            (red * 0.272 + green * 0.534 + blue * 0.131).clamp(0.0, 255.0),
        ];
        for (sample, tone) in painted[..3].iter_mut().zip(&toned) {
            let color = f32::from(*sample);
            *sample = (color * (1.0 - sepia) + tone * sepia).round() as u8;
        }
    }
    if (brightness - 1.0).abs() > 0.001 {
        for sample in &mut painted[..3] {
            *sample = (f32::from(*sample) * brightness).round().clamp(0.0, 255.0) as u8;
        }
    }
    if (contrast - 1.0).abs() > 0.001 {
        for sample in &mut painted[..3] {
            let color = f32::from(*sample);
            *sample = ((color - 128.0) * contrast + 128.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    if (saturate - 1.0).abs() > 0.001 {
        let luma = 0.2126 * f32::from(painted[0])
            + 0.7152 * f32::from(painted[1])
            + 0.0722 * f32::from(painted[2]);
        for sample in &mut painted[..3] {
            let color = f32::from(*sample);
            *sample = (luma + (color - luma) * saturate).round().clamp(0.0, 255.0) as u8;
        }
    }
    if hue_changes(hue) {
        let (r, g, b) = hue_spin(
            f32::from(painted[0]),
            f32::from(painted[1]),
            f32::from(painted[2]),
            hue,
        );
        painted[0] = r.round().clamp(0.0, 255.0) as u8;
        painted[1] = g.round().clamp(0.0, 255.0) as u8;
        painted[2] = b.round().clamp(0.0, 255.0) as u8;
    }
    painted[3] = (f32::from(painted[3]) * opacity).round().clamp(0.0, 255.0) as u8;
    painted
}

fn paint_box_shadow(
    pixels: &mut [u8],
    slide: &StyledSlide,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    ellipse: bool,
    shadow: BoxShadow,
) {
    let ox = scale(shadow.x, slide.output_width, slide.source_width);
    let oy = scale(shadow.y, slide.output_height, slide.source_height);
    let blur = scale(shadow.blur, slide.output_height, slide.source_height).clamp(0, 16);
    let spread = scale(shadow.spread, slide.output_height, slide.source_height);
    let sx = x + ox - spread;
    let sy = y + oy - spread;
    let sw = (w + spread * 2).max(1);
    let sh = (h + spread * 2).max(1);
    let faded = |alpha: u8| Rgba {
        r: shadow.color.r,
        g: shadow.color.g,
        b: shadow.color.b,
        a: alpha,
    };
    if blur <= 0 {
        stamp_shadow_shape(
            pixels,
            slide.output_width,
            slide.output_height,
            sx,
            sy,
            sw,
            sh,
            radius + spread,
            ellipse,
            shadow.color,
        );
        return;
    }
    let samples = ((2 * blur + 1) * (2 * blur + 1)).max(1) as u16;
    let alpha = ((u16::from(shadow.color.a) * 8) / samples.max(8))
        .clamp(1, u16::from(shadow.color.a)) as u8;
    for dy in -blur..=blur {
        for dx in -blur..=blur {
            if dx * dx + dy * dy > blur * blur {
                continue;
            }
            stamp_shadow_shape(
                pixels,
                slide.output_width,
                slide.output_height,
                sx + dx,
                sy + dy,
                sw,
                sh,
                radius + spread,
                ellipse,
                faded(alpha),
            );
        }
    }
}

fn stamp_shadow_shape(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    ellipse: bool,
    color: Rgba,
) {
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    for py in y0..y1 {
        for px in x0..x1 {
            if in_item_shape(px, py, x, y, w, h, radius, ellipse) {
                paint(pixels, frame_w, frame_h, px, py, color);
            }
        }
    }
}

fn paint_inset_shadow(
    pixels: &mut [u8],
    slide: &StyledSlide,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    ellipse: bool,
    shadow: BoxShadow,
) {
    let ox = scale(shadow.x, slide.output_width, slide.source_width);
    let oy = scale(shadow.y, slide.output_height, slide.source_height);
    let blur = scale(shadow.blur, slide.output_height, slide.source_height).clamp(0, 16);
    let spread = scale(shadow.spread, slide.output_height, slide.source_height);
    let inner_x = x + ox + spread;
    let inner_y = y + oy + spread;
    let inner_w = (w - spread * 2).max(1);
    let inner_h = (h - spread * 2).max(1);
    let inner_r = (radius - spread).max(0);
    let (x0, y0, x1, y1) = clipped_rect(slide.output_width, slide.output_height, x, y, w, h);
    for py in y0..y1 {
        for px in x0..x1 {
            if !in_item_shape(px, py, x, y, w, h, radius, ellipse) {
                continue;
            }
            let inside =
                in_item_shape(px, py, inner_x, inner_y, inner_w, inner_h, inner_r, ellipse);
            if inside && blur <= 0 {
                continue;
            }
            if inside {
                let dx = (px - inner_x).min(inner_x + inner_w - 1 - px);
                let dy = (py - inner_y).min(inner_y + inner_h - 1 - py);
                let dist = dx.min(dy);
                if dist >= blur {
                    continue;
                }
                let fade = 1.0 - dist as f32 / blur.max(1) as f32;
                let color = Rgba {
                    r: shadow.color.r,
                    g: shadow.color.g,
                    b: shadow.color.b,
                    a: (f32::from(shadow.color.a) * fade).round() as u8,
                };
                paint(
                    pixels,
                    slide.output_width,
                    slide.output_height,
                    px,
                    py,
                    color,
                );
            } else {
                paint(
                    pixels,
                    slide.output_width,
                    slide.output_height,
                    px,
                    py,
                    shadow.color,
                );
            }
        }
    }
}

fn shadow_offsets(shadow: TextShadow, slide: &StyledSlide) -> Vec<(f32, f32)> {
    let ox = scale(shadow.x, slide.output_width, slide.source_width) as f32;
    let oy = scale(shadow.y, slide.output_height, slide.source_height) as f32;
    let radius = scale(shadow.blur, slide.output_height, slide.source_height).clamp(0, 8);
    let mut offsets = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            offsets.push((ox + dx as f32, oy + dy as f32));
        }
    }
    if offsets.is_empty() {
        offsets.push((ox, oy));
    }
    offsets
}

fn font_family(value: &str) -> String {
    value
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|ch: char| ch == '\'' || ch == '"')
        .to_string()
}

fn align_in(value: &str) -> Option<Align> {
    if value.contains("text-align") {
        value.split(':').nth(1).map(parse_align)
    } else if matches!(value.trim(), "left" | "center" | "right" | "justify") {
        Some(parse_align(value))
    } else {
        None
    }
}

fn blit(
    pixels: &mut [u8],
    slide: &StyledSlide,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    image: &Raster,
    fit: Fit,
    flip_x: bool,
    flip_y: bool,
    crop: Crop,
) {
    if image.width == 0 || image.height == 0 || w <= 0 || h <= 0 {
        return;
    }
    let frame_w = slide.output_width;
    let frame_h = slide.output_height;
    let (draw_w, draw_h, ox, oy) = if fit == Fit::Tile {
        (
            scale(image.width as f32, slide.output_width, slide.source_width).max(1),
            scale(
                image.height as f32,
                slide.output_height,
                slide.source_height,
            )
            .max(1),
            0,
            0,
        )
    } else {
        fitted(image.width, image.height, w, h, fit)
    };
    if draw_w <= 0 || draw_h <= 0 {
        return;
    }
    let crop = crop.clamped();
    let inset_left = (w as f32 * crop.left / 100.0).round() as i32;
    let inset_right = (w as f32 * crop.right / 100.0).round() as i32;
    let inset_top = (h as f32 * crop.top / 100.0).round() as i32;
    let inset_bottom = (h as f32 * crop.bottom / 100.0).round() as i32;
    let vis_x0 = x + inset_left.max(0);
    let vis_y0 = y + inset_top.max(0);
    let vis_x1 = x + w - inset_right.max(0);
    let vis_y1 = y + h - inset_bottom.max(0);
    if fit != Fit::Tile && image.width == 1 && image.height == 1 && image.rgba.len() >= 4 {
        let color = Rgba {
            r: image.rgba[0],
            g: image.rgba[1],
            b: image.rgba[2],
            a: image.rgba[3],
        };
        let fill_x0 = vis_x0.max(x + ox);
        let fill_y0 = vis_y0.max(y + oy);
        let fill_x1 = vis_x1.min(x + ox + draw_w);
        let fill_y1 = vis_y1.min(y + oy + draw_h);
        fill_rect(
            pixels,
            frame_w,
            frame_h,
            fill_x0,
            fill_y0,
            fill_x1 - fill_x0,
            fill_y1 - fill_y0,
            color,
        );
        return;
    }
    for dy in inset_top.max(0)..(h - inset_bottom.max(0)) {
        let dest_y = y + dy;
        if dest_y < 0 || dest_y >= frame_h as i32 {
            continue;
        }
        let local_y = if fit == Fit::Tile {
            dy.rem_euclid(draw_h)
        } else {
            dy - oy
        };
        if local_y < 0 || local_y >= draw_h {
            continue;
        }
        let mut sy = (local_y as u32 * image.height) / draw_h as u32;
        if flip_y {
            sy = image.height - 1 - sy;
        }
        let src_row = (sy * image.width) * 4;
        let dest_row = (dest_y as u32 * frame_w) * 4;
        for dx in inset_left.max(0)..(w - inset_right.max(0)) {
            let dest_x = x + dx;
            if dest_x < 0 || dest_x >= frame_w as i32 {
                continue;
            }
            let local_x = if fit == Fit::Tile {
                dx.rem_euclid(draw_w)
            } else {
                dx - ox
            };
            if local_x < 0 || local_x >= draw_w {
                continue;
            }
            let mut sx = (local_x as u32 * image.width) / draw_w as u32;
            if flip_x {
                sx = image.width - 1 - sx;
            }
            let src = (src_row + sx * 4) as usize;
            if src + 3 >= image.rgba.len() {
                continue;
            }
            let dst = (dest_row + dest_x as u32 * 4) as usize;
            let alpha = image.rgba[src + 3];
            if alpha == 255 {
                pixels[dst..dst + 4].copy_from_slice(&image.rgba[src..src + 4]);
            } else if alpha > 0 {
                let cover = u16::from(alpha);
                let remain = 255 - cover;
                for channel in 0..3 {
                    let below = u16::from(pixels[dst + channel]);
                    let above = u16::from(image.rgba[src + channel]);
                    pixels[dst + channel] = ((above * cover + below * remain) / 255) as u8;
                }
                pixels[dst + 3] = 255;
            }
        }
    }
}

fn fitted(image_w: u32, image_h: u32, box_w: i32, box_h: i32, fit: Fit) -> (i32, i32, i32, i32) {
    let image_w = image_w as i32;
    let image_h = image_h as i32;
    let (draw_w, draw_h) = match fit {
        Fit::Fill | Fit::Tile => (box_w, box_h),
        Fit::Contain => {
            if box_w * image_h <= box_h * image_w {
                (box_w, (box_w * image_h) / image_w)
            } else {
                ((box_h * image_w) / image_h, box_h)
            }
        }
        Fit::Cover => {
            if box_w * image_h >= box_h * image_w {
                (box_w, (box_w * image_h) / image_w.max(1))
            } else {
                ((box_h * image_w) / image_h.max(1), box_h)
            }
        }
        Fit::Blur => {
            if box_w * image_h <= box_h * image_w {
                (box_w, (box_w * image_h) / image_w.max(1))
            } else {
                ((box_h * image_w) / image_h.max(1), box_h)
            }
        }
    };
    (
        draw_w.max(1),
        draw_h.max(1),
        (box_w - draw_w) / 2,
        (box_h - draw_h) / 2,
    )
}

fn fill(pixels: &mut [u8], width: u32, height: u32, color: Rgba) {
    fill_rect(
        pixels,
        width,
        height,
        0,
        0,
        width as i32,
        height as i32,
        color,
    );
}

fn fill_round_rect(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: Rgba,
) {
    let radius = radius.min(w / 2).min(h / 2).max(0);
    for dy in 0..h {
        for dx in 0..w {
            if outside_round_corner(dx, dy, w, h, radius) {
                continue;
            }
            paint(pixels, frame_w, frame_h, x + dx, y + dy, color);
        }
    }
}

fn outside_round_corner(dx: i32, dy: i32, w: i32, h: i32, radius: i32) -> bool {
    if radius <= 0 {
        return false;
    }
    let (cx, cy) = if dx < radius && dy < radius {
        (radius, radius)
    } else if dx >= w - radius && dy < radius {
        (w - radius - 1, radius)
    } else if dx < radius && dy >= h - radius {
        (radius, h - radius - 1)
    } else if dx >= w - radius && dy >= h - radius {
        (w - radius - 1, h - radius - 1)
    } else {
        return false;
    };
    let ddx = dx - cx;
    let ddy = dy - cy;
    ddx * ddx + ddy * ddy > radius * radius
}

fn fill_item_background(
    pixels: &mut [u8],
    slide: &StyledSlide,
    style: &BoxStyle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
) {
    if style.gradient.is_none() && style.background.is_none() {
        return;
    }
    let frame_w = slide.output_width;
    let frame_h = slide.output_height;
    if let Some(gradient) = &style.gradient
        && !gradient.stops.is_empty()
    {
        let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
        let cx = x as f32 + w as f32 / 2.0;
        let cy = y as f32 + h as f32 / 2.0;
        let radius_to_corner = (w as f32).hypot(h as f32) / 2.0;
        let linear = gradient.linear_angle_deg.map(|angle| {
            // Screen y grows downward. CSS 0deg points up.
            let (dx, cos) = angle.to_radians().sin_cos();
            let dy = -cos;
            let length = (w as f32 * dx.abs() + h as f32 * dy.abs()).max(1.0);
            (dx / length, dy / length)
        });
        let focus = style
            .pptx_radial_focus
            .map(|[x, y]| (x, y, 1.0 - (x * x + y * y)));
        for py in y0..y1 {
            for px in x0..x1 {
                if in_item_shape(px, py, x, y, w, h, radius, style.elliptical_background) {
                    let offset_x = px as f32 + 0.5 - cx;
                    let offset_y = py as f32 + 0.5 - cy;
                    let position = match linear {
                        Some((dx, dy)) => offset_x * dx + offset_y * dy + 0.5,
                        None => {
                            if let Some((fx, fy, remaining)) = focus {
                                // Intersect the ray from the focus through this pixel with the outer circle.
                                let dx = offset_x / radius_to_corner - fx;
                                let dy = offset_y / radius_to_corner - fy;
                                let dot = dx * fx + dy * fy;
                                let distance_squared = dx * dx + dy * dy;
                                ((dot * dot + distance_squared * remaining).sqrt() + dot)
                                    / remaining
                            } else {
                                (offset_x * offset_x + offset_y * offset_y).sqrt()
                                    / radius_to_corner
                            }
                        }
                    };
                    paint(
                        pixels,
                        frame_w,
                        frame_h,
                        px,
                        py,
                        interpolate_stops(&gradient.stops, position),
                    );
                }
            }
        }
        return;
    }
    let Some(background) = style.background else {
        return;
    };
    if style.elliptical_background {
        let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
        for py in y0..y1 {
            for px in x0..x1 {
                if in_item_shape(px, py, x, y, w, h, radius, true) {
                    paint(pixels, frame_w, frame_h, px, py, background);
                }
            }
        }
    } else if radius > 0 {
        fill_round_rect(pixels, frame_w, frame_h, x, y, w, h, radius, background);
    } else {
        fill_rect(pixels, frame_w, frame_h, x, y, w, h, background);
    }
}

fn parse_radial_focus(value: &str) -> Option<[f32; 2]> {
    let mut components = value.split_whitespace();
    let x = components.next()?.parse::<f32>().ok()?;
    let y = components.next()?.parse::<f32>().ok()?;
    (components.next().is_none() && x * x + y * y < 1.0).then_some([x, y])
}

fn parse_gradient(value: &str) -> Option<Gradient> {
    let value = value.trim().strip_suffix(')')?;
    let (inner, radial) = if let Some(inner) = value.strip_prefix("linear-gradient(") {
        (inner, false)
    } else {
        (value.strip_prefix("radial-gradient(")?, true)
    };
    let parts = split_gradient_args(inner);
    let first = parts.first()?.trim();
    let (linear_angle_deg, raw) = if radial {
        if !matches!(first, "circle" | "circle at center") {
            return None;
        }
        (None, &parts[1..])
    } else if let Some(degrees) = first.strip_suffix("deg") {
        let parsed = degrees.trim().parse::<f32>().ok()?;
        (Some(parsed.is_finite().then_some(parsed)?), &parts[1..])
    } else {
        (Some(180.0), parts.as_slice())
    };
    if raw.len() < 2 {
        return None;
    }
    let last = (raw.len() - 1) as f32;
    let mut stops = Vec::new();
    for (index, part) in raw.iter().enumerate() {
        let part = part.trim();
        let (color, position) = if let Some((color, percent)) =
            part.rsplit_once(char::is_whitespace)
            && let Some(number) = percent.trim().strip_suffix('%')
            && let Ok(parsed) = number.parse::<f32>()
            && parsed.is_finite()
        {
            (parse_color(color.trim())?, (parsed / 100.0).clamp(0.0, 1.0))
        } else {
            (parse_color(part)?, index as f32 / last)
        };
        stops.push((position, color));
    }
    stops.sort_by(|left, right| left.0.total_cmp(&right.0));
    Some(Gradient {
        linear_angle_deg,
        stops,
    })
}

fn split_gradient_args(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.trim().is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn interpolate_stops(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    if t <= stops[0].0 {
        return stops[0].1;
    }
    if t >= stops[stops.len() - 1].0 {
        return stops[stops.len() - 1].1;
    }
    for pair in stops.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t >= t0 && t <= t1 {
            let span = (t1 - t0).max(f32::EPSILON);
            let f = ((t - t0) / span).clamp(0.0, 1.0);
            return lerp_rgba(c0, c1, f);
        }
    }
    stops[stops.len() - 1].1
}

fn lerp_rgba(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let (t, alpha) = if from.a == to.a {
        (t, from.a)
    } else {
        let to_alpha = f32::from(to.a) * t;
        let alpha = f32::from(from.a) * (1.0 - t) + to_alpha;
        if alpha == 0.0 {
            return Rgba::TRANSPARENT;
        }
        (to_alpha / alpha, alpha.round() as u8)
    };
    let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t).round() as u8;
    Rgba {
        r: mix(from.r, to.r),
        g: mix(from.g, to.g),
        b: mix(from.b, to.b),
        a: alpha,
    }
}

fn fill_rect(
    pixels: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Rgba,
) {
    let (x0, y0, x1, y1) = clipped_rect(frame_w, frame_h, x, y, w, h);
    if color.a == 0 || x0 >= x1 || y0 >= y1 {
        return;
    }
    if color.a == 255 {
        let pixel = [color.r, color.g, color.b, 255];
        for py in y0..y1 {
            let start = ((py as u32 * frame_w + x0 as u32) * 4) as usize;
            let end = ((py as u32 * frame_w + x1 as u32) * 4) as usize;
            for chunk in pixels[start..end].as_chunks_mut::<4>().0 {
                chunk.copy_from_slice(&pixel);
            }
        }
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            paint(pixels, frame_w, frame_h, px, py, color);
        }
    }
}

fn scale(value: f32, output: u32, source: f32) -> i32 {
    if source <= 0.0 {
        return value.round() as i32;
    }
    (value * output as f32 / source).round() as i32
}

fn px(value: &str) -> Option<f32> {
    value.trim().trim_end_matches("px").trim().parse().ok()
}

fn parse_align(value: &str) -> Align {
    match value.trim() {
        "left" => Align::Left,
        "right" => Align::Right,
        "justify" => Align::Justify,
        _ => Align::Center,
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let hex = hex.trim();
    let full = match hex.len() {
        3 | 4 => hex
            .chars()
            .map(|ch| format!("{ch}{ch}"))
            .collect::<String>(),
        6 | 8 => hex.to_string(),
        _ => return None,
    };
    let r = u8::from_str_radix(&full[0..2], 16).ok()?;
    let g = u8::from_str_radix(&full[2..4], 16).ok()?;
    let b = u8::from_str_radix(&full[4..6], 16).ok()?;
    let a = if full.len() == 8 {
        u8::from_str_radix(&full[6..8], 16).ok()?
    } else {
        255
    };
    Some(Rgba { r, g, b, a })
}

fn parse_alpha(value: &str) -> Option<u8> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        let number: f32 = percent.trim().parse().ok()?;
        return Some(alpha_byte(number / 100.0));
    }
    let number: f32 = value.parse().ok()?;
    Some(alpha_byte(number))
}

fn alpha_byte(number: f32) -> u8 {
    if number <= 1.0 {
        (number.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        number.clamp(0.0, 255.0).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_ellipse_preserves_background_in_corners_and_obeys_bounds() {
        let mut slide = sample();
        slide.items[0].lines.clear();
        slide.items[0].style =
            "left:-40px;top:20px;width:160px;height:80px;background-color:#ff000080;border-radius:50%;"
                .into();
        let frame = render_styled(&slide);
        assert_eq!(frame.pixel(52, 11), Rgba::BLACK);
        assert_eq!(frame.pixel(60, 30), Rgba::BLACK);
        assert_eq!(frame.pixel(0, 30), parse_color("#800000").unwrap());
        assert_eq!(frame.pixel(40, 30), parse_color("#800000").unwrap());
    }

    fn sample() -> StyledSlide {
        StyledSlide {
            output_width: 960,
            output_height: 540,
            source_width: SOURCE_WIDTH,
            source_height: SOURCE_HEIGHT,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "top:0px;left:0px;width:1920px;height:200px;background-color:#FF0000;"
                    .into(),
                align: String::new(),
                lines: vec![StyledLine {
                    align: "text-align: left;".into(),
                    line_bar: true,
                    wrapped: false,
                    shift_y: 0,
                    runs: vec![TextRun {
                        text: "A".into(),
                        style: "font-size: 80px;color: #FFFFFF;".into(),
                        place_on: None,
                        paint: true,
                    }],
                }],
                image: None,
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x: false,
                flip_y: false,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: Crop::default(),
            }],
        }
    }

    #[test]
    fn parses_freeshow_item_style() {
        let style = parse_style(
            "top:88px;left:50px;height:904px;width:1820px;font-size: 100px;color: #FFFFFF;text-align: center;",
        );
        assert_eq!(style.top, 88.0);
        assert_eq!(style.left, 50.0);
        assert_eq!(style.height, 904.0);
        assert_eq!(style.width, 1820.0);
        assert_eq!(style.font_px, 100.0);
        assert_eq!(style.color, Rgba::WHITE);
        assert_eq!(style.align, Align::Center);
        let named = parse_style("font-family: Arial, sans-serif;font-weight: bold;");
        assert_eq!(named.font_family, "Arial");
        assert!(named.bold);
        let faded = parse_color("rgb(255 0 0 / 0.5)").unwrap();
        assert_eq!(faded.r, 255);
        assert_eq!(faded.a, 128);
        let shadow = parse_style("text-shadow: 2px 2px 10px #000000;")
            .shadow
            .unwrap();
        assert_eq!(shadow.x, 2.0);
        assert_eq!(shadow.y, 2.0);
        assert_eq!(shadow.blur, 10.0);
        assert_eq!(shadow.color, Rgba::BLACK);
        let faded_shadow = parse_style("text-shadow: 2px 2px 4px rgb(0 0 0 / 80%);")
            .shadow
            .unwrap();
        assert_eq!(faded_shadow.color.a, 204);
        assert!(parse_style("text-shadow: none;").shadow.is_none());
        let box_shadow = parse_style("box-shadow: 2px 2px 8px 0 rgb(0 0 0 / 0.5);")
            .box_shadow
            .unwrap();
        assert_eq!(box_shadow.x, 2.0);
        assert_eq!(box_shadow.y, 2.0);
        assert_eq!(box_shadow.blur, 8.0);
        assert_eq!(box_shadow.spread, 0.0);
        assert_eq!(box_shadow.color.a, 128);
        assert!(!box_shadow.inset);
        let inset = parse_style("box-shadow: inset 4px 6px 2px 3px #ff0000;")
            .box_shadow
            .unwrap();
        assert!(inset.inset);
        assert_eq!(inset.spread, 3.0);
        assert_eq!(
            inset.color,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert!(parse_style("box-shadow: none;").box_shadow.is_none());
        let rounded = parse_style("border-radius: 8px;backdrop-filter: blur(10px);");
        assert_eq!(rounded.radius, 8.0);
        assert!(!rounded.elliptical_background);
        assert_eq!(rounded.backdrop, "blur(10px)");
        assert!(parse_style("border-radius: 50%;").elliptical_background);
        let gradient = parse_style("background: linear-gradient(90deg, #FF0000 0%, #0000FF 100%);");
        let fill = gradient.gradient.unwrap();
        assert_eq!(fill.linear_angle_deg, Some(90.0));
        assert_eq!(fill.stops[0].1.r, 255);
        assert_eq!(fill.stops[1].1.b, 255);
        assert_eq!(gradient.background.unwrap().r, 255);
    }

    #[test]
    fn linear_gradients_are_painted_across_the_item_box() {
        let frame = render_styled(&style_slide(
            "left:0px;top:0px;width:1920px;height:1080px;background:linear-gradient(90deg, #FF0000 0%, #0000FF 100%);",
        ));
        let left = frame.pixel(0, 270);
        let right = frame.pixel(959, 270);
        assert!(
            left.r > 200 && left.b < 40,
            "left should be red, got {left:?}"
        );
        assert!(
            right.b > 200 && right.r < 40,
            "right should be blue, got {right:?}"
        );
    }

    #[test]
    fn circular_radial_gradients_are_centered_on_the_item_box() {
        let frame = render_styled(&style_slide(
            "left:0px;top:0px;width:1920px;height:1080px;background:radial-gradient(circle at center, #FF0000 0%, #0000FF 100%);",
        ));
        let center = frame.pixel(479, 269);
        let edge = frame.pixel(479, 0);
        let corner = frame.pixel(0, 0);
        assert!(center.r > 245 && center.b < 10, "{center:?}");
        assert!(edge.r > 120 && edge.b > 120, "{edge:?}");
        assert!(corner.r < 10 && corner.b > 245, "{corner:?}");
    }

    #[test]
    fn powerpoint_radial_gradient_preserves_focus_and_outer_circle() {
        let frame = render_styled(&style_slide(
            "left:0px;top:0px;width:1920px;height:1080px;background:radial-gradient(circle at center, #FF0000 0%, #0000FF 100%);--pptx-radial-focus:0.5 0;",
        ));
        let focus = frame.pixel(755, 269);
        let center = frame.pixel(479, 269);
        let corner = frame.pixel(959, 539);
        assert!(focus.r > 245 && focus.b < 10, "{focus:?}");
        assert!((165..=175).contains(&center.r), "{center:?}");
        assert!(corner.r < 10 && corner.b > 245, "{corner:?}");
    }

    #[test]
    fn transparent_gradient_stops_blend_without_an_invisible_tint() {
        let mut slide = style_slide(
            "left:0px;top:0px;width:1920px;height:1080px;background:linear-gradient(90deg, #FF0000 0%, #0000FF00 100%);",
        );
        slide.background = Rgba::TRANSPARENT;
        let mut base = render_styled(&StyledSlide {
            background: parse_color("#00FF00").unwrap(),
            items: Vec::new(),
            ..slide.clone()
        });
        composite_styled(&mut base, &slide);
        let pixel = base.pixel(479, 270);
        assert!((125..=130).contains(&pixel.r), "{pixel:?}");
        assert!((125..=130).contains(&pixel.g), "{pixel:?}");
        assert_eq!(pixel.b, 0);
    }

    #[test]
    fn tiled_media_repeats_the_picture_across_the_item_box() {
        let mut slide = style_slide("left:0px;top:0px;width:1920px;height:1080px;");
        // A 4x1 source-pixel stamp becomes a 2 output-pixel-wide tile at 960x540.
        slide.items[0].fit = Fit::Tile;
        slide.items[0].image = Some(Raster {
            width: 4,
            height: 1,
            rgba: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
        });
        let frame = render_styled(&slide);
        let left = frame.pixel(0, 10);
        let mid = frame.pixel(1, 10);
        let wrap = frame.pixel(2, 10);
        assert_eq!(
            left,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert_eq!(
            mid,
            Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 255
            }
        );
        assert_eq!(wrap, left);
    }

    fn style_slide(style: &str) -> StyledSlide {
        let mut slide = sample();
        slide.items[0].style = style.into();
        slide.items[0].lines.clear();
        slide
    }

    #[test]
    fn box_shadow_and_rounded_corners_and_backdrop_filters_are_painted() {
        let shadow = render_styled(&style_slide(
            "left:200px;top:200px;width:200px;height:200px;background:#ffffff;box-shadow: 200px 0px 0px 0 #ff0000;",
        ));
        // The white box is around x=100,y=100 at 960x540. A 200px source shadow is 100 output pixels to the right.
        let x = 100 + 100 + 20;
        let y = 100 + 20;
        let i = ((y * 960 + x) * 4) as usize;
        assert!(
            shadow.pixels[i] > 200 && shadow.pixels[i + 1] < 40 && shadow.pixels[i + 2] < 40,
            "expected red box-shadow, got {:?}",
            &shadow.pixels[i..i + 4]
        );
        let sharp = render_styled(&style_slide(
            "left:0px;top:0px;width:400px;height:400px;background:#ffffff;",
        ));
        let rounded = render_styled(&style_slide(
            "left:0px;top:0px;width:400px;height:400px;background:#ffffff;border-radius: 200px;",
        ));
        // The far corner of a rounded box should stay the slide background.
        // The box is 200x200 output pixels; (199,0) sits outside a 100px corner radius.
        let c = 199 * 4;
        assert_eq!(&sharp.pixels[c..c + 3], &[255, 255, 255]);
        assert!(
            rounded.pixels[c] < 40 && rounded.pixels[c + 1] < 40 && rounded.pixels[c + 2] < 40,
            "rounded corner should stay black, got {:?}",
            &rounded.pixels[c..c + 4]
        );
        let mut under = sample();
        under.background = Rgba {
            r: 0,
            g: 0,
            b: 255,
            a: 255,
        };
        under.items.insert(
            0,
            StyledItem {
                style: "left:0px;top:0px;width:1920px;height:1080px;background:#0000ff;".into(),
                align: String::new(),
                lines: Vec::new(),
                image: None,
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x: false,
                flip_y: false,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: Crop::default(),
            },
        );
        // Replace the sample item with a half-cover backdrop that inverts the blue field.
        under.items[1].style =
            "left:0px;top:0px;width:960px;height:1080px;backdrop-filter: invert(1);".into();
        under.items[1].lines.clear();
        let frame = render_styled(&under);
        // Left half should invert blue -> yellow-ish (255-0, 255-0, 255-255) = (255,255,0)
        let left = ((100 * 960 + 100) * 4) as usize;
        assert!(
            frame.pixels[left] > 200 && frame.pixels[left + 1] > 200 && frame.pixels[left + 2] < 40,
            "backdrop invert, left={:?}",
            &frame.pixels[left..left + 4]
        );
    }

    #[test]
    fn text_shadow_is_painted_beside_the_letters() {
        let plain = render_styled(&shadow_slide("font-size: 48px;color: #FFFFFF;"));
        let shadowed = render_styled(&shadow_slide(
            "font-size: 48px;color: #FFFFFF;text-shadow: 40px 0px 0px #FF0000;",
        ));
        assert!(frame_has_red(&shadowed));
        assert!(!frame_has_red(&plain));
    }

    fn shadow_slide(run: &str) -> StyledSlide {
        StyledSlide {
            output_width: 160,
            output_height: 80,
            source_width: 160.0,
            source_height: 80.0,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "left:0px;top:0px;width:160px;height:80px;".into(),
                align: "align-items: flex-start;".into(),
                lines: vec![StyledLine {
                    align: "left".into(),
                    line_bar: true,
                    wrapped: false,
                    shift_y: 0,
                    runs: vec![TextRun {
                        text: "A".into(),
                        style: run.into(),
                        place_on: None,
                        paint: true,
                    }],
                }],
                image: None,
                fit: Fit::Contain,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x: false,
                flip_y: false,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: Crop::default(),
            }],
        }
    }

    #[test]
    fn a_line_background_covers_the_line_and_rounds_the_corner() {
        let mut slide = shadow_slide("font-size: 20px;color: #FFFFFF;");
        slide.items[0].line_bg = Some(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
        slide.items[0].line_radius = 12.0;
        let frame = render_styled(&slide);
        assert_eq!(
            frame.pixel(0, 0),
            Rgba::BLACK,
            "the rounded corner stays clear"
        );
        assert_eq!(frame.pixel(20, 4).r, 255, "the line bar is red");
    }

    #[test]
    fn lines_are_centered_unless_the_item_aligns_them() {
        let mut slide = shadow_slide("font-size: 20px;color: #FFFFFF;");
        slide.items[0].align.clear();
        slide.items[0].line_bg = Some(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
        let centered = render_styled(&slide);
        assert_ne!(
            centered.pixel(4, 2).r,
            255,
            "the bar is not pinned to the top"
        );
        assert_eq!(
            centered.pixel(4, 40).r,
            255,
            "the bar sits in the middle of the item"
        );
        slide.items[0].align = "align-items: flex-end;".into();
        let bottom = render_styled(&slide);
        assert_eq!(bottom.pixel(4, 2).r, 0);
        assert!(
            bottom.pixel(4, 70).r > 200,
            "flex-end keeps the bar at the bottom"
        );
    }

    #[test]
    fn line_height_stretches_each_lyric_line() {
        let mut slide = shadow_slide("font-size: 20px;color: #FFFFFF;");
        slide.output_height = 160;
        slide.source_height = 160.0;
        slide.items[0].style = "left:0px;top:0px;width:160px;height:160px;".into();
        slide.items[0].line_bg = Some(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
        let second = slide.items[0].lines[0].clone();
        slide.items[0].lines.push(second);
        let packed = render_styled(&slide);
        let spaced_style = "font-size: 20px;color: #FFFFFF;line-height: 80px;";
        slide.items[0].lines[0].runs[0].style = spaced_style.into();
        slide.items[0].lines[1].runs[0].style = spaced_style.into();
        let spaced = render_styled(&slide);
        assert_eq!(packed.pixel(150, 70), Rgba::BLACK);
        assert_eq!(spaced.pixel(150, 70).r, 255);
    }

    #[test]
    fn italic_leans_the_letters_to_the_right() {
        let mut slide = shadow_slide("font-size: 64px;color: #FFFFFF;");
        slide.items[0].lines[0].runs[0].text = "l".into();
        let upright = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 64px;color: #FFFFFF;font-style: italic;".into();
        let italic = render_styled(&slide);
        assert!(rightmost_ink(&italic) > rightmost_ink(&upright));
    }

    fn rightmost_ink(frame: &crate::Frame) -> i32 {
        let mut max = 0;
        for y in 0..frame.height {
            for x in 0..frame.width {
                if frame.pixel(x, y).r > 180 {
                    max = max.max(x as i32);
                }
            }
        }
        max
    }

    #[test]
    fn an_underline_sits_under_the_letters() {
        let mut slide = shadow_slide("font-size: 48px;color: #FFFFFF;");
        slide.items[0].lines[0].runs[0].text = "Hello".into();
        let plain = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 48px;color: #FFFFFF;text-decoration: underline;".into();
        let lined = render_styled(&slide);
        assert!(lowest_ink(&lined) > lowest_ink(&plain));
    }

    #[test]
    fn a_strikethrough_cuts_through_the_letters() {
        let mut slide = shadow_slide("font-size: 48px;color: #FFFFFF;");
        slide.items[0].lines[0].runs[0].text = "l l".into();
        let plain = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 48px;color: #FFFFFF;text-decoration: line-through;".into();
        let struck = render_styled(&slide);
        assert!(ink_count(&struck) > ink_count(&plain));
    }

    #[test]
    fn item_opacity_fades_the_letters() {
        let mut slide = shadow_slide("font-size: 48px;color: #FFFFFF;");
        let full = render_styled(&slide);
        slide.items[0].opacity = 0.0;
        let gone = render_styled(&slide);
        slide.items[0].opacity = 0.4;
        let faded = render_styled(&slide);
        assert!(ink_count(&full) > 0);
        assert_eq!(ink_count(&gone), 0);
        assert!(brightest(&faded) < brightest(&full));
        assert!(brightest(&faded) > 20);
        assert!((filter_opacity("blur(2px) opacity(40%)") - 0.4).abs() < 0.001);
    }

    #[test]
    fn a_stored_filter_grays_the_picture() {
        let image = Raster {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        let plain = placed_image(&image, 32, 18, Fit::Fill, false, false, "", "", 6.0, 0.3);
        let gray = placed_image(
            &image,
            32,
            18,
            Fit::Fill,
            false,
            false,
            "grayscale(1)",
            "",
            6.0,
            0.3,
        );
        let multiplied = placed_image(
            &image,
            32,
            18,
            Fit::Fill,
            false,
            false,
            "",
            "multiply",
            6.0,
            0.3,
        );
        assert!(frame_has_red(&plain));
        assert!(!frame_has_red(&gray));
        assert!(channel_ink(&multiplied, 0) < 20);
        let wide = Raster {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let cropped = crop_raster(&wide, 1, 0, 0, 0);
        assert_eq!(cropped.width, 1);
        assert_eq!(cropped.rgba, vec![0, 0, 255, 255]);
        let wide = Raster {
            width: 4,
            height: 1,
            rgba: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
        };
        let contained = placed_image(&wide, 4, 4, Fit::Contain, false, false, "", "", 6.0, 0.3);
        let blurred = placed_image(&wide, 4, 4, Fit::Blur, false, false, "", "", 6.0, 0.3);
        assert_eq!(contained.pixel(0, 3), crate::Rgba::BLACK);
        assert!(blurred.pixel(0, 3).r > 20);
        assert!(blurred.pixel(0, 1).r > 180);
    }

    #[test]
    fn one_pixel_contain_fills_the_centered_square() {
        let image = Raster {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        let frame = placed_image(&image, 8, 4, Fit::Contain, false, false, "", "", 6.0, 0.3);
        assert_eq!(frame.pixel(0, 0), crate::Rgba::BLACK);
        assert_eq!(
            frame.pixel(4, 1),
            crate::Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert_eq!(frame.pixel(7, 0), crate::Rgba::BLACK);
    }

    #[test]
    fn grayscale_turns_a_red_letter_gray() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let color = render_styled(&slide);
        slide.items[0].grayscale = 1.0;
        let gray = render_styled(&slide);
        assert!(frame_has_red(&color));
        assert!(!frame_has_red(&gray));
        assert!((filter_grayscale("opacity(1) grayscale(100%)") - 1.0).abs() < 0.001);
    }

    #[test]
    fn invert_flips_a_red_letter_toward_cyan() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let color = render_styled(&slide);
        slide.items[0].invert = 1.0;
        let flipped = render_styled(&slide);
        assert!(frame_has_red(&color));
        assert!(!frame_has_red(&flipped));
        assert!(channel_ink(&flipped, 1) > 180);
        assert!((filter_invert("invert(100%)") - 1.0).abs() < 0.001);
    }

    #[test]
    fn sepia_warms_a_red_letter() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let color = render_styled(&slide);
        slide.items[0].sepia = 1.0;
        let warm = render_styled(&slide);
        assert!(frame_has_red(&color));
        assert!(!frame_has_red(&warm));
        assert!(channel_ink(&warm, 0) > channel_ink(&warm, 1));
        assert!(channel_ink(&warm, 1) > 40);
        assert!((filter_sepia("sepia(1)") - 1.0).abs() < 0.001);
    }

    #[test]
    fn brightness_dims_a_red_letter() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let full = render_styled(&slide);
        slide.items[0].brightness = 0.0;
        let dark = render_styled(&slide);
        assert!(frame_has_red(&full));
        assert!(!frame_has_red(&dark));
        assert!((filter_brightness("brightness(50%)") - 0.5).abs() < 0.001);
        assert!((filter_brightness("brightness(2)") - 2.0).abs() < 0.001);
    }

    #[test]
    fn contrast_flattens_a_red_letter_to_gray() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let full = render_styled(&slide);
        slide.items[0].contrast = 0.0;
        let flat = render_styled(&slide);
        assert!(frame_has_red(&full));
        assert!(!frame_has_red(&flat));
        assert!(channel_ink(&flat, 1) > 80);
        assert!((filter_contrast("contrast(0)") - 0.0).abs() < 0.001);
    }

    #[test]
    fn saturate_drains_the_color_from_a_red_letter() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let full = render_styled(&slide);
        slide.items[0].saturate = 0.0;
        let gray = render_styled(&slide);
        assert!(frame_has_red(&full));
        assert!(!frame_has_red(&gray));
        assert!(channel_ink(&gray, 1) > 30);
        assert!((filter_saturate("saturate(0)") - 0.0).abs() < 0.001);
    }

    #[test]
    fn hue_rotate_turns_a_red_letter_cyan() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let full = render_styled(&slide);
        slide.items[0].hue_rotate = 180.0;
        let spun = render_styled(&slide);
        assert!(frame_has_red(&full));
        assert!(!frame_has_red(&spun));
        assert!(channel_ink(&spun, 1) > 80);
        assert!(channel_ink(&spun, 2) > 80);
        assert!((filter_hue_rotate("hue-rotate(180deg)") - 180.0).abs() < 0.001);
    }

    #[test]
    fn blur_spreads_a_red_letter() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let sharp = render_styled(&slide);
        slide.items[0].blur = 6.0;
        let soft = render_styled(&slide);
        assert!(reddish_count(&soft) > reddish_count(&sharp));
        assert!((filter_blur("blur(6px)") - 6.0).abs() < 0.001);
    }

    #[test]
    fn multiply_knocks_a_red_letter_out_on_black() {
        let mut slide = shadow_slide("font-size: 48px;color: #FF0000;");
        let normal = render_styled(&slide);
        slide.items[0].blend = Blend::Multiply;
        let multiplied = render_styled(&slide);
        assert!(frame_has_red(&normal));
        assert!(!frame_has_red(&multiplied));
        assert_eq!(blend_mode("multiply"), Blend::Multiply);
        assert_eq!(blend_mode("screen"), Blend::Screen);
    }

    #[test]
    fn letter_spacing_spreads_the_word() {
        let mut slide = shadow_slide("font-size: 32px;color: #FFFFFF;");
        slide.items[0].lines[0].runs[0].text = "Hi".into();
        let plain = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 32px;color: #FFFFFF;letter-spacing: 30px;".into();
        let spaced = render_styled(&slide);
        assert!(rightmost_ink(&spaced) >= rightmost_ink(&plain) + 20);
    }

    #[test]
    fn a_long_line_wraps_unless_nowrap() {
        let mut slide = shadow_slide("font-size: 28px;color: #FFFFFF;");
        slide.items[0].style = "left:0px;top:0px;width:70px;height:80px;".into();
        slide.items[0].lines[0].runs[0].text = "One Two".into();
        let wrapped = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 28px;color: #FFFFFF;white-space: nowrap;".into();
        let single = render_styled(&slide);
        assert!(lowest_ink(&wrapped) > lowest_ink(&single));
    }

    #[test]
    fn justify_spreads_the_first_wrapped_row() {
        let mut slide = shadow_slide("font-size: 28px;color: #FFFFFF;");
        slide.items[0].style = "left:0px;top:0px;width:130px;height:80px;".into();
        slide.items[0].lines[0].align = "left".into();
        slide.items[0].lines[0].runs[0].text = "One Two Three".into();
        let packed = render_styled(&slide);
        slide.items[0].lines[0].align = "justify".into();
        let justified = render_styled(&slide);
        assert!(
            rightmost_ink(&justified) > rightmost_ink(&packed) + 8,
            "justified {} packed {}",
            rightmost_ink(&justified),
            rightmost_ink(&packed)
        );
    }

    #[test]
    fn a_long_word_breaks_across_rows() {
        let mut slide = shadow_slide("font-size: 28px;color: #FFFFFF;");
        slide.items[0].style = "left:0px;top:0px;width:70px;height:80px;".into();
        slide.items[0].lines[0].runs[0].text = "ABCDEFGHIJKLMNOP".into();
        let broken = render_styled(&slide);
        slide.items[0].style = "left:0px;top:0px;width:70px;height:80px;white-space:nowrap;".into();
        let single = render_styled(&slide);
        assert!(lowest_ink(&broken) > lowest_ink(&single));
    }

    #[test]
    fn a_wrapped_row_keeps_the_line_background() {
        let mut slide = shadow_slide("font-size: 28px;color: #FFFFFF;");
        slide.items[0].style = "left:0px;top:0px;width:70px;height:80px;".into();
        slide.items[0].lines[0].runs[0].text = "One Two".into();
        slide.items[0].line_bg = Some(Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        });
        let frame = render_styled(&slide);
        assert_eq!(
            frame.pixel(4, 4).r,
            255,
            "the first row has the line background"
        );
        assert!(
            frame.pixel(4, 40).r > 200,
            "the wrapped row keeps the line background"
        );
    }

    #[test]
    fn word_spacing_opens_a_gap_between_words() {
        let mut slide = shadow_slide("font-size: 32px;color: #FFFFFF;");
        slide.items[0].lines[0].runs[0].text = "Hi Yo".into();
        let plain = render_styled(&slide);
        slide.items[0].lines[0].runs[0].style =
            "font-size: 32px;color: #FFFFFF;word-spacing: 40px;".into();
        let spaced = render_styled(&slide);
        assert!(rightmost_ink(&spaced) >= rightmost_ink(&plain) + 30);
    }

    #[test]
    fn text_transform_paints_the_case_on_the_slide() {
        let mut forced = shadow_slide("font-size: 40px;color: #FFFFFF;text-transform: uppercase;");
        forced.items[0].lines[0].runs[0].text = "go".into();
        let mut written = shadow_slide("font-size: 40px;color: #FFFFFF;");
        written.items[0].lines[0].runs[0].text = "GO".into();
        assert_eq!(
            fingerprint_of(&render_styled(&forced)),
            fingerprint_of(&render_styled(&written))
        );
        forced.items[0].lines[0].runs[0].style =
            "font-size: 40px;color: #FFFFFF;text-transform: lowercase;".into();
        forced.items[0].lines[0].runs[0].text = "GO".into();
        written.items[0].lines[0].runs[0].text = "go".into();
        assert_eq!(
            fingerprint_of(&render_styled(&forced)),
            fingerprint_of(&render_styled(&written))
        );
    }

    #[test]
    fn a_text_stroke_outlines_the_letters() {
        let plain = render_styled(&shadow_slide("font-size: 48px;color: #FFFFFF;"));
        let stroked = render_styled(&shadow_slide(
            "font-size: 48px;color: #FFFFFF;-webkit-text-stroke-width: 6px;-webkit-text-stroke-color: #FF0000;",
        ));
        assert!(!frame_has_red(&plain));
        assert!(frame_has_red(&stroked));
    }

    fn lowest_ink(frame: &crate::Frame) -> i32 {
        let mut lowest = 0;
        for y in 0..frame.height {
            for x in 0..frame.width {
                if frame.pixel(x, y).r > 180 {
                    lowest = y as i32;
                }
            }
        }
        lowest
    }

    fn ink_count(frame: &crate::Frame) -> usize {
        frame
            .pixels
            .chunks(4)
            .filter(|pixel| pixel[0] > 180 && pixel[3] > 200)
            .count()
    }

    fn brightest(frame: &crate::Frame) -> u8 {
        frame
            .pixels
            .chunks(4)
            .map(|pixel| pixel[0])
            .max()
            .unwrap_or(0)
    }

    fn frame_has_red(frame: &crate::Frame) -> bool {
        frame
            .pixels
            .chunks(4)
            .any(|pixel| pixel[0] > 180 && pixel[1] < 40 && pixel[2] < 40 && pixel[3] > 200)
    }

    fn channel_ink(frame: &crate::Frame, channel: usize) -> u8 {
        frame
            .pixels
            .chunks(4)
            .map(|pixel| pixel[channel])
            .max()
            .unwrap_or(0)
    }

    fn reddish_count(frame: &crate::Frame) -> usize {
        frame
            .pixels
            .chunks(4)
            .filter(|pixel| pixel[0] > 40 && pixel[0] > pixel[1] && pixel[0] > pixel[2])
            .count()
    }

    #[test]
    fn styled_slide_paints_background_text_and_is_stable() {
        let frame = render_styled(&sample());
        assert_eq!(
            frame.pixel(0, 0),
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
        assert_eq!(frame.pixel(10, 120), Rgba::BLACK);
        assert_ne!(
            fingerprint_of(&frame),
            fingerprint_of(&clear_frame(960, 540))
        );
        assert_eq!(
            fingerprint_of(&frame),
            fingerprint_of(&render_styled(&sample()))
        );
    }

    #[test]
    fn image_layer_covers_its_box() {
        let mut slide = sample();
        slide.items[0].lines.clear();
        slide.items[0].image = Some(Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 255, 0, 255],
        });
        let frame = render_styled(&slide);
        assert_eq!(frame.pixel(20, 20).g, 255);
        assert_eq!(frame.pixel(20, 20).r, 0);
    }

    #[test]
    fn a_flipped_picture_mirrors_inside_its_box() {
        let slide = picture_slide(false, false);
        let frame = render_styled(&slide);
        assert_eq!(frame.pixel(0, 0).r, 255, "the left pixel starts red");
        assert_eq!(frame.pixel(1, 0).b, 255, "the right pixel starts blue");
        let flipped = render_styled(&picture_slide(true, false));
        assert_eq!(flipped.pixel(0, 0).b, 255, "flipped swaps left and right");
        assert_eq!(flipped.pixel(1, 0).r, 255);
        let vertical = render_styled(&picture_slide(false, true));
        assert_eq!(
            vertical.pixel(1, 0).g,
            255,
            "flippedY brings the bottom row to the top"
        );
        assert_eq!(
            vertical.pixel(0, 1).r,
            255,
            "the top row moves to the bottom"
        );
    }

    #[test]
    fn a_left_crop_hides_the_left_of_a_picture() {
        let mut slide = picture_slide(false, false);
        slide.items[0].crop.left = 50.0;
        let frame = render_styled(&slide);
        assert_eq!(frame.pixel(0, 0), Rgba::BLACK);
        assert_eq!(frame.pixel(1, 0).b, 255);
    }

    fn picture_slide(flip_x: bool, flip_y: bool) -> StyledSlide {
        StyledSlide {
            output_width: 2,
            output_height: 2,
            source_width: 2.0,
            source_height: 2.0,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "left:0px;top:0px;width:2px;height:2px;".into(),
                align: String::new(),
                lines: Vec::new(),
                image: Some(Raster {
                    width: 2,
                    height: 2,
                    rgba: vec![255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 0, 255, 0, 255, 0, 255],
                }),
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x,
                flip_y,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: Crop::default(),
            }],
        }
    }

    #[test]
    fn a_left_slide_brings_the_next_slide_in_from_the_left() {
        let from = crate::Frame {
            width: 4,
            height: 2,
            pixels: [0, 0, 255, 255].repeat(8),
        };
        let to = crate::Frame {
            width: 4,
            height: 2,
            pixels: [255, 0, 0, 255].repeat(8),
        };
        assert_eq!(
            slide_frames(&from, &to, 0.0, SlideWay::LeftRight)
                .pixel(0, 0)
                .b,
            255
        );
        let mid = slide_frames(&from, &to, 0.5, SlideWay::LeftRight);
        assert_eq!(
            mid.pixel(0, 0).r,
            255,
            "the incoming slide covers the leading edge"
        );
        assert_eq!(
            mid.pixel(3, 0),
            Rgba::BLACK,
            "the trailing edge stays clear"
        );
        assert_eq!(
            slide_frames(&from, &to, 1.0, SlideWay::LeftRight)
                .pixel(3, 0)
                .r,
            255
        );
        assert_eq!(slide_way("right_left"), SlideWay::RightLeft);
    }

    #[test]
    fn scale_grows_the_next_slide_from_the_center() {
        let from = crate::Frame {
            width: 4,
            height: 4,
            pixels: [0, 0, 255, 255].repeat(16),
        };
        let to = crate::Frame {
            width: 4,
            height: 4,
            pixels: [255, 0, 0, 255].repeat(16),
        };
        assert_eq!(scale_frames(&from, &to, 0.0).pixel(0, 0).b, 255);
        let mid = scale_frames(&from, &to, 0.5);
        assert_eq!(
            mid.pixel(0, 0),
            Rgba::BLACK,
            "the corner stays outside the scaled slides"
        );
        assert!(
            mid.pixel(1, 1).r > 40,
            "the next slide is growing in the center"
        );
        assert_eq!(scale_frames(&from, &to, 1.0).pixel(3, 3).r, 255);
        let eased = ease_progress("sine", 0.25);
        assert!(eased < 0.2, "sine eases in slower than linear, got {eased}");
        assert_eq!(ease_progress("linear", 0.25), 0.25);
        assert!(
            ease_progress("back", 0.25) < 0.0,
            "back overshoots behind the start"
        );
        assert!(ease_progress("cubic", 0.25) < 0.1);
        assert!(ease_progress("circ", 0.25) < 0.1);
        assert!(
            ease_progress("elastic", 0.25) < 0.0,
            "elastic swings behind the start"
        );
        assert!(
            ease_progress("bounce", 0.9) < ease_progress("bounce", 0.85),
            "bounce steps backward before it settles"
        );
        assert_eq!(ease_progress("nope", 0.25), 0.25);
        for name in [
            "sine", "back", "circ", "cubic", "elastic", "bounce", "linear",
        ] {
            assert!(
                ease_progress(name, 0.0).abs() < 0.001,
                "{name} starts at rest"
            );
            assert!(
                (ease_progress(name, 1.0) - 1.0).abs() < 0.001,
                "{name} finishes at the next slide"
            );
        }
    }

    #[test]
    fn blur_spreads_the_outgoing_slide() {
        let mut pixels = vec![0u8; 8 * 2 * 4];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        for x in 0..4 {
            pixels[(x * 4) as usize] = 255;
        }
        let from = crate::Frame {
            width: 8,
            height: 2,
            pixels,
        };
        let to = crate::Frame {
            width: 8,
            height: 2,
            pixels: [0, 0, 255, 255].repeat(16),
        };
        let faded = crossfade(&from, &to, 0.5);
        let blurred = blur_frames(&from, &to, 0.5);
        assert!(
            blurred.pixel(6, 0).r > faded.pixel(6, 0).r,
            "blur {} fade {}",
            blurred.pixel(6, 0).r,
            faded.pixel(6, 0).r
        );
        assert_eq!(blur_frames(&from, &to, 0.0).pixel(0, 0).r, 255);
        assert_eq!(blur_frames(&from, &to, 1.0).pixel(0, 0).b, 255);
    }

    #[test]
    fn spin_turns_the_outgoing_slide_halfway() {
        let mut pixels = vec![0u8; 4 * 4 * 4];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        for y in 0..4 {
            for x in 0..2 {
                pixels[((y * 4 + x) * 4) as usize] = 255;
            }
        }
        let from = crate::Frame {
            width: 4,
            height: 4,
            pixels,
        };
        let to = crate::Frame {
            width: 4,
            height: 4,
            pixels: [0, 0, 255, 255].repeat(16),
        };
        let faded = crossfade(&from, &to, 0.5);
        let spun = spin_frames(&from, &to, 0.5);
        assert!(
            spun.pixel(3, 1).r > faded.pixel(3, 1).r,
            "spin {} fade {}",
            spun.pixel(3, 1).r,
            faded.pixel(3, 1).r
        );
        assert_eq!(spin_frames(&from, &to, 0.0).pixel(0, 1).r, 255);
        assert_eq!(spin_frames(&from, &to, 1.0).pixel(3, 1).b, 255);
    }

    #[test]
    fn crossfade_blends_and_clear_is_black() {
        let black = clear_frame(4, 2);
        let mut white = black.clone();
        white.pixels.fill(255);
        assert_eq!(crossfade(&black, &white, 0.0).pixel(0, 0), Rgba::BLACK);
        assert_eq!(
            crossfade(&black, &white, 1.0).pixel(0, 0),
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255
            }
        );
        let mid = crossfade(&black, &white, 0.5).pixel(0, 0);
        assert!((mid.r as i16 - 128).abs() <= 1);
        assert_eq!(clear_frame(4, 2).pixel(1, 1), Rgba::BLACK);
    }

    #[test]
    fn underlay_shows_through_a_transparent_slide() {
        let image = Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 255, 255],
        };
        let slide = render_styled(&StyledSlide {
            output_width: 8,
            output_height: 4,
            source_width: SOURCE_WIDTH,
            source_height: SOURCE_HEIGHT,
            background: Rgba::TRANSPARENT,
            items: Vec::new(),
        });
        let frame = with_underlay(slide, Some(&image), 8, 4);
        assert_eq!(frame.pixel(3, 1).b, 255);
        assert_eq!(frame.pixel(3, 1).r, 0);
        assert_eq!(frame.pixel(0, 1), Rgba::BLACK);
        let covered = image_frame(&image, 8, 4);
        assert_eq!(covered.pixel(3, 1).b, 255);
        assert_eq!(covered.pixel(0, 0), Rgba::BLACK);
    }

    #[test]
    fn lyric_text_is_proportional_antialiased_and_centered() {
        let slide = StyledSlide {
            output_width: 400,
            output_height: 160,
            source_width: 400.0,
            source_height: 160.0,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "left:0px;top:0px;width:400px;height:160px;font-size:72px;color:#FFFFFF;text-align:center;".into(),
                align: String::new(),
                lines: vec![StyledLine {
                    align: String::new(),
            line_bar: true,
            wrapped: false,
            shift_y: 0,
                    runs: vec![TextRun {
                        text: "Hi".into(),
                        style: String::new(),
                        place_on: None,
                        paint: true,
                    }],
                }],
                image: None,
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
            flip_x: false,
            flip_y: false,
            opacity: 1.0,
            grayscale: 0.0,
            invert: 0.0,
            sepia: 0.0,
            brightness: 1.0,
            contrast: 1.0,
            saturate: 1.0,
            hue_rotate: 0.0,
            blur: 0.0,
            blend: Blend::Normal,
            crop: Crop::default(),
            }],
        };
        let frame = render_styled(&slide);
        let mut min_x = u32::MAX;
        let mut max_x = 0;
        let mut gray = false;
        for y in 0..frame.height {
            for x in 0..frame.width {
                let pixel = frame.pixel(x, y);
                if pixel.r == 0 {
                    continue;
                }
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                if pixel.r < 255 {
                    gray = true;
                }
            }
        }
        assert!(gray, "glyph edges should blend");
        assert!(min_x > 40, "centered text starts away from the left edge");
        assert!(max_x + 40 < frame.width);
        assert!(
            max_x > min_x + 20,
            "a proportional word is wider than one bitmap cell"
        );
    }

    #[test]
    fn arial_is_used_when_the_system_has_it() {
        let bundled = render_styled(&centered_word());
        let mut arial = centered_word();
        arial.items[0].style.push_str("font-family: Arial;");
        let named = render_styled(&arial);
        let has_arial = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Arial.ttf",
        ]
        .iter()
        .any(|path| std::path::Path::new(path).is_file());
        if has_arial {
            assert_ne!(fingerprint_of(&bundled), fingerprint_of(&named));
        } else {
            assert_eq!(fingerprint_of(&bundled), fingerprint_of(&named));
        }
        let mut missing = centered_word();
        missing.items[0]
            .style
            .push_str("font-family: NotARealFont;");
        assert_eq!(
            fingerprint_of(&bundled),
            fingerprint_of(&render_styled(&missing))
        );
    }

    fn centered_word() -> StyledSlide {
        StyledSlide {
            output_width: 400,
            output_height: 160,
            source_width: 400.0,
            source_height: 160.0,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "left:0px;top:0px;width:400px;height:160px;font-size:72px;color:#FFFFFF;text-align:center;".into(),
                align: String::new(),
                lines: vec![StyledLine {
                    align: String::new(),
            line_bar: true,
            wrapped: false,
            shift_y: 0,
                    runs: vec![TextRun {
                        text: "Hi".into(),
                        style: String::new(),
                        place_on: None,
                        paint: true,
                    }],
                }],
                image: None,
                fit: Fit::Fill,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
            flip_x: false,
            flip_y: false,
            opacity: 1.0,
            grayscale: 0.0,
            invert: 0.0,
            sepia: 0.0,
            brightness: 1.0,
            contrast: 1.0,
            saturate: 1.0,
            hue_rotate: 0.0,
            blur: 0.0,
            blend: Blend::Normal,
            crop: Crop::default(),
            }],
        }
    }

    #[test]
    fn contain_letterboxes_and_cover_fills() {
        let image = Raster {
            width: 4,
            height: 1,
            rgba: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
        };
        let mut slide = StyledSlide {
            output_width: 8,
            output_height: 8,
            source_width: 8.0,
            source_height: 8.0,
            background: Rgba::BLACK,
            items: vec![StyledItem {
                style: "left:0px;top:0px;width:8px;height:8px;".into(),
                align: String::new(),
                lines: Vec::new(),
                image: Some(image),
                fit: Fit::Contain,
                line_bg: None,
                line_gap: 0.0,
                line_radius: 0.0,
                flip_x: false,
                flip_y: false,
                opacity: 1.0,
                grayscale: 0.0,
                invert: 0.0,
                sepia: 0.0,
                brightness: 1.0,
                contrast: 1.0,
                saturate: 1.0,
                hue_rotate: 0.0,
                blur: 0.0,
                blend: Blend::Normal,
                crop: Crop::default(),
            }],
        };
        let contained = render_styled(&slide);
        assert_eq!(contained.pixel(0, 0), Rgba::BLACK);
        assert_eq!(contained.pixel(0, 3).r, 255);
        assert_eq!(contained.pixel(6, 3).b, 255);
        slide.items[0].fit = Fit::Cover;
        let covered = render_styled(&slide);
        assert_ne!(covered.pixel(0, 0), Rgba::BLACK);
        assert_eq!(covered.pixel(0, 0).a, 255);
    }

    #[test]
    fn fit_raster_keeps_aspect_and_shrinks() {
        let mut rgba = vec![0u8; 8 * 4 * 4];
        rgba[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let fitted = fit_raster(
            &Raster {
                width: 8,
                height: 4,
                rgba,
            },
            4,
            4,
        );
        assert_eq!((fitted.width, fitted.height), (4, 2));
        assert_eq!(&fitted.rgba[0..4], &[255, 0, 0, 255]);
    }

    fn fingerprint_of(frame: &crate::Frame) -> u64 {
        crate::fingerprint(&frame.pixels)
    }
}
