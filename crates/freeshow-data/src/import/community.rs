// SPDX-License-Identifier: GPL-3.0-only

//! MediaShout, Quelea, SoftProjector, EasySlides, VerseView, VideoPsalm,
//! CSV, image-only shows, and FreeShow project packages.

use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};
use zip::ZipArchive;

use super::{Section, file_stem_str, show_file};
use crate::{DataError, ShowFile};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];

pub fn import_shows(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
) -> Result<Option<Vec<ShowFile>>, DataError> {
    let ext = extension(file_name);
    if looks_like_zip(bytes) {
        if ext == "qsp" || ext == "vpc" || ext == "zip" {
            return zip_shows(file_name, bytes, media);
        }
        if (IMAGE_EXTS.iter().any(|item| *item == ext) || ext == "zip" || ext.is_empty())
            && let Some(shows) = image_zip(file_name, bytes, media)?
        {
            return Ok(Some(shows));
        }
    }
    if ext == "csv" {
        return Ok(Some(vec![csv_show(file_name, bytes)?]));
    }
    if IMAGE_EXTS.iter().any(|item| *item == ext) {
        return Ok(Some(vec![image_show(file_name, bytes, source, media)?]));
    }
    if ext == "pdf" || bytes.starts_with(b"%PDF") {
        return Ok(Some(vec![pdf_show(file_name, bytes, source, media)?]));
    }
    let text = String::from_utf8_lossy(bytes).replace('\u{feff}', "");
    let trimmed = text.trim();
    if is_mediashout(trimmed) {
        return Ok(Some(mediashout(file_name, trimmed)?));
    }
    if is_easyslides(trimmed) {
        return Ok(Some(easyslides(file_name, trimmed)?));
    }
    if is_verseview(trimmed) {
        return Ok(Some(verseview(file_name, trimmed)?));
    }
    if is_quelea(trimmed) {
        return Ok(Some(vec![quelea(file_name, trimmed)?]));
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && is_videopsalm_value(&value)
    {
        return Ok(Some(videopsalm(file_name, &value)?));
    }
    if let Some(value) = parse_videopsalm_text(trimmed)
        && is_videopsalm_value(&value)
    {
        return Ok(Some(videopsalm(file_name, &value)?));
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && is_mediashout_mdb(&value)
    {
        return Ok(Some(mediashout_mdb(file_name, &value)?));
    }
    if (ext == "json" || ext == "vpc")
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && is_videopsalm_value(&value)
    {
        return Ok(Some(videopsalm(file_name, &value)?));
    }
    Ok(None)
}

pub fn is_project_package(file_name: &str, bytes: &[u8]) -> bool {
    let ext = extension(file_name);
    if looks_like_zip(bytes) {
        return match ext.as_str() {
            "project" | "shows" => true,
            "zip" => {
                let Ok(mut zip) = ZipArchive::new(Cursor::new(bytes)) else {
                    return false;
                };
                (0..zip.len().min(crate::portable::MAX_ENTRIES)).any(|index| {
                    let Ok(file) = zip.by_index(index) else {
                        return false;
                    };
                    let filename = crate::portable::basename(file.name());
                    if !matches!(extension(filename).as_str(), "json" | "project" | "shows")
                        || file.size() > crate::portable::MAX_DOCUMENT_BYTES
                    {
                        return false;
                    }
                    let mut data = Vec::new();
                    file.take(crate::portable::MAX_DOCUMENT_BYTES + 1)
                        .read_to_end(&mut data)
                        .is_ok()
                        && project_value(&data).is_some()
                })
            }
            _ => false,
        };
    }
    matches!(ext.as_str(), "project" | "shows" | "json" | "") && project_value(bytes).is_some()
}

pub fn import_project_package(
    path: &Path,
    bytes: &[u8],
    shows_dir: &Path,
    media_dir: &Path,
) -> Result<Vec<String>, DataError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let bundled = if looks_like_zip(bytes) {
        zip_entries(bytes)?
    } else {
        Vec::new()
    };
    let payload = if looks_like_zip(bytes) {
        ["data.json", "project.json"]
            .into_iter()
            .find_map(|wanted| {
                bundled
                    .iter()
                    .find(|(name, _)| name == wanted)
                    .and_then(|(_, data)| project_value(data))
            })
            .or_else(|| bundled.iter().find_map(|(_, data)| project_value(data)))
    } else {
        project_value(bytes)
    };
    let payload =
        payload.ok_or_else(|| DataError::Scripture(format!("{name} did not contain a project")))?;
    crate::portable::install(
        payload,
        &bundled,
        &file_stem_str(name),
        shows_dir,
        media_dir,
    )
}

pub fn softprojector_database(bytes: &[u8]) -> Result<Vec<ShowFile>, DataError> {
    let path = std::env::temp_dir().join(format!("freeshow-sps-{}.sqlite", crate::new_id()));
    fs::write(&path, bytes)?;
    let result = (|| {
        let connection = rusqlite::Connection::open(&path)
            .map_err(|err| DataError::Scripture(err.to_string()))?;
        if !table_exists(&connection, "Songs") {
            return Ok(Vec::new());
        }
        let mut statement = connection
            .prepare("SELECT * FROM Songs")
            .map_err(|err| DataError::Scripture(err.to_string()))?;
        let columns: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut rows = statement
            .query([])
            .map_err(|err| DataError::Scripture(err.to_string()))?;
        let mut shows = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|err| DataError::Scripture(err.to_string()))?
        {
            let get = |name: &str| column_text(row, &columns, name);
            let title = get("title");
            let lyrics = get("song_text");
            if lyrics.trim().is_empty() {
                continue;
            }
            let name = if title.trim().is_empty() {
                "Song".to_string()
            } else {
                title.clone()
            };
            let sections = softprojector_sections(&lyrics);
            if sections.is_empty() {
                continue;
            }
            let mut show = show_file(&format!("{name}.sps"), &name, sections);
            if let Some(document) = show.show.as_object_mut() {
                document.insert("origin".to_string(), json!("softprojector"));
                document.insert(
                    "meta".to_string(),
                    json!({
                        "number": get("number"),
                        "title": name,
                        "author": get("words"),
                        "composer": get("music"),
                        "note": get("notes"),
                        "key": get("tune")
                    }),
                );
            }
            shows.push(show);
        }
        Ok(shows)
    })();
    let _ = fs::remove_file(&path);
    result
}

