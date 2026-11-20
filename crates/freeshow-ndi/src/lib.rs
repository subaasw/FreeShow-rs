// SPDX-License-Identifier: GPL-3.0-only

use std::sync::mpsc;

use gstreamer::prelude::*;
use gstreamer_app::AppSrc;

/// Sends the audience picture to an NDI receiver. The show keeps running when
/// the NDI plugin is missing.
pub trait VideoSend {
    fn name(&self) -> &'static str;
    fn open(&self, width: u32, height: u32) -> NdiSender;
}

pub struct NdiOutput {
    pub source_name: String,
}

impl Default for NdiOutput {
    fn default() -> Self {
        Self {
            source_name: "FreeShow NDI".to_string(),
        }
    }
}

impl VideoSend for NdiOutput {
    fn name(&self) -> &'static str {
        "ndi"
    }

    fn open(&self, width: u32, height: u32) -> NdiSender {
        NdiSender::open(&self.source_name, width, height)
    }
}

pub struct NdiSender {
    width: u32,
    height: u32,
    frames: Option<mpsc::SyncSender<Vec<u8>>>,
}

impl NdiSender {
    pub fn open(name: &str, width: u32, height: u32) -> Self {
        Self::open_audio(name, width, height, None)
    }

    pub fn open_routed(name: &str, width: u32, height: u32, output: &str) -> Self {
        Self::open_audio(name, width, height, Some(output.to_string()))
    }

    pub fn disabled(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frames: None,
        }
    }

    fn open_audio(name: &str, width: u32, height: u32, output: Option<String>) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = name.to_string();
        let started = std::thread::Builder::new()
            .name("freeshow-ndi".into())
            .spawn(move || serve(name, width, height, rx, output));
        if started.is_err() {
            tracing::error!("could not start the ndi sender");
            return Self {
                width,
                height,
                frames: None,
            };
        }
        Self {
            width,
            height,
            frames: Some(tx),
        }
    }

    pub fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        if width != self.width || height != self.height {
            return;
        }
        let Some(frames) = &self.frames else {
            return;
        };
        let _ = frames.try_send(pixels.to_vec());
    }
}

fn explain_failure(pipeline: &gstreamer::Pipeline, name: &str, err: impl std::fmt::Display) {
    let mut missing_runtime = false;
    if let Some(bus) = pipeline.bus() {
        while let Some(message) = bus.pop() {
            if let gstreamer::MessageView::Error(error) = message.view() {
                let detail = error.debug().unwrap_or_default().to_string();
                if detail.contains("Failed loading NDI SDK")
                    || error.error().to_string().contains("NDI SDK")
                {
                    missing_runtime = true;
                } else {
                    tracing::error!("ndi sender: {} ({detail})", error.error());
                }
            }
        }
    }
    if missing_runtime {
        tracing::info!("ndi sender {name} needs the NDI runtime");
    } else {
        tracing::error!("ndi sender {name} did not play: {err}");
    }
}

fn pipeline_description(name: &str, width: u32, height: u32, output: Option<&str>) -> String {
    let safe_name = name.replace('\\', "\\\\").replace('"', "\\\"");
    let destination = if let Some(output) = output {
        format!(
            "ndi_combine.video ndisinkcombiner name=ndi_combine ! ndisink ndi-name=\"{safe_name}\" {} ! queue ! ndi_combine.audio",
            freeshow_media::network_audio_mix(output)
        )
    } else {
        format!("ndisink ndi-name=\"{safe_name}\"")
    };
    format!(
        "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=UYVY,width={width},height={height},framerate=30/1 ! {destination}"
    )
}

