// ---------------------------------------------------------------------------
// http.rs — Axum route handlers for WP2
// ---------------------------------------------------------------------------

use crate::ctx::Ctx;
use crate::server::events::WsSink;
use anyhow::Context as _;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Multipart, Path, Query, Request, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use margins_meeting_protocol::{
    AudioChunkBatchV1, AudioChunkV1, ClientMessageBodyV1, ClientMessageV1, ContentDigestV1,
    DigestAlgorithmV1, DurationMillis, MessageId, ProtocolVersionV1, SessionId, SessionMillis,
    UnixMillis, WorkspaceAttachV1, WorkspaceErrorV1, WorkspaceMemoReplaceV1, WorkspaceMemoUpdateV1,
    WorkspaceNoteAssociationUpdateV1, WorkspaceRenameV1, WorkspaceResponseV1,
    AUDIO_CHUNK_BATCH_CONTENT_TYPE_V1,
};
use margins_workflows::workspace_service::{
    is_memo_revision_conflict, ScopedCredentialStore, ServicePrincipal, WorkspaceService,
    OP_CAPTURE_WRITE, OP_MEMO_WRITE, OP_SESSION_CREATE, OP_SESSION_READ, OP_SESSION_WRITE,
    OP_WORKSPACE_READ,
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
    pub workspace_service: Arc<WorkspaceService>,
    pub credential_store: ScopedCredentialStore,
    pub service_principal: ServicePrincipal,
    pub asr_setup: super::asr_state::SpeechSetup,
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    pub remote_asr_jobs: super::remote_asr::RemoteAsrJobs,
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
        // Current Workspace authority API. Unlike /api/invoke, these routes
        // are typed, explicitly Workspace-scoped, and use the shared service.
        .route("/v1/capabilities", get(workspace_capabilities))
        .route("/v1/workspaces/:workspace/speech-setup", get(workspace_speech_setup).post(workspace_retry_speech_setup))
        .route("/v1/workspaces/:workspace", get(workspace_summary))
        .route("/v1/workspaces/:workspace/recall", post(workspace_recall))
        .route("/v1/workspaces/:workspace/current", get(workspace_current))
        .route(
            "/v1/workspaces/:workspace/active-sessions",
            get(workspace_active_sessions),
        )
        .route(
            "/v1/workspaces/:workspace/sessions",
            get(workspace_sessions).post(workspace_create_session),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session",
            get(workspace_session).delete(workspace_discard_session),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/commands",
            post(workspace_session_command),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/live-checkpoint",
            put(workspace_live_checkpoint).layer(DefaultBodyLimit::max(
                margins_workflows::workspace_service::MAX_LIVE_CHECKPOINT_BYTES,
            )),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/attach",
            post(workspace_attach_session),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/title",
            put(workspace_rename_session),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/segments/:segment/lanes/:lane/chunks/:sequence",
            put(workspace_audio_chunk),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/audio-chunks",
            post(workspace_audio_chunks).layer(DefaultBodyLimit::max(
                ((margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES + 64 * 1024)
                    * u64::from(
                        margins_workflows::workspace_service::DEFAULT_MAX_IN_FLIGHT_CHUNKS,
                    )) as usize,
            )),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/events",
            get(workspace_events),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/transcript",
            get(workspace_transcript),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/artifacts",
            get(workspace_artifacts),
        )
        .route(
            "/v1/workspaces/:workspace/artifacts/:artifact/content",
            get(workspace_artifact_content),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/memo",
            get(workspace_memo).put(workspace_update_memo),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/memo/replace",
            post(workspace_replace_memo),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/note-association",
            get(workspace_note_association)
                .put(workspace_link_note)
                .delete(workspace_unlink_note),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/jobs/latest",
            get(workspace_latest_job),
        )
        .route(
            "/v1/workspaces/:workspace/sessions/:session/jobs/transcribe",
            post(workspace_request_transcription),
        )
        .route(
            "/v1/workspaces/:workspace/imports",
            post(workspace_multipart_import).layer(DefaultBodyLimit::max(
                margins_workflows::workspace_service::DEFAULT_MAX_IMPORT_BYTES as usize
                    + 1024 * 1024,
            )),
        )
        .route(
            "/v1/workspaces/:workspace/imports/:upload",
            get(workspace_import_receipt)
                .put(workspace_import)
                .layer(DefaultBodyLimit::max(
                    margins_workflows::workspace_service::DEFAULT_MAX_IMPORT_BYTES as usize,
                )),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions",
            post(browser_start),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/snapshot",
            get(browser_snapshot),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/pause",
            post(browser_pause),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/resume",
            post(browser_resume),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/heartbeat",
            post(browser_heartbeat),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/stop",
            post(browser_stop),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/notepad",
            put(browser_notepad),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/chunks/:sequence",
            put(browser_chunk),
        )
        // WebSocket — token auth via ?token= query param
        .route("/ws/events", get(ws_handler))
        // SPA static assets — no auth, token injected into index.html
        .fallback(assets_fallback)
        .with_state(state)
}

