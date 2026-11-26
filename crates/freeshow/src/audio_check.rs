// SPDX-License-Identifier: GPL-3.0-only

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use freeshow_core::{AppSettings, SpeakerRoute};
use freeshow_data::Library;
use freeshow_media::AudioOutputDevice;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AudioCheckRow, AudioMappingRow, ControlWindow, Session, strings};

const LIBRARY_NAME: &str = "Use library device name";

struct DeviceInventory {
    default: Option<AudioOutputDevice>,
    audio: Vec<AudioOutputDevice>,
    video: Vec<String>,
    video_default_sink: bool,
}

impl DeviceInventory {
    fn scan() -> Self {
        let (default, audio) = freeshow_media::audio_output_inventory();
        let video = freeshow_media::video_output_devices();
        Self {
            default,
            audio,
            video,
            video_default_sink: freeshow_media::video_default_audio_sink_available(),
        }
    }

    fn choices(&self, paths: DevicePaths) -> Vec<String> {
        let relevant: BTreeSet<_> = if paths.audio {
            self.audio
                .iter()
                .map(|device| device.name.as_str())
                .collect()
        } else {
            self.video.iter().map(String::as_str).collect()
        };
        relevant
            .into_iter()
            .filter(|name| {
                (!paths.audio
                    || self
                        .audio
                        .iter()
                        .filter(|device| device.name == *name)
                        .count()
                        == 1)
                    && (!paths.video
                        || self.video.iter().filter(|device| *device == name).count() == 1)
            })
            .map(str::to_owned)
            .collect()
    }

