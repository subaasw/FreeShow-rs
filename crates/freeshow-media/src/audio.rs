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
}

impl Playback {
    pub fn new(pcm: Pcm, volume: f32) -> Self {
        Self::shared(Arc::new(pcm), volume)
    }

    fn shared(pcm: Arc<Pcm>, volume: f32) -> Self {
        Self {
            pcm,
            position: 0.0,
            volume: volume.clamp(0.0, 1.0),
            paused: false,
            looping: false,
            start: 0.0,
            end: 0.0,
            tempo: 1.0,
        }
    }

    /// Playback speed. A value of 2 plays the file twice as fast. Zero and negative values stay at normal speed.
    pub fn set_rate(&mut self, rate: f64) {
        self.tempo = if rate > 0.0 { rate } else { 1.0 };
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
        !self.paused && !self.looping && self.frame_count() > 0 && self.position >= self.play_limit()
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
                } else if channel < src_channels {
                    self.pcm.samples[base + channel]
                } else {
                    self.pcm.samples[base]
                };
                output[frame * dst_channels + channel] = sample * self.volume;
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
    fn start(pcm: Arc<Pcm>, volume: f32, device_name: Option<&str>) -> Result<Self, MediaError> {
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
        let supported = device
            .default_output_config()
            .map_err(|err| MediaError::Audio(err.to_string()))?;
        let rate = supported.sample_rate().0;
        let channels = supported.channels();
        let playback = Arc::new(Mutex::new(Playback::shared(pcm, volume)));
        let state = Arc::clone(&playback);
        let config: cpal::StreamConfig = supported.clone().into();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| fill_locked(&state, data, rate, channels),
                |err| tracing_audio(err),
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
                |err| tracing_audio(err),
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
                |err| tracing_audio(err),
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

    pub fn position_seconds(&self) -> f64 {
        self.playback
            .lock()
            .map(|playback| playback.position_seconds())
            .unwrap_or(0.0)
    }

    pub fn duration_seconds(&self) -> f64 {
        self.playback
            .lock()
            .map(|playback| playback.duration_seconds())
            .unwrap_or(0.0)
    }

    pub fn is_paused(&self) -> bool {
        self.playback
            .lock()
            .map(|playback| playback.is_paused())
            .unwrap_or(false)
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
            playback.set_volume(volume);
        }
    }
}

/// One decoded track sent to the physical destinations in the library routing graph.
pub struct AudioOutput {
    // ponytail: each device has its own clock; add clock-drift resampling if
    // long-running outputs must remain sample synchronized.
    outputs: Vec<(Option<String>, DeviceAudioOutput)>,
}

impl AudioOutput {
    pub fn play_file(path: &Path, volume: f32) -> Result<Self, MediaError> {
        Self::play_file_routed(path, &[(None, volume)])
    }

    pub fn play_file_routed(
        path: &Path,
        routes: &[(Option<String>, f32)],
    ) -> Result<Self, MediaError> {
        let bytes = std::fs::read(path).map_err(|err| MediaError::Audio(err.to_string()))?;
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("wav");
        let pcm = Arc::new(decode_with_hint(&bytes, extension)?);
        // Keep a silent clock for an intentionally disconnected source, so end
        // actions and playlist advancement still run without leaking to Main.
        let silent = [(None, 0.0)];
        let routes = if routes.is_empty() {
            &silent[..]
        } else {
            routes
        };
        let mut outputs = Vec::new();
        for (device, gain) in routes {
            // A missing named destination fails closed: never play its cue in
            // the default speakers as a fallback.
            let output = DeviceAudioOutput::start(pcm.clone(), *gain, device.as_deref())?;
            outputs.push((device.clone(), output));
        }
        Ok(Self { outputs })
    }

    pub fn start(pcm: Pcm, volume: f32, device_name: Option<&str>) -> Result<Self, MediaError> {
        Ok(Self {
            outputs: vec![(
                device_name.map(str::to_string),
                DeviceAudioOutput::start(Arc::new(pcm), volume, device_name)?,
            )],
        })
    }

    pub fn set_route_gains(&self, routes: &[(Option<String>, f32)]) {
        for (device, output) in &self.outputs {
            let gain = routes
                .iter()
                .find(|(name, _)| name == device)
                .map(|(_, gain)| *gain)
                .unwrap_or(0.0);
            output.set_volume(gain);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        for (_, output) in &self.outputs {
            output.set_volume(volume);
        }
    }

    pub fn set_paused(&self, paused: bool) {
        for (_, output) in &self.outputs {
            output.set_paused(paused);
        }
    }

    pub fn seek_seconds(&self, seconds: f64) {
        for (_, output) in &self.outputs {
            output.seek_seconds(seconds);
        }
    }

    pub fn position_seconds(&self) -> f64 {
        self.outputs[0].1.position_seconds()
    }
    pub fn duration_seconds(&self) -> f64 {
        self.outputs[0].1.duration_seconds()
    }
    pub fn is_paused(&self) -> bool {
        self.outputs[0].1.is_paused()
    }
    pub fn is_finished(&self) -> bool {
        self.outputs.iter().all(|(_, output)| output.is_finished())
    }

    pub fn set_looping(&self, looping: bool) {
        for (_, output) in &self.outputs {
            output.set_looping(looping);
        }
    }

    pub fn set_span(&self, from: f64, to: f64) {
        for (_, output) in &self.outputs {
            output.set_span(from, to);
        }
    }

    pub fn set_rate(&self, rate: f64) {
        for (_, output) in &self.outputs {
            output.set_rate(rate);
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
