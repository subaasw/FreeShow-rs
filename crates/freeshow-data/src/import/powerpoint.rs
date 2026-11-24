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

mod numbering;
mod text;
mod theme;
use theme::{Color, GradientFill, ObjectStyle, ResolvedFill, ThemeReader};

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
        style: String,
        first: String,
        transparent: bool,
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
    let mut speaker_notes = Vec::new();
    let mut notes_xml_bytes = 0;
    let mut import_warnings = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let xml = read_xml(&mut archive, part)?.ok_or_else(|| invalid("slide part is missing"))?;
        slide_xml_bytes += xml.len();
        if slide_xml_bytes > 64 * 1024 * 1024 {
            return Err(invalid("expanded slide XML exceeds 64 MiB"));
        }
        let object_styles = themes.object_styles(&mut archive, part, &xml, &size)?;
        let mut content = slide_content(&xml, &size, object_styles)?;
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
        let notes = rels
            .iter()
            .find(|rel| rel.kind == "notesSlide" && !rel.external);
        speaker_notes.push(notes.and_then(|rel| {
            match read_speaker_notes(&mut archive, &rel.target, &mut notes_xml_bytes) {
                Ok(notes) => Some(notes),
                Err(error) => {
                    import_warnings.push(format!(
                        "Slide {}: Could not import speaker notes: {error}",
                        index + 1
                    ));
                    None
                }
            }
        }));
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
    for (index, (fill, notes)) in fills.into_iter().zip(speaker_notes).enumerate() {
        let slide = &mut show.show["slides"][format!("s{}", index + 1)];
        if let Some(notes) = notes {
            slide["notes"] = json!(notes);
        }
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
            Some(Background::CssGradient {
                style,
                first,
                transparent,
            }) => {
                apply_solid_color(slide, &first)?;
                if transparent {
                    slide["settings"]["color"] = json!("#00000000");
                }
                slide["items"].as_array_mut().unwrap().insert(
                    0,
                    json!({
                        "type":"shape",
                        "style":format!("left:0px;top:0px;width:1920px;height:{height}px;{style}"),
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

fn read_speaker_notes(
    archive: &mut Archive<'_>,
    part: &str,
    expanded_bytes: &mut usize,
) -> Result<String, DataError> {
    use theme::child;

    const NOTES_LIMIT: usize = 64 * 1024 * 1024;
    if *expanded_bytes >= NOTES_LIMIT {
        return Err(invalid("expanded notes XML exceeds 64 MiB"));
    }
    let xml = read_xml(archive, part)?.ok_or_else(|| invalid("notes part is missing"))?;
    *expanded_bytes += xml.len();
    if *expanded_bytes > NOTES_LIMIT {
        return Err(invalid("expanded notes XML exceeds 64 MiB"));
    }
    let document = roxmltree::Document::parse(&xml).map_err(invalid)?;
    let root = document.root_element();
    if !root.has_tag_name("notes") {
        return Err(invalid("expected a notes slide"));
    }
    let Some(tree) = child(root, "cSld").and_then(|slide| child(slide, "spTree")) else {
        return Ok(String::new());
    };
    let paragraphs = tree
        .descendants()
        .filter(|node| node.has_tag_name("sp"))
        .filter_map(|shape| {
            let nonvisual = child(child(shape, "nvSpPr")?, "nvPr")?;
            let placeholder = child(nonvisual, "ph")?;
            if placeholder.attribute("type") != Some("body") {
                return None;
            }
            child(shape, "txBody")
        })
        .flat_map(|body| body.children().filter(|node| node.has_tag_name("p")))
        .map(|paragraph| {
            let mut line = String::new();
            for el in paragraph.children().filter(roxmltree::Node::is_element) {
                match el.tag_name().name() {
                    "br" => line.push('\n'),
                    "r" | "fld" => {
                        if let Some(text) = child(el, "t").and_then(|node| node.text()) {
                            line.push_str(text);
                        }
                    }
                    _ => {}
                }
            }
            line
        })
        .collect::<Vec<_>>();
    Ok(paragraphs.join("\n"))
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
    rotation: f64,
    flip_x: bool,
    flip_y: bool,
    group_matrix: Option<[f64; 4]>,
    lines: Vec<String>,
    style: ObjectStyle,
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

#[derive(Clone, Copy)]
struct GroupTransform {
    // Columns of the linear transform, followed separately by its translation.
    matrix: [f64; 4],
    offset: [f64; 2],
}

impl GroupTransform {
    const IDENTITY: Self = Self {
        matrix: [1.0, 0.0, 0.0, 1.0],
        offset: [0.0, 0.0],
    };

    fn vector(self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d] = self.matrix;
        [a * x + c * y, b * x + d * y]
    }

    fn point(self, xy: [f64; 2]) -> [f64; 2] {
        let [x, y] = self.vector(xy);
        [x + self.offset[0], y + self.offset[1]]
    }

    fn compose(self, child: Self) -> Self {
        let [a, b, c, d] = child.matrix;
        let [a, b] = self.vector([a, b]);
        let [c, d] = self.vector([c, d]);
        Self {
            matrix: [a, b, c, d],
            offset: self.point(child.offset),
        }
    }
}

#[derive(Default)]
struct GroupBounds {
    origin: [f64; 2],
    size: [f64; 2],
    child_origin: [f64; 2],
    child_size: [f64; 2],
    rotation: f64,
    flips: [bool; 2],
}

impl GroupBounds {
    fn transform(&self, slide: &SlideSize) -> GroupTransform {
        let scale = |axis: usize| {
            let child = self.child_size[axis];
            let scale = if child > 0.0 {
                self.size[axis] / child
            } else {
                1.0
            };
            if self.flips[axis] { -scale } else { scale }
        };
        let (sx, sy) = (scale(0), scale(1));
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let ratio = slide.px_x(1.0) / slide.px_y(1.0);
        let mut transform = GroupTransform {
            matrix: [sx * cos, sx * sin / ratio, -sy * sin * ratio, sy * cos],
            offset: [0.0, 0.0],
        };
        let [child_x, child_y] = transform.vector([
            self.child_origin[0] + self.child_size[0] / 2.0,
            self.child_origin[1] + self.child_size[1] / 2.0,
        ]);
        transform.offset = [
            self.origin[0] + self.size[0] / 2.0 - child_x,
            self.origin[1] + self.size[1] / 2.0 - child_y,
        ];
        transform
    }
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
    fn apply_group(&mut self, group: GroupTransform) {
        let [half_w, half_h] = [self.width / 2.0, self.height / 2.0];
        let [x, y] = group.point([self.x + half_w, self.y + half_h]);
        let [a, b, c, d] = group.matrix;
        let determinant = a * d - b * c;
        if ![x, y, determinant].iter().all(|value| value.is_finite()) || determinant == 0.0 {
            return;
        }
        self.x = x - half_w;
        self.y = y - half_h;
        if group.matrix != GroupTransform::IDENTITY.matrix {
            self.group_matrix = Some(group.matrix);
        }
    }

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
        if self.rotation != 0.0 {
            style.push_str(&format!("transform:rotate({}deg);", self.rotation));
        }
        if let Some([a, b, c, d]) = self.group_matrix {
            style.push_str(&format!("--pptx-group-matrix:{a} {b} {c} {d};"));
        }
        if !matches!(self.kind, ObjectKind::Picture) {
            if let Some(fill) = &self.style.fill {
                style.push_str(fill);
            }
            if self.flip_x {
                style.push_str("--pptx-fill-flip-x:1;");
            }
            if self.flip_y {
                style.push_str("--pptx-fill-flip-y:1;");
            }
        }
        match self.kind {
            ObjectKind::Picture => {
                let src = self.src?;
                Some(json!({
                    "type": "media",
                    "style": style,
                    "fit": if self.tile { "tile" } else { "fill" },
                    "flipped": self.flip_x,
                    "flippedY": self.flip_y,
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
            ObjectKind::Shape => Some(json!({
                "type": "shape",
                "style": style,
                "lines": []
            })),
            ObjectKind::Text => {
                let font_px = 60.0 * self.style.text.as_ref().map_or(1.0, |text| text.font_scale);
                style.push_str(&format!("text-align:center;font-size:{font_px}px;"));
                let (align, lines) = if let Some(text) = self.style.text {
                    let [left, top, right, bottom] = text.insets;
                    style.push_str(&format!(
                        "--pptx-text-insets:{left} {top} {right} {bottom};"
                    ));
                    if text.nowrap {
                        style.push_str("white-space:nowrap;");
                    }
                    (text.align, text.lines)
                } else {
                    (String::new(), self.lines.iter().map(|line| {
                        json!({ "align": "", "text": [{ "value": line, "style": "" }] })
                    }).collect())
                };
                Some(json!({
                    "style": style,
                    "align": align,
                    "lines": lines
                }))
            }
        }
    }
}

fn slide_content(
    xml: &str,
    size: &SlideSize,
    mut styles: HashMap<u64, ObjectStyle>,
) -> Result<SlideContent, DataError> {
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
    let mut groups = Vec::new();
    let mut group_bounds: Option<GroupBounds> = None;
    loop {
        let start = reader.buffer_position();
        match reader.read_event().map_err(invalid)? {
            Event::Start(element) => {
                let name = local_name(element.name().as_ref());
                match (stack.last().map(String::as_str), name.as_str()) {
                    (_, "spTree") => in_tree = true,
                    (Some("spTree" | "grpSp"), "grpSp") if in_tree => {
                        groups.push(groups.last().copied().unwrap_or(GroupTransform::IDENTITY));
                    }
                    (Some("grpSpPr"), "xfrm") if !groups.is_empty() => {
                        let (rotation, flip_x, flip_y) = object_transform(&element);
                        group_bounds = Some(GroupBounds {
                            rotation,
                            flips: [flip_x, flip_y],
                            ..GroupBounds::default()
                        });
                    }
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
                            rotation: 0.0,
                            flip_x: false,
                            flip_y: false,
                            group_matrix: None,
                            lines: Vec::new(),
                            style: styles.remove(&start).unwrap_or_default(),
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
                    _ => {}
                }
                if let Some(group) = group_bounds.as_mut() {
                    let coordinates = |x, y| {
                        [
                            size.px_x(number(&element, x).unwrap_or(0.0)),
                            size.px_y(number(&element, y).unwrap_or(0.0)),
                        ]
                    };
                    match name.as_str() {
                        "off" => group.origin = coordinates("x", "y"),
                        "ext" => group.size = coordinates("cx", "cy"),
                        "chOff" => group.child_origin = coordinates("x", "y"),
                        "chExt" => group.child_size = coordinates("cx", "cy"),
                        _ => {}
                    }
                }
                if let Some(object) = current.as_mut() {
                    match (stack.last().map(String::as_str), name.as_str()) {
                        (Some("spPr"), "xfrm") => {
                            (object.rotation, object.flip_x, object.flip_y) =
                                object_transform(&element);
                        }
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
                        (Some("solidFill"), "srgbClr")
                            if object.style.fill.is_none()
                                && stack.len() >= 2
                                && stack[stack.len() - 2] == "spPr" =>
                        {
                            if let Some(val) = attribute(&element, "val")
                                && val.len() == 6
                            {
                                object.style.fill = Some(format!("background:#{val};"));
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
                if name == "xfrm"
                    && let Some(bounds) = group_bounds.take()
                    && let Some(group) = groups.last_mut()
                {
                    *group = group.compose(bounds.transform(size));
                }
                if name == "grpSp"
                    && stack
                        .iter()
                        .rev()
                        .nth(1)
                        .is_some_and(|name| matches!(name.as_str(), "spTree" | "grpSp"))
                {
                    groups.pop();
                }
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
                    let has_text = object.style.text.is_some() || !object.lines.is_empty();
                    if matches!(object.kind, ObjectKind::Shape) && has_text {
                        object.kind = ObjectKind::Text;
                    }
                    if object.width > 0.0 || object.height > 0.0 || has_text {
                        if let Some(&group) = groups.last() {
                            object.apply_group(group);
                        }
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

fn object_transform(element: &BytesStart<'_>) -> (f64, bool, bool) {
    let rotation = (number(element, "rot").unwrap_or(0.0) / 60_000.0) % 360.0;
    let flipped = |key| matches!(attribute(element, key).as_deref(), Some("1" | "true"));
    (rotation, flipped("flipH"), flipped("flipV"))
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
    let mut gradient_stop_list = false;
    let mut gradient_stop_count = 0;
    let mut gradient_angle = 90.0;
    let mut gradient_scaled = false;
    let mut gradient_fixed = false;
    let mut gradient_tile = [0.0; 4];
    let mut gradient_path = None;
    let mut focus_insets = [0.5; 4];
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
                        (Some("bgPr"), "gradFill") => {
                            gradient_fixed = matches!(
                                attribute(&element, "rotWithShape").as_deref(),
                                Some("0" | "false")
                            );
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
                            let target = if stack[stack.len() - 2] == "gs" {
                                &mut gs_color
                            } else {
                                &mut color
                            };
                            if let Some(color) = target {
                                color.add_transform(&name, attribute(&element, "val").as_deref());
                            }
                        }
                        (Some("gradFill"), "gsLst") => gradient_stop_list = true,
                        (Some("gsLst") | Some("gradFill"), "gs") => {
                            gradient_stop_count += 1;
                            gs_pos = attribute(&element, "pos")
                                .and_then(|value| theme::percentage(&value));
                            gs_color = None;
                        }
                        (Some("gradFill"), "lin") => {
                            gradient_angle =
                                theme::linear_gradient_angle(attribute(&element, "ang").as_deref());
                            gradient_scaled = matches!(
                                attribute(&element, "scaled").as_deref(),
                                Some("1" | "true")
                            );
                        }
                        (Some("gradFill"), "tileRect") => {
                            gradient_tile = theme::gradient_insets(|key| {
                                attribute(&element, key).and_then(|value| theme::percentage(&value))
                            });
                        }
                        (Some("gradFill"), "path") => {
                            gradient_path =
                                Some(attribute(&element, "path").unwrap_or_else(|| "shape".into()));
                        }
                        (Some("path"), "fillToRect") if gradient_path.is_some() => {
                            focus_insets = theme::focus_insets(|key| {
                                attribute(&element, key).and_then(|value| theme::percentage(&value))
                            });
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
    let gradient = (gradient_stops.len() >= 2
        || (gradient_stop_list && gradient_stops.len() == gradient_stop_count))
        .then(|| {
            gradient_stops.sort_by(|left, right| left.0.total_cmp(&right.0));
            GradientFill {
                linear_angle: gradient_path.is_none().then_some(gradient_angle),
                linear_scaled: gradient_scaled,
                fixed_orientation: gradient_fixed,
                tile_insets: gradient_tile,
                focus: gradient_path
                    .as_deref()
                    .and_then(|path| theme::GradientFocus::from_path(path, focus_insets)),
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
                Some(ResolvedFill::Gradient {
                    style,
                    first,
                    transparent,
                }) => Some(Background::CssGradient {
                    style,
                    first,
                    transparent,
                }),
                None => None,
            }
        }
        Some(Background::Gradient(fill)) => {
            match themes.resolve_gradient(archive, part, xml, &fill)? {
                Some(ResolvedFill::Color(color)) => Some(Background::Color(color)),
                Some(ResolvedFill::Gradient {
                    style,
                    first,
                    transparent,
                }) => Some(Background::CssGradient {
                    style,
                    first,
                    transparent,
                }),
                None => None,
            }
        }
        fill => fill,
    })
}

fn apply_solid_color(slide: &mut serde_json::Value, color: &str) -> Result<(), DataError> {
    let channels = u32::from_str_radix(&color[1..7], 16).map_err(invalid)?;
    let alpha = if color.len() == 9 {
        u32::from_str_radix(&color[7..], 16).map_err(invalid)?
    } else {
        255
    };
    let luminance =
        299 * ((channels >> 16) & 255) + 587 * ((channels >> 8) & 255) + 114 * (channels & 255);
    slide["settings"]["color"] = json!(color);
    if luminance * alpha >= 155_000 * 255 {
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
    fn speaker_notes_follow_slide_relationships_into_the_stage_model() {
        let files = archive(&[
            ("ppt/presentation.xml", br#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst>
                <p:sldId r:id="first"/><p:sldId r:id="second"/><p:sldId r:id="third"/>
            </p:sldIdLst></p:presentation>"#),
            ("ppt/_rels/presentation.xml.rels", br#"<Relationships>
                <Relationship Id="first" Type="o/slide" Target="slides/slide7.xml"/>
                <Relationship Id="second" Type="o/slide" Target="slides/slide2.xml"/>
                <Relationship Id="third" Type="o/slide" Target="slides/slide3.xml"/>
            </Relationships>"#),
            ("ppt/slides/slide7.xml", br#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>First slide</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/slide2.xml", br#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>Second slide</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/slide3.xml", br#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>Third slide</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/_rels/slide7.xml.rels", br#"<Relationships><Relationship Id="not1" Type="o/notesSlide" Target="../notesSlides/notesSlide99.xml"/></Relationships>"#),
            ("ppt/slides/_rels/slide2.xml.rels", br#"<Relationships><Relationship Id="not2" Type="o/notesSlide" Target="../notesSlides/missing.xml"/></Relationships>"#),
            ("ppt/slides/_rels/slide3.xml.rels", br#"<Relationships><Relationship Id="not3" Type="o/notesSlide" TargetMode="External" Target="https://example.invalid/notes.xml"/></Relationships>"#),
            ("ppt/notesSlides/notesSlide99.xml", br#"<p:notes xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
                <p:sp><p:nvSpPr><p:nvPr><p:ph type="sldNum"/></p:nvPr></p:nvSpPr>
                    <p:txBody><a:p><a:fld><a:t>7</a:t></a:fld></a:p></p:txBody></p:sp>
                <p:sp><p:nvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
                    <p:txBody><a:p><a:r><a:t>Read &amp; </a:t></a:r><a:r><a:t>pause</a:t></a:r><a:br/>
                        <a:r><a:t>Wait</a:t></a:r></a:p><a:p/><a:p><a:r><a:t>Continue.</a:t></a:r></a:p></p:txBody></p:sp>
            </p:spTree></p:cSld></p:notes>"#),
        ]);
        let show = import("Notes.pptx", &files, None).unwrap();
        let slides = show.ordered_slides();
        assert_eq!(slides[0].lines, ["First slide"]);
        assert_eq!(slides[0].notes, "Read & pause\nWait\n\nContinue.");
        assert_eq!(slides[1].lines, ["Second slide"]);
        assert!(slides[1].notes.is_empty());
        assert!(slides[2].notes.is_empty());
        let warnings = show.show["settings"]["importWarnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .as_str()
                .unwrap()
                .starts_with("Slide 2: Could not import speaker notes")
        );
    }

    #[test]
    fn gradient_stop_precision_and_degenerate_fills_survive_import() {
        let files = archive(&[
            (
                "ppt/slides/slide1.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst>
                <a:gs pos="50125"><a:srgbClr val="FF0000"/></a:gs>
                <a:gs pos="50.375%"><a:srgbClr val="0000FF"/></a:gs>
                </a:gsLst></a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Band</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst/></a:gradFill></p:bgPr></p:bg>
                <a:p><a:r><a:t>Empty</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide3.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst>
                <a:gs pos="35000"><a:srgbClr val="334455"><a:alpha val="50000"/></a:srgbClr></a:gs>
                </a:gsLst></a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Solid</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
        ]);
        let show = import("Stops.pptx", &files, None).unwrap();
        let slides = &show.show["slides"];
        let style = slides["s1"]["items"][0]["style"].as_str().unwrap();
        assert!(
            style.contains("#FF0000 50.125%, #0000FF 50.375%"),
            "{style}"
        );
        assert!(!style.contains("--pptx-gradient-gamma"));
        assert_eq!(slides["s2"]["settings"]["color"], "#000000");
        assert_eq!(slides["s3"]["settings"]["color"], "#33445580");
    }

    #[test]
    fn gradients_become_full_slide_shape_items() {
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
            (
                "ppt/slides/slide3.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill>
                    <a:gsLst>
                        <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                        <a:gs pos="100000"><a:srgbClr val="0000FF"><a:alpha val="0"/></a:srgbClr></a:gs>
                    </a:gsLst>
                    <a:path path="circle"><a:fillToRect l="75000" t="50%" r="25000" b="50%"/></a:path>
                </a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Radial</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide4.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill>
                    <a:gsLst>
                        <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                        <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                    </a:gsLst>
                    <a:path><a:fillToRect l="30%" t="20000" r="10000" b="40000"/></a:path>
                </a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Rectangle</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide5.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgRef idx="1002"><a:srgbClr val="00AA00"/></p:bgRef></p:bg>
                    <a:p><a:r><a:t>Theme rectangle</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide6.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill>
                    <a:gsLst>
                        <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                        <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                    </a:gsLst>
                    <a:lin ang="2700000" scaled="true"/>
                    <a:tileRect l="25%" t="10000"/>
                </a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Scaled angle</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide7.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgRef idx="1003"><a:srgbClr val="FF0000"/></p:bgRef></p:bg>
                    <a:p><a:r><a:t>Theme scaled</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide8.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst>
                    <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                    </a:gsLst><a:path path="shape"/></a:gradFill></p:bgPr></p:bg>
                    <a:p><a:r><a:t>Shape path</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide9.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst>
                    <a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                    </a:gsLst><a:path path="circle"><a:fillToRect l="25000" t="25000" r="50000" b="25000"/></a:path>
                    </a:gradFill></p:bgPr></p:bg><a:p><a:r><a:t>Ellipse focus</a:t></a:r></a:p></p:cSld></p:sld>"#,
            ),
            ("ppt/slides/_rels/slide1.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide2.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide5.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide7.xml.rels", layout_rel),
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
                                <a:path path="circle"><a:fillToRect l="25000" t="75000" r="75000" b="25000"/></a:path>
                            </a:gradFill>
                            <a:gradFill>
                                <a:gsLst>
                                    <a:gs pos="0"><a:schemeClr val="phClr"/></a:gs>
                                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                                </a:gsLst>
                                <a:path path="rect"><a:fillToRect l="50000" r="50000"/></a:path>
                            </a:gradFill>
                            <a:gradFill>
                                <a:gsLst>
                                    <a:gs pos="0"><a:schemeClr val="phClr"/></a:gs>
                                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                                </a:gsLst>
                                <a:lin ang="2730000" scaled="1"/>
                                <a:tileRect l="-10000" r="60%"/>
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
        assert!(
            first["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-gradient-gamma:1;")
        );
        assert!(
            second["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-gradient-gamma:1;")
        );
        assert!(
            !first["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-linear-scaled")
        );
        for (slide, angle, tile) in [
            ("s6", "135", "0.25 0.1 0 0"),
            ("s7", "135.5", "-0.1 0 0.6 0"),
        ] {
            let style = show.show["slides"][slide]["items"][0]["style"]
                .as_str()
                .unwrap();
            assert!(
                style.contains(&format!(
                    "linear-gradient({angle}deg, #FF0000 0%, #0000FF 100%);--pptx-linear-scaled:1;"
                )),
                "{style}"
            );
            assert!(
                style.contains(&format!("--pptx-gradient-tile:{tile};")),
                "{style}"
            );
        }
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
                .contains("radial-gradient(circle at center, #00AA00 0%, #0000FF 100%)"),
            "{}",
            second["items"][0]["style"]
        );
        assert!(
            show.show["slides"]["s3"]["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("radial-gradient(circle at center, #FF0000 0%, #0000FF00 100%)")
        );
        assert_eq!(show.show["slides"]["s3"]["settings"]["color"], "#00000000");
        assert!(
            second["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-radial-focus:-0.50000 0.50000;")
        );
        assert!(
            show.show["slides"]["s3"]["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-radial-focus:0.50000 0.00000;")
        );
        for (slide, focus) in [
            ("s4", "0.30000 0.20000 0.10000 0.40000"),
            ("s5", "0.50000 0.50000 0.50000 0.50000"),
            ("s8", "0.50000 0.50000 0.50000 0.50000"),
        ] {
            assert!(
                show.show["slides"][slide]["items"][0]["style"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("--pptx-rectangular-focus:{focus};"))
            );
        }
        assert!(
            show.show["slides"]["s9"]["items"][0]["style"]
                .as_str()
                .unwrap()
                .contains("--pptx-radial-ellipse:-0.25 0 0.25 0.5;")
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
        assert!(
            Path::new(slides["s1"]["items"][0]["src"].as_str().unwrap())
                .starts_with(path.canonicalize().unwrap())
        );
        assert_eq!(
            slides["s2"]["items"][0]["src"],
            "https://example.org/sky.png"
        );
        assert_eq!(slides["s2"]["items"][0]["fit"], "fill");
        assert_ne!(slides["s3"]["items"][0]["type"], "media");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn nested_group_transforms_preserve_positions_and_item_order() {
        let files = archive(&[(
            "ppt/slides/slide1.xml",
            br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
                <p:grpSp><p:grpSpPr><a:xfrm rot="5400000" flipH="1">
                    <a:off x="635000" y="1270000"/><a:ext cx="2540000" cy="1270000"/>
                    <a:chOff x="63500" y="127000"/><a:chExt cx="1270000" cy="1270000"/>
                </a:xfrm></p:grpSpPr>
                <p:sp><p:spPr><a:xfrm rot="2700000">
                    <a:off x="127000" y="190500"/><a:ext cx="254000" cy="127000"/>
                </a:xfrm></p:spPr><p:txBody><a:p><a:r><a:t>Outer</a:t></a:r></a:p></p:txBody></p:sp>
                <p:grpSp><p:grpSpPr><a:xfrm flipV="true">
                    <a:off x="254000" y="381000"/><a:ext cx="635000" cy="635000"/>
                    <a:chOff x="0" y="0"/><a:chExt cx="317500" cy="635000"/>
                </a:xfrm></p:grpSpPr>
                <p:sp><p:spPr><a:xfrm>
                    <a:off x="31750" y="95250"/><a:ext cx="127000" cy="127000"/>
                </a:xfrm></p:spPr><p:txBody><a:p><a:r><a:t>Inner</a:t></a:r></a:p></p:txBody></p:sp>
                </p:grpSp></p:grpSp>
                <p:sp><p:spPr><a:xfrm>
                    <a:off x="63500" y="127000"/><a:ext cx="63500" cy="127000"/>
                </a:xfrm></p:spPr><p:txBody><a:p><a:r><a:t>Outside</a:t></a:r></a:p></p:txBody></p:sp>
            </p:spTree></p:cSld></p:sld>"#,
        )]);
        let show = import("Groups.pptx", &files, None).unwrap();
        let items = show.show["slides"]["s1"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        for (item, label, bounds, expected) in [
            (
                &items[0],
                "Outer",
                "left:360px;top:430px;width:40px;height:20px;",
                [0.0, -2.0, -1.0, 0.0],
            ),
            (
                &items[1],
                "Inner",
                "left:275px;top:370px;width:20px;height:20px;",
                [0.0, -4.0, 1.0, 0.0],
            ),
        ] {
            assert_eq!(item["lines"][0]["text"][0]["value"], label);
            let style = item["style"].as_str().unwrap();
            assert!(style.starts_with(bounds), "{style}");
            let matrix = style
                .split_once("--pptx-group-matrix:")
                .unwrap()
                .1
                .split_once(';')
                .unwrap()
                .0;
            let values = matrix
                .split_whitespace()
                .map(|part| part.parse::<f64>().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 4);
            for (actual, expected) in values.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-8, "{matrix}");
            }
        }
        assert!(
            items[0]["style"]
                .as_str()
                .unwrap()
                .contains("transform:rotate(45deg);")
        );
        assert_eq!(items[2]["lines"][0]["text"][0]["value"], "Outside");
        let outside = items[2]["style"].as_str().unwrap();
        assert!(outside.starts_with("left:10px;top:20px;"));
        assert!(!outside.contains("--pptx-group-matrix"));
    }

    #[test]
    fn positioned_shapes_keep_direct_and_theme_gradients() {
        let files = archive(&[
            (
                "ppt/slides/slide1.xml",
                br#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
                    <p:sp><p:spPr>
                        <a:xfrm rot="2700000" flipH="1"><a:off x="914400" y="685800"/><a:ext cx="4572000" cy="1371600"/></a:xfrm>
                        <a:prstGeom prst="rect"/>
                        <a:gradFill rotWithShape="false"><a:gsLst>
                            <a:gs pos="0"><a:schemeClr val="accent1"/></a:gs>
                            <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                        </a:gsLst><a:lin ang="2700000" scaled="1"/></a:gradFill>
                    </p:spPr><p:style><a:fillRef idx="1"><a:srgbClr val="FF0000"/></a:fillRef>
                        <a:fontRef idx="major"><a:schemeClr val="accent1"/></a:fontRef>
                    </p:style>
                    <p:txBody><a:bodyPr anchor="b" lIns="0" tIns="12700" rIns="25400" bIns="38100" wrap="none">
                        <a:normAutofit fontScale="75000" lnSpcReduction="20%"/>
                    </a:bodyPr><a:lstStyle>
                        <a:lvl1pPr algn="r" marL="127000" marR="63500"><a:spcBef><a:spcPct val="25000"/></a:spcBef>
                            <a:buClr><a:schemeClr val="accent1"/></a:buClr><a:buSzPct val="75000"/>
                            <a:buFont typeface="Symbol"/><a:buChar char="&#xf0b7;"/>
                            <a:defRPr sz="2400" b="1"><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr>
                    </a:lstStyle>
                    <a:p><a:pPr indent="-63500"><a:lnSpc><a:spcPct val="150%"/></a:lnSpc>
                        <a:spcAft><a:spcPts val="200"/></a:spcAft><a:defRPr i="true"/></a:pPr>
                        <a:r><a:t xml:space="preserve">Caption </a:t></a:r>
                        <a:r><a:rPr b="0" sz="1800" u="sng" strike="sngStrike" spc="50">
                            <a:latin typeface="+mj-lt"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>
                            <a:highlight><a:srgbClr val="FFFF00"/></a:highlight>
                        </a:rPr><a:t>accent</a:t></a:r>
                        <a:br/><a:fld id="field"><a:t>Cached field</a:t></a:fld>
                    </a:p><a:p/><a:p><a:pPr algn="ctr"><a:lnSpc><a:spcPts val="1000"/></a:lnSpc><a:buNone/></a:pPr><a:r><a:t>Final</a:t></a:r></a:p>
                    </p:txBody></p:sp>
                    <p:sp><p:spPr>
                        <a:xfrm><a:ext cx="4572000" cy="1371600"/></a:xfrm>
                        <a:prstGeom prst="ellipse"/>
                    </p:spPr><p:style><a:fillRef idx="1"><a:srgbClr val="FF0000"/></a:fillRef></p:style></p:sp>
                    <p:sp><p:spPr><a:xfrm><a:ext cx="4572000" cy="1371600"/></a:xfrm><a:noFill/></p:spPr>
                    <p:style><a:fillRef idx="1"><a:srgbClr val="FF0000"/></a:fillRef></p:style>
                    <p:txBody><a:p><a:r><a:rPr><a:solidFill><a:srgbClr val="F0F0F0"/></a:solidFill></a:rPr><a:t>Transparent</a:t></a:r></a:p></p:txBody></p:sp>
                </p:spTree></p:cSld></p:sld>"#,
            ),
            ("ppt/slides/_rels/slide1.xml.rels", br#"<Relationships><Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/layout.xml"/></Relationships>"#),
            ("ppt/slideLayouts/layout.xml", br#"<p:sldLayout xmlns:p="p"><p:cSld/></p:sldLayout>"#),
            ("ppt/slideLayouts/_rels/layout.xml.rels", br#"<Relationships><Relationship Id="m" Type="o/slideMaster" Target="../slideMasters/master.xml"/></Relationships>"#),
            ("ppt/slideMasters/master.xml", br#"<p:sldMaster xmlns:p="p"><p:cSld/></p:sldMaster>"#),
            ("ppt/slideMasters/_rels/master.xml.rels", br#"<Relationships><Relationship Id="t" Type="o/theme" Target="../theme/theme.xml"/></Relationships>"#),
            ("ppt/theme/theme.xml", br#"<a:theme xmlns:a="a"><a:themeElements>
                <a:clrScheme><a:accent1><a:srgbClr val="00AA00"/></a:accent1></a:clrScheme>
                <a:fontScheme><a:majorFont><a:latin typeface="Georgia"/></a:majorFont>
                    <a:minorFont><a:latin typeface="Arial"/></a:minorFont></a:fontScheme>
                <a:fmtScheme><a:fillStyleLst><a:gradFill rotWithShape="0"><a:gsLst>
                    <a:gs pos="0"><a:schemeClr val="phClr"/></a:gs>
                    <a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs>
                </a:gsLst><a:path path="shape"/></a:gradFill></a:fillStyleLst></a:fmtScheme>
            </a:themeElements></a:theme>"#),
        ]);
        let show = import("Shape gradients.pptx", &files, None).unwrap();
        let items = show.show["slides"]["s1"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        let direct = items[0]["style"].as_str().unwrap();
        assert!(
            direct.contains("background:linear-gradient(135deg, #00AA00 0%, #0000FF 100%)"),
            "{direct}"
        );
        assert!(direct.contains("--pptx-linear-scaled:1;"));
        assert!(direct.contains("--pptx-gradient-fixed:1;"));
        assert!(direct.contains("transform:rotate(45deg);"));
        assert!(direct.contains("--pptx-fill-flip-x:1;"));
        assert!(direct.contains("--pptx-text-insets:0 2 4 6;"));
        assert!(direct.contains("white-space:nowrap;"));
        assert!(direct.contains("font-size:45px;"));
        assert_eq!(items[0]["align"], "align-items:flex-end;");
        assert_eq!(items[0]["lines"][0]["align"], "right");
        assert_eq!(items[0]["lines"][0]["pptxMargins"], json!([20, 10, -10]));
        assert_eq!(items[0]["lines"][1]["pptxMargins"], json!([20, 10, 0]));
        assert_eq!(
            items[0]["lines"][0]["pptxBullet"],
            json!({"value":"•", "style":"color:#00AA00;--pptx-bullet-scale:75%;"})
        );
        for index in 1..4 {
            assert!(items[0]["lines"][index].get("pptxBullet").is_none());
        }
        assert_eq!(
            items[0]["lines"][0]["pptxSpacing"],
            json!(["130%", "25%", null])
        );
        assert_eq!(
            items[0]["lines"][1]["pptxSpacing"],
            json!(["130%", null, "4px"])
        );
        assert_eq!(
            items[0]["lines"][0]["text"][0],
            json!({
                "value": "Caption ",
                "style": "font-family:Arial;font-size:36px;color:#00AA00;font-weight:bold;font-style:italic;"
            })
        );
        assert_eq!(
            items[0]["lines"][0]["text"][1],
            json!({
                "value": "accent",
                "style": "font-family:Georgia;font-size:27px;color:#FF0000;background-color:#FFFF00;font-weight:normal;font-style:italic;text-decoration:underline line-through;letter-spacing:1px;"
            })
        );
        assert_eq!(items[0]["lines"][1]["align"], "right");
        assert_eq!(items[0]["lines"][1]["text"][0]["value"], "Cached field");
        assert_eq!(items[0]["lines"][2]["text"][0]["value"], " ");
        assert_eq!(items[0]["lines"][3]["align"], "center");
        assert_eq!(
            items[0]["lines"][3]["pptxSpacing"],
            json!(["20px", "25%", null])
        );
        assert_eq!(items[0]["lines"][3]["text"][0]["value"], "Final");
        let themed = items[1]["style"].as_str().unwrap();
        assert!(themed.contains("border-radius:50%;background:radial-gradient(circle at center, #FF0000 0%, #0000FF 100%)"), "{themed}");
        assert!(themed.contains("--pptx-shape-ellipse:1;"));
        assert!(themed.contains("--pptx-gradient-fixed:1;"));
        let transparent = items[2]["style"].as_str().unwrap();
        assert!(!transparent.contains("background:"));
        assert!(transparent.contains("--pptx-text-insets:14.4 7.2 14.4 7.2;"));
        assert!(!transparent.contains("white-space:nowrap;"));
        assert!(transparent.contains("font-size:60px;"));
    }

    #[test]
    fn numbered_lists_resume_outer_sequences_and_honor_saved_starts() {
        let files = archive(&[("ppt/slides/slide1.xml", br#"
            <p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:sp><p:spPr>
                <a:xfrm><a:ext cx="4572000" cy="1371600"/></a:xfrm>
            </p:spPr><p:txBody><a:bodyPr/><a:lstStyle>
                <a:lvl1pPr><a:buAutoNum type="arabicPeriod" startAt="3"/></a:lvl1pPr>
                <a:lvl2pPr><a:buAutoNum type="alphaLcParenR" startAt="27"/></a:lvl2pPr>
            </a:lstStyle>
                <a:p><a:r><a:t>Outer</a:t></a:r><a:br/><a:r><a:t>Continuation</a:t></a:r></a:p>
                <a:p><a:pPr lvl="1"/><a:r><a:t>Nested one</a:t></a:r></a:p>
                <a:p><a:pPr lvl="1"/><a:r><a:t>Nested two</a:t></a:r></a:p>
                <a:p><a:pPr><a:buAutoNum type="arabicPeriod" startAt="3"/></a:pPr><a:r><a:t>Outer again</a:t></a:r></a:p>
                <a:p><a:pPr lvl="1"/><a:r><a:t>Nested restart</a:t></a:r></a:p>
                <a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>Plain after nested</a:t></a:r></a:p>
                <a:p><a:r><a:t>Outer continues</a:t></a:r></a:p>
                <a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>Plain instead of outer</a:t></a:r></a:p>
                <a:p><a:r><a:t>Outer restarts</a:t></a:r></a:p>
                <a:p><a:pPr><a:buAutoNum type="romanUcParenBoth" startAt="9"/></a:pPr><a:r><a:t>Roman one</a:t></a:r></a:p>
                <a:p><a:pPr><a:buAutoNum type="romanUcParenBoth" startAt="9"/></a:pPr><a:r><a:t>Roman two</a:t></a:r></a:p>
            </p:txBody></p:sp></p:spTree></p:cSld></p:sld>
        "#)]);
        let show = import("Numbered lists.pptx", &files, None).unwrap();
        let lines = show.show["slides"]["s1"]["items"][0]["lines"]
            .as_array()
            .unwrap();
        let markers: Vec<_> = lines
            .iter()
            .map(|line| line["pptxBullet"]["value"].as_str())
            .collect();
        assert_eq!(
            markers,
            [
                Some("3."),
                None,
                Some("aa)"),
                Some("bb)"),
                Some("4."),
                Some("aa)"),
                None,
                Some("5."),
                None,
                Some("3."),
                Some("(IX)"),
                Some("(X)")
            ]
        );
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
                            <a:xfrm rot="-1350000"><a:off x="914400" y="685800"/><a:ext cx="4572000" cy="1371600"/></a:xfrm>
                            <a:prstGeom prst="roundRect"/>
                            <a:solidFill><a:srgbClr val="00AA00"/></a:solidFill>
                        </p:spPr>
                        <p:txBody><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:pic>
                        <p:spPr>
                            <a:xfrm rot="5400000" flipH="true" flipV="1"><a:off x="0" y="3429000"/><a:ext cx="3048000" cy="1714500"/></a:xfrm>
                            <a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>
                        </p:spPr>
                        <p:blipFill><a:blip r:embed="img"/><a:srcRect l="12500"/></p:blipFill>
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
                .contains("transform:rotate(-22.5deg);")
        );
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
        assert!(
            items[1]["style"]
                .as_str()
                .unwrap()
                .contains("transform:rotate(90deg);")
        );
        assert_eq!(items[1]["flipped"], true);
        assert_eq!(items[1]["flippedY"], true);
        assert_eq!(items[1]["cropping"]["left"], 12.5);
        assert!(!items[1]["style"].as_str().unwrap().contains("background:"));
        assert!(
            Path::new(items[1]["src"].as_str().unwrap()).starts_with(path.canonicalize().unwrap())
        );
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
