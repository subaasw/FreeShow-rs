// SPDX-License-Identifier: GPL-3.0-only

use super::{EditSnapshot, Session};
use freeshow_data::{Library, ShowFile};

#[derive(PartialEq, Eq)]
enum TextTarget {
    Layer { slide: String, item: usize },
    Show,
}

pub(super) struct PendingEditorText {
    target: TextTarget,
    snapshot: EditSnapshot,
    saved_base: Option<serde_json::Value>,
    path: Option<std::path::PathBuf>,
    text: String,
    checkpoint_text: Option<String>,
}

impl PendingEditorText {
    fn candidate(&self, split_lines: u32) -> Result<Option<ShowFile>, String> {
        let mut edited = ShowFile {
            id: self.snapshot.show_id.clone(),
            show: self.snapshot.show.clone(),
            path: self.path.clone(),
        };
        let changed = match &self.target {
            TextTarget::Layer { slide, item } => {
                if edited.item_text(slide, *item).as_deref() == Some(self.text.as_str()) {
                    return Ok(None);
                }
                edited.set_item_text(slide, *item, &self.text)
            }
            TextTarget::Show => {
                if edited.lyrics_text() == self.text.trim_end() {
                    return Ok(None);
                }
                if edited.has_locked_layers() {
                    return Err("Unlock the show’s layers before replacing its text".into());
                }
                edited.set_lyrics_text(&self.text, split_lines)
            }
        };
        if !changed {
            return Err("Could not update the show text".into());
        }
        Ok((edited.show != self.snapshot.show).then_some(edited))
    }

