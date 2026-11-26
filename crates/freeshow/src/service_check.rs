// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use freeshow_data::{EXPORTS_DIR, Library, ShowFile};
use freeshow_media::VideoPlayer;
use freeshow_output::fingerprint;
use serde_json::{Value, json};

use crate::live::{audience_frame, cleared_audience_frame, stage_frame};
use crate::session::{PendingPlay, Session};

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
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // Query this process and read the output only if libproc filled the structure.
    let read = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(info.pti_resident_size as f64 / (1024.0 * 1024.0))
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

const HANDOFF_WORDS: &str = "Doors open at 10:30";
const HANDOFF_NOTE: &str = "Bring up the lectern microphone";
const HANDOFF_CLIP: &[u8] =
    include_bytes!("../../freeshow-data/tests/fixtures/service-handoff.mp4");
const HANDOFF_STILLS: [(&str, &str, [u8; 4]); 2] = [
    ("Media/red/Banner.png", "Red banner", [220, 40, 30, 255]),
    ("Media/blue/Banner.png", "Blue banner", [30, 80, 220, 255]),
];

fn export_handoff_fixture(
    root: &Path,
    imports: &Path,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    const STARTER: &str = "freeshow-rust-starter-announcement";
    const TEMPLATE: &str = "Rehearsal Announcement";
    let mut sender = Session::open(root.to_path_buf());
    if !sender.create_project() {
        return Err("handoff could not create the sender project".into());
    }
    sender.switch_tab("templates");
    if !sender.browse_template(STARTER) {
        return Err("handoff could not open the announcement starter".into());
    }
    sender.library_action("template-new-show", STARTER)?;
    if !sender.save_lyrics(HANDOFF_WORDS) || !sender.save_notes(HANDOFF_NOTE) {
        return Err("handoff could not save the sender's text and stage note".into());
    }
    let show_name = sender.show_name();
    sender.library_action("slide-to-template", TEMPLATE)?;
    sender.switch_tab("templates");
    let template_id = sender
        .template_catalog
        .id_named(TEMPLATE)
        .ok_or("handoff template was not saved")?;
    if !sender.browse_template(&template_id) {
        return Err("handoff could not select the saved template".into());
    }
    sender.library_action("template-export", &template_id)?;
    std::fs::copy(
        root.join(EXPORTS_DIR)
            .join(format!("{TEMPLATE} - Template.project")),
        imports.join("Template.project"),
    )?;

    let project_id = sender
        .project_id
        .as_deref()
        .ok_or("handoff project was not selected")?;
    let mut project = sender
        .projects
        .project_body(project_id)
        .ok_or("handoff project was not saved")?;
    let cues = project["shows"]
        .as_array_mut()
        .ok_or("handoff project had no cue list")?;
    for (relative, label, color) in HANDOFF_STILLS {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().ok_or("handoff still path had no parent")?)?;
        std::fs::write(
            path,
            freeshow_output::Frame {
                width: 1,
                height: 1,
                pixels: color.to_vec(),
            }
            .encode_png()?,
        )?;
        cues.push(json!({"id":relative, "type":"image", "name":label}));
    }
    std::fs::write(root.join("Media/Closing.mp4"), HANDOFF_CLIP)?;
    cues.push(json!({"id":"Media/Closing.mp4", "type":"video", "name":"Closing clip"}));
    if !sender.projects.put_project(project_id, project) {
        return Err("handoff could not add media cues to the sender project".into());
    }
    sender.projects.save(&Library::open(root).projects_path())?;
    let (package, warnings) = sender.export_current_project_package()?;
    if !warnings.is_empty() {
        return Err(format!(
            "handoff project export reported dependencies: {}",
            warnings.join(", ")
        )
        .into());
    }
    std::fs::copy(package, imports.join("Service.project"))?;
    Ok((show_name, template_id))
}

