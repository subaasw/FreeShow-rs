// SPDX-License-Identifier: GPL-3.0-only

mod ltc;

pub use ltc::{LtcInput, decode_ltc, encode_ltc};

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A timecode listener. The show keeps the wall clock when no source is connected.
pub trait TimecodeSource {
    fn name(&self) -> &'static str;
    fn start(&self);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub frames_per_second: f64,
}

impl Timecode {
    pub fn display(self) -> String {
        format!(
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }

    /// Milliseconds, using the same hours/minutes/seconds/frames formula as FreeShow.
    pub fn millis(self) -> f64 {
        let rate = if self.frames_per_second <= 0.0 {
            25.0
        } else {
            self.frames_per_second
        };
        (f64::from(self.hours) * 3600.0
            + f64::from(self.minutes) * 60.0
            + f64::from(self.seconds)
            + f64::from(self.frames) / rate)
            * 1000.0
    }

    fn from_parts(parts: [u8; 8]) -> Self {
        let frames = (parts[1] << 4) | parts[0];
        let seconds = (parts[3] << 4) | parts[2];
        let minutes = (parts[5] << 4) | parts[4];
        let hours = ((parts[7] & 0x01) << 4) | parts[6];
        let fps_code = (parts[7] >> 1) & 0x03;
        Self {
            hours,
            minutes,
            seconds,
            frames,
            frames_per_second: fps_from_code(fps_code),
        }
    }

    fn from_full(hours_byte: u8, minutes: u8, seconds: u8, frames: u8) -> Self {
        Self {
            hours: hours_byte & 0x1f,
            minutes,
            seconds,
            frames,
            frames_per_second: fps_from_code((hours_byte >> 5) & 0x03),
        }
    }
}

fn fps_from_code(code: u8) -> f64 {
    match code {
        0 => 24.0,
        1 => 25.0,
        2 => 29.97,
        _ => 30.0,
    }
}

/// Assembles MIDI quarter-frames and full-frame SysEx into a timecode.
#[derive(Debug, Default)]
pub struct MtcDecoder {
    parts: [u8; 8],
}

impl MtcDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Option<Timecode> {
        if bytes.first() == Some(&0xf1) && bytes.len() >= 2 {
            let data = bytes[1];
            let kind = ((data >> 4) & 0x07) as usize;
            self.parts[kind] = data & 0x0f;
            if kind == 7 {
                return Some(Timecode::from_parts(self.parts));
            }
            return None;
        }
        if bytes.len() >= 9
            && bytes[0] == 0xf0
            && bytes[1] == 0x7f
            && bytes[3] == 0x01
            && bytes[4] == 0x01
        {
            let code = Timecode::from_full(bytes[5], bytes[6], bytes[7], bytes[8]);
            self.parts = [
                code.frames & 0x0f,
                (code.frames >> 4) & 0x01,
                code.seconds & 0x0f,
                (code.seconds >> 4) & 0x03,
                code.minutes & 0x0f,
                (code.minutes >> 4) & 0x03,
                code.hours & 0x0f,
                ((code.hours >> 4) & 0x01) | (fps_code(code.frames_per_second) << 1),
            ];
            return Some(code);
        }
        None
    }
}

fn fps_code(rate: f64) -> u8 {
    if (rate - 24.0).abs() < 0.01 {
        0
    } else if (rate - 25.0).abs() < 0.01 {
        1
    } else if (rate - 29.97).abs() < 0.01 {
        2
    } else {
        3
    }
}

static LATEST: Mutex<Option<(Timecode, Instant)>> = Mutex::new(None);

pub fn remember(code: Timecode) {
    if let Ok(mut latest) = LATEST.lock() {
        *latest = Some((code, Instant::now()));
    }
}

/// The stage clock while MIDI timecode is arriving. Empty after two seconds of silence.
pub fn latest_clock() -> Option<String> {
    let Ok(latest) = LATEST.lock() else {
        return None;
    };
    let (code, at) = latest.as_ref()?;
    if at.elapsed() > Duration::from_secs(2) {
        return None;
    }
    Some(code.display())
}

#[derive(Default)]
pub struct MtcInput {
    pub port: String,
}

impl TimecodeSource for MtcInput {
    fn name(&self) -> &'static str {
        "mtc"
    }

    fn start(&self) {
        let wanted = if self.port.is_empty() {
            std::env::var("FREESHOW_MTC_IN").unwrap_or_default()
        } else {
            self.port.clone()
        };
        let started = std::thread::Builder::new()
            .name("freeshow-mtc".into())
            .spawn(move || listen(wanted));
        if started.is_err() {
            tracing::error!("could not start the timecode listener");
        }
    }
}

fn listen(wanted: String) {
    let midi = match midir::MidiInput::new("freeshow-mtc") {
        Ok(midi) => midi,
        Err(err) => {
            tracing::error!("timecode input is not available: {err}");
            return;
        }
    };
    let ports = midi.ports();
    if ports.is_empty() {
        tracing::info!("mtc input: no ports");
        return;
    }
    let port = if wanted.is_empty() {
        ports[0].clone()
    } else {
        match ports.into_iter().find(|port| {
            midi.port_name(port)
                .unwrap_or_default()
                .eq_ignore_ascii_case(&wanted)
        }) {
            Some(port) => port,
            None => {
                tracing::error!("mtc input {wanted} is not connected");
                return;
            }
        }
    };
    let name = midi.port_name(&port).unwrap_or_else(|_| "mtc".to_string());
    let mut decoder = MtcDecoder::default();
    let connection = midi.connect(
        &port,
        "freeshow-mtc",
        move |_stamp, bytes, _| {
            if let Some(code) = decoder.push(bytes) {
                remember(code);
            }
        },
        (),
    );
    match connection {
        Ok(_connection) => {
            tracing::info!("mtc input {name}");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        Err(err) => tracing::error!("mtc input {name} did not open: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quarter(kind: u8, value: u8) -> [u8; 2] {
        [0xf1, (kind << 4) | value]
    }

    #[test]
    fn quarter_frames_assemble_one_timecode() {
        let mut decoder = MtcDecoder::default();
        let pieces = [
            quarter(0, 4),
            quarter(1, 0),
            quarter(2, 3),
            quarter(3, 0),
            quarter(4, 2),
            quarter(5, 0),
            quarter(6, 1),
        ];
        for piece in pieces {
            assert!(decoder.push(&piece).is_none());
        }
        let code = decoder.push(&quarter(7, 2)).unwrap();
        assert_eq!(code.hours, 1);
        assert_eq!(code.minutes, 2);
        assert_eq!(code.seconds, 3);
        assert_eq!(code.frames, 4);
        assert_eq!(code.frames_per_second, 25.0);
        assert_eq!(code.display(), "01:02:03:04");
        assert!((code.millis() - 3_723_160.0).abs() < 0.01);
    }

    #[test]
    fn a_full_frame_sets_the_clock() {
        let mut decoder = MtcDecoder::default();
        let message = [0xf0, 0x7f, 0x7f, 0x01, 0x01, 0x20 | 1, 2, 3, 4, 0xf7];
        let code = decoder.push(&message).unwrap();
        assert_eq!(code.display(), "01:02:03:04");
        assert_eq!(code.frames_per_second, 25.0);
        assert!(decoder.push(&[0x90, 0, 100]).is_none());
    }
}