    fn discard_checkpoint(&self, library: Option<&Library>) -> Result<(), String> {
        if let (Some(library), Some(path), Some(_)) = (library, &self.path, &self.checkpoint_text) {
            library
                .discard_typing_draft(path)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl Session {
    pub fn has_buffered_editor_text(&self) -> bool {
        !self.pending_editor_text.is_empty()
    }

    pub fn editor_text_has_recovery_draft(&self) -> bool {
        self.current_show()
            .and_then(|show| show.path.as_deref())
            .is_some_and(|path| self.has_pending_show_draft(path))
    }

    fn editor_text_target(&self, whole_show: bool) -> Option<TextTarget> {
        if self.tab != "shows" {
            return None;
        }
        if whole_show {
            return Some(TextTarget::Show);
        }
        let (show, slide) = self.selected_layer_slide()?;
        Some(TextTarget::Layer {
            slide,
            item: self.current_item(show),
        })
    }

    fn pending_editor_text_index(&self, whole_show: bool) -> Option<usize> {
        if self.pending_editor_text.is_empty() {
            return None;
        }
        let show = self.current_show()?;
        let target = self.editor_text_target(whole_show)?;
        self.pending_editor_text
            .iter()
            .position(|pending| show.id == pending.snapshot.show_id && target == pending.target)
    }

    pub fn pending_editor_text(&self, whole_show: bool) -> Option<&str> {
        let index = self.pending_editor_text_index(whole_show)?;
        Some(&self.pending_editor_text[index].text)
    }

    pub fn queue_editor_text(&mut self, whole_show: bool, text: &str) {
        if let Some(index) = self.pending_editor_text_index(whole_show) {
            text.clone_into(&mut self.pending_editor_text[index].text);
            return;
        }
        self.flush_editor_text();
        let Some(target) = self.editor_text_target(whole_show) else {
            return;
        };
        let Some(show) = self.current_show() else {
            return;
        };
        let label = if whole_show {
            "Edit show text"
        } else {
            "Edit lyrics"
        };
        let Some(snapshot) = self.edit_snapshot(label) else {
            return;
        };
        self.pending_editor_text.push(PendingEditorText {
            target,
            saved_base: self.timer_save_bases.get(&snapshot.show_id).cloned(),
            snapshot,
            path: show.path.clone(),
            text: text.to_owned(),
            checkpoint_text: None,
        });
    }

    pub fn checkpoint_editor_text(&mut self) -> Result<(), String> {
        if self.pending_editor_text.is_empty() {
            return Ok(());
        }
        let Some(library) = self.root.as_ref().map(Library::open) else {
            return Ok(());
        };
        let split = self.preferences.split_lines();
        for pending in &mut self.pending_editor_text {
            let Some(path) = pending.path.as_deref() else {
                continue;
            };
            if pending.checkpoint_text.as_ref() == Some(&pending.text)
                || path.parent() != Some(library.shows_dir().as_path())
            {
                continue;
            }
            if self.drafts.iter().any(|entry| entry.destination() == path) {
                continue;
            }
            if let Some(edited) = pending.candidate(split)? {
                let base = pending
                    .saved_base
                    .as_ref()
                    .unwrap_or(&pending.snapshot.show);
                let draft = library
                    .prepare_typing_draft(&edited, base)
                    .map_err(|error| error.to_string())?;
                draft.record().map_err(|error| error.to_string())?;
                pending.checkpoint_text = Some(pending.text.clone());
            } else {
                pending.discard_checkpoint(Some(&library))?;
                pending.checkpoint_text = None;
            }
        }
        Ok(())
    }

    pub fn flush_editor_text(&mut self) -> bool {
        let mut success = true;
        for pending in std::mem::take(&mut self.pending_editor_text) {
            let current = self
                .shows
                .iter()
                .find(|show| show.id == pending.snapshot.show_id);
            let paths = [
                current.and_then(|show| show.path.as_deref()),
                pending.path.as_deref(),
            ];
            if paths
                .into_iter()
                .flatten()
                .any(|path| self.has_pending_show_draft(path))
            {
                self.notice =
                    "Resolve this show’s recovery draft before saving the latest text".into();
                self.pending_editor_text.push(pending);
                success = false;
            } else if let Err(error) = self.save_editor_text(pending) {
                self.notice = error;
                success = false;
            }
        }
        success
    }

    pub fn clear_pending_layer_text(&mut self) {
        if let Some(index) = self.pending_editor_text_index(false) {
            let pending = self.pending_editor_text.remove(index);
            if let Err(error) =
                pending.discard_checkpoint(self.root.as_ref().map(Library::open).as_ref())
            {
                tracing::warn!(%error, "could not remove the completed text checkpoint");
            }
        }
    }

    pub fn pending_layer_matches_saved_show(&self) -> bool {
        self.current_show()
            .zip(self.pending_editor_text_index(false))
            .is_none_or(|(show, index)| show.show == self.pending_editor_text[index].snapshot.show)
    }

    fn save_editor_text(&mut self, pending: PendingEditorText) -> Result<(), String> {
        let Some(edited) = pending.candidate(self.preferences.split_lines())? else {
            return pending.discard_checkpoint(self.root.as_ref().map(Library::open).as_ref());
        };
        let PendingEditorText {
            target,
            snapshot,
            saved_base,
            ..
        } = pending;
        let whole_show = target == TextTarget::Show;
        let mut matches = self
            .shows
            .iter()
            .enumerate()
            .filter(|(_, show)| show.id == edited.id);
        if let Some((index, _)) = matches.next()
            && matches.next().is_none()
            && self.shows[index].show == snapshot.show
            && self.shows[index].path == edited.path
        {
            self.shows[index].show = edited.show;
            return self.persist_typing_edit(index, snapshot, |session| {
                if whole_show && session.show_index == Some(index) {
                    let last = session.shows[index]
                        .ordered_slides()
                        .len()
                        .saturating_sub(1);
                    session.slide_index = Some(session.slide_index.unwrap_or(0).min(last));
                    session.layer_selection = None;
                    session.item_index = 0;
                }
            });
        }
        self.keep_conflicting_edit(&edited, saved_base.as_ref().unwrap_or(&snapshot.show))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_typing_checkpoint_survives_reopening_before_autosave() {
        const CHILD_LIBRARY: &str = "FREESHOW_TYPING_CHECKPOINT_TEST_LIBRARY";
        if let Some(root) = std::env::var_os(CHILD_LIBRARY) {
            let root = std::path::PathBuf::from(root);
            let mut session = Session::open(root.clone());
            assert!(session.create_show_with("song", "Song", "Saved"));
            let path = session.current_show().unwrap().path.clone().unwrap();
            session.queue_editor_text(true, "First checkpoint");
            session.checkpoint_editor_text().unwrap();
            session.queue_editor_text(true, "Latest checkpoint\n\nSecond slide");
            session.checkpoint_editor_text().unwrap();
            assert!(!session.editor_text_has_recovery_draft());
            assert!(!session.can_undo());
            assert_eq!(ShowFile::load(&path).unwrap().lyrics_text(), "Saved");
            assert!(!root.join("Backups/Backups").exists());
            // Leave without letting this process flush or close the editor.
            std::process::exit(0);
        }

        let root = tempfile::tempdir().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("the_latest_typing_checkpoint_survives_reopening_before_autosave")
            .env(CHILD_LIBRARY, root.path())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stdout)
        );
        let path = root.path().join("Shows/Song.show");
        assert_eq!(ShowFile::load(&path).unwrap().lyrics_text(), "Saved");
        let mut reopened = Session::open(root.path().to_path_buf());
        assert!(reopened.show_draft_status().contains("Recovery draft"));
        reopened.library_action("retry-draft", "").unwrap();
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "Latest checkpoint\n\nSecond slide"
        );
        assert!(!root.path().join("Backups/Drafts/Song.typing").exists());
        assert!(reopened.undo());
        assert_eq!(ShowFile::load(&path).unwrap().lyrics_text(), "Saved");
        assert!(Library::open(root.path()).load_show_drafts().0.is_empty());
    }

    #[test]
    fn a_checkpoint_clears_on_save_and_when_typing_returns_to_saved_text() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("song", "Song", "Saved"));
        let path = session.current_show().unwrap().path.clone().unwrap();
        let library = Library::open(root.path());

