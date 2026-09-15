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
    recovered.set_finalize(finalize).unwrap();
    let recovery = recovered.recovery_path("segment-a").unwrap();
    std::fs::write(&recovery, b"durable recovery evidence").unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    let last = recovered.pending_chunks().unwrap().remove(0);
    recovered.acknowledge(&last).unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    recovered.acknowledge_control(&close).unwrap();
    assert!(recovered.ready_to_finalize().unwrap());
    recovered.mark_completed().unwrap();
    assert!(!recovery.exists());
    assert!(!recovered.root().join("producer-token").exists());
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