fn table_exists(connection: &rusqlite::Connection, name: &str) -> bool {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
}

fn column_text(row: &rusqlite::Row<'_>, columns: &[String], name: &str) -> String {
    let Some(index) = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case(name))
    else {
        return String::new();
    };
    row.get::<_, rusqlite::types::Value>(index)
        .ok()
        .map(sql_text)
        .unwrap_or_default()
}

fn sql_text(value: rusqlite::types::Value) -> String {
    match value {
        rusqlite::types::Value::Null => String::new(),
        rusqlite::types::Value::Integer(value) => value.to_string(),
        rusqlite::types::Value::Real(value) => value.to_string(),
        rusqlite::types::Value::Text(value) => value,
        rusqlite::types::Value::Blob(value) => String::from_utf8_lossy(&value).into_owned(),
    }
}

fn softprojector_sections(lyrics: &str) -> Vec<Section> {
    lyrics
        .split("\n\n")
        .filter_map(|block| {
            let mut lines: Vec<String> =
                block.lines().map(|line| line.trim().to_string()).collect();
            lines.retain(|line| !line.is_empty());
            if lines.is_empty() {
                return None;
            }
            let group = group_name(&lines.remove(0));
            if lines.is_empty() {
                return None;
            }
            Some(Section::plain(group, lines))
        })
        .collect()
}

fn mediashout(file_name: &str, xml: &str) -> Result<Vec<ShowFile>, DataError> {
    let root = parse_xml(xml)?;
    let mut shows = Vec::new();
    for song in descendants(&root, "Song")
        .into_iter()
        .chain(descendants(&root, "song"))
    {
        let title = child_text(song, "Title")
            .or_else(|| child_text(song, "SongTitle"))
            .unwrap_or_else(|| file_stem_str(file_name));
        let sections: Vec<Section> = descendants(song, "Element")
            .into_iter()
            .filter_map(|element| {
                let text = child_text(element, "Text")?;
                if text.trim().is_empty() {
                    return None;
                }
                Some(Section::plain(
                    "Verse".into(),
                    text.lines()
                        .map(|line| line.trim().to_string())
                        .filter(|line| !line.is_empty())
                        .collect(),
                ))
            })
            .filter(|section| !section.lines.is_empty())
            .collect();
        if sections.is_empty() {
            continue;
        }
        let mut show = show_file(&format!("{title}.ssc"), &title, sections);
        set_origin(&mut show, "mediashout");
        set_meta(
            &mut show,
            json!({
                "title": title,
                "author": child_text(song, "Author").unwrap_or_default(),
                "copyright": child_text(song, "Copyright").unwrap_or_default(),
                "CCLI": child_text(song, "CCLI").unwrap_or_default()
            }),
        );
        shows.push(show);
    }
    if shows.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    Ok(shows)
}

fn mediashout_mdb(file_name: &str, value: &Value) -> Result<Vec<ShowFile>, DataError> {
    let songs = value.get("Songs").and_then(Value::as_array).cloned();
    let verses = value.get("Verses").and_then(Value::as_array).cloned();
    let (Some(songs), Some(verses)) = (songs, verses) else {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    };
    let mut shows = Vec::new();
    for song in songs {
        let record = json_i64(song.get("Record"));
        let title = song
            .get("Title")
            .and_then(Value::as_str)
            .unwrap_or("Song")
            .to_string();
        let mut items: Vec<(i64, String)> = verses
            .iter()
            .filter(|verse| json_i64(verse.get("Record")) == record)
            .filter(|verse| json_i64(verse.get("Type")) != 102)
            .filter_map(|verse| {
                let text = verse.get("Text").and_then(Value::as_str)?;
                if text.trim().is_empty() {
                    return None;
                }
                Some((json_i64(verse.get("Number")), text.to_string()))
            })
            .collect();
        items.sort_by_key(|(number, _)| *number);
        let sections: Vec<Section> = items
            .into_iter()
            .map(|(_, text)| {
                Section::plain(
                    "Verse".into(),
                    text.lines()
                        .map(|line| line.trim().to_string())
                        .filter(|line| !line.is_empty())
                        .collect(),
                )
            })
            .filter(|section| !section.lines.is_empty())
            .collect();
        if sections.is_empty() {
            continue;
        }
        let mut show = show_file(&format!("{title}.mdb"), &title, sections);
        set_origin(&mut show, "mediashout");
        set_meta(
            &mut show,
            json!({
                "title": title,
                "author": song.get("Author").and_then(Value::as_str).unwrap_or(""),
                "copyright": song.get("Copyright").and_then(Value::as_str).unwrap_or(""),
                "CCLI": song.get("CCLI").and_then(Value::as_str).unwrap_or("")
            }),
        );
        shows.push(show);
    }
    if shows.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    Ok(shows)
}

