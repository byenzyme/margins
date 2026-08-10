use margins_meeting_protocol::*;
use margins_meeting_runtime::*;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

const BASE: u64 = 1_800_000_000_000;

fn digest(payload: &[u8]) -> ContentDigestV1 {
    ContentDigestV1 {
        algorithm: DigestAlgorithmV1::Sha256,
        hex: format!("{:x}", Sha256::digest(payload)),
    }
}

fn lane(id: &str, source: &str) -> CaptureLaneV1 {
    CaptureLaneV1 {
        lane_id: id.into(),
        source_ids: vec![source.into()],
        label: None,
        format: AudioFormatV1 {
            codec: AudioCodecV1::Opus,
            container: AudioContainerV1::Ogg,
            sample_rate_hz: 48_000,
            channel_count: 1,
        },
    }
}

fn create(session: &str, message: &str, key: &str, lanes: &[(&str, &str)]) -> ClientMessageV1 {
    let source_ids: Vec<_> = lanes
        .iter()
        .map(|(_, source)| *source)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    client(
        session,
        message,
        0,
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: key.into(),
            started_at_unix_ms: UnixMillis(BASE),
            title: Some("Runtime test".into()),
            sources: source_ids
                .into_iter()
                .map(|id| CaptureSourceV1 {
                    source_id: id.into(),
                    kind: CaptureSourceKindV1::Microphone,
                    label: None,
                    external_id: None,
                })
                .collect(),
            lanes: lanes.iter().map(|(id, source)| lane(id, source)).collect(),
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "browser".into(),
                    producer_version: Some("1".into()),
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: UnixMillis(BASE),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    )
}

fn client(session: &str, message: &str, tick: u64, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: message.into(),
        session_id: session.into(),
        sent_at_unix_ms: UnixMillis(BASE + tick),
        body,
    }
}

fn chunk(
    session: &str,
    message: &str,
    segment: &str,
    lane: &str,
    sequence: u64,
    digest_byte: &str,
) -> ClientMessageV1 {
    let payload = vec![sequence as u8, digest_byte.as_bytes()[0]];
    client(
        session,
        message,
        10 + sequence,
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: segment.into(),
            lane_id: lane.into(),
            sequence,
            starts_at_ms: SessionMillis(sequence * 100),
            duration_ms: DurationMillis(100),
            payload_digest: digest(&payload),
            payload,
        }),
    )
}

fn discontinuity(
    session: &str,
    message: &str,
    id: &str,
    segment: &str,
    lane: &str,
    start: u64,
    end: u64,
) -> ClientMessageV1 {
    client(
        session,
        message,
        100 + start,
        ClientMessageBodyV1::CaptureDiscontinuity(CaptureDiscontinuityV1 {
            discontinuity_id: id.into(),
            segment_id: segment.into(),
            lane_id: lane.into(),
            sequence_range: SequenceRangeV1 {
                start,
                end_exclusive: end,
            },
            starts_at_ms: SessionMillis(start * 100),
            duration_ms: DurationMillis(100),
            reason: DiscontinuityReasonV1::NetworkLoss,
            detail: None,
        }),
    )
}

fn close(
    session: &str,
    message: &str,
    segment: &str,
    boundaries: &[(&str, u64)],
) -> ClientMessageV1 {
    client(
        session,
        message,
        1_000,
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: segment.into(),
            ended_at_ms: SessionMillis(1_000),
            lane_boundaries: boundaries
                .iter()
                .map(|(lane, next)| LaneBoundaryV1 {
                    lane_id: (*lane).into(),
                    next_sequence: *next,
                })
                .collect(),
            reason: SegmentCloseReasonV1::Stop,
        }),
    )
}

fn finalize(session: &str, message: &str, references: &[(&str, &str)]) -> ClientMessageV1 {
    client(
        session,
        message,
        1_100,
        ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(1_000),
            segment_closes: references
                .iter()
                .map(|(segment, close)| SegmentCloseReferenceV1 {
                    segment_id: (*segment).into(),
                    close_message_id: (*close).into(),
                })
                .collect(),
            reason: SessionFinalizeReasonV1::Completed,
        }),
    )
}

