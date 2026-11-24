// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, sync::Arc};

use roxmltree::{Document, Node};

use super::{Archive, SlideSize, invalid, read_xml, relationships, text::TextBody};
use crate::DataError;

#[derive(Clone)]
pub(super) struct Color {
    value: ColorValue,
    transforms: Vec<Transform>,
}

type Rgb = [f64; 3];

#[derive(Clone, Copy)]
struct ResolvedColor {
    rgb: Rgb,
    alpha: f64,
}

#[derive(Clone)]
enum ColorValue {
    Rgb(Rgb),
    Scheme(String),
}

#[derive(Clone, Copy)]
enum Transform {
    Tint(f64),
    Shade(f64),
    Lightness {
        scale: f64,
        offset: f64,
    },
    Saturation {
        scale: f64,
        offset: f64,
    },
    Hue {
        scale: f64,
        offset: f64,
    },
    Channel {
        index: usize,
        scale: f64,
        offset: f64,
    },
    Alpha {
        scale: f64,
        offset: f64,
    },
    Gamma,
    InverseGamma,
    Invert,
    Gray,
    Complement,
}

impl Color {
    pub(super) fn parse(kind: &str, attr: impl FnOnce(&str) -> Option<String>) -> Option<Self> {
        let value = attr(if kind == "sysClr" { "lastClr" } else { "val" })?;
        let value = match kind {
            "schemeClr" => ColorValue::Scheme(value),
            "sysClr" | "srgbClr"
                if value.len() == 6 && value.bytes().all(|ch| ch.is_ascii_hexdigit()) =>
            {
                let raw = u32::from_str_radix(&value, 16).ok()?;
                ColorValue::Rgb([16, 8, 0].map(|bit| f64::from((raw >> bit) & 255) / 255.0))
            }
            _ => return None,
        };
        Some(Self {
            value,
            transforms: Vec::new(),
        })
    }

    fn in_node(node: Node<'_, '_>) -> Option<Self> {
        let color = node.children().find(Node::is_element)?;
        let mut parsed = Self::parse(color.tag_name().name(), |key| {
            color.attribute(key).map(str::to_owned)
        })?;
        for transform in color.children().filter(Node::is_element) {
            parsed.add_transform(transform.tag_name().name(), transform.attribute("val"));
        }
        Some(parsed)
    }

    pub(super) fn add_transform(&mut self, kind: &str, value: Option<&str>) {
        if let Some(transform) = parse_transform(kind, value) {
            self.transforms.push(transform);
        }
    }

    pub(super) fn literal(&self) -> Option<String> {
        self.hex_in(|_| None)
    }

    fn hex_in(&self, scheme: impl Fn(&str) -> Option<ResolvedColor>) -> Option<String> {
        self.resolve(scheme).map(hex)
    }

    fn resolve(&self, scheme: impl Fn(&str) -> Option<ResolvedColor>) -> Option<ResolvedColor> {
        let mut color = match &self.value {
            ColorValue::Rgb(rgb) => ResolvedColor {
                rgb: *rgb,
                alpha: 1.0,
            },
            ColorValue::Scheme(slot) => scheme(slot)?,
        };
        for transform in &self.transforms {
            match *transform {
                Transform::Alpha { scale, offset } => {
                    color.alpha = (color.alpha * scale + offset).clamp(0.0, 1.0);
                }
                other => color.rgb = other.apply_rgb(color.rgb),
            }
        }
        Some(color)
    }
}

/// A resolved solid color or CSS gradient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResolvedFill {
    Color(String),
    Gradient {
        style: String,
        first: String,
        transparent: bool,
    },
}

#[derive(Clone)]
pub(super) struct GradientFill {
    /// CSS angle for a linear fill.
    pub linear_angle: Option<f64>,
    pub linear_scaled: bool,
    pub fixed_orientation: bool,
    pub focus: Option<GradientFocus>,
    pub tile_insets: [f64; 4],
    /// Positions in 0..=1 and DrawingML colors, which may still be scheme slots.
    pub stops: Vec<(f64, Color)>,
}

#[derive(Clone)]
pub(super) enum GradientFocus {
    RadialPoint([f64; 2]),
    RadialEllipse([f64; 4]),
    Rectangle([f64; 4]),
    Shape([f64; 4]),
}

#[derive(Clone, Copy)]
enum GradientShape {
    Rectangle,
    Ellipse,
    Polygon,
}

impl GradientFocus {
    pub(super) fn from_path(path: &str, [left, top, right, bottom]: [f64; 4]) -> Option<Self> {
        let edges = [left, top, right, bottom];
        if left + right > 1.0 || top + bottom > 1.0 {
            return None;
        }
        match path {
            "circle" if (left + right - 1.0).abs() < 1e-9 && (top + bottom - 1.0).abs() < 1e-9 => {
                let (x, y) = (left - right, top - bottom);
                (x * x + y * y < 1.0 && (x != 0.0 || y != 0.0)).then_some(Self::RadialPoint([x, y]))
            }
            "circle" if edges.iter().all(|edge| *edge >= 0.0) => {
                let (x, y) = (left - right, top - bottom);
                let (rx, ry) = (1.0 - left - right, 1.0 - top - bottom);
                Some(Self::RadialEllipse([x, y, rx.max(0.0), ry.max(0.0)]))
            }
            "rect" if edges.iter().all(|edge| *edge >= 0.0) => Some(Self::Rectangle(edges)),
            "shape" if edges.iter().all(|edge| *edge >= 0.0) => Some(Self::Shape(edges)),
            // shortcut: extended radial focuses and non-nested paths use the centered fill until their geometry is supported.
            _ => None,
        }
    }
}

impl GradientFill {
    pub(super) fn from_node(fill: roxmltree::Node<'_, '_>) -> Option<Self> {
        let line = child(fill, "lin");
        let angle = linear_gradient_angle(line.and_then(|line| line.attribute("ang")));
        let linear_scaled = matches!(
            line.and_then(|line| line.attribute("scaled")),
            Some("1" | "true")
        );
        let mut stops = Vec::new();
        let mut stop_count = None;
        if let Some(list) = child(fill, "gsLst") {
            let children = list.children().filter(|node| node.has_tag_name("gs"));
            stop_count = Some(children.clone().count());
            for stop in children {
                let pos = stop
                    .attribute("pos")
                    .and_then(percentage)
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                if let Some(color) = Color::in_node(stop) {
                    stops.push((pos, color));
                }
            }
        }
        if stops.len() < 2 && stop_count != Some(stops.len()) {
            return None;
        }
        stops.sort_by(|left, right| left.0.total_cmp(&right.0));
        let path = child(fill, "path");
        let focus = path.and_then(|path| {
            let rect = child(path, "fillToRect");
            GradientFocus::from_path(
                path.attribute("path").unwrap_or("shape"),
                focus_insets(|key| {
                    rect.and_then(|rect| rect.attribute(key))
                        .and_then(percentage)
                }),
            )
        });
        let tile = child(fill, "tileRect");
        Some(Self {
            linear_angle: path.is_none().then_some(angle),
            linear_scaled,
            fixed_orientation: matches!(fill.attribute("rotWithShape"), Some("0" | "false")),
            focus,
            tile_insets: gradient_insets(|key| {
                tile.and_then(|tile| tile.attribute(key))
                    .and_then(percentage)
            }),
            stops,
        })
    }

    fn resolve(
        &self,
        scheme: impl Fn(&str) -> Option<ResolvedColor>,
        shape: GradientShape,
    ) -> Option<ResolvedFill> {
        let mut resolved = Vec::new();
        for (pos, color) in &self.stops {
            resolved.push((*pos, color.hex_in(&scheme)?));
        }
        let gamma = match resolved.as_slice() {
            [] => return Some(ResolvedFill::Color("#000000".into())),
            [(_, color)] => return Some(ResolvedFill::Color(color.clone())),
            [(start, _), (end, _)] => *start == 0.0 && *end == 1.0,
            [(start, first), (middle, _), (end, last)] => {
                *start == 0.0 && *end == 1.0 && (0.0 < *middle && *middle < 1.0) && first == last
            }
            _ => false,
        };
        let body = resolved
            .iter()
            .map(|(pos, color)| format!("{color} {}%", pos * 100.0))
            .collect::<Vec<_>>()
            .join(", ");
        let mut style = match self.linear_angle {
            Some(angle) => format!("background:linear-gradient({angle}deg, {body});"),
            None => format!("background:radial-gradient(circle at center, {body});"),
        };
        if self.linear_angle.is_some() && self.linear_scaled {
            style.push_str("--pptx-linear-scaled:1;");
        }
        if self.fixed_orientation {
            style.push_str("--pptx-gradient-fixed:1;");
        }
        if self.tile_insets != [0.0; 4] {
            let [left, top, right, bottom] = self.tile_insets;
            style.push_str(&format!(
                "--pptx-gradient-tile:{left} {top} {right} {bottom};"
            ));
        }
        match self.focus {
            Some(GradientFocus::RadialPoint([x, y])) => {
                style.push_str(&format!("--pptx-radial-focus:{x:.5} {y:.5};"))
            }
            Some(GradientFocus::RadialEllipse([x, y, rx, ry])) => {
                style.push_str(&format!("--pptx-radial-ellipse:{x} {y} {rx} {ry};"))
            }
            Some(GradientFocus::Shape([l, t, r, b])) if matches!(shape, GradientShape::Ellipse) => {
                let (x, y) = (l - r, t - b);
                let (rx, ry) = ((1.0 - l - r).max(0.0), (1.0 - t - b).max(0.0));
                style.push_str(&format!(
                    "--pptx-radial-ellipse:{x} {y} {rx} {ry};--pptx-shape-ellipse:1;"
                ));
            }
            Some(GradientFocus::Shape([l, t, r, b]))
                if matches!(shape, GradientShape::Polygon)
                    && (l + r - 1.0).abs() < 1e-9
                    && (t + b - 1.0).abs() < 1e-9 =>
            {
                style.push_str(&format!(
                    "--pptx-rectangular-focus:{l:.5} {t:.5} {r:.5} {b:.5};--pptx-polygon-focus:{l} {t};"
                ));
            }
            Some(GradientFocus::Rectangle([l, t, r, b]) | GradientFocus::Shape([l, t, r, b])) => {
                style.push_str(&format!(
                    "--pptx-rectangular-focus:{l:.5} {t:.5} {r:.5} {b:.5};"
                ))
            }
            None => {}
        }
        if gamma {
            style.push_str("--pptx-gradient-gamma:1;");
        }
        Some(ResolvedFill::Gradient {
            style,
            first: resolved[0].1.clone(),
            transparent: resolved.iter().any(|(_, color)| color.len() == 9),
        })
    }
}

