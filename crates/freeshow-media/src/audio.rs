// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::MediaError;
use freeshow_core::{NetworkRoute, SpeakerRoute, speaker_matrix};

#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

pub fn decode_audio(bytes: &[u8]) -> Result<Pcm, MediaError> {
    decode_with_hint(bytes, "wav")
}

pub fn decode_with_hint(bytes: &[u8], extension: &str) -> Result<Pcm, MediaError> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let source = MediaSourceStream::new(Box::new(cursor), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|err| MediaError::Decode(err.to_string()))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| MediaError::Decode("audio file has no track".into()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| MediaError::Decode(err.to_string()))?;
    let mut rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut channels = 1u16;
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(err) => return Err(MediaError::Decode(err.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymError::IoError(_)) | Err(SymError::DecodeError(_)) => continue,
            Err(err) => return Err(MediaError::Decode(err.to_string())),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        channels = spec.channels.count() as u16;
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        samples.extend_from_slice(buffer.samples());
    }
    if samples.is_empty() {
        return Err(MediaError::Decode("audio file has no samples".into()));
    }
    Ok(Pcm {
        rate,
        channels: channels.max(1),
        samples,
    })
}

pub fn apply_volume(samples: &[f32], volume: f32) -> Vec<f32> {
    let volume = volume.clamp(0.0, 1.0);
    samples.iter().map(|sample| sample * volume).collect()
}

#[derive(Debug)]
pub struct Playback {
    pcm: Arc<Pcm>,
    position: f64,
    volume: f32,
    paused: bool,
    looping: bool,
    start: f64,
    end: f64,
    tempo: f64,
    routes: Vec<SpeakerRoute>,
    matrix: Vec<Vec<f32>>,
}

impl Playback {
    pub fn new(pcm: Pcm, volume: f32) -> Self {
        Self::shared(Arc::new(pcm), volume)
    }

    pub(crate) fn shared(pcm: Arc<Pcm>, volume: f32) -> Self {
        Self {
            pcm,
            position: 0.0,
            volume: volume.clamp(0.0, 1.0),
            paused: false,
            looping: false,
            start: 0.0,
            end: 0.0,
            tempo: 1.0,
            routes: vec![SpeakerRoute::stereo(None, 1.0)],
            matrix: Vec::new(),
        }
    }

