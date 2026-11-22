// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::fmt::Write;

use encoding_rs::Encoding;
use serde_json::{Value, json};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Format {
    font: Option<i32>,
    size: Option<i32>,
    color: Option<usize>,
    background: Option<usize>,
    highlight: Option<usize>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    caps: Option<bool>,
    spacing_twips: Option<i32>,
    hidden: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Destination {
    #[default]
    Body,
    FontTable,
    ColorTable,
    Ignore,
}

#[derive(Clone, Copy)]
struct State {
    destination: Destination,
    format: Format,
    align: Option<&'static str>,
    unicode_fallback: usize,
    encoding: &'static Encoding,
}

impl Default for State {
    fn default() -> Self {
        Self {
            destination: Destination::Body,
            format: Format::default(),
            align: None,
            unicode_fallback: 1,
            encoding: encoding_rs::WINDOWS_1252,
        }
    }
}

struct Run {
    format: Format,
    text: Vec<u16>,
}

#[derive(Default)]
struct Line {
    align: Option<&'static str>,
    runs: Vec<Run>,
}

pub(super) struct Document {
    lines: Vec<Line>,
    fonts: HashMap<i32, String>,
    colors: Vec<Option<[u8; 3]>>,
}

impl Document {
    pub(super) fn plain_text(&self) -> String {
        let mut text = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index != 0 {
                text.push(b'\n' as u16);
            }
            for run in &line.runs {
                text.extend_from_slice(&run.text);
            }
        }
        String::from_utf16_lossy(&text)
    }

    pub(super) fn styled_lines(&self, align: &str, underline: bool, strike: bool) -> Vec<Value> {
        let mut lines = self.lines.as_slice();
        if lines.last().is_some_and(|line| line.runs.is_empty()) {
            lines = &lines[..lines.len() - 1];
        }
        lines
            .iter()
            .map(|line| {
                let mut runs: Vec<_> = line
                    .runs
                    .iter()
                    .map(|run| {
                        json!({
                            "value":String::from_utf16_lossy(&run.text),
                            "style":self.css(run.format, underline, strike),
                        })
                    })
                    .collect();
                if runs.is_empty() {
                    runs.push(json!({"value":"","style":""}));
                }
                json!({"align":line.align.unwrap_or(align),"text":runs})
            })
            .collect()
    }

    fn css(&self, format: Format, underline: bool, strike: bool) -> String {
        let mut css = String::new();
        if let Some(family) = format.font.and_then(|index| self.fonts.get(&index))
            && !family.is_empty()
            && !family.contains([';', ':', '{', '}', '\n', '\r'])
        {
            let _ = write!(css, "font-family:{family};");
        }
        if let Some(size) = format.size.filter(|size| (2..=20000).contains(size)) {
            let _ = write!(css, "font-size:{}px;", f64::from(size) / 2.0);
        }
        if let Some([r, g, b]) = format
            .color
            .and_then(|index| self.colors.get(index))
            .copied()
            .flatten()
        {
            let _ = write!(css, "color:#{r:02x}{g:02x}{b:02x};");
        }
        let color_at = |index: usize| self.colors.get(index).copied().flatten();
        if let Some([r, g, b]) = format
            .highlight
            .and_then(color_at)
            .or_else(|| format.background.and_then(color_at))
        {
            let _ = write!(css, "background-color:#{r:02x}{g:02x}{b:02x};");
        }
        if let Some(bold) = format.bold {
            css.push_str(if bold {
                "font-weight:bold;"
            } else {
                "font-weight:normal;"
            });
        }
        if let Some(italic) = format.italic {
            css.push_str(if italic {
                "font-style:italic;"
            } else {
                "font-style:normal;"
            });
        }
        if format.underline.is_some() || format.strike.is_some() {
            css.push_str(
                match (
                    format.underline.unwrap_or(underline),
                    format.strike.unwrap_or(strike),
                ) {
                    (true, true) => "text-decoration:underline line-through;",
                    (true, false) => "text-decoration:underline;",
                    (false, true) => "text-decoration:line-through;",
                    (false, false) => "text-decoration:none;",
                },
            );
        }
        if let Some(caps) = format.caps {
            css.push_str(if caps {
                "text-transform:uppercase;"
            } else {
                "text-transform:none;"
            });
        }
        if let Some(twips) = format.spacing_twips {
            let _ = write!(css, "letter-spacing:{}px;", f64::from(twips) / 20.0);
        }
        css
    }
}