pub(super) fn linear_gradient_angle(raw: Option<&str>) -> f64 {
    let angle = raw
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|angle| angle.is_finite())
        .unwrap_or(0.0);
    // DrawingML starts at right and CSS starts at up. Both increase clockwise.
    (90.0 + angle / 60_000.0).rem_euclid(360.0)
}

pub(super) fn gradient_insets(inset: impl Fn(&str) -> Option<f64>) -> [f64; 4] {
    ["l", "t", "r", "b"].map(|key| inset(key).unwrap_or(0.0))
}

pub(super) fn focus_insets(inset: impl Fn(&str) -> Option<f64>) -> [f64; 4] {
    ["l", "t", "r", "b"].map(|key| inset(key).unwrap_or(0.5))
}

pub(super) fn percentage(value: &str) -> Option<f64> {
    let (number, divisor) = value
        .strip_suffix('%')
        .map_or((value, 100_000.0), |value| (value, 100.0));
    let parsed = number.parse::<f64>().ok()?;
    parsed.is_finite().then(|| parsed / divisor)
}

fn hex(ResolvedColor { rgb, alpha }: ResolvedColor) -> String {
    let [r, g, b] = rgb.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    let a = (alpha * 255.0).round() as u8;
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

impl Transform {
    fn apply_rgb(self, rgb: Rgb) -> Rgb {
        match self {
            // DrawingML tint and shade mix with white or black in linear RGB.
            Self::Tint(factor) => rgb.map(|c| to_srgb(1.0 - (1.0 - to_linear(c)) * factor)),
            Self::Shade(factor) => rgb.map(|c| to_srgb(to_linear(c) * factor)),
            Self::Lightness { scale, offset } => adjust_lightness(rgb, scale, offset),
            Self::Saturation { scale, offset } => {
                let (hue, sat, light) = to_hsl(rgb);
                from_hsl(hue, (sat * scale + offset).clamp(0.0, 1.0), light)
            }
            Self::Hue { scale, offset } => {
                let (hue, sat, light) = to_hsl(rgb);
                from_hsl((hue * scale + offset).rem_euclid(360.0), sat, light)
            }
            Self::Channel {
                index,
                scale,
                offset,
            } => {
                let mut rgb = rgb;
                rgb[index] = (rgb[index] * scale + offset).clamp(0.0, 1.0);
                rgb
            }
            Self::Invert => rgb.map(|c| 1.0 - c),
            Self::Gray => {
                let y = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
                [y, y, y]
            }
            Self::Complement => {
                let (hue, sat, light) = to_hsl(rgb);
                from_hsl((hue + 180.0).rem_euclid(360.0), sat, light)
            }
            Self::Alpha { .. } => rgb,
            Self::Gamma => rgb.map(to_srgb),
            Self::InverseGamma => rgb.map(to_linear),
        }
    }
}

fn parse_transform(kind: &str, value: Option<&str>) -> Option<Transform> {
    match kind {
        "inv" | "invClr" => Some(Transform::Invert),
        "gray" | "grayClr" => Some(Transform::Gray),
        "comp" => Some(Transform::Complement),
        "alpha" => percentage_opt(value).map(|percent| Transform::Alpha {
            scale: 0.0,
            offset: percent,
        }),
        "alphaMod" => percentage_opt(value).map(|percent| Transform::Alpha {
            scale: percent,
            offset: 0.0,
        }),
        "alphaOff" => percentage_opt(value).map(|percent| Transform::Alpha {
            scale: 1.0,
            offset: percent,
        }),
        "gamma" => Some(Transform::Gamma),
        "invGamma" => Some(Transform::InverseGamma),
        "tint" => percentage_opt(value)
            .filter(|percent| (0.0..=1.0).contains(percent))
            .map(Transform::Tint),
        "shade" => percentage_opt(value)
            .filter(|percent| (0.0..=1.0).contains(percent))
            .map(Transform::Shade),
        "lum" => percentage_opt(value).map(|percent| Transform::Lightness {
            scale: 0.0,
            offset: percent,
        }),
        "lumMod" => percentage_opt(value).map(|percent| Transform::Lightness {
            scale: percent,
            offset: 0.0,
        }),
        "lumOff" => percentage_opt(value).map(|percent| Transform::Lightness {
            scale: 1.0,
            offset: percent,
        }),
        "sat" => percentage_opt(value).map(|percent| Transform::Saturation {
            scale: 0.0,
            offset: percent,
        }),
        "satMod" => percentage_opt(value).map(|percent| Transform::Saturation {
            scale: percent,
            offset: 0.0,
        }),
        "satOff" => percentage_opt(value).map(|percent| Transform::Saturation {
            scale: 1.0,
            offset: percent,
        }),
        "hue" => hue_degrees_opt(value).map(|degrees| Transform::Hue {
            scale: 0.0,
            offset: degrees,
        }),
        "hueMod" => percentage_opt(value).map(|percent| Transform::Hue {
            scale: percent,
            offset: 0.0,
        }),
        "hueOff" => hue_degrees_opt(value).map(|degrees| Transform::Hue {
            scale: 1.0,
            offset: degrees,
        }),
        "red" | "green" | "blue" => channel(kind, 0.0, percentage_opt(value)?),
        "redMod" | "greenMod" | "blueMod" => channel(kind, percentage_opt(value)?, 0.0),
        "redOff" | "greenOff" | "blueOff" => channel(kind, 1.0, percentage_opt(value)?),
        _ => None,
    }
}

fn percentage_opt(value: Option<&str>) -> Option<f64> {
    value.and_then(percentage)
}

fn hue_degrees_opt(value: Option<&str>) -> Option<f64> {
    value.and_then(hue_degrees)
}

fn hue_degrees(value: &str) -> Option<f64> {
    if let Some(percent) = value.strip_suffix('%') {
        let parsed = percent.parse::<f64>().ok()?;
        return parsed.is_finite().then_some(parsed / 100.0 * 360.0);
    }
    let parsed = value.parse::<f64>().ok()?;
    parsed.is_finite().then_some(parsed / 60_000.0)
}

fn channel(kind: &str, scale: f64, offset: f64) -> Option<Transform> {
    if !scale.is_finite() || !offset.is_finite() {
        return None;
    }
    let index = match kind {
        "red" | "redMod" | "redOff" => 0,
        "green" | "greenMod" | "greenOff" => 1,
        "blue" | "blueMod" | "blueOff" => 2,
        _ => return None,
    };
    Some(Transform::Channel {
        index,
        scale,
        offset,
    })
}

fn adjust_lightness(rgb: Rgb, scale: f64, offset: f64) -> Rgb {
    let [r, g, b] = rgb;
    let light = (r.max(g).max(b) + r.min(g).min(b)) / 2.0;
    let changed = (light * scale + offset).clamp(0.0, 1.0);
    let span = 1.0 - (2.0 * light - 1.0).abs();
    if span == 0.0 {
        return [changed; 3];
    }
    // Rescale the channels around the new HSL lightness, keeping hue and saturation.
    let ratio = (1.0 - (2.0 * changed - 1.0).abs()) / span;
    rgb.map(|c| (changed + (c - light) * ratio).clamp(0.0, 1.0))
}

fn to_hsl([r, g, b]: Rgb) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return (0.0, 0.0, light);
    }
    let sat = delta / (1.0 - (2.0 * light - 1.0).abs());
    let hue = if max == r {
        (g - b) / delta
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    ((hue * 60.0).rem_euclid(360.0), sat.clamp(0.0, 1.0), light)
}

fn from_hsl(hue: f64, sat: f64, light: f64) -> Rgb {
    let sat = sat.clamp(0.0, 1.0);
    let light = light.clamp(0.0, 1.0);
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let hue = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let match_light = light - chroma / 2.0;
    [r, g, b].map(|c| (c + match_light).clamp(0.0, 1.0))
}

fn to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

type ColorMap = HashMap<String, String>;

#[derive(Default)]
struct Theme {
    colors: HashMap<String, ResolvedColor>,
    major_font: Option<String>,
    minor_font: Option<String>,
    fills: Vec<Option<StyleFill>>,
    backgrounds: Vec<Option<StyleFill>>,
    lines: Vec<LineStyle>,
}

#[derive(Clone)]
enum StyleFill {
    Solid(Color),
    Gradient(GradientFill),
}

#[derive(Default)]
struct LineStyle {
    emu: Option<u32>,
    paint: Option<LinePaint>,
    cap: Option<StrokeCap>,
    dash: Option<Vec<f64>>,
}

#[derive(Clone, Copy, Default)]
pub(super) enum StrokeCap {
    #[default]
    Flat,
    Round,
    Square,
}

impl StrokeCap {
    pub fn code(self) -> u8 {
        match self {
            Self::Flat => 0,
            Self::Round => 1,
            Self::Square => 2,
        }
    }
}

enum LinePaint {
    Solid(Color),
    Suppressed,
}

