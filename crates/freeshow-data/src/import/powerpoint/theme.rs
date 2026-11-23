// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, sync::Arc};

use roxmltree::{Document, Node};

use super::{Archive, invalid, read_xml, relationships};
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
    pub focus: Option<GradientFocus>,
    pub tile_insets: [f64; 4],
    /// Positions in 0..=1 and DrawingML colors, which may still be scheme slots.
    pub stops: Vec<(f64, Color)>,
}

#[derive(Clone)]
pub(super) enum GradientFocus {
    RadialPoint([f64; 2]),
    Rectangle([f64; 4]),
}

impl GradientFocus {
    pub(super) fn from_path(path: &str, [left, top, right, bottom]: [f64; 4]) -> Option<Self> {
        match path {
            "circle" if (left + right - 1.0).abs() < 1e-9 && (top + bottom - 1.0).abs() < 1e-9 => {
                let (x, y) = (left - right, top - bottom);
                (x * x + y * y < 1.0 && (x != 0.0 || y != 0.0)).then_some(Self::RadialPoint([x, y]))
            }
            "rect"
                if [left, top, right, bottom].iter().all(|edge| *edge >= 0.0)
                    && left + right <= 1.0
                    && top + bottom <= 1.0 =>
            {
                Some(Self::Rectangle([left, top, right, bottom]))
            }
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
                path.attribute("path").unwrap_or("rect"),
                gradient_insets(|key| {
                    rect.and_then(|rect| rect.attribute(key))
                        .and_then(percentage)
                }),
            )
        });
        let tile = child(fill, "tileRect");
        Some(Self {
            linear_angle: path.is_none().then_some(angle),
            linear_scaled,
            focus,
            tile_insets: gradient_insets(|key| {
                tile.and_then(|tile| tile.attribute(key))
                    .and_then(percentage)
            }),
            stops,
        })
    }

    fn resolve(&self, scheme: impl Fn(&str) -> Option<ResolvedColor>) -> Option<ResolvedFill> {
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
            Some(GradientFocus::Rectangle([l, t, r, b])) => style.push_str(&format!(
                "--pptx-rectangular-focus:{l:.5} {t:.5} {r:.5} {b:.5};"
            )),
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
    fills: Vec<Option<StyleFill>>,
    backgrounds: Vec<Option<StyleFill>>,
}

#[derive(Clone)]
enum StyleFill {
    Solid(Color),
    Gradient(GradientFill),
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

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|node| node.has_tag_name(name))
}

#[derive(Clone, Default)]
struct Context {
    theme: Option<Arc<Theme>>,
    master: ColorMap,
    colors: ColorMap,
}

#[derive(Default)]
pub(super) struct ThemeReader {
    themes: HashMap<String, Arc<Theme>>,
    parents: HashMap<String, Context>,
    xml_bytes: usize,
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
        let Some(theme) = &context.theme else {
            return Ok(None);
        };
        let resolve = |slot: &str| {
            let key = context.colors.get(slot).map_or_else(
                || match slot {
                    "bg1" => "lt1",
                    "bg2" => "lt2",
                    "tx1" => "dk1",
                    "tx2" => "dk2",
                    slot => slot,
                },
                String::as_str,
            );
            theme.colors.get(key).copied()
        };
        let scheme = |slot: &str| {
            if slot == "phClr" {
                color.and_then(|color| color.resolve(resolve))
            } else {
                resolve(slot)
            }
        };
        Ok(match style {
            Some(index) => match theme.style(index) {
                Some(StyleFill::Solid(fill)) => fill.hex_in(scheme).map(ResolvedFill::Color),
                Some(StyleFill::Gradient(fill)) => fill.resolve(scheme),
                None => None,
            },
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
        let Some(theme) = &context.theme else {
            return Ok(fill.resolve(|_| None));
        };
        let resolve = |slot: &str| {
            let key = context.colors.get(slot).map_or_else(
                || match slot {
                    "bg1" => "lt1",
                    "bg2" => "lt2",
                    "tx1" => "dk1",
                    "tx2" => "dk2",
                    slot => slot,
                },
                String::as_str,
            );
            theme.colors.get(key).copied()
        };
        Ok(fill.resolve(resolve))
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
