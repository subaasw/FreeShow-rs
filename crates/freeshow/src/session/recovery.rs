// SPDX-License-Identifier: GPL-3.0-only

use super::{EditSelection, EditSnapshot, Session};
use freeshow_data::{Library, ShowDraft, ShowFile, new_id};
use std::path::Path;

#[derive(Clone)]
pub(super) struct PendingDraft {
    draft: ShowDraft,
    recorded: bool,
}

impl PendingDraft {
    pub(super) fn recorded(draft: ShowDraft) -> Self {
        Self {
            draft,
            recorded: true,
        }
    }

    pub(super) fn destination(&self) -> &Path {
        &self.draft.destination
    }

    fn record_if_needed(&mut self) -> Result<(), freeshow_data::DataError> {
        if !self.recorded {
            self.draft.record()?;
            self.recorded = true;
        }
        Ok(())
    }
}

impl Session {
    pub(super) fn has_pending_show_draft(&self, path: &Path) -> bool {
        self.drafts
            .iter()
            .any(|pending| pending.destination() == path)
    }

    pub(super) fn check_pending_show_draft(&self, index: usize) -> Result<(), String> {
        if self.shows[index]
            .path
            .as_deref()
            .is_some_and(|path| self.has_pending_show_draft(path))
        {
            return Err("Resolve this show’s recovery draft before making another edit".into());
        }
        Ok(())
    }

    /// The editor mutates a candidate in place. Roll it back unless the save commits.
    pub(super) fn persist_show_edit(
        &mut self,
        index: usize,
        snapshot: EditSnapshot,
    ) -> Result<(), String> {
        self.persist_show_edit_then(index, snapshot, |_| {})
    }

    pub(super) fn persist_show_edit_then(
        &mut self,
        index: usize,
        snapshot: EditSnapshot,
        select: impl FnOnce(&mut Self),
    ) -> Result<(), String> {
        self.persist_edit(index, snapshot, false, select)
    }

    pub(super) fn persist_typing_edit(
        &mut self,
        index: usize,
        snapshot: EditSnapshot,
        select: impl FnOnce(&mut Self),
    ) -> Result<(), String> {
        self.persist_edit(index, snapshot, true, select)
    }

    fn persist_edit(
        &mut self,
        index: usize,
        snapshot: EditSnapshot,
        typing: bool,
        select: impl FnOnce(&mut Self),
    ) -> Result<(), String> {
        if self.shows[index].show == snapshot.show {
            select(self);
            return Ok(());
        }
        if let Err(error) = self.write_show_edit(index, &snapshot, typing) {
            self.shows[index].show = snapshot.show;
            return Err(error);
        }
        self.timer_save_bases.remove(&snapshot.show_id);
        select(self);
        self.commit_edit(index, snapshot);
        self.notice.clear();
        Ok(())
    }

    fn write_show_edit(
        &mut self,
        index: usize,
        snapshot: &EditSnapshot,
        typing: bool,
    ) -> Result<(), String> {
        self.check_pending_show_draft(index)?;
        let show = &self.shows[index];
        let Some(path) = &show.path else {
            return Ok(());
        };
        let library = self.root.as_ref().map(Library::open);
        let Some(library) =
            library.filter(|library| path.parent() == Some(library.shows_dir().as_path()))
        else {
            return show
                .save(path)
                .map_err(|err| format!("Could not save show: {err}"));
        };
        let base = self
            .timer_save_bases
            .get(&show.id)
            .unwrap_or(&snapshot.show);
        let draft = if typing {
            library.prepare_typing_draft(show, base)
        } else {
            library.prepare_show_draft(show, base)
        }
        .map_err(|err| err.to_string())?;
        if let Err(error) = draft.record() {
            self.drafts.push(PendingDraft {
                draft,
                recorded: false,
            });
            return Err(format!(
                "Could not write a recovery file. The draft is currently in memory: {error}"
            ));
        }
        if let Err(error) = draft.apply() {
            self.drafts.push(PendingDraft::recorded(draft));
            return Err(format!("Show save failed. Recovery draft kept. {error}"));
        }
        // The saved revision also identifies a completed draft after an interrupted cleanup.
        if let Err(error) = draft.discard() {
            tracing::warn!(%error, "could not remove the completed recovery file");
        }
        Ok(())
    }

