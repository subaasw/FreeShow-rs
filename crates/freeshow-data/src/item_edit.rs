// SPDX-License-Identifier: GPL-3.0-only

use serde_json::Value;

use crate::ShowFile;

const DEFAULT_BOX: &str = "top:88px;left:50px;height:904px;width:1820px;";
const DEFAULT_FONT_PX: i32 = 100;

/// Geometry and fill of one slide item box, in 1920×1080 slide pixels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemBox {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub background: String,
    pub box_shadow: String,
    pub backdrop_filter: String,
    pub radius: String,
    /// `top`, `center` or `bottom`, from the box `align-items`.
    pub valign: String,
    /// Clockwise turn in whole degrees, from the box `transform: rotate()`.
    pub rotation: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemTypography {
    pub font_px: i32,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub color: String,
    pub align: String,
    /// Line height as a percent of the font size; 0 leaves the font's own spacing.
    pub line_height_pct: i32,
    pub letter_spacing: i32,
    /// Outline (`-webkit-text-stroke`) width in pixels; 0 is no outline.
    pub outline_width: i32,
    pub outline_color: String,
    pub shadow: TextShadow,
    /// CSS `text-transform`: empty, `uppercase`, `lowercase` or `capitalize`.
    pub text_case: String,
}

/// A `text-shadow` as FreeShow's editor writes it: offsets, blur and colour.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextShadow {
    pub x: i32,
    pub y: i32,
    pub blur: i32,
    pub color: String,
}

impl TextShadow {
    /// Reads `2px 2px 10px #000000` (lengths first, then the colour, which may hold spaces).
    pub fn parse(value: &str) -> Self {
        let mut shadow = Self::default();
        let mut rest = value.trim();
        let mut lengths = Vec::new();
        while lengths.len() < 3 {
            let (token, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let number = token.trim_end_matches("px");
            match number.parse::<f32>() {
                Ok(length) if !token.is_empty() => {
                    lengths.push(length.round() as i32);
                    rest = tail.trim_start();
                }
                _ => break,
            }
        }
        if let [x, y, rest_lengths @ ..] = lengths.as_slice() {
            shadow.x = *x;
            shadow.y = *y;
            shadow.blur = rest_lengths.first().copied().unwrap_or(0);
        }
        shadow.color = rest.trim().to_string();
        shadow
    }

    /// The CSS value, or empty when the shadow shows nothing.
    pub fn css(&self) -> String {
        if self.x == 0 && self.y == 0 && self.blur == 0 {
            return String::new();
        }
        let color = if self.color.trim().is_empty() {
            "#000000"
        } else {
            self.color.trim()
        };
        format!("{}px {}px {}px {color}", self.x, self.y, self.blur)
    }
}

/// An item's CSS `filter` and `mix-blend-mode`, as whole numbers the inspector edits.
/// Percent fields use 100 for "unchanged" where CSS uses 1 (opacity, brightness, contrast, saturate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFilters {
    pub blur: i32,
    pub grayscale: i32,
    pub sepia: i32,
    pub invert: i32,
    pub brightness: i32,
    pub contrast: i32,
    pub saturate: i32,
    pub hue_rotate: i32,
    pub opacity: i32,
    pub blend: String,
}

impl Default for ItemFilters {
    fn default() -> Self {
        Self {
            blur: 0,
            grayscale: 0,
            sepia: 0,
            invert: 0,
            brightness: 100,
            contrast: 100,
            saturate: 100,
            hue_rotate: 0,
            opacity: 100,
            blend: String::new(),
        }
    }
}