fn ack(response: &RuntimeResponseV1) -> &AudioAcknowledgementV1 {
    response
        .messages
        .iter()
        .find_map(|message| match &message.body {
            ServerMessageBodyV1::AudioAcknowledged(ack) => Some(ack),
            _ => None,
        })
        .expect("response should contain an acknowledgement")
}

fn rejection(response: &RuntimeResponseV1) -> &CommandRejectedV1 {
    response
        .messages
        .iter()
        .find_map(|message| match &message.body {
            ServerMessageBodyV1::CommandRejected(rejection) => Some(rejection),
            _ => None,
        })
        .expect("response should contain a rejection")
}

#[test]
fn out_of_order_multilane_acks_are_canonical_and_close_waits_for_every_gap() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create(
            "session",
            "create",
            "create-key",
            &[("mic", "phone"), ("remote", "call")],
        ))
        .unwrap();

    let response = runtime
        .handle(chunk("session", "mic-2", "part-1", "mic", 2, "2"))
        .unwrap();
    assert_eq!(ack(&response).durable_through_sequence, 0);
    assert_eq!(
        ack(&response).durable_out_of_order,
        vec![SequenceRangeV1 {
            start: 2,
            end_exclusive: 3
        }]
    );

    let response = runtime
        .handle(chunk("session", "mic-0", "part-1", "mic", 0, "0"))
        .unwrap();
    assert_eq!(ack(&response).durable_through_sequence, 1);
    assert_eq!(
        ack(&response).durable_out_of_order,
        vec![SequenceRangeV1 {
            start: 2,
            end_exclusive: 3
        }]
    );
    let response = runtime
        .handle(discontinuity(
            "session", "mic-gap", "gap-1", "part-1", "mic", 1, 2,
        ))
        .unwrap();
    assert_eq!(ack(&response).durable_through_sequence, 3);
    assert!(ack(&response).durable_out_of_order.is_empty());

    runtime
        .handle(chunk("session", "remote-1", "part-1", "remote", 1, "3"))
        .unwrap();
    let pending = runtime
        .handle(close(
            "session",
            "close-part-1",
            "part-1",
            &[("mic", 3), ("remote", 2)],
        ))
        .unwrap();
    assert!(pending.messages.is_empty(), "close must remain pending");

    let completed = runtime
        .handle(chunk("session", "remote-0", "part-1", "remote", 0, "4"))
        .unwrap();
    assert_eq!(ack(&completed).durable_through_sequence, 2);
    assert!(matches!(
        completed.messages.last().unwrap().body,
        ServerMessageBodyV1::SegmentFinalized(_)
    ));

    let snapshot = runtime
        .storage()
        .snapshot(&SessionId::from("session"))
        .unwrap()
        .unwrap();
    for (sequence, event) in snapshot.events().iter().enumerate() {
        assert_eq!(event.sequence, sequence as u64);
        event.validate().unwrap();
    }
}

#[test]
fn multipart_finalize_freezes_exact_close_operations_and_allows_close_race() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "phone")]))
        .unwrap();
    runtime
        .handle(chunk("session", "a-0", "part-a", "mic", 0, "a"))
        .unwrap();
    runtime
        .handle(chunk("session", "b-0", "part-b", "mic", 0, "b"))
        .unwrap();

    let pending = runtime
        .handle(finalize(
            "session",
            "finalize",
            &[("part-a", "close-a"), ("part-b", "close-b")],
        ))
        .unwrap();
    assert!(pending.messages.is_empty());

    let unlisted = runtime
        .handle(chunk("session", "unlisted", "part-c", "mic", 0, "c"))
        .unwrap();
    assert_eq!(rejection(&unlisted).code, "invalid_transition");
    let wrong_close = runtime
        .handle(close("session", "wrong-close", "part-a", &[("mic", 1)]))
        .unwrap();
    assert_eq!(rejection(&wrong_close).code, "invalid_transition");

    let first = runtime
        .handle(close("session", "close-b", "part-b", &[("mic", 1)]))
        .unwrap();
    assert!(first
        .messages
        .iter()
        .any(|event| matches!(event.body, ServerMessageBodyV1::SegmentFinalized(_))));
    assert!(!first
        .messages
        .iter()
        .any(|event| matches!(event.body, ServerMessageBodyV1::SessionFinalized(_))));

    let last = runtime
        .handle(close("session", "close-a", "part-a", &[("mic", 1)]))
        .unwrap();
    let finalized = last
        .messages
        .iter()
        .find_map(|event| match &event.body {
            ServerMessageBodyV1::SessionFinalized(value) => Some(value),
            _ => None,
        })
        .expect("last exact close should finalize the session");
    assert_eq!(finalized.finalize_message_id, MessageId::from("finalize"));
    assert_eq!(
        finalized.segment_closes,
        vec![
            SegmentCloseReferenceV1 {
                segment_id: "part-a".into(),
                close_message_id: "close-a".into(),
            },
            SegmentCloseReferenceV1 {
                segment_id: "part-b".into(),
                close_message_id: "close-b".into(),
            },
        ]
    );
}

