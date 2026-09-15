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
    recovered.set_close(close).unwrap();
    recovered.set_finalize(finalize).unwrap();
    assert!(!recovered.ready_to_finalize().unwrap());
    let last = recovered.pending_chunks().unwrap().remove(0);
    recovered.acknowledge(&last).unwrap();
    assert!(recovered.ready_to_finalize().unwrap());
}
