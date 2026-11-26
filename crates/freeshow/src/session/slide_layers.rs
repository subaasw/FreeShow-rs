// SPDX-License-Identifier: GPL-3.0-only

use super::Session;
use freeshow_data::{ItemBox, ShowFile, css_set, css_value};
use freeshow_output::{ItemGeometry, SOURCE_HEIGHT, SOURCE_WIDTH, item_geometry, parse_style};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LayerSelection {
    show_id: String,
    slide_id: String,
    occurrence: usize,
    items: Vec<usize>,
}

pub struct LayerDrag {
    selection: LayerSelection,
    primary: usize,
    geometry: ItemGeometry,
    group_resize: Option<GroupResize>,
    layers: Vec<Value>,
    horizontal: SnapAxis,
    vertical: SnapAxis,
}

struct GroupResize {
    origin: [f32; 2],
    size: [f32; 2],
    min_scale: f32,
    max_scale: f32,
}

impl GroupResize {
    fn new(items: &[Value], indices: &[usize]) -> Option<Self> {
        let mut first = [f32::INFINITY; 2];
        let mut last = [f32::NEG_INFINITY; 2];
        let mut min_scale = 0.0_f32;
        let mut max_scale = f32::INFINITY;
        for &index in indices {
            let geometry = layer_geometry(&items[index]);
            for at in [geometry.width, geometry.height] {
                min_scale = min_scale.max(1.0 / at);
                max_scale = max_scale.min(at.max(8000.0) / at);
            }
            for point in geometry.corners() {
                if point.iter().any(|at| !at.is_finite()) {
                    return None;
                }
                for axis in 0..2 {
                    first[axis] = first[axis].min(point[axis]);
                    last[axis] = last[axis].max(point[axis]);
                }
            }
        }
        let size = [last[0] - first[0], last[1] - first[1]];
        size.iter()
            .all(|at| at.is_finite() && *at > 0.0)
            .then_some(Self {
                origin: first,
                size,
                min_scale,
                max_scale,
            })
    }

    fn scale(&self, dx: f32, dy: f32) -> f32 {
        if !dx.is_finite() || !dy.is_finite() {
            return 1.0;
        }
        let dx = dx / self.size[0];
        let dy = dy / self.size[1];
        let delta = if dx.abs() >= dy.abs() { dx } else { dy };
        let scale = (1.0 + delta).clamp(self.min_scale, self.max_scale);
        if (0..2).all(|axis| (self.origin[axis] + self.size[axis] * scale).is_finite()) {
            scale
        } else {
            1.0
        }
    }

    fn outline(&self, scale: f32) -> [[f32; 2]; 4] {
        let [left, top] = self.origin;
        let right = left + self.size[0] * scale;
        let bottom = top + self.size[1] * scale;
        [[left, top], [right, top], [right, bottom], [left, bottom]]
    }
}

struct SnapAxis {
    selected: [i64; 3],
    guides: Vec<i64>,
}

impl LayerDrag {
    pub fn resize_preview(&self, dx: f32, dy: f32, uniform: bool) -> [[f32; 2]; 4] {
        self.group_resize.as_ref().map_or_else(
            || self.geometry.resized(dx, dy, uniform).corners(),
            |group| group.outline(group.scale(dx, dy)),
        )
    }

    pub fn snap(
        &self,
        dx: i32,
        dy: i32,
        tolerance_x: i32,
        tolerance_y: i32,
    ) -> (i32, i32, i32, i32) {
        let (dx, guide_x) = self.horizontal.snap(dx, tolerance_x);
        let (dy, guide_y) = self.vertical.snap(dy, tolerance_y);
        (dx, dy, guide_x, guide_y)
    }
}

impl SnapAxis {
    fn new(
        boxes: &[Option<[[f32; 2]; 4]>],
        selected: &[usize],
        coordinate: usize,
        extent: i32,
    ) -> Self {
        let extent = i64::from(extent);
        let (mut first, mut last) = (i64::MAX, i64::MIN);
        let mut guides = anchors(0, extent).to_vec();
        for (index, area) in boxes.iter().enumerate() {
            let Some(area) = area else { continue };
            let points = area.map(|point| point[coordinate]);
            let start = i64::from(points.into_iter().fold(f32::INFINITY, f32::min).round() as i32);
            let end =
                i64::from(points.into_iter().fold(f32::NEG_INFINITY, f32::max).round() as i32);
            if selected.binary_search(&index).is_ok() {
                first = first.min(start);
                last = last.max(end);
            } else {
                guides.extend(
                    anchors(start, end)
                        .into_iter()
                        .filter(|position| (0..=extent).contains(position)),
                );
            }
        }
        guides.sort_unstable();
        guides.dedup();
        Self {
            selected: anchors(first, last),
            guides,
        }
    }

    fn snap(&self, delta: i32, tolerance: i32) -> (i32, i32) {
        let delta = i64::from(delta);
        let closest = self
            .guides
            .iter()
            .flat_map(|&guide| {
                self.selected
                    .iter()
                    .map(move |&edge| (guide - (edge + delta), guide))
            })
            .filter(|&(offset, _)| offset.abs() <= i64::from(tolerance))
            .min_by_key(|&(offset, guide)| (offset.abs(), guide));
        match closest {
            Some((offset, guide)) => (
                (delta + offset).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                guide as i32,
            ),
            None => (delta as i32, -1),
        }
    }
}

fn anchors(start: i64, end: i64) -> [i64; 3] {
    [start, start + (end - start) / 2, end]
}

pub(super) fn layer_name(show: &ShowFile, slide: &str, index: usize, item: &Value) -> String {
    if let Some(name) = item["name"].as_str().filter(|name| !name.trim().is_empty()) {
        return name.to_owned();
    }
    let text = show.item_text(slide, index);
    let first_line = text
        .as_deref()
        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()));
    let source = item["src"]
        .as_str()
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty());
    let kind = match item["type"].as_str() {
        Some("media" | "image") => "Picture",
        Some("shape") => "Shape",
        Some("video") => "Video",
        Some("camera") => "Camera",
        Some("chart") => "Chart",
        Some("qr") => "QR code",
        _ => "Text",
    };
    first_line.or(source).unwrap_or(kind).to_owned()
}

impl Session {
    pub(super) fn selected_layer_slide(&self) -> Option<(&ShowFile, String)> {
        let show = self.current_show()?;
        let slide = show.ordered_slides().get(self.slide_index?)?.id.clone();
        Some((show, slide))
    }

    pub fn layer_text_target(&self) -> String {
        self.selected_layers()
            .map_or_else(String::new, |selection| {
                format!(
                    "{:?}",
                    (
                        selection.show_id,
                        selection.slide_id,
                        selection.occurrence,
                        self.item_index
                    )
                )
            })
    }

