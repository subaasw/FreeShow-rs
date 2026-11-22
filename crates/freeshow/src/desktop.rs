// SPDX-License-Identifier: GPL-3.0-only

#[cfg(any(target_os = "linux", test))]
use std::path::Path;

#[cfg(target_os = "linux")]
const OWNER: &str = "# Managed by freeshow-rust";

#[cfg(any(target_os = "linux", test))]
fn value(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = path
        .to_str()
        .ok_or("desktop integration needs a UTF-8 installation path")?;
    if path.contains(['\n', '\r', '\0']) {
        return Err("desktop paths cannot contain newlines or NUL bytes".into());
    }
    Ok(path.replace('\\', "\\\\"))
}

#[cfg(any(target_os = "linux", test))]
fn executable(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = path
        .to_str()
        .ok_or("desktop integration needs a UTF-8 installation path")?;
    if path.contains('=') {
        return Err("desktop executable paths cannot contain '='".into());
    }
    let mut quoted = String::new();
    for ch in path.chars() {
        if ['\\', '"', '`', '$'].contains(&ch) {
            quoted.push('\\');
        }
        if ch == '%' {
            quoted.push('%');
        }
        quoted.push(ch);
    }
    // String-value unescaping happens before Exec argument unquoting.
    // GLib checks that argv[0] exists before expanding %% field codes.
    // Use env so installation folders containing a literal percent still launch.
    Ok(format!("/usr/bin/env \"{}\"", value(Path::new(&quoted))?))
}

pub fn install(uninstall: bool) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = uninstall;
        return Err("desktop installation is only available in the Linux archive".into());
    }
    #[cfg(target_os = "linux")]
    {
        use std::{fs, path::PathBuf};
        let binary = std::env::current_exe()?;
        let root = binary
            .parent()
            .and_then(Path::parent)
            .ok_or("invalid installation directory")?;
        if !root.join("AppRun").is_file()
            || !root
                .join("share/icons/hicolor/512x512/apps/freeshow-rust.png")
                .is_file()
        {
            return Err("extract the Linux archive and run its install.sh".into());
        }
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let desktop = data.join("applications/org.freeshow.rust.desktop");
        let launcher = home.join(".local/bin/freeshow-rust");
        if desktop.exists() && !fs::read_to_string(&desktop)?.starts_with(OWNER) {
            return Err("refusing to replace an unrelated desktop entry".into());
        }
        if fs::symlink_metadata(&launcher).is_ok_and(|meta| !meta.file_type().is_symlink()) {
            return Err(
                "refusing to replace an existing file at ~/.local/bin/freeshow-rust".into(),
            );
        }
        if !desktop.exists()
            && fs::symlink_metadata(&launcher).is_ok()
            && fs::read_link(&launcher)? != root.join("AppRun")
        {
            return Err("refusing to replace an unrelated launcher symlink".into());
        }
        if uninstall {
            if desktop.exists() {
                fs::remove_file(&desktop)?;
            }
            if fs::symlink_metadata(&launcher).is_ok() {
                fs::remove_file(&launcher)?;
            }
        } else {
            let program = root.join("AppRun");
            let mut entry = String::new();
            for line in include_str!("../../../assets/org.freeshow.rust.desktop").lines() {
                if line.starts_with("Exec=") {
                    entry.push_str(&format!("Exec={} --show %f", executable(&program)?));
                } else if line.starts_with("TryExec=") {
                    entry.push_str(&format!("TryExec={}", value(&program)?));
                } else if line.starts_with("Icon=") {
                    entry.push_str(&format!(
                        "Icon={}",
                        value(&root.join("share/icons/hicolor/512x512/apps/freeshow-rust.png"))?
                    ));
                } else {
                    entry.push_str(line);
                }
                entry.push('\n');
            }
            fs::create_dir_all(desktop.parent().unwrap())?;
            fs::create_dir_all(launcher.parent().unwrap())?;
            use std::io::Write;
            let temporary = desktop.with_extension(format!("{}.tmp", freeshow_data::new_id()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(entry.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(temporary, &desktop)?;
            if fs::symlink_metadata(&launcher).is_ok() {
                fs::remove_file(&launcher)?;
            }
            std::os::unix::fs::symlink(&program, &launcher)?;
        }
        if let Err(error) = std::process::Command::new("update-desktop-database")
            .arg(desktop.parent().unwrap())
            .status()
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        println!(
            "{}",
            if uninstall {
                "Removed FreeShow Rust desktop integration; library files were kept."
            } else {
                "Installed FreeShow Rust in your application menu."
            }
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_exec_paths_escape_both_string_and_argument_levels() {
        assert_eq!(
            executable(Path::new("/tmp/My App/AppRun")).unwrap(),
            "/usr/bin/env \"/tmp/My App/AppRun\""
        );
        let quoted = executable(Path::new("/tmp/a\\b\"$`%/AppRun")).unwrap();
        assert_eq!(quoted, r#"/usr/bin/env "/tmp/a\\\\b\\"\\$\\`%%/AppRun""#);
        assert!(executable(Path::new("/tmp/a=b/AppRun")).is_err());
        assert!(executable(Path::new("/tmp/a\nb/AppRun")).is_err());
    }
}
