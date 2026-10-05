use margins_meeting_protocol::*;
use serde::{de::DeserializeOwned, Serialize};
use std::{collections::BTreeMap, fmt::Debug};

fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let encoded = serde_json::to_vec(value).expect("value should serialize");
    let decoded = serde_json::from_slice::<T>(&encoded).expect("value should deserialize");
    assert_eq!(*value, decoded);
}

fn digest() -> ContentDigestV1 {
    ContentDigestV1 {
        algorithm: DigestAlgorithmV1::Sha256,
        hex: "ab".repeat(32),
    }
}

fn span(id: &str, text: &str) -> TranscriptSpanV1 {
    TranscriptSpanV1 {
        span_id: id.into(),
        starts_at_ms: SessionMillis(100),
        ends_at_ms: SessionMillis(900),
        speaker: Some("speaker-1".into()),
        text: text.into(),
        confidence_per_mille: Some(975),
    }
}

fn client(body: ClientMessageBodyV1, index: u64) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: format!("client-{index}").into(),
        session_id: "session-1".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000 + index),
        body,
    }
}

fn server(body: ServerMessageBodyV1, index: u64) -> ServerMessageV1 {
    ServerMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: format!("server-{index}").into(),
        session_id: "session-1".into(),
        sequence: index,
        sent_at_unix_ms: UnixMillis(1_800_000_001_000 + index),
        body,
    }
}

#[test]
fn every_client_message_variant_roundtrips() {
    let mut attributes = BTreeMap::new();
    attributes.insert("runtime".into(), "browser".into());

    let messages = vec![
        client(
            ClientMessageBodyV1::CreateSession(CreateSessionV1 {
                idempotency_key: "create-1".into(),
                started_at_unix_ms: UnixMillis(1_800_000_000_000),
                title: Some("Design review".into()),
                sources: vec![CaptureSourceV1 {
                    source_id: "mic".into(),
                    kind: CaptureSourceKindV1::Microphone,
                    label: Some("Host microphone".into()),
                    external_id: None,
                }],
                lanes: vec![CaptureLaneV1 {
                    lane_id: "mic-mono".into(),
                    source_ids: vec!["mic".into()],
                    label: None,
                    format: AudioFormatV1 {
                        codec: AudioCodecV1::Opus,
                        container: AudioContainerV1::Webm,
                        sample_rate_hz: 48_000,
                        channel_count: 1,
                    },
                }],
                provenance: CaptureProvenanceV1 {
                    hops: vec![CaptureProvenanceHopV1 {
                        producer: "margins-web".into(),
                        producer_version: Some("2.4.0".into()),
                        mode: CaptureModeV1::Live,
                        observed_at_unix_ms: UnixMillis(1_800_000_000_000),
                        attributes,
                    }],
                },
            }),
            0,
        ),
        client(
            ClientMessageBodyV1::ResumeSession(ResumeSessionV1 {
                after_server_sequence: Some(4),
            }),
            1,
        ),
        client(
            ClientMessageBodyV1::AppendProvenanceHop(AppendProvenanceHopV1 {
                provenance_hop_id: "relay-vps-1".into(),
                hop: CaptureProvenanceHopV1 {
                    producer: "margins-relay".into(),
                    producer_version: Some("1.0.0".into()),
                    mode: CaptureModeV1::Relayed,
                    observed_at_unix_ms: UnixMillis(1_800_000_000_010),
                    attributes: BTreeMap::new(),
                },
            }),
            2,
        ),
        client(
            ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                segment_id: "segment-1".into(),
                lane_id: "mic-mono".into(),
                sequence: 7,
                starts_at_ms: SessionMillis(7_000),
                duration_ms: DurationMillis(1_000),
                payload_digest: digest(),
                payload: vec![0, 1, 127, 255],
            }),
            3,
        ),
        client(
            ClientMessageBodyV1::CaptureDiscontinuity(CaptureDiscontinuityV1 {
                discontinuity_id: "gap-1".into(),
                segment_id: "segment-1".into(),
                lane_id: "mic-mono".into(),
                sequence_range: SequenceRangeV1 {
                    start: 8,
                    end_exclusive: 9,
                },
                starts_at_ms: SessionMillis(8_000),
                duration_ms: DurationMillis(1_000),
                reason: DiscontinuityReasonV1::NetworkLoss,
                detail: Some("relay queue expired".into()),
            }),
            4,
        ),
        client(
            ClientMessageBodyV1::CaptureHealth(CaptureHealthV1 {
                observed_at_ms: SessionMillis(9_250),
                lane_id: Some("mic-mono".into()),
                state: CaptureHealthStateV1::Recovered,
                queued_audio_ms: Some(DurationMillis(250)),
                dropped_chunk_count: 1,
                clock_skew_ms: Some(-12),
                detail: None,
            }),
            5,
        ),
        client(
            ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                segment_id: "segment-1".into(),
                ended_at_ms: SessionMillis(10_000),
                lane_boundaries: vec![LaneBoundaryV1 {
                    lane_id: "mic-mono".into(),
                    next_sequence: 10,
                }],
                reason: SegmentCloseReasonV1::Pause,
            }),
            6,
        ),
        client(
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(10_000),
                segment_closes: vec![SegmentCloseReferenceV1 {
                    segment_id: "segment-1".into(),
                    close_message_id: "client-6".into(),
                }],
                reason: SessionFinalizeReasonV1::Completed,
            }),
            7,
        ),
    ];

    for message in messages {
        roundtrip(&message);
    }
}

