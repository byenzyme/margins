use margins_meeting_protocol::*;
use margins_workflows::{
    workspace::ensure_service_workspace,
    workspace_service::{ServicePrincipal, WorkspaceService},
};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

const BASE: u64 = 1_800_000_000_000;

fn command(session: &str, message: &str, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: message.into(),
        session_id: session.into(),
        sent_at_unix_ms: UnixMillis(BASE),
        body,
    }
}

fn create(session: &str) -> ClientMessageV1 {
    let sources = ["mic", "system"]
        .into_iter()
        .map(|source| CaptureSourceV1 {
            source_id: source.into(),
            kind: if source == "mic" {
                CaptureSourceKindV1::Microphone
            } else {
                CaptureSourceKindV1::SystemAudio
            },
            label: None,
            external_id: None,
        })
        .collect();
    let lanes = ["mic", "system"]
        .into_iter()
        .map(|lane| CaptureLaneV1 {
            lane_id: lane.into(),
            source_ids: vec![lane.into()],
            label: None,
            format: AudioFormatV1 {
                codec: AudioCodecV1::PcmS16Le,
                container: AudioContainerV1::Raw,
                sample_rate_hz: 48_000,
                channel_count: 1,
            },
        })
        .collect();
    command(
        session,
        "create-1",
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: "reserve-1".to_string(),
            started_at_unix_ms: UnixMillis(BASE),
            title: Some("Composed remote capture".to_string()),
            sources,
            lanes,
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "test-native-client".to_string(),
                    producer_version: Some("1".to_string()),
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: UnixMillis(BASE),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    )
}

fn chunk(session: &str, message: &str, lane: &str, sequence: u64) -> ClientMessageV1 {
    let sample = if lane == "mic" {
        0x1100_i16 + sequence as i16
    } else {
        0x2200_i16 + sequence as i16
    };
    let payload = (0..4_800)
        .flat_map(|_| sample.to_le_bytes())
        .collect::<Vec<_>>();
    command(
        session,
        message,
        ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
            segment_id: "segment-1".into(),
            lane_id: lane.into(),
            sequence,
            starts_at_ms: SessionMillis(sequence * 100),
            duration_ms: DurationMillis(100),
            payload_digest: ContentDigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                hex: format!("{:x}", Sha256::digest(&payload)),
            },
            payload,
        }),
    )
}

fn close(session: &str) -> ClientMessageV1 {
    command(
        session,
        "close-1",
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "segment-1".into(),
            ended_at_ms: SessionMillis(200),
            lane_boundaries: ["mic", "system"]
                .into_iter()
                .map(|lane| LaneBoundaryV1 {
                    lane_id: lane.into(),
                    next_sequence: 2,
                })
                .collect(),
            reason: SegmentCloseReasonV1::Stop,
        }),
    )
}

fn finalize(session: &str) -> ClientMessageV1 {
    command(
        session,
        "finalize-1",
        ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(200),
            segment_closes: vec![SegmentCloseReferenceV1 {
                segment_id: "segment-1".into(),
                close_message_id: "close-1".into(),
            }],
            reason: SessionFinalizeReasonV1::Completed,
        }),
    )
}