    /// Playback speed. A value of 2 plays the file twice as fast. Zero and negative values stay at normal speed.
    pub fn set_rate(&mut self, rate: f64) {
        self.tempo = if rate.is_finite() && rate > 0.0 {
            rate
        } else {
            1.0
        };
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// Plays from `from_seconds`. A `to_seconds` above zero stops there; zero plays through the file.
    pub fn set_span(&mut self, from_seconds: f64, to_seconds: f64) {
        let frames = self.frame_count() as f64;
        let rate = f64::from(self.pcm.rate.max(1));
        self.start = (from_seconds.max(0.0) * rate).round().min(frames);
        self.end = if to_seconds > 0.0 {
            (to_seconds * rate).round().clamp(self.start, frames)
        } else {
            0.0
        };
        self.position = self.start;
    }

    fn frame_count(&self) -> usize {
        let channels = self.pcm.channels.max(1) as usize;
        self.pcm.samples.len() / channels
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Moves the playhead. `seconds` is clamped to the decoded file.
    pub fn position_seconds(&self) -> f64 {
        self.position / f64::from(self.pcm.rate.max(1))
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frame_count() as f64 / f64::from(self.pcm.rate.max(1))
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The playhead has reached the end and the file is not set to repeat.
    pub fn is_finished(&self) -> bool {
        !self.paused
            && !self.looping
            && self.frame_count() > 0
            && self.position >= self.play_limit()
    }

    fn play_limit(&self) -> f64 {
        let frames = self.frame_count() as f64;
        if self.end > 0.0 {
            self.end.min(frames)
        } else {
            frames
        }
    }

    pub fn seek_seconds(&mut self, seconds: f64) {
        let channels = self.pcm.channels.max(1) as usize;
        let frames = self.pcm.samples.len() / channels;
        let target = (seconds.max(0.0) * f64::from(self.pcm.rate.max(1))).round();
        self.position = target.min(frames as f64);
    }

    pub fn set_routes(&mut self, routes: &[SpeakerRoute]) {
        if self.routes != routes {
            self.routes = routes.to_vec();
            self.matrix.clear();
        }
    }

    /// Fills interleaved output frames, resampling by stepping through the source.
    pub fn fill(&mut self, output: &mut [f32], output_rate: u32, output_channels: u16) {
        if self.paused {
            output.fill(0.0);
            return;
        }
        let src_channels = self.pcm.channels.max(1) as usize;
        let dst_channels = output_channels.max(1) as usize;
        if dst_channels == 0 || output.is_empty() {
            return;
        }
        if self.matrix.len() != dst_channels {
            self.matrix = speaker_matrix(self.pcm.channels, output_channels, &self.routes);
        }
        output.fill(0.0);
        let frame_count = self.frame_count();
        if frame_count == 0 {
            output.fill(0.0);
            return;
        }
        let limit = if self.end > 0.0 {
            self.end.min(frame_count as f64)
        } else {
            frame_count as f64
        };
        let frames = output.len() / dst_channels;
        let step = f64::from(self.pcm.rate.max(1)) / f64::from(output_rate.max(1)) * self.tempo;
        for frame in 0..frames {
            if self.position >= limit && self.looping && self.start < limit {
                let span = limit - self.start;
                let over = self.position - self.start;
                self.position = self.start + over % span;
            }
            let silent = self.position >= limit;
            let index = self.position as usize;
            let base = index * src_channels;
            for channel in 0..dst_channels {
                let sample = if silent || base >= self.pcm.samples.len() {
                    0.0
                } else {
                    self.matrix
                        .get(channel)
                        .map(|weights| {
                            weights
                                .iter()
                                .zip(&self.pcm.samples[base..base + src_channels])
                                .map(|(weight, sample)| weight * sample)
                                .sum::<f32>()
                        })
                        .unwrap_or(0.0)
                };
                output[frame * dst_channels + channel] = (sample * self.volume).clamp(-1.0, 1.0);
            }
            if !silent {
                self.position += step;
            }
        }
    }
}

struct DeviceAudioOutput {
    stream: cpal::Stream,
    playback: Arc<Mutex<Playback>>,
}

impl DeviceAudioOutput {
    fn start(
        pcm: Arc<Pcm>,
        routes: &[SpeakerRoute],
        device_name: Option<&str>,
        options: &AudioPlaybackOptions,
    ) -> Result<Self, MediaError> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => host
                .output_devices()
                .map_err(|err| MediaError::Audio(err.to_string()))?
                .find(|device| device.name().ok().as_deref() == Some(name))
                .ok_or_else(|| MediaError::Audio(format!("no output device named {name}")))?,
            None => host
                .default_output_device()
                .ok_or_else(|| MediaError::Audio("no default output device".into()))?,
        };
        let mut supported = device
            .default_output_config()
            .map_err(|err| MediaError::Audio(err.to_string()))?;
        let required = routes
            .iter()
            .filter_map(|route| route.output_channel)
            .max()
            .map(|index| index + 1)
            .unwrap_or(1);
        if supported.channels() < required {
            let rate = supported.sample_rate();
            supported = device
                .supported_output_configs()
                .map_err(|err| MediaError::Audio(err.to_string()))?
                .filter(|config| config.channels() >= required)
                .min_by_key(|config| config.channels())
                .map(|config| {
                    let selected_rate = cpal::SampleRate(
                        rate.0
                            .clamp(config.min_sample_rate().0, config.max_sample_rate().0),
                    );
                    config.with_sample_rate(selected_rate)
                })
                .ok_or_else(|| {
                    MediaError::Audio(format!("output device has no channel {}", required))
                })?;
        }
        let rate = supported.sample_rate().0;
        let channels = supported.channels();
        let mut state = Playback::shared(pcm, 1.0);
        state.set_routes(routes);
        options.apply(&mut state);
        let playback = Arc::new(Mutex::new(state));
        let state = Arc::clone(&playback);
        let config: cpal::StreamConfig = supported.clone().into();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| fill_locked(&state, data, rate, channels),
                tracing_audio,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    let mut floats = vec![0.0; data.len()];
                    fill_locked(&state, &mut floats, rate, channels);
                    for (slot, sample) in data.iter_mut().zip(floats) {
                        *slot = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                    }
                },
                tracing_audio,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    let mut floats = vec![0.0; data.len()];
                    fill_locked(&state, &mut floats, rate, channels);
                    for (slot, sample) in data.iter_mut().zip(floats) {
                        let shifted = (sample.clamp(-1.0, 1.0) + 1.0) * 0.5;
                        *slot = (shifted * f32::from(u16::MAX)) as u16;
                    }
                },
                tracing_audio,
                None,
            ),
            other => {
                return Err(MediaError::Audio(format!(
                    "unsupported output sample format {other:?}"
                )));
            }
        }
        .map_err(|err| MediaError::Audio(err.to_string()))?;
        stream
            .play()
            .map_err(|err| MediaError::Audio(err.to_string()))?;
        Ok(Self { stream, playback })
    }

    /// Holds the playhead. A later play of the same file continues from here.
    pub fn set_paused(&self, paused: bool) {
        if paused {
            let _ = self.stream.pause();
        }
        if let Ok(mut playback) = self.playback.lock() {
            playback.set_paused(paused);
        }
        if !paused {
            let _ = self.stream.play();
        }
    }

    pub fn seek_seconds(&self, seconds: f64) {
        if let Ok(mut playback) = self.playback.lock() {
            playback.seek_seconds(seconds);
        }
    }

    pub fn is_finished(&self) -> bool {
        self.playback
            .lock()
            .map(|playback| playback.is_finished())
            .unwrap_or(false)
    }

    pub fn set_looping(&self, looping: bool) {
        if let Ok(mut playback) = self.playback.lock() {
            playback.set_looping(looping);
        }
    }

    pub fn set_span(&self, from_seconds: f64, to_seconds: f64) {
        if let Ok(mut playback) = self.playback.lock() {
            playback.set_span(from_seconds, to_seconds);
        }
    }

    pub fn set_rate(&self, rate: f64) {
        if let Ok(mut playback) = self.playback.lock() {
            playback.set_rate(rate);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Ok(mut playback) = self.playback.lock() {
            for route in &mut playback.routes {
                route.gain = volume.clamp(0.0, 1.0);
            }
            playback.matrix.clear();
        }
    }

    fn set_routes(&self, routes: &[SpeakerRoute]) {
        if let Ok(mut playback) = self.playback.lock() {
            playback.set_routes(routes);
        }
    }
}

