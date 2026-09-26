use crate::{recording, transcript_store, AppState, MemoLine};
use margins::core::{MemoMoment, TimedMemoDocument, TimedMemoLine};
use margins_meeting_protocol::{
    DurationMillis, LiveErrorCodeV1, LiveErrorV1, LiveHealthV1, LiveMemoLineV1,
    LiveMutationResponseV1, LiveOperationId, LiveSessionStatusV1, LiveSessionV1, LiveSnapshotV1,
    LiveTranscriptFreshnessV1, LiveTranscriptLineV1, ProtocolVersionV1, SessionId, SessionMillis,
    UnixMillis,
};
#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
use margins_meeting_protocol::{
    LiveDiscoveryV1, LiveEndpointsV1, LivePermissionsV1, LiveRuntimeV1, LiveSessionRequestV1,
    LiveStartRequestV1, LiveUpdateNotepadRequestV1, DESKTOP_LIVE_API_PREFIX_V1,
};
#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
use margins_meeting_runtime::{LiveRuntime, LiveRuntimeCommandV1, LiveRuntimeFuture};
#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_TRANSCRIPT_LINES: usize = 80;
const MAX_TRANSCRIPT_CHARS: usize = 20_000;
const MAX_NOTEPAD_CHARS: usize = 100_000;
const MAX_NOTEPAD_LINES: usize = 1_000;
const MAX_OPERATION_REPLAYS: usize = 256;
const DISCOVERY_FILENAME: &str = "desktop-live.v1.json";

#[derive(Clone)]
struct OperationRecord {
    fingerprint: Vec<u8>,
    response: LiveMutationResponseV1,
}

#[derive(Default)]
struct OperationReplays {
    records: HashMap<LiveOperationId, OperationRecord>,
    order: VecDeque<LiveOperationId>,
}

impl OperationReplays {
    fn get(
        &self,
        operation_id: &LiveOperationId,
        fingerprint: &[u8],
    ) -> Result<Option<LiveMutationResponseV1>, LiveErrorV1> {
        let Some(record) = self.records.get(operation_id) else {
            return Ok(None);
        };
        if record.fingerprint == fingerprint {
            let mut response = record.response.clone();
            response.idempotent_replay = true;
            Ok(Some(response))
        } else {
            Err(live_error(
                LiveErrorCodeV1::BadRequest,
                "This operation id was already used for a different request.",
                false,
            ))
        }
    }

    fn insert(
        &mut self,
        operation_id: LiveOperationId,
        fingerprint: Vec<u8>,
        response: LiveMutationResponseV1,
    ) {
        if self.records.contains_key(&operation_id) {
            return;
        }
        self.order.push_back(operation_id.clone());
        self.records.insert(
            operation_id,
            OperationRecord {
                fingerprint,
                response,
            },
        );
        while self.order.len() > MAX_OPERATION_REPLAYS {
            if let Some(expired) = self.order.pop_front() {
                self.records.remove(&expired);
            }
        }
    }
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
struct LiveApiHandle {
    discovery_path: PathBuf,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
static LIVE_API_HANDLE: OnceLock<Mutex<Option<LiveApiHandle>>> = OnceLock::new();

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn live_api_handle() -> &'static Mutex<Option<LiveApiHandle>> {
    LIVE_API_HANDLE.get_or_init(|| Mutex::new(None))
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
#[derive(Clone)]
struct LiveHttpState {
    runtime: Arc<dyn LiveRuntime>,
    token: String,
    operations: Arc<Mutex<OperationReplays>>,
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
struct NativeLiveRuntime {
    app_state: Arc<AppState>,
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
impl NativeLiveRuntime {
    fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state }
    }
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
impl LiveRuntime for NativeLiveRuntime {
    fn snapshot(&self, session_id: Option<&str>) -> Result<LiveSnapshotV1, LiveErrorV1> {
        snapshot_for_state(&self.app_state, session_id)
    }

