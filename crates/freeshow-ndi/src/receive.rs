// SPDX-License-Identifier: GPL-3.0-only

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gstreamer::prelude::*;
use openmediatransport::{
    DecodedVideoFrame, Discovery, FrameType, OmtAddress, OmtError, ReceiverConfig, ReceiverSession,
};

/// A named NDI or OMT source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSource {
    pub name: String,
    pub kind: SourceKind,
}

impl NetworkSource {
    pub fn label(&self) -> String {
        let protocol = match self.kind {
            SourceKind::Ndi => "NDI",
            SourceKind::Omt => "OMT",
        };
        format!("{protocol} · {}", self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Ndi,
    Omt,
}

/// True when the GStreamer `ndisrc` element is installed.
pub fn ndi_receive_available() -> bool {
    element_available("ndisrc")
}

pub fn element_available(name: &str) -> bool {
    if gstreamer::init().is_err() {
        return false;
    }
    gstreamer::ElementFactory::find(name).is_some()
}

/// The GStreamer launch line used to receive an NDI source into RGBA appsink.
pub fn ndi_receive_pipeline(name: &str, width: u32, height: u32) -> String {
    let name = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "ndisrc ndi-name=\"{name}\" ! videoconvert ! videoscale ! video/x-raw,format=RGBA,width={width},height={height} ! appsink name=sink max-buffers=1 drop=true sync=false"
    )
}

/// Known NDI sources. Empty when the finder plugin is missing.
pub fn ndi_sources() -> Vec<NetworkSource> {
    let mut sources = sources_from_env("FREESHOW_NDI_SOURCES", SourceKind::Ndi);
    for source in discover_ndi() {
        if !sources.iter().any(|existing| existing.name == source.name) {
            sources.push(source);
        }
    }
    sources
}

pub fn omt_sources() -> Vec<NetworkSource> {
    let mut sources = sources_from_env("FREESHOW_OMT_SOURCES", SourceKind::Omt);
    if let Ok(discovery) = discover_omt() {
        for address in discovery.sources() {
            let name = address.instance_name();
            if !sources.iter().any(|entry| entry.name == name) {
                sources.push(NetworkSource {
                    name,
                    kind: SourceKind::Omt,
                });
            }
        }
    }
    sources
}

pub fn network_sources() -> Vec<NetworkSource> {
    let mut sources = ndi_sources();
    sources.extend(omt_sources());
    sources
}

/// Parse a comma- or newline-separated list of source names.
pub fn parse_source_list(text: &str, kind: SourceKind) -> Vec<NetworkSource> {
    text.split([',', '\n', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| NetworkSource {
            name: name.to_string(),
            kind,
        })
        .collect()
}

fn sources_from_env(var: &str, kind: SourceKind) -> Vec<NetworkSource> {
    std::env::var(var)
        .ok()
        .map(|value| parse_source_list(&value, kind))
        .unwrap_or_default()
}

fn discover_ndi() -> Vec<NetworkSource> {
    if !ndi_receive_available() {
        return Vec::new();
    }
    let monitor = gstreamer::DeviceMonitor::new();
    let _ = monitor.add_filter(Some("Video/Source"), None);
    let _ = monitor.start();
    let mut sources = Vec::new();
    for device in monitor.devices() {
        let name = device.display_name().to_string();
        if name.is_empty() {
            continue;
        }
        let class = device.device_class().to_ascii_lowercase();
        let looks = class.contains("ndi")
            || name.to_ascii_lowercase().contains("ndi")
            || factory_matches(&device, "ndisrc");
        if looks
            && !sources
                .iter()
                .any(|source: &NetworkSource| source.name == name)
        {
            sources.push(NetworkSource {
                name,
                kind: SourceKind::Ndi,
            });
        }
    }
    monitor.stop();
    sources
}

fn factory_matches(device: &gstreamer::Device, factory: &str) -> bool {
    device
        .properties()
        .and_then(|values| {
            values
                .get::<String>("device.api")
                .ok()
                .or_else(|| values.get::<String>("api").ok())
        })
        .is_some_and(|api| api.contains(factory) || factory.contains(&api))
}

type Picture = (u32, u32, Vec<u8>);
type LatestPicture = Mutex<Option<Picture>>;

/// Receives one NDI or OMT picture as RGBA frames.
#[derive(Default)]
pub struct NetworkReceiver {
    pipeline: Option<gstreamer::Pipeline>,
    sink: Option<gstreamer_app::AppSink>,
    omt: Option<Arc<LatestPicture>>,
}

impl NetworkReceiver {
    pub fn open_ndi(name: &str, width: u32, height: u32) -> Self {
        Self::open(
            &ndi_receive_pipeline(name, width, height),
            ndi_receive_available(),
        )
    }

    pub fn open_omt(name: &str) -> Self {
        if name.starts_with("omt://")
            && !OmtAddress::from_url(name).is_ok_and(|address| address.port != 0)
        {
            return Self::default();
        }
        let latest = Arc::new(Mutex::new(None));
        let slot = Arc::downgrade(&latest);
        let name = name.to_string();
        match std::thread::Builder::new()
            .name("freeshow-omt-input".into())
            .spawn(move || receive_omt(&name, slot))
        {
            Ok(_) => Self {
                pipeline: None,
                sink: None,
                omt: Some(latest),
            },
            Err(error) => {
                tracing::warn!(%error, "could not start OMT input");
                Self::default()
            }
        }
    }

    fn open(description: &str, available: bool) -> Self {
        if !available {
            return Self::default();
        }
        if gstreamer::init().is_err() {
            return Self::default();
        }
        let Ok(element) = gstreamer::parse::launch(description) else {
            return Self::default();
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
            return Self::default();
        };
        let sink = pipeline
            .by_name("sink")
            .and_then(|element| element.downcast::<gstreamer_app::AppSink>().ok());
        if let Err(error) = pipeline.set_state(gstreamer::State::Playing) {
            tracing::warn!(%error, "could not start NDI input");
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Self::default();
        }
        Self {
            pipeline: Some(pipeline),
            sink,
            omt: None,
        }
    }

    pub fn is_live(&self) -> bool {
        self.sink.is_some() || self.omt.is_some()
    }

    pub fn next_frame(&self) -> Option<Picture> {
        if let Some(latest) = &self.omt {
            return latest.lock().ok()?.take();
        }
        let sample = self
            .sink
            .as_ref()?
            .try_pull_sample(gstreamer::ClockTime::ZERO)?;
        let buffer = sample.buffer()?;
        let map = buffer.map_readable().ok()?;
        let caps = sample.caps()?;
        let struct_ = caps.structure(0)?;
        let width = struct_.get::<i32>("width").ok()? as u32;
        let height = struct_.get::<i32>("height").ok()? as u32;
        Some((width, height, map.as_slice().to_vec()))
    }
}

impl Drop for NetworkReceiver {
    fn drop(&mut self) {
        if let Some(pipeline) = &self.pipeline {
            let _ = pipeline.set_state(gstreamer::State::Null);
        }
    }
}

fn discover_omt() -> Result<Discovery, OmtError> {
    let mut discovery = Discovery::new()?;
    discovery.refresh()?;
    Ok(discovery)
}

fn omt_address<'a>(name: &str, sources: &'a [OmtAddress]) -> Option<&'a OmtAddress> {
    if let Some(exact) = sources
        .iter()
        .find(|address| address.instance_name() == name)
    {
        return Some(exact);
    }
    let mut matching = sources.iter().filter(|address| address.name == name);
    let single = matching.next()?;
    matching.next().is_none().then_some(single)
}