/// One decoded track sent to the physical destinations in the library routing graph.
#[derive(Debug, Clone)]
pub struct AudioPlaybackOptions {
    pub looping: bool,
    pub from_seconds: f64,
    pub to_seconds: f64,
    pub rate: f64,
    pub effects: Vec<crate::AudioEffect>,
}

impl Default for AudioPlaybackOptions {
    fn default() -> Self {
        Self {
            looping: false,
            from_seconds: 0.0,
            to_seconds: 0.0,
            rate: 1.0,
            effects: Vec::new(),
        }
    }
}

impl AudioPlaybackOptions {
    pub(crate) fn apply(&self, playback: &mut Playback) {
        playback.set_looping(self.looping);
        playback.set_span(self.from_seconds, self.to_seconds);
        playback.set_rate(self.rate);
    }
}

pub struct AudioOutput {
    // ponytail: each device has its own clock; add clock-drift resampling if
    // long-running outputs must remain sample synchronized.
    outputs: Vec<(Option<String>, DeviceAudioOutput)>,
    #[cfg(feature = "gst")]
    network: Option<crate::network::NetworkAudio>,
}

impl AudioOutput {
    pub fn play_file(path: &Path, volume: f32) -> Result<Self, MediaError> {
        Self::play_file_routed(path, &[SpeakerRoute::stereo(None, volume)])
    }

    pub fn play_file_routed(path: &Path, routes: &[SpeakerRoute]) -> Result<Self, MediaError> {
        Self::play_file_with_network(path, routes, &[])
    }

