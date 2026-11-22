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
    let duration_secs = if exists { media_duration(&path) } else { None };
    MediaEntry {
        name,
        path,
        kind,
        exists,
        width,
        height,
        duration_secs,
    }
}

/// Reads a duration in whole seconds from a common audio or video header.
pub fn media_duration(path: &Path) -> Option<u32> {
    let bytes = read_for_duration(path)?;
    wav_duration(&bytes)
        .or_else(|| mp4_duration(&bytes))
        .or_else(|| flac_duration(&bytes))
        .or_else(|| mp3_duration(&bytes))
}

fn read_for_duration(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(path).ok()?;
    let len = meta.len() as usize;
    let mut file = fs::File::open(path).ok()?;
    use std::io::{Read, Seek, SeekFrom};
    if len <= 2 * 1024 * 1024 {
        let mut bytes = Vec::with_capacity(len);
        file.read_to_end(&mut bytes).ok()?;
        return Some(bytes);
    }
    let window = 1024 * 1024;
    let mut head = vec![0u8; window];
    let read_head = file.read(&mut head).ok()?;
    head.truncate(read_head);
    file.seek(SeekFrom::End(-(window as i64))).ok()?;
    let mut tail = vec![0u8; window];
    let read_tail = file.read(&mut tail).ok()?;
    tail.truncate(read_tail);
    head.extend(tail);
    Some(head)
}

fn wav_duration(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 44 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut index = 12;
    let mut byte_rate = 0u32;
    let mut data_size = None;
    while index + 8 <= bytes.len() {
        let kind = &bytes[index..index + 4];
        let size = u32::from_le_bytes(bytes[index + 4..index + 8].try_into().ok()?) as usize;
        let start = index + 8;
        let end = start.saturating_add(size);
        if kind == b"fmt " && size >= 16 && end <= bytes.len() {
            byte_rate = u32::from_le_bytes(bytes[start + 8..start + 12].try_into().ok()?);
        } else if kind == b"data" {
            data_size = Some(size as u64);
        }
        index = start.saturating_add(size + size % 2);
    }
    if byte_rate == 0 {
        return None;
    }
    let seconds = data_size? / u64::from(byte_rate);
    u32::try_from(seconds).ok().filter(|seconds| *seconds > 0)
}

fn flac_duration(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 42 || !bytes.starts_with(b"fLaC") {
        return None;
    }
    // STREAMINFO sample rate is 20 bits starting at byte 18 of the file.
    let packed = u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let rate = packed >> 12;
    let samples = ((u64::from(bytes[21]) & 0x0F) << 32)
        | u64::from(u32::from_be_bytes(bytes[22..26].try_into().ok()?));
    if rate == 0 || samples == 0 {
        return None;
    }
    u32::try_from(samples / u64::from(rate))
        .ok()
        .filter(|seconds| *seconds > 0)
}

fn mp4_duration(bytes: &[u8]) -> Option<u32> {
    let mvhd = find_box(bytes, *b"mvhd")?;
    if mvhd.len() < 20 {
        return None;
    }
    let version = mvhd[0];
    let (timescale, duration) = if version == 1 {
        if mvhd.len() < 28 {
            return None;
        }
        let timescale = u32::from_be_bytes(mvhd[20..24].try_into().ok()?);
        let duration = u64::from_be_bytes(mvhd[24..32].try_into().ok()?);
        (timescale, duration)
    } else {
        let timescale = u32::from_be_bytes(mvhd[12..16].try_into().ok()?);
        let duration = u64::from(u32::from_be_bytes(mvhd[16..20].try_into().ok()?));
        (timescale, duration)
    };
    if timescale == 0 || duration == 0 {
        return None;
    }
    u32::try_from(duration / u64::from(timescale))
        .ok()
        .filter(|seconds| *seconds > 0)
}

fn find_box(data: &[u8], typ: [u8; 4]) -> Option<&[u8]> {
    let mut index = 0;
    while index + 8 <= data.len() {
        let mut size = u32::from_be_bytes(data[index..index + 4].try_into().ok()?) as usize;
        let box_type: [u8; 4] = data[index + 4..index + 8].try_into().ok()?;
        let mut header = 8;
        if size == 1 {
            if index + 16 > data.len() {
                return None;
            }
            size = u64::from_be_bytes(data[index + 8..index + 16].try_into().ok()?) as usize;
            header = 16;
        } else if size == 0 {
            size = data.len() - index;
        }
        if size < header || index + size > data.len() {
            return None;
        }
        let payload = &data[index + header..index + size];
        if box_type == typ {
            return Some(payload);
        }
        if box_type == *b"moov" || box_type == *b"trak" || box_type == *b"mdia" {
            if let Some(found) = find_box(payload, typ) {
                return Some(found);
            }
        }
        index += size;
    }
    None
}

