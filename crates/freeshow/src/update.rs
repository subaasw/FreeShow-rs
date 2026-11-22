// SPDX-License-Identifier: GPL-3.0-only

//! Check GitHub Releases for a newer FreeShow-rs build. No credentials.

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
    let download_url = json
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name").and_then(serde_json::Value::as_str)?;
                if !asset_matches_platform(name) {
                    return None;
                }
                asset
                    .get("browser_download_url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        });
    Ok(ReleaseUpdate {
        tag: tag.to_string(),
        version: version.clone(),
        newer: is_newer(&version, current),
        html_url,
        download_url,
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
                {"name": "FreeShow-rs-0.2.0-macos-arm64.dmg", "browser_download_url": "https://example/mac"}
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
    }
}
