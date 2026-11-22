// SPDX-License-Identifier: GPL-3.0-only

use freeshow_core::{NetworkRoute, SpeakerRoute};
use std::path::Path;

use freeshow_output::Raster;

use crate::MediaError;

#[cfg(feature = "gst")]
mod gst_player {
    use super::*;
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer_app::AppSink;

    use freeshow_output::{OUTPUT_HEIGHT, OUTPUT_WIDTH};

    pub struct VideoPlayer {
        pipeline: gst::Pipeline,
        sink: AppSink,
        audio: AudioBranch,
        looping: std::cell::Cell<bool>,
        rewound: std::cell::Cell<bool>,
        rate: std::cell::Cell<f64>,
        muted: std::cell::Cell<bool>,
        gain: std::cell::Cell<f64>,
        start: std::cell::Cell<f64>,
        end: std::cell::Cell<f64>,
    }

    impl VideoPlayer {
        pub fn open(path: &Path) -> Result<Self, MediaError> {
            Self::open_routed(path, &[SpeakerRoute::stereo(None, 1.0)])
        }

        pub fn open_routed(path: &Path, routes: &[SpeakerRoute]) -> Result<Self, MediaError> {
            Self::open_with_network(path, routes, &[])
        }

        pub fn open_with_network(
            path: &Path,
            routes: &[SpeakerRoute],
            network: &[NetworkRoute],
        ) -> Result<Self, MediaError> {
            let location = path
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            Self::from_launch_network(
                &format!(
                    "filesrc location=\"{location}\" ! decodebin name=dec ! videoconvert ! {} ! appsink name=sink max-buffers=1 drop=true sync=true",
                    rgba_caps()
                ),
                routes,
                network,
            )
        }

        pub fn open_camera(name: &str) -> Result<Self, MediaError> {
            let device = camera_devices()?
                .into_iter()
                .find(|device| device.display_name() == name)
                .ok_or_else(|| video_error(format!("no camera named {name}")))?;
            Self::from_camera_source(device.create_element(None).map_err(video_error)?)
        }

        fn from_camera_source(source: gst::Element) -> Result<Self, MediaError> {
            // Camera frames are read into CPU pixels without a graphics context.
            let pipeline = gst::parse::launch(&format!(
                "decodebin name=dec force-sw-decoders=true ! videoconvert ! {} ! appsink name=sink max-buffers=1 drop=true sync=false",
                rgba_caps()
            ))
            .map_err(video_error)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| video_error("camera launch did not return a pipeline"))?;
            pipeline.add(&source).map_err(video_error)?;
            source
                .link(&pipeline.by_name("dec").expect("camera decoder exists"))
                .map_err(video_error)?;
            let player = Self::from_pipeline(pipeline, AudioBranch::new_targets(Vec::new())?)?;
            player.set_looping(false);
            Ok(player)
        }

        pub fn from_launch(description: &str) -> Result<Self, MediaError> {
            Self::from_launch_routed(description, &[SpeakerRoute::stereo(None, 1.0)])
        }

        fn from_launch_routed(
            description: &str,
            routes: &[SpeakerRoute],
        ) -> Result<Self, MediaError> {
            Self::from_launch_network(description, routes, &[])
        }

        fn from_launch_network(
            description: &str,
            routes: &[SpeakerRoute],
            network: &[NetworkRoute],
        ) -> Result<Self, MediaError> {
            gst::init().map_err(video_error)?;
            let mut groups = std::collections::BTreeMap::<Option<String>, Vec<SpeakerRoute>>::new();
            for route in routes {
                groups
                    .entry(route.device.clone())
                    .or_default()
                    .push(route.clone());
            }
            let mut sinks = Vec::new();
            for (device, routes) in groups {
                sinks.push((routes, speaker_sink(device.as_deref())?, false));
            }
            for route in network {
                let sink = gst::ElementFactory::make("interaudiosink")
                    .property("channel", crate::network_channel(&route.output_id, true))
                    .build()
                    .map_err(video_error)?;
                sinks.push((
                    vec![SpeakerRoute::stereo(
                        Some(route.output_id.clone()),
                        route.gain,
                    )],
                    sink,
                    true,
                ));
            }
            Self::from_launch_with_audio(description, AudioBranch::new_targets(sinks)?)
        }

        fn from_launch_with_audio(
            description: &str,
            audio: AudioBranch,
        ) -> Result<Self, MediaError> {
            let element = gst::parse::launch(description)
                .map_err(|err| MediaError::Video(err.to_string()))?;
            let pipeline = element
                .downcast::<gst::Pipeline>()
                .map_err(|_| MediaError::Video("launch did not return a pipeline".into()))?;
            Self::from_pipeline(pipeline, audio)
        }

