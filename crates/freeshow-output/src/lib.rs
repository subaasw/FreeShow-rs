// SPDX-License-Identifier: GPL-3.0-only

//! Renders slide frames to RGBA pixels. Audience and stage windows upload these
//! frames through wgpu. This crate stays headless so a known slide can be checked
//! without a window.

mod charts;
mod styled;
mod text;

pub use charts::{ChartPoint, chart_image, display_url, web_preview};
pub use styled::qr_image;
pub use styled::{
    Align, Blend, Crop, Fit, Raster, SOURCE_HEIGHT, SOURCE_WIDTH, SlideWay, StyledItem, StyledLine,
    StyledSlide, TextRun, blend_mode, blur_frames, clear_frame, crop_raster, crossfade,
    decode_image, ease_progress, filter_blur, filter_brightness, filter_contrast, filter_grayscale,
    filter_hue_rotate, filter_invert, filter_opacity, filter_saturate, filter_sepia, fit_raster,
    image_frame, over, parse_color, parse_style, placed_image, render_styled, scale_frames,
    slide_frames, slide_way, spin_frames, with_underlay,
};

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideLine {
    pub text: String,
    pub color: Rgba,
    pub scale: u32,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideContent {
    pub width: u32,
    pub height: u32,
    pub background: Rgba,
    pub lines: Vec<SlideLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn encode_png(&self) -> image::ImageResult<Vec<u8>> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(bytes)
    }

    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        let index = ((y * self.width + x) * 4) as usize;
        Rgba {
            r: self.pixels[index],
            g: self.pixels[index + 1],
            b: self.pixels[index + 2],
            a: self.pixels[index + 3],
        }
    }
}

/// 960 * 4 is a multiple of wgpu's 256-byte row alignment.
pub const OUTPUT_WIDTH: u32 = 960;
pub const OUTPUT_HEIGHT: u32 = 540;

pub fn render(slide: &SlideContent) -> Frame {
    let width = slide.width;
    let height = slide.height;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        pixels.extend_from_slice(&[
            slide.background.r,
            slide.background.g,
            slide.background.b,
            slide.background.a,
        ]);
    }
    for line in &slide.lines {
        draw_line(&mut pixels, width, height, line);
    }
    Frame {
        width,
        height,
        pixels,
    }
}

pub fn fingerprint(pixels: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in pixels {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Centers lyric lines on a black frame. The same pixels feed the output window and the preview.
pub fn slide_frame(lines: &[String], width: u32, height: u32) -> Frame {
    let scale = if lines.len() > 8 { 2 } else { 3 };
    let line_h = 8 * scale as i32 + scale as i32;
    let block_h = lines.len() as i32 * line_h;
    let mut y = ((height as i32 - block_h) / 2).max(scale as i32);
    let mut drawn = Vec::new();
    for line in lines {
        let text_w = line.chars().count() as i32 * 8 * scale as i32;
        drawn.push(SlideLine {
            text: line.clone(),
            color: Rgba::WHITE,
            scale,
            x: (width as i32 - text_w) / 2,
            y,
        });
        y += line_h;
    }
    render(&SlideContent {
        width,
        height,
        background: Rgba::BLACK,
        lines: drawn,
    })
}

pub fn title_card(label: &str, width: u32, height: u32) -> Frame {
    let scale = 4u32;
    let text_w = label.chars().count() as i32 * 8 * scale as i32;
    let text_h = 8 * scale as i32;
    render(&SlideContent {
        width,
        height,
        background: Rgba::BLACK,
        lines: vec![SlideLine {
            text: label.to_string(),
            color: Rgba::WHITE,
            scale,
            x: (width as i32 - text_w) / 2,
            y: (height as i32 - text_h) / 2,
        }],
    })
}

fn glyph(ch: char) -> [u8; 8] {
    BASIC_FONTS
        .get(ch)
        .or_else(|| LATIN_FONTS.get(ch))
        .unwrap_or([0; 8])
}

pub(crate) fn draw_line(pixels: &mut [u8], width: u32, height: u32, line: &SlideLine) {
    let scale = line.scale.max(1) as i32;
    let mut cursor_x = line.x;
    for ch in line.text.chars() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) == 0 {
                    continue;
                }
                let origin_x = cursor_x + col * scale;
                let origin_y = line.y + row as i32 * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        paint(
                            pixels,
                            width,
                            height,
                            origin_x + dx,
                            origin_y + dy,
                            line.color,
                        );
                    }
                }
            }
        }
        cursor_x += 8 * scale;
    }
}

