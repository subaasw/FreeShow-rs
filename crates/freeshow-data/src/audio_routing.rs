// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

/// A physical speaker destination. `None` is the system default device.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRoute {
    pub device: Option<String>,
    pub gain: f32,
}

/// Resolves FreeShow's source -> mixer -> speaker graph. An explicit graph with
/// no speaker path stays silent; network destinations never fall back to speakers.
pub fn audio_routes(settings: &Value, sources: &[&str], main_gain: f32) -> Vec<AudioRoute> {
    let routing = &settings["audioRouting"];
    let Some(connections) = routing["connections"].as_array() else {
        return if routing.is_null() {
            vec![AudioRoute {
                device: None,
                gain: main_gain.clamp(0.0, 1.0),
            }]
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
            .or_else(|| id.strip_prefix("channel_"));
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
    let mut destinations = BTreeMap::<Option<String>, f32>::new();
    let mut stack: Vec<_> = sources
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|source| (source.to_string(), channel_gain(source), HashSet::new()))
        .collect();
    // ponytail: enumerate acyclic paths; use topological propagation if large
    // routing graphs make path enumeration expensive.
    while let Some((id, gain, mut visited)) = stack.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        if id == "speaker_default" {
            *destinations.entry(None).or_default() += gain;
            continue;
        }
        if let Some(output) = routing["outputs"].as_array().and_then(|outputs| {
            outputs
                .iter()
                .find(|output| output["id"].as_str() == Some(&id))
        }) {
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
                    *destinations.entry(device).or_default() += gain;
                }
            }
            continue;
        }
        if id.starts_with("speaker_sub_") {
            // Chromium device IDs are opaque. The native app resolves this key
            // using its saved ID -> OS device-name mapping.
            *destinations.entry(Some(id)).or_default() += gain;
            continue;
        }
        let mut targets = HashSet::new();
        for connection in connections {
            if connection["from"].as_str() != Some(&id)
                || connection["type"].as_str() == Some("sidechain")
                // Hardware channel selection needs a channel-aware sink. Do not
                // accidentally play a cue in every channel of that device.
                || ["channelIndex", "fromChannelIndex", "toChannelIndex"].iter()
                    .any(|key| !connection[key].is_null())
            {
                continue;
            }
            if let Some(to) = connection["to"].as_str() {
                if targets.insert(to) && !visited.contains(to) {
                    stack.push((to.to_string(), gain * channel_gain(to), visited.clone()));
                }
            }
        }
    }
    destinations
        .into_iter()
        .map(|(device, gain)| AudioRoute { device, gain })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                gain: 0.4
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
                gain: 0.6
            }]
        );
        settings["audioRouting"]["connections"] = json!([
            {"from":"drawer_audio", "to":"network_sub_a"},
            {"from":"drawer_audio", "to":"speaker_default", "channelIndex":1}
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
