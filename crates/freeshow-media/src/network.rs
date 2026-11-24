// SPDX-License-Identifier: GPL-3.0-only

/// Each live source owns a separate inter-pipeline channel. Consumers mix the
/// audio file and video tracks for a single FreeShow output ID.
pub fn network_channel(output: &str, video: bool) -> String {
    use std::fmt::Write;
    // IDs cross a C-string boundary in GStreamer. Hex keeps arbitrary IDs
    // distinct, including quotes, separators, and embedded NUL bytes.
    let mut channel = format!("freeshow-rust-{}-", if video { "video" } else { "audio" });
    for byte in output.bytes() {
        let _ = write!(channel, "{byte:02x}");
    }
    channel
}

/// GStreamer fragment with a stereo output, ready to link to an encoder or NDI.
pub fn network_audio_mix(output: &str) -> String {
    let channel = |video| network_channel(output, video);
    let input = |video| {
        format!(
            "interaudiosrc channel=\"{}\" period-time=10000000 buffer-time=100000000 latency-time=20000000 ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! network_mix. ",
            channel(video)
        )
    };
    format!(
        "{}{} audiomixer name=network_mix ! audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=2",
        input(false),
        input(true)
    )
}

#[cfg(feature = "gst")]
mod source {
    use crate::{AudioPlaybackOptions, MediaError, Pcm, Playback};
    use freeshow_core::NetworkRoute;
    use gst::prelude::*;
    use gstreamer as gst;
    use gstreamer_app::{AppSrc, AppSrcCallbacks};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn network_gain(gain: f32) -> f64 {
        if gain.is_finite() {
            f64::from(gain.clamp(0.0, 1.0))
        } else {
            0.0
        }
    }

    fn audio_error(error: impl std::fmt::Display) -> MediaError {
        MediaError::Audio(error.to_string())
    }

    pub(crate) struct NetworkBranches {
        parent: gst::Bin,
        tee: gst::Element,
        video: bool,
        gains: Vec<(String, gst::Element)>,
    }

    impl NetworkBranches {
        pub fn new(parent: &gst::Bin, tee: gst::Element, video: bool) -> Self {
            Self {
                parent: parent.clone(),
                tee,
                video,
                gains: Vec::new(),
            }
        }

        pub fn set_routes(&mut self, routes: &[NetworkRoute]) -> Result<(), MediaError> {
            // Keep muted branches until the clip closes so toggling a destination does not relink it.
            for (id, volume) in &self.gains {
                let gain = routes
                    .iter()
                    .find(|route| &route.output_id == id)
                    .map_or(0.0, |route| network_gain(route.gain));
                volume.set_property("volume", gain);
            }
            for route in routes {
                if !self.gains.iter().any(|(id, _)| *id == route.output_id) {
                    let volume = self.connect(route)?;
                    self.gains.push((route.output_id.clone(), volume));
                }
            }
            Ok(())
        }

