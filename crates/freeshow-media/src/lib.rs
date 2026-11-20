// SPDX-License-Identifier: GPL-3.0-only

//! Media playback for the audience output. Stills are rasters. Audio is decoded
//! to PCM and played with cpal. Video frames come from GStreamer when that
//! feature is enabled.

mod audio;
mod video;

pub use audio::{AudioOutput, Pcm, Playback, apply_volume, decode_audio, output_devices};
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
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => MediaKind::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => MediaKind::Video,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => MediaKind::Audio,
        _ => MediaKind::Other,
    }
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
    }
}
