// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use serde_json::{Value, json};

/// Asks a local model to rewrite the selected slide. The slide stays as it is
/// when no model is configured or the request fails.
pub trait SlideAssist {
    fn name(&self) -> &'static str;
    fn rewrite(&self, prompt: &str, slide: &str) -> Option<AssistReply>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistReply {
    pub message: String,
    pub lines: Option<Vec<String>>,
}

pub struct OllamaAssist {
    pub base: String,
    pub model: String,
}

impl OllamaAssist {
    /// Present when `FREESHOW_AI` is `ollama` or an Ollama base URL.
    pub fn configured() -> Option<Self> {
        let raw = std::env::var("FREESHOW_AI").ok()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let base = if raw.eq_ignore_ascii_case("ollama") {
            "http://127.0.0.1:11434".to_string()
        } else {
            raw.trim_end_matches('/').to_string()
        };
        let model = std::env::var("FREESHOW_AI_MODEL").unwrap_or_else(|_| "gemma3:4b".to_string());
        Some(Self { base, model })
    }
}

impl SlideAssist for OllamaAssist {
    fn name(&self) -> &'static str {
        "ai"
    }

    fn rewrite(&self, prompt: &str, slide: &str) -> Option<AssistReply> {
        match complete(&self.base, &self.model, prompt, slide) {
            Ok(reply) => Some(reply),
            Err(err) => {
                tracing::error!("ai: {err}");
                None
            }
        }
    }
}

const SYSTEM: &str = "You edit slides in FreeShow. Reply with JSON only: {\"content\": a short confirmation, \"items\": [{\"lines\": [{\"text\": [{\"value\": \"one lyric line\"}]}]}]}. Use items null when the slide should stay unchanged.";

pub fn lines_from_reply(text: &str) -> Result<AssistReply, String> {
    let trimmed = text.trim();
    let json_text = fenced(trimmed);
    let value: Value = serde_json::from_str(json_text)
        .or_else(|_| serde_json::from_str(&slice_object(json_text)))
        .map_err(|err| err.to_string())?;
    let message = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let lines = match value.get("items") {
        Some(Value::Null) | None => None,
        Some(Value::Array(items)) => Some(item_lines(items)),
        Some(_) => return Err("ai items were not a list".to_string()),
    };
    Ok(AssistReply { message, lines })
}

fn item_lines(items: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for item in items {
        let Some(rows) = item.get("lines").and_then(Value::as_array) else {
            continue;
        };
        for row in rows {
            let Some(parts) = row.get("text").and_then(Value::as_array) else {
                continue;
            };
            for part in parts {
                if let Some(value) = part.get("value").and_then(Value::as_str) {
                    let value = value.trim();
                    if !value.is_empty() {
                        lines.push(value.to_string());
                    }
                }
            }
        }
    }
    lines
}

fn fenced(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.trim_start_matches(['j', 's', 'o', 'n', 'J', 'S', 'O', 'N']);
    let rest = rest.trim_start_matches('\n');
    rest.trim_end_matches('`').trim()
}

fn slice_object(text: &str) -> String {
    let Some(start) = text.find('{') else {
        return text.to_string();
    };
    let Some(end) = text.rfind('}') else {
        return text.to_string();
    };
    text[start..=end].to_string()
}

fn complete(base: &str, model: &str, prompt: &str, slide: &str) -> Result<AssistReply, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "format": "json",
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": format!("{prompt}\n\nCurrent slide:\n{slide}") }
        ]
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(15))
        .build();
    let response = agent
        .post(&format!("{base}/api/chat"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("ai returned {code}"),
            other => other.to_string(),
        })?;
    let text = response.into_string().map_err(|err| err.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    let content = value
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or("");
    lines_from_reply(content)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn slide_items_become_lyric_lines() {
        let reply = lines_from_reply(
            r#"```json
            {"content":"Updated the verse.","items":[{"lines":[{"text":[{"value":"Amazing grace"},{"value":"  "},{"value":"how sweet"}]}]}]}
            ```"#,
        )
        .unwrap();
        assert_eq!(reply.message, "Updated the verse.");
        assert_eq!(
            reply.lines.unwrap(),
            vec!["Amazing grace".to_string(), "how sweet".to_string()]
        );
        let unchanged = lines_from_reply(r#"{"content":"That is a song.","items":null}"#).unwrap();
        assert_eq!(unchanged.lines, None);
    }

    #[test]
    fn an_ollama_reply_rewrites_the_slide() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            let header_end = loop {
                let read = stream.read(&mut buf).unwrap();
                if read == 0 {
                    break 0;
                }
                request.extend_from_slice(&buf[..read]);
                if let Some(end) = request.windows(4).position(|mark| mark == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let header = String::from_utf8_lossy(&request[..header_end]).to_string();
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < header_end + length {
                let read = stream.read(&mut buf).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
            }
            let text = String::from_utf8_lossy(&request);
            assert!(text.contains("Amazing grace"));
            assert!(text.contains("gemma3:4b"));
            let content = r#"{"content":"Done","items":[{"lines":[{"text":[{"value":"Amazing grace"},{"value":"how sweet"}]}]}]}"#;
            let body = format!(
                r#"{{"message":{{"content":{}}}}}"#,
                serde_json::to_string(content).unwrap()
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let assist = OllamaAssist {
            base: format!("http://127.0.0.1:{port}"),
            model: "gemma3:4b".to_string(),
        };
        let reply = assist
            .rewrite("Rewrite this slide as short lyric lines.", "Amazing grace")
            .unwrap();
        assert_eq!(
            reply.lines.unwrap(),
            vec!["Amazing grace".to_string(), "how sweet".to_string()]
        );
        server.join().unwrap();
    }
}
