// SPDX-License-Identifier: GPL-3.0-only

//! Stage and slide chart boxes, plus a static preview card for web boxes.

use crate::styled::Raster;
use crate::text::{self, Clip, Typeface};
use crate::{Rgba, paint, parse_color};

const FALLBACK: [Rgba; 8] = [
    Rgba {
        r: 0x3b,
        g: 0x82,
        b: 0xf6,
        a: 255,
    },
    Rgba {
        r: 0x10,
        g: 0xb9,
        b: 0x81,
        a: 255,
    },
    Rgba {
        r: 0xf5,
        g: 0x9e,
        b: 0x0b,
        a: 255,
    },
    Rgba {
        r: 0xec,
        g: 0x48,
        b: 0x99,
        a: 255,
    },
    Rgba {
        r: 0x8b,
        g: 0x5c,
        b: 0xf6,
        a: 255,
    },
    Rgba {
        r: 0xf4,
        g: 0x3f,
        b: 0x5e,
        a: 255,
    },
    Rgba {
        r: 0x06,
        g: 0xb6,
        b: 0xd4,
        a: 255,
    },
    Rgba {
        r: 0x14,
        g: 0xb8,
        b: 0xa6,
        a: 255,
    },
];

/// One row from FreeShow `chart.data` (a JSON grid or `Label: value` lines).
#[derive(Debug, Clone, PartialEq)]
pub struct ChartPoint {
    pub label: String,
    pub value: f64,
    pub color: String,
}

/// Paints a bar, line, or pie/doughnut chart. Empty data returns `None`.
pub fn chart_image(
    kind: &str,
    points: &[ChartPoint],
    hole_size: u8,
    width: u32,
    height: u32,
) -> Option<Raster> {
    let width = width.max(80);
    let height = height.max(80);
    if points.is_empty() {
        return None;
    }
    let mut raster = blank(width, height);
    match kind.to_ascii_lowercase().as_str() {
        "line" => paint_line(&mut raster, points),
        "pie" | "doughnut" | "donut" => paint_pie(&mut raster, points, hole_size),
        _ => paint_bar(&mut raster, points),
    }
    Some(raster)
}

/// A static page card for a stage web box. This is not a live browser.
pub fn web_preview(url: &str, width: u32, height: u32) -> Raster {
    let width = width.max(160);
    let height = height.max(90);
    let mut raster = solid(
        width,
        height,
        Rgba {
            r: 0x1a,
            g: 0x1a,
            b: 0x24,
            a: 255,
        },
    );
    let bar = (height / 8).clamp(22, 48);
    fill_rect(
        &mut raster,
        0,
        0,
        width,
        bar,
        Rgba {
            r: 0x2a,
            g: 0x2a,
            b: 0x38,
            a: 255,
        },
    );
    let radius = (bar as i32 / 5).max(3);
    let cy = bar as i32 / 2;
    let colors = [
        Rgba {
            r: 0xff,
            g: 0x5f,
            b: 0x57,
            a: 255,
        },
        Rgba {
            r: 0xfe,
            g: 0xbc,
            b: 0x2e,
            a: 255,
        },
        Rgba {
            r: 0x28,
            g: 0xc8,
            b: 0x40,
            a: 255,
        },
    ];
    for (index, color) in colors.iter().enumerate() {
        let cx = 12 + index as i32 * (radius * 2 + 8);
        fill_circle(&mut raster, cx, cy, radius, *color);
    }
    let shown = display_url(url);
    let font = (bar as f32 * 0.45).clamp(12.0, 22.0);
    draw_text(
        &mut raster,
        &shown,
        12 + 3 * (radius * 2 + 8),
        cy + (font * 0.35) as i32,
        font,
        Rgba::WHITE,
    );
    let host_size = (height as f32 * 0.12).clamp(18.0, 42.0);
    draw_text(
        &mut raster,
        if shown.is_empty() { "Web" } else { &shown },
        24,
        (bar + height / 2) as i32,
        host_size,
        Rgba::WHITE,
    );
    raster
}

pub fn display_url(url: &str) -> String {
    let mut text = url.trim();
    if let Some(rest) = text.split_once("://").map(|(_, rest)| rest) {
        text = rest;
    }
    if let Some(rest) = text.strip_prefix("www.") {
        text = rest;
    }
    text.trim_end_matches('/').to_string()
}

