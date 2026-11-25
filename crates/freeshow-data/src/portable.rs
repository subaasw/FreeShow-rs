// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{Catalog, DataError, ProjectsFile, ShowFile, atomic_write, new_id};

pub(crate) const MAX_PACKAGE_BYTES: u64 = 768 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 10_000;

fn package_error(message: impl Into<String>) -> DataError {
    DataError::Scripture(message.into())
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or("")
}

fn safe_name(path: &str) -> String {
    let name: String = basename(path)
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();
    let name = name.to_ascii_lowercase();
    let name = name.trim_matches('.');
    let name = name.strip_prefix("asset-").unwrap_or(name);
    // Leave room for numeric collision suffixes on the destination filesystem.
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let stem = &stem[..stem.len().min(96)];
    if ext.is_empty() {
        format!("asset-{stem}")
    } else {
        format!("asset-{stem}.{}", &ext[..ext.len().min(16)])
    }
}

/// Electron FreeShow uses a signed UTF-16 path hash to disambiguate duplicate basenames.
fn electron_media_name(source: &str) -> String {
    let name = basename(source);
    let hash = source.encode_utf16().fold(0i32, |sum, unit| {
        sum.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    let prefix = if hash < 0 { 'i' } else { 'a' };
    let value = hash.unsigned_abs();
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}__{prefix}{value}.{ext}"),
        None => format!("{name}__{prefix}{value}"),
    }
}

fn legacy_external(reference: &str) -> bool {
    reference.contains("://") || reference.starts_with("camera:") || reference.starts_with("data:")
}

fn dependency_warning(reference: &str, label: &str) -> String {
    if reference.starts_with("freeshow-protected://") {
        format!("Protected media cannot be bundled: {label}")
    } else {
        format!("External media: {label}")
    }
}

fn numbered(name: &str, n: usize) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{n}.{ext}"),
        None => format!("{name}-{n}"),
    }
}

fn project_row_kind(row: &Value) -> &str {
    row["type"].as_str().unwrap_or("show")
}

fn media_row(kind: &str) -> bool {
    matches!(
        kind,
        "image" | "video" | "audio" | "pdf" | "ppt" | "file" | "folder" | "player"
    )
}

pub(crate) fn walk_media(value: &mut Value, visit: &mut impl FnMut(&mut Value)) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if crate::media_library::media_reference_key(key) && value.is_string() {
                    visit(value);
                } else {
                    walk_media(value, visit);
                }
            }
        }
        Value::Array(rows) => {
            for value in rows {
                walk_media(value, visit);
            }
        }
        _ => {}
    }
}

fn package_media(payload: &mut Value, mut visit: impl FnMut(&mut Value)) {
    if let Some(rows) = payload["project"]["shows"].as_array_mut() {
        for row in rows {
            if media_row(project_row_kind(row)) && row["id"].is_string() {
                visit(&mut row["id"]);
            }
        }
    }
    for key in ["shows", "overlays", "templates"] {
        if let Some(entries) = payload.get_mut(key) {
            walk_media(entries, &mut visit);
        }
    }
}

const SHOW_REFS: &[&str] = &["show", "showId", "showID", "startShow", "start_show"];
const OVERLAY_REFS: &[&str] = &[
    "overlays",
    "overlay",
    "overlayId",
    "id_select_overlay",
    "clear_overlay",
];
const TEMPLATE_REFS: &[&str] = &[
    "template",
    "templateId",
    "firstSlideTemplate",
    "set_template",
];

fn reference_ids(value: &Value, keys: &[&str], ids: &mut BTreeSet<String>) {
    fn gather(value: &Value, ids: &mut BTreeSet<String>) {
        if let Some(id) = value
            .as_str()
            .or_else(|| value["id"].as_str())
            .filter(|id| !id.is_empty())
        {
            ids.insert(id.into());
        }
        if let Some(values) = value.as_array() {
            for value in values {
                gather(value, ids);
            }
        }
    }
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if keys.contains(&key.as_str()) {
                    gather(value, ids);
                }
                reference_ids(value, keys, ids);
            }
        }
        Value::Array(rows) => {
            for value in rows {
                reference_ids(value, keys, ids);
            }
        }
        _ => {}
    }
}

