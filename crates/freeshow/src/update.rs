// SPDX-License-Identifier: GPL-3.0-only

//! Check GitHub Releases for a newer FreeShow-rs build. No credentials.

use std::io::Read;
use std::time::Duration;

#[allow(dead_code)]
const DEFAULT_REPO: &str = "subaasw/FreeShow-rs";
const USER_AGENT: &str = "FreeShow-rs";

/// A published GitHub Release compared to this binary's version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseUpdate {
    pub tag: String,
    pub version: String,
    pub newer: bool,
    pub html_url: String,
    pub download_url: Option<String>,
    /// Sidecar checksum for `download_url`, when the release published one.
    pub checksum_url: Option<String>,
}

/// Looks up the latest GitHub Release for `repo` (`owner/name`).
pub fn check_latest(repo: &str, current: &str, api_base: &str) -> Result<ReleaseUpdate, String> {
    let repo = repo.trim().trim_matches('/');
    if repo.is_empty() || !repo.contains('/') {
        return Err("repository must be owner/name".into());
    }
    let url = format!(
        "{}/repos/{repo}/releases/latest",
        api_base.trim_end_matches('/')
    );
    let response = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|err| err.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("GitHub returned {}", response.status()));
    }
    let body = response.into_string().map_err(|err| err.to_string())?;
    parse_release(&body, current)
}

#[allow(dead_code)]
pub fn check_default() -> Result<ReleaseUpdate, String> {
    check_latest(
        DEFAULT_REPO,
        env!("CARGO_PKG_VERSION"),
        "https://api.github.com",
    )
}

pub fn parse_release(body: &str, current: &str) -> Result<ReleaseUpdate, String> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("release JSON: {err}"))?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .ok_or_else(|| "release is missing tag_name".to_string())?;
    let version = tag.trim_start_matches('v').to_string();
    let html_url = json
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let assets = json
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let download = assets.iter().find_map(|asset| {
        let name = asset.get("name").and_then(serde_json::Value::as_str)?;
        if !asset_matches_platform(name) {
            return None;
        }
        let url = asset
            .get("browser_download_url")
            .and_then(serde_json::Value::as_str)?;
        Some((name.to_string(), url.to_string()))
    });
    let download_url = download.as_ref().map(|(_, url)| url.clone());
    let checksum_url = download
        .as_ref()
        .and_then(|(name, _)| checksum_asset_url(&assets, name));
    Ok(ReleaseUpdate {
        tag: tag.to_string(),
        version: version.clone(),
        newer: is_newer(&version, current),
        html_url,
        download_url,
        checksum_url,
    })
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    compare_versions(latest) > compare_versions(current)
}

fn compare_versions(value: &str) -> [u64; 3] {
    let mut parts = value
        .trim()
        .trim_start_matches('v')
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok());
    [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ]
}

fn asset_matches_platform(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let os_ok = if cfg!(target_os = "linux") {
        name.contains("linux") || name.ends_with(".appimage") || name.ends_with(".tar.gz")
    } else if cfg!(target_os = "macos") {
        name.contains("mac") || name.contains("darwin") || name.ends_with(".dmg")
    } else if cfg!(target_os = "windows") {
        name.contains("win") || name.ends_with(".msi") || name.ends_with(".exe")
    } else {
        true
    };
    let arch_ok = if cfg!(target_arch = "aarch64") {
        name.contains("aarch64") || name.contains("arm64") || !looks_arch_specific(&name)
    } else if cfg!(target_arch = "x86_64") {
        name.contains("x86_64")
            || name.contains("amd64")
            || name.contains("x64")
            || !looks_arch_specific(&name)
    } else {
        true
    };
    os_ok && arch_ok && !name.contains("sha256") && !name.ends_with(".sig")
}

fn looks_arch_specific(name: &str) -> bool {
    name.contains("x86_64")
        || name.contains("amd64")
        || name.contains("x64")
        || name.contains("aarch64")
        || name.contains("arm64")
}

