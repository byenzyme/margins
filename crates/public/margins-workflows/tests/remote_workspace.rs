use margins_meeting_protocol::*;
use margins_workflows::remote_workspace::*;
use margins_workflows::workspace_service::DEFAULT_MAX_IN_FLIGHT_CHUNKS;
use ropus::{Application, Bitrate, Channels, Encoder};
use sha2::{Digest as _, Sha256};

fn chunk(sequence: u64, byte: u8) -> ClientMessageV1 {
    let payload = vec![byte; 960];
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: format!("chunk-{sequence}").into(),
        session_id: "session-a".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000),
        body: ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "segment-a".into(),
            lane_id: "mic".into(),
            sequence,
            starts_at_ms: SessionMillis(sequence * 10),
            duration_ms: DurationMillis(10),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: format!("{:x}", Sha256::digest(&payload)),
            },
            payload,
        }),
    }
}

#[test]
fn transport_selection_rejects_unsafe_or_plain_remote_targets() {
    assert!(matches!(
        RemoteEndpoint::parse("ssh://beelink").unwrap(),
        RemoteEndpoint::SshAlias(value) if value == "beelink"
    ));
    assert!(matches!(
        RemoteEndpoint::parse("https://margins.example.test").unwrap(),
        RemoteEndpoint::Https(_)
    ));
    assert!(RemoteEndpoint::parse("http://margins.example.test").is_err());
    assert!(RemoteEndpoint::parse("ssh://user@host").is_err());
    assert!(RemoteEndpoint::parse("ssh://host/;touch-x").is_err());
    assert!(validate_ssh_alias("-oProxyCommand=bad").is_err());
    let tunnel = ssh_tunnel_args("configured-host", 38111, 8787).unwrap();
    assert_eq!(tunnel.last().unwrap(), "configured-host");
    assert!(tunnel.contains(&"ExitOnForwardFailure=yes".to_string()));
    assert!(tunnel.contains(&"127.0.0.1:38111:127.0.0.1:8787".to_string()));
}

#[test]
fn http_client_preserves_present_null_for_optional_route_results() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        for expected_path in [
            "/v1/workspaces/workspace-a/sessions/session-a/note-association",
            "/v1/workspaces/workspace-a/sessions/session-a/jobs/latest",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(
                request.starts_with(&format!("GET {expected_path} HTTP/1.1")),
                "unexpected request: {request}"
            );
            let body = br#"{"ok":true,"result":null}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        }
    });
    let client = WorkspaceHttpClient::new(
        url::Url::parse(&format!("http://{address}/")).unwrap(),
        "scoped-token",
        "workspace-a",
        None,
    )
    .unwrap();

    assert!(client.note_association("session-a").unwrap().is_none());
    assert!(client.latest_job("session-a").unwrap().is_none());
    server.join().unwrap();
}

