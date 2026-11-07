// SPDX-License-Identifier: GPL-3.0-only

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;

pub fn log_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("freeshow-rust.log")
}

/// Creates the config directory, appends a start line to the log, and installs
/// a stderr tracing subscriber when one is not already set.
pub fn init_logging(config_dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(config_dir)?;
    let path = log_file_path(config_dir);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "--- freeshow-rust start ---")?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
    Ok(path)
}
