// SPDX-License-Identifier: GPL-3.0-only

//! Media playback for the audience output. Stills are rasters. Audio is decoded
//! to PCM and played with cpal. Video frames come from GStreamer when that
//! feature is enabled.

mod audio;
mod runtime;
pub use runtime::{RuntimeStatus, check_runtime};
mod network;
mod video;
pub use freeshow_core::NetworkRoute;
pub use network::{network_audio_mix, network_channel};

pub use audio::{
    AudioOutput, AudioPlaybackOptions, Pcm, Playback, apply_volume, decode_audio, output_devices,
};
pub use video::{VideoPlayer, cameras, video_output_devices};

use std::path::Path;

#[derive(Debug)]
pub enum MediaError {
    Decode(String),
    Audio(String),
    Video(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(message) | Self::Audio(message) | Self::Video(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for MediaError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Other,
}

pub fn media_kind(path: &Path) -> MediaKind {
    let raw = path.to_string_lossy();
    if is_remote_url(&raw) {
        return remote_media_kind(&raw);
    }
    match extension_of(&raw).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => MediaKind::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => MediaKind::Video,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => MediaKind::Audio,
        _ => MediaKind::Other,
    }
}

/// A path or media field that points at an HTTP or HTTPS resource.
pub fn is_remote_url(path: &str) -> bool {
    let path = path.trim();
    path.starts_with("http://") || path.starts_with("https://")
}

fn extension_of(path: &str) -> String {
    let bare = path.split(['?', '#']).next().unwrap_or(path);
    Path::new(bare)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn remote_media_kind(path: &str) -> MediaKind {
    match extension_of(path).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => MediaKind::Image,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => MediaKind::Audio,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "m3u8" | "mpd" => MediaKind::Video,
        _ if is_stream_page(path) => MediaKind::Video,
        _ => MediaKind::Video,
    }
}

/// YouTube, Vimeo, and similar pages that are not a direct media file.
pub fn is_stream_page(path: &str) -> bool {
    host_of(path).is_some_and(|host| {
        host == "youtube.com"
            || host.ends_with(".youtube.com")
            || host == "youtu.be"
            || host == "vimeo.com"
            || host.ends_with(".vimeo.com")
    })
}

fn host_of(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("https://")
        .or_else(|| path.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.rsplit_once(']').map_or(host, |(_, host)| host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.trim().trim_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_kinds() {
        assert_eq!(media_kind(Path::new("bg.PNG")), MediaKind::Image);
        assert_eq!(media_kind(Path::new("clip.mp4")), MediaKind::Video);
        assert_eq!(media_kind(Path::new("song.mp3")), MediaKind::Audio);
        assert_eq!(media_kind(Path::new("notes.txt")), MediaKind::Other);
        assert_eq!(
            media_kind(Path::new("https://cdn.example/cover.png?token=1")),
            MediaKind::Image
        );
        assert_eq!(
            media_kind(Path::new("https://cdn.example/clip.mp4")),
            MediaKind::Video
        );
        assert_eq!(
            media_kind(Path::new("https://vimeo.com/123")),
            MediaKind::Video
        );
        assert_eq!(
            media_kind(Path::new("https://youtu.be/abcd")),
            MediaKind::Video
        );
        assert!(is_stream_page("https://vimeo.com/123"));
        assert!(!is_stream_page("https://cdn.example/clip.mp4"));
    }
}
