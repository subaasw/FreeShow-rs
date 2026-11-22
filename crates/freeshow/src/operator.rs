// SPDX-License-Identifier: GPL-3.0-only

//! Settings, inspector, stage-editor, profile login, weather, and RSS chrome.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::feeds::{RssItem, WeatherReport};
use crate::update::ReleaseUpdate;

static WEATHER_CACHE: Mutex<Vec<((i64, i64), crate::feeds::WeatherReport)>> =
    Mutex::new(Vec::new());

pub fn cached_weather(latitude: f64, longitude: f64) -> Option<WeatherReport> {
    let key = weather_key(latitude, longitude);
    WEATHER_CACHE
        .lock()
        .ok()?
        .iter()
        .find(|(stored, _)| *stored == key)
        .map(|(_, report)| report.clone())
}

fn publish_weather(report: &WeatherReport) {
    let key = weather_key(report.latitude, report.longitude);
    if let Ok(mut cache) = WEATHER_CACHE.lock() {
        if let Some(existing) = cache.iter_mut().find(|(stored, _)| *stored == key) {
            existing.1 = report.clone();
        } else {
            cache.push((key, report.clone()));
        }
    }
}

/// Operator extras that do not belong on the hot Go/Next path.
#[derive(Debug, Clone, Default)]
pub struct SessionChrome {
    pub weather: Vec<((i64, i64), WeatherReport)>,
    pub weather_fetched_at: Option<Instant>,
    pub rss_items: Vec<RssItem>,
    pub rss_fetched_at: Option<Instant>,
    pub active_profile: Option<String>,
    pub profile_locked: bool,
    pub update_alert: Option<String>,
}

impl SessionChrome {
    pub fn weather_for(&self, latitude: f64, longitude: f64) -> Option<&WeatherReport> {
        let key = weather_key(latitude, longitude);
        self.weather
            .iter()
            .find(|(stored, _)| *stored == key)
            .map(|(_, report)| report)
    }

    pub fn store_weather(&mut self, report: WeatherReport) {
        let key = weather_key(report.latitude, report.longitude);
        publish_weather(&report);
        if let Some(existing) = self.weather.iter_mut().find(|(stored, _)| *stored == key) {
            existing.1 = report;
        } else {
            self.weather.push((key, report));
        }
        self.weather_fetched_at = Some(Instant::now());
    }

    pub fn ticker(&self) -> String {
        self.rss_items
            .iter()
            .map(|item| item.title.as_str())
            .filter(|title| !title.is_empty())
            .take(5)
            .collect::<Vec<_>>()
            .join("  ·  ")
    }
}

fn weather_key(latitude: f64, longitude: f64) -> (i64, i64) {
    (
        (latitude * 100.0).round() as i64,
        (longitude * 100.0).round() as i64,
    )
}

/// An access-control profile stored in synced `profiles`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessProfile {
    pub id: String,
    pub name: String,
    pub password: Option<String>,
    pub color: String,
}

impl AccessProfile {
    pub fn allows_login(&self, password: &str) -> bool {
        match self.password.as_deref().filter(|value| !value.is_empty()) {
            None => true,
            Some(expected) => expected == password,
        }
    }
}

pub fn profiles_from(value: &serde_json::Value) -> Vec<AccessProfile> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    for (id, entry) in map {
        let name = entry
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(id)
            .trim();
        if name.is_empty() {
            continue;
        }
        let password = entry
            .get("password")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let color = entry
            .get("color")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        profiles.push(AccessProfile {
            id: id.clone(),
            name: name.to_string(),
            password,
            color,
        });
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    profiles
}

pub fn settings_text(
    preferences: &freeshow_data::FreeShowPreferences,
    chrome: &SessionChrome,
) -> String {
    let mut lines = vec![
        format!(
            "general.language={}",
            preferences.language.as_deref().unwrap_or("en")
        ),
        format!(
            "general.timeFormat={}",
            preferences.time_format.as_deref().unwrap_or("24")
        ),
        format!(
            "general.autosave={}",
            preferences.autosave.as_deref().unwrap_or("15min")
        ),
        format!("general.alertUpdates={}", preferences.alert_updates()),
        format!(
            "display.autoOutput={}",
            preferences.auto_output.unwrap_or(false)
        ),
        format!("display.outLocked={}", preferences.out_locked()),
        format!(
            "display.fullColors={}",
            preferences.full_colors.unwrap_or(false)
        ),
        format!(
            "display.splitLines={}",
            preferences.split_lines.unwrap_or(0)
        ),
        "audio.volume labels follow mixer routes".to_string(),
        format!(
            "connection.remotePort={}",
            preferences
                .remote_port
                .unwrap_or(freeshow_remote::REMOTE_PORT)
        ),
        format!(
            "connection.stagePort={}",
            preferences
                .stage_port
                .unwrap_or(freeshow_remote::STAGE_PORT)
        ),
        format!(
            "theme={}",
            preferences.theme.as_deref().unwrap_or("default")
        ),
        format!(
            "files.labelsDisabled={}",
            preferences.labels_disabled.unwrap_or(false)
        ),
    ];
    let profiles = profiles_from(&preferences.profiles);
    if profiles.is_empty() {
        lines.push("profiles=(none)".into());
    } else {
        for profile in &profiles {
            let mark = if chrome.active_profile.as_deref() == Some(profile.id.as_str()) {
                "*"
            } else {
                ""
            };
            lines.push(format!(
                "profiles.{id}{mark}={name}",
                id = profile.id,
                name = profile.name
            ));
        }
        if chrome.profile_locked {
            lines.push("profiles.locked=true".into());
        }
    }
    if let Some(alert) = &chrome.update_alert {
        lines.push(format!("alert={alert}"));
    }
    lines.join("\n")
}

