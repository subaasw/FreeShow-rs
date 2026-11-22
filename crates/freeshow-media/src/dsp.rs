// SPDX-License-Identifier: GPL-3.0-only

//! A compact audio-effects chain applied to decoded PCM.

use crate::Pcm;

/// One processor in an `audioEffects` stack.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEffect {
    Gain(f32),
    Eq {
        freq: f32,
        gain_db: f32,
        q: f32,
    },
    Delay {
        ms: f32,
        feedback: f32,
        mix: f32,
    },
    Compressor {
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    },
    Limiter {
        ceiling_db: f32,
    },
}

impl AudioEffect {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Gain(_) => "gain",
            Self::Eq { .. } => "eq",
            Self::Delay { .. } => "delay",
            Self::Compressor { .. } => "compressor",
            Self::Limiter { .. } => "limiter",
        }
    }
}

/// Parse enabled processors from a FreeShow `audioEffects` object.
pub fn effects_from_value(value: &serde_json::Value) -> Vec<AudioEffect> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    for config in map.values() {
        let Some(stack) = config.get("stack").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for item in stack {
            if item.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(effect) = parse_effect(item) {
                effects.push(effect);
            }
        }
    }
    effects
}

fn parse_effect(item: &serde_json::Value) -> Option<AudioEffect> {
    let kind = item
        .get("type")
        .or_else(|| item.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("gain")
        .to_ascii_lowercase();
    let cfg = item.get("config").unwrap_or(item);
    let num = |keys: &[&str], default: f32| {
        keys.iter()
            .find_map(|key| {
                cfg.get(*key)
                    .and_then(serde_json::Value::as_f64)
                    .map(|value| value as f32)
            })
            .unwrap_or(default)
    };
    match kind.as_str() {
        "gain" | "volume" => Some(AudioEffect::Gain(num(
            &["gain", "volume", "outputGain"],
            1.0,
        ))),
        "eq" | "equalizer" | "peaking" | "filter" => Some(AudioEffect::Eq {
            freq: num(&["freq", "frequency", "hz"], 1000.0),
            gain_db: num(&["gain", "gainDb", "db"], 0.0),
            q: num(&["q", "Q"], 0.7),
        }),
        "delay" | "echo" => Some(AudioEffect::Delay {
            ms: num(&["delay", "delayMs", "time"], 120.0),
            feedback: num(&["feedback"], 0.25),
            mix: num(&["mix", "wet"], 0.25),
        }),
        "compressor" | "compress" => Some(AudioEffect::Compressor {
            threshold_db: num(&["threshold", "thresholdDb"], -18.0),
            ratio: num(&["ratio"], 4.0),
            attack_ms: num(&["attack", "attackMs"], 10.0),
            release_ms: num(&["release", "releaseMs"], 80.0),
        }),
        "limiter" | "limit" => Some(AudioEffect::Limiter {
            ceiling_db: num(&["ceiling", "ceilingDb", "threshold"], -1.0),
        }),
        _ => None,
    }
}

/// Apply the chain in place to interleaved `f32` PCM.
pub fn apply_effects(pcm: &mut Pcm, effects: &[AudioEffect]) {
    if effects.is_empty() || pcm.samples.is_empty() || pcm.channels == 0 {
        return;
    }
    for effect in effects {
        match effect {
            AudioEffect::Gain(gain) => apply_gain(&mut pcm.samples, *gain),
            AudioEffect::Eq { freq, gain_db, q } => {
                apply_peaking(pcm, *freq, *gain_db, *q);
            }
            AudioEffect::Delay { ms, feedback, mix } => apply_delay(pcm, *ms, *feedback, *mix),
            AudioEffect::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
            } => apply_compressor(pcm, *threshold_db, *ratio, *attack_ms, *release_ms),
            AudioEffect::Limiter { ceiling_db } => apply_limiter(&mut pcm.samples, *ceiling_db),
        }
    }
}

fn apply_gain(samples: &mut [f32], gain: f32) {
    let gain = gain.clamp(0.0, 8.0);
    for sample in samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

fn apply_peaking(pcm: &mut Pcm, freq: f32, gain_db: f32, q: f32) {
    if gain_db.abs() < 0.01 {
        return;
    }
    let rate = pcm.rate.max(1) as f32;
    let freq = freq.clamp(20.0, rate / 2.0 - 1.0);
    let q = q.clamp(0.1, 12.0);
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / rate;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha / a;
    if a0.abs() < f32::EPSILON {
        return;
    }
    let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0);
    let channels = pcm.channels as usize;
    let mut history = vec![(0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32); channels];
    for (index, sample) in pcm.samples.iter_mut().enumerate() {
        let ch = index % channels;
        let (x1, x2, y1, y2) = history[ch];
        let x0 = *sample;
        let y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        history[ch] = (x0, x1, y0, y1);
        *sample = y0.clamp(-1.0, 1.0);
    }
}