#[test]
fn paced_two_lane_delivery_batches_requests_without_serializing_the_producer() {
    use std::io::{Read as _, Write as _};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    fn read_request(stream: &mut std::net::TcpStream) -> (String, String, Vec<u8>) {
        stream
            // Disposable debug builds can pause this fixture while macOS flushes
            // the durable spool. This timeout detects a genuinely stuck client;
            // it is not a product request-latency budget.
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut part = [0_u8; 8192];
            let read = stream.read(&mut part).unwrap();
            assert_ne!(read, 0, "HTTP request ended before its headers");
            bytes.extend_from_slice(&part[..read]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or_default();
        while bytes.len() < header_end + content_length {
            let mut part = [0_u8; 8192];
            let read = stream.read(&mut part).unwrap();
            assert_ne!(read, 0, "HTTP request ended before its body");
            bytes.extend_from_slice(&part[..read]);
        }
        let request_line = headers.lines().next().unwrap().to_string();
        (
            request_line,
            headers,
            bytes[header_end..header_end + content_length].to_vec(),
        )
    }

    fn respond(stream: &mut std::net::TcpStream, result: serde_json::Value) {
        let body = serde_json::to_vec(&serde_json::json!({"ok":true,"result":result})).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let server_stopped = stopped.clone();
    let capability_requests = Arc::new(AtomicUsize::new(0));
    let server_capabilities = capability_requests.clone();
    let batch_requests = Arc::new(AtomicUsize::new(0));
    let server_batches = batch_requests.clone();
    let server = std::thread::spawn(move || {
        while !server_stopped.load(Ordering::Acquire) {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("paced HTTP fixture failed: {error}"),
            };
            let (line, headers, body) = read_request(&mut stream);
            if line.starts_with("GET /v1/capabilities ") {
                server_capabilities.fetch_add(1, Ordering::AcqRel);
                respond(
                    &mut stream,
                    serde_json::to_value(WorkspaceCapabilitiesV1 {
                        protocol_version: ProtocolVersionV1,
                        instance_id: "instance-paced".into(),
                        workspace_id: "workspace-a".into(),
                        limits: WorkspaceLimitsV1 {
                            max_chunk_bytes: 1_048_576,
                            max_in_flight_chunks: DEFAULT_MAX_IN_FLIGHT_CHUNKS,
                            max_event_page: 256,
                            max_import_bytes: 1_048_576,
                            spool_reserve_bytes: 0,
                        },
                        capture_formats: Vec::new(),
                        operations: Vec::new(),
                        asr_available: false,
                        recall_available: false,
                    })
                    .unwrap(),
                );
            } else if line.contains("/audio-chunks ") {
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("x-margins-instance-id: instance-paced"),
                    "capture write omitted its instance fence"
                );
                server_batches.fetch_add(1, Ordering::AcqRel);
                let batch = AudioChunkBatchV1::decode(
                    &body,
                    DEFAULT_MAX_IN_FLIGHT_CHUNKS as usize,
                    1_048_576,
                )
                .unwrap();
                // Model the measured pre-Opus SSH boundary (~158.5 ms/request,
                // 6.31 requests/s) rather than an unrealistically fast LAN.
                std::thread::sleep(Duration::from_millis(160));
                respond(
                    &mut stream,
                    serde_json::json!(
                        batch
                            .commands
                            .iter()
                            .map(|_| serde_json::json!({"messages":[],"idempotent_replay":false}))
                            .collect::<Vec<_>>()
                    ),
                );
            } else if line.contains("/commands ") {
                respond(
                    &mut stream,
                    serde_json::json!({"messages":[],"idempotent_replay":false}),
                );
            } else {
                panic!("unexpected paced fixture request: {line}");
            }
        }
    });

    let client = WorkspaceHttpClient::new(
        url::Url::parse(&format!("http://{address}/")).unwrap(),
        "scoped-token",
        "workspace-a",
        Some("instance-paced".into()),
    )
    .unwrap();
    client.capabilities().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "paced-transfer",
        "instance-paced",
        &format!("http://{address}/"),
        "workspace-a",
        "session-paced",
        "producer-secret",
        0,
    )
    .unwrap();
    let parent = spool.root().parent().unwrap().to_path_buf();
    let uploader_done = Arc::new(AtomicBool::new(false));
    let uploader_done_flag = uploader_done.clone();
    let uploader_client = client.clone();
    let uploader = std::thread::spawn(move || {
        while !uploader_done_flag.load(Ordering::Acquire) {
            let mut spool = DurableTransferSpool::open(&parent, "paced-transfer", 0).unwrap();
            deliver_available(&mut spool, &uploader_client).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("paced-segment".into(), 0).unwrap();
    let capture_started = Instant::now();
    let frame = vec![0_u8; OPUS_PACKET_FRAME_SAMPLES_V1 as usize * 2];
    let mut max_pending_chunks = 0;
    let mut max_pending_bytes = 0_u64;
    let mut max_pending_age = Duration::ZERO;
    for index in 0..100_u64 {
        transfer
            .append_s16le(NativeRemoteLane::Microphone, 16_000, &frame)
            .unwrap();
        transfer
            .append_s16le(NativeRemoteLane::System, 16_000, &frame)
            .unwrap();
        let pending = transfer.spool().pending_chunks().unwrap();
        max_pending_chunks = max_pending_chunks.max(pending.len());
        max_pending_bytes =
            max_pending_bytes.max(pending.iter().map(|chunk| chunk.size_bytes).sum::<u64>());
        max_pending_age = max_pending_age.max(
            pending
                .iter()
                .filter_map(|chunk| chunk.path.metadata().ok()?.modified().ok()?.elapsed().ok())
                .max()
                .unwrap_or_default(),
        );
        let deadline = capture_started + Duration::from_millis((index + 1) * 20);
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        }
    }
    let capture_elapsed = capture_started.elapsed();
    let stop_started = Instant::now();
    transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let pending_at_stop = transfer.spool().pending_chunks().unwrap().len();
    uploader_done.store(true, Ordering::Release);
    uploader.join().unwrap();
    let mut spool = transfer.into_spool();
    deliver_chunks(&mut spool, &client).unwrap();
    let stop_drain = stop_started.elapsed();
    let metrics = client.delivery_metrics();

    eprintln!(
        "paced_fixture capture_ms={} max_pending_chunks={} max_pending_bytes={} max_pending_age_ms={} pending_at_stop={} stop_drain_ms={} http_batches={} durable_commands={} encoded_bytes={} batch_body_bytes={} batch_request_ms={}",
        capture_elapsed.as_millis(),
        max_pending_chunks,
        max_pending_bytes,
        max_pending_age.as_millis(),
        pending_at_stop,
        stop_drain.as_millis(),
        metrics.http_batch_requests,
        metrics.durable_audio_commands,
        metrics.encoded_payload_bytes,
        metrics.batch_body_bytes,
        metrics.batch_request_micros / 1_000,
    );

    stopped.store(true, Ordering::Release);
    server.join().unwrap();
    assert!(capture_elapsed >= Duration::from_millis(1_980));
    // The producer must not inherit the 40 * 160 ms per-command network cost.
    // Leave debug-host fsync/scheduling out of this assertion; the separately
    // reported paced metrics are the performance evidence.
    assert!(capture_elapsed < Duration::from_secs(5));
    assert!(pending_at_stop <= 12, "pending at stop: {pending_at_stop}");
    assert!(
        max_pending_chunks <= 16,
        "max pending: {max_pending_chunks}"
    );
    assert!(max_pending_bytes < 20_000, "max bytes: {max_pending_bytes}");
    assert!(max_pending_age < Duration::from_millis(800));
    assert!(
        stop_drain < Duration::from_millis(650),
        "stop drain: {stop_drain:?}"
    );
    assert_eq!(capability_requests.load(Ordering::Acquire), 1);
    assert_eq!(
        batch_requests.load(Ordering::Acquire) as u64,
        metrics.http_batch_requests
    );
    // Exact cadence varies with debug-host fsync speed: a slower producer ages
    // partial batches sooner. Require a substantial reduction from 40 one-command
    // requests; the service test independently verifies the 16-command ceiling.
    assert!(metrics.http_batch_requests <= 8, "metrics: {metrics:?}");
    assert!(metrics.durable_audio_commands >= 40, "metrics: {metrics:?}");
    assert_eq!(
        metrics.durable_audio_commands,
        metrics.durable_audio_receipts
    );
    assert!(metrics.batch_body_bytes > metrics.encoded_payload_bytes);
}

#[test]
fn durable_spool_recovers_frames_and_retries_without_changing_identity() {
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "transfer-a",
        "instance-a",
        "https://margins.example.test/",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    assert_eq!(spool.producer_token().unwrap(), "producer-secret");
    assert!(
        !std::fs::read_to_string(spool.root().join("manifest.json"))
            .unwrap()
            .contains("producer-secret")
    );
    let first = spool.append_chunk(&chunk(0, 0x11)).unwrap();
    let exact = spool.append_chunk(&chunk(0, 0x11)).unwrap();
    assert_eq!(first.path, exact.path);
    assert!(spool.append_chunk(&chunk(0, 0x22)).is_err());
    spool.append_chunk(&chunk(1, 0x12)).unwrap();

    // Reopen is the crash/reload boundary: frames are self-describing and do
    // not depend on an in-memory upload queue or a successfully returned call.
    drop(spool);
    let recovered = DurableTransferSpool::open(temp.path(), "transfer-a", 0).unwrap();
    let pending = recovered.pending_chunks().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].command.message_id.as_ref(), "chunk-0");
    recovered.acknowledge(&pending[0]).unwrap();
    drop(recovered);
    let mut recovered = DurableTransferSpool::open(temp.path(), "transfer-a", 0).unwrap();
    assert_eq!(recovered.pending_chunks().unwrap().len(), 1);

    let close = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "close-a".into(),
        session_id: "session-a".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_100),
        body: ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "segment-a".into(),
            ended_at_ms: SessionMillis(20),
            lane_boundaries: vec![LaneBoundaryV1 {
                lane_id: "mic".into(),
                next_sequence: 2,
            }],
            reason: SegmentCloseReasonV1::Stop,
        }),
    };
    let finalize = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "finalize-a".into(),
        session_id: "session-a".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_100),
        body: ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(20),
            segment_closes: vec![SegmentCloseReferenceV1 {
                segment_id: "segment-a".into(),
                close_message_id: "close-a".into(),
            }],
            reason: SessionFinalizeReasonV1::Completed,
        }),
    };
    recovered.set_close(close.clone()).unwrap();
    // Deliberately retain a snapshot from before memo/finalize were appended.
    // Public retry/finalize must re-read under the manifest lock before making
    // the remote finalization call.
    let mut stale_finalize = DurableTransferSpool::open(temp.path(), "transfer-a", 0).unwrap();
    recovered
        .set_memo_intent(WorkspaceMemoReplaceV1 {
            request_id: "memo-a".into(),
            expected_revision: "memo-empty".into(),
            lines: Vec::new(),
        })
        .unwrap();
    recovered.set_finalize(finalize).unwrap();
    let recovery = recovered.recovery_path("segment-a").unwrap();
    std::fs::write(&recovery, b"durable recovery evidence").unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    let last = recovered.pending_chunks().unwrap().remove(0);
    recovered.acknowledge(&last).unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    recovered.acknowledge_control(&close).unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_requests = requests.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let capabilities = WorkspaceCapabilitiesV1 {
            protocol_version: ProtocolVersionV1,
            instance_id: "instance-a".into(),
            workspace_id: "workspace-a".into(),
            limits: WorkspaceLimitsV1 {
                max_chunk_bytes: 1024,
                max_in_flight_chunks: 1,
                max_event_page: 10,
                max_import_bytes: 1024,
                spool_reserve_bytes: 0,
            },
            capture_formats: Vec::new(),
            operations: Vec::new(),
            asr_available: false,
            recall_available: false,
        };
        while stop_rx.try_recv().is_err() {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    server_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut request = [0_u8; 4096];
                    let _ = stream.read(&mut request);
                    let body = serde_json::to_vec(&serde_json::json!({
                        "ok": true,
                        "result": capabilities,
                    }))
                    .unwrap();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(error) => panic!("fixture server failed: {error}"),
            }
        }
    });
    let client = WorkspaceHttpClient::new(
        url::Url::parse(&format!("http://{address}/")).unwrap(),
        "scoped-token",
        "workspace-a",
        None,
    )
    .unwrap();
    let error = deliver_finalize(&mut stale_finalize, &client).unwrap_err();
    assert!(error.to_string().contains("memo input"));
    stop_tx.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!recovered.manifest().completed);
    recovered.acknowledge_memo().unwrap();
    assert!(recovered.ready_to_finalize().unwrap());
    recovered.mark_completed().unwrap();
    assert!(!recovery.exists());
    assert!(!recovered.root().join("producer-token").exists());
}

