// SPDX-License-Identifier: GPL-3.0-only

//! Writes print/PDF lyrics and FreeShow project packages.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::{Catalog, DataError, ProjectEntry, ProjectsFile, ShowFile};

/// A zip FreeShow can import: `{project, shows, overlays}` in `project.json`.
pub fn write_project_package(
    project: &ProjectEntry,
    project_body: &Value,
    shows: &[ShowFile],
    overlays: Option<&Catalog>,
    dest: &Path,
) -> Result<(), DataError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut shows_map = serde_json::Map::new();
    for show in shows {
        shows_map.insert(show.id.clone(), show.show.clone());
    }
    let mut payload = json!({
        "project": package_project(project, project_body),
        "shows": shows_map,
    });
    if let Some(catalog) = overlays
        && let Ok(Value::Object(map)) = catalog
            .to_json()
            .map(|text| serde_json::from_str::<Value>(&text).unwrap_or(Value::Null))
    {
        payload["overlays"] = Value::Object(map);
    }
    let json = serde_json::to_vec_pretty(&payload)?;
    let file = File::create(dest)?;
    let mut zip = ZipWriter::new(file);
    zip.start_file("project.json", SimpleFileOptions::default())
        .map_err(|err| DataError::Scripture(err.to_string()))?;
    zip.write_all(&json)?;
    zip.finish()
        .map_err(|err| DataError::Scripture(err.to_string()))?;
    Ok(())
}

/// A portable FreeShow project with local media and reusable templates.
pub fn write_portable_project(
    project: &ProjectEntry,
    project_body: &Value,
    shows: &[ShowFile],
    library: &crate::Library,
    dest: &Path,
) -> Result<Vec<String>, DataError> {
    let shows = crate::portable::project_shows(project_body, shows);
    let overlays = Catalog::load(&library.overlays_path())?;
    let templates = Catalog::load(&library.templates_path())?;
    let mut payload = json!({
        "project": package_project(project, project_body),
        "shows": shows,
    });
    crate::portable::include_designs(&mut payload, &overlays, &templates);
    crate::portable::write_archive(payload, library.root(), dest)
}

/// Shares one editable template and its media through the ordinary package importer.
pub fn write_portable_template(
    id: &str,
    template: &Value,
    library: &crate::Library,
    dest: &Path,
) -> Result<Vec<String>, DataError> {
    let mut payload = json!({
        "project": { "id": id, "name": template["name"], "shows": [] },
        "freeshowRustTemplateOnly": true,
    });
    payload["templates"][id] = template.clone();
    let overlays = Catalog::load(&library.overlays_path())?;
    let templates = Catalog::load(&library.templates_path())?;
    crate::portable::include_designs(&mut payload, &overlays, &templates);
    crate::portable::write_archive(payload, library.root(), dest)
}

fn package_project(project: &ProjectEntry, body: &Value) -> Value {
    let mut object = body.as_object().cloned().unwrap_or_default();
    object.insert("id".into(), json!(project.id));
    object
        .entry("name")
        .or_insert_with(|| json!(project.name.clone()));
    Value::Object(object)
}

impl ProjectsFile {
    /// The stored project object, if this id exists.
    pub fn project_body(&self, id: &str) -> Option<Value> {
        self.document.get("projects")?.get(id).cloned()
    }
}

/// A one-page-per-slide PDF of a show's lyrics, using Helvetica.
pub fn write_show_pdf(show: &ShowFile, dest: &Path) -> Result<(), DataError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let title = show.name();
    let slides = show.ordered_slides();
    let pages: Vec<Vec<String>> = if slides.is_empty() {
        vec![vec![title.clone()]]
    } else {
        slides
            .iter()
            .map(|slide| {
                let mut lines = Vec::new();
                if let Some(group) = slide.group.as_deref().filter(|group| !group.is_empty()) {
                    lines.push(group.to_string());
                }
                lines.extend(slide.lines.iter().cloned());
                if lines.is_empty() {
                    lines.push(String::new());
                }
                lines
            })
            .collect()
    };
    fs::write(dest, lyric_pdf(&title, &pages))?;
    Ok(())
}