pub(crate) fn paint(pixels: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: Rgba) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as u32;
    let y = y as u32;
    if x >= width || y >= height {
        return;
    }
    let index = ((y * width + x) * 4) as usize;
    if color.a == 255 {
        pixels[index] = color.r;
        pixels[index + 1] = color.g;
        pixels[index + 2] = color.b;
        pixels[index + 3] = 255;
        return;
    }
    if color.a == 0 {
        return;
    }
    let src_a = u32::from(color.a);
    let dst_a = 255 - src_a;
    for (offset, channel) in [color.r, color.g, color.b].into_iter().enumerate() {
        let dst = u32::from(pixels[index + offset]);
        pixels[index + offset] = ((u32::from(channel) * src_a + dst * dst_a) / 255) as u8;
    }
    pixels[index + 3] = 255;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_slide() -> SlideContent {
        SlideContent {
            width: 32,
            height: 16,
            background: Rgba::BLACK,
            lines: vec![SlideLine {
                text: "A".to_string(),
                color: Rgba::WHITE,
                scale: 1,
                x: 0,
                y: 0,
            }],
        }
    }

    #[test]
    fn frame_covers_the_requested_size_with_background() {
        let frame = render(&sample_slide());
        assert_eq!(frame.width, 32);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.pixels.len(), 32 * 16 * 4);
        assert_eq!(frame.pixel(0, 0), Rgba::BLACK);
    }

    #[test]
    fn glyph_a_paints_its_top_pixels() {
        // font8x8 'A' row 0 is 0x0C. Bit 0 is the leftmost pixel, so columns 2 and 3 are ink.
        let frame = render(&sample_slide());
        assert_eq!(frame.pixel(2, 0), Rgba::WHITE);
        assert_eq!(frame.pixel(3, 0), Rgba::WHITE);
        assert_eq!(frame.pixel(1, 0), Rgba::BLACK);
    }

    #[test]
    fn known_slide_is_stable_and_sensitive_to_text() {
        let first = render(&sample_slide());
        let second = render(&sample_slide());
        assert_eq!(fingerprint(&first.pixels), fingerprint(&second.pixels));
        assert_ne!(fingerprint(&first.pixels), 0);

        let mut changed = sample_slide();
        changed.lines[0].text = "B".to_string();
        let other = render(&changed);
        assert_ne!(fingerprint(&first.pixels), fingerprint(&other.pixels));
    }

    #[test]
    fn output_and_stage_cards_differ_and_fill_the_output_size() {
        let output = title_card("OUTPUT", OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let stage = title_card("STAGE", OUTPUT_WIDTH, OUTPUT_HEIGHT);
        assert_eq!(output.width, OUTPUT_WIDTH);
        assert_eq!(output.height, OUTPUT_HEIGHT);
        assert_eq!(
            output.pixels.len(),
            (OUTPUT_WIDTH * OUTPUT_HEIGHT * 4) as usize
        );
        assert_eq!(output.pixel(0, 0), Rgba::BLACK);
        assert_ne!(fingerprint(&output.pixels), fingerprint(&stage.pixels));
        assert_eq!(
            fingerprint(&output.pixels),
            fingerprint(&title_card("OUTPUT", OUTPUT_WIDTH, OUTPUT_HEIGHT).pixels)
        );
    }

    #[test]
    fn lyric_slide_is_stable_and_not_blank() {
        let frame = slide_frame(
            &[
                "Amazing grace".to_string(),
                "how sweet the sound".to_string(),
            ],
            OUTPUT_WIDTH,
            OUTPUT_HEIGHT,
        );
        assert_eq!(frame.width, OUTPUT_WIDTH);
        assert_eq!(frame.pixel(0, 0), Rgba::BLACK);
        assert_ne!(
            fingerprint(&frame.pixels),
            fingerprint(&slide_frame(&[], OUTPUT_WIDTH, OUTPUT_HEIGHT).pixels)
        );
        assert_eq!(
            fingerprint(&frame.pixels),
            fingerprint(
                &slide_frame(
                    &[
                        "Amazing grace".to_string(),
                        "how sweet the sound".to_string()
                    ],
                    OUTPUT_WIDTH,
                    OUTPUT_HEIGHT
                )
                .pixels
            )
        );
    }
}
