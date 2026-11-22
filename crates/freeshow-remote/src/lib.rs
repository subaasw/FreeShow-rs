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
use freeshow_output::Frame;
use serde_json::Value;
use socketioxide::SocketIo;
use socketioxide::extract::{Data, SocketRef};
use tokio::sync::oneshot;

pub use protocol::{
    Command, RemoteBible, RemoteBook, RemoteChapter, RemoteFile, RemoteFolder, RemoteLayout,
    RemoteOverlay, RemoteProject, RemoteProjectItem, RemoteShow, RemoteSlide, RemoteTemplate,
    RemoteTimer, RemoteVerse, ShowSnapshot, handle_message, handle_message_authenticated,
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
    stage_only: bool,
    stage_frames: Option<mpsc::SyncSender<StageFrameRequest>>,
    max_connections: u32,
    connections: Arc<std::sync::atomic::AtomicUsize>,
}

impl AppState {
    pub fn new(bus: ShowBus, password: Option<String>) -> Self {
        Self {
            bus,
            password: password
                .map(|password| password.trim().to_string())
                .filter(|password| !password.is_empty()),
            stage_only: false,
            stage_frames: None,
            max_connections: 10,
            connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.max_connections = max.max(1);
        self
    }

    fn try_acquire_connection(&self) -> bool {
        let max = self.max_connections as usize;
        loop {
            let current = self.connections.load(std::sync::atomic::Ordering::SeqCst);
            if current >= max {
                return false;
            }
            if self
                .connections
                .compare_exchange(
                    current,
                    current + 1,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    fn release_connection(&self) {
        self.connections
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
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

/// A pending browser request for the next frame the native stage paints.
pub struct StageFrameRequest(oneshot::Sender<Frame>);

impl StageFrameRequest {
    pub fn publish(self, frame: &Frame) {
        if !self.0.is_closed() {
            let _ = self.0.send(frame.clone());
        }
    }
}

pub fn spawn(
    snapshot: Arc<Mutex<ShowSnapshot>>,
    commands: mpsc::Sender<Command>,
    password: Option<String>,
    ports: RemotePorts,
    stage_frames: mpsc::SyncSender<StageFrameRequest>,
) {
    spawn_limited(snapshot, commands, password, ports, stage_frames, 10);
}

pub fn spawn_limited(
    snapshot: Arc<Mutex<ShowSnapshot>>,
    commands: mpsc::Sender<Command>,
    password: Option<String>,
    ports: RemotePorts,
    stage_frames: mpsc::SyncSender<StageFrameRequest>,
    max_connections: u32,
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
                let mut state = AppState::new(ShowBus { snapshot, commands }, password)
                    .with_max_connections(max_connections);
                state.stage_frames = Some(stage_frames);
                let remote = with_socket(remote_router(state.clone()), state.clone(), "REMOTE");
                let stage = stage_router(state);
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
        .route("/stage-frame", get(stage_frame_png))
        .route("/state", get(state_json))
        .route("/action", post(action))
        .route("/api/state", get(state_json))
        .route("/api/action", post(action))
        .route("/api/{channel}", post(api_channel).get(api_channel_get))
        .route("/webhook", post(webhook).get(webhook_get))
        .route("/companion", get(companion_index))
        .route(
            "/companion/{action}",
            post(companion_press).get(companion_press_get),
        )
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

pub fn stage_router(mut state: AppState) -> Router {
    state.stage_only = true;
    let router = Router::new()
        .route("/", get(stage_page))
        .route("/stage-frame", get(stage_frame_png))
        .route("/state", get(state_json))
        .route("/action", post(action))
        .route("/ws", get(ws_upgrade))
        .with_state(state.clone());
    with_socket(router, state, "STAGE")
}

fn with_socket(router: Router, state: AppState, event: &'static str) -> Router {
    let (layer, io) = SocketIo::new_layer();
    io.ns("/", async move |socket: SocketRef| {
        let state = state.clone();
        if !state.try_acquire_connection() {
            tracing::warn!("remote connection limit reached");
            let _ = socket.disconnect();
            return;
        }
        let released = state.clone();
        socket.on_disconnect(async move || {
            released.release_connection();
        });
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

async fn stage_frame_png(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    if !state.authorized(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let requests = state.stage_frames.ok_or(StatusCode::NOT_FOUND)?;
    let (sender, receiver) = oneshot::channel();
    requests
        .try_send(StageFrameRequest(sender))
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let png = tokio::task::spawn_blocking(move || frame.encode_png())
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        png,
    ))
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

/// Maps a short Companion/webhook name to a FreeShow API channel.
pub fn normalize_action_channel(action: &str) -> String {
    let trimmed = action.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return "API:next_slide".to_string();
    }
    if trimmed.contains(':') {
        return trimmed.to_string();
    }
    let key = trimmed.replace(['-', ' '], "_").to_ascii_lowercase();
    match key.as_str() {
        "next" | "next_slide" | "go" | "press" => "API:next_slide".into(),
        "previous" | "prev" | "previous_slide" => "API:previous_slide".into(),
        "clear" | "clear_all" => "API:clear_all".into(),
        "clear_slide" => "API:clear_slide".into(),
        "state" | "get_state" => "API:get_cleared".into(),
        other => format!("API:{other}"),
    }
}

fn message_from_action(action: &str, data: Option<Value>) -> Value {
    let mut message = serde_json::json!({ "channel": normalize_action_channel(action) });
    if let Some(Value::Object(map)) = data {
        if let Some(channel) = map.get("channel").and_then(Value::as_str) {
            message["channel"] = Value::String(normalize_action_channel(channel));
        }
        if let Some(action) = map
            .get("action")
            .or_else(|| map.get("event"))
            .and_then(Value::as_str)
        {
            message["channel"] = Value::String(normalize_action_channel(action));
        }
        if let Some(payload) = map.get("data") {
            message["data"] = payload.clone();
        }
    } else if let Some(data) = data {
        message["data"] = data;
    }
    message
}

fn run_action(
    state: &AppState,
    headers: &HeaderMap,
    action: &str,
    data: Option<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !state.authorized(headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "channel": "PASSWORD",
                "data": { "password": true }
            })),
        ));
    }
    let mut authenticated = true;
    let incoming = message_from_action(action, data);
    let replies = dispatch(state, &incoming, &mut authenticated);
    let body = if replies.len() == 1 {
        replies.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(replies)
    };
    Ok(Json(body))
}

fn json_body(bytes: &[u8]) -> Option<Value> {
    if bytes.is_empty() {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

async fn api_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(channel): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    run_action(&state, &headers, &channel, json_body(&body))
}

async fn api_channel_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(channel): axum::extract::Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    run_action(&state, &headers, &channel, None)
}

async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    run_action(&state, &headers, "next", json_body(&body))
}

async fn webhook_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let action = query
        .get("action")
        .or_else(|| query.get("channel"))
        .or_else(|| query.get("event"))
        .cloned()
        .unwrap_or_else(|| "next".to_string());
    run_action(&state, &headers, &action, None)
}

async fn companion_index(
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
    Ok(Json(serde_json::json!({
        "name": "FreeShow",
        "protocol": "http+ws",
        "actions": [
            "next",
            "previous",
            "clear",
            "clear_slide",
            "go"
        ]
    })))
}

async fn companion_press(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    run_action(&state, &headers, &action, json_body(&body))
}

async fn companion_press_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    run_action(&state, &headers, &action, None)
}

async fn ws_upgrade(State(state): State<AppState>, socket: WebSocketUpgrade) -> impl IntoResponse {
    if !state.try_acquire_connection() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    socket
        .on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
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
    state.release_connection();
}

fn dispatch(state: &AppState, incoming: &Value, authenticated: &mut bool) -> Vec<Value> {
    if state.stage_only
        && !matches!(
            incoming.get("channel").and_then(Value::as_str),
            Some("PASSWORD" | "ACCESS" | "LAYOUTS" | "LAYOUT" | "API:get_output")
        )
    {
        return Vec::new();
    }
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
            audio_mixer: Value::Null,
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

    #[test]
    fn webhook_and_companion_names_map_to_api_channels() {
        assert_eq!(normalize_action_channel("next"), "API:next_slide");
        assert_eq!(normalize_action_channel("PREVIOUS"), "API:previous_slide");
        assert_eq!(normalize_action_channel("API:clear_all"), "API:clear_all");
        assert_eq!(
            normalize_action_channel("toggle_media_mute"),
            "API:toggle_media_mute"
        );
    }

    #[tokio::test]
    async fn rest_webhook_and_companion_advance_the_slide() {
        let (state, rx) = harness();
        let app = remote_router(state);

        let api = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/next")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api.status(), 200);
        assert_eq!(rx.try_recv(), Ok(Command::SetSlide(1)));

        let webhook = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/webhook?action=previous")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(webhook.status(), 200);
        assert_eq!(rx.try_recv(), Ok(Command::SetSlide(0)));

        let companion = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/companion/next")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(companion.status(), 200);
        assert_eq!(rx.try_recv(), Ok(Command::SetSlide(1)));

        let index = app
            .oneshot(
                Request::builder()
                    .uri("/companion")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(index.status(), 200);
        let body: Value =
            serde_json::from_slice(&index.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["protocol"], "http+ws");
        assert!(
            body["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a == "next")
        );
    }

    #[tokio::test]
    async fn state_and_action_require_the_password() {
        let (mut state, rx) = harness();
        state.password = Some("1234".into());

        let app = remote_router(state);
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), 401);
        let body: Value =
            serde_json::from_slice(&unauthorized.into_body().collect().await.unwrap().to_bytes())
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
        let body: Value =
            serde_json::from_slice(&authorized.into_body().collect().await.unwrap().to_bytes())
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
                json!({ "channel": "API:next_slide" }).to_string().into(),
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
        for _ in 0..9 {
            let message = socket.next().await.unwrap().unwrap();
            let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_ne!(reply["channel"], "ERROR");
        }

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({ "channel": "API:next_slide" }).to_string().into(),
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
    async fn stage_frames_require_the_password_and_return_the_native_pixels() {
        let (mut state, commands) = harness();
        state.password = Some("stage-pass".into());
        let (sender, receiver) = mpsc::sync_channel::<StageFrameRequest>(1);
        state.stage_frames = Some(sender);
        let stage = stage_router(state.clone());

        for password in ["", "wrong"] {
            let response = stage
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/stage-frame")
                        .header("x-freeshow-password", password)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }

        let producer = std::thread::spawn(move || {
            let frame = Frame {
                width: 2,
                height: 1,
                pixels: vec![255, 0, 0, 255, 0, 255, 0, 128],
            };
            for _ in 0..2 {
                receiver
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .publish(&frame);
            }
        });
        for app in [stage, remote_router(state)] {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/stage-frame")
                        .header("x-freeshow-password", "stage-pass")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let png = response.into_body().collect().await.unwrap().to_bytes();
            let raster = freeshow_output::decode_image(&png).unwrap();
            assert_eq!((raster.width, raster.height), (2, 1));
            assert_eq!(raster.rgba, [255, 0, 0, 255, 0, 255, 0, 128]);
        }
        producer.join().unwrap();
        assert!(commands.try_recv().is_err());
    }

    #[tokio::test]
    async fn stage_transports_read_the_show_without_advancing_it() {
        let (state, rx) = harness();
        let app = stage_router(state.clone());
        let advance = json!({ "channel": "API:next_slide" });
        let read = json!({ "channel": "API:get_output" });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/action")
                    .header("content-type", "application/json")
                    .body(Body::from(advance.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "[]"
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { serve(listener, app).await.unwrap() });
        let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        for message in [&advance, &read] {
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                message.to_string().into(),
            ))
            .await
            .unwrap();
        }
        let reply = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let output: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
        assert_eq!(output["data"]["lines"][0], "Amazing grace");

        let output = tokio::task::spawn_blocking(move || {
            let (tx, replies) = mpsc::channel();
            let socket = ClientBuilder::new(format!("http://127.0.0.1:{port}"))
                .on("STAGE", move |payload, _client: RawClient| {
                    if let Payload::Text(values) = payload {
                        for value in values {
                            let _ = tx.send(value);
                        }
                    }
                })
                .connect()
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
            socket.emit("STAGE", advance).unwrap();
            socket.emit("STAGE", read).unwrap();
            let reply = replies.recv_timeout(Duration::from_secs(5)).unwrap();
            socket.disconnect().ok();
            reply
        })
        .await
        .unwrap();
        assert_eq!(output["data"]["lines"][0], "Amazing grace");
        assert_eq!(state.bus.snapshot.lock().unwrap().slide_index, Some(0));
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
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
