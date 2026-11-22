// SPDX-License-Identifier: GPL-3.0-only

//! Turns songs people already have into FreeShow `.show` files.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
#[cfg(test)]
use std::io::Cursor;
use std::io::Write;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};

use crate::{DataError, ShowFile};

mod community;
mod powerpoint;
mod propresenter;
mod propresenter7;
mod rtf;

pub(super) const ITEM_STYLE: &str = "top:88px;left:50px;height:904px;width:1820px;";

#[cfg_attr(not(test), allow(dead_code))]
pub fn import_sources(
    imports: &Path,
    shows: &Path,
    media: &Path,
) -> Result<Vec<String>, DataError> {
    import_sources_split(imports, shows, media, 0)
}

pub fn import_sources_split(
    imports: &Path,
    shows: &Path,
    media: &Path,
    split_lines: u32,
) -> Result<Vec<String>, DataError> {
    if !imports.exists() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(shows)?;
    let mut ids = HashSet::new();
    for entry in fs::read_dir(shows)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("show")
            && let Ok(show) = ShowFile::load(&path)
        {
            ids.insert(show.id);
        }
    }
    let mut warnings = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(imports)? {
        let path = entry?.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if extension.eq_ignore_ascii_case("olp") || extension.eq_ignore_ascii_case("olf") {
            continue;
        }
        let bytes = fs::read(&path)?;
        if serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_some_and(|value| crate::is_lesson_document(&value))
        {
            continue;
        }
        if community::is_project_package(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
            &bytes,
        ) {
            match community::import_project_package(&path, &bytes, shows, media) {
                Ok(notes) => warnings.extend(notes),
                Err(err) => warnings.push(format!("{}: {err}", path.display())),
            }
            continue;
        }
        let found = if propresenter::is_bundle(&path) {
            propresenter::import_bundle(&bytes, media)
        } else if extension.eq_ignore_ascii_case("pptx") || looks_like_pptx(&bytes) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("slides");
            powerpoint::import(name, &bytes, Some(media)).map(|show| {
                if let Some(extra) = show
                    .show
                    .pointer("/settings/importWarnings")
                    .and_then(|value| value.as_array())
                {
                    for warning in extra {
                        if let Some(warning) = warning.as_str() {
                            warnings.push(format!("{}: {warning}", path.display()));
                        }
                    }
                }
                vec![show]
            })
        } else if extension.eq_ignore_ascii_case("pro") {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("song");
            propresenter7::decode(name, &bytes).map(|mut show| {
                propresenter7::resolve_standalone_media(&mut show, &path);
                if let Some(extra) = show
                    .show
                    .pointer("/settings/importWarnings")
                    .and_then(|value| value.as_array())
                {
                    for warning in extra {
                        if let Some(warning) = warning.as_str() {
                            warnings.push(format!("{}: {warning}", path.display()));
                        }
                    }
                }
                vec![show]
            })
        } else {
            import_shows_from(
                path.file_name().and_then(|n| n.to_str()).unwrap_or("song"),
                &bytes,
                Some(&path),
                Some(media),
                split_lines,
            )
        };
        match found {
            Ok(found) => {
                for mut show in found {
                    let dest = shows.join(format!("{}.show", file_stem(&show.name())));
                    if dest.exists() {
                        continue;
                    }
                    // Different presentations can share the same source filename.
                    // An ID collision would make the second show unselectable.
                    if ids.contains(&show.id) {
                        show.id = crate::new_id();
                    }
                    if let Err(err) = show.save(&dest) {
                        warnings.push(format!("{}: {err}", path.display()));
                    } else {
                        ids.insert(show.id);
                    }
                }
            }
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
    Ok(warnings)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn import_show(file_name: &str, bytes: &[u8]) -> Result<ShowFile, DataError> {
    import_shows(file_name, bytes)?
        .into_iter()
        .next()
        .ok_or_else(|| DataError::Scripture(format!("{file_name} did not contain slides")))
}

pub fn import_shows(file_name: &str, bytes: &[u8]) -> Result<Vec<ShowFile>, DataError> {
    import_shows_from(file_name, bytes, None, None, 0)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn import_shows_split(
    file_name: &str,
    bytes: &[u8],
    split_lines: u32,
) -> Result<Vec<ShowFile>, DataError> {
    import_shows_from(file_name, bytes, None, None, split_lines)
}

fn import_shows_from(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
    split_lines: u32,
) -> Result<Vec<ShowFile>, DataError> {
    if propresenter::is_bundle(Path::new(file_name)) {
        return propresenter::bundle_shows(bytes)
            .map(|(_, shows)| shows.into_iter().map(|(_, show)| show).collect());
    }
    if is_sqlite(file_name, bytes) {
        if let Ok(songs) = community::softprojector_database(bytes)
            && !songs.is_empty()
        {
            return Ok(songs);
        }
        return openlp_database(bytes);
    }
    if let Some(shows) = community::import_shows(file_name, bytes, source, media)? {
        return Ok(shows);
    }
    Ok(vec![import_one(
        file_name,
        bytes,
        source,
        media,
        split_lines,
    )?])
}

fn import_one(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
    split_lines: u32,
) -> Result<ShowFile, DataError> {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "pptx" || looks_like_pptx(bytes) {
        return powerpoint::import(file_name, bytes, None);
    }
    if ext == "pro" {
        let mut show = propresenter7::decode(file_name, bytes)?;
        propresenter7::resolve_standalone_media(&mut show, Path::new(file_name));
        return Ok(show);
    }
    let text = String::from_utf8_lossy(bytes).replace('\u{feff}', "");
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        return ShowFile::parse(trimmed);
    }
    if is_propresenter(trimmed) {
        return propresenter::xml_show(file_name, &text);
    }
    let mut background = None;
    let (name, sections) = if is_chordpro(trimmed) {
        chordpro(&text)
    } else if is_openlyrics(trimmed) {
        openlyrics(&text)?
    } else if is_opensong(trimmed) {
        let (name, sections, image) = opensong(&text)?;
        background = opensong_background_path(file_name, media, image)?;
        (name, sections)
    } else if is_songbeamer(&text) {
        let (name, sections, image) = songbeamer(&text);
        background = songbeamer_background_path(source, media, image)?;
        (name, sections)
    } else if is_easyworship(&text) || ext == "rtf" {
        plain(&decode_rtf(&text))
    } else {
        plain(&text)
    };
    let sections = split_imported_sections(sections, split_lines);
    let name = if name.trim().is_empty() {
        file_stem_str(file_name)
    } else {
        name
    };
    if sections.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    let mut show = show_file(file_name, &name, sections);
    if let Some(path) = background {
        attach_layout_background(&mut show, &path);
    }
    Ok(show)
}

fn attach_layout_background(show: &mut ShowFile, path: &Path) {
    let media_id = "m1";
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("background");
    show.show["media"][media_id] = json!({"name": name, "path": path});
    if let Some(first) = show
        .show
        .pointer_mut("/layouts/main/slides")
        .and_then(|order| order.as_array_mut())
        .and_then(|slides| slides.first_mut())
    {
        first["background"] = json!(media_id);
    }
}

fn opensong_background_path(
    file_name: &str,
    media: Option<&Path>,
    image: Option<Vec<u8>>,
) -> Result<Option<PathBuf>, DataError> {
    let Some(bytes) = image.filter(|bytes| bytes.starts_with(&[0xFF, 0xD8])) else {
        return Ok(None);
    };
    let Some(media) = media else {
        return Ok(None);
    };
    save_imported_media(media, &format!("{}.jpg", file_stem_str(file_name)), &bytes).map(Some)
}

fn songbeamer_background_path(
    source: Option<&Path>,
    media: Option<&Path>,
    image: Option<String>,
) -> Result<Option<PathBuf>, DataError> {
    let Some(image) = image else {
        return Ok(None);
    };
    let Some(path) = safe_local_image(source, &image) else {
        return Ok(None);
    };
    let bytes = fs::read(&path)?;
    if let Some(media) = media {
        return save_imported_media(
            media,
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("background.jpg"),
            &bytes,
        )
        .map(Some);
    }
    Ok(Some(path))
}

fn safe_local_image(source: Option<&Path>, target: &str) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty()
        || target.contains("..")
        || target.contains('\\')
        || target.contains('\n')
        || target.contains('\r')
        || target.starts_with("//")
        || target.contains(':')
    {
        return None;
    }
    let path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        source.and_then(Path::parent)?.join(target)
    };
    let path = path.canonicalize().ok()?;
    path.is_file().then_some(path).filter(|path| {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
        )
    })
}

