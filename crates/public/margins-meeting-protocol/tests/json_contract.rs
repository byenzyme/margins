use margins_meeting_protocol::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn as_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("contract value should serialize")
}

#[test]
fn create_session_json_shape_is_stable() {
    let message = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "msg-create".into(),
        session_id: "session-mobile".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000),
        body: ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: "idem-create".into(),
            started_at_unix_ms: UnixMillis(1_800_000_000_000),
            title: None,
            sources: vec![CaptureSourceV1 {
                source_id: "source-mic".into(),
                kind: CaptureSourceKindV1::Microphone,
                label: Some("Phone mic".into()),
                external_id: None,
            }],
            lanes: vec![CaptureLaneV1 {
                lane_id: "lane-mic".into(),
                source_ids: vec!["source-mic".into()],
                label: None,
                format: AudioFormatV1 {
                    codec: AudioCodecV1::Opus,
                    container: AudioContainerV1::Ogg,
                    sample_rate_hz: 48_000,
                    channel_count: 1,
                },
            }],
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "margins-mobile".into(),
                    producer_version: None,
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: UnixMillis(1_800_000_000_000),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    };

    assert_eq!(
        as_json(&message),
        json!({
            "protocol_version": 1,
            "message_id": "msg-create",
            "session_id": "session-mobile",
            "sent_at_unix_ms": 1_800_000_000_000_u64,
            "body": {
                "type": "create_session",
                "payload": {
                    "idempotency_key": "idem-create",
                    "started_at_unix_ms": 1_800_000_000_000_u64,
                    "sources": [{
                        "source_id": "source-mic",
                        "kind": "microphone",
                        "label": "Phone mic"
                    }],
                    "lanes": [{
                        "lane_id": "lane-mic",
                        "source_ids": ["source-mic"],
                        "format": {
                            "codec": "opus",
                            "container": "ogg",
                            "sample_rate_hz": 48000,
                            "channel_count": 1
                        }
                    }],
                    "provenance": {"hops": [{
                        "producer": "margins-mobile",
                        "mode": "live",
                        "observed_at_unix_ms": 1_800_000_000_000_u64
                    }]}
                }
            }
        })
    );
}

#[test]
fn chunk_and_ack_json_shapes_lock_ordering_contract() {
    let chunk = ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "msg-chunk-5".into(),
        session_id: "session-1".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_005_000),
        body: ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "segment-1".into(),
            lane_id: "lane-mic".into(),
            sequence: 5,
            starts_at_ms: SessionMillis(5_000),
            duration_ms: DurationMillis(1_000),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: "abcd".into(),
            },
            payload: vec![0, 127, 255],
        }),
    };
    assert_eq!(
        as_json(&chunk),
        json!({
            "protocol_version": 1,
            "message_id": "msg-chunk-5",
            "session_id": "session-1",
            "sent_at_unix_ms": 1_800_000_005_000_u64,
            "body": {"type": "audio_chunk", "payload": {
                "segment_id": "segment-1",
                "lane_id": "lane-mic",
                "sequence": 5,
                "starts_at_ms": 5000,
                "duration_ms": 1000,
                "payload_digest": {"algorithm": "sha256", "hex": "abcd"},
                "payload": "AH//"
            }}
        })
    );

    let ack = ServerMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: "msg-ack".into(),
        session_id: "session-1".into(),
        sequence: 9,
        sent_at_unix_ms: UnixMillis(1_800_000_005_100),
        body: ServerMessageBodyV1::AudioAcknowledged(AudioAcknowledgementV1 {
            segment_id: "segment-1".into(),
            lane_id: "lane-mic".into(),
            durable_through_sequence: 4,
            durable_out_of_order: vec![SequenceRangeV1 {
                start: 5,
                end_exclusive: 6,
            }],
        }),
    };
    assert_eq!(
        as_json(&ack)["body"],
        json!({"type": "audio_acknowledged", "payload": {
            "segment_id": "segment-1",
            "lane_id": "lane-mic",
            "durable_through_sequence": 4,
            "durable_out_of_order": [{"start": 5, "end_exclusive": 6}]
        }})
    );
}

