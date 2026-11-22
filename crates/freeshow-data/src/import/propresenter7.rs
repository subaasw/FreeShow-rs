// SPDX-License-Identifier: GPL-3.0-only

mod wire;

use std::collections::{HashMap, HashSet};

use prost::Message;
use serde_json::{Map, Value, json};

use super::{ITEM_STYLE, decode_rtf, file_stem_str};
use crate::{DataError, ShowFile};

pub(super) fn decode(file_name: &str, bytes: &[u8]) -> Result<ShowFile, DataError> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid("presentation exceeds 16 MiB"));
    }
    let document = wire::Presentation::decode(bytes).map_err(invalid)?;
    let mut cues = HashMap::new();
    let mut cue_order = Vec::new();
    for cue in &document.cues {
        let Some(id) = id(&cue.uuid) else { continue };
        let action = cue
            .actions
            .iter()
            .find_map(|action| action.slide.as_ref()?.presentation.as_ref());
        if action.is_none() && background(cue).is_none() {
            continue;
        }
        if cues.insert(id, (cue, action)).is_some() {
            return Err(invalid("duplicate slide cue identifier"));
        }
        cue_order.push(id);
    }
    if cues.is_empty() {
        return Err(invalid("presentation contains no slide cues"));
    }

    let mut slides = Map::new();
    let mut media = Map::new();
    let mut groups = HashMap::new();
    let mut in_group = HashSet::new();
    let mut default_order = Vec::new();
    for group in &document.groups {
        let info = group.group.as_ref();
        let mut members = Vec::new();
        for cue_id in &group.cues {
            let Some(&(cue, action)) = cues.get(cue_id.value.as_str()) else {
                continue;
            };
            if !in_group.insert(cue_id.value.as_str()) {
                continue;
            }
            let group = if members.is_empty() { info } else { None };
            members.push(add_slide(cue, action, group, &mut slides, &mut media));
        }
        let Some(mut parent) = members.first().cloned() else {
            continue;
        };
        if members.len() > 1 {
            let mut children = Map::new();
            let mut order = Vec::new();
            for mut node in members.into_iter().skip(1) {
                let child_id = node.as_object_mut().unwrap().remove("id").unwrap();
                let child_key = child_id.as_str().unwrap();
                slides[child_key]["group"] = Value::Null;
                slides[child_key]["color"] = Value::Null;
                children.insert(child_key.to_owned(), node);
                order.push(child_id);
            }
            slides[parent["id"].as_str().unwrap()]["children"] = json!(order);
            parent["children"] = Value::Object(children);
        }
        if let Some(group_id) = info.and_then(|info| id(&info.uuid)) {
            if groups.insert(group_id, parent.clone()).is_some() {
                return Err(invalid("duplicate cue group identifier"));
            }
        }
        default_order.push(parent);
    }
    for cue_id in cue_order {
        if !in_group.contains(cue_id) {
            let (cue, action) = cues[cue_id];
            default_order.push(add_slide(cue, action, None, &mut slides, &mut media));
        }
    }

    let mut layouts = Map::new();
    let mut active = None;
    let mut covered = HashSet::new();
    let mut steps = 0;
    for arrangement in &document.arrangements {
        let mut order = Vec::new();
        for group in &arrangement.groups {
            let Some(node) = groups.get(group.value.as_str()) else {
                continue;
            };
            steps += 1 + node["children"].as_object().map_or(0, Map::len);
            if steps > 100_000 {
                return Err(invalid("arrangements expand to too many slide steps"));
            }
            covered.insert(node["id"].as_str().unwrap().to_owned());
            order.push(node.clone());
        }
        if order.is_empty() {
            continue;
        }
        if (layouts.len() + 2).saturating_mul(document.notes.len()) > 32 * 1024 * 1024 {
            return Err(invalid("arrangement notes exceed the import size limit"));
        }
        let layout_id = format!("arrangement-{}", layouts.len() + 1);
        if active.is_none()
            || id(&arrangement.uuid).is_some_and(|key| Some(key) == id(&document.selected))
        {
            active = Some(layout_id.clone());
        }
        layouts.insert(
            layout_id,
            json!({"name": arrangement.name, "notes": document.notes, "slides": order}),
        );
    }
    if active.is_none()
        || default_order
            .iter()
            .any(|node| !covered.contains(node["id"].as_str().unwrap()))
    {
        layouts.insert(
            "main".into(),
            json!({"name":"All slides","notes":document.notes,"slides":default_order}),
        );
    }
    let name = if document.name.trim().is_empty() {
        file_stem_str(file_name)
    } else {
        document.name
    };
    let mut show = ShowFile::blank(
        id(&document.uuid).unwrap_or(&file_stem_str(file_name)),
        &name,
    );
    show.show["origin"] = json!("propresenter");
    show.show["settings"]["activeLayout"] = json!(active.as_deref().unwrap_or("main"));
    show.show["slides"] = Value::Object(slides);
    show.show["media"] = Value::Object(media);
    show.show["layouts"] = Value::Object(layouts);
    if let Some(ccli) = document.ccli {
        show.show["meta"] =
            json!({"author": ccli.author, "title": ccli.title, "publisher": ccli.publisher});
        if ccli.number != 0 {
            show.show["meta"]["CCLI"] = json!(ccli.number.to_string());
        }
        if ccli.year != 0 {
            show.show["meta"]["year"] = json!(ccli.year.to_string());
        }
    }
    Ok(show)
}

