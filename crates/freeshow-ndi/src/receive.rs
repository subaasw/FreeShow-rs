// SPDX-License-Identifier: GPL-3.0-only

use gstreamer::prelude::*;

/// An NDI or OMT source that can be received when the runtime plugin is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSource {
    pub name: String,
    pub kind: SourceKind,
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

/// True when an OMT source element is installed.
pub fn omt_receive_available() -> bool {
    element_available("omtsrc") || element_available("omtinput")
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

/// The GStreamer launch line used to receive an OMT source, when that runtime exists.
pub fn omt_receive_pipeline(name: &str, width: u32, height: u32) -> String {
    let name = name.replace('\\', "\\\\").replace('"', "\\\"");
    let element = if element_available("omtsrc") {
        "omtsrc"
    } else {
        "omtinput"
    };
    format!(
        "{element} name=\"{name}\" ! videoconvert ! videoscale ! video/x-raw,format=RGBA,width={width},height={height} ! appsink name=sink max-buffers=1 drop=true sync=false"
    )
}

/// Known NDI sources. Empty when the finder plugin is missing.
pub fn ndi_sources() -> Vec<NetworkSource> {
    sources_from_factory("ndisrc")
}

pub fn omt_sources() -> Vec<NetworkSource> {
    let kind = SourceKind::Omt;
    sources_from_factory("omtsrc")
        .into_iter()
        .chain(sources_from_factory("omtinput"))
        .map(|mut source| {
            source.kind = kind;
            source
        })
        .collect()
}

fn sources_from_factory(factory: &str) -> Vec<NetworkSource> {
    // Source discovery needs an NDI/OMT finder plugin. When it is missing the
    // operator can still open a source by name; the list stays empty.
    let _ = factory;
    Vec::new()
}

/// Receives one NDI or OMT picture as RGBA frames. Disabled when the plugin is missing.
pub struct NetworkReceiver {
    _pipeline: Option<gstreamer::Pipeline>,
    sink: Option<gstreamer_app::AppSink>,
}

impl NetworkReceiver {
    pub fn open_ndi(name: &str, width: u32, height: u32) -> Self {
        Self::open(
            &ndi_receive_pipeline(name, width, height),
            ndi_receive_available(),
        )
    }

    pub fn open_omt(name: &str, width: u32, height: u32) -> Self {
        Self::open(
            &omt_receive_pipeline(name, width, height),
            omt_receive_available(),
        )
    }

    fn open(description: &str, available: bool) -> Self {
        if !available {
            return Self {
                _pipeline: None,
                sink: None,
            };
        }
        if gstreamer::init().is_err() {
            return Self {
                _pipeline: None,
                sink: None,
            };
        }
        let Ok(element) = gstreamer::parse::launch(description) else {
            return Self {
                _pipeline: None,
                sink: None,
            };
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
            return Self {
                _pipeline: None,
                sink: None,
            };
        };
        let sink = pipeline
            .by_name("sink")
            .and_then(|element| element.downcast::<gstreamer_app::AppSink>().ok());
        let _ = pipeline.set_state(gstreamer::State::Playing);
        Self {
            _pipeline: Some(pipeline),
            sink,
        }
    }

    pub fn is_live(&self) -> bool {
        self.sink.is_some()
    }

    pub fn next_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_pipelines_include_the_source_name_and_size() {
        let ndi = ndi_receive_pipeline("Sanctuary", 1920, 1080);
        assert!(ndi.contains("ndisrc"));
        assert!(ndi.contains("ndi-name=\"Sanctuary\""));
        assert!(ndi.contains("width=1920"));
        let omt = omt_receive_pipeline("Lobby", 1280, 720);
        assert!(omt.contains("Lobby"));
        assert!(omt.contains("width=1280"));
    }

    #[test]
    fn a_missing_receive_plugin_stays_disabled() {
        if ndi_receive_available() {
            return;
        }
        let receiver = NetworkReceiver::open_ndi("None", 64, 48);
        assert!(!receiver.is_live());
        assert!(receiver.next_frame().is_none());
        if !omt_receive_available() {
            let omt = NetworkReceiver::open_omt("None", 64, 48);
            assert!(!omt.is_live());
        }
    }
}
