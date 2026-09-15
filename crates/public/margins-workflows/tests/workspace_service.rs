use margins_meeting_protocol::*;
use margins_workflows::{
    remote_workspace::{
        native_create_session_command, remote_opus_packet_stream_for_asr, DurableTransferSpool,
        NativeRemoteLane, NativeRemoteTransfer,
    },
    workspace::ensure_service_workspace,
    workspace_service::{
        ScopedCredentialStore, ServicePrincipal, WorkspaceService, OP_IMPORT_WRITE, OP_SESSION_READ,
    },
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
    let capabilities = service.capabilities(&owner).unwrap();
    assert_eq!(capabilities.capture_formats.len(), 1);
    assert_eq!(capabilities.capture_formats[0].sample_rate_hz, 16_000);
    assert_eq!(
        capabilities.limits.max_in_flight_chunks,
        margins_workflows::workspace_service::DEFAULT_MAX_IN_FLIGHT_CHUNKS
    );
    let mut unsupported = create("unsupported-rate");
    let ClientMessageBodyV1::CreateSession(unsupported_create) = &mut unsupported.body else {
        unreachable!()
    };
    unsupported_create.lanes[0].format.sample_rate_hz = 32_000;
    assert!(service.reserve_session(&owner, unsupported).is_err());

    // This fixture deliberately uses the former 48 kHz wire declaration: a
    // pending pre-cutover spool must replay with its recorded format intact.
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
    let mut odd_pcm = chunk("capture-a", "odd-pcm", "mic", 0);
    let ClientMessageBodyV1::AudioChunk(odd) = &mut odd_pcm.body else {
        unreachable!();
    };
    odd.payload.pop();
    odd.payload_digest.hex = format!("{:x}", Sha256::digest(&odd.payload));
    assert!(service
        .execute_capture(&owner, &reservation.producer_token, odd_pcm)
        .unwrap_err()
        .to_string()
        .contains("partial sample"));
    let mut wrong_duration = chunk("capture-a", "wrong-duration", "mic", 0);
    let ClientMessageBodyV1::AudioChunk(wrong) = &mut wrong_duration.body else {
        unreachable!();
    };
    wrong.duration_ms = DurationMillis(99);
    assert!(service
        .execute_capture(&owner, &reservation.producer_token, wrong_duration)
        .unwrap_err()
        .to_string()
        .contains("duration"));
    let link = WorkspaceNoteAssociationUpdateV1 {
        request_id: "note-link-1".into(),
        source_id: "home".into(),
        relative_path: "meetings/capture-a.md".into(),
        observed_content_hash: Some("observed-hash".into()),
        expected_revision: 0,
    };
    let associated = service
        .link_note(&owner, &SessionId("capture-a".into()), &link)
        .unwrap();
    assert_eq!(
        service
            .link_note(&owner, &SessionId("capture-a".into()), &link)
            .unwrap(),
        associated
    );
    assert_eq!(associated.relative_path, "meetings/capture-a.md");
    assert!(service
        .latest_job(&owner, &SessionId("capture-a".into()))
        .unwrap()
        .is_none());
    assert!(!notes.join("meetings/capture-a.md").exists());
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
    let final_command = finalize("capture-a");
    let finalized = restarted
        .execute_capture(&owner, &reservation.producer_token, final_command.clone())
        .unwrap();
    let lost_ack_replay = restarted
        .execute_capture(&owner, &reservation.producer_token, final_command)
        .unwrap();
    assert!(!finalized.idempotent_replay);
    assert!(lost_ack_replay.idempotent_replay);
    assert_eq!(finalized.messages, lost_ack_replay.messages);

    let page = restarted.sessions(&owner, None, 10).unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].session_id.as_ref(), "capture-a");
    assert!(page.sessions[0].input_finalized);
    assert_eq!(page.sessions[0].capture_lanes.len(), 2);
    assert!(page.sessions[0]
        .capture_lanes
        .iter()
        .all(|lane| lane.format.codec == AudioCodecV1::PcmS16Le));
    let record = restarted
        .repository_record(&owner, &SessionId("capture-a".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(record.segments[0].audio.format.sample_rate_hz, 48_000);
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
fn composed_service_validates_and_finalizes_native_opus_without_relabeling() {
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
    let service = WorkspaceService::open("host-opus", workspace).unwrap();
    let owner = ServicePrincipal::full("client-a", "team");
    let create = native_create_session_command(
        "opus-a",
        "reserve-opus-a",
        Some("Opus composition".into()),
        "service-test",
    );
    let reservation = service.reserve_session(&owner, create).unwrap();
    let spool = DurableTransferSpool::create(
        temp.path(),
        "opus-transfer",
        "host-opus",
        "https://example.test",
        "team",
        "opus-a",
        &reservation.producer_token,
        0,
    )
    .unwrap();
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.begin_segment("segment-opus".into(), 0).unwrap();
    let mic = (0..9_600)
        .map(|index| ((index * 271) % 30_000) as i16 - 15_000)
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    let system = vec![0_u8; 19_200];
    transfer
        .append_s16le(NativeRemoteLane::Microphone, 16_000, &mic)
        .unwrap();
    transfer
        .append_s16le(NativeRemoteLane::System, 16_000, &system)
        .unwrap();
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
    let ClientMessageBodyV1::CloseSegment(close_body) = &close.body else {
        panic!("expected close");
    };
    assert_eq!(close_body.ended_at_ms.0, 600);
    let finalize = transfer
        .seal_session(600, SessionFinalizeReasonV1::Completed)
        .unwrap();
    let commands = transfer
        .spool()
        .pending_chunks()
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.command)
        .collect::<Vec<_>>();

    let mut wrong_duration = commands[0].clone();
    let ClientMessageBodyV1::AudioChunk(chunk) = &mut wrong_duration.body else {
        unreachable!();
    };
    chunk.duration_ms.0 += 1;
    assert!(service
        .execute_capture(&owner, &reservation.producer_token, wrong_duration)
        .unwrap_err()
        .to_string()
        .contains("duration"));

    let mut wrong_source_start = commands
        .iter()
        .find(|command| {
            matches!(
                &command.body,
                ClientMessageBodyV1::AudioChunk(chunk) if chunk.sequence == 1
            )
        })
        .expect("600 ms input should produce a terminal sequence-one block")
        .clone();
    let ClientMessageBodyV1::AudioChunk(chunk) = &mut wrong_source_start.body else {
        unreachable!();
    };
    let mut blocks = decode_opus_packet_blocks_v1(&chunk.payload).unwrap();
    blocks[0].source_start_frame += 1;
    chunk.payload = blocks[0].encode().unwrap();
    chunk.payload_digest.hex = format!("{:x}", Sha256::digest(&chunk.payload));
    assert!(service
        .execute_capture(&owner, &reservation.producer_token, wrong_source_start)
        .unwrap_err()
        .to_string()
        .contains("source start"));

    service
        .execute_audio_batch(
            &owner,
            &reservation.producer_token,
            &SessionId("opus-a".into()),
            commands,
        )
        .unwrap();
    service
        .execute_capture(&owner, &reservation.producer_token, close)
        .unwrap();
    service
        .execute_capture(&owner, &reservation.producer_token, finalize)
        .unwrap();

    let artifacts = service.artifacts(&owner, "opus-a").unwrap();
    assert_eq!(artifacts.len(), 2);
    let stored =
        margins_store::canonical::list_session_artifacts(&captures.join(".margins"), "opus-a")
            .unwrap();
    for lane in ["mic", "system"] {
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.kind == format!("audio_{lane}_opus")));
        let artifact = stored
            .iter()
            .find(|artifact| artifact.kind == format!("audio_{lane}_opus"))
            .unwrap();
        let bytes = std::fs::read(captures.join(&artifact.path)).unwrap();
        assert_eq!(
            validate_opus_packet_stream_v1(&bytes)
                .unwrap()
                .source_frame_count,
            9_600
        );
        assert_eq!(
            remote_opus_packet_stream_for_asr(&bytes).unwrap().len(),
            9_600
        );
    }
    let summary = service
        .sessions(&owner, None, 10)
        .unwrap()
        .sessions
        .remove(0);
    assert_eq!(summary.capture_duration_ms, Some(DurationMillis(600)));
}