impl ItemFilters {
    /// Reads `blur(4px) grayscale(1) hue-rotate(90deg)`; fractions and percents both work.
    pub fn parse(filter: &str, blend: &str) -> Self {
        let mut filters = Self {
            blend: blend.trim().to_string(),
            ..Self::default()
        };
        let mut rest = filter;
        while let Some(open) = rest.find('(') {
            let name = rest[..open].trim();
            let Some(close) = rest[open..].find(')') else {
                break;
            };
            let arg = rest[open + 1..open + close].trim();
            rest = &rest[open + close + 1..];
            let percent = || {
                if let Some(value) = arg.strip_suffix('%') {
                    value.trim().parse::<f32>().ok()
                } else {
                    arg.parse::<f32>().ok().map(|value| value * 100.0)
                }
                .map(|value| value.round() as i32)
            };
            let length = |suffix: &str| {
                arg.strip_suffix(suffix)
                    .unwrap_or(arg)
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|value| value.round() as i32)
            };
            match name {
                "blur" => filters.blur = length("px").unwrap_or(0),
                "grayscale" => filters.grayscale = percent().unwrap_or(0),
                "sepia" => filters.sepia = percent().unwrap_or(0),
                "invert" => filters.invert = percent().unwrap_or(0),
                "brightness" => filters.brightness = percent().unwrap_or(100),
                "contrast" => filters.contrast = percent().unwrap_or(100),
                "saturate" => filters.saturate = percent().unwrap_or(100),
                "hue-rotate" => filters.hue_rotate = length("deg").unwrap_or(0),
                "opacity" => filters.opacity = percent().unwrap_or(100),
                _ => {}
            }
        }
        filters
    }

    /// The CSS `filter` value, leaving out every function that changes nothing.
    pub fn css(&self) -> String {
        let fraction = |value: i32| f64::from(value) / 100.0;
        let mut parts = Vec::new();
        if self.blur > 0 {
            parts.push(format!("blur({}px)", self.blur));
        }
        for (name, value, unchanged) in [
            ("grayscale", self.grayscale, 0),
            ("sepia", self.sepia, 0),
            ("invert", self.invert, 0),
            ("brightness", self.brightness, 100),
            ("contrast", self.contrast, 100),
            ("saturate", self.saturate, 100),
        ] {
            if value != unchanged {
                parts.push(format!("{name}({})", fraction(value)));
            }
        }
        if self.hue_rotate.rem_euclid(360) != 0 {
            parts.push(format!("hue-rotate({}deg)", self.hue_rotate));
        }
        if self.opacity != 100 {
            parts.push(format!("opacity({})", fraction(self.opacity)));
        }
        parts.join(" ")
    }
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
            italic: css_value(&style, "font-style")
                .is_some_and(|value| value.eq_ignore_ascii_case("italic")),
            color: css_value(&style, "color").unwrap_or_default(),
            align: css_value(&style, "text-align").unwrap_or_default(),
            line_height_pct: css_value(&style, "line-height")
                .and_then(|value| line_height_percent(&value))
                .unwrap_or(0),
            letter_spacing: css_px(&style, "letter-spacing").unwrap_or(0),
            outline_width: css_px(&style, "-webkit-text-stroke-width")
                .or_else(|| {
                    css_value(&style, "-webkit-text-stroke").and_then(|value| {
                        value
                            .split_whitespace()
                            .next()?
                            .trim_end_matches("px")
                            .parse::<f32>()
                            .ok()
                            .map(|width| width.round() as i32)
                    })
                })
                .unwrap_or(0),
            outline_color: css_value(&style, "-webkit-text-stroke-color")
                .or_else(|| {
                    css_value(&style, "-webkit-text-stroke").and_then(|value| {
                        value
                            .split_once(char::is_whitespace)
                            .map(|(_, color)| color.trim().to_string())
                    })
                })
                .unwrap_or_default(),
            shadow: css_value(&style, "text-shadow")
                .map(|value| TextShadow::parse(&value))
                .unwrap_or_default(),
            text_case: css_value(&style, "text-transform")
                .filter(|value| value != "none")
                .unwrap_or_default(),
        }
    }

    /// Sets the text line height as a percent of the font size; 0 removes it.
    pub fn set_item_line_height(&mut self, slide_id: &str, index: usize, percent: i32) -> bool {
        let value = if percent <= 0 {
            String::new()
        } else {
            let em = f64::from(percent.clamp(10, 500)) / 100.0;
            format!("{em}em")
        };
        self.set_item_css(slide_id, index, "line-height", &value)
    }

    pub fn set_item_letter_spacing(&mut self, slide_id: &str, index: usize, px: i32) -> bool {
        let value = if px == 0 {
            String::new()
        } else {
            format!("{}px", px.clamp(-300, 100))
        };
        self.set_item_css(slide_id, index, "letter-spacing", &value)
    }

    /// Sets the text outline; a width of 0 removes it.
    pub fn set_item_outline(
        &mut self,
        slide_id: &str,
        index: usize,
        width: i32,
        color: &str,
    ) -> bool {
        let (width, color) = if width <= 0 {
            (String::new(), String::new())
        } else {
            let color = if color.trim().is_empty() {
                "#000000"
            } else {
                color.trim()
            };
            (format!("{}px", width.min(100)), color.to_string())
        };
        self.set_item_css(slide_id, index, "-webkit-text-stroke", "")
            && self.set_item_css(slide_id, index, "-webkit-text-stroke-width", &width)
            && self.set_item_css(slide_id, index, "-webkit-text-stroke-color", &color)
    }

    /// Sets vertical alignment (`top`, `center`, `bottom`) in the item's `align` CSS.
    pub fn set_item_valign(&mut self, slide_id: &str, index: usize, valign: &str) -> bool {
        let value = match valign {
            "top" => "flex-start",
            "bottom" => "flex-end",
            _ => "",
        };
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        let align = item.get("align").and_then(Value::as_str).unwrap_or("");
        let next = css_set(align, "align-items", value);
        if next.trim().is_empty() {
            item.remove("align");
        } else {
            item.insert("align".to_string(), Value::String(next));
        }
        true
    }

    /// Sets the text case; an empty value or `none` removes it.
    pub fn set_item_text_case(&mut self, slide_id: &str, index: usize, case: &str) -> bool {
        let value = if case == "none" { "" } else { case };
        self.set_item_css(slide_id, index, "text-transform", value)
    }

    pub fn set_item_shadow(&mut self, slide_id: &str, index: usize, shadow: &TextShadow) -> bool {
        self.set_item_css(slide_id, index, "text-shadow", &shadow.css())
    }

    pub fn item_filters(&self, slide_id: &str, index: usize) -> ItemFilters {
        let text = |key: &str| {
            self.item(slide_id, index)
                .and_then(|item| item.get(key))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        ItemFilters::parse(&text("filter"), &text("blend"))
    }

    /// Writes the item's `filter` and `blend` the way FreeShow stores them; unchanged ones are removed.
    pub fn set_item_filters(
        &mut self,
        slide_id: &str,
        index: usize,
        filters: &ItemFilters,
    ) -> bool {
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        let css = filters.css();
        if css.is_empty() {
            item.remove("filter");
        } else {
            item.insert("filter".to_string(), Value::String(css));
        }
        let blend = filters.blend.trim();
        if blend.is_empty() || blend == "normal" {
            item.remove("blend");
        } else {
            item.insert("blend".to_string(), Value::String(blend.to_string()));
        }
        true
    }

    /// Position, size and box fill of an item, from its `style` (FreeShow's default box when unset).
    pub fn item_box(&self, slide_id: &str, index: usize) -> ItemBox {
        let style = self
            .item(slide_id, index)
            .and_then(|item| item.get("style"))
            .and_then(Value::as_str)
            .filter(|style| !style.trim().is_empty())
            .unwrap_or(DEFAULT_BOX);
        let fallback = |key| css_px(DEFAULT_BOX, key).unwrap_or(0);
        let background = css_value(style, "background");
        ItemBox {
            left: css_px(style, "left").unwrap_or_else(|| fallback("left")),
            top: css_px(style, "top").unwrap_or_else(|| fallback("top")),
            width: css_px(style, "width").unwrap_or_else(|| fallback("width")),
            height: css_px(style, "height").unwrap_or_else(|| fallback("height")),
            background: background
                .as_ref()
                .filter(|value| {
                    value.starts_with("linear-gradient(") || value.starts_with("radial-gradient(")
                })
                .cloned()
                .or_else(|| css_value(style, "background-color"))
                .or(background)
                .unwrap_or_default(),
            radius: css_value(style, "border-radius").unwrap_or_default(),
            box_shadow: css_value(style, "box-shadow").unwrap_or_default(),
            backdrop_filter: css_value(style, "backdrop-filter").unwrap_or_default(),
            // FreeShow keeps vertical alignment in the item's `align` CSS, not its `style`.
            valign: match self
                .item(slide_id, index)
                .and_then(|item| item.get("align"))
                .and_then(Value::as_str)
                .and_then(|align| css_value(align, "align-items"))
                .as_deref()
            {
                Some("flex-start" | "start" | "top") => "top".to_string(),
                Some("flex-end" | "end" | "bottom") => "bottom".to_string(),
                _ => "center".to_string(),
            },
            rotation: css_value(style, "transform")
                .and_then(|value| {
                    value
                        .trim()
                        .strip_prefix("rotate(")?
                        .strip_suffix(')')?
                        .trim()
                        .strip_suffix("deg")?
                        .trim()
                        .parse::<f32>()
                        .ok()
                })
                .map_or(0, |degrees| degrees.round() as i32),
        }
    }

    /// Sets one declaration on the item box `style` (not the text runs). An empty value removes it.
    pub fn set_item_box_css(
        &mut self,
        slide_id: &str,
        index: usize,
        key: &str,
        value: &str,
    ) -> bool {
        let style = self
            .item(slide_id, index)
            .and_then(|item| item.get("style"))
            .and_then(Value::as_str)
            .filter(|style| !style.trim().is_empty())
            .unwrap_or(DEFAULT_BOX)
            .to_string();
        let Some(item) = self.item_mut(slide_id, index) else {
            return false;
        };
        item.insert(
            "style".to_string(),
            Value::String(css_set(&style, key, value)),
        );
        true
    }

    pub fn set_item_font_size(&mut self, slide_id: &str, index: usize, px: i32) -> bool {
        let size = px.clamp(12, 400);
        self.set_item_css(slide_id, index, "font-size", &format!("{size}px"))
    }

    pub fn set_item_italic(&mut self, slide_id: &str, index: usize, italic: bool) -> bool {
        self.set_item_css(
            slide_id,
            index,
            "font-style",
            if italic { "italic" } else { "" },
        )
    }

    pub fn set_item_bold(&mut self, slide_id: &str, index: usize, bold: bool) -> bool {
        self.set_item_css(
            slide_id,
            index,
            "font-weight",
            if bold { "bold" } else { "" },
        )
    }

    pub fn set_item_color(&mut self, slide_id: &str, index: usize, color: &str) -> bool {
        self.set_item_css(slide_id, index, "color", color)
    }

    pub fn set_item_align(&mut self, slide_id: &str, index: usize, align: &str) -> bool {
        self.set_item_css(slide_id, index, "text-align", align)
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

/// The value of `key` in an inline CSS declaration list.
/// `1.1em`, `110%` or `1.1` as a whole percent of the font size.
fn line_height_percent(value: &str) -> Option<i32> {
    let value = value.trim();
    let number = if let Some(percent) = value.strip_suffix('%') {
        percent.trim().parse::<f32>().ok()?
    } else {
        value.trim_end_matches("em").trim().parse::<f32>().ok()? * 100.0
    };
    Some(number.round() as i32)
}

pub fn css_value(style: &str, key: &str) -> Option<String> {
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

/// `key` as whole pixels (`px`/`pt` suffix optional).
pub fn css_px(style: &str, key: &str) -> Option<i32> {
    let value = css_value(style, key)?;
    let number = value
        .trim()
        .trim_end_matches("px")
        .trim_end_matches("pt")
        .trim();
    number.parse().ok()
}

/// `style` with `key` replaced by `value`; an empty value removes the declaration.
pub fn css_set(style: &str, key: &str, value: &str) -> String {
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

    #[test]
    fn item_filters_round_trip_and_drop_what_changes_nothing() {
        let filters = ItemFilters::parse(
            "blur(4px) grayscale(1) brightness(120%) hue-rotate(90deg)",
            "multiply",
        );
        assert_eq!(filters.blur, 4);
        assert_eq!(filters.grayscale, 100);
        assert_eq!(filters.brightness, 120);
        assert_eq!(filters.hue_rotate, 90);
        assert_eq!(filters.contrast, 100);
        assert_eq!(filters.blend, "multiply");
        assert_eq!(
            filters.css(),
            "blur(4px) grayscale(1) brightness(1.2) hue-rotate(90deg)"
        );
        assert_eq!(ItemFilters::default().css(), "");
        let half = ItemFilters {
            opacity: 50,
            ..ItemFilters::default()
        };
        assert_eq!(half.css(), "opacity(0.5)");
    }

    fn show() -> ShowFile {
        ShowFile::blank("id", "Song")
    }

    #[test]
    fn spacing_outline_and_shadow_round_trip_as_freeshow_css() {
        let mut show = show();
        assert!(show.set_item_text("s1", 0, "Hi"));
        assert!(show.set_item_line_height("s1", 0, 110));
        assert!(show.set_item_letter_spacing("s1", 0, 3));
        assert!(show.set_item_outline("s1", 0, 6, "#ff0000"));
        let shadow = TextShadow {
            x: 2,
            y: 4,
            blur: 10,
            color: "rgb(0 0 0 / 0.5)".into(),
        };
        assert!(show.set_item_shadow("s1", 0, &shadow));
        let style = show
            .show
            .pointer("/slides/s1/items/0/lines/0/text/0/style")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert!(style.contains("line-height: 1.1em"), "{style}");
        assert!(style.contains("letter-spacing: 3px"), "{style}");
        assert!(
            style.contains("text-shadow: 2px 4px 10px rgb(0 0 0 / 0.5)"),
            "{style}"
        );
        let typography = show.item_typography("s1", 0);
        assert_eq!(typography.line_height_pct, 110);
        assert_eq!(typography.letter_spacing, 3);
        assert_eq!(
            (typography.outline_width, typography.outline_color.as_str()),
            (6, "#ff0000")
        );
        assert_eq!(typography.shadow, shadow);

        assert!(show.set_item_line_height("s1", 0, 0));
        assert!(show.set_item_outline("s1", 0, 0, ""));
        assert!(show.set_item_shadow("s1", 0, &TextShadow::default()));
        let typography = show.item_typography("s1", 0);
        assert_eq!(typography.line_height_pct, 0);
        assert_eq!(typography.outline_width, 0);
        assert_eq!(typography.shadow, TextShadow::default());
    }

    #[test]
    fn freeshow_shorthand_outline_and_shadow_read_back() {
        let shadow = TextShadow::parse("0 0 5px #000");
        assert_eq!((shadow.x, shadow.y, shadow.blur), (0, 0, 5));
        assert_eq!(shadow.color, "#000");
        let mut show = show();
        assert!(show.set_item_text("s1", 0, "Hi"));
        assert!(show.set_item_css("s1", 0, "-webkit-text-stroke", "3px #00ff00"));
        let typography = show.item_typography("s1", 0);
        assert_eq!(typography.outline_width, 3);
        assert_eq!(typography.outline_color, "#00ff00");
    }

    #[test]
    fn typography_and_boxes_keep_valid_show_json() {
        let mut show = show();
        assert_eq!(show.item_count("s1"), 1);
        assert!(show.set_item_text("s1", 0, "Hello\nWorld"));
        assert_eq!(show.item_text("s1", 0).as_deref(), Some("Hello\nWorld"));
        assert!(show.set_item_font_size("s1", 0, 120));
        assert!(show.set_item_bold("s1", 0, true));
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