fn quelea(file_name: &str, xml: &str) -> Result<ShowFile, DataError> {
    let root = parse_xml(xml)?;
    let title = child_text(&root, "title").unwrap_or_else(|| file_stem_str(file_name));
    let lyrics_owner = children_named(&root, "lyrics").into_iter().next().cloned();
    let lyrics = lyrics_owner.as_ref().unwrap_or(&root);
    let sections: Vec<Section> = children_named(lyrics, "section")
        .into_iter()
        .filter_map(|section| {
            let group = group_name(
                section
                    .attrs
                    .get("title")
                    .map(String::as_str)
                    .unwrap_or("Verse"),
            );
            let text = child_text(section, "lyrics").unwrap_or(section.text.clone());
            let lines: Vec<String> = text
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            (!lines.is_empty()).then_some(Section::plain(group, lines))
        })
        .collect();
    if sections.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    let mut show = show_file(&format!("{title}.xml"), &title, sections);
    set_origin(&mut show, "quelea");
    set_meta(
        &mut show,
        json!({
            "title": title,
            "author": child_text(&root, "author").unwrap_or_default(),
            "CCLI": child_text(&root, "ccli").unwrap_or_default(),
            "copyright": child_text(&root, "copyright").unwrap_or_default(),
            "key": child_text(&root, "key").unwrap_or_default(),
            "publisher": child_text(&root, "publisher").unwrap_or_default(),
            "year": child_text(&root, "year").unwrap_or_default()
        }),
    );
    Ok(show)
}

fn easyslides(file_name: &str, xml: &str) -> Result<Vec<ShowFile>, DataError> {
    let root = parse_xml(xml)?;
    let mut shows = Vec::new();
    for item in descendants(&root, "Item") {
        let title = child_text(item, "Title1").unwrap_or_else(|| file_stem_str(file_name));
        let mut contents = child_text(item, "Contents").unwrap_or_default();
        contents = contents.replace('[', "\n\n[");
        contents = contents.replace("\n\n\n\n", "\n\n");
        let mut cleaned = String::new();
        let mut chars = contents.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '[' && chars.peek().is_some_and(char::is_ascii_digit) {
                while chars.next().is_some_and(|next| next != ']') {}
                cleaned.push_str("\n\n");
            } else {
                cleaned.push(ch);
            }
        }
        let sections: Vec<Section> = cleaned
            .split("\n\n")
            .filter_map(|block| {
                let mut lines: Vec<String> = block
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
                if lines.is_empty() {
                    return None;
                }
                let group = if lines[0].starts_with('[') && lines[0].ends_with(']') {
                    let name = lines.remove(0);
                    group_name(name.trim_matches(|ch| ch == '[' || ch == ']'))
                } else {
                    "Verse".into()
                };
                (!lines.is_empty()).then_some(Section::plain(group, lines))
            })
            .collect();
        if sections.is_empty() {
            continue;
        }
        let mut show = show_file(&format!("{title}.xml"), &title, sections);
        set_origin(&mut show, "easyslides");
        if let Some(number) = child_text(item, "SongNumber") {
            set_meta(&mut show, json!({"number": number}));
        }
        shows.push(show);
    }
    if shows.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    Ok(shows)
}

fn verseview(file_name: &str, xml: &str) -> Result<Vec<ShowFile>, DataError> {
    let root = parse_xml(xml)?;
    let mut shows = Vec::new();
    for song in descendants(&root, "song") {
        let title = child_text(song, "name").unwrap_or_else(|| file_stem_str(file_name));
        let mut blocks = Vec::new();
        for tag in ["slide", "slide2"] {
            if let Some(text) = child_text(song, tag) {
                for part in text.split("<slide>") {
                    let lines: Vec<String> = part
                        .replace("<BR>", "<br>")
                        .replace("<Br>", "<br>")
                        .split("<br>")
                        .map(|line| line.trim().to_string())
                        .filter(|line| !line.is_empty())
                        .collect();
                    if !lines.is_empty() {
                        blocks.push(Section::plain("Verse".into(), lines));
                    }
                }
            }
        }
        if blocks.is_empty() {
            continue;
        }
        let mut show = show_file(&format!("{title}.xml"), &title, blocks);
        set_origin(&mut show, "verseview");
        set_meta(
            &mut show,
            json!({
                "title": title,
                "author": child_text(song, "author").unwrap_or_default(),
                "copyright": child_text(song, "copyright").unwrap_or_default(),
                "publisher": child_text(song, "copyright").unwrap_or_default(),
                "number": child_text(song, "subcat").unwrap_or_default()
            }),
        );
        shows.push(show);
    }
    if shows.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    Ok(shows)
}