    pub fn show_draft_status(&self) -> String {
        self.drafts.first().map_or_else(String::new, |pending| {
            let file = pending
                .draft
                .destination
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let location = if pending.recorded {
                "Recovery draft"
            } else {
                "Draft in memory"
            };
            if self.drafts.len() == 1 {
                format!("{location} for {file}")
            } else {
                format!("{location} for {file} ({} pending)", self.drafts.len())
            }
        })
    }

    pub fn record_memory_drafts(&mut self) -> Result<(), freeshow_data::DataError> {
        self.drafts
            .iter_mut()
            .try_for_each(PendingDraft::record_if_needed)
    }

    pub(super) fn keep_conflicting_edit(
        &mut self,
        edited: &ShowFile,
        base: &serde_json::Value,
    ) -> Result<(), String> {
        let library = self
            .root
            .as_ref()
            .map(Library::open)
            .ok_or("Choose a library first")?;
        let draft = library
            .prepare_typing_draft(edited, base)
            .map_err(|error| error.to_string())?;
        let mut pending = PendingDraft {
            draft,
            recorded: false,
        };
        let status = match pending.record_if_needed() {
            Ok(()) => "The show changed while you were typing. Save the recovery draft as a copy to keep both versions.".into(),
            Err(error) => format!("The show changed while you were typing. The recovery draft is still in memory. {error}"),
        };
        self.drafts.push(pending);
        Err(status)
    }

    pub(super) fn resolve_show_draft(&mut self, action: &str) -> Result<String, String> {
        let Some(pending) = self.drafts.first().cloned() else {
            return Err("There is no recovery draft".into());
        };
        let draft = &pending.draft;
        let original_index = self
            .shows
            .iter()
            .position(|show| show.path.as_ref() == Some(&draft.destination));
        match action {
            "retry-draft" => {
                self.drafts[0]
                    .record_if_needed()
                    .map_err(|error| error.to_string())?;
                let show = draft.apply().map_err(|error| error.to_string())?;
                self.hold_program();
                self.tab = "shows".into();
                let index = match original_index {
                    Some(index) => {
                        self.shows[index] = show;
                        self.show_index = Some(index);
                        self.slide_index = self.first_slide();
                        self.item_index = 0;
                        index
                    }
                    None => self.insert_sorted(show),
                };
                self.timer_save_bases.remove(&draft.id);
                self.layer_selection = None;
                let base = ShowFile {
                    id: draft.id.clone(),
                    show: draft.base.clone(),
                    path: None,
                };
                let selection = EditSelection {
                    slide_index: base.first_enabled_slide(),
                    ..Default::default()
                };
                self.commit_edit(
                    index,
                    EditSnapshot {
                        label: "Recover show".to_string(),
                        show_id: base.id,
                        show: base.show,
                        selection,
                    },
                );
            }
            "copy-draft" => {
                let id = new_id();
                let path = draft
                    .destination
                    .with_file_name(format!("Recovered-{id}.show"));
                let mut copy = ShowFile {
                    id,
                    show: draft.edited.clone(),
                    path: Some(path.clone()),
                };
                copy.set_name(&format!("{} (recovered)", copy.name()));
                copy.save(&path).map_err(|error| error.to_string())?;
                self.keep_saved_version(draft, original_index);
                self.hold_program();
                self.tab = "shows".into();
                self.insert_sorted(copy);
            }
            "discard-draft" => {
                draft.discard().map_err(|error| error.to_string())?;
                self.keep_saved_version(draft, original_index);
                self.drafts.remove(0);
                return Ok("Recovery draft discarded".into());
            }
            _ => return Err("Unknown recovery action".into()),
        }
        self.drafts.remove(0);
        if let Err(error) = draft.discard() {
            tracing::warn!(%error, "could not remove the completed recovery file");
        }
        let status = if action == "copy-draft" {
            "Recovered show saved as a copy"
        } else {
            "Recovered show saved"
        };
        Ok(status.into())
    }

