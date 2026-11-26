// SPDX-License-Identifier: GPL-3.0-only

//! Operator library edits: rename, duplicate and delete shows, the whole-show
//! text editor, slide delete/duplicate/reorder, and project rename/delete.
//!
//! Every action returns a short message for the status bar, or an error the
//! operator can read, instead of only writing to the log.

use freeshow_data::{Library, ProjectEntry, ProjectsFile, ShowFile};

use super::{Session, scripture_catalog, unique_name};
use freeshow_data::new_id;
use freeshow_media::{MediaKind, media_kind};

#[derive(Debug)]
pub struct MissingMedia {
    pub key: String,
    pub label: String,
    pub reference: String,
    kind: MediaKind,
    project: bool,
}

fn missing_local_media(root: Option<&std::path::Path>, reference: &str) -> Option<MediaKind> {
    if reference.is_empty() || reference.contains("://") || reference.starts_with("data:") {
        return None;
    }
    let kind = media_kind(std::path::Path::new(reference));
    (kind != MediaKind::Other && crate::live::resolve_media(root, reference).is_none())
        .then_some(kind)
}

/// A show file name FreeShow and every desktop OS accept: path separators and
/// reserved characters become spaces, and leading dots are dropped.
pub fn show_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned
        .split_whitespace()
        .filter(|word| !word.chars().all(|c| c == '.'))
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned.trim_start_matches('.').trim_end_matches(['.', ' ']);
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() {
        "Untitled".to_string()
    } else {
        cleaned
    }
}

/// What clicking a project item should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPick {
    /// A show is open for editing and presenting; nothing went live.
    Opened,
    /// Play or show this drawer row, the same as clicking it in the drawer.
    Drawer {
        tab: &'static str,
        label: &'static str,
        index: usize,
    },
    /// A section header is picked so it can be moved, renamed or removed.
    Section,
}

/// Projects newest first, the order the project picker lists them.
fn projects_newest_first(projects: &ProjectsFile) -> Vec<ProjectEntry> {
    let mut list = projects.projects();
    list.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.name.cmp(&b.name)));
    list
}

/// The project to open at startup: FreeShow's saved `activeProject`, else the newest.
pub fn startup_project(projects: &ProjectsFile, root: &std::path::Path) -> Option<String> {
    let list = projects_newest_first(projects);
    let saved = std::fs::read_to_string(root.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|settings| settings.get("activeProject")?.as_str().map(str::to_string));
    saved
        .filter(|id| list.iter().any(|project| &project.id == id))
        .or_else(|| list.first().map(|project| project.id.clone()))
}

impl Session {
    /// Project names for the picker, newest first.
    pub fn project_choices(&self) -> Vec<String> {
        projects_newest_first(&self.projects)
            .into_iter()
            .map(|project| project.name)
            .collect()
    }

    /// Picker index of the open project, or -1.
    pub fn project_choice_index(&self) -> i32 {
        projects_newest_first(&self.projects)
            .iter()
            .position(|project| Some(&project.id) == self.project_id.as_ref())
            .map_or(-1, |index| index as i32)
    }

    /// Stores the open project as FreeShow's `activeProject` so it reopens next time.
    pub(super) fn remember_project(&mut self) {
        let id = self.project_id.clone();
        self.patch_settings(|settings| {
            settings["activeProject"] =
                id.map_or(serde_json::Value::Null, serde_json::Value::String);
        });
    }

    fn open_project(&mut self, choice: &str) -> Result<String, String> {
        let index: usize = choice.trim().parse().map_err(|_| "pick a project")?;
        let project = projects_newest_first(&self.projects)
            .into_iter()
            .nth(index)
            .ok_or("that project no longer exists")?;
        if self.project_id.as_ref() == Some(&project.id) {
            return Ok(String::new());
        }
        self.project_id = Some(project.id);
        self.project_item = 0;
        self.project_picked = false;
        self.remember_project();
        Ok(format!("Opened project “{}”", project.name))
    }

    /// Brings files dropped on the window into the library: pictures and videos
    /// to `Media/`, audio to `Audio/`, Bibles to `Bibles/`, and anything else is
    /// imported as songs or presentations through `Imports/`.
    /// A file can be missing from a show's cues or from a standalone project row.
    pub fn missing_media(&self) -> Vec<MissingMedia> {
        let mut rows = Vec::new();
        let root = self.root.as_deref();
        let row = |id: &str, reference: &str, name: &str, project: bool| {
            let kind = missing_local_media(root, reference)?;
            let location = if project { "Project" } else { "Show" };
            let file = reference.rsplit(['/', '\\']).next().unwrap_or(reference);
            Some(MissingMedia {
                key: serde_json::json!([location, id, reference]).to_string(),
                label: format!("{file} · {location} “{name}”"),
                reference: reference.into(),
                kind,
                project,
            })
        };
        if let Some(show) = self.current_show() {
            let name = show.name();
            rows.extend(
                show.media_references()
                    .iter()
                    .filter_map(|path| row(&show.id, path, &name, false)),
            );
        }
        if let Some(project) = self.current_project() {
            let mut seen = std::collections::BTreeSet::new();
            rows.extend(
                project
                    .shows
                    .iter()
                    .filter(|entry| matches!(entry.kind.as_str(), "image" | "video" | "audio"))
                    .filter(|entry| seen.insert(entry.id.as_str()))
                    .filter_map(|entry| row(&project.id, &entry.id, &project.name, true)),
            );
        }
        rows
    }

