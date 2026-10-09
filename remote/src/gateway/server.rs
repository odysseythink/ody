//! Axum HTTP/WebSocket surface of the gateway.

use super::auth::LoginLimiter;
use super::auth::Sessions;
use super::auth::clear_session_cookie;
use super::auth::extract_session_cookie;
use super::auth::session_set_cookie;
use super::auth::verify_password;
use super::hub::Hub;
use crate::proto::AgentToGateway;
use crate::proto::ClientToGateway;
use crate::proto::GatewayToAgent;
use crate::proto::GatewayToClient;
use crate::proto::RpcOutcome;
use crate::proto::is_read_only_method;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use futures::SinkExt;
use futures::StreamExt;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

const INDEX_HTML: &str = include_str!("../../static/index.html");
const MANIFEST: &str = include_str!("../../static/manifest.webmanifest");
const ICON_SVG: &str = include_str!("../../static/icon.svg");

/// Static gateway configuration loaded at startup.
pub struct GatewayConfig {
    pub username: String,
    /// argon2 encoded password hash (see [`super::auth::hash_password`]).
    pub password_hash: String,
    /// Shared secret agents present as a Bearer token on /ws/agent.
    pub agent_token: String,
    pub session_ttl: Duration,
}

#[derive(serde::Deserialize)]
struct RawGatewayConfig {
    username: String,
    password_hash: String,
    agent_token: String,
    #[serde(default = "default_session_ttl_hours")]
    session_ttl_hours: u64,
}

fn default_session_ttl_hours() -> u64 {
    24 * 7
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayConfigError {
    #[error("invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

impl GatewayConfig {
    /// Parse the gateway TOML config file.
    pub fn from_toml_str(text: &str) -> Result<Self, GatewayConfigError> {
        let raw: RawGatewayConfig = toml::from_str(text)?;
        if raw.username.trim().is_empty() {
            return Err(GatewayConfigError::Invalid(
                "username must not be empty".into(),
            ));
        }
        if raw.agent_token.len() < 16 {
            return Err(GatewayConfigError::Invalid(
                "agent_token must be at least 16 characters".into(),
            ));
        }
        // Fail fast on a malformed hash instead of rejecting every login at
        // runtime.
        if !super::auth::verify_password_probe(&raw.password_hash) {
            return Err(GatewayConfigError::Invalid(
                "password_hash is not a valid argon2 hash (use `ody-remote-gateway hash-password`)"
                    .into(),
            ));
        }
        Ok(Self {
            username: raw.username,
            password_hash: raw.password_hash,
            agent_token: raw.agent_token,
            session_ttl: Duration::from_secs(raw.session_ttl_hours * 3600),
        })
    }
}

pub struct AppState {
    username: String,
    password_hash: String,
    agent_token_digest: [u8; 32],
    sessions: Sessions,
    limiter: Mutex<LoginLimiter>,
    pub hub: Hub,
    session_ttl: Duration,
}

pub fn router(config: GatewayConfig) -> Router {
    let state = Arc::new(AppState {
        username: config.username,
        password_hash: config.password_hash,
        agent_token_digest: Sha256::digest(config.agent_token.as_bytes()).into(),
        sessions: Sessions::new(config.session_ttl),
        limiter: Mutex::new(LoginLimiter::new()),
        hub: Hub::new(),
        session_ttl: config.session_ttl,
    });
    Router::new()
        .route("/", get(index))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/instances", get(instances))
        .route("/ws", get(client_ws))
        .route("/ws/agent", get(agent_ws))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        MANIFEST,
    )
}

async fn icon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], ICON_SVG)
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginBody>) -> Response {
    {
        let limiter = crate::lock(&state.limiter);
        if !limiter.check(&body.username) {
            return (StatusCode::TOO_MANY_REQUESTS, "too many failures").into_response();
        }
    }
    let ok =
        body.username == state.username && verify_password(&state.password_hash, &body.password);
    let mut limiter = crate::lock(&state.limiter);
    if !ok {
        limiter.record_failure(&body.username);
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }
    limiter.record_success(&body.username);
    drop(limiter);
    let token = state.sessions.issue();
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            session_set_cookie(&token, state.session_ttl),
        )],
        "ok",
    )
        .into_response()
}

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = session_from_headers(&state, &headers) {
        state.sessions.revoke(&token);
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_session_cookie())],
        "ok",
    )
        .into_response()
}