impl LineStyle {
    fn read(line: Node<'_, '_>) -> Self {
        let paint = line
            .children()
            .find_map(|fill| match fill.tag_name().name() {
                "solidFill" => Some(
                    Color::in_node(fill)
                        .map(LinePaint::Solid)
                        .unwrap_or(LinePaint::Suppressed),
                ),
                "noFill" | "gradFill" | "pattFill" => Some(LinePaint::Suppressed),
                _ => None,
            });
        let emu = line.attribute("w").and_then(|width| width.parse().ok());
        let cap = match line.attribute("cap") {
            Some("flat") => Some(StrokeCap::Flat),
            Some("rnd") => Some(StrokeCap::Round),
            Some("sq") => Some(StrokeCap::Square),
            _ => None,
        };
        let dash = child(line, "prstDash")
            .and_then(|dash| match dash.attribute("val")? {
                "solid" => Some(Vec::new()),
                "dot" => Some(vec![1.0, 3.0]),
                "dash" => Some(vec![4.0, 3.0]),
                "dashDot" => Some(vec![4.0, 3.0, 1.0, 3.0]),
                "lgDash" => Some(vec![8.0, 3.0]),
                "lgDashDot" => Some(vec![8.0, 3.0, 1.0, 3.0]),
                "lgDashDotDot" => Some(vec![8.0, 3.0, 1.0, 3.0, 1.0, 3.0]),
                "sysDot" => Some(vec![1.0, 1.0]),
                "sysDash" => Some(vec![3.0, 1.0]),
                "sysDashDot" => Some(vec![3.0, 1.0, 1.0, 1.0]),
                "sysDashDotDot" => Some(vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
                _ => None,
            })
            .or_else(|| child(line, "custDash").map(|dash| custom_dash(dash).unwrap_or_default()));
        Self {
            emu,
            paint,
            cap,
            dash,
        }
    }
}

fn custom_dash(dash: Node<'_, '_>) -> Option<Vec<f64>> {
    let mut lengths = Vec::new();
    for stop in dash.children().filter(|node| node.has_tag_name("ds")) {
        for attribute in ["d", "sp"] {
            let length = percentage(stop.attribute(attribute)?)?;
            if length <= 0.0 {
                return None;
            }
            lengths.push(length);
        }
    }
    Some(lengths)
}

impl Theme {
    fn parse(xml: &str) -> Result<Self, DataError> {
        let document = Document::parse(xml).map_err(invalid)?;
        let mut theme = Self::default();
        let Some(elements) = child(document.root_element(), "themeElements") else {
            return Ok(theme);
        };
        if let Some(scheme) = child(elements, "clrScheme") {
            for slot in scheme.children().filter(Node::is_element) {
                if let Some(color) = Color::in_node(slot).and_then(|color| color.resolve(|_| None))
                {
                    theme.colors.insert(slot.tag_name().name().into(), color);
                }
            }
        }
        if let Some(format) = child(elements, "fmtScheme") {
            theme.fills = style_fills(format, "fillStyleLst");
            theme.backgrounds = style_fills(format, "bgFillStyleLst");
            if let Some(lines) = child(format, "lnStyleLst") {
                theme.lines = lines
                    .children()
                    .filter(|line| line.has_tag_name("ln"))
                    .map(LineStyle::read)
                    .collect();
            }
        }
        if let Some(fonts) = child(elements, "fontScheme") {
            let latin = |label| {
                child(fonts, label)
                    .and_then(|font| child(font, "latin"))
                    .and_then(|latin| latin.attribute("typeface"))
                    .map(str::to_owned)
            };
            theme.major_font = latin("majorFont");
            theme.minor_font = latin("minorFont");
        }
        Ok(theme)
    }

