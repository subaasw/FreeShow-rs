// SPDX-License-Identifier: GPL-3.0-only

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use freeshow_data::{
    BACKUPS_DIR, BIBLES_DIR, CONFIG_DIR, EVENTS_FILE, OVERLAYS_FILE, PROJECTS_FILE, SHOWS_DIR,
    STAGE_FILE, SYNCED_SETTINGS_FILE, TEMPLATES_FILE,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Copies the library into a FreeShow backup zip, then to a cloud folder or URL when one is set.
/// A missing destination or a failed upload leaves the local zip in place and the show keeps running.
pub trait CloudBackup {
    fn name(&self) -> &'static str;
    fn backup(&self, library: &Path) -> BackupReport;
    fn restore_latest(&self, library: &Path) -> BackupReport;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    pub path: Option<PathBuf>,
    pub files: usize,
    pub copied: bool,
    pub uploaded: bool,
}

impl BackupReport {
    fn empty() -> Self {
        Self {
            path: None,
            files: 0,
            copied: false,
            uploaded: false,
        }
    }
}

pub struct LibraryCloud;

impl CloudBackup for LibraryCloud {
    fn name(&self) -> &'static str {
        "cloud"
    }

    fn backup(&self, library: &Path) -> BackupReport {
        let dir = env_path("FREESHOW_CLOUD_DIR");
        let url = std::env::var("FREESHOW_CLOUD_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());
        let token = std::env::var("FREESHOW_CLOUD_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        backup_library(library, dir.as_deref(), url.as_deref(), token.as_deref())
    }

    fn restore_latest(&self, library: &Path) -> BackupReport {
        restore_latest(library)
    }
}

const STORES: &[(&str, &str)] = &[
    (PROJECTS_FILE, "PROJECTS.json"),
    (TEMPLATES_FILE, "TEMPLATES.json"),
    (OVERLAYS_FILE, "OVERLAYS.json"),
    (EVENTS_FILE, "EVENTS.json"),
    (STAGE_FILE, "STAGE.json"),
    (SYNCED_SETTINGS_FILE, "SYNCED_SETTINGS.json"),
];

pub fn backup_library(
    library: &Path,
    cloud_dir: Option<&Path>,
    cloud_url: Option<&str>,
    token: Option<&str>,
) -> BackupReport {
    let entries = collect(library);
    let Some(path) = write_zip(library, &entries) else {
        tracing::error!("could not write the backup");
        return BackupReport::empty();
    };
    tracing::info!("backup {}", path.display());
    let mut report = BackupReport {
        path: Some(path.clone()),
        files: entries.len(),
        copied: false,
        uploaded: false,
    };
    if let Some(dir) = cloud_dir {
        match fs::create_dir_all(dir).and_then(|_| {
            let name = path.file_name().unwrap_or_default();
            fs::copy(&path, dir.join(name))
        }) {
            Ok(_) => report.copied = true,
            Err(err) => tracing::error!("cloud folder was not updated: {err}"),
        }
    }
    if let Some(url) = cloud_url.filter(|url| !url.is_empty()) {
        match fs::read(&path)
            .and_then(|bytes| put_zip(url, token, &bytes).map_err(|err| std::io::Error::other(err)))
        {
            Ok(()) => report.uploaded = true,
            Err(err) => tracing::error!("cloud upload was not completed: {err}"),
        }
    }
    report
}

pub fn restore_latest(library: &Path) -> BackupReport {
    let Some(path) = newest_zip(&library.join(BACKUPS_DIR)) else {
        tracing::error!("no backup to restore");
        return BackupReport::empty();
    };
    match restore_zip(library, &path) {
        Ok(files) => {
            tracing::info!("restored {}", path.display());
            BackupReport {
                path: Some(path),
                files,
                copied: false,
                uploaded: false,
            }
        }
        Err(err) => {
            tracing::error!("could not restore the backup: {err}");
            BackupReport::empty()
        }
    }
}

fn collect(library: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    if let Ok(dir) = fs::read_dir(library.join(SHOWS_DIR)) {
        for file in dir.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if !name.to_ascii_lowercase().ends_with(".show") {
                continue;
            }
            if let Ok(bytes) = fs::read(file.path()) {
                entries.push((format!("SHOWS/{name}"), bytes));
            }
        }
    }
    for (file, zip_name) in STORES {
        let path = library.join(CONFIG_DIR).join(file);
        if let Ok(bytes) = fs::read(path) {
            entries.push(((*zip_name).to_string(), bytes));
        }
    }
    if let Ok(dir) = fs::read_dir(library.join(BIBLES_DIR)) {
        for file in dir.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if !name.to_ascii_lowercase().ends_with(".fsb") {
                continue;
            }
            if let Ok(bytes) = fs::read(file.path()) {
                entries.push((format!("BIBLE_{name}"), bytes));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn write_zip(library: &Path, entries: &[(String, Vec<u8>)]) -> Option<PathBuf> {
    let dir = library.join(BACKUPS_DIR);
    fs::create_dir_all(&dir).ok()?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M");
    let mut name = format!("{stamp}.zip");
    if dir.join(&name).exists() {
        name = format!("{stamp}-{}.zip", chrono::Local::now().format("%S"));
    }
    let path = dir.join(name);
    let file = File::create(&path).ok()?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        if zip.start_file(name, options).is_err() {
            return None;
        }
        if zip.write_all(bytes).is_err() {
            return None;
        }
    }
    zip.finish().ok()?;
    Some(path)
}

fn restore_zip(library: &Path, path: &Path) -> Result<usize, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|err| err.to_string())?;
    let mut count = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| err.to_string())?;
        let name = entry.name().replace('\\', "/");
        if name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|err| err.to_string())?;
        let Some(dest) = restore_path(library, &name) else {
            continue;
        };
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::write(&dest, bytes).map_err(|err| err.to_string())?;
        count += 1;
    }
    Ok(count)
}

