// SPDX-License-Identifier: GPL-3.0-only

use serde_json::{Value, json};
use std::ops::Range;

use crate::css_set;

/// Applies a declaration to a validated UTF-8 range in the flattened item text.
pub(crate) fn format_box_range(lines: &mut [Value], range: Range<usize>, key: &str, value: &str) {
    let mut offset = 0;
    for line in lines {
        if let Some(parts) = line.get_mut("text").and_then(Value::as_array_mut) {
            let mut new_parts = Vec::with_capacity(parts.len());
            for run in parts.drain(..) {
                let text = run_text(&run);
                let end = offset + text.len();
                let start = range.start.max(offset);
                let stop = range.end.min(end);
                let style = run["style"].as_str().unwrap_or_default();
                let new_style = (start < stop).then(|| css_set(style, key, value));
                if let Some(new_style) = new_style.filter(|new_style| new_style != style) {
                    let start = start - offset;
                    let stop = stop - offset;
                    if start > 0 {
                        new_parts.push(styled_run(Some(&run), &text[..start]));
                    }
                    let mut selected = styled_run(Some(&run), &text[start..stop]);
                    selected["style"] = json!(new_style);
                    new_parts.push(selected);
                    if stop < text.len() {
                        new_parts.push(styled_run(Some(&run), &text[stop..]));
                    }
                } else {
                    new_parts.push(run);
                }
                offset = end;
            }
            *parts = new_parts;
        }
        // The plain text editor inserts one newline between stored paragraphs.
        offset += 1;
    }
}

/// Updates the edited range of a text box and leaves the untouched rich-text runs intact.
///
/// The plain-text editor reports the full value. Matching its prefix and suffix
/// recovers the range for a keystroke, selection replacement, or paste.
pub(crate) fn replace_box_lines(lines: &mut Vec<Value>, text: &str) {
    if text.is_empty() {
        lines.clear();
        return;
    }
    if lines.is_empty() {
        lines.push(json!({"align": "", "text": []}));
    }
    let rows = lines
        .iter()
        .map(|line| runs(line).iter().map(run_text).collect::<String>())
        .collect::<Vec<_>>();
    let old = rows.join("\n");
    if old == text {
        return;
    }
    let prefix = matching_bytes(old.chars(), text.chars());
    let suffix = matching_bytes(old[prefix..].chars().rev(), text[prefix..].chars().rev());
    let end = old.len() - suffix;
    let (first_row, first_byte) = locate(&rows, prefix);
    let (last_row, last_byte) = locate(&rows, end);
    let first = &lines[first_row];
    let last = &lines[last_row];
    let style_at = if prefix == end {
        rows[first_row][..first_byte]
            .char_indices()
            .next_back()
            .map_or(first_byte, |(offset, _)| offset)
    } else {
        first_byte
    };
    let donor = run_at(first, style_at);
    let head = slice_runs(first, 0..first_byte);
    let tail = slice_runs(last, last_byte..rows[last_row].len());
    let inserted = &text[prefix..text.len() - suffix];
    let count = inserted.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let mut replacement = Vec::with_capacity(count);
    for (index, insert) in inserted.split('\n').enumerate() {
        let mut line = if index == 0 {
            first.as_object().cloned().unwrap_or_default()
        } else if index + 1 == count && first_row != last_row {
            last.as_object().cloned().unwrap_or_default()
        } else {
            json!({"align": first.get("align").cloned().unwrap_or(json!(""))})
                .as_object()
                .cloned()
                .unwrap_or_default()
        };
        let mut parts = Vec::new();
        if index == 0 {
            parts.extend(head.iter().cloned());
        }
        if !insert.is_empty() {
            parts.push(styled_run(donor, insert));
        }
        if index + 1 == count {
            parts.extend(tail.iter().cloned());
        }
        if parts.is_empty() {
            parts.push(styled_run(donor, ""));
        }
        merge_adjacent_runs(&mut parts);
        line.insert("text".into(), Value::Array(parts));
        replacement.push(Value::Object(line));
    }
    lines.splice(first_row..=last_row, replacement);
}

