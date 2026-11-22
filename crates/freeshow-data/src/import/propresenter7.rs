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
    let mut import_warnings = Vec::new();
    for cue in &document.cues {
        let Some(id) = id(&cue.uuid) else { continue };
        let unsupported = unsupported_actions(cue);
        if !unsupported.is_empty() {
            let name = if cue.name.trim().is_empty() {
                id
            } else {
                cue.name.as_str()
            };
            import_warnings.push(format!(
                "Cue '{name}' has unsupported actions ({})",
                unsupported.join(", ")
            ));
        }
        let action = cue.actions.iter().find_map(|action| {
            action_carries_slide(action)?;
            action.slide.as_ref()?.presentation.as_ref()
        });
        if action.is_none()
            && background(cue).is_none()
            && !cue
                .actions
                .iter()
                .any(|action| clear_target(action).is_some())
        {
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
    let mut cue_slides = HashMap::new();
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
            members.push(add_slide(
                cue,
                action,
                group,
                &mut slides,
                &mut media,
                &mut cue_slides,
            ));
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
        if let Some(group_id) = info.and_then(|info| id(&info.uuid))
            && groups.insert(group_id, parent.clone()).is_some()
        {
            return Err(invalid("duplicate cue group identifier"));
        }
        default_order.push(parent);
    }
    for cue_id in cue_order {
        if !in_group.contains(cue_id) {
            let (cue, action) = cues[cue_id];
            default_order.push(add_slide(
                cue,
                action,
                None,
                &mut slides,
                &mut media,
                &mut cue_slides,
            ));
        }
    }
    let mut unresolved_timer = false;
    for node in default_order.iter_mut().chain(groups.values_mut()) {
        unresolved_timer |= resolve_timed_cue(node, &cue_slides);
        if let Some(children) = node.get_mut("children").and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                unresolved_timer |= resolve_timed_cue(child, &cue_slides);
            }
        }
    }
    if unresolved_timer {
        import_warnings.push("Some timed jumps target a cue that could not be imported".into());
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
        remove_terminal_next_timer(&mut order, &slides);
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
        remove_terminal_next_timer(&mut default_order, &slides);
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
    let layout_key = show.show["settings"]["activeLayout"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            show.show["layouts"]
                .as_object()
                .and_then(|layouts| layouts.keys().next())
                .cloned()
        });
    if let Some(key) = layout_key
        && let Some(recording) = document.timeline.as_ref()
        && let Some(timeline) = layout_timeline(recording, &show.layout_steps(), &cue_slides)
    {
        show.show["layouts"][key]["timeline"] = timeline;
    }
    if !import_warnings.is_empty() {
        show.show["settings"]["importWarnings"] = json!(import_warnings);
    }
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

fn resolve_timed_cue(node: &mut Value, cue_slides: &HashMap<String, String>) -> bool {
    let Some(cue_id) = node.pointer("/nextTimerTarget/cue").and_then(Value::as_str) else {
        return false;
    };
    if let Some(slide_id) = cue_slides.get(cue_id) {
        node["nextTimerTarget"] = json!({"slide":slide_id});
        return false;
    }
    let node = node.as_object_mut().unwrap();
    node.remove("nextTimer");
    node.remove("nextTimerTarget");
    true
}

fn remove_terminal_next_timer(order: &mut [Value], slides: &Map<String, Value>) {
    // An implicit Next stops at the end of this presentation.
    for node in order.iter_mut().rev() {
        let id = node["id"].as_str().unwrap();
        let last_child = slides[id]["children"].as_array().and_then(|children| {
            children
                .iter()
                .rev()
                .filter_map(Value::as_str)
                .find(|id| node["children"][*id]["disabled"] == false)
        });
        let last = if let Some(child) = last_child {
            &mut node["children"][child]
        } else if node["disabled"] == false {
            node
        } else {
            continue;
        };
        if last["nextTimerTarget"] == "next" {
            let last = last.as_object_mut().unwrap();
            last.remove("nextTimer");
            last.remove("nextTimerTarget");
        }
        return;
    }
}

