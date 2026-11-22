// SPDX-License-Identifier: GPL-3.0-only

use crate::MediaError;
use std::path::Path;

pub struct RuntimeStatus {
    pub version: String,
    pub missing: Vec<String>,
    pub ndi_plugin: bool,
}

#[cfg(feature = "gst")]
pub fn check_runtime() -> Result<RuntimeStatus, MediaError> {
    use gstreamer as gst;
    gst::init().map_err(|error| MediaError::Video(error.to_string()))?;
    let mut required = vec![
        "appsrc",
        "appsink",
        "filesrc",
        "decodebin",
        "videoconvert",
        "videoscale",
        "videorate",
        "audioconvert",
        "audioresample",
        "audiomixer",
        "volume",
        "tee",
        "queue",
        "fakesink",
        "interaudiosrc",
        "interaudiosink",
        "h264parse",
        "mp4mux",
        "flvmux",
        "rtmpsink",
        "aacparse",
        "avenc_aac",
    ];
    if cfg!(target_os = "macos") {
        required.extend(["osxaudiosink", "avfvideosrc"]);
    }
    let mut missing: Vec<String> = required
        .into_iter()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .map(str::to_string)
        .collect();
    if gst::ElementFactory::find("x264enc").is_none()
        && gst::ElementFactory::find("vtenc_h264").is_none()
    {
        missing.push("x264enc or vtenc_h264".into());
    }
    Ok(RuntimeStatus {
        version: gst::version_string().to_string(),
        missing,
        ndi_plugin: gst::ElementFactory::find("ndisink").is_some(),
    })
}

/// Write a short 1080p30 H.264 sample using the same runtime as playback.
#[cfg(feature = "gst")]
pub fn write_benchmark_video(path: &Path) -> Result<(), MediaError> {
    use gst::prelude::*;
    use gstreamer as gst;

    let error = |message: &str| MediaError::Video(message.into());
    gst::init().map_err(|err| error(&err.to_string()))?;
    let pipeline = gst::parse::launch(
        "videotestsrc num-buffers=90 pattern=smpte ! video/x-raw,width=1920,height=1080,framerate=30/1 ! videoconvert ! x264enc tune=zerolatency speed-preset=ultrafast ! mp4mux ! filesink name=clip",
    )
    .map_err(|err| error(&err.to_string()))?
    .downcast::<gst::Pipeline>()
    .map_err(|_| error("benchmark launch did not return a pipeline"))?;
    let sink = pipeline.by_name("clip").expect("benchmark filesink");
    let location = path
        .to_str()
        .ok_or_else(|| error("invalid benchmark video path"))?;
    sink.set_property("location", location);
    let bus = pipeline.bus().expect("benchmark bus");
    let result = pipeline
        .set_state(gst::State::Playing)
        .map_err(|err| error(&err.to_string()))
        .and_then(|_| {
            let message = bus
                .timed_pop_filtered(
                    gst::ClockTime::from_seconds(15),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                )
                .ok_or_else(|| error("benchmark video encoding timed out"))?;
            if let gst::MessageView::Error(failure) = message.view() {
                return Err(error(&failure.error().to_string()));
            }
            Ok(())
        });
    let _ = pipeline.set_state(gst::State::Null);
    result
}

#[cfg(not(feature = "gst"))]
pub fn write_benchmark_video(_path: &Path) -> Result<(), MediaError> {
    Err(MediaError::Video("built without GStreamer support".into()))
}

#[cfg(not(feature = "gst"))]
pub fn check_runtime() -> Result<RuntimeStatus, MediaError> {
    Err(MediaError::Video("built without GStreamer support".into()))
}
