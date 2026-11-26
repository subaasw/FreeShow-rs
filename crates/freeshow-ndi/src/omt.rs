// SPDX-License-Identifier: GPL-3.0-only

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use openmediatransport::{AudioIngress, Codec, Discovery, FrameType, MediaFrame, OmtError, Sender};

const FPS: i32 = 30;
const PERIOD: Duration = Duration::from_nanos(1_000_000_000 / FPS as u64);

pub struct OmtSender {
    width: u32,
    height: u32,
    frames: mpsc::SyncSender<Vec<u8>>,
    worker: std::thread::JoinHandle<()>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl OmtSender {
    pub fn open_routed(name: &str, width: u32, height: u32, output: &str) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = name.to_string();
        let output = output.to_string();
        let ready = std::sync::Arc::<std::sync::atomic::AtomicBool>::default();
        let worker_ready = ready.clone();
        let worker = std::thread::Builder::new()
            .name("freeshow-omt".into())
            .spawn(move || {
                let start = || {
                    let mut sender = Sender::create(&name, FrameType::VIDEO | FrameType::AUDIO)?;
                    let mut discovery = Discovery::new()?;
                    discovery.register(&name, sender.port())?;
                    worker_ready.store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::info!("omt sender {name} on port {}", sender.port());
                    stream(&mut sender, width, height, &rx, &output)
                };
                if let Err(error) = start() {
                    tracing::error!(%error, "omt sender {name} stopped");
                }
            })?;
        Ok(Self {
            width,
            height,
            frames: tx,
            worker,
            ready,
        })
    }

    pub fn status(&self) -> crate::SenderStatus {
        crate::sender_status(&self.worker, &self.ready)
    }

    pub fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        if (width, height) == (self.width, self.height)
            && pixels.len() == width as usize * height as usize * 4
            && !self.worker.is_finished()
        {
            let _ = self.frames.try_send(pixels.to_vec());
        }
    }
}

fn stream(
    sender: &mut Sender,
    width: u32,
    height: u32,
    frames: &mpsc::Receiver<Vec<u8>>,
    output: &str,
) -> Result<(), OmtError> {
    let epoch = Instant::now();
    let audio = match start_audio(output, sender.audio_ingress()) {
        Ok(pipeline) => Some(pipeline),
        Err(error) => {
            tracing::warn!(%error, "omt audio is unavailable for output {output}");
            None
        }
    };
    let result = send_video(sender, width, height, frames, epoch);
    if let Some(audio) = audio {
        let _ = audio.set_state(gst::State::Null);
    }
    result
}

