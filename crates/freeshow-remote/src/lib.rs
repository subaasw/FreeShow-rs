// SPDX-License-Identifier: GPL-3.0-only

mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Router, serve};
use serde_json::Value;
use socketioxide::SocketIo;
use socketioxide::extract::{Data, SocketRef};

pub use protocol::{
    Command, RemoteBible, RemoteBook, RemoteChapter, RemoteFile, RemoteFolder, RemoteLayout,
    RemoteOverlay, RemoteProject, RemoteTemplate, RemoteTimer,
    RemoteProjectItem, RemoteShow, RemoteSlide, RemoteVerse, ShowSnapshot,
    handle_message, handle_message_authenticated,
};

pub const REMOTE_PORT: u16 = 5510;
pub const STAGE_PORT: u16 = 5511;

/// Ports for the two web servers. Defaults match FreeShow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemotePorts {
    pub remote: u16,
    pub stage: u16,
}

impl Default for RemotePorts {
    fn default() -> Self {
        Self {
            remote: REMOTE_PORT,
            stage: STAGE_PORT,
        }
    }
}

const REMOTE_PAGE: &str = include_str!("../pages/remote.html");
const STAGE_PAGE: &str = include_str!("../pages/stage.html");

#[derive(Clone)]
pub struct ShowBus {
    pub snapshot: Arc<Mutex<ShowSnapshot>>,
    pub commands: mpsc::Sender<Command>,
}

/// State shared by the remote and stage servers.
#[derive(Clone)]
pub struct AppState {
    pub bus: ShowBus,
    /// Password required from network clients. `None` or an empty value is open.
    pub password: Option<String>,
}

impl AppState {
    pub fn new(bus: ShowBus, password: Option<String>) -> Self {
        Self {
            bus,
            password: password
                .map(|password| password.trim().to_string())
                .filter(|password| !password.is_empty()),
        }
    }

    pub fn requires_password(&self) -> bool {
        self.password.is_some()
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = self.password.as_deref() else {
            return true;
        };
        let supplied = headers
            .get("x-freeshow-password")
            .and_then(|value| value.to_str().ok())
            .or_else(|| {
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
            });
        supplied.is_some_and(|value| value == expected)
    }
}

/// An optional show-control listener. The app starts it and keeps running if it cannot bind.
pub trait Integration {
    fn name(&self) -> &'static str;
    fn start(&self, bus: ShowBus);
}

impl ShowBus {
    pub fn apply(&self, message: &Value) -> Vec<Value> {
        let mut authenticated = true;
        dispatch_message(self, message, None, &mut authenticated)
    }
}

pub fn spawn(
    snapshot: Arc<Mutex<ShowSnapshot>>,
    commands: mpsc::Sender<Command>,
    password: Option<String>,
    ports: RemotePorts,
) {
    let started = std::thread::Builder::new()
        .name("freeshow-remote".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!("remote runtime failed: {err}");
                    return;
                }
            };
            runtime.block_on(async move {
                let state = AppState::new(ShowBus { snapshot, commands }, password);
                let remote = with_socket(remote_router(state.clone()), state.clone(), "REMOTE");
                let stage = with_socket(stage_router(state.clone()), state, "STAGE");
                tokio::join!(
                    listen(ports.remote, remote, "remote"),
                    listen(ports.stage, stage, "stage"),
                );
            });
        });
    if started.is_err() {
        tracing::error!("could not start the remote server thread");
    }
}

pub fn remote_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(remote_page))
        .route("/stage", get(stage_page))
        .route("/state", get(state_json))
        .route("/action", post(action))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

pub fn stage_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(stage_page))
        .route("/state", get(state_json))
        .route("/action", post(action))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

fn with_socket(router: Router, state: AppState, event: &'static str) -> Router {
    let (layer, io) = SocketIo::new_layer();
    io.ns("/", async move |socket: SocketRef| {
        let state = state.clone();
        let authenticated = Arc::new(AtomicBool::new(!state.requires_password()));
        socket.on(
            event,
            async move |socket: SocketRef, Data::<Value>(data)| {
                let mut accepted = authenticated.load(Ordering::Relaxed);
                for reply in dispatch(&state, &data, &mut accepted) {
                    if let Err(err) = socket.emit(event, &reply) {
                        tracing::debug!("could not reply on {event}: {err}");
                    }
                }
                authenticated.store(accepted, Ordering::Relaxed);
            },
        );
    });
    router.layer(layer)
}

async fn listen(port: u16, app: Router, name: &str) {
    match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => {
            tracing::info!("{name} page on http://127.0.0.1:{port}/");
            if let Err(err) = serve(listener, app).await {
                tracing::error!("{name} server stopped: {err}");
            }
        }
        Err(err) => tracing::error!("{name} port {port} is not available: {err}"),
    }
}

async fn remote_page() -> Html<&'static str> {
    Html(REMOTE_PAGE)
}

async fn stage_page() -> Html<&'static str> {
    Html(STAGE_PAGE)
}