fn restore_path(library: &Path, name: &str) -> Option<PathBuf> {
    if let Some(file) = name.strip_prefix("SHOWS/") {
        if file.is_empty() || file.contains("..") {
            return None;
        }
        return Some(library.join(SHOWS_DIR).join(file));
    }
    if let Some(file) = name.strip_prefix("BIBLE_") {
        if file.is_empty() || file.contains('/') || file.contains("..") {
            return None;
        }
        return Some(library.join(BIBLES_DIR).join(file));
    }
    let file = STORES
        .iter()
        .find(|(_, zip_name)| *zip_name == name)
        .map(|(file, _)| *file)?;
    Some(library.join(CONFIG_DIR).join(file))
}

fn newest_zip(dir: &Path) -> Option<PathBuf> {
    let mut zips = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<_>>();
    zips.sort_by(|left, right| left.0.cmp(&right.0));
    zips.pop().map(|(_, path)| path)
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn put_zip(url: &str, token: Option<&str>, bytes: &[u8]) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    let request = agent.put(url).set("Content-Type", "application/zip");
    let request = if let Some(token) = token {
        request.set("Authorization", &format!("Bearer {token}"))
    } else {
        request
    };
    match request.send_bytes(bytes) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(format!("cloud returned {code}")),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_round_trips_shows_config_and_a_bible() {
        let root = std::env::temp_dir().join(format!("freeshow-cloud-{}", std::process::id()));
        let cloud =
            std::env::temp_dir().join(format!("freeshow-cloud-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&cloud);
        fs::create_dir_all(root.join(SHOWS_DIR)).unwrap();
        fs::create_dir_all(root.join(CONFIG_DIR)).unwrap();
        fs::create_dir_all(root.join(BIBLES_DIR)).unwrap();
        let show = br#"["song-1",{"name":"Amazing Grace"}]"#;
        let projects = br#"{"projects":{"p1":{"name":"Sunday"}}}"#;
        let bible = b"bible-bytes";
        fs::write(root.join("Shows/Amazing Grace.show"), show).unwrap();
        fs::write(root.join("Config/projects.json"), projects).unwrap();
        fs::write(root.join("Bibles/KJV.fsb"), bible).unwrap();

        let report = backup_library(&root, Some(&cloud), None, None);
        assert!(report.copied);
        assert_eq!(report.files, 3);
        let zip_path = report.path.unwrap();
        let stored = zip_names(&zip_path);
        assert!(stored.contains(&"SHOWS/Amazing Grace.show".to_string()));
        assert!(stored.contains(&"PROJECTS.json".to_string()));
        assert!(stored.contains(&"BIBLE_KJV.fsb".to_string()));
        assert!(cloud.join(zip_path.file_name().unwrap()).is_file());

        fs::remove_file(root.join("Shows/Amazing Grace.show")).unwrap();
        fs::remove_file(root.join("Config/projects.json")).unwrap();
        fs::remove_file(root.join("Bibles/KJV.fsb")).unwrap();
        let restored = restore_latest(&root);
        assert_eq!(restored.files, 3);
        assert_eq!(
            fs::read(root.join("Shows/Amazing Grace.show")).unwrap(),
            show
        );
        assert_eq!(
            fs::read(root.join("Config/projects.json")).unwrap(),
            projects
        );
        assert_eq!(fs::read(root.join("Bibles/KJV.fsb")).unwrap(), bible);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&cloud);
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect()
    }
}