#[test]
fn every_server_message_variant_roundtrips() {
    let boundary = || LaneBoundaryV1 {
        lane_id: "mic-mono".into(),
        next_sequence: 10,
    };

    let messages = vec![
        server(
            ServerMessageBodyV1::SessionCreated(SessionCreatedV1 {
                create_message_id: "client-0".into(),
                created_at_unix_ms: UnixMillis(1_800_000_000_100),
            }),
            0,
        ),
        server(
            ServerMessageBodyV1::ReplayCompleted(ReplayCompletedV1 {
                resume_message_id: "client-1".into(),
                replayed_through_server_sequence: Some(0),
            }),
            1,
        ),
        server(
            ServerMessageBodyV1::ProvenanceHopRecorded(ProvenanceHopRecordedV1 {
                append_message_id: "client-2".into(),
                provenance_hop_id: "relay-vps-1".into(),
            }),
            2,
        ),
        server(
            ServerMessageBodyV1::CommandRejected(CommandRejectedV1 {
                rejected_message_id: "client-bad".into(),
                code: "invalid_message".into(),
                retryable: false,
                message: Some("invalid boundary".into()),
                details: BTreeMap::new(),
            }),
            3,
        ),
        server(
            ServerMessageBodyV1::AudioAcknowledged(AudioAcknowledgementV1 {
                segment_id: "segment-1".into(),
                lane_id: "mic-mono".into(),
                durable_through_sequence: 5,
                durable_out_of_order: vec![SequenceRangeV1 {
                    start: 6,
                    end_exclusive: 8,
                }],
            }),
            4,
        ),
        server(
            ServerMessageBodyV1::SegmentFinalized(SegmentFinalizedV1 {
                segment_id: "segment-1".into(),
                close_message_id: "client-4".into(),
                finalized_at_unix_ms: UnixMillis(1_800_000_011_000),
                duration_ms: DurationMillis(10_000),
                lane_boundaries: vec![boundary()],
            }),
            5,
        ),
        server(
            ServerMessageBodyV1::SessionFinalized(SessionFinalizedV1 {
                finalize_message_id: "client-5".into(),
                finalized_at_unix_ms: UnixMillis(1_800_000_012_000),
                duration_ms: DurationMillis(10_000),
                segment_closes: vec![SegmentCloseReferenceV1 {
                    segment_id: "segment-1".into(),
                    close_message_id: "client-6".into(),
                }],
            }),
            6,
        ),
        server(
            ServerMessageBodyV1::TranscriptPartial(TranscriptPartialV1 {
                segment_id: "segment-1".into(),
                hypothesis_id: "hypothesis-1".into(),
                revision: 2,
                spans: vec![span("span-p1", "hello wor")],
            }),
            7,
        ),
        server(
            ServerMessageBodyV1::TranscriptCommitted(TranscriptCommittedV1 {
                segment_id: "segment-1".into(),
                commit_id: "commit-1".into(),
                commit_sequence: 0,
                committed_through_ms: SessionMillis(900),
                spans: vec![span("span-c1", "hello world")],
            }),
            8,
        ),
        server(
            ServerMessageBodyV1::Memo(MemoEventV1 {
                memo_id: "memo-1".into(),
                revision: 3,
                status: MemoStatusV1::Final,
                markdown: "# Decisions\n\nShip it.".into(),
                based_on_commit_sequence: 8,
                generated_at_unix_ms: UnixMillis(1_800_000_013_000),
            }),
            9,
        ),
        server(
            ServerMessageBodyV1::ArtifactReady(ArtifactReadyV1 {
                artifact_id: "artifact-1".into(),
                revision: 1,
                kind: ArtifactKindV1::Transcript,
                media_type: "application/json".into(),
                byte_length: 1_024,
                digest: digest(),
                locator: "objects/session-1/transcript.json".into(),
                generated_at_unix_ms: UnixMillis(1_800_000_014_000),
            }),
            10,
        ),
    ];

    for message in messages {
        roundtrip(&message);
    }
}

