// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};

fn bundle_paths(executable: &Path) -> Option<(PathBuf, PathBuf)> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    Some((
        contents.join("PlugIns"),
        contents.join("Helpers/gst-plugin-scanner"),
    ))
}

fn windows_bundle_paths(executable: &Path) -> Option<(PathBuf, PathBuf)> {
    let directory = executable.parent()?;
    directory
        .join("share/windows-build.json")
        .is_file()
        .then(|| {
            (
                directory.join("lib/gstreamer-1.0"),
                directory.join("gst-plugin-scanner.exe"),
            )
        })
}

/// Must run at the start of main, before any threads or GStreamer calls.
pub fn configure_bundle() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(any(target_os = "macos", windows)) {
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let paths = if cfg!(windows) {
        windows_bundle_paths(&executable)
    } else {
        bundle_paths(&executable)
    };
    let Some((plugins, scanner)) = paths else {
        return Ok(());
    };
    if !plugins.is_dir() || !scanner.is_file() {
        return Err("the app bundle is missing its GStreamer runtime".into());
    }
    for key in [
        "GST_PLUGIN_SYSTEM_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GST_PLUGIN_PATH",
        "GST_PLUGIN_PATH_1_0",
    ] {
        // SAFETY: main calls this before logging, UI setup, or spawning threads.
        unsafe {
            std::env::set_var(key, &plugins);
        }
    }
    for key in ["GST_PLUGIN_SCANNER", "GST_PLUGIN_SCANNER_1_0"] {
        // SAFETY: this is still single-threaded startup.
        unsafe {
            std::env::set_var(key, &scanner);
        }
    }
    if std::env::var_os("GST_REGISTRY").is_none() && std::env::var_os("GST_REGISTRY_1_0").is_none()
    {
        let cache = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA")
                .map(|root| PathBuf::from(root).join("FreeShow Rust/Cache"))
        } else {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Library/Caches/org.freeshow.rust"))
        };
        if let Some(cache) = cache {
            std::fs::create_dir_all(&cache)?;
            // SAFETY: this is still single-threaded startup.
            unsafe {
                std::env::set_var(
                    "GST_REGISTRY_1_0",
                    cache.join(format!("registry-{}.bin", std::env::consts::ARCH)),
                );
            }
        }
    }
    Ok(())
}

pub fn check(media: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let report = freeshow_media::check_runtime()?;
    println!(
        "{}",
        serde_json::json!({"checked_at":freeshow_core::local_timestamp(),"gstreamer":report.version,"missing":report.missing,"ndi_plugin":report.ndi_plugin})
    );
    if !report.missing.is_empty() {
        return Err("required GStreamer elements are missing".into());
    }
    if let Some(path) = media {
        let mut player = freeshow_media::VideoPlayer::open_with_network(path, &[], &[])?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(frame) = player.next_frame() {
                println!(
                    "{}",
                    serde_json::json!({"decoded_video":true,"width":frame.width,"height":frame.height})
                );
                break;
            }
            if let Some(error) = player.take_error() {
                return Err(error.into());
            }
            if std::time::Instant::now() >= deadline {
                return Err("no video frame decoded within five seconds".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_paths_follow_a_relocated_bundle_with_spaces() {
        assert_eq!(
            bundle_paths(Path::new(
                "/Applications/FreeShow Rust.app/Contents/MacOS/freeshow"
            )),
            Some((
                PathBuf::from("/Applications/FreeShow Rust.app/Contents/PlugIns"),
                PathBuf::from(
                    "/Applications/FreeShow Rust.app/Contents/Helpers/gst-plugin-scanner"
                )
            ))
        );
        assert!(bundle_paths(Path::new("/tmp/freeshow")).is_none());
    }
}
