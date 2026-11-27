// SPDX-License-Identifier: GPL-3.0-only

use std::cell::RefCell;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
    mpsc,
};
use std::time::Duration;

use freeshow_data::Library;
use slint::ComponentHandle;
use tempfile::NamedTempFile;
use url::Url;

use crate::{ControlWindow, Session, drawer_rows, selected};

const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TRANSFERRING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const SOURCE_DIR: &str = ".freeshow-sources";
const STAGING_LOCK: &str = ".download.lock";
const PENDING_PREFIX: &str = ".pending-";
const PENDING_SUFFIX: &str = ".part";
const PENDING_RANDOM_LENGTH: usize = 6;

#[derive(Clone, Copy, Default)]
struct Progress {
    bytes: u64,
    total: Option<u64>,
    checking: bool,
}

#[derive(Default)]
struct Control {
    phase: AtomicU8,
    progress: Mutex<Progress>,
}

impl Control {
    fn cancel(&self) -> bool {
        self.phase
            .compare_exchange(TRANSFERRING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn check(&self) -> Result<(), String> {
        if self.phase.load(Ordering::Acquire) == CANCELLED {
            Err("Download cancelled.".into())
        } else {
            Ok(())
        }
    }

    fn commit(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(
                TRANSFERRING,
                COMMITTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| "Download cancelled.".into())
    }

    fn update(&self, progress: Progress) {
        *self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = progress;
    }

    fn display(&self) -> String {
        match self.phase.load(Ordering::Acquire) {
            CANCELLED => return "Cancelling download…".into(),
            COMMITTING => return "Finishing the download…".into(),
            _ => {}
        }
        let progress = *self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if progress.checking {
            return "Checking that the video opens offline…".into();
        }
        if let Some(total) = progress.total.filter(|&total| total > 0) {
            format!(
                "Downloading {}% · {}",
                (progress.bytes * 100 / total).min(100),
                bytes_text(progress.bytes)
            )
        } else if progress.bytes > 0 {
            format!("Downloading · {}", bytes_text(progress.bytes))
        } else {
            "Connecting to video source…".into()
        }
    }
}

fn bytes_text(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn direct_url(text: &str) -> Result<Url, String> {
    let mut url =
        Url::parse(text.trim()).map_err(|_| "Enter a direct HTTP or HTTPS video link.")?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("Enter a direct HTTP or HTTPS video link.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Use a link without a username or password in the address.".into());
    }
    url.set_fragment(None);
    Ok(url)
}

fn fetch(mut url: Url, control: &Control) -> Result<(ureq::Response, Url), String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build();
    for _ in 0..=5 {
        control.check()?;
        let response = agent
            .get(url.as_str())
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => format!("Video server returned HTTP {status}."),
                ureq::Error::Transport(_) => {
                    "Could not read from the video server. Check the connection and retry.".into()
                }
            })?;
        control.check()?;
        match response.status() {
            200 => return Ok((response, url)),
            301 | 302 | 303 | 307 | 308 => {
                let next = response
                    .header("Location")
                    .and_then(|location| url.join(location).ok())
                    .ok_or("The video server returned an invalid redirect.")?;
                url = direct_url(next.as_str())?;
            }
            status => {
                return Err(format!(
                    "Video server returned HTTP {status}. A complete file is required."
                ));
            }
        }
    }
    Err("The video server redirected too many times.".into())
}

fn extension(head: &[u8], mime: &str, url: &Url) -> Result<&'static str, String> {
    let suffix = url
        .path()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if head
        .get(4..8)
        .is_some_and(|tag| matches!(tag, b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free"))
    {
        return Ok(if suffix == "mov" || mime == "video/quicktime" {
            "mov"
        } else {
            "mp4"
        });
    }
    if head.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Ok(if head.windows(4).any(|word| word == b"webm") {
            "webm"
        } else {
            "mkv"
        });
    }
    Err("The download is not an MP4, MOV, WebM, or MKV video file.".into())
}

fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .take(48)
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, ' ' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches([' ', '_', '-']);
    // A stable prefix avoids Windows device names and hidden filenames.
    if cleaned.is_empty() {
        "Downloaded video".into()
    } else {
        format!("Video {cleaned}")
    }
}

