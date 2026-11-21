// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use freeshow_data::{Library, ProjectsFile, ShowFile};
use serde_json::{Map, Value};

mod lessons;
pub use lessons::import_lesson_document;

/// Loads ChurchApps presenter plans into a FreeShow library.
/// The show keeps running when no account is connected or a request fails.
pub trait ContentSource {
    fn name(&self) -> &'static str;
    fn sync(&self, library: &Path) -> SyncReport;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub shows: usize,
    pub project_id: Option<String>,
}

impl SyncReport {
    fn none() -> Self {
        Self {
            shows: 0,
            project_id: None,
        }
    }
}

pub struct ChurchApps;

impl ContentSource for ChurchApps {
    fn name(&self) -> &'static str {
        "churchapps"
    }

    fn sync(&self, library: &Path) -> SyncReport {
        let local = lessons::import_sources(library);
        match load_configured(library) {
            Ok(report) => SyncReport {
                shows: report.shows + local.shows,
                project_id: report.project_id.or(local.project_id),
            },
            Err(err) => {
                tracing::error!("churchapps: {err}");
                local
            }
        }
    }
}

const API: &str = "https://api.churchapps.org";
const ITEM_STYLE: &str = "left:50px;top:120px;width:1820px;height:840px;";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServicePlan {
    id: String,
    name: String,
    entries: Vec<PlanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanEntry {
    Section {
        id: String,
        name: String,
        notes: String,
    },
    Show(PlanShow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanShow {
    id: String,
    title: String,
    author: String,
    key: String,
    notes: String,
    sections: Vec<LyricSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LyricSection {
    label: String,
    slides: Vec<String>,
}

fn load_configured(library: &Path) -> Result<SyncReport, String> {
    let document = if let Ok(path) = std::env::var("FREESHOW_CHURCHAPPS_FILE") {
        if path.trim().is_empty() {
            return Ok(SyncReport::none());
        }
        if std::fs::metadata(&path)
            .map_err(|err| err.to_string())?
            .len()
            > 16 * 1024 * 1024
        {
            return Err("ChurchApps document exceeds 16 MiB".into());
        }
        std::fs::read_to_string(path).map_err(|err| err.to_string())?
    } else {
        let Some(token) = std::env::var("FREESHOW_CHURCHAPPS_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
        else {
            return Ok(SyncReport::none());
        };
        fetch_plans(token.trim())?
    };
    let value: Value = serde_json::from_str(&document).map_err(|err| err.to_string())?;
    if freeshow_data::is_lesson_document(&value) {
        return import_lesson_document(library, &value);
    }
    let plans = plans_from_document(&document)?;
    if plans.is_empty() {
        return Err("churchapps returned no plans".to_string());
    }
    let project_id = plans[0].id.clone();
    let shows = write_plans(library, &plans)?;
    tracing::info!("churchapps wrote {shows} shows");
    Ok(SyncReport {
        shows,
        project_id: Some(project_id),
    })
}

pub(crate) fn plans_from_document(document: &str) -> Result<Vec<ServicePlan>, String> {
    let value: Value = serde_json::from_str(document).map_err(|err| err.to_string())?;
    let plans = value
        .get("plans")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "churchapps document has no plans".to_string())?;
    let mut parsed = Vec::new();
    for plan in plans {
        if let Some(plan) = parse_plan(plan) {
            parsed.push(plan);
        }
    }
    Ok(parsed)
}

pub(crate) fn write_plans(library_root: &Path, plans: &[ServicePlan]) -> Result<usize, String> {
    let library = Library::open(library_root);
    std::fs::create_dir_all(library.shows_dir()).map_err(|err| err.to_string())?;
    let mut projects =
        ProjectsFile::load(&library.projects_path()).map_err(|err| err.to_string())?;
    let mut count = 0;
    for plan in plans {
        for entry in &plan.entries {
            let PlanEntry::Show(song) = entry else {
                continue;
            };
            let show = show_file(song);
            let path = library
                .shows_dir()
                .join(format!("{}.show", file_id(&song.id)));
            show.save(&path).map_err(|err| err.to_string())?;
            count += 1;
        }
        if !projects.add_project(&plan.id, &plan.name) {
            projects.set_project_name(&plan.id, &plan.name);
        }
        projects.clear_shows(&plan.id);
        for entry in &plan.entries {
            match entry {
                PlanEntry::Section { id, name, notes } => {
                    projects.append_section(&plan.id, id, name, notes);
                }
                PlanEntry::Show(song) => {
                    projects.append_show(&plan.id, &song.id);
                }
            }
        }
    }
    projects
        .save(&library.projects_path())
        .map_err(|err| err.to_string())?;
    Ok(count)
}

fn parse_plan(plan: &Value) -> Option<ServicePlan> {
    let id = plan.get("id")?.as_str()?.to_string();
    let name = plan
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(&id)
        .to_string();
    let mut entries = Vec::new();
    for (index, item) in plan
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let notes = [item.get("description"), item.get("assignees")]
            .into_iter()
            .filter_map(|value| value.and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        entries.push(PlanEntry::Section {
            id: format!("chumsection_{id}_{index}"),
            name: label,
            notes,
        });
        for child in item
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(show) = parse_child(child) {
                entries.push(PlanEntry::Show(show));
            }
        }
    }
    if entries.is_empty() {
        return None;
    }
    Some(ServicePlan { id, name, entries })
}

fn parse_child(child: &Value) -> Option<PlanShow> {
    let kind = child.get("itemType").and_then(Value::as_str).unwrap_or("");
    match kind {
        "arrangementKey" => parse_song(child),
        "item" | "header" => Some(text_show(child)),
        _ => None,
    }
}

fn parse_song(child: &Value) -> Option<PlanShow> {
    let arrangement_key = child.get("arrangementKey")?;
    let arrangement = child.get("arrangement")?;
    let song = child.get("song")?;
    let detail = child.get("songDetail")?;
    let key_id = arrangement_key.get("id")?.as_str()?.to_string();
    let key = arrangement_key
        .get("keySignature")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arrangement_name = arrangement
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let title = detail
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let id = arrangement
        .get("freeShowId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("chumssong_{key_id}"));
    Some(PlanShow {
        id,
        title: format!("{title} ({arrangement_name} - {key})"),
        author: detail
            .get("artist")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        key: detail
            .get("keySignature")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string(),
        notes: song
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        sections: parse_lyrics(
            arrangement
                .get("lyrics")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
    })
}

fn text_show(child: &Value) -> PlanShow {
    let id = child
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("item")
        .to_string();
    let title = child
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let description = child
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let slides = if description.trim().is_empty() {
        Vec::new()
    } else {
        vec![description.clone()]
    };
    PlanShow {
        id: format!("chumsshow_{id}"),
        title,
        author: String::new(),
        key: String::new(),
        notes: description,
        sections: if slides.is_empty() {
            Vec::new()
        } else {
            vec![LyricSection {
                label: String::new(),
                slides,
            }]
        },
    }
}

fn parse_lyrics(lyrics: &str) -> Vec<LyricSection> {
    if lyrics.is_empty() {
        return Vec::new();
    }
    if !lyrics.starts_with('[') {
        let slides = lyrics
            .split("\n\n")
            .map(str::trim)
            .filter(|slide| !slide.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        return if slides.is_empty() {
            Vec::new()
        } else {
            vec![LyricSection {
                label: "Lyrics".to_string(),
                slides,
            }]
        };
    }
    let mut sections = Vec::new();
    let mut label = String::new();
    let mut buffer = Vec::new();
    let mut slides = Vec::new();
    let flush = |slides: &mut Vec<String>, buffer: &mut Vec<String>| {
        if buffer.is_empty() {
            return;
        }
        let text = buffer.join("\n").trim().to_string();
        buffer.clear();
        if !text.is_empty() {
            slides.push(text);
        }
    };
    for line in lyrics.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            flush(&mut slides, &mut buffer);
            if !label.is_empty() {
                sections.push(LyricSection {
                    label: label.clone(),
                    slides: std::mem::take(&mut slides),
                });
            }
            label = section_name(&trimmed[1..trimmed.len() - 1]);
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut slides, &mut buffer);
        } else {
            buffer.push(line.to_string());
        }
    }
    flush(&mut slides, &mut buffer);
    if !label.is_empty() {
        sections.push(LyricSection { label, slides });
    }
    sections
}

fn section_name(label: &str) -> String {
    let trimmed = label.trim();
    let stem = trimmed
        .trim_end_matches(|ch: char| ch.is_ascii_digit())
        .trim_end();
    if stem.is_empty() {
        trimmed.to_string()
    } else {
        stem.to_string()
    }
}

struct Draft {
    id: String,
    group: String,
    lines: Vec<String>,
    children: Vec<String>,
}

fn show_file(song: &PlanShow) -> ShowFile {
    let mut show = ShowFile::blank(&song.id, &song.title);
    if let Some(slides) = show.show.get_mut("slides").and_then(Value::as_object_mut) {
        slides.clear();
    }
    if let Some(order) = show
        .show
        .pointer_mut("/layouts/main/slides")
        .and_then(Value::as_array_mut)
    {
        order.clear();
    }
    let mut parents = Vec::new();
    let mut index = 1;
    let mut dedup: HashMap<String, String> = HashMap::new();
    let mut slides: Vec<Draft> = Vec::new();
    for section in &song.sections {
        let mut parent = None;
        let mut children = Vec::new();
        for (offset, text) in section.slides.iter().enumerate() {
            let lines = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let key = format!("{}\n{}", section.label, lines.join("\n"));
            let id = if let Some(id) = dedup.get(&key) {
                id.clone()
            } else {
                let id = format!("s{index}");
                index += 1;
                dedup.insert(key, id.clone());
                slides.push(Draft {
                    id: id.clone(),
                    group: section.label.clone(),
                    lines,
                    children: Vec::new(),
                });
                id
            };
            if offset == 0 {
                parent = Some(id.clone());
                parents.push(id);
            } else if parent.as_deref() != Some(id.as_str())
                && !children.iter().any(|child| child == &id)
            {
                children.push(id);
            }
        }
        if let Some(parent) = parent
            && !children.is_empty()
            && let Some(slide) = slides.iter_mut().find(|slide| slide.id == parent)
        {
            slide.children = children;
        }
    }
    if slides.is_empty() {
        slides.push(Draft {
            id: "s1".to_string(),
            group: String::new(),
            lines: vec![song.title.clone()],
            children: Vec::new(),
        });
        parents.push("s1".to_string());
    }
    if let Some(stored) = show.show.get_mut("slides").and_then(Value::as_object_mut) {
        for slide in &slides {
            let mut value = serde_json::json!({
                "group": slide.group,
                "globalGroup": slide.group.to_lowercase(),
                "color": null,
                "settings": {},
                "notes": "",
                "items": [{
                    "style": ITEM_STYLE,
                    "lines": slide.lines.iter().map(|line| serde_json::json!({
                        "align": "",
                        "text": [{ "style": "", "value": line }]
                    })).collect::<Vec<_>>()
                }]
            });
            if !slide.children.is_empty() {
                value["children"] = Value::Array(
                    slide
                        .children
                        .iter()
                        .map(|id| Value::String(id.clone()))
                        .collect(),
                );
            }
            stored.insert(slide.id.clone(), value);
        }
    }
    if let Some(order) = show
        .show
        .pointer_mut("/layouts/main/slides")
        .and_then(Value::as_array_mut)
    {
        for id in &parents {
            order.push(serde_json::json!({ "id": id }));
        }
    }
    if let Some(object) = show.show.as_object_mut() {
        object.insert(
            "origin".to_string(),
            Value::String("churchApps".to_string()),
        );
        object.insert(
            "category".to_string(),
            Value::String("churchapps".to_string()),
        );
        let meta = object
            .entry("meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert("title".to_string(), Value::String(song.title.clone()));
            meta.insert("author".to_string(), Value::String(song.author.clone()));
            meta.insert("key".to_string(), Value::String(song.key.clone()));
            meta.insert("CCLI".to_string(), Value::String(String::new()));
        }
    }
    if let Some(notes) = show.show.pointer_mut("/layouts/main/notes") {
        *notes = Value::String(song.notes.clone());
    }
    show
}

fn file_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn fetch_plans(token: &str) -> Result<String, String> {
    let plans: Vec<Value> = serde_json::from_str(&get_json(
        &format!("{API}/doing/plans/presenter"),
        Some(token),
    )?)
    .map_err(|err| err.to_string())?;
    let mut assembled = Vec::new();
    for mut plan in plans {
        let church = plan
            .get("churchId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let id = plan
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if church.is_empty() || id.is_empty() {
            continue;
        }
        let mut items: Vec<Value> = match get_json(
            &format!("{API}/doing/planItems/presenter/{church}/{id}"),
            Some(token),
        ) {
            Ok(body) => serde_json::from_str(&body).unwrap_or_default(),
            Err(err) => {
                tracing::error!("churchapps plan {id}: {err}");
                Vec::new()
            }
        };
        for item in &mut items {
            let Some(children) = item.get_mut("children").and_then(Value::as_array_mut) else {
                continue;
            };
            for child in children.iter_mut() {
                attach_arrangement(&church, child);
            }
        }
        if let Some(plan) = plan.as_object_mut() {
            plan.insert("items".to_string(), Value::Array(items));
        }
        assembled.push(plan);
    }
    serde_json::to_string(&serde_json::json!({ "plans": assembled })).map_err(|err| err.to_string())
}

fn attach_arrangement(church: &str, child: &mut Value) {
    if child.get("itemType").and_then(Value::as_str) != Some("arrangementKey") {
        return;
    }
    if child.get("arrangement").is_some() {
        return;
    }
    let church_id = child
        .get("churchId")
        .and_then(Value::as_str)
        .unwrap_or(church);
    let Some(related) = child.get("relatedId").and_then(Value::as_str) else {
        return;
    };
    let Ok(body) = get_json(
        &format!("{API}/content/arrangementKeys/presenter/{church_id}/{related}"),
        None,
    ) else {
        return;
    };
    let Ok(data) = serde_json::from_str::<Value>(&body) else {
        return;
    };
    let Some(child) = child.as_object_mut() else {
        return;
    };
    for key in ["arrangementKey", "arrangement", "song", "songDetail"] {
        if let Some(value) = data.get(key) {
            child.insert(key.to_string(), value.clone());
        }
    }
}

fn get_json(url: &str, token: Option<&str>) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .build();
    let request = agent.get(url);
    let request = if let Some(token) = token {
        request.set("Authorization", &format!("Bearer {token}"))
    } else {
        request
    };
    match request.call() {
        Ok(response) => response.into_string().map_err(|err| err.to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("churchapps returned {code}")),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"{
      "plans": [{
        "id": "p1",
        "name": "Sunday",
        "churchId": "c1",
        "items": [{
          "label": "Worship",
          "description": "stand",
          "children": [
            {
              "itemType": "arrangementKey",
              "relatedId": "arr1",
              "arrangementKey": { "id": "key1", "keySignature": "G" },
              "arrangement": { "name": "Default", "lyrics": "[Verse 1]\nAmazing grace\nhow sweet the sound\n\nstill more\n\n[Chorus 1]\nPraise God\nfrom whom" },
              "song": { "notes": "slow" },
              "songDetail": { "title": "Amazing Grace", "artist": "John Newton", "keySignature": "G" }
            },
            {
              "itemType": "arrangementKey",
              "arrangementKey": { "id": "key2", "keySignature": "C" },
              "arrangement": { "name": "Short", "lyrics": "Be still\n\nKnow" },
              "song": { "notes": "" },
              "songDetail": { "title": "Be Still", "artist": "", "keySignature": "C" }
            },
            { "itemType": "item", "id": "welcome", "label": "Welcome", "description": "Coffee after service" }
          ]
        }]
      }]
    }"#;

    #[test]
    fn a_churchapps_plan_becomes_shows() {
        let root = std::env::temp_dir().join(format!("freeshow-churchapps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plans = plans_from_document(PLAN).unwrap();
        assert_eq!(write_plans(&root, &plans).unwrap(), 3);
        assert_eq!(write_plans(&root, &plans).unwrap(), 3);

        let song = ShowFile::load(&root.join("Shows/chumssong_key1.show")).unwrap();
        assert_eq!(song.name(), "Amazing Grace (Default - G)");
        assert_eq!(
            song.show.get("origin").and_then(Value::as_str),
            Some("churchApps")
        );
        assert_eq!(
            song.show.pointer("/meta/author").and_then(Value::as_str),
            Some("John Newton")
        );
        assert_eq!(
            song.show.pointer("/meta/key").and_then(Value::as_str),
            Some("G")
        );
        assert_eq!(
            song.show
                .pointer("/layouts/main/notes")
                .and_then(Value::as_str),
            Some("slow")
        );
        assert_eq!(
            song.show
                .pointer("/layouts/main/slides")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            song.show
                .pointer("/slides/s1/children/0")
                .and_then(Value::as_str),
            Some("s2")
        );
        let slides = song.ordered_slides();
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].group.as_deref(), Some("Verse"));
        assert_eq!(
            slides[0].lines,
            vec!["Amazing grace", "how sweet the sound"]
        );
        assert_eq!(slides[1].lines, vec!["still more"]);
        assert_eq!(slides[2].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[2].lines, vec!["Praise God", "from whom"]);

        let quiet = ShowFile::load(&root.join("Shows/chumssong_key2.show")).unwrap();
        let quiet_slides = quiet.ordered_slides();
        assert_eq!(quiet_slides.len(), 2);
        assert_eq!(quiet_slides[0].group.as_deref(), Some("Lyrics"));
        assert_eq!(quiet_slides[0].lines, vec!["Be still"]);
        assert_eq!(quiet_slides[1].lines, vec!["Know"]);

        let welcome = ShowFile::load(&root.join("Shows/chumsshow_welcome.show")).unwrap();
        assert_eq!(
            welcome.ordered_slides()[0].lines,
            vec!["Coffee after service"]
        );

        let projects = ProjectsFile::load(&root.join("Config/projects.json")).unwrap();
        let entries = projects.projects();
        assert_eq!(entries[0].name, "Sunday");
        assert_eq!(entries[0].shows.len(), 4);
        assert_eq!(entries[0].shows[0].label, "Worship");
        assert_eq!(entries[0].shows[1].id, "chumssong_key1");
        assert_eq!(entries[0].shows[3].id, "chumsshow_welcome");
        let stored = std::fs::read_to_string(root.join("Config/projects.json")).unwrap();
        assert!(stored.contains("\"notes\":\"stand\""));
        let _ = std::fs::remove_dir_all(&root);
    }
}
