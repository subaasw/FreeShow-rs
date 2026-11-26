// SPDX-License-Identifier: GPL-3.0-only

mod wire;

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::fmt::Write;

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
            && cue_picture(cue).is_none()
            && cue_audio(cue).is_none()
            && !cue.actions.iter().any(|action| {
                immediate_clear_target(action).is_some()
                    || (action_delay(action).is_some()
                        && (clear_target(action).is_some()
                            || audio_media(action).is_some()
                            || (action.enabled && picture_media(action).is_some())))
            })
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
        && let Some(timeline) =
            layout_timeline(recording, &show.layout_steps(), &cue_slides, &mut media)
    {
        show.show["layouts"][key]["timeline"] = timeline;
    }
    show.show["media"] = Value::Object(media);
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
        .any(|action| immediate_clear_target(action) == Some(wire::ClearTarget::All))
    {
        clear.push("clear_all");
    }
    if cue
        .actions
        .iter()
        .any(|action| immediate_clear_target(action) == Some(wire::ClearTarget::Audio))
    {
        clear.push("clear_audio");
    }
    if cue
        .actions
        .iter()
        .any(|action| immediate_clear_target(action) == Some(wire::ClearTarget::Slide))
    {
        clear.push("clear_slide");
    }
    match cue_picture(cue) {
        Some(CuePicture::Clear) if !clear.contains(&"clear_all") => clear.push("clear_background"),
        Some(CuePicture::Clear) => {}
        Some(CuePicture::Media(media_action, path, foreground)) => {
            node["background"] = json!(insert_media(media, media_action, path, foreground));
        }
        None => {}
    }
    if !clear.contains(&"clear_all")
        && let Some((audio, path)) = cue_audio(cue)
    {
        node["audio"] = json!([insert_media(media, audio, path, false)]);
        if action.is_none() && node["background"].is_null() {
            settings["actionOnly"] = json!(true);
        }
    }
    if !clear.is_empty() {
        node["actions"]["slideActions"] = json!([{"triggers":clear}]);
        if action.is_none() {
            settings["actionOnly"] = json!(true);
        }
    }
    let delayed: Vec<_> = cue
        .actions
        .iter()
        .filter_map(|action| {
            let time = action_delay(action)?;
            let (kind, data) = media_or_clear_action(action, media)?;
            Some(json!({"time": time, "type": kind, "data": data}))
        })
        .collect();
    if !delayed.is_empty() {
        node["delayedActions"] = json!(delayed);
        if action.is_none() && node["background"].is_null() {
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
            || (timeline_millis(action.delay).is_some() && clear_target(action).is_some())
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
    media: &mut Map<String, Value>,
) -> Option<Value> {
    let cues = if !timeline.cues_v2.is_empty() {
        &timeline.cues_v2
    } else {
        &timeline.cues
    };
    let mut positions = HashMap::<&str, Vec<usize>>::new();
    for (index, (id, node)) in steps.iter().enumerate() {
        if node["disabled"] != true {
            positions.entry(id).or_default().push(index);
        }
    }
    let mut current = 0usize;
    let mut actions = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let delay = cue.action.as_ref().map_or(0.0, |action| action.delay);
        if !(cue.trigger_time >= 0.0 && delay >= 0.0) {
            continue;
        }
        let Some(time) = timeline_millis(cue.trigger_time + delay) else {
            continue;
        };
        let (kind, data) = if let Some(action) = &cue.action {
            let Some(media_action) = media_or_clear_action(action, media) else {
                continue;
            };
            media_action
        } else {
            let Some(occurrences) = id(&cue.cue_id)
                .and_then(|cue_id| cue_slides.get(cue_id))
                .and_then(|slide_id| positions.get(slide_id.as_str()))
            else {
                continue;
            };
            let offset = occurrences.partition_point(|index| *index < current);
            current = occurrences.get(offset).copied().unwrap_or(occurrences[0]);
            ("slide", json!({"id": steps[current].0, "index": current}))
        };
        actions.push(json!({
            "id": format!("t{}", index + 1),
            "time": time,
            "name": cue.name,
            "type": kind,
            "data": data
        }));
    }
    let soundtrack = timeline.audio_action.as_ref().and_then(|action| {
        action_carries_media(action)?;
        let audio = action.media.as_ref()?;
        if !action.enabled || audio.video.is_some() {
            return None;
        }
        let delay = timeline_millis(action.delay)?;
        let path = media_source(audio)?;
        Some((insert_media(media, audio, path, false), delay))
    });
    if actions.is_empty() && soundtrack.is_none() {
        return None;
    }
    actions.sort_by_key(|action| action["time"].as_u64());
    let mut value = json!({"actions": actions});
    if let Some((id, delay)) = soundtrack {
        value["soundtrack"] = json!(id);
        if delay > 0 {
            value["soundtrackDelayMs"] = json!(delay);
        }
    }
    if timeline.r#loop {
        value["loop"] = json!(true);
    }
    if timeline.duration > 0.0 {
        value["duration"] = json!(timeline.duration);
    }
    Some(value)
}