    pub fn relink_missing_media(&mut self, key: &str, replacement: &str) -> Result<String, String> {
        let entry = self
            .missing_media()
            .into_iter()
            .find(|entry| entry.key == key)
            .ok_or("Select a file that is still missing")?;
        let root = self
            .root
            .as_deref()
            .ok_or("Choose a library folder first")?;
        let replacement = crate::live::resolve_media(Some(root), replacement.trim())
            .ok_or("The replacement file could not be found")?;
        if media_kind(&replacement) != entry.kind {
            return Err(
                "Choose the same kind of file as the missing picture, video or audio".into(),
            );
        }
        if replacement
            .metadata()
            .map_err(|error| error.to_string())?
            .len()
            == 0
        {
            return Err("The replacement file is empty".into());
        }
        if !entry.project {
            let show = self
                .current_show()
                .ok_or("Open the show that uses this file")?;
            if let Some(path) = show.path.as_deref()
                && self.has_pending_show_draft(path)
            {
                return Err("Resolve the show's recovery draft before relinking media".into());
            }
        }
        let library = Library::open(root);
        let directory = if entry.kind == MediaKind::Audio {
            library.audio_dir()
        } else {
            library.media_dir()
        };
        let saved = copy_into(&replacement, &directory)
            .map_err(|error| format!("Could not copy the replacement into the library: {error}"))?;
        let path = std::path::absolute(&saved).map_err(|error| error.to_string())?;
        let stored = path
            .to_str()
            .ok_or("The replacement path cannot be saved as UTF-8")?;
        if entry.project {
            let id = self
                .project_id
                .as_deref()
                .ok_or("Open the project that uses this file")?;
            let mut body = self
                .projects
                .project_body(id)
                .ok_or("Open the project that uses this file")?;
            let mut count = 0;
            if let Some(rows) = body["shows"].as_array_mut() {
                for row in rows {
                    if row["id"].as_str() == Some(&entry.reference)
                        && matches!(row["type"].as_str(), Some("image" | "video" | "audio"))
                    {
                        row["id"] = stored.into();
                        count += 1;
                    }
                }
            }
            if count == 0 {
                return Err("The missing media is no longer in this project".into());
            }
            let mut projects = self.projects.clone();
            if !projects.put_project(id, body) {
                return Err("Could not update the current project".into());
            }
            projects
                .save(&library.projects_path())
                .map_err(|error| format!("Could not save the relinked project: {error}"))?;
            self.projects = projects;
        } else {
            let snapshot = self
                .edit_snapshot("Relink media")
                .ok_or("Open the show that uses this file")?;
            let index = self.show_index.ok_or("Open the show that uses this file")?;
            if self.shows[index].relink_media(&entry.reference, stored) == 0 {
                return Err("The missing media is no longer in this show".into());
            }
            self.persist_show_edit(index, snapshot)?;
            if !self.timeline_playing {
                self.load_timeline();
            }
        }
        self.reload_local_media(&library);
        Ok(format!(
            "Relinked to {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    }

    fn reload_local_media(&mut self, library: &Library) {
        if let Ok(mut entries) = library.list_media_entries() {
            for camera in freeshow_media::cameras() {
                entries.push(freeshow_data::MediaEntry::camera(&camera));
            }
            self.media = entries;
        }
        if let Ok(list) = library.list_media_files(&library.audio_dir()) {
            self.audio = list;
        }
    }

    pub(crate) fn refresh_downloaded_media(&mut self, root: &std::path::Path) {
        if self.root.as_deref() == Some(root) {
            let selected = self
                .media_index
                .and_then(|index| self.media.get(index))
                .map(|entry| entry.path.clone());
            self.reload_local_media(&Library::open(root));
            self.media_index =
                selected.and_then(|path| self.media.iter().position(|entry| entry.path == path));
        }
    }

    pub fn import_files(&mut self, paths: &[std::path::PathBuf]) -> Result<String, String> {
        let root = self.root.clone().ok_or("choose a library folder first")?;
        let library = Library::open(&root);
        let (mut songs, mut media, mut audio, mut bibles, mut skipped) = (0, 0, 0, 0, 0);
        let mut copy_errors = Vec::new();
        for path in paths {
            if !path.is_file() {
                skipped += 1;
                continue;
            }
            let extension = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let (dir, counter) = match media_kind(path) {
                MediaKind::Image | MediaKind::Video => (library.media_dir(), &mut media),
                MediaKind::Audio => (library.audio_dir(), &mut audio),
                MediaKind::Other
                    if matches!(extension.as_str(), "fsb" | "usfm" | "sfm" | "usx") =>
                {
                    (library.bibles_dir(), &mut bibles)
                }
                MediaKind::Other => (library.imports_dir(), &mut songs),
            };
            match copy_into(path, &dir) {
                Ok(_) => *counter += 1,
                Err(err) => copy_errors.push(format!("could not copy {}: {err}", path.display())),
            }
        }
        let mut notes = Vec::new();
        let before = self.shows.len();
        let before_projects = self.projects.projects().len();
        let before_templates = self.templates.len();
        if songs > 0 {
            notes = library
                .import_show_sources()
                .map_err(|err| err.to_string())?;
            self.reload_import_catalog(None);
        }
        if bibles > 0 {
            if let Ok(found) = library.import_scripture_sources() {
                notes.extend(found);
            }
            if let Ok(list) = library.list_bibles() {
                self.scripture = scripture_catalog(&list);
                self.bibles = list;
            }
        }
        if media > 0 || audio > 0 || songs > 0 {
            self.reload_local_media(&library);
        }
        for note in &notes {
            tracing::warn!("import: {note}");
        }
        for error in &copy_errors {
            tracing::warn!("import: {error}");
        }
        let shows = self.shows.len().saturating_sub(before);
        let projects = self
            .projects
            .projects()
            .len()
            .saturating_sub(before_projects);
        let templates = self.templates.len().saturating_sub(before_templates);
        let mut parts = Vec::new();
        for (count, singular) in [
            (shows, "show"),
            (projects, "project"),
            (templates, "template"),
        ] {
            if count > 0 {
                parts.push(format!(
                    "{count} {singular}{}",
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
        if songs > 0 && parts.is_empty() {
            if let Some(error) = notes.first() {
                return Err(error.clone());
            }
            parts.push("0 new shows, projects or templates".into());
        }
        if media > 0 {
            parts.push(format!(
                "{media} media file{}",
                if media == 1 { "" } else { "s" }
            ));
        }
        if audio > 0 {
            parts.push(format!(
                "{audio} audio file{}",
                if audio == 1 { "" } else { "s" }
            ));
        }
        if bibles > 0 {
            parts.push(format!(
                "{bibles} Bible{}",
                if bibles == 1 { "" } else { "s" }
            ));
        }
        if parts.is_empty() {
            if let Some(error) = copy_errors.into_iter().next() {
                return Err(error);
            }
            return Err(if skipped > 0 {
                "drop files, not folders".into()
            } else {
                "nothing to import".into()
            });
        }
        let mut message = format!("Imported {}", parts.join(", "));
        if songs > 0 && shows == 0 {
            message = if notes
                .iter()
                .any(|note| note.contains("save as .pptx") || note.contains("pdftoppm"))
            {
                "Import did not add a show".into()
            } else {
                format!("{message} (no new shows; a show with that name may exist already)")
            };
        }
        if let Some(first) = notes.first() {
            let first = first.rsplit(['/', '\\']).next().unwrap_or(first);
            message.push_str(&format!(" · {first}"));
            if notes.len() > 1 {
                message.push_str(&format!(" · {} more in the log", notes.len() - 1));
            }
        }
        if !copy_errors.is_empty() {
            message.push_str(&format!(
                " · {} file(s) could not be copied",
                copy_errors.len()
            ));
        }
        Ok(message)
    }

    /// Opens the project item at `index`. A show opens without going live, as in
    /// FreeShow; media and audio items resolve to their drawer row.
    pub fn pick_project_item(&mut self, index: usize) -> Result<ProjectPick, String> {
        let item = self
            .current_project()
            .and_then(|project| project.shows.get(index).cloned())
            .ok_or("that project item no longer exists")?;
        self.project_item = index;
        self.project_picked = true;
        let label = self
            .project_labels()
            .get(index)
            .cloned()
            .unwrap_or_default();
        let missing = || format!("“{label}” is not in this library");
        self.search_query.clear();
        match item.kind.as_str() {
            "show" | "" => {
                let show = self
                    .shows
                    .iter()
                    .position(|show| show.id == item.id)
                    .ok_or_else(missing)?;
                self.hold_program();
                self.tab = "shows".to_string();
                if self.show_index != Some(show) {
                    self.show_index = Some(show);
                    // Opening a project show picks no slide until it is presented.
                    self.slide_index = None;
                    self.item_index = 0;
                    self.load_timeline();
                }
                Ok(ProjectPick::Opened)
            }
            "image" | "video" => {
                let wanted = crate::live::resolve_media(self.root.as_deref(), &item.id);
                let real = self
                    .media
                    .iter()
                    .position(|entry| wanted.as_ref() == Some(&entry.path) || entry.name == item.id)
                    .ok_or_else(missing)?;
                self.tab = "media".to_string();
                let index = self.visible_drawer_index(Some(real)).ok_or_else(missing)?;
                Ok(ProjectPick::Drawer {
                    tab: "media",
                    label: "Media",
                    index,
                })
            }
            "audio" => {
                let wanted = std::path::Path::new(&item.id)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| item.id.clone());
                let real = self
                    .audio
                    .iter()
                    .position(|name| {
                        name == &item.id
                            || std::path::Path::new(name)
                                .file_name()
                                .is_some_and(|file| file.to_string_lossy() == wanted)
                    })
                    .ok_or_else(missing)?;
                self.tab = "audio".to_string();
                let index = self.visible_drawer_index(Some(real)).ok_or_else(missing)?;
                Ok(ProjectPick::Drawer {
                    tab: "audio",
                    label: "Audio",
                    index,
                })
            }
            "section" => Ok(ProjectPick::Section),
            other => Err(format!("open this {other} item from its drawer tab")),
        }
    }

    /// Adds a section header below the picked project row, or at the end, and picks it.
    fn add_project_section(&mut self, name: &str) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let name = name.trim();
        let name = if name.is_empty() { "Section" } else { name };
        let index = self
            .highlighted_project_item()
            .map_or(self.project_labels().len(), |item| item + 1);
        if !self.projects.insert_section(&id, index, &new_id(), name) {
            return Err("could not add the section".into());
        }
        if !self.save_projects() {
            return Err("could not save projects.json".into());
        }
        self.project_item = index;
        self.project_picked = true;
        Ok(format!(
            "Added section “{name}”; type a new name under the project buttons"
        ))
    }

    /// Renames the picked section header.
    fn rename_project_section(&mut self, name: &str) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let name = name.trim();
        if name.is_empty() {
            return Err("a section name cannot be empty".into());
        }
        let index = self
            .highlighted_project_item()
            .ok_or("click the section to rename first")?;
        if !self.projects.rename_section(&id, index, name) {
            return Err("only a section header can be renamed here".into());
        }
        if !self.save_projects() {
            return Err("could not save projects.json".into());
        }
        Ok(format!("Renamed section to “{name}”"))
    }

    /// Runs one operator library action and returns the status-bar message.
    pub fn library_action(&mut self, action: &str, arg: &str) -> Result<String, String> {
        match action {
            "rename-show" => self.rename_show(arg),
            "duplicate-show" => self.duplicate_show(),
            "delete-show" => self.delete_show(),
            "restore-show" => self.restore_deleted_show(),
            "retry-draft" | "copy-draft" | "discard-draft" => self.resolve_show_draft(action),
            "show-text" => self.save_show_text(arg),
            "duplicate-slide" => self.edit_slides("Duplicate slide", |show, index| {
                show.duplicate_slide_at(index)
            }),
            "delete-slide" => self.edit_slides("Delete slide", |show, index| {
                show.remove_slide_at(index)
                    .then(|| index.min(show.ordered_slides().len().saturating_sub(1)))
            }),
            "slide-up" => {
                self.edit_slides("Move slide", |show, index| show.move_slide_at(index, -1))
            }
            "slide-down" => {
                self.edit_slides("Move slide", |show, index| show.move_slide_at(index, 1))
            }
            "slide-to-overlay" => self.save_slide_as("overlay", ""),
            "slide-to-template" => self.save_slide_as("template", arg),
            "template-new-show" => self.use_template_as_new(arg),
            "template-apply-style" => self.apply_browsed_template(arg),
            "template-toggle-favorite" => self.toggle_template_favorite(arg),
            "template-export" => self.export_browsed_template(arg),
            "open-project" => self.open_project(arg),
            "rename-project" => self.rename_project(arg),
            "delete-project" => self.delete_current_project(),
            "remove-project-item" => self.remove_project_item(),
            "add-section" => self.add_project_section(arg),
            "rename-section" => self.rename_project_section(arg),
            "new-drawer-entry" => self.new_drawer_entry(),
            "save-event" => self.save_event(arg),
            "delete-event" => self.delete_event(),
            "save-player" => self.save_player_video(arg),
            "delete-player" => self.delete_player_video(),
            "save-rss" => self.save_rss_feeds(arg),
            other => Err(format!("unknown action {other:?}")),
        }
        .inspect(|message| self.notice = message.clone())
        .inspect_err(|error| {
            tracing::warn!("{action}: {error}");
            self.notice = error.clone();
        })
    }

    fn toggle_template_favorite(&mut self, id: &str) -> Result<String, String> {
        let Some((_, name, _)) = self
            .selected_template()
            .filter(|(selected, _, _)| *selected == id)
        else {
            return Err("Select a template first".into());
        };
        let root = self
            .root
            .as_deref()
            .ok_or("Choose a library folder first")?;
        let library = Library::open(root);
        let message = if self.is_template_favorite(id) {
            format!("Removed “{name}” from favorites")
        } else {
            format!("Added “{name}” to favorites")
        };
        let save_error = |error| format!("Could not save template favorites. {error}");
        let mut saved = library.load_template_favorites().map_err(save_error)?;
        if self.is_template_favorite(id) {
            saved.remove(id);
        } else {
            saved.insert(id.to_owned());
        }
        library
            .save_template_favorites(&saved)
            .map_err(save_error)?;
        self.template_favorites = saved;
        Ok(message)
    }

    fn export_browsed_template(&self, id: &str) -> Result<String, String> {
        let Some((_, name, template)) = self.selected_template().filter(|(key, _, _)| *key == id)
        else {
            return Err("Select a template first".into());
        };
        let root = self
            .root
            .as_deref()
            .ok_or("Choose a library folder first")?;
        let path = self
            .exports_dir()?
            .join(format!("{} - Template.project", super::export_stem(name)));
        let warnings =
            freeshow_data::write_portable_template(id, template, &Library::open(root), &path)
                .map_err(|err| err.to_string())?;
        if warnings.is_empty() {
            Ok(format!("Exported template to {}", path.display()))
        } else {
            Ok(format!(
                "Exported template with {} dependency warnings · {}",
                warnings.len(),
                warnings.join(" · ")
            ))
        }
    }

    fn use_template_as_new(&mut self, id: &str) -> Result<String, String> {
        let Some((_, name, template)) = self.selected_template().filter(|(key, _, _)| *key == id)
        else {
            return Err("Select a template first".into());
        };
        let name = unique_name(name, self.shows.iter().map(ShowFile::name).collect());
        let show = ShowFile::from_template(&new_id(), &name, template)
            .ok_or_else(|| format!("“{name}” has no usable layers"))?;
        self.create_library_show(show)?;
        self.set_search("");
        Ok(format!("Opened a new show from the template as “{name}”"))
    }

    fn apply_browsed_template(&mut self, id: &str) -> Result<String, String> {
        let Some((_, name, template)) = self.selected_template().filter(|(key, _, _)| *key == id)
        else {
            return Err("Select a template first".into());
        };
        if freeshow_data::template_text_item(template).is_none() {
            return Err("This template has no text style. Use as New to copy its layout.".into());
        }
        let name = name.to_owned();
        let target = self
            .template_target_show()
            .ok_or("Open a show to apply the style")?;
        self.notice.clear();
        if self.apply_template_id(id) {
            self.set_search("");
            Ok(format!("Applied “{name}” to “{target}”"))
        } else if self.notice.is_empty() {
            Err("Could not apply the template style".into())
        } else {
            Err(self.notice.clone())
        }
    }

    /// Saves the selected slide's boxes as a new overlay or template in the library.
    fn save_slide_as(&mut self, kind: &str, requested_name: &str) -> Result<String, String> {
        let index = self.selected_show()?;
        let show = &self.shows[index];
        let slides = show.ordered_slides();
        let slide = self
            .slide_index
            .and_then(|position| slides.get(position))
            .ok_or("select a slide first")?;
        let items = show.slide_items(&slide.id);
        if items.is_empty() {
            return Err("this slide has nothing to save".into());
        }
        let root = self.root.clone().ok_or("choose a library folder first")?;
        let library = Library::open(&root);
        let (base, catalog, path) = if kind == "overlay" {
            let base = slide
                .group
                .clone()
                .filter(|group| !group.trim().is_empty())
                .or_else(|| slide.lines.first().cloned())
                .unwrap_or_else(|| show.name());
            (base, &mut self.overlay_catalog, library.overlays_path())
        } else {
            (
                show.name(),
                &mut self.template_catalog,
                library.templates_path(),
            )
        };
        let base = if requested_name.trim().is_empty() {
            base.trim()
        } else {
            requested_name.trim()
        };
        let name = unique_name(base, catalog.names());
        let mut entry =
            serde_json::json!({ "name": name, "color": null, "category": null, "items": items });
        if kind == "template" {
            let design = &show.show["slides"][&slide.id]["settings"];
            entry["settings"] = serde_json::json!({});
            if let Some(color) = design.get("color").filter(|value| value.is_string()) {
                entry["settings"]["backgroundColor"] = color.clone();
            }
            if let Some(size) = design.get("resolution").filter(|value| value.is_object()) {
                entry["settings"]["resolution"] = size.clone();
            }
        }
        let id = new_id();
        let mut saved = catalog.clone();
        saved.insert(&id, entry);
        saved
            .save(&path)
            .map_err(|err| format!("could not save the {kind}: {err}"))?;
        let names = saved.names();
        *catalog = saved;
        if kind == "overlay" {
            self.overlays = names;
        } else {
            self.templates = names;
            self.template_pick = Some(id);
        }
        Ok(format!("Saved {kind} \u{201c}{name}\u{201d}"))
    }

    /// True right after a delete, while the deleted show can still be brought back.
    pub fn can_restore_show(&self) -> bool {
        !self.deleted_shows.is_empty()
    }

    /// Current show name, or empty when no show is selected.
    pub fn show_name(&self) -> String {
        if self.tab != "shows" {
            return String::new();
        }
        self.current_show().map(ShowFile::name).unwrap_or_default()
    }

    /// The whole show as editor text (`[Group]` headers, blank line between slides).
    pub fn show_text(&self) -> String {
        if self.tab != "shows" {
            return String::new();
        }
        self.current_show()
            .map(ShowFile::lyrics_text)
            .unwrap_or_default()
    }

    fn shows_dir(&self) -> Result<std::path::PathBuf, String> {
        let root = self.root.as_ref().ok_or("choose a library folder first")?;
        Ok(Library::open(root).shows_dir())
    }

    fn selected_show(&self) -> Result<usize, String> {
        if self.tab != "shows" {
            return Err("open the Shows tab first".into());
        }
        self.show_index
            .filter(|index| *index < self.shows.len())
            .ok_or_else(|| "select a show first".into())
    }

    /// Inserts a show in name order, selects it, and returns its index.
    pub(super) fn insert_sorted(&mut self, show: ShowFile) -> usize {
        let name = show.name().to_lowercase();
        let pos = self
            .shows
            .iter()
            .position(|existing| existing.name().to_lowercase() > name)
            .unwrap_or(self.shows.len());
        self.shows.insert(pos, show);
        self.show_index = Some(pos);
        self.slide_index = self.shows[pos].first_enabled_slide().or(Some(0));
        self.item_index = 0;
        pos
    }

    fn free_show_path(
        &self,
        name: &str,
        keep: Option<&std::path::Path>,
    ) -> Result<std::path::PathBuf, String> {
        let dir = self.shows_dir()?;
        let base = show_file_name(name);
        let mut path = dir.join(format!("{base}.show"));
        let mut number = 2;
        while path.exists() && Some(path.as_path()) != keep {
            path = dir.join(format!("{base} {number}.show"));
            number += 1;
        }
        Ok(path)
    }

    fn rename_show(&mut self, name: &str) -> Result<String, String> {
        let index = self.selected_show()?;
        self.check_pending_show_draft(index)?;
        let name = name.trim();
        if name.is_empty() {
            return Err("a show name cannot be empty".into());
        }
        let old_name = self.shows[index].name();
        if old_name == name {
            return Ok(String::new());
        }
        let others: Vec<String> = self
            .shows
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, show)| show.name())
            .collect();
        let name = unique_name(name, others);
        let old_path = self.shows[index].path.clone();
        let mut show = self.shows.remove(index);
        show.set_name(&name);
        let path = self.free_show_path(&name, old_path.as_deref())?;
        if let Err(err) = show.save(&path) {
            let _ = show.set_name(&old_name);
            self.shows.insert(index, show);
            return Err(format!("could not save {}: {err}", path.display()));
        }
        if let Some(old) = old_path.filter(|old| *old != path) {
            let _ = std::fs::remove_file(old);
        }
        show.path = Some(path);
        self.timer_save_bases.remove(&show.id);
        let slide = self.slide_index;
        self.insert_sorted(show);
        self.slide_index = slide;
        Ok(format!("Renamed “{old_name}” to “{name}”"))
    }

    fn duplicate_show(&mut self) -> Result<String, String> {
        let index = self.selected_show()?;
        let source = &self.shows[index];
        let name = unique_name(
            &format!("{} copy", source.name()),
            self.shows.iter().map(ShowFile::name).collect(),
        );
        let mut copy = ShowFile {
            id: new_id(),
            show: source.show.clone(),
            path: None,
        };
        copy.set_name(&name);
        let path = self.free_show_path(&name, None)?;
        copy.save(&path)
            .map_err(|err| format!("could not save {}: {err}", path.display()))?;
        copy.path = Some(path);
        self.insert_sorted(copy);
        Ok(format!("Created “{name}”"))
    }

    fn delete_show(&mut self) -> Result<String, String> {
        let index = self.selected_show()?;
        self.check_pending_show_draft(index)?;
        let name = self.shows[index].name();
        if let Some(path) = self.shows[index].path.clone()
            && path.exists()
        {
            std::fs::remove_file(&path)
                .map_err(|err| format!("could not delete {}: {err}", path.display()))?;
        }
        let show = self.shows.remove(index);
        self.timer_save_bases.remove(&show.id);
        self.history.retain(|snap| snap.show_id != show.id);
        self.redo.retain(|snap| snap.show_id != show.id);
        self.deleted_shows.push(show);
        if self.deleted_shows.len() > 20 {
            self.deleted_shows.remove(0);
        }
        self.show_index = None;
        self.slide_index = None;
        self.item_index = 0;
        Ok(format!("Deleted “{name}”"))
    }

    fn restore_deleted_show(&mut self) -> Result<String, String> {
        let mut show = self.deleted_shows.pop().ok_or("nothing to restore")?;
        let name = show.name();
        let path = self.free_show_path(&name, None)?;
        if let Err(err) = show.save(&path) {
            self.deleted_shows.push(show);
            return Err(format!("could not restore {}: {err}", path.display()));
        }
        show.path = Some(path);
        self.tab = "shows".to_string();
        self.insert_sorted(show);
        Ok(format!("Restored “{name}”"))
    }

    fn save_show_text(&mut self, text: &str) -> Result<String, String> {
        let index = self.selected_show()?;
        if self.shows[index].lyrics_text() == text.trim_end() {
            return Ok(String::new());
        }
        if self.shows[index].has_locked_layers() {
            return Err("Unlock the show’s layers before replacing its text".into());
        }
        let snap = self
            .edit_snapshot("Edit show text")
            .ok_or("select a show first")?;
        let split = self.preferences.split_lines();
        if !self.shows[index].set_lyrics_text(text, split) {
            return Err("could not update the show text".into());
        }
        self.persist_show_edit(index, snap)?;
        self.layer_selection = None;
        let count = self.shows[index].ordered_slides().len();
        let last = count.saturating_sub(1);
        self.slide_index = Some(self.slide_index.unwrap_or(0).min(last));
        self.item_index = 0;
        Ok(format!(
            "Saved {count} slide{}",
            if count == 1 { "" } else { "s" }
        ))
    }

    fn edit_slides(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut ShowFile, usize) -> Option<usize>,
    ) -> Result<String, String> {
        let index = self.selected_show()?;
        let slide = self.slide_index.ok_or("select a slide first")?;
        let snap = self.edit_snapshot(label).ok_or("select a show first")?;
        let Some(next) = edit(&mut self.shows[index], slide) else {
            return Err(match label {
                "Delete slide" => "a show keeps at least one slide".into(),
                "Move slide" => "the slide cannot move further".into(),
                _ => format!("{label} failed"),
            });
        };
        self.persist_show_edit(index, snap)?;
        self.layer_selection = None;
        self.slide_index = Some(next);
        self.item_index = 0;
        Ok(String::new())
    }

    fn rename_project(&mut self, name: &str) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let name = name.trim();
        if name.is_empty() {
            return Err("a project name cannot be empty".into());
        }
        if !self.projects.set_project_name(&id, name) {
            return Err("could not rename the project".into());
        }
        if !self.save_projects() {
            return Err("could not save projects.json".into());
        }
        Ok(format!("Renamed project to “{name}”"))
    }