    fn style(&self, index: u32) -> Option<&StyleFill> {
        let (list, offset) = match index {
            1..=999 => (&self.fills, 1),
            1001.. => (&self.backgrounds, 1001),
            _ => return None,
        };
        list.get(usize::try_from(index - offset).ok()?)?.as_ref()
    }
}

fn style_fills(format: Node<'_, '_>, name: &str) -> Vec<Option<StyleFill>> {
    child(format, name)
        .into_iter()
        .flat_map(|list| list.children().filter(Node::is_element))
        .map(|fill| {
            if fill.has_tag_name("solidFill") {
                Color::in_node(fill).map(StyleFill::Solid)
            } else if fill.has_tag_name("gradFill") {
                GradientFill::from_node(fill).map(StyleFill::Gradient)
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|node| node.has_tag_name(name))
}

#[derive(Clone, Default)]
pub(super) struct Context {
    theme: Option<Arc<Theme>>,
    master: ColorMap,
    colors: ColorMap,
}

impl Context {
    pub(super) fn text_color(&self, node: Node<'_, '_>) -> Option<String> {
        Color::in_node(node)?.hex_in(|slot| self.color(slot))
    }

    pub(super) fn font_family<'a>(&'a self, typeface: &'a str) -> Option<&'a str> {
        let typeface = match typeface {
            "+mj-lt" => self.theme.as_ref()?.major_font.as_deref()?,
            "+mn-lt" => self.theme.as_ref()?.minor_font.as_deref()?,
            other if !other.starts_with('+') => other,
            _ => return None,
        }
        .trim();
        (!typeface.is_empty()
            && !typeface
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, ';' | ',')))
        .then_some(typeface)
    }

    fn color(&self, slot: &str) -> Option<ResolvedColor> {
        let key = self.colors.get(slot).map_or_else(
            || match slot {
                "bg1" => "lt1",
                "bg2" => "lt2",
                "tx1" => "dk1",
                "tx2" => "dk2",
                slot => slot,
            },
            String::as_str,
        );
        self.theme.as_ref()?.colors.get(key).copied()
    }

    fn color_with_placeholder(
        &self,
        slot: &str,
        placeholder: Option<&Color>,
    ) -> Option<ResolvedColor> {
        if slot == "phClr" {
            placeholder.and_then(|color| color.resolve(|slot| self.color(slot)))
        } else {
            self.color(slot)
        }
    }

    fn style(
        &self,
        index: u32,
        placeholder: Option<&Color>,
        shape: GradientShape,
    ) -> Option<ResolvedFill> {
        let color = |slot: &str| self.color_with_placeholder(slot, placeholder);
        match self.theme.as_ref()?.style(index)? {
            StyleFill::Solid(fill) => fill.hex_in(color).map(ResolvedFill::Color),
            StyleFill::Gradient(fill) => fill.resolve(color, shape),
        }
    }
}

#[derive(Default)]
pub(super) struct ThemeReader {
    themes: HashMap<String, Arc<Theme>>,
    parents: HashMap<String, Context>,
    xml_bytes: usize,
}

#[derive(Default)]
pub(super) struct ObjectStyle {
    pub fill: Option<String>,
    pub text: Option<TextBody>,
    pub paths: Vec<CustomPath>,
    pub path_shades: Vec<i8>,
    pub connector: Option<ConnectorStroke>,
}

pub(super) struct ConnectorStroke {
    pub width: [f64; 2],
    pub color: String,
    pub inverse: bool,
    pub cap: StrokeCap,
    pub dash: Vec<f64>,
}

fn connector_direction(shape: Node<'_, '_>, properties: Node<'_, '_>) -> Option<bool> {
    match child(properties, "prstGeom").and_then(|geom| geom.attribute("prst")) {
        Some("line" | "straightConnector1") => Some(false),
        Some("lineInv") => Some(true),
        None if shape.has_tag_name("cxnSp") && child(properties, "custGeom").is_none() => {
            Some(false)
        }
        _ => None,
    }
}

fn connector_stroke(
    shape: Node<'_, '_>,
    properties: Node<'_, '_>,
    context: &Context,
    size: &SlideSize,
    inverse: bool,
) -> Option<ConnectorStroke> {
    let reference = child(shape, "style").and_then(|style| child(style, "lnRef"));
    let index = reference
        .and_then(|reference| reference.attribute("idx"))
        .and_then(|index| index.parse::<usize>().ok());
    let themed = index.and_then(|index| context.theme.as_ref()?.lines.get(index.checked_sub(1)?));
    let line = child(properties, "ln");
    let direct = line.map(LineStyle::read).unwrap_or_default();
    let placeholder = reference.and_then(Color::in_node);
    let scheme = |slot: &str| context.color_with_placeholder(slot, placeholder.as_ref());
    let color = match direct
        .paint
        .as_ref()
        .or_else(|| themed.and_then(|line| line.paint.as_ref()))
    {
        Some(LinePaint::Suppressed) => return None,
        Some(LinePaint::Solid(color)) => color.hex_in(scheme)?,
        None if index == Some(0) && line.is_none() => return None,
        None => placeholder
            .and_then(|color| color.hex_in(|slot| context.color(slot)))
            .unwrap_or_else(|| "#000000".into()),
    };
    // Office's hard-coded line width is 3/4 pt when no drawing or theme width is set.
    let emu = f64::from(
        direct
            .emu
            .or_else(|| themed.and_then(|line| line.emu))
            .unwrap_or(9_525),
    );
    let width = [size.px_x(emu).max(1.0), size.px_y(emu).max(1.0)];
    Some(ConnectorStroke {
        width,
        color,
        inverse,
        cap: direct
            .cap
            .or_else(|| themed.and_then(|line| line.cap))
            .unwrap_or_default(),
        dash: direct
            .dash
            .or_else(|| themed.and_then(|line| line.dash.clone()))
            .unwrap_or_default(),
    })
}

type CustomPath = Vec<Vec<[f64; 2]>>;
type ShadedPaths = (Vec<CustomPath>, Vec<i8>);

fn preset_paths(properties: Node<'_, '_>) -> Option<ShadedPaths> {
    let geometry = child(properties, "prstGeom")?;
    let adjustment = |name: &str, default| {
        child(geometry, "avLst")
            .and_then(|guides| {
                guides
                    .children()
                    .find(|guide| guide.has_tag_name("gd") && guide.attribute("name") == Some(name))
            })
            .and_then(|guide| {
                let mut formula = guide.attribute("fmla")?.split_ascii_whitespace();
                if formula.next()? != "val" {
                    return None;
                }
                let value = f64::from(formula.next()?.parse::<i32>().ok()?);
                formula.next().is_none().then_some(value)
            })
            .unwrap_or(default)
    };
    let short_side = || {
        let ext = child(child(properties, "xfrm")?, "ext")?;
        let (width, height) = (shape_extent(ext, "cx")?, shape_extent(ext, "cy")?);
        let short = width.min(height);
        Some([short / width, short / height])
    };
    let name = geometry.attribute("prst")?;
    let arc_angle = |guide, default| adjustment(guide, default).clamp(0.0, DRAWINGML_TURN - 1.0);
    let band_width = |guide, default| adjustment(guide, default).clamp(0.0, 50_000.0) / 100_000.0;
    let outline = match name {
        "triangle" => vec![
            [0.0, 1.0],
            [
                adjustment("adj", 50_000.0).clamp(0.0, 100_000.0) / 100_000.0,
                0.0,
            ],
            [1.0, 1.0],
        ],
        "rtTriangle" => vec![[0.0, 1.0], [0.0, 0.0], [1.0, 1.0]],
        "diamond" => vec![[0.0, 0.5], [0.5, 0.0], [1.0, 0.5], [0.5, 1.0]],
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" | "leftRightArrow" | "upDownArrow" => {
            block_arrow(
                name,
                short_side()?,
                adjustment("adj1", 50_000.0),
                adjustment("adj2", 50_000.0),
            )
        }
        "chevron" | "homePlate" => {
            let [sx, _] = short_side()?;
            let head = (sx * adjustment("adj", 50_000.0) / 100_000.0).clamp(0.0, 1.0);
            let shoulder = 1.0 - head;
            let mut points = vec![
                [0.0, 0.0],
                [shoulder, 0.0],
                [1.0, 0.5],
                [shoulder, 1.0],
                [0.0, 1.0],
            ];
            if name == "chevron" {
                points.push([head, 0.5]);
            }
            points
        }
        "parallelogram" | "trapezoid" | "nonIsoscelesTrapezoid" => {
            let [sx, _] = short_side()?;
            let offset =
                |guide, limit| (sx * adjustment(guide, 25_000.0) / 100_000.0).clamp(0.0, limit);
            match name {
                "parallelogram" => {
                    let x = offset("adj", 1.0);
                    vec![[0.0, 1.0], [x, 0.0], [1.0, 0.0], [1.0 - x, 1.0]]
                }
                "trapezoid" => {
                    let x = offset("adj", 0.5);
                    vec![[0.0, 1.0], [x, 0.0], [1.0 - x, 0.0], [1.0, 1.0]]
                }
                _ => vec![
                    [0.0, 1.0],
                    [offset("adj1", 0.5), 0.0],
                    [1.0 - offset("adj2", 0.5), 0.0],
                    [1.0, 1.0],
                ],
            }
        }
        "roundRect" | "round1Rect" | "round2SameRect" | "round2DiagRect" | "snip1Rect"
        | "snip2SameRect" | "snip2DiagRect" | "snipRoundRect" => {
            rect_corner_outline(name, short_side()?, adjustment)?
        }
        "pie" | "chord" | "arc" => {
            let defaults = match name {
                "pie" => [0.0, 16_200_000.0],
                "chord" => [2_700_000.0, 16_200_000.0],
                _ => [16_200_000.0, 0.0],
            };
            let start = arc_angle("adj1", defaults[0]);
            let end = arc_angle("adj2", defaults[1]);
            let sweep = clockwise_sweep(start, end);
            let mut outline = preset_ellipse_arc([0.5; 2], short_side()?, start, sweep)?;
            if sweep != DRAWINGML_TURN && name != "chord" {
                outline.push([0.5, 0.5]);
            }
            outline
        }
        "blockArc" => {
            return block_arc_paths(
                short_side()?,
                arc_angle("adj1", 10_800_000.0),
                arc_angle("adj2", 0.0),
                band_width("adj3", 25_000.0),
            )
            .map(|paths| (paths, Vec::new()));
        }
        "pieWedge" => {
            let mut outline = vec![[0.0, 1.0]];
            append_ellipse_arc(
                &mut outline,
                [1.0; 2],
                -std::f64::consts::PI,
                std::f64::consts::FRAC_PI_2,
            )?;
            outline.push([1.0, 1.0]);
            outline
        }
        "donut" | "noSmoking" => {
            use std::f64::consts::{PI, TAU};

            let size @ [sx, sy] = short_side()?;
            let thickness = band_width("adj", if name == "donut" { 25_000.0 } else { 18_750.0 });
            if thickness == 0.0 {
                return Some(ShadedPaths::default());
            }
            let mut contours = vec![ellipse_contour([0.5; 2], PI, TAU)?];
            let inner @ [rx, ry] = [0.5 - sx * thickness, 0.5 - sy * thickness];
            if rx > 0.0 && ry > 0.0 {
                if name == "donut" {
                    contours.push(ellipse_contour(inner, PI, -TAU)?);
                } else {
                    let diagonal = sx.atan2(sy);
                    let (sin, cos) = diagonal.sin_cos();
                    let radius = 1.0 / (cos * sx / rx).hypot(sin * sy / ry);
                    let offset = (thickness / 2.0).atan2(radius);
                    let start = (diagonal - offset).to_degrees() * 60_000.0;
                    let sweep = (2.0 * offset - PI).to_degrees() * 60_000.0;
                    for angle in [start, start - DRAWINGML_TURN / 2.0] {
                        let hole = preset_ellipse_arc(inner, size, angle, sweep)?;
                        if hole.len() >= 3 {
                            contours.push(hole);
                        }
                    }
                }
            }
            return Some((vec![contours], Vec::new()));
        }
        "cube" | "bevel" | "foldedCorner" | "can" => {
            return shaded_preset_faces(
                name,
                short_side()?,
                adjustment(
                    "adj",
                    match name {
                        "bevel" => 12_500.0,
                        "foldedCorner" => 16_667.0,
                        _ => 25_000.0,
                    },
                ),
            );
        }
        "hexagon" => {
            let [sx, _] = short_side()?;
            let x = (sx * adjustment("adj", 25_000.0) / 100_000.0).clamp(0.0, 0.5);
            let dy = (adjustment("vf", 115_470.0) / 200_000.0).clamp(0.0, 1.0)
                * (std::f64::consts::PI / 3.0).sin();
            vec![
                [0.0, 0.5],
                [x, 0.5 - dy],
                [1.0 - x, 0.5 - dy],
                [1.0, 0.5],
                [1.0 - x, 0.5 + dy],
                [x, 0.5 + dy],
            ]
        }
        "octagon" | "plus" => {
            let [sx, sy] = short_side()?;
            let default = if name == "octagon" {
                29_289.0
            } else {
                25_000.0
            };
            let inset = adjustment("adj", default).clamp(0.0, 50_000.0) / 100_000.0;
            let (x, y) = (sx * inset, sy * inset);
            if name == "octagon" {
                vec![
                    [0.0, y],
                    [x, 0.0],
                    [1.0 - x, 0.0],
                    [1.0, y],
                    [1.0, 1.0 - y],
                    [1.0 - x, 1.0],
                    [x, 1.0],
                    [0.0, 1.0 - y],
                ]
            } else {
                vec![
                    [0.0, y],
                    [x, y],
                    [x, 0.0],
                    [1.0 - x, 0.0],
                    [1.0 - x, y],
                    [1.0, y],
                    [1.0, 1.0 - y],
                    [1.0 - x, 1.0 - y],
                    [1.0 - x, 1.0],
                    [x, 1.0],
                    [x, 1.0 - y],
                    [0.0, 1.0 - y],
                ]
            }
        }
        name => {
            // Default DrawingML star adjustments and ellipse scale factors from ECMA-376.
            let (points, inner, horizontal, vertical) = match name {
                "star4" => (4, 12_500.0, 100_000.0, 100_000.0),
                "star5" => (5, 19_098.0, 105_146.0, 110_557.0),
                "star6" => (6, 28_868.0, 115_470.0, 100_000.0),
                "star7" => (7, 34_601.0, 102_572.0, 105_210.0),
                "star8" => (8, 37_500.0, 100_000.0, 100_000.0),
                "star10" => (10, 42_533.0, 105_146.0, 100_000.0),
                "star12" => (12, 37_500.0, 100_000.0, 100_000.0),
                "star16" => (16, 37_500.0, 100_000.0, 100_000.0),
                "star24" => (24, 37_500.0, 100_000.0, 100_000.0),
                "star32" => (32, 37_500.0, 100_000.0, 100_000.0),
                _ => return None,
            };
            let inner = adjustment("adj", inner).clamp(0.0, 50_000.0) / 50_000.0;
            // Only presets with a declared scale guide let an adjustment replace it.
            let radius = |guide, default| {
                let scale = if default == 100_000.0 {
                    default
                } else {
                    adjustment(guide, default)
                };
                (scale / 200_000.0_f64).clamp(0.0, 1.0)
            };
            let (rx, ry) = (radius("hf", horizontal), radius("vf", vertical));
            (0..2 * points)
                .map(|vertex| {
                    let angle = f64::from(vertex) * std::f64::consts::PI / f64::from(points);
                    let (sin, cos) = angle.sin_cos();
                    let factor = if vertex % 2 == 0 { 1.0 } else { inner };
                    [0.5 + rx * factor * sin, ry * (1.0 - factor * cos)]
                })
                .collect()
        }
    };
    let paths = if outline.len() < 3 {
        Vec::new()
    } else {
        vec![vec![outline]]
    };
    Some((paths, Vec::new()))
}

