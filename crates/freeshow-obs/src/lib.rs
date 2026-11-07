// SPDX-License-Identifier: GPL-3.0-only

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Sends OBS WebSocket 5 requests. A closed OBS does not stop the show.
pub trait ObsControl {
    fn name(&self) -> &'static str;
    fn set_scene(&self, scene: &str);
    fn toggle_stream(&self);
    fn toggle_record(&self);
}

#[derive(Debug, Clone)]
pub struct ObsOutput {
    pub host: String,
    pub port: u16,
    pub password: String,
}

impl ObsOutput {
    pub fn from_env() -> Self {
        from_environment().unwrap_or(Self {
            host: "localhost".to_string(),
            port: 4455,
            password: String::new(),
        })
    }

    /// Present when `FREESHOW_OBS` names a host, optionally `host:port`.
    pub fn configured() -> Option<Self> {
        from_environment()
    }
}

impl ObsControl for ObsOutput {
    fn name(&self) -> &'static str {
        "obs"
    }

    fn set_scene(&self, scene: &str) {
        if scene.trim().is_empty() {
            tracing::error!("obs scene name is missing");
            return;
        }
        if let Err(err) = send_request(
            &self.host,
            self.port,
            &self.password,
            "SetCurrentProgramScene",
            Some(json!({ "sceneName": scene })),
        ) {
            tracing::error!("obs: {err}");
        }
    }

    fn toggle_stream(&self) {
        toggle(self, "StartStream", "StopStream", stream_on());
    }

    fn toggle_record(&self) {
        toggle(self, "StartRecord", "StopRecord", record_on());
    }
}

fn from_environment() -> Option<ObsOutput> {
    let raw = std::env::var("FREESHOW_OBS").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let password = std::env::var("FREESHOW_OBS_PASSWORD").unwrap_or_default();
    if let Some((host, port)) = raw.rsplit_once(':')
        && let Ok(port) = port.parse::<u16>()
        && !host.is_empty()
    {
        return Some(ObsOutput {
            host: host.to_string(),
            port,
            password,
        });
    }
    Some(ObsOutput {
        host: raw.to_string(),
        port: 4455,
        password,
    })
}

fn stream_on() -> &'static AtomicBool {
    static ON: AtomicBool = AtomicBool::new(false);
    &ON
}

fn record_on() -> &'static AtomicBool {
    static ON: AtomicBool = AtomicBool::new(false);
    &ON
}

fn toggle(obs: &ObsOutput, start: &str, stop: &str, flag: &AtomicBool) {
    let turning_on = !flag.load(Ordering::Relaxed);
    let request = if turning_on { start } else { stop };
    match send_request(&obs.host, obs.port, &obs.password, request, None) {
        Ok(()) => flag.store(turning_on, Ordering::Relaxed),
        Err(err) => tracing::error!("obs: {err}"),
    }
}

/// OBS identifies a client with `base64(sha256(base64(sha256(password + salt)) + challenge))`.
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let mut first = Sha256::new();
    first.update(password.as_bytes());
    first.update(salt.as_bytes());
    let secret = base64::engine::general_purpose::STANDARD.encode(first.finalize());
    let mut second = Sha256::new();
    second.update(secret.as_bytes());
    second.update(challenge.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(second.finalize())
}

pub fn identify_message(authentication: Option<&str>) -> String {
    let mut body = json!({
        "rpcVersion": 1,
        "eventSubscriptions": (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 10) | (1 << 16)
    });
    if let Some(authentication) = authentication {
        body["authentication"] = json!(authentication);
    }
    json!({ "op": 1, "d": body }).to_string()
}

pub fn request_message(
    request_type: &str,
    request_id: &str,
    request_data: Option<Value>,
) -> String {
    json!({
        "op": 6,
        "d": {
            "requestType": request_type,
            "requestId": request_id,
            "requestData": request_data.unwrap_or_else(|| json!({}))
        }
    })
    .to_string()
}

