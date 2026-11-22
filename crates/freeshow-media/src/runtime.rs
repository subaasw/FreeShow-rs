// SPDX-License-Identifier: GPL-3.0-only

use crate::MediaError;

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

#[cfg(not(feature = "gst"))]
pub fn check_runtime() -> Result<RuntimeStatus, MediaError> {
    Err(MediaError::Video("built without GStreamer support".into()))
}
