// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use gstreamer::prelude::*;
use gstreamer_app::AppSrc;

/// Writes the audience picture to a file or an RTMP destination.
/// The show keeps running when an encoder or a stream URL is unavailable.
pub trait OutputCapture {
    fn name(&self) -> &'static str;
    fn toggle_file(&self, path: &Path, width: u32, height: u32) -> bool;
    fn start_rtmp(&self, url: &str, width: u32, height: u32) -> bool;
    fn submit(&self, pixels: &[u8], width: u32, height: u32);
}

pub struct ServiceCapture;

impl OutputCapture for ServiceCapture {
    fn name(&self) -> &'static str {
        "record"
    }

    fn toggle_file(&self, path: &Path, width: u32, height: u32) -> bool {
        if file_active() {
            stop_slot(&FILE);
            false
        } else {
            start_slot(&FILE, file_pipeline(path, width, height), width, height)
        }
    }

    fn start_rtmp(&self, url: &str, width: u32, height: u32) -> bool {
        if url.trim().is_empty() {
            return false;
        }
        stop_slot(&RTMP);
        start_slot(&RTMP, rtmp_pipeline(url, width, height), width, height)
    }

    fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        send(&FILE, pixels, width, height);
        send(&RTMP, pixels, width, height);
    }
}

enum Job {
    Frame(Vec<u8>),
    Stop,
}

struct Running {
    width: u32,
    height: u32,
    jobs: SyncSender<Job>,
    thread: JoinHandle<()>,
}

struct Slot {
    inner: Mutex<Option<Running>>,
}

impl Slot {
    fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

fn file_slot() -> &'static Slot {
    static SLOT: OnceLock<Slot> = OnceLock::new();
    SLOT.get_or_init(Slot::new)
}

fn rtmp_slot() -> &'static Slot {
    static SLOT: OnceLock<Slot> = OnceLock::new();
    SLOT.get_or_init(Slot::new)
}

const FILE: fn() -> &'static Slot = file_slot;
const RTMP: fn() -> &'static Slot = rtmp_slot;

fn file_active() -> bool {
    file_slot()
        .inner
        .lock()
        .ok()
        .is_some_and(|slot| slot.is_some())
}

fn start_slot(
    slot: &fn() -> &'static Slot,
    pipelines: Vec<String>,
    width: u32,
    height: u32,
) -> bool {
    let Some((pipeline, appsrc)) = open_pipeline(&pipelines) else {
        return false;
    };
    let (jobs, rx) = mpsc::sync_channel(2);
    let thread = std::thread::Builder::new()
        .name("freeshow-record".into())
        .spawn(move || serve(pipeline, appsrc, rx))
        .ok();
    let Some(thread) = thread else {
        tracing::error!("could not start the recording");
        return false;
    };
    if let Ok(mut current) = slot().inner.lock() {
        *current = Some(Running {
            width,
            height,
            jobs,
            thread,
        });
        true
    } else {
        false
    }
}

fn stop_slot(slot: &fn() -> &'static Slot) {
    let running = slot().inner.lock().ok().and_then(|mut slot| slot.take());
    let Some(running) = running else {
        return;
    };
    let _ = running.jobs.send(Job::Stop);
    let _ = running.thread.join();
}

fn send(slot: &fn() -> &'static Slot, pixels: &[u8], width: u32, height: u32) {
    let Ok(slot) = slot().inner.lock() else {
        return;
    };
    let Some(running) = slot.as_ref() else {
        return;
    };
    if running.width != width || running.height != height {
        return;
    }
    let _ = running.jobs.try_send(Job::Frame(pixels.to_vec()));
}

fn file_pipeline(path: &Path, width: u32, height: u32) -> Vec<String> {
    let location = gst_string(&path.display().to_string());
    encoders()
        .into_iter()
        .map(|encoder| {
            format!(
                "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! {encoder} ! h264parse ! mp4mux fragment-duration=1000000000 ! filesink location=\"{location}\""
            )
        })
        .collect()
}

fn rtmp_pipeline(url: &str, width: u32, height: u32) -> Vec<String> {
    let location = gst_string(url);
    encoders()
        .into_iter()
        .map(|encoder| {
            format!(
                "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! {encoder} ! h264parse ! flvmux streamable=true ! rtmpsink location=\"{location}\""
            )
        })
        .collect()
}

fn encoders() -> [&'static str; 2] {
    [
        "x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 bitrate=2000",
        "vtenc_h264 realtime=true bitrate=2000",
    ]
}

fn gst_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn open_pipeline(descriptions: &[String]) -> Option<(gstreamer::Pipeline, AppSrc)> {
    if let Err(err) = gstreamer::init() {
        tracing::error!("recording did not start: {err}");
        return None;
    }
    for description in descriptions {
        let Ok(element) = gstreamer::parse::launch(description) else {
            continue;
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
            continue;
        };
        let Some(appsrc) = pipeline
            .by_name("src")
            .and_then(|element| element.dynamic_cast::<AppSrc>().ok())
        else {
            let _ = pipeline.set_state(gstreamer::State::Null);
            continue;
        };
        if pipeline.set_state(gstreamer::State::Playing).is_err() {
            let _ = pipeline.set_state(gstreamer::State::Null);
            continue;
        }
        tracing::info!("recording started");
        return Some((pipeline, appsrc));
    }
    tracing::error!("recording did not start");
    None
}

fn serve(pipeline: gstreamer::Pipeline, appsrc: AppSrc, jobs: Receiver<Job>) {
    let mut index = 0u64;
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Frame(frame) => {
                let mut buffer = gstreamer::Buffer::from_slice(frame);
                if let Some(buffer) = buffer.get_mut() {
                    buffer.set_pts(gstreamer::ClockTime::from_nseconds(
                        index * 1_000_000_000 / 30,
                    ));
                    buffer.set_duration(gstreamer::ClockTime::from_nseconds(1_000_000_000 / 30));
                }
                index += 1;
                if appsrc.push_buffer(buffer).is_err() {
                    break;
                }
            }
            Job::Stop => {
                let _ = appsrc.end_of_stream();
                if let Some(bus) = pipeline.bus() {
                    let _ = bus.timed_pop_filtered(
                        gstreamer::ClockTime::from_seconds(3),
                        &[gstreamer::MessageType::Eos, gstreamer::MessageType::Error],
                    );
                }
                break;
            }
        }
    }
    let _ = pipeline.set_state(gstreamer::State::Null);
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_few_frames_become_an_mp4() {
        let path = std::env::temp_dir().join(format!("freeshow-record-{}.mp4", std::process::id()));
        let _ = fs::remove_file(&path);
        let capture = ServiceCapture;
        assert!(capture.toggle_file(&path, 320, 180));
        let frame = vec![255u8, 0, 0, 255].repeat(320 * 180);
        for _ in 0..8 {
            capture.submit(&frame, 320, 180);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!capture.toggle_file(&path, 320, 180));
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.len() > 32, "file was {} bytes", bytes.len());
        assert!(
            bytes
                .windows(4)
                .any(|mark| mark == b"ftyp" || mark == b"moof"),
            "recording was not an mp4"
        );
        let _ = fs::remove_file(&path);
    }
}