fn videopsalm(file_name: &str, value: &Value) -> Result<Vec<ShowFile>, DataError> {
    let songs = value
        .get("Songs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let album = value.get("Text").and_then(Value::as_str).unwrap_or("");
    let mut shows = Vec::new();
    for (index, song) in songs.iter().enumerate() {
        let title = song
            .get("Text")
            .and_then(Value::as_str)
            .unwrap_or("Song")
            .replace("<br>", "")
            .trim()
            .to_string();
        let title = if title.is_empty() {
            format!("Song {}", index + 1)
        } else {
            title
        };
        let verses = song
            .get("Verses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let sequence: Vec<String> = song
            .get("Sequence")
            .and_then(Value::as_str)
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut sections = Vec::new();
        let mut by_key = HashMap::new();
        for (verse_index, verse) in verses.iter().enumerate() {
            let text = verse.get("Text").and_then(Value::as_str).unwrap_or("");
            let lines = videopsalm_lines(text);
            if lines.is_empty() {
                continue;
            }
            let key = sequence.get(verse_index).cloned().unwrap_or_default();
            let group = if key.is_empty() {
                "Verse".into()
            } else {
                videopsalm_group(&key)
            };
            let section = Section::plain(group, lines);
            if !key.is_empty() {
                by_key.entry(key).or_insert(sections.len());
            }
            sections.push(section);
        }
        if sequence.len() >= sections.len() && !by_key.is_empty() {
            let ordered: Vec<Section> = sequence
                .iter()
                .filter_map(|key| {
                    by_key
                        .get(key)
                        .and_then(|index| sections.get(*index))
                        .cloned()
                })
                .collect();
            if !ordered.is_empty() {
                sections = ordered;
            }
        }
        if sections.is_empty() {
            continue;
        }
        let mut show = show_file(&format!("{title}.json"), &title, sections);
        set_origin(&mut show, "videopsalm");
        set_meta(
            &mut show,
            json!({
                "title": title,
                "artist": album,
                "author": song.get("Author").and_then(Value::as_str).unwrap_or(""),
                "composer": song.get("Composer").and_then(Value::as_str).unwrap_or(""),
                "copyright": song.get("Copyright").and_then(Value::as_str).unwrap_or(""),
                "CCLI": song.get("CCLI").and_then(Value::as_str).unwrap_or(""),
                "number": song.get("ID").map(|value| value.to_string().trim_matches('"').to_string()).unwrap_or_default()
            }),
        );
        shows.push(show);
    }
    if shows.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    Ok(shows)
}

fn videopsalm_lines(text: &str) -> Vec<String> {
    text.split("<br>")
        .map(|line| {
            let mut out = String::new();
            for part in line.split(']') {
                if let Some((lyric, _chord)) = part.split_once('[') {
                    out.push_str(lyric);
                } else {
                    out.push_str(part);
                }
            }
            out.replace("<fNirmala UI>", "")
                .replace("</f>", "")
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn videopsalm_group(key: &str) -> String {
    match key
        .chars()
        .find(|ch| ch.is_ascii_alphabetic())
        .unwrap_or('V')
    {
        'C' | 'R' => "Chorus".into(),
        'P' => "Pre-chorus".into(),
        'B' => "Bridge".into(),
        'T' => "Tag".into(),
        'I' => "Intro".into(),
        'O' => "Outro".into(),
        'N' => "Break".into(),
        _ => "Verse".into(),
    }
}

fn csv_show(file_name: &str, bytes: &[u8]) -> Result<ShowFile, DataError> {
    let text = String::from_utf8_lossy(bytes).replace('\u{feff}', "");
    let mut sections = Vec::new();
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = parse_csv_line(line);
        if fields.is_empty() {
            continue;
        }
        sections.push(Section::plain("Verse".into(), fields));
    }
    if sections.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain slides"
        )));
    }
    let mut show = show_file(file_name, &file_stem_str(file_name), sections);
    set_origin(&mut show, "csv");
    Ok(show)
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            index += 1;
            let mut field = String::new();
            while index < chars.len() {
                if chars[index] == '"' {
                    if index + 1 < chars.len() && chars[index + 1] == '"' {
                        field.push('"');
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                field.push(chars[index]);
                index += 1;
            }
            fields.push(field);
            if index < chars.len() && chars[index] == ',' {
                index += 1;
            }
        } else {
            let start = index;
            while index < chars.len() && chars[index] != ',' {
                index += 1;
            }
            fields.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string(),
            );
            if index < chars.len() && chars[index] == ',' {
                index += 1;
            }
        }
    }
    fields.retain(|field| !field.is_empty());
    fields
}

fn image_show(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
) -> Result<ShowFile, DataError> {
    let name = file_stem_str(file_name);
    let stored = store_media(media, file_name, bytes)?;
    let path = stored.unwrap_or_else(|| {
        source
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(file_name))
    });
    Ok(images_to_show(&name, &[(name.clone(), path)]))
}

fn pdf_show(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
) -> Result<ShowFile, DataError> {
    let name = file_stem_str(file_name);
    let pages = render_pdf_pages(file_name, bytes, source, media)?;
    if pages.is_empty() {
        return Err(DataError::Scripture(format!(
            "{file_name} did not contain pages"
        )));
    }
    let rendered = pages.iter().any(|(_, path)| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    });
    let mut show = if rendered {
        images_to_show(&name, &pages)
    } else {
        pdf_text_show(&name, pages.len())
    };
    show.show["origin"] = json!("pdf");
    Ok(show)
}

