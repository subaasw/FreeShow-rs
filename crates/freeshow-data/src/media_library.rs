// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{DataError, Library};

/// A still, video, or audio file in the library Media folder or a `mediaFolders` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: MediaEntryKind,
    pub exists: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaEntryKind {
    Image,
    Video,
    Audio,
    Other,
    Camera,
}

impl MediaEntry {
    pub fn camera(device: &str) -> Self {
        Self {
            name: format!("Camera: {device}"),
            path: PathBuf::new(),
            kind: MediaEntryKind::Camera,
            exists: true,
            width: None,
            height: None,
            duration_secs: None,
        }
    }

    /// Drawer and remote label: the file name, plus size, duration, or a missing badge.
    pub fn label(&self) -> String {
        if self.kind == MediaEntryKind::Camera {
            return self.name.clone();
        }
        let title = self.display_name();
        if !self.exists {
            return format!("{title}  missing");
        }
        if let (Some(width), Some(height)) = (self.width, self.height) {
            return format!("{title}  {width}x{height}");
        }
        if let Some(seconds) = self.duration_secs {
            return format!("{title}  {}", format_duration(seconds));
        }
        title
    }

    fn display_name(&self) -> String {
        let Some(parent) = self.path.parent().and_then(|path| path.file_name()) else {
            return self.name.clone();
        };
        if parent == "Media" {
            self.name.clone()
        } else {
            format!("{}/{}", parent.to_string_lossy(), self.name)
        }
    }
}

fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn media_entry_kind(path: &Path) -> MediaEntryKind {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => MediaEntryKind::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => MediaEntryKind::Video,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => MediaEntryKind::Audio,
        _ => MediaEntryKind::Other,
    }
}

/// Reads still-image pixel size from a file header when the format is known.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    let bytes = fs::read(path).ok()?;
    match media_entry_kind(path) {
        MediaEntryKind::Image => dimensions_from_bytes(&bytes),
        _ => None,
    }
}

fn dimensions_from_bytes(bytes: &[u8]) -> Option<(u32, u32)> {
    png_size(bytes)
        .or_else(|| jpeg_size(bytes))
        .or_else(|| gif_size(bytes))
        .or_else(|| bmp_size(bytes))
        .or_else(|| webp_size(bytes))
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 8 < bytes.len() {
        if bytes[index] != 0xFF {
            return None;
        }
        while index < bytes.len() && bytes[index] == 0xFF {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        let marker = bytes[index];
        index += 1;
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if index + 1 >= bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        if length < 2 || index + length > bytes.len() {
            return None;
        }
        if matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        ) && length >= 7
        {
            let height = u16::from_be_bytes([bytes[index + 3], bytes[index + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            return (width > 0 && height > 0).then_some((width, height));
        }
        index += length;
    }
    None
}

fn gif_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    (width > 0 && height > 0).then_some((width, height))
}

fn bmp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 26 || !bytes.starts_with(b"BM") {
        return None;
    }
    let header = u32::from_le_bytes(bytes[14..18].try_into().ok()?);
    if header < 16 {
        return None;
    }
    let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?);
    let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?).unsigned_abs();
    (width > 0 && height > 0).then_some((width as u32, height))
}

fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8 " if bytes.len() >= 30 => {
            let width = u16::from_le_bytes([bytes[26], bytes[27]]) as u32 & 0x3FFF;
            let height = u16::from_le_bytes([bytes[28], bytes[29]]) as u32 & 0x3FFF;
            (width > 0 && height > 0).then_some((width, height))
        }
        b"VP8L" if bytes.len() >= 25 => {
            let bits = u32::from_le_bytes(bytes[21..25].try_into().ok()?);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Some((width, height))
        }
        b"VP8X" if bytes.len() >= 30 => {
            let width = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]) + 1;
            let height = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
}

