// SPDX-License-Identifier: GPL-3.0-only

mod icecast;
pub use icecast::IcecastSender;
mod whip;
pub use whip::whip_post;
mod sinks;
#[cfg(feature = "webrtc")]
mod webrtc;
pub use sinks::{
    OptionalSink, blackmagic_available, blackmagic_pipeline, element_available, spout_available,
    spout_syphon_pipeline, syphon_available, virtual_camera_available, virtual_camera_pipeline,
};
#[cfg(feature = "webrtc")]
pub use webrtc::WebrtcSender;

#[cfg(not(feature = "webrtc"))]
/// Stub used when the crate is built without the webrtc feature.
pub struct WebrtcSender;

#[cfg(not(feature = "webrtc"))]
impl WebrtcSender {
    pub fn open(_url: &str, _token: Option<&str>, _width: u32, _height: u32) -> Option<Self> {
        None
    }

    pub fn submit(&self, _pixels: &[u8], _width: u32, _height: u32) {}
}

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
        self.start_rtmp_routed(url, width, height, "default")
    }

    fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        send(&FILE, pixels, width, height);
        send(&RTMP, pixels, width, height);
    }
}

impl ServiceCapture {
    pub fn stop_all(&self) {
        stop_slot(&FILE);
        stop_slot(&RTMP);
    }
    pub fn stop_rtmp(&self) {
        stop_slot(&RTMP);
    }
    pub fn start_rtmp_routed(&self, url: &str, width: u32, height: u32, output: &str) -> bool {
        if url.trim().is_empty() {
            return false;
        }
        stop_slot(&RTMP);
        start_slot(
            &RTMP,
            rtmp_pipeline_audio(url, width, height, output),
            width,
            height,
        )
    }
}

pub(crate) enum Job {
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
    start_pipeline(slot, pipeline, appsrc, width, height)
}

