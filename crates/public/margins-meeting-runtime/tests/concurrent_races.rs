use margins_meeting_protocol::*;
use margins_meeting_runtime::*;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Barrier},
    thread,
};

fn message(id: &str, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: id.into(),
        session_id: "race-session".into(),
        sent_at_unix_ms: UnixMillis(1_800_000_000_000),
        body,
    }
}

fn create() -> ClientMessageV1 {
    message(
        "create",
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: "race-key".into(),
            started_at_unix_ms: UnixMillis(1_800_000_000_000),
            title: None,
            sources: vec![CaptureSourceV1 {
                source_id: "source".into(),
                kind: CaptureSourceKindV1::RemoteParticipant,
                label: None,
                external_id: None,
            }],
            lanes: ["left", "right"]
                .into_iter()
                .map(|lane_id| CaptureLaneV1 {
                    lane_id: lane_id.into(),
                    source_ids: vec!["source".into()],
                    label: None,
                    format: AudioFormatV1 {
                        codec: AudioCodecV1::Opus,
                        container: AudioContainerV1::Webm,
                        sample_rate_hz: 48_000,
                        channel_count: 1,
                    },
                })
                .collect(),
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "mobile".into(),
                    producer_version: None,
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: UnixMillis(1_800_000_000_000),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    )
}

fn chunk(lane: &str, sequence: u64) -> ClientMessageV1 {
    let payload = vec![sequence as u8];
    message(
        &format!("{lane}-{sequence}"),
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "part".into(),
            lane_id: lane.into(),
            sequence,
            starts_at_ms: SessionMillis(sequence * 10),
            duration_ms: DurationMillis(10),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: format!("{:x}", Sha256::digest(&payload)),
            },
            payload,
        }),
    )
}

#[test]
fn concurrent_multilane_chunks_close_and_finalize_commit_once_without_lost_updates() {
    let runtime = Arc::new(MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new()));
    runtime.handle(create()).unwrap();

    let close = message(
        "close",
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "part".into(),
            ended_at_ms: SessionMillis(1_000),
            lane_boundaries: vec![
                LaneBoundaryV1 {
                    lane_id: "left".into(),
                    next_sequence: 16,
                },
                LaneBoundaryV1 {
                    lane_id: "right".into(),
                    next_sequence: 16,
                },
            ],
            reason: SegmentCloseReasonV1::Stop,
        }),
    );
    let finalize = message(
        "finalize",
        ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(1_000),
            segment_closes: vec![SegmentCloseReferenceV1 {
                segment_id: "part".into(),
                close_message_id: "close".into(),
            }],
            reason: SessionFinalizeReasonV1::Completed,
        }),
    );

    let mut commands = vec![close, finalize];
    for lane in ["left", "right"] {
        for sequence in (0..16).rev() {
            commands.push(chunk(lane, sequence));
        }
    }
    let handles: Vec<_> = commands
        .into_iter()
        .map(|command| {
            let runtime = Arc::clone(&runtime);
            thread::spawn(move || runtime.handle(command))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }

    let snapshot = runtime
        .storage()
        .snapshot(&SessionId::from("race-session"))
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.audio_chunks().count(), 32);
    assert_eq!(
        snapshot
            .events()
            .iter()
            .filter(|event| matches!(event.body, ServerMessageBodyV1::SegmentFinalized(_)))
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .events()
            .iter()
            .filter(|event| matches!(event.body, ServerMessageBodyV1::SessionFinalized(_)))
            .count(),
        1
    );
    for (sequence, event) in snapshot.events().iter().enumerate() {
        assert_eq!(event.sequence, sequence as u64);
    }

    for lane in ["left", "right"] {
        let final_ack = snapshot
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.body {
                ServerMessageBodyV1::AudioAcknowledged(ack)
                    if ack.lane_id == LaneId::from(lane) =>
                {
                    Some(ack)
                }
                _ => None,
            });
        let final_ack = final_ack.unwrap();
        assert_eq!(final_ack.durable_through_sequence, 16);
        assert!(final_ack.durable_out_of_order.is_empty());
    }
}

#[test]
fn concurrent_conflicting_writers_linearize_to_one_chunk_and_one_rejection() {
    for round in 0..32 {
        let runtime = Arc::new(MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new()));
        runtime.handle(create()).unwrap();

        let mut left = chunk("left", 0);
        left.message_id = format!("left-{round}").into();
        let mut right = chunk("left", 0);
        right.message_id = format!("right-{round}").into();
        let ClientMessageBodyV1::AudioChunk(value) = &mut right.body else {
            unreachable!();
        };
        value.payload = vec![round as u8, 99];
        value.payload_digest.hex = format!("{:x}", Sha256::digest(&value.payload));

        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = [left, right]
            .into_iter()
            .map(|command| {
                let runtime = Arc::clone(&runtime);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    runtime.handle(command).unwrap()
                })
            })
            .collect();
        barrier.wait();
        let responses: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            responses
                .iter()
                .flat_map(|response| &response.messages)
                .filter(|event| matches!(event.body, ServerMessageBodyV1::AudioAcknowledged(_)))
                .count(),
            1
        );
        assert_eq!(
            responses
                .iter()
                .flat_map(|response| &response.messages)
                .filter(|event| matches!(event.body, ServerMessageBodyV1::CommandRejected(_)))
                .count(),
            1
        );
        let snapshot = runtime
            .storage()
            .snapshot(&SessionId::from("race-session"))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.audio_chunks().count(), 1);
        assert_eq!(snapshot.events().len(), 3);
    }
}