#[test]
fn all_closed_enums_roundtrip() {
    for value in [
        CaptureSourceKindV1::Microphone,
        CaptureSourceKindV1::SystemAudio,
        CaptureSourceKindV1::RemoteParticipant,
        CaptureSourceKindV1::Media,
        CaptureSourceKindV1::Mixed,
    ] {
        roundtrip(&value);
    }
    for value in [
        DiscontinuityReasonV1::QueueOverflow,
        DiscontinuityReasonV1::DeviceUnavailable,
        DiscontinuityReasonV1::PermissionRevoked,
        DiscontinuityReasonV1::ClockReset,
        DiscontinuityReasonV1::SuspendResume,
        DiscontinuityReasonV1::NetworkLoss,
        DiscontinuityReasonV1::SourceEnded,
        DiscontinuityReasonV1::Unknown,
    ] {
        roundtrip(&value);
    }
    for value in [
        CaptureHealthStateV1::Healthy,
        CaptureHealthStateV1::Degraded,
        CaptureHealthStateV1::Stalled,
        CaptureHealthStateV1::Recovered,
        CaptureHealthStateV1::Ended,
    ] {
        roundtrip(&value);
    }
    for value in [
        AudioCodecV1::PcmS16Le,
        AudioCodecV1::PcmF32Le,
        AudioCodecV1::Opus,
        AudioCodecV1::AacLc,
    ] {
        roundtrip(&value);
    }
    for value in [
        AudioContainerV1::Raw,
        AudioContainerV1::Webm,
        AudioContainerV1::Ogg,
        AudioContainerV1::Mp4,
        AudioContainerV1::PacketStream,
    ] {
        roundtrip(&value);
    }
    for value in [DigestAlgorithmV1::Sha256, DigestAlgorithmV1::Blake3] {
        roundtrip(&value);
    }
    for value in [
        CaptureModeV1::Live,
        CaptureModeV1::Imported,
        CaptureModeV1::Relayed,
    ] {
        roundtrip(&value);
    }
    for value in [
        SegmentCloseReasonV1::Pause,
        SegmentCloseReasonV1::Rollover,
        SegmentCloseReasonV1::SourceEnded,
        SegmentCloseReasonV1::Stop,
        SegmentCloseReasonV1::Error,
    ] {
        roundtrip(&value);
    }
    for value in [
        SessionFinalizeReasonV1::Completed,
        SessionFinalizeReasonV1::Cancelled,
        SessionFinalizeReasonV1::SourceEnded,
        SessionFinalizeReasonV1::Error,
    ] {
        roundtrip(&value);
    }
    for value in [MemoStatusV1::Partial, MemoStatusV1::Final] {
        roundtrip(&value);
    }
    for value in [
        ArtifactKindV1::Transcript,
        ArtifactKindV1::Memo,
        ArtifactKindV1::Audio,
        ArtifactKindV1::Captions,
        ArtifactKindV1::Other,
    ] {
        roundtrip(&value);
    }
}

fn opus_block(start: bool, end: bool, source_start: u64, source_frames: u32) -> OpusPacketBlockV1 {
    OpusPacketBlockV1 {
        stream_start: start,
        stream_end: end,
        pre_skip_48k: if start { 312 } else { 0 },
        sample_rate_hz: OPUS_PACKET_STREAM_SAMPLE_RATE_HZ_V1,
        source_start_frame: source_start,
        source_frame_count: source_frames,
        frame_samples: OPUS_PACKET_FRAME_SAMPLES_V1,
        packets: vec![vec![0xf8, 0xff, 0xfe]],
    }
}