fn render_pdf_pages(
    file_name: &str,
    bytes: &[u8],
    source: Option<&Path>,
    media: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, DataError> {
    let Some(media_dir) = media else {
        return Ok(pdf_placeholder_pages(file_name, bytes));
    };
    fs::create_dir_all(media_dir)?;
    let pdf_path = if let Some(path) = source.filter(|path| path.is_file()) {
        path.to_path_buf()
    } else {
        let dest = media_dir.join(
            Path::new(file_name)
                .file_name()
                .unwrap_or_else(|| "import.pdf".as_ref()),
        );
        if !dest.exists() {
            fs::write(&dest, bytes)?;
        }
        dest
    };
    let prefix = media_dir.join(format!("{}-page", file_stem_str(file_name)));
    let rendered = Command::new("pdftoppm")
        .args(["-png", "-r", "72"])
        .arg(&pdf_path)
        .arg(&prefix)
        .output();
    if rendered
        .as_ref()
        .is_ok_and(|output| output.status.success())
    {
        let mut pages = Vec::new();
        let stem = prefix
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string());
        if let Ok(entries) = fs::read_dir(media_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if name.starts_with(&stem) && name.ends_with(".png") {
                    pages.push((file_stem_str(name), path));
                }
            }
        }
        pages.sort_by(|left, right| left.1.file_name().cmp(&right.1.file_name()));
        if !pages.is_empty() {
            return Ok(pages);
        }
    }
    Ok(pdf_placeholder_pages(file_name, bytes))
}

fn pdf_placeholder_pages(file_name: &str, bytes: &[u8]) -> Vec<(String, PathBuf)> {
    (1..=pdf_page_count(bytes))
        .map(|index| (format!("Page {index}"), PathBuf::from(file_name)))
        .collect()
}

fn pdf_page_count(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes);
    let mut pages = 0;
    let mut count = 0;
    for after_type in text.split("/Type").skip(1) {
        let mut tokens = after_type
            .split(|ch: char| ch.is_ascii_whitespace() || "()<>[]{}/%".contains(ch))
            .filter(|token| !token.is_empty());
        match tokens.next() {
            Some("Page") => pages += 1,
            Some("Pages") => {
                let tree = after_type.split("endobj").next().unwrap_or("");
                if let Some(rest) = tree.split_once("/Count").map(|(_, rest)| rest.trim_start()) {
                    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                    count = count.max(rest[..digits].parse::<usize>().unwrap_or(0));
                }
            }
            _ => {}
        }
    }
    // A fallback uses labels alone. Keep malformed PDFs from allocating
    // unbounded slides based on a forged page count.
    count.max(pages).clamp(1, 2000)
}

fn pdf_text_show(name: &str, pages: usize) -> ShowFile {
    let mut slides = serde_json::Map::new();
    let mut order = Vec::new();
    for index in 0..pages.max(1) {
        let slide_id = format!("s{}", index + 1);
        let text = format!("Page {}", index + 1);
        slides.insert(
            slide_id.clone(),
            json!({
                "group": "",
                "color": null,
                "settings": {},
                "notes": "",
                "items": [{
                    "style": super::ITEM_STYLE,
                    "lines": [{"align": "", "text": [{"value": text, "style": "font-size: 80px;"}]}]
                }]
            }),
        );
        order.push(json!({"id": slide_id}));
    }
    let document = json!({
        "name": name,
        "origin": "pdf",
        "settings": {"activeLayout": "main", "template": null},
        "slides": slides,
        "layouts": {"main": {"name": "Default", "notes": "", "slides": order}},
        "media": {}
    });
    ShowFile::parse(&json!([name, document]).to_string()).expect("pdf show is valid JSON")
}

fn image_zip(
    file_name: &str,
    bytes: &[u8],
    media: Option<&Path>,
) -> Result<Option<Vec<ShowFile>>, DataError> {
    let files = zip_entries(bytes)?;
    let mut images = Vec::new();
    for (name, data) in files {
        let ext = extension(&name);
        if IMAGE_EXTS.iter().any(|item| *item == ext) {
            let dest = store_media(
                media,
                Path::new(&name)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&name),
                &data,
            )?
            .unwrap_or_else(|| PathBuf::from(&name));
            images.push((file_stem_str(&name), dest));
        }
    }
    if images.is_empty() {
        return Ok(None);
    }
    Ok(Some(vec![images_to_show(
        &file_stem_str(file_name),
        &images,
    )]))
}

fn images_to_show(name: &str, images: &[(String, PathBuf)]) -> ShowFile {
    let mut slides = serde_json::Map::new();
    let mut media = serde_json::Map::new();
    let mut order = Vec::new();
    for (index, (label, path)) in images.iter().enumerate() {
        let slide_id = format!("s{}", index + 1);
        let media_id = format!("m{}", index + 1);
        media.insert(
            media_id.clone(),
            json!({"name": path.file_name().and_then(|n| n.to_str()).unwrap_or(label), "path": path, "videoType": "foreground"}),
        );
        slides.insert(
            slide_id.clone(),
            json!({"group": "", "color": null, "settings": {}, "notes": "", "items": []}),
        );
        order.push(json!({"id": slide_id, "background": media_id}));
    }
    let document = json!({
        "name": name,
        "origin": "image",
        "settings": {"activeLayout": "main", "template": null},
        "slides": slides,
        "layouts": {"main": {"name": "Default", "notes": "", "slides": order}},
        "media": media
    });
    ShowFile::parse(&json!([name, document]).to_string()).expect("image show is valid JSON")
}