#[test]
fn output_event_discriminants_are_stable() {
    let events = [
        ServerMessageBodyV1::TranscriptPartial(TranscriptPartialV1 {
            segment_id: "s".into(),
            hypothesis_id: "h".into(),
            revision: 1,
            spans: vec![],
        }),
        ServerMessageBodyV1::TranscriptCommitted(TranscriptCommittedV1 {
            segment_id: "s".into(),
            commit_id: "c".into(),
            commit_sequence: 2,
            committed_through_ms: SessionMillis(800),
            spans: vec![],
        }),
        ServerMessageBodyV1::Memo(MemoEventV1 {
            memo_id: "m".into(),
            revision: 4,
            status: MemoStatusV1::Partial,
            markdown: "Working…".into(),
            based_on_commit_sequence: 2,
            generated_at_unix_ms: UnixMillis(1_800_000_000_000),
        }),
        ServerMessageBodyV1::ArtifactReady(ArtifactReadyV1 {
            artifact_id: "a".into(),
            revision: 1,
            kind: ArtifactKindV1::Memo,
            media_type: "text/markdown".into(),
            byte_length: 10,
            digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Blake3,
                hex: "abcd".into(),
            },
            locator: "memo-token".into(),
            generated_at_unix_ms: UnixMillis(1_800_000_000_100),
        }),
    ];
    let types: Vec<_> = events
        .iter()
        .map(|event| as_json(event)["type"].clone())
        .collect();
    assert_eq!(
        types,
        vec![
            json!("transcript_partial"),
            json!("transcript_committed"),
            json!("memo"),
            json!("artifact_ready")
        ]
    );
}

#[test]
fn every_message_discriminant_is_stable() {
    let digest = || ContentDigestV1 {
        algorithm: DigestAlgorithmV1::Sha256,
        hex: "00".into(),
    };
    let client_bodies = [
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: "i".into(),
            started_at_unix_ms: UnixMillis(0),
            title: None,
            sources: vec![],
            lanes: vec![],
            provenance: CaptureProvenanceV1 { hops: vec![] },
        }),
        ClientMessageBodyV1::ResumeSession(ResumeSessionV1 {
            after_server_sequence: Some(1),
        }),
        ClientMessageBodyV1::AppendProvenanceHop(AppendProvenanceHopV1 {
            provenance_hop_id: "p".into(),
            hop: CaptureProvenanceHopV1 {
                producer: "relay".into(),
                producer_version: None,
                mode: CaptureModeV1::Relayed,
                observed_at_unix_ms: UnixMillis(0),
                attributes: BTreeMap::new(),
            },
        }),
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "s".into(),
            lane_id: "l".into(),
            sequence: 0,
            starts_at_ms: SessionMillis(0),
            duration_ms: DurationMillis(0),
            payload_digest: digest(),
            payload: vec![],
        }),
        ClientMessageBodyV1::CaptureDiscontinuity(CaptureDiscontinuityV1 {
            discontinuity_id: "d".into(),
            segment_id: "s".into(),
            lane_id: "l".into(),
            sequence_range: SequenceRangeV1 {
                start: 0,
                end_exclusive: 1,
            },
            starts_at_ms: SessionMillis(0),
            duration_ms: DurationMillis(1),
            reason: DiscontinuityReasonV1::Unknown,
            detail: None,
        }),
        ClientMessageBodyV1::CaptureHealth(CaptureHealthV1 {
            observed_at_ms: SessionMillis(0),
            lane_id: None,
            state: CaptureHealthStateV1::Healthy,
            queued_audio_ms: None,
            dropped_chunk_count: 0,
            clock_skew_ms: None,
            detail: None,
        }),
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "s".into(),
            ended_at_ms: SessionMillis(0),
            lane_boundaries: vec![],
            reason: SegmentCloseReasonV1::Stop,
        }),
        ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(0),
            segment_closes: vec![],
            reason: SessionFinalizeReasonV1::Completed,
        }),
    ];
    let client_types: Vec<_> = client_bodies
        .iter()
        .map(|body| as_json(body)["type"].clone())
        .collect();
    assert_eq!(
        client_types,
        vec![
            json!("create_session"),
            json!("resume_session"),
            json!("append_provenance_hop"),
            json!("audio_chunk"),
            json!("capture_discontinuity"),
            json!("capture_health"),
            json!("close_segment"),
            json!("finalize_session"),
        ]
    );

    let server_bodies = [
        ServerMessageBodyV1::SessionCreated(SessionCreatedV1 {
            create_message_id: "m".into(),
            created_at_unix_ms: UnixMillis(0),
        }),
        ServerMessageBodyV1::ReplayCompleted(ReplayCompletedV1 {
            resume_message_id: "m".into(),
            replayed_through_server_sequence: None,
        }),
        ServerMessageBodyV1::ProvenanceHopRecorded(ProvenanceHopRecordedV1 {
            append_message_id: "m".into(),
            provenance_hop_id: "p".into(),
        }),
        ServerMessageBodyV1::CommandRejected(CommandRejectedV1 {
            rejected_message_id: "m".into(),
            code: "conflict".into(),
            retryable: false,
            message: None,
            details: BTreeMap::new(),
        }),
        ServerMessageBodyV1::AudioAcknowledged(AudioAcknowledgementV1 {
            segment_id: "s".into(),
            lane_id: "l".into(),
            durable_through_sequence: 0,
            durable_out_of_order: vec![],
        }),
        ServerMessageBodyV1::SegmentFinalized(SegmentFinalizedV1 {
            segment_id: "s".into(),
            close_message_id: "m".into(),
            finalized_at_unix_ms: UnixMillis(0),
            duration_ms: DurationMillis(0),
            lane_boundaries: vec![],
        }),
        ServerMessageBodyV1::SessionFinalized(SessionFinalizedV1 {
            finalize_message_id: "m".into(),
            finalized_at_unix_ms: UnixMillis(0),
            duration_ms: DurationMillis(0),
            segment_closes: vec![],
        }),
        ServerMessageBodyV1::TranscriptPartial(TranscriptPartialV1 {
            segment_id: "s".into(),
            hypothesis_id: "h".into(),
            revision: 0,
            spans: vec![],
        }),
        ServerMessageBodyV1::TranscriptCommitted(TranscriptCommittedV1 {
            segment_id: "s".into(),
            commit_id: "c".into(),
            commit_sequence: 0,
            committed_through_ms: SessionMillis(0),
            spans: vec![],
        }),
        ServerMessageBodyV1::Memo(MemoEventV1 {
            memo_id: "m".into(),
            revision: 0,
            status: MemoStatusV1::Partial,
            markdown: String::new(),
            based_on_commit_sequence: 0,
            generated_at_unix_ms: UnixMillis(0),
        }),
        ServerMessageBodyV1::ArtifactReady(ArtifactReadyV1 {
            artifact_id: "a".into(),
            revision: 0,
            kind: ArtifactKindV1::Other,
            media_type: "application/octet-stream".into(),
            byte_length: 0,
            digest: digest(),
            locator: "x".into(),
            generated_at_unix_ms: UnixMillis(0),
        }),
    ];
    let server_types: Vec<_> = server_bodies
        .iter()
        .map(|body| as_json(body)["type"].clone())
        .collect();
    assert_eq!(
        server_types,
        vec![
            json!("session_created"),
            json!("replay_completed"),
            json!("provenance_hop_recorded"),
            json!("command_rejected"),
            json!("audio_acknowledged"),
            json!("segment_finalized"),
            json!("session_finalized"),
            json!("transcript_partial"),
            json!("transcript_committed"),
            json!("memo"),
            json!("artifact_ready"),
        ]
    );
}