#[test]
fn opus_packet_stream_framing_roundtrips_and_rejects_hostile_counts_before_allocation() {
    let encoded = opus_block(true, true, 0, 216).encode().unwrap();
    let decoded = decode_opus_packet_blocks_v1(&encoded).unwrap();
    assert_eq!(decoded, vec![opus_block(true, true, 0, 216)]);
    let summary = validate_opus_packet_stream_v1(&encoded).unwrap();
    assert_eq!(summary.source_frame_count, 216);
    assert_eq!(summary.pre_skip_48k, 312);

    let mut hostile = encoded.clone();
    hostile[28..30].copy_from_slice(&u16::MAX.to_le_bytes());
    let error = decode_opus_packet_blocks_v1(&hostile).unwrap_err();
    assert!(error.contains("packet count"));

    let mut truncated = encoded;
    truncated.pop();
    assert!(decode_opus_packet_blocks_v1(&truncated).is_err());

    let mut bad_magic = opus_block(true, true, 0, 216).encode().unwrap();
    bad_magic[0] ^= 0xff;
    assert!(decode_opus_packet_blocks_v1(&bad_magic)
        .unwrap_err()
        .contains("magic/version"));

    let mut packet_length_bomb = opus_block(true, true, 0, 216).encode().unwrap();
    packet_length_bomb[32..34].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_opus_packet_blocks_v1(&packet_length_bomb)
        .unwrap_err()
        .contains("packet payload"));
}

#[test]
fn opus_complete_stream_rejects_gaps_missing_markers_and_terminal_overrun() {
    let first = opus_block(true, false, 0, 320).encode().unwrap();
    let mut second = opus_block(false, true, 321, 216);
    let mut bytes = first.clone();
    bytes.extend(second.encode().unwrap());
    assert!(validate_opus_packet_stream_v1(&bytes)
        .unwrap_err()
        .contains("gap or overlap"));

    second.source_start_frame = 319;
    let mut overlap = first.clone();
    overlap.extend(second.encode().unwrap());
    assert!(validate_opus_packet_stream_v1(&overlap)
        .unwrap_err()
        .contains("gap or overlap"));

    second.source_start_frame = 320;
    let mut missing_start = opus_block(false, true, 0, 216);
    missing_start.pre_skip_48k = 0;
    assert!(validate_opus_packet_stream_v1(&missing_start.encode().unwrap()).is_err());

    assert!(validate_opus_packet_stream_v1(&first)
        .unwrap_err()
        .contains("missing END"));

    let mut after_end = opus_block(true, true, 0, 216).encode().unwrap();
    after_end.extend(second.encode().unwrap());
    assert!(validate_opus_packet_stream_v1(&after_end)
        .unwrap_err()
        .contains("after END"));
}

#[test]
fn binary_audio_batch_keeps_json_metadata_separate_from_payloads() {
    let command = client(
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "segment-1".into(),
            lane_id: "mic".into(),
            sequence: 0,
            starts_at_ms: SessionMillis(0),
            duration_ms: DurationMillis(20),
            payload_digest: digest(),
            payload: vec![7, 8, 9],
        }),
        20,
    );
    let batch = AudioChunkBatchV1 {
        commands: vec![command],
    };
    let encoded = batch.encode(8, 1024).unwrap();
    let decoded = AudioChunkBatchV1::decode(&encoded, 8, 1024).unwrap();
    assert_eq!(decoded, batch);

    let mut trailing = encoded;
    trailing.push(0);
    assert!(AudioChunkBatchV1::decode(&trailing, 8, 1024).is_err());
}

#[test]
fn sequence_ranges_are_half_open() {
    let range = SequenceRangeV1 {
        start: 3,
        end_exclusive: 5,
    };
    assert!(!range.is_empty());
    assert!(!range.contains(2));
    assert!(range.contains(3));
    assert!(range.contains(4));
    assert!(!range.contains(5));
    assert!(SequenceRangeV1 {
        start: 5,
        end_exclusive: 5
    }
    .is_empty());
    let inverted = SequenceRangeV1 {
        start: 6,
        end_exclusive: 5,
    };
    assert!(!inverted.is_empty());
    assert!(!inverted.is_valid());
    assert!(inverted.validate().is_err());
}

#[test]
fn ack_validation_rejects_noncanonical_ranges_and_reports_coverage() {
    let valid = AudioAcknowledgementV1 {
        segment_id: "segment".into(),
        lane_id: "lane".into(),
        durable_through_sequence: 4,
        durable_out_of_order: vec![SequenceRangeV1 {
            start: 6,
            end_exclusive: 8,
        }],
    };
    assert!(valid.validate().is_ok());
    assert!(valid.covers(3));
    assert!(!valid.covers(4));
    assert!(valid.covers(7));

    let adjacent = AudioAcknowledgementV1 {
        durable_out_of_order: vec![SequenceRangeV1 {
            start: 4,
            end_exclusive: 5,
        }],
        ..valid
    };
    assert!(adjacent.validate().is_err());
}
