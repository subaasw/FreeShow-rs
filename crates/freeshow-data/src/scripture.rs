// SPDX-License-Identifier: GPL-3.0-only

//! Turns USFM, OSIS, Zefania, Beblia, and OpenSong scripture into the FreeShow `.fsb` JSON array.

use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};

use crate::{BibleFile, DataError, json_u64};

pub fn import_sources(dir: &Path) -> Result<Vec<String>, DataError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut warnings = Vec::new();
    let mut usfm = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "usfm" | "sfm" => usfm.push(path),
            "xml" => {
                if let Err(err) = import_xml_file(&path) {
                    warnings.push(format!("{}: {err}", path.display()));
                }
            }
            _ => {}
        }
    }
    usfm.sort();
    if !usfm.is_empty()
        && let Err(err) = import_usfm_files(dir, &usfm)
    {
        warnings.push(err.to_string());
    }
    Ok(warnings)
}

pub fn import_scripture(file_name: &str, text: &str) -> Result<BibleFile, DataError> {
    let document = if text.trim_start().starts_with('[') || text.trim_start().starts_with('{') {
        freeshow_document(text)?
    } else if text.contains("XMLBIBLE") && text.contains("BIBLEBOOK") {
        zefania_document(text)?
    } else if text.contains("osisText") && text.contains("osisID") {
        osis_document(text)?
    } else if looks_like_beblia(text) {
        beblia_document(text)?
    } else if looks_like_opensong_bible(text) {
        opensong_document(text)?
    } else if text.contains("\\id") || text.contains("\\v") {
        usfm_document(text)
    } else {
        return Err(DataError::Scripture(format!(
            "{file_name} is not FreeShow, USFM, OSIS, Zefania, Beblia, or OpenSong"
        )));
    };
    let mut document = document;
    if document.get("name").and_then(Value::as_str).is_none() {
        document
            .as_object_mut()
            .expect("bible document is an object")
            .insert("name".to_string(), Value::String("Scripture".into()));
    }
    let id = file_name
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let id = if id.is_empty() {
        "scripture".to_string()
    } else {
        id
    };
    Ok(BibleFile::from_document(id, document))
}