#[test]
fn reservation_intent_survives_lost_response_and_promotes_without_secret_leak() {
    let temp = tempfile::tempdir().unwrap();
    let command = native_create_session_command(
        "session-a",
        "reserve-transfer-a",
        Some("Durable reservation".into()),
        "test",
    );
    let intent = CaptureReservationIntentV1 {
        schema: "margins.capture-reservation.v1".into(),
        transfer_id: "transfer-a".into(),
        instance_id: "instance-a".into(),
        remote_url: "https://margins.example.test".into(),
        workspace_id: "workspace-a".into(),
        session_id: "session-a".into(),
        request: CaptureReservationRequestV1::Create {
            command: command.clone(),
        },
    }
    .persist(temp.path())
    .unwrap();

    // Simulate a server commit followed by a lost response/crash. Reopen uses
    // the exact request identity and contains no returned producer capability.
    drop(intent);
    let recovered = pending_capture_reservations(temp.path()).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].request,
        CaptureReservationRequestV1::Create { command }
    );
    let json = std::fs::read_to_string(temp.path().join("reservations/transfer-a.json")).unwrap();
    assert!(!json.contains("producer-secret"));

    DurableTransferSpool::create(
        temp.path(),
        "transfer-a",
        "instance-a",
        "https://margins.example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    recovered[0].remove(temp.path()).unwrap();
    assert!(
        pending_capture_reservations(temp.path())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        DurableTransferSpool::open(temp.path(), "transfer-a", 0)
            .unwrap()
            .producer_token()
            .unwrap(),
        "producer-secret"
    );
}

