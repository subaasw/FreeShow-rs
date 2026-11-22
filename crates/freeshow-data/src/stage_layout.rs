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
    /// A custom text box stored with the layout.
    Text,
    /// The value of a variable from `settings_synced.json`.
    Variable,
    /// The current slide number out of the total.
    SlideTracker,
    /// An image or video file from the library.
    Media,
    /// A live camera feed from the discovered native devices.
    Camera,
    CurrentOutput,
    QrCode,
    /// A web page preview; a live browser is not embedded.
    Web,
    /// A bar, line, or pie chart from stored `chart` data.
    Chart,
    /// A FreeShow item this app does not draw yet, shown as its label.
    Custom,
}

/// Clock options stored on a FreeShow stage item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageClockOptions {
    /// FreeShow defaults to showing seconds.
    pub seconds: bool,
    /// For example `DD/MM/YYYY`; `none` hides the date.
    pub date_format: String,
}

impl Default for StageClockOptions {
    fn default() -> Self {
        Self {
            seconds: true,
            date_format: "none".into(),
        }
    }
}

/// Content and colors shared by audience and stage QR items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrItem {
    pub text: String,
    pub color: String,
    pub background: String,
}

impl QrItem {
    pub fn parse(item: &Value) -> Option<Self> {
        let data = item.get("qr_code")?;
        Some(Self {
            text: data.get("text")?.as_str()?.to_owned(),
            color: data
                .get("color")
                .and_then(Value::as_str)
                .unwrap_or("#000000")
                .into(),
            background: data
                .get("background")
                .and_then(Value::as_str)
                .unwrap_or("#FFFFFF")
                .into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartItem {
    pub kind: String,
    pub data: String,
    pub hole_size: u8,
}

impl ChartItem {
    pub fn parse(item: &Value) -> Option<Self> {
        let data = item.get("chart")?;
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("bar")
            .to_owned();
        let chart_data = data
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if kind.is_empty() && chart_data.is_empty() {
            return None;
        }
        let hole_size = data
            .get("holeSize")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(100) as u8;
        Some(Self {
            kind: if kind.is_empty() { "bar".into() } else { kind },
            data: chart_data,
            hole_size,
        })
    }

    pub fn samples(&self) -> Vec<ChartSample> {
        parse_chart_data(&self.data)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSample {
    pub label: String,
    pub value: f64,
    pub color: String,
}

fn parse_chart_data(data: &str) -> Vec<ChartSample> {
    let grid = parse_chart_grid(data);
    let has_header = grid.first().is_some_and(|row| {
        row.first()
            .is_some_and(|label| label.eq_ignore_ascii_case("label"))
            && row
                .get(1)
                .is_some_and(|value| parse_chart_value(value).is_none())
    });
    let start = if has_header { 1 } else { 0 };
    grid[start..]
        .iter()
        .map(|row| {
            let label = row.first().cloned().unwrap_or_default();
            let value = row
                .get(1)
                .and_then(|cell| parse_chart_value(cell))
                .unwrap_or(0.0);
            let color = row.get(2).cloned().unwrap_or_default();
            ChartSample {
                label,
                value,
                color,
            }
        })
        .collect()
}

fn parse_chart_grid(data: &str) -> Vec<Vec<String>> {
    if let Ok(rows) = serde_json::from_str::<Vec<Vec<Value>>>(data) {
        return rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| match cell {
                        Value::String(text) => text,
                        Value::Number(number) => number.to_string(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect();
    }
    data.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            if let Some((label, value)) = line.split_once(':') {
                vec![label.trim().to_string(), value.trim().to_string()]
            } else {
                vec![line.trim().to_string(), "0".into()]
            }
        })
        .collect()
}

fn parse_chart_value(text: &str) -> Option<f64> {
    let text = text.trim();
    let text = text.strip_suffix('%').unwrap_or(text).trim();
    text.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageItemSpec {
    pub kind: StageItemKind,
    /// The key this item has in `stage.json`.
    pub id: String,
    pub style: String,
    pub align: String,
    pub offset: i64,
    pub line_count: Option<usize>,
    /// The saved one-based box number as a zero-based index.
    pub text_box: Option<usize>,
    pub invert_items: bool,
    /// Explicit display name for a box; empty when the layout has none.
    pub label: String,
    /// Static lines for a `text` item.
    pub lines: Vec<String>,
    /// Variable id for a `variable` item.
    pub variable_id: Option<String>,
    /// Timer id for a `timer` item. Empty uses the running timer.
    pub timer_id: Option<String>,
    /// Media path for a `media` item.
    pub src: Option<String>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub output_source: Option<String>,
    pub show_output_label: bool,
    pub include_media: bool,
    pub fit: Option<String>,
    pub flip_x: bool,
    pub flip_y: bool,
    pub qr: Option<QrItem>,
    pub url: Option<String>,
    pub chart: Option<ChartItem>,
    pub clock: StageClockOptions,
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
        let kind = kind_of(&id, item_value);
        let legacy = item_value
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        let label = item_value
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
            .unwrap_or("")
            .to_string();
        let legacy_id = id.split('#').nth(1);
        let timer_id = item_value
            .pointer("/timer/id")
            .and_then(Value::as_str)
            .or(legacy_id.filter(|_| kind == StageItemKind::Timer))
            .map(str::to_string);
        let variable_id = item_value
            .pointer("/variable/id")
            .and_then(Value::as_str)
            .or(legacy_id.filter(|_| kind == StageItemKind::Variable))
            .map(str::to_string);
        items.push(StageItemSpec {
            kind,
            id: id.clone(),
            style: item_value
                .get("style")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            align: item_value
                .get("align")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            offset: item_value
                .get("slideOffset")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| i64::from(legacy && id.contains("next"))),
            line_count: item_value
                .get("lineCount")
                .and_then(Value::as_u64)
                .map(|count| count as usize),
            text_box: item_value
                .get("itemNumber")
                .and_then(Value::as_u64)
                .and_then(|number| number.checked_sub(1))
                .and_then(|index| usize::try_from(index).ok()),
            invert_items: item_value.get("invertItems") == Some(&Value::Bool(true)),
            label,
            lines: if kind == StageItemKind::Text {
                text_lines(item_value)
            } else {
                Vec::new()
            },
            variable_id,
            timer_id,
            src: item_value
                .get("src")
                .and_then(Value::as_str)
                .filter(|src| !src.is_empty())
                .map(str::to_string),
            device_name: item_value
                .pointer("/device/name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
            device_type: item_value
                .pointer("/device/type")
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
                .map(str::to_string),
            output_source: item_value
                .pointer("/currentOutput/source")
                .and_then(Value::as_str)
                .filter(|source| !source.is_empty())
                .map(str::to_string),
            show_output_label: item_value.pointer("/currentOutput/showLabel")
                == Some(&Value::Bool(true)),
            include_media: kind == StageItemKind::Slide
                && if legacy {
                    !id.contains("_text")
                } else {
                    item_value["includeMedia"].as_bool() == Some(true)
                },
            fit: item_value["fit"].as_str().map(str::to_owned),
            flip_x: item_value["flipped"].as_bool() == Some(true),
            flip_y: item_value["flippedY"].as_bool() == Some(true),
            qr: QrItem::parse(item_value),
            url: web_url(item_value),
            chart: ChartItem::parse(item_value),
            clock: StageClockOptions {
                seconds: item_value
                    .pointer("/clock/seconds")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                date_format: item_value
                    .pointer("/clock/dateFormat")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        if legacy
                            && item_value.pointer("/clock/show_date") == Some(&Value::Bool(true))
                        {
                            "DD/MM/YYYY"
                        } else {
                            "none"
                        }
                    })
                    .to_string(),
            },
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

fn web_url(item: &Value) -> Option<String> {
    for pointer in ["/web/src", "/web/url", "/url"] {
        if let Some(url) = item
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        {
            return Some(url.to_string());
        }
    }
    item.get("src")
        .and_then(Value::as_str)
        .filter(|src| !src.is_empty() && looks_like_url(src))
        .map(str::to_string)
}

fn looks_like_url(text: &str) -> bool {
    let text = text.trim();
    text.contains("://") || text.starts_with("www.")
}

fn item(kind: StageItemKind, style: &str, offset: i64, line_count: Option<usize>) -> StageItemSpec {
    StageItemSpec {
        kind,
        id: String::new(),
        style: style.to_string(),
        align: String::new(),
        offset,
        line_count,
        text_box: None,
        invert_items: false,
        label: String::new(),
        lines: Vec::new(),
        variable_id: None,
        timer_id: None,
        src: None,
        device_name: None,
        device_type: None,
        output_source: None,
        show_output_label: false,
        include_media: false,
        fit: None,
        flip_x: false,
        flip_y: false,
        qr: None,
        url: None,
        chart: None,
        clock: StageClockOptions::default(),
    }
}

/// Flattens a stage text box's runs into one string per line.
fn text_lines(item: &Value) -> Vec<String> {
    item.get("lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| {
                    if let Some(runs) = line.get("text").and_then(Value::as_array) {
                        return Some(
                            runs.iter()
                                .filter_map(|run| run.get("value").and_then(Value::as_str))
                                .collect::<String>(),
                        );
                    }
                    line.get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn kind_of(id: &str, item: &Value) -> StageItemKind {
    let id = id.to_ascii_lowercase();
    match item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "slide_text" => StageItemKind::Slide,
        "slide_notes" => StageItemKind::Notes,
        "clock" => StageItemKind::Clock,
        "timer" => StageItemKind::Timer,
        "text" => StageItemKind::Text,
        "variable" => StageItemKind::Variable,
        "slide_tracker" => StageItemKind::SlideTracker,
        "media" => StageItemKind::Media,
        "camera" => StageItemKind::Camera,
        "current_output" => StageItemKind::CurrentOutput,
        "qr_code" => StageItemKind::QrCode,
        "web" => StageItemKind::Web,
        "chart" => StageItemKind::Chart,
        "" => legacy_kind(&id),
        _ => StageItemKind::Custom,
    }
}

/// Layouts older than FreeShow 1.4 stored the item type in the id.
fn legacy_kind(id: &str) -> StageItemKind {
    if id.contains("current_output") {
        StageItemKind::CurrentOutput
    } else if id.contains("tracker") {
        StageItemKind::SlideTracker
    } else if id.contains("notes") {
        StageItemKind::Notes
    } else if id.contains("clock") {
        StageItemKind::Clock
    } else if id.contains("timer") {
        StageItemKind::Timer
    } else if id.contains("variable") {
        StageItemKind::Variable
    } else if id.contains("slide") || id.contains("text") {
        StageItemKind::Slide
    } else {
        StageItemKind::Custom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_freeshow_stage_layout_and_falls_back() {
        let layout = parse_stage(
            r##"{"default":{"name":"Main","disabled":false,"settings":{"color":"#101010"},"itemOrder":["textNext","textCurrent","notes","caption","var","timer","media","web","chart"],"items":{"textCurrent":{"type":"slide_text","style":"left:1px;"},"textNext":{"type":"slide_text","slideOffset":1,"lineCount":2,"itemNumber":2,"invertItems":true,"style":"left:2px;"},"notes":{"type":"slide_notes","style":"left:3px;"},"caption":{"type":"text","label":"Welcome","style":"left:4px;","lines":[{"align":"","text":[{"value":"Hello "},{"value":"world"}]}]},"var":{"type":"variable","variable":{"id":"v1"},"style":"left:5px;"},"timer":{"type":"timer","timer":{"id":"sermon"},"style":"left:6px;"},"media":{"type":"media","src":"Media/bg.png","style":"left:7px;"},"web":{"type":"web","style":"left:8px;","web":{"src":"https://example.com/lyrics"}},"chart":{"type":"chart","style":"left:9px;","chart":{"type":"pie","data":"A: 10\nB: 20","holeSize":40}},"hidden":{"type":"clock","enabled":false,"style":""}}}}"##,
        )
        .unwrap();
        assert_eq!(layout.name, "Main");
        assert_eq!(layout.background, "#101010");
        assert_eq!(layout.items[0].kind, StageItemKind::Slide);
        assert_eq!(layout.items[0].offset, 1);
        assert_eq!(layout.items[0].line_count, Some(2));
        assert_eq!(layout.items[0].text_box, Some(1));
        assert!(layout.items[0].invert_items);
        assert_eq!(layout.items[2].kind, StageItemKind::Notes);
        assert_eq!(layout.items[3].kind, StageItemKind::Text);
        assert_eq!(layout.items[3].label, "Welcome");
        assert_eq!(layout.items[3].lines, vec!["Hello world".to_string()]);
        assert_eq!(layout.items[4].kind, StageItemKind::Variable);
        assert_eq!(layout.items[4].variable_id.as_deref(), Some("v1"));
        assert_eq!(layout.items[5].kind, StageItemKind::Timer);
        assert_eq!(layout.items[5].timer_id.as_deref(), Some("sermon"));
        assert_eq!(layout.items[6].kind, StageItemKind::Media);
        assert_eq!(layout.items[6].src.as_deref(), Some("Media/bg.png"));
        assert_eq!(layout.items[7].kind, StageItemKind::Web);
        assert_eq!(
            layout.items[7].url.as_deref(),
            Some("https://example.com/lyrics")
        );
        assert_eq!(layout.items[8].kind, StageItemKind::Chart);
        let chart = layout.items[8].chart.as_ref().expect("chart data");
        assert_eq!(chart.kind, "pie");
        assert_eq!(chart.hole_size, 40);
        assert_eq!(chart.samples()[0].label, "A");
        assert_eq!(chart.samples()[0].value, 10.0);
        assert_eq!(chart.samples()[1].label, "B");
        assert_eq!(chart.samples()[1].value, 20.0);
        assert_eq!(layout.items[7].id, "web");
        assert_eq!(layout.items[7].label, "");
        assert!(
            load_stage_file(Path::new("/no/such/stage.json"))
                .items
                .len()
                >= 4
        );
    }

    #[test]
    fn legacy_item_ids_keep_their_type() {
        let layout = parse_stage(
            r##"{"old":{"name":"Old","itemOrder":["slide_text","slide_notes","first_active_timer","variables#v1","slide_tracker","clock"],"items":{"slide_text":{"style":""},"slide_notes":{"style":""},"first_active_timer":{"style":""},"variables#v1":{"style":""},"slide_tracker":{"style":""},"clock":{"style":""}}}}"##,
        )
        .unwrap();
        assert_eq!(layout.items[0].kind, StageItemKind::Slide);
        assert_eq!(layout.items[1].kind, StageItemKind::Notes);
        assert_eq!(layout.items[2].kind, StageItemKind::Timer);
        assert_eq!(layout.items[3].kind, StageItemKind::Variable);
        assert_eq!(layout.items[3].variable_id.as_deref(), Some("v1"));
        assert_eq!(layout.items[4].kind, StageItemKind::SlideTracker);
        assert_eq!(layout.items[5].kind, StageItemKind::Clock);
    }

    #[test]
    fn clock_options_default_to_seconds_and_no_date() {
        let layout = parse_stage(
            r##"{"clock":{"name":"Clock","items":{"c":{"type":"clock","clock":{"seconds":false,"dateFormat":"DD/MM/YYYY"},"style":""}}}}"##,
        )
        .unwrap();
        assert!(!layout.items[0].clock.seconds);
        assert_eq!(layout.items[0].clock.date_format, "DD/MM/YYYY");
    }

    #[test]
    fn legacy_next_and_clock_options_match_freeshow() {
        let layout = parse_stage(
            r#"{"old":{"itemOrder":["next_slide_text","next_slide_notes","next_modern","clock"],"items":{
                "next_slide_text":{},
                "next_slide_notes":{},
                "next_modern":{"type":"slide_text"},
                "clock":{"clock":{"seconds":false,"show_date":true}}
            }}}"#,
        )
        .unwrap();
        assert_eq!(layout.items[0].kind, StageItemKind::Slide);
        assert_eq!(layout.items[0].offset, 1);
        assert_eq!(layout.items[1].kind, StageItemKind::Notes);
        assert_eq!(layout.items[1].offset, 1);
        assert_eq!(layout.items[2].offset, 0);
        assert!(!layout.items[3].clock.seconds);
        assert_eq!(layout.items[3].clock.date_format, "DD/MM/YYYY");
    }

    #[test]
    fn camera_items_keep_their_saved_device() {
        let layout = parse_stage(
            r##"{"stage":{"name":"Cams","items":{"cam":{"type":"camera","device":{"name":"PTZ Front","type":"camera"},"style":"left:9px;"}}}}"##,
        )
        .unwrap();
        assert_eq!(layout.items[0].kind, StageItemKind::Camera);
        assert_eq!(layout.items[0].id, "cam");
        assert_eq!(layout.items[0].device_name.as_deref(), Some("PTZ Front"));
        assert_eq!(layout.items[0].device_type.as_deref(), Some("camera"));
    }

    #[test]
    fn chart_data_parses_json_grids_and_labeled_lines() {
        let json = ChartItem {
            kind: "bar".into(),
            data: r##"[["Label","Value","Color"],["A","10","#ff0000"],["B","20%"]]"##.into(),
            hole_size: 0,
        };
        let samples = json.samples();
        assert_eq!(samples[0].label, "A");
        assert_eq!(samples[0].value, 10.0);
        assert_eq!(samples[0].color, "#ff0000");
        assert_eq!(samples[1].value, 20.0);
        let lines = ChartItem {
            kind: "bar".into(),
            data: "One: 3\nTwo: 7".into(),
            hole_size: 0,
        };
        assert_eq!(lines.samples()[0].label, "One");
        assert_eq!(lines.samples()[1].value, 7.0);
    }
}
