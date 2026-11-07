// SPDX-License-Identifier: GPL-3.0-only

//! FreeShow `Config/stage.json` layouts. A missing file uses the default
//! current-slide, next-slide, notes, clock, and timer arrangement.

use std::fs;
use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageItemKind {
    Slide,
    Notes,
    Clock,
    Timer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageItemSpec {
    pub kind: StageItemKind,
    pub style: String,
    pub offset: i64,
    pub line_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageLayout {
    pub name: String,
    pub background: String,
    pub items: Vec<StageItemSpec>,
}

pub fn default_stage() -> StageLayout {
    StageLayout {
        name: "Default".into(),
        background: "#12121c".into(),
        items: vec![
            item(
                StageItemKind::Slide,
                "left:25px;top:25px;width:1870px;height:520px;color:#ffffff;font-size:72px;",
                0,
                None,
            ),
            item(
                StageItemKind::Slide,
                "left:25px;top:560px;width:1870px;height:250px;color:#aaaaaa;font-size:48px;",
                1,
                Some(2),
            ),
            item(
                StageItemKind::Notes,
                "left:25px;top:830px;width:1200px;height:140px;color:#f0f0ff;font-size:36px;",
                0,
                None,
            ),
            item(
                StageItemKind::Clock,
                "left:25px;top:980px;width:500px;height:80px;color:#ffffff;font-size:48px;",
                0,
                None,
            ),
            item(
                StageItemKind::Timer,
                "left:1400px;top:980px;width:490px;height:80px;color:#F0008C;font-size:48px;",
                0,
                None,
            ),
        ],
    }
}

pub fn load_stage_file(path: &Path) -> StageLayout {
    let Ok(text) = fs::read_to_string(path) else {
        return default_stage();
    };
    parse_stage(&text).unwrap_or_else(|_| default_stage())
}

pub fn parse_stage(text: &str) -> Result<StageLayout, ()> {
    let value: Value = serde_json::from_str(text).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    let layout = object
        .values()
        .find(|layout| layout.get("disabled").and_then(Value::as_bool) != Some(true))
        .ok_or(())?;
    layout_from(layout)
}

/// The layout stored under this id, including one marked disabled.
pub fn stage_by_id(text: &str, id: &str) -> Option<StageLayout> {
    let value: Value = serde_json::from_str(text).ok()?;
    let layout = value.get(id)?;
    layout_from(layout).ok()
}

fn layout_from(layout: &Value) -> Result<StageLayout, ()> {
    let name = layout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Stage")
        .to_string();
    let background = layout
        .pointer("/settings/color")
        .and_then(Value::as_str)
        .unwrap_or("#12121c")
        .to_string();
    let items_value = layout.get("items").and_then(Value::as_object).ok_or(())?;
    let order = layout
        .get("itemOrder")
        .and_then(Value::as_array)
        .map(|order| {
            order
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| items_value.keys().cloned().collect());
    let mut items = Vec::new();
    for id in order {
        let Some(item_value) = items_value.get(&id) else {
            continue;
        };
        if item_value.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let Some(kind) = kind_of(&id, item_value.get("type").and_then(Value::as_str)) else {
            continue;
        };
        items.push(StageItemSpec {
            kind,
            style: item_value
                .get("style")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            offset: item_value
                .get("slideOffset")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            line_count: item_value
                .get("lineCount")
                .and_then(Value::as_u64)
                .map(|count| count as usize),
        });
    }
    if items.is_empty() {
        return Err(());
    }
    Ok(StageLayout {
        name,
        background,
        items,
    })
}

fn item(kind: StageItemKind, style: &str, offset: i64, line_count: Option<usize>) -> StageItemSpec {
    StageItemSpec {
        kind,
        style: style.to_string(),
        offset,
        line_count,
    }
}

fn kind_of(id: &str, type_name: Option<&str>) -> Option<StageItemKind> {
    let id = id.to_ascii_lowercase();
    match type_name.unwrap_or("").to_ascii_lowercase().as_str() {
        "slide_text" => Some(StageItemKind::Slide),
        "slide_notes" => Some(StageItemKind::Notes),
        "clock" => Some(StageItemKind::Clock),
        "timer" => Some(StageItemKind::Timer),
        _ if id.contains("notes") => Some(StageItemKind::Notes),
        _ if id.contains("clock") => Some(StageItemKind::Clock),
        _ if id.contains("timer") => Some(StageItemKind::Timer),
        _ if id.contains("slide") || id.contains("text") => Some(StageItemKind::Slide),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_freeshow_stage_layout_and_falls_back() {
        let layout = parse_stage(
            r##"{"default":{"name":"Main","disabled":false,"settings":{"color":"#101010"},"itemOrder":["textNext","textCurrent","notes"],"items":{"textCurrent":{"type":"slide_text","style":"left:1px;"},"textNext":{"type":"slide_text","slideOffset":1,"lineCount":2,"style":"left:2px;"},"notes":{"type":"slide_notes","style":"left:3px;"},"hidden":{"type":"clock","enabled":false,"style":""}}}}"##,
        )
        .unwrap();
        assert_eq!(layout.name, "Main");
        assert_eq!(layout.background, "#101010");
        assert_eq!(layout.items[0].kind, StageItemKind::Slide);
        assert_eq!(layout.items[0].offset, 1);
        assert_eq!(layout.items[0].line_count, Some(2));
        assert_eq!(layout.items[2].kind, StageItemKind::Notes);
        assert!(
            load_stage_file(Path::new("/no/such/stage.json"))
                .items
                .len()
                >= 4
        );
    }
}