fn timeline_millis(seconds: f64) -> Option<u64> {
    let millis = (seconds * 1000.0).round();
    (seconds >= 0.0 && millis < u64::MAX as f64).then_some(millis as u64)
}

fn action_delay(action: &wire::Action) -> Option<u64> {
    timeline_millis(action.delay).filter(|_| action.delay > 0.0)
}

fn media_or_clear_action(
    action: &wire::Action,
    media: &mut Map<String, Value>,
) -> Option<(&'static str, Value)> {
    if let Some(target) = clear_target(action) {
        let kind = match target {
            wire::ClearTarget::Audio => "clear_audio",
            wire::ClearTarget::Background => "clear_background",
            wire::ClearTarget::Slide => "clear_slide",
            wire::ClearTarget::All => "clear_all",
        };
        return Some((kind, json!({})));
    }
    if !action.enabled {
        return None;
    }
    if let Some((audio, path)) = audio_media(action) {
        return Some((
            "audio",
            json!({"id": insert_media(media, audio, path, false)}),
        ));
    }
    let (picture, path, foreground) = picture_media(action)?;
    let kind = if foreground {
        "foreground"
    } else {
        "background"
    };
    Some((
        kind,
        json!({"id": insert_media(media, picture, path, foreground)}),
    ))
}

fn immediate_clear_target(action: &wire::Action) -> Option<wire::ClearTarget> {
    (action.delay == 0.0)
        .then(|| clear_target(action))
        .flatten()
}

fn clear_target(action: &wire::Action) -> Option<wire::ClearTarget> {
    if action.kind() != wire::ActionType::Clear || !action.enabled {
        return None;
    }
    let clear = action
        .clear
        .as_ref()
        .filter(|clear| clear.destination == 0)?;
    wire::ClearTarget::try_from(clear.target).ok()
}

enum CuePicture<'a> {
    Clear,
    Media(&'a wire::MediaAction, &'a str, bool),
}

fn cue_picture(cue: &wire::Cue) -> Option<CuePicture<'_>> {
    if cue
        .actions
        .iter()
        .any(|action| immediate_clear_target(action) == Some(wire::ClearTarget::All))
    {
        return Some(CuePicture::Clear);
    }
    cue.actions
        .iter()
        .rev()
        .filter(|action| action.delay == 0.0)
        .find_map(|action| {
            if immediate_clear_target(action) == Some(wire::ClearTarget::Background) {
                return Some(CuePicture::Clear);
            }
            let (media, path, foreground) = picture_media(action)?;
            Some(CuePicture::Media(media, path, foreground))
        })
}

fn picture_media(action: &wire::Action) -> Option<(&wire::MediaAction, &str, bool)> {
    action_carries_media(action)?;
    let media = action
        .media
        .as_ref()
        .filter(|media| matches!(media.layer, 0 | 1) && media.audio.is_none())?;
    let foreground = action.kind() == wire::ActionType::ForegroundMedia || media.layer == 1;
    Some((media, media_source(media)?, foreground))
}

fn audio_media(action: &wire::Action) -> Option<(&wire::MediaAction, &str)> {
    action_carries_media(action)?;
    let media = action
        .media
        .as_ref()
        .filter(|media| action.enabled && media.audio.is_some())?;
    Some((media, media_source(media)?))
}

