// SPDX-License-Identifier: GPL-3.0-only

use serde_json::Value;

use crate::ShowFile;

const DEFAULT_BOX: &str = "top:88px;left:50px;height:904px;width:1820px;";
const DEFAULT_FONT_PX: i32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTypography {
    pub font_px: i32,
    pub family: String,
    pub bold: bool,
}

impl ShowFile {
    pub fn item_count(&self, slide_id: &str) -> usize {
        self.items(slide_id).map(Vec::len).unwrap_or(0)
    }

    pub fn item_text(&self, slide_id: &str, index: usize) -> Option<String> {
        let item = self.items(slide_id)?.get(index)?;
        let lines = item.get("lines")?.as_array()?;
        let mut text = Vec::new();
        for line in lines {
            let mut row = String::new();
            if let Some(parts) = line.get("text").and_then(Value::as_array) {
                for part in parts {
                    if let Some(value) = part.get("value").and_then(Value::as_str) {
                        row.push_str(value);
                    }
                }
            }
            text.push(row);
        }
        Some(text.join("\n"))
    }

    pub fn set_item_text(&mut self, slide_id: &str, index: usize, text: &str) -> bool {
        if !self.ensure_item(slide_id, index) {
            return false;
        }
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        let lines_value = item
            .entry("lines")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(lines) = lines_value.as_array_mut() else {
            return false;
        };
        let new_lines: Vec<&str> = if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').collect()
        };
        lines.truncate(new_lines.len());
        while lines.len() < new_lines.len() {
            lines.push(serde_json::json!({
                "align": "",
                "text": [{ "value": "", "style": "" }]
            }));
        }
        for (line, text) in lines.iter_mut().zip(new_lines) {
            let Some(line) = line.as_object_mut() else {
                continue;
            };
            let parts = line
                .entry("text")
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(parts) = parts.as_array_mut() else {
                continue;
            };
            if parts.is_empty() {
                parts.push(serde_json::json!({ "value": text, "style": "" }));
            } else if let Some(first) = parts[0].as_object_mut() {
                first.insert("value".to_string(), Value::String(text.to_string()));
                parts.truncate(1);
            }
        }
        true
    }

    pub fn add_item_box(&mut self, slide_id: &str) -> Option<usize> {
        let count = self.item_count(slide_id);
        let style = offset_box(DEFAULT_BOX, 40 * count as i32, 40 * count as i32);
        let items = self.items_mut(slide_id)?;
        items.push(serde_json::json!({
            "style": style,
            "lines": [{ "align": "", "text": [{ "value": "", "style": "" }] }]
        }));
        Some(items.len() - 1)
    }

    pub fn remove_item_box(&mut self, slide_id: &str, index: usize) -> bool {
        let Some(items) = self.items_mut(slide_id) else {
            return false;
        };
        if items.len() <= 1 || index >= items.len() {
            return false;
        }
        items.remove(index);
        true
    }

    pub fn nudge_item_at(
        &mut self,
        slide_id: &str,
        index: usize,
        dx: i32,
        dy: i32,
        dw: i32,
        dh: i32,
    ) -> bool {
        let style = self
            .item(slide_id, index)
            .and_then(|item| item.get("style"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_BOX.to_string());
        let next = crate::adjust_box(&style, dx, dy, dw, dh);
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        item.insert("style".to_string(), Value::String(next));
        true
    }

    pub fn item_typography(&self, slide_id: &str, index: usize) -> ItemTypography {
        let style = self.item_style_text(slide_id, index);
        ItemTypography {
            font_px: css_px(&style, "font-size").unwrap_or(DEFAULT_FONT_PX),
            family: css_value(&style, "font-family").unwrap_or_default(),
            bold: css_value(&style, "font-weight")
                .is_some_and(|value| value.eq_ignore_ascii_case("bold") || value == "700"),
        }
    }

    pub fn adjust_item_font_size(&mut self, slide_id: &str, index: usize, delta: i32) -> bool {
        let typography = self.item_typography(slide_id, index);
        let size = (typography.font_px + delta).clamp(12, 400);
        self.set_item_css(slide_id, index, "font-size", &format!("{size}px"))
    }

    pub fn toggle_item_bold(&mut self, slide_id: &str, index: usize) -> bool {
        if self.item_typography(slide_id, index).bold {
            self.set_item_css(slide_id, index, "font-weight", "")
        } else {
            self.set_item_css(slide_id, index, "font-weight", "bold")
        }
    }

    pub fn set_item_font_family(&mut self, slide_id: &str, index: usize, family: &str) -> bool {
        let family = family.trim();
        if family.is_empty() {
            self.set_item_css(slide_id, index, "font-family", "")
        } else {
            self.set_item_css(slide_id, index, "font-family", family)
        }
    }

    fn item_style_text(&self, slide_id: &str, index: usize) -> String {
        let Some(item) = self.item(slide_id, index) else {
            return String::new();
        };
        let item_style = item.get("style").and_then(Value::as_str).unwrap_or("");
        let run_style = item
            .get("lines")
            .and_then(Value::as_array)
            .and_then(|lines| lines.first())
            .and_then(|line| line.get("text"))
            .and_then(Value::as_array)
            .and_then(|parts| parts.first())
            .and_then(|part| part.get("style"))
            .and_then(Value::as_str)
            .unwrap_or("");
        format!("{item_style};{run_style}")
    }

    fn set_item_css(&mut self, slide_id: &str, index: usize, key: &str, value: &str) -> bool {
        if !self.ensure_item(slide_id, index) {
            return false;
        }
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        let lines = item
            .entry("lines")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(lines) = lines.as_array_mut() else {
            return false;
        };
        if lines.is_empty() {
            lines.push(serde_json::json!({
                "align": "",
                "text": [{ "value": "", "style": "" }]
            }));
        }
        let mut wrote = false;
        for line in lines {
            let Some(parts) = line
                .as_object_mut()
                .and_then(|line| line.get_mut("text"))
                .and_then(Value::as_array_mut)
            else {
                continue;
            };
            if parts.is_empty() {
                parts.push(serde_json::json!({ "value": "", "style": "" }));
            }
            for part in parts {
                let Some(part) = part.as_object_mut() else {
                    continue;
                };
                let style = part
                    .get("style")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                part.insert(
                    "style".to_string(),
                    Value::String(css_set(&style, key, value)),
                );
                wrote = true;
            }
        }
        wrote
    }

    fn items(&self, slide_id: &str) -> Option<&Vec<Value>> {
        self.show
            .pointer(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array)
    }

    fn items_mut(&mut self, slide_id: &str) -> Option<&mut Vec<Value>> {
        self.show
            .pointer_mut(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array_mut)
    }

    fn item(&self, slide_id: &str, index: usize) -> Option<&Value> {
        self.items(slide_id)?.get(index)
    }

    fn item_mut(
        &mut self,
        slide_id: &str,
        index: usize,
    ) -> Option<&mut serde_json::Map<String, Value>> {
        self.items_mut(slide_id)?
            .get_mut(index)
            .and_then(Value::as_object_mut)
    }

    fn ensure_item(&mut self, slide_id: &str, index: usize) -> bool {
        let Some(items) = self
            .show
            .pointer(&format!("/slides/{slide_id}/items"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        index < items.len()
    }
}

fn offset_box(style: &str, dx: i32, dy: i32) -> String {
    crate::adjust_box(style, dx, dy, 0, 0)
}

fn css_value(style: &str, key: &str) -> Option<String> {
    for decl in style.split(';') {
        let decl = decl.trim();
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn css_px(style: &str, key: &str) -> Option<i32> {
    let value = css_value(style, key)?;
    let number = value
        .trim()
        .trim_end_matches("px")
        .trim_end_matches("pt")
        .trim();
    number.parse().ok()
}

fn css_set(style: &str, key: &str, value: &str) -> String {
    let mut rest = Vec::new();
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, _)) = decl.split_once(':') else {
            rest.push(decl.to_string());
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(key) {
            rest.push(decl.to_string());
        }
    }
    if !value.is_empty() {
        rest.push(format!("{key}: {value}"));
    }
    if rest.is_empty() {
        String::new()
    } else {
        format!("{};", rest.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show() -> ShowFile {
        ShowFile::blank("id", "Song")
    }

    #[test]
    fn typography_and_boxes_keep_valid_show_json() {
        let mut show = show();
        assert_eq!(show.item_count("s1"), 1);
        assert!(show.set_item_text("s1", 0, "Hello\nWorld"));
        assert_eq!(show.item_text("s1", 0).as_deref(), Some("Hello\nWorld"));
        assert!(show.adjust_item_font_size("s1", 0, 20));
        assert!(show.toggle_item_bold("s1", 0));
        assert!(show.set_item_font_family("s1", 0, "Georgia"));
        let typography = show.item_typography("s1", 0);
        assert_eq!(typography.font_px, 120);
        assert!(typography.bold);
        assert_eq!(typography.family, "Georgia");
        let style = show
            .show
            .pointer("/slides/s1/items/0/lines/0/text/0/style")
            .and_then(Value::as_str)
            .unwrap();
        assert!(style.contains("font-size: 120px"));
        assert!(style.contains("font-weight: bold"));
        assert!(style.contains("font-family: Georgia"));

        let second = show.add_item_box("s1").unwrap();
        assert_eq!(second, 1);
        assert!(show.set_item_text("s1", 1, "Caption"));
        assert!(show.nudge_item_at("s1", 1, 20, 0, 0, 0));
        let moved = show
            .show
            .pointer("/slides/s1/items/1/style")
            .and_then(Value::as_str)
            .unwrap();
        assert!(moved.contains("left:110px"));
        assert_eq!(show.item_text("s1", 0).as_deref(), Some("Hello\nWorld"));
        assert_eq!(show.item_count("s1"), 2);
        assert!(show.remove_item_box("s1", 1));
        assert_eq!(show.item_count("s1"), 1);
        assert!(!show.remove_item_box("s1", 0));
        let json = show.to_json().unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("Georgia"));
    }
}
