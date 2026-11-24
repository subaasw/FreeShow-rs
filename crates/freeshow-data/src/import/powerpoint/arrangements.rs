// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{Archive, invalid, read_xml, relationships, theme::child};
use crate::{DataError, ShowFile};

const PRESENTATION: &str = "ppt/presentation.xml";

#[derive(Default)]
enum Selection {
    #[default]
    All,
    Range(usize, usize),
    Custom(u32),
    Invalid,
}

pub(super) struct Playback {
    pub(super) use_timings: bool,
    repeat: bool,
    selection: Selection,
}

impl Playback {
    pub(super) fn without_timings() -> Self {
        Self {
            use_timings: false,
            repeat: false,
            selection: Selection::All,
        }
    }

    pub(super) fn read(archive: &mut Archive<'_>) -> Result<Self, DataError> {
        let mut playback = Self {
            use_timings: true,
            ..Self::without_timings()
        };
        let rels = relationships(archive, PRESENTATION)?;
        let Some(props) = rels
            .iter()
            .find(|rel| rel.kind == "presProps" && !rel.external)
        else {
            return Ok(playback);
        };
        let xml = read_xml(archive, &props.target)?
            .ok_or_else(|| invalid("presentation properties are missing"))?;
        let doc = roxmltree::Document::parse(&xml).map_err(invalid)?;
        let root = doc.root_element();
        if !root.has_tag_name("presentationPr") {
            return Err(invalid("expected presentation properties"));
        }
        let Some(show) = child(root, "showPr") else {
            return Ok(playback);
        };
        playback.use_timings = !matches!(show.attribute("useTimings"), Some("0" | "false"));
        playback.repeat = matches!(show.attribute("loop"), Some("1" | "true"));
        playback.selection = if let Some(range) = child(show, "sldRg") {
            let read = |key| {
                range
                    .attribute(key)
                    .and_then(|value| value.trim().parse::<usize>().ok())
            };
            match (read("st"), read("end")) {
                (Some(start), Some(end)) if start > 0 && end >= start => {
                    Selection::Range(start, end)
                }
                _ => Selection::Invalid,
            }
        } else if let Some(custom) = child(show, "custShow") {
            custom
                .attribute("id")
                .and_then(|id| id.trim().parse().ok())
                .map_or(Selection::Invalid, Selection::Custom)
        } else {
            Selection::All
        };
        Ok(playback)
    }

    pub(super) fn install(
        &self,
        show: &mut ShowFile,
        archive: &mut Archive<'_>,
        presentation: Option<&str>,
        parts: &[String],
        warnings: &mut Vec<String>,
    ) {
        let main = show.show["layouts"]["main"]["slides"].as_array().unwrap();
        let mut layouts =
            match custom_arrangements(archive, presentation, parts, main, self.repeat, warnings) {
                Ok(layouts) => layouts,
                Err(error) => {
                    warnings.push(format!("Could not import custom shows: {error}"));
                    Map::new()
                }
            };
        let active = match self.selection {
            Selection::All => Some("main".into()),
            Selection::Range(start, end) if end <= main.len() => {
                let id = "powerpoint-range".to_owned();
                layouts.insert(
                    id.clone(),
                    arrangement(
                        &format!("Slides {start}–{end}"),
                        main[start - 1..end].to_vec(),
                        self.repeat,
                    ),
                );
                Some(id)
            }
            Selection::Custom(id) => {
                let layout = layout_id(id);
                layouts.contains_key(&layout).then_some(layout)
            }
            _ => None,
        };
        let stored = show.show["layouts"].as_object_mut().unwrap();
        if !layouts.is_empty() {
            stored.get_mut("main").unwrap()["name"] = json!("All slides");
            stored.extend(layouts);
        }
        set_loop(
            stored.get_mut("main").unwrap()["slides"]
                .as_array_mut()
                .unwrap(),
            self.repeat,
        );
        if let Some(active) = active {
            show.set_active_layout(&active);
        } else {
            warnings.push(
                "Could not select the saved slide range or custom show. Imported all slides."
                    .into(),
            );
        }
    }
}

fn custom_arrangements(
    archive: &mut Archive<'_>,
    presentation: Option<&str>,
    parts: &[String],
    main: &[Value],
    repeat: bool,
    warnings: &mut Vec<String>,
) -> Result<Map<String, Value>, DataError> {
    let mut layouts = Map::new();
    let Some(xml) = presentation.filter(|xml| xml.contains("custShowLst")) else {
        return Ok(layouts);
    };
    let document = roxmltree::Document::parse(xml).map_err(invalid)?;
    let Some(list) = child(document.root_element(), "custShowLst") else {
        return Ok(layouts);
    };
    let parts: HashMap<_, _> = parts
        .iter()
        .enumerate()
        .map(|(index, part)| (part.as_str(), index))
        .collect();
    let rels = relationships(archive, PRESENTATION)?;
    let steps: HashMap<_, _> = rels
        .iter()
        .filter(|rel| rel.kind == "slide" && !rel.external)
        .filter_map(|rel| Some((rel.id.as_str(), *parts.get(rel.target.as_str())?)))
        .collect();
    let mut total_steps = 0;
    for custom in list.children().filter(|node| node.has_tag_name("custShow")) {
        let name = custom.attribute("name").unwrap_or("").trim();
        let id = custom
            .attribute("id")
            .and_then(|id| id.trim().parse().ok())
            .map(layout_id);
        let order = child(custom, "sldLst").and_then(|slides| {
            slides
                .children()
                .filter(|node| node.has_tag_name("sld"))
                .map(|slide| {
                    let id = slide
                        .attributes()
                        .find(|attr| attr.name() == "id" && attr.namespace().is_some())?
                        .value();
                    steps.get(id).copied()
                })
                .collect::<Option<Vec<_>>>()
        });
        let Some((id, order)) = id
            .zip(order)
            .filter(|(id, order)| !order.is_empty() && !layouts.contains_key(id))
        else {
            warnings.push(format!(
                "Could not import custom show {name:?}: invalid ID or slide list"
            ));
            continue;
        };
        total_steps += order.len();
        if total_steps > 100_000 {
            warnings
                .push("Could not import the remaining custom shows: too many slide steps".into());
            break;
        }
        layouts.insert(
            id,
            arrangement(
                name,
                order.into_iter().map(|index| main[index].clone()).collect(),
                repeat,
            ),
        );
    }
    Ok(layouts)
}

fn layout_id(id: u32) -> String {
    format!("powerpoint-custom-{id}")
}

fn arrangement(name: &str, mut order: Vec<Value>, repeat: bool) -> Value {
    set_loop(&mut order, repeat);
    json!({"name": name, "notes": "", "slides": order})
}

fn set_loop(order: &mut [Value], repeat: bool) {
    if repeat
        && let Some(last) = order
            .iter_mut()
            .rfind(|step| step["disabled"].as_bool() != Some(true))
        && last.get("nextTimer").is_some()
    {
        last["nextTimerTarget"] = json!("first");
    }
}