fn add_slide(
    cue: &wire::Cue,
    action: Option<&wire::PresentationSlide>,
    group: Option<&wire::Group>,
    slides: &mut Map<String, Value>,
    media: &mut Map<String, Value>,
    cue_slides: &mut HashMap<String, String>,
) -> Value {
    let slide_id = format!("s{}", slides.len() + 1);
    if let Some(cue_id) = id(&cue.uuid) {
        cue_slides.insert(cue_id.to_owned(), slide_id.clone());
    }
    let mut node = json!({"id": slide_id, "disabled": !cue.enabled});
    if cue.enabled
        && cue.completion_action() == wire::CompletionAction::AfterTime
        && (0.001..=3600.0).contains(&cue.completion_time)
    {
        match cue.completion_target() {
            wire::CompletionTarget::Next => {
                node["nextTimer"] = json!(cue.completion_time);
                node["nextTimerTarget"] = json!("next");
            }
            wire::CompletionTarget::First => {
                node["nextTimer"] = json!(cue.completion_time);
                node["nextTimerTarget"] = json!("first");
            }
            wire::CompletionTarget::Cue => {
                node["nextTimer"] = json!(cue.completion_time);
                node["nextTimerTarget"] =
                    json!({"cue":id(&cue.completion_cue).unwrap_or_default()});
            }
            wire::CompletionTarget::None => {}
        }
    }
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
                .filter_map(|item| slide_item(item.element.as_ref()?)),
        );
    }
    let mut clear = Vec::new();
    if cue
        .actions
        .iter()
        .any(|action| clear_target(action) == Some(wire::ClearTarget::Audio))
    {
        clear.push("clear_audio");
    }
    match background(cue) {
        Some(CueBackground::Clear) => clear.push("clear_background"),
        Some(CueBackground::Media(media_action, path)) => {
            let media_id = format!("m{}", media.len() + 1);
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            let mut item = json!({"name":name,"path":path});
            if let Some(video) = &media_action.video {
                item["loop"] = json!(video.playback == 1);
            }
            media.insert(media_id.clone(), item);
            node["background"] = json!(media_id);
        }
        None => {}
    }
    if !clear.is_empty() {
        node["actions"]["slideActions"] = json!([{"triggers":clear}]);
        if action.is_none() {
            settings["actionOnly"] = json!(true);
        }
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

fn action_carries_slide(action: &wire::Action) -> Option<()> {
    match action.kind() {
        wire::ActionType::PresentationSlide | wire::ActionType::Unknown => Some(()),
        _ => None,
    }
    .filter(|_| action.slide.is_some())
}

fn action_carries_media(action: &wire::Action) -> Option<()> {
    match action.kind() {
        wire::ActionType::Media
        | wire::ActionType::ForegroundMedia
        | wire::ActionType::BackgroundMedia
        | wire::ActionType::Unknown => Some(()),
        _ => None,
    }
    .filter(|_| action.media.is_some())
}

fn unsupported_actions(cue: &wire::Cue) -> Vec<&'static str> {
    let mut labels = Vec::new();
    for action in &cue.actions {
        if action_carries_slide(action).is_some()
            || action_carries_media(action).is_some()
            || clear_target(action).is_some()
        {
            continue;
        }
        match action.kind() {
            wire::ActionType::Unknown => {}
            kind => {
                let label = kind.label();
                if !labels.contains(&label) {
                    labels.push(label);
                }
            }
        }
    }
    labels
}