fn matching_bytes(left: impl Iterator<Item = char>, right: impl Iterator<Item = char>) -> usize {
    left.zip(right)
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum()
}

fn locate(rows: &[String], mut offset: usize) -> (usize, usize) {
    for (row, text) in rows.iter().enumerate() {
        if offset <= text.len() {
            return (row, offset);
        }
        offset -= text.len() + 1;
    }
    unreachable!("offset is within the item text");
}

fn runs(line: &Value) -> &[Value] {
    line.get("text")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn run_text(run: &Value) -> &str {
    run.get("value").and_then(Value::as_str).unwrap_or_default()
}

fn run_at(line: &Value, offset: usize) -> Option<&Value> {
    let parts = runs(line);
    let mut end = 0;
    for run in parts {
        end += run_text(run).len();
        if end > offset {
            return Some(run);
        }
    }
    parts.last()
}

fn styled_run(donor: Option<&Value>, text: &str) -> Value {
    let mut run = donor
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    run.entry("style").or_insert_with(|| json!(""));
    run.insert("value".into(), json!(text));
    Value::Object(run)
}

fn slice_runs(line: &Value, keep: Range<usize>) -> Vec<Value> {
    let mut result = Vec::new();
    let mut start = 0;
    for run in runs(line) {
        let text = run_text(run);
        let end = start + text.len();
        let a = keep.start.max(start);
        let b = keep.end.min(end);
        if (a < b && a == start && b == end)
            || (start == end && (keep.start..keep.end).contains(&start))
        {
            result.push(run.clone());
        } else if a < b {
            result.push(styled_run(Some(run), &text[a - start..b - start]));
        }
        start = end;
    }
    result
}

fn merge_adjacent_runs(runs: &mut Vec<Value>) {
    let mut merged: Vec<Value> = Vec::with_capacity(runs.len());
    for new in runs.drain(..) {
        if let Some(previous) = merged.last_mut()
            && let Some(a) = previous.as_object()
            && let Some(b) = new.as_object()
            && a.keys().all(|key| key == "value" || key == "style")
            && b.keys().all(|key| key == "value" || key == "style")
            && a.get("style") == b.get("style")
        {
            previous["value"] = json!(format!("{}{}", run_text(previous), run_text(&new)));
        } else {
            merged.push(new);
        }
    }
    *runs = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unicode_text_edit_keeps_untouched_styles_and_paragraph_metadata() {
        let mut lines = vec![
            json!({"align":"center","metadata":{"keep":true},"text":[
                {"value":"Welcome ","style":"font-family:Inter;"},
                {"value":"舞台🎵","style":"font-weight:bold;color:#f80;","external":{"keep":true}},
                {"value":" today","style":"font-style:italic;"}
            ]}),
            json!({"align":"left","text":[{"value":"Second line","style":"color:#0f0;"}]}),
        ];
        let untouched = lines[1].clone();
        replace_box_lines(&mut lines, "Welcome 礼堂🎵 today\nSecond line");
        assert_eq!(lines[0]["text"][1]["value"], "礼堂");
        assert_eq!(lines[0]["text"][1]["external"]["keep"], true);
        assert_eq!(lines[0]["text"][2]["value"], "🎵");
        assert_eq!(lines[0]["text"][2]["style"], "font-weight:bold;color:#f80;");
        assert_eq!(lines[0]["text"][3]["style"], "font-style:italic;");
        assert_eq!(lines[0]["metadata"]["keep"], true);
        assert_eq!(lines[1], untouched);

        replace_box_lines(&mut lines, "Welcome 礼堂🎵\n today\nSecond line");
        assert_eq!(lines[1]["align"], "center");
        assert_eq!(
            lines[1]["text"][0],
            json!({"value":" today","style":"font-style:italic;"})
        );
        assert_eq!(lines[2], untouched);
    }
}