    pub fn selected_layer_is_countdown(&self) -> bool {
        self.selected_layer_slide().is_some_and(|(show, slide)| {
            let item = self.current_item(show);
            crate::slide_countdown::is_countdown_item(&show.show["slides"][slide]["items"][item])
        })
    }

    /// Front to back, matching the order an operator sees on the canvas.
    pub fn layer_labels(&self) -> Vec<String> {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return Vec::new();
        };
        let Some(items) = show.show["slides"][&slide]["items"].as_array() else {
            return Vec::new();
        };
        items
            .iter()
            .enumerate()
            .rev()
            .map(|(index, item)| {
                let name = layer_name(show, &slide, index, item);
                let locked = if freeshow_data::item_locked(item) {
                    " · locked"
                } else {
                    ""
                };
                let hidden = if freeshow_data::item_hidden(item) {
                    " · hidden"
                } else {
                    ""
                };
                let grouped = if show.layer_group(&slide, index).is_some()
                    && show.group_members(&slide, index).len() > 1
                {
                    " · grouped"
                } else {
                    ""
                };
                format!("{}  {name}{locked}{hidden}{grouped}", index + 1)
            })
            .collect()
    }

    fn selected_layers(&self) -> Option<LayerSelection> {
        let (show, slide) = self.selected_layer_slide()?;
        let count = show.item_count(&slide);
        if self.tab != "shows" || count == 0 {
            return None;
        }
        let primary = self.item_index.min(count - 1);
        let occurrence = self.slide_index?;
        let items = self.layer_selection.as_ref().filter(|selection| {
            selection.show_id == show.id
                && selection.slide_id == slide
                && selection.occurrence == occurrence
                && selection.items.binary_search(&primary).is_ok()
                && selection.items.iter().all(|&index| index < count)
        });
        let items = items.map_or_else(
            || show.group_members(&slide, primary),
            |selection| selection.items.clone(),
        );
        Some(LayerSelection {
            show_id: show.id.clone(),
            slide_id: slide,
            occurrence,
            items,
        })
    }

    pub fn layer_indices(&self) -> Vec<usize> {
        self.selected_layers()
            .map_or_else(Vec::new, |selection| selection.items)
    }

    pub fn show_has_locked_layers(&self) -> bool {
        self.current_show().is_some_and(ShowFile::has_locked_layers)
    }

    pub fn active_layer_flags(&self) -> (bool, bool) {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return (false, false);
        };
        let item = self.current_item(show);
        (
            show.layer_locked(&slide, item),
            show.layer_hidden(&slide, item),
        )
    }

    pub fn selected_layer_flags(&self) -> (bool, bool, bool) {
        let Some(selection) = self.selected_layers() else {
            return (false, false, false);
        };
        let Some(show) = self.current_show() else {
            return (false, false, false);
        };
        let locked = |index| show.layer_locked(&selection.slide_id, index);
        (
            selection.items.iter().copied().any(locked),
            selection.items.iter().copied().all(locked),
            selection
                .items
                .iter()
                .all(|&index| show.layer_hidden(&selection.slide_id, index)),
        )
    }

    pub fn set_layer_flags(&mut self, lock: bool, value: bool) -> bool {
        let items = self.layer_indices();
        let label = match (lock, value) {
            (true, true) => "Lock layers",
            (true, false) => "Unlock layers",
            (false, true) => "Hide layers",
            (false, false) => "Show layers",
        };
        !items.is_empty()
            && self.edit_selected_slide(label, |show, slide, _| {
                items.iter().all(|&index| {
                    if lock {
                        show.set_layer_locked(slide, index, value)
                    } else {
                        show.set_layer_hidden(slide, index, value)
                    }
                })
            })
    }

    pub fn edit_source_size(&self) -> (f32, f32) {
        self.selected_layer_slide()
            .map_or((SOURCE_WIDTH, SOURCE_HEIGHT), |(show, id)| {
                crate::live::slide_source_size(&show.show["slides"][&id])
            })
    }

    pub fn active_layer_geometry(&self) -> Option<ItemGeometry> {
        let (show, slide) = self.selected_layer_slide()?;
        let items = show.show["slides"][&slide]["items"].as_array()?;
        items
            .get(self.item_index.min(items.len().saturating_sub(1)))
            .map(layer_geometry)
    }

    pub fn active_layer_outline(&self, is_group: bool) -> Option<[[f32; 2]; 4]> {
        if is_group {
            let (show, slide) = self.selected_layer_slide()?;
            let items = show.show["slides"][&slide]["items"].as_array()?;
            return GroupResize::new(items, &self.layer_indices()).map(|group| group.outline(1.0));
        }
        self.active_layer_geometry().map(ItemGeometry::corners)
    }

    pub fn layer_outlines(&self) -> Vec<ItemGeometry> {
        let Some(selection) = self.selected_layers() else {
            return Vec::new();
        };
        let Some(show) = self.current_show() else {
            return Vec::new();
        };
        let primary = self.current_item(show);
        selection
            .items
            .iter()
            .filter(|&&index| index != primary && !show.layer_hidden(&selection.slide_id, index))
            .map(|&index| layer_geometry(&show.show["slides"][&selection.slide_id]["items"][index]))
            .collect()
    }

    fn set_layer_selection(&mut self, items: Vec<usize>, primary: usize) {
        self.item_index = primary;
        self.layer_selection = self.selected_layers().and_then(|mut selection| {
            selection.items = items;
            (!selection.items.is_empty()).then_some(selection)
        });
    }

    pub fn select_item(&mut self, index: usize) -> bool {
        if index >= self.item_position().1 {
            return false;
        }
        // A single selection is explicit so users can edit one member of a saved group.
        self.set_layer_selection(vec![index], index);
        true
    }

    pub fn select_layer_group(&mut self, index: usize) -> bool {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return false;
        };
        let members = show.group_members(&slide, index);
        if members.is_empty() {
            return false;
        }
        self.set_layer_selection(members, index);
        true
    }

    /// An additive selection can be assembled using the list or the canvas.
    pub fn extend_layer_selection(&mut self, index: usize) -> bool {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return false;
        };
        let members = show.group_members(&slide, index);
        if members.is_empty() {
            return false;
        }
        let mut items = self.layer_indices();
        items.extend(members);
        items.sort_unstable();
        items.dedup();
        self.set_layer_selection(items, index);
        true
    }

    fn expanded_layer_groups(&self, show: &ShowFile, slide: &str, indices: &[usize]) -> Vec<usize> {
        let mut expanded = indices
            .iter()
            .flat_map(|&index| show.group_members(slide, index))
            .collect::<Vec<_>>();
        expanded.sort_unstable();
        expanded.dedup();
        expanded
    }

    /// Group, Ungroup, select the active group, and align as a single group.
    pub fn layer_group_controls(&self) -> (bool, bool, bool, bool) {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return (false, false, false, false);
        };
        let indices = self.layer_indices();
        let active = show.group_members(&slide, self.current_item(show));
        let single_group = active.len() > 1 && active == indices;
        let expanded = self.expanded_layer_groups(show, &slide, &indices);
        let unlocked =
            |items: &[usize]| items.iter().all(|&index| !show.layer_locked(&slide, index));
        let has_members = indices
            .iter()
            .any(|&index| show.group_members(&slide, index).len() > 1);
        (
            indices.len() > 1 && !single_group && unlocked(&expanded),
            has_members && unlocked(&expanded),
            active.len() > 1 && !single_group,
            single_group,
        )
    }

    pub fn set_layer_grouping(&mut self, group: bool) -> bool {
        let (can_group, can_ungroup, _, _) = self.layer_group_controls();
        if (group && !can_group) || (!group && !can_ungroup) {
            return false;
        }
        let Some((show, slide)) = self.selected_layer_slide() else {
            return false;
        };
        let primary = self.current_item(show);
        let indices = self.expanded_layer_groups(show, &slide, &self.layer_indices());
        let id = group.then(freeshow_data::new_id);
        let label = if group {
            "Group layers"
        } else {
            "Ungroup layers"
        };
        self.edit_selected_slide_then(
            label,
            |show, slide, _| {
                indices
                    .iter()
                    .all(|&index| show.set_layer_group(slide, index, id.as_deref()))
                    .then_some(indices)
            },
            |session, indices| session.set_layer_selection(indices, primary),
        )
    }

    pub fn exclude_primary_layer(&mut self) {
        let (primary, _) = self.item_position();
        let mut items = self.layer_indices();
        if items.len() > 1
            && let Ok(at) = items.binary_search(&primary)
        {
            items.remove(at);
            let next = items[at.min(items.len() - 1)];
            self.set_layer_selection(items, next);
        }
    }

    pub fn pick_canvas_layer(&mut self, index: usize) -> bool {
        if self.layer_indices().binary_search(&index).is_ok() {
            self.item_index = index;
            true
        } else {
            self.select_layer_group(index)
        }
    }

    pub fn duplicate_layer(&mut self) -> bool {
        let selected = self.layer_indices();
        let (primary, _) = self.item_position();
        let Some(at) = selected.iter().position(|&index| index == primary) else {
            return false;
        };
        self.edit_selected_slide_then(
            "Duplicate layers",
            |show, slide, _| {
                let mut groups = std::collections::HashMap::new();
                for group in selected
                    .iter()
                    .filter_map(|&index| show.layer_group(slide, index))
                {
                    let (count, _) = groups
                        .entry(group.to_owned())
                        .or_insert_with(|| (0, freeshow_data::new_id()));
                    *count += 1;
                }
                let duplicated = selected.iter().rev().all(|&index| {
                    let group = show
                        .layer_group(slide, index)
                        .and_then(|id| groups.get(id))
                        .filter(|(count, _)| *count > 1)
                        .map(|(_, id)| id.as_str());
                    show.duplicate_item_box(slide, index).is_some_and(|copy| {
                        group.is_none() || show.set_layer_group(slide, copy, group)
                    })
                });
                duplicated.then(|| {
                    selected
                        .iter()
                        .enumerate()
                        .map(|(offset, index)| index + offset + 1)
                        .collect::<Vec<_>>()
                })
            },
            |session, copies| {
                let primary = copies[at];
                session.set_layer_selection(copies, primary);
            },
        )
    }

    pub fn can_move_layers(&self, forward: bool) -> bool {
        let Some(selection) = self.selected_layers() else {
            return false;
        };
        let Some(show) = self.current_show() else {
            return false;
        };
        let count = show.item_count(&selection.slide_id);
        let locked = |index| show.layer_locked(&selection.slide_id, index);
        !selection.items.iter().copied().any(locked)
            && selection.items.iter().any(|&index| {
                adjacent_layer(index, count, forward).is_some_and(|next| {
                    selection.items.binary_search(&next).is_err() && !locked(next)
                })
            })
    }

    pub fn move_item_box(&mut self, forward: bool) -> bool {
        let mut selected = self.layer_indices();
        let mut primary = self.item_position().0;
        if !self.can_move_layers(forward) {
            return false;
        }
        self.edit_selected_slide_then(
            "Reorder layers",
            |show, slide, _| {
                let count = show.item_count(slide);
                for visit in 0..selected.len() {
                    let at = if forward {
                        selected.len() - 1 - visit
                    } else {
                        visit
                    };
                    let from = selected[at];
                    let Some(next) = adjacent_layer(from, count, forward) else {
                        continue;
                    };
                    if selected.binary_search(&next).is_ok() || show.layer_locked(slide, next) {
                        continue;
                    }
                    show.move_item_box(slide, from, forward)?;
                    selected[at] = next;
                    if from == primary {
                        primary = next;
                    }
                }
                Some((selected, primary))
            },
            |session, (selected, primary)| session.set_layer_selection(selected, primary),
        )
    }

    pub fn remove_item_box(&mut self) -> bool {
        let selected = self.layer_indices();
        let (primary, count) = self.item_position();
        if selected.is_empty() || selected.len() == count || self.selected_layer_flags().0 {
            return false;
        }
        let position = primary.saturating_sub(selected.partition_point(|&index| index <= primary));
        let next = position.min(count - selected.len() - 1);
        self.edit_selected_slide_then(
            "Remove layers",
            |show, slide, _| {
                selected
                    .iter()
                    .rev()
                    .all(|&index| show.remove_item_box(slide, index))
                    .then_some(())
            },
            |session, ()| {
                session.select_item(next);
            },
        )
    }

    fn selected_layer_boxes(&self) -> Vec<(usize, ItemBox)> {
        let Some(selection) = self.selected_layers() else {
            return Vec::new();
        };
        let Some(show) = self.current_show() else {
            return Vec::new();
        };
        selection
            .items
            .iter()
            .map(|&index| (index, show.item_box(&selection.slide_id, index)))
            .collect()
    }

    /// Aligns one box to the slide or a selection to its enclosing bounds.
    pub fn align_layer(&mut self, alignment: i32) -> bool {
        let mut boxes = self.selected_layer_boxes();
        if boxes.is_empty() || !(0..8).contains(&alignment) {
            return false;
        }
        let horizontal = matches!(alignment, 0..=2 | 6);
        if self.layer_group_controls().3 {
            if alignment >= 6 {
                return false;
            }
            return self.align_saved_group(alignment, horizontal);
        }
        if alignment >= 6 {
            if boxes.len() < 3 {
                return false;
            }
            boxes.sort_by_key(|(index, area)| (axis(area, horizontal).0, *index));
            let first = axis(&boxes[0].1, horizontal).0;
            let (last, last_size) = axis(&boxes.last().unwrap().1, horizontal);
            let sizes: i64 = boxes.iter().map(|(_, area)| axis(area, horizontal).1).sum();
            let free = last + last_size - first - sizes;
            let steps = (boxes.len() - 1) as i64;
            let mut offset = first;
            let positions = boxes
                .iter()
                .enumerate()
                .map(|(step, (index, area))| {
                    let projected = offset + (free * step as i64).div_euclid(steps);
                    offset += axis(area, horizontal).1;
                    (*index, axis_position(area, horizontal, projected))
                })
                .collect();
            return self.write_layer_positions("Distribute layers", positions);
        }
        let (start, end) = if boxes.len() == 1 {
            let (width, height) = self.edit_source_size();
            (
                0,
                if horizontal {
                    width as i64
                } else {
                    height as i64
                },
            )
        } else {
            let (first, size) = axis(&boxes[0].1, horizontal);
            boxes
                .iter()
                .skip(1)
                .fold((first, first + size), |(start, end), (_, area)| {
                    let (at, size) = axis(area, horizontal);
                    (start.min(at), end.max(at + size))
                })
        };
        let positions = boxes
            .iter()
            .map(|(index, area)| {
                let (_, size) = axis(area, horizontal);
                let at = match alignment % 3 {
                    0 => start,
                    1 => start + (end - start - size) / 2,
                    _ => end - size,
                };
                (*index, axis_position(area, horizontal, at))
            })
            .collect();
        self.write_layer_positions("Align layers", positions)
    }

    fn align_saved_group(&mut self, alignment: i32, horizontal: bool) -> bool {
        let Some((show, slide)) = self.selected_layer_slide() else {
            return false;
        };
        let items = &show.show["slides"][&slide]["items"];
        let coordinate = usize::from(!horizontal);
        let (first, last) = self
            .layer_indices()
            .into_iter()
            .flat_map(|index| layer_geometry(&items[index]).corners())
            .map(|point| point[coordinate])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(start, end), at| {
                (start.min(at), end.max(at))
            });
        let (width, height) = self.edit_source_size();
        let extent = if horizontal { width } else { height };
        let offset = match alignment % 3 {
            0 => -first,
            1 => (extent - (first + last)) / 2.0,
            _ => extent - last,
        }
        .round() as i32;
        let (dx, dy) = if horizontal { (offset, 0) } else { (0, offset) };
        self.move_selected_layers("Align group to slide", dx, dy)
    }

    pub fn translate_layers(&mut self, dx: i32, dy: i32) -> bool {
        self.move_selected_layers("Nudge layers", dx, dy)
    }

    fn move_selected_layers(&mut self, label: &str, dx: i32, dy: i32) -> bool {
        let selected = self.layer_indices();
        if selected.is_empty() || self.selected_layer_flags().0 {
            return false;
        }
        self.edit_selected_slide(label, |show, slide, _| {
            selected.iter().all(|&index| {
                let area = layer_geometry(&show.show["slides"][slide]["items"][index]);
                (dx == 0
                    || show.set_item_box_css(
                        slide,
                        index,
                        "left",
                        &format!("{:.3}px", area.left + dx as f32),
                    ))
                    && (dy == 0
                        || show.set_item_box_css(
                            slide,
                            index,
                            "top",
                            &format!("{:.3}px", area.top + dy as f32),
                        ))
            })
        })
    }

    fn write_layer_positions(&mut self, label: &str, positions: Vec<(usize, (i32, i32))>) -> bool {
        !positions.is_empty()
            && !self.selected_layer_flags().0
            && self.edit_selected_slide(label, |show, slide, _| {
                positions.iter().all(|&(index, (x, y))| {
                    let area = show.item_box(slide, index);
                    (area.left == x
                        || show.set_item_box_css(slide, index, "left", &format!("{x}px")))
                        && (area.top == y
                            || show.set_item_box_css(slide, index, "top", &format!("{y}px")))
                })
            })
    }

    /// Front-most slide item under a point in the editor's source coordinate space.
    pub fn item_box_at(&self, x: f32, y: f32) -> Option<usize> {
        let (show, slide) = self.selected_layer_slide()?;
        (0..show.item_count(&slide)).rev().find(|&index| {
            !show.layer_hidden(&slide, index)
                && !show.layer_locked(&slide, index)
                && layer_geometry(&show.show["slides"][&slide]["items"][index]).contains(x, y)
        })
    }

    pub fn begin_layer_drag(&self) -> Option<LayerDrag> {
        let selection = self.selected_layers()?;
        let show = self.current_show()?;
        let primary = self.current_item(show);
        let is_group = self.layer_group_controls().3;
        let hidden = if is_group {
            selection
                .items
                .iter()
                .all(|&index| show.layer_hidden(&selection.slide_id, index))
        } else {
            show.layer_hidden(&selection.slide_id, primary)
        };
        if hidden
            || selection
                .items
                .iter()
                .any(|&index| show.layer_locked(&selection.slide_id, index))
        {
            return None;
        }
        let layers = show.slide_items(&selection.slide_id);
        let boxes: Vec<_> = layers
            .iter()
            .enumerate()
            .map(|(index, item)| {
                (!show.layer_hidden(&selection.slide_id, index))
                    .then(|| layer_geometry(item).corners())
            })
            .collect();
        let (width, height) = self.edit_source_size();
        let horizontal = SnapAxis::new(&boxes, &selection.items, 0, width.round() as i32);
        let vertical = SnapAxis::new(&boxes, &selection.items, 1, height.round() as i32);
        let geometry = layer_geometry(&layers[primary]);
        let group_resize = if is_group {
            GroupResize::new(&layers, &selection.items)
        } else {
            None
        };
        Some(LayerDrag {
            selection,
            primary,
            geometry,
            group_resize,
            layers,
            horizontal,
            vertical,
        })
    }

    fn matches_layer_drag(&self, drag: &LayerDrag) -> bool {
        let Some(selection) = self.selected_layers() else {
            return false;
        };
        let Some(show) = self.current_show() else {
            return false;
        };
        selection == drag.selection
            && self.current_item(show) == drag.primary
            && show.show["slides"][&selection.slide_id]["items"].as_array() == Some(&drag.layers)
    }

    pub fn finish_layer_move(&mut self, drag: LayerDrag, dx: i32, dy: i32) -> bool {
        self.matches_layer_drag(&drag) && self.move_selected_layers("Move layers", dx, dy)
    }

    pub fn finish_layer_resize(
        &mut self,
        drag: LayerDrag,
        dx: f32,
        dy: f32,
        uniform: bool,
    ) -> bool {
        if !self.matches_layer_drag(&drag) {
            return false;
        }
        if let Some(group) = drag.group_resize {
            let scale = group.scale(dx, dy);
            return scale != 1.0
                && self.edit_selected_slide("Resize group", |show, slide, _| {
                    let items = &mut show.show["slides"][slide]["items"];
                    drag.selection
                        .items
                        .iter()
                        .all(|&index| resize_group_layer(&mut items[index], group.origin, scale))
                });
        }
        if drag.selection.items.len() != 1 {
            return false;
        }
        let area = drag.geometry.resized(dx, dy, uniform);
        let previous = drag.geometry;
        self.edit_selected_slide("Resize layer", |show, slide, index| {
            [
                ("left", previous.left, area.left),
                ("top", previous.top, area.top),
                ("width", previous.width, area.width),
                ("height", previous.height, area.height),
            ]
            .into_iter()
            .all(|(key, before, after)| {
                before == after
                    || show.set_item_box_css(slide, index, key, &format!("{after:.3}px"))
            })
        })
    }
}

