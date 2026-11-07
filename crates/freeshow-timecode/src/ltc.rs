// SPDX-License-Identifier: GPL-3.0-only

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{Timecode, TimecodeSource, remember};

const SYNC: [bool; 16] = [
    false, false, true, true, true, true, true, true, true, true, true, true, true, true, false,
    true,
];

/// Linear timecode as a mono audio tone. `samples` are in -1.0..1.0.
pub fn encode_ltc(code: Timecode, sample_rate: u32) -> Vec<f32> {
    let bits = frame_bits(code);
    let width = samples_per_bit(sample_rate, code.frames_per_second).round() as usize;
    let width = width.max(2);
    let mut level = 1.0f32;
    let mut samples = vec![level];
    for bit in bits {
        level = -level;
        for index in 0..width {
            if bit && index == width / 2 {
                level = -level;
            }
            samples.push(level);
        }
    }
    level = -level;
    samples.push(level);
    samples
}

pub fn decode_ltc(samples: &[f32], sample_rate: u32, frames_per_second: f64) -> Option<Timecode> {
    let width = samples_per_bit(sample_rate, frames_per_second);
    if width < 2.0 {
        return None;
    }
    let bits = bits_from_audio(samples, width)?;
    let code = timecode_from_bits(&bits, frames_per_second)?;
    Some(code)
}

fn samples_per_bit(sample_rate: u32, frames_per_second: f64) -> f32 {
    let rate = if frames_per_second <= 0.0 {
        25.0
    } else {
        frames_per_second
    };
    sample_rate as f32 / (rate as f32 * 80.0)
}

fn frame_bits(code: Timecode) -> [bool; 80] {
    let mut bits = [false; 80];
    write_bcd(&mut bits, 0, code.frames % 10, 4);
    write_bcd(&mut bits, 8, code.frames / 10, 2);
    write_bcd(&mut bits, 16, code.seconds % 10, 4);
    write_bcd(&mut bits, 24, code.seconds / 10, 3);
    write_bcd(&mut bits, 32, code.minutes % 10, 4);
    write_bcd(&mut bits, 40, code.minutes / 10, 3);
    write_bcd(&mut bits, 48, code.hours % 10, 4);
    write_bcd(&mut bits, 56, code.hours / 10, 2);
    bits[64..].copy_from_slice(&SYNC);
    bits
}

fn write_bcd(bits: &mut [bool], start: usize, value: u8, width: usize) {
    for index in 0..width {
        bits[start + index] = ((value >> index) & 1) == 1;
    }
}

fn read_bcd(bits: &[bool], start: usize, width: usize) -> u8 {
    let mut value = 0u8;
    for index in 0..width {
        if bits.get(start + index).copied().unwrap_or(false) {
            value |= 1 << index;
        }
    }
    value
}

fn bits_from_audio(samples: &[f32], samples_per_bit: f32) -> Option<Vec<bool>> {
    let mut crossings = Vec::new();
    let mut previous = *samples.first()?;
    for (index, sample) in samples.iter().copied().enumerate().skip(1) {
        if (previous <= 0.0 && sample > 0.0) || (previous > 0.0 && sample <= 0.0) {
            crossings.push(index);
        }
        previous = sample;
    }
    if crossings.len() < 3 {
        return None;
    }
    let half = samples_per_bit * 0.75;
    let mut bits = Vec::new();
    let mut index = 0;
    while index + 1 < crossings.len() {
        let gap = (crossings[index + 1] - crossings[index]) as f32;
        if gap < half {
            if index + 2 >= crossings.len() {
                break;
            }
            bits.push(true);
            index += 2;
        } else {
            bits.push(false);
            index += 1;
        }
    }
    if bits.len() < 80 {
        return None;
    }
    Some(bits)
}

fn timecode_from_bits(bits: &[bool], frames_per_second: f64) -> Option<Timecode> {
    let position = bits.windows(16).position(|window| window == SYNC)?;
    if position < 64 {
        return None;
    }
    let frame = &bits[position - 64..position];
    let frames = read_bcd(frame, 0, 4) + read_bcd(frame, 8, 2) * 10;
    let seconds = read_bcd(frame, 16, 4) + read_bcd(frame, 24, 3) * 10;
    let minutes = read_bcd(frame, 32, 4) + read_bcd(frame, 40, 3) * 10;
    let hours = read_bcd(frame, 48, 4) + read_bcd(frame, 56, 2) * 10;
    Some(Timecode {
        hours,
        minutes,
        seconds,
        frames,
        frames_per_second,
    })
}

pub struct LtcInput;

impl TimecodeSource for LtcInput {
    fn name(&self) -> &'static str {
        "ltc"
    }

    fn start(&self) {
        let wanted = std::env::var("FREESHOW_LTC_IN").unwrap_or_default();
        if wanted.trim().is_empty() {
            return;
        }
        let fps = std::env::var("FREESHOW_LTC_FPS")
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|rate| *rate > 0.0)
            .unwrap_or(25.0);
        let started = std::thread::Builder::new()
            .name("freeshow-ltc".into())
            .spawn(move || listen(wanted, fps));
        if started.is_err() {
            tracing::error!("could not start the ltc listener");
        }
    }
}

fn listen(wanted: String, frames_per_second: f64) {
    let host = cpal::default_host();
    let device = if wanted.eq_ignore_ascii_case("default") {
        host.default_input_device()
    } else {
        host.input_devices().ok().and_then(|mut devices| {
            devices.find(|device| {
                device
                    .name()
                    .ok()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&wanted))
            })
        })
    };
    let Some(device) = device else {
        tracing::error!("ltc input {wanted} is not connected");
        return;
    };
    let name = device.name().unwrap_or_else(|_| wanted.clone());
    let config = match device.default_input_config() {
        Ok(config) => config,
        Err(err) => {
            tracing::error!("ltc input {name} has no configuration: {err}");
            return;
        }
    };
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let pending = Arc::new(Mutex::new(Vec::<f32>::new()));
    let samples = Arc::clone(&pending);
    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _| {
            let mut mono = Vec::with_capacity(data.len() / channels.max(1));
            for frame in data.chunks(channels.max(1)) {
                mono.push(frame[0]);
            }
            let Ok(mut buffer) = samples.lock() else {
                return;
            };
            buffer.extend(mono);
            let limit = rate as usize;
            if buffer.len() > limit {
                let extra = buffer.len() - limit;
                buffer.drain(..extra);
            }
            if let Some(code) = decode_ltc(&buffer, rate, frames_per_second) {
                remember(code);
            }
        },
        |err| tracing::error!("ltc input stopped: {err}"),
        None,
    );
    match stream {
        Ok(stream) => {
            if stream.play().is_err() {
                tracing::error!("ltc input {name} did not start");
                return;
            }
            tracing::info!("ltc input {name}");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        Err(err) => tracing::error!("ltc input {name} did not open: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ltc_tone_decodes_to_the_same_time() {
        let code = Timecode {
            hours: 1,
            minutes: 2,
            seconds: 3,
            frames: 4,
            frames_per_second: 25.0,
        };
        let samples = encode_ltc(code, 48_000);
        let decoded = decode_ltc(&samples, 48_000, 25.0).unwrap();
        assert_eq!(decoded.display(), "01:02:03:04");
        assert_eq!(decoded.frames_per_second, 25.0);

        let later = Timecode {
            hours: 10,
            minutes: 20,
            seconds: 30,
            frames: 15,
            frames_per_second: 25.0,
        };
        let decoded = decode_ltc(&encode_ltc(later, 48_000), 48_000, 25.0).unwrap();
        assert_eq!(decoded.display(), "10:20:30:15");
    }
}
