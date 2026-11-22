// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, sync::Arc};

use roxmltree::{Document, Node};

use super::{Archive, invalid, read_xml, relationships};
use crate::DataError;

#[derive(Clone)]
pub(super) enum Color {
    Rgb(String),
    Scheme(String),
}

impl Color {
    pub(super) fn parse(kind: &str, attr: impl FnOnce(&str) -> Option<String>) -> Option<Self> {
        let value = attr(if kind == "sysClr" { "lastClr" } else { "val" })?;
        match kind {
            "schemeClr" => Some(Self::Scheme(value)),
            "sysClr" | "srgbClr"
                if value.len() == 6 && value.bytes().all(|ch| ch.is_ascii_hexdigit()) =>
            {
                Some(Self::Rgb(format!("#{value}")))
            }
            _ => None,
        }
    }

    fn in_node(node: Node<'_, '_>) -> Option<Self> {
        let color = node.children().find(Node::is_element)?;
        // shortcut: solid colors with transforms need DrawingML color math.
        if color.children().any(|node| node.is_element()) {
            return None;
        }
        Self::parse(color.tag_name().name(), |key| {
            color.attribute(key).map(str::to_owned)
        })
    }
}

type ColorMap = HashMap<String, String>;

#[derive(Default)]
struct Theme {
    colors: ColorMap,
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
                if let Some(Color::Rgb(color)) = Color::in_node(slot) {
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
        let resolve = |color: &Color| match color {
            Color::Rgb(rgb) => Some(rgb.clone()),
            Color::Scheme(slot) => {
                let key = context.colors.get(slot).map_or_else(
                    || match slot.as_str() {
                        "bg1" => "lt1",
                        "bg2" => "lt2",
                        "tx1" => "dk1",
                        "tx2" => "dk2",
                        slot => slot,
                    },
                    String::as_str,
                );
                theme.colors.get(key).cloned()
            }
        };
        Ok(match style {
            Some(index) => match theme.style(index) {
                Some(Color::Scheme(slot)) if slot == "phClr" => color.and_then(resolve),
                Some(fill) => resolve(fill),
                None => None,
            },
            None => color.and_then(resolve),
        })
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