fn zip_shows(
    file_name: &str,
    bytes: &[u8],
    media: Option<&Path>,
) -> Result<Option<Vec<ShowFile>>, DataError> {
    let files = zip_entries(bytes)?;
    let mut shows = Vec::new();
    let mut images = Vec::new();
    for (name, data) in &files {
        if IMAGE_EXTS.iter().any(|item| *item == extension(name)) {
            images.push((name.clone(), data.clone()));
            continue;
        }
        if let Some(found) = import_shows(name, data, None, media)? {
            shows.extend(found);
        }
    }
    if shows.is_empty() && !images.is_empty() {
        let mut stored = Vec::new();
        for (name, data) in images {
            let dest = store_media(
                media,
                Path::new(&name)
                    .file_name()
                    .and_then(|file| file.to_str())
                    .unwrap_or(&name),
                &data,
            )?
            .unwrap_or_else(|| PathBuf::from(&name));
            stored.push((file_stem_str(&name), dest));
        }
        shows.push(images_to_show(&file_stem_str(file_name), &stored));
    }
    Ok((!shows.is_empty()).then_some(shows))
}

fn store_media(
    media: Option<&Path>,
    name: &str,
    bytes: &[u8],
) -> Result<Option<PathBuf>, DataError> {
    let Some(dir) = media else {
        return Ok(None);
    };
    fs::create_dir_all(dir)?;
    let dest = dir.join(Path::new(name).file_name().unwrap_or_else(|| name.as_ref()));
    if !dest.exists() {
        fs::write(&dest, bytes)?;
    }
    Ok(Some(dest))
}

fn project_value(bytes: &[u8]) -> Option<Value> {
    if bytes.len() as u64 > crate::portable::MAX_DOCUMENT_BYTES {
        return None;
    }
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    value.get("project").is_some().then_some(value)
}

fn is_mediashout(text: &str) -> bool {
    text.contains("MediaShout5_Document") || text.contains("<MediaShout")
}

fn is_easyslides(text: &str) -> bool {
    text.contains("<Easyslides") || text.contains("<EasiSlides")
}

fn is_verseview(text: &str) -> bool {
    text.contains("<songDB") && text.contains("<song")
}

fn is_quelea(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("<song") && lower.contains("<section") && lower.contains("<lyrics")
}

fn is_mediashout_mdb(value: &Value) -> bool {
    value.get("Songs").is_some_and(Value::is_array)
        && value.get("Verses").is_some_and(Value::is_array)
}

fn is_videopsalm_value(value: &Value) -> bool {
    value
        .get("Songs")
        .and_then(Value::as_array)
        .is_some_and(|songs| {
            songs.iter().any(|song| {
                song.get("Verses").and_then(Value::as_array).is_some() || song.get("Text").is_some()
            })
        })
}

fn parse_videopsalm_text(text: &str) -> Option<Value> {
    let prepared = text.replace("{\n", "{").replace("}\n", "}");
    // Quote bare identifiers before colons when this looks like VideoPsalm JSON.
    if !prepared.contains("Songs") {
        return None;
    }
    let mut out = String::new();
    let chars: Vec<char> = prepared.chars().collect();
    let mut index = 0;
    let mut in_string = false;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '"' && (index == 0 || chars[index - 1] != '\\') {
            in_string = !in_string;
            out.push(ch);
            index += 1;
            continue;
        }
        if !in_string && ch.is_ascii_alphabetic() {
            let start = index;
            while index < chars.len() && chars[index].is_ascii_alphanumeric() {
                index += 1;
            }
            let word: String = chars[start..index].iter().collect();
            let mut cursor = index;
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor < chars.len() && chars[cursor] == ':' {
                out.push('"');
                out.push_str(&word);
                out.push('"');
                continue;
            }
            out.push_str(&word);
            continue;
        }
        out.push(ch);
        index += 1;
    }
    serde_json::from_str(&out).ok()
}

fn set_origin(show: &mut ShowFile, origin: &str) {
    if let Some(document) = show.show.as_object_mut() {
        document.insert("origin".to_string(), json!(origin));
    }
}

fn set_meta(show: &mut ShowFile, meta: Value) {
    if let Some(document) = show.show.as_object_mut() {
        document.insert("meta".to_string(), meta);
    }
}

fn group_name(raw: &str) -> String {
    let key: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    match key.as_str() {
        "verse" | "v" | "s" => "Verse".into(),
        "chorus" | "c" | "r" | "refrain" => "Chorus".into(),
        "bridge" | "b" => "Bridge".into(),
        "prechorus" | "p" => "Pre-chorus".into(),
        "intro" | "i" => "Intro".into(),
        "outro" | "o" | "end" | "ending" => "Outro".into(),
        "tag" | "t" => "Tag".into(),
        _ => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                "Verse".into()
            } else {
                trimmed.to_string()
            }
        }
    }
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] == b'P' && bytes[1] == b'K'
}

