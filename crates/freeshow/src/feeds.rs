// SPDX-License-Identifier: GPL-3.0-only

//! Weather (Open-Meteo) and RSS helpers. No API keys.

use std::time::Duration;

/// A current-conditions report from Open-Meteo.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherReport {
    pub latitude: f64,
    pub longitude: f64,
    pub temperature_c: f64,
    pub weather_code: u32,
    pub description: String,
}

impl WeatherReport {
    pub fn temperature_label(&self, fahrenheit: bool) -> String {
        if fahrenheit {
            format!("{:.0}°F", self.temperature_c * 9.0 / 5.0 + 32.0)
        } else {
            format!("{:.0}°C", self.temperature_c)
        }
    }
}

/// Parse a current-weather Open-Meteo JSON body.
pub fn parse_open_meteo(
    body: &str,
    latitude: f64,
    longitude: f64,
) -> Result<WeatherReport, String> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("weather JSON: {err}"))?;
    let current = json.get("current").or_else(|| json.get("current_weather"));
    let temperature = current
        .and_then(|value| {
            value
                .get("temperature_2m")
                .or_else(|| value.get("temperature"))
        })
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| "weather is missing temperature".to_string())?;
    let code = current
        .and_then(|value| {
            value
                .get("weather_code")
                .or_else(|| value.get("weathercode"))
        })
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    Ok(WeatherReport {
        latitude,
        longitude,
        temperature_c: temperature,
        weather_code: code,
        description: weather_description(code),
    })
}

pub fn weather_description(code: u32) -> String {
    let text = match code {
        0 => "Clear",
        1 | 2 => "Mostly clear",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 | 56 | 57 => "Drizzle",
        61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 => "Rain",
        71 | 73 | 75 | 77 | 85 | 86 => "Snow",
        95 | 96 | 99 => "Thunderstorm",
        _ => "Weather",
    };
    text.to_string()
}

/// Fetch current weather from Open-Meteo. `base` is the API origin (tests pass a mock).
pub fn fetch_weather(latitude: f64, longitude: f64, base: &str) -> Result<WeatherReport, String> {
    let url = format!(
        "{}/v1/forecast?latitude={latitude}&longitude={longitude}&current=temperature_2m,weather_code",
        base.trim_end_matches('/')
    );
    let body = http_get(&url)?;
    parse_open_meteo(&body, latitude, longitude)
}

/// One item from an RSS or Atom feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssItem {
    pub title: String,
    pub link: String,
}

/// Parse RSS 2.0 or Atom XML without hitting the network.
pub fn parse_rss(xml: &str) -> Vec<RssItem> {
    let mut items = Vec::new();
    for (open, close) in [("<item", "</item>"), ("<entry", "</entry>")] {
        let mut rest = xml;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len()..];
            let Some(end) = after.find(close) else {
                break;
            };
            let block = &after[..end];
            let title = tag_text(block, "title").unwrap_or_default();
            let link = tag_text(block, "link")
                .or_else(|| attr_text(block, "link", "href"))
                .unwrap_or_default();
            if !title.is_empty() {
                items.push(RssItem { title, link });
            }
            rest = &after[end + close.len()..];
        }
    }
    items
}

pub fn fetch_rss(url: &str) -> Result<Vec<RssItem>, String> {
    let body = http_get(url)?;
    Ok(parse_rss(&body))
}

fn http_get(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(8))
        .call()
        .map_err(|err| err.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("feed returned {}", response.status()));
    }
    response.into_string().map_err(|err| err.to_string())
}

fn tag_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let start = block.find(&open)?;
    let after = &block[start + open.len()..];
    let inner = after.split_once('>')?.1;
    let text = inner.split_once(&close)?.0;
    let text = decode_basic(text);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn attr_text(block: &str, name: &str, attr: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = block.find(&open)?;
    let tag = block[start..].split('>').next()?;
    let key = format!("{attr}=\"");
    let value = tag.split_once(&key)?.1.split('"').next()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn decode_basic(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("<![CDATA[", "")
        .replace("]]>", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn serve_once(body: &str, content_type: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|chunk| chunk == b"\r\n\r\n") && request.len() < 65536 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            stream.write_all(payload.as_bytes()).unwrap();
        });
        (format!("http://{addr}"), thread)
    }

    #[test]
    fn open_meteo_and_rss_fixtures_parse_without_a_network() {
        let weather = parse_open_meteo(
            r#"{"current":{"temperature_2m":18.4,"weather_code":0}}"#,
            27.7,
            85.3,
        )
        .unwrap();
        assert_eq!(weather.temperature_label(false), "18°C");
        assert_eq!(weather.description, "Clear");
        let rss = parse_rss(
            r#"<rss><channel>
                <item><title>Sunday plan</title><link>https://example/sunday</link></item>
                <item><title>Midweek &amp; choir</title><link>https://example/mid</link></item>
              </channel></rss>"#,
        );
        assert_eq!(rss[0].title, "Sunday plan");
        assert_eq!(rss[1].title, "Midweek & choir");
        let atom = parse_rss(
            r#"<feed><entry><title>Note</title><link href="https://example/n"/></entry></feed>"#,
        );
        assert_eq!(atom[0].link, "https://example/n");
    }

    #[test]
    fn weather_and_rss_fetch_from_a_local_server() {
        let (base, server) = serve_once(
            r#"{"current":{"temperature_2m":5.0,"weather_code":71}}"#,
            "application/json",
        );
        let report = fetch_weather(1.0, 2.0, &base).unwrap();
        server.join().unwrap();
        assert_eq!(report.description, "Snow");
        assert_eq!(report.temperature_label(true), "41°F");

        let xml = r#"<rss><channel><item><title>Ping</title><link>https://example/p</link></item></channel></rss>"#;
        let (url, server) = serve_once(xml, "application/rss+xml");
        let items = fetch_rss(&url).unwrap();
        server.join().unwrap();
        assert_eq!(items[0].title, "Ping");
    }
}