fn shaded_preset_faces(name: &str, [sx, sy]: [f64; 2], adjustment: f64) -> Option<ShadedPaths> {
    let limit = if name == "cube" { 1.0 } else { 0.5 };
    let inset = (adjustment / 100_000.0).clamp(0.0, limit);
    let (x, y) = (sx * inset, sy * inset);
    let (r, b) = (1.0 - x, 1.0 - y);
    let (faces, shades) = match name {
        "cube" => (
            vec![
                vec![[0.0, y], [r, y], [r, 1.0], [0.0, 1.0]],
                vec![[r, y], [1.0, 0.0], [1.0, b], [r, 1.0]],
                vec![[0.0, y], [x, 0.0], [1.0, 0.0], [r, y]],
            ],
            vec![0, -20, 20],
        ),
        "bevel" => (
            vec![
                vec![[x, y], [r, y], [r, b], [x, b]],
                vec![[0.0, 0.0], [1.0, 0.0], [r, y], [x, y]],
                vec![[0.0, 1.0], [x, b], [r, b], [1.0, 1.0]],
                vec![[0.0, 0.0], [x, y], [x, b], [0.0, 1.0]],
                vec![[1.0, 0.0], [1.0, 1.0], [r, b], [r, y]],
            ],
            vec![0, 20, -20, 40, -40],
        ),
        "foldedCorner" => (
            vec![
                vec![[0.0, 0.0], [1.0, 0.0], [1.0, b], [r, 1.0], [0.0, 1.0]],
                vec![[r, 1.0], [r + x / 5.0, b + y / 5.0], [1.0, b]],
            ],
            vec![0, -20],
        ),
        "can" => {
            use std::f64::consts::PI;

            let y = (sy * adjustment / 200_000.0).clamp(0.0, 0.25);
            if y == 0.0 {
                return Some((
                    vec![vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]]],
                    Vec::new(),
                ));
            }
            let radii = [0.5, y];
            let mut side = vec![[0.0, y]];
            append_ellipse_arc(&mut side, radii, PI, -PI)?;
            side.push([1.0, 1.0 - y]);
            append_ellipse_arc(&mut side, radii, 0.0, PI)?;
            let mut lid = vec![[0.0, y]];
            append_ellipse_arc(&mut lid, radii, PI, PI)?;
            append_ellipse_arc(&mut lid, radii, 0.0, PI)?;
            lid.pop();
            (vec![side, lid], vec![0, 40])
        }
        _ => return None,
    };
    Some((faces.into_iter().map(|face| vec![face]).collect(), shades))
}

fn clockwise_sweep(start: f64, end: f64) -> f64 {
    if end > start {
        end - start
    } else {
        end - start + DRAWINGML_TURN
    }
}

fn preset_ellipse_arc(
    radii @ [rx, ry]: [f64; 2],
    [sx, sy]: [f64; 2],
    start: f64,
    sweep: f64,
) -> Option<Vec<[f64; 2]>> {
    // Restore the original aspect ratio before converting the saved ray angles.
    let raw_radii = [rx * sy, ry * sx];
    let parameter = ellipse_ray_parameter(raw_radii, start);
    let span = ellipse_parameter_span(raw_radii, parameter, start, sweep);
    ellipse_contour(radii, parameter, span)
}

fn block_arc_paths(
    size @ [sx, sy]: [f64; 2],
    start: f64,
    end: f64,
    thickness: f64,
) -> Option<Vec<CustomPath>> {
    if thickness == 0.0 {
        return Some(Vec::new());
    }
    let sweep = clockwise_sweep(start, end);
    let inner = [0.5 - sx * thickness, 0.5 - sy * thickness];
    let outline = |from, sweep| preset_ellipse_arc([0.5; 2], size, from, sweep);
    let hole = |to, sweep: f64| preset_ellipse_arc(inner, size, to, -sweep);
    let mut outer = outline(start, sweep)?;
    if inner.into_iter().any(|r| r == 0.0) {
        if sweep != DRAWINGML_TURN {
            outer.push([0.5, 0.5]);
        }
        return Some(vec![vec![outer]]);
    }
    let reverse = hole(end, sweep)?;
    if sweep == DRAWINGML_TURN {
        return Some(vec![vec![outer, reverse]]);
    }
    if outer.len() + reverse.len() <= 128 {
        outer.extend(reverse);
        return Some(vec![vec![outer]]);
    }

    // Divide very long bands into adjoining contours to keep each within the compositor limit.
    let half = sweep / 2.0;
    let middle = start + half;
    let mut first = outline(start, half)?;
    first.extend(hole(middle, half)?);
    let mut second = outline(middle, half)?;
    second.extend(hole(end, half)?);
    Some(vec![vec![first, second]])
}

fn ellipse_contour([rx, ry]: [f64; 2], start: f64, span: f64) -> Option<Vec<[f64; 2]>> {
    let point = |angle: f64| {
        let (sin, cos) = angle.sin_cos();
        [0.5 + rx * cos, 0.5 + ry * sin]
    };
    let mut outline = vec![point(start)];
    append_ellipse_arc(&mut outline, [rx, ry], start, span)?;
    if same_path_point(outline[0], *outline.last()?) {
        outline.pop();
    }
    if outline.len() == 2 {
        // A narrow chord still has a curved edge when one segment meets the tolerance.
        outline.insert(1, point(start + span / 2.0));
    }
    Some(outline)
}

fn rect_corner_outline(
    name: &str,
    [sx, sy]: [f64; 2],
    adjustment: impl Fn(&str, f64) -> f64,
) -> Option<Vec<[f64; 2]>> {
    let inset = |guide, default| adjustment(guide, default).clamp(0.0, 50_000.0) / 100_000.0;
    let single = || inset("adj", 16_667.0);
    let first = |default| inset("adj1", default);
    let second = |default| inset("adj2", default);
    // Corners run clockwise from the top left.
    let (insets, rounded) = match name {
        "roundRect" => ([single(); 4], [true; 4]),
        "round1Rect" => ([0.0, single(), 0.0, 0.0], [true; 4]),
        "round2SameRect" => {
            let (a, b) = (first(16_667.0), second(0.0));
            ([a, a, b, b], [true; 4])
        }
        "round2DiagRect" => {
            let (a, b) = (first(16_667.0), second(0.0));
            ([a, b, a, b], [true; 4])
        }
        "snip1Rect" => ([0.0, single(), 0.0, 0.0], [false; 4]),
        "snip2SameRect" => {
            let (a, b) = (first(16_667.0), second(0.0));
            ([a, a, b, b], [false; 4])
        }
        "snip2DiagRect" => {
            let (a, b) = (first(0.0), second(16_667.0));
            ([a, b, a, b], [false; 4])
        }
        "snipRoundRect" => (
            [first(16_667.0), second(16_667.0), 0.0, 0.0],
            [true, false, false, false],
        ),
        _ => return None,
    };

    let mut points = Vec::new();
    let quarter = std::f64::consts::FRAC_PI_2;
    for (corner, (inset, rounded)) in insets.into_iter().zip(rounded).enumerate() {
        let (x, y) = (sx * inset, sy * inset);
        let (start, end) = match corner {
            0 => ([0.0, y], [x, 0.0]),
            1 => ([1.0 - x, 0.0], [1.0, y]),
            2 => ([1.0, 1.0 - y], [1.0 - x, 1.0]),
            _ => ([x, 1.0], [0.0, 1.0 - y]),
        };
        push_path_vertex(&mut points, start)?;
        if rounded && inset > 0.0 {
            append_ellipse_arc(
                &mut points,
                [x, y],
                (corner as f64 - 2.0) * quarter,
                quarter,
            )?;
        } else {
            push_path_vertex(&mut points, end)?;
        }
    }
    if same_path_point(points[0], *points.last()?) {
        points.pop();
    }
    Some(points)
}

fn block_arrow(name: &str, [sx, sy]: [f64; 2], shaft: f64, head: f64) -> Vec<[f64; 2]> {
    let vertical = matches!(name, "upArrow" | "downArrow" | "upDownArrow");
    let double = matches!(name, "leftRightArrow" | "upDownArrow");
    let along = if vertical { sy } else { sx };
    let head = (along * head / 100_000.0).clamp(0.0, if double { 0.5 } else { 1.0 });
    let top = 0.5 - shaft.clamp(0.0, 100_000.0) / 200_000.0;
    let bottom = 1.0 - top;
    let shoulder = 1.0 - head;
    let mut points = if double {
        vec![
            [0.0, 0.5],
            [head, 0.0],
            [head, top],
            [shoulder, top],
            [shoulder, 0.0],
            [1.0, 0.5],
            [shoulder, 1.0],
            [shoulder, bottom],
            [head, bottom],
            [head, 1.0],
        ]
    } else {
        vec![
            [0.0, top],
            [shoulder, top],
            [shoulder, 0.0],
            [1.0, 0.5],
            [shoulder, 1.0],
            [shoulder, bottom],
            [0.0, bottom],
        ]
    };
    for point in &mut points {
        let [x, y] = *point;
        *point = match name {
            "leftArrow" => [1.0 - x, y],
            "upArrow" => [y, 1.0 - x],
            "downArrow" | "upDownArrow" => [y, x],
            _ => [x, y],
        };
    }
    points
}

#[test]
fn block_arc_preserves_its_inner_band_on_wide_and_almost_closed_shapes() {
    let wide = block_arc_paths([0.5, 1.0], 0.0, 5_400_000.0, 0.25).unwrap();
    let points = &wide[0][0];
    assert!(same_path_point(points[0], [1.0, 0.5]));
    assert!(
        points
            .iter()
            .any(|&point| same_path_point(point, [0.5, 1.0]))
    );
    assert!(
        points
            .iter()
            .any(|&point| same_path_point(point, [0.5, 0.75]))
    );
    assert!(same_path_point(*points.last().unwrap(), [0.875, 0.5]));
    let long = block_arc_paths([1.0; 2], 0.0, 21_599_999.0, 0.01).unwrap();
    assert_eq!(long[0].len(), 2);
    assert!(
        long[0]
            .iter()
            .all(|points| (3..=128).contains(&points.len()))
    );
    let full = block_arc_paths([0.5, 1.0], 0.0, 0.0, 0.25).unwrap();
    assert!(full[0][0][1][1] > 0.5 && full[0][1][1][1] < 0.5);
}