/// Rehearses a package handoff with disposable libraries and embedded local media.
fn check_portable_handoff() -> Result<Value, Box<dyn std::error::Error>> {
    let sender_dir = tempfile::tempdir()?;
    let receiver_dir = tempfile::tempdir()?;
    let imports = Library::open(receiver_dir.path()).imports_dir();
    std::fs::create_dir_all(&imports)?;
    let (show_name, template_id) = export_handoff_fixture(sender_dir.path(), &imports)?;
    sender_dir.close()?;

    let mut receiver = Session::open(receiver_dir.path().to_path_buf());
    if !receiver.warnings.is_empty() {
        return Err(format!("handoff import reported: {}", receiver.warnings.join(", ")).into());
    }
    receiver.library_action("open-project", "0")?;
    let cue_order = receiver.project_labels();
    if cue_order
        != [
            show_name.as_str(),
            "Red banner",
            "Blue banner",
            "Closing clip",
        ]
    {
        return Err(format!("handoff cue order changed: {cue_order:?}").into());
    }
    receiver.output_width = 320;
    receiver.output_height = 180;
    let imported_show = receiver
        .shows
        .iter()
        .find(|show| show.name() == show_name)
        .map(|show| show.id.clone())
        .ok_or("handoff announcement was not imported")?;
    if !receiver.present_show(&imported_show) {
        return Err("handoff could not take the imported announcement".into());
    }
    let _ = audience_frame(&receiver);
    let taken = receiver.to_snapshot();
    if taken
        .slide_lines()
        .first()
        .is_none_or(|line| line != HANDOFF_WORDS)
        || taken.notes() != HANDOFF_NOTE
    {
        return Err("handoff did not keep the live announcement or stage note".into());
    }
    for (_, label, color) in HANDOFF_STILLS {
        if !receiver.advance_show() {
            return Err(format!("handoff could not take {label}").into());
        }
        let image = audience_frame(&receiver);
        let pixel = image.pixel(image.width / 2, image.height / 2);
        if [pixel.r, pixel.g, pixel.b, pixel.a] != color {
            return Err(format!("handoff rendered the wrong picture for {label}").into());
        }
    }
    if !receiver.advance_show() {
        return Err("handoff could not take the imported closing clip".into());
    }
    let [Some(PendingPlay::Video(clip)), _] = receiver.take_plays() else {
        return Err("handoff did not queue the imported video".into());
    };
    if !clip.starts_with(Library::open(receiver_dir.path()).media_dir())
        || std::fs::read(&clip)? != HANDOFF_CLIP
    {
        return Err("handoff video did not resolve inside the receiving library".into());
    }
    let playback = freeshow_media::probe_routed_video(&clip)?;
    let first_frame = [playback.frame.width, playback.frame.height];
    receiver.replace_underlay(Some(playback.frame));
    let picture = fingerprint(&audience_frame(&receiver).pixels);
    if !receiver.clear_all_layers()
        || fingerprint(&cleared_audience_frame(&receiver).pixels) == picture
    {
        return Err("handoff Clear did not remove the closing clip".into());
    }

    let replacement = receiver_dir.path().join("Replacement.mp4");
    std::fs::rename(&clip, &replacement)?;
    let missing = receiver
        .missing_media()
        .into_iter()
        .find(|entry| Path::new(&entry.reference) == clip)
        .ok_or("handoff did not report the missing closing clip")?;
    receiver.relink_missing_media(
        &missing.key,
        replacement
            .to_str()
            .ok_or("handoff replacement path was not Unicode")?,
    )?;
    let new_path = receiver
        .projects
        .project_body(
            receiver
                .project_id
                .as_deref()
                .ok_or("handoff receiver project went missing")?,
        )
        .and_then(|project| project["shows"][3]["id"].as_str().map(PathBuf::from))
        .ok_or("handoff relink did not update the closing cue")?;
    if !receiver.missing_media().is_empty()
        || !new_path.starts_with(Library::open(receiver_dir.path()).media_dir())
    {
        return Err("handoff relink did not resolve the missing clip locally".into());
    }
    let relinked = freeshow_media::probe_routed_video(&new_path)?;

    receiver.switch_tab("templates");
    if !receiver.browse_template(&template_id) {
        return Err("handoff standalone template did not appear in the gallery".into());
    }
    receiver.library_action("template-new-show", &template_id)?;
    if receiver.lyrics() != HANDOFF_WORDS
        || !receiver.notes().is_empty()
        || !receiver.save_lyrics("Edited after handoff")
    {
        return Err(
            "handoff template did not reopen as editable text without private notes".into(),
        );
    }
    let edited_name = receiver.show_name();
    let reopened = Session::open(receiver_dir.path().to_path_buf());
    if !reopened.shows.iter().any(|show| {
        show.name() == edited_name
            && show.item_text("s1", 0).as_deref() == Some("Edited after handoff")
    }) {
        return Err("handoff edits did not survive reopening the receiving library".into());
    }

    Ok(json!({
        "cue_order": cue_order,
        "project_stage_note_preserved": true,
        "distinct_same_named_pictures": true,
        "video_first_frame_pixels": first_frame,
        "video_audio_peak": playback.audio_peak,
        "relinked_video_audio_peak": relinked.audio_peak,
        "video_relinked_and_decoded": true,
        "audience_cleared": true,
        "template_edited_and_reopened_without_private_notes": true,
    }))
}

