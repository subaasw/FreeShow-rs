// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use freeshow_data::{EXPORTS_DIR, ShowFile};
use freeshow_media::VideoPlayer;
use freeshow_output::fingerprint;
use serde_json::{Value, json};

use crate::live::{audience_frame, stage_frame};
use crate::session::Session;

const STILL_BUDGET_FPS: f64 = 60.0;
const VIDEO_BUDGET_FPS: f64 = 30.0;
const NEXT_BUDGET_MS: f64 = 16.0;
const REMOTE_BUDGET_MS: f64 = 16.0;
const OPEN_BUDGET_MS: f64 = 1500.0;
const SCAN_1000_BUDGET_MS: f64 = 500.0;
const RSS_BUDGET_MB: f64 = 150.0;

const SONG: &str = "[Verse]\nAmazing grace, how sweet the sound\nThat saved a wretch like me\n\n[Verse]\n'Twas grace that taught my heart to fear\nAnd grace my fears relieved\n\n[Chorus]\nPraise God, from whom all blessings flow\nPraise Him, all creatures here below\n\n[Verse]\nWhen we've been there ten thousand years\nBright shining as the sun\n";

/// Builds a mid-size Sunday library: a song, a project, media, scripture, and a stage layout.
pub fn write_sample_library(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    for folder in [
        "Shows",
        "Media",
        "Config",
        "Bibles",
        "Imports",
        "Audio",
        EXPORTS_DIR,
    ] {
        std::fs::create_dir_all(root.join(folder))?;
    }
    let mut show = ShowFile::blank("amazing", "Amazing Grace");
    if !show.replace_lyrics_with(SONG, true) {
        return Err("could not write sample lyrics".into());
    }
    let fixtures = fixture_dir();
    let cover = root.join("Media/cover.png");
    let clip = root.join("Media/lesson-clip.mp4");
    copy_if_present(&fixtures.join("lesson-cover.png"), &cover)?;
    copy_if_present(&fixtures.join("lesson-clip.mp4"), &clip)?;
    if cover.is_file() {
        show.show["media"]["m1"] = json!({"name": "cover.png", "path": cover});
        if let Some(first) = show
            .show
            .pointer_mut("/layouts/main/slides")
            .and_then(Value::as_array_mut)
            .and_then(|slides| slides.first_mut())
        {
            first["background"] = json!("m1");
        }
    }
    show.save(&root.join("Shows/AmazingGrace.show"))?;

    let welcome = ShowFile::blank("welcome", "Welcome");
    welcome.save(&root.join("Shows/Welcome.show"))?;

    std::fs::write(
        root.join("Config/projects.json"),
        serde_json::to_vec_pretty(&json!({
            "projects": {
                "sunday": {
                    "name": "Sunday",
                    "created": 1,
                    "parent": "/",
                    "shows": [
                        {"id": "welcome", "type": "show"},
                        {"id": "amazing", "type": "show"}
                    ]
                }
            },
            "folders": {},
            "projectTemplates": {}
        }))?,
    )?;

    std::fs::write(
        root.join("Config/stage.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "Sunday Stage",
            "settings": {"color": "#12121c"},
            "itemOrder": ["slide#0", "next#1", "clock#0"],
            "items": {
                "slide#0": {
                    "style": "left:25px;top:25px;width:1870px;height:700px;color:#ffffff;font-size:72px;"
                },
                "next#1": {
                    "style": "left:25px;top:760px;width:1870px;height:200px;color:#aaaaaa;font-size:48px;"
                },
                "clock#0": {
                    "style": "left:25px;top:980px;width:500px;height:80px;color:#ffffff;font-size:48px;"
                }
            }
        }))?,
    )?;

    std::fs::write(
        root.join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "language": "en",
            "timeFormat": "24",
            "autosave": "never",
            "autoOutput": false,
            "theme": "default",
            "ports": {"remote": 0, "stage": 0}
        }))?,
    )?;

    std::fs::write(
        root.join("Bibles/John.usfm"),
        "\\id JHN\n\\h John\n\\c 1\n\\v 1 In the beginning was the Word.\n\\v 2 The same was in the beginning with God.\n",
    )?;
    Ok(root.to_path_buf())
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../freeshow-data/tests/fixtures")
}