#[test]
fn concurrent_reservation_promotion_creates_once_and_capture_lease_fences_loser() {
    if let Ok(root) = std::env::var("MARGINS_TEST_CAPTURE_LEASE_ROOT") {
        let spool =
            DurableTransferSpool::open(std::path::Path::new(&root), "promotion-race", 0).unwrap();
        assert!(spool.acquire_capture_lease().is_err());
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let intent = CaptureReservationIntentV1 {
        schema: "margins.capture-reservation.v1".into(),
        transfer_id: "promotion-race".into(),
        instance_id: "instance-a".into(),
        remote_url: "https://margins.example.test".into(),
        workspace_id: "workspace-a".into(),
        session_id: "session-a".into(),
        request: CaptureReservationRequestV1::Create {
            command: native_create_session_command(
                "session-a",
                "reserve-promotion-race",
                None,
                "test",
            ),
        },
    }
    .persist(&root)
    .unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let first_root = root.clone();
    let first_intent = intent.clone();
    let first_calls = calls.clone();
    let first = std::thread::spawn(move || {
        promote_capture_reservation(&first_intent, &first_root, 0, || {
            first_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok("producer-secret".into())
        })
        .unwrap()
    });
    entered_rx.recv().unwrap();
    let second_root = root.clone();
    let second_intent = intent.clone();
    let second_calls = calls.clone();
    let second = std::thread::spawn(move || {
        promote_capture_reservation(&second_intent, &second_root, 0, || {
            second_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("must-not-replace".into())
        })
        .unwrap()
    });
    release_tx.send(()).unwrap();
    let mut owner = first.join().unwrap();
    let reopened = second.join().unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(owner.producer_token().unwrap(), "producer-secret");
    assert_eq!(reopened.producer_token().unwrap(), "producer-secret");

    let lease = owner.acquire_capture_lease().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "concurrent_reservation_promotion_creates_once_and_capture_lease_fences_loser",
        ])
        .env("MARGINS_TEST_CAPTURE_LEASE_ROOT", &root)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "capture lease subprocess failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    let close = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "promotion-close".into(),
        session_id: "session-a".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000),
        body: ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "segment-a".into(),
            ended_at_ms: SessionMillis(0),
            lane_boundaries: Vec::new(),
            reason: SegmentCloseReasonV1::Error,
        }),
    };
    owner.set_close(close.clone()).unwrap();
    drop(lease);
    assert!(reopened.acquire_capture_lease().is_ok());
    let final_reopen = promote_capture_reservation(&intent, &root, 0, || {
        panic!("completed promotion must not reserve again")
    })
    .unwrap();
    assert_eq!(final_reopen.manifest().close_commands, vec![close]);
}

