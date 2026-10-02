use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptWord};
use margins_server::{
    asr::{RemoteAsrJobs, SpeechSetup},
    browser::{sweep_expired_browser_captures, BROWSER_OWNER_LEASE_MS},
    http::build_router,
    ServerState,
};
use margins_store::canonical;
use margins_workflows::{
    workspace::ensure_service_workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

const OWNER: &str = "a41a08f5-1583-4eec-98c7-95da0394b416";
const TOKEN: &str = "browser-http-test-token-0123456789abcdef";
const START: u64 = 1_800_000_000_000;

struct TimingAsr;

impl AsrBackend for TimingAsr {
    fn transcribe(&self, request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        Ok(AsrResult {
            words: vec![TranscriptWord {
                start_ms: request.session_offset_ms + 1_000,
                end_ms: request.session_offset_ms + 1_100,
                text: "spoken evidence".into(),
                speaker: None,
                confidence_per_mille: None,
            }],
            detected_language: None,
        })
    }
}

fn server_state(root: &Path) -> ServerState {
    let notes = root.join("notes");
    let captures = root.join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&root.join("home"), "practice", None, &notes, &captures).unwrap();
    let service = Arc::new(WorkspaceService::open("test-instance", workspace).unwrap());
    // A supported server can accept finalized audio while its model installs.
    service.enable_deferred_asr();
    let credential_store = ScopedCredentialStore::open(root.join("credentials.json")).unwrap();
    let principal = ServicePrincipal::full("test-principal", "practice");
    credential_store
        .register(
            &principal.id,
            TOKEN,
            principal.workspace_ids.iter().cloned().collect(),
            principal.operations.iter().cloned().collect(),
            None,
        )
        .unwrap();
    ServerState {
        workspace_service: service,
        credential_store,
        service_principal: principal,
        asr_setup: SpeechSetup::new(false, false),
        remote_asr_jobs: RemoteAsrJobs::default(),
    }
}

fn router(root: &Path) -> Router {
    build_router(server_state(root))
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    body: Vec<u8>,
    owner: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method.clone())
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("x-margins-instance-id", "test-instance");
    if !body.is_empty() && body.first() == Some(&b'{') {
        request = request.header("content-type", "application/json");
    }
    if owner {
        request = request.header("x-margins-capture-owner", OWNER);
    }
    if method == Method::PUT && path.contains("/chunks/") {
        let sequence: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
        request = request
            .header(
                "x-margins-captured-start-unix-ms",
                (START + sequence * 1_000).to_string(),
            )
            .header(
                "x-margins-captured-end-unix-ms",
                (START + sequence * 1_000 + 1_000).to_string(),
            );
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "non-JSON {status} for {path}: {error}; {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, value)
}

async fn post(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    call(
        app,
        Method::POST,
        path,
        serde_json::to_vec(&body).unwrap(),
        false,
    )
    .await
}