        fn connect(&self, route: &NetworkRoute) -> Result<gst::Element, MediaError> {
            let bin = gst::Bin::new();
            let queue = gst::ElementFactory::make("queue")
                .property("max-size-buffers", 2_u32)
                .build()
                .map_err(audio_error)?;
            let volume = gst::ElementFactory::make("volume")
                .property("volume", network_gain(route.gain))
                .build()
                .map_err(audio_error)?;
            let sink = gst::ElementFactory::make("interaudiosink")
                .property(
                    "channel",
                    super::network_channel(&route.output_id, self.video),
                )
                .property("async", false)
                .build()
                .map_err(audio_error)?;
            bin.add_many([&queue, &volume, &sink])
                .map_err(audio_error)?;
            if self.video {
                let convert = gst::ElementFactory::make("audioconvert")
                    .build()
                    .map_err(audio_error)?;
                let resample = gst::ElementFactory::make("audioresample")
                    .build()
                    .map_err(audio_error)?;
                bin.add_many([&convert, &resample]).map_err(audio_error)?;
                gst::Element::link_many([&queue, &convert, &resample, &volume, &sink])
                    .map_err(audio_error)?;
            } else {
                gst::Element::link_many([&queue, &volume, &sink]).map_err(audio_error)?;
            }
            let queue_sink = queue.static_pad("sink").expect("queue sink");
            let input = gst::GhostPad::with_target(&queue_sink).map_err(audio_error)?;
            bin.add_pad(&input).map_err(audio_error)?;
            let output = self
                .tee
                .request_pad_simple("src_%u")
                .ok_or_else(|| MediaError::Audio("could not request a network audio pad".into()))?;
            // The tee must not push into the new branch before its elements are running.
            let probe = output.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, |_, _| {
                gst::PadProbeReturn::Ok
            });
            let attached = (|| {
                self.parent.add(&bin).map_err(audio_error)?;
                output.link(&input).map_err(audio_error)?;
                bin.sync_state_with_parent().map_err(audio_error)
            })();
            if attached.is_err() {
                if output.is_linked() {
                    let _ = output.unlink(&input);
                }
                let _ = bin.set_state(gst::State::Null);
                let _ = self.parent.remove(&bin);
            }
            if let Some(probe) = probe {
                output.remove_probe(probe);
            }
            if attached.is_err() {
                self.tee.release_request_pad(&output);
            }
            attached.map(|()| volume)
        }
    }

    pub(crate) struct NetworkAudio {
        pipeline: gst::Pipeline,
        pub playback: Arc<Mutex<Playback>>,
        branches: Mutex<NetworkBranches>,
    }

    impl NetworkAudio {
        #[cfg(test)]
        pub fn new(pcm: Arc<Pcm>, routes: &[NetworkRoute]) -> Result<Self, MediaError> {
            Self::configured(pcm, routes, &AudioPlaybackOptions::default())
        }

        pub fn configured(
            pcm: Arc<Pcm>,
            routes: &[NetworkRoute],
            options: &AudioPlaybackOptions,
        ) -> Result<Self, MediaError> {
            let mut initial = Playback::shared(pcm, 1.0);
            options.apply(&mut initial);
            Self::from_playback(initial, routes)
        }

        pub fn from_playback(
            initial: Playback,
            routes: &[NetworkRoute],
        ) -> Result<Self, MediaError> {
            gst::init().map_err(audio_error)?;
            let pipeline = gst::Pipeline::new();
            let paused = initial.is_paused();
            let src = AppSrc::builder()
                .name("audio_source")
                .is_live(true)
                .format(gst::Format::Time)
                .caps(
                    &gst::Caps::builder("audio/x-raw")
                        .field("format", "F32LE")
                        .field("rate", 48000_i32)
                        .field("channels", 2_i32)
                        .field("layout", "interleaved")
                        .build(),
                )
                .build();
            src.set_property("max-bytes", 15360_u64);
            let tee = gst::ElementFactory::make("tee")
                .build()
                .map_err(audio_error)?;
            pipeline
                .add_many([src.upcast_ref::<gst::Element>(), &tee])
                .map_err(audio_error)?;
            src.link(&tee).map_err(audio_error)?;
            if routes.is_empty() {
                let sink = gst::ElementFactory::make("fakesink")
                    .property("sync", true)
                    .build()
                    .map_err(audio_error)?;
                pipeline.add(&sink).map_err(audio_error)?;
                tee.link(&sink).map_err(audio_error)?;
            }
            let mut branches = NetworkBranches::new(pipeline.upcast_ref(), tee, false);
            branches.set_routes(routes)?;
            let playback = Arc::new(Mutex::new(initial));
            let state = Arc::clone(&playback);
            let frames = AtomicU64::new(0);
            src.set_callbacks(
                AppSrcCallbacks::builder()
                    .need_data(move |src, _| {
                        let mut samples = vec![0.0_f32; 960 * 2];
                        if let Ok(mut playback) = state.lock() {
                            playback.fill(&mut samples, 48000, 2);
                        };
                        let frame = frames.fetch_add(960, Ordering::Relaxed);
                        let bytes: Vec<u8> =
                            samples.into_iter().flat_map(f32::to_le_bytes).collect();
                        let mut buffer = gst::Buffer::from_mut_slice(bytes);
                        if let Some(buffer) = buffer.get_mut() {
                            buffer.set_pts(gst::ClockTime::from_nseconds(
                                frame * 1_000_000_000 / 48000,
                            ));
                            buffer.set_duration(gst::ClockTime::from_mseconds(20));
                        }
                        let _ = src.push_buffer(buffer);
                        // Keep the feed alive with silence at EOF. A later seek, loop,
                        // or resume must work without recreating the network source.
                    })
                    .build(),
            );
            pipeline
                .set_state(if paused {
                    gst::State::Paused
                } else {
                    gst::State::Playing
                })
                .map_err(audio_error)?;
            Ok(Self {
                pipeline,
                playback,
                branches: Mutex::new(branches),
            })
        }

        pub fn set_routes(&self, routes: &[NetworkRoute]) {
            if let Ok(mut branches) = self.branches.lock()
                && let Err(error) = branches.set_routes(routes)
            {
                eprintln!("network audio routing: {error}");
            }
        }

        pub fn set_paused(&self, paused: bool) {
            if let Ok(mut playback) = self.playback.lock() {
                playback.set_paused(paused);
            }
            let _ = self.pipeline.set_state(if paused {
                gst::State::Paused
            } else {
                gst::State::Playing
            });
        }
    }

    impl Drop for NetworkAudio {
        fn drop(&mut self) {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }
}