#[test]
fn native_opus_adapter_preserves_lane_identity_source_count_and_duration() {
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "native-transfer",
        "instance-a",
        "https://margins.example.test/",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer
        .begin_segment("segment-native".into(), 250)
        .unwrap();
    let mic = (0..3_200)
        .map(|index| [i16::MIN, -23_456, -1, 0, 1, 12_345, i16::MAX][index % 7])
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    let system = (0..3_200)
        .map(|index| [31_337_i16, 2, -17, -30_001][index % 4])
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    assert_eq!(
        transfer
            .append_s16le(NativeRemoteLane::Microphone, 16_000, &mic)
            .unwrap(),
        3_200
    );
    assert_eq!(
        transfer
            .append_s16le(NativeRemoteLane::System, 16_000, &system)
            .unwrap(),
        3_200
    );
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let ClientMessageBodyV1::CloseSegment(close) = close.body else {
        panic!("expected close");
    };
    assert_eq!(close.ended_at_ms.0, 450);
    assert_eq!(close.lane_boundaries.len(), 2);
    assert!(
        close
            .lane_boundaries
            .iter()
            .all(|boundary| boundary.next_sequence == 2)
    );

    let chunks = transfer.spool().pending_chunks().unwrap();
    assert_eq!(chunks.len(), 4);
    let mut encoded = std::collections::BTreeMap::new();
    for chunk in chunks {
        let ClientMessageBodyV1::AudioChunk(audio) = chunk.command.body else {
            panic!("expected audio");
        };
        assert!(matches!(audio.starts_at_ms.0, 250 | 350));
        encoded
            .entry(audio.lane_id.0)
            .or_insert_with(Vec::new)
            .extend(audio.payload);
    }
    assert_eq!(
        remote_opus_packet_stream_for_asr(&encoded["mic"])
            .unwrap()
            .len(),
        3_200
    );
    assert_eq!(
        remote_opus_packet_stream_for_asr(&encoded["system"])
            .unwrap()
            .len(),
        3_200
    );
    assert_ne!(encoded["mic"], encoded["system"]);
}

#[test]
fn native_opus_attach_uses_session_lane_identity_not_current_instance_preference() {
    let command =
        native_create_session_command("session-a", "native-format", Some("native".into()), "test");
    let ClientMessageBodyV1::CreateSession(create) = command.body else {
        panic!("expected create");
    };
    validate_native_opus_capture_lanes(&create.lanes).unwrap();

    let mut legacy = create.lanes;
    for lane in &mut legacy {
        lane.format = AudioFormatV1 {
            codec: AudioCodecV1::PcmS16Le,
            container: AudioContainerV1::Raw,
            sample_rate_hz: 48_000,
            channel_count: 1,
        };
    }
    let error = validate_native_opus_capture_lanes(&legacy)
        .unwrap_err()
        .to_string();
    assert!(error.contains("retry its existing transfer"), "{error}");
}

fn native_stream_for_frames(frames: usize) -> (Vec<u8>, CloseSegmentV1) {
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "native-boundary",
        "instance-a",
        "https://margins.example.test/",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer
        .begin_segment("segment-boundary".into(), 700)
        .unwrap();
    let pcm = (0..frames)
        .map(|index| ((index as i32 * 997 % 32_000) - 16_000) as i16)
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &pcm)
        .unwrap();
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let ClientMessageBodyV1::CloseSegment(close) = close.body else {
        panic!("expected close");
    };
    let mut chunks = transfer
        .spool()
        .pending_chunks()
        .unwrap()
        .into_iter()
        .map(|chunk| match chunk.command.body {
            ClientMessageBodyV1::AudioChunk(audio) => (audio.sequence, audio.payload),
            _ => panic!("expected audio chunk"),
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(sequence, _)| *sequence);
    let stream = chunks
        .into_iter()
        .flat_map(|(_, payload)| payload)
        .collect();
    (stream, close)
}

#[test]
fn native_opus_uses_real_16k_lookahead_and_handles_exact_durable_multiples_and_tail() {
    let encoder = Encoder::builder(16_000, Channels::Mono, Application::Voip)
        .bitrate(Bitrate::Bits(24_000))
        .build()
        .unwrap();
    assert_eq!(
        encoder.lookahead(),
        104,
        "ropus lookahead is in input-rate frames"
    );

    for frames in [1_280_usize, 2_560, 1_403, 1_600, 3_200, 1_723] {
        let (stream, close) = native_stream_for_frames(frames);
        let blocks = decode_opus_packet_blocks_v1(&stream).unwrap();
        let summary = validate_opus_packet_stream_v1(&stream).unwrap();
        assert_eq!(summary.pre_skip_48k, 312);
        assert_eq!(summary.source_frame_count, frames as u64);
        assert!(blocks.first().unwrap().stream_start);
        assert!(blocks.last().unwrap().stream_end);
        assert!(blocks.last().unwrap().source_frame_count > 0);
        assert_eq!(
            remote_opus_packet_stream_for_asr(&stream).unwrap().len(),
            frames
        );
        assert_eq!(
            close.ended_at_ms.0,
            700 + (frames as u64 * 1_000).div_ceil(16_000)
        );
    }
}