#[test]
fn composed_service_is_the_same_canonical_store_across_retry_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace = ensure_service_workspace(
        &temp.path().join("state"),
        "team",
        Some("Team"),
        &notes,
        &captures,
    )
    .unwrap();
    let service = WorkspaceService::open("host-a", workspace.clone()).unwrap();
    let owner = ServicePrincipal::full("client-a", "team");
    let other = ServicePrincipal::full("client-b", "team");
    let reservation = service
        .reserve_session(&owner, create("capture-a"))
        .unwrap();

    assert!(service
        .execute_capture(
            &other,
            &reservation.producer_token,
            chunk("capture-a", "forged", "mic", 0)
        )
        .is_err());
    // Out of order, followed by an exact retry standing in for a lost ACK.
    service
        .execute_capture(
            &owner,
            &reservation.producer_token,
            chunk("capture-a", "mic-1", "mic", 1),
        )
        .unwrap();
    let exact = chunk("capture-a", "mic-0", "mic", 0);
    let first = service
        .execute_capture(&owner, &reservation.producer_token, exact.clone())
        .unwrap();
    let replay = service
        .execute_capture(&owner, &reservation.producer_token, exact)
        .unwrap();
    assert_eq!(first.messages, replay.messages);
    assert!(!first.idempotent_replay);
    assert!(replay.idempotent_replay);
    for lane in ["system"] {
        for sequence in 0..2 {
            service
                .execute_capture(
                    &owner,
                    &reservation.producer_token,
                    chunk("capture-a", &format!("{lane}-{sequence}"), lane, sequence),
                )
                .unwrap();
        }
    }
    service
        .execute_capture(&owner, &reservation.producer_token, close("capture-a"))
        .unwrap();

    drop(service);
    let restarted = WorkspaceService::open("host-a", workspace).unwrap();
    restarted
        .execute_capture(&owner, &reservation.producer_token, finalize("capture-a"))
        .unwrap();

    let page = restarted.sessions(&owner, None, 10).unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].session_id.as_ref(), "capture-a");
    assert!(page.sessions[0].input_finalized);
    assert!(restarted
        .repository_record(&owner, &SessionId("capture-a".to_string()))
        .unwrap()
        .is_some());
    let artifacts =
        margins_store::canonical::list_session_artifacts(&captures.join(".margins"), "capture-a")
            .unwrap();
    assert_eq!(artifacts.len(), 2);
    for (kind, expected) in [
        ("audio_mic_pcm", 0x1100_i16),
        ("audio_system_pcm", 0x2200_i16),
    ] {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.kind == kind)
            .unwrap();
        let bytes = std::fs::read(captures.join(&artifact.path)).unwrap();
        let samples = bytes
            .chunks_exact(2)
            .map(|value| i16::from_le_bytes([value[0], value[1]]))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 9_600);
        assert_eq!(samples[0], expected);
        assert_eq!(samples[4_800], expected + 1);
    }
    let meta = margins_store::canonical::get_session_meta(&captures.join(".margins"), "capture-a")
        .unwrap();
    assert_eq!(meta.segments[0].duration_secs, Some(0.2));
    assert!(captures.join(".margins/sessions.sqlite").is_file());
    assert!(!captures.join(".margins/meeting-runtime.sqlite").exists());
    assert!(!temp.path().join("unrelated/.margins").exists());
}

#[test]
fn memo_source_and_import_authorization_are_independent_and_retry_safe() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host-a", workspace).unwrap();
    let owner = ServicePrincipal::full("owner", "team");
    let reservation = service
        .reserve_session(&owner, create("capture-a"))
        .unwrap();
    let initial = service
        .memo(&owner, &SessionId("capture-a".into()))
        .unwrap();
    let request = WorkspaceMemoUpdateV1 {
        request_id: "memo-1".to_string(),
        expected_revision: initial.revision,
        observed_at_ms: SessionMillis(12_345),
        paused: false,
        text: "A delayed observation".to_string(),
    };
    let written = service
        .update_memo(&owner, &SessionId("capture-a".into()), &request)
        .unwrap();
    let replay = service
        .update_memo(&owner, &SessionId("capture-a".into()), &request)
        .unwrap();
    assert_eq!(written, replay);
    assert_eq!(written.lines[0].created_secs, 12.345);
    assert!(service
        .link_note(
            &owner,
            &SessionId("capture-a".into()),
            "home",
            "../escape.md",
            None,
            0
        )
        .is_err());

    service
        .execute_capture(
            &owner,
            &reservation.producer_token,
            chunk("capture-a", "mic-0", "mic", 0),
        )
        .unwrap();
    let phone = ServicePrincipal::upload_only("phone", "team");
    assert!(service.sessions(&phone, None, 10).is_err());
    let receipt = service
        .import_finished_file(
            &phone,
            "upload-1",
            "phone-a",
            "../../memo.m4a",
            Some("Memo"),
            b"compressed-audio",
        )
        .unwrap();
    let replay = service
        .import_finished_file(
            &phone,
            "upload-1",
            "phone-a",
            "different-name.m4a",
            Some("Memo"),
            b"compressed-audio",
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(receipt.stored_path, replay.stored_path);
    assert!(service
        .import_receipt(&owner, "upload-1")
        .unwrap()
        .is_none());
    assert_eq!(
        service
            .import_receipt(&phone, "upload-1")
            .unwrap()
            .unwrap()
            .session_id,
        "phone-a"
    );
    assert!(captures.join(&receipt.stored_path).is_file());
}
