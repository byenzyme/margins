// ---------------------------------------------------------------------------
// http.rs — Axum route handlers for WP2
// ---------------------------------------------------------------------------

use crate::ctx::Ctx;
use crate::server::events::WsSink;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, Request, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

// ---------------------------------------------------------------------------
// Shared server state passed to every handler
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ServerState {
    pub ctx: Arc<CtxState>,
    pub sink: Arc<WsSink>,
    pub token: String,
}

/// Wrapper so `Ctx` (which is not Clone) lives behind an Arc.
pub struct CtxState(pub Ctx);

// ---------------------------------------------------------------------------
// Build the router — all routes share the same ServerState
// ---------------------------------------------------------------------------

pub fn build_router(state: ServerState) -> Router {
    let health_cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Health — no auth, permissive CORS
        .route("/health", get(health_handler).layer(health_cors))
        // Authenticated API — auth checked inside each handler
        .route("/api/invoke/:command", post(invoke_handler))
        .route("/api/audio/chunk", post(audio_chunk_handler))
        .route("/api/live-audio/pcm", post(live_pcm_handler))
        // WebSocket — token auth via ?token= query param
        .route("/ws/events", get(ws_handler))
        // SPA static assets — no auth, token injected into index.html
        .fallback(assets_fallback)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Auth helper — used inside handlers that need bearer auth
// ---------------------------------------------------------------------------

fn check_bearer(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {}", expected))
        .unwrap_or(false)
}

fn capture_owner(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("X-Margins-Capture-Owner")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

fn capture_recording_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("X-Margins-Recording-Id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

fn capture_protocol_is_current(headers: &HeaderMap) -> bool {
    headers
        .get("X-Margins-Capture-Protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u8>().ok())
        == Some(crate::web_session::HOSTED_CAPTURE_PROTOCOL_VERSION)
}

fn capture_protocol_error() -> Response {
    Json(json!({
        "ok": false,
        "error": format!(
            "Hosted capture protocol changed to v{}; reload this browser tab before recording.",
            crate::web_session::HOSTED_CAPTURE_PROTOCOL_VERSION
        )
    }))
    .into_response()
}

fn hosted_capture_command(command: &str) -> bool {
    matches!(
        command,
        "start_recording"
            | "stop_recording"
            | "discard_recording"
            | "pause_recording"
            | "resume_recording"
            | "get_web_recording_status"
            | "list_web_recording_recoveries"
            | "sync_memo"
            | "checkpoint_memo_line"
            | "heartbeat_web_recording"
            | "claim_web_recording_recovery"
            | "hydrate_web_recording_memo"
            | "update_web_recording_notepad"
            | "get_web_recording_notepad"
            | "request_backchannel_for_memo"
            | "steer_backchannel_for_memo"
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /health — no auth
async fn health_handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": {
            "recall": cfg!(feature = "recall"),
            "parakeet_asr": cfg!(feature = "parakeet-asr"),
        },
    }))
}

/// POST /api/invoke/{command} — dispatch to the shared command router
async fn invoke_handler(
    State(state): State<ServerState>,
    Path(command): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !check_bearer(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    if hosted_capture_command(&command) && !capture_protocol_is_current(&headers) {
        return capture_protocol_error();
    }

    let args: Value = if body.is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Json(json!({ "ok": false, "error": format!("invalid JSON body: {e}") }))
                    .into_response();
            }
        }
    };

    let ctx = &state.ctx.0;
    match crate::dispatch::dispatch(ctx, &command, args).await {
        Ok(result) => Json(json!({ "ok": true, "result": result })).into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct LivePcmQuery {
    channel: Option<String>,
    sample_rate: Option<u32>,
}

/// POST raw little-endian f32 PCM into the continuously decoding Linux worker.
async fn live_pcm_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<LivePcmQuery>,
    body: Bytes,
) -> Response {
    if !check_bearer(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    if !capture_protocol_is_current(&headers) {
        return capture_protocol_error();
    }
    let Some(recording_id) = capture_recording_id(&headers) else {
        return Json(json!({ "ok": false, "error": "missing X-Margins-Recording-Id header" }))
            .into_response();
    };
    let Some(owner_id) = capture_owner(&headers) else {
        return Json(json!({ "ok": false, "error": "missing X-Margins-Capture-Owner header" }))
            .into_response();
    };
    let channel = match query.channel.as_deref().unwrap_or("mic") {
        "mic" => margins::recorder::LiveAudioChannel::Mic,
        "system" => margins::recorder::LiveAudioChannel::System,
        other => {
            return Json(json!({ "ok": false, "error": format!("unknown channel: {other}") }))
                .into_response();
        }
    };
    let sample_rate = query.sample_rate.unwrap_or(16_000);
    match crate::web_session::handle_live_pcm_chunk(
        &state.ctx.0.state,
        recording_id,
        owner_id,
        channel,
        sample_rate,
        &body,
    ) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => Json(json!({ "ok": false, "error": error })).into_response(),
    }
}

/// POST /api/audio/chunk — append a browser webm/opus chunk. Both the
/// non-secret recording ID and secret owner capability stay out of URLs/logs.
async fn audio_chunk_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !check_bearer(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    if !capture_protocol_is_current(&headers) {
        return capture_protocol_error();
    }

    let Some(recording_id) = capture_recording_id(&headers) else {
        return Json(json!({ "ok": false, "error": "missing X-Margins-Recording-Id header" }))
            .into_response();
    };
    let Some(owner_id) = capture_owner(&headers) else {
        return Json(json!({ "ok": false, "error": "missing X-Margins-Capture-Owner header" }))
            .into_response();
    };
    let Some(sequence) = headers
        .get("X-Margins-Chunk-Sequence")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return Json(
            json!({ "ok": false, "error": "missing or invalid X-Margins-Chunk-Sequence header" }),
        )
        .into_response();
    };

    match crate::web_session::handle_audio_chunk(
        &state.ctx.0.state,
        recording_id,
        owner_id,
        sequence,
        &body,
    ) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

/// Fallback: serve embedded static assets with SPA index.html fallback.
async fn assets_fallback(State(state): State<ServerState>, request: Request) -> Response {
    crate::server::assets::serve_asset(request.uri().path(), &state.token)
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct WsQuery {
    token: Option<String>,
}

/// GET /ws/events — token auth via ?token= query param, then upgrade
async fn ws_handler(
    State(state): State<ServerState>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let provided = query.token.unwrap_or_default();
    if provided != state.token {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let sink = state.sink.clone();
    ws.on_upgrade(move |socket| handle_ws(socket, sink))
}

async fn handle_ws(mut socket: WebSocket, sink: Arc<WsSink>) {
    let mut rx = sink.subscribe();
    loop {
        tokio::select! {
            // Forward broadcast events to the WebSocket client
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Slow consumer — skip missed messages, keep going
                    }
                }
            }
            // Handle client messages (ping/close)
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}