pub fn reference_hits(document: &Value, query: &str) -> Option<Vec<crate::VerseHit>> {
    let query = query.trim();
    let (book_query, chapter, start, end) = parse_reference(query)?;
    let books = document.get("books")?.as_array()?;
    let book = books.iter().find(|book| book_matches(book, &book_query))?;
    let book_name = book.get("name").and_then(Value::as_str).unwrap_or("");
    let chapters = book.get("chapters")?.as_array()?;
    let chapter_value = chapters
        .iter()
        .find(|chapter_value| json_u64(chapter_value.get("number")) == Some(chapter))?;
    let verses = chapter_value.get("verses")?.as_array()?;
    let mut hits = Vec::new();
    for verse in verses {
        let number = json_u64(verse.get("number")).unwrap_or(0);
        if number < start || number > end {
            continue;
        }
        hits.push(crate::VerseHit {
            reference: format!("{book_name} {chapter}:{number}"),
            text: verse
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Some(hits)
}

fn import_xml_file(path: &Path) -> Result<(), DataError> {
    let text = fs::read_to_string(path)?;
    let bible = import_scripture(
        path.file_stem().and_then(|s| s.to_str()).unwrap_or("bible"),
        &text,
    )?;
    let dest = path.with_extension("fsb");
    if dest.exists() && !newer(path, &dest) {
        return Ok(());
    }
    crate::atomic_write(&dest, bible.to_json()?.as_bytes())?;
    Ok(())
}

fn import_usfm_files(dir: &Path, files: &[PathBuf]) -> Result<(), DataError> {
    let dest = dir.join("USFM.fsb");
    let stale = !dest.exists() || files.iter().any(|path| newer(path, &dest));
    if !stale {
        return Ok(());
    }
    let mut books = Vec::new();
    for path in files {
        let text = fs::read_to_string(path)?;
        books.extend(usfm_books(&text));
    }
    books.sort_by_key(|book| json_u64(book.get("number")).unwrap_or(0));
    let name = if books.len() == 1 {
        books[0]
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("USFM")
            .to_string()
    } else {
        "USFM".to_string()
    };
    let bible = BibleFile::from_document("usfm", json!({ "name": name, "books": books }));
    crate::atomic_write(&dest, bible.to_json()?.as_bytes())?;
    Ok(())
}

fn newer(source: &Path, dest: &Path) -> bool {
    let source_time = source.metadata().and_then(|meta| meta.modified()).ok();
    let dest_time = dest.metadata().and_then(|meta| meta.modified()).ok();
    match (source_time, dest_time) {
        (Some(source_time), Some(dest_time)) => source_time > dest_time,
        _ => true,
    }
}

fn freeshow_document(text: &str) -> Result<Value, DataError> {
    let value: Value = serde_json::from_str(text)?;
    if let Some(pair) = value.as_array() {
        if pair.len() == 2 && pair[1].is_object() {
            return Ok(pair[1].clone());
        }
        return Err(DataError::Scripture(
            "FreeShow bible must be [id, bible]".into(),
        ));
    }
    if value.get("books").is_some() {
        return Ok(value);
    }
    Err(DataError::Scripture("JSON bible has no books array".into()))
}

fn zefania_document(text: &str) -> Result<Value, DataError> {
    read_xml(text, XmlKind::Zefania)
}

fn osis_document(text: &str) -> Result<Value, DataError> {
    read_xml(text, XmlKind::Osis)
}

fn beblia_document(text: &str) -> Result<Value, DataError> {
    read_xml(text, XmlKind::Beblia)
}

fn opensong_document(text: &str) -> Result<Value, DataError> {
    read_xml(text, XmlKind::OpenSong)
}

fn looks_like_beblia(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<bible")
        && lower.contains("<verse")
        && !lower.contains("osistext")
        && !lower.contains("biblebook")
}

fn looks_like_opensong_bible(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<b ") && lower.contains("<c ") && lower.contains("<v ")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum XmlKind {
    Zefania,
    Osis,
    Beblia,
    OpenSong,
}

fn read_xml(text: &str, kind: XmlKind) -> Result<Value, DataError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut build = Build {
        strip_brackets: kind == XmlKind::OpenSong,
        ..Default::default()
    };
    let mut skip = 0u32;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if skip > 0 || name.eq_ignore_ascii_case("note") {
                    if name.eq_ignore_ascii_case("note") || skip > 0 {
                        skip += 1;
                    }
                    if name.eq_ignore_ascii_case("note") {
                        continue;
                    }
                }
                if skip > 0 {
                    continue;
                }
                start_element(&mut build, kind, &name, &event)?;
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                if skip == 0 && name.eq_ignore_ascii_case("br") {
                    build.push_text(" ");
                }
            }
            Ok(Event::Text(event)) => {
                if skip == 0 {
                    let text = event
                        .unescape()
                        .map_err(|err| DataError::Scripture(err.to_string()))?;
                    build.push_text(&text);
                }
            }
            Ok(Event::CData(event)) => {
                if skip == 0 {
                    build.push_text(&String::from_utf8_lossy(&event));
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                end_element(&mut build, &name);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(DataError::Scripture(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    build.finish_book();
    if build.books.is_empty() {
        return Err(DataError::Scripture("bible XML has no books".into()));
    }
    let name = if build.title.is_empty() {
        if build.bible_name.is_empty() {
            "Scripture".to_string()
        } else {
            build.bible_name.clone()
        }
    } else {
        build.title.clone()
    };
    let mut document = json!({ "name": name, "books": build.books });
    if !build.copyright.is_empty() {
        document["metadata"] = json!({ "copyright": build.copyright });
    }
    Ok(document)
}

fn start_element(
    build: &mut Build,
    kind: XmlKind,
    name: &str,
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<(), DataError> {
    let attr = |key: &str| attribute(event, key);
    match name.to_ascii_lowercase().as_str() {
        "xmlbible" => {
            if let Some(name) = attr("biblename") {
                build.bible_name = name;
            }
        }
        "bible" if kind == XmlKind::Beblia => {
            if let Some(name) = attr("name").or_else(|| attr("translation")) {
                build.bible_name = name;
            }
            if let Some(info) = attr("info") {
                build.copyright = info;
            }
        }
        "book" if kind == XmlKind::Beblia => {
            build.finish_book();
            build.book_number = attr("number")
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            build.book_name = attr("name").unwrap_or_else(|| english_book(build.book_number));
            build.book_abbr.clear();
            build.in_book = true;
        }
        "b" if kind == XmlKind::OpenSong => {
            build.finish_book();
            build.book_number = build.books.len() as u64 + 1;
            build.book_name = attr("n").unwrap_or_else(|| english_book(build.book_number));
            build.book_abbr.clear();
            build.in_book = true;
        }
        "title" => build.in_title = true,
        "biblebook" if kind == XmlKind::Zefania => {
            build.finish_book();
            build.book_name = attr("bname").unwrap_or_default();
            build.book_abbr = attr("babbr").unwrap_or_default();
            build.book_number = attr("bnumber")
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            build.in_book = true;
        }
        "div" if kind == XmlKind::Osis => {
            let id = attr("osisID").unwrap_or_default();
            let is_book = attr("type").as_deref() == Some("book") || !id.contains('.');
            if is_book && !id.is_empty() {
                build.finish_book();
                let (number, english) = osis_book(&id);
                build.book_name = attr("name").unwrap_or_else(|| english.to_string());
                build.book_abbr = attr("abbr").unwrap_or_else(|| id.clone());
                build.book_number = number;
                build.in_book = true;
            }
        }
        "chapter" | "c" => {
            build.finish_chapter();
            build.chapter_number = attr("cnumber")
                .or_else(|| attr("number"))
                .or_else(|| attr("n"))
                .or_else(|| attr("osisID"))
                .as_deref()
                .and_then(last_number)
                .unwrap_or(build.chapters.len() as u64 + 1);
            build.in_chapter = true;
        }
        "vers" | "verse" | "v" => {
            build.finish_verse();
            build.verse_number = attr("vnumber")
                .or_else(|| attr("number"))
                .or_else(|| attr("n"))
                .or_else(|| attr("osisID"))
                .as_deref()
                .and_then(last_number)
                .unwrap_or(build.verses.len() as u64 + 1);
            build.in_verse = true;
        }
        _ => {}
    }
    Ok(())
}

fn end_element(build: &mut Build, name: &str) {
    match name.to_ascii_lowercase().as_str() {
        "title" => build.in_title = false,
        "biblebook" | "div" | "book" | "b" => {
            if build.in_book {
                build.finish_book();
            }
        }
        "chapter" | "c" => build.finish_chapter(),
        "vers" | "verse" | "v" => build.finish_verse(),
        _ => {}
    }
}

#[derive(Default)]
struct Build {
    bible_name: String,
    title: String,
    in_title: bool,
    in_book: bool,
    in_chapter: bool,
    in_verse: bool,
    book_name: String,
    book_abbr: String,
    copyright: String,
    strip_brackets: bool,
    book_number: u64,
    chapter_number: u64,
    verse_number: u64,
    verse_text: String,
    verses: Vec<Value>,
    chapters: Vec<Value>,
    books: Vec<Value>,
}

impl Build {
    fn push_text(&mut self, text: &str) {
        if self.in_title {
            if !self.title.is_empty() {
                self.title.push(' ');
            }
            self.title.push_str(text.trim());
            return;
        }
        if self.in_verse {
            self.verse_text.push(' ');
            self.verse_text.push_str(text);
        }
    }

    fn finish_verse(&mut self) {
        if !self.in_verse {
            return;
        }
        let text = clean_verse(&self.verse_text);
        let text = if self.strip_brackets {
            strip_note_markers(&text)
        } else {
            text
        };
        if !text.is_empty() {
            self.verses.push(json!({
                "number": self.verse_number,
                "text": text,
            }));
        }
        self.verse_text.clear();
        self.in_verse = false;
    }

    fn finish_chapter(&mut self) {
        self.finish_verse();
        if !self.in_chapter {
            return;
        }
        self.chapters.push(json!({
            "number": self.chapter_number,
            "verses": std::mem::take(&mut self.verses),
        }));
        self.in_chapter = false;
    }

    fn finish_book(&mut self) {
        self.finish_chapter();
        if !self.in_book {
            return;
        }
        let has_text = self.chapters.iter().any(|chapter| {
            chapter
                .get("verses")
                .and_then(Value::as_array)
                .is_some_and(|verses| !verses.is_empty())
        });
        if !has_text {
            self.chapters.clear();
            self.in_book = false;
            return;
        }
        if self.book_name.is_empty() {
            self.book_name = english_book(self.book_number);
        }
        let mut book = json!({
            "number": self.book_number,
            "name": self.book_name,
            "chapters": std::mem::take(&mut self.chapters),
        });
        if !self.book_abbr.is_empty() {
            book.as_object_mut().expect("book object").insert(
                "abbreviation".to_string(),
                Value::String(self.book_abbr.clone()),
            );
        }
        self.books.push(book);
        self.in_book = false;
        self.book_name.clear();
        self.book_abbr.clear();
    }
}

fn usfm_document(text: &str) -> Value {
    let books = usfm_books(text);
    let name = books
        .first()
        .and_then(|book| book.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("USFM");
    json!({ "name": name, "books": books })
}

fn usfm_books(text: &str) -> Vec<Value> {
    let mut books = Vec::new();
    let mut book_name = String::new();
    let mut book_number = 0u64;
    let mut book_abbr = String::new();
    let mut chapters = Vec::new();
    let mut verses = Vec::new();
    let mut chapter_number = 0u64;
    let mut verse_number = 0u64;
    let mut verse_text = String::new();
    let mut in_book = false;
    let mut in_chapter = false;
    let mut skipping = false;
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let flush_verse = |verses: &mut Vec<Value>, verse_number: u64, verse_text: &mut String| {
        let text = clean_verse(verse_text);
        verse_text.clear();
        if verse_number > 0 && !text.is_empty() {
            verses.push(json!({"number": verse_number, "text": text}));
        }
    };
    let flush_chapter = |chapters: &mut Vec<Value>,
                         verses: &mut Vec<Value>,
                         chapter_number: u64,
                         in_chapter: &mut bool| {
        if *in_chapter {
            chapters.push(json!({
                "number": chapter_number,
                "verses": std::mem::take(verses),
            }));
            *in_chapter = false;
        }
    };
    while index < chars.len() {
        if chars[index] != '\\' {
            if !skipping {
                verse_text.push(chars[index]);
            }
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        while index < chars.len() && chars[index].is_ascii_alphanumeric() {
            index += 1;
        }
        if index < chars.len() && chars[index] == '*' {
            skipping = false;
            index += 1;
            continue;
        }
        let marker: String = chars[start..index].iter().collect();
        let marker = marker.to_ascii_lowercase();
        if marker == "f" || marker == "x" || marker == "fe" {
            skipping = true;
            continue;
        }
        if matches!(
            marker.as_str(),
            "id" | "h" | "toc1" | "mt" | "mt1" | "c" | "v"
        ) {
            let rest = take_rest(&chars, &mut index);
            match marker.as_str() {
                "id" => {
                    flush_verse(&mut verses, verse_number, &mut verse_text);
                    flush_chapter(&mut chapters, &mut verses, chapter_number, &mut in_chapter);
                    if in_book {
                        books.push(book_value(
                            book_number,
                            &book_name,
                            &book_abbr,
                            std::mem::take(&mut chapters),
                        ));
                    }
                    let mut parts = rest.split_whitespace();
                    let code = parts.next().unwrap_or("").to_string();
                    let (number, english) = usfm_book(&code);
                    book_number = number;
                    book_abbr = code;
                    book_name = parts.collect::<Vec<_>>().join(" ");
                    if book_name.is_empty() {
                        book_name = english.to_string();
                    }
                    in_book = true;
                    verse_number = 0;
                }
                "h" | "toc1" | "mt" | "mt1" => {
                    if marker == "h" || book_name.is_empty() || book_name == usfm_book(&book_abbr).1
                    {
                        let name = rest.trim();
                        if !name.is_empty() {
                            book_name = name.to_string();
                        }
                    }
                }
                "c" => {
                    flush_verse(&mut verses, verse_number, &mut verse_text);
                    flush_chapter(&mut chapters, &mut verses, chapter_number, &mut in_chapter);
                    chapter_number = rest
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                    in_chapter = true;
                    verse_number = 0;
                }
                "v" => {
                    flush_verse(&mut verses, verse_number, &mut verse_text);
                    let mut parts = rest.splitn(2, char::is_whitespace);
                    verse_number = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                    if !in_chapter {
                        chapter_number = 1;
                        in_chapter = true;
                    }
                    if let Some(text) = parts.next() {
                        verse_text.push_str(text);
                    }
                }
                _ => {}
            }
        }
    }
    flush_verse(&mut verses, verse_number, &mut verse_text);
    flush_chapter(&mut chapters, &mut verses, chapter_number, &mut in_chapter);
    if in_book {
        books.push(book_value(book_number, &book_name, &book_abbr, chapters));
    }
    books
}

fn take_rest(chars: &[char], index: &mut usize) -> String {
    while *index < chars.len() && chars[*index].is_whitespace() && chars[*index] != '\n' {
        *index += 1;
    }
    let start = *index;
    while *index < chars.len() && chars[*index] != '\\' && chars[*index] != '\n' {
        *index += 1;
    }
    chars[start..*index].iter().collect()
}

fn book_value(number: u64, name: &str, abbr: &str, chapters: Vec<Value>) -> Value {
    let mut book = json!({
        "number": number,
        "name": name,
        "chapters": chapters,
    });
    if !abbr.is_empty() {
        book.as_object_mut()
            .expect("book object")
            .insert("abbreviation".to_string(), Value::String(abbr.to_string()));
    }
    book
}

/// Reads "John 3:16", "John 3:16-18", "John 3.16", "John 3 16" and a whole chapter, "John 3".
fn parse_reference(query: &str) -> Option<(String, u64, u64, u64)> {
    let mut parts = query.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let verses = |text: &str| -> Option<(u64, u64)> {
        let text = text.trim_end_matches('-');
        if let Some((start, end)) = text.split_once('-') {
            let (start, end) = (start.parse().ok()?, end.parse().ok()?);
            (end >= start).then_some((start, end))
        } else {
            let start = text.parse().ok()?;
            Some((start, start))
        }
    };
    let last = parts.pop()?;
    let (chapter, start, end) = if let Some((chapter, rest)) = last.split_once([':', '.']) {
        let (start, end) = verses(rest)?;
        (chapter.parse().ok()?, start, end)
    } else if parts.len() >= 2
        && let Some(chapter) = parts.last().and_then(|part| part.parse::<u64>().ok())
        && let Some((start, end)) = verses(last)
    {
        parts.pop();
        (chapter, start, end)
    } else {
        (last.parse().ok()?, 1, u64::MAX)
    };
    Some((parts.join(" "), chapter, start, end))
}

fn book_matches(book: &Value, query: &str) -> bool {
    let query = query.to_lowercase();
    let name = book.get("name").and_then(Value::as_str).unwrap_or("");
    let abbr = book
        .get("abbreviation")
        .and_then(Value::as_str)
        .unwrap_or("");
    name.eq_ignore_ascii_case(&query)
        || abbr.eq_ignore_ascii_case(&query)
        || (query.len() >= 3 && name.to_lowercase().starts_with(&query))
}

fn last_number(text: &str) -> Option<u64> {
    text.rsplit(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn english_book(number: u64) -> String {
    OSIS.get(number.saturating_sub(1) as usize)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_default()
}

fn strip_note_markers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            let mut end = index + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end > index + 1
                && end + 1 < chars.len()
                && chars[end] == ']'
                && chars[end + 1] == ' '
            {
                index = end + 2;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn clean_verse(text: &str) -> String {
    text.replace("<BR art=\"x-p\"/>", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn local_name(name: &[u8]) -> String {
    let raw = String::from_utf8_lossy(name);
    raw.rsplit(':').next().unwrap_or("").to_string()
}

fn attribute(event: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    event
        .try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|attr| attr.unescape_value().ok().map(|value| value.to_string()))
}

fn osis_book(id: &str) -> (u64, &'static str) {
    OSIS.iter()
        .enumerate()
        .find(|(_, (code, _))| code.eq_ignore_ascii_case(id))
        .map(|(index, (_, name))| ((index as u64) + 1, *name))
        .unwrap_or((0, ""))
}

fn usfm_book(code: &str) -> (u64, &'static str) {
    USFM.iter()
        .enumerate()
        .find(|(_, (mark, _))| mark.eq_ignore_ascii_case(code))
        .map(|(index, (_, name))| ((index as u64) + 1, *name))
        .unwrap_or((0, ""))
}

const OSIS: &[(&str, &str)] = &[
    ("Gen", "Genesis"),
    ("Exod", "Exodus"),
    ("Lev", "Leviticus"),
    ("Num", "Numbers"),
    ("Deut", "Deuteronomy"),
    ("Josh", "Joshua"),
    ("Judg", "Judges"),
    ("Ruth", "Ruth"),
    ("1Sam", "1 Samuel"),
    ("2Sam", "2 Samuel"),
    ("1Kgs", "1 Kings"),
    ("2Kgs", "2 Kings"),
    ("1Chr", "1 Chronicles"),
    ("2Chr", "2 Chronicles"),
    ("Ezra", "Ezra"),
    ("Neh", "Nehemiah"),
    ("Esth", "Esther"),
    ("Job", "Job"),
    ("Ps", "Psalm"),
    ("Prov", "Proverbs"),
    ("Eccl", "Ecclesiastes"),
    ("Song", "Song of Solomon"),
    ("Isa", "Isaiah"),
    ("Jer", "Jeremiah"),
    ("Lam", "Lamentations"),
    ("Ezek", "Ezekiel"),
    ("Dan", "Daniel"),
    ("Hos", "Hosea"),
    ("Joel", "Joel"),
    ("Amos", "Amos"),
    ("Obad", "Obadiah"),
    ("Jonah", "Jonah"),
    ("Mic", "Micah"),
    ("Nah", "Nahum"),
    ("Hab", "Habakkuk"),
    ("Zeph", "Zephaniah"),
    ("Hag", "Haggai"),
    ("Zech", "Zechariah"),
    ("Mal", "Malachi"),
    ("Matt", "Matthew"),
    ("Mark", "Mark"),
    ("Luke", "Luke"),
    ("John", "John"),
    ("Acts", "Acts"),
    ("Rom", "Romans"),
    ("1Cor", "1 Corinthians"),
    ("2Cor", "2 Corinthians"),
    ("Gal", "Galatians"),
    ("Eph", "Ephesians"),
    ("Phil", "Philippians"),
    ("Col", "Colossians"),
    ("1Thess", "1 Thessalonians"),
    ("2Thess", "2 Thessalonians"),
    ("1Tim", "1 Timothy"),
    ("2Tim", "2 Timothy"),
    ("Titus", "Titus"),
    ("Phlm", "Philemon"),
    ("Heb", "Hebrews"),
    ("Jas", "James"),
    ("1Pet", "1 Peter"),
    ("2Pet", "2 Peter"),
    ("1John", "1 John"),
    ("2John", "2 John"),
    ("3John", "3 John"),
    ("Jude", "Jude"),
    ("Rev", "Revelation"),
];

const USFM: &[(&str, &str)] = &[
    ("GEN", "Genesis"),
    ("EXO", "Exodus"),
    ("LEV", "Leviticus"),
    ("NUM", "Numbers"),
    ("DEU", "Deuteronomy"),
    ("JOS", "Joshua"),
    ("JDG", "Judges"),
    ("RUT", "Ruth"),
    ("1SA", "1 Samuel"),
    ("2SA", "2 Samuel"),
    ("1KI", "1 Kings"),
    ("2KI", "2 Kings"),
    ("1CH", "1 Chronicles"),
    ("2CH", "2 Chronicles"),
    ("EZR", "Ezra"),
    ("NEH", "Nehemiah"),
    ("EST", "Esther"),
    ("JOB", "Job"),
    ("PSA", "Psalm"),
    ("PRO", "Proverbs"),
    ("ECC", "Ecclesiastes"),
    ("SNG", "Song of Solomon"),
    ("ISA", "Isaiah"),
    ("JER", "Jeremiah"),
    ("LAM", "Lamentations"),
    ("EZK", "Ezekiel"),
    ("DAN", "Daniel"),
    ("HOS", "Hosea"),
    ("JOL", "Joel"),
    ("AMO", "Amos"),
    ("OBA", "Obadiah"),
    ("JON", "Jonah"),
    ("MIC", "Micah"),
    ("NAM", "Nahum"),
    ("HAB", "Habakkuk"),
    ("ZEP", "Zephaniah"),
    ("HAG", "Haggai"),
    ("ZEC", "Zechariah"),
    ("MAL", "Malachi"),
    ("MAT", "Matthew"),
    ("MRK", "Mark"),
    ("LUK", "Luke"),
    ("JHN", "John"),
    ("ACT", "Acts"),
    ("ROM", "Romans"),
    ("1CO", "1 Corinthians"),
    ("2CO", "2 Corinthians"),
    ("GAL", "Galatians"),
    ("EPH", "Ephesians"),
    ("PHP", "Philippians"),
    ("COL", "Colossians"),
    ("1TH", "1 Thessalonians"),
    ("2TH", "2 Thessalonians"),
    ("1TI", "1 Timothy"),
    ("2TI", "2 Timothy"),
    ("TIT", "Titus"),
    ("PHM", "Philemon"),
    ("HEB", "Hebrews"),
    ("JAS", "James"),
    ("1PE", "1 Peter"),
    ("2PE", "2 Peter"),
    ("1JN", "1 John"),
    ("2JN", "2 John"),
    ("3JN", "3 John"),
    ("JUD", "Jude"),
    ("REV", "Revelation"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_read_the_ways_operators_type_them() {
        let max = u64::MAX;
        for (query, expected) in [
            ("John 3:16", ("John", 3, 16, 16)),
            ("John 3:16-18", ("John", 3, 16, 18)),
            ("John 3.16", ("John", 3, 16, 16)),
            ("John 3 16", ("John", 3, 16, 16)),
            ("John 3 16-18", ("John", 3, 16, 18)),
            ("John 3", ("John", 3, 1, max)),
            ("1 John 1:9", ("1 John", 1, 9, 9)),
            ("1 John 1", ("1 John", 1, 1, max)),
            ("Song of Solomon 2:4", ("Song of Solomon", 2, 4, 4)),
        ] {
            let (book, chapter, start, end) = expected;
            assert_eq!(
                parse_reference(query),
                Some((book.to_string(), chapter, start, end)),
                "{query}"
            );
        }
        assert_eq!(parse_reference("John"), None);
        assert_eq!(parse_reference("John 3:18-16"), None);
        assert_eq!(parse_reference("grace alone"), None);
    }

    #[test]
    fn zefania_osis_and_usfm_become_searchable_bibles() {
        let zefania = r#"
            <XMLBIBLE biblename="Sample">
              <INFORMATION><title>Sample Zefania</title></INFORMATION>
              <BIBLEBOOK bnumber="43" bname="John">
                <CHAPTER cnumber="3">
                  <VERS vnumber="16">For God so loved the world<NOTE>skip</NOTE></VERS>
                  <VERS vnumber="17">For God sent the Son</VERS>
                </CHAPTER>
              </BIBLEBOOK>
            </XMLBIBLE>
        "#;
        let bible = import_scripture("sample.xml", zefania).unwrap();
        assert_eq!(bible.name(), "Sample Zefania");
        let hits = bible.search("John 3:16-17");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].reference, "John 3:16");
        assert!(hits[0].text.contains("loved"));
        assert!(!hits[0].text.contains("skip"));

        let osis = r#"
            <osis><osisText>
              <header><work><title>Sample OSIS</title></work></header>
              <div type="book" osisID="Gen">
                <chapter osisID="Gen.1">
                  <verse osisID="Gen.1.1">In the beginning</verse>
                </chapter>
              </div>
            </osisText></osis>
        "#;
        let bible = import_scripture("osis.xml", osis).unwrap();
        assert_eq!(bible.search("Genesis 1:1")[0].text, "In the beginning");

        let usfm = "\\id JHN\\h John\n\\c 3\n\\v 16 For God so loved the world\n\\f a footnote\\f*\n\\v 17 For God sent the Son\n";
        let bible = import_scripture("john.usfm", usfm).unwrap();
        assert_eq!(
            bible.search("JHN 3:16")[0].text,
            "For God so loved the world"
        );
        assert!(!bible.search("loved")[0].text.contains("footnote"));

        let beblia = r#"
            <bible name="KJV" info="public domain">
              <testament>
                <book number="43">
                  <chapter number="3">
                    <verse number="16">For God so loved the world</verse>
                  </chapter>
                </book>
              </testament>
            </bible>
        "#;
        let bible = import_scripture("kjv.xml", beblia).unwrap();
        assert_eq!(bible.name(), "KJV");
        assert_eq!(
            bible.search("John 3:16")[0].text,
            "For God so loved the world"
        );

        let opensong = r#"
            <bible>
              <b n="John">
                <c n="3">
                  <v n="16">For God so loved [1] the world</v>
                  <v n="17"></v>
                </c>
              </b>
            </bible>
        "#;
        let bible = import_scripture("opensong.xml", opensong).unwrap();
        assert_eq!(
            bible.search("John 3:16")[0].text,
            "For God so loved the world"
        );
        assert!(bible.search("John 3:17").is_empty());
    }

    #[test]
    fn library_writes_fsb_files_from_xml_and_usfm() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-bibles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("sample.xml"),
            r#"<XMLBIBLE biblename="KJV"><BIBLEBOOK bnumber="43" bname="John"><CHAPTER cnumber="3"><VERS vnumber="16">loved</VERS></CHAPTER></BIBLEBOOK></XMLBIBLE>"#,
        )
        .unwrap();
        fs::write(
            root.join("43-JHN.usfm"),
            "\\id JHN\n\\c 1\n\\v 1 In the beginning was the Word\n",
        )
        .unwrap();
        let warnings = import_sources(&root).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let saved = BibleFile::load(&root.join("sample.fsb")).unwrap();
        assert_eq!(saved.search("John 3:16")[0].text, "loved");
        let usfm = BibleFile::load(&root.join("USFM.fsb")).unwrap();
        assert_eq!(
            usfm.search("John 1:1")[0].text,
            "In the beginning was the Word"
        );
        let again = import_sources(&root).unwrap();
        assert!(again.is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