    fn describe(&self, device: Option<&str>, video: bool, port: Option<u16>) -> String {
        if video {
            let Some(device) = device else {
                return if self.video_default_sink {
                    "Default video audio sink available. Verify with a clip.".into()
                } else {
                    "Default video audio sink unavailable".into()
                };
            };
            return match self.video.iter().filter(|name| *name == device).count() {
                0 => "Device missing from the video audio list".into(),
                1 => "Device listed for video. Verify the port with a clip.".into(),
                _ => "Multiple video devices have this name".into(),
            };
        }
        let found = if let Some(name) = device {
            let mut matches = self.audio.iter().filter(|device| device.name == name);
            let first = matches.next();
            if matches.next().is_some() {
                return "Multiple audio devices have this name".into();
            }
            first
        } else {
            self.default.as_ref()
        };
        let Some(device) = found else {
            return "Audio device unavailable".into();
        };
        match device.max_channels {
            Some(max) if port.is_some_and(|port| port >= max) => {
                format!("Channel unavailable. Device reports {max} output channels.")
            }
            Some(max) => format!("Device listed · {max} output channels · {}", device.name),
            None => format!("No output format reported for {}", device.name),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct DevicePaths {
    audio: bool,
    video: bool,
}

struct SourceRoutes {
    label: String,
    video: bool,
    speakers: Vec<SpeakerRoute>,
    network_count: usize,
}

fn check_sources(session: &Session) -> Vec<SourceRoutes> {
    let library = session.root.as_deref().map(Library::open);
    let settings = library
        .as_ref()
        .and_then(|library| std::fs::read(library.settings_path()).ok())
        .and_then(|text| serde_json::from_slice::<Value>(&text).ok())
        .unwrap_or_default();
    let main = if session.audio_muted {
        0.0
    } else {
        session.volume
    };
    let collect = |label: String, video, sources: &[&str]| SourceRoutes {
        label,
        video,
        speakers: freeshow_data::audio_routes(&settings, sources, main),
        network_count: freeshow_data::network_audio_routes(&settings, sources, main).len(),
    };
    let (audience, _) = session.audience();
    let mut groups = vec![
        collect("Audio files".into(), false, &["drawer_audio"]),
        collect(
            "Audience video".into(),
            true,
            &["output_window", &format!("output_win_sub_{audience}")],
        ),
        collect("Playlist defaults".into(), false, &["playlists_default"]),
    ];
    let playlists: BTreeSet<_> = settings["audioRouting"]["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edge| edge["from"].as_str())
        .filter_map(|source| source.strip_prefix("playlist_sub_"))
        .collect();
    for id in playlists {
        let name = library
            .as_ref()
            .and_then(|library| library.playlist(id))
            .map_or_else(|| id.to_owned(), |playlist| playlist.name);
        groups.push(collect(
            format!("Playlist · {name}"),
            false,
            &[&format!("playlist_sub_{id}"), "playlists_default"],
        ));
    }
    groups
}

fn device_paths(groups: &[SourceRoutes]) -> BTreeMap<String, DevicePaths> {
    let mut keys = BTreeMap::<_, DevicePaths>::new();
    for group in groups {
        for device in group
            .speakers
            .iter()
            .filter_map(|route| route.device.as_ref())
        {
            let paths = keys.entry(device.clone()).or_default();
            paths.audio |= !group.video;
            paths.video |= group.video;
        }
    }
    keys
}

fn update_report(
    ui: &ControlWindow,
    groups: &[SourceRoutes],
    running: &BTreeMap<String, String>,
    saved: &BTreeMap<String, String>,
    inventory: &DeviceInventory,
) {
    let mut rows = Vec::new();
    for group in groups {
        if group.speakers.is_empty() {
            rows.push(AudioCheckRow {
                source: group.label.clone().into(),
                destination: "No local speaker route".into(),
                detail: if group.network_count == 0 {
                    "No configured audio destination".into()
                } else {
                    format!("{} configured network destinations", group.network_count).into()
                },
            });
        }
        for route in &group.speakers {
            let device = route
                .device
                .as_deref()
                .map(|key| running.get(key).map_or(key, String::as_str));
            let mut destination = device.unwrap_or("System default").to_owned();
            if let Some(port) = route.output_channel {
                use std::fmt::Write as _;
                let _ = write!(destination, " · port {}", port + 1);
            }
            if let Some(channel) = route.input_channel {
                use std::fmt::Write as _;
                let _ = write!(destination, " · input {}", channel + 1);
            }
            let level = if route.gain <= 0.0 {
                "Silent at current mixer settings".into()
            } else {
                format!("Mixer {:.0}%", route.gain * 100.0)
            };
            rows.push(AudioCheckRow {
                source: group.label.clone().into(),
                destination: destination.into(),
                detail: format!(
                    "{} · {level}",
                    inventory.describe(device, group.video, route.output_channel)
                )
                .into(),
            });
        }
        if !group.speakers.is_empty() && group.network_count > 0 {
            rows.push(AudioCheckRow {
                source: group.label.clone().into(),
                destination: "Network audio".into(),
                detail: format!("{} configured destinations", group.network_count).into(),
            });
        }
    }
    let mappings = device_paths(groups)
        .into_iter()
        .map(|(key, paths)| {
            let mut choices = vec![LIBRARY_NAME.to_owned()];
            choices.extend(inventory.choices(paths));
            let choice = saved.get(&key).map_or(LIBRARY_NAME, String::as_str);
            if !choices.iter().any(|value| value == choice) {
                choices.push(choice.to_owned());
            }
            let pending = running.get(&key) != saved.get(&key);
            AudioMappingRow {
                key: key.into(),
                choice: choice.into(),
                choices: strings(&choices),
                pending,
            }
        })
        .collect::<Vec<_>>();
    ui.set_audio_check_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_audio_mapping_rows(ModelRc::new(VecModel::from(mappings)));
}

pub fn wire(
    ui: &ControlWindow,
    session: Rc<RefCell<Session>>,
    settings: Rc<RefCell<AppSettings>>,
    settings_path: PathBuf,
) -> slint::Timer {
    let (sender, received) = mpsc::channel();
    let inventory = Rc::<RefCell<Option<DeviceInventory>>>::default();
    let scan_ui = ui.as_weak();
    ui.on_scan_audio_devices(move || {
        let Some(ui) = scan_ui.upgrade() else { return };
        if ui.get_audio_scan_running() {
            return;
        }
        ui.set_audio_scan_running(true);
        ui.set_audio_check_status("Scanning output devices…".into());
        let sender = sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("freeshow-audio-check".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(DeviceInventory::scan);
                let _ = sender.send(result.ok());
            })
        {
            ui.set_audio_scan_running(false);
            ui.set_audio_check_status(format!("Could not start the device scan. {error}").into());
        }
    });

    let setter_ui = ui.as_weak();
    let setter_session = Rc::clone(&session);
    let setter_settings = Rc::clone(&settings);
    let setter_inventory = Rc::clone(&inventory);
    ui.on_save_audio_mapping(move |key, name| {
        let Some(ui) = setter_ui.upgrade() else {
            return;
        };
        let scan = setter_inventory.borrow();
        let Some(inventory) = scan.as_ref() else {
            return;
        };
        let session = setter_session.borrow();
        let groups = check_sources(&session);
        let paths = device_paths(&groups);
        let mut settings = setter_settings.borrow_mut();
        let choices = paths
            .get(key.as_str())
            .map(|paths| inventory.choices(*paths));
        if choices.as_ref().is_none_or(|choices| {
            !name.is_empty() && !choices.iter().any(|choice| choice == name.as_str())
        }) {
            ui.set_audio_check_status(
                "Choose a device listed for this route and scan again if devices changed.".into(),
            );
            update_report(
                &ui,
                &groups,
                &session.audio_device_map,
                &settings.audio_devices,
                inventory,
            );
            return;
        }
        let mut candidate = settings.clone();
        if name.is_empty() {
            candidate.audio_devices.remove(key.as_str());
        } else {
            candidate
                .audio_devices
                .insert(key.to_string(), name.to_string());
        }
        match candidate.save(&settings_path) {
            Ok(()) => {
                *settings = candidate;
                let notice = if settings.audio_devices == session.audio_device_map {
                    "Saved mappings match current playback."
                } else {
                    "Mapping saved. Restart FreeShow to apply it."
                };
                ui.set_audio_check_status(notice.into());
            }
            Err(error) => {
                ui.set_audio_check_status(format!("Could not save audio mapping. {error}").into())
            }
        }
        update_report(
            &ui,
            &groups,
            &session.audio_device_map,
            &settings.audio_devices,
            inventory,
        );
    });

    let scan_ui = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let Ok(result) = received.try_recv() else {
                return;
            };
            let Some(ui) = scan_ui.upgrade() else { return };
            ui.set_audio_scan_running(false);
            let Some(scanned) = result else {
                ui.set_audio_check_status("Device scan failed. Try scanning again.".into());
                return;
            };
            let session = session.borrow();
            update_report(
                &ui,
                &check_sources(&session),
                &session.audio_device_map,
                &settings.borrow().audio_devices,
                &scanned,
            );
            ui.set_audio_check_status(
                format!(
                    "{} audio-file devices · {} video audio devices",
                    scanned.audio.len(),
                    scanned.video.len()
                )
                .into(),
            );
            *inventory.borrow_mut() = Some(scanned);
        },
    );
    timer
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn audio_check_reads_playback_routes_and_distinguishes_missing_devices_and_ports() {
        let root =
            std::env::temp_dir().join(format!("freeshow-audio-check-{}", freeshow_data::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            Library::open(&root).settings_path(),
            json!({"audioRouting":{"connections":[
                {"from":"drawer_audio","to":"main"},
                {"from":"main","to":"speaker_sub_amp","channelIndex":3},
                {"from":"output_window","to":"speaker_sub_amp"},
                {"from":"playlists_default","to":"network_sub_stream"}
            ]}})
            .to_string(),
        )
        .unwrap();
        let mut session = Session::open(root.clone());
        session.volume = 0.5;
        session
            .audio_device_map
            .insert("speaker_sub_amp".into(), "Amplifier".into());
        let groups = check_sources(&session);
        let audio = &groups[0].speakers[0];
        assert_eq!(audio.device.as_deref(), Some("speaker_sub_amp"));
        assert_eq!(audio.gain, 0.5);
        assert_eq!(audio.output_channel, Some(3));
        let runtime = session.audio_routes();
        assert_eq!(runtime[0].device.as_deref(), Some("Amplifier"));
        assert_eq!(runtime[0].output_channel, audio.output_channel);
        assert!(groups[2].speakers.is_empty());
        assert_eq!(groups[2].network_count, 1);

        let inventory = DeviceInventory {
            default: None,
            audio: vec![
                AudioOutputDevice {
                    name: "Amplifier".into(),
                    max_channels: Some(2),
                },
                AudioOutputDevice {
                    name: "Headphones".into(),
                    max_channels: Some(2),
                },
            ],
            video: vec!["Amplifier".into()],
            video_default_sink: true,
        };
        let keys = device_paths(&groups);
        assert_eq!(inventory.choices(keys["speaker_sub_amp"]), ["Amplifier"]);
        assert!(
            inventory
                .describe(Some("Amplifier"), false, audio.output_channel)
                .contains("Channel unavailable")
        );
        assert!(
            inventory
                .describe(Some("Headphones"), true, None)
                .contains("missing")
        );
        assert!(
            inventory
                .describe(None, false, None)
                .contains("unavailable")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