    fn execute(&self, command: LiveRuntimeCommandV1) -> LiveRuntimeFuture<'_> {
        let app_state = Arc::clone(&self.app_state);
        Box::pin(async move {
            command
                .validate()
                .map_err(|error| live_error(LiveErrorCodeV1::BadRequest, error, false))?;
            match command {
                LiveRuntimeCommandV1::Start(request) => {
                    let ctx = crate::ctx::Ctx::no_emit(Arc::clone(&app_state));
                    crate::async_runtime::spawn_blocking(move || {
                        crate::start_recording_impl(&ctx, request.name, None, request.project_id)
                    })
                    .await
                    .map_err(|error| {
                        live_error(
                            LiveErrorCodeV1::Internal,
                            format!("Start failed: {error}"),
                            true,
                        )
                    })?
                    .map_err(classify_runtime_error)?;
                    mutation_response(&app_state, None)
                }
                LiveRuntimeCommandV1::Pause(request) => {
                    ensure_generation(
                        &app_state,
                        request.session_id.as_ref(),
                        request.expected_generation,
                    )?;
                    let ctx = crate::ctx::Ctx::no_emit(Arc::clone(&app_state));
                    crate::pause_recording_impl(&ctx).map_err(classify_runtime_error)?;
                    mutation_response(&app_state, None)
                }
                LiveRuntimeCommandV1::Resume(request) => {
                    ensure_generation(
                        &app_state,
                        request.session_id.as_ref(),
                        request.expected_generation,
                    )?;
                    let ctx = crate::ctx::Ctx::no_emit(Arc::clone(&app_state));
                    crate::resume_recording_impl(&ctx).map_err(classify_runtime_error)?;
                    mutation_response(&app_state, None)
                }
                LiveRuntimeCommandV1::Stop(request) => {
                    ensure_generation(
                        &app_state,
                        request.session_id.as_ref(),
                        request.expected_generation,
                    )?;
                    let ctx = crate::ctx::Ctx::no_emit(Arc::clone(&app_state));
                    let stopped = crate::stop_recording_impl(&ctx)
                        .await
                        .map_err(classify_runtime_error)?;
                    mutation_response(&app_state, Some(SessionId(stopped)))
                }
                LiveRuntimeCommandV1::UpdateNotepad(request) => {
                    update_notepad_text(
                        &app_state,
                        request.operation_id.as_ref(),
                        request.session_id.as_ref(),
                        request.expected_generation,
                        &request.expected_notepad_revision,
                        request.text,
                    )?;
                    mutation_response(&app_state, None)
                }
            }
        })
    }
}

#[cfg(feature = "tauri-app")]
pub(crate) fn start_loopback_api(state: Arc<AppState>) -> Result<(), String> {
    start_loopback_api_for_runtime(
        Arc::new(NativeLiveRuntime::new(state)),
        LiveRuntimeV1::MarginsDesktop,
    )
}

#[cfg(feature = "live-runtime")]
pub(crate) fn start_cli_loopback_api(state: Arc<AppState>) -> Result<(), String> {
    start_loopback_api_for_runtime(
        Arc::new(NativeLiveRuntime::new(state)),
        LiveRuntimeV1::MarginsCli,
    )
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn start_loopback_api_for_runtime(
    runtime: Arc<dyn LiveRuntime>,
    runtime_kind: LiveRuntimeV1,
) -> Result<(), String> {
    shutdown_loopback_api();

    let token = generate_token();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("Could not start local companion service: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure local companion service: {error}"))?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("Could not read local companion address: {error}"))?;
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let discovery_path = discovery_path();
    let discovery = discovery_document(&base_url, &token, runtime_kind);
    write_private_json(&discovery_path, &discovery)?;