fn download(root: &Path, url: Url, title: &str, control: &Control) -> Result<PathBuf, String> {
    let media = Library::open(root).media_dir();
    let staging = media.join(SOURCE_DIR);
    fs::create_dir_all(&staging).map_err(disk_error)?;
    let _guard = lock_staging(&staging)
        .map_err(disk_error)?
        .ok_or("The library is busy with another download. Try again in a moment.")?;
    clear_pending(&staging).map_err(disk_error)?;
    let source_origin = url.origin().ascii_serialization();
    let (response, final_url) = fetch(url, control)?;
    let mime = response
        .header("Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("text/")
        || mime.starts_with("image/")
        || mime.starts_with("audio/")
        || matches!(
            mime.as_str(),
            "application/json" | "application/vnd.apple.mpegurl" | "application/dash+xml"
        )
    {
        return Err("The link returned a page, playlist, or other file. Use a direct downloadable video link.".into());
    }
    if response
        .header("Content-Encoding")
        .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
    {
        return Err(
            "The server returned an encoded response. Use a direct video file link.".into(),
        );
    }
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    if total.is_some_and(|size| size == 0 || size > MAX_BYTES) {
        return Err("The video is empty or exceeds the 2 GiB download limit.".into());
    }
    let mut temporary = pending_file(&staging).map_err(disk_error)?;
    let mut body = response.into_reader().take(MAX_BYTES + 1);
    let mut buffer = [0; 64 * 1024];
    let mut head = Vec::new();
    let mut bytes = 0;
    loop {
        control.check()?;
        let read = body.read(&mut buffer);
        control.check()?;
        let count = read.map_err(|_| "The video transfer was interrupted. Retry the download.")?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_BYTES {
            return Err("The video exceeds the 2 GiB download limit.".into());
        }
        if head.len() < 512 {
            head.extend_from_slice(&buffer[..count.min(512 - head.len())]);
        }
        temporary.write_all(&buffer[..count]).map_err(disk_error)?;
        control.update(Progress {
            bytes,
            total,
            checking: false,
        });
    }
    if bytes == 0 || total.is_some_and(|total| total != bytes) {
        return Err("The video transfer was empty or incomplete. Retry the download.".into());
    }
    let ext = extension(&head, &mime, &final_url)?;
    temporary.as_file().sync_all().map_err(disk_error)?;
    control.update(Progress {
        bytes,
        total,
        checking: true,
    });
    control.check()?;
    let local_path = fs::canonicalize(temporary.path()).map_err(disk_error)?;
    if freeshow_media::video_thumbnail(&local_path, 96, 54).is_none() {
        control.check()?;
        return Err("FreeShow could not decode a video frame from this file.".into());
    }
    control.commit()?;
    let name = safe_name(title);
    publish_download(temporary, &media, &staging, &name, ext, &source_origin)
}

struct StagingLock(File);

impl Drop for StagingLock {
    fn drop(&mut self) {
        // A handle inherited by a child can keep the lock alive after this file closes.
        let _ = self.0.unlock();
    }
}

fn lock_staging(staging: &Path) -> io::Result<Option<StagingLock>> {
    if !fs::symlink_metadata(staging)?.is_dir() {
        return Err(io::Error::other(
            "The download staging path must be a folder in the media library.",
        ));
    }
    // Keep the lock file in place so different processes always lock the same file.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(staging.join(STAGING_LOCK))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(StagingLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn pending_file(staging: &Path) -> io::Result<NamedTempFile> {
    tempfile::Builder::new()
        .prefix(PENDING_PREFIX)
        .suffix(PENDING_SUFFIX)
        .rand_bytes(PENDING_RANDOM_LENGTH)
        .tempfile_in(staging)
}

fn clear_pending(staging: &Path) -> io::Result<()> {
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // The first downloader used tempfile's default name for unfinished credits.
        let pending_id = name
            .strip_prefix(PENDING_PREFIX)
            .and_then(|name| name.strip_suffix(PENDING_SUFFIX))
            .or_else(|| name.strip_prefix(".tmp"));
        let is_pending = pending_id.is_some_and(|id| {
            id.len() == PENDING_RANDOM_LENGTH && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
        if is_pending && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn recover_pending(root: &Path) -> io::Result<()> {
    let staging = Library::open(root).media_dir().join(SOURCE_DIR);
    match lock_staging(&staging) {
        Ok(Some(_guard)) => clear_pending(&staging),
        Ok(None) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn disk_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::StorageFull {
        "The library disk is full. Free up space and retry the download.".into()
    } else {
        format!("Could not save the video. {error}")
    }
}

fn publish_download(
    mut video: NamedTempFile,
    media: &Path,
    receipts: &Path,
    stem: &str,
    ext: &str,
    origin: &str,
) -> Result<PathBuf, String> {
    use std::io::ErrorKind::AlreadyExists;
    for number in 1..10_000 {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!(" {number}")
        };
        let name = format!("{stem}{suffix}.{ext}");
        let target = media.join(&name);
        if target.exists() {
            continue;
        }
        let metadata = receipts.join(format!("{name}.json"));
        let mut receipt = pending_file(receipts).map_err(disk_error)?;
        let credit = serde_json::json!({"sourceOrigin": origin}).to_string();
        receipt.write_all(credit.as_bytes()).map_err(disk_error)?;
        receipt.as_file().sync_all().map_err(disk_error)?;
        match receipt.persist_noclobber(&metadata) {
            Ok(_) => {}
            Err(error) if error.error.kind() == AlreadyExists => continue,
            Err(error) => return Err(disk_error(error.error)),
        }
        match video.persist_noclobber(&target) {
            Ok(_) => return Ok(target),
            Err(error) => {
                if let Err(cleanup) = fs::remove_file(&metadata) {
                    tracing::warn!(%cleanup, "could not remove the incomplete video credit");
                }
                if error.error.kind() != AlreadyExists {
                    return Err(disk_error(error.error));
                }
                video = error.file;
            }
        }
    }
    Err("Too many videos use this name. Enter a different name and retry.".into())
}

struct DownloadJob {
    root: PathBuf,
    control: Arc<Control>,
    result: mpsc::Receiver<Result<PathBuf, String>>,
}

pub(crate) fn wire(ui: &ControlWindow, session: Rc<RefCell<Session>>) -> slint::Timer {
    if let Some(root) = session.borrow().root.clone() {
        let _ = std::thread::Builder::new()
            .name("freeshow-video-recovery".into())
            .spawn(move || {
                if let Err(error) = recover_pending(&root) {
                    tracing::warn!(%error, "could not clean up interrupted video downloads");
                }
            });
    }
    let job = Rc::<RefCell<Option<DownloadJob>>>::default();
    let start_ui = ui.as_weak();
    let start_session = Rc::clone(&session);
    let start_job = Rc::clone(&job);
    ui.on_start_video_download(move |url, title| {
        let Some(ui) = start_ui.upgrade() else { return };
        if start_job.borrow().is_some() {
            return;
        }
        let root = start_session.borrow().root.clone();
        let result = direct_url(url.as_str()).and_then(|url| {
            let root = root.ok_or("Choose a library folder first.")?;
            let work_root = root.clone();
            let control = Arc::<Control>::default();
            let work_control = control.clone();
            let (send, result) = mpsc::channel();
            let title = title.to_string();
            std::thread::Builder::new()
                .name("freeshow-video-download".into())
                .spawn(move || {
                    let outcome = download(&work_root, url, &title, &work_control);
                    let _ = send.send(if work_control.check().is_err() {
                        Err("Download cancelled.".into())
                    } else {
                        outcome
                    });
                })
                .map_err(|error| format!("Could not start the download. {error}"))?;
            Ok(DownloadJob {
                root,
                control,
                result,
            })
        });
        match result {
            Ok(next) => {
                *start_job.borrow_mut() = Some(next);
                ui.set_video_download_active(true);
                ui.set_video_download_status("Connecting to video source…".into());
            }
            Err(error) => ui.set_video_download_status(error.into()),
        }
    });

    let cancel_job = Rc::clone(&job);
    let cancel_ui = ui.as_weak();
    ui.on_cancel_video_download(move || {
        if let Some(current) = cancel_job.borrow().as_ref()
            && let Some(ui) = cancel_ui.upgrade()
        {
            let message = if current.control.cancel() {
                "Cancelling download…"
            } else if current.control.check().is_err() {
                return;
            } else {
                "Finishing the download…"
            };
            ui.set_video_download_status(message.into());
        }
    });

    let ui = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(150),
        move || {
            let mut active = job.borrow_mut();
            let Some(current) = active.as_ref() else {
                return;
            };
            let Some(ui) = ui.upgrade() else { return };
            let outcome = match current.result.try_recv() {
                Ok(outcome) => outcome,
                Err(mpsc::TryRecvError::Empty) => {
                    ui.set_video_download_status(current.control.display().into());
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("The download stopped unexpectedly. Try again.".into())
                }
            };
            let completed = active.take().expect("active download");
            ui.set_video_download_active(false);
            match outcome {
                Ok(path) => {
                    let mut session = session.borrow_mut();
                    session.refresh_downloaded_media(&completed.root);
                    if session.tab == "media" {
                        ui.set_drawer_rows(drawer_rows(&session));
                        ui.set_selected_drawer(selected(
                            session.visible_drawer_index(session.media_index),
                        ));
                    }
                    ui.set_video_download_url("".into());
                    ui.set_video_download_name("".into());
                    ui.set_video_download_status(
                        format!(
                            "Saved {} for offline playback.",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        )
                        .into(),
                    );
                }
                Err(error) => ui.set_video_download_status(error.into()),
            }
        },
    );
    timer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn accept(listener: &TcpListener) -> (TcpStream, String) {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = BufReader::new(&mut stream);
        let mut first_line = String::new();
        request.read_line(&mut first_line).unwrap();
        loop {
            let mut line = String::new();
            if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        (stream, first_line)
    }

    fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n"
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }

    #[test]
    #[ignore = "requires a small disposable volume set in FREESHOW_TEST_DOWNLOAD_VOLUME"]
    fn disk_full_download_cleans_staging_and_can_retry() {
        const MIB: usize = 1024 * 1024;
        let volume = std::env::var_os("FREESHOW_TEST_DOWNLOAD_VOLUME")
            .expect("set a disposable volume path");
        let root = tempfile::tempdir_in(volume).unwrap();
        let library = Library::open(root.path());
        let media = library.media_dir();
        let receipts = media.join(SOURCE_DIR);
        fs::create_dir_all(&receipts).unwrap();
        let existing = media.join("Keep.mp4");
        fs::write(&existing, b"existing media").unwrap();

        let mut ballast = NamedTempFile::new_in(root.path()).unwrap();
        let block = vec![0xa5; MIB];
        let mut filled = false;
        for _ in 0..48 {
            match ballast.write_all(&block) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::StorageFull => {
                    filled = true;
                    break;
                }
                Err(error) => panic!("could not fill the test volume: {error}"),
            }
        }
        assert!(
            filled,
            "test needs a disposable volume with less than 48 MiB free"
        );
        let used = ballast.as_file().metadata().unwrap().len();
        // Leave enough room to open staging and write part of the file.
        ballast.as_file().set_len(used - MIB as u64).unwrap();

        let fixture = include_bytes!("../../freeshow-data/tests/fixtures/service-handoff.mp4");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = direct_url(&format!(
            "http://{}/clip.mp4",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        // A valid top-level MP4 padding box keeps the fixture small in the repository.
        let mut body = fixture.to_vec();
        body.extend_from_slice(&(4 * MIB as u32).to_be_bytes());
        body.extend_from_slice(b"free");
        body.resize(body.len() + 4 * MIB - 8, 0);
        let size = body.len();
        let server = thread::spawn(move || {
            let headers = format!("Content-Type: video/mp4\r\nContent-Length: {size}\r\n");
            let (mut failed, _) = accept(&listener);
            respond(&mut failed, "200 OK", &headers, &[]);
            let _ = failed.write_all(&body);
            drop(failed);
            let (mut retry, _) = accept(&listener);
            respond(&mut retry, "200 OK", &headers, &body);
        });

        let progress = Control::default();
        let error = download(root.path(), url.clone(), "Capacity check", &progress).unwrap_err();
        assert_eq!(
            error,
            "The library disk is full. Free up space and retry the download."
        );
        assert!(progress.progress.lock().unwrap().bytes > 0);
        assert_eq!(fs::read_dir(&receipts).unwrap().count(), 1); // Only the lock remains.
        assert_eq!(library.list_media_entries().unwrap().len(), 1);
        assert_eq!(fs::read(&existing).unwrap(), b"existing media");

        drop(ballast);
        let saved = download(root.path(), url, "Capacity check", &Control::default()).unwrap();
        assert_eq!(fs::metadata(&saved).unwrap().len(), size as u64);
        assert!(freeshow_media::video_thumbnail(&saved, 96, 54).is_some());
        assert_eq!(library.list_media_entries().unwrap().len(), 2);
        server.join().unwrap();
    }

    #[test]
    fn retries_failed_download_and_saves_a_playable_copy_with_origin_credit() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("fixture.mp4");
        if freeshow_media::write_benchmark_video(&fixture).is_err() {
            // As in the media integration tests, the runtime encoder is optional.
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let video = fs::read(&fixture).unwrap();
        let body = video.clone();
        let server = thread::spawn(move || {
            let (mut failed, _) = accept(&listener);
            respond(
                &mut failed,
                "200 OK",
                "Content-Type: video/mp4\r\nContent-Length: 100\r\n",
                b"incomplete",
            );
            drop(failed);
            let (mut redirect, _) = accept(&listener);
            respond(
                &mut redirect,
                "302 Found",
                "Location: /clip.mp4?private=signed\r\nContent-Length: 0\r\n",
                b"",
            );
            drop(redirect);
            let (mut success, request) = accept(&listener);
            assert!(request.starts_with("GET /clip.mp4?private=signed "));
            let headers = format!(
                "Content-Type: video/mp4\r\nContent-Length: {}\r\n",
                body.len()
            );
            respond(&mut success, "200 OK", &headers, &body);
        });
        let library = Library::open(root.path());
        let media = library.media_dir();
        fs::create_dir_all(&media).unwrap();
        let existing = media.join("Video Service clip.mp4");
        fs::write(&existing, b"existing").unwrap();
        let url = direct_url(&format!("{origin}/download?key=initial-secret")).unwrap();

        assert!(
            download(
                root.path(),
                url.clone(),
                "Service clip",
                &Control::default()
            )
            .is_err()
        );
        assert_eq!(library.list_media_entries().unwrap().len(), 1);
        let receipts = media.join(SOURCE_DIR);
        assert_eq!(fs::read_dir(&receipts).unwrap().count(), 1); // The lock remains.

        let saved = download(root.path(), url, "Service clip", &Control::default()).unwrap();
        server.join().unwrap();
        assert_eq!(saved, media.join("Video Service clip 2.mp4"));
        assert_eq!(fs::read(&saved).unwrap(), video);
        assert_eq!(fs::read(&existing).unwrap(), b"existing");
        assert!(freeshow_media::video_thumbnail(&saved, 96, 54).is_some());
        assert!(
            library
                .list_media_entries()
                .unwrap()
                .iter()
                .any(|entry| entry.path == saved)
        );
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(receipts.join("Video Service clip 2.mp4.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt, serde_json::json!({"sourceOrigin": origin}));
        assert_eq!(fs::read_dir(&receipts).unwrap().count(), 2);
    }

    #[test]
    fn cancelled_transfer_discards_partial_video() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = direct_url(&format!(
            "http://{}/clip.mp4",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let (started, wait) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = accept(&listener);
            respond(
                &mut stream,
                "200 OK",
                "Content-Type: video/mp4\r\nContent-Length: 131072\r\n",
                &[0; 65536],
            );
            started.send(()).unwrap();
            paused.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let control = Control::default();
        thread::scope(|scope| {
            let transfer = scope.spawn(|| download(root.path(), url, "Cancelled clip", &control));
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while control.progress.lock().unwrap().bytes == 0 {
                assert!(std::time::Instant::now() < until, "no download progress");
                thread::sleep(Duration::from_millis(5));
            }
            assert!(control.cancel());
            resume.send(()).unwrap();
            assert_eq!(transfer.join().unwrap(), Err("Download cancelled.".into()));
        });
        server.join().unwrap();
        let library = Library::open(root.path());
        assert!(library.list_media_entries().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(library.media_dir().join(SOURCE_DIR))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn startup_recovers_abandoned_staging_without_disturbing_an_active_download() {
        let root = tempfile::tempdir().unwrap();
        let library = Library::open(root.path());
        let media = library.media_dir();
        let staging = media.join(SOURCE_DIR);
        recover_pending(root.path()).unwrap();
        assert!(!staging.exists());

        fs::create_dir_all(&staging).unwrap();
        let guard = lock_staging(&staging).unwrap().unwrap();
        #[cfg(unix)]
        let _inherited_handle = guard.0.try_clone().unwrap();
        let partial = staging.join(".pending-Ab0cd1.part");
        let legacy_credit = staging.join(".tmpa0B1c2");
        let credit = staging.join("Video Service.mp4.json");
        let unrelated = staging.join(".pending-not-a-download.part");
        let completed = media.join("Video Service.mp4");
        fs::write(&partial, b"partial video").unwrap();
        fs::write(&legacy_credit, b"partial credit").unwrap();
        fs::write(&credit, b"published credit").unwrap();
        fs::write(&unrelated, b"unrelated").unwrap();
        fs::write(&completed, b"published video").unwrap();

        recover_pending(root.path()).unwrap();
        assert!(partial.is_file());
        assert!(legacy_credit.is_file());
        assert!(lock_staging(&staging).unwrap().is_none());
        drop(guard);

        recover_pending(root.path()).unwrap();
        assert!(!partial.exists());
        assert!(!legacy_credit.exists());
        assert_eq!(fs::read(&credit).unwrap(), b"published credit");
        assert_eq!(fs::read(&completed).unwrap(), b"published video");
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated");
        recover_pending(root.path()).unwrap();
    }
}