fn paint_bar(raster: &mut Raster, points: &[ChartPoint]) {
    let width = raster.width as f32;
    let height = raster.height as f32;
    let left = width * 0.08;
    let right = width * 0.94;
    let top = height * 0.1;
    let bottom = height * 0.82;
    let usable_w = (right - left).max(1.0);
    let usable_h = (bottom - top).max(1.0);
    let max_value = points
        .iter()
        .map(|point| point.value)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let count = points.len() as f32;
    let slot = usable_w / count;
    let bar_w = (slot * 0.62).clamp(4.0, 80.0);
    draw_axis(raster, left as i32, top as i32, right as i32, bottom as i32);
    for (index, point) in points.iter().enumerate() {
        let color = sample_color(point, index);
        let x = left + slot * (index as f32 + 0.5) - bar_w / 2.0;
        let h = usable_h * (point.value / max_value) as f32;
        let y = bottom - h;
        fill_rect(
            raster,
            x.round() as u32,
            y.round() as u32,
            bar_w.round().max(1.0) as u32,
            h.round().max(1.0) as u32,
            color,
        );
        if !point.label.is_empty() {
            draw_text(
                raster,
                &point.label,
                x.round() as i32,
                (bottom + height * 0.08) as i32,
                (height * 0.045).clamp(10.0, 18.0),
                Rgba::WHITE,
            );
        }
    }
}

fn paint_line(raster: &mut Raster, points: &[ChartPoint]) {
    let width = raster.width as f32;
    let height = raster.height as f32;
    let left = width * 0.08;
    let right = width * 0.94;
    let top = height * 0.1;
    let bottom = height * 0.82;
    let usable_w = (right - left).max(1.0);
    let usable_h = (bottom - top).max(1.0);
    let max_value = points
        .iter()
        .map(|point| point.value)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let count = points.len().max(1);
    draw_axis(raster, left as i32, top as i32, right as i32, bottom as i32);
    let coords: Vec<(i32, i32)> = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let x = left + usable_w * (index as f32 + 0.5) / count as f32;
            let y = bottom - usable_h * (point.value / max_value) as f32;
            (x.round() as i32, y.round() as i32)
        })
        .collect();
    let color = sample_color(&points[0], 0);
    for pair in coords.windows(2) {
        stroke_line(raster, pair[0].0, pair[0].1, pair[1].0, pair[1].1, 3, color);
    }
    for &(x, y) in &coords {
        fill_circle(raster, x, y, 5, color);
    }
}

fn paint_pie(raster: &mut Raster, points: &[ChartPoint], hole_size: u8) {
    let width = raster.width as f32;
    let height = raster.height as f32;
    let legend = if height > 80.0 { height * 0.16 } else { 0.0 };
    let cx = width / 2.0;
    let cy = (height - legend) / 2.0;
    let radius = (width.min(height - legend) * 0.42).max(8.0);
    let hole = radius * (f32::from(hole_size.min(90)) / 100.0);
    let total: f64 = points.iter().map(|point| point.value.max(0.0)).sum();
    let total = if total == 0.0 {
        points.len().max(1) as f64
    } else {
        total
    };
    let mut start = -90.0_f32;
    let mut spans = Vec::new();
    for (index, point) in points.iter().enumerate() {
        let value = if points.iter().all(|row| row.value == 0.0) {
            1.0
        } else {
            point.value.max(0.0)
        };
        let degrees = (value / total) as f32 * 360.0;
        spans.push((start, start + degrees, sample_color(point, index)));
        start += degrees;
    }
    for y in 0..raster.height {
        for x in 0..raster.width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > radius || dist < hole {
                continue;
            }
            let mut angle = dy.atan2(dx).to_degrees();
            if angle < -90.0 {
                angle += 360.0;
            }
            if let Some((_, _, color)) = spans
                .iter()
                .find(|(from, to, _)| angle >= *from && angle < *to)
            {
                put(raster, x as i32, y as i32, *color);
            }
        }
    }
}

fn draw_axis(raster: &mut Raster, left: i32, top: i32, right: i32, bottom: i32) {
    let axis = Rgba {
        r: 0x80,
        g: 0x80,
        b: 0x90,
        a: 255,
    };
    stroke_line(raster, left, bottom, right, bottom, 2, axis);
    stroke_line(raster, left, top, left, bottom, 2, axis);
}

fn sample_color(point: &ChartPoint, index: usize) -> Rgba {
    if !point.color.is_empty()
        && let Some(color) = parse_color(&point.color)
    {
        return color;
    }
    FALLBACK[index % FALLBACK.len()]
}