        session.queue_editor_text(false, "Reverted");
        session.checkpoint_editor_text().unwrap();
        assert_eq!(library.load_show_drafts().0.len(), 1);
        session.queue_editor_text(false, "Saved");
        session.checkpoint_editor_text().unwrap();
        assert!(library.load_show_drafts().0.is_empty());
        assert!(session.flush_editor_text());
        assert!(!session.can_undo());

        session.queue_editor_text(false, "Published edit");
        session.checkpoint_editor_text().unwrap();
        assert_eq!(library.load_show_drafts().0.len(), 1);
        assert!(session.flush_editor_text());
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "Published edit"
        );
        assert_eq!(session.history.len(), 1);
        assert!(library.load_show_drafts().0.is_empty());
    }

    #[test]
    fn layer_text_keeps_its_slide_when_the_operator_changes_selection() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("song", "Song", "Original\n\nSecond"));
        session.select_slide(0);
        let on_air = crate::live::audience_frame(&session);
        let path = session.current_show().unwrap().path.clone().unwrap();
        session.queue_editor_text(false, "First keystrokes");
        session.queue_editor_text(false, "Final words");
        assert_eq!(session.pending_editor_text(false), Some("Final words"));
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "Original\n\nSecond"
        );

        session.select_slide_for_edit(1);
        assert_eq!(session.pending_editor_text(false), None);
        assert!(session.flush_editor_text());
        assert_eq!(session.slide_index, Some(1));
        assert_eq!(session.lyrics(), "Second");
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "Final words\n\nSecond"
        );
        assert_eq!(session.history.len(), 1);
        assert_eq!(crate::live::audience_frame(&session), on_air);
    }

    #[test]
    fn whole_show_text_cannot_be_saved_to_the_next_open_show() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("b", "Bravo", "Bravo text"));
        let bravo = session.current_show().unwrap().path.clone().unwrap();
        assert!(session.create_show_with("a", "Alpha", "Alpha text"));
        let alpha = session.current_show().unwrap().path.clone().unwrap();
        session.queue_editor_text(true, "Alpha draft\n\nNew slide");
        session.select_drawer(1);
        assert_eq!(session.pending_editor_text(true), None);
        assert_eq!(session.show_text(), "Bravo text");
        assert!(session.flush_editor_text());
        assert_eq!(session.current_show().unwrap().id, "b");
        assert_eq!(
            ShowFile::load(&alpha).unwrap().lyrics_text(),
            "Alpha draft\n\nNew slide"
        );
        assert_eq!(ShowFile::load(&bravo).unwrap().lyrics_text(), "Bravo text");
    }

    #[test]
    fn delayed_text_becomes_a_recovery_draft_if_a_newer_edit_was_saved() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::open(root.path().to_path_buf());
        assert!(session.create_show_with("song", "Song", "Original"));
        let path = session.current_show().unwrap().path.clone().unwrap();
        session.queue_editor_text(false, "Text still being typed");
        session.checkpoint_editor_text().unwrap();
        assert!(session.save_lyrics("New saved revision"));
        session
            .timer_save_bases
            .insert("song".into(), session.current_show().unwrap().show.clone());
        session.queue_editor_text(false, "Text continued after a newer save");
        session.checkpoint_editor_text().unwrap();
        let mut reopened = Session::open(root.path().to_path_buf());
        assert!(reopened.library_action("retry-draft", "").is_err());
        drop(reopened);
        assert!(!session.flush_editor_text());
        assert!(session.editor_text_has_recovery_draft());
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "New saved revision"
        );
        assert!(session.library_action("retry-draft", "").is_err());
        session.library_action("copy-draft", "").unwrap();
        assert_eq!(session.lyrics(), "Text continued after a newer save");
        assert_eq!(
            ShowFile::load(&path).unwrap().lyrics_text(),
            "New saved revision"
        );
    }
}
