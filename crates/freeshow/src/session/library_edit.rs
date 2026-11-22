// SPDX-License-Identifier: GPL-3.0-only

//! Operator library edits: rename, duplicate and delete shows, the whole-show
//! text editor, slide delete/duplicate/reorder, and project rename/delete.
//!
//! Every action returns a short message for the status bar, or an error the
//! operator can read, instead of only writing to the log.

use freeshow_data::{Library, ProjectEntry, ProjectsFile, ShowFile};

use super::{Session, unique_name};
use freeshow_data::new_id;

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
        self.remember_project();
        Ok(format!("Opened project “{}”", project.name))
    }

    /// Runs one operator library action and returns the status-bar message.
    pub fn library_action(&mut self, action: &str, arg: &str) -> Result<String, String> {
        match action {
            "rename-show" => self.rename_show(arg),
            "duplicate-show" => self.duplicate_show(),
            "delete-show" => self.delete_show(),
            "restore-show" => self.restore_deleted_show(),
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
            "open-project" => self.open_project(arg),
            "rename-project" => self.rename_project(arg),
            "delete-project" => self.delete_current_project(),
            "remove-project-item" => self.remove_project_item(),
            other => Err(format!("unknown action {other:?}")),
        }
        .inspect(|message| self.notice = message.clone())
        .inspect_err(|error| {
            tracing::warn!("{action}: {error}");
            self.notice = error.clone();
        })
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
    fn insert_sorted(&mut self, show: ShowFile) -> usize {
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
        let name = self.shows[index].name();
        if let Some(path) = self.shows[index].path.clone()
            && path.exists()
        {
            std::fs::remove_file(&path)
                .map_err(|err| format!("could not delete {}: {err}", path.display()))?;
        }
        let show = self.shows.remove(index);
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

    fn save_current_show(&mut self, index: usize) -> Result<(), String> {
        let show = &self.shows[index];
        match &show.path {
            Some(path) => show
                .save(path)
                .map_err(|err| format!("could not save {}: {err}", path.display())),
            None => Ok(()),
        }
    }

    fn save_show_text(&mut self, text: &str) -> Result<String, String> {
        let index = self.selected_show()?;
        if self.shows[index].lyrics_text() == text.trim_end() {
            return Ok(String::new());
        }
        let snap = self.edit_snapshot("Edit show text");
        let split = self.preferences.split_lines();
        if !self.shows[index].set_lyrics_text(text, split) {
            return Err("could not update the show text".into());
        }
        self.save_current_show(index)?;
        self.commit_edit(snap);
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
        let snap = self.edit_snapshot(label);
        let Some(next) = edit(&mut self.shows[index], slide) else {
            return Err(match label {
                "Delete slide" => "a show keeps at least one slide".into(),
                "Move slide" => "the slide cannot move further".into(),
                _ => format!("{label} failed"),
            });
        };
        self.save_current_show(index)?;
        self.commit_edit(snap);
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
        self.remember_project();
        Ok(format!("Deleted project “{name}”"))
    }

    fn remove_project_item(&mut self) -> Result<String, String> {
        let id = self.project_id.clone().ok_or("open a project first")?;
        let index = self.project_item;
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
        Ok(format!("Removed “{label}” from the project"))
    }
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
        session.library_action("remove-project-item", "").unwrap();
        assert_eq!(session.project_labels(), ["Untitled 2"]);
        session.library_action("delete-project", "").unwrap();
        assert!(session.project_id.is_none());
        assert!(session.library_action("rename-project", "x").is_err());
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