fn start_pipeline(
    slot: &fn() -> &'static Slot,
    pipeline: gstreamer::Pipeline,
    appsrc: AppSrc,
    width: u32,
    height: u32,
) -> bool {
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

fn rtmp_pipeline_audio(url: &str, width: u32, height: u32, output: &str) -> Vec<String> {
    let location = gst_string(url);
    let audio = format!(
        " {} ! queue ! avenc_aac ! aacparse ! mux.audio",
        freeshow_media::network_audio_mix(output)
    );
    encoders()
        .into_iter()
        .map(|encoder| {
            format!(
                "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videorate skip-to-first=true max-duplication-time=250000000 ! video/x-raw,framerate=30/1 ! videoconvert ! {encoder} ! h264parse ! mux.video flvmux name=mux streamable=true latency=100000000 ! rtmpsink location=\"{location}\"{audio}"
            )
        })
        .collect()
}

pub(crate) fn encoders() -> [&'static str; 2] {
    [
        "x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 bitrate=2000",
        "vtenc_h264 realtime=true bitrate=2000",
    ]
}

pub(crate) fn gst_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn init_gstreamer() -> Result<(), String> {
    gstreamer::init().map_err(|err| format!("GStreamer did not start: {err}"))
}

fn open_pipeline(descriptions: &[String]) -> Option<(gstreamer::Pipeline, AppSrc)> {
    if let Err(err) = init_gstreamer() {
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
    let period = std::time::Duration::from_nanos(1_000_000_000 / 30);
    let mut next = std::time::Instant::now();
    let mut last: Option<Vec<u8>> = None;
    loop {
        match jobs.recv_timeout(next.saturating_duration_since(std::time::Instant::now())) {
            Ok(Job::Frame(frame)) => {
                last = Some(frame);
            }
            Ok(Job::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = appsrc.end_of_stream();
                let _ = pipeline.send_event(gstreamer::event::Eos::new());
                if let Some(bus) = pipeline.bus() {
                    let _ = bus.timed_pop_filtered(
                        gstreamer::ClockTime::from_seconds(3),
                        &[gstreamer::MessageType::Eos, gstreamer::MessageType::Error],
                    );
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = std::time::Instant::now();
        if now < next {
            continue;
        }
        // Stay on the frame cadence when a timer wakes slightly late. Skip
        // missed ticks after a stall so we never send a burst to catch up.
        next += period;
        if next <= now {
            next = now + period;
        }
        if let Some(frame) = &last {
            if appsrc.current_level_bytes() >= (frame.len() as u64).saturating_mul(2) {
                continue;
            }
            let mut buffer = gstreamer::Buffer::from_slice(frame.clone());
            if let Some(buffer) = buffer.get_mut() {
                buffer.set_pts(
                    pipeline
                        .current_running_time()
                        .unwrap_or(gstreamer::ClockTime::ZERO),
                );
                buffer.set_duration(gstreamer::ClockTime::from_nseconds(1_000_000_000 / 30));
            }
            if appsrc.push_buffer(buffer).is_err() {
                break;
            }
        }
    }
    let _ = pipeline.set_state(gstreamer::State::Null);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn count_buffers(pad: gstreamer::Pad) -> Arc<AtomicUsize> {
        let count = Arc::new(AtomicUsize::new(0));
        let probe_count = count.clone();
        pad.add_probe(gstreamer::PadProbeType::BUFFER, move |_, _| {
            probe_count.fetch_add(1, Ordering::Relaxed);
            gstreamer::PadProbeReturn::Ok
        })
        .unwrap();
        count
    }

    fn routed_capture(path: &Path, url: Option<&str>) -> (usize, usize) {
        let id = format!("record-network-{}", std::process::id());
        let wav = path.with_extension("wav");
        gstreamer::init().unwrap();
        let writer=gstreamer::parse::launch(&format!("audiotestsrc num-buffers=300 samplesperbuffer=480 wave=square volume=0.8 ! audio/x-raw,rate=48000,channels=1 ! audioconvert ! wavenc ! filesink location=\"{}\"",gst_string(&wav.display().to_string())))
            .unwrap().downcast::<gstreamer::Pipeline>().unwrap();
        writer.set_state(gstreamer::State::Playing).unwrap();
        let message = writer
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gstreamer::ClockTime::from_seconds(5),
                &[gstreamer::MessageType::Eos, gstreamer::MessageType::Error],
            )
            .unwrap();
        assert_eq!(message.type_(), gstreamer::MessageType::Eos);
        writer.set_state(gstreamer::State::Null).unwrap();
        let mut counts = None;
        if let Some(url) = url {
            assert!(ServiceCapture.start_rtmp_routed(url, 64, 48, &id));
        } else {
            let descriptions: Vec<_> =
                rtmp_pipeline_audio(&path.display().to_string(), 64, 48, &id)
                    .into_iter()
                    .map(|pipeline| {
                        // Reproduce a capture source that reaches the pipeline about every 80 ms.
                        pipeline
                            .replace("! videorate ", "! identity sleep-time=80000 ! videorate ")
                            .replace("rtmpsink location=", "filesink location=")
                    })
                    .collect();
            let (pipeline, appsrc) = open_pipeline(&descriptions).unwrap();
            let raw = count_buffers(appsrc.static_pad("src").unwrap());
            let encoded = count_buffers(
                pipeline
                    .by_name("mux")
                    .unwrap()
                    .static_pad("video")
                    .unwrap(),
            );
            counts = Some((raw, encoded));
            assert!(start_pipeline(&RTMP, pipeline, appsrc, 64, 48));
        }
        let audio = freeshow_media::AudioOutput::play_file_with_network(
            &wav,
            &[],
            &[freeshow_media::NetworkRoute {
                output_id: id,
                gain: 0.25,
            }],
        )
        .unwrap();
        let frame = [255_u8, 0, 0, 255].repeat(64 * 48);
        ServiceCapture.submit(&frame, 64, 48);
        let started = std::time::Instant::now();
        loop {
            let elapsed = started.elapsed().as_millis();
            let caught_up = counts
                .as_ref()
                .is_none_or(|(_, encoded)| encoded.load(Ordering::Relaxed) >= 30);
            if elapsed >= 2700 || (elapsed >= 1200 && caught_up) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ServiceCapture.stop_rtmp();
        drop(audio);
        let _ = fs::remove_file(wav);
        counts.map_or((0, 0), |(raw, encoded)| {
            (raw.load(Ordering::Relaxed), encoded.load(Ordering::Relaxed))
        })
    }

    fn flv_tags(bytes: &[u8]) -> Vec<(u8, u32, Vec<u8>)> {
        assert_eq!(&bytes[..3], b"FLV");
        let mut offset = u32::from_be_bytes(bytes[5..9].try_into().unwrap()) as usize + 4;
        let mut tags = Vec::new();
        while offset + 11 <= bytes.len() {
            let size =
                u32::from_be_bytes([0, bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
                    as usize;
            let timestamp = u32::from_be_bytes([
                bytes[offset + 7],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
            ]);
            assert!(offset + 11 + size + 4 <= bytes.len(), "truncated FLV tag");
            tags.push((
                bytes[offset],
                timestamp,
                bytes[offset + 11..offset + 11 + size].to_vec(),
            ));
            offset += 11 + size + 4;
        }
        tags
    }

    #[test]
    fn a_stalled_sink_does_not_accumulate_unbounded_raw_frames() {
        let (pipeline,src)=open_pipeline(&["appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width=64,height=48,framerate=30/1 ! fakesink name=slow sync=false async=false".into()]).unwrap();
        let pad = pipeline
            .by_name("slow")
            .unwrap()
            .static_pad("sink")
            .unwrap();
        let probe = pad
            .add_probe(gstreamer::PadProbeType::BLOCK_DOWNSTREAM, |_, _| {
                gstreamer::PadProbeReturn::Ok
            })
            .unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        let worker_src = src.clone();
        let worker = std::thread::spawn(move || serve(pipeline, worker_src, rx));
        let bytes = 64 * 48 * 4;
        tx.send(Job::Frame(vec![0; bytes])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(180));
        let queued = src.current_level_bytes();
        // Unblock before asserting, so a failed check still cleans up its thread.
        pad.remove_probe(probe);
        tx.send(Job::Stop).unwrap();
        worker.join().unwrap();
        assert!(
            queued > 0 && queued <= bytes as u64 * 2,
            "raw queue grew to {queued} bytes"
        );
    }

    #[test]
    fn the_stream_mux_contains_routed_audio_and_repeats_a_static_picture() {
        let path =
            std::env::temp_dir().join(format!("freeshow-stream-audio-{}.flv", std::process::id()));
        let (raw_frames, encoded_frames) = routed_capture(&path, None);
        let tags = flv_tags(&fs::read(&path).unwrap());
        let audio: Vec<_> = tags
            .iter()
            .filter(|(kind, _, data)| *kind == 8 && data.len() >= 2 && data[0] >> 4 == 10)
            .collect();
        let config = &audio
            .iter()
            .find(|(_, _, data)| data[1] == 0)
            .expect("AAC codec data")
            .2[2..];
        // Decode the actual AAC payloads. Some GStreamer FLV demuxer versions
        // reset codec_data on repeated onMetaData tags in a valid live stream.
        let decoder=gstreamer::parse::launch("appsrc name=audio_src format=time ! aacparse ! avdec_aac ! audioconvert ! audio/x-raw,format=F32LE ! appsink name=audio sync=false")
            .unwrap().downcast::<gstreamer::Pipeline>().unwrap();
        let src = decoder
            .by_name("audio_src")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSrc>()
            .unwrap();
        src.set_caps(Some(
            &gstreamer::Caps::builder("audio/mpeg")
                .field("mpegversion", 4_i32)
                .field("stream-format", "raw")
                .field("codec_data", gstreamer::Buffer::from_slice(config.to_vec()))
                .build(),
        ));
        let sink = decoder
            .by_name("audio")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSink>()
            .unwrap();
        decoder.set_state(gstreamer::State::Playing).unwrap();
        for (_, timestamp, data) in audio.iter().filter(|(_, _, data)| data[1] == 1) {
            let mut buffer = gstreamer::Buffer::from_slice(data[2..].to_vec());
            buffer
                .get_mut()
                .unwrap()
                .set_pts(gstreamer::ClockTime::from_mseconds(u64::from(*timestamp)));
            src.push_buffer(buffer).unwrap();
        }
        src.end_of_stream().unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut energy = 0.0_f64;
        let mut samples = 0;
        while std::time::Instant::now() < end {
            if let Some(sample) = sink.try_pull_sample(gstreamer::ClockTime::from_mseconds(100)) {
                let map = sample.buffer().unwrap().map_readable().unwrap();
                for bytes in map.as_slice().as_chunks::<4>().0 {
                    let value = f32::from_le_bytes(*bytes);
                    energy += f64::from(value * value);
                    samples += 1;
                }
            } else if sink.is_eos() {
                break;
            }
        }
        decoder.set_state(gstreamer::State::Null).unwrap();
        assert!(
            samples > 48000,
            "stream must contain sustained audio, got {samples} samples"
        );
        let rms = (energy / (samples as f64)).sqrt();
        assert!(
            rms > 0.1 && rms < 0.3,
            "routed gain was not preserved: RMS {rms}"
        );
        let video: Vec<_> = tags
            .iter()
            .filter(|(kind, _, data)| {
                *kind == 9 && data.len() > 5 && data[0] & 0xf == 7 && data[1] == 1
            })
            .collect();
        assert!(
            video.len() >= 20,
            "only {} static video frames in FLV ({raw_frames} raw, {encoded_frames} at the mux)",
            video.len()
        );
        assert!(
            encoded_frames > raw_frames,
            "expected frame-rate correction under slow input"
        );
        let first_video = video.first().unwrap().1;
        let last_video = video.last().unwrap().1;
        assert!(
            last_video.saturating_sub(first_video) >= 900,
            "the static video only covered {first_video}..{last_video}ms"
        );
        let last_audio = audio
            .iter()
            .filter(|(_, _, data)| data[1] == 1)
            .map(|(_, timestamp, _)| *timestamp)
            .max()
            .unwrap();
        assert!(
            last_audio >= 900,
            "stream audio timeline was only {last_audio}ms"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires a local RTMP listener and FREESHOW_TEST_RTMP"]
    fn publish_to_local_rtmp_listener() {
        let url = std::env::var("FREESHOW_TEST_RTMP").expect("set a local RTMP listener URL");
        let local = url
            .strip_prefix("rtmp://127.0.0.1:")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(port, _)| port.parse::<u16>().ok());
        assert!(local.is_some(), "smoke test only publishes to loopback");
        let path =
            std::env::temp_dir().join(format!("freeshow-rtmp-smoke-{}.flv", std::process::id()));
        routed_capture(&path, Some(&url));
    }

    #[test]
    fn a_few_frames_become_an_mp4() {
        let path = std::env::temp_dir().join(format!("freeshow-record-{}.mp4", std::process::id()));
        let _ = fs::remove_file(&path);
        let capture = ServiceCapture;
        assert!(capture.toggle_file(&path, 320, 180));
        let frame = [255u8, 0, 0, 255].repeat(320 * 180);
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