pub fn send_request(
    host: &str,
    port: u16,
    password: &str,
    request_type: &str,
    request_data: Option<Value>,
) -> Result<(), String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or_else(|| format!("obs address {host}:{port} did not resolve"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|err| format!("obs is not connected at {host}:{port}: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| err.to_string())?;
    let url = format!("ws://{host}:{port}");
    let (mut socket, _) =
        tungstenite::client::client(url, stream).map_err(|err| err.to_string())?;
    let hello = read_message(&mut socket, 0)?;
    let authentication = hello
        .pointer("/d/authentication")
        .filter(|value| value.is_object())
        .map(|auth| {
            let salt = auth.get("salt").and_then(Value::as_str).unwrap_or("");
            let challenge = auth.get("challenge").and_then(Value::as_str).unwrap_or("");
            if password.is_empty() {
                Err("obs requires a password".to_string())
            } else {
                Ok(auth_string(password, salt, challenge))
            }
        })
        .transpose()?;
    let identify = identify_message(authentication.as_deref());
    socket
        .send(tungstenite::Message::Text(identify.into()))
        .map_err(|err| err.to_string())?;
    let _identified = read_message(&mut socket, 2)?;
    let request_id = next_request_id();
    let request = request_message(request_type, &request_id, request_data);
    socket
        .send(tungstenite::Message::Text(request.into()))
        .map_err(|err| err.to_string())?;
    let response = read_message(&mut socket, 7)?;
    let result = response
        .pointer("/d/requestStatus/result")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !result {
        let comment = response
            .pointer("/d/requestStatus/comment")
            .and_then(Value::as_str)
            .unwrap_or("obs request failed");
        return Err(comment.to_string());
    }
    Ok(())
}

fn next_request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed).to_string()
}

fn read_message<S>(socket: &mut tungstenite::WebSocket<S>, opcode: i64) -> Result<Value, String>
where
    S: Read + Write,
{
    for _ in 0..8 {
        let message = socket.read().map_err(|err| err.to_string())?;
        let text = match message {
            tungstenite::Message::Text(text) => text.to_string(),
            tungstenite::Message::Binary(bytes) => {
                String::from_utf8(bytes.to_vec()).map_err(|err| err.to_string())?
            }
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
            tungstenite::Message::Close(_) => return Err("obs closed the connection".to_string()),
            _ => continue,
        };
        let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
        if value.get("op").and_then(Value::as_i64) == Some(opcode) {
            return Ok(value);
        }
    }
    Err("obs did not answer".to_string())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use tungstenite::{Message, accept};

    use super::*;

    #[test]
    fn the_password_hash_matches_obs() {
        assert_eq!(
            auth_string("secret", "PZ1S7H", "challenge"),
            "LExCLaT6GRSbGiRHEzxhDOglLoSz8+Kkhlx6CSQRfM0="
        );
    }

    #[test]
    fn a_scene_request_identifies_and_sets_the_program_scene() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = accept(stream).unwrap();
            socket
                .send(Message::Text(
                    r#"{"op":0,"d":{"obsWebSocketVersion":"5.0.1","rpcVersion":1,"authentication":{"challenge":"challenge","salt":"PZ1S7H"}}}"#
                        .to_string()
                        .into(),
                ))
                .unwrap();
            let identify: Value =
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(identify["op"], 1);
            assert_eq!(
                identify["d"]["authentication"],
                "LExCLaT6GRSbGiRHEzxhDOglLoSz8+Kkhlx6CSQRfM0="
            );
            assert_eq!(identify["d"]["rpcVersion"], 1);
            socket
                .send(Message::Text(
                    r#"{"op":2,"d":{"negotiatedRpcVersion":1}}"#.to_string().into(),
                ))
                .unwrap();
            let request: Value =
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["op"], 6);
            assert_eq!(request["d"]["requestType"], "SetCurrentProgramScene");
            assert_eq!(request["d"]["requestData"]["sceneName"], "Program");
            let id = request["d"]["requestId"].as_str().unwrap().to_string();
            socket
                .send(Message::Text(
                    json!({
                        "op": 7,
                        "d": {
                            "requestType": "SetCurrentProgramScene",
                            "requestId": id,
                            "requestStatus": { "result": true, "code": 100 }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .unwrap();
        });

        send_request(
            "127.0.0.1",
            port,
            "secret",
            "SetCurrentProgramScene",
            Some(json!({ "sceneName": "Program" })),
        )
        .unwrap();
        server.join().unwrap();
    }
}