fn zip_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, DataError> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| DataError::Scripture(err.to_string()))?;
    if archive.len() > crate::portable::MAX_ENTRIES {
        return Err(DataError::Scripture("Archive has too many entries".into()));
    }
    let mut remaining = crate::portable::MAX_PACKAGE_BYTES;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| DataError::Scripture(err.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        if file.size() > remaining {
            return Err(DataError::Scripture(
                "Archive exceeds the unpacked size limit".into(),
            ));
        }
        let mut data = Vec::new();
        (&mut file)
            .take(remaining + 1)
            .read_to_end(&mut data)
            .map_err(|err| DataError::Scripture(err.to_string()))?;
        if data.len() as u64 > remaining {
            return Err(DataError::Scripture(
                "Archive exceeds the unpacked size limit".into(),
            ));
        }
        remaining -= data.len() as u64;
        files.push((name, data));
    }
    Ok(files)
}

fn json_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_u64().map(|n| n as i64))
            .unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

#[derive(Clone)]
struct XmlEl {
    name: String,
    attrs: HashMap<String, String>,
    text: String,
    children: Vec<XmlEl>,
}

fn parse_xml(text: &str) -> Result<XmlEl, DataError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<XmlEl> = Vec::new();
    let mut root = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local(event.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in event.attributes().flatten() {
                    let key = local(attr.key.as_ref());
                    let value = attr
                        .unescape_value()
                        .map_err(|err| DataError::Scripture(err.to_string()))?
                        .into_owned();
                    attrs.insert(key, value);
                }
                stack.push(XmlEl {
                    name,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(event)) => {
                let name = local(event.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in event.attributes().flatten() {
                    let key = local(attr.key.as_ref());
                    let value = attr
                        .unescape_value()
                        .map_err(|err| DataError::Scripture(err.to_string()))?
                        .into_owned();
                    attrs.insert(key, value);
                }
                let el = XmlEl {
                    name,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(el);
                } else {
                    root = Some(el);
                }
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .unescape()
                    .map_err(|err| DataError::Scripture(err.to_string()))?;
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(&text);
                }
            }
            Ok(Event::CData(event)) => {
                let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(&text);
                }
            }
            Ok(Event::End(_)) => {
                let Some(finished) = stack.pop() else {
                    continue;
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(finished);
                } else {
                    root = Some(finished);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(DataError::Scripture(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    root.ok_or_else(|| DataError::Scripture("empty XML document".into()))
}

fn local(name: &[u8]) -> String {
    let text = String::from_utf8_lossy(name);
    text.rsplit(':').next().unwrap_or(&text).to_string()
}

fn children_named<'a>(node: &'a XmlEl, name: &str) -> Vec<&'a XmlEl> {
    node.children
        .iter()
        .filter(|child| child.name.eq_ignore_ascii_case(name))
        .collect()
}

fn descendants<'a>(node: &'a XmlEl, name: &str) -> Vec<&'a XmlEl> {
    let mut found = children_named(node, name);
    for child in &node.children {
        found.extend(descendants(child, name));
    }
    found
}