fn add_slide(
    cue: &wire::Cue,
    action: Option<&wire::PresentationSlide>,
    group: Option<&wire::Group>,
    slides: &mut Map<String, Value>,
    media: &mut Map<String, Value>,
) -> Value {
    let slide_id = format!("s{}", slides.len() + 1);
    let mut node = json!({"id": slide_id, "disabled": !cue.enabled});
    let label = group.map_or(cue.name.as_str(), |group| group.name.as_str());
    let mut settings = json!({});
    let mut items = Vec::new();
    if let Some(slide) = action.and_then(|action| action.slide.as_ref()) {
        if let Some(size) = &slide.size
            && valid_size(size)
        {
            settings["resolution"] = json!({"width":size.width,"height":size.height});
        }
        if slide.draws_background
            && let Some(color) = &slide.background
        {
            settings["color"] = json!(css_color(color));
        }
        items.extend(
            slide
                .elements
                .iter()
                .filter_map(|item| text_item(item.element.as_ref()?)),
        );
    }
    if let Some((media_action, path)) = background(cue) {
        let media_id = format!("m{}", media.len() + 1);
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let mut item = json!({"name":name,"path":path});
        if let Some(video) = &media_action.video {
            item["loop"] = json!(video.playback == 1);
        }
        media.insert(media_id.clone(), item);
        node["background"] = json!(media_id);
    }
    slides.insert(
        slide_id,
        json!({
            "group":label,
            "color":group.and_then(|group|group.color.as_ref()).map(css_color),
            "settings":settings,
            "notes":action.and_then(|action| action.notes.as_ref()).map(|notes|rtf(&notes.rtf)).unwrap_or_default(),
            "items":items
        }),
    );
    node
}

fn background(cue: &wire::Cue) -> Option<(&wire::MediaAction, &str)> {
    cue.actions.iter().find_map(|action| {
        let media = action.media.as_ref().filter(|media| media.layer == 0)?;
        let url = media.element.as_ref()?.url.as_ref()?;
        let path = if url.absolute.is_empty() {
            &url.relative
        } else {
            &url.absolute
        };
        (!path.is_empty() && !path.contains('\0')).then_some((media, path.as_str()))
    })
}

pub(super) fn resolve_standalone_media(show: &mut ShowFile, source: &std::path::Path) {
    if let Some(media) = show.show["media"].as_object_mut() {
        for entry in media.values_mut() {
            let Some(path) = entry["path"].as_str() else {
                continue;
            };
            let path = if path.starts_with("file:") {
                url::Url::parse(path)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())
            } else {
                let path = std::path::Path::new(path);
                source
                    .parent()
                    .filter(|_| path.is_relative())
                    .map(|parent| parent.join(path))
                    .filter(|path| path.is_file())
            };
            if let Some(path) = path {
                entry["path"] = json!(path.to_string_lossy());
                if let Some(name) = path.file_name() {
                    entry["name"] = json!(name.to_string_lossy());
                }
            }
        }
    }
}