fn lyric_pdf(title: &str, pages: &[Vec<String>]) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        String::new(), // pages object filled later
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_string(),
    ];
    let mut page_ids = Vec::new();
    for (index, lines) in pages.iter().enumerate() {
        let heading = if index == 0 {
            title
        } else {
            lines.first().map(String::as_str).unwrap_or(title)
        };
        let body = if index == 0 {
            lines.as_slice()
        } else {
            &lines[1.min(lines.len())..]
        };
        let stream = page_stream(heading, body);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
        let content_id = objects.len();
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        page_ids.push(objects.len());
    }
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    objects[1] = format!(
        "<< /Type /Pages /Kids [{kids}] /Count {} >>",
        page_ids.len()
    );

    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(body.len());
        body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let startxref = body.len();
    body.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n",
        objects.len() + 1
    ));
    body.into_bytes()
}

fn page_stream(heading: &str, lines: &[String]) -> String {
    let mut stream = String::from("BT\n/F2 18 Tf\n72 740 Td\n");
    stream.push_str(&pdf_text(heading));
    stream.push_str(" Tj\n/F1 14 Tf\n0 -28 Td\n");
    for (index, line) in lines.iter().take(24).enumerate() {
        if index > 0 {
            stream.push_str("0 -18 Td\n");
        }
        stream.push_str(&pdf_text(line));
        stream.push_str(" Tj\n");
    }
    stream.push_str("ET");
    stream
}

fn pdf_text(text: &str) -> String {
    // Helvetica uses WinAnsi; keep Latin-1 as octal escapes and replace other scripts.
    let mut escaped = String::new();
    for ch in text.chars() {
        match ch {
            '(' | ')' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            ' '..='~' => escaped.push(ch),
            c if (c as u32) <= 0xff && c != '\u{7f}' => {
                escaped.push_str(&format!("\\{:03o}", c as u8));
            }
            _ => escaped.push('?'),
        }
    }
    format!("({escaped})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SHOWS_DIR;
    use std::io::{Cursor, Read};

    #[test]
    fn show_pdf_and_project_package_round_trip() {
        let root =
            std::env::temp_dir().join(format!("freeshow-rust-export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut show = ShowFile::blank("grace", "Amazing Grace");
        assert!(show.replace_lyrics_with("[Verse]\nAmazing grace\n\n[Chorus]\nPraise God", true));
        let show_path = root.join(SHOWS_DIR).join("Grace.show");
        show.save(&show_path).unwrap();

        let pdf = root.join("Exports/grace.pdf");
        write_show_pdf(&show, &pdf).unwrap();
        let bytes = fs::read(&pdf).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("Amazing Grace"));
        assert!(text.contains("Praise God"));
        assert!(text.contains("%%EOF"));
        assert_eq!(pdf_text("café"), "(caf\\351)");
        assert_eq!(pdf_text("你好"), "(??)");

        let mut projects = ProjectsFile::empty();
        let body = json!({
            "name": "Sunday",
            "shows": [{"id": "grace", "type": "show"}]
        });
        assert!(projects.put_project("sunday", body.clone()));
        let dest = root.join("Exports/sunday.project");
        let entry = projects.projects().into_iter().next().unwrap();
        write_project_package(
            &entry,
            &projects.project_body("sunday").unwrap(),
            &[show],
            None,
            &dest,
        )
        .unwrap();
        let file = fs::read(&dest).unwrap();
        assert!(file.starts_with(b"PK"));
        let mut archive = zip::ZipArchive::new(Cursor::new(file)).unwrap();
        let mut entry = archive.by_name("project.json").unwrap();
        let mut json = String::new();
        entry.read_to_string(&mut json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["project"]["name"], "Sunday");
        assert!(value["shows"].get("grace").is_some());
        let _ = fs::remove_dir_all(&root);
    }
}