    let router_state = LiveHttpState {
        runtime,
        token,
        operations: Arc::new(Mutex::new(OperationReplays::default())),
    };
    let router = build_router(router_state);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let cleanup_path = discovery_path.clone();
    crate::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("margins: local companion service could not start: {error}");
                let _ = std::fs::remove_file(cleanup_path);
                return;
            }
        };
        let server = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
        if let Err(error) = server.await {
            eprintln!("margins: local companion service stopped: {error}");
        }
        let _ = std::fs::remove_file(cleanup_path);
    });
    *live_api_handle().lock().unwrap() = Some(LiveApiHandle {
        discovery_path,
        shutdown: Some(shutdown_tx),
    });
    Ok(())
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
pub(crate) fn shutdown_loopback_api() {
    if let Some(mut handle) = live_api_handle().lock().unwrap().take() {
        if let Some(shutdown) = handle.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = std::fs::remove_file(handle.discovery_path);
    }
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn build_router(state: LiveHttpState) -> axum::Router {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/v1/live/snapshot", get(snapshot_handler))
        .route("/v1/live/start", post(start_handler))
        .route("/v1/live/pause", post(pause_handler))
        .route("/v1/live/resume", post(resume_handler))
        .route("/v1/live/stop", post(stop_handler))
        .route("/v1/live/notepad", post(update_notepad_handler))
        .with_state(state)
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn check_bearer(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    bearer_value_matches(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        expected,
    )
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime", test))]
fn bearer_value_matches(value: Option<&str>, expected: &str) -> bool {
    value.is_some_and(|value| value == format!("Bearer {expected}"))
}

#[cfg(test)]
fn canonical_route_paths() -> [&'static str; 6] {
    [
        "/v1/live/snapshot",
        "/v1/live/start",
        "/v1/live/pause",
        "/v1/live/resume",
        "/v1/live/stop",
        "/v1/live/notepad",
    ]
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn unauthorized() -> axum::response::Response {
    use axum::response::IntoResponse;

    (
        axum::http::StatusCode::UNAUTHORIZED,
        axum::Json(live_error(
            LiveErrorCodeV1::Unauthorized,
            "Unauthorized",
            false,
        )),
    )
        .into_response()
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn error_response(error: LiveErrorV1) -> axum::response::Response {
    use axum::response::IntoResponse;

    let status = match error.code {
        LiveErrorCodeV1::Unauthorized => axum::http::StatusCode::UNAUTHORIZED,
        LiveErrorCodeV1::BadRequest => axum::http::StatusCode::BAD_REQUEST,
        LiveErrorCodeV1::NoActiveSession => axum::http::StatusCode::NOT_FOUND,
        LiveErrorCodeV1::SessionMismatch
        | LiveErrorCodeV1::GenerationMismatch
        | LiveErrorCodeV1::NotepadChanged => axum::http::StatusCode::CONFLICT,
        LiveErrorCodeV1::AlreadyRecording | LiveErrorCodeV1::Busy | LiveErrorCodeV1::NotReady => {
            axum::http::StatusCode::CONFLICT
        }
        LiveErrorCodeV1::Internal => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, axum::Json(error)).into_response()
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn snapshot_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    match state
        .runtime
        .snapshot(query.get("session_id").map(String::as_str))
    {
        Ok(snapshot) => axum::Json(snapshot).into_response(),
        Err(error) => error_response(error),
    }
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn start_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<LiveStartRequestV1>,
) -> axum::response::Response {
    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    let operation_id = request.operation_id.clone();
    mutate_with_replay(
        &state,
        &operation_id,
        &request,
        LiveRuntimeCommandV1::Start(request.clone()),
    )
    .await
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn pause_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<LiveSessionRequestV1>,
) -> axum::response::Response {
    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    let operation_id = request.operation_id.clone();
    mutate_with_replay(
        &state,
        &operation_id,
        &request,
        LiveRuntimeCommandV1::Pause(request.clone()),
    )
    .await
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn resume_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<LiveSessionRequestV1>,
) -> axum::response::Response {
    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    let operation_id = request.operation_id.clone();
    mutate_with_replay(
        &state,
        &operation_id,
        &request,
        LiveRuntimeCommandV1::Resume(request.clone()),
    )
    .await
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn stop_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<LiveSessionRequestV1>,
) -> axum::response::Response {
    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    let operation_id = request.operation_id.clone();
    mutate_with_replay(
        &state,
        &operation_id,
        &request,
        LiveRuntimeCommandV1::Stop(request.clone()),
    )
    .await
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn update_notepad_handler(
    axum::extract::State(state): axum::extract::State<LiveHttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<LiveUpdateNotepadRequestV1>,
) -> axum::response::Response {
    if !check_bearer(&headers, &state.token) {
        return unauthorized();
    }
    let operation_id = request.operation_id.clone();
    mutate_with_replay(
        &state,
        &operation_id,
        &request,
        LiveRuntimeCommandV1::UpdateNotepad(request.clone()),
    )
    .await
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
async fn mutate_with_replay<T>(
    state: &LiveHttpState,
    operation_id: &LiveOperationId,
    request: &T,
    command: LiveRuntimeCommandV1,
) -> axum::response::Response
where
    T: Serialize,
{
    use axum::response::IntoResponse;

    let fingerprint = match serde_json::to_vec(request) {
        Ok(value) => value,
        Err(error) => {
            return error_response(live_error(
                LiveErrorCodeV1::BadRequest,
                format!("Bad request: {error}"),
                false,
            ));
        }
    };
    {
        let operations = state.operations.lock().unwrap();
        match operations.get(operation_id, &fingerprint) {
            Ok(Some(response)) => return axum::Json(response).into_response(),
            Ok(None) => {}
            Err(error) => return error_response(error),
        }
    }

    match state.runtime.execute(command).await {
        Ok(response) => {
            state.operations.lock().unwrap().insert(
                operation_id.clone(),
                fingerprint,
                response.clone(),
            );
            axum::Json(response).into_response()
        }
        Err(error) => error_response(error),
    }
}

fn mutation_response(
    state: &Arc<AppState>,
    stopped_session_id: Option<SessionId>,
) -> Result<LiveMutationResponseV1, LiveErrorV1> {
    Ok(LiveMutationResponseV1 {
        protocol_version: ProtocolVersionV1,
        idempotent_replay: false,
        stopped_session_id,
        snapshot: snapshot_for_state(state, None)?,
    })
}

fn snapshot_for_state(
    state: &Arc<AppState>,
    explicit_session_id: Option<&str>,
) -> Result<LiveSnapshotV1, LiveErrorV1> {
    let now = unix_ms_now();
    let active = {
        let mut guard = state.recording.lock().unwrap();
        guard.as_mut().map(|rec| {
            let status = recording::recording_status_from_state(rec);
            (
                rec.session_name.clone(),
                rec.work_dir.clone(),
                rec.live_generation,
                rec.memo_lines.clone(),
                rec.live_backchannel.as_ref().map(|live| live.client()),
                (chrono::Local::now() - rec.start_time)
                    .num_milliseconds()
                    .max(0) as u64,
                status,
            )
        })
    };

    let Some((session_name, work_dir, generation, memo_lines, live_client, elapsed_ms, status)) =
        active
    else {
        if explicit_session_id.is_some() {
            return Err(live_error(
                LiveErrorCodeV1::NoActiveSession,
                "There is no active recording.",
                true,
            ));
        }
        return Ok(idle_snapshot(now));
    };

    if explicit_session_id.is_some_and(|expected| expected != session_name) {
        return Err(live_error(
            LiveErrorCodeV1::SessionMismatch,
            "That recording is not the active recording.",
            true,
        ));
    }

    let margins_dir = work_dir.join(".margins");
    let memo_lines = margins::session::SqliteWorkspaceAuthorityStorage::open(&margins_dir)
        .and_then(|authority| authority.memo(&session_name))
        .map(|memo| memo.lines)
        .unwrap_or(memo_lines);
    let live_context = live_client.and_then(|client| client.request_context(elapsed_ms).ok());
    let (transcript, freshness) = if let Some(context) = live_context {
        (
            context.transcript,
            Some(LiveTranscriptFreshnessV1 {
                decoded_until_ms: DurationMillis(context.decoded_until_ms),
                committed_until_ms: DurationMillis(context.committed_until_ms),
                updated_at_unix_ms: UnixMillis(now),
                age_ms: DurationMillis(0),
            }),
        )
    } else {
        let assembled = transcript_store::assembled_live_transcript(&margins_dir, &session_name)
            .ok()
            .flatten();
        let watermark = status.transcript_watermark;
        (
            assembled
                .as_ref()
                .map(|snapshot| snapshot.transcript.clone())
                .unwrap_or_default(),
            watermark.map(|watermark| LiveTranscriptFreshnessV1 {
                decoded_until_ms: DurationMillis(watermark.decoded_until_ms),
                committed_until_ms: DurationMillis(watermark.committed_until_ms),
                updated_at_unix_ms: UnixMillis(watermark.updated_unix_ms),
                age_ms: DurationMillis(now.saturating_sub(watermark.updated_unix_ms)),
            }),
        )
    };

    Ok(LiveSnapshotV1 {
        protocol_version: ProtocolVersionV1,
        server_unix_ms: UnixMillis(now),
        session: Some(LiveSessionV1 {
            session_id: SessionId(session_name),
            status: session_status(&status),
            elapsed_ms: DurationMillis(elapsed_ms),
            generation,
        }),
        health: LiveHealthV1 {
            capture_phase: status.capture_phase,
            tap_status: status.tap_status,
            tap_warning: status.tap_warning,
            system_audio_expected: status.system_audio_expected,
            system_audio_observed: status.system_audio_observed,
            microphone_peak_milli: status
                .mic_level
                .map(|level| (level.clamp(0.0, 1.0) * 1_000.0) as u16),
            transcript_freshness: freshness,
        },
        rolling_transcript: bounded_transcript_lines(&transcript),
        memo_lines: live_memo_lines(&memo_lines),
        notepad_revision: notepad_revision(&memo_lines),
    })
}

fn idle_snapshot(now: u64) -> LiveSnapshotV1 {
    LiveSnapshotV1 {
        protocol_version: ProtocolVersionV1,
        server_unix_ms: UnixMillis(now),
        session: None,
        health: LiveHealthV1 {
            capture_phase: "idle".to_string(),
            tap_status: "not_expected".to_string(),
            tap_warning: None,
            system_audio_expected: false,
            system_audio_observed: false,
            microphone_peak_milli: None,
            transcript_freshness: None,
        },
        rolling_transcript: Vec::new(),
        memo_lines: Vec::new(),
        notepad_revision: notepad_revision(&[]),
    }
}

fn session_status(status: &recording::RecordingStatus) -> LiveSessionStatusV1 {
    match status.capture_phase.as_str() {
        "recording" => LiveSessionStatusV1::Recording,
        "paused" => LiveSessionStatusV1::Paused,
        "finalizing" => LiveSessionStatusV1::Finalizing,
        "needs_attention" => LiveSessionStatusV1::NeedsAttention,
        _ if status.is_recording => LiveSessionStatusV1::Recording,
        _ => LiveSessionStatusV1::Idle,
    }
}

fn ensure_generation(
    state: &Arc<AppState>,
    session_id: &str,
    expected_generation: Option<u64>,
) -> Result<(), LiveErrorV1> {
    let guard = state.recording.lock().unwrap();
    let Some(rec) = guard.as_ref() else {
        return Err(live_error(
            LiveErrorCodeV1::NoActiveSession,
            "There is no active recording.",
            true,
        ));
    };
    if rec.session_name != session_id {
        return Err(live_error(
            LiveErrorCodeV1::SessionMismatch,
            "That recording is not the active recording.",
            true,
        ));
    }
    if expected_generation.is_some_and(|expected| expected != rec.live_generation) {
        return Err(live_error(
            LiveErrorCodeV1::GenerationMismatch,
            "The recording changed; read the latest state and try again.",
            true,
        ));
    }
    Ok(())
}

fn append_memo_text(
    state: &Arc<AppState>,
    session_id: &str,
    expected_generation: Option<u64>,
    text: String,
) -> Result<(), LiveErrorV1> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(live_error(
            LiveErrorCodeV1::BadRequest,
            "Memo text cannot be empty.",
            false,
        ));
    }
    let mut guard = state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or_else(|| {
        live_error(
            LiveErrorCodeV1::NoActiveSession,
            "There is no active recording.",
            true,
        )
    })?;
    if rec.session_name != session_id {
        return Err(live_error(
            LiveErrorCodeV1::SessionMismatch,
            "That recording is not the active recording.",
            true,
        ));
    }
    if expected_generation.is_some_and(|expected| expected != rec.live_generation) {
        return Err(live_error(
            LiveErrorCodeV1::GenerationMismatch,
            "The recording changed; read the latest state and try again.",
            true,
        ));
    }
    let elapsed_secs = recording::recording_status_from_state(rec).elapsed_secs;
    let line = memo_line(text, elapsed_secs, rec.paused, rec.segment_index);
    let authority =
        margins::session::SqliteWorkspaceAuthorityStorage::open(rec.work_dir.join(".margins"))
            .map_err(|error| live_error(LiveErrorCodeV1::Internal, error.to_string(), true))?;
    let current = authority
        .memo(&rec.session_name)
        .map_err(|error| live_error(LiveErrorCodeV1::Internal, error.to_string(), true))?;
    let mut document = TimedMemoDocument::from_committed(current.lines);
    let mut text = document.plain_text();
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&line.text);
    let request_id = format!(
        "native-append-{}-{}",
        current.revision,
        elapsed_secs.to_bits()
    );
    let saved = authority
        .update_memo(
            &rec.session_name,
            "native-desktop",
            &request_id,
            &current.revision,
            (elapsed_secs * 1000.0).max(0.0) as u64,
            rec.paused,
            &text,
        )
        .map_err(|error| {
            live_error(
                LiveErrorCodeV1::Internal,
                format!("Could not save memo: {error}"),
                true,
            )
        })?;
    document = TimedMemoDocument::from_committed(saved.lines);
    rec.memo_lines = document.into_lines();
    Ok(())
}

fn update_notepad_text(
    state: &Arc<AppState>,
    operation_id: &str,
    session_id: &str,
    expected_generation: Option<u64>,
    expected_notepad_revision: &str,
    text: String,
) -> Result<(), LiveErrorV1> {
    if text.chars().count() > MAX_NOTEPAD_CHARS {
        return Err(live_error(
            LiveErrorCodeV1::BadRequest,
            "The notepad is too large.",
            false,
        ));
    }
    if text
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .count()
        > MAX_NOTEPAD_LINES
    {
        return Err(live_error(
            LiveErrorCodeV1::BadRequest,
            "The notepad has too many lines.",
            false,
        ));
    }

    let mut guard = state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or_else(|| {
        live_error(
            LiveErrorCodeV1::NoActiveSession,
            "There is no active recording.",
            true,
        )
    })?;
    if rec.session_name != session_id {
        return Err(live_error(
            LiveErrorCodeV1::SessionMismatch,
            "That recording is not the active recording.",
            true,
        ));
    }
    if expected_generation.is_some_and(|expected| expected != rec.live_generation) {
        return Err(live_error(
            LiveErrorCodeV1::GenerationMismatch,
            "The recording changed; read the latest state and try again.",
            true,
        ));
    }
    let authority =
        margins::session::SqliteWorkspaceAuthorityStorage::open(rec.work_dir.join(".margins"))
            .map_err(|error| live_error(LiveErrorCodeV1::Internal, error.to_string(), true))?;
    let current = authority
        .memo(&rec.session_name)
        .map_err(|error| live_error(LiveErrorCodeV1::Internal, error.to_string(), true))?;
    if expected_notepad_revision != current.revision {
        return Err(live_error(
            LiveErrorCodeV1::NotepadChanged,
            "The notepad changed somewhere else. Review the latest text and try again.",
            true,
        ));
    }

    let elapsed_secs = recording::recording_status_from_state(rec).elapsed_secs;
    let saved = authority
        .update_memo(
            &rec.session_name,
            "native-desktop",
            operation_id,
            &current.revision,
            (elapsed_secs * 1000.0).max(0.0) as u64,
            rec.paused,
            &text,
        )
        .map_err(|error| {
            live_error(
                LiveErrorCodeV1::Internal,
                format!("Could not save notepad: {error}"),
                true,
            )
        })?;
    rec.memo_lines = saved.lines;
    Ok(())
}

fn notepad_revision(lines: &[MemoLine]) -> String {
    TimedMemoDocument::from_committed(lines.to_vec()).revision()
}

pub(crate) fn append_active_memo_text(
    state: &Arc<AppState>,
    session_id: &str,
    text: String,
) -> Result<(), String> {
    append_memo_text(state, session_id, None, text).map_err(|error| error.message)
}

fn memo_line(text: String, elapsed_secs: f64, paused: bool, segment_index: i64) -> MemoLine {
    TimedMemoLine::at(text, memo_moment(elapsed_secs, paused, segment_index))
}

fn memo_moment(elapsed_secs: f64, paused: bool, segment_index: i64) -> MemoMoment {
    if paused {
        MemoMoment::paused(elapsed_secs, segment_index.saturating_add(1) as u32)
    } else {
        MemoMoment::recording(elapsed_secs)
    }
}

#[cfg(test)]
fn reconcile_notepad_lines(
    current: &[MemoLine],
    next_text: Vec<String>,
    elapsed_secs: f64,
    paused: bool,
    segment_index: i64,
) -> Vec<MemoLine> {
    TimedMemoDocument::from_committed(current.to_vec())
        .reconcile_plain_text(
            &next_text.join("\n"),
            memo_moment(elapsed_secs, paused, segment_index),
        )
        .into_lines()
}

fn memo_session_ms(line: &MemoLine) -> Option<SessionMillis> {
    line.block_ordinal
        .is_none()
        .then(|| SessionMillis((line.created_secs.max(0.0) * 1_000.0).round() as u64))
}

fn live_memo_lines(lines: &[MemoLine]) -> Vec<LiveMemoLineV1> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| LiveMemoLineV1 {
            index: index as u32,
            at_ms: memo_session_ms(line),
            text: line.text.clone(),
        })
        .collect()
}

fn bounded_transcript_lines(transcript: &str) -> Vec<LiveTranscriptLineV1> {
    let mut total_chars = 0usize;
    let mut out = Vec::new();
    for text in transcript
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let next_total = total_chars.saturating_add(text.chars().count());
        if out.len() >= MAX_TRANSCRIPT_LINES || next_total > MAX_TRANSCRIPT_CHARS {
            break;
        }
        total_chars = next_total;
        out.push(LiveTranscriptLineV1 {
            at_ms: parse_context_line_ms(text).map(SessionMillis),
            text: text.to_string(),
        });
    }
    out.reverse();
    out
}

fn parse_context_line_ms(line: &str) -> Option<u64> {
    let end = line.find(']')?;
    if !line.starts_with('[') || end <= 1 {
        return None;
    }
    parse_elapsed_to_ms(&line[1..end])
}

fn parse_elapsed_to_ms(value: &str) -> Option<u64> {
    let parts = value.split(':').collect::<Vec<_>>();
    let seconds = match parts.as_slice() {
        [m, s] => m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?,
        [h, m, s] => {
            h.parse::<u64>().ok()? * 3600 + m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(seconds * 1_000)
}

fn classify_runtime_error(error: String) -> LiveErrorV1 {
    let code = if error.contains("Already recording") {
        LiveErrorCodeV1::AlreadyRecording
    } else if error.contains("already finalizing")
        || error.contains("already starting")
        || error.contains("still being finalized")
    {
        LiveErrorCodeV1::Busy
    } else if error.contains("Not recording") || error.contains("no active capture") {
        LiveErrorCodeV1::NoActiveSession
    } else {
        LiveErrorCodeV1::Internal
    };
    let retryable = matches!(
        code,
        LiveErrorCodeV1::Busy | LiveErrorCodeV1::NoActiveSession | LiveErrorCodeV1::Internal
    );
    live_error(code, error, retryable)
}

fn live_error(code: LiveErrorCodeV1, message: impl ToString, retryable: bool) -> LiveErrorV1 {
    LiveErrorV1::new(code, message.to_string(), retryable)
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn discovery_path() -> PathBuf {
    crate::settings::app_data_dir().join(DISCOVERY_FILENAME)
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn discovery_document(base_url: &str, token: &str, runtime: LiveRuntimeV1) -> LiveDiscoveryV1 {
    LiveDiscoveryV1 {
        protocol_version: ProtocolVersionV1,
        runtime,
        profile: crate::settings::profile_name(),
        pid: std::process::id(),
        base_url: base_url.to_string(),
        token: token.to_string(),
        permissions: LivePermissionsV1 {
            loopback_only: true,
            private_file: true,
        },
        endpoints: LiveEndpointsV1 {
            snapshot: format!("{DESKTOP_LIVE_API_PREFIX_V1}/snapshot"),
            start: format!("{DESKTOP_LIVE_API_PREFIX_V1}/start"),
            pause: format!("{DESKTOP_LIVE_API_PREFIX_V1}/pause"),
            resume: format!("{DESKTOP_LIVE_API_PREFIX_V1}/resume"),
            stop: format!("{DESKTOP_LIVE_API_PREFIX_V1}/stop"),
            update_notepad: format!("{DESKTOP_LIVE_API_PREFIX_V1}/notepad"),
        },
        generated_at_unix_ms: UnixMillis(unix_ms_now()),
    }
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not find companion discovery directory.".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create companion discovery directory: {error}"))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{:016x}",
        DISCOVERY_FILENAME,
        std::process::id(),
        rand::random::<u64>()
    ));
    let raw = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not write companion discovery: {error}"))?;
    write_private_bytes(&tmp, &raw)?;
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not publish companion discovery: {error}")
    })?;
    repair_private_permissions(path)?;
    Ok(())
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Could not create companion discovery file: {error}"))?;
    repair_private_permissions(path)?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not write companion discovery file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not save companion discovery file: {error}"))
}