pub(crate) fn project_shows(
    project: &Value,
    available: &[ShowFile],
) -> serde_json::Map<String, Value> {
    let mut pending = BTreeSet::new();
    if let Some(rows) = project["shows"].as_array() {
        pending.extend(
            rows.iter()
                .filter(|row| matches!(project_row_kind(row), "show" | ""))
                .filter_map(|row| row["id"].as_str())
                .map(str::to_owned),
        );
    }
    let mut selected = serde_json::Map::new();
    let mut examined = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if examined.insert(id.clone())
            && let Some(show) = available.iter().find(|show| show.id == id)
        {
            reference_ids(&show.show, SHOW_REFS, &mut pending);
            selected.insert(id, show.show.clone());
        }
    }
    selected
}

/// Only copy assets that the selected project and its design entries declare.
pub(crate) fn include_designs(payload: &mut Value, overlays: &Catalog, templates: &Catalog) {
    let mut overlay_ids = BTreeSet::new();
    let mut template_ids = BTreeSet::new();
    if let Some(rows) = payload["project"]["shows"].as_array() {
        for row in rows {
            if project_row_kind(row) == "overlay"
                && let Some(id) = row["id"].as_str()
            {
                overlay_ids.insert(id.into());
            }
        }
    }
    for value in [payload.get("shows"), payload.get("templates")]
        .into_iter()
        .flatten()
    {
        reference_ids(value, OVERLAY_REFS, &mut overlay_ids);
        reference_ids(value, TEMPLATE_REFS, &mut template_ids);
    }
    for id in &template_ids {
        if let Some(template) = templates.get(id) {
            reference_ids(template, OVERLAY_REFS, &mut overlay_ids);
        }
    }
    for (key, ids, catalog) in [
        ("overlays", overlay_ids, overlays),
        ("templates", template_ids, templates),
    ] {
        for id in ids {
            if let Some(entry) = catalog.get(&id) {
                payload[key][id] = entry.clone();
            }
        }
    }
}

fn local_asset(root: &Path, reference: &str) -> Option<PathBuf> {
    let portable = reference.replace('\\', "/");
    let original = Path::new(&portable);
    let extension = original.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "webp"
            | "svg"
            | "avif"
            | "heic"
            | "mp4"
            | "m4v"
            | "mov"
            | "webm"
            | "mkv"
            | "avi"
            | "mpg"
            | "mpeg"
            | "wmv"
            | "m2ts"
            | "mp3"
            | "wav"
            | "ogg"
            | "flac"
            | "m4a"
            | "aac"
            | "opus"
            | "aiff"
            | "aif"
            | "pdf"
            | "ppt"
            | "pptx"
    ) {
        return None;
    }
    [
        root.join(original),
        root.join("Media").join(original),
        root.join("Audio").join(original),
    ]
    .into_iter()
    .find(|file| file.is_file())
}