#[derive(Default)]
struct Reader {
    state: State,
    stack: Vec<State>,
    fallback: usize,
    lines: Vec<Line>,
    line: Line,
    fonts: HashMap<i32, String>,
    font_name: Vec<u16>,
    font_encodings: HashMap<i32, &'static Encoding>,
    colors: Vec<Option<[u8; 3]>>,
    color: [Option<u8>; 3],
    default_font: Option<i32>,
}

impl Reader {
    fn emit(&mut self, unit: u16) {
        match self.state.destination {
            Destination::Ignore => {}
            Destination::FontTable if unit == b';' as u16 => {
                if let Some(index) = self.state.format.font {
                    let name = String::from_utf16_lossy(&self.font_name);
                    self.fonts.insert(index, name.trim().to_owned());
                }
                self.font_name.clear();
            }
            Destination::FontTable => self.font_name.push(unit),
            Destination::ColorTable if unit == b';' as u16 => {
                let components = std::mem::take(&mut self.color);
                self.colors.push(
                    components
                        .iter()
                        .any(Option::is_some)
                        .then(|| components.map(Option::unwrap_or_default)),
                );
            }
            Destination::ColorTable => {}
            Destination::Body if self.state.format.hidden => {}
            Destination::Body if unit == b'\n' as u16 => {
                self.lines.push(std::mem::take(&mut self.line))
            }
            Destination::Body => {
                if self.line.align.is_none() {
                    self.line.align = self.state.align;
                }
                let format = self.state.format;
                if let Some(run) = self.line.runs.last_mut()
                    && (run.format == format
                        || ((0xdc00..=0xdfff).contains(&unit)
                            && run
                                .text
                                .last()
                                .is_some_and(|c| (0xd800..=0xdbff).contains(c))))
                {
                    run.text.push(unit);
                } else {
                    self.line.runs.push(Run {
                        format,
                        text: vec![unit],
                    });
                }
            }
        }
    }

