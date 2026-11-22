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
    let mut sources = sources_from_env("FREESHOW_NDI_SOURCES", SourceKind::Ndi);
    for source in sources_from_factory("ndisrc") {
        if !sources.iter().any(|existing| existing.name == source.name) {
            sources.push(source);
        }
    }
    sources
}

pub fn omt_sources() -> Vec<NetworkSource> {
    let mut sources = sources_from_env("FREESHOW_OMT_SOURCES", SourceKind::Omt);
    for source in sources_from_factory("omtsrc")
        .into_iter()
        .chain(sources_from_factory("omtinput"))
    {
        if !sources.iter().any(|existing| existing.name == source.name) {
            sources.push(source);
        }
    }
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

fn sources_from_factory(factory: &str) -> Vec<NetworkSource> {
    let kind = if factory.starts_with("ndi") {
        SourceKind::Ndi
    } else {
        SourceKind::Omt
    };
    if !element_available(factory) {
        return Vec::new();
    }
    if gstreamer::init().is_err() {
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
            || class.contains("omt")
            || name.to_ascii_lowercase().contains("ndi")
            || name.to_ascii_lowercase().contains("omt")
            || factory_matches(&device, factory);
        if looks
            && !sources
                .iter()
                .any(|source: &NetworkSource| source.name == name)
        {
            sources.push(NetworkSource { name, kind });
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
    fn source_lists_parse_and_stay_empty_without_a_plugin() {
        let listed = parse_source_list("Cam 1, Lobby\n ", SourceKind::Ndi);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Cam 1");
        if !ndi_receive_available() {
            // DeviceMonitor has nothing to find; the explicit list still works.
            assert!(sources_from_factory("ndisrc").is_empty());
        }
        let _ = ndi_sources();
        let _ = omt_sources();
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