fn connect_omt(name: &str) -> Result<ReceiverSession, OmtError> {
    let config = ReceiverConfig {
        frame_types: FrameType::VIDEO,
        connect_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    if name.starts_with("omt://") {
        return ReceiverSession::connect(name, config);
    }
    let discovery = discover_omt()?;
    let address = omt_address(name, discovery.sources()).ok_or_else(|| {
        OmtError::InvalidArgument(format!("OMT source {name:?} is missing or ambiguous"))
    })?;
    ReceiverSession::connect_from_address(address, config)
}

fn receive_omt(name: &str, slot: Weak<LatestPicture>) {
    while slot.strong_count() > 0 {
        let receiver = match connect_omt(name) {
            Ok(receiver) => receiver,
            Err(error) => {
                tracing::debug!(%error, "waiting for OMT input {name}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        while slot.strong_count() > 0 {
            if let Some(frame) = receiver.recv_video_timeout(Duration::from_millis(200)) {
                if let Some(picture) = omt_picture(&frame)
                    && let Some(slot) = slot.upgrade()
                    && let Ok(mut latest) = slot.lock()
                {
                    *latest = Some(picture);
                }
            } else if !name.starts_with("omt://")
                && receiver.state() == openmediatransport::SessionState::Reconnecting
            {
                // A discovered sender may come back on a different port.
                break;
            }
        }
    }
}

fn omt_picture(frame: &DecodedVideoFrame) -> Option<Picture> {
    let row = (frame.width as usize).checked_mul(4)?;
    let stride = frame.stride as usize;
    let height = frame.height as usize;
    if row == 0 || height == 0 || stride < row || frame.pixels.len() < stride.checked_mul(height)? {
        return None;
    }
    let mut rgba = vec![0; row * height];
    for (from, to) in frame
        .pixels
        .chunks_exact(stride)
        .zip(rgba.chunks_exact_mut(row))
    {
        openmediatransport::bgra_to_rgba_into(&from[..row], to);
    }
    Some((frame.width, frame.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_pipelines_include_the_source_name_and_size() {
        let ndi = ndi_receive_pipeline("Sanctuary", 1920, 1080);
        assert!(ndi.contains("ndisrc"));
        assert!(ndi.contains("ndi-name=\"Sanctuary\""));
        assert!(ndi.contains("width=1920"));
    }

    #[test]
    fn source_lists_parse_and_stay_empty_without_a_plugin() {
        let listed = parse_source_list("Cam 1, Lobby\n ", SourceKind::Ndi);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Cam 1");
        if !ndi_receive_available() {
            // DeviceMonitor has nothing to find; the explicit list still works.
            assert!(discover_ndi().is_empty());
        }
        let _ = ndi_sources();
    }

    #[test]
    fn a_missing_receive_plugin_stays_disabled() {
        if ndi_receive_available() {
            return;
        }
        let receiver = NetworkReceiver::open_ndi("None", 64, 48);
        assert!(!receiver.is_live());
        assert!(receiver.next_frame().is_none());
    }

    #[test]
    fn omt_short_names_must_be_unambiguous() {
        let sources = [
            OmtAddress::from_full_name("Desk (Lobby)", 6400),
            OmtAddress::from_full_name("Booth (Lobby)", 6401),
            OmtAddress::from_full_name("Booth (Stage)", 6402),
        ];
        assert!(omt_address("Lobby", &sources).is_none());
        assert_eq!(omt_address("Desk (Lobby)", &sources).unwrap().port, 6400);
        assert_eq!(omt_address("Stage", &sources).unwrap().port, 6402);
    }

    #[test]
    fn native_omt_receives_rgba_from_a_loopback_sender() {
        use openmediatransport::{Codec, MediaFrame, Sender};
        let mut sender = Sender::create("FreeShow input test", FrameType::VIDEO).unwrap();
        let receiver = NetworkReceiver::open_omt(&format!("omt://127.0.0.1:{}", sender.port()));
        assert!(receiver.is_live());
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while std::time::Instant::now() < deadline {
            sender.poll_accept().unwrap();
            sender.poll_peer_metadata().unwrap();
            sender
                .send_video(MediaFrame {
                    frame_type: FrameType::VIDEO,
                    codec: Codec::Bgra.as_i32(),
                    width: 64,
                    height: 48,
                    stride: 64 * 4,
                    data: [30, 20, 220, 255].repeat(64 * 48),
                    ..Default::default()
                })
                .unwrap();
            if let Some((width, height, rgba)) = receiver.next_frame() {
                assert_eq!((width, height, rgba.len()), (64, 48, 64 * 48 * 4));
                assert!(rgba[0] > 180 && rgba[2] < 70, "expected red RGBA");
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no OMT frame reached the receiver");
    }
}
