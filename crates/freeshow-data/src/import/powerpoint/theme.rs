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
            if !parsed.add_transform(transform.tag_name().name(), transform.attribute("val")) {
                return None;
            }
        }
        Some(parsed)
    }

    pub(super) fn add_transform(&mut self, kind: &str, value: Option<&str>) -> bool {
        let Some(transform) = parse_transform(kind, value) else {
            return true;
        };
        self.transforms.push(transform);
        true
    }

    pub(super) fn literal(&self) -> Option<String> {
        self.hex_in(|_| None)
    }

    pub(super) fn hex_in(&self, scheme: impl Fn(&str) -> Option<Rgb>) -> Option<String> {
        self.resolve(scheme).map(hex)
    }

    fn resolve(&self, scheme: impl Fn(&str) -> Option<Rgb>) -> Option<Rgb> {
        let mut rgb = match &self.value {
            ColorValue::Rgb(rgb) => *rgb,
            ColorValue::Scheme(slot) => scheme(slot)?,
        };
        for transform in &self.transforms {
            rgb = transform.apply(rgb);
        }
        Some(rgb)
    }
}

/// A resolved slide fill: a solid hex or a CSS linear-gradient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResolvedFill {
    Color(String),
    Gradient { css: String, first: String },
}

#[derive(Clone)]
pub(super) struct GradientFill {
    /// CSS degrees: 0 is upward, 90 is to the right.
    pub angle: f64,
    /// Positions in 0..=1 and DrawingML colors, which may still be scheme slots.
    pub stops: Vec<(f64, Color)>,
}

impl GradientFill {
    pub(super) fn from_node(fill: roxmltree::Node<'_, '_>) -> Option<Self> {
        let mut angle = 90.0;
        if let Some(line) = child(fill, "lin")
            && let Some(raw) = line.attribute("ang")
            && let Ok(value) = raw.parse::<f64>()
            && value.is_finite()
        {
            // DrawingML 0 is left-to-right, counter-clockwise; CSS 0 is up, clockwise.
            angle = 90.0 - value / 60_000.0;
        }
        let mut stops = Vec::new();
        if let Some(list) = child(fill, "gsLst") {
            for stop in list.children().filter(|node| node.has_tag_name("gs")) {
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
        if stops.len() < 2 {
            return None;
        }
        stops.sort_by(|left, right| left.0.total_cmp(&right.0));
        Some(Self { angle, stops })
    }

    pub(super) fn resolve(&self, scheme: impl Fn(&str) -> Option<Rgb>) -> Option<ResolvedFill> {
        let mut resolved = Vec::new();
        for (pos, color) in &self.stops {
            resolved.push((*pos, color.hex_in(&scheme)?));
        }
        Some(css_gradient(self.angle, &resolved))
    }
}

pub(super) fn css_gradient(angle: f64, stops: &[(f64, String)]) -> ResolvedFill {
    let first = stops[0].1.clone();
    let body = stops
        .iter()
        .map(|(pos, color)| format!("{color} {}%", (pos * 100.0).round()))
        .collect::<Vec<_>>()
        .join(", ");
    ResolvedFill::Gradient {
        css: format!("linear-gradient({}deg, {body})", angle.round()),
        first,
    }
}

fn percentage(value: &str) -> Option<f64> {
    let (number, divisor) = value
        .strip_suffix('%')
        .map_or((value, 100_000.0), |value| (value, 100.0));
    let parsed = number.parse::<f64>().ok()?;
    parsed.is_finite().then(|| parsed / divisor)
}

fn hex(rgb: Rgb) -> String {
    let [r, g, b] = rgb.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02X}{g:02X}{b:02X}")
}

impl Transform {
    fn apply(self, rgb: Rgb) -> Rgb {
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
        }
    }
}

fn parse_transform(kind: &str, value: Option<&str>) -> Option<Transform> {
    match kind {
        "inv" | "invClr" => Some(Transform::Invert),
        "gray" | "grayClr" => Some(Transform::Gray),
        "comp" => Some(Transform::Complement),
        "alpha" | "alphaMod" | "alphaOff" | "gamma" | "invGamma" => None,
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
    colors: HashMap<String, Rgb>,
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
            assert!(color.add_transform(kind, *value));
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
        assert_eq!(color("#FF0000", &[("alpha", Some("50000"))]), "#FF0000");
    }
}