    fn keep_saved_version(&mut self, draft: &ShowDraft, original_index: Option<usize>) {
        if let (Some(index), Ok(show)) = (original_index, draft.saved_show()) {
            let current = &self.shows[index];
            if show.id == current.id {
                if show.show == current.show {
                    self.timer_save_bases.remove(&current.id);
                    return;
                }
                if self.timer_save_bases.get(&current.id) == Some(&show.show) {
                    // The saved base still matches the timer edits waiting to be written.
                    return;
                }
            }
            let previous_id = current.id.clone();
            self.hold_edited_program(&previous_id);
            self.timer_save_bases.remove(&previous_id);
            self.shows[index] = show;
            if self.show_index == Some(index) {
                self.layer_selection = None;
                self.slide_index = self.first_slide();
                self.item_index = 0;
            }
            self.history
                .retain(|entry| entry.snapshot.show_id != previous_id);
            self.redo
                .retain(|entry| entry.snapshot.show_id != previous_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_edit_recovers_after_reopening_and_is_undoable() {
        let root = std::env::temp_dir().join(format!("freeshow-draft-retry-{}", new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "Saved lyrics"));
        let path = session.shows[0].path.clone().unwrap();
        let blocked = path.with_extension("show.tmp");
        std::fs::create_dir(&blocked).unwrap();

        assert!(!session.save_lyrics("Draft lyrics"));
        assert_eq!(session.lyrics(), "Saved lyrics");
        assert!(!session.can_undo());
        assert!(session.show_draft_status().contains("Recovery draft"));
        session.autosave();
        assert_eq!(ShowFile::load(&path).unwrap().lyrics_text(), "Saved lyrics");
        drop(session);

        let mut reopened = Session::open(root.clone());
        assert!(reopened.show_draft_status().contains("Recovery draft"));
        assert!(reopened.library_action("retry-draft", "").is_err());
        std::fs::remove_dir(&blocked).unwrap();
        reopened.library_action("retry-draft", "").unwrap();
        assert_eq!(reopened.lyrics(), "Draft lyrics");
        assert!(reopened.show_draft_status().is_empty());
        assert_eq!(ShowFile::load(&path).unwrap().lyrics_text(), "Draft lyrics");
        assert!(reopened.undo());
        assert_eq!(reopened.lyrics(), "Saved lyrics");
        assert!(Session::open(root.clone()).show_draft_status().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_changed_original_can_be_kept_beside_a_draft_or_reloaded_on_discard() {
        let root = std::env::temp_dir().join(format!("freeshow-draft-conflict-{}", new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "Original"));
        let path = session.shows[0].path.clone().unwrap();
        let mut external = ShowFile::load(&path).unwrap();
        assert!(external.set_lyrics_text("External version", 0));
        external.save(&path).unwrap();
        session.autosave();
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "External version"
        );

        assert!(!session.save_lyrics("My version"));
        assert!(session.notice.contains("changed on disk"));
        assert_eq!(session.lyrics(), "Original");
        session.autosave();
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "External version"
        );
        assert!(session.library_action("retry-draft", "").is_err());
        session.library_action("copy-draft", "").unwrap();
        let copy = session.shows.iter().find(|show| show.id != "song").unwrap();
        assert_eq!(copy.lyrics_text(), "My version");
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "External version"
        );
        assert_eq!(session.autosave(), 0);

        let index = session
            .shows
            .iter()
            .position(|show| show.id == "song")
            .unwrap();
        session.select_drawer(index);
        session.select_slide_for_edit(0);
        assert!(external.set_lyrics_text("Newer external version", 0));
        external.save(&path).unwrap();
        assert!(!session.save_notes("Draft to discard"));
        session.library_action("discard-draft", "").unwrap();
        assert!(session.show_draft_status().is_empty());
        assert_eq!(session.lyrics(), "Newer external version");
        assert_eq!(session.autosave(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unwritable_recovery_directory_keeps_the_draft_in_memory() {
        let root = std::env::temp_dir().join(format!("freeshow-draft-memory-{}", new_id()));
        let mut session = Session::open(root.clone());
        assert!(session.create_show_with("song", "Song", "Original"));
        let directory = root.join("Backups/Drafts");
        std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
        std::fs::write(&directory, "blocks directory creation").unwrap();

        assert!(!session.save_lyrics("Draft"));
        assert_eq!(session.lyrics(), "Original");
        assert!(session.show_draft_status().contains("Draft in memory"));
        assert!(session.record_memory_drafts().is_err());
        assert!(session.show_draft_status().contains("Draft in memory"));
        std::fs::remove_file(directory).unwrap();
        session.record_memory_drafts().unwrap();
        assert!(session.show_draft_status().contains("Recovery draft"));
        assert_eq!(session.lyrics(), "Original");
        drop(session);

        let mut session = Session::open(root.clone());
        assert!(session.show_draft_status().contains("Recovery draft"));
        session.library_action("retry-draft", "").unwrap();
        assert_eq!(session.lyrics(), "Draft");
        assert!(session.show_draft_status().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