    pub fn play_file_with_network(
        path: &Path,
        routes: &[SpeakerRoute],
        network_routes: &[NetworkRoute],
    ) -> Result<Self, MediaError> {
        Self::play_file_configured(
            path,
            routes,
            network_routes,
            &AudioPlaybackOptions::default(),
        )
    }

    pub fn play_file_configured(
        path: &Path,
        routes: &[SpeakerRoute],
        network_routes: &[NetworkRoute],
        options: &AudioPlaybackOptions,
    ) -> Result<Self, MediaError> {
        let bytes = std::fs::read(path).map_err(|err| MediaError::Audio(err.to_string()))?;
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("wav");
        let mut decoded = decode_with_hint(&bytes, extension)?;
        crate::apply_effects(&mut decoded, &options.effects);
        let pcm = Arc::new(decoded);
        // Keep a silent clock for an intentionally disconnected source, so end
        // actions and playlist advancement still run without leaking to Main.
        #[cfg(feature = "gst")]
        let network = if !network_routes.is_empty() || routes.is_empty() {
            Some(crate::network::NetworkAudio::configured(
                Arc::clone(&pcm),
                network_routes,
                options,
            )?)
        } else {
            None
        };
        #[cfg(not(feature = "gst"))]
        if !network_routes.is_empty() {
            return Err(MediaError::Audio("network audio requires GStreamer".into()));
        }
        #[cfg(not(feature = "gst"))]
        let silent = [SpeakerRoute::stereo(None, 0.0)];
        #[cfg(not(feature = "gst"))]
        let routes = if routes.is_empty() {
            &silent[..]
        } else {
            routes
        };
        let mut outputs = Vec::new();
        let mut devices = std::collections::BTreeMap::<Option<String>, Vec<SpeakerRoute>>::new();
        for route in routes {
            devices
                .entry(route.device.clone())
                .or_default()
                .push(route.clone());
        }
        for (device, routes) in devices {
            let output =
                DeviceAudioOutput::start(pcm.clone(), &routes, device.as_deref(), options)?;
            outputs.push((device, output));
        }
        Ok(Self {
            outputs,
            #[cfg(feature = "gst")]
            network,
        })
    }

    pub fn start(pcm: Pcm, volume: f32, device_name: Option<&str>) -> Result<Self, MediaError> {
        Ok(Self {
            #[cfg(feature = "gst")]
            network: None,
            outputs: vec![(
                device_name.map(str::to_string),
                DeviceAudioOutput::start(
                    Arc::new(pcm),
                    &[SpeakerRoute::stereo(
                        device_name.map(str::to_string),
                        volume,
                    )],
                    device_name,
                    &AudioPlaybackOptions::default(),
                )?,
            )],
        })
    }

    pub fn set_network_routes(&self, routes: &[NetworkRoute]) {
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network {
            network.set_routes(routes);
        }
        let _ = routes;
    }