        fn from_pipeline(pipeline: gst::Pipeline, audio: AudioBranch) -> Result<Self, MediaError> {
            let appsink = pipeline
                .by_name("sink")
                .ok_or_else(|| MediaError::Video("appsink is missing".into()))?;
            let sink = appsink
                .dynamic_cast::<AppSink>()
                .map_err(|_| MediaError::Video("sink is not an appsink".into()))?;
            attach_audio(&pipeline, &audio);
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|err| MediaError::Video(err.to_string()))?;
            Ok(Self {
                pipeline,
                sink,
                audio,
                looping: std::cell::Cell::new(true),
                // Appsink can report EOS before asynchronous startup finishes.
                // Wait for an initial frame before trying to loop the source.
                rewound: std::cell::Cell::new(true),
                rate: std::cell::Cell::new(1.0),
                muted: std::cell::Cell::new(false),
                gain: std::cell::Cell::new(1.0),
                start: std::cell::Cell::new(0.0),
                end: std::cell::Cell::new(0.0),
            })
        }

        pub fn set_looping(&self, looping: bool) {
            self.looping.set(looping);
        }

        pub fn set_muted(&self, muted: bool) {
            self.muted.set(muted);
            self.apply_gain();
        }

        pub fn set_volume(&self, volume: f64) {
            self.gain.set(volume.clamp(0.0, 1.0));
            self.apply_gain();
        }

        fn apply_gain(&self) {
            let level = if self.muted.get() {
                0.0
            } else {
                self.gain.get()
            };
            // This element exists before decodebin discovers the audio pad, so
            // controls set during startup are retained when audio is linked.
            self.audio.volume.set_property("volume", level);
        }

        pub fn set_audio_routes(&self, routes: &[SpeakerRoute]) {
            if let Ok(mut mix) = self.audio.mix.lock() {
                mix.set_routes(routes);
            }
        }

        pub fn set_network_routes(&self, routes: &[NetworkRoute]) {
            if let Ok(mut mix) = self.audio.mix.lock() {
                mix.set_network_routes(routes);
            }
        }

        pub fn output_volume(&self) -> Option<f64> {
            self.pipeline
                .by_name("vol")
                .map(|volume| volume.property("volume"))
        }

        pub fn set_paused(&self, paused: bool) {
            let state = if paused {
                gst::State::Paused
            } else {
                gst::State::Playing
            };
            let _ = self.pipeline.set_state(state);
        }

        pub fn position_seconds(&self) -> f64 {
            clock_seconds(self.pipeline.query_position::<gst::ClockTime>())
        }

        pub fn duration_seconds(&self) -> f64 {
            clock_seconds(self.pipeline.query_duration::<gst::ClockTime>())
        }

        pub fn set_rate(&self, rate: f64) {
            self.rate.set(if rate > 0.0 { rate } else { 1.0 });
            let _ = self.seek_seconds(self.position_seconds());
        }

        /// Plays from `from_seconds`. A later `to_seconds` stops there; zero plays through the file.
        pub fn set_span(&self, from_seconds: f64, to_seconds: f64) {
            let from = from_seconds.max(0.0);
            self.start.set(from);
            self.end
                .set(if to_seconds > from { to_seconds } else { 0.0 });
            let _ = self.seek_seconds(from);
        }

        pub fn seek_seconds(&self, seconds: f64) -> bool {
            let from = seconds.max(0.0);
            let nanos = (from * 1_000_000_000.0) as u64;
            let stop = self.end.get();
            let (stop_type, stop_at) = if stop > from {
                (
                    gst::SeekType::Set,
                    gst::ClockTime::from_nseconds((stop * 1_000_000_000.0) as u64),
                )
            } else {
                (gst::SeekType::End, gst::ClockTime::ZERO)
            };
            self.pipeline
                .seek(
                    self.rate.get(),
                    gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                    gst::SeekType::Set,
                    gst::ClockTime::from_nseconds(nanos),
                    stop_type,
                    stop_at,
                )
                .is_ok()
        }

        pub fn next_frame(&mut self) -> Option<Raster> {
            self.poll_frame(gst::ClockTime::from_mseconds(40))
        }

        /// Poll a secondary preview without waiting on the caller's event loop.
        pub fn try_next_frame(&mut self) -> Option<Raster> {
            self.poll_frame(gst::ClockTime::ZERO)
        }

        fn poll_frame(&mut self, wait: gst::ClockTime) -> Option<Raster> {
            if let Some(frame) = self.pull(wait) {
                self.rewound.set(false);
                return Some(frame);
            }
            if self.looping.get()
                && self.sink.is_eos()
                && !self.rewound.get()
                && self.seek_seconds(self.start.get())
            {
                self.rewound.set(true);
                return self.pull(wait);
            }
            None
        }

        /// The clip has reached the end and is not set to repeat.
        pub fn is_finished(&self) -> bool {
            !self.looping.get() && self.sink.is_eos()
        }

        fn pull(&mut self, wait: gst::ClockTime) -> Option<Raster> {
            let sample = self.sink.try_pull_sample(wait)?;
            let buffer = sample.buffer()?;
            let caps = sample.caps()?;
            let structure = caps.structure(0)?;
            let width = structure.get::<i32>("width").ok()? as u32;
            let height = structure.get::<i32>("height").ok()? as u32;
            let map = buffer.map_readable().ok()?;
            let row_bytes = width as usize * 4;
            let data = map.as_slice();
            if width == 0 || height == 0 || data.len() < row_bytes * height as usize {
                return None;
            }
            let mut rgba = Vec::with_capacity(row_bytes * height as usize);
            for row in 0..height as usize {
                let start = row * row_bytes;
                rgba.extend_from_slice(&data[start..start + row_bytes]);
            }
            Some(Raster {
                width,
                height,
                rgba,
            })
        }
    }

    impl Drop for VideoPlayer {
        fn drop(&mut self) {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }

    fn camera_devices() -> Result<Vec<gst::Device>, MediaError> {
        gst::init().map_err(video_error)?;
        let monitor = gst::DeviceMonitor::new();
        monitor
            .add_filter(Some("Video/Source"), None)
            .ok_or_else(|| video_error("could not enumerate cameras"))?;
        monitor.start().map_err(video_error)?;
        let devices = monitor.devices();
        let _ = monitor.stop();
        Ok(devices.into_iter().collect())
    }

    pub fn cameras() -> Vec<String> {
        camera_devices()
            .unwrap_or_default()
            .iter()
            .map(|device| device.display_name().to_string())
            .collect()
    }

    fn video_error(error: impl std::fmt::Display) -> MediaError {
        MediaError::Video(error.to_string())
    }

    fn speaker_sink(device: Option<&str>) -> Result<gst::Element, MediaError> {
        let Some(name) = device else {
            return gst::ElementFactory::make("autoaudiosink")
                .build()
                .map_err(video_error);
        };
        #[cfg(target_os = "macos")]
        {
            let (id, _) = macos_speaker_devices()?
                .into_iter()
                .find(|(_, label)| label == name)
                .ok_or_else(|| video_error(format!("no video audio output device named {name}")))?;
            gst::ElementFactory::make("osxaudiosink")
                .property("device", i32::try_from(id).map_err(video_error)?)
                .build()
                .map_err(video_error)
        }
        #[cfg(not(target_os = "macos"))]
        {
            speaker_devices()?
                .into_iter()
                .find(|device| device.display_name() == name)
                .ok_or_else(|| video_error(format!("no video audio output device named {name}")))?
                .create_element(None)
                .map_err(video_error)
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_speaker_devices() -> Result<Vec<(u32, String)>, MediaError> {
        use coreaudio::audio_unit::{Scope, macos_helpers};
        // GStreamer's macOS provider can block discovery in a headless process.
        // Query output metadata through the CoreAudio bindings CPAL already
        // uses, then pass the native device ID to osxaudiosink.
        let mut devices = Vec::new();
        for id in macos_helpers::get_audio_device_ids().map_err(video_error)? {
            if macos_helpers::get_audio_device_supports_scope(id, Scope::Output).unwrap_or(false) {
                if let Ok(name) = macos_helpers::get_device_name(id) {
                    devices.push((id, name));
                }
            }
        }
        Ok(devices)
    }

    #[cfg(not(target_os = "macos"))]
    fn speaker_devices() -> Result<Vec<gst::Device>, MediaError> {
        let monitor = gst::DeviceMonitor::new();
        monitor
            .add_filter(Some("Audio/Sink"), None)
            .ok_or_else(|| video_error("could not enumerate audio sinks"))?;
        monitor.start().map_err(video_error)?;
        let devices = monitor.devices();
        let _ = monitor.stop();
        Ok(devices.into_iter().collect())
    }

    pub fn video_output_devices() -> Vec<String> {
        if gst::init().is_err() {
            return Vec::new();
        }
        #[cfg(target_os = "macos")]
        {
            macos_speaker_devices()
                .unwrap_or_default()
                .into_iter()
                .map(|(_, name)| name)
                .collect()
        }
        #[cfg(not(target_os = "macos"))]
        {
            speaker_devices()
                .unwrap_or_default()
                .into_iter()
                .map(|device| device.display_name().to_string())
                .collect()
        }
    }

    struct AudioBranch {
        bin: gst::Bin,
        volume: gst::Element,
        mix: std::sync::Arc<std::sync::Mutex<ChannelMix>>,
    }

    impl AudioBranch {
        #[cfg(test)]
        fn new(sinks: Vec<(Option<String>, f32, gst::Element)>) -> Result<Self, MediaError> {
            Self::new_routed(
                sinks
                    .into_iter()
                    .map(|(device, gain, sink)| (vec![SpeakerRoute::stereo(device, gain)], sink))
                    .collect(),
            )
        }

        #[cfg(test)]
        fn new_routed(sinks: Vec<(Vec<SpeakerRoute>, gst::Element)>) -> Result<Self, MediaError> {
            Self::new_targets(
                sinks
                    .into_iter()
                    .map(|(routes, sink)| (routes, sink, false))
                    .collect(),
            )
        }

        fn new_targets(
            mut sinks: Vec<(Vec<SpeakerRoute>, gst::Element, bool)>,
        ) -> Result<Self, MediaError> {
            let bin = gst::Bin::new();
            let queue = gst::ElementFactory::make("queue")
                .build()
                .map_err(video_error)?;
            let convert = gst::ElementFactory::make("audioconvert")
                .build()
                .map_err(video_error)?;
            let volume = gst::ElementFactory::make("volume")
                .name("vol")
                .property("volume", 0.0_f64)
                .build()
                .map_err(video_error)?;
            let tee = gst::ElementFactory::make("tee")
                .build()
                .map_err(video_error)?;
            bin.add_many([&queue, &convert, &volume, &tee])
                .map_err(video_error)?;
            gst::Element::link_many([&queue, &convert, &volume, &tee]).map_err(video_error)?;
            let sinkpad = queue
                .static_pad("sink")
                .ok_or_else(|| video_error("audio queue has no sink"))?;
            let ghost = gst::GhostPad::with_target(&sinkpad).map_err(video_error)?;
            bin.add_pad(&ghost).map_err(video_error)?;
            if sinks.is_empty() {
                // Consume disconnected audio so video, seek, and end actions keep
                // running. Never substitute the default room speakers.
                sinks.push((
                    vec![SpeakerRoute::stereo(None, 0.0)],
                    gst::ElementFactory::make("fakesink")
                        .property("sync", true)
                        .build()
                        .map_err(video_error)?,
                    false,
                ));
            }
            let mut outputs = Vec::new();
            for (routes, sink, network) in sinks {
                let queue = gst::ElementFactory::make("queue")
                    .build()
                    .map_err(video_error)?;
                let upstream = queue.static_pad("src").expect("queue has a source pad");
                let normalize = gst::ElementFactory::make("audioconvert")
                    .build()
                    .map_err(video_error)?;
                let input_format = gst::ElementFactory::make("capsfilter")
                    .build()
                    .map_err(video_error)?;
                let convert = gst::ElementFactory::make("audioconvert")
                    .build()
                    .map_err(video_error)?;
                let resample = gst::ElementFactory::make("audioresample")
                    .build()
                    .map_err(video_error)?;
                let filter = gst::ElementFactory::make("capsfilter")
                    .build()
                    .map_err(video_error)?;
                let gain_element = gst::ElementFactory::make("volume")
                    .property("volume", 0.0_f64)
                    .build()
                    .map_err(video_error)?;
                bin.add_many([
                    &queue,
                    &normalize,
                    &input_format,
                    &convert,
                    &filter,
                    &resample,
                    &gain_element,
                    &sink,
                ])
                .map_err(video_error)?;
                gst::Element::link_many([
                    &tee,
                    &queue,
                    &normalize,
                    &input_format,
                    &convert,
                    &filter,
                    &resample,
                    &gain_element,
                    &sink,
                ])
                .map_err(video_error)?;
                outputs.push(ChannelOutput {
                    network,
                    device: routes[0].device.clone(),
                    routes,
                    upstream,
                    pending: Default::default(),
                    input_format,
                    convert,
                    filter,
                    volume: gain_element,
                });
            }
            let mix = ChannelMix {
                channels: None,
                outputs,
            };
            mix.apply();
            Ok(Self {
                bin,
                volume,
                mix: std::sync::Arc::new(std::sync::Mutex::new(mix)),
            })
        }
    }

    struct ChannelOutput {
        network: bool,
        device: Option<String>,
        routes: Vec<SpeakerRoute>,
        upstream: gst::Pad,
        pending: std::sync::Arc<std::sync::Mutex<Option<MixUpdate>>>,
        input_format: gst::Element,
        convert: gst::Element,
        filter: gst::Element,
        volume: gst::Element,
    }

    struct MixUpdate {
        matrix: gst::Array,
        caps: gst::Caps,
        gain: f64,
    }

    impl ChannelOutput {
        fn configure(&self, matrix: Vec<gst::Array>, caps: gst::Caps, gain: f64) {
            let active = self.upstream.is_active();
            let update = MixUpdate {
                matrix: gst::Array::new(matrix),
                caps,
                gain,
            };
            let replaced = self.pending.lock().unwrap().replace(update).is_some();
            if active && replaced {
                return;
            }
            let pending = self.pending.clone();
            let convert = self.convert.clone();
            let input_format = self.input_format.clone();
            let filter = self.filter.clone();
            let volume = self.volume.clone();
            let update = move || -> bool {
                let queued = pending.lock().unwrap().take();
                let Some(MixUpdate { matrix, caps, gain }) = queued else {
                    return true;
                };
                volume.set_property("volume", 0.0_f64);
                let was_explicit = !convert
                    .property::<gst::Array>("mix-matrix")
                    .as_slice()
                    .is_empty();
                let current_outputs = convert_channels(&convert);
                let next_outputs = caps
                    .structure(0)
                    .and_then(|structure| structure.get::<i32>("channels").ok());
                let channels_change = current_outputs != next_outputs;
                // GStreamer 1.26 rejects a mix-matrix whose row count does not
                // already match the converter's output channel count.
                if was_explicit && channels_change {
                    convert.set_property("mix-matrix", &gst::Array::new(Vec::<gst::Array>::new()));
                }
                filter.set_property("caps", &caps);
                let rebuild = was_explicit || !matrix.as_slice().is_empty();
                if rebuild && channels_change {
                    kick_convert_format(&input_format, &convert);
                }
                let current_outputs = convert_channels(&convert);
                let matrix_rows = matrix.as_slice().len() as i32;
                // Before the converter has negotiated, install the matrix first.
                // After that, wait until the output channel count matches, because
                // GStreamer 1.26 rejects a mix-matrix of the wrong height.
                let fits = matrix.as_slice().is_empty()
                    || next_outputs.is_none()
                    || current_outputs.is_none()
                    || current_outputs == Some(matrix_rows);
                if !fits {
                    pending
                        .lock()
                        .unwrap()
                        .replace(MixUpdate { matrix, caps, gain });
                    return false;
                }
                convert.set_property("mix-matrix", &matrix);
                if rebuild {
                    kick_convert_format(&input_format, &convert);
                }
                volume.set_property("volume", gain);
                true
            };
            // An IDLE probe can execute on the operator thread while holding the pad
            // lock. Negotiate an active branch from its streaming task instead.
            if active {
                self.upstream.add_probe(
                    gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                    move |_, _| {
                        if update() {
                            gst::PadProbeReturn::Remove
                        } else {
                            gst::PadProbeReturn::Ok
                        }
                    },
                );
            } else {
                let _ = update();
            }
        }
    }

    struct ChannelMix {
        channels: Option<u16>,
        outputs: Vec<ChannelOutput>,
    }

    impl ChannelMix {
        fn set_routes(&mut self, routes: &[SpeakerRoute]) {
            let mut changed = false;
            for output in self.outputs.iter_mut().filter(|output| !output.network) {
                let selected: Vec<_> = routes
                    .iter()
                    .filter(|route| route.device == output.device)
                    .cloned()
                    .collect();
                if output.routes != selected {
                    output.routes = selected;
                    changed = true;
                }
            }
            if changed {
                self.apply();
            }
        }

        fn set_network_routes(&mut self, routes: &[NetworkRoute]) {
            let mut changed = false;
            for output in self.outputs.iter_mut().filter(|output| output.network) {
                let selected: Vec<_> = routes
                    .iter()
                    .filter(|route| Some(&route.output_id) == output.device.as_ref())
                    .map(|route| SpeakerRoute::stereo(Some(route.output_id.clone()), route.gain))
                    .collect();
                if selected != output.routes {
                    output.routes = selected;
                    changed = true;
                }
            }
            if changed {
                self.apply();
            }
        }

        fn apply(&self) {
            for output in &self.outputs {
                let explicit = output
                    .routes
                    .iter()
                    .any(|route| route.input_channel.is_some() || route.output_channel.is_some());
                if !explicit {
                    let gain: f32 = output.routes.iter().map(|route| route.gain).sum();
                    output.configure(
                        Vec::new(),
                        gst::Caps::new_any(),
                        f64::from(gain.clamp(0.0, 10.0)),
                    );
                    continue;
                }
                let Some(inputs) = self.channels else {
                    output.volume.set_property("volume", 0.0_f64);
                    continue;
                };
                let full_source = output
                    .routes
                    .iter()
                    .any(|route| route.output_channel.is_none());
                let outputs = output
                    .routes
                    .iter()
                    .filter_map(|route| route.output_channel)
                    .max()
                    .map(|index| index + 1)
                    .unwrap_or(2)
                    .max(if full_source { inputs.max(2) } else { 2 });
                let matrix = freeshow_core::speaker_matrix(inputs, outputs, &output.routes);
                let rows: Vec<_> = matrix.into_iter().map(gst::Array::new).collect();
                output.configure(rows, output_caps(outputs), 1.0);
            }
        }
    }

    fn output_caps(channels: u16) -> gst::Caps {
        let caps = gst::Caps::builder("audio/x-raw").field("channels", i32::from(channels));
        let mask = match channels {
            1 => 0x0,
            2 => 0x3,
            3 => 0xb,
            4 => 0x33,
            5 => 0x37,
            6 => 0x3f,
            7 => 0x13f,
            8 => 0xc3f,
            _ => return caps.build(),
        };
        caps.field("channel-mask", gst::Bitmask::new(mask)).build()
    }

    fn convert_channels(convert: &gst::Element) -> Option<i32> {
        convert
            .static_pad("src")
            .and_then(|pad| pad.current_caps())
            .and_then(|caps| caps.structure(0)?.get::<i32>("channels").ok())
    }

    fn kick_convert_format(input_format: &gst::Element, convert: &gst::Element) {
        let current = convert
            .static_pad("sink")
            .and_then(|pad| pad.current_caps());
        let is_f32 = current
            .as_ref()
            .and_then(|caps| caps.structure(0))
            .and_then(|caps| caps.get::<&str>("format").ok())
            == Some("F32LE");
        let format = if is_f32 { "F64LE" } else { "F32LE" };
        input_format.set_property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", format)
                .build(),
        );
    }

    fn attach_audio(pipeline: &gst::Pipeline, audio: &AudioBranch) {
        let Some(decode) = pipeline.by_name("dec") else {
            return;
        };
        let pipeline = pipeline.downgrade();
        let bin = audio.bin.clone();
        let mix = std::sync::Arc::clone(&audio.mix);
        decode.connect_pad_added(move |_decode, pad| {
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            if !caps
                .structure(0)
                .is_some_and(|structure| structure.name().starts_with("audio/"))
                || bin.parent().is_some()
            {
                return;
            }
            if let Ok(mut mix) = mix.lock() {
                mix.channels = caps
                    .structure(0)
                    .and_then(|caps| caps.get::<i32>("channels").ok())
                    .filter(|channels| (1..=64).contains(channels))
                    .map(|channels| channels as u16);
                mix.apply();
            }
            let Some(pipeline) = pipeline.upgrade() else {
                return;
            };
            let linked = (|| {
                pipeline.add(&bin).map_err(video_error)?;
                let sinkpad = bin
                    .static_pad("sink")
                    .ok_or_else(|| video_error("audio bin has no sink"))?;
                pad.link(&sinkpad).map_err(video_error)?;
                bin.sync_state_with_parent().map_err(video_error)
            })();
            if let Err(error) = linked {
                eprintln!("video audio routing: {error}");
            }
        });
    }

    #[cfg(test)]
    mod audio_tests {
        use super::*;

        #[test]
        fn camera_pipeline_uses_the_selected_source_without_recreating_it() {
            gst::init().unwrap();
            for compressed in [false, true] {
                let description = if compressed {
                    "videotestsrc is-live=true pattern=red ! video/x-raw,format=I420 ! jpegenc"
                } else {
                    "videotestsrc is-live=true pattern=red"
                };
                let source = gst::parse::bin_from_description(description, true)
                    .unwrap()
                    .upcast::<gst::Element>();
                source.set_property("name", "selected-native-camera");
                let mut player = VideoPlayer::from_camera_source(source.clone()).unwrap();
                assert_eq!(
                    player.pipeline.by_name("selected-native-camera"),
                    Some(source)
                );
                assert!(!player.looping.get());
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if let Some(frame) = player.next_frame() {
                        let center =
                            ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
                        let pixel = &frame.rgba[center..center + 4];
                        // JPEG conversion can shift the color through its YUV color space.
                        if compressed {
                            assert!(
                                pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40,
                                "{pixel:?}"
                            );
                        } else {
                            assert_eq!(pixel, &[255, 0, 0, 255]);
                        }
                        assert_eq!(pixel[3], 255);
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "camera produced no frame (compressed={compressed}): {:?}",
                        player
                            .pipeline
                            .bus()
                            .unwrap()
                            .pop_filtered(&[gst::MessageType::Error])
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }

        fn capture_sink() -> AppSink {
            let sink = AppSink::builder().build();
            sink.set_caps(Some(
                &gst::Caps::builder("audio/x-raw")
                    .field("format", "F32LE")
                    .field("channels", 1_i32)
                    .field("rate", 8000_i32)
                    .field("layout", "interleaved")
                    .build(),
            ));
            sink.set_property("sync", false);
            sink.set_property("async", false);
            sink.set_property("max-buffers", 4_u32);
            sink.set_property("drop", true);
            sink
        }

        fn expect_amplitude(sink: &AppSink, expected: f32) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut last = None;
            while std::time::Instant::now() < deadline {
                if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
                    let map = sample.buffer().unwrap().map_readable().unwrap();
                    let amplitude = map
                        .as_slice()
                        .chunks_exact(4)
                        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()).abs())
                        .fold(0.0_f32, f32::max);
                    last = Some(amplitude);
                    if (amplitude - expected).abs() < 0.005 {
                        return;
                    }
                }
            }
            panic!("expected audio amplitude {expected}, received {last:?}");
        }

        fn expect_channels(sink: &AppSink, expected: &[f32]) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut last = vec![0.0_f32; expected.len()];
            while std::time::Instant::now() < deadline {
                if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
                    let channels = sample
                        .caps()
                        .and_then(|caps| caps.structure(0))
                        .and_then(|caps| caps.get::<i32>("channels").ok());
                    if channels != Some(expected.len() as i32) {
                        continue;
                    }
                    let map = sample.buffer().unwrap().map_readable().unwrap();
                    last.fill(0.0);
                    for frame in map.as_slice().chunks_exact(4 * expected.len()) {
                        for (index, value) in frame.chunks_exact(4).enumerate() {
                            last[index] = last[index]
                                .max(f32::from_le_bytes(value.try_into().unwrap()).abs());
                        }
                    }
                    if last
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() < 0.005)
                    {
                        return;
                    }
                }
            }
            panic!("expected channels {expected:?}, got {last:?}");
        }

        #[test]
        fn video_audio_reaches_only_the_named_network_output() {
            gst::init().unwrap();
            let id = format!("video-network-{}", std::process::id());
            let receiver = gst::parse::launch(&format!(
                "{} ! appsink name=capture sync=false max-buffers=4 drop=true",
                crate::network_audio_mix(&id)
            ))
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
            let sink = receiver
                .by_name("capture")
                .unwrap()
                .dynamic_cast::<AppSink>()
                .unwrap();
            receiver.set_state(gst::State::Playing).unwrap();
            let player=VideoPlayer::from_launch_network(
                "videotestsrc is-live=true ! video/x-raw,width=2,height=2 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=sink max-buffers=1 drop=true sync=false audiotestsrc is-live=true wave=square volume=0.8 samplesperbuffer=480 ! audio/x-raw,format=F32LE,channels=1,rate=48000 ! decodebin name=dec",&[],&[NetworkRoute {output_id:id.clone(),gain:0.25}]).unwrap();
            player.set_volume(1.0);
            expect_amplitude(&sink, 0.2);
            player.set_network_routes(&[NetworkRoute {
                output_id: id.clone(),
                gain: 0.5,
            }]);
            expect_amplitude(&sink, 0.4);
            player.set_muted(true);
            expect_amplitude(&sink, 0.0);
            player.set_muted(false);
            expect_amplitude(&sink, 0.4);
            player.set_network_routes(&[]);
            expect_amplitude(&sink, 0.0);
            drop(player);
            receiver.set_state(gst::State::Null).unwrap();
        }

        #[test]
        fn active_audio_branch_keeps_only_the_latest_update_until_the_next_buffer() {
            gst::init().unwrap();
            let sink = gst::ElementFactory::make("fakesink").build().unwrap();
            let audio = AudioBranch::new(vec![(None, 1.0, sink)]).unwrap();
            let config = audio.mix.lock().unwrap();
            let branch = &config.outputs[0];
            branch.upstream.set_active(true).unwrap();
            branch.configure(Vec::new(), gst::Caps::new_any(), 0.25);
            branch.configure(Vec::new(), gst::Caps::new_any(), 0.75);
            assert_eq!(branch.volume.property::<f64>("volume"), 1.0);
            assert_eq!(branch.pending.lock().unwrap().as_ref().unwrap().gain, 0.75);
            branch.upstream.set_active(false).unwrap();
        }

        #[test]
        fn video_channel_matrix_preserves_sources_and_never_leaks_to_unmapped_ports() {
            gst::init().unwrap();
            let sink = capture_sink();
            sink.set_caps(Some(
                &gst::Caps::builder("audio/x-raw")
                    .field("format", "F32LE")
                    .field("channels", gst::IntRange::<i32>::new(1, 64))
                    .field("rate", 8000_i32)
                    .field("layout", "interleaved")
                    .build(),
            ));
            let route = SpeakerRoute {
                input_channel: Some(1),
                output_channel: Some(1),
                ..SpeakerRoute::stereo(None, 0.5)
            };
            let audio = AudioBranch::new_routed(vec![(vec![route.clone()], sink.clone().upcast())])
                .unwrap();
            let player = VideoPlayer::from_launch_with_audio(
                "videotestsrc is-live=true ! video/x-raw,width=2,height=2,framerate=5/1 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=sink max-buffers=1 drop=true sync=false interleave name=stereo ! audio/x-raw,format=F32LE,channels=2,rate=8000 ! decodebin name=dec audiotestsrc is-live=true wave=square volume=0.8 samplesperbuffer=64 ! audio/x-raw,format=F32LE,channels=1,rate=8000 ! stereo.sink_0 audiotestsrc is-live=true wave=square volume=0.4 samplesperbuffer=64 ! audio/x-raw,format=F32LE,channels=1,rate=8000 ! stereo.sink_1", audio
            ).unwrap();
            player.set_volume(1.0);
            expect_channels(&sink, &[0.0, 0.2]);
            let stereo = [
                SpeakerRoute {
                    input_channel: Some(0),
                    output_channel: Some(0),
                    ..SpeakerRoute::stereo(None, 0.25)
                },
                route.clone(),
            ];
            for _ in 0..3 {
                player.set_audio_routes(&stereo);
                expect_channels(&sink, &[0.2, 0.2]);
                player.set_audio_routes(std::slice::from_ref(&route));
                expect_channels(&sink, &[0.0, 0.2]);
            }
            player.set_audio_routes(&stereo);
            expect_channels(&sink, &[0.2, 0.2]);
            player.set_muted(true);
            expect_channels(&sink, &[0.0, 0.0]);
            player.set_muted(false);
            expect_channels(&sink, &[0.2, 0.2]);
            player.set_audio_routes(&[route]);
            expect_channels(&sink, &[0.0, 0.2]);
            player.set_audio_routes(&[SpeakerRoute {
                output_channel: Some(3),
                ..SpeakerRoute::stereo(None, 0.5)
            }]);
            expect_channels(&sink, &[0.0, 0.0, 0.0, 0.3]);
        }

        #[test]
        fn decoded_audio_only_reaches_its_routed_sinks_and_controls_keep_their_gain() {
            gst::init().unwrap();
            let room = capture_sink();
            let stage = capture_sink();
            let audio = AudioBranch::new(vec![
                (None, 0.0, room.clone().upcast()),
                (Some("Stage".into()), 0.25, stage.clone().upcast()),
            ])
            .unwrap();
            let player = VideoPlayer::from_launch_with_audio(
                "videotestsrc is-live=true ! video/x-raw,width=2,height=2,framerate=5/1 ! videoconvert ! video/x-raw,format=RGBA ! appsink name=sink max-buffers=1 drop=true sync=false audiotestsrc is-live=true wave=square volume=1 samplesperbuffer=64 ! audio/x-raw,format=F32LE,channels=1,rate=8000 ! decodebin name=dec", audio
            ).unwrap();
            // Set controls immediately, before decodebin necessarily has a pad.
            player.set_muted(false);
            player.set_volume(1.0);
            expect_amplitude(&stage, 0.25);
            expect_amplitude(&room, 0.0);
            player.set_audio_routes(&[
                SpeakerRoute::stereo(None, 0.5),
                SpeakerRoute::stereo(Some("Stage".into()), 0.1),
            ]);
            player.set_volume(0.4);
            expect_amplitude(&room, 0.2);
            expect_amplitude(&stage, 0.04);
            player.set_muted(true);
            expect_amplitude(&room, 0.0);
            expect_amplitude(&stage, 0.0);
            player.set_muted(false);
            expect_amplitude(&stage, 0.04);
            player.set_audio_routes(&[]);
            expect_amplitude(&room, 0.0);
            expect_amplitude(&stage, 0.0);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn named_coreaudio_sinks_keep_the_requested_device_and_reject_unknown_names() {
            gst::init().unwrap();
            if let Some((id, name)) = macos_speaker_devices().unwrap().first() {
                let sink = speaker_sink(Some(name)).unwrap();
                assert_eq!(sink.factory().unwrap().name(), "osxaudiosink");
                assert_eq!(sink.property::<i32>("device"), i32::try_from(*id).unwrap());
            }
            assert!(speaker_sink(Some("freeshow-nonexistent-speaker-46f7c81d")).is_err());
        }

        #[test]
        fn a_disconnected_source_consumes_audio_without_opening_speakers() {
            gst::init().unwrap();
            let audio = AudioBranch::new(Vec::new()).unwrap();
            let factories: Vec<_> = audio
                .bin
                .children()
                .iter()
                .filter_map(|element| element.factory().map(|factory| factory.name().to_string()))
                .collect();
            assert!(factories.iter().any(|name| name == "fakesink"));
            assert!(!factories.iter().any(|name| name == "autoaudiosink"));
            assert_eq!(
                audio.mix.lock().unwrap().outputs[0]
                    .volume
                    .property::<f64>("volume"),
                0.0
            );
        }
    }

    fn clock_seconds(time: Option<gst::ClockTime>) -> f64 {
        time.map(|time| time.nseconds() as f64 / 1_000_000_000.0)
            .unwrap_or(0.0)
    }

    fn rgba_caps() -> String {
        format!(
            "videoscale add-borders=true ! video/x-raw,format=RGBA,width={OUTPUT_WIDTH},height={OUTPUT_HEIGHT},pixel-aspect-ratio=1/1"
        )
    }
}