    fn delete_current_project(&mut self) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let name = self.project_title();
        if !self.projects.delete_project(&id) {
            return Err("could not delete the project".into());
        }
        if !self.save_projects() {
            return Err("could not save projects.json".into());
        }
        self.project_id = projects_newest_first(&self.projects)
            .first()
            .map(|project| project.id.clone());
        self.project_item = 0;
        self.project_picked = false;
        self.remember_project();
        Ok(format!("Deleted project “{name}”"))
    }

    fn remove_project_item(&mut self) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let index = self
            .highlighted_project_item()
            .ok_or("click the project item to remove first")?;
        let label = self
            .project_labels()
            .get(index)
            .cloned()
            .ok_or("select a project item first")?;
        if !self.projects.remove_show_at(&id, index) {
            return Err("could not remove the item".into());
        }
        if !self.save_projects() {
            return Err("could not save projects.json".into());
        }
        let len = self.project_labels().len();
        self.project_item = index.min(len.saturating_sub(1));
        self.project_picked = false;
        Ok(format!("Removed “{label}” from the project"))
    }
}

/// Copies `path` into `dir`, keeping an existing file with the same name and contents.
fn same_file_contents(left: &std::path::Path, right: &std::path::Path) -> std::io::Result<bool> {
    use std::io::{BufRead, BufReader};
    let mut left = BufReader::new(std::fs::File::open(left)?);
    let mut right = BufReader::new(std::fs::File::open(right)?);
    loop {
        let a = left.fill_buf()?;
        let b = right.fill_buf()?;
        let size = a.len().min(b.len());
        if size == 0 {
            return Ok(a.is_empty() && b.is_empty());
        }
        if a[..size] != b[..size] {
            return Ok(false);
        }
        left.consume(size);
        right.consume(size);
    }
}