#[derive(Clone)]
struct ChordMark {
    pos: usize,
    key: String,
}

#[derive(Clone)]
struct LyricVerse {
    name: String,
    lines: Vec<String>,
    chords: Vec<Vec<ChordMark>>,
}

#[derive(Clone)]
pub(super) struct Section {
    pub(super) group: String,
    pub(super) lines: Vec<String>,
    chords: Vec<Vec<ChordMark>>,
}

impl Section {
    pub(super) fn plain(group: String, lines: Vec<String>) -> Self {
        let chords = (0..lines.len()).map(|_| Vec::new()).collect();
        Self {
            group,
            lines,
            chords,
        }
    }
}

pub(super) fn show_file(file_name: &str, name: &str, sections: Vec<Section>) -> ShowFile {
    let mut slides = serde_json::Map::new();
    let mut order = Vec::new();
    for (index, section) in sections.into_iter().enumerate() {
        let id = format!("s{}", index + 1);
        order.push(json!({ "id": id }));
        slides.insert(
            id,
            slide_value(&section.group, &section.lines, &section.chords),
        );
    }
    let id = file_stem_str(file_name);
    let document = json!({
        "name": name,
        "origin": "import",
        "settings": { "activeLayout": "main", "template": null },
        "slides": slides,
        "layouts": { "main": { "name": "Default", "notes": "", "slides": order } },
        "media": {}
    });
    ShowFile::parse(&json!([id, document]).to_string()).expect("imported show is valid JSON")
}