#[cfg(feature = "gst")]
pub use gst_player::{VideoPlayer, cameras, video_output_devices};

#[cfg(not(feature = "gst"))]
pub struct VideoPlayer;

#[cfg(not(feature = "gst"))]
impl VideoPlayer {
    pub fn open(_path: &Path) -> Result<Self, MediaError> {
        Err(MediaError::Video(
            "video decode needs GStreamer; rebuild with the gst feature".into(),
        ))
    }

    pub fn open_routed(path: &Path, _routes: &[SpeakerRoute]) -> Result<Self, MediaError> {
        Self::open(path)
    }

    pub fn set_audio_routes(&self, _routes: &[SpeakerRoute]) {}

    pub fn open_with_network(
        path: &Path,
        routes: &[SpeakerRoute],
        _network: &[NetworkRoute],
    ) -> Result<Self, MediaError> {
        Self::open_routed(path, routes)
    }
    pub fn set_network_routes(&self, _routes: &[NetworkRoute]) {}

    pub fn open_camera(_name: &str) -> Result<Self, MediaError> {
        Err(MediaError::Video(
            "cameras need GStreamer; rebuild with the gst feature".into(),
        ))
    }

    pub fn next_frame(&mut self) -> Option<Raster> {
        None
    }

    pub fn try_next_frame(&mut self) -> Option<Raster> {
        None
    }