#[test]
fn native_opus_decoder_rejects_valid_non_twenty_millisecond_packets() {
    let mut encoder = Encoder::builder(16_000, Channels::Mono, Application::Voip)
        .bitrate(Bitrate::Bits(24_000))
        .build()
        .unwrap();
    let mut packet = [0_u8; OPUS_PACKET_MAX_BYTES_V1];
    let bytes = encoder.encode(&[0_i16; 160], &mut packet).unwrap();
    let stream = OpusPacketBlockV1 {
        stream_start: true,
        stream_end: true,
        pre_skip_48k: 312,
        sample_rate_hz: 16_000,
        source_start_frame: 0,
        source_frame_count: 56,
        frame_samples: OPUS_PACKET_FRAME_SAMPLES_V1,
        packets: vec![packet[..bytes].to_vec()],
    }
    .encode()
    .unwrap();
    assert!(validate_opus_packet_stream_v1(&stream).is_ok());
    let error = remote_opus_packet_stream_for_asr(&stream)
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected 320 (20 ms)"), "{error}");
}

#[test]
fn native_opus_silent_lane_preserves_source_timeline_without_dtx_elision() {
    let frames = 32_000;
    let (stream, close) = native_stream_for_frames(frames);
    let decoded = remote_opus_packet_stream_for_asr(&stream).unwrap();
    assert_eq!(decoded.len(), frames);
    assert_eq!(close.ended_at_ms.0, 2_700);
    let peak = decoded
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    // The deterministic input above is not silent; run a true silent lane too.
    assert!(peak > 0.0);

    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "silent",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("silent-segment".into(), 0).unwrap();
    transfer
        .append_s16le(NativeRemoteLane::System, 16_000, &vec![0; frames * 2])
        .unwrap();
    transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let mut chunks = transfer.spool().pending_chunks().unwrap();
    chunks.sort_by_key(|chunk| match &chunk.command.body {
        ClientMessageBodyV1::AudioChunk(audio) => audio.sequence,
        _ => u64::MAX,
    });
    let encoded = chunks
        .into_iter()
        .flat_map(|chunk| match chunk.command.body {
            ClientMessageBodyV1::AudioChunk(audio) => audio.payload,
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    let summary = validate_opus_packet_stream_v1(&encoded).unwrap();
    assert_eq!(summary.source_frame_count, frames as u64);
    assert!(summary.packet_count >= 101, "DTX must remain disabled");
    let decoded = remote_opus_packet_stream_for_asr(&encoded).unwrap();
    assert_eq!(decoded.len(), frames);
    assert!(decoded.iter().all(|sample| sample.abs() <= 1.0 / 32_768.0));
}

#[test]
fn native_opus_crash_recovery_closes_durable_prefix_and_next_segment_is_contiguous() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spool = DurableTransferSpool::create(
        root,
        "crash",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer
        .begin_segment("crashed-segment".into(), 5_000)
        .unwrap();
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![3; 3_200])
        .unwrap();
    // The 100 ms source checkpoint is already fsynced even though the first
    // five-packet Opus block is still held for terminal-tail-safe framing.
    assert!(transfer.spool().pending_chunks().unwrap().is_empty());
    drop(transfer);

    let spool = DurableTransferSpool::open(root, "crash", 0).unwrap();
    let mut recovered = NativeRemoteTransfer::new(spool);
    let close = recovered
        .recover_interrupted_segment(SegmentCloseReasonV1::Error)
        .unwrap()
        .expect("100 ms checkpoint should be recoverable");
    let ClientMessageBodyV1::CloseSegment(close) = close.body else {
        panic!("expected recovery close");
    };
    assert_eq!(close.ended_at_ms.0, 5_100);
    assert_eq!(recovered.last_closed_ended_at_ms(), Some(5_100));

    recovered
        .begin_segment("resumed-segment".into(), 5_100)
        .unwrap();
    recovered
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![7; 640])
        .unwrap();
    recovered.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    assert_eq!(recovered.last_closed_ended_at_ms(), Some(5_120));
    let final_command = recovered
        .seal_session(5_120, SessionFinalizeReasonV1::Completed)
        .unwrap();
    let ClientMessageBodyV1::FinalizeSession(finalize) = final_command.body else {
        panic!("expected finalize");
    };
    assert_eq!(finalize.ended_at_ms.0, 5_120);
    assert_eq!(finalize.segment_closes.len(), 2);
}

