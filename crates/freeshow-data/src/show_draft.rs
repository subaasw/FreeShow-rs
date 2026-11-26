// SPDX-License-Identifier: GPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{BACKUPS_DIR, DataError, Library, ShowFile, atomic_replace};

/// A pending library show edit, recorded before replacing the saved show.
#[derive(Clone)]
pub struct ShowDraft {
    pub destination: PathBuf,
    pub id: String,
    pub base: Value,
    pub edited: Value,
    journal: PathBuf,
}

impl Library {
    fn show_draft_directory(&self) -> PathBuf {
        self.root().join(BACKUPS_DIR).join("Drafts")
    }

    pub fn prepare_show_draft(
        &self,
        show: &ShowFile,
        base: &Value,
    ) -> Result<ShowDraft, DataError> {
        let destination = show.path.as_deref().ok_or_else(invalid_draft)?;
        let journal = self.show_draft_path(destination, "draft")?;
        Ok(ShowDraft {
            destination: destination.to_path_buf(),
            id: show.id.clone(),
            base: base.clone(),
            edited: show.show.clone(),
            journal,
        })
    }

    /// Keeps typing checkpoints independent of other saves to the same show.
    pub fn prepare_typing_draft(
        &self,
        show: &ShowFile,
        base: &Value,
    ) -> Result<ShowDraft, DataError> {
        let mut draft = self.prepare_show_draft(show, base)?;
        draft.journal.set_extension("typing");
        Ok(draft)
    }

    pub fn discard_typing_draft(&self, destination: &Path) -> Result<(), DataError> {
        remove_journal(&self.show_draft_path(destination, "typing")?)
    }

    fn show_draft_path(&self, destination: &Path, extension: &str) -> Result<PathBuf, DataError> {
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(invalid_draft)?;
        if destination.parent() != Some(self.shows_dir().as_path()) || !show_name(Path::new(name)) {
            return Err(invalid_draft());
        }
        Ok(self
            .show_draft_directory()
            .join(name)
            .with_extension(extension))
    }

    /// Returns pending drafts and leaves unreadable journal files available for repair.
    pub fn load_show_drafts(&self) -> (Vec<ShowDraft>, Vec<String>) {
        let entries = match fs::read_dir(self.show_draft_directory()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (vec![], vec![]),
            Err(error) => {
                return (
                    vec![],
                    vec![format!("Could not read recovery drafts: {error}")],
                );
            }
        };
        let mut drafts = Vec::new();
        let mut warnings = Vec::new();
        for entry in entries {
            let result = entry.map_err(DataError::from).and_then(|entry| {
                let journal = entry.path();
                if !entry.file_type()?.is_file()
                    || !matches!(
                        journal.extension().and_then(|ext| ext.to_str()),
                        Some("draft" | "typing")
                    )
                {
                    return Ok(None);
                }
                let value: Value = serde_json::from_slice(&fs::read(&journal)?)?;
                let filename = value["file"]
                    .as_str()
                    .map(Path::new)
                    .ok_or_else(invalid_draft)?;
                let id = value["id"].as_str().ok_or_else(invalid_draft)?;
                if value["version"] != 1
                    || !show_name(filename)
                    || journal.file_stem() != filename.file_stem()
                    || !value["base"].is_object()
                    || !value["edited"].is_object()
                {
                    return Err(invalid_draft());
                }
                let draft = ShowDraft {
                    destination: self.shows_dir().join(filename),
                    id: id.to_string(),
                    base: value["base"].clone(),
                    edited: value["edited"].clone(),
                    journal,
                };
                if draft
                    .saved_show()
                    .is_ok_and(|show| show.id == draft.id && show.show == draft.edited)
                {
                    draft.discard()?;
                    return Ok(None);
                }
                Ok(Some(draft))
            });
            match result {
                Ok(Some(draft)) => drafts.push(draft),
                Ok(None) => {}
                Err(error) => warnings.push(format!("Could not load a recovery draft: {error}")),
            }
        }
        drafts.sort_by(|a, b| a.journal.cmp(&b.journal));
        (drafts, warnings)
    }
}

impl ShowDraft {
    pub fn record(&self) -> Result<(), DataError> {
        let file = self
            .destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(invalid_draft)?;
        let version = Value::from(1);
        let file = Value::from(file);
        let id = Value::from(self.id.as_str());
        let journal = std::collections::BTreeMap::from([
            ("version", &version),
            ("file", &file),
            ("id", &id),
            ("base", &self.base),
            ("edited", &self.edited),
        ]);
        atomic_replace(&self.journal, &serde_json::to_vec(&journal)?)
    }

    pub fn saved_show(&self) -> Result<ShowFile, DataError> {
        if !fs::symlink_metadata(&self.destination)?.is_file() {
            return Err(invalid_draft());
        }
        ShowFile::load(&self.destination)
    }

    /// Recovery is also idempotent when the show was saved just before an interruption.
    pub fn apply(&self) -> Result<ShowFile, DataError> {
        let saved = self.saved_show().map_err(|error| {
            DataError::Scripture(format!("Could not open the original show: {error}"))
        })?;
        if saved.id != self.id || (saved.show != self.base && saved.show != self.edited) {
            return Err(DataError::Scripture(
                "The saved show changed on disk. Save the draft as a copy to keep both versions."
                    .into(),
            ));
        }
        let mut show = saved;
        if show.show != self.edited {
            show.show.clone_from(&self.edited);
            show.save(&self.destination)?;
        }
        Ok(show)
    }

    pub fn discard(&self) -> Result<(), DataError> {
        remove_journal(&self.journal)
    }
}

fn remove_journal(path: &Path) -> Result<(), DataError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
}

fn show_name(path: &Path) -> bool {
    path.file_name() == Some(path.as_os_str())
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("show"))
}

fn invalid_draft() -> DataError {
    DataError::Scripture("Invalid show recovery draft".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reopening_finds_an_interrupted_commit_and_cleans_up_an_applied_draft() {
        let root = std::env::temp_dir().join(format!("freeshow-show-draft-{}", crate::new_id()));
        let library = Library::open(&root);
        let path = library.shows_dir().join("Welcome.show");
        let mut show = ShowFile::blank("welcome", "Welcome");
        show.show["futureField"] = json!({"retained": true});
        show.save(&path).unwrap();
        show.path = Some(path.clone());
        let base = show.show.clone();
        show.set_name("Rehearsal welcome");
        let draft = library.prepare_show_draft(&show, &base).unwrap();
        draft.record().unwrap();

        let (pending, warnings) = Library::open(&root).load_show_drafts();
        assert!(warnings.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(ShowFile::load(&path).unwrap().show, base);
        let saved = pending[0].apply().unwrap();
        assert_eq!(saved.show, show.show);
        assert_eq!(saved.show["futureField"], json!({"retained": true}));

        let (pending, warnings) = Library::open(&root).load_show_drafts();
        assert!(pending.is_empty());
        assert!(warnings.is_empty());
        assert!(!draft.journal.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