fn cue_audio(cue: &wire::Cue) -> Option<(&wire::MediaAction, &str)> {
    for action in cue.actions.iter().rev() {
        if matches!(
            immediate_clear_target(action),
            Some(wire::ClearTarget::Audio | wire::ClearTarget::All)
        ) {
            break;
        }
        if action.delay == 0.0
            && let Some(audio) = audio_media(action)
        {
            return Some(audio);
        }
    }
    None
}

fn insert_media(
    media: &mut Map<String, Value>,
    action: &wire::MediaAction,
    path: &str,
    foreground: bool,
) -> String {
    let id = format!("m{}", media.len() + 1);
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let mut entry = json!({"name": name, "path": path});
    if let Some(video) = &action.video {
        entry["loop"] = json!(video.playback == 1);
        let end = match video.end_behavior {
            1 => Some("black"),
            2 => Some("clear"),
            3 => Some("fade_black"),
            4 => Some("fade_clear"),
            _ => None,
        };
        if let Some(end) = end {
            entry["endBehavior"] = json!(end);
        }
    }
    if let Some(audio) = &action.audio {
        entry["loop"] = json!(audio.playback == 1);
    }
    if let Some(element) = &action.element {
        let video = element
            .video
            .as_ref()
            .and_then(|video| video.transport.as_ref());
        let audio = element
            .audio
            .as_ref()
            .and_then(|audio| audio.transport.as_ref());
        for (key, transport) in [("videoTransport", video), ("audioTransport", audio)] {
            if let Some(transport) = transport {
                entry[key] = import_transport(transport);
            }
        }
    }
    if action.always_retrigger {
        entry["retrigger"] = json!(true);
    }
    if foreground {
        entry["videoType"] = json!("foreground");
        entry["muted"] = json!(false);
    }
    media.insert(id.clone(), entry);
    id
}

fn import_transport(transport: &wire::Transport) -> Value {
    let from = if transport.in_point.is_finite() {
        transport.in_point.max(0.0)
    } else {
        0.0
    };
    let to = if transport.out_point.is_finite() && transport.out_point > from {
        transport.out_point
    } else {
        0.0
    };
    let rate = if transport.play_rate.is_finite() && transport.play_rate > 0.0 {
        transport.play_rate
    } else {
        1.0
    };
    json!({"fromTime": from, "toTime": to, "speed": rate})
}

