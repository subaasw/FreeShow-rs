// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::time::Duration;

use freeshow_data::{Library, ProjectsFile, ShowFile};
use serde_json::{Map, Value};

/// Loads a Planning Center service plan into a FreeShow library.
/// The show keeps running when no account is connected or the request fails.
pub trait PlanSource {
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

pub struct PlanningCenter;

impl PlanSource for PlanningCenter {
    fn name(&self) -> &'static str {
        "planningcenter"
    }

    fn sync(&self, library: &Path) -> SyncReport {
        match load_configured(library) {
            Ok(report) => report,
            Err(err) => {
                tracing::error!("planning center: {err}");
                SyncReport::none()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePlan {
    pub id: String,
    pub name: String,
    pub items: Vec<PlanItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanItem {
    Section { id: String, name: String },
    Show(PlanShow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanShow {
    pub id: String,
    pub title: String,
    pub author: String,
    pub ccli: String,
    pub category: String,
    pub notes: String,
    pub sections: Vec<LyricSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricSection {
    pub label: String,
    pub lines: Vec<String>,
}

const SECTION_HEADERS: &[&str] = &[
    "POST-CHORUS",
    "POSTCHORUS",
    "POST-CORO",
    "POSTCORO",
    "PRE-CHORUS",
    "PRECHORUS",
    "PRE-CORO",
    "PRECORO",
    "INSTRUMENTAL",
    "ESTRIBILLO",
    "BREAKDOWN",
    "TURNAROUND",
    "INTERLUDE",
    "REFRENG",
    "REFRAIN",
    "ENDING",
    "PUENTE",
    "CHORUS",
    "VERSE",
    "VERSO",
    "BRIDGE",
    "INTRO",
    "OUTRO",
    "FINAL",
    "BREAK",
    "HOOK",
    "VAMP",
    "POST",
    "VERS",
    "CORO",
    "PRE",
    "TAG",
];

fn load_configured(library: &Path) -> Result<SyncReport, String> {
    let document = if let Ok(path) = std::env::var("FREESHOW_PCO_FILE") {
        if path.trim().is_empty() {
            return Ok(SyncReport::none());
        }
        std::fs::read_to_string(path).map_err(|err| err.to_string())?
    } else {
        let Some(token) = std::env::var("FREESHOW_PCO_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
        else {
            return Ok(SyncReport::none());
        };
        let Some(plan) = std::env::var("FREESHOW_PCO_PLAN")
            .ok()
            .filter(|plan| !plan.trim().is_empty())
        else {
            return Err("FREESHOW_PCO_PLAN is missing".to_string());
        };
        let Some((service_type, plan_id)) = plan.trim().split_once('/') else {
            return Err("FREESHOW_PCO_PLAN must look like serviceTypeId/planId".to_string());
        };
        fetch_plan(token.trim(), service_type, plan_id)?
    };
    let plan = plan_from_document(&document)?;
    let project_id = plan.id.clone();
    let shows = write_plan(library, &plan)?;
    tracing::info!("planning center plan {project_id} wrote {shows} shows");
    Ok(SyncReport {
        shows,
        project_id: Some(project_id),
    })
}

pub fn plan_from_document(document: &str) -> Result<ServicePlan, String> {
    let value: Value = serde_json::from_str(document).map_err(|err| err.to_string())?;
    let plan = value
        .get("data")
        .filter(|data| {
            data.get("type")
                .and_then(Value::as_str)
                .is_some_and(is_plan)
        })
        .or_else(|| value.get("data").filter(|data| data.get("id").is_some()))
        .ok_or_else(|| "planning center document has no plan".to_string())?;
    let id = plan
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("pco")
        .to_string();
    let attributes = plan.get("attributes").cloned().unwrap_or(Value::Null);
    let name = plan_name(&attributes, &id);
    let resources = index_resources(&value);
    let mut items = Vec::new();
    for item in ordered_items(&value, &resources) {
        if let Some(parsed) = plan_item(&item, &resources) {
            items.push(parsed);
        }
    }
    if items.is_empty() {
        return Err("planning center plan has no items".to_string());
    }
    Ok(ServicePlan { id, name, items })
}

pub fn write_plan(library_root: &Path, plan: &ServicePlan) -> Result<usize, String> {
    let library = Library::open(library_root);
    std::fs::create_dir_all(library.shows_dir()).map_err(|err| err.to_string())?;
    let mut count = 0;
    for item in &plan.items {
        let PlanItem::Show(song) = item else {
            continue;
        };
        let show = show_file(song);
        let path = library
            .shows_dir()
            .join(format!("{}.show", file_id(&song.id)));
        show.save(&path).map_err(|err| err.to_string())?;
        count += 1;
    }
    let path = library.projects_path();
    let mut projects = ProjectsFile::load(&path).map_err(|err| err.to_string())?;
    if !projects.add_project(&plan.id, &plan.name) {
        projects.set_project_name(&plan.id, &plan.name);
    }
    projects.clear_shows(&plan.id);
    for item in &plan.items {
        match item {
            PlanItem::Section { id, name } => {
                projects.append_section(&plan.id, id, name, "");
            }
            PlanItem::Show(song) => {
                projects.append_show(&plan.id, &song.id);
            }
        }
    }
    projects.save(&path).map_err(|err| err.to_string())?;
    Ok(count)
}

fn show_file(song: &PlanShow) -> ShowFile {
    let mut show = ShowFile::blank(&song.id, &song.title);
    let sections = if song.sections.is_empty() {
        vec![LyricSection {
            label: String::new(),
            lines: vec![song.title.clone()],
        }]
    } else {
        song.sections.clone()
    };
    for (index, section) in sections.iter().enumerate() {
        let slide_id = if index == 0 {
            "s1".to_string()
        } else {
            show.add_slide()
                .unwrap_or_else(|| format!("s{}", index + 1))
        };
        show.set_plain_text(&slide_id, &section.lines.join("\n"));
        show.set_group(&slide_id, &section.label);
    }
    if let Some(object) = show.show.as_object_mut() {
        object.insert("origin".to_string(), Value::String("pco".to_string()));
        object.insert("category".to_string(), Value::String(song.category.clone()));
        let meta = object
            .entry("meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert("title".to_string(), Value::String(song.title.clone()));
            meta.insert("author".to_string(), Value::String(song.author.clone()));
            meta.insert("CCLI".to_string(), Value::String(song.ccli.clone()));
        }
    }
    if let Some(notes) = show.show.pointer_mut("/layouts/main/notes") {
        *notes = Value::String(song.notes.clone());
    }
    show
}

fn plan_item(item: &Value, resources: &Map<String, Value>) -> Option<PlanItem> {
    let id = item.get("id")?.as_str()?.to_string();
    let attributes = item.get("attributes")?;
    let kind = attributes
        .get("item_type")
        .and_then(Value::as_str)
        .unwrap_or("item");
    let title = attributes
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match kind {
        "header" => Some(PlanItem::Section {
            id: format!("pcosection_{id}"),
            name: title,
        }),
        "media" => None,
        "song" => Some(PlanItem::Show(song_show(&id, &title, item, resources))),
        _ => Some(PlanItem::Show(item_show(&id, &title, attributes))),
    }
}

fn song_show(item_id: &str, title: &str, item: &Value, resources: &Map<String, Value>) -> PlanShow {
    let song_id = related(item, "song").unwrap_or_else(|| item_id.to_string());
    let arrangement_id = related(item, "arrangement");
    let song = resources.get(&resource_key("Song", &song_id));
    let arrangement = arrangement_id
        .as_ref()
        .and_then(|id| resources.get(&resource_key("Arrangement", id)));
    let song_attributes = song.and_then(|song| song.get("attributes"));
    let arrangement_attributes = arrangement.and_then(|arrangement| arrangement.get("attributes"));
    let title = song_attributes
        .and_then(|attributes| attributes.get("title"))
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or(title)
        .to_string();
    let author = song_attributes
        .and_then(|attributes| attributes.get("author"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ccli = song_attributes
        .and_then(|attributes| {
            attributes
                .get("ccli_number")
                .or_else(|| attributes.get("CCLI"))
        })
        .map(value_text)
        .unwrap_or_default();
    let notes = arrangement_attributes
        .and_then(|attributes| attributes.get("notes"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let api_sections = arrangement_attributes
        .and_then(|attributes| attributes.get("sections"))
        .and_then(Value::as_array)
        .map(|sections| {
            sections
                .iter()
                .filter_map(|section| {
                    let label = section.get("label")?.as_str()?.to_string();
                    let lines =
                        lyric_lines(section.get("lyrics").and_then(Value::as_str).unwrap_or(""));
                    Some(LyricSection { label, lines })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let chart = arrangement_attributes
        .and_then(|attributes| attributes.get("chord_chart"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut sections = merge_sections(api_sections, chart_sections(chart));
    let sequence = arrangement_attributes
        .and_then(|attributes| attributes.get("sequence"))
        .and_then(Value::as_array)
        .map(|sequence| {
            sequence
                .iter()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !sequence.is_empty() {
        sections = order_sections(sections, &sequence);
    }
    sections.retain(|section| section.lines.iter().any(|line| !line.trim().is_empty()));
    PlanShow {
        id: format!("pcosong_{song_id}"),
        title,
        author,
        ccli,
        category: "planning_center".to_string(),
        notes,
        sections,
    }
}

fn item_show(id: &str, title: &str, attributes: &Value) -> PlanShow {
    let html = attributes
        .get("html_details")
        .and_then(Value::as_str)
        .unwrap_or("");
    let description = attributes
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let source = if html.trim().is_empty() {
        description.to_string()
    } else {
        plain_html(html)
    };
    let sections = text_sections(&source);
    PlanShow {
        id: format!("pcosong_{id}"),
        title: if title.is_empty() {
            "Item".to_string()
        } else {
            title.to_string()
        },
        author: String::new(),
        ccli: String::new(),
        category: "planning_center_generic".to_string(),
        notes: String::new(),
        sections,
    }
}

fn merge_sections(api: Vec<LyricSection>, chart: Vec<LyricSection>) -> Vec<LyricSection> {
    if api.is_empty() {
        return chart;
    }
    let mut merged = Vec::new();
    for section in api {
        let lyrics = if section.lines.is_empty() {
            find_section(&section.label, &chart)
                .map(|found| found.lines.clone())
                .unwrap_or_default()
        } else {
            section.lines
        };
        merged.push(LyricSection {
            label: section.label,
            lines: lyrics,
        });
    }
    for section in chart {
        if find_section(&section.label, &merged).is_none() {
            merged.push(section);
        }
    }
    merged
}

fn order_sections(sections: Vec<LyricSection>, sequence: &[String]) -> Vec<LyricSection> {
    sequence
        .iter()
        .map(|label| {
            find_section(label, &sections)
                .cloned()
                .unwrap_or(LyricSection {
                    label: label.clone(),
                    lines: Vec::new(),
                })
        })
        .collect()
}

fn find_section<'a>(label: &str, sections: &'a [LyricSection]) -> Option<&'a LyricSection> {
    let search = normalize_label(label);
    sections
        .iter()
        .find(|section| normalize_label(&section.label) == search)
        .or_else(|| {
            sections.iter().find(|section| {
                let candidate = normalize_label(&section.label);
                !candidate.is_empty()
                    && !search.is_empty()
                    && (candidate.starts_with(&search) || search.starts_with(&candidate))
            })
        })
}

fn chart_sections(chart: &str) -> Vec<LyricSection> {
    let mut sections = Vec::new();
    let mut label = String::new();
    let mut lines = Vec::new();
    for line in chart.split('\n') {
        let trimmed = line.trim().trim_end_matches('\r');
        if is_keyword(trimmed) {
            continue;
        }
        if let Some(header) = section_label(trimmed) {
            push_section(&mut sections, &label, &lines);
            label = header;
            lines.clear();
            continue;
        }
        if label.is_empty() {
            continue;
        }
        if let Some(lyric) = lyric_line(trimmed) {
            lines.push(lyric);
        }
    }
    push_section(&mut sections, &label, &lines);
    if sections.is_empty() {
        let lines = chart
            .lines()
            .filter_map(|line| lyric_line(line.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(LyricSection {
                label: String::new(),
                lines,
            });
        }
    }
    sections
}

fn push_section(sections: &mut Vec<LyricSection>, label: &str, lines: &[String]) {
    if label.is_empty() || lines.is_empty() {
        return;
    }
    sections.push(LyricSection {
        label: label.to_string(),
        lines: lines.to_vec(),
    });
}

fn text_sections(text: &str) -> Vec<LyricSection> {
    text.split("\n\n")
        .filter_map(|block| {
            let lines = lyric_lines(block);
            if lines.is_empty() {
                None
            } else {
                Some(LyricSection {
                    label: String::new(),
                    lines,
                })
            }
        })
        .collect()
}

fn lyric_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| lyric_line(line.trim()))
        .collect()
}

fn section_label(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_matches(|ch| ch == '[' || ch == ']').trim();
    if trimmed.is_empty() || trimmed.contains('[') {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();
    for header in SECTION_HEADERS {
        if upper == *header {
            return Some(trimmed.to_string());
        }
        let prefix = format!("{header} ");
        if let Some(rest) = upper.strip_prefix(&prefix)
            && rest.chars().any(|ch| ch.is_ascii_digit())
            && rest
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch.is_ascii_whitespace())
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn lyric_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_keyword(trimmed) || section_label(trimmed).is_some() {
        return None;
    }
    let without_chords = strip_brackets(trimmed);
    let text = without_chords
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() || is_chord_line(&text) {
        return None;
    }
    Some(text)
}

fn is_keyword(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper == "COLUMN_BREAK" || upper.ends_with("_BREAK") || upper.ends_with("_BRAKE") {
        return true;
    }
    upper == "SECTION"
        || upper.starts_with("SECTION ")
        || upper.starts_with("TRANSPOSE")
        || upper.starts_with("REDEFINE")
}

fn strip_brackets(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find(']') else {
            out.push_str(&rest[start..]);
            return out;
        };
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn is_chord_line(line: &str) -> bool {
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    !tokens.is_empty() && tokens.iter().all(|token| is_chord(token))
}

fn is_chord(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(root) = chars.next() else {
        return false;
    };
    if !matches!(root, 'A'..='G') {
        return false;
    }
    let rest: String = chars.collect();
    let rest = rest
        .trim_start_matches(['#', 'b'])
        .trim_start_matches("maj7")
        .trim_start_matches("maj")
        .trim_start_matches("min")
        .trim_start_matches('m')
        .trim_start_matches("sus")
        .trim_start_matches("dim")
        .trim_start_matches("aug")
        .trim_start_matches("add");
    let rest = rest.trim_start_matches(|ch: char| ch.is_ascii_digit());
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    rest.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '#' || ch == 'b')
        && !rest.chars().any(|ch| ch.is_ascii_lowercase())
}

fn plain_html(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            out.push_str(after);
            break;
        };
        if after[..end].to_ascii_lowercase().starts_with("<br") {
            out.push('\n');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn plan_name(attributes: &Value, id: &str) -> String {
    attributes
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .or_else(|| attributes.get("dates").and_then(Value::as_str))
        .or_else(|| {
            attributes
                .get("sort_date")
                .and_then(Value::as_str)
                .map(|date| date.get(..10).unwrap_or(date))
        })
        .unwrap_or(id)
        .to_string()
}

fn index_resources(document: &Value) -> Map<String, Value> {
    let mut resources = Map::new();
    if let Some(included) = document.get("included").and_then(Value::as_array) {
        for resource in included {
            insert_resource(&mut resources, resource);
        }
    }
    if let Some(data) = document.get("data") {
        if data.is_array() {
            for resource in data.as_array().into_iter().flatten() {
                insert_resource(&mut resources, resource);
            }
        } else {
            insert_resource(&mut resources, data);
        }
    }
    resources
}

fn insert_resource(resources: &mut Map<String, Value>, resource: &Value) {
    let Some(kind) = resource.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(id) = resource.get("id").and_then(Value::as_str) else {
        return;
    };
    resources.insert(resource_key(kind, id), resource.clone());
}

fn ordered_items(document: &Value, resources: &Map<String, Value>) -> Vec<Value> {
    let plan = document.get("data").unwrap_or(&Value::Null);
    if let Some(refs) = plan
        .pointer("/relationships/items/data")
        .and_then(Value::as_array)
    {
        let ordered = refs
            .iter()
            .filter_map(|item| {
                let id = item.get("id")?.as_str()?;
                resources.get(&resource_key("Item", id)).cloned()
            })
            .collect::<Vec<_>>();
        if !ordered.is_empty() {
            return ordered;
        }
    }
    let mut items = resources
        .iter()
        .filter(|(key, _)| key.starts_with("item:"))
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        item.pointer("/attributes/sequence")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    });
    items
}

fn related(item: &Value, name: &str) -> Option<String> {
    item.pointer(&format!("/relationships/{name}/data/id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn resource_key(kind: &str, id: &str) -> String {
    format!("{}:{id}", kind.to_ascii_lowercase())
}

fn is_plan(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("plan")
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
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

fn normalize_label(label: &str) -> String {
    label
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn fetch_plan(token: &str, service_type: &str, plan_id: &str) -> Result<String, String> {
    let path = format!("/services/v2/service_types/{service_type}/plans/{plan_id}?include=items");
    let mut document: Value =
        serde_json::from_str(&get_json(token, &path)?).map_err(|err| err.to_string())?;
    let resources = index_resources(&document);
    let items = ordered_items(&document, &resources);
    let included = document
        .as_object_mut()
        .and_then(|document| {
            document
                .entry("included")
                .or_insert_with(|| Value::Array(Vec::new()));
            document.get_mut("included")
        })
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "planning center plan could not be read".to_string())?;
    for item in items {
        if item
            .pointer("/attributes/item_type")
            .and_then(Value::as_str)
            != Some("song")
        {
            continue;
        }
        let Some(item_id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let song_path = format!(
            "/services/v2/service_types/{service_type}/plans/{plan_id}/items/{item_id}/song"
        );
        if let Ok(song) = get_json(token, &song_path)
            && let Ok(song) = serde_json::from_str::<Value>(&song)
            && let Some(data) = song.get("data")
        {
            included.push(data.clone());
            if let Some(arrangement) = related(&item, "arrangement")
                && let Some(song_id) = data.get("id").and_then(Value::as_str)
            {
                let arrangement_path =
                    format!("/services/v2/songs/{song_id}/arrangements/{arrangement}");
                if let Ok(arrangement) = get_json(token, &arrangement_path)
                    && let Ok(arrangement) = serde_json::from_str::<Value>(&arrangement)
                    && let Some(data) = arrangement.get("data")
                {
                    included.push(data.clone());
                }
            }
        }
    }
    serde_json::to_string(&document).map_err(|err| err.to_string())
}

fn get_json(token: &str, path: &str) -> Result<String, String> {
    let url = format!("https://api.planningcenteronline.com{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .build();
    match agent
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(response) => response.into_string().map_err(|err| err.to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("planning center returned {code}")),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"{
      "data": {
        "type": "Plan",
        "id": "55",
        "attributes": { "title": "Sunday", "sort_date": "2026-11-08T15:00:00Z" },
        "relationships": {
          "items": { "data": [
            { "type": "Item", "id": "1" },
            { "type": "Item", "id": "2" },
            { "type": "Item", "id": "4" }
          ] }
        }
      },
      "included": [
        { "type": "Item", "id": "1", "attributes": { "title": "Welcome", "item_type": "header", "sequence": 1 } },
        {
          "type": "Item",
          "id": "2",
          "attributes": { "title": "Amazing Grace", "item_type": "song", "sequence": 2 },
          "relationships": {
            "song": { "data": { "type": "Song", "id": "9" } },
            "arrangement": { "data": { "type": "Arrangement", "id": "3" } }
          }
        },
        { "type": "Song", "id": "9", "attributes": { "title": "Amazing Grace", "author": "John Newton", "ccli_number": 22025 } },
        {
          "type": "Arrangement",
          "id": "3",
          "attributes": {
            "notes": "slow",
            "sequence": ["Verse 1", "Chorus"],
            "chord_chart": "Chorus\nPraise [D]God\nfrom whom\nCOLUMN_BREAK\nG D\n\nVerse 1\nAmazing [G]grace\nhow sweet the sound\nTRANSPOSE +1"
          }
        },
        { "type": "Item", "id": "4", "attributes": { "title": "Announcements", "item_type": "item", "description": "Coffee after service", "sequence": 3 } }
      ]
    }"#;

    #[test]
    fn a_plan_becomes_shows_in_service_order() {
        let root = std::env::temp_dir().join(format!("freeshow-pco-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plan = plan_from_document(PLAN).unwrap();
        assert_eq!(plan.name, "Sunday");
        assert_eq!(write_plan(&root, &plan).unwrap(), 2);
        assert_eq!(write_plan(&root, &plan).unwrap(), 2);

        let song = ShowFile::load(&root.join("Shows/pcosong_9.show")).unwrap();
        assert_eq!(song.name(), "Amazing Grace");
        assert_eq!(song.show.get("origin").and_then(Value::as_str), Some("pco"));
        assert_eq!(
            song.show.pointer("/meta/author").and_then(Value::as_str),
            Some("John Newton")
        );
        assert_eq!(
            song.show.pointer("/meta/CCLI").and_then(Value::as_str),
            Some("22025")
        );
        assert_eq!(
            song.show
                .pointer("/layouts/main/notes")
                .and_then(Value::as_str),
            Some("slow")
        );
        let slides = song.ordered_slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].group.as_deref(), Some("Verse 1"));
        assert_eq!(
            slides[0].lines,
            vec!["Amazing grace", "how sweet the sound"]
        );
        assert_eq!(slides[1].group.as_deref(), Some("Chorus"));
        assert_eq!(slides[1].lines, vec!["Praise God", "from whom"]);

        let notice = ShowFile::load(&root.join("Shows/pcosong_4.show")).unwrap();
        assert_eq!(
            notice.ordered_slides()[0].lines,
            vec!["Coffee after service"]
        );

        let projects = ProjectsFile::load(&root.join("Config/projects.json")).unwrap();
        let entries = projects.projects();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Sunday");
        assert_eq!(entries[0].shows.len(), 3);
        assert_eq!(entries[0].shows[0].label, "Welcome");
        assert_eq!(entries[0].shows[1].id, "pcosong_9");
        assert_eq!(entries[0].shows[2].id, "pcosong_4");
        let _ = std::fs::remove_dir_all(&root);
    }
}
