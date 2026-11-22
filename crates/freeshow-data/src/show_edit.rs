// SPDX-License-Identifier: GPL-3.0-only

//! Operator edits on a show's active layout: the whole-show text editor and
//! slide delete, duplicate and reorder. Slides are reused where possible so
//! item styles, colors, notes and media survive a text edit.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::ShowFile;

const DEFAULT_ITEM_STYLE: &str = "top:88px;left:50px;height:904px;width:1820px;";

/// Where an ordered slide index sits in the active layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LayoutSpot {
    entry: usize,
    child: Option<usize>,
}

impl ShowFile {
    /// Renames the show. Returns false for a blank name.
    pub fn set_name(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(show) = self.show.as_object_mut() else {
            return false;
        };
        show.insert("name".into(), Value::String(name.to_string()));
        true
    }

    /// The active layout as editor text: `[Group]` headers and blank lines between slides.
    pub fn lyrics_text(&self) -> String {
        let mut out = String::new();
        for (index, slide) in self.ordered_slides().iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            if let Some(group) = slide
                .group
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty())
            {
                out.push('[');
                out.push_str(group);
                out.push_str("]\n");
            }
            for line in &slide.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }

    /// Rebuilds the active layout from editor text. Each blank-line block is a
    /// slide and a leading `[Name]` line sets its group. A repeated block with the
    /// same group and text points at the same slide, as FreeShow does for a chorus.
    /// Existing slides are reused in order, keeping their style, color, notes and
    /// media; new slides copy the text style of the slide before them.
    pub fn set_lyrics_text(&mut self, text: &str, split_lines: u32) -> bool {
        let layout_id = self.active_layout();
        let previous: Vec<(String, String)> = self
            .ordered_slides()
            .into_iter()
            .map(|slide| (slide.id, slide.group.unwrap_or_default()))
            .collect();
        let old_entries: HashMap<String, Value> = self
            .layout_entries(&layout_id)
            .into_iter()
            .filter_map(|entry| Some((entry.get("id")?.as_str()?.to_string(), entry)))
            .collect();
        let stanzas = super::split_stanzas(editor_stanzas(text), split_lines);
        let mut used: HashSet<String> = HashSet::new();
        let mut repeated: HashMap<(String, String), String> = HashMap::new();
        let mut order = Vec::new();
        let mut template: Option<Value> = None;
        for (group, body) in stanzas {
            if !group.is_empty()
                && let Some(id) = repeated.get(&(group.clone(), body.clone()))
            {
                order.push(json!({ "id": id }));
                continue;
            }
            // Prefer the old slide with the same group, then the next unused one.
            let reused = previous
                .iter()
                .find(|(id, old)| !group.is_empty() && *old == group && !used.contains(id))
                .or_else(|| previous.iter().find(|(id, _)| !used.contains(id)))
                .map(|(id, _)| id.clone());
            let id = match reused {
                Some(id) => id,
                None => self.fresh_slide_id(&used),
            };
            let mut slide = self
                .show
                .pointer(&format!("/slides/{id}"))
                .cloned()
                .unwrap_or_else(|| blank_slide(template.as_ref()));
            if let Some(object) = slide.as_object_mut() {
                object.remove("children");
                object.insert("group".into(), Value::String(group.clone()));
            }
            write_slide_text(&mut slide, &body);
            template = Some(slide.clone());
            if let Some(slides) = self.show.get_mut("slides").and_then(Value::as_object_mut) {
                slides.insert(id.clone(), slide);
            } else if let Some(show) = self.show.as_object_mut() {
                show.insert("slides".into(), json!({ id.clone(): slide }));
            }
            let entry = old_entries
                .get(&id)
                .cloned()
                .map(|mut entry| {
                    if let Some(object) = entry.as_object_mut() {
                        object.remove("children");
                    }
                    entry
                })
                .unwrap_or_else(|| json!({ "id": id }));
            order.push(entry);
            if !group.is_empty() {
                repeated.insert((group, body), id.clone());
            }
            used.insert(id);
        }
        if !self.set_layout_entries(&layout_id, order) {
            return false;
        }
        self.prune_unused_slides();
        true
    }

    /// Removes the slide at an ordered index from the active layout. A slide no
    /// layout uses any more is deleted from the show.
    pub fn remove_slide_at(&mut self, index: usize) -> bool {
        let layout_id = self.active_layout();
        let Some(spot) = self.layout_spot(index) else {
            return false;
        };
        let mut entries = self.layout_entries(&layout_id);
        if entries.len() <= 1 && spot.child.is_none() {
            return false;
        }
        match spot.child {
            None => {
                entries.remove(spot.entry);
            }
            Some(child) => {
                let Some(parent) = entries[spot.entry].get("id").and_then(Value::as_str) else {
                    return false;
                };
                let pointer = format!("/slides/{parent}/children");
                let Some(children) = self
                    .show
                    .pointer_mut(&pointer)
                    .and_then(Value::as_array_mut)
                else {
                    return false;
                };
                if child >= children.len() {
                    return false;
                }
                children.remove(child);
            }
        }
        if !self.set_layout_entries(&layout_id, entries) {
            return false;
        }
        self.prune_unused_slides();
        true
    }

    /// Copies the slide at an ordered index and places the copy right after it.
    /// Returns the ordered index of the copy.
    pub fn duplicate_slide_at(&mut self, index: usize) -> Option<usize> {
        let layout_id = self.active_layout();
        let spot = self.layout_spot(index)?;
        let source_id = self.ordered_slides().get(index)?.id.clone();
        let mut slide = self.show.pointer(&format!("/slides/{source_id}"))?.clone();
        if let Some(object) = slide.as_object_mut() {
            object.remove("children");
        }
        let id = self.fresh_slide_id(&HashSet::new());
        self.show
            .get_mut("slides")?
            .as_object_mut()?
            .insert(id.clone(), slide);
        let mut entries = self.layout_entries(&layout_id);
        let mut entry = match spot.child {
            None => entries.get(spot.entry)?.clone(),
            Some(_) => json!({}),
        };
        if let Some(object) = entry.as_object_mut() {
            object.remove("children");
            object.insert("id".into(), Value::String(id));
        }
        entries.insert(spot.entry + 1, entry);
        if !self.set_layout_entries(&layout_id, entries) {
            return None;
        }
        // The copy lands after the source entry and all of its children.
        let children = self.children_count(spot.entry);
        Some(self.ordered_index_of_entry(spot.entry) + children + 1)
    }

    /// Moves the layout entry holding an ordered index one step up or down.
    /// Returns the new ordered index of the moved slide.
    pub fn move_slide_at(&mut self, index: usize, delta: isize) -> Option<usize> {
        let layout_id = self.active_layout();
        let spot = self.layout_spot(index)?;
        if spot.child.is_some() {
            return None;
        }
        let mut entries = self.layout_entries(&layout_id);
        let target = spot.entry.checked_add_signed(delta)?;
        if target >= entries.len() {
            return None;
        }
        let entry = entries.remove(spot.entry);
        entries.insert(target, entry);
        if !self.set_layout_entries(&layout_id, entries) {
            return None;
        }
        Some(self.ordered_index_of_entry(target))
    }

    fn layout_entries(&self, layout_id: &str) -> Vec<Value> {
        self.show
            .pointer(&format!("/layouts/{layout_id}/slides"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn set_layout_entries(&mut self, layout_id: &str, entries: Vec<Value>) -> bool {
        let Some(show) = self.show.as_object_mut() else {
            return false;
        };
        let layouts = show.entry("layouts").or_insert_with(|| json!({}));
        let Some(layouts) = layouts.as_object_mut() else {
            return false;
        };
        let layout = layouts
            .entry(layout_id.to_string())
            .or_insert_with(|| json!({ "name": "Default", "notes": "", "slides": [] }));
        let Some(layout) = layout.as_object_mut() else {
            return false;
        };
        layout.insert("slides".into(), Value::Array(entries));
        true
    }

    fn slide_exists(&self, id: &str) -> bool {
        self.show.pointer(&format!("/slides/{id}")).is_some()
    }

    fn children_of(&self, id: &str) -> Vec<String> {
        self.show
            .pointer(&format!("/slides/{id}/children"))
            .and_then(Value::as_array)
            .map(|children| {
                children
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|child| self.slide_exists(child))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn layout_spot(&self, index: usize) -> Option<LayoutSpot> {
        let entries = self.layout_entries(&self.active_layout());
        let mut cursor = 0usize;
        for (entry_index, entry) in entries.iter().enumerate() {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !self.slide_exists(id) {
                continue;
            }
            if cursor == index {
                return Some(LayoutSpot {
                    entry: entry_index,
                    child: None,
                });
            }
            cursor += 1;
            let all_children: Vec<String> = self
                .show
                .pointer(&format!("/slides/{id}/children"))
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            for (child_index, child) in all_children.iter().enumerate() {
                if !self.slide_exists(child) {
                    continue;
                }
                if cursor == index {
                    return Some(LayoutSpot {
                        entry: entry_index,
                        child: Some(child_index),
                    });
                }
                cursor += 1;
            }
        }
        None
    }

    fn children_count(&self, entry: usize) -> usize {
        self.layout_entries(&self.active_layout())
            .get(entry)
            .and_then(|entry| entry.get("id").and_then(Value::as_str).map(str::to_string))
            .map(|id| self.children_of(&id).len())
            .unwrap_or(0)
    }

    fn ordered_index_of_entry(&self, target: usize) -> usize {
        let entries = self.layout_entries(&self.active_layout());
        let mut cursor = 0;
        for (index, entry) in entries.iter().enumerate() {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !self.slide_exists(id) {
                continue;
            }
            if index == target {
                return cursor;
            }
            cursor += 1 + self.children_of(id).len();
        }
        cursor
    }

    fn fresh_slide_id(&self, taken: &HashSet<String>) -> String {
        let count = self
            .show
            .get("slides")
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len);
        let mut number = count + 1;
        loop {
            let id = format!("s{number}");
            if !self.slide_exists(&id) && !taken.contains(&id) {
                return id;
            }
            number += 1;
        }
    }

    /// Deletes slides that no layout entry, and no kept slide's children, refer to.
    fn prune_unused_slides(&mut self) {
        let mut keep: HashSet<String> = HashSet::new();
        if let Some(layouts) = self.show.get("layouts").and_then(Value::as_object) {
            for layout in layouts.values() {
                for entry in layout
                    .get("slides")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = entry.get("id").and_then(Value::as_str) {
                        keep.insert(id.to_string());
                    }
                }
            }
        }
        let parents: Vec<String> = keep.iter().cloned().collect();
        for parent in parents {
            for child in self.children_of(&parent) {
                keep.insert(child);
            }
        }
        if let Some(slides) = self.show.get_mut("slides").and_then(Value::as_object_mut) {
            slides.retain(|id, _| keep.contains(id));
        }
    }
}

/// Editor text as `(group, body)` blocks. A bracketed first line is always a group.
fn editor_stanzas(text: &str) -> Vec<(String, String)> {
    let text = text.replace("\r\n", "\n");
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line.trim_end());
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    let stanzas: Vec<(String, String)> = blocks
        .into_iter()
        .map(|mut lines| {
            let first = lines[0].trim();
            let group = first
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
                .map(|inner| inner.trim().to_string())
                .or_else(|| super::group_header(first));
            match group {
                Some(group) => {
                    lines.remove(0);
                    (group, lines.join("\n"))
                }
                None => (String::new(), lines.join("\n")),
            }
        })
        .collect();
    if stanzas.is_empty() {
        vec![(String::new(), String::new())]
    } else {
        stanzas
    }
}

/// A new slide whose text item copies the box and run style of `template`.
fn blank_slide(template: Option<&Value>) -> Value {
    let item = template
        .and_then(|slide| text_item(slide))
        .map(|item| {
            let mut item = item.clone();
            if let Some(object) = item.as_object_mut() {
                object.remove("autoFontSize");
            }
            item
        })
        .unwrap_or_else(|| {
            json!({
                "style": DEFAULT_ITEM_STYLE,
                "lines": [{ "align": "", "text": [{ "value": "", "style": "" }] }]
            })
        });
    json!({
        "group": "",
        "color": null,
        "settings": {},
        "notes": "",
        "items": [item]
    })
}

fn text_item(slide: &Value) -> Option<&Value> {
    slide
        .get("items")?
        .as_array()?
        .iter()
        .find(|item| item.get("lines").is_some_and(Value::is_array))
}

/// Writes `body` into the first text item, one line per row, keeping the first
/// line's alignment and run style for every row.
fn write_slide_text(slide: &mut Value, body: &str) {
    let Some(object) = slide.as_object_mut() else {
        return;
    };
    let items = object.entry("items").or_insert_with(|| json!([]));
    let Some(items) = items.as_array_mut() else {
        return;
    };
    let position = items
        .iter()
        .position(|item| item.get("lines").is_some_and(Value::is_array));
    let index = match position {
        Some(index) => index,
        None => {
            items.insert(0, json!({ "style": DEFAULT_ITEM_STYLE, "lines": [] }));
            0
        }
    };
    let Some(item) = items[index].as_object_mut() else {
        return;
    };
    let old_lines = item
        .get("lines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let line_template = |row: usize| -> (Value, String) {
        let line = old_lines.get(row).or_else(|| old_lines.first());
        let align = line
            .and_then(|line| line.get("align"))
            .cloned()
            .unwrap_or(json!(""));
        let style = line
            .and_then(|line| line.pointer("/text/0/style"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        (align, style)
    };
    let rows: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.split('\n').collect()
    };
    let lines: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let (align, style) = line_template(row);
            json!({ "align": align, "text": [{ "value": text, "style": style }] })
        })
        .collect();
    item.insert("lines".into(), Value::Array(lines));
    item.remove("autoFontSize");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled_song() -> ShowFile {
        let mut show = ShowFile::blank("song", "Song");
        assert!(show.set_lyrics_text("[Verse 1]\nOne\nTwo\n\n[Chorus]\nSing", 0));
        let slide = show.show.pointer_mut("/slides/s1").unwrap();
        slide["color"] = json!("#ff0000");
        slide["notes"] = json!("slow");
        slide["items"][0]["style"] = json!("left:10px;top:20px;width:900px;height:500px;");
        slide["items"][0]["lines"][0]["text"][0]["style"] = json!("font-size:80px;color:#ffee00;");
        show
    }

    #[test]
    fn the_editor_round_trips_groups_and_lines() {
        let show = styled_song();
        assert_eq!(show.lyrics_text(), "[Verse 1]\nOne\nTwo\n\n[Chorus]\nSing");
        let groups: Vec<_> = show.ordered_slides().into_iter().map(|s| s.group).collect();
        assert_eq!(groups, [Some("Verse 1".into()), Some("Chorus".into())]);
    }

    #[test]
    fn a_text_edit_keeps_slide_style_and_notes() {
        let mut show = styled_song();
        assert!(show.set_lyrics_text(
            "[Verse 1]\nOne changed\nTwo\nThree\n\n[Chorus]\nSing\n\n[Bridge]\nNew part",
            0
        ));
        let slide = show.show.pointer("/slides/s1").unwrap();
        assert_eq!(slide["color"], "#ff0000");
        assert_eq!(slide["notes"], "slow");
        assert_eq!(
            slide["items"][0]["style"],
            "left:10px;top:20px;width:900px;height:500px;"
        );
        assert_eq!(slide["items"][0]["lines"].as_array().unwrap().len(), 3);
        assert_eq!(
            slide["items"][0]["lines"][2]["text"][0]["style"],
            "font-size:80px;color:#ffee00;"
        );
        assert_eq!(
            slide["items"][0]["lines"][0]["text"][0]["value"],
            "One changed"
        );
        let order = show.ordered_slides();
        assert_eq!(order.len(), 3);
        assert_eq!(order[2].group.as_deref(), Some("Bridge"));
        assert_eq!(order[2].lines, ["New part"]);
    }

    #[test]
    fn a_repeated_chorus_shares_one_slide_and_removed_slides_are_pruned() {
        let mut show = styled_song();
        assert!(show.set_lyrics_text("[Chorus]\nSing\n\n[Verse 1]\nOne\n\n[Chorus]\nSing", 0));
        let ids: Vec<_> = show.ordered_slides().into_iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], ids[2]);
        assert_eq!(ids[1], "s1", "the verse keeps its own styled slide");
        assert!(show.set_lyrics_text("Only one", 0));
        assert_eq!(show.show["slides"].as_object().unwrap().len(), 1);
        assert_eq!(show.lyrics_text(), "Only one");
    }

    #[test]
    fn any_bracketed_first_line_is_a_group() {
        let mut show = ShowFile::blank("w", "Welcome");
        assert!(show.set_lyrics_text("[Welcome]\nGood morning\n\nChorus:\nHi", 0));
        let groups: Vec<_> = show.ordered_slides().into_iter().map(|s| s.group).collect();
        assert_eq!(groups, [Some("Welcome".into()), Some("Chorus".into())]);
    }

    #[test]
    fn slides_can_be_removed_duplicated_and_moved() {
        let mut show = styled_song();
        assert_eq!(show.duplicate_slide_at(0), Some(1));
        let order = show.ordered_slides();
        assert_eq!(order.len(), 3);
        assert_eq!(order[1].lines, order[0].lines);
        assert_ne!(order[1].id, order[0].id);
        assert_eq!(show.move_slide_at(2, -1), Some(1));
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus"));
        assert_eq!(show.move_slide_at(0, -1), None);
        assert!(show.remove_slide_at(1));
        assert_eq!(show.ordered_slides().len(), 2);
        assert!(
            show.show.pointer("/slides/s2").is_none(),
            "unused slide is pruned"
        );
        assert!(show.remove_slide_at(0));
        assert!(!show.remove_slide_at(0), "the last slide stays");
    }

    #[test]
    fn child_slides_are_removed_from_their_parent() {
        let mut show = styled_song();
        let child = show.show["slides"]["s2"].clone();
        show.show["slides"]["c1"] = child;
        show.show["slides"]["s1"]["children"] = json!(["c1"]);
        assert_eq!(show.ordered_slides().len(), 3);
        assert_eq!(show.duplicate_slide_at(0), Some(2));
        assert!(show.remove_slide_at(1));
        assert!(show.show.pointer("/slides/c1").is_none());
        assert_eq!(show.ordered_slides().len(), 3);
    }

    #[test]
    fn a_name_cannot_be_blank() {
        let mut show = ShowFile::blank("a", "A");
        assert!(!show.set_name("  "));
        assert!(show.set_name(" Holy Holy "));
        assert_eq!(show.name(), "Holy Holy");
    }
}