/// Writes the documented FreeShow data.json format and records sources that stay external.
pub(crate) fn write_archive(
    mut payload: Value,
    library: &Path,
    dest: &Path,
) -> Result<Vec<String>, DataError> {
    let mut references = BTreeSet::new();
    package_media(&mut payload, |value| {
        if let Some(reference) = value
            .as_str()
            .filter(|s| !s.is_empty() && !s.starts_with("data:"))
        {
            references.insert(reference.to_owned());
        }
    });
    let mut assets: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut mapped = BTreeMap::new();
    let mut warnings = Vec::new();
    if let Some(rows) = payload["project"]["shows"].as_array() {
        for row in rows {
            if let Some(id) = row["id"].as_str() {
                let catalog = match project_row_kind(row) {
                    "show" | "" => "shows",
                    "overlay" => "overlays",
                    _ => continue,
                };
                if payload[catalog].get(id).is_none() {
                    warnings.push(format!("Missing {catalog} entry: {id}"));
                }
            }
        }
    }
    let mut size = 0;
    for reference in references {
        let label = basename(reference.split(['?', '#']).next().unwrap_or(&reference));
        if legacy_external(&reference) {
            warnings.push(dependency_warning(&reference, label));
            continue;
        }
        let Some(path) = local_asset(library, &reference) else {
            warnings.push(format!("Missing or unavailable media: {label}"));
            continue;
        };
        let file = File::open(&path)?;
        let remaining = MAX_PACKAGE_BYTES - size;
        if file.metadata()?.len() > remaining || assets.len() >= MAX_ENTRIES - 1 {
            return Err(package_error("Project media exceeds the package limit"));
        }
        let mut bytes = Vec::new();
        file.take(remaining + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > remaining {
            return Err(package_error("Project media exceeds the package limit"));
        }
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = if let Some((name, _)) = assets.iter().find(|(name, data)| {
            name.rsplit_once('.')
                .is_some_and(|(_, ext)| ext == extension)
                && *data == &bytes
        }) {
            name.clone()
        } else {
            let name = safe_name(&reference);
            let mut candidate = name.clone();
            let mut n = 2;
            while assets
                .keys()
                .any(|key| key.eq_ignore_ascii_case(&candidate))
            {
                candidate = numbered(&name, n);
                n += 1;
            }
            size += bytes.len() as u64;
            assets.insert(candidate.clone(), bytes);
            candidate
        };
        mapped.insert(reference, name);
    }
    package_media(&mut payload, |value| {
        if let Some(name) = value.as_str().and_then(|path| mapped.get(path)) {
            *value = Value::String(name.clone());
        }
    });
    payload["files"] = json!(assets.keys().collect::<Vec<_>>());
    if !warnings.is_empty() {
        payload["freeshowRustWarnings"] = json!(warnings);
    }
    let data = serde_json::to_vec_pretty(&payload)?;
    if data.len() as u64 > MAX_DOCUMENT_BYTES || data.len() as u64 + size > MAX_PACKAGE_BYTES {
        return Err(package_error("Project metadata exceeds the package limit"));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() && !dest.is_file() {
        return Err(package_error("The export destination is not a file"));
    }
    let staging = dest.with_extension(format!("{}.part", new_id()));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive
            .start_file("data.json", options)
            .map_err(|err| package_error(err.to_string()))?;
        archive.write_all(&data)?;
        for (name, bytes) in assets {
            archive
                .start_file(name, options)
                .map_err(|err| package_error(err.to_string()))?;
            archive.write_all(&bytes)?;
        }
        archive
            .finish()
            .map_err(|err| package_error(err.to_string()))?
            .sync_all()?;
        crate::replace_file(&staging, dest)?;
        Ok(warnings)
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

fn assign_id(id: &str, identical: bool, occupied: impl Fn(&str) -> bool) -> String {
    if identical || (!id.is_empty() && !occupied(id)) {
        return id.to_owned();
    }
    loop {
        let id = new_id();
        if !occupied(&id) {
            return id;
        }
    }
}

fn remap_reference(value: &mut Value, ids: &BTreeMap<String, String>) {
    if let Some(id) = value.as_str().and_then(|id| ids.get(id)) {
        *value = Value::String(id.clone());
    } else if let Some(id) = value.get_mut("id") {
        remap_reference(id, ids);
    } else if let Some(list) = value.as_array_mut() {
        for value in list {
            remap_reference(value, ids);
        }
    }
}

fn remap(value: &mut Value, keys: &[&str], ids: &BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if keys.contains(&key.as_str()) {
                    remap_reference(value, ids);
                }
                remap(value, keys, ids);
            }
        }
        Value::Array(rows) => {
            for row in rows {
                remap(row, keys, ids);
            }
        }
        _ => {}
    }
}