fn serve(
    name: String,
    width: u32,
    height: u32,
    frames: mpsc::Receiver<Vec<u8>>,
    output: Option<String>,
) {
    if let Err(err) = gstreamer::init() {
        tracing::error!("ndi sender did not start: {err}");
        return;
    }
    let safe_name = name.replace('"', "'");
    let description = pipeline_description(&name, width, height, output.as_deref());
    let Ok(element) = gstreamer::parse::launch(&description) else {
        tracing::error!("ndi sender {safe_name} is not available");
        return;
    };
    let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
        tracing::error!("ndi sender did not start");
        return;
    };
    let Some(appsrc) = pipeline
        .by_name("src")
        .and_then(|element| element.dynamic_cast::<AppSrc>().ok())
    else {
        tracing::error!("ndi sender is missing appsrc");
        return;
    };
    if let Err(err) = pipeline.set_state(gstreamer::State::Playing) {
        explain_failure(&pipeline, &safe_name, err);
        let _ = pipeline.set_state(gstreamer::State::Null);
        return;
    }
    let (change, _, _) = pipeline.state(gstreamer::ClockTime::from_seconds(2));
    if let Err(err) = change {
        explain_failure(&pipeline, &safe_name, err);
        let _ = pipeline.set_state(gstreamer::State::Null);
        return;
    }
    tracing::info!("ndi sender {safe_name}");
    let period = std::time::Duration::from_nanos(1_000_000_000 / 30);
    let mut next = std::time::Instant::now();
    let mut last = vec![0u8; width as usize * height as usize * 4];
    let mut cached = rgba_to_uyvy(&last, width, height).unwrap_or_default();
    let mut changed = false;
    loop {
        match frames.recv_timeout(next.saturating_duration_since(std::time::Instant::now())) {
            Ok(frame) => {
                last = frame;
                changed = true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if std::time::Instant::now() < next {
            continue;
        }
        next = std::time::Instant::now() + period;
        if changed {
            if let Some(frame) = rgba_to_uyvy(&last, width, height) {
                cached = frame;
            }
            changed = false;
        }
        if appsrc.current_level_bytes() >= (cached.len() as u64).saturating_mul(2) {
            continue;
        }
        let mut buffer = gstreamer::Buffer::from_slice(cached.clone());
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
    let _ = pipeline.set_state(gstreamer::State::Null);
}

/// Pack RGBA8 into UYVY 4:2:2, the picture format FreeShow sends to NDI.
/// The conversion is BT.601 full range with the same integer weights FreeShow uses.
pub fn rgba_to_uyvy(pixels: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width % 2 != 0 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    if pixels.len() < width * height * 4 {
        return None;
    }
    let mut out = vec![0u8; width * height * 2];
    for y in 0..height {
        let mut source = y * width * 4;
        let mut dest = y * width * 2;
        for _ in 0..(width / 2) {
            let r0 = pixels[source] as i32;
            let g0 = pixels[source + 1] as i32;
            let b0 = pixels[source + 2] as i32;
            let r1 = pixels[source + 4] as i32;
            let g1 = pixels[source + 5] as i32;
            let b1 = pixels[source + 6] as i32;
            out[dest] = clamp_u8(((-43 * r0 - 85 * g0 + 128 * b0) >> 8) + 128);
            out[dest + 1] = clamp_u8((77 * r0 + 150 * g0 + 29 * b0) >> 8);
            out[dest + 2] = clamp_u8(((128 * r0 - 107 * g0 - 21 * b0) >> 8) + 128);
            out[dest + 3] = clamp_u8((77 * r1 + 150 * g1 + 29 * b1) >> 8);
            source += 8;
            dest += 4;
        }
    }
    Some(out)
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the GStreamer NDI combiner plugin, but not the SDK"]
    fn the_combiner_attaches_audio_to_video_buffers() {
        gstreamer::init().unwrap();
        let id = format!("ndi-meta-test-{}", std::process::id());
        let description = pipeline_description("FreeShow Test", 32, 32, Some(&id)).replace(
            "ndisink ndi-name=\"FreeShow Test\"",
            "appsink name=capture sync=false",
        );
        let pipeline = gstreamer::parse::launch(&description)
            .unwrap()
            .downcast::<gstreamer::Pipeline>()
            .unwrap();
        let src = pipeline
            .by_name("src")
            .unwrap()
            .dynamic_cast::<AppSrc>()
            .unwrap();
        let sink = pipeline
            .by_name("capture")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSink>()
            .unwrap();
        pipeline.set_state(gstreamer::State::Playing).unwrap();
        let mut attached = false;
        let mut names = Vec::new();
        for index in 0..20_u64 {
            let mut buffer =
                gstreamer::Buffer::from_slice(vec![128_u8, 200, 128, 200].repeat(32 * 32 / 2));
            buffer
                .get_mut()
                .unwrap()
                .set_pts(gstreamer::ClockTime::from_nseconds(
                    index * 1_000_000_000 / 30,
                ));
            buffer
                .get_mut()
                .unwrap()
                .set_duration(gstreamer::ClockTime::from_nseconds(1_000_000_000 / 30));
            src.push_buffer(buffer).unwrap();
            if let Some(sample) = sink.try_pull_sample(gstreamer::ClockTime::from_mseconds(200)) {
                names = sample
                    .buffer()
                    .unwrap()
                    .iter_meta::<gstreamer::Meta>()
                    .map(|meta| meta.api().name().to_string())
                    .collect();
                attached = names.iter().any(|name| {
                    name.to_lowercase().contains("ndi") && name.to_lowercase().contains("audio")
                });
                if attached {
                    break;
                }
            }
        }
        pipeline.set_state(gstreamer::State::Null).unwrap();
        assert!(
            attached,
            "NDI video did not carry audio metadata: {names:?}"
        );
    }

    #[test]
    fn white_and_red_pack_to_uyvy() {
        let white = rgba_to_uyvy(&[255, 255, 255, 255, 255, 255, 255, 255], 2, 1).unwrap();
        assert_eq!(white, vec![128, 255, 128, 255]);
        let red = rgba_to_uyvy(&[255, 0, 0, 255, 255, 0, 0, 255], 2, 1).unwrap();
        assert_eq!(red, vec![85, 76, 255, 76]);
        assert!(rgba_to_uyvy(&[255, 0, 0, 255], 1, 1).is_none());
    }
}