#[cfg(feature = "gst")]
pub(crate) use source::{NetworkAudio, NetworkBranches};

#[cfg(all(test, feature = "gst"))]
mod tests {
    use super::*;
    use crate::Pcm;
    use freeshow_core::NetworkRoute;
    use gst::prelude::*;
    use gstreamer as gst;
    use gstreamer_app::AppSink;
    use std::sync::Arc;

    fn receiver(id: &str) -> (gst::Pipeline, AppSink) {
        let pipeline = gst::parse::launch(&format!(
            "{} ! appsink name=capture sync=false max-buffers=4 drop=true",
            network_audio_mix(id)
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let sink = pipeline
            .by_name("capture")
            .unwrap()
            .dynamic_cast::<AppSink>()
            .unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        (pipeline, sink)
    }

    fn amplitude(sink: &AppSink, expected: f32) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let mut last = 0.0;
        while std::time::Instant::now() < end {
            if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
                let map = sample.buffer().unwrap().map_readable().unwrap();
                last = map
                    .as_slice()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| f32::from_le_bytes(*bytes).abs())
                    .fold(0.0_f32, f32::max);
                if (last - expected).abs() < 0.01 {
                    return;
                }
            }
        }
        panic!("network amplitude expected {expected}, got {last}");
    }

    #[test]
    fn late_network_feed_keeps_the_paused_playhead() {
        gst::init().unwrap();
        let id = format!("late-network-source-{}", std::process::id());
        let (receiver, sink) = receiver(&id);
        let mut samples = vec![0.1; 48000];
        samples.extend(vec![0.6; 48000 * 3]);
        let mut playback = crate::Playback::new(
            Pcm {
                rate: 48000,
                channels: 1,
                samples,
            },
            1.0,
        );
        playback.seek_seconds(1.25);
        playback.set_paused(true);
        let source = source::NetworkAudio::from_playback(
            playback,
            &[NetworkRoute {
                output_id: id,
                gain: 0.5,
            }],
        )
        .unwrap();
        amplitude(&sink, 0.0);
        assert_eq!(source.playback.lock().unwrap().position_seconds(), 1.25);
        source.set_paused(false);
        amplitude(&sink, 0.3);
        drop(source);
        receiver.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn pcm_feeds_the_selected_network_bus_and_mute_and_pause_do_not_leak() {
        gst::init().unwrap();
        let id = format!("network-source-\"quoted\"\\-\0{}", std::process::id());
        let (pipeline, sink) = receiver(&id);
        let (other_pipeline, other) = receiver(&format!("{id}-other"));
        let pcm = Arc::new(Pcm {
            rate: 48000,
            channels: 1,
            samples: vec![0.8; 48000 * 3],
        });
        let source = source::NetworkAudio::new(pcm, &[]).unwrap();
        source.playback.lock().unwrap().set_looping(true);
        amplitude(&sink, 0.0);
        source.set_routes(&[NetworkRoute {
            output_id: id.clone(),
            gain: 0.25,
        }]);
        amplitude(&sink, 0.2);
        amplitude(&other, 0.0);
        source.set_routes(&[NetworkRoute {
            output_id: id.clone(),
            gain: 0.5,
        }]);
        amplitude(&sink, 0.4);
        source.set_paused(true);
        amplitude(&sink, 0.0);
        let position = source.playback.lock().unwrap().position_seconds();
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert_eq!(source.playback.lock().unwrap().position_seconds(), position);
        source.set_paused(false);
        amplitude(&sink, 0.4);
        source.set_routes(&[
            NetworkRoute {
                output_id: id.clone(),
                gain: 0.5,
            },
            NetworkRoute {
                output_id: format!("{id}-other"),
                gain: 0.25,
            },
        ]);
        amplitude(&sink, 0.4);
        amplitude(&other, 0.2);
        source.set_routes(&[NetworkRoute {
            output_id: id.clone(),
            gain: f32::NAN,
        }]);
        amplitude(&sink, 0.0);
        source.set_routes(&[]);
        amplitude(&sink, 0.0);
        amplitude(&other, 0.0);
        drop(source);
        pipeline.set_state(gst::State::Null).unwrap();
        other_pipeline.set_state(gst::State::Null).unwrap();
    }
}
