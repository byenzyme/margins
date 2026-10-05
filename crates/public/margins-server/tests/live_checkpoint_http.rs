//! The native (BB/Menu) recorder publishes its provisional CoreML checkpoint
//! through the real Workspace HTTP client. Exercise that exact client against
//! a loopback margins-server, before and after the session is finalized.

use margins_server::{
    asr::{RemoteAsrJobs, SpeechSetup},
    http::build_router,
    ServerState,
};
use margins_workflows::{
    remote_workspace::{native_create_session_command, WorkspaceHttpClient},
    workspace::ensure_service_workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use std::{path::Path, sync::Arc};

const TOKEN: &str = "live-checkpoint-http-token";

struct Server {
    url: url::Url,
    service: Arc<WorkspaceService>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn serve(root: &Path) -> Server {
    let notes = root.join("notes");
    let captures = root.join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&root.join("home"), "practice", None, &notes, &captures).unwrap();
    let service = Arc::new(
        WorkspaceService::open_with_capabilities("test-instance", workspace, true, false).unwrap(),
    );
    let principal = ServicePrincipal::full("test-principal", "practice");
    let credentials = ScopedCredentialStore::open(root.join("credentials.json")).unwrap();
    credentials
        .register(
            &principal.id,
            TOKEN,
            principal.workspace_ids.iter().cloned().collect(),
            principal.operations.iter().cloned().collect(),
            None,
        )
        .unwrap();
    let app = build_router(ServerState {
        workspace_service: service.clone(),
        credential_store: credentials,
        service_principal: principal,
        asr_setup: SpeechSetup::new(false, false),
        remote_asr_jobs: RemoteAsrJobs::default(),
    });
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let join = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
    });
    Server {
        url: url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
        service,
        shutdown: Some(shutdown),
        join: Some(join),
    }
}

/// The exact shape `margins-capture` writes (`write_checkpoint`): pretty JSON
/// with a trailing newline.
fn checkpoint(terminal: bool, decoded_until_ms: u64) -> Vec<u8> {
    let value = serde_json::json!({
        "version": 2,
        "terminal": terminal,
        "start_offset_ms": 0,
        "decoded_until_ms": decoded_until_ms,
        "committed_until_ms": decoded_until_ms,
        "captured_until_ms": terminal.then_some(decoded_until_ms),
        "live_dropped_samples": 0,
        "live_recovered_frames": 0,
        "transcripts": [{ "words": [
            { "channel": 0, "start_ms": 100, "end_ms": 400, "text": " hello" },
            { "channel": 1, "start_ms": 500, "end_ms": 900, "text": " there" },
        ] }],
    });
    let mut body = serde_json::to_vec_pretty(&value).unwrap();
    body.push(b'\n');
    body
}

#[test]
fn native_live_checkpoint_round_trips_through_the_workspace_http_client() {
    use margins_meeting_protocol::{SegmentCloseReasonV1, SessionFinalizeReasonV1};
    use margins_workflows::remote_workspace::{
        deliver_transfer, DurableTransferSpool, NativeRemoteLane, NativeRemoteTransfer,
    };

    let temp = tempfile::tempdir().unwrap();
    let server = serve(temp.path());
    let client = WorkspaceHttpClient::new(server.url.clone(), TOKEN, "practice", None).unwrap();
    let capabilities = client.capabilities().unwrap();
    let session = "remote-2026-10-05-13-51-26-89f6bb09";
    let reservation = client
        .reserve(&native_create_session_command(
            session,
            "reserve-live-checkpoint",
            None,
            "margins-native-cli",
        ))
        .unwrap();
    let token = reservation.producer_token.clone();

    client
        .put_live_checkpoint(session, &token, checkpoint(false, 3_000))
        .expect("an active producer may publish a provisional checkpoint");
    client
        .put_live_checkpoint(session, &token, checkpoint(true, 4_000))
        .expect("an active producer may publish its terminal checkpoint");
    let stored = server
        .service
        .margins_dir()
        .join(format!("{session}_remote.live-transcript.json"));
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&stored).unwrap()).unwrap();
    assert_eq!(stored["terminal"], true);
    assert_eq!(stored["decoded_until_ms"], 4_000);

    // Seal and deliver exactly as the native recorder's Stop does.
    let spool = DurableTransferSpool::create(
        &temp.path().join("transfers"),
        "live-checkpoint-transfer",
        capabilities.instance_id.as_ref(),
        server.url.as_str(),
        "practice",
        session,
        &token,
        capabilities.limits.spool_reserve_bytes,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("native-seg".into(), 0).unwrap();
    for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
        transfer
            .append_f32(lane, 16_000, &vec![0.0; 64_000])
            .unwrap();
    }
    transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let ended = transfer.last_closed_ended_at_ms().unwrap();
    transfer
        .seal_session(ended, SessionFinalizeReasonV1::Completed)
        .unwrap();
    let mut spool = transfer.into_spool();
    deliver_transfer(&mut spool, &client).unwrap();

    // After finalize the server refuses provisional words; the native
    // recorder therefore never publishes after Stop.
    let error = client
        .put_live_checkpoint(session, &token, checkpoint(true, 4_000))
        .expect_err("finalize retires the producer");
    assert!(
        format!("{error:#}").contains("capture producer is no longer active"),
        "{error:#}"
    );
}

/// The v0.4.15 failure: CoreML's terminal flush committed a tail word whose
/// token duration ran past the decoded audio. The service rejects the whole
/// checkpoint, which the helper logged only as `category=application`.
#[test]
fn tail_word_past_the_decoded_watermark_is_rejected_and_a_clamped_one_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let server = serve(temp.path());
    let client = WorkspaceHttpClient::new(server.url.clone(), TOKEN, "practice", None).unwrap();
    let session = "remote-tail-word";
    let token = client
        .reserve(&native_create_session_command(
            session,
            "reserve-tail-word",
            None,
            "margins-native-cli",
        ))
        .unwrap()
        .producer_token;
    let with_tail = |end_ms: u64| {
        let mut body = serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "terminal": true,
            "start_offset_ms": 0,
            "decoded_until_ms": 9_300,
            "committed_until_ms": 9_300,
            "captured_until_ms": 9_300,
            "live_dropped_samples": 0,
            "live_recovered_frames": 0,
            "transcripts": [{ "words": [
                { "channel": 0, "start_ms": 9_100, "end_ms": end_ms, "text": " done" },
            ] }],
        }))
        .unwrap();
        body.push(b'\n');
        body
    };

    let error = client
        .put_live_checkpoint(session, &token, with_tail(9_420))
        .expect_err("a word ending past decoded_until_ms is invalid");
    let message = format!("{error:#}");
    assert!(message.starts_with("invalid_request: "), "{message}");
    assert!(message.contains("word timeline"), "{message}");

    client
        .put_live_checkpoint(session, &token, with_tail(9_300))
        .expect("the clamped checkpoint margins-capture now writes is accepted");
}
