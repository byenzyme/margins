use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use margins_server::{asr::{RemoteAsrJobs, SpeechSetup}, http::build_router, ServerState};
use margins_workflows::{
    workspace::ensure_service_workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

const OWNER: &str = "a41a08f5-1583-4eec-98c7-95da0394b416";
const TOKEN: &str = "browser-http-test-token-0123456789abcdef";

fn router(root: &Path) -> Router {
    let notes = root.join("notes");
    let captures = root.join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&root.join("home"), "practice", None, &notes, &captures).unwrap();
    let service = Arc::new(WorkspaceService::open("test-instance", workspace).unwrap());
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
    build_router(ServerState {
        workspace_service: service,
        credential_store,
        service_principal: principal,
        asr_setup: SpeechSetup::new(false, false),
        remote_asr_jobs: RemoteAsrJobs::default(),
    })
}

async fn call(app: &Router, method: Method, path: &str, body: Vec<u8>, owner: bool) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("x-margins-instance-id", "test-instance");
    if !body.is_empty() && body.first() == Some(&b'{') {
        request = request.header("content-type", "application/json");
    }
    if owner {
        request = request.header("x-margins-capture-owner", OWNER);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024).await.unwrap();
    let value = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!("non-JSON {status} for {path}: {error}; {}", String::from_utf8_lossy(&body))
    });
    (status, value)
}

async fn post(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    call(app, Method::POST, path, serde_json::to_vec(&body).unwrap(), false).await
}

#[tokio::test]
async fn stop_rejects_gap_until_missing_webm_chunk_is_durable_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(temp.path());
    let base = "/v1/workspaces/practice/browser/sessions";
    let (status, started) = post(&app, base, json!({"ownerId": OWNER, "name": "Runtime HTTP test"})).await;
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
    let (status, gap) = post(&app, &stop_path, json!({"ownerId": OWNER, "expectedNextSequence": 3})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(gap["error"]["code"], "browser_chunk_gap");
    assert_eq!(gap["error"]["message"], "Missing browser audio sequence 1; retry the chunk before Stop.");

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
    let (_, started) = post(&app, base, json!({"ownerId": OWNER, "name": "Pause resume HTTP test"})).await;
    let session = started["result"]["sessionId"].as_str().unwrap();
    let first = format!("{base}/{session}/chunks/0");
    let (_, acknowledged) = call(&app, Method::PUT, &first, vec![b'A'], true).await;
    assert_eq!(acknowledged["result"]["durable"], true);
    let pause_path = format!("{base}/{session}/pause");
    let (status, paused) = post(&app, &pause_path, json!({"ownerId": OWNER, "expectedNextSequence": 1})).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["result"]["status"], "paused");
    drop(app);

    let restarted = router(temp.path());
    let (status, snapshot) = call(
        &restarted,
        Method::GET,
        &format!("{base}/{session}/snapshot?ownerId={OWNER}"),
        vec![],
        false,
    ).await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    assert_eq!(snapshot["result"]["status"], "paused");
    let (status, resumed) = post(&restarted, &format!("{base}/{session}/resume"), json!({"ownerId": OWNER})).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["result"]["status"], "recording");
    let (_, second) = call(&restarted, Method::PUT, &format!("{base}/{session}/chunks/1"), vec![b'B'], true).await;
    assert_eq!(second["result"]["durable"], true, "{second}");
    let (status, finished) = post(&restarted, &format!("{base}/{session}/stop"), json!({"ownerId": OWNER, "expectedNextSequence": 2})).await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["result"]["inputFinalized"], true);
}