async fn state_json(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !state.authorized(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "channel": "PASSWORD",
                "data": { "password": true }
            })),
        ));
    }
    let snapshot = state
        .bus
        .snapshot
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    Ok(Json(snapshot.view()))
}

async fn action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(incoming): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !state.authorized(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "channel": "PASSWORD",
                "data": { "password": true }
            })),
        ));
    }
    let mut authenticated = true;
    let replies = dispatch(&state, &incoming, &mut authenticated);
    let body = if replies.len() == 1 {
        replies.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(replies)
    };
    Ok(Json(body))
}

async fn ws_upgrade(State(state): State<AppState>, socket: WebSocketUpgrade) -> impl IntoResponse {
    socket.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut authenticated = !state.requires_password();
    while let Some(Ok(message)) = socket.recv().await {
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let incoming = serde_json::from_str(&text).unwrap_or(Value::Null);
        for reply in dispatch(&state, &incoming, &mut authenticated) {
            let payload = reply.to_string();
            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    }
}

fn dispatch(state: &AppState, incoming: &Value, authenticated: &mut bool) -> Vec<Value> {
    dispatch_message(
        &state.bus,
        incoming,
        state.password.as_deref(),
        authenticated,
    )
}

fn dispatch_message(
    bus: &ShowBus,
    incoming: &Value,
    password: Option<&str>,
    authenticated: &mut bool,
) -> Vec<Value> {
    let (replies, commands) = {
        let mut snapshot = bus.snapshot.lock().unwrap_or_else(|err| err.into_inner());
        handle_message_authenticated(&mut snapshot, incoming, password, authenticated)
    };
    for command in commands {
        let _ = bus.commands.send(command);
    }
    replies
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use futures_util::{SinkExt, StreamExt};
    use http_body_util::BodyExt;
    use rust_socketio::{ClientBuilder, Payload, RawClient};
    use serde_json::{Value, json};
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tower::ServiceExt;

    use super::*;

    fn harness() -> (AppState, mpsc::Receiver<Command>) {
        let snapshot = ShowSnapshot {
            shows: vec![RemoteShow {
                id: "grace".into(),
                name: "Amazing Grace".into(),
                layouts: Vec::new(),
                active_layout: String::new(),
                slides: vec![
                    RemoteSlide {
                        label: "Amazing grace".into(),
                        lines: vec!["Amazing grace".into()],
                        notes: "Stand".into(),
                        id: String::new(),
                        group: String::new(),
                    },
                    RemoteSlide {
                        label: "how sweet".into(),
                        lines: vec!["how sweet".into()],
                        notes: String::new(),
                        id: String::new(),
                        group: String::new(),
                    },
                ],
            }],
            show_index: Some(0),
            slide_index: Some(0),
            cleared: false,
            project: vec!["Amazing Grace".into()],
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
            project_item: 0,
            next_item_on_last_slide: true,
        };
        let (commands, rx) = mpsc::channel();
        (
            AppState::new(
                ShowBus {
                    snapshot: Arc::new(Mutex::new(snapshot)),
                    commands,
                },
                None,
            ),
            rx,
        )
    }

    #[tokio::test]
    async fn post_next_slide_returns_the_following_line() {
        let (state, rx) = harness();
        let response = remote_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/action")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"channel":"API:next_slide"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["channel"], "OUT");
        assert_eq!(value["data"]["lines"][0], "how sweet");
        assert_eq!(rx.try_recv(), Ok(Command::SetSlide(1)));
    }

    #[tokio::test]
    async fn state_and_action_require_the_password() {
        let (mut state, rx) = harness();
        state.password = Some("1234".into());

        let app = remote_router(state);
        let unauthorized = app
            .clone()
            .oneshot(Request::builder().uri("/state").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), 401);
        let body: Value = serde_json::from_slice(
            &unauthorized
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(body["channel"], "PASSWORD");
        assert!(body.get("lines").is_none(), "no lyrics before access");

        let authorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/state")
                    .header("x-freeshow-password", "1234")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), 200);
        let body: Value = serde_json::from_slice(
            &authorized
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(body["lines"][0], "Amazing grace");

        let blocked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/action")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"channel":"API:next_slide"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), 401);
        assert!(rx.try_recv().is_err());

        let accepted = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/action")
                    .header("content-type", "application/json")
                    .header("x-freeshow-password", "1234")
                    .body(Body::from(r#"{"channel":"API:next_slide"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), 200);
        assert_eq!(rx.try_recv(), Ok(Command::SetSlide(1)));
    }

    #[tokio::test]
    async fn websocket_handshake_unlocks_the_remote() {
        let (mut state, rx) = harness();
        state.password = Some("1234".into());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = with_socket(remote_router(state.clone()), state, "REMOTE");
        tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });
        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "PASSWORD" }).to_string().into(),
            ))
            .await
            .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(reply["channel"], "PASSWORD");
        assert_eq!(reply["data"]["password"], true);

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "API:next_slide" })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(200), socket.next())
                .await
                .is_err(),
            "an unauthenticated connection gets no reply"
        );
        assert!(rx.try_recv().is_err());

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "ACCESS", "data": "0000" })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(reply["channel"], "ERROR");
        assert_eq!(reply["data"]["key"], "wrongPass");

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "ACCESS", "data": "1234" })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(reply["channel"], "ACCESS");
        for _ in 0..8 {
            let message = socket.next().await.unwrap().unwrap();
            let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_ne!(reply["channel"], "ERROR");
        }

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "API:next_slide" })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(reply["channel"], "OUT");
        assert_eq!(reply["data"]["lines"][0], "how sweet");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Command::SetSlide(1)
        );
    }

    #[tokio::test]
    async fn remote_page_lists_the_controls() {
        let (state, _) = harness();
        let response = remote_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let page = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(page.contains("Next"));
        assert!(page.contains("API:next_slide"));
    }

    #[tokio::test]
    async fn stage_page_shows_the_current_line() {
        let (state, _) = harness();
        let app = stage_router(state);
        let page = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let html = String::from_utf8(
            page.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("id=\"lines\""));
        let state_response = app
            .oneshot(
                Request::builder()
                    .uri("/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(
            &state_response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(body["lines"][0], "Amazing grace");
        assert_eq!(body["next_lines"][0], "how sweet");
        assert_eq!(body["notes"], "Stand");
    }

    #[tokio::test]
    async fn websocket_speaks_the_remote_channel() {
        let (state, _rx) = harness();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = with_socket(remote_router(state.clone()), state, "REMOTE");
        tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });
        let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"channel":"API:get_output"}"#.into(),
            ))
            .await
            .unwrap();
        let message = socket.next().await.unwrap().unwrap();
        let text = message.to_text().unwrap();
        assert!(text.contains("Amazing grace"));
    }

    #[tokio::test]
    async fn socketio_handshake_unlocks_the_remote() {
        let (mut state, rx) = harness();
        state.password = Some("1234".into());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = with_socket(remote_router(state.clone()), state, "REMOTE");
        tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });
        let reply = tokio::task::spawn_blocking(move || {
            let (tx, replies) = mpsc::channel();
            let socket = ClientBuilder::new(format!("http://127.0.0.1:{port}"))
                .on("REMOTE", move |payload, _client: RawClient| {
                    if let Payload::Text(values) = payload {
                        for value in values {
                            let _ = tx.send(value);
                        }
                    }
                })
                .connect()
                .expect("socket.io connection");
            // Give the namespace handler a moment to attach before the first event.
            std::thread::sleep(Duration::from_millis(100));
            socket
                .emit("REMOTE", json!({ "channel": "PASSWORD" }))
                .expect("emit");
            let first = replies
                .recv_timeout(Duration::from_secs(5))
                .expect("password reply");
            assert_eq!(first["data"]["password"], true);

            socket
                .emit("REMOTE", json!({ "channel": "ACCESS", "data": "0000" }))
                .expect("emit");
            let error = replies
                .recv_timeout(Duration::from_secs(5))
                .expect("wrong password reply");
            assert_eq!(error["data"]["key"], "wrongPass");

            socket
                .emit("REMOTE", json!({ "channel": "ACCESS", "data": "1234" }))
                .expect("emit");
            loop {
                let value = replies
                    .recv_timeout(Duration::from_secs(5))
                    .expect("access reply");
                if value["channel"] == "MEDIA" {
                    break;
                }
            }

            socket
                .emit("REMOTE", json!({ "channel": "API:next_slide" }))
                .expect("emit");
            let reply = replies
                .recv_timeout(Duration::from_secs(5))
                .expect("next slide reply");
            socket.disconnect().ok();
            reply
        })
        .await
        .unwrap();
        assert_eq!(reply["channel"], "OUT");
        assert_eq!(reply["data"]["lines"][0], "how sweet");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Command::SetSlide(1)
        );
    }

    #[tokio::test]
    async fn socketio_remote_event_advances_the_slide() {
        let (state, rx) = harness();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = with_socket(remote_router(state.clone()), state, "REMOTE");
        tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });
        let reply = tokio::task::spawn_blocking(move || {
            let (tx, replies) = mpsc::channel();
            let socket = ClientBuilder::new(format!("http://127.0.0.1:{port}"))
                .on("REMOTE", move |payload, _client: RawClient| {
                    if let Payload::Text(values) = payload {
                        for value in values {
                            let _ = tx.send(value);
                        }
                    }
                })
                .connect()
                .expect("socket.io connection");
            // Give the namespace handler a moment to attach before the first event.
            std::thread::sleep(Duration::from_millis(100));
            socket
                .emit("REMOTE", json!({ "channel": "API:next_slide" }))
                .expect("emit");
            let reply = replies
                .recv_timeout(Duration::from_secs(5))
                .expect("socket.io reply");
            socket.disconnect().ok();
            reply
        })
        .await
        .unwrap();
        assert_eq!(reply["channel"], "OUT");
        assert_eq!(reply["data"]["lines"][0], "how sweet");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Command::SetSlide(1)
        );
    }
}