fn blank(width: u32, height: u32) -> Raster {
    Raster {
        width,
        height,
        rgba: vec![0; width as usize * height as usize * 4],
    }
}

fn solid(width: u32, height: u32, color: Rgba) -> Raster {
    let mut raster = blank(width, height);
    fill_rect(&mut raster, 0, 0, width, height, color);
    raster
}

fn fill_rect(raster: &mut Raster, x: u32, y: u32, w: u32, h: u32, color: Rgba) {
    let x1 = (x + w).min(raster.width);
    let y1 = (y + h).min(raster.height);
    for py in y..y1 {
        for px in x..x1 {
            put(raster, px as i32, py as i32, color);
        }
    }
}

fn fill_circle(raster: &mut Raster, cx: i32, cy: i32, radius: i32, color: Rgba) {
    let r2 = radius * radius;
    for y in (cy - radius)..=(cy + radius) {
        for x in (cx - radius)..=(cx + radius) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                put(raster, x, y, color);
            }
        }
    }
}

fn stroke_line(raster: &mut Raster, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, color: Rgba) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        for oy in -width / 2..=width / 2 {
            for ox in -width / 2..=width / 2 {
                put(raster, x + ox, y + oy, color);
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let doubled = 2 * err;
        if doubled >= dy {
            err += dy;
            x += sx;
        }
        if doubled <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn put(raster: &mut Raster, x: i32, y: i32, color: Rgba) {
    paint(&mut raster.rgba, raster.width, raster.height, x, y, color);
}

fn draw_text(raster: &mut Raster, text: &str, x: i32, baseline: i32, font_px: f32, color: Rgba) {
    text::draw(
        &mut raster.rgba,
        raster.width,
        raster.height,
        text,
        color,
        font_px,
        x as f32,
        baseline as f32,
        &Clip {
            x: 0,
            y: 0,
            w: raster.width as i32,
            h: raster.height as i32,
        },
        Typeface {
            family: "",
            bold: false,
            italic: false,
        },
        0.0,
        0.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(label: &str, value: f64, color: &str) -> ChartPoint {
        ChartPoint {
            label: label.into(),
            value,
            color: color.into(),
        }
    }

    fn has_color(raster: &Raster, color: Rgba) -> bool {
        raster.rgba.chunks_exact(4).any(|px| {
            px[0].abs_diff(color.r) < 8
                && px[1].abs_diff(color.g) < 8
                && px[2].abs_diff(color.b) < 8
                && px[3] > 200
        })
    }

    #[test]
    fn bar_and_pie_charts_use_the_stored_colors() {
        let points = [
            sample("A", 10.0, "#ff0000"),
            sample("B", 20.0, "#00ff00"),
            sample("C", 30.0, "#0000ff"),
        ];
        let bar = chart_image("bar", &points, 0, 400, 240).unwrap();
        let pie = chart_image("pie", &points, 0, 400, 240).unwrap();
        for color in [
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 255,
            },
            Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 255,
            },
        ] {
            assert!(has_color(&bar, color), "bar missing {color:?}");
            assert!(has_color(&pie, color), "pie missing {color:?}");
        }
        let doughnut = chart_image("pie", &points, 50, 400, 240).unwrap();
        let cx = 200;
        let cy = ((240.0_f32 - 240.0 * 0.16) / 2.0).round() as usize;
        let i = (cy * 400 + cx) * 4;
        assert_eq!(&doughnut.rgba[i..i + 4], &[0, 0, 0, 0]);
        assert!(chart_image("bar", &[], 0, 100, 100).is_none());
    }

    #[test]
    fn a_line_chart_paints_the_series_color() {
        let points = [sample("A", 1.0, "#3b82f6"), sample("B", 4.0, "#3b82f6")];
        let line = chart_image("line", &points, 0, 400, 240).unwrap();
        assert!(has_color(
            &line,
            Rgba {
                r: 0x3b,
                g: 0x82,
                b: 0xf6,
                a: 255
            }
        ));
    }

    #[test]
    fn web_preview_shows_the_page_host() {
        let card = web_preview("https://www.example.com/song", 400, 240);
        assert_eq!(card.width, 400);
        assert!(card.rgba.iter().any(|byte| *byte > 0));
        assert_eq!(
            display_url("https://www.example.com/song/"),
            "example.com/song"
        );
        assert_eq!(display_url("example.com"), "example.com");
    }
}