fn send_video(
    sender: &mut Sender,
    width: u32,
    height: u32,
    frames: &mpsc::Receiver<Vec<u8>>,
    epoch: Instant,
) -> Result<(), OmtError> {
    let mut next = Instant::now();
    let mut bgra = vec![0; width as usize * height as usize * 4];
    loop {
        match frames.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(mut pixels) => {
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                bgra = pixels;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if now < next {
            continue;
        }
        next = now + PERIOD;
        sender.poll_accept()?;
        sender.poll_peer_metadata()?;
        if sender.video_subscribed() {
            sender.send_video(MediaFrame {
                frame_type: FrameType::VIDEO,
                timestamp: (now.duration_since(epoch).as_nanos() / 100) as i64,
                codec: Codec::Bgra.as_i32(),
                width: width as i32,
                height: height as i32,
                stride: width as i32 * 4,
                frame_rate_n: FPS,
                aspect_ratio: width as f32 / height as f32,
                data: bgra.clone(),
                ..Default::default()
            })?;
        }
    }
}

fn start_audio(
    output: &str,
    ingress: AudioIngress,
) -> Result<gst::Pipeline, Box<dyn std::error::Error>> {
    gst::init()?;
    let description = format!(
        "{} ! appsink name=sink max-buffers=4 drop=true sync=false",
        freeshow_media::network_audio_mix(output)
    );
    let pipeline = gst::parse::launch(&description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "expected a network audio pipeline")?;
    let sink = pipeline
        .by_name("sink")
        .and_then(|sink| sink.dynamic_cast::<AppSink>().ok())
        .ok_or("missing network audio appsink")?;
    sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                if let Some(frame) = sample.buffer().and_then(audio_frame)
                    && let Err(error) = ingress.send(frame)
                {
                    tracing::warn!(%error, "omt audio send failed");
                    return Err(gst::FlowError::Error);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    if let Err(error) = pipeline.set_state(gst::State::Playing) {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(error.into());
    }
    Ok(pipeline)
}

fn audio_frame(buffer: &gst::BufferRef) -> Option<MediaFrame> {
    let timestamp = (buffer.pts()?.nseconds() / 100) as i64;
    let map = buffer.map_readable().ok()?;
    let interleaved = map.as_slice();
    if interleaved.is_empty() || !interleaved.len().is_multiple_of(8) {
        return None;
    }
    let mut planar = Vec::with_capacity(interleaved.len());
    for offset in [0, 4] {
        for pair in interleaved.as_chunks::<8>().0 {
            planar.extend_from_slice(&pair[offset..offset + 4]);
        }
    }
    Some(MediaFrame {
        frame_type: FrameType::AUDIO,
        timestamp,
        codec: Codec::Fpa1.as_i32(),
        sample_rate: 48_000,
        channels: 2,
        samples_per_channel: (interleaved.len() / 8) as i32,
        active_channels: 0b11,
        data: planar,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmediatransport::{ReceiverConfig, ReceiverSession};

    #[test]
    fn audio_packets_preserve_planar_channel_order_and_time() {
        gst::init().unwrap();
        let mut buffer = gst::Buffer::from_slice(
            [0.25_f32, -0.5, 0.75, -1.0]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        buffer
            .get_mut()
            .unwrap()
            .set_pts(gst::ClockTime::from_mseconds(50));
        let frame = audio_frame(&buffer).unwrap();
        assert_eq!(
            (frame.channels, frame.samples_per_channel, frame.timestamp),
            (2, 2, 500_000)
        );
        let samples: Vec<_> = frame
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect();
        assert_eq!(samples, [0.25, 0.75, -0.5, -1.0]);
    }

    #[test]
    fn sends_picture_and_routed_stereo_to_a_loopback_receiver() {
        gst::init().unwrap();
        let id = format!("omt-loopback-{}", std::process::id());
        let source = gst::parse::launch(&format!(
            "audiotestsrc is-live=true wave=square volume=0.25 ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! interaudiosink channel={} sync=false",
            freeshow_media::network_channel(&id, false)
        ))
        .unwrap();
        let mut sender =
            Sender::create("FreeShow loopback", FrameType::VIDEO | FrameType::AUDIO).unwrap();
        let receiver = ReceiverSession::connect(
            format!("omt://127.0.0.1:{}", sender.port()),
            ReceiverConfig {
                frame_types: FrameType::VIDEO | FrameType::AUDIO,
                auto_reconnect: false,
                ..Default::default()
            },
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send([220, 20, 30, 255].repeat(64 * 64)).unwrap();
        source.set_state(gst::State::Playing).unwrap();
        let worker = std::thread::spawn(move || stream(&mut sender, 64, 64, &rx, &id));
        let video = receiver.recv_video_timeout(Duration::from_secs(4));
        let mut stereo = None;
        let deadline = Instant::now() + Duration::from_secs(4);
        while let Some(frame) =
            receiver.recv_audio_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            let (left, right) = frame
                .pcm_planar_f32
                .split_at(frame.samples_per_channel as usize * 4);
            let audible = |plane: &[u8]| {
                plane
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|sample| f32::from_le_bytes(*sample).abs() > 0.1)
            };
            if audible(left) && audible(right) {
                stereo = Some(frame);
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        drop(tx);
        worker.join().unwrap().unwrap();
        source.set_state(gst::State::Null).unwrap();
        let video = video.expect("missing OMT video");
        assert_eq!((video.width, video.height), (64, 64));
        assert!(video.pixels[2] > 180 && video.pixels[0] < 70);
        let audio = stereo.expect("missing OMT stereo from the assigned audio bus");
        assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
    }
}