fn resize_group_layer(item: &mut Value, [left, top]: [f32; 2], scale: f32) -> bool {
    let area = layer_geometry(item);
    let declarations = [
        ("left", left + (area.left - left) * scale),
        ("top", top + (area.top - top) * scale),
        ("width", area.width * scale),
        ("height", area.height * scale),
    ];
    if declarations.iter().any(|(_, value)| !value.is_finite()) || !item.is_object() {
        return false;
    }
    let mut style = item["style"].as_str().unwrap_or_default().to_owned();
    if let Some(lines) = item.get_mut("lines").and_then(Value::as_array_mut) {
        style = scaled_font(&style, scale);
        for runs in lines
            .iter_mut()
            .filter_map(|line| line.get_mut("text").and_then(Value::as_array_mut))
        {
            for run in runs {
                if let Some(style) = run["style"].as_str()
                    && css_value(style, "font-size").is_some()
                {
                    run["style"] = Value::String(scaled_font(style, scale));
                }
            }
        }
    }
    for (name, value) in declarations {
        style = css_set(&style, name, &format!("{value:.3}px"));
    }
    item["style"] = Value::String(style);
    true
}

fn scaled_font(style: &str, scale: f32) -> String {
    let size = parse_style(style).font_px * scale;
    if size.is_finite() {
        css_set(style, "font-size", &format!("{size:.3}px"))
    } else {
        style.to_owned()
    }
}