    fn clock(&self) -> Option<Arc<Mutex<Playback>>> {
        if let Some((_, output)) = self.outputs.first() {
            return Some(Arc::clone(&output.playback));
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network {
            return Some(Arc::clone(&network.playback));
        }
        None
    }

    pub fn set_route_gains(&self, routes: &[SpeakerRoute]) {
        for (device, output) in &self.outputs {
            let selected: Vec<_> = routes
                .iter()
                .filter(|route| &route.device == device)
                .cloned()
                .collect();
            output.set_routes(&selected);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        for (_, output) in &self.outputs {
            output.set_volume(volume);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network
            && let Ok(mut playback) = network.playback.lock()
        {
            playback.set_volume(volume);
        }
    }

    pub fn set_paused(&self, paused: bool) {
        for (_, output) in &self.outputs {
            output.set_paused(paused);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network {
            network.set_paused(paused);
        }
    }

    pub fn seek_seconds(&self, seconds: f64) {
        for (_, output) in &self.outputs {
            output.seek_seconds(seconds);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network
            && let Ok(mut playback) = network.playback.lock()
        {
            playback.seek_seconds(seconds);
        }
    }

    pub fn position_seconds(&self) -> f64 {
        self.clock()
            .and_then(|clock| {
                clock
                    .lock()
                    .ok()
                    .map(|playback| playback.position_seconds())
            })
            .unwrap_or(0.0)
    }
    pub fn duration_seconds(&self) -> f64 {
        self.clock()
            .and_then(|clock| {
                clock
                    .lock()
                    .ok()
                    .map(|playback| playback.duration_seconds())
            })
            .unwrap_or(0.0)
    }
    pub fn is_paused(&self) -> bool {
        self.clock()
            .and_then(|clock| clock.lock().ok().map(|playback| playback.is_paused()))
            .unwrap_or(false)
    }
    pub fn is_finished(&self) -> bool {
        let finished = self.outputs.iter().all(|(_, output)| output.is_finished());
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network {
            return finished
                && network
                    .playback
                    .lock()
                    .map(|playback| playback.is_finished())
                    .unwrap_or(false);
        }
        finished
    }

    pub fn set_looping(&self, looping: bool) {
        for (_, output) in &self.outputs {
            output.set_looping(looping);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network
            && let Ok(mut playback) = network.playback.lock()
        {
            playback.set_looping(looping);
        }
    }

    pub fn set_span(&self, from: f64, to: f64) {
        for (_, output) in &self.outputs {
            output.set_span(from, to);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network
            && let Ok(mut playback) = network.playback.lock()
        {
            playback.set_span(from, to);
        }
    }

    pub fn set_rate(&self, rate: f64) {
        for (_, output) in &self.outputs {
            output.set_rate(rate);
        }
        #[cfg(feature = "gst")]
        if let Some(network) = &self.network
            && let Ok(mut playback) = network.playback.lock()
        {
            playback.set_rate(rate);
        }
    }
}

fn fill_locked(state: &Mutex<Playback>, data: &mut [f32], rate: u32, channels: u16) {
    if let Ok(mut playback) = state.lock() {
        playback.fill(data, rate, channels);
    }
}

fn tracing_audio(err: cpal::StreamError) {
    eprintln!("audio output: {err}");
}

pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
        let data_bytes = samples.len() * 2;
        let mut bytes = Vec::with_capacity(44 + data_bytes);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[cfg(feature = "gst")]
    #[test]
    fn a_network_only_file_needs_no_physical_output_device() {
        let path =
            std::env::temp_dir().join(format!("freeshow-network-only-{}.wav", std::process::id()));
        std::fs::write(&path, wav_bytes(&vec![16000; 16000], 8000)).unwrap();
        let output = AudioOutput::play_file_configured(
            &path,
            &[],
            &[NetworkRoute {
                output_id: format!("file-test-{}", std::process::id()),
                gain: 0.5,
            }],
            &AudioPlaybackOptions {
                looping: true,
                from_seconds: 0.8,
                to_seconds: 1.2,
                rate: 1.0,
                effects: Vec::new(),
            },
        )
        .unwrap();
        assert!(output.outputs.is_empty());
        assert_eq!(output.duration_seconds(), 2.0);
        assert!(output.position_seconds() >= 0.8 && output.position_seconds() <= 1.2);
        output.set_paused(true);
        output.seek_seconds(0.8);
        assert_eq!(output.position_seconds(), 0.8);
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(output.position_seconds(), 0.8);
        assert!(output.is_paused());
        drop(output);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn playback_mixes_only_into_assigned_hardware_channels() {
        let pcm = Pcm {
            rate: 8000,
            channels: 2,
            samples: vec![0.8, 0.4, 0.6, 0.2],
        };
        let mut playback = Playback::new(pcm, 1.0);
        let cue = SpeakerRoute {
            output_channel: Some(3),
            ..SpeakerRoute::stereo(None, 0.5)
        };
        playback.set_routes(&[cue]);
        let mut output = [0.0; 4];
        playback.fill(&mut output, 8000, 4);
        assert_eq!(&output[..3], &[0.0, 0.0, 0.0]);
        assert!((output[3] - 0.3).abs() < 0.001);
        playback.seek_seconds(0.0);
        playback.set_routes(&[
            SpeakerRoute {
                input_channel: Some(0),
                output_channel: Some(0),
                ..SpeakerRoute::stereo(None, 0.5)
            },
            SpeakerRoute {
                input_channel: Some(1),
                output_channel: Some(1),
                ..SpeakerRoute::stereo(None, 0.25)
            },
        ]);
        let mut stereo = [0.0; 2];
        playback.fill(&mut stereo, 8000, 2);
        assert_eq!(stereo, [0.4, 0.1]);
        playback.set_routes(&[]);
        playback.fill(&mut stereo, 8000, 2);
        assert_eq!(stereo, [0.0, 0.0]);
        assert!(playback.is_finished());
    }

    #[test]
    fn wav_decodes_and_volume_scales_playback() {
        let pcm = decode_audio(&wav_bytes(&[16384, -16384], 8_000)).unwrap();
        assert_eq!(pcm.rate, 8_000);
        assert_eq!(pcm.channels, 1);
        assert!((pcm.samples[0] - 0.5).abs() < 0.02);
        let quiet = apply_volume(&pcm.samples, 0.5);
        assert!((quiet[0] - 0.25).abs() < 0.02);
        let mut playback = Playback::new(pcm, 0.5);
        let mut output = [0.0; 2];
        playback.fill(&mut output, 8_000, 1);
        assert!((output[0] - 0.25).abs() < 0.02);
        assert!((output[1] + 0.25).abs() < 0.02);
        let pcm = decode_audio(&wav_bytes(&[16384, -16384], 8_000)).unwrap();
        let mut playback = Playback::new(pcm, 1.0);
        playback.set_looping(true);
        let mut looped = [0.0; 4];
        playback.fill(&mut looped, 8_000, 1);
        assert!((looped[2] - looped[0]).abs() < 0.02);
        assert!((looped[3] - looped[1]).abs() < 0.02);
        assert!(looped[2].abs() > 0.2);
        let pcm = decode_audio(&wav_bytes(&[16384, -16384, 0, 16384], 8_000)).unwrap();
        let mut playback = Playback::new(pcm, 1.0);
        playback.set_span(1.0 / 8_000.0, 3.0 / 8_000.0);
        let mut trimmed = [1.0; 4];
        playback.fill(&mut trimmed, 8_000, 1);
        assert!(trimmed[0] < -0.2);
        assert!(trimmed[1].abs() < 0.05);
        assert_eq!(trimmed[2], 0.0);
        assert_eq!(trimmed[3], 0.0);
        let pcm = decode_audio(&wav_bytes(&[16384, -16384, 0, 16384], 8_000)).unwrap();
        let mut playback = Playback::new(pcm, 1.0);
        playback.set_rate(2.0);
        let mut fast = [0.0; 2];
        playback.fill(&mut fast, 8_000, 1);
        assert!(fast[0] > 0.2);
        assert!(fast[1].abs() < 0.05);
    }

    #[test]
    fn pause_holds_the_playhead() {
        let pcm = decode_audio(&wav_bytes(&[16384, -16384, 0, 0], 8_000)).unwrap();
        let mut playback = Playback::new(pcm, 1.0);
        playback.set_paused(true);
        let mut output = [1.0; 2];
        playback.fill(&mut output, 8_000, 1);
        assert_eq!(output, [0.0, 0.0]);
        assert_eq!(playback.position, 0.0);
        playback.set_paused(false);
        playback.fill(&mut output, 8_000, 1);
        assert!(output[0] > 0.2);
        assert!(playback.position > 0.0);
    }

    #[test]
    fn seek_moves_the_playhead() {
        let pcm = decode_audio(&wav_bytes(&[16384, -16384, 0, 0], 8_000)).unwrap();
        let mut playback = Playback::new(pcm, 1.0);
        playback.seek_seconds(2.0 / 8_000.0);
        assert!((playback.position - 2.0).abs() < 0.01);
        playback.seek_seconds(-1.0);
        assert_eq!(playback.position, 0.0);
        playback.seek_seconds(30.0);
        assert_eq!(playback.position, 4.0);
        assert!((playback.duration_seconds() - 4.0 / 8_000.0).abs() < 0.0001);
        playback.seek_seconds(2.0 / 8_000.0);
        assert!((playback.position_seconds() - 2.0 / 8_000.0).abs() < 0.0001);
    }

    #[test]
    fn device_list_does_not_panic() {
        let _ = output_devices();
    }
}
