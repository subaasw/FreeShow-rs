// SPDX-License-Identifier: GPL-3.0-only

//! Adding and editing Calendar events, Player videos and RSS feeds from the drawer.

use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone};
use freeshow_data::{Catalog, Library, player_video_id};
use serde_json::{Value, json};

use super::Session;

/// What the drawer form shows for the picked Calendar event or Player video.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrawerForm {
    /// Calendar: name, date, time. Player: name, link. RSS: feed addresses.
    pub fields: [String; 3],
    /// Calendar: the show the event starts (0 = none, otherwise show index + 1).
    pub show: usize,
    /// True while an existing entry is being edited.
    pub editing: bool,
}

/// Calendar rows, soonest first, as `(event id, label)`. An event that starts a show names it.
pub(super) fn event_rows(
    catalog: &Catalog,
    shows: &[freeshow_data::ShowFile],
) -> (Vec<String>, Vec<String>) {
    let mut rows: Vec<(Option<chrono::DateTime<Local>>, String, String)> = catalog
        .entries()
        .into_iter()
        .filter_map(|(id, event)| {
            let name = event.get("name").and_then(Value::as_str)?.to_string();
            let when = super::event_time(&event).map(|time| time.with_timezone(&Local));
            let label = match when {
                Some(when) => {
                    // The year only when it is not this year, so names fit the drawer cell.
                    let day = if when.year() == Local::now().year() {
                        "%a %-d %b"
                    } else {
                        "%a %-d %b %Y"
                    };
                    let timed = event.get("time").and_then(Value::as_bool) != Some(false);
                    let format = if timed {
                        format!("{day} %H:%M")
                    } else {
                        day.to_string()
                    };
                    format!("{} · {name}", when.format(&format))
                }
                None => name,
            };
            let started = event
                .pointer("/action/data/id")
                .and_then(Value::as_str)
                .filter(|_| {
                    event.pointer("/action/id").and_then(Value::as_str) == Some("start_show")
                })
                .and_then(|show_id| shows.iter().find(|show| show.id == show_id))
                .map(freeshow_data::ShowFile::name)
                .filter(|name| !name.is_empty());
            let label = match started {
                Some(show) => format!("{label} ▸ {show}"),
                None => label,
            };
            Some((when, id, label))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    rows.into_iter().map(|(_, id, label)| (id, label)).unzip()
}

/// Local `YYYY-MM-DD` and optional `HH:MM` as FreeShow's stored UTC instant.
fn event_instant(date: &str, time: &str) -> Result<(String, bool), String> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map_err(|_| "write the date as YYYY-MM-DD, for example 2026-11-29".to_string())?;
    let (clock, timed) = if time.trim().is_empty() {
        (NaiveTime::MIN, false)
    } else {
        let parsed = NaiveTime::parse_from_str(time.trim(), "%H:%M")
            .map_err(|_| "write the time as HH:MM, for example 09:30".to_string())?;
        (parsed, true)
    };
    let local = Local
        .from_local_datetime(&date.and_time(clock))
        .earliest()
        .ok_or("that time does not exist on this day (clock change)")?;
    let utc = local.with_timezone(&chrono::Utc);
    Ok((utc.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(), timed))
}

impl Session {
    fn require_library(&self) -> Result<Library, String> {
        self.root
            .as_deref()
            .map(Library::open)
            .ok_or_else(|| "choose a library folder first".to_string())
    }

    pub(super) fn reload_events(&mut self) {
        if let Some(root) = &self.root
            && let Ok(catalog) = Library::open(root).load_events()
        {
            self.event_catalog = catalog;
        }
        let (ids, labels) = event_rows(&self.event_catalog, &self.shows);
        self.event_ids = ids;
        self.events = labels;
    }

    /// The form contents for the drawer's picked entry, or a blank form.
    pub fn drawer_form(&self) -> DrawerForm {
        match self.tab.as_str() {
            "calendar" => {
                let Some(event) = self
                    .drawer_pick
                    .and_then(|index| self.event_ids.get(index))
                    .and_then(|id| self.event_catalog.get(id))
                else {
                    let today = Local::now().format("%Y-%m-%d").to_string();
                    return DrawerForm {
                        fields: [String::new(), today, String::new()],
                        ..DrawerForm::default()
                    };
                };
                let when = super::event_time(event).map(|time| time.with_timezone(&Local));
                let timed = event.get("time").and_then(Value::as_bool) != Some(false);
                let show = super::scheduled_show(event)
                    .and_then(|id| self.shows.iter().position(|show| show.id == id))
                    .map_or(0, |index| index + 1);
                DrawerForm {
                    fields: [
                        event
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        when.map(|when| when.format("%Y-%m-%d").to_string())
                            .unwrap_or_default(),
                        when.filter(|_| timed)
                            .map(|when| when.format("%H:%M").to_string())
                            .unwrap_or_default(),
                    ],
                    show,
                    editing: true,
                }
            }
            "player" => match self
                .drawer_pick
                .and_then(|index| self.player_videos.get(index))
            {
                Some(video) => DrawerForm {
                    fields: [
                        video.name.clone(),
                        video.path.clone().unwrap_or_default(),
                        String::new(),
                    ],
                    show: 0,
                    editing: true,
                },
                None => DrawerForm::default(),
            },
            "rss" => DrawerForm {
                fields: [
                    crate::operator::rss_urls_from(&self.preferences).join(" "),
                    String::new(),
                    String::new(),
                ],
                show: 0,
                editing: false,
            },
            _ => DrawerForm::default(),
        }
    }

    /// Starts a blank form for a new Calendar event or Player video.
    pub(super) fn new_drawer_entry(&mut self) -> Result<String, String> {
        self.drawer_pick = None;
        Ok(String::new())
    }

    /// Saves the Calendar form: `name`, `date`, `time`, and `show` (0 = no show).
    pub(super) fn save_event(&mut self, arg: &str) -> Result<String, String> {
        let library = self.require_library()?;
        let mut parts = arg.split('\n');
        let name = parts.next().unwrap_or("").trim().to_string();
        let date = parts.next().unwrap_or("");
        let time = parts.next().unwrap_or("");
        let show: usize = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
        if name.is_empty() {
            return Err("give the event a name".into());
        }
        let (from, timed) = event_instant(date, time)?;
        let show_id = match show {
            0 => None,
            index => Some(
                self.shows
                    .get(index - 1)
                    .ok_or("that show is no longer in the library")?
                    .id
                    .clone(),
            ),
        };
        let mut catalog = library.load_events().map_err(|error| {
            format!("events.json could not be read, so it was left alone: {error}")
        })?;
        let picked = self
            .drawer_pick
            .and_then(|index| self.event_ids.get(index))
            .cloned();
        let id = picked.clone().unwrap_or_else(freeshow_data::new_id);
        // Keep notes, location, repeat and anything else FreeShow stored.
        let mut event = catalog
            .get(&id)
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "color": null, "repeat": false }));
        let entry = event.as_object_mut().expect("event object");
        entry.insert("name".into(), json!(name));
        entry.insert("from".into(), json!(from));
        entry.insert("to".into(), json!(from));
        entry.insert("time".into(), json!(timed));
        match show_id {
            Some(show_id) => {
                entry.insert("type".into(), json!("action"));
                entry.insert(
                    "action".into(),
                    json!({ "id": "start_show", "data": { "id": show_id } }),
                );
                entry.remove("show");
            }
            None => {
                if super::scheduled_show(&Value::Object(entry.clone())).is_some() {
                    entry.remove("action");
                    entry.remove("show");
                }
                entry.insert("type".into(), json!("event"));
            }
        }
        catalog.insert(&id, event);
        catalog
            .save(&library.events_path())
            .map_err(|error| format!("could not save the event: {error}"))?;
        // A moved event may fire again at its new time.
        self.fired_events.retain(|fired| fired != &id);
        self.reload_events();
        // An edited event stays picked; after adding, the form is blank for the next one.
        self.drawer_pick = picked
            .is_some()
            .then(|| self.event_ids.iter().position(|entry| entry == &id))
            .flatten();
        Ok(if picked.is_some() {
            format!("Saved {name}")
        } else {
            format!("Added {name} to the calendar")
        })
    }

    pub(super) fn delete_event(&mut self) -> Result<String, String> {
        let library = self.require_library()?;
        let id = self
            .drawer_pick
            .and_then(|index| self.event_ids.get(index))
            .cloned()
            .ok_or("click the event to delete first")?;
        let mut catalog = library.load_events().map_err(|error| {
            format!("events.json could not be read, so it was left alone: {error}")
        })?;
        let name = catalog
            .get(&id)
            .and_then(|event| event.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("the event")
            .to_string();
        catalog.remove(&id);
        catalog
            .save(&library.events_path())
            .map_err(|error| format!("could not delete the event: {error}"))?;
        self.drawer_pick = None;
        self.reload_events();
        Ok(format!("Deleted {name}"))
    }

    /// Saves the Player form: `name` and a YouTube, Vimeo, stream or file address.
    pub(super) fn save_player_video(&mut self, arg: &str) -> Result<String, String> {
        let library = self.require_library()?;
        let (name, link) = arg.split_once('\n').unwrap_or((arg, ""));
        let link = link.trim();
        if link.is_empty() {
            return Err("paste a YouTube or Vimeo link, a stream address or a file path".into());
        }
        let picked = self
            .drawer_pick
            .and_then(|index| self.player_videos.get(index))
            .map(|video| video.id.clone());
        let entry = match player_video_id(link) {
            Some((kind, video_id)) => {
                let name = match name.trim() {
                    "" if kind == "youtube" => format!("YouTube {video_id}"),
                    "" => format!("Vimeo {video_id}"),
                    name => name.to_string(),
                };
                json!({ "id": video_id, "name": name, "type": kind })
            }
            None => {
                let is_web = link.starts_with("http://") || link.starts_with("https://");
                if !is_web && !std::path::Path::new(link).is_file() {
                    return Err(format!("no file at {link}"));
                }
                let name = match name.trim() {
                    "" => link
                        .rsplit(['/', '\\'])
                        .find(|part| !part.is_empty())
                        .unwrap_or(link)
                        .to_string(),
                    name => name.to_string(),
                };
                json!({ "name": name, "type": "file", "path": link })
            }
        };
        let name = entry["name"].as_str().unwrap_or_default().to_string();
        let id = picked.clone().unwrap_or_else(freeshow_data::new_id);
        let saved = library.edit_synced_settings(|settings| {
            let videos = settings.entry("playerVideos").or_insert_with(|| json!({}));
            if !videos.is_object() {
                *videos = json!({});
            }
            let videos = videos.as_object_mut().expect("player videos");
            // Keep tags and other FreeShow fields on an edited entry.
            let mut merged = videos
                .get(&id)
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            let fields = merged.as_object_mut().expect("player video");
            fields.remove("path");
            fields.remove("url");
            fields.remove("id");
            for (key, value) in entry.as_object().expect("entry") {
                fields.insert(key.clone(), value.clone());
            }
            videos.insert(id.clone(), merged);
        });
        if !saved {
            return Err("settings_synced.json could not be read, so it was left alone".into());
        }
        self.load_player_videos();
        self.drawer_pick = picked
            .is_some()
            .then(|| self.player_videos.iter().position(|video| video.id == id))
            .flatten();
        Ok(if picked.is_some() {
            format!("Saved {name}")
        } else {
            format!("Added {name} to the player")
        })
    }

    pub(super) fn delete_player_video(&mut self) -> Result<String, String> {
        let library = self.require_library()?;
        let video = self
            .drawer_pick
            .and_then(|index| self.player_videos.get(index))
            .cloned()
            .ok_or("click the video to delete first")?;
        let removed = library.edit_synced_settings(|settings| {
            if let Some(videos) = settings
                .get_mut("playerVideos")
                .and_then(Value::as_object_mut)
            {
                videos.remove(&video.id);
            }
        });
        if !removed {
            return Err("settings_synced.json could not be read, so it was left alone".into());
        }
        if self.player_now.as_deref() == Some(video.name.as_str()) {
            self.player_now = None;
        }
        self.drawer_pick = None;
        self.load_player_videos();
        Ok(format!("Deleted {}", video.name))
    }

    /// Replaces the RSS feed list with the addresses in `arg` (spaces, commas or new lines).
    pub(super) fn save_rss_feeds(&mut self, arg: &str) -> Result<String, String> {
        let feeds: Vec<String> = arg
            .split(|c: char| c.is_whitespace() || c == ',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(bad) = feeds
            .iter()
            .find(|url| !(url.starts_with("http://") || url.starts_with("https://")))
        {
            return Err(format!(
                "{bad} is not a web address (start it with https://)"
            ));
        }
        let count = feeds.len();
        let saved = self.patch_settings(|settings| {
            let special = settings
                .as_object_mut()
                .expect("settings object")
                .entry("special")
                .or_insert_with(|| json!({}));
            if !special.is_object() {
                *special = json!({});
            }
            let special = special.as_object_mut().expect("special");
            special.remove("rssUrl");
            special.insert("rssFeeds".into(), json!(feeds));
        });
        if !saved {
            return Err("settings.json could not be saved".into());
        }
        // Fetch the new feeds now instead of at the next refresh.
        self.chrome.rss_items.clear();
        self.chrome.rss_fetched_at = None;
        Ok(match count {
            0 => "Removed the RSS feeds".to_string(),
            1 => "Saved 1 RSS feed; headlines arrive in a moment".to_string(),
            count => format!("Saved {count} RSS feeds; headlines arrive in a moment"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn library(name: &str) -> (Dir, Session) {
        let dir = std::env::temp_dir().join(format!(
            "freeshow-drawer-edit-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Shows")).unwrap();
        std::fs::create_dir_all(dir.join("Config")).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"special":{"guideSeen":true}}"#,
        )
        .unwrap();
        let session = Session::open(dir.clone());
        (Dir(dir), session)
    }

    #[test]
    fn calendar_events_are_added_edited_and_deleted_in_freeshow_format() {
        let (dir, mut session) = library("calendar");
        assert!(session.create_show());
        session.switch_tab("calendar");
        assert!(
            session
                .library_action("save-event", "\n2026-11-29\n09:30\n0")
                .is_err()
        );
        assert!(
            session
                .library_action("save-event", "Service\n29/11/2026\n\n0")
                .is_err()
        );
        session
            .library_action("save-event", "Service\n2026-11-29\n09:30\n1")
            .unwrap();
        session
            .library_action("save-event", "Lunch\n2026-11-28\n\n0")
            .unwrap();
        assert_eq!(session.events.len(), 2);
        assert!(
            session.events[0].ends_with("· Lunch"),
            "{:?}",
            session.events
        );
        assert!(session.events[1].contains("09:30 · Service"));

        let stored: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.0.join("Config/events.json")).unwrap(),
        )
        .unwrap();
        let service = stored
            .as_object()
            .unwrap()
            .values()
            .find(|event| event["name"] == "Service")
            .unwrap();
        assert_eq!(service["type"], "action");
        assert_eq!(service["action"]["id"], "start_show");
        assert_eq!(
            service["action"]["data"]["id"],
            session.shows[0].id.as_str()
        );
        assert_eq!(service["time"], true);
        assert!(super::super::event_time(service).is_some());

        // Picking a row loads it into the form; saving edits that event in place.
        let _ = session.select_drawer(1);
        let form = session.drawer_form();
        assert!(form.editing);
        assert_eq!(
            form.fields,
            [
                "Service".to_string(),
                "2026-11-29".to_string(),
                "09:30".to_string()
            ]
        );
        assert_eq!(form.show, 1);
        session
            .library_action("save-event", "Morning service\n2026-11-29\n10:00\n0")
            .unwrap();
        assert_eq!(session.event_catalog.entries().len(), 2);
        let form = session.drawer_form();
        assert_eq!(form.fields[0], "Morning service");
        assert_eq!(form.show, 0);
        let edited = session
            .event_catalog
            .entries()
            .into_iter()
            .find(|(_, event)| event["name"] == "Morning service")
            .unwrap()
            .1;
        assert_eq!(edited["type"], "event");
        assert!(edited.get("action").is_none());

        session.library_action("delete-event", "").unwrap();
        assert_eq!(session.events.len(), 1);
        assert!(!session.drawer_form().editing);
        assert!(session.library_action("delete-event", "").is_err());
    }

    #[test]
    fn player_videos_are_added_edited_and_deleted() {
        let (dir, mut session) = library("player");
        std::fs::write(
            dir.0.join("Config/settings_synced.json"),
            r#"{"variables":{"v":{"name":"Keep"}}}"#,
        )
        .unwrap();
        session.switch_tab("player");
        assert!(session.library_action("save-player", "Intro\n").is_err());
        assert!(
            session
                .library_action("save-player", "Clip\n/no/such/file.mp4")
                .is_err()
        );
        session
            .library_action("save-player", "\nhttps://youtu.be/dQw4w9WgXcQ")
            .unwrap();
        assert_eq!(session.drawer(), ["YouTube dQw4w9WgXcQ (YouTube)"]);
        let stored: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.0.join("Config/settings_synced.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["variables"]["v"]["name"], "Keep");
        let video = stored["playerVideos"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert_eq!(video["id"], "dQw4w9WgXcQ");
        assert_eq!(video["type"], "youtube");

        // Adding leaves a blank form; picking the row edits it.
        assert!(!session.drawer_form().editing);
        let _ = session.select_drawer(0);
        assert!(session.drawer_form().editing);
        session
            .library_action("save-player", "Welcome loop\nhttps://vimeo.com/144160599")
            .unwrap();
        assert_eq!(session.drawer(), ["Welcome loop (Vimeo)"]);
        assert_eq!(
            session.drawer_form().fields[1],
            "https://vimeo.com/144160599"
        );
        session.library_action("new-drawer-entry", "").unwrap();
        assert_eq!(session.drawer_form(), DrawerForm::default());
        session
            .library_action("save-player", "Stream\nhttps://cdn.example/live.m3u8")
            .unwrap();
        assert_eq!(session.player_videos.len(), 2);
        let stream = session
            .player_videos
            .iter()
            .position(|video| video.name == "Stream")
            .unwrap();
        let _ = session.select_drawer(stream);
        session.library_action("delete-player", "").unwrap();
        assert_eq!(session.drawer(), ["Welcome loop (Vimeo)"]);
    }

    #[test]
    fn rss_feeds_are_saved_to_settings() {
        let (dir, mut session) = library("rss");
        session.switch_tab("rss");
        assert!(
            session
                .library_action("save-rss", "example.com/feed")
                .is_err()
        );
        session
            .library_action(
                "save-rss",
                "https://example.com/a.xml, https://example.com/b.xml",
            )
            .unwrap();
        assert_eq!(
            session.drawer_form().fields[0],
            "https://example.com/a.xml https://example.com/b.xml"
        );
        let stored: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.0.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(stored["special"]["guideSeen"], true);
        assert_eq!(
            stored["special"]["rssFeeds"][1],
            "https://example.com/b.xml"
        );
        session.library_action("save-rss", " ").unwrap();
        assert_eq!(session.drawer_form().fields[0], "");
    }
}