fn checksum_asset_url(assets: &[serde_json::Value], asset_name: &str) -> Option<String> {
    let want = [
        format!("{asset_name}.sha256"),
        format!("{asset_name}.sha256.txt"),
        "SHA256SUMS".into(),
        "SHA256SUMS.txt".into(),
        "sha256sums.txt".into(),
    ];
    assets.iter().find_map(|asset| {
        let name = asset.get("name").and_then(serde_json::Value::as_str)?;
        if !want.iter().any(|wanted| wanted.eq_ignore_ascii_case(name)) {
            return None;
        }
        asset
            .get("browser_download_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// SHA-256 hex digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Read a GNU-coreutils or `*  filename` sidecar and return the hex digest.
pub fn parse_checksum_file(text: &str, asset_name: &str) -> Option<String> {
    let file_name = std::path::Path::new(asset_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(asset_name);
    let mut fallback = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (digest, name) = if let Some((digest, name)) = line.split_once("  ") {
            (digest.trim(), name.trim())
        } else if let Some((digest, name)) = line.split_once(" *") {
            (digest.trim(), name.trim())
        } else {
            let digest = line.split_whitespace().next().unwrap_or("");
            (digest, "")
        };
        if !is_sha256_hex(digest) {
            continue;
        }
        if name.is_empty() {
            fallback = Some(digest.to_ascii_lowercase());
            continue;
        }
        let listed = std::path::Path::new(name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(name);
        if listed == file_name || listed == asset_name || name == asset_name {
            return Some(digest.to_ascii_lowercase());
        }
        if fallback.is_none() {
            fallback = Some(digest.to_ascii_lowercase());
        }
    }
    fallback
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub fn verify_checksum(bytes: &[u8], expected: &str) -> Result<(), String> {
    let expected = expected.trim().to_ascii_lowercase();
    if !is_sha256_hex(&expected) {
        return Err("checksum is not a SHA-256 hex digest".into());
    }
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(format!(
            "checksum mismatch: got {actual}, expected {expected}"
        ));
    }
    Ok(())
}

/// Result of writing a verified payload over a binary, with a rollback copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub target: std::path::PathBuf,
    pub backup: std::path::PathBuf,
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|err| err.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("download returned {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(256 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > 256 * 1024 * 1024 {
        return Err("download is larger than 256 MiB".into());
    }
    Ok(bytes)
}

/// Download `update`, verify its checksum, and swap it over `target`.
pub fn install_update(
    update: &ReleaseUpdate,
    target: &std::path::Path,
) -> Result<InstallResult, String> {
    let url = update
        .download_url
        .as_deref()
        .ok_or_else(|| "release has no download for this platform".to_string())?;
    let bytes = http_get_bytes(url)?;
    if let Some(checksum_url) = update.checksum_url.as_deref() {
        let text =
            String::from_utf8(http_get_bytes(checksum_url)?).map_err(|err| err.to_string())?;
        let name = url.rsplit('/').next().unwrap_or("freeshow");
        let expected = parse_checksum_file(&text, name)
            .ok_or_else(|| "checksum file did not contain a SHA-256 digest".to_string())?;
        verify_checksum(&bytes, &expected)?;
    } else {
        return Err("release has no checksum sidecar; refusing to install".into());
    }
    swap_bytes(target, &bytes)
}

/// Replace `target` with `bytes`, keeping a `.bak` copy for [`rollback`].
pub fn swap_bytes(target: &std::path::Path, bytes: &[u8]) -> Result<InstallResult, String> {
    if bytes.is_empty() {
        return Err("update payload is empty".into());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let staging = target.with_file_name({
        let mut name = target
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "freeshow".into());
        name.push(".new");
        name
    });
    let backup = target.with_file_name({
        let mut name = target
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "freeshow".into());
        name.push(".bak");
        name
    });
    std::fs::write(&staging, bytes).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&staging)
            .map_err(|err| err.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&staging, perms).map_err(|err| err.to_string())?;
    }
    if target.exists() {
        std::fs::rename(target, &backup).map_err(|err| {
            let _ = std::fs::remove_file(&staging);
            format!("could not back up the current binary: {err}")
        })?;
    } else if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    if let Err(err) = std::fs::rename(&staging, target) {
        let _ = rollback(target, &backup);
        let _ = std::fs::remove_file(&staging);
        return Err(format!("could not install the new binary: {err}"));
    }
    Ok(InstallResult {
        target: target.to_path_buf(),
        backup,
    })
}

/// Restore the backup written by [`swap_bytes`].
pub fn rollback(target: &std::path::Path, backup: &std::path::Path) -> Result<(), String> {
    if !backup.exists() {
        return Err("backup is missing".into());
    }
    if target.exists() {
        let failed = target.with_file_name({
            let mut name = target
                .file_name()
                .map(|name| name.to_os_string())
                .unwrap_or_else(|| "freeshow".into());
            name.push(".failed");
            name
        });
        let _ = std::fs::rename(target, &failed);
    }
    std::fs::rename(backup, target).map_err(|err| err.to_string())
}

/// Start `path` with `args` and detach it from this process.
pub fn relaunch(path: &std::path::Path, args: &[String]) -> Result<u32, String> {
    let child = std::process::Command::new(path)
        .args(args)
        .spawn()
        .map_err(|err| format!("could not relaunch {}: {err}", path.display()))?;
    Ok(child.id())
}

/// Install from a verified payload, then start the new binary.
#[allow(dead_code)]
pub fn install_and_relaunch(
    update: &ReleaseUpdate,
    target: &std::path::Path,
    args: &[String],
) -> Result<InstallResult, String> {
    let installed = install_update(update, target)?;
    if let Err(err) = relaunch(&installed.target, args) {
        let _ = rollback(&installed.target, &installed.backup);
        return Err(err);
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn serve_once(body: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|chunk| chunk == b"\r\n\r\n") && request.len() < 65536 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            stream.write_all(payload.as_bytes()).unwrap();
        });
        (format!("http://{addr}"), thread)
    }

    #[test]
    fn a_newer_tag_is_an_update_and_picks_the_platform_asset() {
        let body = r#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/subaasw/FreeShow-rs/releases/tag/v0.2.0",
            "assets": [
                {"name": "FreeShow-rs-0.2.0-windows.exe", "browser_download_url": "https://example/win"},
                {"name": "FreeShow-rs-0.2.0-linux-x86_64.tar.gz", "browser_download_url": "https://example/linux"},
                {"name": "FreeShow-rs-0.2.0-macos-arm64.dmg", "browser_download_url": "https://example/mac"},
                {"name": "FreeShow-rs-0.2.0-linux-x86_64.tar.gz.sha256", "browser_download_url": "https://example/linux.sha256"}
            ]
        }"#;
        let update = parse_release(body, "0.1.0").unwrap();
        assert_eq!(update.tag, "v0.2.0");
        assert!(update.newer);
        assert!(update.html_url.contains("v0.2.0"));
        if cfg!(target_os = "linux") {
            assert_eq!(
                update.download_url.as_deref(),
                Some("https://example/linux")
            );
            assert_eq!(
                update.checksum_url.as_deref(),
                Some("https://example/linux.sha256")
            );
        }
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn check_latest_reads_the_releases_api_without_credentials() {
        let body = r#"{"tag_name":"v0.1.0","html_url":"https://example/tag","assets":[]}"#;
        let (base, server) = serve_once(body);
        let update = check_latest("subaasw/FreeShow-rs", "0.1.0", &base).unwrap();
        server.join().unwrap();
        assert_eq!(update.version, "0.1.0");
        assert!(!update.newer);
        assert!(update.checksum_url.is_none());
    }

    #[test]
    fn checksum_files_and_a_failed_swap_roll_back() {
        let payload = b"freeshow-update-payload";
        let digest = sha256_hex(payload);
        assert_eq!(
            parse_checksum_file(
                &format!("{digest}  freeshow-linux-x86_64.tar.gz\n"),
                "freeshow-linux-x86_64.tar.gz"
            )
            .as_deref(),
            Some(digest.as_str())
        );
        assert!(verify_checksum(payload, &digest).is_ok());
        assert!(verify_checksum(payload, &"00".repeat(32)).is_err());

        let root =
            std::env::temp_dir().join(format!("freeshow-update-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("freeshow");
        std::fs::write(&target, b"old-binary").unwrap();
        let installed = swap_bytes(&target, payload).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), payload);
        assert_eq!(std::fs::read(&installed.backup).unwrap(), b"old-binary");
        rollback(&target, &installed.backup).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old-binary");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_update_verifies_the_sidecar_and_refuses_a_bad_hash() {
        let payload = b"hello-freeshow";
        let digest = sha256_hex(payload);
        let sidecar_bytes = format!("{digest}  freeshow-linux\n").into_bytes();
        let payload_bytes = payload.to_vec();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|chunk| chunk == b"\r\n\r\n") && request.len() < 65536
                {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let path = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let (status, body, ty) = if path.ends_with("freeshow-linux.sha256") {
                    ("200 OK", sidecar_bytes.as_slice(), "text/plain")
                } else if path.ends_with("freeshow-linux") {
                    (
                        "200 OK",
                        payload_bytes.as_slice(),
                        "application/octet-stream",
                    )
                } else if path.ends_with("bad.sha256") {
                    (
                        "200 OK",
                        b"0000000000000000000000000000000000000000000000000000000000000000  freeshow-linux\n"
                            .as_slice(),
                        "text/plain",
                    )
                } else {
                    ("404 Not Found", b"missing".as_slice(), "text/plain")
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let base = format!("http://{addr}");
        let root =
            std::env::temp_dir().join(format!("freeshow-update-http-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("freeshow");
        std::fs::write(&target, b"previous").unwrap();
        let ok = ReleaseUpdate {
            tag: "v0.2.0".into(),
            version: "0.2.0".into(),
            newer: true,
            html_url: "https://example/tag".into(),
            download_url: Some(format!("{base}/freeshow-linux")),
            checksum_url: Some(format!("{base}/freeshow-linux.sha256")),
        };
        let installed = install_update(&ok, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello-freeshow");
        assert_eq!(std::fs::read(&installed.backup).unwrap(), b"previous");

        let bad = ReleaseUpdate {
            tag: "v0.3.0".into(),
            version: "0.3.0".into(),
            newer: true,
            html_url: "https://example/tag".into(),
            download_url: Some(format!("{base}/freeshow-linux")),
            checksum_url: Some(format!("{base}/bad.sha256")),
        };
        assert!(install_update(&bad, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"hello-freeshow");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relaunch_starts_the_new_binary() {
        let root = std::env::temp_dir().join(format!("freeshow-relaunch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("started");
        #[cfg(unix)]
        let pid = {
            use std::os::unix::fs::PermissionsExt;
            let script = root.join("relaunch.sh");
            std::fs::write(&script, "#!/bin/sh\nprintf ok > \"$1\"\n").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            relaunch(&script, &[marker.to_string_lossy().into_owned()]).unwrap()
        };
        #[cfg(windows)]
        let pid = {
            let copy = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join(r"System32\robocopy.exe");
            let source = root.join("source folder");
            std::fs::create_dir(&source).unwrap();
            std::fs::write(source.join("started"), "ok").unwrap();
            relaunch(
                &copy,
                &[
                    source.to_string_lossy().into_owned(),
                    root.to_string_lossy().into_owned(),
                    "started".into(),
                    "/R:0".into(),
                    "/W:0".into(),
                ],
            )
            .unwrap()
        };
        assert!(pid > 0);
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(&marker).ok().as_deref() == Some("ok") {
                let _ = std::fs::remove_dir_all(&root);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&root);
        panic!("relaunched process did not write the marker");
    }
}