#[test]
fn service_accepts_the_advertised_sixteen_command_catch_up_batch() {
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
    let service = WorkspaceService::open("host-a", workspace).unwrap();
    let owner = ServicePrincipal::full("client-a", "team");
    let reservation = service
        .reserve_session(&owner, create("catch-up-batch"))
        .unwrap();
    let commands = (0..16)
        .map(|sequence| {
            chunk(
                "catch-up-batch",
                &format!("mic-{sequence}"),
                "mic",
                sequence,
            )
        })
        .collect::<Vec<_>>();
    let responses = service
        .execute_audio_batch(
            &owner,
            &reservation.producer_token,
            &SessionId("catch-up-batch".into()),
            commands,
        )
        .unwrap();
    assert_eq!(responses.len(), 16);

    let oversized = (0..17)
        .map(|sequence| {
            chunk(
                "catch-up-batch",
                &format!("system-{sequence}"),
                "system",
                sequence,
            )
        })
        .collect();
    assert!(service
        .execute_audio_batch(
            &owner,
            &reservation.producer_token,
            &SessionId("catch-up-batch".into()),
            oversized,
        )
        .unwrap_err()
        .to_string()
        .contains("advertised command bound"));
}

#[test]
fn finalized_asr_capable_service_durably_admits_one_revision_stable_job() {
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
    let service =
        WorkspaceService::open_with_capabilities("host-a", workspace.clone(), true, false).unwrap();
    let owner = ServicePrincipal::full("client-a", "team");
    let reservation = service.reserve_session(&owner, create("asr-a")).unwrap();
    for lane in ["mic", "system"] {
        for sequence in 0..2 {
            service
                .execute_capture(
                    &owner,
                    &reservation.producer_token,
                    chunk("asr-a", &format!("{lane}-{sequence}"), lane, sequence),
                )
                .unwrap();
        }
    }
    service
        .execute_capture(&owner, &reservation.producer_token, close("asr-a"))
        .unwrap();
    let finalize_command = finalize("asr-a");
    service
        .execute_capture(
            &owner,
            &reservation.producer_token,
            finalize_command.clone(),
        )
        .unwrap();
    let admitted = service
        .latest_job(&owner, &SessionId("asr-a".into()))
        .unwrap()
        .unwrap();
    assert_eq!(admitted.job_id, "transcribe:asr-a");
    assert_eq!(admitted.operation, "transcribe_session");
    assert_eq!(admitted.status, "queued");
    assert_eq!(admitted.attempt, 1);

    // A lost finalize ACK replays the same receipt and must not manufacture a
    // second processing attempt for an unchanged finalized input revision.
    service
        .execute_capture(&owner, &reservation.producer_token, finalize_command)
        .unwrap();
    assert_eq!(
        service
            .latest_job(&owner, &SessionId("asr-a".into()))
            .unwrap()
            .unwrap(),
        admitted
    );
    drop(service);

    let restarted =
        WorkspaceService::open_with_capabilities("host-a", workspace, true, false).unwrap();
    assert_eq!(
        restarted.pending_transcription_jobs().unwrap(),
        vec![admitted]
    );
}