fn layout_timeline(
    timeline: &wire::Timeline,
    steps: &[(String, Value)],
    cue_slides: &HashMap<String, String>,
) -> Option<Value> {
    let cues = if !timeline.cues_v2.is_empty() {
        &timeline.cues_v2
    } else {
        &timeline.cues
    };
    if cues.is_empty() {
        return None;
    }
    let mut positions = HashMap::<&str, Vec<usize>>::new();
    for (index, (id, node)) in steps.iter().enumerate() {
        if node["disabled"] != true {
            positions.entry(id).or_default().push(index);
        }
    }
    let mut current = 0usize;
    let mut actions = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let Some(cue_id) = id(&cue.cue_id) else {
            continue;
        };
        let Some(slide_id) = cue_slides.get(cue_id) else {
            continue;
        };
        let Some(occurrences) = positions.get(slide_id.as_str()) else {
            continue;
        };
        let offset = occurrences.partition_point(|index| *index < current);
        let position = occurrences.get(offset).copied().unwrap_or(occurrences[0]);
        if !cue.trigger_time.is_finite() || cue.trigger_time < 0.0 {
            continue;
        }
        current = position;
        actions.push(json!({
            "id": format!("t{}", index + 1),
            "time": (cue.trigger_time * 1000.0).round() as u64,
            "name": cue.name,
            "type": "slide",
            "data": { "id": slide_id, "index": position }
        }));
    }
    if actions.is_empty() {
        return None;
    }
    let mut value = json!({"actions": actions});
    if timeline.r#loop {
        value["loop"] = json!(true);
    }
    if timeline.duration > 0.0 {
        value["duration"] = json!(timeline.duration);
    }
    Some(value)
}

fn clear_target(action: &wire::Action) -> Option<wire::ClearTarget> {
    if action.kind() != wire::ActionType::Clear || !action.enabled || action.delay != 0.0 {
        return None;
    }
    let clear = action
        .clear
        .as_ref()
        .filter(|clear| clear.destination == 0)?;
    match clear.target() {
        target @ (wire::ClearTarget::Audio | wire::ClearTarget::Background) => Some(target),
        wire::ClearTarget::All => None,
    }
}

enum CueBackground<'a> {
    Clear,
    Media(&'a wire::MediaAction, &'a str),
}

