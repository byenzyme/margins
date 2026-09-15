use margins_meeting_protocol::*;
use margins_workflows::remote_workspace::*;
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
    assert!(!std::fs::read_to_string(spool.root().join("manifest.json"))
        .unwrap()
        .contains("producer-secret"));
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
        request: CaptureReservationRequestV1::Create { command: command.clone() },
    }
    .persist(temp.path())
    .unwrap();

    // Simulate a server commit followed by a lost response/crash. Reopen uses
    // the exact request identity and contains no returned producer capability.
    drop(intent);
    let recovered = pending_capture_reservations(temp.path()).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].request, CaptureReservationRequestV1::Create { command });
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
    assert!(pending_capture_reservations(temp.path()).unwrap().is_empty());
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
        let spool = DurableTransferSpool::open(std::path::Path::new(&root), "promotion-race", 0)
            .unwrap();
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
fn native_pcm_adapter_preserves_distinct_lane_samples_channels_and_duration() {
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
    let mut transfer = NativePcmTransfer::new(spool);
    transfer
        .begin_segment("segment-native".into(), 250)
        .unwrap();
    let mic = (0..4_800)
        .map(|index| [i16::MIN, -23_456, -1, 0, 1, 12_345, i16::MAX][index % 7])
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    let system = (0..4_800)
        .map(|index| [31_337_i16, 2, -17, -30_001][index % 4])
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    assert_eq!(
        transfer
            .append_s16le(NativePcmLane::Microphone, 48_000, &mic)
            .unwrap(),
        4_800
    );
    assert_eq!(
        transfer
            .append_s16le(NativePcmLane::System, 48_000, &system)
            .unwrap(),
        4_800
    );
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let ClientMessageBodyV1::CloseSegment(close) = close.body else {
        panic!("expected close");
    };
    assert_eq!(close.ended_at_ms.0, 350);
    assert_eq!(close.lane_boundaries.len(), 2);
    assert!(close
        .lane_boundaries
        .iter()
        .all(|boundary| boundary.next_sequence == 1));

    let chunks = transfer.spool().pending_chunks().unwrap();
    assert_eq!(chunks.len(), 2);
    let mut encoded = std::collections::BTreeMap::new();
    for chunk in chunks {
        let ClientMessageBodyV1::AudioChunk(audio) = chunk.command.body else {
            panic!("expected audio");
        };
        assert_eq!(audio.starts_at_ms.0, 250);
        assert_eq!(audio.duration_ms.0, 100);
        assert_eq!(audio.payload.len(), 4_800 * 2);
        encoded.insert(audio.lane_id.0, audio.payload);
    }
    assert_eq!(encoded["mic"], mic);
    assert_eq!(encoded["system"], system);
    assert_ne!(encoded["mic"], encoded["system"]);
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