fn apply_delay(pcm: &mut Pcm, ms: f32, feedback: f32, mix: f32) {
    let channels = pcm.channels as usize;
    let delay_frames = ((ms.max(0.0) / 1000.0) * pcm.rate as f32).round() as usize;
    if delay_frames == 0 {
        return;
    }
    let mix = mix.clamp(0.0, 1.0);
    let feedback = feedback.clamp(0.0, 0.95);
    let mut ring = vec![0.0_f32; (delay_frames + 1) * channels];
    let mut pos = 0usize;
    for (index, sample) in pcm.samples.iter_mut().enumerate() {
        let ch = index % channels;
        let read = (pos + ch) % ring.len();
        let echo = ring[read];
        let wet = *sample + echo * feedback;
        ring[read] = wet;
        *sample = (*sample * (1.0 - mix) + echo * mix).clamp(-1.0, 1.0);
        if ch + 1 == channels {
            pos = (pos + channels) % ring.len();
        }
    }
}

fn apply_compressor(pcm: &mut Pcm, threshold_db: f32, ratio: f32, attack_ms: f32, release_ms: f32) {
    let threshold = db_to_lin(threshold_db).max(1.0e-6);
    let ratio = ratio.max(1.0);
    let rate = pcm.rate.max(1) as f32;
    let attack = (-1.0 / (0.001 * attack_ms.max(0.1) * rate)).exp();
    let release = (-1.0 / (0.001 * release_ms.max(1.0) * rate)).exp();
    let mut envelope = 0.0_f32;
    for sample in &mut pcm.samples {
        let abs = sample.abs();
        let coeff = if abs > envelope { attack } else { release };
        envelope = coeff * envelope + (1.0 - coeff) * abs;
        let gain = if envelope <= threshold {
            1.0
        } else {
            let over = envelope / threshold;
            (threshold * over.powf(1.0 / ratio) / envelope.max(1.0e-6)).min(1.0)
        };
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

fn apply_limiter(samples: &mut [f32], ceiling_db: f32) {
    let ceiling = db_to_lin(ceiling_db).clamp(0.05, 1.0);
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak <= ceiling || peak <= f32::EPSILON {
        for sample in samples {
            *sample = sample.clamp(-ceiling, ceiling);
        }
        return;
    }
    let gain = ceiling / peak;
    for sample in samples {
        *sample = (*sample * gain).clamp(-ceiling, ceiling);
    }
}

fn db_to_lin(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f32, freq: f32, amp: f32) -> Pcm {
        let n = (rate as f32 * seconds) as usize;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / rate as f32;
            samples.push((2.0 * std::f32::consts::PI * freq * t).sin() * amp);
        }
        Pcm {
            rate,
            channels: 1,
            samples,
        }
    }

    #[test]
    fn gain_eq_delay_and_limiter_change_the_buffer() {
        let effects = effects_from_value(&serde_json::json!({
            "main": {"stack": [
                {"type": "gain", "config": {"gain": 0.5}},
                {"type": "eq", "config": {"freq": 1000.0, "gain": 6.0, "q": 0.7}},
                {"type": "delay", "config": {"delayMs": 20.0, "mix": 0.4, "feedback": 0.1}},
                {"type": "compressor", "config": {"threshold": -12.0, "ratio": 4.0}},
                {"type": "limiter", "config": {"ceiling": -1.0}}
            ]}
        }));
        assert_eq!(effects.len(), 5);
        let mut pcm = tone(48_000, 0.1, 440.0, 0.9);
        let peak_before = pcm.samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        apply_effects(&mut pcm, &effects);
        let peak_after = pcm.samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!(peak_after < peak_before);
        assert!(peak_after <= 0.95);
        let mut quiet = tone(48_000, 0.02, 440.0, 0.8);
        apply_effects(&mut quiet, &[AudioEffect::Gain(0.25)]);
        let peak = quiet.samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!((peak - 0.2).abs() < 0.02, "{peak}");
    }
}
