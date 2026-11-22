// SPDX-License-Identifier: GPL-3.0-only

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

/// Local wall-clock time with its UTC offset, e.g. `2026-11-22T18:05:09.123+05:45`.
pub fn local_timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        .to_string()
}

/// Log lines carry the operator's local time and zone offset instead of UTC `Z`.
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        w.write_str(&local_timestamp())
    }
}

pub fn log_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("freeshow-rust.log")
}

/// Creates the config directory, appends a start line to the log, and installs
/// a stderr tracing subscriber when one is not already set.
pub fn init_logging(config_dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(config_dir)?;
    let path = log_file_path(config_dir);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "--- freeshow-rust start {} ---", local_timestamp())?;
    let _ = tracing_subscriber::fmt()
        .with_timer(LocalTime)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::local_timestamp;

    #[test]
    fn timestamps_are_local_with_an_explicit_offset() {
        let stamp = local_timestamp();
        assert_eq!(
            stamp.len(),
            "2026-11-22T18:05:09.123+05:45".len(),
            "{stamp}"
        );
        let zone = &stamp[23..];
        assert!(zone.starts_with('+') || zone.starts_with('-'), "{stamp}");
        assert_eq!(&zone[3..4], ":", "{stamp}");
    }
}