#[test]
fn v1_rejects_other_protocol_versions_and_unknown_message_types() {
    let wrong_version = json!({
        "protocol_version": 2,
        "message_id": "msg",
        "session_id": "session",
        "sent_at_unix_ms": 1,
        "body": {"type": "finalize_session", "payload": {
            "ended_at_ms": 10,
            "segment_closes": [],
            "reason": "completed"
        }}
    });
    let error = serde_json::from_value::<ClientMessageV1>(wrong_version)
        .expect_err("V1 DTO must reject version 2");
    assert!(error.to_string().contains("unsupported protocol version 2"));

    let unknown_type = json!({
        "protocol_version": 1,
        "message_id": "msg",
        "session_id": "session",
        "sent_at_unix_ms": 1,
        "body": {"type": "future_command", "payload": {}}
    });
    assert!(serde_json::from_value::<ClientMessageV1>(unknown_type).is_err());
}

#[test]
fn additive_unknown_fields_are_accepted_but_not_reemitted() {
    let value = json!({
        "protocol_version": 1,
        "message_id": "msg",
        "session_id": "session",
        "sent_at_unix_ms": 1,
        "future_envelope_field": true,
        "body": {"type": "capture_health", "payload": {
            "observed_at_ms": 10,
            "state": "healthy",
            "dropped_chunk_count": 0,
            "future_health_field": "ignored"
        }}
    });
    let parsed: ClientMessageV1 = serde_json::from_value(value).expect("additive fields work");
    let encoded = as_json(&parsed);
    assert!(encoded.get("future_envelope_field").is_none());
    assert!(encoded["body"]["payload"]
        .get("future_health_field")
        .is_none());
}

#[test]
fn unknown_server_event_types_roundtrip_without_poisoning_the_stream() {
    let value = json!({
        "protocol_version": 1,
        "message_id": "server-future",
        "session_id": "session",
        "sequence": 12,
        "sent_at_unix_ms": 1,
        "body": {
            "type": "future_runtime_event",
            "payload": {"nested": [1, true, "value"]}
        }
    });

    let parsed: ServerMessageV1 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(as_json(&parsed), value);
    assert!(matches!(parsed.body, ServerMessageBodyV1::Unknown(_)));
}
