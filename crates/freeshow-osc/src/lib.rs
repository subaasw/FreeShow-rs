// SPDX-License-Identifier: GPL-3.0-only

use std::net::UdpSocket;

use freeshow_remote::ShowBus;
use rosc::{OscPacket, decoder, encoder};
use serde_json::{Value, json};

/// FreeShow listens for OSC on the same port as its WebSocket API.
pub const DEFAULT_PORT: u16 = 5505;

pub use freeshow_remote::Integration;

pub struct OscInput {
    pub port: u16,
}

impl Default for OscInput {
    fn default() -> Self {
        Self { port: DEFAULT_PORT }
    }
}

impl Integration for OscInput {
    fn name(&self) -> &'static str {
        "osc"
    }

    fn start(&self, bus: ShowBus) {
        let port = self.port;
        let started = std::thread::Builder::new()
            .name("freeshow-osc".into())
            .spawn(move || match UdpSocket::bind(("0.0.0.0", port)) {
                Ok(socket) => {
                    tracing::info!("osc on udp {port}");
                    serve_socket(socket, bus);
                }
                Err(err) => tracing::error!("osc port {port} is not available: {err}"),
            });
        if started.is_err() {
            tracing::error!("could not start the osc listener");
        }
    }
}

pub fn serve_socket(socket: UdpSocket, bus: ShowBus) {
    let mut buffer = [0u8; 2048];
    while let Ok(size) = socket.recv(&mut buffer) {
        for message in messages_in_packet(&buffer[..size]) {
            bus.apply(&message);
        }
    }
}

pub fn messages_in_packet(bytes: &[u8]) -> Vec<Value> {
    let Ok((_, packet)) = decoder::decode_udp(bytes) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    collect(packet, &mut messages);
    messages
}

fn collect(packet: OscPacket, messages: &mut Vec<Value>) {
    match packet {
        OscPacket::Message(message) => {
            if let Some(value) = message_for_address(&message.addr) {
                messages.push(value);
            }
        }
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                collect(packet, messages);
            }
        }
    }
}

/// Map a FreeShow OSC address onto a remote channel message.
/// `/freeshow/slide/next` and `/slide/next` are the same action.
pub fn message_for_address(address: &str) -> Option<Value> {
    let path = address
        .strip_prefix("/freeshow")
        .unwrap_or(address)
        .trim_matches('/');
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        ["slide", "next"] => Some(json!({ "channel": "API:next_slide" })),
        ["slide", "previous"] => Some(json!({ "channel": "API:previous_slide" })),
        ["slide", index] => index.parse::<usize>().ok().map(
            |index| json!({ "channel": "API:index_select_slide", "data": { "index": index } }),
        ),
        ["clear", "all" | "slide" | "background"] => Some(json!({ "channel": "API:clear_all" })),
        ["show", id, "open" | "start"] => Some(json!({ "channel": "SHOW", "data": id })),
        _ => None,
    }
}

pub fn encode_address(address: &str) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(rosc::OscMessage {
        addr: address.to_string(),
        args: Vec::new(),
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use freeshow_remote::{Command, RemoteShow, RemoteSlide, ShowSnapshot};

    use super::*;

    fn bus() -> (ShowBus, mpsc::Receiver<Command>) {
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
            taken_slide: None,
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
        (
            ShowBus {
                snapshot: Arc::new(Mutex::new(snapshot)),
                commands,
            },
            rx,
        )
    }

    #[test]
    fn freeshow_slide_paths_become_remote_channels() {
        assert_eq!(
            message_for_address("/freeshow/slide/next").unwrap()["channel"],
            "API:next_slide"
        );
        assert_eq!(
            message_for_address("/slide/previous").unwrap()["channel"],
            "API:previous_slide"
        );
        assert_eq!(
            message_for_address("/freeshow/slide/1").unwrap()["data"]["index"],
            1
        );
        assert_eq!(
            message_for_address("/freeshow/clear/all").unwrap()["channel"],
            "API:clear_all"
        );
        assert_eq!(
            message_for_address("/freeshow/show/grace/start").unwrap()["data"],
            "grace"
        );
        assert!(message_for_address("/freeshow/nope").is_none());
    }

    #[test]
    fn a_udp_packet_advances_the_slide() {
        let (bus, rx) = bus();
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || serve_socket(listener, bus));
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(&encode_address("/freeshow/slide/next"), ("127.0.0.1", port))
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Command::SetSlide(1)
        );
    }
}
