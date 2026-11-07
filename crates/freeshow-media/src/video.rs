// SPDX-License-Identifier: GPL-3.0-only

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
            let location = path.to_string_lossy().replace('"', "\\\"");
            Self::from_launch(&format!(
                "filesrc location=\"{location}\" ! decodebin name=dec ! videoconvert ! {} ! appsink name=sink max-buffers=1 drop=true sync=true",
                rgba_caps()
            ))
        }

        pub fn open_camera(name: &str) -> Result<Self, MediaError> {
            let index = cameras()
                .iter()
                .position(|camera| camera == name)
                .unwrap_or(0);
            let source = if cfg!(target_os = "macos") {
                format!("avfvideosrc device-index={index}")
            } else if cfg!(windows) {
                format!("ksvideosrc device-index={index}")
            } else {
                format!("v4l2src device=/dev/video{index}")
            };
            Self::from_launch(&format!(
                "{source} ! videoconvert ! {} ! appsink name=sink max-buffers=1 drop=true sync=false",
                rgba_caps()
            ))
        }

        pub fn from_launch(description: &str) -> Result<Self, MediaError> {
            gst::init().map_err(|err| MediaError::Video(err.to_string()))?;
            let element = gst::parse::launch(description)
                .map_err(|err| MediaError::Video(err.to_string()))?;
            let pipeline = element
                .downcast::<gst::Pipeline>()
                .map_err(|_| MediaError::Video("launch did not return a pipeline".into()))?;
            let appsink = pipeline
                .by_name("sink")
                .ok_or_else(|| MediaError::Video("appsink is missing".into()))?;
            let sink = appsink
                .dynamic_cast::<AppSink>()
                .map_err(|_| MediaError::Video("sink is not an appsink".into()))?;
            attach_audio(&pipeline);
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|err| MediaError::Video(err.to_string()))?;
            Ok(Self {
                pipeline,
                sink,
                looping: std::cell::Cell::new(true),
                rewound: std::cell::Cell::new(false),
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
            let Some(volume) = self.pipeline.by_name("vol") else {
                return;
            };
            let level = if self.muted.get() { 0.0 } else { self.gain.get() };
            volume.set_property("volume", level);
        }

        pub fn output_volume(&self) -> Option<f64> {
            self.pipeline.by_name("vol").map(|volume| volume.property("volume"))
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
            self.end.set(if to_seconds > from { to_seconds } else { 0.0 });
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
            if let Some(frame) = self.pull() {
                self.rewound.set(false);
                return Some(frame);
            }
            if self.looping.get()
                && self.sink.is_eos()
                && !self.rewound.get()
                && self.seek_seconds(self.start.get())
            {
                self.rewound.set(true);
                return self.pull();
            }
            None
        }

        /// The clip has reached the end and is not set to repeat.
        pub fn is_finished(&self) -> bool {
            !self.looping.get() && self.sink.is_eos()
        }

        fn pull(&mut self) -> Option<Raster> {
            let sample = self
                .sink
                .try_pull_sample(gst::ClockTime::from_mseconds(40))?;
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

    pub fn cameras() -> Vec<String> {
        if gst::init().is_err() {
            return Vec::new();
        }
        let monitor = gst::DeviceMonitor::new();
        if monitor.add_filter(Some("Video/Source"), None).is_none() || monitor.start().is_err() {
            return Vec::new();
        }
        let names = monitor
            .devices()
            .iter()
            .map(|device| device.display_name().to_string())
            .collect();
        let _ = monitor.stop();
        names
    }

    fn attach_audio(pipeline: &gst::Pipeline) {
        let Some(decode) = pipeline.by_name("dec") else {
            return;
        };
        let pipeline = pipeline.clone();
        decode.connect_pad_added(move |_decode, pad| {
            let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
            let Some(structure) = caps.structure(0) else {
                return;
            };
            if !structure.name().starts_with("audio/") || pipeline.by_name("vol").is_some() {
                return;
            }
            let Ok(queue) = gst::ElementFactory::make("queue").build() else {
                return;
            };
            let Ok(convert) = gst::ElementFactory::make("audioconvert").build() else {
                return;
            };
            let Ok(resample) = gst::ElementFactory::make("audioresample").build() else {
                return;
            };
            let Ok(volume) = gst::ElementFactory::make("volume")
                .name("vol")
                .property("volume", 0.0_f64)
                .build()
            else {
                return;
            };
            let Ok(sink) = gst::ElementFactory::make("autoaudiosink").build() else {
                return;
            };
            if pipeline
                .add_many([&queue, &convert, &resample, &volume, &sink])
                .is_err()
            {
                return;
            }
            let _ = gst::Element::link_many([&queue, &convert, &resample, &volume, &sink]);
            for element in [&queue, &convert, &resample, &volume, &sink] {
                let _ = element.sync_state_with_parent();
            }
            if let Some(sinkpad) = queue.static_pad("sink") {
                let _ = pad.link(&sinkpad);
            }
        });
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
pub use gst_player::{VideoPlayer, cameras};

#[cfg(not(feature = "gst"))]
pub struct VideoPlayer;

#[cfg(not(feature = "gst"))]
impl VideoPlayer {
    pub fn open(_path: &Path) -> Result<Self, MediaError> {
        Err(MediaError::Video(
            "video decode needs GStreamer; rebuild with the gst feature".into(),
        ))
    }

    pub fn open_camera(_name: &str) -> Result<Self, MediaError> {
        Err(MediaError::Video(
            "cameras need GStreamer; rebuild with the gst feature".into(),
        ))
    }

    pub fn next_frame(&mut self) -> Option<Raster> {
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

#[cfg(all(test, feature = "gst"))]
mod tests {
    use gstreamer as gst;
    use gstreamer::prelude::*;

    use super::*;

    #[test]
    fn a_wide_frame_is_letterboxed() {
        let mut player = VideoPlayer::from_launch(
            "videotestsrc num-buffers=1 pattern=red ! video/x-raw,width=8,height=2 ! videoconvert ! videoscale add-borders=true ! video/x-raw,format=RGBA,width=8,height=8,pixel-aspect-ratio=1/1 ! appsink name=sink sync=false",
        )
        .expect("letterbox pipeline");
        let frame = player.next_frame().expect("letterboxed frame");
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
        let frame = player.next_frame().expect("one red frame");
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
        let path = std::env::temp_dir().join(format!("freeshow-rust-loop-{}.mkv", std::process::id()));
        let location = path.to_string_lossy().replace('"', "");
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
        for _ in 0..40 {
            if player.next_frame().is_some() {
                frames += 1;
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
        let path = std::env::temp_dir().join(format!("freeshow-rust-mute-{}.wav", std::process::id()));
        let location = path.to_string_lossy().replace('"', "");
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