    fn literal(&mut self, ch: char) {
        if self.fallback > 0 {
            self.fallback -= 1;
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                self.emit(*unit);
            }
        }
    }

    fn hex_run(&mut self, chars: &[char], index: &mut usize) {
        let mut bytes = Vec::new();
        while let Some(byte) = hex_byte(chars, *index) {
            *index += 3;
            if self.fallback == 0 {
                bytes.push(byte);
            } else {
                self.fallback -= 1;
            }
            if chars.get(*index) != Some(&'\\') || hex_byte(chars, *index + 1).is_none() {
                break;
            }
            *index += 1;
        }
        if bytes.is_empty() {
            return;
        }
        let encoding = self
            .state
            .format
            .font
            .or(self.default_font)
            .and_then(|font| self.font_encodings.get(&font).copied())
            .unwrap_or(self.state.encoding);
        // A double-byte encoding may write its ASCII-range trail byte literally.
        if let Some(&trail) = chars.get(*index)
            && ('@'..='~').contains(&trail)
            && !matches!(trail, '\\' | '{' | '}')
            && encoding
                .decode_without_bom_handling_and_without_replacement(&bytes)
                .is_none()
        {
            bytes.push(trail as u8);
            if encoding
                .decode_without_bom_handling_and_without_replacement(&bytes)
                .is_some()
            {
                *index += 1;
            } else {
                bytes.pop();
            }
        }
        let (decoded, _) = encoding.decode_without_bom_handling(&bytes);
        for unit in decoded.encode_utf16() {
            self.emit(unit);
        }
    }

    fn control(&mut self, word: &str, number: Option<i32>) {
        use Destination::*;

        match word {
            "fonttbl" if self.state.destination == Body => self.state.destination = FontTable,
            "colortbl" if self.state.destination == Body => self.state.destination = ColorTable,
            "stylesheet" | "info" | "pict" | "object" | "header" | "footer" => {
                self.state.destination = Ignore;
            }
            "uc" => self.state.unicode_fallback = number.unwrap_or(1).clamp(0, 16) as usize,
            "ansicpg" if self.state.destination == Body => {
                if let Some(encoding) = number.and_then(code_page) {
                    self.state.encoding = encoding;
                }
            }
            "mac" if self.state.destination == Body => self.state.encoding = encoding_rs::MACINTOSH,
            "u" if self.state.destination != Ignore => {
                if let Some(code) = number {
                    self.emit(code as u16);
                    self.fallback = self.state.unicode_fallback;
                }
            }
            "par" | "line" if self.state.destination == Body => self.emit(b'\n' as u16),
            "tab" if self.state.destination == Body => self.emit(b'\t' as u16),
            "emdash" if self.state.destination == Body => self.emit('—' as u16),
            "endash" if self.state.destination == Body => self.emit('–' as u16),
            "lquote" if self.state.destination == Body => self.emit('‘' as u16),
            "rquote" if self.state.destination == Body => self.emit('’' as u16),
            "ldblquote" if self.state.destination == Body => self.emit('“' as u16),
            "rdblquote" if self.state.destination == Body => self.emit('”' as u16),
            "bullet" if self.state.destination == Body => self.emit('•' as u16),
            "emspace" if self.state.destination == Body => self.emit('\u{2003}' as u16),
            "enspace" if self.state.destination == Body => self.emit('\u{2002}' as u16),
            "qmspace" if self.state.destination == Body => self.emit('\u{2005}' as u16),
            "f" if self.state.destination == FontTable => {
                self.state.format.font = number;
                self.font_name.clear();
            }
            "fcharset" | "cpg" if self.state.destination == FontTable => {
                let code = if word == "fcharset" {
                    number.and_then(charset_code_page)
                } else {
                    number
                };
                if let (Some(font), Some(encoding)) =
                    (self.state.format.font, code.and_then(code_page))
                {
                    let entry = self.font_encodings.entry(font);
                    if word == "cpg" {
                        entry.insert_entry(encoding);
                    } else {
                        entry.or_insert(encoding);
                    }
                }
            }
            "red" | "green" | "blue" if self.state.destination == ColorTable => {
                let index = match word {
                    "red" => 0,
                    "green" => 1,
                    _ => 2,
                };
                self.color[index] = number.and_then(|value| u8::try_from(value).ok());
            }
            _ if self.state.destination == Body => self.format(word, number),
            _ => {}
        }
    }

    fn format(&mut self, word: &str, number: Option<i32>) {
        let on = Some(number != Some(0));
        let format = &mut self.state.format;
        match word {
            "deff" => self.default_font = number,
            "f" => format.font = number,
            "fs" => format.size = number,
            "cf" => format.color = number.and_then(|index| usize::try_from(index).ok()),
            "cb" => format.background = number.and_then(|index| usize::try_from(index).ok()),
            "highlight" => format.highlight = number.and_then(|index| usize::try_from(index).ok()),
            "b" => format.bold = on,
            "i" => format.italic = on,
            "ul" => format.underline = on,
            "ulnone" => format.underline = Some(false),
            "strike" => format.strike = on,
            "caps" => format.caps = on,
            "v" => format.hidden = number != Some(0),
            "expnd" => format.spacing_twips = number.map(|value| value.clamp(-2000, 2000) * 5),
            "expndtw" => format.spacing_twips = number.map(|value| value.clamp(-10000, 10000)),
            "plain" => {
                *format = Format {
                    font: self.default_font,
                    size: Some(24),
                    color: Some(0),
                    background: None,
                    highlight: None,
                    bold: Some(false),
                    italic: Some(false),
                    underline: Some(false),
                    strike: Some(false),
                    caps: Some(false),
                    spacing_twips: Some(0),
                    hidden: false,
                }
            }
            "ql" | "qr" | "qc" | "qj" | "pard" => {
                let align = match word {
                    "qr" => "right",
                    "qc" => "center",
                    "qj" => "justify",
                    _ => "left",
                };
                self.state.align = Some(align);
                self.line.align = Some(align);
            }
            _ => {}
        }
    }
}