fn copy_into(path: &std::path::Path, dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut target = dir.join(name);
    let size = std::fs::metadata(path)?.len();
    let mut number = 2;
    while target.exists() {
        if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == size)
            && same_file_contents(&target, path)?
        {
            return Ok(target);
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        target = dir.join(format!("{stem} {number}{ext}"));
        number += 1;
    }
    let mut source = std::fs::File::open(path)?;
    let candidate = dir.join(format!(".freeshow-copy-{}.part", new_id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)?;
        std::io::copy(&mut source, &mut file)?;
        file.sync_all()?;
        drop(file);
        // The normal importer has one writer. Guard the destination if another process has added it.
        let reservation = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        drop(reservation);
        if let Err(error) = std::fs::rename(&candidate, &target) {
            let _ = std::fs::remove_file(&target);
            return Err(error);
        }
        Ok(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(candidate);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn library(name: &str) -> (Dir, Session) {
        let dir =
            std::env::temp_dir().join(format!("freeshow-libedit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Shows")).unwrap();
        std::fs::create_dir_all(dir.join("Config")).unwrap();
        let session = Session::open(dir.clone());
        (Dir(dir), session)
    }

    fn files(dir: &Dir) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.0.join("Shows"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn dropped_files_import_the_rest_when_one_destination_is_unwritable() {
        let (dir, mut session) = library("drop-partial");
        let outside = dir.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        // A file at the Audio path prevents directory creation on every platform.
        std::fs::write(dir.0.join("Audio"), b"obstruction").unwrap();
        let audio = outside.join("service.wav");
        let song = outside.join("Welcome.txt");
        std::fs::write(&audio, b"audio").unwrap();
        std::fs::write(&song, "Welcome to the service").unwrap();

        let result = session.import_files(&[audio, song]).unwrap();
        assert!(result.contains("1 show"), "{result}");
        assert!(result.contains("1 file(s) could not be copied"), "{result}");
        assert!(session.shows.iter().any(|show| show.name() == "Welcome"));
    }

    #[test]
    fn dropped_files_land_in_the_right_folders_and_import_once() {
        let (dir, mut session) = library("drop");
        let outside = dir.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let song = outside.join("Blessed Assurance.txt");
        std::fs::write(
            &song,
            "Blessed assurance\nJesus is mine\n\nThis is my story",
        )
        .unwrap();
        let picture = outside.join("cross.png");
        std::fs::write(&picture, b"png").unwrap();
        let message = session
            .import_files(&[song.clone(), picture.clone()])
            .unwrap();
        assert!(
            message.starts_with("Imported 1 show, 1 media file"),
            "{message}"
        );
        assert!(
            session
                .shows
                .iter()
                .any(|show| show.name() == "Blessed Assurance")
        );
        assert!(dir.0.join("Media/cross.png").exists());
        assert!(
            dir.0
                .join("Imports/Imported/Blessed Assurance.txt")
                .exists()
        );
        // A deleted imported show does not come back on the next start.
        session.tab = "shows".into();
        session.show_index = session
            .shows
            .iter()
            .position(|show| show.name() == "Blessed Assurance");
        session.library_action("delete-show", "").unwrap();
        let reopened = Session::open(dir.0.clone());
        assert!(
            reopened
                .shows
                .iter()
                .all(|show| show.name() != "Blessed Assurance")
        );
        assert!(
            session
                .import_files(std::slice::from_ref(&outside))
                .is_err()
        );
    }

    #[test]
    fn clicking_a_project_show_opens_it_without_going_live() {
        let (_dir, mut session) = library("pick");
        assert!(session.create_project());
        assert!(session.create_show());
        session.library_action("show-text", "One\n\nTwo").unwrap();
        assert!(session.create_show());
        session.tab = "media".into();
        assert_eq!(session.pick_project_item(0), Ok(ProjectPick::Opened));
        assert_eq!(session.tab, "shows");
        assert_eq!(session.show_name(), "Untitled");
        assert_eq!(session.slide_index, None, "no slide looks picked yet");
        assert!(
            session.program_content.borrow().is_none(),
            "nothing went live"
        );
        // The first Next presents the first slide, not the second.
        assert!(session.advance_show());
        assert_eq!(session.slide_index, Some(0));
        assert!(session.pick_project_item(5).is_err());
    }

    #[test]
    fn file_names_are_safe_on_every_platform() {
        assert_eq!(show_file_name("AC/DC: Back?"), "AC DC Back");
        assert_eq!(show_file_name("../../etc"), "etc");
        assert_eq!(show_file_name("  "), "Untitled");
        assert_eq!(show_file_name("Holy, Holy, Holy"), "Holy, Holy, Holy");
    }

    #[test]
    fn a_song_is_written_renamed_duplicated_deleted_and_restored() {
        let (dir, mut session) = library("flow");
        assert!(session.create_show());
        session
            .library_action(
                "show-text",
                "[Verse 1]\nAmazing grace\n\n[Chorus]\nMy chains are gone",
            )
            .unwrap();
        assert_eq!(session.current_show().unwrap().ordered_slides().len(), 2);
        session
            .library_action("rename-show", "Amazing Grace / Chains")
            .unwrap();
        assert_eq!(session.show_name(), "Amazing Grace / Chains");
        assert_eq!(files(&dir), ["Amazing Grace Chains.show"]);
        session.library_action("duplicate-show", "").unwrap();
        assert_eq!(session.show_name(), "Amazing Grace / Chains copy");
        assert_eq!(files(&dir).len(), 2);
        assert_eq!(
            session.show_text(),
            "[Verse 1]\nAmazing grace\n\n[Chorus]\nMy chains are gone"
        );
        session.library_action("delete-show", "").unwrap();
        assert_eq!(files(&dir), ["Amazing Grace Chains.show"]);
        assert!(session.can_restore_show());
        session.library_action("restore-show", "").unwrap();
        assert_eq!(files(&dir).len(), 2);
        assert_eq!(session.show_name(), "Amazing Grace / Chains copy");
        let reloaded = Session::open(dir.0.clone());
        assert_eq!(reloaded.shows.len(), 2);
    }

    #[test]
    fn slide_edits_save_and_undo() {
        let (_dir, mut session) = library("slides");
        assert!(session.create_show());
        session.library_action("show-text", "A\n\nB\n\nC").unwrap();
        session.slide_index = Some(2);
        session.library_action("slide-up", "").unwrap();
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.show_text(), "A\n\nC\n\nB");
        session.library_action("duplicate-slide", "").unwrap();
        assert_eq!(session.slide_index, Some(2));
        assert_eq!(session.show_text(), "A\n\nC\n\nC\n\nB");
        session.library_action("delete-slide", "").unwrap();
        assert_eq!(session.show_text(), "A\n\nC\n\nB");
        assert!(session.undo());
        assert_eq!(session.show_text(), "A\n\nC\n\nC\n\nB");
        session.slide_index = Some(0);
        assert!(session.library_action("slide-up", "").is_err());
        assert_eq!(session.notice, "the slide cannot move further");
    }

    #[test]
    fn a_slide_can_become_an_overlay_or_a_template() {
        let (dir, mut session) = library("as-overlay");
        assert!(session.create_show());
        session.library_action("rename-show", "Notices").unwrap();
        session
            .library_action("show-text", "[Welcome]\nWelcome home\n\nCoffee after")
            .unwrap();
        session.slide_index = Some(0);
        session.library_action("slide-to-overlay", "").unwrap();
        session.library_action("slide-to-overlay", "").unwrap();
        assert_eq!(session.overlays, ["Welcome", "Welcome 2"]);
        session.slide_index = Some(1);
        session.library_action("slide-to-template", "").unwrap();
        assert_eq!(session.templates, ["Notices"]);
        let reloaded = Session::open(dir.0.clone());
        assert_eq!(reloaded.overlays, ["Welcome", "Welcome 2"]);
        assert_eq!(reloaded.templates, ["Notices"]);
        let id = reloaded.overlay_catalog.id_named("Welcome").unwrap();
        let text = reloaded.overlay_catalog.get(&id).unwrap().to_string();
        assert!(text.contains("Welcome home"), "{text}");
    }

    #[test]
    fn templates_browse_by_id_apply_style_with_undo_and_make_independent_shows() {
        let (dir, mut session) = library("template-gallery");
        std::fs::create_dir_all(dir.0.join("Media")).unwrap();
        let picture = freeshow_output::Frame {
            width: 1,
            height: 1,
            pixels: vec![230, 140, 45, 255],
        };
        std::fs::write(
            dir.0.join("Media/Poster.png"),
            picture.encode_png().unwrap(),
        )
        .unwrap();
        assert!(session.create_show());
        session.library_action("rename-show", "Original").unwrap();
        assert!(session.save_lyrics("Template example"));
        assert!(session.edit_selected_slide("Design poster", |show, slide, _| {
            let slide = &mut show.show["slides"][slide];
            slide["settings"]["color"] = serde_json::json!("#203040");
            slide["settings"]["resolution"] = serde_json::json!({"width": 1000, "height": 1000});
            slide["items"][0]["style"] = serde_json::json!("width:500px;height:200px;left:100px;top:80px;");
            slide["items"].as_array_mut().unwrap().push(serde_json::json!({
                "type": "media", "src": "Media/Poster.png", "style": "width:300px;height:200px;left:200px;top:400px;",
                "custom": {"keep":true}
            }));
            true
        }));
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        session
            .library_action("slide-to-template", "Welcome poster")
            .unwrap();
        let library = Library::open(&dir.0);
        let on_disk = freeshow_data::Catalog::load(&library.templates_path()).unwrap();
        let id = on_disk.id_named("Welcome poster").unwrap();
        let original_template = on_disk.get(&id).unwrap();
        assert_eq!(original_template["settings"]["backgroundColor"], "#203040");

        let gallery_image = crate::live::catalog_thumbnail(&session, original_template, 128, 72);
        assert_eq!((gallery_image.width, gallery_image.height), (72, 72));
        assert_eq!(gallery_image.pixel(25, 36), picture.pixel(0, 0));
        session.template_catalog.insert(
            "same-name",
            serde_json::json!({
                "name": "Welcome poster", "items": [{"type": "text", "style": "left:5px;"}]
            }),
        );
        session.switch_tab("templates");
        session.set_search("Welcome poster");
        assert_eq!(session.template_entries().len(), 2);
        assert!(session.browse_template("same-name"));
        assert_eq!(session.selected_template().unwrap().0, "same-name");
        assert_eq!(session.current_show().unwrap().name(), "Original");
        assert!(session.browse_template(&id));
        session.set_search("no match");
        assert!(session.selected_template().is_none());
        assert!(session.library_action("template-new-show", &id).is_err());
        session.set_search("poster");
        assert_eq!(session.selected_template().unwrap().0, id);
        assert!(session.library_action("template-new-show", &id).is_ok());
        assert_eq!(session.show_name(), "Welcome poster");
        assert!(session.search_query.is_empty());
        let created = session.current_show().unwrap();
        let persisted = ShowFile::load(created.path.as_deref().unwrap()).unwrap();
        let slide = &persisted.show["slides"]["s1"];
        assert_eq!(slide["settings"]["resolution"]["width"], 1000);
        assert_eq!(slide["settings"]["color"], "#203040");
        assert_eq!(slide["items"], original_template["items"]);
        assert!(session.save_lyrics("Independent new copy"));
        assert_eq!(session.template_catalog.get(&id), Some(original_template));

        assert!(session.create_show());
        assert!(session.save_lyrics("Keep my actual words"));
        let before = session.current_show().unwrap().show.clone();
        let history = session.history.len();
        session.switch_tab("templates");
        assert!(session.browse_template(&id));
        session.library_action("template-apply-style", &id).unwrap();
        assert_eq!(session.history.len(), history + 1);
        assert_eq!(session.lyrics(), "Keep my actual words");
        assert_eq!(
            session.current_show().unwrap().show["slides"]["s1"]["items"][0]["style"],
            original_template["items"][0]["style"]
        );
        assert!(session.undo());
        assert_eq!(session.current_show().unwrap().show, before);
        assert_eq!(crate::live::audience_frame(&session), on_air);
    }

    #[test]
    fn a_relinked_offline_video_reopens_and_previews_without_changing_the_program() {
        let (dir, mut session) = library("relink-video");
        assert!(session.create_show());
        assert!(session.save_lyrics("Keep the live cue"));
        let missing = dir.0.join("Lost/Story.mp4");
        let replacement = dir.0.join("Replacement.mp4");
        std::fs::write(
            &replacement,
            include_bytes!("../../../freeshow-data/tests/fixtures/lesson-clip.mp4"),
        )
        .unwrap();
        assert!(
            session.edit_selected_slide("Add a video cue", |show, _, _| {
                show.show["media"]["clip"] = serde_json::json!({
                    "path": missing,
                    "loop": false,
                    "muted": false,
                });
                show.show["layouts"]["main"]["slides"][0]["background"] = "clip".into();
                true
            })
        );
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        let before = session.current_show().unwrap().show.clone();
        let key = session.missing_media().into_iter().next().unwrap().key;
        let folder = dir.0.join("Media");
        std::fs::write(&folder, "blocked media directory").unwrap();
        assert!(
            session
                .relink_missing_media(&key, replacement.to_str().unwrap())
                .is_err()
        );
        assert_eq!(session.current_show().unwrap().show, before);
        std::fs::remove_file(&folder).unwrap();

        session.switch_tab("media");
        session
            .relink_missing_media(&key, replacement.to_str().unwrap())
            .unwrap();
        assert_eq!(session.last_edit_label(), Some("Relink media"));
        assert!(session.missing_media().is_empty());
        assert_eq!(crate::live::audience_frame(&session), on_air);
        let copied = folder.join("Replacement.mp4");
        let chosen = session
            .media_preview_entry(copied.to_str().unwrap())
            .unwrap();
        let mut player = crate::open_muted_media_preview(&chosen.path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let first = loop {
            if let Some(frame) = player.try_next_frame() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "The local preview did not decode"
            );
            std::thread::sleep(std::time::Duration::from_millis(15));
        };
        assert!(first.width > 0);
        drop(player);
        std::fs::remove_file(replacement).unwrap();

        assert!(session.undo());
        assert_eq!(session.current_show().unwrap().show, before);
        assert!(session.redo());
        let mut reopened = Session::open(dir.0.clone());
        assert!(reopened.present_show_id(&session.current_show().unwrap().id));
        let Some(crate::session::PendingPlay::Video(path)) =
            reopened.take_plays().into_iter().flatten().next()
        else {
            panic!("The relinked cue must queue the copied video");
        };
        assert_eq!(path, copied);
        assert!(!reopened.media_loop && !reopened.media_muted);
    }

    #[test]
    fn relinking_a_project_picture_preserves_existing_files_and_cue_order() {
        let (dir, mut session) = library("relink-project");
        assert!(session.create_project());
        assert!(session.create_show());
        let id = session.project_id.clone().unwrap();
        let mut body = session.projects.project_body(&id).unwrap();
        body["shows"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id":"Lost/Picture.png",
                "type":"image",
                "name":"Project cue"
            }));
        assert!(session.projects.put_project(&id, body));
        assert!(session.save_projects());
        let media = dir.0.join("Media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("Picture.png"), b"existing file").unwrap();
        let source = dir.0.join("Picture.png");
        let image = include_bytes!("../../../freeshow-data/tests/fixtures/lesson-cover.png");
        std::fs::write(&source, image).unwrap();

        let key = session
            .missing_media()
            .into_iter()
            .find(|entry| entry.project)
            .unwrap()
            .key;
        assert!(
            session
                .relink_missing_media(&key, source.to_str().unwrap())
                .is_ok()
        );
        assert_eq!(
            std::fs::read(media.join("Picture.png")).unwrap(),
            b"existing file"
        );
        let project = session.current_project().unwrap();
        assert_eq!(project.shows[0].kind, "show");
        assert_eq!(project.shows[1].label, "Project cue");
        assert_eq!(std::fs::read(&project.shows[1].id).unwrap(), image);
        assert!(matches!(
            session.pick_project_item(1),
            Ok(ProjectPick::Drawer { tab: "media", .. })
        ));
        assert!(session.missing_media().is_empty());
    }

    #[test]
    fn exported_template_appears_in_an_open_receiving_library_after_drop() {
        let (source, mut session) = library("share-template");
        let (_destination, mut receiver) = library("receive-template");
        assert!(session.create_show());
        assert!(session.save_lyrics("Reusable wording"));
        assert!(
            session.edit_selected_slide("Set template font", |show, slide, _| {
                show.set_item_font_family(slide, 0, "Arial")
            })
        );
        session
            .library_action("slide-to-template", "Welcome")
            .unwrap();
        session.switch_tab("templates");
        let id = session.template_catalog.id_named("Welcome").unwrap();
        assert!(session.browse_template(&id));
        let notice = session.library_action("template-export", &id).unwrap();
        assert!(notice.contains("Verify or install these fonts for this package (Arial)"));
        let manifest = receiver
            .import_files(&[source.0.join("Exports/Welcome - Template.project")])
            .unwrap();
        assert_eq!(
            manifest,
            "Imported 1 template · Verify or install these fonts for this package (Arial)"
        );
        assert!(receiver.projects.projects().is_empty());
        assert_eq!(receiver.templates, ["Welcome"]);
        receiver.switch_tab("templates");
        assert!(receiver.browse_template(&id));
        receiver.library_action("template-new-show", &id).unwrap();
        assert_eq!(receiver.lyrics(), "Reusable wording");
        assert!(receiver.save_lyrics("Local edits after import"));
        receiver.switch_tab("templates");
        assert!(receiver.browse_template(&id));
        receiver.library_action("template-new-show", &id).unwrap();
        assert_eq!(receiver.lyrics(), "Reusable wording");
    }

    #[test]
    fn template_favorites_keep_stable_ids_and_filter_the_keyboard_gallery() {
        let (dir, mut session) = library("favorite-templates");
        let library = Library::open(&dir.0);
        let entry = serde_json::json!({"name":"Équipe Welcome","items":[{"style":"color:#fff;"}]});
        for id in ["alpha", "beta"] {
            session.template_catalog.insert(id, entry.clone());
        }
        session
            .template_catalog
            .save(&library.templates_path())
            .unwrap();
        session.switch_tab("templates");
        assert!(session.browse_template("beta"));
        session
            .library_action("template-toggle-favorite", "beta")
            .unwrap();
        assert!(!session.is_template_favorite("alpha"));
        assert!(session.is_template_favorite("beta"));
        assert_eq!(library.load_templates().unwrap(), session.template_catalog);
        session.library_action("template-export", "beta").unwrap();
        let (_receiver_dir, mut receiver) = self::library("receive-favorite-template");
        receiver
            .import_files(&[dir.0.join("Exports/Équipe Welcome - Template.project")])
            .unwrap();
        assert!(!receiver.is_template_favorite("beta"));

        let mut reopened = Session::open(dir.0.clone());
        assert!(reopened.is_template_favorite("beta"));
        reopened.switch_tab("templates");
        reopened.set_search("ÉQUIPE");
        reopened.template_favorites_only = true;
        assert_eq!(reopened.drawer(), ["Équipe Welcome"]);
        assert!(!reopened.browse_template("alpha"));
        reopened.select_drawer(0);
        assert_eq!(reopened.selected_template().unwrap().0, "beta");
        reopened
            .library_action("template-toggle-favorite", "beta")
            .unwrap();
        assert!(reopened.drawer().is_empty());
        assert!(reopened.selected_template().is_none());
        assert!(library.load_template_favorites().unwrap().is_empty());
    }

    #[test]
    fn bundled_starters_can_be_previewed_customized_and_shared_from_a_clean_library() {
        let (dir, mut session) = library("starter-templates");
        let library = Library::open(&dir.0);
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.save_lyrics("Already on air"));
        session.select_slide(0);
        let live = crate::live::audience_frame(&session);
        session.switch_tab("templates");
        let rows = session.template_entries();
        assert_eq!(rows.len(), 5);
        for (id, _, design) in rows {
            assert!(session.is_starter_template(id));
            let thumbnail = crate::live::catalog_thumbnail(&session, design, 128, 72);
            let (pixels, _) = thumbnail.pixels.as_chunks::<4>();
            assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
        }
        const TITLE: &str = "freeshow-rust-starter-title";
        assert!(session.browse_template(TITLE));
        session
            .library_action("template-toggle-favorite", TITLE)
            .unwrap();
        session.template_favorites_only = true;
        assert_eq!(session.template_entries().len(), 1);
        session.library_action("template-export", TITLE).unwrap();
        session.library_action("template-new-show", TITLE).unwrap();
        assert_eq!(session.lyrics(), "Welcome");
        assert!(session.save_lyrics("Welcome to the evening service"));
        let saved =
            ShowFile::load(session.current_show().unwrap().path.as_deref().unwrap()).unwrap();
        assert_eq!(
            saved.plain_text("s1").lines().next().unwrap(),
            "Welcome to the evening service"
        );
        assert_eq!(saved.show["slides"]["s1"]["settings"]["color"], "#123d39");
        assert_eq!(crate::live::audience_frame(&session), live);
        assert!(!library.templates_path().exists());

        let (_dir, mut receiver) = self::library("receive-starter-template");
        receiver
            .import_files(&[dir.0.join("Exports/Title — Welcome - Template.project")])
            .unwrap();
        receiver.switch_tab("templates");
        assert_eq!(receiver.template_entries().len(), 5);
        assert!(!receiver.is_starter_template(TITLE));
        assert!(receiver.browse_template(TITLE));
        receiver.library_action("template-new-show", TITLE).unwrap();
        assert_eq!(receiver.lyrics(), "Welcome");
    }

    #[test]
    fn a_failed_template_save_leaves_the_gallery_unchanged() {
        let (dir, mut session) = library("template-save-failure");
        assert!(session.create_show());
        assert!(session.save_lyrics("Still in the show"));
        let library = Library::open(&dir.0);
        let path = library.templates_path();
        let config = path.parent().unwrap();
        std::fs::rename(config, dir.0.join("original-config")).unwrap();
        std::fs::write(config, "The config path is blocked by a file").unwrap();
        assert!(
            session
                .library_action("slide-to-template", "Blocked")
                .is_err()
        );
        assert!(session.templates.is_empty());
        assert!(session.template_catalog.named_entries().is_empty());
        assert_eq!(session.lyrics(), "Still in the show");
    }

    #[test]
    fn a_project_section_can_be_added_picked_and_renamed() {
        let (dir, mut session) = library("sections");
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        session.pick_project_item(0).unwrap();
        session.library_action("add-section", "").unwrap();
        assert_eq!(session.project_kinds(), ["", "Section", ""]);
        assert_eq!(session.highlighted_project_item(), Some(1));
        session
            .library_action("rename-section", " Sermon ")
            .unwrap();
        assert!(session.library_action("rename-section", "  ").is_err());
        let mut reloaded = Session::open(dir.0.clone());
        assert_eq!(reloaded.project_labels()[1], "Sermon");
        assert_eq!(reloaded.pick_project_item(1), Ok(ProjectPick::Section));
        reloaded.pick_project_item(0).unwrap();
        assert!(reloaded.library_action("rename-section", "Oops").is_err());
    }

    #[test]
    fn projects_can_be_renamed_trimmed_and_deleted() {
        let (_dir, mut session) = library("projects");
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        assert_eq!(session.project_labels().len(), 2);
        session
            .library_action("rename-project", "Sunday 9am")
            .unwrap();
        assert_eq!(session.project_title(), "Sunday 9am");
        session.project_item = 0;
        assert!(
            session.library_action("remove-project-item", "").is_err(),
            "Untitled 2 is open, so no project row is highlighted"
        );
        session.pick_project_item(0).unwrap();
        session.library_action("remove-project-item", "").unwrap();
        assert_eq!(session.project_labels(), ["Untitled 2"]);
        session.library_action("delete-project", "").unwrap();
        assert!(session.project_id.is_none());
        assert!(session.library_action("rename-project", "x").is_err());
    }

    #[test]
    fn the_project_highlight_follows_the_open_show() {
        let (_dir, mut session) = library("highlight");
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        session.pick_project_item(0).unwrap();
        assert_eq!(session.highlighted_project_item(), Some(0));
        let other = session
            .shows
            .iter()
            .position(|s| s.name() == "Untitled 2")
            .unwrap();
        session.show_index = Some(other);
        assert_eq!(session.highlighted_project_item(), None);
        assert!(session.add_current_show());
        assert_eq!(session.highlighted_project_item(), Some(2));
        session.library_action("remove-project-item", "").unwrap();
        assert_eq!(session.project_labels(), ["Untitled", "Untitled 2"]);
    }

    #[test]
    fn a_show_opened_from_the_library_leaves_the_project_unselected() {
        let (_dir, mut session) = library("library-open");
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        let first = session
            .shows
            .iter()
            .position(|show| show.name() == "Untitled")
            .unwrap();
        session.tab = "shows".into();
        let _ = session.select_drawer(first);
        assert_eq!(session.highlighted_project_item(), None);
        assert!(!session.move_project(1), "nothing picked to move");
        assert_eq!(session.project_labels(), ["Untitled", "Untitled 2"]);
        session.pick_project_item(0).unwrap();
        assert!(session.move_project(1));
        assert_eq!(session.project_labels(), ["Untitled 2", "Untitled"]);
        assert_eq!(session.highlighted_project_item(), Some(1));
    }

    #[test]
    fn a_dragged_project_item_lands_where_it_is_dropped() {
        let (_dir, mut session) = library("library-drag");
        assert!(session.create_project());
        assert!(session.create_show());
        assert!(session.create_show());
        assert!(session.create_show());
        assert_eq!(
            session.project_labels(),
            ["Untitled", "Untitled 2", "Untitled 3"]
        );
        session.pick_project_item(0).unwrap();
        assert!(session.drop_project_item(2, 0));
        assert_eq!(
            session.project_labels(),
            ["Untitled 3", "Untitled", "Untitled 2"]
        );
        assert_eq!(
            session.highlighted_project_item(),
            Some(1),
            "the picked item keeps its highlight"
        );
        assert!(!session.drop_project_item(1, 1));
        assert!(!session.drop_project_item(0, 9));
    }

    #[test]
    fn the_picked_project_reopens_next_time() {
        let (dir, mut session) = library("picker");
        assert!(session.create_project());
        session.library_action("rename-project", "Older").unwrap();
        assert!(session.create_project());
        session.library_action("rename-project", "Newer").unwrap();
        assert_eq!(session.project_choices(), ["Newer", "Older"]);
        assert_eq!(session.project_choice_index(), 0);
        session.library_action("open-project", "1").unwrap();
        assert_eq!(session.project_title(), "Older");
        let reopened = Session::open(dir.0.clone());
        assert_eq!(reopened.project_title(), "Older");
        assert!(session.library_action("open-project", "9").is_err());
    }
}
