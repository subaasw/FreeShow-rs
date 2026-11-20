// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

pub use freeshow_core::SpeakerRoute as AudioRoute;

/// Resolves FreeShow's source -> mixer -> speaker graph. An explicit graph with
/// no speaker path stays silent; network destinations never fall back to speakers.
pub fn audio_routes(settings: &Value, sources: &[&str], main_gain: f32) -> Vec<AudioRoute> {
    resolve_routes(settings, sources, main_gain, false)
}

pub fn network_audio_routes(
    settings: &Value,
    sources: &[&str],
    main_gain: f32,
) -> Vec<freeshow_core::NetworkRoute> {
    resolve_routes(settings, sources, main_gain, true)
        .into_iter()
        .filter_map(|route| {
            route.device.map(|output_id| freeshow_core::NetworkRoute {
                output_id,
                gain: route.gain,
            })
        })
        .collect()
}

fn resolve_routes(
    settings: &Value,
    sources: &[&str],
    main_gain: f32,
    network: bool,
) -> Vec<AudioRoute> {
    let routing = &settings["audioRouting"];
    let Some(connections) = routing["connections"].as_array() else {
        if network {
            if !routing.is_null() {
                return Vec::new();
            }
            return settings["outputs"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, output)| {
                    output["enabled"].as_bool() != Some(false)
                        && ["ndi", "rtmp", "webrtc", "omt"]
                            .iter()
                            .any(|kind| output[kind].as_bool() == Some(true))
                })
                .map(|(id, _)| {
                    let channel = &settings["audioChannelsData"][format!("channel_{id}")];
                    let gain = if channel["isMuted"].as_bool() == Some(true) {
                        0.0
                    } else {
                        channel["volume"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32
                    };
                    AudioRoute::stereo(Some(id.clone()), gain)
                })
                .collect();
        }
        return if routing.is_null() {
            vec![AudioRoute::stereo(None, main_gain.clamp(0.0, 1.0))]
        } else {
            Vec::new()
        };
    };
    let channels = &settings["audioChannelsData"];
    let channel_gain = |id: &str| {
        let linked = routing["channels"]
            .as_array()
            .and_then(|channels| {
                channels
                    .iter()
                    .find(|channel| channel["id"].as_str() == Some(id))
            })
            .and_then(|channel| channel["outputLink"].as_str())
            .or_else(|| id.strip_prefix("channel_"))
            .or_else(|| id.strip_prefix("output_win_sub_"));
        if linked
            .is_some_and(|output| settings["outputs"][output]["enabled"].as_bool() == Some(false))
        {
            return 0.0;
        }
        if id == "main" {
            return main_gain.clamp(0.0, 1.0);
        }
        let data = &channels[id];
        if data["isMuted"].as_bool() == Some(true) {
            return 0.0;
        }
        data["volume"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0) as f32
    };
    let mut destinations = BTreeMap::<(Option<String>, Option<u16>, Option<u16>), f32>::new();
    let mut stack: Vec<_> = sources
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|source| {
            (
                source.to_string(),
                channel_gain(source),
                None,
                None,
                HashSet::new(),
            )
        })
        .collect();
    // ponytail: enumerate acyclic paths; use topological propagation if large
    // routing graphs make path enumeration expensive.
    while let Some((id, gain, input, output_channel, mut visited)) = stack.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        if let Some(output) = id.strip_prefix("network_sub_") {
            if network && settings["outputs"][output]["enabled"].as_bool() != Some(false) {
                *destinations
                    .entry((Some(output.to_string()), None, None))
                    .or_default() += gain;
            }
            continue;
        }
        if id == "speaker_default" {
            if network {
                continue;
            }
            *destinations
                .entry((None, input, output_channel))
                .or_default() += gain;
            continue;
        }
        if let Some(output) = routing["outputs"].as_array().and_then(|outputs| {
            outputs
                .iter()
                .find(|output| output["id"].as_str() == Some(&id))
        }) {
            if network {
                continue;
            }
            if output["type"].as_str() == Some("speaker")
                && output["enabled"].as_bool() != Some(false)
            {
                let name = output["name"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty());
                let default = output["deviceId"].as_str() == Some("default");
                if default || name.is_some() {
                    let device = if default {
                        None
                    } else {
                        name.map(str::to_string)
                    };
                    *destinations
                        .entry((device, input, output_channel))
                        .or_default() += gain;
                }
            }
            continue;
        }
        if id.starts_with("speaker_sub_") {
            if network {
                continue;
            }
            // Chromium device IDs are opaque. The native app resolves this key
            // using its saved ID -> OS device-name mapping.
            *destinations
                .entry((Some(id), input, output_channel))
                .or_default() += gain;
            continue;
        }
        let mut targets = HashSet::new();
        for connection in connections {
            if connection["from"].as_str() != Some(&id)
                || connection["type"].as_str() == Some("sidechain")
            {
                continue;
            }
            let Some(to) = connection["to"].as_str() else {
                continue;
            };
            let speaker = to == "speaker_default"
                || to.starts_with("speaker_sub_")
                || routing["outputs"].as_array().is_some_and(|outputs| {
                    outputs.iter().any(|o| {
                        o["id"].as_str() == Some(to) && o["type"].as_str() == Some("speaker")
                    })
                });
            let index = |value: &Value| -> Result<Option<u16>, ()> {
                if value.is_null() {
                    return Ok(None);
                }
                value
                    .as_u64()
                    .filter(|index| *index < 64)
                    .map(|index| Some(index as u16))
                    .ok_or(())
            };
            let Ok(from_channel) = index(&connection["fromChannelIndex"]) else {
                continue;
            };
            let target_value = if !connection["toChannelIndex"].is_null() {
                &connection["toChannelIndex"]
            } else {
                &connection["channelIndex"]
            };
            let Ok(mut to_channel) = index(target_value) else {
                continue;
            };
            if !speaker && (from_channel.is_some() || to_channel.is_some()) {
                continue;
            }
            if to.starts_with("speaker_sub_") {
                to_channel = Some(to_channel.unwrap_or(0));
            }
            let mut from_channel = from_channel;
            if speaker && from_channel.is_none() && to_channel.is_some() {
                // FreeShow uses a splitter for multiple ports on the same
                // speaker, and a mono ChannelMerger input for a single port.
                let ports: HashSet<_> = connections
                    .iter()
                    .filter(|c| {
                        c["from"] == connection["from"]
                            && c["to"] == connection["to"]
                            && c["type"].as_str() != Some("sidechain")
                    })
                    .filter(|c| index(&c["fromChannelIndex"]).is_ok())
                    .filter_map(|c| {
                        let value = if !c["toChannelIndex"].is_null() {
                            &c["toChannelIndex"]
                        } else {
                            &c["channelIndex"]
                        };
                        index(value).ok().map(|index| index.unwrap_or(0))
                    })
                    .collect();
                if ports.len() > 1 {
                    from_channel = to_channel;
                }
            }
            if targets.insert((to, from_channel, to_channel)) && !visited.contains(to) {
                stack.push((
                    to.to_string(),
                    gain * channel_gain(to),
                    from_channel.or(input),
                    to_channel.or(output_channel),
                    visited.clone(),
                ));
            }
        }
    }
    destinations
        .into_iter()
        .map(
            |((device, input_channel, output_channel), gain)| AudioRoute {
                device,
                gain,
                input_channel,
                output_channel,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn network_mix_is_independent_from_room_and_disabled_outputs_stay_silent() {
        let mut settings = json!({"outputs":{"stream":{"enabled":true,"ndi":true}},
        "audioChannelsData":{"live":{"volume":0.5}},
        "audioRouting":{"connections":[
            {"from":"drawer_audio","to":"main"},{"from":"main","to":"speaker_default"},
            {"from":"drawer_audio","to":"live"},{"from":"live","to":"network_sub_stream"}
        ]}});
        assert_eq!(
            audio_routes(&settings, &["drawer_audio"], 0.0),
            vec![AudioRoute::stereo(None, 0.0)]
        );
        assert_eq!(
            network_audio_routes(&settings, &["drawer_audio"], 0.0),
            vec![freeshow_core::NetworkRoute {
                output_id: "stream".into(),
                gain: 0.5
            }]
        );
        settings["audioChannelsData"]["live"]["isMuted"] = json!(true);
        assert_eq!(
            network_audio_routes(&settings, &["drawer_audio"], 1.0)[0].gain,
            0.0
        );
        settings["outputs"]["stream"]["enabled"] = json!(false);
        assert!(network_audio_routes(&settings, &["drawer_audio"], 1.0).is_empty());
        assert!(network_audio_routes(&Value::Null, &["drawer_audio"], 1.0).is_empty());
    }

    #[test]
    fn physical_ports_keep_stereo_and_a_single_port_receives_a_mono_mix() {
        let mut settings = json!({"audioRouting":{"connections":[
            {"from":"drawer_audio","to":"main"},
            {"from":"main","to":"speaker_sub_x","channelIndex":0},
            {"from":"main","to":"speaker_sub_x","channelIndex":1},
            {"from":"main","to":"speaker_sub_x","channelIndex":1}
        ]}});
        let routes = audio_routes(&settings, &["drawer_audio"], 0.5);
        assert_eq!(routes.len(), 2);
        for (index, route) in routes.iter().enumerate() {
            assert_eq!(
                (route.input_channel, route.output_channel, route.gain),
                (Some(index as u16), Some(index as u16), 0.5)
            );
        }
        settings["audioRouting"]["connections"] = json!([
            {"from":"drawer_audio","to":"speaker_sub_x","toChannelIndex":3}
        ]);
        let routes = audio_routes(&settings, &["drawer_audio"], 1.0);
        assert_eq!(
            (routes[0].input_channel, routes[0].output_channel),
            (None, Some(3))
        );
        for value in [json!(-1), json!(64), json!(1.5), json!("2")] {
            settings["audioRouting"]["connections"][0]["toChannelIndex"] = value;
            assert!(audio_routes(&settings, &["drawer_audio"], 1.0).is_empty());
        }
    }

    #[test]
    fn secondary_routes_bypass_main_and_follow_their_own_gain_and_mute() {
        let mut settings = json!({
            "audioChannelsData": {"cue": {"volume": 0.4}},
            "audioRouting": {
                "channels": [{"id":"main"}, {"id":"cue"}],
                "outputs": [{"id":"speaker_sub_hash", "type":"speaker", "name":"Stage Speakers", "deviceId":"chrome-hash"}],
                "connections": [
                    {"from":"drawer_audio", "to":"cue"},
                    {"from":"cue", "to":"speaker_sub_hash"},
                    {"from":"cue", "to":"speaker_sub_hash"},
                    {"from":"cue", "to":"cue"},
                    {"from":"cue", "to":"main", "type":"sidechain"},
                    {"from":"main", "to":"speaker_default"}
                ]
            }
        });
        assert_eq!(
            audio_routes(&settings, &["drawer_audio"], 0.0),
            vec![AudioRoute {
                device: Some("Stage Speakers".into()),
                gain: 0.4,
                input_channel: None,
                output_channel: Some(0)
            }]
        );
        settings["audioChannelsData"]["cue"]["isMuted"] = json!(true);
        assert_eq!(audio_routes(&settings, &["drawer_audio"], 1.0)[0].gain, 0.0);
        settings["audioChannelsData"]["cue"]["isMuted"] = json!(false);
        settings["audioRouting"]["channels"][1]["outputLink"] = json!("stage");
        settings["outputs"] = json!({"stage":{"enabled":false}});
        assert_eq!(audio_routes(&settings, &["drawer_audio"], 1.0)[0].gain, 0.0);
        assert!(audio_routes(&settings, &["playlists_default"], 1.0).is_empty());
    }

    #[test]
    fn mixer_chains_sum_paths_and_disconnected_or_unsupported_sinks_stay_silent() {
        let mut settings = json!({
            "audioChannelsData": {"bus": {"volume":0.5}},
            "audioRouting": {"connections":[
                {"from":"drawer_audio", "to":"main"},
                {"from":"drawer_audio", "to":"bus"},
                {"from":"bus", "to":"main"},
                {"from":"main", "to":"speaker_default"}
            ]}
        });
        assert_eq!(
            audio_routes(&settings, &["drawer_audio"], 0.4),
            vec![AudioRoute {
                device: None,
                gain: 0.6,
                input_channel: None,
                output_channel: None
            }]
        );
        settings["audioRouting"]["connections"] = json!([
            {"from":"drawer_audio", "to":"network_sub_a"},
            {"from":"drawer_audio", "to":"speaker_default", "channelIndex":-1}
        ]);
        assert!(audio_routes(&settings, &["drawer_audio"], 1.0).is_empty());
        assert_eq!(
            audio_routes(&Value::Null, &["drawer_audio"], 0.3)[0].gain,
            0.3
        );
        assert!(
            audio_routes(
                &json!({"audioRouting":{"connections":null}}),
                &["drawer_audio"],
                1.0
            )
            .is_empty()
        );
    }
}
