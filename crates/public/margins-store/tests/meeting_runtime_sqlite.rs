use margins_meeting_protocol::*;
use margins_meeting_runtime::{MeetingRuntime, MeetingRuntimeStorage};
use margins_store::SqliteMeetingRuntimeStorage;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Barrier},
    thread,
};

fn message(session: &str, id: impl Into<String>, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: id.into().into(),
        session_id: session.into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000),
        body,
    }
}

fn create(session: &str) -> ClientMessageV1 {
    message(
        session,
        "create",
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: format!("create-{session}"),
            started_at_unix_ms: UnixMillis(1_800_000_000_000),
            title: None,
            sources: vec![CaptureSourceV1 {
                source_id: "mic".into(),
                kind: CaptureSourceKindV1::Microphone,
                label: None,
                external_id: None,
            }],
            lanes: vec![CaptureLaneV1 {
                lane_id: "audio".into(),
                source_ids: vec!["mic".into()],
                label: None,
                format: AudioFormatV1 {
                    codec: AudioCodecV1::PcmS16Le,
                    container: AudioContainerV1::Raw,
                    sample_rate_hz: 48_000,
                    channel_count: 1,
                },
            }],
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "sqlite-test".into(),
                    producer_version: None,
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: UnixMillis(1_800_000_000_000),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    )
}

fn chunk(session: &str, id: impl Into<String>, sequence: u64, bytes: usize) -> ClientMessageV1 {
    let payload = vec![(sequence % 251) as u8; bytes];
    message(
        session,
        id,
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "part".into(),
            lane_id: "audio".into(),
            sequence,
            starts_at_ms: SessionMillis(sequence * 20),
            duration_ms: DurationMillis(20),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: format!("{:x}", Sha256::digest(&payload)),
            },
            payload,
        }),
    )
}

fn close(session: &str, boundary: u64) -> ClientMessageV1 {
    message(
        session,
        "close",
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "part".into(),
            ended_at_ms: SessionMillis(boundary * 20),
            lane_boundaries: vec![LaneBoundaryV1 {
                lane_id: "audio".into(),
                next_sequence: boundary,
            }],
            reason: SegmentCloseReasonV1::Stop,
        }),
    )
}

fn ack_count(messages: &[ServerMessageV1]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message.body, ServerMessageBodyV1::AudioAcknowledged(_)))
        .count()
}

#[test]
fn exact_retry_after_restart_replays_one_receipt_and_one_blob() {
    let temp = tempfile::tempdir().unwrap();
    let command = chunk("durable", "chunk-0", 0, 4096);
    let first = {
        let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
        runtime.handle(create("durable")).unwrap();
        runtime.handle(command.clone()).unwrap()
    };
    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    let replay = runtime.handle(command).unwrap();
    assert!(replay.idempotent_replay);
    assert_eq!(replay.messages, first.messages);
    let stats = runtime.storage().stats().unwrap();
    assert_eq!((stats.sessions, stats.receipts, stats.chunks), (1, 2, 1));
    assert_eq!(stats.blob_bytes, 4096);
}

#[test]
fn staged_blob_without_metadata_never_produces_a_false_ack_and_is_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let storage = SqliteMeetingRuntimeStorage::open(temp.path()).unwrap();
    let runtime = MeetingRuntime::new(storage.clone());
    runtime.handle(create("crash-boundary")).unwrap();
    storage.fail_before_metadata_commit_once();
    let command = chunk("crash-boundary", "chunk-0", 0, 2048);
    assert!(runtime.handle(command.clone()).is_err());
    let failed = storage.stats().unwrap();
    assert_eq!((failed.receipts, failed.chunks), (1, 0));
    assert_eq!(
        failed.blob_bytes, 2048,
        "immutable staging may precede metadata"
    );

    let reopened = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    let accepted = reopened.handle(command).unwrap();
    assert_eq!(ack_count(&accepted.messages), 1);
    assert_eq!(
        (
            reopened.storage().stats().unwrap().receipts,
            reopened.storage().stats().unwrap().chunks
        ),
        (2, 1)
    );
}

#[test]
fn missing_chunk_cannot_be_hidden_by_close_or_restart() {
    let temp = tempfile::tempdir().unwrap();
    {
        let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
        runtime.handle(create("gap")).unwrap();
        runtime.handle(chunk("gap", "chunk-1", 1, 32)).unwrap();
        assert!(runtime.handle(close("gap", 2)).unwrap().messages.is_empty());
    }
    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    let completion = runtime.handle(chunk("gap", "chunk-0", 0, 32)).unwrap();
    assert_eq!(ack_count(&completion.messages), 1);
    assert!(completion
        .messages
        .iter()
        .any(|message| matches!(message.body, ServerMessageBodyV1::SegmentFinalized(_))));
}

#[test]
fn concurrent_writers_use_sqlite_revision_cas_without_lost_updates() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MeetingRuntime::new(
        SqliteMeetingRuntimeStorage::open(temp.path()).unwrap(),
    ));
    runtime.handle(create("race")).unwrap();
    let barrier = Arc::new(Barrier::new(9));
    let handles: Vec<_> = (0..8)
        .map(|sequence| {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                runtime
                    .handle(chunk("race", format!("chunk-{sequence}"), sequence, 128))
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    for handle in handles {
        assert_eq!(ack_count(&handle.join().unwrap().messages), 1);
    }
    let stats = runtime.storage().stats().unwrap();
    assert_eq!(stats.chunks, 8);
    assert_eq!(stats.receipts, 9);
}

#[test]
fn compact_session_state_does_not_accumulate_payloads_or_event_history() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    runtime.handle(create("scale")).unwrap();
    let baseline = runtime.storage().stats().unwrap().session_state_bytes;
    for sequence in 0..256 {
        runtime
            .handle(chunk("scale", format!("chunk-{sequence}"), sequence, 1024))
            .unwrap();
    }
    let stats = runtime.storage().stats().unwrap();
    assert_eq!(stats.chunks, 256);
    assert_eq!(stats.blob_bytes, 256 * 1024);
    assert_eq!(stats.receipts, 257);
    assert_eq!(stats.events, 257);
    assert!(
        stats.session_state_bytes <= baseline + 128,
        "compact state grew from {baseline} to {}",
        stats.session_state_bytes
    );
    assert!(stats.chunk_metadata_bytes < stats.blob_bytes / 2);

    let stored = runtime
        .storage()
        .load_session(&SessionId::from("scale"))
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_vec(&stored).unwrap();
    assert!(encoded.len() < 2048);
    assert!(!encoded
        .windows(128)
        .any(|window| window.iter().all(|byte| *byte == 0)));
}