#[test]
fn finalized_session_attach_is_generation_fenced_and_retry_safe() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host-a", workspace).unwrap();
    let owner = ServicePrincipal::full("owner", "team");
    let first = service
        .reserve_session(&owner, create("capture-a"))
        .unwrap();
    for lane in ["mic", "system"] {
        for sequence in 0..2 {
            service
                .execute_capture(
                    &owner,
                    &first.producer_token,
                    chunk("capture-a", &format!("{lane}-{sequence}"), lane, sequence),
                )
                .unwrap();
        }
    }
    service
        .execute_capture(&owner, &first.producer_token, close("capture-a"))
        .unwrap();
    service
        .execute_capture(&owner, &first.producer_token, finalize("capture-a"))
        .unwrap();

    let request = WorkspaceAttachV1 {
        request_id: "8cc97936-9cb3-4d27-853f-9818cbe9ea72".into(),
        prior_finalize_message_id: "finalize-1".into(),
        requested_at_unix_ms: UnixMillis(BASE + 200),
        started_at_ms: SessionMillis(200),
    };
    let second = service
        .attach_session(&owner, &SessionId("capture-a".into()), &request)
        .unwrap();
    let replay = service
        .attach_session(&owner, &SessionId("capture-a".into()), &request)
        .unwrap();
    assert_eq!(second.producer_token, replay.producer_token);
    assert!(replay.response.idempotent_replay);

    let second_chunk = |lane: &str| {
        let payload = vec![if lane == "mic" { 0x31 } else { 0x42 }; 9_600];
        command(
            "capture-a",
            &format!("second-{lane}-0"),
            ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                segment_id: "segment-2".into(),
                lane_id: lane.into(),
                sequence: 0,
                starts_at_ms: SessionMillis(200),
                duration_ms: DurationMillis(100),
                payload_digest: ContentDigestV1 {
                    algorithm: DigestAlgorithmV1::Sha256,
                    hex: format!("{:x}", Sha256::digest(&payload)),
                },
                payload,
            }),
        )
    };
    assert!(service
        .execute_capture(&owner, &first.producer_token, second_chunk("mic"))
        .is_err());
    for lane in ["mic", "system"] {
        service
            .execute_capture(&owner, &second.producer_token, second_chunk(lane))
            .unwrap();
    }
    let close = command(
        "capture-a",
        "close-2",
        ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
            segment_id: "segment-2".into(),
            ended_at_ms: SessionMillis(300),
            lane_boundaries: ["mic", "system"]
                .into_iter()
                .map(|lane| LaneBoundaryV1 {
                    lane_id: lane.into(),
                    next_sequence: 1,
                })
                .collect(),
            reason: SegmentCloseReasonV1::Stop,
        }),
    );
    service
        .execute_capture(&owner, &second.producer_token, close)
        .unwrap();
    let finalize = command(
        "capture-a",
        "finalize-2",
        ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
            ended_at_ms: SessionMillis(300),
            segment_closes: vec![SegmentCloseReferenceV1 {
                segment_id: "segment-2".into(),
                close_message_id: "close-2".into(),
            }],
            reason: SessionFinalizeReasonV1::Completed,
        }),
    );
    service
        .execute_capture(&owner, &second.producer_token, finalize)
        .unwrap();
    let page = service.sessions(&owner, None, 10).unwrap();
    assert_eq!(
        page.sessions[0].capture_duration_ms,
        Some(DurationMillis(300))
    );
    assert_eq!(
        page.sessions[0]
            .capture_finalize_message_id
            .as_ref()
            .map(|id| id.as_ref()),
        Some("finalize-2")
    );
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
            &WorkspaceNoteAssociationUpdateV1 {
                request_id: "bad-link".into(),
                source_id: "home".into(),
                relative_path: "../escape.md".into(),
                observed_content_hash: None,
                expected_revision: 0,
            },
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

#[test]
fn scoped_credentials_are_owner_only_expiring_and_revocable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("credentials.json");
    let store = ScopedCredentialStore::open(&path).unwrap();
    let token = store
        .issue(
            "shortcut-phone",
            vec!["team".to_string()],
            vec![OP_IMPORT_WRITE.to_string()],
            None,
        )
        .unwrap();
    let principal = store.authorize(&token, "team").unwrap();
    principal.require("team", OP_IMPORT_WRITE).unwrap();
    assert!(principal.require("team", OP_SESSION_READ).is_err());
    assert!(store.authorize(&token, "another-workspace").is_err());
    assert!(store
        .register(
            "scope-substitution",
            &token,
            vec!["another-workspace".to_string()],
            vec![OP_SESSION_READ.to_string()],
            None,
        )
        .is_err());
    store.authorize(&token, "team").unwrap();
    assert_eq!(store.revoke("shortcut-phone").unwrap(), 1);
    assert!(store.authorize(&token, "team").is_err());

    let expired = store
        .issue(
            "short-lived",
            vec!["team".to_string()],
            vec![OP_SESSION_READ.to_string()],
            Some(std::time::Duration::ZERO),
        )
        .unwrap();
    assert!(store.authorize(&expired, "team").is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