async fn upload_at(
    app: &Router,
    path: &str,
    payload: Vec<u8>,
    start_unix_ms: u64,
    end_unix_ms: u64,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("x-margins-instance-id", "test-instance")
        .header("x-margins-capture-owner", OWNER)
        .header(
            "x-margins-captured-start-unix-ms",
            start_unix_ms.to_string(),
        )
        .header("x-margins-captured-end-unix-ms", end_unix_ms.to_string())
        .body(Body::from(payload))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn stop_rejects_gap_until_missing_webm_chunk_is_durable_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (status, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Runtime HTTP test", "startedAtUnixMs": START}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let session = started["result"]["sessionId"].as_str().unwrap();
    assert_eq!(started["result"]["status"], "recording");
    let chunk_path = |sequence| format!("{base}/{session}/chunks/{sequence}");
    let stop_path = format!("{base}/{session}/stop");
    for sequence in [0, 2] {
        let (_, acknowledged) = call(
            &app,
            Method::PUT,
            &chunk_path(sequence),
            vec![b'A' + sequence as u8],
            true,
        )
        .await;
        assert_eq!(acknowledged["result"]["durable"], true, "{acknowledged}");
    }
    let (status, gap) = post(
        &app,
        &stop_path,
        json!({"ownerId": OWNER, "expectedNextSequence": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(gap["error"]["code"], "browser_chunk_gap");
    assert_eq!(
        gap["error"]["message"],
        "Missing browser audio sequence 1; retry the chunk before Stop."
    );

    // Reopen the actual SQLite runtime and its producer authority, not an
    // in-memory test double. The out-of-order acknowledgement must survive.
    drop(app);
    let restarted = router(temp.path());
    let (_, acknowledged) = call(&restarted, Method::PUT, &chunk_path(1), vec![b'B'], true).await;
    assert_eq!(acknowledged["result"]["durable"], true, "{acknowledged}");
    let (status, finished) = post(
        &restarted,
        &stop_path,
        json!({"ownerId": OWNER, "expectedNextSequence": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["inputFinalized"], true);
    // Stop must leave the admitted job queued until speech setup publishes
    // model readiness. An eager worker would fail the still-unavailable model.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let (_, job) = call(
        &restarted,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/jobs/latest"),
        vec![],
        false,
    )
    .await;
    assert_eq!(job["result"]["status"], "queued", "{job}");
    // A lost Stop response retries cleanly after the producer reservation is released.
    let (status, replayed) = post(
        &restarted,
        &stop_path,
        json!({"ownerId": OWNER, "expectedNextSequence": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["result"]["inputFinalized"], true);
}

#[tokio::test]
async fn paused_capture_resumes_from_durable_runtime_state_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Pause resume HTTP test", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let first = format!("{base}/{session}/chunks/0");
    let (_, acknowledged) = call(&app, Method::PUT, &first, vec![b'A'], true).await;
    assert_eq!(acknowledged["result"]["durable"], true);
    let pause_path = format!("{base}/{session}/pause");
    let (status, paused) = post(
        &app,
        &pause_path,
        json!({"ownerId": OWNER, "expectedNextSequence": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["result"]["status"], "paused");
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["input_finalized"], false, "{summary}");
    drop(app);

    let restarted = router(temp.path());
    let (status, snapshot) = call(
        &restarted,
        Method::GET,
        &format!("{base}/{session}/snapshot?ownerId={OWNER}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    assert_eq!(snapshot["result"]["status"], "paused");
    let (_, summary) = call(
        &restarted,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["input_finalized"], false, "{summary}");
    let (status, resumed) = post(
        &restarted,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["result"]["status"], "recording");
    let (_, second) = call(
        &restarted,
        Method::PUT,
        &format!("{base}/{session}/chunks/1"),
        vec![b'B'],
        true,
    )
    .await;
    assert_eq!(second["result"]["durable"], true, "{second}");
    let (status, finished) = post(
        &restarted,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["inputFinalized"], true);
}

#[tokio::test]
async fn user_can_finish_with_saved_audio_and_explicit_missing_ranges() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Incomplete HTTP test", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    for sequence in [0, 2] {
        let (status, receipt) = upload_at(
            &app,
            &format!("{base}/{session}/chunks/{sequence}"),
            vec![b'A' + sequence as u8],
            START + sequence * 1_000,
            START + sequence * 1_000 + 1_000,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
    }
    let (status, gap) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 3, "segmentEndedUnixMs": START + 3_000}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{gap}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/finish-incomplete"),
        json!({"ownerId": OWNER, "expectedNextSequence": 3, "segmentEndedUnixMs": START + 3_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["incomplete"], true);
    assert_eq!(finished["result"]["missingRanges"][0]["start"], 1);
    assert_eq!(finished["result"]["missingRanges"][0]["end_exclusive"], 2);
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["input_finalized"], true, "{summary}");
    assert_eq!(summary["result"]["capture_incomplete"], true, "{summary}");
    assert_eq!(summary["result"]["capture_gaps"][0]["start_sequence"], 1);
    assert_eq!(summary["result"]["capture_gaps"][0]["starts_at_ms"], 1_000);
    let (_, job) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/jobs/latest"),
        vec![],
        false,
    )
    .await;
    assert_eq!(job["result"]["status"], "queued", "{job}");
}

#[tokio::test]
async fn empty_capture_requires_explicit_incomplete_finish() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Empty capture", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let (status, blocked) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 0}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["error"]["retryable"], false, "{blocked}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/finish-incomplete"),
        json!({"ownerId": OWNER, "expectedNextSequence": 0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["incomplete"], true);
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["capture_incomplete"], true, "{summary}");
}

#[tokio::test]
async fn expired_owner_lease_finalizes_out_of_order_audio_as_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let state = server_state(temp.path());
    let app = build_router(state.clone());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Expired owner HTTP test", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    for sequence in [0, 2] {
        let (status, receipt) = upload_at(
            &app,
            &format!("{base}/{session}/chunks/{sequence}"),
            vec![b'A' + sequence as u8],
            START + sequence * 1_000,
            START + sequence * 1_000 + 1_000,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
    }
    let observed_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + BROWSER_OWNER_LEASE_MS
        + 2_000;
    assert_eq!(
        sweep_expired_browser_captures(&state, observed_unix_ms).unwrap(),
        1
    );
    let (status, resumed) = post(
        &app,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER, "segmentStartedUnixMs": START + 4_000}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{resumed}");
    assert_eq!(resumed["error"]["code"], "browser_lease_expired");
    assert_eq!(resumed["error"]["retryable"], false);
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["capture_incomplete"], true, "{summary}");
    assert_eq!(summary["result"]["capture_gaps"][0]["start_sequence"], 1);
    let (_, active) = call(
        &app,
        Method::GET,
        "/v1/workspaces/practice/active-sessions",
        vec![],
        false,
    )
    .await;
    assert!(!active.to_string().contains(session), "{active}");
    drop(app);
    let restarted = router(temp.path());
    let (_, summary) = call(
        &restarted,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["capture_incomplete"], true, "{summary}");
}

#[tokio::test]
async fn reload_rotates_webm_segment_and_uses_server_durable_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Reload HTTP test", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let (status, _) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/0"),
        b"first-webm-stream".to_vec(),
        START,
        START + 1_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The page lost the acknowledgement, then reloaded. Server projection is
    // authoritative and avoids reusing sequence zero for a different Blob.
    let (_, snapshot) = call(
        &app,
        Method::GET,
        &format!("{base}/{session}/snapshot?ownerId={OWNER}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(snapshot["result"]["nextSequence"], 1, "{snapshot}");
    let (status, pause) = post(
        &app,
        &format!("{base}/{session}/pause"),
        json!({"ownerId": OWNER, "expectedNextSequence": 1, "segmentEndedUnixMs": START + 1_000, "recoveredAfterReload": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pause}");
    let (status, resume) = post(
        &app,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER, "segmentStartedUnixMs": START + 10_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resume}");
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/1"),
        b"second-webm-stream".to_vec(),
        START + 10_000,
        START + 11_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 2, "segmentEndedUnixMs": START + 11_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["incomplete"], true, "{finished}");
    let (_, artifacts) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/artifacts"),
        vec![],
        false,
    )
    .await;
    let audio = artifacts["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|artifact| artifact["kind"] == "audio_mic_webm")
        .collect::<Vec<_>>();
    assert_eq!(audio.len(), 2, "{artifacts}");
    assert_ne!(audio[0]["artifact_id"], audio[1]["artifact_id"]);
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    assert_eq!(summary["result"]["capture_incomplete"], true, "{summary}");
    assert_eq!(summary["result"]["capture_gaps"][0]["start_sequence"], 1);
    assert_eq!(summary["result"]["capture_gaps"][0]["end_exclusive"], 1);
}

#[tokio::test]
async fn reload_during_upload_marks_gap_and_keeps_recording() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = server_state(temp.path());
    state.workspace_service.set_asr_available(true);
    state.asr_setup = SpeechSetup::new(true, true);
    state.remote_asr_jobs = RemoteAsrJobs::with_backend(Arc::new(TimingAsr));
    let service = state.workspace_service.clone();
    let principal = state.service_principal.clone();
    let app = build_router(state);
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Reload during upload", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let webm = include_bytes!("fixtures/legacy-browser.webm").to_vec();
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/0"),
        webm.clone(),
        START,
        START + 1_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    // Sequence 1 was assigned by the old page but never reached the server.
    let (status, paused) = post(
        &app,
        &format!("{base}/{session}/pause"),
        json!({"ownerId": OWNER, "expectedNextSequence": 2,
            "segmentEndedUnixMs": START + 5_000, "recoveredAfterReload": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["result"]["status"], "paused");
    assert_eq!(paused["result"]["nextSequence"], 2);
    let (status, resumed) = post(
        &app,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER, "segmentStartedUnixMs": START + 10_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/2"),
        webm,
        START + 10_000,
        START + 11_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 3,
            "segmentEndedUnixMs": START + 11_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["incomplete"], true);
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    let gaps = summary["result"]["capture_gaps"].as_array().unwrap();
    assert!(
        gaps.iter().any(|gap| gap["start_sequence"] == 1
            && gap["end_exclusive"] == 2
            && gap["starts_at_ms"] == 1_000),
        "{summary}"
    );
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &session.into())
            .unwrap()
            .unwrap();
        if job.status == "complete" {
            break;
        }
        if job.status == "failed" {
            panic!("ASR job failed: {:?}", job.failure);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (_, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/transcript"),
        vec![],
        false,
    )
    .await;
    let body = transcript["result"]["body"].as_str().unwrap();
    assert!(
        body.contains("at 00:01: browser-000000 chunks 1..2"),
        "{body}"
    );
    assert!(
        body.contains("[00:11] you (mic): spoken evidence"),
        "{body}"
    );
}

#[tokio::test]
async fn pause_time_remains_between_memo_and_later_transcript_words() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = server_state(temp.path());
    state.workspace_service.set_asr_available(true);
    state.asr_setup = SpeechSetup::new(true, true);
    state.remote_asr_jobs = RemoteAsrJobs::with_backend(Arc::new(TimingAsr));
    let service = state.workspace_service.clone();
    let principal = state.service_principal.clone();
    let app = build_router(state);
    let base = "/v1/workspaces/practice/browser/sessions";
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Timed capture", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let webm = include_bytes!("fixtures/legacy-browser.webm").to_vec();
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/0"),
        webm.clone(),
        START,
        START + 1_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, paused) = post(
        &app,
        &format!("{base}/{session}/pause"),
        json!({"ownerId": OWNER, "expectedNextSequence": 1, "segmentEndedUnixMs": START + 1_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    let (_, memo) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/memo"),
        vec![],
        false,
    )
    .await;
    let (status, memo) = call(
        &app,
        Method::PUT,
        &format!("/v1/workspaces/practice/sessions/{session}/memo"),
        serde_json::to_vec(&json!({
            "request_id": "timing-memo",
            "expected_revision": memo["result"]["revision"],
            "observed_at_ms": 7_000,
            "paused": true,
            "text": "Decision during pause"
        }))
        .unwrap(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{memo}");
    assert_eq!(memo["result"]["lines"][0]["created_secs"], 7.0);
    let (status, resumed) = post(
        &app,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER, "segmentStartedUnixMs": START + 10_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/1"),
        webm,
        START + 10_000,
        START + 11_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 2, "segmentEndedUnixMs": START + 11_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    let segments = canonical::get_session_meta(service.margins_dir(), session)
        .unwrap()
        .segments;
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].offset_ms, 0);
    assert_eq!(segments[1].offset_ms, 10_000);

    let (status, requested) = post(
        &app,
        &format!("/v1/workspaces/practice/sessions/{session}/jobs/transcribe"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{requested}");
    for _ in 0..100 {
        let job = service
            .latest_job(&principal, &session.into())
            .unwrap()
            .unwrap();
        match job.status.as_str() {
            "complete" => break,
            "failed" => panic!("ASR job failed: {:?}", job.failure),
            _ => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    let job = service
        .latest_job(&principal, &session.into())
        .unwrap()
        .unwrap();
    assert_eq!(job.status, "complete", "job did not finish: {job:?}");
    let (_, transcript) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/transcript"),
        vec![],
        false,
    )
    .await;
    let body = transcript["result"]["body"].as_str().unwrap();
    assert!(
        body.contains("[00:01] you (mic): spoken evidence"),
        "{body}"
    );
    assert!(
        body.contains("[00:11] you (mic): spoken evidence"),
        "{body}"
    );
}

#[tokio::test]
async fn server_clock_stamps_memo_and_bounds_backwards_or_future_browser_time() {
    let temp = tempfile::tempdir().unwrap();
    let state = server_state(temp.path());
    let service = state.workspace_service.clone();
    let app = build_router(state);
    let base = "/v1/workspaces/practice/browser/sessions";
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let (_, started) = post(
        &app,
        base,
        json!({"ownerId": OWNER, "name": "Remote clock", "startedAtUnixMs": START}),
    )
    .await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let (_, summary) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}"),
        vec![],
        false,
    )
    .await;
    let server_start =
        chrono::DateTime::parse_from_rfc3339(summary["result"]["started_at"].as_str().unwrap())
            .unwrap()
            .timestamp_millis();
    assert!(
        server_start >= before - 1_000 && server_start <= before + 5_000,
        "{summary}"
    );
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/0"),
        vec![b'A'],
        START,
        START + 1,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (_, memo) = call(
        &app,
        Method::GET,
        &format!("/v1/workspaces/practice/sessions/{session}/memo"),
        vec![],
        false,
    )
    .await;
    let (status, saved_memo) = call(
        &app,
        Method::PUT,
        &format!("/v1/workspaces/practice/sessions/{session}/memo"),
        serde_json::to_vec(&json!({"request_id": "server-clock-memo",
            "expected_revision": memo["result"]["revision"], "paused": false,
            "text": "Observed on project host"}))
        .unwrap(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved_memo}");
    assert!(
        saved_memo["result"]["lines"][0]["created_secs"]
            .as_f64()
            .unwrap()
            < 5.0
    );
    let (status, paused) = post(
        &app,
        &format!("{base}/{session}/pause"),
        json!({"ownerId": OWNER, "expectedNextSequence": 1,
            "segmentEndedUnixMs": START + 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    // The wall clock stepped back an hour during Pause. Resume and the new
    // WebM stream must remain valid on the server's monotonic capture lane.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let stepped_back = START - 3_600_000;
    let (status, resumed) = post(
        &app,
        &format!("{base}/{session}/resume"),
        json!({"ownerId": OWNER, "segmentStartedUnixMs": stepped_back}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/1"),
        vec![b'B'],
        stepped_back,
        stepped_back + 1_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    // A later forward jump is capped at server receive time plus tolerance.
    let jumped_forward = START + 24 * 60 * 60 * 1_000;
    let (status, receipt) = upload_at(
        &app,
        &format!("{base}/{session}/chunks/2"),
        vec![b'C'],
        jumped_forward,
        jumped_forward + 1_000,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, finished) = post(
        &app,
        &format!("{base}/{session}/stop"),
        json!({"ownerId": OWNER, "expectedNextSequence": 3,
            "segmentEndedUnixMs": jumped_forward + 1_000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    let segments = canonical::get_session_meta(service.margins_dir(), session)
        .unwrap()
        .segments;
    assert_eq!(segments.len(), 2);
    assert!(
        segments[1].offset_ms >= 20 && segments[1].offset_ms <= 31_000,
        "unexpected second segment offset: {}",
        segments[1].offset_ms
    );
}
