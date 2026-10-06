//! Typed Workspace HTTP routes over the shared service.

use crate::ServerState;
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
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
use margins_workflows::workspace_service::{ServicePrincipal, OP_SESSION_WRITE, OP_WORKSPACE_READ};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};

pub fn build_router(state: ServerState) -> Router {
    let health_cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true})) }).layer(health_cors))
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
            "/v1/workspaces/:workspace/browser/sessions",
            post(crate::browser::start),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/snapshot",
            get(crate::browser::snapshot),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/pause",
            post(crate::browser::pause),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/resume",
            post(crate::browser::resume),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/heartbeat",
            post(crate::browser::heartbeat),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/stop",
            post(crate::browser::stop),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/finish-incomplete",
            post(crate::browser::finish_incomplete),
        )
        .route(
            "/v1/workspaces/:workspace/browser/sessions/:recording/chunks/:sequence",
            put(crate::browser::chunk).layer(DefaultBodyLimit::max(
                margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES as usize,
            )),
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

pub(crate) fn workspace_auth_operation(
    state: &ServerState,
    headers: &HeaderMap,
    workspace: &str,
    operation: &str,
) -> Result<ServicePrincipal, Response> {
    let principal = workspace_auth(state, headers, Some(workspace))?;
    workspace_instance_fence(state, headers)?;
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

pub(crate) fn workspace_ok<T: serde::Serialize>(value: T) -> Response {
    Json(WorkspaceResponseV1 {
        ok: true,
        result: Some(value),
        error: None,
    })
    .into_response()
}

pub(crate) fn workspace_error(
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

pub(crate) fn service_error(error: anyhow::Error) -> Response {
    let message = error.to_string();
    let (status, code) = if message.contains("revision conflict")
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
        .map(|capabilities| {
            let mut value =
                serde_json::to_value(capabilities).expect("Workspace capabilities serialize");
            value["capture_protocol_version"] = json!(crate::HOSTED_CAPTURE_PROTOCOL_VERSION);
            workspace_ok(value)
        })
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
    if !crate::asr::supported() {
        return workspace_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "asr_unavailable",
            false,
            "This server cannot transcribe recordings",
        );
    }
    state
        .asr_setup
        .start(
            state.workspace_service.clone(),
            state.service_principal.clone(),
            state.remote_asr_jobs.clone(),
        )
        .map(|_| workspace_ok(state.asr_setup.snapshot()))
        .unwrap_or_else(service_error)
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
    match state
        .workspace_service
        .execute_capture(&principal, token, command)
    {
        Ok(response) => {
            // The service durably admits ASR on finalize. Native producers use
            // this route, so wake the worker for jobs created after startup.
            if finalized && state.workspace_service.asr_available() {
                if let Err(error) = state.remote_asr_jobs.schedule_pending(
                    state.workspace_service.clone(),
                    state.service_principal.clone(),
                ) {
                    eprintln!(
                        "[margins-server] deferred ASR schedule after native finalize: {error:#}"
                    );
                }
            }
            workspace_ok(response)
        }
        Err(error) => service_error(error),
    }
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
        .artifact_content_file(&principal, &artifact)
    {
        Ok(file) => (
            StatusCode::OK,
            [("Content-Type", "application/octet-stream")],
            Body::from_stream(tokio_util::io::ReaderStream::new(
                tokio::fs::File::from_std(file),
            )),
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

#[derive(serde::Deserialize)]
struct WorkspaceMemoHttpUpdate {
    request_id: String,
    expected_revision: String,
    /// Omitted by browser clients so the server stamps memo and audio on its
    /// own clock. Native capture may still provide an exact device offset.
    observed_at_ms: Option<SessionMillis>,
    paused: bool,
    text: String,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn workspace_update_memo(
    State(state): State<ServerState>,
    Path((workspace, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceMemoHttpUpdate>,
) -> Response {
    let principal = match workspace_auth(&state, &headers, Some(&workspace)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session_id = SessionId(session);
    let observed_at_ms = match request.observed_at_ms {
        Some(value) => value,
        None => {
            let summary = match state.workspace_service.session(&principal, &session_id) {
                Ok(value) => value,
                Err(error) => return service_error(error),
            };
            let started = match chrono::DateTime::parse_from_rfc3339(&summary.started_at) {
                Ok(value) => value.timestamp_millis().max(0) as u64,
                Err(_) => {
                    return workspace_error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "invalid_session_time",
                        false,
                        "Meeting start time is invalid",
                    )
                }
            };
            let observed = now_unix_ms().saturating_sub(started);
            SessionMillis(if summary.input_finalized {
                summary
                    .capture_duration_ms
                    .map_or(observed, |duration| observed.min(duration.0))
            } else {
                observed
            })
        }
    };
    let update = WorkspaceMemoUpdateV1 {
        request_id: request.request_id,
        expected_revision: request.expected_revision,
        observed_at_ms,
        paused: request.paused,
        text: request.text,
    };
    state
        .workspace_service
        .update_memo(&principal, &session_id, &update)
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
    let job = match state
        .workspace_service
        .request_transcription_job(&principal, &SessionId(session))
    {
        Ok(value) => value,
        Err(error) => return service_error(error),
    };
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