pub fn apply_update_alert(
    chrome: &mut SessionChrome,
    preferences: &freeshow_data::FreeShowPreferences,
    update: &ReleaseUpdate,
) {
    if preferences.alert_updates() && update.newer {
        chrome.update_alert = Some(format!(
            "Update {} is available ({})",
            update.tag, update.html_url
        ));
    } else {
        chrome.update_alert = None;
    }
}

pub fn weather_due(last: Option<Instant>, now: Instant, every: Duration) -> bool {
    last.is_none_or(|fetched| now.saturating_duration_since(fetched) >= every)
}

pub fn default_weather_interval() -> Duration {
    Duration::from_secs(600)
}

pub fn rss_urls_from(preferences: &freeshow_data::FreeShowPreferences) -> Vec<String> {
    let special = &preferences.special;
    let mut urls = Vec::new();
    if let Some(url) = special
        .get("rssUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        urls.push(url.to_string());
    }
    if let Some(list) = special
        .get("rssFeeds")
        .and_then(serde_json::Value::as_array)
    {
        for item in list {
            let url = item
                .as_str()
                .or_else(|| item.get("url").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|url| !url.is_empty());
            if let Some(url) = url {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

pub fn weather_interval(preferences: &freeshow_data::FreeShowPreferences) -> Duration {
    special_secs(&preferences.special, "weatherRefreshSec")
        .or_else(|| special_secs(&preferences.special, "weatherRefresh"))
        .unwrap_or_else(default_weather_interval)
}

pub fn rss_interval(preferences: &freeshow_data::FreeShowPreferences) -> Duration {
    special_secs(&preferences.special, "rssRefreshSec").unwrap_or(Duration::from_secs(300))
}

fn special_secs(special: &serde_json::Value, key: &str) -> Option<Duration> {
    special
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_secs)
}

pub fn weather_locations_from(
    special: &serde_json::Value,
    items: &[serde_json::Value],
) -> Vec<(f64, f64, bool)> {
    let mut found = Vec::new();
    for item in items {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("weather") {
            continue;
        }
        let weather = item.get("weather").unwrap_or(item);
        let lat = weather
            .get("latitude")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let lon = weather
            .get("longitude")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let fahrenheit = weather
            .get("useFahrenheit")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        found.push((lat, lon, fahrenheit));
    }
    if found.is_empty() {
        let lat = special
            .get("weatherLatitude")
            .and_then(serde_json::Value::as_f64);
        let lon = special
            .get("weatherLongitude")
            .and_then(serde_json::Value::as_f64);
        if let (Some(lat), Some(lon)) = (lat, lon) {
            found.push((
                lat,
                lon,
                special
                    .get("weatherUseFahrenheit")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true),
            ));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_login_and_settings_text_include_alerts() {
        let profiles = profiles_from(&serde_json::json!({
            "house": {"name": "House", "password": "secret", "color": "#f00"},
            "admin": {"name": "Admin"}
        }));
        assert_eq!(profiles[0].name, "Admin");
        assert!(profiles[0].allows_login(""));
        assert!(!profiles[1].allows_login("nope"));
        assert!(profiles[1].allows_login("secret"));

        let prefs = freeshow_data::FreeShowPreferences {
            alert_updates: Some(true),
            language: Some("en".into()),
            profiles: serde_json::json!({"house": {"name": "House", "password": "x"}}),
            ..freeshow_data::FreeShowPreferences::default()
        };
        let mut chrome = SessionChrome {
            profile_locked: true,
            ..SessionChrome::default()
        };
        apply_update_alert(
            &mut chrome,
            &prefs,
            &ReleaseUpdate {
                tag: "v0.2.0".into(),
                version: "0.2.0".into(),
                newer: true,
                html_url: "https://example/tag".into(),
                download_url: None,
                checksum_url: None,
            },
        );
        let text = settings_text(&prefs, &chrome);
        assert!(text.contains("general.alertUpdates=true"), "{text}");
        assert!(text.contains("Update v0.2.0"), "{text}");
        assert!(text.contains("profiles.locked=true"), "{text}");
        assert!(weather_due(None, Instant::now(), Duration::from_secs(10)));
        assert_eq!(
            weather_locations_from(
                &serde_json::json!({"weatherLatitude": 27.7, "weatherLongitude": 85.3}),
                &[]
            ),
            vec![(27.7, 85.3, false)]
        );
        assert_eq!(
            rss_urls_from(&freeshow_data::FreeShowPreferences {
                special: serde_json::json!({"rssUrl": "https://example/feed.xml"}),
                ..freeshow_data::FreeShowPreferences::default()
            }),
            vec!["https://example/feed.xml".to_string()]
        );
    }
}