#[test]
fn command_chunk_discontinuity_and_provenance_idempotency_conflicts_are_separate() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    let create_message = create("session", "create", "key", &[("mic", "phone")]);
    let first = runtime.handle(create_message.clone()).unwrap();
    let retry = runtime.handle(create_message).unwrap();
    assert!(retry.idempotent_replay);
    assert_eq!(retry.messages, first.messages);

    let original = chunk("session", "chunk-0", "part", "mic", 0, "a");
    let accepted = runtime.handle(original.clone()).unwrap();
    let exact_retry = runtime.handle(original.clone()).unwrap();
    assert!(exact_retry.idempotent_replay);
    assert_eq!(accepted.messages, exact_retry.messages);

    let same_digest_new_message = chunk("session", "chunk-0-copy", "part", "mic", 0, "a");
    assert!(runtime
        .handle(same_digest_new_message)
        .unwrap()
        .messages
        .is_empty());
    let digest_conflict = runtime
        .handle(chunk("session", "chunk-0-conflict", "part", "mic", 0, "b"))
        .unwrap();
    assert_eq!(rejection(&digest_conflict).code, "sequence_conflict");

    let mut reused_message = original;
    reused_message.body = ClientMessageBodyV1::CaptureHealth(CaptureHealthV1 {
        observed_at_ms: SessionMillis(50),
        lane_id: Some("mic".into()),
        state: CaptureHealthStateV1::Healthy,
        queued_audio_ms: None,
        dropped_chunk_count: 0,
        clock_skew_ms: None,
        detail: None,
    });
    let conflict = runtime.handle(reused_message.clone()).unwrap();
    assert_eq!(rejection(&conflict).code, "conflict");
    assert_eq!(
        runtime.handle(reused_message).unwrap().messages,
        conflict.messages
    );

    let hop = |message: &str, producer: &str| {
        client(
            "session",
            message,
            200,
            ClientMessageBodyV1::AppendProvenanceHop(AppendProvenanceHopV1 {
                provenance_hop_id: "relay-hop".into(),
                hop: CaptureProvenanceHopV1 {
                    producer: producer.into(),
                    producer_version: None,
                    mode: CaptureModeV1::Relayed,
                    observed_at_unix_ms: UnixMillis(BASE + 200),
                    attributes: BTreeMap::new(),
                },
            }),
        )
    };
    assert_eq!(
        runtime
            .handle(hop("hop-1", "relay"))
            .unwrap()
            .messages
            .len(),
        1
    );
    assert!(runtime
        .handle(hop("hop-2", "relay"))
        .unwrap()
        .messages
        .is_empty());
    assert_eq!(
        rejection(&runtime.handle(hop("hop-3", "other")).unwrap()).code,
        "conflict"
    );

    let gap = discontinuity("session", "gap", "gap-id", "part", "mic", 1, 2);
    runtime.handle(gap).unwrap();
    let gap_conflict = discontinuity("session", "gap-conflict", "gap-id", "part", "mic", 2, 3);
    assert_eq!(
        rejection(&runtime.handle(gap_conflict).unwrap()).code,
        "conflict"
    );
}