/// Prepare all references and destinations before publishing anything, then write the project last.
pub(crate) fn install(
    mut payload: Value,
    bundled: &[(String, Vec<u8>)],
    name: &str,
    shows_dir: &Path,
    media_dir: &Path,
) -> Result<Vec<String>, DataError> {
    let root = shows_dir.parent().unwrap_or(shows_dir);
    if !payload["project"].is_object() {
        return Err(package_error("Project metadata is missing"));
    }
    let mut export_warnings = Vec::new();
    if let Some(rows) = payload["freeshowRustWarnings"].as_array() {
        export_warnings.extend(rows.iter().filter_map(Value::as_str).map(str::to_string));
    }
    let mut files = Vec::new();
    let requires_bundles = payload.get("files").is_some();
    if let Some(rows) = payload.get("files") {
        let rows = rows
            .as_array()
            .ok_or_else(|| package_error("Project files must be a list"))?;
        for row in rows {
            files.push(
                row.as_str()
                    .ok_or_else(|| package_error("Invalid project media entry"))?
                    .to_owned(),
            );
        }
    } else if let Some(rows) = payload["media"].as_object() {
        files.extend(rows.keys().cloned());
    }
    let mut paths = BTreeMap::new();
    let mut new_files: Vec<(PathBuf, &[u8])> = Vec::new();
    for source in files {
        let file = basename(&source);
        if file.is_empty() || matches!(file, "." | "..") {
            return Err(package_error("Invalid project media name"));
        }
        if legacy_external(&source) {
            if !source.starts_with("data:") {
                let label = basename(source.split(['?', '#']).next().unwrap_or(&source));
                let warning = dependency_warning(&source, label);
                if !export_warnings.contains(&warning) {
                    export_warnings.push(warning);
                }
            }
            continue;
        }
        let original = electron_media_name(&source);
        let linux = electron_media_name(&source.replace('\\', "/"));
        let windows = electron_media_name(&source.replace('/', "\\"));
        let mut hashed = bundled.iter().filter(|(entry, _)| {
            let name = basename(entry);
            name == original || name == linux || name == windows
        });
        let mut plain = bundled.iter().filter(|(entry, _)| basename(entry) == file);
        let exact = hashed.next();
        let (entry, ambiguous) = if let Some(entry) = exact {
            (Some(entry), hashed.next().is_some())
        } else {
            (plain.next(), plain.next().is_some())
        };
        if ambiguous {
            return Err(package_error(format!(
                "Package has ambiguous media named {file}"
            )));
        }
        let Some((_, bytes)) = entry else {
            if requires_bundles {
                return Err(package_error(format!("Package is missing {file}")));
            }
            export_warnings.push(format!("Media was not bundled: {file}"));
            continue;
        };
        let category = if matches!(
            Path::new(file)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus" | "aiff" | "aif"
        ) {
            "Audio"
        } else {
            "Media"
        };
        let directory = if category == "Media" {
            media_dir.to_path_buf()
        } else {
            root.join("Audio")
        };
        let candidate = safe_name(file);
        let mut dest = directory.join(&candidate);
        let mut n = 2;
        loop {
            if let Some((_, prior)) = new_files.iter().find(|(path, _)| *path == dest) {
                if *prior == bytes {
                    break;
                }
            } else {
                match fs::symlink_metadata(&dest) {
                    Ok(meta)
                        if meta.is_file()
                            && meta.len() == bytes.len() as u64
                            && fs::read(&dest)? == *bytes =>
                    {
                        break;
                    }
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        new_files.push((dest.clone(), bytes));
                        break;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            dest = directory.join(numbered(&candidate, n));
            n += 1;
        }
        // The existing decoder and follow-media cues compare native filesystem paths.
        let native = std::path::absolute(&dest)?;
        let local = native
            .to_str()
            .ok_or_else(|| package_error("Library media path is not valid UTF-8"))?;
        paths.insert(source.clone(), local.to_owned());
        paths
            .entry(file.to_owned())
            .or_insert_with(|| local.to_owned());
    }
    package_media(&mut payload, |value| {
        if let Some(mapped) = value.as_str().and_then(|path| paths.get(path)) {
            *value = Value::String(mapped.clone());
        }
    });
    let overlays_path = root.join("Config/overlays.json");
    let templates_path = root.join("Config/templates.json");
    let projects_path = root.join("Config/projects.json");
    let mut overlays = Catalog::load(&overlays_path)?;
    let mut templates = Catalog::load(&templates_path)?;
    let before_overlays = overlays.clone();
    let before_templates = templates.clone();
    let mut overlay_ids = BTreeMap::new();
    let mut template_ids = BTreeMap::new();
    for (key, catalog, mapping) in [
        ("overlays", &mut overlays, &mut overlay_ids),
        ("templates", &mut templates, &mut template_ids),
    ] {
        if let Some(entries) = payload[key].as_object() {
            for (id, value) in entries {
                let imported = assign_id(id, catalog.get(id) == Some(value), |id| {
                    catalog.get(id).is_some()
                });
                catalog.insert(&imported, value.clone());
                mapping.insert(id.clone(), imported);
            }
        }
    }
    let mut library_shows = BTreeMap::new();
    if shows_dir.exists() {
        for entry in fs::read_dir(shows_dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "show")
                && let Ok(show) = ShowFile::load(&path)
            {
                library_shows.insert(show.id, show.show);
            }
        }
    }
    let mut show_ids = BTreeMap::new();
    let mut imported_shows = Vec::new();
    if let Some(entries) = payload["shows"].as_object() {
        for (id, value) in entries {
            if !value.is_object() {
                return Err(package_error("Package has an invalid show"));
            }
            let mut value = value.clone();
            remap(&mut value, TEMPLATE_REFS, &template_ids);
            remap(&mut value, OVERLAY_REFS, &overlay_ids);
            let identical = library_shows.get(id) == Some(&value);
            let imported = assign_id(id, identical, |id| library_shows.contains_key(id));
            show_ids.insert(id.clone(), imported.clone());
            if !identical {
                library_shows.insert(imported.clone(), value.clone());
                imported_shows.push(ShowFile {
                    id: imported,
                    show: value,
                    path: None,
                });
            }
        }
    }
    let mut project = payload["project"].clone();
    if let Some(rows) = project["shows"].as_array_mut() {
        for row in rows {
            let mapping = match project_row_kind(row) {
                "show" | "" => &show_ids,
                "overlay" => &overlay_ids,
                _ => continue,
            };
            if let Some(id) = row["id"].as_str().and_then(|id| mapping.get(id)) {
                row["id"] = Value::String(id.clone());
            }
        }
    }
    let id = project
        .as_object_mut()
        .unwrap()
        .remove("id")
        .and_then(|id| id.as_str().map(str::to_owned))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| name.to_owned());
    project["parent"] = json!("/");
    if !project["name"].is_string() {
        project["name"] = json!(name);
    }
    let projects = if payload["freeshowRustTemplateOnly"] == true {
        None
    } else {
        let mut projects = ProjectsFile::load(&projects_path)?;
        let current = projects.project_body(&id);
        let project_id = assign_id(&id, current.as_ref() == Some(&project), |id| {
            projects.project_body(id).is_some()
        });
        if !projects.put_project(&project_id, project) {
            return Err(package_error("Could not register the imported project"));
        }
        Some(projects)
    };

    let mut created = Vec::new();
    let mut replaced = Vec::new();
    let result = (|| {
        for (path, bytes) in new_files {
            write_new(&path, bytes, &mut created)?;
        }
        for mut show in imported_shows {
            remap(&mut show.show, SHOW_REFS, &show_ids);
            let mut path = shows_dir.join(format!("{}.show", basename(&show.id)));
            if path.exists()
                || !show
                    .id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
            {
                path = shows_dir.join(format!("{}.show", new_id()));
            }
            write_new(&path, show.to_json()?.as_bytes(), &mut created)?;
        }
        for (changed, path, catalog) in [
            (overlays != before_overlays, overlays_path, &overlays),
            (templates != before_templates, templates_path, &templates),
        ] {
            if changed {
                let previous = match fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                    Err(err) => return Err(err.into()),
                };
                catalog.save(&path)?;
                replaced.push((path, previous));
            }
        }
        if let Some(projects) = projects {
            projects.save(&projects_path)?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let mut failures = Vec::new();
        for (path, previous) in replaced.into_iter().rev() {
            let restored = match previous {
                Some(bytes) => atomic_write(&path, &bytes),
                None => fs::remove_file(&path).map_err(DataError::from),
            };
            if let Err(rollback) = restored {
                failures.push(format!("could not restore {}: {rollback}", path.display()));
            }
        }
        for path in created.into_iter().rev() {
            if let Err(rollback) = fs::remove_file(&path) {
                failures.push(format!("could not remove {}: {rollback}", path.display()));
            }
        }
        return if failures.is_empty() {
            Err(err)
        } else {
            Err(package_error(format!(
                "{err}. Rollback: {}",
                failures.join(", ")
            )))
        };
    }
    Ok(export_warnings)
}

fn write_new(path: &Path, bytes: &[u8], created: &mut Vec<PathBuf>) -> Result<(), DataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    created.push(path.to_path_buf());
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;
    use std::io::Cursor;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("freeshow-portable-{}", new_id()));
            fs::create_dir_all(root.join("Imports")).unwrap();
            Self(root)
        }

        fn import(&self) -> Vec<String> {
            crate::import::import_sources(
                &self.0.join("Imports"),
                &self.0.join("Shows"),
                &self.0.join("Media"),
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn offline_project_round_trip_deduplicates_assets_and_rolls_back_on_save_failure() {
        let source = Fixture::new();
        let source_library = Library::open(&source.0);
        for (path, bytes) in [
            ("Media/a/banner.png", b"first".as_slice()),
            ("Media/b/banner.png", b"second".as_slice()),
            ("Media/c/dupe.png", b"second".as_slice()),
        ] {
            let path = source.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let mut show = ShowFile::blank("shared", "Packaged show");
        show.show["settings"]["template"] = json!("poster");
        show.show["slides"]["s1"]["items"] = json!([
            {"type":"media","src":"Media/a/banner.png"},
            {"type":"media","src":"Media/b/banner.png"},
            {"type":"media","src":"Media/c/dupe.png"},
            {"type":"media","src":"Media/missing.png"},
            {"type":"media","src":"https://example.org/clip.mp4?key=secret"}
        ]);
        show.show["layouts"]["main"]["slides"][0]["actions"]["startShow"] = json!({"id":"linked"});
        let linked = ShowFile::blank("linked", "Linked dependency");
        let mut templates = Catalog::empty();
        templates.insert(
            "poster",
            json!({"name":"Poster","items":[{"type":"media","src":"Media/c/dupe.png"}]}),
        );
        templates.insert(
            "unused",
            json!({"name":"Do not share","items":[{"type":"media","src":"private.png"}]}),
        );
        templates.save(&source_library.templates_path()).unwrap();
        let mut overlays = Catalog::empty();
        overlays.insert("lower-third", json!({"name":"Lower third","items":[]}));
        overlays.save(&source_library.overlays_path()).unwrap();
        let project = json!({"name":"Portable","parent":"sender-folder","shows":[
            {"id":"shared"}, {"id":"Media/a/banner.png","type":"image"},
            {"id":"lower-third","type":"overlay"}
        ]});
        let mut source_projects = ProjectsFile::empty();
        assert!(source_projects.put_project("sunday", project.clone()));

        let destination = Fixture::new();
        let archive = destination.0.join("Imports/Portable.project");
        let warnings = crate::write_portable_project(
            &source_projects.projects()[0],
            &project,
            &[show, linked],
            &source_library,
            &archive,
        )
        .unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(!warnings.join(" ").contains("secret"));
        let bytes = fs::read(&archive).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 3);
        let payload: Value = serde_json::from_reader(zip.by_name("data.json").unwrap()).unwrap();
        assert_eq!(payload["files"].as_array().unwrap().len(), 2);
        assert!(payload["templates"].get("unused").is_none());

        let library = Library::open(&destination.0);
        let original = ShowFile::blank("shared", "Receiving show");
        original
            .save(&library.shows_dir().join("My show.show"))
            .unwrap();
        let receiving_linked = ShowFile::blank("linked", "Receiving dependency");
        receiving_linked
            .save(&library.shows_dir().join("My dependency.show"))
            .unwrap();
        let mut projects = ProjectsFile::empty();
        assert!(projects.put_project(
            "sunday",
            json!({"name":"Receiving project","shows":[{"id":"shared"}]})
        ));
        projects.save(&library.projects_path()).unwrap();
        let mut receiving = Catalog::empty();
        receiving.insert("poster", json!({"name":"Receiving poster","items":[]}));
        receiving.save(&library.templates_path()).unwrap();
        fs::create_dir_all(library.media_dir()).unwrap();
        let preexisting = library.media_dir().join("asset-banner.png");
        fs::write(&preexisting, b"receiving").unwrap();

        // Force the last atomic write to fail after the media, shows and catalogs have been written.
        let blocker = library.projects_path().with_file_name("projects.json.tmp");
        fs::create_dir(&blocker).unwrap();
        let rejected = destination.import();
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(archive.exists());
        assert_eq!(library.list_shows().unwrap().len(), 2);
        assert_eq!(fs::read_dir(library.media_dir()).unwrap().count(), 1);
        assert_eq!(Catalog::load(&library.templates_path()).unwrap(), receiving);
        assert!(
            Catalog::load(&library.overlays_path())
                .unwrap()
                .names()
                .is_empty()
        );
        assert_eq!(
            ProjectsFile::load(&library.projects_path()).unwrap(),
            projects
        );

        fs::remove_dir(blocker).unwrap();
        assert_eq!(destination.import(), warnings);
        assert!(!archive.exists());
        assert_eq!(fs::read(&preexisting).unwrap(), b"receiving");
        let projects = ProjectsFile::load(&library.projects_path()).unwrap();
        let imported = projects
            .projects()
            .into_iter()
            .find(|row| row.name == "Portable")
            .unwrap();
        assert_ne!(imported.id, "sunday");
        assert_eq!(imported.parent, "/");
        assert_ne!(imported.shows[0].id, "shared");
        let shows = library.list_shows().unwrap();
        assert!(
            shows
                .iter()
                .any(|show| show.id == "shared" && show.show == original.show)
        );
        let show = shows
            .iter()
            .find(|show| show.id == imported.shows[0].id)
            .unwrap();
        let linked_id = show.started_show_at(0).unwrap();
        assert_ne!(linked_id, "linked");
        assert!(
            shows
                .iter()
                .any(|show| show.id == linked_id && show.name() == "Linked dependency")
        );
        let items = &show.show["slides"]["s1"]["items"];
        let first = items[0]["src"].as_str().unwrap();
        let second = items[1]["src"].as_str().unwrap();
        assert_eq!(first, imported.shows[1].id);
        assert_eq!(fs::read(first).unwrap(), b"first");
        assert_eq!(fs::read(second).unwrap(), b"second");
        assert_eq!(items[1]["src"], items[2]["src"]);
        let remapped = show.show["settings"]["template"].as_str().unwrap();
        assert_ne!(remapped, "poster");
        let catalog = Catalog::load(&library.templates_path()).unwrap();
        assert_eq!(catalog.get(remapped).unwrap()["items"][0]["src"], second);
        assert_eq!(catalog.get("poster"), receiving.get("poster"));
    }

    #[test]
    fn an_editable_template_imports_without_creating_a_project() {
        let source = Fixture::new();
        let destination = Fixture::new();
        fs::create_dir_all(source.0.join("Media")).unwrap();
        fs::write(source.0.join("Media/art.png"), b"artwork").unwrap();
        let template = json!({"name":"Editable","settings":{"backgroundColor":"#123456"},"items":[
            {"type":"media","src":"Media/art.png","custom":{"keep":true}},
            {"type":"text","lines":[{"text":[{"value":"Edit me"}]}]}
        ]});
        crate::write_portable_template(
            "art",
            &template,
            &Library::open(&source.0),
            &destination.0.join("Imports/Editable.project"),
        )
        .unwrap();
        assert!(destination.import().is_empty());
        let library = Library::open(&destination.0);
        assert!(!library.projects_path().exists());
        let templates = Catalog::load(&library.templates_path()).unwrap();
        let entry = templates.get("art").unwrap();
        let editable = ShowFile::from_template("copy", "Copy", entry).unwrap();
        assert_eq!(editable.item_text("s1", 1).as_deref(), Some("Edit me"));
        assert_eq!(
            fs::read(entry["items"][0]["src"].as_str().unwrap()).unwrap(),
            b"artwork"
        );
        assert_eq!(
            editable.show["slides"]["s1"]["items"][0]["custom"]["keep"],
            true
        );
    }

    #[test]
    fn reference_freeshow_archives_distinguish_windows_paths_with_the_same_basename() {
        let dest = Fixture::new();
        let payload = json!({
            "project": {"name": "Electron", "shows":[
                {"type":"image", "id":r"C:\Stage\photo.png"},
                {"type":"image", "id":r"D:\Deck\photo.png"}
            ]},
            "files": [r"C:\Stage\photo.png", r"D:\Deck\photo.png", "freeshow-protected://art/demo.png"]
        });
        let files = [
            ("photo__a299541354.png".into(), b"first image".to_vec()),
            ("photo__i1196274368.png".into(), b"second image".to_vec()),
        ];
        let report = install(
            payload,
            &files,
            "electron",
            &dest.0.join("Shows"),
            &dest.0.join("Media"),
        )
        .unwrap();
        assert_eq!(report, ["Protected media cannot be bundled: demo.png"]);
        let projects = ProjectsFile::load(&Library::open(&dest.0).projects_path()).unwrap();
        let project = &projects.projects()[0];
        assert_eq!(fs::read(&project.shows[0].id).unwrap(), b"first image");
        assert_eq!(fs::read(&project.shows[1].id).unwrap(), b"second image");
    }

    #[test]
    fn documented_media_paths_from_windows_install_under_local_library() {
        let dest = Fixture::new();
        let payload = json!({
            "project": {"name": "Windows sample", "shows":[{"type":"image", "id":"C:\\Stage\\photo.png"}]},
            "files": ["C:\\Stage\\photo.png"]
        });
        let files = [("photo.png".into(), b"from the archive".to_vec())];
        assert!(
            install(
                payload,
                &files,
                "windows",
                &dest.0.join("Shows"),
                &dest.0.join("Media")
            )
            .unwrap()
            .is_empty()
        );
        let projects = ProjectsFile::load(&Library::open(&dest.0).projects_path()).unwrap();
        let paths = projects.projects();
        let path = Path::new(&paths[0].shows[0].id);
        assert!(path.starts_with(dest.0.join("Media")));
        assert_eq!(fs::read(path).unwrap(), b"from the archive");
    }
}
