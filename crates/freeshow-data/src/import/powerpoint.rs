// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use serde_json::json;
use zip::{ZipArchive, result::ZipError};

use super::{Section, attribute, file_stem_str, local_name, show_file};

use crate::{DataError, ShowFile};

mod theme;
use theme::{Color, GradientFill, ResolvedFill, ThemeReader};

const XML_LIMIT: u64 = 8 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
type Archive<'a> = ZipArchive<Cursor<&'a [u8]>>;

#[derive(Clone)]
enum Background {
    Color(String),
    Theme {
        style: Option<u32>,
        color: Option<Color>,
    },
    Picture {
        part: String,
        crop: [f64; 4],
        inset: [f64; 4],
        tile: bool,
        linked: bool,
    },
    Gradient(GradientFill),
    CssGradient {
        css: String,
        first: String,
    },
    Unsupported,
}

struct Relationship {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

pub(super) fn import(
    file_name: &str,
    bytes: &[u8],
    media: Option<&Path>,
) -> Result<ShowFile, DataError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() > 10_000 {
        return Err(invalid("too many entries"));
    }
    let mut unique = HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(invalid)?;
        if !unique.insert(file.name().to_owned()) {
            return Err(invalid("duplicate ZIP part"));
        }
    }
    let (parts, size) = slide_order(&mut archive)?;
    let height = size.height;
    if parts.is_empty() || parts.len() > 2000 {
        return Err(invalid("expected between 1 and 2000 slides"));
    }
    let mut sections = Vec::new();
    let mut fills = Vec::new();
    let mut hidden = Vec::new();
    let mut slide_xml_bytes = 0;
    let mut cached = HashMap::new();
    let mut themes = ThemeReader::default();
    let mut assets = BTreeMap::new();
    let mut image_bytes = 0_u64;
    let mut object_items: Vec<Vec<SlideObject>> = Vec::new();
    let mut import_warnings = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let xml = read_xml(&mut archive, part)?.ok_or_else(|| invalid("slide part is missing"))?;
        slide_xml_bytes += xml.len();
        if slide_xml_bytes > 64 * 1024 * 1024 {
            return Err(invalid("expanded slide XML exceeds 64 MiB"));
        }
        let mut content = slide_content(&xml, &size)?;
        let lines = std::mem::take(&mut content.lines);
        let disabled = content.hidden;
        let found = find_background(&mut archive, &mut cached, part, Some(&xml), 0)?;
        let fill = realize_background(&mut themes, &mut archive, part, &xml, found)?;
        if let Some(Background::Picture { part, linked, .. }) = &fill
            && media.is_some()
            && !linked
            && !assets.contains_key(part)
        {
            let mut file = archive.by_name(part).map_err(invalid)?;
            if file.size() > IMAGE_LIMIT || image_bytes + file.size() > 256 * 1024 * 1024 {
                return Err(invalid("embedded backgrounds exceed size limits"));
            }
            let mut data = Vec::new();
            file.by_ref().take(IMAGE_LIMIT + 1).read_to_end(&mut data)?;
            if data.len() as u64 != file.size() {
                return Err(invalid("embedded background is truncated"));
            }
            image_bytes += data.len() as u64;
            assets.insert(part.clone(), data);
        }
        let picture_ids = content.picture_ids.clone();
        let rels = relationships(&mut archive, part)?;
        for rid in picture_ids {
            if let Some(rel) = rels.iter().find(|rel| rel.id == rid && rel.kind == "image") {
                if !rel.external && media.is_some() && !assets.contains_key(&rel.target) {
                    let mut file = archive.by_name(&rel.target).map_err(invalid)?;
                    if file.size() > IMAGE_LIMIT || image_bytes + file.size() > 256 * 1024 * 1024 {
                        return Err(invalid("embedded pictures exceed size limits"));
                    }
                    let mut data = Vec::new();
                    file.by_ref().take(IMAGE_LIMIT + 1).read_to_end(&mut data)?;
                    if data.len() as u64 != file.size() {
                        return Err(invalid("embedded picture is truncated"));
                    }
                    image_bytes += data.len() as u64;
                    assets.insert(rel.target.clone(), data);
                }
                content.resolve_picture(rel);
            }
        }
        import_warnings.extend(
            content
                .warnings
                .iter()
                .map(|warning| format!("Slide {}: {warning}", index + 1)),
        );
        object_items.push(content.objects);
        sections.push(Section::plain(format!("Slide {}", index + 1), lines));
        fills.push(fill);
        hidden.push(disabled);
    }
    let mut files = HashMap::new();
    if let Some(directory) = media {
        for (part, bytes) in assets {
            files.insert(
                part.clone(),
                super::save_imported_media(directory, &part, &bytes)?,
            );
        }
    }
    let mut show = show_file(file_name, &file_stem_str(file_name), sections);
    show.show["origin"] = json!("powerpoint");
    for (index, fill) in fills.into_iter().enumerate() {
        let slide = &mut show.show["slides"][format!("s{}", index + 1)];
        slide["settings"]["resolution"] = json!({"width": 1920, "height": height});
        if !object_items[index].is_empty() {
            for object in &mut object_items[index] {
                if let Some(src) = object.src.as_ref()
                    && let Some(saved) = files.get(src)
                {
                    object.src = Some(saved.to_string_lossy().into_owned());
                }
            }
            let items: Vec<serde_json::Value> = object_items[index]
                .drain(..)
                .filter_map(SlideObject::into_item)
                .collect();
            slide["items"] = json!(items);
        } else {
            // The existing text importer uses one centered text box per slide.
            slide["items"][0]["style"] = json!(format!(
                "left:50px;top:{}px;width:1820px;height:{}px;text-align:center;font-size:60px;",
                height * 0.11,
                height * 0.78
            ));
        }
        match fill {
            Some(Background::Color(color)) => apply_solid_color(slide, &color)?,
            Some(Background::Picture {
                part,
                crop,
                inset,
                tile,
                linked,
            }) if linked || files.contains_key(&part) => {
                let src = if linked {
                    json!(part)
                } else {
                    json!(files[&part])
                };
                let [left, top, right, bottom] = inset;
                let style = format!(
                    "left:{}px;top:{}px;width:{}px;height:{}px;",
                    left * 19.2,
                    top * height / 100.0,
                    (100.0 - left - right) * 19.2,
                    (100.0 - top - bottom) * height / 100.0
                );
                // A slide-local image item lets a later slide return to its
                // own background, and preserves PowerPoint's stretch/crop.
                slide["items"].as_array_mut().unwrap().insert(0, json!({
                    "type":"media", "style":style, "fit":if tile { "tile" } else { "fill" }, "src":src,
                    "cropping":{"type":"ppt","left":crop[0],"top":crop[1],"right":crop[2],"bottom":crop[3]}
                }));
            }
            Some(Background::CssGradient { css, first }) => {
                apply_solid_color(slide, &first)?;
                slide["items"].as_array_mut().unwrap().insert(
                    0,
                    json!({
                        "type":"shape",
                        "style":format!("left:0px;top:0px;width:1920px;height:{height}px;background:{css};"),
                        "lines":[]
                    }),
                );
            }
            _ => {}
        }
        if hidden[index] {
            show.show["layouts"]["main"]["slides"][index]["disabled"] = json!(true);
        }
    }
    if !import_warnings.is_empty() {
        show.show["settings"]["importWarnings"] = json!(import_warnings);
    }
    Ok(show)
}