fn child_text(node: &XmlEl, name: &str) -> Option<String> {
    children_named(node, name)
        .into_iter()
        .next()
        .map(|child| {
            if child.text.trim().is_empty() && !child.children.is_empty() {
                child
                    .children
                    .iter()
                    .map(|inner| inner.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            } else {
                child.text.clone()
            }
        })
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, ProjectsFile};

    #[test]
    fn community_formats_import_lyrics_images_and_projects() {
        let mediashout = br#"<?xml version="1.0"?>
<MediaShout5_Document><Cues><Song>
<Title>Amazing Grace</Title><Author>Newton</Author><CCLI>123</CCLI>
<Content>
<Element><Text>Amazing grace, how sweet the sound</Text></Element>
<Element><Text>I once was lost, but now am found</Text></Element>
</Content>
</Song></Cues></MediaShout5_Document>"#;
        let shows = import_shows("grace.ssc", mediashout, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(shows[0].show["origin"], "mediashout");
        assert_eq!(shows[0].name(), "Amazing Grace");
        assert_eq!(
            shows[0].ordered_slides()[1].lines[0],
            "I once was lost, but now am found"
        );

        let mdb = json!({"Songs":[{"Record":1,"Title":"Be Thou My Vision","Author":"Irish","CCLI":"9"}],
            "Verses":[{"Record":1,"Number":1,"Type":1,"Text":"Be thou my vision"},{"Record":1,"Number":2,"Type":1,"Text":"High King of heaven"}]});
        let shows = import_shows("songs.json", mdb.to_string().as_bytes(), None, None)
            .unwrap()
            .unwrap();
        assert_eq!(shows[0].name(), "Be Thou My Vision");
        assert_eq!(shows[0].ordered_slides().len(), 2);

        let quelea =
            br#"<song><title>Holy Holy Holy</title><author>Heber</author><ccli>1</ccli><key>D</key>
<lyrics>
<section title="Verse 1"><lyrics>Holy, holy, holy
Lord God Almighty</lyrics></section>
<section title="Chorus"><lyrics>God in three persons</lyrics></section>
</lyrics></song>"#;
        let show = import_shows("holy.xml", quelea, None, None)
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "quelea");
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Verse"));
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus"));

        let sps = write_sps();
        let shows = softprojector_database(&sps).unwrap();
        assert_eq!(shows[0].name(), "It Is Well");
        assert_eq!(shows[0].ordered_slides()[0].group.as_deref(), Some("Verse"));
        assert_eq!(
            shows[0].ordered_slides()[0].lines[0],
            "When peace like a river"
        );

        let easy = br#"<Easyslides><Item><Title1>And Can It Be</Title1><SongNumber>7</SongNumber>
<Contents>[Verse]
And can it be that I should gain

[Chorus]
Amazing love, how can it be</Contents></Item></Easyslides>"#;
        let show = import_shows("easyslides.xml", easy, None, None)
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "easyslides");
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus"));
        assert_eq!(show.show["meta"]["number"], "7");

        let verseview = br#"<songDB><song><name>Blessed Assurance</name><author>Crosby</author>
<slide>Blessed assurance, Jesus is mine&lt;BR&gt;O what a foretaste&lt;slide&gt;This is my story</slide>
</song></songDB>"#;
        let show = import_shows("vv.xml", verseview, None, None)
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "verseview");
        assert_eq!(show.ordered_slides().len(), 2);
        assert_eq!(
            show.ordered_slides()[0].lines[0],
            "Blessed assurance, Jesus is mine"
        );

        let vp = br#"{Text:"Hymns",Songs:[{Text:"Great Is Thy Faithfulness",Author:"Chisholm",Sequence:"V1 C",Verses:[{Text:"Great is thy faithfulness, O God my Father"},{Text:"Morning by morning new mercies I see"}]}]}"#;
        let show = import_shows("songs.vpc", vp, None, None)
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "videopsalm");
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Verse"));
        assert_eq!(show.ordered_slides()[1].group.as_deref(), Some("Chorus"));

        let csv =
            b"\"Amazing grace\",\"how sweet the sound\"\nPraise God,from whom all blessings flow\n";
        let show = import_shows("rows.csv", csv, None, None)
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "csv");
        assert_eq!(
            show.ordered_slides()[0].lines,
            ["Amazing grace", "how sweet the sound"]
        );
        assert_eq!(show.ordered_slides()[1].lines[0], "Praise God");

        let png = tiny_png();
        let root = std::env::temp_dir().join(format!("freeshow-community-{}", crate::new_id()));
        fs::create_dir_all(root.join("Shows")).unwrap();
        fs::create_dir_all(root.join("Media")).unwrap();
        fs::create_dir_all(root.join("Imports")).unwrap();
        fs::create_dir_all(root.join("Config")).unwrap();
        let show = import_shows("cover.png", &png, None, Some(&root.join("Media")))
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(show.show["origin"], "image");
        assert!(
            show.show["media"]["m1"]["path"]
                .as_str()
                .unwrap()
                .ends_with("cover.png")
        );
        assert_eq!(
            show.show["layouts"]["main"]["slides"][0]["background"],
            "m1"
        );

        let package = json!({
            "project": {"id":"sunday","name":"Sunday Service","shows":[{"id":"grace","type":"show"}]},
            "shows": {"grace": {"name":"Grace","settings":{"activeLayout":"main"},
                "slides":{"s1":{"group":"Verse","items":[{"style":"","lines":[{"text":[{"value":"Hello","style":""}]}]}]}},
                "layouts":{"main":{"name":"Default","slides":[{"id":"s1"}]}}}},
            "overlays": {"logo":{"name":"Logo","items":[]}}
        });
        fs::write(root.join("Imports/sunday.project"), package.to_string()).unwrap();
        let notes = import_project_package(
            &root.join("Imports/sunday.project"),
            package.to_string().as_bytes(),
            &root.join("Shows"),
            &root.join("Media"),
        )
        .unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let imported = ShowFile::load(&root.join("Shows/grace.show")).unwrap();
        assert_eq!(imported.name(), "Grace");
        let projects = ProjectsFile::load(&root.join("Config/projects.json")).unwrap();
        assert_eq!(projects.projects()[0].name, "Sunday Service");
        let overlays = Catalog::load(&root.join("Config/overlays.json")).unwrap();
        assert_eq!(overlays.names(), ["Logo"]);
        let _ = fs::remove_dir_all(&root);
    }

    fn write_sps() -> Vec<u8> {
        let path =
            std::env::temp_dir().join(format!("freeshow-sps-fix-{}.sqlite", crate::new_id()));
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute(
                    "CREATE TABLE Songs (title TEXT, song_text TEXT, number INTEGER, words TEXT, music TEXT, notes TEXT, tune TEXT)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO Songs VALUES ('It Is Well', 'Verse 1\nWhen peace like a river\n\nChorus\nIt is well with my soul', 1, 'Spafford', 'Bliss', '', 'C')",
                    [],
                )
                .unwrap();
        }
        let bytes = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);
        bytes
    }

    #[test]
    fn a_pdf_imports_as_one_slide_per_page() {
        let root = std::env::temp_dir().join(format!("freeshow-pdf-{}", crate::new_id()));
        fs::create_dir_all(root.join("Shows")).unwrap();
        fs::create_dir_all(root.join("Media")).unwrap();
        let bytes = two_page_pdf();
        let shows = import_shows("notes.pdf", &bytes, None, Some(&root.join("Media")))
            .unwrap()
            .unwrap();
        assert_eq!(shows.len(), 1);
        let show = &shows[0];
        assert_eq!(show.show["origin"], "pdf");
        assert_eq!(show.ordered_slides().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    fn two_page_pdf() -> Vec<u8> {
        br#"%PDF-1.1
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj
trailer<</Root 1 0 R>>
%%EOF
"#
        .to_vec()
    }

    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0x73,
            0xA2, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }
}