#[test]
fn native_opus_close_cleanup_crash_reuses_the_durable_two_lane_close() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spool = DurableTransferSpool::create(
        root,
        "close-cleanup-crash",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("segment-a".into(), 0).unwrap();
    for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
        transfer
            .append_s16le(lane, 16_000, &vec![3; 6_400])
            .unwrap();
    }
    let transfer_path = root.join("close-cleanup-crash");
    let open_path = transfer_path.join("open-native-segment.json");
    let system_path = transfer_path.join("recovery/segment-a-system.s16le");
    let system_durable_path = system_path.with_extension("durable");
    let open = std::fs::read(&open_path).unwrap();
    let system = std::fs::read(&system_path).unwrap();
    let system_durable = std::fs::read(&system_durable_path).unwrap();
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let pending = transfer.spool().pending_chunks().unwrap();
    let pending_before = pending.len();
    assert!(pending_before > 0);
    // Also preserve the legitimate ACK-fsynced/frame-not-yet-unlinked state.
    let redundant = &pending[0];
    let receipt = transfer_path
        .join("acks")
        .join(redundant.path.file_name().unwrap())
        .with_extension("ack");
    std::fs::write(receipt, redundant.payload_digest.as_bytes()).unwrap();
    drop(transfer);

    // Reconstruct the exact observable state after the microphone journal was
    // removed but before the system journal/open marker cleanup committed.
    std::fs::write(&system_path, system).unwrap();
    std::fs::write(&system_durable_path, system_durable).unwrap();
    std::fs::write(&open_path, open).unwrap();

    let spool = DurableTransferSpool::open(root, "close-cleanup-crash", 0).unwrap();
    let mut recovered = NativeRemoteTransfer::new(spool);
    let replayed = recovered
        .recover_interrupted_segment(SegmentCloseReasonV1::Error)
        .unwrap()
        .expect("the already-durable close must survive cleanup replay");
    assert_eq!(replayed, close);
    assert_eq!(
        recovered.spool().pending_chunks().unwrap().len(),
        pending_before
    );
    assert!(!open_path.exists());
    assert!(!system_path.exists());
    assert!(!system_durable_path.exists());
}

#[test]
fn native_opus_pause_resume_final_duration_is_the_last_media_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "pause-resume",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("first".into(), 0).unwrap();
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![0; 3_200])
        .unwrap();
    transfer.close_segment(SegmentCloseReasonV1::Pause).unwrap();
    assert_eq!(transfer.last_closed_ended_at_ms(), Some(100));
    transfer.begin_segment("second".into(), 375).unwrap();
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![0; 6_400])
        .unwrap();
    transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let media_boundary = transfer.last_closed_ended_at_ms().unwrap();
    assert_eq!(media_boundary, 575);

    // A delayed uploader/memo path must not be sampled into the capture clock.
    std::thread::sleep(std::time::Duration::from_millis(40));
    let final_command = transfer
        .seal_session(media_boundary, SessionFinalizeReasonV1::Completed)
        .unwrap();
    let ClientMessageBodyV1::FinalizeSession(finalize) = final_command.body else {
        panic!("expected finalize");
    };
    assert_eq!(finalize.ended_at_ms.0, 575);
    assert_eq!(finalize.segment_closes.len(), 2);
}

#[test]
fn native_opus_sub_checkpoint_crash_is_bounded_and_low_space_fails_truthfully() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spool = DurableTransferSpool::create(
        root,
        "short-crash",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("short-segment".into(), 0).unwrap();
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![1; 2_528])
        .unwrap();
    assert!(transfer.spool().pending_chunks().unwrap().is_empty());
    drop(transfer);
    let spool = DurableTransferSpool::open(root, "short-crash", 0).unwrap();
    let mut recovered = NativeRemoteTransfer::new(spool);
    assert!(
        recovered
            .recover_interrupted_segment(SegmentCloseReasonV1::Error)
            .unwrap()
            .is_none()
    );
    assert!(recovered.spool().pending_chunks().unwrap().is_empty());
    recovered
        .seal_session(0, SessionFinalizeReasonV1::Error)
        .unwrap();

    let full = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        full.path(),
        "full",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        u64::MAX,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("full-segment".into(), 0).unwrap();
    let error = transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![0; 3_200])
        .unwrap_err()
        .to_string();
    assert!(error.contains("disk reserve"), "{error}");
    assert!(transfer.spool().pending_chunks().unwrap().is_empty());
}