impl Library {
    /// Extra media directories stored as `mediaFolders` in `settings.json`.
    pub fn media_folder_paths(&self) -> Vec<PathBuf> {
        let settings: Value = fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let Some(folders) = settings.get("mediaFolders").and_then(Value::as_object) else {
            return Vec::new();
        };
        let media_dir = self.media_dir();
        let media_key = unique_key(&media_dir);
        let mut paths = Vec::new();
        let mut seen = HashSet::from([media_key]);
        for folder in folders.values() {
            let Some(path) = folder
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                continue;
            };
            let path = PathBuf::from(path);
            if !seen.insert(unique_key(&path)) {
                continue;
            }
            paths.push(path);
        }
        paths
    }

    /// Files in `Media/` and any `mediaFolders` paths, with size or a missing badge.
    pub fn list_media_entries(&self) -> Result<Vec<MediaEntry>, DataError> {
        let mut entries = list_dir(&self.media_dir())?;
        let mut seen: HashSet<PathBuf> = entries
            .iter()
            .map(|entry| unique_key(&entry.path))
            .collect();
        for folder in self.media_folder_paths() {
            for entry in list_dir(&folder)? {
                if !seen.insert(unique_key(&entry.path)) {
                    continue;
                }
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

fn unique_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn list_dir(dir: &Path) -> Result<Vec<MediaEntry>, DataError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        entries.push(inspect_file(name, path));
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    Ok(entries)
}

fn inspect_file(name: String, path: PathBuf) -> MediaEntry {
    let exists = path.is_file();
    let kind = media_entry_kind(&path);
    let (width, height) = if exists && kind == MediaEntryKind::Image {
        image_dimensions(&path).map_or((None, None), |(width, height)| (Some(width), Some(height)))
    } else {
        (None, None)
    };
    MediaEntry {
        name,
        path,
        kind,
        exists,
        width,
        height,
        duration_secs: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("freeshow-rust-media-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Media")).unwrap();
        path
    }

    /// A PNG header whose IHDR says `width` × `height`. No pixel data is required.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn library_media_lists_sizes_folders_and_missing_files() {
        let root = root("entries");
        let extra = root.join("photos");
        fs::create_dir_all(&extra).unwrap();
        fs::write(root.join("Media/bg.png"), png_header(1920, 1080)).unwrap();
        fs::write(extra.join("sky.png"), png_header(8, 4)).unwrap();
        fs::write(root.join("Media/clip.mp4"), b"not a real video").unwrap();
        fs::write(
            root.join("settings.json"),
            serde_json::json!({"mediaFolders":{"photos":{"name":"Photos","path": extra}}})
                .to_string(),
        )
        .unwrap();

        let library = Library::open(&root);
        let entries = library.list_media_entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "bg.png");
        assert_eq!(entries[0].kind, MediaEntryKind::Image);
        assert_eq!(
            (entries[0].width, entries[0].height),
            (Some(1920), Some(1080))
        );
        assert_eq!(entries[0].label(), "bg.png  1920x1080");
        assert_eq!(entries[1].name, "clip.mp4");
        assert_eq!(entries[1].kind, MediaEntryKind::Video);
        assert_eq!(entries[1].label(), "clip.mp4");
        assert_eq!(entries[2].name, "sky.png");
        assert_eq!(entries[2].path, extra.join("sky.png"));
        assert_eq!(entries[2].label(), "photos/sky.png  8x4");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn image_headers_report_pixel_size() {
        assert_eq!(
            dimensions_from_bytes(&png_header(320, 200)),
            Some((320, 200))
        );
        let mut jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x10, 0x00, 0x20,
        ];
        jpeg.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(jpeg_size(&jpeg), Some((32, 16)));
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&7u16.to_le_bytes());
        gif.extend_from_slice(&9u16.to_le_bytes());
        assert_eq!(gif_size(&gif), Some((7, 9)));
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_symlink_is_listed_as_missing() {
        let root = root("missing");
        std::os::unix::fs::symlink(root.join("Media/no-such.png"), root.join("Media/gone.png"))
            .unwrap();
        let entries = Library::open(&root).list_media_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].exists);
        assert_eq!(entries[0].label(), "gone.png  missing");
        let _ = fs::remove_dir_all(&root);
    }
}