fn text_item(element: &wire::Element) -> Option<Value> {
    let text = element.text.as_ref().filter(|_| !element.hidden)?;
    let content = rtf(&text.rtf);
    if content.trim().is_empty() {
        return None;
    }
    let mut style = element
        .bounds
        .as_ref()
        .and_then(|rect| {
            let position = rect.origin.as_ref();
            let (x, y) = position.map_or((0.0, 0.0), |p| (p.x, p.y));
            let size = rect.size.as_ref().filter(|size| valid_size(size))?;
            (x.is_finite() && y.is_finite()).then(|| {
                format!(
                    "left:{x}px;top:{y}px;width:{}px;height:{}px;",
                    size.width, size.height
                )
            })
        })
        .unwrap_or_else(|| ITEM_STYLE.to_owned());
    let vertical = match text.vertical {
        0 => "flex-start",
        2 => "flex-end",
        _ => "center",
    };
    style.push_str(&format!("align-items:{vertical};"));
    let mut alignment = "";
    if let Some(attributes) = &text.attributes {
        if let Some(font) = &attributes.font {
            let family = if font.family.is_empty() {
                &font.name
            } else {
                &font.family
            };
            if !family.is_empty() && !family.contains([';', ':', '{', '}', '\n', '\r']) {
                style.push_str(&format!("font-family:{family};"));
            }
            if font.size.is_finite() && (1.0..=10000.0).contains(&font.size) {
                style.push_str(&format!("font-size:{}px;", font.size));
            }
            if font.bold {
                style.push_str("font-weight:bold;");
            }
            if font.italic {
                style.push_str("font-style:italic;");
            }
        }
        if let Some(color) = &attributes.color {
            style.push_str(&format!("color:{};", css_color(color)));
        }
        if let Some(paragraph) = &attributes.paragraph {
            alignment = match paragraph.alignment {
                1 => "right",
                2 => "center",
                3 => "justify",
                _ => "left",
            };
            style.push_str(&format!("text-align:{alignment};"));
            if paragraph.line_height.is_finite() && (0.1..=10.0).contains(&paragraph.line_height) {
                style.push_str(&format!("line-height:{};", paragraph.line_height));
            }
        }
        if attributes.kerning.is_finite() && attributes.kerning != 0.0 {
            style.push_str(&format!(
                "letter-spacing:{}px;",
                attributes.kerning.clamp(-500.0, 500.0)
            ));
        }
        let underline = attributes.underline.as_ref().is_some_and(|d| d.style != 0);
        let strike = attributes
            .strikethrough
            .as_ref()
            .is_some_and(|d| d.style != 0);
        match (underline, strike) {
            (true, true) => style.push_str("text-decoration:underline line-through;"),
            (true, false) => style.push_str("text-decoration:underline;"),
            (false, true) => style.push_str("text-decoration:line-through;"),
            _ => {}
        }
    }
    Some(json!({
        "style":style,
        "lines":content.lines().map(|line| json!({"align":alignment,"text":[{"value":line,"style":""}]})).collect::<Vec<_>>()
    }))
}

fn valid_size(size: &wire::Size) -> bool {
    (1.0..=32768.0).contains(&size.width) && (1.0..=32768.0).contains(&size.height)
}

fn css_color(color: &wire::Color) -> String {
    let rgb = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        rgb(color.red),
        rgb(color.green),
        rgb(color.blue),
        rgb(color.alpha)
    )
}

fn id(uuid: &Option<wire::Uuid>) -> Option<&str> {
    uuid.as_ref()
        .map(|uuid| uuid.value.as_str())
        .filter(|value| !value.is_empty())
}

fn rtf(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.trim_start().starts_with("{\\rtf") {
        decode_rtf(&text)
    } else {
        text.into_owned()
    }
}

fn invalid(error: impl std::fmt::Display) -> DataError {
    DataError::Scripture(format!("ProPresenter 7: {error}"))
}