    pub fn is_finished(&self) -> bool {
        false
    }

    pub fn set_paused(&self, _paused: bool) {}

    pub fn set_looping(&self, _looping: bool) {}

    pub fn set_rate(&self, _rate: f64) {}

    pub fn set_muted(&self, _muted: bool) {}

    pub fn set_volume(&self, _volume: f64) {}

    pub fn set_span(&self, _from_seconds: f64, _to_seconds: f64) {}

    pub fn output_volume(&self) -> Option<f64> {
        None
    }

    pub fn position_seconds(&self) -> f64 {
        0.0
    }

    pub fn duration_seconds(&self) -> f64 {
        0.0
    }

    pub fn seek_seconds(&self, _seconds: f64) -> bool {
        false
    }
}

#[cfg(not(feature = "gst"))]
pub fn cameras() -> Vec<String> {
    Vec::new()
}

#[cfg(not(feature = "gst"))]
pub fn video_output_devices() -> Vec<String> {
    Vec::new()
}

#[cfg(all(test, feature = "gst"))]
mod tests {
    use gstreamer as gst;
    use gstreamer::prelude::*;

    use super::*;

    fn wait_for_frame(player: &mut VideoPlayer) -> Option<Raster> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(frame) = player.next_frame() {
                return Some(frame);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_wide_frame_is_letterboxed() {
        let mut player = VideoPlayer::from_launch(
            "videotestsrc num-buffers=1 pattern=red ! video/x-raw,width=8,height=2 ! videoconvert ! videoscale add-borders=true ! video/x-raw,format=RGBA,width=8,height=8,pixel-aspect-ratio=1/1 ! appsink name=sink sync=false",
        )
        .expect("letterbox pipeline");
        let frame = wait_for_frame(&mut player).expect("letterboxed frame");
        let corner = &frame.rgba[0..4];
        assert_eq!(corner, &[0, 0, 0, 255], "the bars stay black");
        let middle = 4 * (4 * 8);
        assert_eq!(&frame.rgba[middle..middle + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn videotestsrc_frame_is_red() {
        let mut player = VideoPlayer::from_launch(
            "videotestsrc num-buffers=1 pattern=red ! videoconvert ! video/x-raw,format=RGBA,width=16,height=16 ! appsink name=sink sync=false",
        )
        .expect("videotestsrc pipeline");
        let frame = wait_for_frame(&mut player).expect("one red frame");
        assert_eq!(frame.width, 16);
        assert_eq!(frame.rgba[0], 255);
        assert_eq!(frame.rgba[1], 0);
        assert_eq!(frame.rgba[2], 0);
    }

    #[test]
    fn a_test_video_can_seek_to_the_start() {
        let player = VideoPlayer::from_launch(
            "videotestsrc num-buffers=30 ! videoconvert ! video/x-raw,format=RGBA,width=16,height=16 ! appsink name=sink sync=false",
        )
        .expect("videotestsrc pipeline");
        assert!(player.seek_seconds(0.0));
    }

    #[test]
    fn a_finished_clip_plays_again_when_looping() {
        gst::init().expect("gstreamer");
        let path =
            std::env::temp_dir().join(format!("freeshow-rust-loop-{}.mkv", std::process::id()));
        let location = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "");
        let writer = gst::parse::launch(&format!(
            "videotestsrc num-buffers=3 pattern=red ! videoconvert ! avenc_mjpeg ! matroskamux ! filesink location=\"{location}\""
        ))
        .expect("clip writer");
        writer.set_state(gst::State::Playing).expect("start writer");
        let bus = writer.bus().expect("writer bus");
        let wrote = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .map(|message| format!("{:?}", message.type_()))
            .unwrap_or_else(|| "no bus message".into());
        let _ = writer.set_state(gst::State::Null);
        let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let mut player = VideoPlayer::open(&path).expect("open clip");
        let mut frames = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if player.next_frame().is_some() {
                frames += 1;
                if frames >= 4 && player.duration_seconds() > 0.0 {
                    break;
                }
            } else {
                // A pending seek or decoder startup can return immediately.
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let duration = player.duration_seconds();
        let _ = std::fs::remove_file(&path);
        assert!(duration > 0.0, "the clip reports its length");
        assert!(player.output_volume().is_none());
        assert!(
            frames >= 4,
            "a three-frame clip should restart when looping, saw {frames}, file {bytes} bytes, writer {wrote}"
        );
        player.set_span(0.0, duration / 2.0);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let position = player.position_seconds();
        assert!(
            position < duration,
            "playback returns to the start of the stored span, at {position} of {duration}"
        );
    }

    #[test]
    fn a_clip_with_sound_starts_silent() {
        gst::init().expect("gstreamer");
        let path =
            std::env::temp_dir().join(format!("freeshow-rust-mute-{}.wav", std::process::id()));
        let location = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "");
        let writer = gst::parse::launch(&format!(
            "audiotestsrc num-buffers=8 ! audioconvert ! wavenc ! filesink location=\"{location}\""
        ))
        .expect("tone writer");
        writer.set_state(gst::State::Playing).expect("start writer");
        let bus = writer.bus().expect("writer bus");
        let _ = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let _ = writer.set_state(gst::State::Null);
        let player = VideoPlayer::open(&path).expect("open tone");
        std::thread::sleep(std::time::Duration::from_millis(400));
        let _ = std::fs::remove_file(&path);
        assert_eq!(player.output_volume(), Some(0.0));
        player.set_muted(false);
        assert_eq!(player.output_volume(), Some(1.0));
        player.set_volume(0.4);
        let heard = player.output_volume().unwrap_or(0.0);
        assert!((heard - 0.4).abs() < 0.05);
        player.set_muted(true);
        assert_eq!(player.output_volume(), Some(0.0));
        player.set_muted(false);
        let restored = player.output_volume().unwrap_or(0.0);
        assert!((restored - 0.4).abs() < 0.05);
    }
}