fn adjacent_layer(index: usize, count: usize, forward: bool) -> Option<usize> {
    if forward {
        index.checked_add(1).filter(|&next| next < count)
    } else {
        index.checked_sub(1)
    }
}

fn axis(area: &ItemBox, horizontal: bool) -> (i64, i64) {
    let (at, size) = if horizontal {
        (area.left, area.width)
    } else {
        (area.top, area.height)
    };
    (i64::from(at), i64::from(size.max(0)))
}

fn axis_position(area: &ItemBox, horizontal: bool, at: i64) -> (i32, i32) {
    let at = at.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    if horizontal {
        (at, area.top)
    } else {
        (area.left, at)
    }
}

fn layer_geometry(item: &Value) -> ItemGeometry {
    item_geometry(item["style"].as_str().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_group_preserves_legacy_layers_through_edit_duplicate_and_template() {
        let root =
            std::env::temp_dir().join(format!("freeshow-saved-group-{}", freeshow_data::new_id()));
        let library = freeshow_data::Library::open(&root);
        std::fs::create_dir_all(root.join("Media")).unwrap();
        std::fs::write(
            root.join("Media/card.png"),
            include_bytes!("../../../freeshow-data/tests/fixtures/lesson-cover.png"),
        )
        .unwrap();
        let mut legacy = ShowFile::blank("legacy", "Sunday announcement");
        legacy.show["slides"]["s1"]["items"] = serde_json::json!([
            {"type":"text","style":"left:100px;top:300px;width:320px;height:120px;",
                "textMetadata":{"keep":true},"lines":[{"align":"center","text":[
                  {"value":"Welcome ","style":""},
                  {"value":"Sunday","style":"color:#f80;font-weight:bold;"},
                  {"value":" at church","style":"font-style:italic;"}
                ]}]},
            {"type":"shape","name":"Independent strip","style":"left:200px;top:450px;width:240px;height:30px;background:#30b;"},
            {"type":"media","src":"Media/card.png","fit":"contain","style":"left:500px;top:300px;width:260px;height:180px;"}
        ]);
        let path = library.shows_dir().join("Legacy.show");
        legacy.save(&path).unwrap();
        let mut session = Session::open(root.clone());
        session.select_drawer(0);
        session.select_slide(0);
        let taken = crate::live::audience_frame(&session);
        assert!(session.select_item(0));
        assert!(session.extend_layer_selection(2));
        assert!(session.layer_group_controls().0);
        assert!(session.set_layer_grouping(true));
        assert!(session.layer_group_controls().3);
        assert!(session.undo());
        assert!(
            session
                .current_show()
                .unwrap()
                .layer_group("s1", 0)
                .is_none()
        );
        assert!(session.redo());
        assert_eq!(crate::live::edit_frame(&session), taken);

        assert!(session.select_item(0));
        assert!(session.layer_group_controls().2);
        assert!(session.save_lyrics("Welcome This Sunday at church"));
        let saved = ShowFile::load(&path).unwrap();
        assert_eq!(saved.group_members("s1", 2), [0, 2]);
        assert!(saved.layer_group("s1", 1).is_none());
        let item = &saved.show["slides"]["s1"]["items"][0];
        assert_eq!(item["textMetadata"]["keep"], true);
        assert_eq!(item["lines"][0]["text"][2]["style"], "font-style:italic;");
        assert_eq!(crate::live::audience_frame(&session), taken);

        let mut reopened = Session::open(root.clone());
        reopened.select_drawer(0);
        assert!(reopened.pick_first_slide());
        assert_eq!(reopened.layer_indices(), [0, 2]);
        assert!(reopened.pick_canvas_layer(0));
        assert_eq!(reopened.layer_indices(), [0, 2]);
        assert!(reopened.pick_canvas_layer(2));
        assert_eq!(reopened.layer_indices(), [0, 2]);
        assert!(reopened.select_item(2));
        assert!(reopened.set_layer_flags(true, true));
        assert!(reopened.select_layer_group(2));
        assert!(!reopened.set_layer_grouping(false));
        assert!(!reopened.translate_layers(5, 0));
        assert!(reopened.set_layer_flags(true, false));
        assert!(reopened.align_layer(0));
        let show = reopened.current_show().unwrap();
        assert_eq!(show.item_box("s1", 0).left, 0);
        assert_eq!(show.item_box("s1", 1).left, 200);
        assert_eq!(show.item_box("s1", 2).left, 400);
        assert!(reopened.duplicate_layer());
        assert_eq!(reopened.layer_indices(), [1, 4]);
        let show = reopened.current_show().unwrap();
        assert_ne!(show.layer_group("s1", 0), show.layer_group("s1", 1));
        assert_eq!(show.group_members("s1", 0), [0, 3]);
        assert_eq!(show.group_members("s1", 4), [1, 4]);
        assert!(reopened.set_layer_grouping(false));
        assert!(
            reopened
                .current_show()
                .unwrap()
                .layer_group("s1", 4)
                .is_none()
        );
        assert!(reopened.undo());
        assert_eq!(
            reopened.current_show().unwrap().group_members("s1", 4),
            [1, 4]
        );

        reopened
            .library_action("slide-to-template", "Announcement")
            .unwrap();
        let id = reopened.template_catalog.id_named("Announcement").unwrap();
        let template = reopened.template_catalog.get(&id).unwrap();
        let copy = ShowFile::from_template("copy", "Copy", template).unwrap();
        assert_eq!(copy.group_members("s1", 4), [1, 4]);
        assert_eq!(
            copy.item_text("s1", 0).as_deref(),
            Some("Welcome This Sunday at church")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layers_can_be_duplicated_aligned_and_reordered_without_editing_the_taken_slide() {
        let root =
            std::env::temp_dir().join(format!("freeshow-layers-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Background"));
        assert!(session.add_item_box());
        assert!(session.save_lyrics("Title"));
        session.inspector_set("width", "300").unwrap();
        session.inspector_set("height", "120").unwrap();
        session.inspector_set("top", "70").unwrap();
        session.select_slide(0);
        let taken = crate::live::audience_frame(&session);
        assert!(session.select_item(1));

        assert_eq!(session.layer_labels(), ["2  Title", "1  Background"]);
        assert!(session.duplicate_layer());
        assert_eq!(session.item_position(), (2, 3));
        assert_eq!(
            session.layer_labels(),
            ["3  Title", "2  Title", "1  Background"]
        );
        assert!(session.save_lyrics("Subtitle"));
        assert!(session.align_layer(2));
        assert_eq!(session.inspector_form().item_box.left, 1620);
        assert_eq!(session.inspector_form().item_box.top, 70);
        assert!(session.move_item_box(false));
        assert_eq!(
            session.layer_labels(),
            ["3  Title", "2  Subtitle", "1  Background"]
        );
        assert!(session.undo());
        assert_eq!(
            session.layer_labels(),
            ["3  Subtitle", "2  Title", "1  Background"]
        );
        assert_eq!(crate::live::audience_frame(&session).pixels, taken.pixels);

        let show =
            ShowFile::load(session.current_show().unwrap().path.as_deref().unwrap()).unwrap();
        assert_eq!(show.item_text("s1", 2).as_deref(), Some("Subtitle"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transformed_resize_round_trips_as_one_edit_and_keeps_the_taken_frame() {
        let root =
            std::env::temp_dir().join(format!("freeshow-layer-resize-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Resize me"));
        assert!(session.edit_selected_slide("Set imported geometry", |show, slide, index| {
            show.show["slides"][slide]["settings"]["resolution"] =
                serde_json::json!({"width": 1000, "height": 750});
            let item = &mut show.show["slides"][slide]["items"][index];
            item["style"] = Value::String(
                "left:200.125px;top:220.25px;width:400px;height:150px;transform:rotate(90deg);background-color:#f80;".into(),
            );
            item["unrecognized"] = serde_json::json!({"keep":true});
            true
        }));
        session.select_slide(0);
        let taken = crate::live::audience_frame(&session);
        assert_eq!(session.edit_source_size(), (1000.0, 750.0));
        let before = session.active_layer_geometry().unwrap();
        let drag = session.begin_layer_drag().unwrap();
        let preview = drag.resize_preview(-25.5, 60.25, false);
        let history = session.history.len();
        assert!(session.finish_layer_resize(drag, -25.5, 60.25, false));
        assert_eq!(session.history.len(), history + 1);

        let show = session.current_show().unwrap();
        let saved = ShowFile::load(show.path.as_deref().unwrap()).unwrap();
        assert_eq!(
            saved.show["slides"]["s1"]["items"][0]["unrecognized"]["keep"],
            true
        );
        let actual = session.active_layer_geometry().unwrap();
        for (actual, expected) in actual.corners().into_iter().zip(preview) {
            assert!((actual[0] - expected[0]).abs() < 0.001);
            assert!((actual[1] - expected[1]).abs() < 0.001);
        }
        assert_eq!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert!(session.update_live_slide());
        assert_ne!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert!(session.undo());
        assert_eq!(session.active_layer_geometry(), Some(before));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_group_resize_scales_rotated_layers_and_type_in_one_edit() {
        let root =
            std::env::temp_dir().join(format!("freeshow-group-resize-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.edit_selected_slide("Set group resize fixture", |show, slide, _| {
            show.show["slides"][slide]["items"] = serde_json::json!([
                {"type":"text","style":"left:180px;top:180px;width:200px;height:120px;transform:rotate(30deg);",
                    "lines":[{"text":[{"value":"Welcome","style":"font-size:32px;color:#f80;"},
                        {"value":" home","style":"font-weight:bold;"}]}]},
                {"type":"media","src":"Media/card.png","fit":"cover","crop":[10,20,30,40],
                    "style":"left:420px;top:240px;width:160px;height:120px;"}
            ]);
            true
        }));
        session.select_slide(0);
        assert!(session.select_item(0));
        assert!(session.extend_layer_selection(1));
        let temporary = session.begin_layer_drag().unwrap();
        assert!(!session.finish_layer_resize(temporary, 80., 0., false));
        assert!(session.set_layer_grouping(true));
        let taken = crate::live::audience_frame(&session);
        let before = session.current_show().unwrap().slide_items("s1");
        let outline = session.active_layer_outline(true).unwrap();
        let [left, top] = outline[0];
        let dx = (outline[2][0] - left) * 0.5;
        let drag = session.begin_layer_drag().unwrap();
        let preview = drag.resize_preview(dx, 0., false);
        assert_eq!(preview[0], outline[0]);
        let edits = session.history.len();
        assert!(session.finish_layer_resize(drag, dx, 0., false));
        assert_eq!(session.history.len(), edits + 1);
        assert_eq!(session.layer_indices(), [0, 1]);
        let updated = session.active_layer_outline(true).unwrap();
        let close = |actual: [f32; 2], expected: [f32; 2]| {
            assert!(
                (actual[0] - expected[0]).abs() < 0.002,
                "{actual:?} != {expected:?}"
            );
            assert!(
                (actual[1] - expected[1]).abs() < 0.002,
                "{actual:?} != {expected:?}"
            );
        };
        for (updated, expected) in updated.into_iter().zip(preview) {
            close(updated, expected);
        }
        let show = session.current_show().unwrap();
        let saved = ShowFile::load(show.path.as_deref().unwrap()).unwrap();
        let after = saved.slide_items("s1");
        for (old, new) in before.iter().zip(&after) {
            for ([x, y], actual) in layer_geometry(old)
                .corners()
                .into_iter()
                .zip(layer_geometry(new).corners())
            {
                close(actual, [left + (x - left) * 1.5, top + (y - top) * 1.5]);
            }
        }
        let css = |item: &Value, key: &str| css_value(item["style"].as_str().unwrap(), key);
        assert_eq!(css(&after[0], "font-size").as_deref(), Some("150.000px"));
        let runs = &after[0]["lines"][0]["text"];
        assert_eq!(css(&runs[0], "font-size").as_deref(), Some("48.000px"));
        assert_eq!(css(&runs[0], "color").as_deref(), Some("#f80"));
        assert_eq!(runs[1], before[0]["lines"][0]["text"][1]);
        assert_eq!(
            css(&after[0], "transform").as_deref(),
            Some("rotate(30deg)")
        );
        assert_eq!(after[1]["crop"], before[1]["crop"]);
        assert_eq!(after[1]["fit"], "cover");
        assert_eq!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert!(session.update_live_slide());
        assert_ne!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert!(session.undo());
        assert_eq!(session.current_show().unwrap().slide_items("s1"), before);
        assert!(session.select_item(0));
        assert!(session.set_layer_flags(true, true));
        assert!(session.select_layer_group(1));
        assert!(session.begin_layer_drag().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_drag_is_discarded_when_its_selection_or_layer_order_changes() {
        let root =
            std::env::temp_dir().join(format!("freeshow-layer-drag-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.add_item_box());
        assert!(session.save_lyrics("Top"));
        let stale = session.begin_layer_drag().unwrap();
        assert!(session.select_item(0));
        assert!(!session.finish_layer_move(stale, 300, 400));

        assert!(session.select_item(1));
        let stale = session.begin_layer_drag().unwrap();
        assert!(session.move_item_box(false));
        assert!(session.select_item(1));
        assert!(!session.finish_layer_move(stale, 300, 400));

        let original = session.inspector_form().item_box;
        let drag = session.begin_layer_drag().unwrap();
        let before = session.history.len();
        assert!(session.finish_layer_move(drag, 300, 400));
        assert_eq!(session.history.len(), before + 1);
        assert!(session.undo());
        let restored = session.inspector_form().item_box;
        assert_eq!((restored.left, restored.top), (original.left, original.top));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drag_preview_and_failed_save_preserve_redo_until_a_new_drag_commits() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("song", "Song", "Welcome"));
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        let path = session.current_show().unwrap().path.clone().unwrap();
        let original = ShowFile::load(&path).unwrap();
        assert!(session.translate_layers(30, 0));
        let previous = ShowFile::load(&path).unwrap();
        assert!(session.undo());
        assert_eq!((session.history.len(), session.redo.len()), (0, 1));

        let preview = session.begin_layer_drag().unwrap();
        preview.snap(30, 10, 0, 0);
        let (dx, dy, _, _) = preview.snap(70, 20, 0, 0);
        drop(preview); // Cancel drops the pointer gesture without finishing it.
        assert_eq!(ShowFile::load(&path).unwrap(), original);
        assert_eq!(session.current_show().unwrap(), &original);
        assert_eq!((session.history.len(), session.redo.len()), (0, 1));
        assert!(session.show_draft_status().is_empty());

        let blocked = path.with_extension("show.tmp");
        std::fs::create_dir(&blocked).unwrap();
        let drag = session.begin_layer_drag().unwrap();
        assert!(!session.finish_layer_move(drag, dx, dy));
        assert!(session.show_draft_status().contains("Recovery draft"));
        assert_eq!(ShowFile::load(&path).unwrap(), original);
        assert_eq!(session.current_show().unwrap(), &original);
        assert_eq!((session.history.len(), session.redo.len()), (0, 1));

        std::fs::remove_dir(&blocked).unwrap();
        session.library_action("discard-draft", "").unwrap();
        assert!(session.show_draft_status().is_empty());
        assert!(session.redo());
        assert_eq!(ShowFile::load(&path).unwrap(), previous);
        assert!(session.undo());
        let drag = session.begin_layer_drag().unwrap();
        assert!(session.finish_layer_move(drag, dx, dy));
        assert_eq!((session.history.len(), session.redo.len()), (1, 0));
        let moved = ShowFile::load(&path).unwrap();
        let before = original.item_box("s1", 0);
        let after = moved.item_box("s1", 0);
        assert_eq!((after.left, after.top), (before.left + dx, before.top + dy));
        assert!(session.undo());
        assert_eq!(ShowFile::load(&path).unwrap(), original);
        assert_eq!(crate::live::audience_frame(&session), on_air);
    }

    #[test]
    fn group_arrangement_saves_together_and_supports_undo_in_place() {
        let root =
            std::env::temp_dir().join(format!("freeshow-group-layers-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        for (index, (text, left)) in [("First", "100"), ("Middle", "180"), ("Last", "500")]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                assert!(session.add_item_box());
            }
            assert!(session.save_lyrics(text));
            session.inspector_set("left", left).unwrap();
            session.inspector_set("width", "100").unwrap();
        }
        session.select_slide(0);
        let live = crate::live::audience_frame(&session);
        assert!(session.select_item(0));
        assert!(session.extend_layer_selection(2));
        let stale = session.begin_layer_drag().unwrap();
        assert!(session.extend_layer_selection(1));
        assert!(session.pick_canvas_layer(2));
        assert!(!session.finish_layer_move(stale, 700, 700));
        assert!(session.pick_canvas_layer(1));
        assert_eq!(session.layer_indices(), [0, 1, 2]);
        let lefts = |session: &Session| {
            let (show, slide) = session.selected_layer_slide().unwrap();
            (0..show.item_count(&slide))
                .map(|i| show.item_box(&slide, i).left)
                .collect::<Vec<_>>()
        };
        let before = session.history.len();
        assert!(session.align_layer(6));
        assert_eq!(session.history.len(), before + 1);
        assert_eq!(lefts(&session), [100, 300, 500]);

        let drag = session.begin_layer_drag().unwrap();
        assert!(session.finish_layer_move(drag, 40, 0));
        assert_eq!(lefts(&session), [140, 340, 540]);
        assert!(session.undo());
        assert_eq!(lefts(&session), [100, 300, 500]);
        assert_eq!(session.layer_indices(), [0, 1, 2]);
        assert!(session.redo());
        assert_eq!(lefts(&session), [140, 340, 540]);
        assert_eq!(crate::live::audience_frame(&session).pixels, live.pixels);

        let path = session.current_show().unwrap().path.clone().unwrap();
        let marker = path.with_extension("show.tmp");
        std::fs::create_dir(&marker).unwrap();
        let history = session.history.len();
        assert!(!session.translate_layers(10, 0));
        assert_eq!(lefts(&session), [140, 340, 540]);
        assert_eq!(session.history.len(), history);
        let disk = ShowFile::load(&path).unwrap();
        assert_eq!(disk.item_box("s1", 1).left, 340);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moving_and_duplicating_a_selection_preserves_its_layer_order() {
        let root =
            std::env::temp_dir().join(format!("freeshow-group-order-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        for (index, text) in ["A", "B", "C", "D"].into_iter().enumerate() {
            if index > 0 {
                assert!(session.add_item_box());
            }
            assert!(session.save_lyrics(text));
        }
        assert!(session.select_item(0));
        assert!(session.extend_layer_selection(2));
        assert!(session.move_item_box(true));
        assert_eq!(session.layer_labels(), ["4  C", "3  D", "2  A", "1  B"]);
        assert_eq!(session.layer_indices(), [1, 3]);
        assert!(session.duplicate_layer());
        assert_eq!(
            session.layer_labels(),
            ["6  C", "5  C", "4  D", "3  A", "2  A", "1  B"]
        );
        assert_eq!(session.layer_indices(), [2, 5]);
        assert!(session.undo());
        assert_eq!(session.layer_indices(), [1, 3]);
        assert!(session.remove_item_box());
        assert_eq!(session.layer_labels(), ["2  D", "1  B"]);
        assert_eq!(session.layer_indices().len(), 1);
        assert!(session.undo());
        assert_eq!(session.layer_indices(), [1, 3]);
        assert!(session.add_slide());
        assert_eq!(session.layer_indices().len(), 1);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapping_uses_group_edges_and_ignores_selected_layers_as_targets() {
        let styles = [
            "left:100px;width:100px;height:100px;",
            "left:250px;width:100px;height:100px;",
            "left:600px;width:200px;height:100px;transform:rotate(90deg);",
        ];
        let boxes = styles.map(|style| Some(item_geometry(style).corners()));
        let axis = SnapAxis::new(&boxes, &[0, 1], 0, 1920);
        assert_eq!(axis.snap(298, 6), (300, 650));
        assert_eq!(axis.snap(297, 2), (297, -1));
        assert_eq!(axis.snap(-103, 6), (-100, 0));
        assert_eq!(axis.snap(2, 6), (2, -1));
    }

    #[test]
    fn hiding_a_layer_keeps_its_content_and_only_changes_the_program_on_update() {
        let root =
            std::env::temp_dir().join(format!("freeshow-hide-layer-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Bottom"));
        assert!(session.add_item_box());
        assert!(session.save_lyrics("Top"));
        assert!(
            session.edit_selected_slide("Place top", |show, slide, index| {
                show.set_item_box_css(slide, index, "background", "#ff0000")
            })
        );
        let top = session.inspector_form().item_box;
        let point = (top.left as f32 + 10.0, top.top as f32 + 10.0);
        assert_eq!(session.item_box_at(point.0, point.1), Some(1));
        session.select_slide(0);
        let taken = crate::live::audience_frame(&session);
        assert_eq!(session.to_snapshot().slide_lines(), ["Bottom", "Top"]);
        assert!(session.set_layer_flags(false, true));
        assert!(session.active_layer_flags().1);
        assert_eq!(session.lyrics(), "Top");
        assert_eq!(session.item_box_at(point.0, point.1), Some(0));
        assert!(session.begin_layer_drag().is_none());
        assert_ne!(crate::live::edit_frame(&session).pixels, taken.pixels);
        assert_eq!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert_eq!(session.to_snapshot().slide_lines(), ["Bottom", "Top"]);

        assert!(session.update_live_slide());
        assert_ne!(crate::live::audience_frame(&session).pixels, taken.pixels);
        assert_eq!(session.to_snapshot().slide_lines(), ["Bottom"]);
        let show = session.current_show().unwrap();
        assert_eq!(
            show.stage_text_boxes(0, true, 0, true, None),
            [vec!["Bottom".to_string()], Vec::new()]
        );
        let saved = ShowFile::load(show.path.as_deref().unwrap()).unwrap();
        assert!(saved.layer_hidden("s1", 1));
        assert_eq!(saved.item_text("s1", 1).as_deref(), Some("Top"));

        assert!(session.undo());
        assert!(!session.active_layer_flags().1);
        assert!(session.redo());
        assert!(session.set_layer_flags(false, false));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn locked_layers_block_item_edits_and_batch_moves_even_after_reopening() {
        let root =
            std::env::temp_dir().join(format!("freeshow-lock-layer-{}", freeshow_data::new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show());
        assert!(session.save_lyrics("Bottom"));
        assert!(session.add_item_box());
        assert!(session.save_lyrics("Top"));
        assert!(session.set_layer_flags(true, true));
        let history = session.history.len();
        assert!(!session.save_lyrics("Change"));
        assert!(session.inspector_set("left", "1").is_err());
        assert!(!session.remove_item_box());
        assert!(session.begin_layer_drag().is_none());
        assert!(session.library_action("show-text", "Replacement").is_err());
        let top = session.inspector_form().item_box;
        assert_eq!(
            session.item_box_at(top.left as f32 + 10.0, top.top as f32 + 10.0),
            Some(0)
        );
        assert!(session.extend_layer_selection(0));
        assert!(!session.translate_layers(10, 0));
        assert!(!session.align_layer(0));
        assert!(!session.move_item_box(true));
        assert_eq!(session.history.len(), history);
        assert!(session.set_layer_flags(false, true));

        let mut reopened = Session::open(root.clone());
        reopened.select_drawer(0);
        assert!(reopened.pick_first_slide());
        assert!(reopened.select_item(1));
        assert_eq!(reopened.active_layer_flags(), (true, true));
        assert!(reopened.set_layer_flags(false, false));
        assert!(reopened.set_layer_flags(true, false));
        assert!(reopened.save_lyrics("Unlocked"));
        assert_eq!(reopened.lyrics(), "Unlocked");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn templates_preserve_hidden_layers_and_leave_locked_layers_untouched() {
        let mut show = ShowFile::blank("test", "Test");
        assert!(show.add_item_box("s1").is_some());
        assert!(show.set_layer_locked("s1", 0, true));
        assert!(show.set_layer_hidden("s1", 1, true));
        let template = serde_json::json!({ "items": [{ "style": "left:350px;" }] });
        let before = show.item_box("s1", 0).left;
        assert!(freeshow_data::restyle_with_template(
            &mut show.show["slides"]["s1"],
            &template
        ));
        assert_eq!(show.item_box("s1", 0).left, before);
        assert_eq!(show.item_box("s1", 1).left, 350);
        assert!(show.layer_hidden("s1", 1));
    }

    #[test]
    fn a_rotated_layer_can_be_picked_outside_its_unrotated_bounds() {
        let area =
            item_geometry("left:100px;top:100px;width:200px;height:40px;transform:rotate(90deg);");
        assert!(area.contains(200.0, 180.0));
        assert!(!area.contains(280.0, 120.0));
    }
}