#[test]
fn create_key_conflicts_across_sessions_without_creating_a_second_log() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("one", "create-one", "global-key", &[("mic", "s")]))
        .unwrap();
    let error = runtime
        .handle(create("two", "create-two", "global-key", &[("mic", "s")]))
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CreateIdempotencyConflict {
            existing_session_id,
            ..
        } if existing_session_id == SessionId::from("one")
    ));
    assert!(runtime
        .storage()
        .snapshot(&SessionId::from("two"))
        .unwrap()
        .is_none());
}

#[test]
fn reconnect_replays_original_envelopes_and_rejects_future_cursors_deterministically() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    runtime
        .handle(chunk("session", "chunk-2", "part", "mic", 2, "2"))
        .unwrap();
    runtime
        .handle(chunk("session", "chunk-0", "part", "mic", 0, "0"))
        .unwrap();
    let before = runtime
        .storage()
        .snapshot(&SessionId::from("session"))
        .unwrap()
        .unwrap()
        .events()
        .to_vec();

    let resume = client(
        "session",
        "resume",
        500,
        ClientMessageBodyV1::ResumeSession(ResumeSessionV1 {
            after_server_sequence: Some(0),
        }),
    );
    let replay = runtime.handle(resume.clone()).unwrap();
    assert_eq!(&replay.messages[..before.len() - 1], &before[1..]);
    let completed = match &replay.messages.last().unwrap().body {
        ServerMessageBodyV1::ReplayCompleted(value) => value,
        other => panic!("expected replay_completed, got {other:?}"),
    };
    assert_eq!(
        completed.replayed_through_server_sequence,
        before.last().map(|event| event.sequence)
    );
    assert_eq!(runtime.handle(resume).unwrap().messages, replay.messages);

    let future = client(
        "session",
        "future-cursor",
        600,
        ClientMessageBodyV1::ResumeSession(ResumeSessionV1 {
            after_server_sequence: Some(999),
        }),
    );
    let rejected = runtime.handle(future.clone()).unwrap();
    assert_eq!(rejection(&rejected).code, "replay_unavailable");
    assert_eq!(runtime.handle(future).unwrap().messages, rejected.messages);

    let snapshot = runtime
        .storage()
        .snapshot(&SessionId::from("session"))
        .unwrap()
        .unwrap();
    for (sequence, event) in snapshot.events().iter().enumerate() {
        assert_eq!(sequence as u64, event.sequence);
    }
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    let decoded: StoredSessionV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn close_rejects_boundaries_that_exclude_durable_chunks_or_gaps() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    runtime
        .handle(chunk("session", "chunk-2", "part", "mic", 2, "a"))
        .unwrap();
    let too_low = runtime
        .handle(close("session", "close-low", "part", &[("mic", 2)]))
        .unwrap();
    assert_eq!(rejection(&too_low).code, "invalid_transition");

    runtime
        .handle(discontinuity(
            "session", "gap", "gap-id", "part", "mic", 3, 5,
        ))
        .unwrap();
    let excludes_gap = runtime
        .handle(close("session", "close-gap", "part", &[("mic", 4)]))
        .unwrap();
    assert_eq!(rejection(&excludes_gap).code, "invalid_transition");
}

#[test]
fn empty_discontinuity_records_capture_time_without_covering_a_sequence_gap() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    let empty = runtime
        .handle(discontinuity(
            "session",
            "empty-gap",
            "empty-gap-id",
            "part",
            "mic",
            1,
            1,
        ))
        .unwrap();
    assert_eq!(ack(&empty).durable_through_sequence, 0);
    assert!(ack(&empty).durable_out_of_order.is_empty());

    let pending = runtime
        .handle(close("session", "close", "part", &[("mic", 2)]))
        .unwrap();
    assert!(pending.messages.is_empty());
    let still_pending = runtime
        .handle(chunk("session", "chunk-0", "part", "mic", 0, "a"))
        .unwrap();
    assert!(!still_pending
        .messages
        .iter()
        .any(|event| matches!(event.body, ServerMessageBodyV1::SegmentFinalized(_))));

    let covered = runtime
        .handle(discontinuity(
            "session",
            "real-gap",
            "real-gap-id",
            "part",
            "mic",
            1,
            2,
        ))
        .unwrap();
    assert_eq!(ack(&covered).durable_through_sequence, 2);
    assert!(covered
        .messages
        .iter()
        .any(|event| matches!(event.body, ServerMessageBodyV1::SegmentFinalized(_))));
}