pub(super) fn decode(text: &str) -> Document {
    let chars: Vec<char> = text.chars().collect();
    let mut reader = Reader::default();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        index += 1;
        match ch {
            '{' => {
                reader.fallback = 0;
                reader.stack.push(reader.state);
            }
            '}' => {
                reader.fallback = 0;
                if let Some(state) = reader.stack.pop() {
                    reader.state = state;
                }
            }
            '\\' => {
                let Some(&next) = chars.get(index) else {
                    break;
                };
                if next == '\'' && hex_byte(&chars, index).is_some() {
                    reader.hex_run(&chars, &mut index);
                    continue;
                }
                if !next.is_ascii_alphabetic() {
                    index += 1;
                    if reader.fallback > 0 {
                        reader.fallback -= 1;
                        continue;
                    }
                    let literal = match next {
                        '*' => {
                            reader.state.destination = Destination::Ignore;
                            None
                        }
                        '\\' | '{' | '}' => Some(next),
                        '~' => Some('\u{a0}'),
                        '_' => Some('\u{2011}'),
                        _ => None,
                    };
                    if let Some(ch) = literal {
                        reader.literal(ch);
                    }
                    continue;
                }
                let start = index;
                while chars.get(index).is_some_and(char::is_ascii_alphabetic) {
                    index += 1;
                }
                let word: String = chars[start..index].iter().collect();
                let start = index;
                if chars.get(index) == Some(&'-') {
                    index += 1;
                }
                while chars.get(index).is_some_and(char::is_ascii_digit) {
                    index += 1;
                }
                let number = chars[start..index]
                    .iter()
                    .collect::<String>()
                    .parse::<i32>()
                    .ok();
                if chars.get(index) == Some(&' ') {
                    index += 1;
                }
                if word == "bin" {
                    index = index
                        .saturating_add(number.unwrap_or(0).max(0) as usize)
                        .min(chars.len());
                    reader.fallback = reader.fallback.saturating_sub(1);
                } else if reader.fallback > 0 {
                    reader.fallback -= 1;
                } else {
                    reader.control(&word, number);
                }
            }
            '\r' | '\n' => {}
            _ => reader.literal(ch),
        }
    }
    reader.lines.push(reader.line);
    Document {
        lines: reader.lines,
        fonts: reader.fonts,
        colors: reader.colors,
    }
}

fn hex_byte(chars: &[char], index: usize) -> Option<u8> {
    let ['\'', high, low] = chars.get(index..index + 3)? else {
        return None;
    };
    Some((high.to_digit(16)? * 16 + low.to_digit(16)?) as u8)
}

fn code_page(code: i32) -> Option<&'static Encoding> {
    use encoding_rs::*;
    Some(match code {
        866 => IBM866,
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 | 20936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1252 => WINDOWS_1252,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        10000 => MACINTOSH,
        54936 => GB18030,
        65001 => UTF_8,
        _ => return None,
    })
}

fn charset_code_page(charset: i32) -> Option<i32> {
    Some(match charset {
        77 => 10000,
        128 => 932,
        129 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn escaped_bytes_use_the_document_or_font_code_page() {
        let cyrillic = decode(r"{\rtf1\ansi\ansicpg1251 \'cf\'f0\'e8\'e2\'e5\'f2}");
        assert_eq!(cyrillic.plain_text(), "Привет");
        let mixed = decode(
            r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Arial;}{\f1\fcharset128 MS Gothic;}}\f0 Caf\'e9 {\f1\'93\'fa\'96\'7b\'83A} \'80}",
        );
        assert_eq!(mixed.plain_text(), "Café 日本ア €");
        assert_eq!(
            mixed.styled_lines("", false, false)[0]["text"][1]["style"],
            "font-family:MS Gothic;"
        );
    }

    #[test]
    fn unicode_fallback_counts_escaped_bytes_before_decoding() {
        let japanese = decode(r"{\rtf1\ansicpg932\uc2\u26085\'93\'fa\'96\'7b}");
        assert_eq!(japanese.plain_text(), "日本");
    }

    #[test]
    fn unicode_fallback_counts_controls_and_ends_at_group_boundaries() {
        let document = decode(r"{\rtf1\uc4\u8212\emdash\b\*\bin3 {\}A{\u8217}B\u8216{C}D}");
        assert_eq!(document.plain_text(), "—A’B‘CD");
        assert_eq!(
            document.styled_lines("", false, false)[0]["text"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn imports_typographic_spacing() {
        let document = decode(r"{\rtf1 A\emspace B\enspace C\qmspace D}");
        assert_eq!(document.plain_text(), "A\u{2003}B\u{2002}C\u{2005}D");
    }

    #[test]
    fn caps_and_character_spacing_follow_rtf_group_scope() {
        let document =
            decode(r"{\rtf1\caps\expnd4 first {\caps0\expndtw-10 tighter} again \plain normal}");
        assert_eq!(document.plain_text(), "first tighter again normal");
        let lines = document.styled_lines("", false, false);
        let runs = lines[0]["text"].as_array().unwrap();
        assert_eq!(runs.len(), 4);
        let spaced_caps = "text-transform:uppercase;letter-spacing:1px;";
        assert_eq!(runs[0]["style"], spaced_caps);
        assert_eq!(
            runs[1]["style"],
            "text-transform:none;letter-spacing:-0.5px;"
        );
        assert_eq!(runs[2]["style"], spaced_caps);
        assert!(
            runs[3]["style"]
                .as_str()
                .unwrap()
                .ends_with("text-transform:none;letter-spacing:0px;")
        );
    }
}