fn workspace_auth(
    state: &ServerState,
    headers: &HeaderMap,
    workspace: Option<&str>,
) -> Result<ServicePrincipal, Response> {
    let Some(token) = headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Err(workspace_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            false,
            "Unauthorized",
        ));
    };
    if workspace.is_some_and(|value| value != state.workspace_service.workspace_id()) {
        return Err(workspace_error(
            StatusCode::NOT_FOUND,
            "not_found",
            false,
            "Workspace not found",
        ));
    }
    state
        .credential_store
        .authorize(token, state.workspace_service.workspace_id())
        .map_err(|_| {
            workspace_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                false,
                "Unauthorized",
            )
        })
}

fn workspace_auth_operation(
    state: &ServerState,
    headers: &HeaderMap,
    workspace: &str,
    operation: &str,
) -> Result<ServicePrincipal, Response> {
    let principal = workspace_auth(state, headers, Some(workspace))?;
    principal
        .require(state.workspace_service.workspace_id(), operation)
        .map_err(|_| {
            workspace_error(
                StatusCode::FORBIDDEN,
                "forbidden",
                false,
                "Credential does not permit this operation",
            )
        })?;
    Ok(principal)
}

fn workspace_instance_fence(state: &ServerState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(instance_id) = headers
        .get("X-Margins-Instance-Id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    else {
        return Err(workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "instance_id_required",
            false,
            "Missing Workspace instance identity",
        ));
    };
    if instance_id != state.workspace_service.instance_id() {
        return Err(workspace_error(
            StatusCode::CONFLICT,
            "instance_changed",
            false,
            "Workspace service instance identity changed",
        ));
    }
    Ok(())
}

fn workspace_ok<T: serde::Serialize>(value: T) -> Response {
    Json(WorkspaceResponseV1 {
        ok: true,
        result: Some(value),
        error: None,
    })
    .into_response()
}

fn workspace_error(
    status: StatusCode,
    code: &str,
    retryable: bool,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(WorkspaceResponseV1::<Value> {
            ok: false,
            result: None,
            error: Some(WorkspaceErrorV1 {
                code: code.to_string(),
                retryable,
                request_id: None,
                message: message.into(),
            }),
        }),
    )
        .into_response()
}

fn service_error(error: anyhow::Error) -> Response {
    let message = error.to_string();
    let (status, code) = if is_memo_revision_conflict(&error)
        || message.contains("revision conflict")
        || message.contains("changed somewhere else")
        || message.contains("different content")
        || message.contains("already has")
    {
        (StatusCode::CONFLICT, "conflict")
    } else if message.contains("not authorized") || message.contains("authorization failed") {
        (StatusCode::FORBIDDEN, "forbidden")
    } else if message.contains("unavailable") {
        (StatusCode::SERVICE_UNAVAILABLE, "capability_unavailable")
    } else if message.contains("not found") {
        (StatusCode::NOT_FOUND, "not_found")
    } else if message.contains("exceeds") {
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    } else {
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_request")
    };
    workspace_error(status, code, false, message)
}