#[test]
fn prohibition_symbol_has_two_mirrored_cutouts() {
    let document = Document::parse(
        r#"<spPr><xfrm><ext cx="4000" cy="2000"/></xfrm>
            <prstGeom prst="noSmoking"/></spPr>"#,
    )
    .unwrap();
    let (paths, _) = preset_paths(document.root_element()).unwrap();
    let [_, first, second] = paths[0].as_slice() else {
        panic!("expected two cutouts")
    };
    assert!(first.len() > 3);
    assert_eq!(first.len(), second.len());
    for (&[x, y], &opposite) in first.iter().zip(second) {
        assert!(same_path_point(opposite, [1.0 - x, 1.0 - y]));
    }
}

#[test]
fn curved_presets_preserve_the_ray_angle_and_donut_cutout() {
    let paths = |name, guides| {
        let xml = format!(
            r#"<spPr><xfrm><ext cx="4000" cy="2000"/></xfrm>
                <prstGeom prst="{name}"><avLst>{guides}</avLst></prstGeom></spPr>"#
        );
        let document = Document::parse(&xml).unwrap();
        preset_paths(document.root_element()).unwrap().0
    };

    let pie = paths("pie", r#"<gd name="adj1" fmla="val 2700000"/>"#);
    let outline = &pie[0][0];
    let [x, y] = outline[0];
    assert!((x - (0.5 + 0.5 / 5.0f64.sqrt())).abs() < 1e-12);
    assert!((y - (0.5 + 1.0 / 5.0f64.sqrt())).abs() < 1e-12);
    assert_eq!(outline.last(), Some(&[0.5, 0.5]));

    let small = r#"<gd name="adj1" fmla="val 0"/><gd name="adj2" fmla="val 60000"/>"#;
    assert_eq!(paths("chord", small)[0][0].len(), 3);

    let ring = paths("donut", "");
    let [outer, inner] = ring[0].as_slice() else {
        panic!("expected a donut with a cutout")
    };
    assert!(same_path_point(outer[0], [0.0, 0.5]));
    assert!(same_path_point(inner[0], [0.125, 0.5]));
    assert!(outer[1][1] < 0.5 && inner[1][1] > 0.5);
    assert!(paths("donut", r#"<gd name="adj" fmla="val 0"/>"#).is_empty());
}

#[test]
fn adjusted_block_presets_use_the_short_side_in_both_orientations() {
    let outline = |name, width, height, guides| {
        let xml = format!(
            r#"<spPr><xfrm><ext cx="{width}" cy="{height}"/></xfrm>
                <prstGeom prst="{name}"><avLst>{guides}</avLst></prstGeom></spPr>"#
        );
        let document = Document::parse(&xml).unwrap();
        preset_paths(document.root_element())
            .unwrap()
            .0
            .remove(0)
            .remove(0)
    };
    let guides = r#"<gd name="adj1" fmla="val 25000"/><gd name="adj2" fmla="val 80000"/>"#;
    let right = outline("rightArrow", 4000, 2000, guides);
    assert_eq!(
        right,
        [
            [0.0, 0.375],
            [0.6, 0.375],
            [0.6, 0.0],
            [1.0, 0.5],
            [0.6, 1.0],
            [0.6, 0.625],
            [0.0, 0.625]
        ]
    );
    let up = outline("upArrow", 2000, 4000, guides);
    assert_eq!(
        up,
        [
            [0.375, 1.0],
            [0.375, 0.4],
            [0.0, 0.4],
            [0.5, 0.0],
            [1.0, 0.4],
            [0.625, 0.4],
            [0.625, 1.0]
        ]
    );
    let double = outline("leftRightArrow", 1000, 2000, guides);
    assert_eq!((double[1], double[4]), ([0.5, 0.0], [0.5, 0.0]));
    let outer = r#"<gd name="adj" fmla="val 25000"/>"#;
    assert_eq!(outline("chevron", 4000, 2000, outer)[5], [0.125, 0.5]);
    let octagon = outline("octagon", 4000, 2000, outer);
    assert_eq!((octagon[0], octagon[1]), ([0.0, 0.25], [0.125, 0.0]));
}

fn shape_extent(ext: Node<'_, '_>, axis: &str) -> Option<f64> {
    let size = ext.attribute(axis)?.parse::<f64>().ok()?;
    (size.is_finite() && size > 0.0).then_some(size)
}

const DRAWINGML_TURN: f64 = 360.0 * 60_000.0;

fn builtin_guide(name: &str, [w, h]: [f64; 2]) -> Option<f64> {
    let short = w.min(h);
    let value = match name {
        "l" | "t" => 0.0,
        "w" | "r" => w,
        "h" | "b" => h,
        "ss" => short,
        "ls" => w.max(h),
        "hc" => w / 2.0,
        "vc" => h / 2.0,
        "cd2" => DRAWINGML_TURN / 2.0,
        "cd4" => DRAWINGML_TURN / 4.0,
        "cd8" => DRAWINGML_TURN / 8.0,
        "3cd4" => DRAWINGML_TURN * 3.0 / 4.0,
        "3cd8" => DRAWINGML_TURN * 3.0 / 8.0,
        "5cd8" => DRAWINGML_TURN * 5.0 / 8.0,
        "7cd8" => DRAWINGML_TURN * 7.0 / 8.0,
        _ => {
            let (base, divisor) = name.split_once('d')?;
            let extent = match base {
                "w" => w,
                "h" => h,
                "ss" => short,
                _ => return None,
            };
            extent / f64::from(divisor.parse::<std::num::NonZeroU32>().ok()?.get())
        }
    };
    Some(value)
}

fn guide_value(name: &str, guides: &HashMap<&str, Option<f64>>, size: [f64; 2]) -> Option<f64> {
    if let Ok(value) = name.parse::<f64>() {
        return value.is_finite().then_some(value);
    }
    match guides.get(name) {
        Some(value) => *value,
        None => builtin_guide(name, size),
    }
}

fn guide_formula(formula: &str, value: impl Fn(&str) -> Option<f64>) -> Option<f64> {
    let mut fields = formula.split_ascii_whitespace();
    let op = fields.next()?;
    let x = value(fields.next()?)?;
    let y = fields.next();
    let z = fields.next();
    if fields.next().is_some() {
        return None;
    }
    let angle = |units: f64| (units / 60_000.0).to_radians();
    let result = match (op, y, z) {
        ("val", None, None) => x,
        ("abs", None, None) => x.abs(),
        ("sqrt", None, None) => x.sqrt(),
        ("?:", Some(y), Some(z)) => value(if x > 0.0 { y } else { z })?,
        (op, Some(y), None) => {
            let y = value(y)?;
            match op {
                "min" => x.min(y),
                "max" => x.max(y),
                "at2" => y.atan2(x).to_degrees() * 60_000.0,
                "cos" => x * angle(y).cos(),
                "sin" => x * angle(y).sin(),
                "tan" => x * angle(y).tan(),
                _ => return None,
            }
        }
        (op, Some(y), Some(z)) => {
            let (y, z) = (value(y)?, value(z)?);
            match op {
                "*/" => x * y / z,
                "+-" => x + y - z,
                "+/" => (x + y) / z,
                "cat2" => x * z.atan2(y).cos(),
                "sat2" => x * z.atan2(y).sin(),
                "mod" => x.hypot(y).hypot(z),
                "pin" => y.min(z).max(x),
                _ => return None,
            }
        }
        _ => return None,
    };
    result.is_finite().then_some(result)
}

fn custom_paths(properties: Node<'_, '_>) -> Option<ShadedPaths> {
    let geometry = child(properties, "custGeom")?;
    let ext = child(properties, "xfrm").and_then(|xfrm| child(xfrm, "ext"));
    let mut paths = Vec::new();
    let mut shades = Vec::new();
    let mut vertices = 0;
    let mut guide_budget = 16_384;
    for path in child(geometry, "pathLst")?
        .children()
        .filter(Node::is_element)
    {
        if !path.has_tag_name("path") {
            return None;
        }
        let shade = match path.attribute("fill") {
            Some("none") => continue,
            None | Some("norm") => 0,
            Some("lighten") => 40,
            Some("lightenLess") => 20,
            Some("darken") => -40,
            Some("darkenLess") => -20,
            _ => return None,
        };
        let contours = custom_path(path, geometry, ext, &mut guide_budget)?;
        if contours.is_empty() {
            continue;
        }
        vertices += contours.iter().map(Vec::len).sum::<usize>();
        if vertices > 1024 {
            return None;
        }
        paths.push(contours);
        shades.push(shade);
    }
    Some((paths, shades))
}

fn custom_path(
    path: Node<'_, '_>,
    geometry: Node<'_, '_>,
    ext: Option<Node<'_, '_>>,
    guide_budget: &mut usize,
) -> Option<CustomPath> {
    let dimension = |key, axis| match path.attribute(key).unwrap_or("0").parse::<i64>().ok()? {
        0 => shape_extent(ext?, axis),
        n if n > 0 => Some(n as f64),
        _ => None,
    };
    let (width, height) = (dimension("w", "cx")?, dimension("h", "cy")?);
    let size = [width, height];
    let mut guides = HashMap::new();
    for list in ["avLst", "gdLst"]
        .into_iter()
        .filter_map(|name| child(geometry, name))
    {
        for guide in list.children().filter(|node| node.has_tag_name("gd")) {
            // shortcut: bound per-shape guide work until evaluations are cached by path size.
            *guide_budget = guide_budget.checked_sub(1)?;
            if let Some(name) = guide.attribute("name") {
                let result = guide.attribute("fmla").and_then(|formula| {
                    guide_formula(formula, |token| guide_value(token, &guides, size))
                });
                guides.insert(name, result);
            }
        }
    }
    let value = |name: &str| guide_value(name, &guides, size);
    let point = |node: Node<'_, '_>| {
        if !node.has_tag_name("pt") {
            return None;
        }
        let coordinate = |key, extent: f64| {
            let value = value(node.attribute(key)?)? / extent;
            value.is_finite().then_some(value)
        };
        Some([coordinate("x", width)?, coordinate("y", height)?])
    };
    let mut polygon = Vec::new();
    let mut contours = Vec::new();
    let mut remaining_vertices = 1024usize;
    let mut finish_contour = |polygon: &mut Vec<[f64; 2]>| {
        if polygon
            .first()
            .zip(polygon.last())
            .is_some_and(|(&first, &last)| same_path_point(first, last))
        {
            polygon.pop();
        }
        match polygon.len() {
            0..=2 => polygon.clear(),
            3..=128 => {
                remaining_vertices = remaining_vertices.checked_sub(polygon.len())?;
                contours.push(std::mem::take(polygon));
            }
            _ => return None,
        }
        Some(())
    };
    for command in path.children().filter(Node::is_element) {
        let mut points = command.children().filter(Node::is_element);
        match command.tag_name().name() {
            "moveTo" => {
                finish_contour(&mut polygon)?;
                push_path_vertex(&mut polygon, point(points.next()?)?)?;
            }
            "close" => {
                let start = *polygon.first()?;
                finish_contour(&mut polygon)?;
                polygon.push(start);
            }
            "lnTo" if !polygon.is_empty() => {
                push_path_vertex(&mut polygon, point(points.next()?)?)?;
            }
            "arcTo" => flatten_arc_path(&mut polygon, command, size, value)?,
            "quadBezTo" | "cubicBezTo" => {
                let first = point(points.next()?)?;
                let second = point(points.next()?)?;
                let current = *polygon.last()?;
                let curve = if command.has_tag_name("quadBezTo") {
                    [
                        current,
                        lerp_path(current, first, 2.0 / 3.0),
                        lerp_path(second, first, 2.0 / 3.0),
                        second,
                    ]
                } else {
                    [current, first, second, point(points.next()?)?]
                };
                flatten_cubic_path(&mut polygon, curve, 0)?;
            }
            _ => return None,
        }
        if points.next().is_some() {
            return None;
        }
    }
    finish_contour(&mut polygon)?;
    Some(contours)
}

fn push_path_vertex(vertices: &mut Vec<[f64; 2]>, point: [f64; 2]) -> Option<()> {
    // Match the compositor's range for paths that extend beyond the item box.
    if !point.iter().all(|axis| (-1.0..=2.0).contains(axis)) {
        return None;
    }
    // Trigonometric endpoints can round just outside a cardinal edge.
    let point = point.map(|axis| {
        if (-1e-12..0.0).contains(&axis) {
            0.0
        } else if (1.0..=1.0 + 1e-12).contains(&axis) {
            1.0
        } else {
            axis
        }
    });
    if vertices
        .last()
        .is_some_and(|&last| same_path_point(last, point))
    {
        return Some(());
    }
    // Allow a repeated starting point until close removes it before checking the size.
    if vertices.len() > 128 {
        return None;
    }
    vertices.push(point);
    Some(())
}

fn same_path_point(a: [f64; 2], b: [f64; 2]) -> bool {
    a.into_iter().zip(b).all(|(a, b)| (a - b).abs() <= 1e-12)
}

const PATH_FLATNESS: f64 = 1.0 / 2048.0;

fn flatten_arc_path(
    vertices: &mut Vec<[f64; 2]>,
    command: Node<'_, '_>,
    [width, height]: [f64; 2],
    value: impl Fn(&str) -> Option<f64>,
) -> Option<()> {
    let number = |key| value(command.attribute(key)?);
    let (rx, ry) = (number("wR")?, number("hR")?);
    let start_angle = number("stAng")?.rem_euclid(DRAWINGML_TURN);
    let sweep = number("swAng")?;
    if rx < 0.0 || ry < 0.0 || sweep.abs() > DRAWINGML_TURN {
        return None;
    }
    if (rx == 0.0 && ry == 0.0) || sweep == 0.0 {
        return Some(());
    }
    if rx == 0.0 || ry == 0.0 {
        return None;
    }
    let radii = [rx, ry];
    let start = ellipse_ray_parameter(radii, start_angle);
    let span = ellipse_parameter_span(radii, start, start_angle, sweep);
    append_ellipse_arc(vertices, [rx / width, ry / height], start, span)
}

fn ellipse_parameter_span(radii: [f64; 2], parameter: f64, start: f64, sweep: f64) -> f64 {
    use std::f64::consts::TAU;

    let stop = ellipse_ray_parameter(radii, start + sweep);
    if sweep.abs() == DRAWINGML_TURN {
        TAU.copysign(sweep)
    } else if sweep > 0.0 {
        (stop - parameter).rem_euclid(TAU)
    } else {
        -(parameter - stop).rem_euclid(TAU)
    }
}

fn ellipse_ray_parameter([rx, ry]: [f64; 2], angle: f64) -> f64 {
    // DrawingML measures the ray from the ellipse center in path coordinates.
    let (sin, cos) = (angle * std::f64::consts::TAU / DRAWINGML_TURN).sin_cos();
    (rx * sin).atan2(ry * cos)
}

fn append_ellipse_arc(
    vertices: &mut Vec<[f64; 2]>,
    [rx, ry]: [f64; 2],
    start: f64,
    span: f64,
) -> Option<()> {
    let offset = |angle: f64| {
        let (sin, cos) = angle.sin_cos();
        [rx * cos, ry * sin]
    };
    let current @ [x, y] = *vertices.last()?;
    let [dx, dy] = offset(start);
    let (cx, cy) = (x - dx, y - dy);
    // For an ellipse the circular chord error scales by at most its largest radius.
    let max_step = 2.0 * (1.0 - PATH_FLATNESS / rx.max(ry)).clamp(0.0, 1.0).acos();
    let segments = (span.abs() / max_step).ceil().max(1.0) as usize;
    if segments > 129 - vertices.len() {
        return None;
    }
    for segment in 1..=segments {
        let [dx, dy] = offset(start + span * (segment as f64 / segments as f64));
        let point = if span.abs() == std::f64::consts::TAU && segment == segments {
            current
        } else {
            [cx + dx, cy + dy]
        };
        push_path_vertex(vertices, point)?;
    }
    Some(())
}

fn lerp_path([ax, ay]: [f64; 2], [bx, by]: [f64; 2], t: f64) -> [f64; 2] {
    [ax + (bx - ax) * t, ay + (by - ay) * t]
}

fn flatten_cubic_path(
    vertices: &mut Vec<[f64; 2]>,
    [a, b, c, d]: [[f64; 2]; 4],
    depth: u8,
) -> Option<()> {
    // Compare against the control points of a straight segment in the same parameter space.
    // The Bézier stays within the convex hull of the four point-wise differences.
    let near = |control: [f64; 2], chord: [f64; 2]| {
        control
            .into_iter()
            .zip(chord)
            .all(|(p, q)| (p - q).abs() <= PATH_FLATNESS)
    };
    if near(b, lerp_path(a, d, 1.0 / 3.0)) && near(c, lerp_path(a, d, 2.0 / 3.0)) {
        return push_path_vertex(vertices, d);
    }
    if depth == 10 {
        return None;
    }
    let ab = lerp_path(a, b, 0.5);
    let bc = lerp_path(b, c, 0.5);
    let cd = lerp_path(c, d, 0.5);
    let abc = lerp_path(ab, bc, 0.5);
    let bcd = lerp_path(bc, cd, 0.5);
    let midpoint = lerp_path(abc, bcd, 0.5);
    flatten_cubic_path(vertices, [a, ab, abc, midpoint], depth + 1)?;
    flatten_cubic_path(vertices, [midpoint, bcd, cd, d], depth + 1)
}

#[cfg(test)]
#[test]
fn a_closed_bezier_outline_does_not_count_the_repeated_starting_point() {
    let xml = r#"<spPr><custGeom><pathLst><path w="10000" h="10000">
        <moveTo><pt x="5000" y="0"/></moveTo>
        <cubicBezTo><pt x="7761" y="0"/><pt x="10000" y="2239"/><pt x="10000" y="5000"/></cubicBezTo>
        <cubicBezTo><pt x="10000" y="7761"/><pt x="7761" y="10000"/><pt x="5000" y="10000"/></cubicBezTo>
        <cubicBezTo><pt x="2239" y="10000"/><pt x="0" y="7761"/><pt x="0" y="5000"/></cubicBezTo>
        <cubicBezTo><pt x="0" y="2239"/><pt x="2239" y="0"/><pt x="5000" y="0"/></cubicBezTo>
        <close/>
    </path></pathLst></custGeom></spPr>"#;
    let document = Document::parse(xml).unwrap();
    let (paths, _) = custom_paths(document.root_element()).unwrap();
    let vertices = &paths[0][0];
    assert_eq!(vertices.len(), 128);
    assert_eq!(vertices[0], [0.5, 0.0]);
    assert_ne!(vertices.first(), vertices.last());
}

#[cfg(test)]
#[test]
fn full_turn_arcs_close_without_a_repeated_vertex() {
    for sweep in [21_600_000, -21_600_000] {
        let xml = format!(
            r#"<spPr><custGeom><pathLst><path w="200" h="100">
            <moveTo><pt x="200" y="50"/></moveTo>
            <arcTo wR="100" hR="50" stAng="0" swAng="{sweep}"/><close/>
            </path></pathLst></custGeom></spPr>"#
        );
        let document = Document::parse(&xml).unwrap();
        let (paths, _) = custom_paths(document.root_element()).unwrap();
        let vertices = &paths[0][0];
        assert!((60..=128).contains(&vertices.len()));
        assert_eq!(vertices[0], [1.0, 0.5]);
        assert_ne!(vertices.first(), vertices.last());
        assert_eq!(vertices[1][1] > 0.5, sweep > 0);
        assert!(vertices.iter().any(|&[x, _]| x < 0.001));
    }
    let xml = r#"<spPr><custGeom><pathLst><path w="200" h="100">
        <moveTo><pt x="180" y="50"/></moveTo>
        <arcTo wR="80" hR="30" stAng="0" swAng="5400000"/>
        <arcTo wR="80" hR="30" stAng="5400000" swAng="5400000"/>
        <arcTo wR="80" hR="30" stAng="10800000" swAng="5400000"/>
        <arcTo wR="80" hR="30" stAng="16200000" swAng="5400000"/>
        <close/>
    </path></pathLst></custGeom></spPr>"#;
    let document = Document::parse(xml).unwrap();
    let (paths, _) = custom_paths(document.root_element()).unwrap();
    let vertices = &paths[0][0];
    assert_eq!(vertices.len() % 4, 0);
    assert!(!same_path_point(vertices[0], *vertices.last().unwrap()));
}

impl ThemeReader {
    pub(super) fn resolve(
        &mut self,
        archive: &mut Archive<'_>,
        slide: &str,
        xml: &str,
        style: Option<u32>,
        color: Option<&Color>,
    ) -> Result<Option<ResolvedFill>, DataError> {
        if matches!(style, Some(0 | 1000)) {
            return Ok(None);
        }
        let context = self.context(archive, slide, Some(xml), 0)?;
        if context.theme.is_none() {
            return Ok(None);
        }
        let scheme = |slot: &str| context.color_with_placeholder(slot, color);
        Ok(match style {
            Some(index) => context.style(index, color, GradientShape::Rectangle),
            None => color.and_then(|color| color.hex_in(scheme).map(ResolvedFill::Color)),
        })
    }

    pub(super) fn resolve_gradient(
        &mut self,
        archive: &mut Archive<'_>,
        slide: &str,
        xml: &str,
        fill: &GradientFill,
    ) -> Result<Option<ResolvedFill>, DataError> {
        let context = self.context(archive, slide, Some(xml), 0)?;
        Ok(fill.resolve(|slot| context.color(slot), GradientShape::Rectangle))
    }

    pub(super) fn object_styles(
        &mut self,
        archive: &mut Archive<'_>,
        slide: &str,
        xml: &str,
        size: &SlideSize,
    ) -> Result<HashMap<u64, ObjectStyle>, DataError> {
        if !xml.contains("spPr") {
            return Ok(HashMap::new());
        }
        let Ok(document) = Document::parse(xml) else {
            // Keep the streaming importer for older fragments with unbound prefixes.
            return Ok(HashMap::new());
        };
        let context = self.context(archive, slide, Some(xml), 0)?;
        let mut styles = HashMap::new();
        for shape in document.descendants().filter(|node| {
            (node.has_tag_name("sp") || node.has_tag_name("cxnSp"))
                && node.parent_element().is_some_and(|parent| {
                    parent.has_tag_name("spTree") || parent.has_tag_name("grpSp")
                })
        }) {
            let Some(properties) = child(shape, "spPr") else {
                continue;
            };
            let paths = custom_paths(properties).or_else(|| preset_paths(properties));
            let geometry = if paths
                .as_ref()
                .is_some_and(|(paths, _)| paths.len() == 1 && paths[0].len() == 1)
            {
                GradientShape::Polygon
            } else if child(properties, "prstGeom").and_then(|geom| geom.attribute("prst"))
                == Some("ellipse")
            {
                GradientShape::Ellipse
            } else {
                GradientShape::Rectangle
            };
            let reference = child(shape, "style").and_then(|style| child(style, "fillRef"));
            let placeholder = reference.and_then(Color::in_node);
            let scheme = |slot: &str| context.color_with_placeholder(slot, placeholder.as_ref());
            let direct = properties.children().filter(Node::is_element).find(|node| {
                matches!(
                    node.tag_name().name(),
                    "noFill" | "solidFill" | "gradFill" | "blipFill" | "pattFill" | "grpFill"
                )
            });
            let fill = match direct {
                Some(fill) if fill.has_tag_name("solidFill") => Color::in_node(fill)
                    .and_then(|color| color.hex_in(scheme))
                    .map(ResolvedFill::Color),
                Some(fill) if fill.has_tag_name("gradFill") => {
                    GradientFill::from_node(fill).and_then(|fill| fill.resolve(scheme, geometry))
                }
                Some(_) => None,
                None => reference
                    .and_then(|reference| reference.attribute("idx"))
                    .and_then(|index| index.parse().ok())
                    .and_then(|index| context.style(index, placeholder.as_ref(), geometry)),
            };
            let direction = connector_direction(shape, properties);
            let fill = match fill {
                _ if direction.is_some()
                    || paths.as_ref().is_some_and(|(paths, _)| paths.is_empty()) =>
                {
                    Some(String::new())
                }
                Some(ResolvedFill::Color(fill)) => Some(format!("background:{fill};")),
                Some(ResolvedFill::Gradient { style, .. }) => Some(style),
                None if direct.is_some() => Some(String::new()),
                None => None,
            };
            let text = TextBody::read(shape, &context, size);
            let connector = direction
                .and_then(|inverse| connector_stroke(shape, properties, &context, size, inverse));
            if fill.is_some() || text.is_some() {
                let (paths, path_shades) = paths.unwrap_or_default();
                styles.insert(
                    shape.range().start as u64,
                    ObjectStyle {
                        fill,
                        text,
                        paths,
                        path_shades,
                        connector,
                    },
                );
            }
        }
        Ok(styles)
    }

    fn load_xml(
        &mut self,
        archive: &mut Archive<'_>,
        part: &str,
    ) -> Result<Option<String>, DataError> {
        let xml = read_xml(archive, part)?;
        self.xml_bytes += xml.as_ref().map_or(0, String::len);
        if self.xml_bytes > 64 * 1024 * 1024 {
            return Err(invalid("expanded theme and layout XML exceeds 64 MiB"));
        }
        Ok(xml)
    }

    fn context(
        &mut self,
        archive: &mut Archive<'_>,
        part: &str,
        xml: Option<&str>,
        level: usize,
    ) -> Result<Context, DataError> {
        if let Some(context) = self.parents.get(part) {
            return Ok(context.clone());
        }
        let owned;
        let xml = if let Some(xml) = xml {
            xml
        } else {
            owned = self.load_xml(archive, part)?.unwrap_or_default();
            if owned.is_empty() {
                return Ok(Context::default());
            }
            &owned
        };
        let document = Document::parse(xml).map_err(invalid)?;
        let root = document.root_element();
        let kind = ["slideLayout", "slideMaster", "theme"][level];
        let parent = relationships(archive, part)?
            .into_iter()
            .find(|rel| rel.kind == kind);
        let mut context = match parent {
            Some(rel) if level < 2 => self.context(archive, &rel.target, None, level + 1)?,
            Some(rel) => {
                if !self.themes.contains_key(&rel.target) {
                    let theme = self
                        .load_xml(archive, &rel.target)?
                        .map(|xml| Theme::parse(&xml))
                        .transpose()?
                        .unwrap_or_default();
                    self.themes.insert(rel.target.clone(), Arc::new(theme));
                }
                Context {
                    theme: self.themes.get(&rel.target).cloned(),
                    ..Context::default()
                }
            }
            None => Context::default(),
        };
        if let Some(map) = child(root, "clrMap") {
            context.master = color_map(map);
            context.colors = context.master.clone();
        }
        if let Some(overrides) = child(root, "clrMapOvr") {
            if let Some(map) = child(overrides, "overrideClrMapping") {
                context.colors = color_map(map);
            } else if child(overrides, "masterClrMapping").is_some() {
                context.colors = context.master.clone();
            }
        }
        if level > 0 {
            self.parents.insert(part.into(), context.clone());
        }
        Ok(context)
    }
}

fn color_map(node: Node<'_, '_>) -> ColorMap {
    [
        "bg1", "bg2", "tx1", "tx2", "accent1", "accent2", "accent3", "accent4", "accent5",
        "accent6", "hlink", "folHlink",
    ]
    .into_iter()
    .filter_map(|role| Some((role.to_owned(), node.attribute(role)?.to_owned())))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(hex: &str, transforms: &[(&str, Option<&str>)]) -> String {
        let mut color = Color::parse("srgbClr", |_| Some(hex[1..].into())).unwrap();
        for (kind, value) in transforms {
            color.add_transform(kind, *value);
        }
        color.literal().unwrap()
    }

    #[test]
    fn remaining_color_transforms_apply_in_document_order() {
        assert_eq!(color("#FF0000", &[("inv", None)]), "#00FFFF");
        assert_eq!(color("#FF0000", &[("comp", None)]), "#00FFFF");
        assert_eq!(color("#FF0000", &[("gray", None)]), "#4C4C4C");
        assert_eq!(color("#FF0000", &[("satMod", Some("0"))]), "#808080");
        assert_eq!(color("#FF0000", &[("hueOff", Some("10800000"))]), "#00FFFF");
        assert_eq!(color("#808080", &[("redOff", Some("50%"))]), "#FF8080");
        assert_eq!(
            color("#FF0000", &[("unknown", Some("1")), ("inv", None)]),
            "#00FFFF"
        );
        assert_eq!(
            color("#808080", &[("gamma", None), ("alpha", Some("50000"))]),
            "#BCBCBC80"
        );
        assert_eq!(
            color("#808080", &[("gamma", None), ("invGamma", None)]),
            "#808080"
        );
        assert_eq!(
            color(
                "#FF0000",
                &[
                    ("alpha", Some("50%")),
                    ("alphaMod", Some("50000")),
                    ("alphaOff", Some("10000"))
                ]
            ),
            "#FF000059"
        );
    }
}
