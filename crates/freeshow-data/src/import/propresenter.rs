// SPDX-License-Identifier: GPL-3.0-only

use super::{Section, attribute, base64_decode, decode_rtf, file_stem_str, local_name, show_file};
use crate::{DataError, ShowFile};
use quick_xml::{Reader, events::Event};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

fn invalid(message: impl ToString) -> DataError {
    DataError::Scripture(message.to_string())
}

#[derive(Default)]
struct Slide {
    group: String,
    group_number: usize,
    notes: String,
    disabled: bool,
    lines: Vec<String>,
    background: Option<String>,
}

/// ProPresenter 4–6 XML stores plain text and RTF as base64 NSStrings.
pub(super) fn xml_show(file_name: &str, text: &str) -> Result<ShowFile, DataError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut slides = Vec::new();
    let mut depth = 0_usize;
    let mut current: Option<Slide> = None;
    let mut title = String::new();
    let mut group = String::new();
    let mut group_number = 0;
    let mut in_group = false;
    let mut in_text = false;
    let mut in_cue = false;
    let mut capture: Option<(String, String)> = None;
    let mut strings: Vec<(String, String)> = Vec::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(event) => {
                depth += 1;
                match local_name(event.name().as_ref()).as_str() {
                    "RVPresentationDocument" => {
                        title = attribute(&event, "name")
                            .or_else(|| attribute(&event, "CCLISongTitle"))
                            .unwrap_or_default();
                    }
                    "RVSlideGrouping" => {
                        group = attribute(&event, "name").unwrap_or_default();
                        group_number += 1;
                        in_group = true;
                    }
                    "RVDisplaySlide" => {
                        if current.is_some() {
                            return Err(invalid("nested ProPresenter slide"));
                        }
                        let slide = Slide {
                            group: group.clone(),
                            group_number: if in_group { group_number } else { 0 },
                            notes: attribute(&event, "notes").unwrap_or_default(),
                            disabled: attribute(&event, "enabled").as_deref() == Some("false"),
                            ..Slide::default()
                        };
                        current = Some(slide);
                    }
                    "RVTextElement" if current.is_some() => {
                        in_text = true;
                        strings.clear();
                        if let Some(rtf) = attribute(&event, "RTFData") {
                            strings.push(("RTFData".into(), rtf));
                        }
                    }
                    "NSString" if in_text => {
                        capture = Some((
                            attribute(&event, "rvXMLIvarName").unwrap_or_default(),
                            String::new(),
                        ));
                    }
                    "RVMediaCue" if current.is_some() => in_cue = true,
                    "RVVideoElement" | "RVImageElement" if in_cue => {
                        if let Some(slide) = &mut current {
                            slide.background =
                                attribute(&event, "source").filter(|path| !path.is_empty());
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(event) => {
                if let Some((_, text)) = &mut capture {
                    text.push_str(&event.unescape().map_err(invalid)?);
                }
            }
            Event::CData(event) => {
                if let Some((_, text)) = &mut capture {
                    text.push_str(&String::from_utf8_lossy(&event));
                }
            }
            Event::End(event) => {
                depth = depth.saturating_sub(1);
                match local_name(event.name().as_ref()).as_str() {
                    "NSString" => {
                        if let Some(value) = capture.take() {
                            strings.push(value);
                        }
                    }
                    "RVTextElement" => {
                        append_text(current.as_mut(), &strings);
                        in_text = false;
                        capture = None;
                        strings.clear();
                    }
                    "RVMediaCue" => in_cue = false,
                    "RVSlideGrouping" => {
                        group.clear();
                        in_group = false;
                    }
                    "RVDisplaySlide" => {
                        if let Some(slide) = current.take() {
                            slides.push(slide);
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || current.is_some() || slides.is_empty() {
        return Err(invalid("ProPresenter XML has no complete slides"));
    }
    let sections = slides
        .iter()
        .map(|slide| Section::plain(slide.group.clone(), slide.lines.clone()))
        .collect();
    let title = if title.trim().is_empty() {
        file_stem_str(file_name)
    } else {
        title
    };
    let mut show = show_file(file_name, &title, sections);
    show.show["origin"] = json!("propresenter");
    let mut order = Vec::<Value>::new();
    let mut parent: Option<(usize, String)> = None;
    for (index, slide) in slides.into_iter().enumerate() {
        let id = format!("s{}", index + 1);
        show.show["slides"][&id]["notes"] = json!(slide.notes);
        let mut node = json!({"id":id});
        if slide.disabled {
            node["disabled"] = json!(true);
        }
        if let Some(path) = slide.background {
            let media_id = format!("m{}", index + 1);
            show.show["media"][&media_id] = json!({"name":source_basename(&path),"path":path});
            node["background"] = json!(media_id);
        }
        if let Some((number, parent_id)) = &parent
            && *number == slide.group_number
            && *number != 0
        {
            show.show["slides"][parent_id]["children"]
                .as_array_mut()
                .unwrap()
                .push(json!(id));
            show.show["slides"][&id]
                .as_object_mut()
                .unwrap()
                .remove("group");
            order.last_mut().unwrap()["children"][&id] = node;
        } else {
            show.show["slides"][&id]["children"] = json!([]);
            parent = Some((slide.group_number, id));
            order.push(node);
        }
    }
    show.show["layouts"]["main"]["slides"] = json!(order);
    Ok(show)
}

fn append_text(slide: Option<&mut Slide>, strings: &[(String, String)]) {
    let Some(slide) = slide else {
        return;
    };
    let selected = strings
        .iter()
        .find(|(kind, _)| kind == "RTFData")
        .or_else(|| strings.iter().find(|(kind, _)| kind == "PlainText"));
    let selected: Vec<_> = match selected {
        Some(string) => vec![string],
        None => strings.iter().filter(|(kind, _)| kind.is_empty()).collect(),
    };
    for (kind, value) in selected {
        let decoded = if kind.is_empty() {
            value.clone()
        } else {
            base64_decode(value)
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| value.clone())
        };
        let decoded = if decoded.trim_start().starts_with("{\\rtf") {
            decode_rtf(&decoded)
        } else {
            decoded
        };
        if decoded.trim() != "Double-click to edit" {
            slide
                .lines
                .extend(decoded.lines().map(|line| line.trim().to_string()));
        }
    }
}

pub(super) fn is_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("probundle"))
}

// Validate the whole archive before copying any media. ZIP paths are never used
// as filesystem destinations; this also rejects unsafe unused members.
pub(super) fn bundle_shows(
    bytes: &[u8],
) -> Result<(zip::ZipArchive<Cursor<&[u8]>>, Vec<(String, ShowFile)>), DataError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() > 10_000 {
        return Err(invalid("ProPresenter bundle has too many entries"));
    }
    let mut names = HashSet::new();
    let mut total = 0_u64;
    let mut documents = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(invalid)?;
        let name = entry.name().replace('\\', "/");
        if name.starts_with('/')
            || name.contains('\0')
            || name
                .split('/')
                .any(|part| part == ".." || part.contains(':'))
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(invalid(format!("unsafe bundle entry: {name}")));
        }
        if !names.insert(name.clone()) {
            return Err(invalid(format!("duplicate bundle entry: {name}")));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("bundle size overflow"))?;
        if entry.size() > 512 * 1024 * 1024 || total > 2 * 1024 * 1024 * 1024 {
            return Err(invalid("ProPresenter bundle exceeds media size limits"));
        }
        let extension = Path::new(&name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !entry.is_dir() && ["pro4", "pro5", "pro6", "pro"].contains(&extension.as_str()) {
            documents.push(index);
        }
    }
    let mut shows = Vec::new();
    for index in documents {
        let mut entry = archive.by_index(index).map_err(invalid)?;
        if entry.size() > 16 * 1024 * 1024 {
            return Err(invalid("ProPresenter presentation exceeds 16 MiB"));
        }
        let mut document = Vec::new();
        entry
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut document)?;
        if document.len() > 16 * 1024 * 1024 {
            return Err(invalid("ProPresenter presentation exceeds 16 MiB"));
        }
        let name = entry.name();
        let show = if Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pro"))
        {
            super::propresenter7::decode(name, &document)?
        } else {
            xml_show(name, std::str::from_utf8(&document).map_err(invalid)?)?
        };
        shows.push((name.to_owned(), show));
    }
    if shows.is_empty() {
        return Err(invalid(
            "bundle contains no supported ProPresenter presentations",
        ));
    }
    Ok((archive, shows))
}

pub(super) fn import_bundle(bytes: &[u8], media: &Path) -> Result<Vec<ShowFile>, DataError> {
    let (mut archive, mut shows) = bundle_shows(bytes)?;
    let names: Vec<_> = archive.file_names().map(str::to_string).collect();
    // Resolve everything first: ambiguous or missing media must not silently
    // link to the wrong file, or fall back to an absolute path on this machine.
    let mut assets = BTreeMap::new();
    for (document, show) in &shows {
        for item in show.show["media"].as_object().unwrap().values() {
            let source = item["path"].as_str().unwrap();
            let name = find_asset(source, &names, document)?;
            assets.insert((document.clone(), source.to_string()), name.to_string());
        }
    }
    let mut copied = BTreeMap::<String, PathBuf>::new();
    for name in assets.values() {
        if copied.contains_key(name) {
            continue;
        }
        let mut entry = archive.by_name(name).map_err(invalid)?;
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(512 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 512 * 1024 * 1024 {
            return Err(invalid("bundled media exceeds 512 MiB"));
        }
        copied.insert(
            name.clone(),
            super::save_imported_media(media, name, &bytes)?,
        );
    }
    for (document, show) in &mut shows {
        for item in show.show["media"].as_object_mut().unwrap().values_mut() {
            let source = item["path"].as_str().unwrap();
            item["path"] =
                json!(copied[&assets[&(document.clone(), source.to_string())]].to_string_lossy());
        }
    }
    Ok(shows.into_iter().map(|(_, show)| show).collect())
}

fn source_basename(source: &str) -> String {
    source
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(source)
        .to_string()
}

fn find_asset<'a>(source: &str, names: &'a [String], document: &str) -> Result<&'a str, DataError> {
    // Some ProPresenter versions save file URLs with percent-escaped names.
    let mut decoded = Vec::new();
    let mut bytes = source.bytes();
    while let Some(byte) = bytes.next() {
        if source.starts_with("file:") && byte == b'%' {
            let hex = [
                bytes.next().ok_or_else(|| invalid("invalid media URL"))?,
                bytes.next().ok_or_else(|| invalid("invalid media URL"))?,
            ];
            let hex = std::str::from_utf8(&hex).map_err(invalid)?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(invalid)?);
        } else {
            decoded.push(byte);
        }
    }
    let source = String::from_utf8(decoded)
        .map_err(invalid)?
        .replace('\\', "/");
    if !source.starts_with('/') && !source.contains(':') {
        let document = document.replace('\\', "/");
        let mut relative: Vec<_> = document.split('/').collect();
        relative.pop();
        for part in source.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    relative.pop();
                }
                _ => relative.push(part),
            }
        }
        let relative = relative.join("/");
        if let Some(name) = names
            .iter()
            .find(|name| name.replace('\\', "/") == relative)
        {
            return Ok(name);
        }
    }
    let parts: Vec<_> = source.split('/').filter(|part| !part.is_empty()).collect();
    let mut best = 0;
    let mut matches = Vec::new();
    for name in names {
        if name.ends_with('/') {
            continue;
        }
        let normalized = name.replace('\\', "/");
        let score = parts
            .iter()
            .rev()
            .zip(normalized.split('/').rev())
            .take_while(|(a, b)| **a == *b)
            .count();
        if score > best {
            best = score;
            matches.clear();
        }
        if score == best && score > 0 {
            matches.push(name.as_str());
        }
    }
    if matches.len() != 1 {
        return Err(invalid(format!(
            "missing or ambiguous bundled media: {source}"
        )));
    }
    Ok(matches[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Library, SlideBackdrop};
    use std::fs;
    use std::io::Write;

    fn xml(source: &str) -> String {
        format!(
            r#"<RVPresentationDocument name="Sunday Song"><array rvXMLIvarName="groups">
            <RVSlideGrouping name="Verse"><array rvXMLIvarName="slides">
                <RVDisplaySlide notes="First note"><array rvXMLIvarName="displayElements"><RVTextElement>
                    <NSString rvXMLIvarName="PlainText">QW1hemluZyBncmFjZQpob3cgc3dlZXQ=</NSString>
                </RVTextElement></array></RVDisplaySlide>
                <RVDisplaySlide enabled="false"><RVMediaCue><RVVideoElement source="{source}"/></RVMediaCue>
                    <array rvXMLIvarName="displayElements"><RVTextElement><NSString>Praise God</NSString></RVTextElement></array>
                </RVDisplaySlide>
            </array></RVSlideGrouping>
            <RVSlideGrouping name="Chorus"><array><RVDisplaySlide/></array></RVSlideGrouping>
        </array></RVPresentationDocument>"#
        )
    }

    fn bundle(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn xml_preserves_group_children_disabled_slides_notes_and_backgrounds() {
        let show = xml_show("Song.pro6", &xml("C:\\Media\\sky.png")).unwrap();
        assert_eq!(show.name(), "Sunday Song");
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].lines, ["Amazing grace", "how sweet"]);
        assert_eq!(slides[1].lines, ["Praise God"]);
        assert_eq!(slides[2].group.as_deref(), Some("Chorus"));
        assert_eq!(show.show["slides"]["s1"]["notes"], "First note");
        assert_eq!(show.show["slides"]["s1"]["children"], json!(["s2"]));
        assert_eq!(show.step_enabled(0, 1), Some(2));
        assert_eq!(
            show.backdrop_at(1),
            SlideBackdrop::Picture {
                path: "C:\\Media\\sky.png".into(),
                foreground: false
            }
        );
    }

    #[test]
    fn text_decoding_prefers_rtf_and_keeps_unicode_and_escaped_characters() {
        // Pro4/5 also put RTFData directly on RVTextElement.
        let xml = r#"<RVPresentationDocument><slides><RVDisplaySlide><displayElements>
            <RVTextElement RTFData="{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\colortbl;\red255\green0\blue0;}\f0\fs48 Amazing\par grace \'e9 \u233? \u-10179?\u-8704? \\ \{yes\}}"/>
            <RVTextElement><NSString rvXMLIvarName="PlainText">V3Jvbmc=</NSString><NSString rvXMLIvarName="RTFData">e1xydGYxIFByYWlzZSBHb2R9</NSString></RVTextElement>
        </displayElements></RVDisplaySlide></slides></RVPresentationDocument>"#;
        let show = xml_show("Song.pro5", xml).unwrap();
        assert_eq!(
            show.ordered_slides()[0].lines,
            ["Amazing", "grace é é 😀 \\ {yes}", "Praise God"]
        );
        assert!(xml_show("bad.pro6", "<RVPresentationDocument><RVDisplaySlide>").is_err());
        assert!(xml_show("bad.pro6", "<RVPresentationDocument><RVDisplaySlide/>").is_err());
    }

    #[test]
    fn library_import_copies_bundle_media_and_keeps_existing_files_and_shows() {
        let root = std::env::temp_dir().join(format!("freeshow-probundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let library = Library::open(&root);
        fs::create_dir_all(library.imports_dir()).unwrap();
        fs::create_dir_all(library.media_dir()).unwrap();
        fs::write(library.media_dir().join("sky.png"), b"existing picture").unwrap();
        let picture = super::super::base64_decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNg+M8AAAICAQB7CYF4AAAAAElFTkSuQmCC").unwrap();
        let source = xml("file://localhost/old/Media/sky.png");
        let other = source.replace("Sunday Song", "Another Song");
        let bytes = bundle(&[
            ("Other/Song.pro5", other.as_bytes()),
            ("Presentations/Song.pro6", source.as_bytes()),
            ("Media/sky.png", &picture),
        ]);
        fs::write(library.imports_dir().join("Sunday.proBundle"), bytes).unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        let shows = library.list_shows().unwrap();
        assert_eq!(shows.len(), 2);
        assert_ne!(shows[0].id, shows[1].id);
        let SlideBackdrop::Picture { path, .. } = shows[0].backdrop_at(1) else {
            panic!("missing background")
        };
        assert!(Path::new(&path).starts_with(library.media_dir().canonicalize().unwrap()));
        assert_eq!(shows[0].backdrop_at(1), shows[1].backdrop_at(1));
        assert_eq!(fs::read(&path).unwrap(), picture);
        assert_eq!(
            fs::read(library.media_dir().join("sky.png")).unwrap(),
            b"existing picture"
        );
        let original_show = fs::read(shows[0].path.as_ref().unwrap()).unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        assert_eq!(fs::read_dir(library.media_dir()).unwrap().count(), 2);
        assert_eq!(
            fs::read(shows[0].path.as_ref().unwrap()).unwrap(),
            original_show
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relative_backgrounds_resolve_within_each_bundled_presentation() {
        let media = std::env::temp_dir().join(format!(
            "freeshow-relative-probundle-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&media);
        let first = xml("Media/sky.png");
        let second = first.replace("Sunday Song", "Second Song");
        let bytes = bundle(&[
            ("a/Song.pro6", first.as_bytes()),
            ("a/Media/sky.png", b"first background"),
            ("b/Song.pro6", second.as_bytes()),
            ("b/Media/sky.png", b"second background"),
        ]);
        let shows = import_bundle(&bytes, &media).unwrap();
        for (show, expected) in shows
            .iter()
            .zip([b"first background".as_slice(), b"second background"])
        {
            let SlideBackdrop::Picture { path, .. } = show.backdrop_at(1) else {
                panic!("missing background")
            };
            assert_eq!(fs::read(path).unwrap(), expected);
        }
        fs::remove_dir_all(media).unwrap();
    }

    #[test]
    fn protobuf_bundle_keeps_selected_arrangement_and_copies_its_background() {
        let presentation = include_bytes!("../../tests/fixtures/propresenter7.pro");
        let texture = include_bytes!("../../tests/fixtures/lesson-cover.png");
        let standalone = crate::import::import_show("Sunday.pro", presentation).unwrap();
        assert!(matches!(
            standalone.backdrop_at(0),
            SlideBackdrop::Picture { path, .. } if path.ends_with("sky blue.png") && !path.starts_with("file:")
        ));

        let archive = bundle(&[
            ("Presentation/Sunday.pro", presentation),
            ("Media/sky blue.png", texture),
        ]);
        let root = std::env::temp_dir().join(crate::new_id());
        let library = crate::Library::open(&root);
        fs::create_dir_all(library.imports_dir()).unwrap();
        fs::write(library.imports_dir().join("Sunday.proBundle"), archive).unwrap();
        assert!(library.import_show_sources().unwrap().is_empty());
        let mut show = library.list_shows().unwrap().remove(0);
        assert_eq!(show.id, "test-pro7");
        assert_eq!(show.name(), "Test Presentation");
        assert_eq!(show.active_layout(), "arrangement-2");
        assert_eq!(
            show.show["layouts"]["arrangement-2"]["notes"],
            "Service notes"
        );
        assert_eq!(show.show["meta"]["CCLI"], "123");
        let slides = show.ordered_slides();
        assert_eq!(slides.len(), 4);
        assert_eq!(slides[0].lines, ["Chorus"]);
        assert_eq!(slides[0].id, slides[3].id);
        assert_eq!(slides[1].notes, "Leader note");
        assert_eq!(
            slides[1].lines,
            ["Verse", "And another line", "Another text box"]
        );
        assert_eq!(slides[2].lines, ["Disabled child"]);
        assert_eq!(show.step_enabled(1, 1), Some(3));

        let verse = &show.show["slides"][&slides[1].id];
        assert_eq!(verse["settings"]["color"], "#000040ff");
        assert_eq!(
            verse["settings"]["resolution"],
            json!({"width":1280.0,"height":720.0})
        );
        assert_eq!(verse["items"].as_array().unwrap().len(), 2);
        let style = verse["items"][0]["style"].as_str().unwrap();
        for expected in [
            "left:45px",
            "top:60px",
            "width:740px",
            "height:210px",
            "font-family:Arial",
            "font-size:72px",
            "font-weight:bold",
            "font-style:italic",
            "color:#00ff00ff",
            "text-align:right",
            "align-items:flex-end",
        ] {
            assert!(style.contains(expected), "{style}");
        }

        let SlideBackdrop::Picture { path, .. } = show.backdrop_at(0) else {
            panic!("missing bundle background");
        };
        assert!(Path::new(&path).starts_with(library.media_dir().canonicalize().unwrap()));
        assert_eq!(fs::read(path).unwrap(), texture);
        assert!(show.set_active_layout("arrangement-1"));
        assert_eq!(show.ordered_slides()[0].group.as_deref(), Some("Verse 1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundle_rejects_unsafe_entries_missing_media_and_malformed_presentations() {
        let source = xml("missing.png");
        let media =
            std::env::temp_dir().join(format!("freeshow-invalid-probundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&media);
        for name in [
            "../escape.png",
            "/absolute.png",
            "C:\\escape.png",
            "media/../../escape.png",
        ] {
            let bytes = bundle(&[("Song.pro6", source.as_bytes()), (name, b"bad")]);
            assert!(import_bundle(&bytes, &media).is_err(), "{name}");
        }
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .add_symlink(
                "sky.png",
                "/outside",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        assert!(bundle_shows(&archive.finish().unwrap().into_inner()).is_err());
        assert!(import_bundle(&bundle(&[("Song.pro6", source.as_bytes())]), &media).is_err());
        assert!(import_bundle(&bundle(&[("Song.pro", b"protobuf")]), &media).is_err());
        assert!(!media.exists());
        assert_eq!(
            find_asset("literal%.png", &["Media/literal%.png".into()], "Song.pro6").unwrap(),
            "Media/literal%.png"
        );
        let names = vec!["a/sky.png".to_string(), "b/sky.png".to_string()];
        assert!(find_asset("/old/sky.png", &names, "Song.pro6").is_err());
        assert_eq!(
            find_asset("C:\\old\\b\\sky.png", &names, "Song.pro6").unwrap(),
            "b/sky.png"
        );
        assert_eq!(
            find_asset(
                "file:///old/sky%20blue.png",
                &["Media/sky blue.png".into()],
                "Song.pro6"
            )
            .unwrap(),
            "Media/sky blue.png"
        );
    }
}