fn mp3_duration(bytes: &[u8]) -> Option<u32> {
    let start = if bytes.starts_with(b"ID3") && bytes.len() >= 10 {
        let size = ((u32::from(bytes[6]) & 0x7F) << 21)
            | ((u32::from(bytes[7]) & 0x7F) << 14)
            | ((u32::from(bytes[8]) & 0x7F) << 7)
            | (u32::from(bytes[9]) & 0x7F);
        10 + size as usize
    } else {
        0
    };
    if start + 4 > bytes.len() || bytes[start] != 0xFF || bytes[start + 1] & 0xE0 != 0xE0 {
        return None;
    }
    // Look for a Xing/Info header in the first frame payload.
    let window = &bytes[start..bytes.len().min(start + 4096)];
    let tag = window
        .windows(4)
        .position(|chunk| chunk == b"Xing" || chunk == b"Info")?;
    let flags_at = tag + 4;
    if flags_at + 8 > window.len() {
        return None;
    }
    let flags = u32::from_be_bytes(window[flags_at..flags_at + 4].try_into().ok()?);
    if flags & 1 == 0 {
        return None;
    }
    let frames = u32::from_be_bytes(window[flags_at + 4..flags_at + 8].try_into().ok()?);
    let header = u32::from_be_bytes(window[0..4].try_into().ok()?);
    let version = (header >> 19) & 0b11;
    let layer = (header >> 17) & 0b11;
    let sr_index = (header >> 10) & 0b11;
    let mpeg1 = version == 0b11;
    let rate: u32 = match (mpeg1, sr_index) {
        (true, 0) => 44100,
        (true, 1) => 48000,
        (true, 2) => 32000,
        (false, 0) => 22050,
        (false, 1) => 24000,
        (false, 2) => 16000,
        _ => return None,
    };
    let samples: u64 = if layer == 0b01 {
        // Layer III
        if mpeg1 { 1152 } else { 576 }
    } else if layer == 0b10 {
        1152
    } else {
        384
    };
    if frames == 0 {
        return None;
    }
    u32::try_from(u64::from(frames) * samples / u64::from(rate))
        .ok()
        .filter(|seconds| *seconds > 0)
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

    fn pcm_wav(samples: u32, rate: u32) -> Vec<u8> {
        let data_size = samples * 2;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend(vec![0u8; data_size as usize]);
        bytes
    }

    fn mp4_with_duration(timescale: u32, duration: u32) -> Vec<u8> {
        let mut mvhd = vec![0u8; 20];
        mvhd[12..16].copy_from_slice(&timescale.to_be_bytes());
        mvhd[16..20].copy_from_slice(&duration.to_be_bytes());
        mvhd.extend(vec![0u8; 80]);
        let mut mvhd_box = (8 + mvhd.len() as u32).to_be_bytes().to_vec();
        mvhd_box.extend_from_slice(b"mvhd");
        mvhd_box.extend(mvhd);
        let mut moov = (8 + mvhd_box.len() as u32).to_be_bytes().to_vec();
        moov.extend_from_slice(b"moov");
        moov.extend(mvhd_box);
        let mut ftyp = 16u32.to_be_bytes().to_vec();
        ftyp.extend_from_slice(b"ftypisom");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        ftyp.extend(moov);
        ftyp
    }

    #[test]
    fn media_headers_report_duration() {
        assert_eq!(wav_duration(&pcm_wav(16_000, 8_000)), Some(2));
        assert_eq!(mp4_duration(&mp4_with_duration(1_000, 12_500)), Some(12));
        let root = root("duration");
        fs::write(root.join("Media/song.wav"), pcm_wav(8_000, 8_000)).unwrap();
        fs::write(root.join("Media/clip.mp4"), mp4_with_duration(600, 1_800)).unwrap();
        let entries = Library::open(&root).list_media_entries().unwrap();
        assert_eq!(entries[0].label(), "clip.mp4  0:03");
        assert_eq!(entries[1].label(), "song.wav  0:01");
        let _ = fs::remove_dir_all(&root);
    }
}