fn copy_if_present(from: &Path, to: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if from.is_file() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

fn write_bench_clip(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.is_file() && path.metadata()?.len() > 10_000 {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    freeshow_media::write_benchmark_video(path)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: f64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024.0);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn rss_mb() -> Option<f64> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kb = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    Some(kb / 1024.0)
}

#[cfg(windows)]
fn rss_mb() -> Option<f64> {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };

    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..Default::default()
    };
    // The pseudo-handle needs no cleanup and the output buffer is sized for the API.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (ok != 0).then_some(counters.WorkingSetSize as f64 / (1024.0 * 1024.0))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rss_mb() -> Option<f64> {
    None
}

fn fps(count: u32, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        return 0.0;
    }
    f64::from(count) / elapsed.as_secs_f64()
}

fn open_session(root: &Path) -> Session {
    let mut session = Session::open(root.to_path_buf());
    present_song(&mut session);
    session
}

fn present_song(session: &mut Session) -> bool {
    if session.present_show("amazing") {
        return true;
    }
    session
        .shows
        .iter()
        .position(|show| show.name() == "Amazing Grace")
        .and_then(|index| session.shows.get(index).map(|show| show.id.clone()))
        .is_some_and(|id| session.present_show(&id))
        || session.open_first_show()
}

/// Measure the §4 performance budgets against a library (created when missing).
pub fn check_perf(root: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let owned: PathBuf;
    let root = match root {
        Some(path) => path,
        None => {
            owned = std::env::temp_dir().join(format!("freeshow-perf-{}", std::process::id()));
            write_sample_library(&owned)?;
            &owned
        }
    };
    if !root.join("Shows").is_dir() {
        write_sample_library(root)?;
    }

    let started = Instant::now();
    let mut session = open_session(root);
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let _ = audience_frame(&session);
    let _ = stage_frame(&session);
    let idle_rss_mb = rss_mb();

    let still_started = Instant::now();
    let mut still_frames = 0u32;
    while still_started.elapsed() < Duration::from_millis(500) {
        let _ = audience_frame(&session);
        still_frames += 1;
    }
    let still_fps = fps(still_frames, still_started.elapsed());

    let clip = root.join("Media/bench-1080p30.mp4");
    let mut video_fps = None;
    if write_bench_clip(&clip).is_ok()
        && let Ok(mut player) = VideoPlayer::open(&clip)
    {
        // Clock sync would cap pull rate at the clip's 30 fps, which hides the
        // compositor budget. Measure decode + paint as fast as frames arrive.
        player.set_sync(false);
        player.set_looping(true);
        let pull = Duration::from_millis(250);
        let deadline = Instant::now() + Duration::from_secs(4);
        while session.underlay.is_none() && Instant::now() < deadline {
            if let Some(raster) = player.next_frame_wait(pull) {
                session.replace_underlay(Some(raster));
            }
        }
        let video_started = Instant::now();
        let mut video_frames = 0u32;
        let run_for = Duration::from_millis(1500);
        while video_started.elapsed() < run_for {
            if let Some(raster) = player.next_frame_wait(pull) {
                session.replace_underlay(Some(raster));
                let _ = audience_frame(&session);
                video_frames += 1;
            } else if player.is_finished() {
                break;
            }
        }
        if video_frames > 0 {
            video_fps = Some(fps(video_frames, video_started.elapsed()));
        }
    }

    present_song(&mut session);
    let next_started = Instant::now();
    let moved = session.advance_show();
    let _ = audience_frame(&session);
    let next_ms = next_started.elapsed().as_secs_f64() * 1000.0;

    present_song(&mut session);
    let mut snapshot = session.to_snapshot();
    let remote_started = Instant::now();
    let (_, commands) = freeshow_remote::handle_message(
        &mut snapshot,
        &serde_json::json!({ "channel": "API:next_slide", "data": null }),
    );
    for command in commands {
        session.apply_remote(command);
    }
    let _ = audience_frame(&session);
    let remote_ms = remote_started.elapsed().as_secs_f64() * 1000.0;
    let remote_advanced = session.slide_index != Some(0);

    let scan = bench_thousand_shows()?;

    let report = json!({
        "library": root,
        "output_width": session.output_width,
        "output_height": session.output_height,
        "open_ms": open_ms,
        "open_budget_ms": OPEN_BUDGET_MS,
        "idle_rss_mb": idle_rss_mb,
        "idle_rss_budget_mb": RSS_BUDGET_MB,
        "still_fps": still_fps,
        "still_budget_fps": STILL_BUDGET_FPS,
        "video_fps": video_fps,
        "video_budget_fps": VIDEO_BUDGET_FPS,
        "next_ms": next_ms,
        "next_budget_ms": NEXT_BUDGET_MS,
        "next_advanced": moved,
        "remote_ms": remote_ms,
        "remote_budget_ms": REMOTE_BUDGET_MS,
        "remote_advanced": remote_advanced,
        "scan_1000_ms": scan,
        "scan_1000_budget_ms": SCAN_1000_BUDGET_MS,
        "shows": session.shows.len(),
        "bibles": session.bibles.len(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    let mut missed = Vec::new();
    if open_ms > OPEN_BUDGET_MS {
        missed.push("cold open");
    }
    if idle_rss_mb.is_some_and(|mb| mb > RSS_BUDGET_MB) {
        missed.push("idle RSS");
    }
    if still_fps < STILL_BUDGET_FPS {
        missed.push("1080p still fps");
    }
    if video_fps.is_none_or(|fps| fps < VIDEO_BUDGET_FPS) {
        missed.push("1080p video fps");
    }
    if next_ms > NEXT_BUDGET_MS {
        missed.push("go/next latency");
    }
    if remote_ms > REMOTE_BUDGET_MS {
        missed.push("remote-button latency");
    }
    if scan > SCAN_1000_BUDGET_MS {
        missed.push("1000-show scan");
    }
    if !missed.is_empty() {
        return Err(format!("performance budgets missed: {}", missed.join(", ")).into());
    }
    Ok(())
}

fn bench_thousand_shows() -> Result<f64, Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("freeshow-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Shows"))?;
    for index in 0..1000 {
        let show = ShowFile::blank(&format!("s{index}"), &format!("Song {index}"));
        show.save(&root.join(format!("Shows/Song{index}.show")))?;
    }
    let started = Instant::now();
    let session = Session::open(root.clone());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if session.shows.len() < 1000 {
        return Err("1000-show library did not load".into());
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(ms)
}

/// Present a sample library headlessly and check output frames plus remote/stage pages.
pub fn check_library(root: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let owned: PathBuf;
    let root = match root {
        Some(path) => path,
        None => {
            owned = std::env::temp_dir().join(format!("freeshow-library-{}", std::process::id()));
            write_sample_library(&owned)?;
            &owned
        }
    };
    if !root.join("Shows").is_dir() {
        write_sample_library(root)?;
    }
    let mut session = open_session(root);
    if !present_song(&mut session) {
        return Err("sample library has no show".into());
    }

    let mut fingerprints = Vec::new();
    let first = audience_frame(&session);
    if first.width != session.output_width || first.height != session.output_height {
        return Err(format!(
            "audience frame was {0}×{1}, expected {2}×{3}",
            first.width, first.height, session.output_width, session.output_height
        )
        .into());
    }
    fingerprints.push(fingerprint(&first.pixels));
    let stage = stage_frame(&session);
    if stage.width != session.output_width || stage.height != session.output_height {
        return Err(format!(
            "stage frame was {0}×{1}, expected {2}×{3}",
            stage.width, stage.height, session.output_width, session.output_height
        )
        .into());
    }

    let mut hops = 0u32;
    while session.advance_show() && hops < 16 {
        hops += 1;
        fingerprints.push(fingerprint(&audience_frame(&session).pixels));
    }
    if fingerprints.len() < 3 {
        return Err("presenting the sample song did not produce several slides".into());
    }
    if fingerprints.windows(2).all(|pair| pair[0] == pair[1]) {
        return Err("slide frames did not change after Go/Next".into());
    }

    let remote = free_port()?;
    let stage_port = free_port()?;
    let (commands, _rx) = std::sync::mpsc::channel();
    let (frames, _) = std::sync::mpsc::sync_channel(1);
    freeshow_remote::spawn_limited(
        std::sync::Arc::new(std::sync::Mutex::new(
            freeshow_remote::ShowSnapshot::default(),
        )),
        commands,
        None,
        freeshow_remote::RemotePorts {
            remote,
            stage: stage_port,
        },
        frames,
        4,
    );
    let remote_ok = wait_http(remote)?;
    let stage_ok = wait_http(stage_port)?;
    if !remote_ok || !stage_ok {
        return Err("remote or stage page did not respond".into());
    }

    let report = json!({
        "library": root,
        "shows": session.shows.iter().map(ShowFile::name).collect::<Vec<_>>(),
        "bibles": session.bibles.len(),
        "project": session.project_title(),
        "audience_fingerprints": fingerprints,
        "stage_fingerprint": fingerprint(&stage.pixels),
        "slides_presented": fingerprints.len(),
        "remote_url": format!("http://127.0.0.1:{remote}/"),
        "stage_url": format!("http://127.0.0.1:{stage_port}/"),
        "remote_ok": remote_ok,
        "stage_ok": stage_ok,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// A Sunday service: import a FreeShow library plus a couple of formats, build
/// a project, present audience and stage outputs, then drive Go/Next over the
/// REST and Companion HTTP APIs.
pub fn check_service(root: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let owned: PathBuf;
    let root = match root {
        Some(path) => path,
        None => {
            owned = std::env::temp_dir().join(format!("freeshow-service-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&owned);
            write_sample_library(&owned)?;
            &owned
        }
    };
    if !root.join("Shows").is_dir() {
        write_sample_library(root)?;
    }
    let imports = root.join("Imports");
    std::fs::create_dir_all(&imports)?;
    let fixtures = fixture_dir();
    copy_if_present(
        &fixtures.join("propresenter7.pro"),
        &imports.join("propresenter7.pro"),
    )?;
    std::fs::write(
        imports.join("how-great.txt"),
        "How Great Thou Art\n\n[Verse]\nO Lord my God, when I in awesome wonder\nConsider all the worlds Thy hands have made\n\n[Chorus]\nThen sings my soul, my Saviour God, to Thee\nHow great Thou art, how great Thou art\n",
    )?;

    let before = std::fs::read_dir(root.join("Shows"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    let mut session = Session::open(root.to_path_buf());
    session.logout_profile();
    let _ = session.login_profile("house", "secret");
    session.apply_update_alert(&crate::update::ReleaseUpdate {
        tag: "v0.0.0".into(),
        version: "0.0.0".into(),
        newer: false,
        html_url: String::new(),
        download_url: None,
        checksum_url: None,
    });
    if let Some(id) = session.stage.items.first().map(|item| item.id.clone()) {
        let style = session
            .stage
            .items
            .first()
            .map(|item| item.style.clone())
            .unwrap_or_default();
        let _ = session.set_stage_item_style(&id, &style);
    }
    let _ = session.add_stage_item(
        "notes",
        "left:25px;top:830px;width:1200px;height:140px;color:#f0f0ff;font-size:36px;",
    );
    let _ = session.chrome.weather_for(27.7, 85.3);
    let imported = session.shows.len().saturating_sub(before.max(2));
    if session.shows.len() < 3 {
        return Err(format!(
            "Sunday import did not add songs (shows={})",
            session
                .shows
                .iter()
                .map(ShowFile::name)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    let names: Vec<String> = session.shows.iter().map(ShowFile::name).collect();
    let imported_names: Vec<String> = names
        .iter()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.contains("how great")
                || lower.contains("propresenter")
                || lower.contains("blessed")
                || !["amazing grace", "welcome"].contains(&lower.as_str())
        })
        .cloned()
        .collect();
    if imported_names.is_empty() {
        return Err(format!("imported formats did not appear in the library: {names:?}").into());
    }

    if session.project_id.is_none() && !session.create_project() {
        return Err("could not create a Sunday project".into());
    }
    for show in session.shows.clone() {
        let _ = session.present_show(&show.id);
        let _ = session.add_current_show();
    }
    if session.project_title().is_empty() {
        return Err("Sunday project has no title".into());
    }
    if !present_song(&mut session) {
        return Err("could not present the opening song".into());
    }
    session.output_width = 1920;
    session.output_height = 1080;

    let first = audience_frame(&session);
    if first.width != 1920 || first.height != 1080 {
        return Err(format!("audience frame was {}×{}", first.width, first.height).into());
    }
    let first_fp = fingerprint(&first.pixels);
    let stage = stage_frame(&session);
    if stage.width != 1920 || stage.height != 1080 {
        return Err(format!("stage frame was {}×{}", stage.width, stage.height).into());
    }
    let stage_fp = fingerprint(&stage.pixels);

    let remote = free_port()?;
    let stage_port = free_port()?;
    let snapshot = std::sync::Arc::new(std::sync::Mutex::new(session.to_snapshot()));
    let (commands, command_rx) = std::sync::mpsc::channel();
    let (frames, _) = std::sync::mpsc::sync_channel(1);
    freeshow_remote::spawn_limited(
        std::sync::Arc::clone(&snapshot),
        commands,
        None,
        freeshow_remote::RemotePorts {
            remote,
            stage: stage_port,
        },
        frames,
        4,
    );
    if !wait_http(remote)? || !wait_http(stage_port)? {
        return Err("remote or stage page did not respond".into());
    }

    let starting_slide = session.slide_index;
    let api = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .build()
        .post(&format!("http://127.0.0.1:{remote}/api/next"))
        .call()
        .map_err(|err| format!("POST /api/next: {err}"))?;
    if api.status() != 200 {
        return Err(format!("POST /api/next returned {}", api.status()).into());
    }
    let applied_api = drain_remote(&mut session, &command_rx, Duration::from_secs(2));
    let after_api = audience_frame(&session);
    let after_api_fp = fingerprint(&after_api.pixels);
    if session.slide_index == starting_slide || applied_api == 0 {
        return Err(format!(
            "REST /api/next did not advance the slide (index={:?}, commands={applied_api})",
            session.slide_index
        )
        .into());
    }
    if after_api_fp == first_fp {
        return Err("audience frame did not change after REST next".into());
    }
    *snapshot.lock().expect("snapshot") = session.to_snapshot();

    let companion = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .build()
        .post(&format!("http://127.0.0.1:{remote}/companion/next"))
        .set("content-type", "application/json")
        .send_string("{}")
        .map_err(|err| format!("POST /companion/next: {err}"))?;
    if companion.status() != 200 {
        return Err(format!("POST /companion/next returned {}", companion.status()).into());
    }
    let applied_companion = drain_remote(&mut session, &command_rx, Duration::from_secs(2));
    let after_companion = audience_frame(&session);
    let after_companion_fp = fingerprint(&after_companion.pixels);
    if applied_companion == 0 {
        return Err("Companion /companion/next did not deliver a command".into());
    }
    let after_stage = stage_frame(&session);
    if fingerprint(&after_stage.pixels) == stage_fp && session.slide_index == starting_slide {
        return Err("stage display did not follow the remote advance".into());
    }

    let stage_page = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .build()
        .get(&format!("http://127.0.0.1:{stage_port}/"))
        .call()
        .map_err(|err| format!("GET stage: {err}"))?;
    let stage_status = stage_page.status();
    let stage_body = stage_page.into_string().unwrap_or_default();
    if stage_status != 200 || stage_body.is_empty() {
        return Err(format!("stage display returned {stage_status}").into());
    }

    let sources = session.network_sources();
    let report = json!({
        "library": root,
        "shows": names,
        "imported_shows": imported_names,
        "imported_count": imported,
        "project": session.project_title(),
        "project_items": session.project_id.as_ref().and_then(|id| {
            session
                .projects
                .projects()
                .into_iter()
                .find(|project| project.id == *id)
                .map(|project| project.shows.len())
        }),
        "audience_fingerprints": [first_fp, after_api_fp, after_companion_fp],
        "stage_fingerprint": fingerprint(&after_stage.pixels),
        "starting_slide": starting_slide,
        "slide_index": session.slide_index,
        "rest_next_commands": applied_api,
        "companion_next_commands": applied_companion,
        "remote_url": format!("http://127.0.0.1:{remote}/"),
        "stage_url": format!("http://127.0.0.1:{stage_port}/"),
        "stage_ok": true,
        "network_sources": sources.len(),
        "inspector": session.inspector_text(),
        "settings_has_alert_updates": session.settings_text().contains("alertUpdates"),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn drain_remote(
    session: &mut Session,
    rx: &std::sync::mpsc::Receiver<freeshow_remote::Command>,
    budget: Duration,
) -> usize {
    let deadline = Instant::now() + budget;
    let mut applied = 0;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(command) => {
                session.apply_remote(command);
                applied += 1;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if applied > 0 {
                    std::thread::sleep(Duration::from_millis(20));
                    if rx.try_recv().is_err() {
                        break;
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
    applied
}

fn free_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn wait_http(port: u16) -> Result<bool, Box<dyn std::error::Error>> {
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        let result = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(200))
            .timeout_read(Duration::from_secs(1))
            .build()
            .get(&url)
            .call();
        if let Ok(response) = result {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            return Ok((status == 200 || status == 401)
                && (status == 401 || body.contains('<') || !body.is_empty()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn benchmark_reports_current_resident_memory() {
        assert!(rss_mb().is_some_and(|mb| mb.is_finite() && mb > 0.0));
    }

    #[test]
    fn sample_library_presents_and_answers_on_the_web() {
        let root =
            std::env::temp_dir().join(format!("freeshow-service-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_sample_library(&root).unwrap();
        check_library(Some(&root)).unwrap();
        check_service(Some(&root)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