async fn instances(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::proto::InstanceInfo>>, StatusCode> {
    require_session(&state, &headers)?;
    Ok(Json(state.hub.list_instances().await))
}

async fn client_ws(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if session_from_headers(&state, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_client_ws(state, socket))
}

async fn agent_ws(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !agent_token_valid(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_agent_ws(state, socket))
}

fn session_from_headers(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    let token = extract_session_cookie(cookie)?;
    state.sessions.validate(&token).then_some(token)
}

fn require_session(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    session_from_headers(state, headers)
        .map(|_| ())
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn agent_token_valid(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    constant_time_eq(&digest, &state.agent_token_digest)
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn send_json<T: serde::Serialize>(sink: &mpsc::Sender<Message>, msg: &T) {
    let Some(text) = crate::proto::to_json_text(msg) else {
        return;
    };
    // If the socket task is gone there is nothing to do.
    let _ = sink.send(Message::Text(text.into())).await;
}

async fn handle_client_ws(state: Arc<AppState>, socket: WebSocket) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_sink.send(msg).await.is_err() {
                break;
            }
        }
    });
    let client_id = {
        let (typed_tx, mut typed_rx) = mpsc::channel::<GatewayToClient>(64);
        let id = state.hub.client_connected(typed_tx).await;
        let bridge_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = typed_rx.recv().await {
                let Some(text) = crate::proto::to_json_text(&msg) else {
                    continue;
                };
                if bridge_tx.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        });
        id
    };

    while let Some(Ok(msg)) = ws_stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<ClientToGateway>(&text) else {
            continue;
        };
        match parsed {
            ClientToGateway::Rpc {
                id,
                instance,
                method,
                params,
            } => {
                if !is_read_only_method(&method) {
                    send_json(
                        &out_tx,
                        &GatewayToClient::RpcResponse {
                            id,
                            outcome: RpcOutcome::Err {
                                message: format!(
                                    "method `{method}` not allowed in read-only slice"
                                ),
                            },
                        },
                    )
                    .await;
                    continue;
                }
                if state
                    .hub
                    .route_rpc(client_id, id, &instance, method, params)
                    .await
                    .is_err()
                {
                    send_json(
                        &out_tx,
                        &GatewayToClient::RpcResponse {
                            id,
                            outcome: RpcOutcome::Err {
                                message: format!("instance `{instance}` unavailable"),
                            },
                        },
                    )
                    .await;
                }
            }
        }
    }
    state.hub.client_disconnected(client_id).await;
    drop(out_tx);
    let _ = writer.await;
}

async fn handle_agent_ws(state: Arc<AppState>, socket: WebSocket) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_sink.send(msg).await.is_err() {
                break;
            }
        }
    });
    // Bridge hub -> agent: the hub gets an mpsc of typed messages.
    let (agent_tx, mut agent_rx) = mpsc::channel::<GatewayToAgent>(64);
    let bridge_tx = out_tx.clone();
    let bridge = tokio::spawn(async move {
        while let Some(msg) = agent_rx.recv().await {
            let Some(text) = crate::proto::to_json_text(&msg) else {
                continue;
            };
            if bridge_tx.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut agent_id = None;
    while let Some(Ok(msg)) = ws_stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<AgentToGateway>(&text) else {
            continue;
        };
        match parsed {
            AgentToGateway::Hello { machine, instances } => {
                let id = state
                    .hub
                    .agent_connected(machine, instances, agent_tx.clone())
                    .await;
                agent_id = Some(id);
            }
            AgentToGateway::Event { instance, payload } => {
                state.hub.fanout_event(&instance, payload).await;
            }
            AgentToGateway::RpcResponse { id, outcome } => {
                state.hub.resolve_rpc(id, outcome).await;
            }
            AgentToGateway::Status { instance, online } => {
                state.hub.set_instance_status(&instance, online).await;
            }
        }
    }
    if let Some(id) = agent_id {
        state.hub.agent_disconnected(id).await;
    }
    drop(agent_tx);
    drop(out_tx);
    let _ = bridge.await;
    let _ = writer.await;
}