#[cfg(all(any(feature = "tauri-app", feature = "live-runtime"), unix))]
fn repair_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Could not secure companion discovery file: {error}"))
}

#[cfg(all(any(feature = "tauri-app", feature = "live-runtime"), not(unix)))]
fn repair_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(any(feature = "tauri-app", feature = "live-runtime"))]
fn generate_token() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_lines_are_bounded_to_the_latest_entries() {
        let transcript = (0..100)
            .map(|index| format!("[00:{:02}] user: line {index}", index % 60))
            .collect::<Vec<_>>()
            .join("\n");

        let lines = bounded_transcript_lines(&transcript);

        assert_eq!(lines.len(), MAX_TRANSCRIPT_LINES);
        assert_eq!(lines.first().unwrap().text, "[00:20] user: line 20");
        assert_eq!(lines.last().unwrap().text, "[00:39] user: line 99");
        assert_eq!(lines.first().unwrap().at_ms, Some(SessionMillis(20_000)));
    }

    #[test]
    fn bearer_auth_requires_exact_private_token() {
        assert!(bearer_value_matches(
            Some("Bearer secret-token"),
            "secret-token"
        ));
        assert!(!bearer_value_matches(Some("Bearer other"), "secret-token"));
        assert!(!bearer_value_matches(Some("secret-token"), "secret-token"));
        assert!(!bearer_value_matches(None, "secret-token"));
    }

    #[test]
    fn route_paths_stay_under_desktop_live_prefix() {
        assert_eq!(
            canonical_route_paths(),
            [
                "/v1/live/snapshot",
                "/v1/live/start",
                "/v1/live/pause",
                "/v1/live/resume",
                "/v1/live/stop",
                "/v1/live/notepad"
            ]
        );
        for route in canonical_route_paths() {
            assert!(route.starts_with(margins_meeting_protocol::DESKTOP_LIVE_API_PREFIX_V1));
        }
    }

    #[test]
    fn memo_lines_assign_margins_time_and_pause_blocks() {
        let active = memo_line("active".to_string(), 12.5, false, 0);
        assert_eq!(memo_session_ms(&active), Some(SessionMillis(12_500)));
        assert_eq!(active.block_ordinal, None);

        let paused = memo_line("paused".to_string(), 99.0, true, 2);
        assert_eq!(memo_session_ms(&paused), None);
        assert_eq!(paused.block_ordinal, Some(3));
    }

    #[test]
    fn live_memo_lines_include_the_complete_notepad() {
        let lines = (0..60)
            .map(|index| memo_line(format!("line {index}"), index as f64, false, 0))
            .collect::<Vec<_>>();

        let visible = live_memo_lines(&lines);

        assert_eq!(visible.len(), 60);
        assert_eq!(visible.first().unwrap().index, 0);
        assert_eq!(visible.first().unwrap().text, "line 0");
        assert_eq!(visible.last().unwrap().index, 59);
    }

    #[test]
    fn notepad_reconcile_preserves_anchors_and_times_insertions() {
        let current = vec![
            memo_line("first".into(), 2.0, false, 0),
            memo_line("keep".into(), 5.0, false, 0),
        ];

        let next = reconcile_notepad_lines(
            &current,
            vec!["first revised".into(), "new thought".into(), "keep".into()],
            12.0,
            false,
            0,
        );

        assert_eq!(next.len(), 3);
        assert_eq!(next[0].created_secs, 2.0);
        assert_eq!(next[0].edited_secs, Some(12.0));
        assert_eq!(next[1].created_secs, 12.0);
        assert_eq!(next[1].edited_secs, None);
        assert_eq!(next[2].text, current[1].text);
        assert_eq!(next[2].created_secs, current[1].created_secs);
        assert_eq!(next[2].edited_secs, current[1].edited_secs);
    }

    #[test]
    fn notepad_reconcile_join_keeps_the_surviving_line_anchor() {
        let current = vec![
            memo_line("one".into(), 3.0, false, 0),
            memo_line("two".into(), 7.0, false, 0),
        ];

        let next = reconcile_notepad_lines(&current, vec!["onetwo".into()], 20.0, false, 0);

        assert_eq!(next.len(), 1);
        assert_eq!(next[0].text, "onetwo");
        assert_eq!(next[0].created_secs, 3.0);
        assert_eq!(next[0].edited_secs, Some(20.0));
    }

    #[test]
    fn notepad_revision_covers_hidden_timestamp_state() {
        let mut line = memo_line("same text".into(), 3.0, false, 0);
        let first = notepad_revision(&[line.clone()]);
        line.edited_secs = Some(9.0);
        assert_ne!(first, notepad_revision(&[line]));
    }

    #[test]
    fn operation_replay_rejects_reused_id_for_different_body() {
        let operation_id: LiveOperationId = "op".into();
        let response = LiveMutationResponseV1 {
            protocol_version: ProtocolVersionV1,
            idempotent_replay: false,
            stopped_session_id: None,
            snapshot: idle_snapshot(1),
        };
        let mut replays = OperationReplays::default();
        replays.insert(operation_id.clone(), b"first".to_vec(), response);

        assert!(replays.get(&operation_id, b"first").unwrap().is_some());
        let error = replays.get(&operation_id, b"second").unwrap_err();
        assert_eq!(error.code, LiveErrorCodeV1::BadRequest);
    }

    #[cfg(all(feature = "tauri-app", unix))]
    #[test]
    fn discovery_file_is_private_json() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "margins-desktop-live-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let path = dir.join(DISCOVERY_FILENAME);
        let document = discovery_document(
            "http://127.0.0.1:1",
            &"a".repeat(64),
            LiveRuntimeV1::MarginsDesktop,
        );

        write_private_json(&path, &document).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let parsed: LiveDiscoveryV1 =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed.runtime, LiveRuntimeV1::MarginsDesktop);
        assert!(parsed.permissions.loopback_only);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