pub fn check_handoff() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&check_portable_handoff()?)?
    );
    Ok(())
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
        "checked_at": freeshow_core::local_timestamp(),
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
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) && idle_rss_mb.is_none() {
        missed.push("idle RSS unavailable");
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

    let (commands, _rx) = std::sync::mpsc::channel();
    let (frames, _) = std::sync::mpsc::sync_channel(1);
    let listeners = freeshow_remote::spawn_limited(
        std::sync::Arc::new(std::sync::Mutex::new(
            freeshow_remote::ShowSnapshot::default(),
        )),
        commands,
        None,
        freeshow_remote::RemotePorts {
            remote: 0,
            stage: 0,
        },
        frames,
        4,
    );
    let freeshow_remote::RemotePorts {
        remote,
        stage: stage_port,
    } = started_ports(listeners)?;
    let remote_ok = wait_http(remote)?;
    let stage_ok = wait_http(stage_port)?;
    if !remote_ok || !stage_ok {
        return Err("remote or stage page did not respond".into());
    }

    let report = json!({
        "checked_at": freeshow_core::local_timestamp(),
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
    let handoff = check_portable_handoff()?;
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

    let snapshot = std::sync::Arc::new(std::sync::Mutex::new(session.to_snapshot()));
    let (commands, command_rx) = std::sync::mpsc::channel();
    let (frames, _) = std::sync::mpsc::sync_channel(1);
    const PASSWORD: &str = "service-check";
    let listeners = freeshow_remote::spawn_limited(
        std::sync::Arc::clone(&snapshot),
        commands,
        Some(PASSWORD.into()),
        freeshow_remote::RemotePorts {
            remote: 0,
            stage: 0,
        },
        frames,
        4,
    );
    let freeshow_remote::RemotePorts {
        remote,
        stage: stage_port,
    } = started_ports(listeners)?;
    if !wait_http(remote)? || !wait_http(stage_port)? {
        return Err("remote or stage page did not respond".into());
    }

    let remote_state = format!("http://127.0.0.1:{remote}/state");
    for supplied in ["", "wrong-password"] {
        let unauthorized = ureq::get(&remote_state)
            .set("x-freeshow-password", supplied)
            .call();
        if !matches!(unauthorized, Err(ureq::Error::Status(401, _))) {
            return Err("protected remote state did not reject an invalid password".into());
        }
    }
    let before = remote_view(&remote_state, PASSWORD)?;
    if before["slide_index"] != json!(session.slide_index) {
        return Err("authenticated remote did not report the starting slide".into());
    }

    let starting_slide = session.slide_index;
    let api = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .build()
        .post(&format!("http://127.0.0.1:{remote}/api/next"))
        .set("x-freeshow-password", PASSWORD)
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
        .set("x-freeshow-password", PASSWORD)
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

    *snapshot.lock().expect("snapshot") = session.to_snapshot();
    let returned = remote_view(&remote_state, PASSWORD)?;
    let staged = remote_view(&format!("http://127.0.0.1:{stage_port}/state"), PASSWORD)?;
    if returned["slide_index"] == before["slide_index"]
        || returned["slide_index"] != json!(session.slide_index)
        || returned["lines"] != staged["lines"]
    {
        return Err("reconnected remote and stage did not report the current cue".into());
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
        "checked_at": freeshow_core::local_timestamp(),
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
        "remote_reconnected": true,
        "remote_bad_password_rejected": true,
        "rest_next_commands": applied_api,
        "companion_next_commands": applied_companion,
        "remote_url": format!("http://127.0.0.1:{remote}/"),
        "stage_url": format!("http://127.0.0.1:{stage_port}/"),
        "stage_ok": true,
        "portable_handoff": handoff,
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

fn started_ports(
    events: std::sync::mpsc::Receiver<(freeshow_remote::WebPage, freeshow_remote::ListenerState)>,
) -> Result<freeshow_remote::RemotePorts, Box<dyn std::error::Error>> {
    use freeshow_remote::{ListenerState, RemotePorts, WebPage};
    let mut ports = RemotePorts {
        remote: 0,
        stage: 0,
    };
    for _ in 0..2 {
        let (page, state) = events.recv_timeout(Duration::from_secs(4))?;
        let ListenerState::Listening(port) = state else {
            return Err(format!("{page:?} listener reported {state:?}").into());
        };
        match page {
            WebPage::Remote => ports.remote = port,
            WebPage::Stage => ports.stage = port,
        }
    }
    if ports.remote == 0 || ports.stage == 0 {
        return Err("did not receive both listener addresses".into());
    }
    Ok(ports)
}

fn remote_view(url: &str, password: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .build()
        .get(url)
        .set("x-freeshow-password", password)
        .call()?;
    Ok(serde_json::from_reader(response.into_reader())?)
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
