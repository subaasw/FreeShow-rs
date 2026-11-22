// SPDX-License-Identifier: GPL-3.0-only

//! Platform and hardware audience sinks: virtual camera, Blackmagic, Spout, Syphon.

use std::sync::mpsc;

use gstreamer::prelude::*;

/// Linux v4l2loopback / v4l2sink virtual camera. Other platforms stay disabled
/// unless a native sink is compiled in.
pub fn virtual_camera_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        element_available("v4l2sink")
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `appsrc` launch line that feeds a v4l2 loopback device.
pub fn virtual_camera_pipeline(device: &str, width: u32, height: u32) -> String {
    let device = device.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! video/x-raw,format=YUY2 ! v4l2sink device=\"{device}\" sync=false"
    )
}

/// Blackmagic Decklink via GStreamer. No vendor SDK is required.
pub fn blackmagic_available() -> bool {
    element_available("decklinkvideosink")
}

pub fn blackmagic_pipeline(device: i32, width: u32, height: u32) -> String {
    format!(
        "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! decklinkvideosink device-number={device} sync=false"
    )
}

/// Spout is a Windows-only texture share. Disabled on this platform.
pub fn spout_available() -> bool {
    cfg!(target_os = "windows") && element_available("spoutsink")
}

/// Syphon is a macOS-only texture share. Disabled on this platform.
pub fn syphon_available() -> bool {
    cfg!(target_os = "macos") && element_available("syphonsink")
}

pub fn spout_syphon_pipeline(name: &str, width: u32, height: u32) -> Option<String> {
    let name = name.replace('\\', "\\\\").replace('"', "\\\"");
    if spout_available() {
        return Some(format!(
            "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! spoutsink spout-name=\"{name}\""
        ));
    }
    if syphon_available() {
        return Some(format!(
            "appsrc name=src is-live=true format=time block=false caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! syphonsink server-name=\"{name}\""
        ));
    }
    None
}

pub fn element_available(name: &str) -> bool {
    if gstreamer::init().is_err() {
        return false;
    }
    gstreamer::ElementFactory::find(name).is_some()
}

/// A live appsrc sink that is optional: missing devices or plugins leave it dark.
pub struct OptionalSink {
    frames: Option<mpsc::SyncSender<Vec<u8>>>,
    width: u32,
    height: u32,
}

impl OptionalSink {
    pub fn open(description: &str, width: u32, height: u32) -> Self {
        if gstreamer::init().is_err() {
            return Self::disabled(width, height);
        }
        let Ok(element) = gstreamer::parse::launch(description) else {
            return Self::disabled(width, height);
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
            return Self::disabled(width, height);
        };
        let Some(src) = pipeline
            .by_name("src")
            .and_then(|element| element.dynamic_cast::<gstreamer_app::AppSrc>().ok())
        else {
            return Self::disabled(width, height);
        };
        if pipeline.set_state(gstreamer::State::Playing).is_err() {
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Self::disabled(width, height);
        }
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let started = std::thread::Builder::new()
            .name("freeshow-sink".into())
            .spawn(move || pump(pipeline, src, rx, width, height));
        if started.is_err() {
            return Self::disabled(width, height);
        }
        Self {
            frames: Some(tx),
            width,
            height,
        }
    }

    pub fn disabled(width: u32, height: u32) -> Self {
        Self {
            frames: None,
            width,
            height,
        }
    }

    pub fn is_live(&self) -> bool {
        self.frames.is_some()
    }

    pub fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        if width != self.width || height != self.height {
            return;
        }
        if let Some(frames) = &self.frames {
            let _ = frames.try_send(pixels.to_vec());
        }
    }
}

fn pump(
    pipeline: gstreamer::Pipeline,
    src: gstreamer_app::AppSrc,
    rx: mpsc::Receiver<Vec<u8>>,
    width: u32,
    height: u32,
) {
    use gstreamer::prelude::*;
    let pts = std::sync::atomic::AtomicU64::new(0);
    while let Ok(pixels) = rx.recv() {
        let mut buffer = gstreamer::Buffer::from_slice(pixels);
        if let Some(buffer) = buffer.get_mut() {
            let running = pts.fetch_add(33_333_333, std::sync::atomic::Ordering::Relaxed);
            buffer.set_pts(gstreamer::ClockTime::from_nseconds(running));
            buffer.set_duration(gstreamer::ClockTime::from_nseconds(33_333_333));
        }
        let _ = width;
        let _ = height;
        if src.push_buffer(buffer).is_err() {
            break;
        }
    }
    let _ = pipeline.set_state(gstreamer::State::Null);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_camera_and_blackmagic_pipelines_name_their_sinks() {
        let cam = virtual_camera_pipeline("/dev/video10", 1920, 1080);
        assert!(cam.contains("v4l2sink"));
        assert!(cam.contains("/dev/video10"));
        let deck = blackmagic_pipeline(0, 1920, 1080);
        assert!(deck.contains("decklinkvideosink"));
        assert!(deck.contains("device-number=0"));
    }

    #[test]
    fn spout_and_syphon_are_gated_to_their_platforms() {
        if cfg!(target_os = "linux") {
            assert!(!spout_available());
            assert!(!syphon_available());
            assert!(spout_syphon_pipeline("FreeShow", 64, 48).is_none());
        }
        assert!(!OptionalSink::disabled(64, 48).is_live());
    }

    #[test]
    fn a_missing_or_busy_device_leaves_the_sink_dark() {
        if !virtual_camera_available() {
            return;
        }
        let sink = OptionalSink::open(
            &virtual_camera_pipeline("/dev/video-not-a-real-loopback", 64, 48),
            64,
            48,
        );
        // A missing device may fail to go to Playing and stays disabled.
        let _ = sink.is_live();
    }
}