struct SlideSize {
    height: f64,
    emu_cx: f64,
    emu_cy: f64,
}

impl SlideSize {
    fn from_emu(cx: f64, cy: f64) -> Self {
        let height = if cx > 0.0 {
            let suggested = 1920.0 * cy / cx;
            if (240.0..=8192.0).contains(&suggested) {
                suggested
            } else {
                1080.0
            }
        } else {
            1080.0
        };
        Self {
            height,
            emu_cx: if cx > 0.0 { cx } else { 12_192_000.0 },
            emu_cy: if cy > 0.0 { cy } else { 6_858_000.0 },
        }
    }

    fn px_x(&self, emu: f64) -> f64 {
        1920.0 * emu / self.emu_cx
    }

    fn px_y(&self, emu: f64) -> f64 {
        self.height * emu / self.emu_cy
    }
}

fn slide_order(archive: &mut Archive<'_>) -> Result<(Vec<String>, SlideSize), DataError> {
    let part = "ppt/presentation.xml";
    let mut size = SlideSize::from_emu(12_192_000.0, 6_858_000.0);
    let mut ids = Vec::new();
    if let Some(xml) = read_xml(archive, part)? {
        elements(&xml, |name, element| {
            if name == "sldId" {
                if let Some(id) = relationship_attr(element, "id") {
                    ids.push(id);
                }
            } else if name == "sldSz"
                && let (Some(cx), Some(cy)) = (number(element, "cx"), number(element, "cy"))
            {
                size = SlideSize::from_emu(cx, cy);
            }
        })?;
    }
    if !ids.is_empty() {
        let relationships = relationships(archive, part)?;
        let order = ids
            .iter()
            .map(|id| {
                relationships
                    .iter()
                    .find(|rel| rel.id == *id && rel.kind == "slide")
                    .map(|rel| rel.target.clone())
                    .ok_or_else(|| invalid("slide relationship is missing"))
            })
            .collect::<Result<_, _>>()?;
        return Ok((order, size));
    }
    let mut order: Vec<_> = archive
        .file_names()
        .filter(|part| part.starts_with("ppt/slides/slide") && part.ends_with(".xml"))
        .map(str::to_owned)
        .collect();
    order.sort_by_key(|name| {
        name.trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    Ok((order, size))
}

fn slide_text(xml: &str) -> Result<(Vec<String>, bool), DataError> {
    let mut lines = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut capture = false;
    let mut paragraph = false;
    let mut hidden = false;
    let mut chunk = String::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) => match local_name(element.name().as_ref()).as_str() {
                "sld" => {
                    hidden = attribute(&element, "show")
                        .is_some_and(|value| value == "0" || value == "false");
                }
                "p" => paragraph = true,
                "t" => capture = true,
                "br" if paragraph => chunk.push('\n'),
                _ => {}
            },
            Event::Empty(element) if paragraph && local_name(element.name().as_ref()) == "br" => {
                chunk.push('\n');
            }
            Event::Text(text) if capture => chunk.push_str(&text.unescape().map_err(invalid)?),
            Event::End(element) => {
                let name = local_name(element.name().as_ref());
                if name == "t" {
                    capture = false;
                }
                if name == "p" || (name == "t" && !paragraph) {
                    lines.extend(chunk.trim().lines().map(str::to_owned));
                    chunk.clear();
                    paragraph = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((lines, hidden))
}

struct SlideContent {
    lines: Vec<String>,
    hidden: bool,
    objects: Vec<SlideObject>,
    warnings: Vec<String>,
    picture_ids: Vec<String>,
}

struct SlideObject {
    kind: ObjectKind,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    lines: Vec<String>,
    fill: Option<String>,
    ellipse: bool,
    radius: f64,
    crop: [f64; 4],
    tile: bool,
    picture_id: Option<String>,
    src: Option<String>,
    linked: bool,
}

enum ObjectKind {
    Text,
    Shape,
    Picture,
}

impl SlideContent {
    fn resolve_picture(&mut self, rel: &Relationship) {
        for object in &mut self.objects {
            if object.picture_id.as_deref() == Some(rel.id.as_str()) {
                object.src = Some(rel.target.clone());
                object.linked = rel.external;
            }
        }
    }
}

impl SlideObject {
    fn into_item(self) -> Option<serde_json::Value> {
        let mut style = format!(
            "left:{}px;top:{}px;width:{}px;height:{}px;",
            self.x.round(),
            self.y.round(),
            self.width.max(1.0).round(),
            self.height.max(1.0).round()
        );
        if self.ellipse {
            style.push_str("border-radius:50%;");
        } else if self.radius > 0.0 {
            style.push_str(&format!("border-radius:{}px;", self.radius.round()));
        }
        match self.kind {
            ObjectKind::Picture => {
                let src = self.src?;
                Some(json!({
                    "type": "media",
                    "style": style,
                    "fit": if self.tile { "tile" } else { "fill" },
                    "src": src,
                    "cropping": {
                        "type": "ppt",
                        "left": self.crop[0],
                        "top": self.crop[1],
                        "right": self.crop[2],
                        "bottom": self.crop[3]
                    }
                }))
            }
            ObjectKind::Shape => {
                if let Some(fill) = &self.fill {
                    style.push_str(&format!("background:{fill};"));
                }
                Some(json!({
                    "type": "shape",
                    "style": style,
                    "lines": []
                }))
            }
            ObjectKind::Text => {
                if let Some(fill) = &self.fill {
                    style.push_str(&format!("background:{fill};"));
                }
                style.push_str("text-align:center;font-size:60px;");
                let lines = self
                    .lines
                    .iter()
                    .map(|line| json!({ "align": "", "text": [{ "value": line, "style": "" }] }))
                    .collect::<Vec<_>>();
                Some(json!({
                    "style": style,
                    "lines": lines
                }))
            }
        }
    }
}

fn slide_content(xml: &str, size: &SlideSize) -> Result<SlideContent, DataError> {
    let (lines, hidden) = slide_text(xml)?;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::new();
    let mut in_tree = false;
    let mut objects = Vec::new();
    let mut warnings = Vec::new();
    let mut current: Option<SlideObject> = None;
    let mut text = String::new();
    let mut capture = false;
    let mut paragraph = false;
    let mut picture_ids = Vec::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) => {
                let name = local_name(element.name().as_ref());
                match (stack.last().map(String::as_str), name.as_str()) {
                    (_, "spTree") => in_tree = true,
                    (Some("spTree" | "grpSp"), "sp" | "pic" | "cxnSp") if in_tree => {
                        current = Some(SlideObject {
                            kind: if name == "pic" {
                                ObjectKind::Picture
                            } else {
                                ObjectKind::Shape
                            },
                            x: 0.0,
                            y: 0.0,
                            width: 0.0,
                            height: 0.0,
                            lines: Vec::new(),
                            fill: None,
                            ellipse: false,
                            radius: 0.0,
                            crop: [0.0; 4],
                            tile: false,
                            picture_id: None,
                            src: None,
                            linked: false,
                        });
                    }
                    (Some("spTree" | "grpSp"), "graphicFrame") if in_tree => {
                        warnings.push(
                            "tables and charts are not imported as slide objects".to_string(),
                        );
                    }
                    (Some("xfrm") | Some("off"), _) => {}
                    _ => {}
                }
                if let Some(object) = current.as_mut() {
                    match (stack.last().map(String::as_str), name.as_str()) {
                        (Some("xfrm"), "off") => {
                            object.x = size.px_x(number(&element, "x").unwrap_or(0.0));
                            object.y = size.px_y(number(&element, "y").unwrap_or(0.0));
                        }
                        (Some("xfrm"), "ext") => {
                            object.width = size.px_x(number(&element, "cx").unwrap_or(0.0));
                            object.height = size.px_y(number(&element, "cy").unwrap_or(0.0));
                        }
                        (_, "prstGeom") => {
                            match attribute(&element, "prst").as_deref() {
                                Some("ellipse") => object.ellipse = true,
                                Some("roundRect") => {
                                    object.radius =
                                        (object.width.min(object.height) * 0.125).max(8.0);
                                }
                                Some("rect" | "round1Rect" | "round2SameRect") | None => {}
                                Some(other) => {
                                    // Unrecognized presets still occupy their bounding box.
                                    let _ = other;
                                }
                            }
                        }
                        (Some("solidFill"), "srgbClr") => {
                            if let Some(val) = attribute(&element, "val")
                                && val.len() == 6
                            {
                                object.fill = Some(format!("#{val}"));
                            }
                        }
                        (Some("blipFill"), "blip") => {
                            if let Some(id) = relationship_attr(&element, "embed")
                                .or_else(|| relationship_attr(&element, "link"))
                            {
                                object.kind = ObjectKind::Picture;
                                object.picture_id = Some(id.clone());
                                if !picture_ids.contains(&id) {
                                    picture_ids.push(id);
                                }
                            }
                        }
                        (Some("blipFill"), "tile") => object.tile = true,
                        (Some("blipFill"), "srcRect") => object.crop = percentages(&element),
                        (_, "p") => paragraph = true,
                        (_, "t") => capture = true,
                        (_, "br") if paragraph => text.push('\n'),
                        _ => {}
                    }
                }
                stack.push(name);
            }
            Event::Text(value) if capture => {
                text.push_str(&value.unescape().map_err(invalid)?);
            }
            Event::End(element) => {
                let name = local_name(element.name().as_ref());
                if name == "t" {
                    capture = false;
                }
                if (name == "p" || (name == "t" && !paragraph)) && current.is_some() {
                    if let Some(object) = current.as_mut() {
                        object.lines.extend(text.trim().lines().map(str::to_owned));
                    }
                    text.clear();
                    paragraph = false;
                }
                if matches!(name.as_str(), "sp" | "pic" | "cxnSp")
                    && stack.last().map(String::as_str) == Some(name.as_str())
                    && let Some(mut object) = current.take()
                {
                    if matches!(object.kind, ObjectKind::Shape) && !object.lines.is_empty() {
                        object.kind = ObjectKind::Text;
                    }
                    if object.width > 0.0 || object.height > 0.0 || !object.lines.is_empty() {
                        objects.push(object);
                    }
                }
                if name == "spTree" {
                    in_tree = false;
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Keep the historic single-box fallback when the slide has no positioned objects.
    if objects.iter().all(|object| {
        object.x == 0.0 && object.y == 0.0 && object.width == 0.0 && object.height == 0.0
    }) {
        objects.clear();
    }
    Ok(SlideContent {
        lines,
        hidden,
        objects,
        warnings,
        picture_ids,
    })
}

fn find_background(
    archive: &mut Archive<'_>,
    cache: &mut HashMap<String, Option<Background>>,
    part: &str,
    xml: Option<&str>,
    level: usize,
) -> Result<Option<Background>, DataError> {
    if let Some(result) = cache.get(part) {
        return Ok(result.clone());
    }
    let owned;
    let xml = if let Some(xml) = xml {
        xml
    } else {
        owned =
            read_xml(archive, part)?.ok_or_else(|| invalid("layout or master part is missing"))?;
        &owned
    };
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::new();
    let mut depth = None;
    let mut color = None;
    let mut style = None;
    let mut picture = None;
    let mut crop = [0.0; 4];
    let mut inset = [0.0; 4];
    let mut tiled = false;
    let mut gradient_stops: Vec<(f64, Color)> = Vec::new();
    let mut gradient_angle = 90.0;
    let mut gs_pos = None;
    let mut gs_color = None;
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) => {
                let name = local_name(element.name().as_ref());
                if name == "bg" && stack.last().is_some_and(|parent| parent == "cSld") {
                    depth = Some(stack.len());
                } else if depth.is_some() {
                    match (stack.last().map(String::as_str), name.as_str()) {
                        (Some("bg"), "bgRef") => {
                            style = attribute(&element, "idx").and_then(|idx| idx.parse().ok());
                        }
                        (
                            Some("solidFill" | "bgRef" | "gs"),
                            "srgbClr" | "sysClr" | "schemeClr",
                        ) => {
                            let parsed = Color::parse(&name, |key| attribute(&element, key));
                            if stack.iter().any(|name| name == "gs") {
                                gs_color = parsed;
                            } else {
                                color = parsed;
                            }
                        }
                        (Some("srgbClr" | "sysClr" | "schemeClr"), _)
                            if stack.len() >= 2
                                && matches!(
                                    stack[stack.len() - 2].as_str(),
                                    "solidFill" | "bgRef" | "gs"
                                ) =>
                        {
                            let value = attribute(&element, "val");
                            let apply = |color: Option<Color>| {
                                color.and_then(|mut color| {
                                    color
                                        .add_transform(&name, value.as_deref())
                                        .then_some(color)
                                })
                            };
                            if stack.iter().any(|name| name == "gs") {
                                gs_color = apply(gs_color);
                            } else {
                                color = apply(color);
                            }
                        }
                        (Some("gsLst") | Some("gradFill"), "gs") => {
                            gs_pos = attribute(&element, "pos")
                                .and_then(|value| theme_percentage(&value));
                            gs_color = None;
                        }
                        (Some("gradFill"), "lin") => {
                            if let Some(raw) = attribute(&element, "ang")
                                && let Ok(value) = raw.parse::<f64>()
                                && value.is_finite()
                            {
                                gradient_angle = 90.0 - value / 60_000.0;
                            }
                        }
                        (Some("blipFill"), "blip") => {
                            picture = relationship_attr(&element, "embed")
                                .or_else(|| relationship_attr(&element, "link"));
                        }
                        (Some("blipFill"), "tile") => tiled = true,
                        (Some("blipFill"), "srcRect") => crop = percentages(&element),
                        (Some("stretch"), "fillRect") => inset = percentages(&element),
                        _ => {}
                    }
                }
                stack.push(name);
            }
            Event::End(_) => {
                if stack.last().map(String::as_str) == Some("gs")
                    && let (Some(pos), Some(color)) = (gs_pos.take(), gs_color.take())
                {
                    gradient_stops.push((pos.clamp(0.0, 1.0), color));
                }
                stack.pop();
                if depth == Some(stack.len()) {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let gradient = (gradient_stops.len() >= 2).then(|| {
        gradient_stops.sort_by(|left, right| left.0.total_cmp(&right.0));
        GradientFill {
            angle: gradient_angle,
            stops: gradient_stops,
        }
    });
    let fill = if let Some(id) = picture {
        relationships(archive, part)?
            .into_iter()
            .find(|rel| rel.id == id && rel.kind == "image")
            .filter(|rel| rel.external || image_part(&rel.target))
            .map(|rel| Background::Picture {
                part: rel.target,
                crop,
                inset,
                tile: tiled,
                linked: rel.external,
            })
            .or(Some(Background::Unsupported))
    } else if let Some(gradient) = gradient {
        Some(Background::Gradient(gradient))
    } else if style.is_none()
        && let Some(rgb) = color.as_ref().and_then(Color::literal)
    {
        Some(Background::Color(rgb))
    } else if style.is_some() || color.is_some() {
        Some(Background::Theme { style, color })
    } else if depth.is_some() {
        Some(Background::Unsupported)
    } else if level < 2 {
        let parent_kind = if level == 0 {
            "slideLayout"
        } else {
            "slideMaster"
        };
        if let Some(parent) = relationships(archive, part)?
            .into_iter()
            .find(|rel| rel.kind == parent_kind)
        {
            find_background(archive, cache, &parent.target, None, level + 1)?
        } else {
            None
        }
    } else {
        None
    };
    cache.insert(part.to_owned(), fill.clone());
    Ok(fill)
}

fn relationships(archive: &mut Archive<'_>, part: &str) -> Result<Vec<Relationship>, DataError> {
    let (directory, name) = part.rsplit_once('/').unwrap_or(("", part));
    let path = format!("{directory}/_rels/{name}.rels");
    let Some(xml) = read_xml(archive, &path)? else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut invalid_row = false;
    elements(&xml, |name, element| {
        if name != "Relationship" {
            return;
        }
        let external = attribute(element, "TargetMode").as_deref() == Some("External");
        let (Some(id), Some(kind), Some(target)) = (
            attribute(element, "Id"),
            attribute(element, "Type"),
            attribute(element, "Target"),
        ) else {
            invalid_row = true;
            return;
        };
        if !seen.insert(id.clone()) {
            invalid_row = true;
            return;
        }
        let kind = kind.rsplit('/').next().unwrap_or("").to_owned();
        if external {
            if kind == "image"
                && let Some(target) = remote_media_url(&target)
            {
                rows.push(Relationship {
                    id,
                    kind,
                    target,
                    external: true,
                });
            }
            return;
        }
        if let Some(target) = internal_target(directory, &target) {
            rows.push(Relationship {
                id,
                kind,
                target,
                external: false,
            });
        }
    })?;
    if invalid_row {
        return Err(invalid("malformed or duplicate relationship"));
    }
    Ok(rows)
}

fn internal_target(directory: &str, target: &str) -> Option<String> {
    let target = percent_encoding::percent_decode_str(target)
        .decode_utf8()
        .ok()?;
    if target.contains(['\\', ':', '?', '#', '\0']) || target.starts_with("//") {
        return None;
    }
    let mut parts: Vec<_> = if target.starts_with('/') {
        Vec::new()
    } else {
        directory
            .split('/')
            .filter(|part| !part.is_empty())
            .collect()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn read_xml(archive: &mut Archive<'_>, name: &str) -> Result<Option<String>, DataError> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(invalid(error)),
    };
    if file.size() > XML_LIMIT {
        return Err(invalid("XML part exceeds 8 MiB"));
    }
    let mut xml = String::new();
    file.by_ref().take(XML_LIMIT + 1).read_to_string(&mut xml)?;
    if xml.len() as u64 != file.size() {
        return Err(invalid("XML part is truncated"));
    }
    Ok(Some(xml))
}

fn elements(xml: &str, mut visit: impl FnMut(&str, &BytesStart<'_>)) -> Result<(), DataError> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) | Event::Empty(element) => {
                visit(&local_name(element.name().as_ref()), &element)
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn relationship_attr(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attr| {
        let key = std::str::from_utf8(attr.key.as_ref()).ok()?;
        let (_, local) = key.split_once(':')?;
        (local == name)
            .then(|| attr.unescape_value().ok().map(|value| value.into_owned()))
            .flatten()
    })
}

fn number(element: &BytesStart<'_>, key: &str) -> Option<f64> {
    attribute(element, key)?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn percentages(element: &BytesStart<'_>) -> [f64; 4] {
    let mut result = ["l", "t", "r", "b"]
        .map(|key| (number(element, key).unwrap_or(0.0) / 1000.0).clamp(0.0, 99.0));
    if result[0] + result[2] >= 100.0 {
        result[0] = 0.0;
        result[2] = 0.0;
    }
    if result[1] + result[3] >= 100.0 {
        result[1] = 0.0;
        result[3] = 0.0;
    }
    result
}

fn realize_background(
    themes: &mut ThemeReader,
    archive: &mut Archive<'_>,
    part: &str,
    xml: &str,
    fill: Option<Background>,
) -> Result<Option<Background>, DataError> {
    Ok(match fill {
        Some(Background::Theme { style, color }) => {
            match themes.resolve(archive, part, xml, style, color.as_ref())? {
                Some(ResolvedFill::Color(color)) => Some(Background::Color(color)),
                Some(ResolvedFill::Gradient { css, first }) => {
                    Some(Background::CssGradient { css, first })
                }
                None => None,
            }
        }
        Some(Background::Gradient(fill)) => {
            match themes.resolve_gradient(archive, part, xml, &fill)? {
                Some(ResolvedFill::Color(color)) => Some(Background::Color(color)),
                Some(ResolvedFill::Gradient { css, first }) => {
                    Some(Background::CssGradient { css, first })
                }
                None => None,
            }
        }
        fill => fill,
    })
}

fn apply_solid_color(slide: &mut serde_json::Value, color: &str) -> Result<(), DataError> {
    let channels = u32::from_str_radix(&color[1..], 16).map_err(invalid)?;
    let luminance =
        299 * ((channels >> 16) & 255) + 587 * ((channels >> 8) & 255) + 114 * (channels & 255);
    slide["settings"]["color"] = json!(color);
    if luminance >= 155_000 {
        let items = slide["items"]
            .as_array_mut()
            .ok_or_else(|| invalid("slide items"))?;
        let index = items
            .iter()
            .position(|item| {
                item["lines"]
                    .as_array()
                    .is_some_and(|lines| !lines.is_empty())
            })
            .unwrap_or(0);
        let style = items[index]["style"].as_str().unwrap_or("").to_string();
        if !style.contains("color:") {
            items[index]["style"] = json!(format!("{style}color:#000000;"));
        }
    }
    Ok(())
}

fn theme_percentage(value: &str) -> Option<f64> {
    let (number, divisor) = value
        .strip_suffix('%')
        .map_or((value, 100_000.0), |value| (value, 100.0));
    let parsed = number.parse::<f64>().ok()?;
    parsed.is_finite().then(|| parsed / divisor)
}

fn remote_media_url(target: &str) -> Option<String> {
    let target = target.trim();
    let scheme = target.split_once("://")?.0;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    if target.chars().any(|ch| matches!(ch, '\\' | '\n' | '\r')) {
        return None;
    }
    Some(target.to_string())
}

fn image_part(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        ["png", "jpg", "jpeg"]
            .iter()
            .any(|supported| ext.eq_ignore_ascii_case(supported))
    })
}

fn invalid(error: impl std::fmt::Display) -> DataError {
    DataError::Scripture(format!("PowerPoint: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn deck() -> Vec<u8> {
        let order = br#"<p:presentation xmlns:p="p" xmlns:q="r"><p:sldIdLst>
            <p:sldId id="258" q:id="rId3"/><p:sldId id="256" q:id="rId1"/>
            <p:sldId id="257" q:id="rId2"/></p:sldIdLst><p:sldSz cx="1000" cy="750"/></p:presentation>"#;
        let rels = br#"<Relationships>
            <Relationship Id="rId1" Type="o/slide" Target="slides/slide1.xml"/>
            <Relationship Id="rId2" Type="o/slide" Target="/ppt/slides/slide2.xml"/>
            <Relationship Id="rId3" Type="o/slide" Target="slides/slide3.xml"/></Relationships>"#;
        let slide1 = br#"<p:sld><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="EEEEFF"/></a:solidFill>
            </p:bgPr></p:bg><a:p><a:r><a:t>Solid </a:t></a:r><a:r><a:t>override</a:t></a:r></a:p></p:cSld></p:sld>"#;
        let slide2 = b"<p:sld show=\"0\"><p:cSld/></p:sld>";
        let slide3 = br#"<p:sld><p:cSld><a:p><a:r><a:t>Master</a:t></a:r>
            <a:br/><a:r><a:t>image</a:t></a:r></a:p></p:cSld></p:sld>"#;
        let layout = b"<p:sldLayout><p:cSld/></p:sldLayout>";
        let master = br#"<p:sldMaster><p:cSld><p:bg><p:bgPr><a:blipFill><a:blip r:embed="bg"/>
            <a:srcRect l="10000"/><a:stretch><a:fillRect b="25000"/></a:stretch>
            </a:blipFill></p:bgPr></p:bg></p:cSld></p:sldMaster>"#;
        let layout_rel = br#"<Relationships><Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;
        let master_rel = br#"<Relationships><Relationship Id="m" Type="o/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#;
        let media_rel = br#"<Relationships><Relationship Id="bg" Type="o/image" Target="../media/sky%20blue.png"/></Relationships>"#;

        archive(&[
            ("ppt/presentation.xml", order),
            ("ppt/_rels/presentation.xml.rels", rels),
            ("ppt/slides/slide1.xml", slide1),
            ("ppt/slides/slide2.xml", slide2),
            ("ppt/slides/slide3.xml", slide3),
            ("ppt/slides/_rels/slide1.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide2.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide3.xml.rels", layout_rel),
            ("ppt/slideLayouts/slideLayout1.xml", layout),
            ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", master_rel),
            ("ppt/slideMasters/slideMaster1.xml", master),
            ("ppt/slideMasters/_rels/slideMaster1.xml.rels", media_rel),
            (
                "ppt/media/sky blue.png",
                include_bytes!("../../tests/fixtures/lesson-cover.png"),
            ),
        ])
    }

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn solid_theme_fills_follow_the_slide_color_map_and_style_index() {
        let slides = [
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:clrMapOvr><a:overrideClrMapping bg1="accent2"/></p:clrMapOvr></p:sld>"#,
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:bgPr></p:bg></p:cSld></p:sld>"#,
            r#"<p:sld xmlns:p="p"><p:cSld><p:bg><p:bgRef idx="1"/></p:bg></p:cSld></p:sld>"#,
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgRef idx="1001"><a:srgbClr val="112233"/></p:bgRef></p:bg></p:cSld></p:sld>"#,
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgRef idx="1003"><a:srgbClr val="FF0000"><a:lumMod val="50000"/></a:srgbClr></p:bgRef></p:bg></p:cSld></p:sld>"#,
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FF0000"><a:tint val="50000"/></a:srgbClr></a:solidFill></p:bgPr></p:bg></p:cSld></p:sld>"#,
        ];
        let layout_rel = br#"<Relationships><Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;
        let files = archive(&[
            ("ppt/slides/slide1.xml", slides[0].as_bytes()),
            ("ppt/slides/slide2.xml", slides[1].as_bytes()),
            ("ppt/slides/slide3.xml", slides[2].as_bytes()),
            ("ppt/slides/slide4.xml", slides[3].as_bytes()),
            ("ppt/slides/slide5.xml", slides[4].as_bytes()),
            ("ppt/slides/slide6.xml", slides[5].as_bytes()),
            ("ppt/slides/slide7.xml", slides[6].as_bytes()),
            ("ppt/slides/_rels/slide1.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide2.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide3.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide4.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide5.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide6.xml.rels", layout_rel),
            ("ppt/slideLayouts/slideLayout1.xml", br#"<p:sldLayout xmlns:p="p" xmlns:a="a"><p:clrMapOvr><a:overrideClrMapping bg1="accent1"/></p:clrMapOvr></p:sldLayout>"#),
            ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", br#"<Relationships><Relationship Id="m" Type="o/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#),
            ("ppt/slideMasters/slideMaster1.xml", br#"<p:sldMaster xmlns:p="p" xmlns:a="a">
                <p:cSld><p:bg><p:bgRef idx="1002"><a:schemeClr val="bg1"/></p:bgRef></p:bg></p:cSld>
                <p:clrMap bg1="lt1"/>
            </p:sldMaster>"#),
            ("ppt/slideMasters/_rels/slideMaster1.xml.rels", br#"<Relationships><Relationship Id="t" Type="o/theme" Target="../theme/theme1.xml"/></Relationships>"#),
            ("ppt/theme/theme1.xml", br#"<a:theme xmlns:a="a"><a:themeElements>
                <a:clrScheme>
                    <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
                    <a:accent1><a:srgbClr val="CC0000"/></a:accent1>
                    <a:accent2><a:srgbClr val="0066AA"/></a:accent2>
                </a:clrScheme>
                <a:fmtScheme>
                    <a:fillStyleLst><a:solidFill><a:srgbClr val="FF0000"><a:shade val="50000"/></a:srgbClr></a:solidFill></a:fillStyleLst>
                    <a:bgFillStyleLst>
                        <a:gradFill/><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
                        <a:solidFill><a:schemeClr val="phClr"><a:lumOff val="50%"/></a:schemeClr></a:solidFill>
                    </a:bgFillStyleLst>
                </a:fmtScheme>
            </a:themeElements></a:theme>"#),
        ]);
        let show = import("Theme.pptx", &files, None).unwrap();
        let slides = &show.show["slides"];
        assert_eq!(slides["s1"]["settings"]["color"], "#0066AA");
        assert_eq!(slides["s2"]["settings"]["color"], "#FFFFFF");
        assert_eq!(slides["s3"]["settings"]["color"], "#CC0000");
        assert_eq!(slides["s4"]["settings"]["color"], "#BC0000");
        assert!(slides["s5"]["settings"]["color"].is_null());
        assert_eq!(slides["s6"]["settings"]["color"], "#FF8080");
        assert_eq!(slides["s7"]["settings"]["color"], "#FFBCBC");
    }

    #[test]
    fn library_import_resolves_master_backgrounds_and_deck_order() {
        let path = std::env::temp_dir().join(crate::new_id());
        let library = crate::Library::open(&path);
        fs::create_dir_all(library.imports_dir()).unwrap();
        fs::write(library.imports_dir().join("Sunday.pptx"), deck()).unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        let show = library.list_shows().unwrap().remove(0);
        let slides = show.ordered_slides();
        assert_eq!(slides[0].lines, ["Master", "image"]);
        assert_eq!(slides[1].lines, ["Solid override"]);
        assert!(slides[2].lines.is_empty());
        assert_eq!(show.step_enabled(1, 1), None);
        let first = &show.show["slides"]["s1"];
        let second = &show.show["slides"]["s2"];
        let third = &show.show["slides"]["s3"];
        assert_eq!(first["settings"]["resolution"]["height"], 1440.0);
        assert_eq!(second["settings"]["color"], "#EEEEFF");
        assert_eq!(second["items"].as_array().unwrap().len(), 1);
        assert!(
            second["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("color:#000000")
        );
        assert_eq!(first["items"][0]["fit"], "fill");
        assert_eq!(first["items"][0]["cropping"]["left"], 10.0);
        assert!(
            first["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("height:1080px")
        );
        assert_eq!(first["items"][0]["src"], third["items"][0]["src"]);
        let asset = Path::new(first["items"][0]["src"].as_str().unwrap());
        assert!(asset.starts_with(library.media_dir().canonicalize().unwrap()));
        assert_eq!(
            fs::read(asset).unwrap(),
            include_bytes!("../../tests/fixtures/lesson-cover.png")
        );
        assert_eq!(fs::read_dir(library.media_dir()).unwrap().count(), 1);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn linear_gradients_become_full_slide_shape_items() {
        let layout_rel = br#"<Relationships><Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;
        let files = archive(&[
            (
                "ppt/slides/slide1.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill>
                    <a:gsLst>
                        <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                        <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                    </a:gsLst>
                    <a:lin ang="0"/>
                </a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Local</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgRef idx="1001"><a:srgbClr val="00AA00"/></p:bgRef></p:bg>
                    <a:p><a:r><a:t>Theme</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            ("ppt/slides/_rels/slide1.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide2.xml.rels", layout_rel),
            ("ppt/slideLayouts/slideLayout1.xml", br#"<p:sldLayout xmlns:p="p"><p:cSld/></p:sldLayout>"#),
            (
                "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
                br#"<Relationships><Relationship Id="m" Type="o/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#,
            ),
            ("ppt/slideMasters/slideMaster1.xml", br#"<p:sldMaster xmlns:p="p"><p:cSld/><p:clrMap bg1="lt1"/></p:sldMaster>"#),
            (
                "ppt/slideMasters/_rels/slideMaster1.xml.rels",
                br#"<Relationships><Relationship Id="t" Type="o/theme" Target="../theme/theme1.xml"/></Relationships>"#,
            ),
            (
                "ppt/theme/theme1.xml",
                br#"<a:theme xmlns:a="a"><a:themeElements>
                    <a:clrScheme><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1></a:clrScheme>
                    <a:fmtScheme>
                        <a:fillStyleLst><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:fillStyleLst>
                        <a:bgFillStyleLst>
                            <a:gradFill>
                                <a:gsLst>
                                    <a:gs pos="0"><a:schemeClr val="phClr"/></a:gs>
                                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                                </a:gsLst>
                                <a:lin ang="0"/>
                            </a:gradFill>
                        </a:bgFillStyleLst>
                    </a:fmtScheme>
                </a:themeElements></a:theme>"#,
            ),
        ]);
        let show = import("Gradients.pptx", &files, None).unwrap();
        let first = &show.show["slides"]["s1"];
        let second = &show.show["slides"]["s2"];
        let gradient = "linear-gradient(90deg, #FF0000 0%, #0000FF 100%)";
        assert_eq!(first["settings"]["color"], "#FF0000");
        assert_eq!(first["items"][0]["type"], "shape");
        assert!(
            first["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains(gradient),
            "{}",
            first["items"][0]["style"]
        );
        assert_eq!(first["items"].as_array().unwrap().len(), 2);
        assert_eq!(second["settings"]["color"], "#00AA00");
        assert!(
            second["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("linear-gradient(90deg, #00AA00 0%, #0000FF 100%)"),
            "{}",
            second["items"][0]["style"]
        );
    }

    #[test]
    fn tiled_and_linked_media_become_slide_items() {
        let layout = br#"<p:sldLayout xmlns:p="p"><p:cSld/></p:sldLayout>"#;
        let layout_master = br#"<Relationships><Relationship Id="m" Type="o/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#;
        let master = br#"<p:sldMaster xmlns:p="p"><p:cSld/></p:sldMaster>"#;
        let files = archive(&[
            (
                "ppt/slides/slide1.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r"><p:cSld><p:bg><p:bgPr><a:blipFill>
                    <a:blip r:embed="bg"/><a:tile/>
                </a:blipFill></p:bgPr></p:bg></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r"><p:cSld><p:bg><p:bgPr><a:blipFill>
                    <a:blip r:link="bg"/>
                </a:blipFill></p:bgPr></p:bg></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide3.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r"><p:cSld><p:bg><p:bgPr><a:blipFill>
                    <a:blip r:link="bg"/>
                </a:blipFill></p:bgPr></p:bg></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                br#"<Relationships>
                    <Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
                    <Relationship Id="bg" Type="o/image" Target="../media/sky.png"/>
                </Relationships>"#,
            ),
            (
                "ppt/slides/_rels/slide2.xml.rels",
                br#"<Relationships>
                    <Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
                    <Relationship Id="bg" Type="o/image" Target="https://example.org/sky.png" TargetMode="External"/>
                </Relationships>"#,
            ),
            (
                "ppt/slides/_rels/slide3.xml.rels",
                br#"<Relationships>
                    <Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
                    <Relationship Id="bg" Type="o/image" Target="file:///etc/passwd" TargetMode="External"/>
                </Relationships>"#,
            ),
            ("ppt/slideLayouts/slideLayout1.xml", layout),
            ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", layout_master),
            ("ppt/slideMasters/slideMaster1.xml", master),
            (
                "ppt/media/sky.png",
                include_bytes!("../../tests/fixtures/lesson-cover.png"),
            ),
        ]);
        let path = std::env::temp_dir().join(crate::new_id());
        std::fs::create_dir_all(&path).unwrap();
        let show = import("Tiles.pptx", &files, Some(&path)).unwrap();
        let slides = &show.show["slides"];
        assert_eq!(slides["s1"]["items"][0]["fit"], "tile");
        assert!(Path::new(slides["s1"]["items"][0]["src"].as_str().unwrap()).starts_with(&path));
        assert_eq!(
            slides["s2"]["items"][0]["src"],
            "https://example.org/sky.png"
        );
        assert_eq!(slides["s2"]["items"][0]["fit"], "fill");
        assert_ne!(slides["s3"]["items"][0]["type"], "media");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rich_objects_keep_their_bounds_and_tables_warn() {
        let files = archive(&[
            (
                "ppt/presentation.xml",
                br#"<p:presentation xmlns:p="p" xmlns:q="r"><p:sldIdLst>
                    <p:sldId id="256" q:id="rId1"/></p:sldIdLst>
                    <p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                br#"<Relationships><Relationship Id="rId1" Type="o/slide" Target="slides/slide1.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r"><p:cSld><p:spTree>
                    <p:sp>
                        <p:spPr>
                            <a:xfrm><a:off x="914400" y="685800"/><a:ext cx="4572000" cy="1371600"/></a:xfrm>
                            <a:prstGeom prst="roundRect"/>
                            <a:solidFill><a:srgbClr val="00AA00"/></a:solidFill>
                        </p:spPr>
                        <p:txBody><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:pic>
                        <p:spPr>
                            <a:xfrm><a:off x="0" y="3429000"/><a:ext cx="3048000" cy="1714500"/></a:xfrm>
                        </p:spPr>
                        <p:blipFill><a:blip r:embed="img"/></p:blipFill>
                    </p:pic>
                    <p:graphicFrame>
                        <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"/></a:graphic>
                    </p:graphicFrame>
                </p:spTree></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                br#"<Relationships><Relationship Id="img" Type="o/image" Target="../media/sky.png"/></Relationships>"#,
            ),
            (
                "ppt/media/sky.png",
                include_bytes!("../../tests/fixtures/lesson-cover.png"),
            ),
        ]);
        let path = std::env::temp_dir().join(crate::new_id());
        std::fs::create_dir_all(&path).unwrap();
        let show = import("Objects.pptx", &files, Some(&path)).unwrap();
        let items = show.show["slides"]["s1"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0]["style"].as_str().unwrap().contains("left:144px"));
        assert!(items[0]["style"].as_str().unwrap().contains("top:108px"));
        assert!(
            items[0]["style"]
                .as_str()
                .unwrap()
                .contains("border-radius:")
        );
        assert!(
            items[0]["style"]
                .as_str()
                .unwrap()
                .contains("background:#00AA00")
        );
        assert_eq!(items[0]["lines"][0]["text"][0]["value"], "Hello");
        assert_eq!(items[1]["type"], "media");
        assert!(items[1]["style"].as_str().unwrap().contains("left:0px"));
        assert!(Path::new(items[1]["src"].as_str().unwrap()).starts_with(&path));
        let warnings = show.show["settings"]["importWarnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("tables and charts")),
            "{warnings:?}"
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn internal_relationships_cannot_address_the_filesystem_or_network() {
        for path in [
            "../../escape.png",
            "%2e%2e/%2e%2e/escape",
            "https://example.org/p.png",
            "C:/file.png",
            "//server/file.png",
            "..%5cescape.png",
        ] {
            assert_eq!(internal_target("ppt", path), None, "{path}");
        }
        assert_eq!(
            internal_target("ppt/slides", "../media/p%20q.png").as_deref(),
            Some("ppt/media/p q.png")
        );
        assert_eq!(
            remote_media_url("https://example.org/sky.png").as_deref(),
            Some("https://example.org/sky.png")
        );
        for path in [
            "file:///etc/passwd",
            "C:/file.png",
            "//server/file.png",
            "ftp://example.org/a.png",
        ] {
            assert_eq!(remote_media_url(path), None, "{path}");
        }
    }
}
