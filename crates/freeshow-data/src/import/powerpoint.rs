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
use theme::{Color, ThemeReader};

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
    },
    Unsupported,
}

struct Relationship {
    id: String,
    kind: String,
    target: String,
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
    let (parts, height) = slide_order(&mut archive)?;
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
    for (index, part) in parts.iter().enumerate() {
        let xml = read_xml(&mut archive, part)?.ok_or_else(|| invalid("slide part is missing"))?;
        slide_xml_bytes += xml.len();
        if slide_xml_bytes > 64 * 1024 * 1024 {
            return Err(invalid("expanded slide XML exceeds 64 MiB"));
        }
        let (lines, disabled) = slide_text(&xml)?;
        let fill = match find_background(&mut archive, &mut cached, part, Some(&xml), 0)? {
            Some(Background::Theme { style, color }) => themes
                .resolve(&mut archive, part, &xml, style, color.as_ref())?
                .map(Background::Color),
            fill => fill,
        };
        if let Some(Background::Picture { part, .. }) = &fill
            && media.is_some()
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
        // The existing text importer uses one centered text box per slide.
        slide["items"][0]["style"] = json!(format!(
            "left:50px;top:{}px;width:1820px;height:{}px;text-align:center;font-size:60px;",
            height * 0.11,
            height * 0.78
        ));
        match fill {
            Some(Background::Color(color)) => {
                let channels = u32::from_str_radix(&color[1..], 16).map_err(invalid)?;
                let luminance = 299 * ((channels >> 16) & 255)
                    + 587 * ((channels >> 8) & 255)
                    + 114 * (channels & 255);
                slide["settings"]["color"] = json!(color);
                if luminance >= 155_000 {
                    let style = slide["items"][0]["style"].as_str().unwrap();
                    slide["items"][0]["style"] = json!(format!("{style}color:#000000;"));
                }
            }
            Some(Background::Picture { part, crop, inset }) if files.contains_key(&part) => {
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
                    "type":"media", "style":style, "fit":"fill", "src":files[&part],
                    "cropping":{"type":"ppt","left":crop[0],"top":crop[1],"right":crop[2],"bottom":crop[3]}
                }));
            }
            _ => {}
        }
        if hidden[index] {
            show.show["layouts"]["main"]["slides"][index]["disabled"] = json!(true);
        }
    }
    Ok(show)
}

fn slide_order(archive: &mut Archive<'_>) -> Result<(Vec<String>, f64), DataError> {
    let part = "ppt/presentation.xml";
    let mut height = 1080.0;
    let mut ids = Vec::new();
    if let Some(xml) = read_xml(archive, part)? {
        elements(&xml, |name, element| {
            if name == "sldId" {
                if let Some(id) = relationship_attr(element, "id") {
                    ids.push(id);
                }
            } else if name == "sldSz"
                && let (Some(cx), Some(cy)) = (number(element, "cx"), number(element, "cy"))
                && let suggested = 1920.0 * cy / cx
                && (240.0..=8192.0).contains(&suggested)
            {
                height = suggested;
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
        return Ok((order, height));
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
    Ok((order, height))
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
                        (Some("solidFill" | "bgRef"), "srgbClr" | "sysClr" | "schemeClr") => {
                            color = Color::parse(&name, |key| attribute(&element, key));
                        }
                        (Some("schemeClr"), _)
                            if stack.len() >= 2
                                && matches!(
                                    stack[stack.len() - 2].as_str(),
                                    "solidFill" | "bgRef"
                                ) =>
                        {
                            color = None;
                        }
                        (Some("blipFill"), "blip") => {
                            picture = relationship_attr(&element, "embed")
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
                stack.pop();
                if depth == Some(stack.len()) {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let fill = if let Some(id) = picture.filter(|_| !tiled) {
        relationships(archive, part)?
            .into_iter()
            .find(|rel| rel.id == id && rel.kind == "image")
            .filter(|rel| {
                Path::new(&rel.target).extension().is_some_and(|ext| {
                    ["png", "jpg", "jpeg"]
                        .iter()
                        .any(|supported| ext.eq_ignore_ascii_case(supported))
                })
            })
            .map(|rel| Background::Picture {
                part: rel.target,
                crop,
                inset,
            })
            .or(Some(Background::Unsupported))
    } else if style.is_some() || matches!(color, Some(Color::Scheme(_))) {
        Some(Background::Theme { style, color })
    } else if let Some(Color::Rgb(color)) = color {
        Some(Background::Color(color))
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
        if name != "Relationship" || attribute(element, "TargetMode").as_deref() == Some("External")
        {
            return;
        }
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
        if let Some(target) = internal_target(directory, &target) {
            rows.push(Relationship { id, kind, target });
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
        ];
        let layout_rel = br#"<Relationships><Relationship Id="l" Type="o/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;
        let files = archive(&[
            ("ppt/slides/slide1.xml", slides[0].as_bytes()),
            ("ppt/slides/slide2.xml", slides[1].as_bytes()),
            ("ppt/slides/slide3.xml", slides[2].as_bytes()),
            ("ppt/slides/slide4.xml", slides[3].as_bytes()),
            ("ppt/slides/slide5.xml", slides[4].as_bytes()),
            ("ppt/slides/_rels/slide1.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide2.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide3.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide4.xml.rels", layout_rel),
            ("ppt/slides/_rels/slide5.xml.rels", layout_rel),
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
                    <a:fillStyleLst><a:solidFill><a:srgbClr val="FF8800"/></a:solidFill></a:fillStyleLst>
                    <a:bgFillStyleLst><a:gradFill/><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>
                </a:fmtScheme>
            </a:themeElements></a:theme>"#),
        ]);
        let show = import("Theme.pptx", &files, None).unwrap();
        let slides = &show.show["slides"];
        assert_eq!(slides["s1"]["settings"]["color"], "#0066AA");
        assert_eq!(slides["s2"]["settings"]["color"], "#FFFFFF");
        assert_eq!(slides["s3"]["settings"]["color"], "#CC0000");
        assert_eq!(slides["s4"]["settings"]["color"], "#FF8800");
        assert!(slides["s5"]["settings"]["color"].is_null());
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
    }
}
