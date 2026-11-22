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
    Lightness { scale: f64, offset: f64 },
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
        let Some(percent) = value.and_then(percentage) else {
            return false;
        };
        let transform = match kind {
            "tint" if (0.0..=1.0).contains(&percent) => Transform::Tint(percent),
            "shade" if (0.0..=1.0).contains(&percent) => Transform::Shade(percent),
            "lum" => Transform::Lightness {
                scale: 0.0,
                offset: percent,
            },
            "lumMod" => Transform::Lightness {
                scale: percent,
                offset: 0.0,
            },
            "lumOff" => Transform::Lightness {
                scale: 1.0,
                offset: percent,
            },
            _ => return false,
        };
        self.transforms.push(transform);
        true
    }

    pub(super) fn literal(&self) -> Option<String> {
        self.resolve(|_| None).map(hex)
    }

    fn resolve(&self, scheme: impl FnOnce(&str) -> Option<Rgb>) -> Option<Rgb> {
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
            Self::Lightness { scale, offset } => {
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
        }
    }
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
    fills: Vec<Option<Color>>,
    backgrounds: Vec<Option<Color>>,
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
            theme.fills = solid_styles(format, "fillStyleLst");
            theme.backgrounds = solid_styles(format, "bgFillStyleLst");
        }
        Ok(theme)
    }

    fn style(&self, index: u32) -> Option<&Color> {
        let (list, offset) = match index {
            1..=999 => (&self.fills, 1),
            1001.. => (&self.backgrounds, 1001),
            _ => return None,
        };
        list.get(usize::try_from(index - offset).ok()?)?.as_ref()
    }
}

fn solid_styles(format: Node<'_, '_>, name: &str) -> Vec<Option<Color>> {
    child(format, name)
        .into_iter()
        .flat_map(|list| list.children().filter(Node::is_element))
        .map(|fill| {
            fill.has_tag_name("solidFill")
                .then(|| Color::in_node(fill))
                .flatten()
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
    ) -> Result<Option<String>, DataError> {
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
        let rgb = match style {
            Some(index) => theme.style(index).and_then(|fill| {
                fill.resolve(|slot| {
                    if slot == "phClr" {
                        color?.resolve(resolve)
                    } else {
                        resolve(slot)
                    }
                })
            }),
            None => color.and_then(|color| color.resolve(resolve)),
        };
        Ok(rgb.map(hex))
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