#[derive(serde::Deserialize)]
struct SessionPageQuery {
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
struct EventPageQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
struct RecallBody {
    query: String,
    source: Option<String>,
}

async fn workspace_capabilities(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    let principal = match workspace_auth(&state, &headers, None) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .capabilities(&principal)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_speech_setup(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_WORKSPACE_READ)
    {
        return response;
    }
    workspace_ok(state.asr_setup.snapshot())
}

async fn workspace_retry_speech_setup(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_SESSION_WRITE)
    {
        return response;
    }
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    {
        return state
            .asr_setup
            .start(
                state.workspace_service.clone(),
                state.service_principal.clone(),
                state.remote_asr_jobs.clone(),
            )
            .map(|_| workspace_ok(state.asr_setup.snapshot()))
            .unwrap_or_else(service_error);
    }
    #[cfg(not(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    )))]
    workspace_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "asr_unavailable",
        false,
        "This server cannot transcribe recordings",
    )
}

async fn workspace_summary(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .summary(&principal)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_recall(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
    Json(request): Json<RecallBody>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .recall(&principal, &request.query, request.source.as_deref())
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_current(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .current(&principal)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_active_sessions(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .active_sessions(&principal)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_session(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .session(&principal, &SessionId(session))
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_discard_session(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    state
        .workspace_service
        .discard_session(&principal, &SessionId(session))
        .map(|_| workspace_ok(json!({"discarded": true})))
        .unwrap_or_else(service_error)
}

async fn workspace_live_checkpoint(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(checkpoint): Json<Value>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    let Some(token) = headers
        .get("X-Margins-Producer-Token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    else {
        return workspace_error(
            StatusCode::UNAUTHORIZED,
            "producer_token_required",
            false,
            "Missing producer token",
        );
    };
    state
        .workspace_service
        .publish_live_checkpoint(&principal, token, &SessionId(session), &checkpoint)
        .map(|_| {
            workspace_ok(json!({
                "accepted": true,
                "decoded_until_ms": checkpoint["decoded_until_ms"],
                "committed_until_ms": checkpoint["committed_until_ms"],
            }))
        })
        .unwrap_or_else(service_error)
}

async fn workspace_sessions(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    Query(query): Query<SessionPageQuery>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .sessions(
            &principal,
            query.after.as_deref(),
            query.limit.unwrap_or(50),
        )
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_create_session(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
    Json(command): Json<ClientMessageV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    state
        .workspace_service
        .reserve_session(&principal, command)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_session_command(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(command): Json<ClientMessageV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    if command.session_id.as_ref() != session {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "session_mismatch",
            false,
            "path and command session IDs differ",
        );
    }
    let Some(token) = headers
        .get("X-Margins-Producer-Token")
        .and_then(|value| value.to_str().ok())
    else {
        return workspace_error(
            StatusCode::UNAUTHORIZED,
            "producer_token_required",
            false,
            "Missing producer token",
        );
    };
    let finalized = matches!(&command.body, ClientMessageBodyV1::FinalizeSession(_));
    let result = state
        .workspace_service
        .execute_capture(&principal, token, command);
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    if finalized && result.is_ok() && state.workspace_service.asr_available() {
        if let Ok(Some(job)) = state
            .workspace_service
            .latest_job(&state.service_principal, &SessionId(session))
        {
            state.remote_asr_jobs.schedule(
                state.workspace_service.clone(),
                state.service_principal.clone(),
                job,
            );
        }
    }
    #[cfg(not(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    )))]
    let _ = finalized;
    result.map(workspace_ok).unwrap_or_else(service_error)
}

async fn workspace_attach_session(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceAttachV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    state
        .workspace_service
        .attach_session(&principal, &SessionId(session), &request)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_rename_session(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceRenameV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .rename_session(&principal, &SessionId(session), &request)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_audio_chunk(
    State(state): State<ServerState>,
    Path((workspace, session, segment, lane, sequence)): Path<(
        String,
        String,
        String,
        String,
        u64,
    )>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    if body.len() as u64 > margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES {
        return workspace_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            false,
            "Audio chunk exceeds advertised maximum",
        );
    }
    let required = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
    };
    let Some(token) = required("X-Margins-Producer-Token") else {
        return workspace_error(
            StatusCode::UNAUTHORIZED,
            "producer_token_required",
            false,
            "Missing producer token",
        );
    };
    let Some(message_id) = required("X-Margins-Message-Id") else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata_required",
            false,
            "Missing message ID",
        );
    };
    let Some(starts_at_ms) =
        required("X-Margins-Starts-At-Ms").and_then(|value| value.parse().ok())
    else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata_required",
            false,
            "Missing starts-at timestamp",
        );
    };
    let Some(duration_ms) = required("X-Margins-Duration-Ms").and_then(|value| value.parse().ok())
    else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata_required",
            false,
            "Missing duration",
        );
    };
    let Some(digest) = required("X-Margins-Sha256") else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata_required",
            false,
            "Missing SHA-256 digest",
        );
    };
    let sent_at = required("X-Margins-Sent-At-Unix-Ms")
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    let command = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: MessageId(message_id.to_string()),
        session_id: SessionId(session),
        sent_at_unix_ms: UnixMillis(sent_at),
        body: ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: segment.into(),
            lane_id: lane.into(),
            sequence,
            starts_at_ms: SessionMillis(starts_at_ms),
            duration_ms: DurationMillis(duration_ms),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: digest.to_string(),
            },
            payload: body.to_vec(),
        }),
    };
    state
        .workspace_service
        .execute_capture(&principal, token, command)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_audio_chunks(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    let Some(token) = headers
        .get("X-Margins-Producer-Token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    else {
        return workspace_error(
            StatusCode::UNAUTHORIZED,
            "producer_token_required",
            false,
            "Missing producer token",
        );
    };
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some(AUDIO_CHUNK_BATCH_CONTENT_TYPE_V1) {
        return workspace_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            false,
            "Audio batch content type is unsupported",
        );
    }
    let batch = match AudioChunkBatchV1::decode(
        &body,
        margins_workflows::workspace_service::DEFAULT_MAX_IN_FLIGHT_CHUNKS as usize,
        margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES,
    ) {
        Ok(value) => value,
        Err(error) => {
            return workspace_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_audio_batch",
                false,
                error,
            );
        }
    };
    state
        .workspace_service
        .execute_audio_batch(&principal, token, &SessionId(session), batch.commands)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_events(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    Query(query): Query<EventPageQuery>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .events(
            &principal,
            &SessionId(session),
            query.after,
            query.limit.unwrap_or(256),
        )
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_transcript(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .transcript(&principal, &session)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_artifacts(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .artifacts(&principal, &session)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_artifact_content(
    State(state): State<ServerState>,
    Path((workspace, artifact)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .workspace_service
        .artifact_content(&principal, &artifact)
    {
        Ok(bytes) => (
            StatusCode::OK,
            [("Content-Type", "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(error) => service_error(error),
    }
}

async fn workspace_memo(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .memo(&principal, &SessionId(session))
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_update_memo(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceMemoUpdateV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .update_memo(&principal, &SessionId(session), &request)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_replace_memo(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceMemoReplaceV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    state
        .workspace_service
        .replace_memo(&principal, &SessionId(session), &request)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

#[derive(serde::Deserialize)]
struct RevisionQuery {
    expected_revision: u64,
    request_id: String,
}

async fn workspace_note_association(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .note_association(&principal, &SessionId(session))
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_link_note(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceNoteAssociationUpdateV1>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .link_note(&principal, &SessionId(session), &request)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_unlink_note(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    Query(request): Query<RevisionQuery>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .unlink_note(
            &principal,
            &SessionId(session),
            &request.request_id,
            request.expected_revision,
        )
        .map(|()| workspace_ok(serde_json::json!({"unlinked": true})))
        .unwrap_or_else(service_error)
}

async fn workspace_latest_job(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .latest_job(&principal, &SessionId(session))
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_request_transcription(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth_operation(&state, &headers, &workspace, OP_SESSION_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = workspace_instance_fence(&state, &headers) {
        return response;
    }
    let job = match state
        .workspace_service
        .request_transcription_job(&principal, &SessionId(session))
    {
        Ok(value) => value,
        Err(error) => return service_error(error),
    };
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    if state.workspace_service.asr_available() {
        state.remote_asr_jobs.schedule(
            state.workspace_service.clone(),
            state.service_principal.clone(),
            job.clone(),
        );
    }
    workspace_ok(job)
}

async fn workspace_import(
    State(state): State<ServerState>,
    Path((workspace, upload)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = headers
        .get("X-Margins-Session-Id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(&upload);
    let filename = headers
        .get("X-Margins-Filename")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("recording");
    let title = headers
        .get("X-Margins-Title")
        .and_then(|value| value.to_str().ok());
    state
        .workspace_service
        .import_finished_file(&principal, &upload, session, filename, title, &body)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_multipart_import(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut upload_id = None;
    let mut session_id = None;
    let mut filename = None;
    let mut title = None;
    let mut file = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return service_error(error.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        let supplied_filename = field.file_name().map(str::to_string);
        match name.as_str() {
            "file" => {
                filename = supplied_filename.or(filename);
                match field.bytes().await {
                    Ok(bytes)
                        if bytes.len() as u64
                            <= margins_workflows::workspace_service::DEFAULT_MAX_IMPORT_BYTES =>
                    {
                        file = Some(bytes)
                    }
                    Ok(_) => {
                        return workspace_error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "too_large",
                            false,
                            "Import exceeds the advertised maximum",
                        );
                    }
                    Err(error) => return service_error(error.into()),
                }
            }
            "upload_id" | "session_id" | "filename" | "title" => {
                let value = match field.text().await {
                    Ok(value) => value,
                    Err(error) => return service_error(error.into()),
                };
                match name.as_str() {
                    "upload_id" => upload_id = Some(value),
                    "session_id" => session_id = Some(value),
                    "filename" => filename = Some(value),
                    "title" => title = Some(value),
                    _ => unreachable!(),
                }
            }
            _ => {}
        }
    }
    let Some(upload_id) = upload_id.filter(|value| !value.is_empty()) else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata_required",
            false,
            "Multipart import requires upload_id",
        );
    };
    let session_id = session_id.unwrap_or_else(|| upload_id.clone());
    let filename = filename.unwrap_or_else(|| "recording".to_string());
    let Some(file) = file else {
        return workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "file_required",
            false,
            "Multipart import requires an original file",
        );
    };
    state
        .workspace_service
        .import_finished_file(
            &principal,
            &upload_id,
            &session_id,
            &filename,
            title.as_deref(),
            &file,
        )
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn workspace_import_receipt(
    State(state): State<ServerState>,
    Path((workspace, upload)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    state
        .workspace_service
        .import_receipt(&principal, &upload)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserOwnerBody {
    owner_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserStartBody {
    owner_id: String,
    name: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserNotepadBody {
    owner_id: String,
    expected_revision: String,
    text: String,
}

fn browser_snapshot_value(
    state: &ServerState,
    recording: &str,
    owner: &str,
) -> anyhow::Result<Value> {
    let status = crate::web_session::web_recording_status_by_id(&state.ctx.0.state, recording)
        .context("hosted recording not found")?;
    let notepad =
        crate::web_session::get_web_recording_notepad(&state.ctx.0.state, recording, owner)
            .map_err(anyhow::Error::msg)?;
    Ok(json!({
        "recordingId": recording,
        "sessionId": status.session_name,
        "status": if status.capture_phase == "finalizing" { "saving" } else if status.paused { "paused" } else { "recording" },
        "notepad": notepad,
    }))
}

async fn browser_start(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BrowserStartBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_SESSION_CREATE)
    {
        return response;
    }
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    if let Err(error) = crate::validate_session_name(&body.name) {
        return service_error(anyhow::anyhow!(error));
    }
    let work_dir = state.ctx.0.state.work_dir.lock().unwrap().clone();
    let margins_dir = work_dir.join(".margins");
    if let Err(error) = std::fs::create_dir_all(&margins_dir) {
        return service_error(error.into());
    }
    let name = crate::unique_session_name(&work_dir, &margins_dir, &body.name);
    match crate::web_session::start_web_recording(
        &state.ctx.0.state,
        work_dir,
        name,
        body.owner_id.clone(),
    ) {
        Ok(started) => browser_snapshot_value(&state, &started.recording_id, &body.owner_id)
            .map(workspace_ok)
            .unwrap_or_else(service_error),
        Err(error) => service_error(anyhow::anyhow!(error)),
    }
}

async fn browser_snapshot(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    Query(owner): Query<BrowserOwnerBody>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_SESSION_READ) {
        return response;
    }
    browser_snapshot_value(&state, &recording, &owner.owner_id)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

fn browser_pause_value(
    state: &ServerState,
    recording: &str,
    owner: &str,
    paused: bool,
) -> anyhow::Result<Value> {
    crate::web_session::set_web_recording_paused(&state.ctx.0.state, recording, owner, paused)
        .map_err(anyhow::Error::msg)?;
    browser_snapshot_value(state, recording, owner)
}

async fn browser_pause(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(owner): Json<BrowserOwnerBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    browser_pause_value(&state, &recording, &owner.owner_id, true)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn browser_resume(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(owner): Json<BrowserOwnerBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    browser_pause_value(&state, &recording, &owner.owner_id, false)
        .map(workspace_ok)
        .unwrap_or_else(service_error)
}

async fn browser_heartbeat(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(owner): Json<BrowserOwnerBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    match crate::web_session::heartbeat_web_recording(
        &state.ctx.0.state,
        &recording,
        &owner.owner_id,
    ) {
        Ok(()) => browser_snapshot_value(&state, &recording, &owner.owner_id)
            .map(workspace_ok)
            .unwrap_or_else(service_error),
        Err(error) => service_error(anyhow::anyhow!(error)),
    }
}

async fn browser_stop(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(owner): Json<BrowserOwnerBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    let app = state.ctx.0.state.clone();
    match tokio::task::spawn_blocking(move || {
        crate::web_session::stop_web_recording(&app, &recording, &owner.owner_id)
    })
    .await
    {
        Ok(Ok(session_id)) => workspace_ok(json!({"sessionId":session_id, "inputFinalized":true})),
        Ok(Err(error)) => service_error(anyhow::anyhow!(error)),
        Err(error) => service_error(error.into()),
    }
}

async fn browser_notepad(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<BrowserNotepadBody>,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_MEMO_WRITE) {
        return response;
    }
    match crate::web_session::update_web_recording_notepad(
        &state.ctx.0.state,
        &recording,
        &body.owner_id,
        &body.expected_revision,
        &body.text,
    ) {
        Ok(_) => browser_snapshot_value(&state, &recording, &body.owner_id)
            .map(workspace_ok)
            .unwrap_or_else(service_error),
        Err(error) => service_error(anyhow::anyhow!(error)),
    }
}

async fn browser_chunk(
    State(state): State<ServerState>,
    Path((workspace, recording, sequence)): Path<(String, String, u64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = workspace_auth_operation(&state, &headers, &workspace, OP_CAPTURE_WRITE)
    {
        return response;
    }
    let Some(owner) = capture_owner(&headers) else {
        return workspace_error(
            StatusCode::UNAUTHORIZED,
            "owner_required",
            false,
            "Missing capture owner",
        );
    };
    match crate::web_session::handle_audio_chunk(
        &state.ctx.0.state,
        &recording,
        owner,
        sequence,
        &body,
    ) {
        Ok(()) => workspace_ok(json!({"sequence":sequence, "durable":true})),
        Err(error) => service_error(anyhow::anyhow!(error)),
    }
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
        "workspace_protocol": 1,
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

#[cfg(all(
    test,
    any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    )
))]
mod remote_finalize_authority_tests {
    use super::*;
    use axum::http::{header::AUTHORIZATION, HeaderValue};
    use margins_meeting_protocol::{
        ClientMessageBodyV1, SegmentCloseReasonV1, SessionFinalizeReasonV1,
    };
    use margins_workflows::{
        remote_workspace::{
            native_create_session_command, DurableTransferSpool, NativeRemoteLane,
            NativeRemoteTransfer,
        },
        workspace::ensure_service_workspace,
        workspace_service::{OP_CAPTURE_WRITE, OP_SESSION_CREATE},
    };

    #[tokio::test]
    async fn capture_only_http_finalize_schedules_with_internal_service_principal() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        let captures = temp.path().join("captures");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = ensure_service_workspace(
            &temp.path().join("home"),
            "http-authority",
            Some("HTTP authority"),
            &notes,
            &captures,
        )
        .unwrap();
        let service = Arc::new(
            WorkspaceService::open_with_capabilities(
                "http-authority-instance",
                workspace,
                true,
                false,
            )
            .unwrap(),
        );
        let capture = ServicePrincipal::scoped(
            "capture-only",
            ["http-authority".to_string()],
            [OP_SESSION_CREATE.to_string(), OP_CAPTURE_WRITE.to_string()],
        );
        let internal = ServicePrincipal::full("service-internal", "http-authority");
        let session = SessionId("http-capture-finalize".into());
        let reservation = service
            .reserve_session(
                &capture,
                native_create_session_command(
                    session.as_ref(),
                    "http-capture-finalize",
                    None,
                    "test",
                ),
            )
            .unwrap();
        let spool = DurableTransferSpool::create(
            &temp.path().join("spool"),
            "http-capture-finalize",
            "http-authority-instance",
            "https://fixture.invalid",
            "http-authority",
            session.as_ref(),
            &reservation.producer_token,
            0,
        )
        .unwrap();
        let mut transfer = NativeRemoteTransfer::new(spool);
        transfer.begin_segment("segment".into(), 0).unwrap();
        for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
            transfer
                .append_s16le(lane, 16_000, &vec![0; 3_200])
                .unwrap();
        }
        let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
        let ended_at_ms = match &close.body {
            ClientMessageBodyV1::CloseSegment(close) => close.ended_at_ms.0,
            _ => unreachable!(),
        };
        let finalize = transfer
            .seal_session(ended_at_ms, SessionFinalizeReasonV1::Completed)
            .unwrap();
        for chunk in transfer.spool().pending_chunks().unwrap() {
            service
                .execute_capture(&capture, &reservation.producer_token, chunk.command)
                .unwrap();
        }
        service
            .execute_capture(&capture, &reservation.producer_token, close)
            .unwrap();

        let credentials =
            ScopedCredentialStore::open(temp.path().join("credentials.json")).unwrap();
        credentials
            .register(
                "capture-only",
                "capture-token",
                vec!["http-authority".to_string()],
                vec![OP_CAPTURE_WRITE.to_string()],
                None,
            )
            .unwrap();
        let sink = Arc::new(WsSink::new(8));
        let ctx = crate::ctx::Ctx {
            state: crate::build_app_state(
                temp.path().join("work"),
                crate::settings::Settings::default(),
            ),
            sink: sink.clone(),
        };
        let state = ServerState {
            ctx: Arc::new(CtxState(ctx)),
            sink,
            token: "unused-admin-token".to_string(),
            workspace_service: service.clone(),
            credential_store: credentials,
            service_principal: internal.clone(),
            asr_setup: super::super::asr_state::SpeechSetup::new(true, true),
            remote_asr_jobs: super::super::remote_asr::RemoteAsrJobs::default(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer capture-token"),
        );
        headers.insert(
            "X-Margins-Producer-Token",
            HeaderValue::from_str(&reservation.producer_token).unwrap(),
        );
        headers.insert(
            "X-Margins-Instance-Id",
            HeaderValue::from_static("http-authority-instance"),
        );
        assert!(service.latest_job(&capture, &session).is_err());
        let response = workspace_session_command(
            State(state),
            Path(("http-authority".to_string(), session.as_ref().to_string())),
            headers,
            Json(finalize),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let job = service.latest_job(&internal, &session).unwrap().unwrap();
            if job.status != "queued" {
                assert!(matches!(
                    job.status.as_str(),
                    "running" | "complete" | "failed"
                ));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "internal scheduler never claimed the durable job"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