fn media_source(media: &wire::MediaAction) -> Option<&str> {
    let url = media.element.as_ref()?.url.as_ref()?;
    let path = if url.absolute.is_empty() {
        &url.relative
    } else {
        &url.absolute
    };
    (!path.is_empty() && !path.contains('\0')).then_some(path)
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
    } else if let Some(effect) = &fill.background_effect {
        match effect.kind.as_ref()? {
            wire::BackgroundEffectKind::Invert(_) => "backdrop-filter:invert(1);".to_owned(),
            wire::BackgroundEffectKind::Blur(blur)
                if blur.blur_amount.is_finite() && blur.saturation.is_finite() =>
            {
                format!(
                    "backdrop-filter:blur({}px) saturate({});",
                    blur.blur_amount.max(0.0),
                    blur.saturation.max(0.0)
                )
            }
            wire::BackgroundEffectKind::Blur(_) => return None,
        }
    } else {
        let color = fill.color.as_ref().filter(|color| color.alpha > 0.0)?;
        format!("background-color:{};", css_color(color))
    };
    let shape = element.path.as_ref()?.shape.as_ref()?;
    let geometry = match shape.kind() {
        wire::ShapeType::Rectangle
        | wire::ShapeType::Custom
        | wire::ShapeType::RightArrow
        | wire::ShapeType::DoubleArrow => String::new(),
        wire::ShapeType::IsoscelesTriangle => "--shape-fill-polygon:0.5 0,1 1,0 1;".to_owned(),
        wire::ShapeType::RightTriangle => "--shape-fill-polygon:0 0,1 1,0 1;".to_owned(),
        wire::ShapeType::Rhombus => "--shape-fill-polygon:0.5 0,1 0.5,0.5 1,0 0.5;".to_owned(),
        wire::ShapeType::Polygon => shape
            .polygon
            .as_ref()
            .filter(|polygon| (3..=128).contains(&polygon.number_sides))
            .map(|polygon| regular_outline(polygon.number_sides, None))
            .unwrap_or_default(),
        wire::ShapeType::Star => shape
            .star
            .as_ref()
            .filter(|star| {
                (3..=64).contains(&star.number_points)
                    && (0.0..=1.0).contains(&star.inner_radius)
                    && star.inner_radius > 0.0
            })
            .map(|star| regular_outline(star.number_points * 2, Some(star.inner_radius)))
            .unwrap_or_default(),
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

fn regular_outline(vertices: u32, inner_radius: Option<f64>) -> String {
    let outline: Vec<_> = (0..vertices)
        .map(|index| {
            let angle = TAU * f64::from(index) / f64::from(vertices);
            let radius = if index % 2 == 1 {
                inner_radius.unwrap_or(1.0)
            } else {
                1.0
            };
            let (sin, cos) = angle.sin_cos();
            [radius * sin, -radius * cos]
        })
        .collect();
    let min = outline
        .iter()
        .fold([f64::INFINITY; 2], |[left, top], &[x, y]| {
            [left.min(x), top.min(y)]
        });
    let max = outline
        .iter()
        .fold([f64::NEG_INFINITY; 2], |[right, bottom], &[x, y]| {
            [right.max(x), bottom.max(y)]
        });
    let mut css = "--shape-fill-polygon:".to_owned();
    for (index, [x, y]) in outline.into_iter().enumerate() {
        if index != 0 {
            css.push(',');
        }
        let x = (x - min[0]) / (max[0] - min[0]);
        let y = (y - min[1]) / (max[1] - min[1]);
        let _ = write!(css, "{x:.6} {y:.6}");
    }
    css.push(';');
    css
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
    if let Some(margins) = &text.margins {
        let insets = [margins.left, margins.top, margins.right, margins.bottom];
        if insets.iter().all(|edge| edge.is_finite()) && insets != [0.0; 4] {
            let [left, top, right, bottom] = insets;
            let _ = write!(style, "--text-insets:{left} {top} {right} {bottom};");
        }
    }
    if let Some(shadow) = &text.shadow
        && let Some(css) = text_shadow(shadow)
    {
        style.push_str(&css);
    }
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
        let stroke = attributes.stroke_width.abs();
        if stroke.is_finite() && stroke > 0.0 {
            let _ = write!(style, "-webkit-text-stroke-width:{stroke}px;");
            if let Some(color) = &attributes.stroke_color {
                let _ = write!(style, "-webkit-text-stroke-color:{};", css_color(color));
            }
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

fn text_shadow(shadow: &wire::TextShadow) -> Option<String> {
    if !shadow.enabled
        || ![shadow.angle, shadow.offset, shadow.radius, shadow.opacity]
            .iter()
            .all(|value| value.is_finite())
        || shadow.opacity <= 0.0
    {
        return None;
    }
    let (sin, cos) = shadow.angle.rem_euclid(360.0).to_radians().sin_cos();
    let offset = shadow.offset.clamp(0.0, 32768.0);
    let radius = shadow.radius.clamp(0.0, 32768.0);
    let mut color = shadow.color.clone().unwrap_or(wire::Color {
        alpha: 1.0,
        ..Default::default()
    });
    color.alpha *= shadow.opacity.min(1.0) as f32;
    Some(format!(
        "text-shadow:{:.3}px {:.3}px {radius}px {};",
        offset * cos,
        -offset * sin,
        css_color(&color),
    ))
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
        presentation.cues[1].actions = vec![
            background.clone(),
            audio.clone(),
            clear(wire::ClearTarget::Slide as i32),
        ];
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
        assert!(show.clears_slide_at(2));
        assert_eq!(show.delayed_actions_at(1)[0].kind, "clear_background");
        assert_eq!(show.delayed_actions_at(1)[0].time_ms, 1000);
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
        presentation.cues[1].actions = vec![clear(wire::ClearTarget::All as i32)];
        let show = crate::import::import_show("Clears.pro", &presentation.encode_to_vec()).unwrap();
        assert_eq!(show.backdrop_at(0), crate::SlideBackdrop::Clear);
        assert!(show.clears_all_at(2));
        assert!(show.clears_audio_at(2));
        assert!(show.clears_slide_at(2));
        assert!(show.show["media"].as_object().unwrap().is_empty());
    }

    #[test]
    fn imports_delayed_picture_and_audio_as_action_only_child_cues() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let mut foreground = presentation.cues[2].actions[1].clone();
        foreground.delay = 1.0;
        foreground.enabled = true;
        foreground.kind = wire::ActionType::ForegroundMedia as i32;
        let mut audio = foreground.clone();
        audio.delay = 0.25;
        audio.kind = wire::ActionType::Media as i32;
        let media = audio.media.as_mut().unwrap();
        media.audio = Some(wire::Audio::default());
        media.element.as_mut().unwrap().url = Some(wire::Url {
            relative: "Sound.wav".into(),
            ..Default::default()
        });
        presentation.cues[1].enabled = true;
        presentation.cues[1].actions = vec![
            foreground.clone(),
            audio,
            wire::Action {
                enabled: false,
                ..foreground
            },
        ];
        let show =
            crate::import::import_show("Delayed.pro", &presentation.encode_to_vec()).unwrap();
        let index = (0..show.ordered_slides().len())
            .find(|&index| show.delayed_actions_at(index).len() == 2)
            .unwrap();
        let id = &show.ordered_slides()[index].id;
        assert_eq!(show.show["slides"][id]["settings"]["actionOnly"], true);
        assert!(show.audio_at(index).is_empty());
        assert!(!matches!(
            show.backdrop_at(index),
            crate::SlideBackdrop::Picture {
                foreground: true,
                ..
            }
        ));
        let cues = show.delayed_actions_at(index);
        assert_eq!((cues[0].time_ms, cues[0].kind.as_str()), (250, "audio"));
        assert_eq!(
            show.show["media"][cues[0].data.as_deref().unwrap()]["path"],
            "Sound.wav"
        );
        assert_eq!(
            (cues[1].time_ms, cues[1].kind.as_str()),
            (1000, "foreground")
        );
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
                        ..Default::default()
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
        let mut polygon = shape(wire::ShapeType::Polygon as i32, true, false);
        polygon
            .element
            .as_mut()
            .unwrap()
            .path
            .as_mut()
            .unwrap()
            .shape
            .as_mut()
            .unwrap()
            .polygon = Some(wire::Polygon { number_sides: 6 });
        let mut star = shape(wire::ShapeType::Star as i32, true, false);
        star.element
            .as_mut()
            .unwrap()
            .path
            .as_mut()
            .unwrap()
            .shape
            .as_mut()
            .unwrap()
            .star = Some(wire::Star {
            number_points: 4,
            inner_radius: 0.35,
        });
        let effect_shape = |kind, effect| {
            let mut item = shape(kind as i32, true, false);
            item.element.as_mut().unwrap().fill = Some(wire::Fill {
                enabled: true,
                background_effect: Some(wire::BackgroundEffect { kind: Some(effect) }),
                ..Default::default()
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
            polygon,
            star,
            effect_shape(
                wire::ShapeType::Rhombus,
                wire::BackgroundEffectKind::Invert(wire::BackgroundInvert {}),
            ),
            effect_shape(
                wire::ShapeType::Ellipse,
                wire::BackgroundEffectKind::Blur(wire::BackgroundBlur {
                    saturation: 0.5,
                    blur_amount: 12.0,
                }),
            ),
        ]);

        let show = crate::import::import_show("Shapes.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        let items = show.show["slides"][&verse.id]["items"].as_array().unwrap();
        let style = "left:160px;top:90px;width:320px;height:180px;background-color:#ff000080;";
        assert_eq!(items.len(), 14);
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
            items[5..8]
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
        assert_eq!(
            items[10]["style"],
            format!(
                "{style}--shape-fill-polygon:0.500000 0.000000,1.000000 0.250000,1.000000 0.750000,0.500000 1.000000,0.000000 0.750000,0.000000 0.250000;"
            )
        );
        assert_eq!(
            items[11]["style"],
            format!(
                "{style}--shape-fill-polygon:0.500000 0.000000,0.623744 0.376256,1.000000 0.500000,0.623744 0.623744,0.500000 1.000000,0.376256 0.623744,0.000000 0.500000,0.376256 0.376256;"
            )
        );
        assert_eq!(
            items[12]["style"],
            format!(
                "{bounds}backdrop-filter:invert(1);--shape-fill-polygon:0.5 0,1 0.5,0.5 1,0 0.5;"
            )
        );
        assert_eq!(
            items[13]["style"],
            format!("{bounds}backdrop-filter:blur(12px) saturate(0.5);border-radius:50%;")
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
        text.margins = Some(wire::EdgeInsets {
            left: 8.0,
            right: 16.0,
            top: 24.0,
            bottom: 32.0,
        });
        text.shadow = Some(wire::TextShadow {
            angle: 315.0,
            offset: 10.0,
            radius: 3.0,
            color: Some(wire::Color {
                blue: 1.0,
                alpha: 0.5,
                ..Default::default()
            }),
            opacity: 0.5,
            enabled: true,
        });
        let attrs = text.attributes.as_mut().unwrap();
        attrs.stroke_width = 3.0;
        attrs.stroke_color = Some(wire::Color {
            red: 1.0,
            alpha: 1.0,
            ..Default::default()
        });
        let show = crate::import::import_show("Mixed.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        assert_eq!(verse.lines, ["A “red”—😀!", "Last", "Another text box"]);
        let item = &show.show["slides"][&verse.id]["items"][0];
        let style = item["style"].as_str().unwrap();
        assert!(style.contains("--text-insets:8 24 16 32;"));
        assert!(style.contains("text-shadow:7.071px 7.071px 3px #0000ff40;"));
        assert!(
            style.contains("-webkit-text-stroke-width:3px;-webkit-text-stroke-color:#ff0000ff;")
        );
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
    fn imports_rtf_paragraph_indents_and_spacing_into_the_slide_text_box() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let slide = presentation.cues[0].actions[0]
            .slide
            .as_mut()
            .unwrap()
            .presentation
            .as_mut()
            .unwrap();
        slide.slide.as_mut().unwrap().elements[0]
            .element
            .as_mut()
            .unwrap()
            .text
            .as_mut()
            .unwrap()
            .rtf =
            concat!(
                r"{\rtf1\pard\li400\ri200\fi-200\sb80\sa120\sl360\slmult1 First\line Continuation\uc0\u8232 Last line",
                "\\\n",
                r"{\pard\sl-480 Exact\par}{\pard\sl280 At least\par}\pard Unformatted}",
            ).as_bytes().to_vec();
        let show =
            crate::import::import_show("Paragraphs.pro", &presentation.encode_to_vec()).unwrap();
        let verse = &show.ordered_slides()[1];
        assert_eq!(
            verse.lines,
            [
                "First",
                "Continuation",
                "Last line",
                "Exact",
                "At least",
                "Unformatted",
                "Another text box"
            ]
        );
        let rows = &show.show["slides"][&verse.id]["items"][0]["lines"];
        assert_eq!(rows[0]["paragraphMargins"], json!([20, 10, -10]));
        assert_eq!(rows[1]["paragraphMargins"], json!([20, 10, 0]));
        assert_eq!(rows[0]["paragraphSpacing"], json!(["1.5lh", "4px", null]));
        assert_eq!(rows[1]["paragraphSpacing"], json!(["1.5lh", null, null]));
        assert_eq!(rows[2]["paragraphSpacing"], json!(["1.5lh", null, "6px"]));
        assert_eq!(rows[3]["paragraphSpacing"], json!(["24px", null, null]));
        assert_eq!(
            rows[4]["paragraphSpacing"],
            json!(["max(1lh,14px)", null, null])
        );
        assert!(rows[3].get("paragraphMargins").is_none());
        assert!(rows[5].get("paragraphSpacing").is_none());
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
    fn imports_standalone_and_timed_audio_actions() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        let audio = wire::Action {
            kind: wire::ActionType::Media as i32,
            enabled: true,
            media: Some(wire::MediaAction {
                element: Some(wire::Media {
                    url: Some(wire::Url {
                        relative: "Cue.wav".into(),
                        ..Default::default()
                    }),
                    audio: Some(wire::AudioProperties {
                        transport: Some(wire::Transport {
                            play_rate: 1.5,
                            in_point: 1.25,
                            out_point: 3.5,
                        }),
                    }),
                    ..Default::default()
                }),
                audio: Some(wire::Audio { playback: 1 }),
                always_retrigger: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        presentation.cues[1].actions = vec![audio.clone()];
        presentation.cues[1].enabled = true;
        presentation.timeline = Some(wire::Timeline {
            cues_v2: vec![wire::TimelineCue {
                trigger_time: 1.0,
                action: Some(wire::Action {
                    delay: 0.25,
                    ..audio
                }),
                ..Default::default()
            }],
            ..Default::default()
        });
        let show = crate::import::import_show("Audio.pro", &presentation.encode_to_vec()).unwrap();
        let slides = show.ordered_slides();
        let index = (0..slides.len())
            .find(|&index| show.audio_at(index) == ["Cue.wav"])
            .unwrap();
        assert_eq!(
            show.show["slides"][&slides[index].id]["settings"]["actionOnly"],
            true
        );
        assert!(
            !matches!(show.backdrop_at(index), crate::SlideBackdrop::Picture { path, .. } if path == "Cue.wav")
        );
        let cues = crate::timeline_cues_from_show(&show.show);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].time_ms, 1250);
        assert_eq!(cues[0].kind, "audio");
        let media = &show.show["media"][cues[0].data.as_deref().unwrap()];
        assert_eq!(media["path"], "Cue.wav");
        assert_eq!(media["loop"], true);
        assert_eq!(media["retrigger"], true);
        let transport = json!({"fromTime": 1.25, "toTime": 3.5, "speed": 1.5});
        assert_eq!(media["audioTransport"], transport);
        assert_eq!(show.audio_media_at(index)[0]["audioTransport"], transport);
    }

    #[test]
    fn timeline_soundtrack_is_copied_and_linked_with_bundle_media() {
        let bytes = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let mut presentation = wire::Presentation::decode(bytes.as_slice()).unwrap();
        presentation.timeline = Some(wire::Timeline {
            duration: 5.0,
            audio_action: Some(wire::Action {
                delay: 0.625,
                enabled: true,
                kind: wire::ActionType::Media as i32,
                media: Some(wire::MediaAction {
                    audio: Some(wire::Audio { playback: 0 }),
                    element: Some(wire::Media {
                        url: Some(wire::Url {
                            relative: "Backing.wav".into(),
                            absolute: String::new(),
                        }),
                        audio: Some(wire::AudioProperties {
                            transport: Some(wire::Transport {
                                play_rate: 0.75,
                                in_point: 0.5,
                                out_point: 3.25,
                            }),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        let mut bundle = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in [
            ("Song.pro", presentation.encode_to_vec()),
            ("Backing.wav", b"backing track".to_vec()),
            ("sky blue.png", b"background".to_vec()),
        ] {
            bundle
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut bundle, &bytes).unwrap();
        }
        let root = std::env::temp_dir().join(crate::new_id());
        let shows = super::super::propresenter::import_bundle(
            &bundle.finish().unwrap().into_inner(),
            &root,
        )
        .unwrap();
        let show = &shows[0].show;
        let (timeline, _) = crate::timeline_for_show(show).unwrap();
        assert_eq!(timeline["actions"], json!([]));
        assert_eq!(timeline["duration"], 5.0);
        assert_eq!(timeline["soundtrackDelayMs"], 625);
        let soundtrack = timeline["soundtrack"].as_str().unwrap();
        assert_eq!(
            show["media"][soundtrack]["audioTransport"],
            json!({"fromTime": 0.5, "toTime": 3.25, "speed": 0.75})
        );
        let path = std::path::Path::new(show["media"][soundtrack]["path"].as_str().unwrap());
        assert!(path.starts_with(root.canonicalize().unwrap()));
        assert_eq!(std::fs::read(path).unwrap(), b"backing track");
        std::fs::remove_dir_all(root).unwrap();
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
                        ..Default::default()
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
        let timed_clear = |time, delay, target, destination| wire::TimelineCue {
            trigger_time: time,
            name: "Recorded clear".into(),
            action: Some(wire::Action {
                delay,
                enabled: true,
                kind: wire::ActionType::Clear as i32,
                clear: Some(wire::ClearAction {
                    target: target as i32,
                    destination,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let timed_video = wire::TimelineCue {
            trigger_time: 1.5,
            name: "Motion".into(),
            action: Some(wire::Action {
                delay: 0.25,
                enabled: true,
                kind: wire::ActionType::BackgroundMedia as i32,
                media: Some(wire::MediaAction {
                    element: Some(wire::Media {
                        url: Some(wire::Url {
                            relative: "Loop.mp4".into(),
                            ..Default::default()
                        }),
                        video: Some(wire::VideoProperties {
                            transport: Some(wire::Transport {
                                play_rate: 1.5,
                                in_point: 0.25,
                                out_point: 2.0,
                            }),
                        }),
                        ..Default::default()
                    }),
                    video: Some(wire::Video {
                        playback: 1,
                        end_behavior: 4,
                    }),
                    always_retrigger: true,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut timed_foreground = timed_video.clone();
        timed_foreground.trigger_time = 2.0;
        timed_foreground.name = "Foreground".into();
        let foreground_action = timed_foreground.action.as_mut().unwrap();
        foreground_action.kind = wire::ActionType::Media as i32;
        foreground_action.media.as_mut().unwrap().layer = 1;
        presentation.cues[0].actions.push(wire::Action {
            delay: 0.0,
            ..foreground_action.clone()
        });
        presentation.timeline = Some(wire::Timeline {
            cues_v2: vec![
                wire::TimelineCue {
                    trigger_time: 1.5,
                    cue_id: cue_id.clone(),
                    name: "Verse".into(),
                    ..Default::default()
                },
                timed_clear(1.6, 0.4, wire::ClearTarget::Background, 0),
                timed_video,
                timed_foreground,
                timed_clear(2.5, 0.0, wire::ClearTarget::Audio, 0),
                timed_clear(2.5, 0.125, wire::ClearTarget::Slide, 0),
                timed_clear(2.6, 0.1, wire::ClearTarget::All, 0),
                timed_clear(2.75, 0.0, wire::ClearTarget::Audio, 1),
                timed_clear(2.9, -0.1, wire::ClearTarget::Audio, 0),
                timed_clear(3.0, f64::INFINITY, wire::ClearTarget::Audio, 0),
            ],
            duration: 12.0,
            r#loop: true,
            ..Default::default()
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
        assert_eq!(cues.len(), 7);
        assert_eq!(cues[0].time_ms, 1500);
        assert_eq!(cues[0].slide_index, Some(1));
        assert_eq!(cues[1].time_ms, 1750);
        assert_eq!(cues[1].kind, "background");
        let background = cues[1].data.as_deref().unwrap();
        assert_eq!(show.show["media"][background]["path"], "Loop.mp4");
        assert_eq!(show.show["media"][background]["loop"], true);
        assert_eq!(show.show["media"][background]["retrigger"], true);
        assert_eq!(show.show["media"][background]["endBehavior"], "fade_clear");
        assert_eq!(
            show.show["media"][background]["videoTransport"],
            json!({"fromTime":0.25,"toTime":2.0,"speed":1.5})
        );
        assert_eq!(cues[2].time_ms, 2000);
        assert_eq!(cues[2].kind, "clear_background");
        assert_eq!(cues[3].time_ms, 2250);
        assert_eq!(cues[3].kind, "foreground");
        let foreground = cues[3].data.as_deref().unwrap();
        assert_eq!(show.show["media"][foreground]["path"], "Loop.mp4");
        assert_eq!(show.show["media"][foreground]["videoType"], "foreground");
        assert_eq!(show.show["media"][foreground]["muted"], false);
        assert_eq!(
            show.backdrop_media_flags_at(1, "Loop.mp4"),
            Some((true, false, Some("foreground".into())))
        );
        assert!(matches!(
            show.backdrop_at(1),
            crate::SlideBackdrop::Picture {
                foreground: true,
                ..
            }
        ));
        assert_eq!(cues[4].time_ms, 2500);
        assert_eq!(cues[4].kind, "clear_audio");
        assert_eq!(cues[5].time_ms, 2625);
        assert_eq!(cues[5].kind, "clear_slide");
        assert_eq!(cues[6].time_ms, 2700);
        assert_eq!(cues[6].kind, "clear_all");
        assert_eq!(
            crate::timeline_text_from(&cues),
            format!(
                "1.500 slide:s1\n1.750 background:{background}\n2.000 clear_background\n2.250 foreground:{foreground}\n2.500 clear_audio\n2.625 clear_slide\n2.700 clear_all"
            )
        );
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