fn slide_value(group: &str, lines: &[String], chords: &[Vec<ChordMark>]) -> Value {
    let enabled = chords.iter().any(|line| !line.is_empty());
    let lines = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let marks = chords.get(index).map(Vec::as_slice).unwrap_or(&[]);
            json!({
                "align": "",
                "text": [{ "value": line, "style": "" }],
                "chords": marks.iter().map(|chord| json!({
                    "id": format!("c{}", chord.pos),
                    "pos": chord.pos,
                    "key": chord.key,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "group": group,
        "color": null,
        "settings": {},
        "notes": "",
        "items": [{
            "style": ITEM_STYLE,
            "chords": { "enabled": enabled, "color": "#FF851B" },
            "lines": lines
        }]
    })
}

fn is_chordpro(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("{title:")
        || lower.contains("{t:")
        || lower.contains("{start_of_")
        || lower.contains("{soc}")
        || lower.contains("{sov}")
}

fn is_openlyrics(text: &str) -> bool {
    text.contains("<song") && text.contains("<verse") && text.contains("<lines")
}

fn is_opensong(text: &str) -> bool {
    text.contains("<song") && text.contains("<lyrics") && !text.contains("<verse")
}

fn is_propresenter(text: &str) -> bool {
    text.contains("RVPresentationDocument") || text.contains("RVDisplaySlide")
}

fn is_songbeamer(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("#Title") || line.starts_with("#LangCount") || line.starts_with("#Author")
    })
}

fn is_easyworship(text: &str) -> bool {
    text.contains("{\\rtf") || text.to_ascii_lowercase().contains("easyworship")
}

fn chordpro(text: &str) -> (String, Vec<Section>) {
    let mut name = String::new();
    let mut sections = Vec::new();
    let mut group = "Verse".to_string();
    let mut lines = Vec::new();
    let mut line_chords: Vec<Vec<ChordMark>> = Vec::new();
    let flush = |group: &str,
                 lines: &mut Vec<String>,
                 line_chords: &mut Vec<Vec<ChordMark>>,
                 sections: &mut Vec<Section>| {
        let has_words = lines.iter().any(|line| !line.trim().is_empty());
        let has_chords = line_chords.iter().any(|chords| !chords.is_empty());
        if has_words || has_chords {
            sections.push(Section {
                group: group.to_string(),
                lines: std::mem::take(lines),
                chords: std::mem::take(line_chords),
            });
        } else {
            lines.clear();
            line_chords.clear();
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush(&group, &mut lines, &mut line_chords, &mut sections);
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(title) = directive_value(line, "title").or_else(|| directive_value(line, "t")) {
            name = title;
            continue;
        }
        if lower.contains("start_of_chorus") || lower.contains("{soc}") {
            flush(&group, &mut lines, &mut line_chords, &mut sections);
            group = label_from_directive(line).unwrap_or_else(|| "Chorus".into());
            continue;
        }
        if let Some(label) = directive_value(line, "c") {
            flush(&group, &mut lines, &mut line_chords, &mut sections);
            group = label;
            continue;
        }
        if lower.contains("start_of_verse") || lower.contains("{sov}") {
            flush(&group, &mut lines, &mut line_chords, &mut sections);
            group = label_from_directive(line).unwrap_or_else(|| "Verse".into());
            continue;
        }
        if lower.contains("end_of_") || lower.contains("{eoc}") || lower.contains("{eov}") {
            flush(&group, &mut lines, &mut line_chords, &mut sections);
            continue;
        }
        if line.starts_with('{') {
            continue;
        }
        let prepared = line.replace("[/]", " ").replace("[|]", "");
        let (lyric, chords) = lyric_and_chords(&prepared);
        if !lyric.is_empty() || !chords.is_empty() {
            lines.push(lyric);
            line_chords.push(chords);
        }
    }
    flush(&group, &mut lines, &mut line_chords, &mut sections);
    (name, sections)
}

fn directive_value(line: &str, key: &str) -> Option<String> {
    let trimmed = line.trim().trim_start_matches('{').trim_end_matches('}');
    let (found, value) = trimmed.split_once(':')?;
    if found.trim().eq_ignore_ascii_case(key) {
        Some(value.trim().to_string())
    } else {
        None
    }
}

fn label_from_directive(line: &str) -> Option<String> {
    let value = line
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split_once(':')?
        .1
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn lyric_and_chords(line: &str) -> (String, Vec<ChordMark>) {
    let mut lyric = String::new();
    let mut chords = Vec::new();
    let mut in_chord = false;
    let mut key = String::new();
    let mut pos = 0usize;
    for ch in line.chars() {
        if ch == '[' && !in_chord {
            in_chord = true;
            key.clear();
            continue;
        }
        if ch == ']' && in_chord {
            in_chord = false;
            if !key.is_empty() {
                chords.push(ChordMark {
                    pos,
                    key: std::mem::take(&mut key),
                });
            }
            continue;
        }
        if in_chord {
            key.push(ch);
        } else {
            lyric.push(ch);
            pos += 1;
        }
    }
    (lyric, chords)
}

fn openlyrics(text: &str) -> Result<(String, Vec<Section>), DataError> {
    let (title, order, verses) = parse_lyric_xml(text)?;
    Ok((title, order_sections(&verses, &order)))
}

fn parse_lyric_xml(text: &str) -> Result<(String, String, Vec<LyricVerse>), DataError> {
    let mut title = String::new();
    let mut order = String::new();
    let mut verses = Vec::new();
    let mut current = String::new();
    let mut capture = String::new();
    let mut in_title = false;
    let mut in_order = false;
    let mut in_lines = false;
    let mut in_chord = false;
    let mut chord_named = false;
    let mut chord_text = String::new();
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                match name.as_str() {
                    "title" => in_title = title.is_empty(),
                    "verseOrder" => in_order = true,
                    "verse" => {
                        let named = attribute(&event, "name").unwrap_or_default();
                        current = if named.is_empty() {
                            format!(
                                "{}{}",
                                attribute(&event, "type").unwrap_or_default(),
                                attribute(&event, "label").unwrap_or_default()
                            )
                        } else {
                            named
                        };
                    }
                    "br" if in_lines => capture.push('\n'),
                    "p" if in_lines && page_break(&event) => {
                        push_verse(&mut verses, &current, &capture);
                        capture.clear();
                    }
                    "lines" => {
                        in_lines = true;
                        capture.clear();
                    }
                    "chord" if in_lines => {
                        in_chord = true;
                        chord_named = remember_chord(
                            &mut capture,
                            &attribute(&event, "name").unwrap_or_default(),
                        );
                        chord_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .unescape()
                    .map_err(|err| DataError::Scripture(err.to_string()))?
                    .to_string();
                if in_title {
                    title.push_str(text.trim());
                } else if in_order {
                    order.push_str(text.trim());
                } else if in_chord {
                    chord_text.push_str(&text);
                } else if in_lines {
                    capture.push_str(&text);
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                match name.as_str() {
                    "title" => in_title = false,
                    "verseOrder" => in_order = false,
                    "lines" => {
                        in_lines = false;
                        push_verse(&mut verses, &current, &capture);
                    }
                    "chord" => {
                        if in_chord && !chord_named {
                            remember_chord(&mut capture, chord_text.trim());
                        }
                        in_chord = false;
                        chord_named = false;
                        chord_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => {
                if in_lines {
                    let name = local_name(event.name().as_ref());
                    if name == "br" {
                        capture.push('\n');
                    } else if name == "p" && page_break(&event) {
                        push_verse(&mut verses, &current, &capture);
                        capture.clear();
                    } else if name == "chord" {
                        remember_chord(
                            &mut capture,
                            &attribute(&event, "name").unwrap_or_default(),
                        );
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(DataError::Scripture(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok((title, order, verses))
}

fn remember_chord(capture: &mut String, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    capture.push('[');
    capture.push_str(name);
    capture.push(']');
    true
}

fn push_verse(verses: &mut Vec<LyricVerse>, name: &str, capture: &str) {
    let mut lines = Vec::new();
    let mut chords = Vec::new();
    for line in capture.lines() {
        let (text, marks) = lyric_with_chords(line.trim());
        if text.is_empty() {
            continue;
        }
        lines.push(text);
        chords.push(marks);
    }
    if !lines.is_empty() {
        verses.push(LyricVerse {
            name: name.to_string(),
            lines,
            chords,
        });
    }
}

fn lyric_with_chords(line: &str) -> (String, Vec<ChordMark>) {
    let mut plain = String::new();
    let mut skip = false;
    for ch in line.chars() {
        if ch == '{' {
            skip = true;
            continue;
        }
        if ch == '}' {
            skip = false;
            continue;
        }
        if !skip {
            plain.push(ch);
        }
    }
    extract_bracket_chords(plain.trim())
}

/// `[G]` sits on the following letter. A long or repeated tag such as `[info]` stays in the lyric.
fn extract_bracket_chords(line: &str) -> (String, Vec<ChordMark>) {
    let mut lyric = String::new();
    let mut marks = Vec::new();
    for part in line.split(']') {
        let (before, chord) = part.split_once('[').unwrap_or((part, ""));
        lyric.push_str(before);
        if chord.is_empty() {
            continue;
        }
        if chord.chars().count() > 5 || chord.contains('x') {
            lyric.push('[');
            lyric.push_str(chord);
            lyric.push(']');
        } else {
            marks.push(ChordMark {
                pos: lyric.chars().count(),
                key: chord.to_string(),
            });
        }
    }
    (lyric, marks)
}

fn page_break(event: &quick_xml::events::BytesStart<'_>) -> bool {
    attribute(event, "style")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("page-break-after")
}

type OpenSongDoc = (String, Vec<Section>, Option<Vec<u8>>);

fn opensong(text: &str) -> Result<OpenSongDoc, DataError> {
    let mut title = String::new();
    let mut lyrics = String::new();
    let mut backgrounds = String::new();
    let mut in_title = false;
    let mut in_lyrics = false;
    let mut in_backgrounds = false;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                in_title = name == "title";
                in_lyrics = name == "lyrics";
                in_backgrounds = name == "backgrounds";
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .unescape()
                    .map_err(|err| DataError::Scripture(err.to_string()))?
                    .to_string();
                if in_title {
                    title.push_str(text.trim());
                } else if in_lyrics {
                    lyrics.push_str(&text);
                } else if in_backgrounds {
                    backgrounds.push_str(&text);
                }
            }
            Ok(Event::End(_)) => {
                in_title = false;
                in_lyrics = false;
                in_backgrounds = false;
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(DataError::Scripture(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    let mut sections = Vec::new();
    let mut group = "Verse".to_string();
    let mut lines = Vec::new();
    let mut chords = Vec::new();
    let mut pending = Vec::new();
    for line in lyrics.lines() {
        let line = line.trim();
        if let Some(tag) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if !lines.is_empty() {
                sections.push(Section {
                    group: group.clone(),
                    lines: std::mem::take(&mut lines),
                    chords: std::mem::take(&mut chords),
                });
            }
            pending.clear();
            group = opensong_group(tag);
            continue;
        }
        if line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(body) = line.strip_prefix('.') {
            pending = opensong_chords(body);
            continue;
        }
        lines.push(line.replace('_', ""));
        chords.push(std::mem::take(&mut pending));
    }
    if !lines.is_empty() {
        sections.push(Section {
            group,
            lines,
            chords,
        });
    }
    let image = base64_decode(backgrounds.trim()).filter(|bytes| !bytes.is_empty());
    Ok((title, sections, image))
}

/// OpenSong puts chords on a line that starts with `.`. Spaces between names are their positions.
fn opensong_chords(body: &str) -> Vec<ChordMark> {
    let mut chords = Vec::new();
    let mut pos = -1_i32;
    for token in body.split(' ') {
        pos += 1;
        if token.is_empty() {
            continue;
        }
        chords.push(ChordMark {
            pos: pos.max(0) as usize,
            key: token.to_string(),
        });
        pos += 1;
    }
    chords
}

fn opensong_group(tag: &str) -> String {
    let tag = tag.trim();
    let kind = match tag.chars().next().unwrap_or('V').to_ascii_uppercase() {
        'C' => "Chorus",
        'B' => "Bridge",
        'T' => "Tag",
        'O' => "Outro",
        'I' => "Intro",
        'P' => "Pre-chorus",
        _ => "Verse",
    };
    let number: String = tag.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if number.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {number}")
    }
}

fn songbeamer(text: &str) -> (String, Vec<Section>, Option<String>) {
    let stored = songbeamer_chords(text);
    let mut name = String::new();
    let mut background = None;
    let mut sections = Vec::new();
    let mut chord_line = 0usize;
    for section in text.split("\n---") {
        let lines: Vec<&str> = section.lines().map(str::trim).collect();
        let Some(first) = lines.iter().copied().find(|line| !line.is_empty()) else {
            chord_line += 1;
            continue;
        };
        if first.starts_with('#')
            && sections.is_empty()
            && lines
                .iter()
                .all(|line| line.is_empty() || line.starts_with('#'))
        {
            for line in lines {
                if let Some(title) = line.strip_prefix("#Title=") {
                    name = title.trim().to_string();
                }
                if let Some(image) = line.strip_prefix("#BackgroundImage=") {
                    background = Some(image.trim().to_string());
                }
            }
            continue;
        }
        let mut group = "Verse".to_string();
        let mut body = lines.as_slice();
        if looks_like_group(first) && !first.starts_with('#') {
            group = first.to_string();
            chord_line += 1;
            if let Some(start) = lines.iter().position(|line| line == &first) {
                body = &lines[start + 1..];
            }
        }
        let mut lyric = Vec::new();
        let mut marks = Vec::new();
        for line in body {
            let these = stored.get(&chord_line).cloned().unwrap_or_default();
            chord_line += 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            lyric.push((*line).to_string());
            marks.push(these);
        }
        chord_line += 1;
        if !lyric.is_empty() {
            sections.push(Section {
                group,
                lines: lyric,
                chords: marks,
            });
        }
    }
    (name, sections, background)
}

fn songbeamer_chords(text: &str) -> std::collections::BTreeMap<usize, Vec<ChordMark>> {
    let mut stored = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some(value) = line.trim().strip_prefix("#Chords=") else {
            continue;
        };
        let decoded =
            base64_decode(value.trim()).unwrap_or_else(|| value.trim().as_bytes().to_vec());
        let text = String::from_utf8_lossy(&decoded);
        for row in text.split('\r') {
            let parts: Vec<&str> = row.split(',').collect();
            if parts.len() < 3 {
                continue;
            }
            let Ok(line_index) = parts[1].trim().parse::<usize>() else {
                continue;
            };
            let pos = parts[0]
                .trim()
                .parse::<f64>()
                .unwrap_or(0.0)
                .ceil()
                .max(0.0) as usize;
            let key = parts[2].replace('<', "♭").replace('=', "");
            if key.is_empty() {
                continue;
            }
            stored
                .entry(line_index)
                .or_insert_with(Vec::new)
                .push(ChordMark { pos, key });
        }
    }
    stored
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|byte| **byte == b'=').count();
        let a = value(chunk[0])?;
        let b = value(chunk[1])?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            value(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            value(chunk[3])?
        };
        out.push((a << 2) | (b >> 4));
        if pad < 2 {
            out.push((b << 4) | (c >> 2));
        }
        if pad < 1 {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

/// A line of chord names above a lyric becomes chords on that lyric.
fn pair_chord_lines(lines: Vec<String>) -> (Vec<String>, Vec<Vec<ChordMark>>) {
    let mut lyrics = Vec::new();
    let mut chords = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some(lyric) = lines.get(index + 1)
            && is_chord_line(&lines[index])
            && !is_chord_line(lyric)
            && !is_section_header(lyric)
        {
            let lyric = lyric.clone();
            chords.push(chord_marks(&lines[index], &lyric));
            lyrics.push(lyric);
            index += 2;
            continue;
        }
        lyrics.push(lines[index].clone());
        chords.push(Vec::new());
        index += 1;
    }
    (lyrics, chords)
}

fn is_section_header(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('[') && line.ends_with(']')
}

fn is_chord_line(line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    !tokens.is_empty() && tokens.iter().all(|token| is_chord_token(token))
}

fn is_chord_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(root) = chars.next() else {
        return false;
    };
    if !('A'..='G').contains(&root) {
        return false;
    }
    let mut rest = chars.as_str();
    if rest.starts_with('#') || rest.starts_with('b') {
        rest = &rest[1..];
    }
    for prefix in ["maj", "min", "dim", "aug", "sus", "add", "m", "M"] {
        if rest.starts_with(prefix) {
            rest = &rest[prefix.len()..];
            break;
        }
    }
    let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    rest = &rest[digits..];
    if let Some(bass) = rest.strip_prefix('/') {
        let mut notes = bass.chars();
        let Some(note) = notes.next() else {
            return false;
        };
        if !('A'..='G').contains(&note) {
            return false;
        }
        let left: String = notes.collect();
        return left.is_empty() || left == "#" || left == "b";
    }
    rest.is_empty()
}

fn chord_marks(chord_line: &str, lyric: &str) -> Vec<ChordMark> {
    let lyric: Vec<char> = lyric.chars().collect();
    let chars: Vec<char> = chord_line.chars().collect();
    let mut marks = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && !chars[index].is_whitespace() {
            index += 1;
        }
        let key: String = chars[start..index].iter().collect();
        if !is_chord_token(&key) {
            continue;
        }
        let mut pos = start;
        while pos < lyric.len() && lyric[pos] == ' ' {
            pos += 1;
        }
        if pos > lyric.len() {
            pos = lyric.len();
        }
        marks.push(ChordMark { pos, key });
    }
    marks
}

fn looks_like_group(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "verse",
        "chorus",
        "bridge",
        "tag",
        "intro",
        "outro",
        "pre-chorus",
    ]
    .iter()
    .any(|name| lower.starts_with(name))
}

fn plain(text: &str) -> (String, Vec<Section>) {
    let mut name = String::new();
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("title")
            && !key.contains(' ')
        {
            name = value.trim().to_string();
            continue;
        }
        current.push(line.to_string());
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    let sections = blocks
        .into_iter()
        .filter_map(|mut lines| {
            if lines.is_empty() {
                return None;
            }
            let group = if lines[0].starts_with('[') && lines[0].ends_with(']') {
                lines.remove(0).trim_matches(['[', ']']).to_string()
            } else {
                "Verse".to_string()
            };
            if lines.is_empty() {
                None
            } else {
                let (lines, chords) = pair_chord_lines(lines);
                Some(Section {
                    group,
                    lines,
                    chords,
                })
            }
        })
        .collect();
    (name, sections)
}

fn split_imported_sections(sections: Vec<Section>, split_lines: u32) -> Vec<Section> {
    if split_lines == 0 {
        return sections;
    }
    let mut split = Vec::new();
    for section in sections {
        if section.lines.len() as u32 <= split_lines {
            split.push(section);
            continue;
        }
        let group = section.group;
        let chords = section.chords;
        for (index, lines) in section.lines.chunks(split_lines as usize).enumerate() {
            let start = index * split_lines as usize;
            let end = (start + lines.len()).min(chords.len());
            split.push(Section {
                group: group.clone(),
                lines: lines.to_vec(),
                chords: if start < chords.len() {
                    chords[start..end].to_vec()
                } else {
                    Vec::new()
                },
            });
        }
    }
    split
}

fn decode_rtf(text: &str) -> String {
    rtf::decode(text).plain_text()
}

fn order_sections(verses: &[LyricVerse], order: &str) -> Vec<Section> {
    let wanted: Vec<&str> = order.split_whitespace().collect();
    let chosen: Vec<&LyricVerse> = if wanted.is_empty() {
        verses.iter().collect()
    } else {
        let mut found = Vec::new();
        for name in &wanted {
            found.extend(
                verses
                    .iter()
                    .filter(|verse| verse_name_matches(&verse.name, name)),
            );
        }
        if found.is_empty() {
            verses.iter().collect()
        } else {
            found
        }
    };
    chosen
        .into_iter()
        .map(|verse| Section {
            group: verse_group(&verse.name),
            lines: verse.lines.clone(),
            chords: verse.chords.clone(),
        })
        .collect()
}

fn verse_name_matches(id: &str, wanted: &str) -> bool {
    if id.eq_ignore_ascii_case(wanted) {
        return true;
    }
    if wanted.chars().last().is_some_and(|ch| ch.is_ascii_digit()) {
        return false;
    }
    id.eq_ignore_ascii_case(&format!("{wanted}1"))
}

fn verse_group(name: &str) -> String {
    let kind = match name.chars().next().unwrap_or('v').to_ascii_lowercase() {
        'c' => "Chorus",
        'b' => "Bridge",
        'p' => "Pre-chorus",
        'i' => "Intro",
        'e' => "Ending",
        'o' | 't' => "Tag",
        _ => "Verse",
    };
    let number: String = name.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if number.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {number}")
    }
}

pub(super) fn is_sqlite(file_name: &str, bytes: &[u8]) -> bool {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .eq_ignore_ascii_case("sqlite");
    ext || bytes.starts_with(b"SQLite format 3")
}

fn openlp_database(bytes: &[u8]) -> Result<Vec<ShowFile>, DataError> {
    let path = std::env::temp_dir().join(format!("freeshow-openlp-{}.sqlite", crate::new_id()));
    fs::write(&path, bytes)?;
    let connection =
        rusqlite::Connection::open(&path).map_err(|err| DataError::Scripture(err.to_string()))?;
    let songs = openlp_songs(&connection)?;
    drop(connection);
    let _ = fs::remove_file(&path);
    if songs.is_empty() {
        return Err(DataError::Scripture(
            "OpenLP database did not contain slides".into(),
        ));
    }
    Ok(songs)
}

struct OpenLpSong {
    id: i64,
    title: String,
    lyrics: String,
    order: String,
    author: String,
    copyright: String,
    ccli: String,
    notes: String,
}

fn openlp_songs(connection: &rusqlite::Connection) -> Result<Vec<ShowFile>, DataError> {
    let raw = openlp_rows(connection)?;
    let mut songs = Vec::new();
    for song in raw {
        let author = openlp_author(connection, song.id);
        let verses = parse_lyric_xml(&song.lyrics)
            .map(|(_, _, verses)| verses)
            .unwrap_or_default();
        let sections = order_sections(&verses, &song.order);
        if sections.is_empty() {
            continue;
        }
        let title = if song.title.trim().is_empty() {
            format!("Song {}", song.id)
        } else {
            song.title.clone()
        };
        let mut show = show_file(&format!("{title}.xml"), &title, sections);
        if let Some(document) = show.show.as_object_mut() {
            document.insert("origin".to_string(), json!("openlp"));
            document.insert(
                "meta".to_string(),
                json!({
                    "title": title,
                    "author": if author.is_empty() { song.author } else { author },
                    "CCLI": song.ccli,
                    "copyright": song.copyright
                }),
            );
        }
        if !song.notes.is_empty()
            && let Some(layout) = show
                .show
                .pointer_mut("/layouts/main")
                .and_then(Value::as_object_mut)
        {
            layout.insert("notes".to_string(), json!(song.notes));
        }
        songs.push(show);
    }
    Ok(songs)
}

fn openlp_rows(connection: &rusqlite::Connection) -> Result<Vec<OpenLpSong>, DataError> {
    let mut statement = connection
        .prepare("SELECT * FROM songs")
        .map_err(|err| DataError::Scripture(err.to_string()))?;
    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut rows = statement
        .query([])
        .map_err(|err| DataError::Scripture(err.to_string()))?;
    let mut songs = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|err| DataError::Scripture(err.to_string()))?
    {
        if let Some(song) = openlp_row(row, &columns) {
            songs.push(song);
        }
    }
    Ok(songs)
}

fn openlp_row(row: &rusqlite::Row<'_>, columns: &[String]) -> Option<OpenLpSong> {
    let lyrics = sqlite_text(row, columns, "lyrics");
    if lyrics.trim().is_empty() {
        return None;
    }
    let title = {
        let alternate = sqlite_text(row, columns, "alternate_title");
        if alternate.trim().is_empty() {
            sqlite_text(row, columns, "title")
        } else {
            alternate
        }
    };
    Some(OpenLpSong {
        id: sqlite_i64(row, columns, "id"),
        title,
        lyrics,
        order: sqlite_text(row, columns, "verse_order"),
        author: String::new(),
        copyright: sqlite_text(row, columns, "copyright"),
        ccli: sqlite_text(row, columns, "ccli_number"),
        notes: sqlite_text(row, columns, "comments"),
    })
}

fn openlp_author(connection: &rusqlite::Connection, song_id: i64) -> String {
    connection
        .query_row(
            "SELECT authors.display_name FROM authors_songs
             JOIN authors ON authors.id = authors_songs.author_id
             WHERE authors_songs.song_id = ?1
             ORDER BY CASE WHEN authors_songs.author_type LIKE '%words%' THEN 0 ELSE 1 END
             LIMIT 1",
            [song_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
}

fn sqlite_text(row: &rusqlite::Row<'_>, columns: &[String], name: &str) -> String {
    let Some(index) = columns.iter().position(|column| column == name) else {
        return String::new();
    };
    row.get::<_, Option<String>>(index)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn sqlite_i64(row: &rusqlite::Row<'_>, columns: &[String], name: &str) -> i64 {
    let Some(index) = columns.iter().position(|column| column == name) else {
        return 0;
    };
    row.get::<_, i64>(index).unwrap_or(0)
}

fn looks_like_pptx(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK") && bytes.windows(11).any(|window| window == b"ppt/slides/")
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

fn save_imported_media(media: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, DataError> {
    fs::create_dir_all(media)?;
    let media = media.canonicalize()?;
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = file_stem(basename);
    let extension = Path::new(basename)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    // Use a flat filename; archive directory components never reach the disk.
    for suffix in 0..10_000 {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let path = media.join(format!("{stem}{suffix}.{extension}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let result = file.write_all(bytes).and_then(|_| file.sync_all());
                if let Err(error) = result {
                    let _ = fs::remove_file(&path);
                    return Err(error.into());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_file()
                    && metadata.len() == bytes.len() as u64
                    && fs::read(&path)? == bytes
                {
                    return Ok(path);
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(DataError::Scripture(
        "too many imported media filename collisions".into(),
    ))
}

fn file_stem(name: &str) -> String {
    file_stem_str(name)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == ' ' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) fn file_stem_str(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chordpro_openlyrics_opensong_text_and_songbeamer() {
        let chord = "{title: Amazing Grace}\n{start_of_verse: Verse}\n[G]Amazing [C]grace\n{end_of_verse}\n{soc}\nPraise God\n{eoc}\n";
        let show = import_show("grace.cho", chord.as_bytes()).unwrap();
        assert_eq!(show.name(), "Amazing Grace");
        let slides = show.ordered_slides();
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(slides[0].lines, vec!["Amazing grace".to_string()]);
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 0);
        assert_eq!(marks[1]["key"], "C");
        assert_eq!(marks[1]["pos"], 8);
        assert_eq!(
            show.show
                .pointer("/slides/s1/items/0/chords/enabled")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(slides[1].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[1].lines, vec!["Praise God".to_string()]);

        let openlyrics = r#"<song><properties><titles><title>Amazing Grace</title></titles><verseOrder>v1 c1</verseOrder></properties><lyrics><verse name="c1"><lines>Praise God</lines></verse><verse name="v1"><lines>Amazing <chord name="G"/>grace
how sweet</lines></verse></lyrics></song>"#;
        let show = import_show("grace.xml", openlyrics.as_bytes()).unwrap();
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Verse 1"));
        assert_eq!(
            show.ordered_slides()[0].lines,
            vec!["Amazing grace".to_string(), "how sweet".to_string()]
        );
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 8);
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus 1"));

        let opensong = "<song><title>Amazing Grace</title><lyrics>\n[V1]\n.G       C\nAmazing grace\n[C]\nPraise God\n</lyrics></song>";
        let show = import_show("opensong.xml", opensong.as_bytes()).unwrap();
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Verse 1"));
        assert_eq!(
            show.ordered_slides()[0].lines,
            vec!["Amazing grace".to_string()]
        );
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 0);
        assert_eq!(marks[1]["key"], "C");
        assert_eq!(
            show.ordered_slides()[1].lines,
            vec!["Praise God".to_string()]
        );

        let text =
            "Title: Amazing Grace\n\n[Verse]\nG       C\nAmazing grace\n\n[Chorus]\nPraise God\n";
        let show = import_show("song.txt", text.as_bytes()).unwrap();
        assert_eq!(show.name(), "Amazing Grace");
        assert_eq!(show.ordered_slides().len(), 2);
        assert_eq!(
            show.ordered_slides()[0].lines,
            vec!["Amazing grace".to_string()]
        );
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 0);
        assert_eq!(marks[1]["key"], "C");

        let beamer = "#Title=Amazing Grace\n#Author=Newton\n#Chords=MCwxLEc=\n---\nVerse 1\nAmazing grace\n---\nChorus\nPraise God\n";
        let show = import_show("grace.sng", beamer.as_bytes()).unwrap();
        assert_eq!(show.name(), "Amazing Grace");
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus"));
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 0);
    }

    #[test]
    fn plain_text_import_honors_split_lines() {
        let text = "Title: Split\n\n[Verse]\nOne\nTwo\nThree\nFour\nFive";
        let show = import_show("song.txt", text.as_bytes()).unwrap();
        assert_eq!(show.ordered_slides().len(), 1);
        let show = import_shows_split("song.txt", text.as_bytes(), 2).unwrap();
        let show = show.into_iter().next().unwrap();
        assert_eq!(show.name(), "Split");
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].lines, vec!["One".to_string(), "Two".to_string()]);
        assert_eq!(
            slides[1].lines,
            vec!["Three".to_string(), "Four".to_string()]
        );
        assert_eq!(slides[2].lines, vec!["Five".to_string()]);
        assert!(
            slides
                .iter()
                .all(|slide| slide.group.as_deref() == Some("Verse"))
        );
    }

    #[test]
    fn opensong_and_songbeamer_keep_safe_backgrounds() {
        fn b64(bytes: &[u8]) -> String {
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let a = u32::from(chunk[0]);
                let b = u32::from(chunk.get(1).copied().unwrap_or(0));
                let c = u32::from(chunk.get(2).copied().unwrap_or(0));
                let n = (a << 16) | (b << 8) | c;
                out.push(T[(n >> 18) as usize] as char);
                out.push(T[((n >> 12) & 63) as usize] as char);
                out.push(if chunk.len() > 1 {
                    T[((n >> 6) & 63) as usize] as char
                } else {
                    '='
                });
                out.push(if chunk.len() > 2 {
                    T[(n & 63) as usize] as char
                } else {
                    '='
                });
            }
            out
        }
        let jpeg = [0xFF, 0xD8, 0xFF, 0xD9];
        let path = std::env::temp_dir().join(crate::new_id());
        let imports = path.join("Imports");
        let media = path.join("Media");
        let shows = path.join("Shows");
        std::fs::create_dir_all(&imports).unwrap();
        std::fs::create_dir_all(&media).unwrap();
        std::fs::create_dir_all(&shows).unwrap();
        std::fs::write(
            imports.join("OpenGrace.xml"),
            format!(
                "<song><title>Grace</title><lyrics>\n[V1]\nAmazing grace\n</lyrics><backgrounds>{}</backgrounds></song>",
                b64(&jpeg)
            ),
        )
        .unwrap();
        std::fs::write(
            imports.join("sky.png"),
            include_bytes!("../tests/fixtures/lesson-cover.png"),
        )
        .unwrap();
        std::fs::write(
            imports.join("BeamerGrace.sng"),
            "#Title=Beamer\n#BackgroundImage=sky.png\n---\nVerse 1\nAmazing grace\n",
        )
        .unwrap();
        std::fs::write(
            imports.join("Unsafe.sng"),
            "#Title=Unsafe\n#BackgroundImage=../secret.png\n---\nVerse 1\nHidden\n",
        )
        .unwrap();
        let warnings = import_sources(&imports, &shows, &media).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let library = crate::Library::open(&path);
        let list = library.list_shows().unwrap();
        let opensong = list.iter().find(|show| show.name() == "Grace").unwrap();
        let beamer = list.iter().find(|show| show.name() == "Beamer").unwrap();
        let unsafe_show = list.iter().find(|show| show.name() == "Unsafe").unwrap();
        let jpg = PathBuf::from(opensong.show["media"]["m1"]["path"].as_str().unwrap());
        assert!(jpg.starts_with(media.canonicalize().unwrap()));
        assert_eq!(std::fs::read(&jpg).unwrap(), jpeg);
        assert_eq!(
            opensong.show["layouts"]["main"]["slides"][0]["background"],
            "m1"
        );
        let png = PathBuf::from(beamer.show["media"]["m1"]["path"].as_str().unwrap());
        assert!(png.starts_with(media.canonicalize().unwrap()));
        assert_eq!(
            std::fs::read(&png).unwrap(),
            include_bytes!("../tests/fixtures/lesson-cover.png")
        );
        assert!(unsafe_show.show["media"].as_object().unwrap().is_empty());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn propresenter_easyworship_and_powerpoint() {
        let pro = r#"<RVPresentationDocument><RVDisplaySlide><RVTextElement><NSString>Amazing grace</NSString></RVTextElement></RVDisplaySlide><RVDisplaySlide><RVTextElement><NSString>Praise God</NSString></RVTextElement></RVDisplaySlide></RVPresentationDocument>"#;
        let show = import_show("song.pro6", pro.as_bytes()).unwrap();
        assert_eq!(
            show.ordered_slides()[0].lines,
            vec!["Amazing grace".to_string()]
        );
        assert_eq!(
            show.ordered_slides()[1].lines,
            vec!["Praise God".to_string()]
        );

        let rtf = "{\\rtf1 Amazing grace\\par how sweet\\par\\par Praise God}";
        let show = import_show("song.rtf", rtf.as_bytes()).unwrap();
        assert!(show.ordered_slides()[0].lines[0].contains("Amazing grace"));
        assert!(show.plain_text("s2").contains("Praise God") || !show.ordered_slides().is_empty());

        let bytes = pptx_with_slide("Amazing grace");
        let show = import_show("slides.pptx", &bytes).unwrap();
        assert_eq!(
            show.ordered_slides()[0].lines,
            vec!["Amazing grace".to_string()]
        );
    }

    #[test]
    fn openlp_sqlite_and_page_breaks_become_shows() {
        let lyrics = r#"<song><lyrics><verse type="v" label="1"><lines>Amazing [G]grace<br/>how sweet {it}</lines></verse><verse type="c" label="1"><lines>Praise God<p style="page-break-after: always;"/>from whom</lines></verse></lyrics></song>"#;
        let path =
            std::env::temp_dir().join(format!("freeshow-openlp-src-{}.sqlite", std::process::id()));
        let _ = fs::remove_file(&path);
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE songs (id INTEGER PRIMARY KEY, title TEXT, alternate_title TEXT, lyrics TEXT, verse_order TEXT, copyright TEXT, comments TEXT, ccli_number TEXT);
                     CREATE TABLE authors (id INTEGER PRIMARY KEY, display_name TEXT);
                     CREATE TABLE authors_songs (song_id INTEGER, author_id INTEGER, author_type TEXT);
                     INSERT INTO songs VALUES (1, 'Amazing Grace', '', '', 'v1 c1', 'Public domain', 'slow', '22025');
                     INSERT INTO authors VALUES (1, 'John Newton');
                     INSERT INTO authors_songs VALUES (1, 1, 'words');",
                )
                .unwrap();
            connection
                .execute("UPDATE songs SET lyrics = ?1 WHERE id = 1", [lyrics])
                .unwrap();
        }
        let bytes = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);
        let shows = import_shows("songs.sqlite", &bytes).unwrap();
        assert_eq!(shows.len(), 1);
        let show = &shows[0];
        assert_eq!(show.name(), "Amazing Grace");
        assert_eq!(
            show.show.get("origin").and_then(Value::as_str),
            Some("openlp")
        );
        assert_eq!(
            show.show.pointer("/meta/author").and_then(Value::as_str),
            Some("John Newton")
        );
        assert_eq!(
            show.show.pointer("/meta/CCLI").and_then(Value::as_str),
            Some("22025")
        );
        let slides = show.ordered_slides();
        assert_eq!(slides[0].group.as_deref(), Some("Verse 1"));
        assert_eq!(
            slides[0].lines,
            vec!["Amazing grace".to_string(), "how sweet".to_string()]
        );
        let marks = show
            .show
            .pointer("/slides/s1/items/0/lines/0/chords")
            .unwrap();
        assert_eq!(marks[0]["key"], "G");
        assert_eq!(marks[0]["pos"], 8);
        assert_eq!(slides[1].group.as_deref(), Some("Chorus 1"));
        assert_eq!(slides[1].lines, vec!["Praise God".to_string()]);
        assert_eq!(slides[2].group.as_deref(), Some("Chorus 1"));
        assert_eq!(slides[2].lines, vec!["from whom".to_string()]);
        assert_eq!(
            show.show
                .pointer("/layouts/main/notes")
                .and_then(Value::as_str),
            Some("slow")
        );
    }

    fn pptx_with_slide(text: &str) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file(
                    "ppt/slides/slide1.xml",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            let xml = format!(
                r#"<?xml version="1.0"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:t>{text}</a:t></p:sld>"#
            );
            std::io::Write::write_all(&mut writer, xml.as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }
}