fn background(cue: &wire::Cue) -> Option<CueBackground<'_>> {
    cue.actions.iter().rev().find_map(|action| {
        if clear_target(action) == Some(wire::ClearTarget::Background) {
            return Some(CueBackground::Clear);
        }
        action_carries_media(action)?;
        let media = action.media.as_ref().filter(|media| media.layer == 0)?;
        let url = media.element.as_ref()?.url.as_ref()?;
        let path = if url.absolute.is_empty() {
            &url.relative
        } else {
            &url.absolute
        };
        (!path.is_empty() && !path.contains('\0')).then_some(CueBackground::Media(media, path))
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

fn slide_item(element: &wire::Element) -> Option<Value> {
    if element.hidden {
        return None;
    }
    let geometry = box_geometry(element);
    let fill = geometry.as_ref().and_then(|_| box_fill(element));
    let mut style = geometry.unwrap_or_else(|| ITEM_STYLE.to_owned());
    if let Some(fill) = &fill {
        style.push_str(fill);
    }
    if let Some(text) = &element.text
        && let Some(item) = text_item(text, &mut style)
    {
        return Some(item);
    }
    fill.map(|_| json!({"type":"shape","style":style,"lines":[]}))
}

fn box_geometry(element: &wire::Element) -> Option<String> {
    let rect = element.bounds.as_ref()?;
    let (x, y) = rect.origin.as_ref().map_or((0.0, 0.0), |p| (p.x, p.y));
    let size = rect.size.as_ref().filter(|size| valid_size(size))?;
    let mut style = format!(
        "left:{x}px;top:{y}px;width:{}px;height:{}px;",
        size.width, size.height
    );
    if element.rotation.is_finite() && element.rotation != 0.0 {
        style.push_str(&format!("transform:rotate({}deg);", element.rotation));
    }
    (x.is_finite() && y.is_finite()).then_some(style)
}

fn box_fill(element: &wire::Element) -> Option<String> {
    let fill = element.fill.as_ref().filter(|fill| fill.enabled)?;
    let background = if let Some(gradient) = &fill.gradient {
        gradient_fill(gradient)?
    } else {
        let color = fill.color.as_ref().filter(|color| color.alpha > 0.0)?;
        format!("background-color:{};", css_color(color))
    };
    let shape = element.path.as_ref()?.shape.as_ref()?;
    let geometry = match shape.kind() {
        wire::ShapeType::Rectangle
        | wire::ShapeType::Star
        | wire::ShapeType::Polygon
        | wire::ShapeType::Custom
        | wire::ShapeType::RightArrow
        | wire::ShapeType::DoubleArrow => String::new(),
        wire::ShapeType::IsoscelesTriangle => "--shape-fill-polygon:0.5 0,1 1,0 1;".to_owned(),
        wire::ShapeType::RightTriangle => "--shape-fill-polygon:0 0,1 1,0 1;".to_owned(),
        wire::ShapeType::Rhombus => "--shape-fill-polygon:0.5 0,1 0.5,0.5 1,0 0.5;".to_owned(),
        wire::ShapeType::Ellipse => "border-radius:50%;".to_string(),
        wire::ShapeType::RoundedRectangle => {
            let size = element.bounds.as_ref().and_then(|rect| rect.size.as_ref());
            let shortest = size
                .map(|size| size.width.min(size.height))
                .unwrap_or(0.0)
                .max(0.0);
            let roundness = shape
                .rounded_rectangle
                .as_ref()
                .map(|item| item.roundness)
                .filter(|value| value.is_finite())
                .unwrap_or(0.25);
            let radius = if (0.0..=1.0).contains(&roundness) {
                roundness * shortest * 0.5
            } else {
                roundness
            };
            format!("border-radius:{}px;", radius.round())
        }
        wire::ShapeType::Unknown => return None,
    };
    Some(format!("{background}{geometry}"))
}

fn gradient_fill(gradient: &wire::Gradient) -> Option<String> {
    let stops: Vec<_> = gradient
        .stops
        .iter()
        .filter_map(|stop| {
            let position = stop
                .position
                .is_finite()
                .then_some(stop.position)?
                .clamp(0.0, 1.0);
            Some((stop.color.as_ref()?, position))
        })
        .collect();
    if !stops.iter().any(|(color, _)| color.alpha > 0.0) {
        return None;
    }
    if let [(color, _)] = stops.as_slice() {
        return Some(format!("background-color:{};", css_color(color)));
    }
    let start = match wire::GradientType::try_from(gradient.kind).ok()? {
        wire::GradientType::Linear if gradient.angle.is_finite() => {
            // Convert a right-facing, counterclockwise angle to CSS's up-facing clockwise angle.
            format!(
                "linear-gradient({}deg",
                (90.0 - gradient.angle).rem_euclid(360.0)
            )
        }
        wire::GradientType::Radial => "radial-gradient(circle at center".to_owned(),
        _ => return None,
    };
    use std::fmt::Write;
    let mut css = format!("background:{start}");
    for (color, position) in stops {
        let _ = write!(css, ", {} {}%", css_color(color), position * 100.0);
    }
    css.push_str(");");
    Some(css)
}

fn text_item(text: &wire::Text, style: &mut String) -> Option<Value> {
    let content = String::from_utf8_lossy(&text.rtf);
    let rich = content
        .trim_start()
        .starts_with("{\\rtf")
        .then(|| super::rtf::decode(&content));
    if rich.as_ref().map_or_else(
        || content.trim().is_empty(),
        |rtf| rtf.plain_text().trim().is_empty(),
    ) {
        return None;
    }
    let vertical = match text.vertical {
        0 => "flex-start",
        2 => "flex-end",
        _ => "center",
    };
    style.push_str(&format!("align-items:{vertical};"));
    let mut alignment = "";
    let (mut underline, mut strike) = (false, false);
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
        underline = attributes.underline.as_ref().is_some_and(|d| d.style != 0);
        strike = attributes
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
    let lines = if let Some(rtf) = rich {
        rtf.styled_lines(alignment, underline, strike)
    } else {
        content
            .lines()
            .map(|line| json!({"align":alignment,"text":[{"value":line,"style":""}]}))
            .collect()
    };
    Some(json!({"style":style,"lines":lines}))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_global_clear_cues_in_arrangement_and_media_order() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let clear = |target| wire::Action {
            kind: wire::ActionType::Clear as i32,
            enabled: true,
            clear: Some(wire::ClearAction {
                target,
                destination: 0,
            }),
            ..Default::default()
        };
        let background = clear(wire::ClearTarget::Background as i32);
        let audio = clear(wire::ClearTarget::Audio as i32);
        presentation.cues[1].enabled = true;
        presentation.cues[1].actions = vec![background.clone(), audio.clone()];
        presentation.cues[2].actions.insert(1, background.clone());
        presentation.cues[0].actions.extend([
            wire::Action {
                delay: 1.0,
                ..background
            },
            wire::Action {
                clear: Some(wire::ClearAction {
                    target: wire::ClearTarget::Audio as i32,
                    destination: 1,
                }),
                ..audio
            },
        ]);

        let show = crate::import::import_show("Clears.pro", &presentation.encode_to_vec()).unwrap();
        let slides = show.ordered_slides();
        assert!(slides[2].lines.is_empty());
        assert_eq!(
            show.show["slides"][&slides[2].id]["settings"]["actionOnly"],
            true
        );
        assert!(matches!(
            show.backdrop_at(0),
            crate::SlideBackdrop::Picture { .. }
        ));
        assert!(matches!(
            show.backdrop_at(1),
            crate::SlideBackdrop::Picture { .. }
        ));
        assert_eq!(show.backdrop_at(2), crate::SlideBackdrop::Clear);
        assert!(!show.clears_audio_at(1));
        assert!(show.clears_audio_at(2));
        let warnings = show.show["settings"]["importWarnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .as_str()
                .unwrap()
                .ends_with("unsupported actions (clear)")
        );

        let actions = &mut presentation.cues[2].actions;
        let clear_after_media = actions.remove(1);
        actions.push(clear_after_media);
        let show = crate::import::import_show("Clears.pro", &presentation.encode_to_vec()).unwrap();
        assert_eq!(show.backdrop_at(0), crate::SlideBackdrop::Clear);
        assert!(show.show["media"].as_object().unwrap().is_empty());
    }

    #[test]
    fn imports_timed_next_cues_for_each_layout_position() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        presentation.cues[1].enabled = true;
        for (cue, seconds) in presentation.cues.iter_mut().zip([1.25, 2.5, 3.75]) {
            cue.completion_action = wire::CompletionAction::AfterTime as i32;
            cue.completion_target = wire::CompletionTarget::Next as i32;
            cue.completion_time = seconds;
        }
        let mut verse_only = presentation.arrangements[0].clone();
        verse_only.uuid = Some(wire::Uuid {
            value: "verse-only".into(),
        });
        verse_only.groups.truncate(1);
        presentation.arrangements.push(verse_only);
        let mut show =
            crate::import::import_show("Timed.pro", &presentation.encode_to_vec()).unwrap();
        assert_eq!(show.next_timer_at(0), Some(3.75));
        assert_eq!(show.next_timer_at(1), Some(1.25));
        assert_eq!(show.next_timer_at(2), Some(2.5));
        assert_eq!(show.next_timer_at(3), None);
        assert!(show.set_active_layout("arrangement-3"));
        assert_eq!(show.next_timer_at(0), Some(1.25));
        assert_eq!(show.next_timer_at(1), None);

        presentation.cues[1].enabled = false;
        let mut show =
            crate::import::import_show("Timed.pro", &presentation.encode_to_vec()).unwrap();
        assert!(show.set_active_layout("arrangement-3"));
        assert_eq!(show.next_timer_at(0), None);
        assert_eq!(show.next_timer_at(1), None);

        presentation.cues[1].enabled = true;
        presentation.cues[1].completion_target = wire::CompletionTarget::First as i32;
        let mut show =
            crate::import::import_show("Timed.pro", &presentation.encode_to_vec()).unwrap();
        assert!(show.set_active_layout("arrangement-3"));
        assert_eq!(show.next_timer_at(1), Some(2.5));
        assert_eq!(
            show.timed_cue_target_at(1),
            Some(crate::TimedCueTarget::First)
        );
        assert!(!show.ends_at(1));
        show.set_next_timers(3.0);
        assert_eq!(show.timed_cue_target_at(1), None);
    }

    #[test]
    fn resolves_timed_jumps_to_imported_cue_ids() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        presentation.cues[1].enabled = true;
        presentation.cues[2].completion_action = wire::CompletionAction::AfterTime as i32;
        presentation.cues[2].completion_target = wire::CompletionTarget::Cue as i32;
        presentation.cues[2].completion_time = 2.0;
        presentation.cues[2].completion_cue = presentation.cues[1].uuid.clone();

        let show = crate::import::import_show("Jump.pro", &presentation.encode_to_vec()).unwrap();
        let slides = show.ordered_slides();
        assert_eq!(show.next_timer_at(3), Some(2.0));
        assert_eq!(
            show.timed_cue_target_at(3),
            Some(crate::TimedCueTarget::Slide(slides[2].id.clone()))
        );
        assert_eq!(
            show.output_index_for_slide(&slides[2].id, "default", ""),
            Some(2)
        );

        presentation.cues[2].completion_cue = Some(wire::Uuid {
            value: "missing".into(),
        });
        let show = crate::import::import_show("Jump.pro", &presentation.encode_to_vec()).unwrap();
        assert_eq!(show.next_timer_at(0), None);
        assert_eq!(show.timed_cue_target_at(3), None);
        assert_eq!(
            show.show["settings"]["importWarnings"][0],
            "Some timed jumps target a cue that could not be imported"
        );
    }

    #[test]
    fn imports_visible_solid_shapes_in_slide_element_order() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let elements = &mut presentation.cues[0].actions[0]
            .slide
            .as_mut()
            .unwrap()
            .presentation
            .as_mut()
            .unwrap()
            .slide
            .as_mut()
            .unwrap()
            .elements;
        let shape = |kind, enabled, hidden| wire::SlideElement {
            element: Some(wire::Element {
                bounds: Some(wire::Rect {
                    origin: Some(wire::Point { x: 160.0, y: 90.0 }),
                    size: Some(wire::Size {
                        width: 320.0,
                        height: 180.0,
                    }),
                }),
                path: Some(wire::Path {
                    shape: Some(wire::Shape {
                        kind,
                        rounded_rectangle: None,
                    }),
                }),
                fill: Some(wire::Fill {
                    enabled,
                    color: Some(wire::Color {
                        red: 1.0,
                        alpha: 0.5,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                hidden,
                ..Default::default()
            }),
        };
        let gradient_shape = |kind, gradient_type| {
            let mut item = shape(kind, true, false);
            let fill = item.element.as_mut().unwrap().fill.as_mut().unwrap();
            fill.gradient = Some(wire::Gradient {
                kind: gradient_type,
                angle: 30.0,
                stops: vec![
                    wire::ColorStop {
                        color: fill.color.take(),
                        position: 0.25,
                    },
                    wire::ColorStop {
                        color: Some(wire::Color {
                            blue: 1.0,
                            ..Default::default()
                        }),
                        position: 1.0,
                    },
                ],
            });
            item
        };
        elements.insert(0, shape(1, true, false));
        elements.extend([
            shape(2, true, false),
            shape(1, false, false),
            shape(1, true, true),
            shape(6, true, false),
            shape(3, true, false),
            shape(4, true, false),
            shape(5, true, false),
            gradient_shape(3, wire::GradientType::Linear as i32),
            gradient_shape(2, wire::GradientType::Radial as i32),
        ]);

        let show = crate::import::import_show("Shapes.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        let items = show.show["slides"][&verse.id]["items"].as_array().unwrap();
        let style = "left:160px;top:90px;width:320px;height:180px;background-color:#ff000080;";
        assert_eq!(items.len(), 10);
        assert_eq!(items[0], json!({"type":"shape","style":style,"lines":[]}));
        assert!(
            !items[1]["lines"][0]["text"][0]["value"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert_eq!(items[3]["style"], format!("{style}border-radius:50%;"));
        assert_eq!(items[4], json!({"type":"shape","style":style,"lines":[]}));
        for (item, points) in
            items[5..]
                .iter()
                .zip(["0.5 0,1 1,0 1", "0 0,1 1,0 1", "0.5 0,1 0.5,0.5 1,0 0.5"])
        {
            assert_eq!(
                *item,
                json!({"type":"shape","style":format!("{style}--shape-fill-polygon:{points};"),"lines":[]})
            );
        }
        let bounds = "left:160px;top:90px;width:320px;height:180px;";
        assert_eq!(
            items[8]["style"],
            format!(
                "{bounds}background:linear-gradient(60deg, #ff000080 25%, #0000ff00 100%);--shape-fill-polygon:0.5 0,1 1,0 1;"
            )
        );
        assert_eq!(
            items[9]["style"],
            format!(
                "{bounds}background:radial-gradient(circle at center, #ff000080 25%, #0000ff00 100%);border-radius:50%;"
            )
        );
    }

    #[test]
    fn imports_mixed_rtf_runs_into_the_existing_propresenter_text_box() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let text = presentation.cues[0].actions[0]
            .slide
            .as_mut()
            .unwrap()
            .presentation
            .as_mut()
            .unwrap()
            .slide
            .as_mut()
            .unwrap()
            .elements[0]
            .element
            .as_mut()
            .unwrap()
            .text
            .as_mut()
            .unwrap();
        text.rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}{\f1 Times New Roman;}}{\colortbl;\red255\green0\blue0;\red255\green255\blue0;\red0\green255\blue0;}
\qc\f0\fs48 A {\f1\cf1\cb3\highlight2\b0\i0\ul\strike \ldblquote red\rdblquote\emdash\u-10179?\u-8704?\highlight0 !}\par\ql\ulnone\strike0 Last}"
            .to_vec();
        let show = crate::import::import_show("Mixed.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        assert_eq!(verse.lines, ["A “red”—😀!", "Last", "Another text box"]);
        let item = &show.show["slides"][&verse.id]["items"][0];
        let rows = &item["lines"];
        assert_eq!(rows[0]["align"], "center");
        assert_eq!(rows[1]["align"], "left");
        assert_eq!(
            rows[0]["text"][0],
            json!({"value":"A ","style":"font-family:Arial;font-size:24px;"})
        );
        assert_eq!(
            rows[0]["text"][1],
            json!({"value":"“red”—😀","style":"font-family:Times New Roman;font-size:24px;color:#ff0000;background-color:#ffff00;font-weight:normal;font-style:normal;text-decoration:underline line-through;"})
        );
        assert_eq!(rows[0]["text"][2]["value"], "!");
        assert!(
            rows[0]["text"][2]["style"]
                .as_str()
                .unwrap()
                .contains("background-color:#00ff00;")
        );
        assert_eq!(rows[1]["text"][0]["value"], "Last");
        assert_eq!(
            rows[1]["text"][0]["style"],
            "font-family:Arial;font-size:24px;text-decoration:none;"
        );
    }

    #[test]
    fn hidden_rtf_stays_out_of_imported_slides_and_notes() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let slide = presentation.cues[0].actions[0]
            .slide
            .as_mut()
            .unwrap()
            .presentation
            .as_mut()
            .unwrap();
        slide.notes.as_mut().unwrap().rtf =
            br"{\rtf1 \bullet  Note {\v backstage\emdash}for the leader\endash\lquote team\rquote}"
                .to_vec();
        slide.slide.as_mut().unwrap().elements[0]
            .element
            .as_mut()
            .unwrap()
            .text
            .as_mut()
            .unwrap()
            .rtf = br"{\rtf1 First {\v omit \u9731?\line also omitted}line\par\v skip \v0 Second}"
            .to_vec();
        let show = crate::import::import_show("Hidden.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        assert_eq!(verse.lines, ["First line", "Second", "Another text box"]);
        assert_eq!(verse.notes, "• Note for the leader–‘team’");
    }

    #[test]
    fn imports_rotation_rounded_rectangles_timelines_and_warns_on_unsupported_cues() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let cue_id = presentation.cues[0].uuid.clone();
        let slide = presentation.cues[0].actions[0]
            .slide
            .as_mut()
            .unwrap()
            .presentation
            .as_mut()
            .unwrap()
            .slide
            .as_mut()
            .unwrap();
        slide.elements.push(wire::SlideElement {
            element: Some(wire::Element {
                bounds: Some(wire::Rect {
                    origin: Some(wire::Point { x: 10.0, y: 20.0 }),
                    size: Some(wire::Size {
                        width: 200.0,
                        height: 100.0,
                    }),
                }),
                rotation: 15.0,
                path: Some(wire::Path {
                    shape: Some(wire::Shape {
                        kind: wire::ShapeType::RoundedRectangle as i32,
                        rounded_rectangle: Some(wire::RoundedRectangle { roundness: 0.5 }),
                    }),
                }),
                fill: Some(wire::Fill {
                    enabled: true,
                    color: Some(wire::Color {
                        red: 0.0,
                        green: 0.0,
                        blue: 1.0,
                        alpha: 1.0,
                    }),
                    ..Default::default()
                }),
                hidden: false,
                ..Default::default()
            }),
        });
        presentation.timeline = Some(wire::Timeline {
            cues: vec![wire::TimelineCue {
                trigger_time: 1.5,
                cue_id: cue_id.clone(),
                name: "Verse".into(),
            }],
            duration: 12.0,
            r#loop: true,
            cues_v2: Vec::new(),
        });
        presentation.cues.push(wire::Cue {
            uuid: Some(wire::Uuid {
                value: "timer-only".into(),
            }),
            name: "Clock".into(),
            enabled: true,
            actions: vec![wire::Action {
                kind: wire::ActionType::Timer as i32,
                ..Default::default()
            }],
            ..Default::default()
        });
        // A supported slide that also has a macro should stay in the show and warn.
        presentation.cues[0].actions.push(wire::Action {
            kind: wire::ActionType::Macro as i32,
            ..Default::default()
        });
        let show =
            crate::import::import_show("Features.pro", &presentation.encode_to_vec()).unwrap();
        let items = show.show["slides"]["s1"]["items"].as_array().unwrap();
        let rounded = items
            .iter()
            .find(|item| {
                item["style"]
                    .as_str()
                    .is_some_and(|style| style.contains("transform:rotate(15deg)"))
            })
            .unwrap();
        let style = rounded["style"].as_str().unwrap();
        assert!(style.contains("border-radius:25px;"), "{style}");
        assert!(style.contains("background-color:#0000ff"));
        let layout_id = show.show["settings"]["activeLayout"].as_str().unwrap();
        let timeline = &show.show["layouts"][layout_id]["timeline"];
        assert_eq!(timeline["loop"], true);
        assert_eq!(timeline["duration"], 12.0);
        assert_eq!(timeline["actions"][0]["time"], 1500);
        assert_eq!(timeline["actions"][0]["type"], "slide");
        assert_eq!(timeline["actions"][0]["data"]["id"], "s1");
        let cues = crate::timeline_cues_from_show(&show.show);
        assert_eq!(cues[0].time_ms, 1500);
        assert_eq!(cues[0].slide_index, Some(1));
        let warnings = show.show["settings"]["importWarnings"].as_array().unwrap();
        let text = warnings
            .iter()
            .filter_map(|warning| warning.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("unsupported actions"), "{text}");
        assert!(text.contains("timer") || text.contains("macro"), "{text}");
        assert!(
            show.ordered_slides()
                .iter()
                .all(|slide| slide.group.as_deref() != Some("Clock")),
            "timer-only cues must not become slides"
        );
    }
}
