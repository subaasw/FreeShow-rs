// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use freeshow_remote::{Integration, ShowBus};
use serde_json::{Value, json};

/// Listens on one MIDI input. An empty name uses the first connected port.
/// `FREESHOW_MIDI_IN` selects a port by name when this stays empty.
pub struct MidiInput {
    pub port: String,
}

impl Default for MidiInput {
    fn default() -> Self {
        Self {
            port: String::new(),
        }
    }
}

impl Integration for MidiInput {
    fn name(&self) -> &'static str {
        "midi"
    }

    fn start(&self, bus: ShowBus) {
        let wanted = if self.port.is_empty() {
            std::env::var("FREESHOW_MIDI_IN").unwrap_or_default()
        } else {
            self.port.clone()
        };
        let started = std::thread::Builder::new()
            .name("freeshow-midi".into())
            .spawn(move || listen(wanted, bus));
        if started.is_err() {
            tracing::error!("could not start the midi listener");
        }
    }
}

fn listen(wanted: String, bus: ShowBus) {
    let midi = match midir::MidiInput::new("freeshow") {
        Ok(midi) => midi,
        Err(err) => {
            tracing::error!("midi input is not available: {err}");
            return;
        }
    };
    let ports = midi.ports();
    if ports.is_empty() {
        tracing::info!("midi input: no ports");
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
                tracing::error!("midi input {wanted} is not connected");
                return;
            }
        }
    };
    let name = midi.port_name(&port).unwrap_or_else(|_| "midi".to_string());
    let connection = midi.connect(
        &port,
        "freeshow",
        move |_stamp, bytes, _| {
            if let Some(message) = message_for_bytes(bytes) {
                bus.apply(&message);
            }
        },
        (),
    );
    match connection {
        Ok(_connection) => {
            tracing::info!("midi input {name}");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        Err(err) => tracing::error!("midi input {name} did not open: {err}"),
    }
}

/// Map one MIDI message onto a FreeShow remote channel.
/// Channel 1 note 0 is next, note 1 is previous, channel 3 note 0 clears,
/// and channel 5 note 2 selects a slide from the note velocity.
pub fn message_for_bytes(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 3 {
        return None;
    }
    let status = bytes[0];
    let kind = status & 0xf0;
    let channel = (status & 0x0f) + 1;
    let data1 = bytes[1];
    let data2 = bytes[2];
    // A note-on with velocity 0 is a note-off.
    if kind != 0x90 || data2 == 0 {
        return None;
    }
    match (channel, data1) {
        (1, 0) => Some(json!({ "channel": "API:next_slide" })),
        (1, 1) => Some(json!({ "channel": "API:previous_slide" })),
        (3, 0 | 1 | 2) => Some(json!({ "channel": "API:clear_all" })),
        (5, 2) => {
            let index = data2.saturating_sub(1);
            Some(json!({ "channel": "API:index_select_slide", "data": { "index": index } }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use freeshow_remote::{Command, RemoteShow, RemoteSlide, ShowBus, ShowSnapshot};

    use super::*;

    #[test]
    fn default_notes_become_remote_channels() {
        assert_eq!(
            message_for_bytes(&[0x90, 0, 100]).unwrap()["channel"],
            "API:next_slide"
        );
        assert_eq!(
            message_for_bytes(&[0x90, 1, 100]).unwrap()["channel"],
            "API:previous_slide"
        );
        assert_eq!(
            message_for_bytes(&[0x92, 0, 100]).unwrap()["channel"],
            "API:clear_all"
        );
        assert_eq!(
            message_for_bytes(&[0x94, 2, 2]).unwrap()["data"]["index"],
            1
        );
        assert!(message_for_bytes(&[0x90, 0, 0]).is_none());
        assert!(message_for_bytes(&[0x80, 0, 100]).is_none());
        assert!(message_for_bytes(&[0x90, 9, 100]).is_none());
    }

    #[test]
    fn a_note_selects_the_slide() {
        let (commands, rx) = mpsc::channel();
        let snapshot = ShowSnapshot {
            shows: vec![RemoteShow {
                id: "grace".into(),
                name: "Amazing Grace".into(),
                layouts: Vec::new(),
                active_layout: String::new(),
                slides: vec![
                    RemoteSlide {
                        label: "one".into(),
                        lines: vec!["one".into()],
                        notes: String::new(),
                        id: String::new(),
                        group: String::new(),
                    },
                    RemoteSlide {
                        label: "two".into(),
                        lines: vec!["two".into()],
                        notes: String::new(),
                        id: String::new(),
                        group: String::new(),
                    },
                ],
            }],
            show_index: Some(0),
            slide_index: Some(0),
            cleared: false,
            project: Vec::new(),
            projects: Vec::new(),
            project_id: None,
            folders: Vec::new(),
            bibles: std::sync::Arc::new(Vec::new()),
            scripture_lines: Vec::new(),
            overlays: Vec::new(),
            active_overlays: Vec::new(),
            media: Vec::new(),
            audio: Vec::new(),
            templates: Vec::new(),
            timers: Vec::new(),
            active_timer: None,
            timer_paused: false,
            media_loop: true,
            media_paused: false,
            media_muted: true,
            video_time: 0.0,
            video_duration: 0.0,
            playing_audio: String::new(),
            audio_time: 0.0,
            audio_duration: 0.0,
            audio_paused: false,
            volume: 1.0,
            audio_mixer: serde_json::Value::Null,
            project_item: 0,
            next_item_on_last_slide: true,
        };
        let bus = ShowBus {
            snapshot: Arc::new(Mutex::new(snapshot)),
            commands,
        };
        let message = message_for_bytes(&[0x94, 2, 2]).unwrap();
        bus.apply(&message);
        assert_eq!(rx.try_recv().unwrap(), Command::SetSlide(1));
    }
}