fn capture_resampled(rate: u32, samples: &[f32], partitions: &[usize]) -> Vec<u8> {
    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "resample-transfer",
        "instance-a",
        "https://margins.example.test/",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer
        .begin_segment("segment-resample".into(), 0)
        .unwrap();
    let mut offset = 0;
    for &length in partitions {
        let end = (offset + length).min(samples.len());
        transfer
            .append_f32(NativeRemoteLane::Microphone, rate, &samples[offset..end])
            .unwrap();
        offset = end;
    }
    if offset < samples.len() {
        transfer
            .append_f32(NativeRemoteLane::Microphone, rate, &samples[offset..])
            .unwrap();
    }
    transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let mut chunks = transfer
        .spool()
        .pending_chunks()
        .unwrap()
        .into_iter()
        .map(|chunk| match chunk.command.body {
            ClientMessageBodyV1::AudioChunk(audio) => (audio.sequence, audio.payload),
            _ => panic!("expected audio chunk"),
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(sequence, _)| *sequence);
    let encoded = chunks
        .into_iter()
        .flat_map(|(_, bytes)| bytes)
        .collect::<Vec<_>>();
    remote_opus_packet_stream_for_asr(&encoded)
        .unwrap()
        .into_iter()
        .flat_map(|sample| ((sample * 32_768.0).round() as i16).to_le_bytes())
        .collect()
}

#[test]
fn native_resampler_is_partition_invariant_and_drains_non_integer_tail() {
    for rate in [44_100_u32, 48_000_u32] {
        let samples = (0..rate as usize + 1)
            .map(|index| {
                (2.0 * std::f32::consts::PI * 437.0 * index as f32 / rate as f32).sin() * 0.5
            })
            .collect::<Vec<_>>();
        let contiguous = capture_resampled(rate, &samples, &[]);
        let partitioned = capture_resampled(rate, &samples, &[1, 17, 509, 2_048, 73, 8_191]);
        assert_eq!(
            partitioned, contiguous,
            "partitioned {rate} Hz conversion changed bytes"
        );
        let expected_frames =
            ((samples.len() as u128 * 16_000_u128).div_ceil(rate as u128)) as usize;
        assert_eq!(contiguous.len(), expected_frames * 2);
    }
}

#[test]
fn native_resampler_rejects_alias_energy_and_rate_changes() {
    let tone = |frequency: f32| {
        (0..48_000)
            .map(|index| {
                (2.0 * std::f32::consts::PI * frequency * index as f32 / 48_000.0).sin() * 0.8
            })
            .collect::<Vec<_>>()
    };
    let rms = |bytes: &[u8]| {
        let samples = bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f64 / 32_768.0)
            .collect::<Vec<_>>();
        (samples.iter().map(|sample| sample * sample).sum::<f64>() / samples.len() as f64).sqrt()
    };
    let passband = capture_resampled(48_000, &tone(1_000.0), &[]);
    let aliased = capture_resampled(48_000, &tone(12_000.0), &[]);
    assert!(rms(&aliased) < rms(&passband) * 0.05);

    let temp = tempfile::tempdir().unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "rate-change",
        "instance-a",
        "https://example.test",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("segment-rate".into(), 0).unwrap();
    transfer
        .append_f32(NativeRemoteLane::Microphone, 48_000, &[0.0; 64])
        .unwrap();
    assert!(
        transfer
            .append_f32(NativeRemoteLane::Microphone, 44_100, &[0.0; 64])
            .is_err()
    );
}

#[test]
fn stale_delivery_snapshot_cannot_erase_concurrent_manifest_or_frame_intents() {
    let temp = tempfile::tempdir().unwrap();
    let mut producer = DurableTransferSpool::create(
        temp.path(),
        "interleaved-transfer",
        "instance-a",
        "https://margins.example.test/",
        "workspace-a",
        "session-a",
        "producer-secret",
        0,
    )
    .unwrap();
    let close_command = |id: &str, segment: &str, ended: u64| ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: id.into(),
        session_id: "session-a".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000 + ended),
        body: ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: segment.into(),
            ended_at_ms: SessionMillis(ended),
            lane_boundaries: vec![LaneBoundaryV1 {
                lane_id: "mic".into(),
                next_sequence: 1,
            }],
            reason: SegmentCloseReasonV1::Pause,
        }),
    };
    let close_a = close_command("close-a", "segment-a", 10);
    producer.set_close(close_a.clone()).unwrap();

    // The delivery process has deliberately loaded an older full snapshot.
    let delivery = DurableTransferSpool::open(temp.path(), "interleaved-transfer", 0).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    let delivery_thread = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        ack_rx.recv().unwrap();
        delivery.acknowledge_control(&close_a).unwrap();
    });
    ready_rx.recv().unwrap();

    let close_b = close_command("close-b", "segment-b", 20);
    producer.set_close(close_b.clone()).unwrap();
    producer.append_chunk(&chunk(0, 0x41)).unwrap();
    producer
        .set_memo_intent(WorkspaceMemoReplaceV1 {
            request_id: "memo-interleave".into(),
            expected_revision: "memo-empty".into(),
            lines: Vec::new(),
        })
        .unwrap();
    producer
        .set_finalize(ClientMessageV1 {
            protocol_version: ProtocolVersionV1,
            message_id: "finalize-interleave".into(),
            session_id: "session-a".into(),
            sent_at_unix_ms: UnixMillis(1_800_000_000_020),
            body: ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(20),
                segment_closes: vec![
                    SegmentCloseReferenceV1 {
                        segment_id: "segment-a".into(),
                        close_message_id: "close-a".into(),
                    },
                    SegmentCloseReferenceV1 {
                        segment_id: "segment-b".into(),
                        close_message_id: "close-b".into(),
                    },
                ],
                reason: SessionFinalizeReasonV1::Completed,
            }),
        })
        .unwrap();
    ack_tx.send(()).unwrap();
    delivery_thread.join().unwrap();

    // Reopen is the crash boundary: the stale delivery owner wrote only an
    // ACK delta, so it cannot replace newer producer state.
    let recovered = DurableTransferSpool::open(temp.path(), "interleaved-transfer", 0).unwrap();
    assert_eq!(recovered.manifest().close_commands.len(), 2);
    assert_eq!(recovered.pending_chunks().unwrap().len(), 1);
    assert!(recovered.manifest().memo_intent.is_some());
    assert!(recovered.manifest().finalize_command.is_some());
    assert_eq!(recovered.pending_closes(), vec![close_b]);
    assert!(!recovered.manifest().completed);
}
