// SPDX-License-Identifier: GPL-3.0-only

//! Stream Deck USB HID. Enumeration is pure Rust (Linux hidraw / optional
//! `hid` feature). Missing devices stay silent.

/// Elgato / Corsair Stream Deck USB vendor id.
#[cfg(target_os = "linux")]
pub const ELGATO_VID: u16 = 0x0FD9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDeckDevice {
    pub path: String,
    pub product_id: u16,
    pub name: String,
    pub keys: u8,
}

impl StreamDeckDevice {
    pub fn kind(&self) -> &'static str {
        stream_deck_kind(self.product_id)
    }
}

pub fn stream_deck_kind(product_id: u16) -> &'static str {
    match product_id {
        0x0060 | 0x006D => "Stream Deck",
        0x0063 | 0x0090 => "Stream Deck Mini",
        0x006C | 0x008F => "Stream Deck XL",
        0x0080 => "Stream Deck Pedal",
        0x0084 | 0x009B => "Stream Deck Plus",
        0x009A => "Stream Deck Neo",
        0x00B8 => "Stream Deck MK.2",
        _ => "Stream Deck",
    }
}

#[cfg(any(target_os = "linux", test))]
pub fn key_count(product_id: u16) -> u8 {
    match product_id {
        0x0063 | 0x0090 => 6,
        0x0060 | 0x006D | 0x00B8 => 15,
        0x0084 | 0x009B => 8,
        0x009A => 8,
        0x006C | 0x008F => 32,
        0x0080 => 3,
        _ => 15,
    }
}

/// Decode an interrupt report into pressed keys (true = down).
pub fn parse_key_report(bytes: &[u8], keys: u8) -> Vec<bool> {
    let mut pressed = vec![false; keys as usize];
    if bytes.is_empty() {
        return pressed;
    }
    let data = if bytes[0] == 0x01 || bytes[0] == 0x00 {
        &bytes[1..]
    } else {
        bytes
    };
    for (index, slot) in pressed.iter_mut().enumerate() {
        *slot = data.get(index).copied().is_some_and(|value| value != 0);
    }
    pressed
}

/// Map a newly pressed key to a short Companion/REST action name.
pub fn action_for_key(index: usize) -> &'static str {
    match index {
        0 => "next",
        1 => "previous",
        2 => "clear",
        3 => "clear_slide",
        _ => "next",
    }
}

/// Keys that transitioned from up to down.
pub fn newly_pressed(previous: &[bool], current: &[bool]) -> Vec<usize> {
    current
        .iter()
        .enumerate()
        .filter(|(index, down)| **down && !previous.get(*index).copied().unwrap_or(false))
        .map(|(index, _)| index)
        .collect()
}

/// Known Stream Decks on this machine. Empty when none are attached or the
/// path is not readable. Never fails the show.
pub fn list_devices() -> Vec<StreamDeckDevice> {
    #[cfg(target_os = "linux")]
    {
        list_hidraw()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn list_hidraw() -> Vec<StreamDeckDevice> {
    let Ok(entries) = std::fs::read_dir("/sys/class/hidraw") else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let uevent = entry.path().join("device/uevent");
        let Ok(text) = std::fs::read_to_string(&uevent) else {
            continue;
        };
        let Some((vid, pid)) = hid_ids(&text) else {
            continue;
        };
        if vid != ELGATO_VID {
            continue;
        }
        let node = format!("/dev/{}", name.to_string_lossy());
        devices.push(StreamDeckDevice {
            path: node,
            product_id: pid,
            name: stream_deck_kind(pid).to_string(),
            keys: key_count(pid),
        });
    }
    devices
}

#[cfg(any(target_os = "linux", test))]
fn hid_ids(uevent: &str) -> Option<(u16, u16)> {
    for line in uevent.lines() {
        let Some(value) = line.strip_prefix("HID_ID=") else {
            continue;
        };
        // HID_ID=0003:00000FD9:0000006D
        let mut parts = value.split(':');
        let _bus = parts.next()?;
        let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
        let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
        return Some((vid, pid));
    }
    None
}

/// Start polling the first Stream Deck when the `hid` feature is on. No-op otherwise.
pub fn start_listener(on_action: impl Fn(&str) + Send + 'static) -> bool {
    let Some(device) = list_devices().into_iter().next() else {
        return false;
    };
    let _ = (
        device.kind(),
        parse_key_report(&[], device.keys),
        action_for_key(0),
        newly_pressed(&[], &[]),
    );
    #[cfg(not(feature = "hid"))]
    {
        let _ = (device, on_action);
        false
    }
    #[cfg(feature = "hid")]
    {
        poll_device(device, on_action);
        true
    }
}

#[cfg(feature = "hid")]
fn poll_device(device: StreamDeckDevice, on_action: impl Fn(&str) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("freeshow-streamdeck".into())
        .spawn(move || {
            let Ok(mut file) = std::fs::File::open(&device.path) else {
                return;
            };
            use std::io::Read;
            let mut previous = vec![false; device.keys as usize];
            let mut buf = [0u8; 512];
            loop {
                let Ok(read) = file.read(&mut buf) else {
                    break;
                };
                if read == 0 {
                    break;
                }
                let current = parse_key_report(&buf[..read], device.keys);
                for index in newly_pressed(&previous, &current) {
                    on_action(action_for_key(index));
                }
                previous = current;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_reports_map_to_companion_actions() {
        let mut bytes = vec![0x01, 1, 0, 0, 0];
        let keys = parse_key_report(&bytes, 4);
        assert_eq!(keys, vec![true, false, false, false]);
        assert_eq!(action_for_key(0), "next");
        assert_eq!(newly_pressed(&[false, false], &[true, false]), vec![0]);
        assert!(newly_pressed(&[true, false], &[true, false]).is_empty());
        bytes[1] = 0;
        bytes[2] = 1;
        assert!(parse_key_report(&bytes, 4)[1]);
        assert_eq!(action_for_key(1), "previous");
        // Enumeration must not fail when no deck is attached.
        let _ = list_devices();
        assert_eq!(
            hid_ids("HID_ID=0003:00000FD9:0000006D\n"),
            Some((0x0FD9, 0x006D))
        );
        assert_eq!(key_count(0x006C), 32);
        let _ = list_devices();
        let _ = start_listener(|_| {});
        assert_eq!(stream_deck_kind(0x009A), "Stream Deck Neo");
    }
}
