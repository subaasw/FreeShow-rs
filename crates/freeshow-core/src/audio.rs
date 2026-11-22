// SPDX-License-Identifier: GPL-3.0-only

/// A path to a physical speaker. With no output channel the source plays normally.
/// A selected output with no input channel receives a mono mix of the source.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerRoute {
    pub device: Option<String>,
    pub gain: f32,
    pub input_channel: Option<u16>,
    pub output_channel: Option<u16>,
}

impl SpeakerRoute {
    pub fn stereo(device: Option<String>, gain: f32) -> Self {
        Self {
            device,
            gain,
            input_channel: None,
            output_channel: None,
        }
    }
}

/// A stereo feed for one FreeShow network output ID.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkRoute {
    pub output_id: String,
    pub gain: f32,
}

// Web Audio's speaker layouts, matching FreeShow's default interpretation.
// https://www.w3.org/TR/2021/REC-webaudio-20210617/#down-mix
fn mono_weight(inputs: u16, input: usize) -> f32 {
    match inputs {
        1 => 1.0,
        2 => 0.5,
        4 => 0.25,
        6 => match input {
            0 | 1 => std::f32::consts::FRAC_1_SQRT_2,
            2 => 1.0,
            3 => 0.0,
            _ => 0.5,
        },
        _ => {
            if input == 0 {
                1.0
            } else {
                0.0
            }
        }
    }
}

fn speaker_weight(inputs: u16, outputs: u16, output: usize, input: usize) -> f32 {
    let root = std::f32::consts::FRAC_1_SQRT_2;
    let is = |condition: bool| if condition { 1.0 } else { 0.0 };
    match (inputs, outputs) {
        (_, 1) => mono_weight(inputs, input),
        (1, 2 | 4) => is(output < 2 && input == 0),
        (1, 6) => is(output == 2 && input == 0),
        (2, 4 | 6) => is(output == input && output < 2),
        (4, 6) => is([Some(0), Some(1), None, None, Some(2), Some(3)][output] == Some(input)),
        (4, 2) => 0.5 * is(input == output || input == output + 2),
        (6, 2) => is(input == output) + root * is(input == 2 || input == output + 4),
        (6, 4) if output < 2 => is(input == output) + root * is(input == 2),
        (6, 4) => is(input == output + 2),
        _ => is(output == input),
    }
}

/// Rows are destination channels, columns are source channels. Shared by CPAL
/// and GStreamer so a cue cannot acquire a different channel map in a video.
pub fn speaker_matrix(inputs: u16, outputs: u16, routes: &[SpeakerRoute]) -> Vec<Vec<f32>> {
    if inputs == 0 || outputs == 0 || inputs > 64 || outputs > 64 {
        return Vec::new();
    }
    let mut matrix = vec![vec![0.0; inputs as usize]; outputs as usize];
    for route in routes {
        if !route.gain.is_finite() || route.gain < 0.0 {
            continue;
        }
        for (output, row) in matrix.iter_mut().enumerate() {
            if route
                .output_channel
                .is_some_and(|channel| channel as usize != output)
            {
                continue;
            }
            if let Some(input) = route.input_channel {
                if let Some(weight) = row.get_mut(input as usize) {
                    *weight += route.gain;
                }
            } else if route.output_channel.is_some() {
                for (input, weight) in row.iter_mut().enumerate() {
                    *weight += route.gain * mono_weight(inputs, input);
                }
            } else {
                for (input, weight) in row.iter_mut().enumerate() {
                    *weight += route.gain * speaker_weight(inputs, outputs, output, input);
                }
            }
        }
    }
    matrix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_channels_never_leak_to_other_speakers() {
        let cue = SpeakerRoute {
            device: None,
            gain: 0.5,
            input_channel: None,
            output_channel: Some(3),
        };
        assert_eq!(
            speaker_matrix(2, 4, std::slice::from_ref(&cue)),
            vec![vec![0.0; 2], vec![0.0; 2], vec![0.0; 2], vec![0.25, 0.25]]
        );
        let selected = SpeakerRoute {
            input_channel: Some(1),
            ..cue
        };
        assert_eq!(speaker_matrix(2, 4, &[selected])[3], vec![0.0, 0.5]);
        let surround = SpeakerRoute {
            output_channel: Some(0),
            ..SpeakerRoute::stereo(None, 1.0)
        };
        let mono = speaker_matrix(6, 2, &[surround]);
        assert_eq!(mono[0][3], 0.0, "LFE must not enter a mono cue");
        assert_eq!(mono[0][2], 1.0);
        assert_eq!(mono[0][4], 0.5);
        assert!((mono[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001);
        let quad = speaker_matrix(4, 6, &[SpeakerRoute::stereo(None, 1.0)]);
        assert_eq!(quad[2], vec![0.0; 4]);
        assert_eq!(quad[3], vec![0.0; 4]);
        assert_eq!(quad[4], vec![0.0, 0.0, 1.0, 0.0]);
        let invalid = SpeakerRoute {
            device: None,
            gain: 1.0,
            input_channel: Some(9),
            output_channel: Some(0),
        };
        assert_eq!(speaker_matrix(2, 2, &[invalid]), vec![vec![0.0; 2]; 2]);
        assert_eq!(
            speaker_matrix(2, 2, &[SpeakerRoute::stereo(None, 0.4)]),
            vec![vec![0.4, 0.0], vec![0.0, 0.4]]
        );
    }
}

/// The audio-only Icecast node is shared by saved routing and the sender.
pub const ICECAST_BUS: &str = "icecast";

#[derive(Clone, PartialEq, Eq)]
pub struct IcecastDestination {
    pub host: String,
    pub port: u16,
    pub mount: String,
    pub password: String,
}