#[test]
fn rejected_commands_do_not_create_phantom_segments_or_poison_finalize() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    let mut overflow = chunk("session", "overflow", "phantom", "mic", 0, "a");
    let ClientMessageBodyV1::AudioChunk(value) = &mut overflow.body else {
        unreachable!();
    };
    value.starts_at_ms = SessionMillis(MAX_SAFE_JSON_INTEGER);
    assert_eq!(
        rejection(&runtime.handle(overflow).unwrap()).code,
        "invalid_message"
    );

    let finalized = runtime
        .handle(finalize("session", "finalize", &[]))
        .unwrap();
    assert!(finalized
        .messages
        .iter()
        .any(|event| matches!(event.body, ServerMessageBodyV1::SessionFinalized(_))));
}

#[test]
fn chunks_and_nonempty_discontinuities_cannot_claim_the_same_sequence() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    runtime
        .handle(discontinuity(
            "session", "gap", "gap-1", "part", "mic", 1, 3,
        ))
        .unwrap();
    assert_eq!(
        rejection(
            &runtime
                .handle(chunk("session", "chunk-1", "part", "mic", 1, "a"))
                .unwrap()
        )
        .code,
        "sequence_conflict"
    );
    assert_eq!(
        rejection(
            &runtime
                .handle(discontinuity(
                    "session", "overlap", "gap-2", "part", "mic", 2, 4
                ))
                .unwrap()
        )
        .code,
        "sequence_conflict"
    );

    runtime
        .handle(chunk("session", "chunk-0", "other", "mic", 0, "b"))
        .unwrap();
    assert_eq!(
        rejection(
            &runtime
                .handle(discontinuity(
                    "session",
                    "over-audio",
                    "gap-3",
                    "other",
                    "mic",
                    0,
                    1
                ))
                .unwrap()
        )
        .code,
        "sequence_conflict"
    );
}

#[test]
fn declared_digests_and_temporal_boundaries_are_enforced() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    let mut bad_digest = chunk("session", "bad-digest", "part", "mic", 0, "a");
    let ClientMessageBodyV1::AudioChunk(value) = &mut bad_digest.body else {
        unreachable!();
    };
    value.payload[0] ^= 1;
    assert_eq!(
        rejection(&runtime.handle(bad_digest).unwrap()).code,
        "digest_mismatch"
    );

    runtime
        .handle(chunk("session", "chunk-0", "part", "mic", 0, "a"))
        .unwrap();
    assert!(runtime
        .handle(close("session", "close", "part", &[("mic", 2)]))
        .unwrap()
        .messages
        .is_empty());
    let mut too_late = chunk("session", "chunk-1", "part", "mic", 1, "b");
    let ClientMessageBodyV1::AudioChunk(value) = &mut too_late.body else {
        unreachable!();
    };
    value.starts_at_ms = SessionMillis(950);
    value.duration_ms = DurationMillis(100);
    assert_eq!(
        rejection(&runtime.handle(too_late).unwrap()).code,
        "invalid_transition"
    );
}

#[test]
fn finalize_rejects_close_ids_that_are_already_consumed_or_self_referential() {
    let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
    runtime
        .handle(create("session", "create", "key", &[("mic", "s")]))
        .unwrap();
    assert_eq!(
        rejection(
            &runtime
                .handle(finalize("session", "finalize-a", &[("part", "create")]))
                .unwrap()
        )
        .code,
        "invalid_transition"
    );
    assert_eq!(
        rejection(
            &runtime
                .handle(finalize("session", "finalize-b", &[("part", "finalize-b")]))
                .unwrap()
        )
        .code,
        "invalid_transition"
    );
}
