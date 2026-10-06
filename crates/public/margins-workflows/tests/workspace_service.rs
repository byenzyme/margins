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

#[test]
fn workspace_memo_updates_and_replacements_report_stale_revision() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host", workspace).unwrap();
    let alice = ServicePrincipal::full("alice", "team");
    let bob = ServicePrincipal::full("bob", "team");
    let session = SessionId("memo-cas".into());
    service
        .reserve_session(&alice, create(session.as_ref()))
        .unwrap();
    let observed = service.memo(&alice, &session).unwrap().revision;
    let first = service
        .update_memo(
            &alice,
            &session,
            &WorkspaceMemoUpdateV1 {
                request_id: "alice-edit".into(),
                expected_revision: observed.clone(),
                observed_at_ms: SessionMillis(1_234),
                paused: false,
                text: "Alice".into(),
            },
        )
        .unwrap();
    assert_eq!(first.lines[0].created_secs, 1.234);
    let stale = service
        .replace_memo(
            &bob,
            &session,
            &WorkspaceMemoReplaceV1 {
                request_id: "bob-stale".into(),
                expected_revision: observed,
                lines: vec![WorkspaceMemoLineV1 {
                    text: "Bob".into(),
                    created_secs: 2.0,
                    edited_secs: None,
                    draft_started_secs: None,
                    audio_pending_at_mark: false,
                    block_ordinal: None,
                }],
            },
        )
        .unwrap_err();
    assert!(stale.is::<margins_store::MemoRevisionConflict>());
    assert!(margins_workflows::workspace_service::is_memo_revision_conflict(&stale));
    let replaced = service
        .replace_memo(
            &bob,
            &session,
            &WorkspaceMemoReplaceV1 {
                request_id: "bob-fresh".into(),
                expected_revision: first.revision,
                lines: vec![WorkspaceMemoLineV1 {
                    text: "Bob".into(),
                    created_secs: 2.0,
                    edited_secs: None,
                    draft_started_secs: None,
                    audio_pending_at_mark: false,
                    block_ordinal: None,
                }],
            },
        )
        .unwrap();
    assert_eq!(replaced.lines[0].text, "Bob");
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
fn browser_webm_capture_state_reconstructs_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let owner = ServicePrincipal::full("browser", "team");
    let owner_token = "550e8400-e29b-41d4-a716-446655440000";
    let session = SessionId("browser-restart".into());
    let mut create = create(session.as_ref());
    if let ClientMessageBodyV1::CreateSession(body) = &mut create.body {
        body.lanes.truncate(1);
        body.sources.truncate(1);
        body.lanes[0].format = AudioFormatV1 {
            codec: AudioCodecV1::Opus,
            container: AudioContainerV1::Webm,
            sample_rate_hz: 48_000,
            channel_count: 1,
        };
    }
    let service = WorkspaceService::open("host", workspace.clone()).unwrap();
    assert!(service
        .capabilities(&owner)
        .unwrap()
        .capture_formats
        .iter()
        .any(|format| format.codec == AudioCodecV1::Opus
            && format.container == AudioContainerV1::Webm
            && format.sample_rate_hz == 48_000));
    assert!(service
        .reserve_session_with_producer_token(&owner, create.clone(), "owner-1")
        .is_err());
    assert_eq!(
        service
            .reserve_session_with_producer_token(&owner, create, owner_token)
            .unwrap()
            .producer_token,
        owner_token
    );
    let webm_chunk = |segment: &str, sequence: u64, start: u64| {
        let payload = vec![0x1a, 0x45, 0xdf, 0xa3, sequence as u8];
        command(
            session.as_ref(),
            &format!("{segment}-{sequence}"),
            ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                segment_id: segment.into(),
                lane_id: "mic".into(),
                sequence,
                starts_at_ms: SessionMillis(start),
                duration_ms: DurationMillis(100),
                payload_digest: ContentDigestV1 {
                    algorithm: DigestAlgorithmV1::Sha256,
                    hex: format!("{:x}", Sha256::digest(&payload)),
                },
                payload,
            }),
        )
    };
    service
        .execute_capture(&owner, owner_token, webm_chunk("browser", 0, 0))
        .unwrap();
    service
        .execute_capture(&owner, owner_token, webm_chunk("browser", 2, 200))
        .unwrap();
    drop(service);

    let service = WorkspaceService::open("host", workspace.clone()).unwrap();
    assert!(service
        .capture_state_for_producer(&owner, "wrong-owner", &session)
        .is_err());
    let recovered = service
        .capture_state_for_producer(&owner, owner_token, &session)
        .unwrap();
    assert!(!recovered.input_finalized);
    assert_eq!(recovered.segments.len(), 1);
    let ack = &recovered.segments[0].lanes[0].acknowledgement;
    assert_eq!(ack.durable_through_sequence, 1);
    assert_eq!(ack.durable_out_of_order[0].start, 2);
    assert_eq!(ack.durable_out_of_order[0].end_exclusive, 3);
    let (reopened, replay) = service
        .recover_capture_for_producer(&owner, owner_token, &session, "browser-recover".into())
        .unwrap();
    assert_eq!(reopened, recovered);
    assert!(replay
        .messages
        .iter()
        .any(|message| matches!(message.body, ServerMessageBodyV1::ReplayCompleted(_))));
    let (_, repeated_replay) = service
        .recover_capture_for_producer(&owner, owner_token, &session, "browser-recover".into())
        .unwrap();
    assert!(repeated_replay.idempotent_replay);
    assert_eq!(repeated_replay.messages, replay.messages);

    service
        .execute_capture(&owner, owner_token, webm_chunk("browser", 1, 100))
        .unwrap();
    service
        .execute_capture(
            &owner,
            owner_token,
            command(
                session.as_ref(),
                "browser-pause",
                ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                    segment_id: "browser".into(),
                    ended_at_ms: SessionMillis(300),
                    lane_boundaries: vec![LaneBoundaryV1 {
                        lane_id: "mic".into(),
                        next_sequence: 3,
                    }],
                    reason: SegmentCloseReasonV1::Pause,
                }),
            ),
        )
        .unwrap();
    drop(service);

    let service = WorkspaceService::open("host", workspace).unwrap();
    let paused = service.capture_state(&owner, &session).unwrap();
    assert_eq!(
        paused.segments[0].lanes[0]
            .acknowledgement
            .durable_through_sequence,
        3
    );
    let close = paused.segments[0].close.as_ref().unwrap();
    assert_eq!(close.message_id.as_ref(), "browser-pause");
    assert!(close.finalized);
    assert_eq!(close.command.reason, SegmentCloseReasonV1::Pause);
    assert_eq!(close.command.lane_boundaries[0].next_sequence, 3);
    let resume_id: MessageId = "browser-resume-1".into();
    assert!(!service
        .capture_command_recorded(&owner, owner_token, &session, &resume_id)
        .unwrap());
    service
        .execute_capture(
            &owner,
            owner_token,
            command(
                session.as_ref(),
                resume_id.as_ref(),
                ClientMessageBodyV1::ResumeSession(ResumeSessionV1 {
                    after_server_sequence: None,
                }),
            ),
        )
        .unwrap();
    assert!(service
        .capture_command_recorded(&owner, owner_token, &session, &resume_id)
        .unwrap());
    service
        .execute_capture(&owner, owner_token, webm_chunk("browser-2", 0, 300))
        .unwrap();
    service
        .execute_capture(
            &owner,
            owner_token,
            command(
                session.as_ref(),
                "browser-stop",
                ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                    segment_id: "browser-2".into(),
                    ended_at_ms: SessionMillis(400),
                    lane_boundaries: vec![LaneBoundaryV1 {
                        lane_id: "mic".into(),
                        next_sequence: 1,
                    }],
                    reason: SegmentCloseReasonV1::Stop,
                }),
            ),
        )
        .unwrap();
    service
        .execute_capture(
            &owner,
            owner_token,
            command(
                session.as_ref(),
                "browser-finish",
                ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(400),
                    segment_closes: vec![
                        SegmentCloseReferenceV1 {
                            segment_id: "browser".into(),
                            close_message_id: "browser-pause".into(),
                        },
                        SegmentCloseReferenceV1 {
                            segment_id: "browser-2".into(),
                            close_message_id: "browser-stop".into(),
                        },
                    ],
                    reason: SessionFinalizeReasonV1::Completed,
                }),
            ),
        )
        .unwrap();
    let final_state = service
        .capture_state_for_producer(&owner, owner_token, &session)
        .unwrap();
    assert!(final_state.input_finalized);
    assert_eq!(final_state.segments.len(), 2);
    assert_eq!(final_state.segments[0].segment_id.as_ref(), "browser");
    assert_eq!(final_state.segments[1].segment_id.as_ref(), "browser-2");
    assert_eq!(
        final_state.segments[1]
            .close
            .as_ref()
            .unwrap()
            .message_id
            .as_ref(),
        "browser-stop"
    );
}

#[test]
fn discard_finished_session_removes_source_material_and_preserves_home_note() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host-a", workspace).unwrap();
    let owner = ServicePrincipal::full("producer", "team");
    let session = SessionId("to-discard".into());
    let reservation = service
        .reserve_session(&owner, create(session.as_ref()))
        .unwrap();
    assert!(service.discard_session(&owner, &session).is_err());
    service
        .publish_live_checkpoint(
            &owner,
            &reservation.producer_token,
            &session,
            &serde_json::json!({
                "version": 2, "terminal": false, "decoded_until_ms": 0, "committed_until_ms": 0,
                "transcripts": [{"words": []}]
            }),
        )
        .unwrap();
    let live_files = [
        service.margins_dir().join("to-discard_remote_live.lock"),
        service
            .margins_dir()
            .join("to-discard_remote.live-transcript.json"),
    ];
    assert!(live_files.iter().all(|path| path.exists()));
    for lane in ["mic", "system"] {
        for sequence in 0..2 {
            service
                .execute_capture(
                    &owner,
                    &reservation.producer_token,
                    chunk(
                        session.as_ref(),
                        &format!("{lane}-{sequence}"),
                        lane,
                        sequence,
                    ),
                )
                .unwrap();
        }
    }
    service
        .execute_capture(&owner, &reservation.producer_token, close(session.as_ref()))
        .unwrap();
    service
        .execute_capture(
            &owner,
            &reservation.producer_token,
            finalize(session.as_ref()),
        )
        .unwrap();
    let note = notes.join("linked.md");
    std::fs::write(&note, "Connected note remains").unwrap();
    let artifact = service.margins_dir().join("artifacts/to-discard");
    assert!(artifact.exists());
    assert!(service
        .margins_dir()
        .join("meeting-blobs")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
    service.discard_session(&owner, &session).unwrap();
    assert!(service
        .sessions(&owner, None, 10)
        .unwrap()
        .sessions
        .is_empty());
    assert!(!artifact.exists());
    assert!(live_files.iter().all(|path| !path.exists()));
    assert!(service
        .margins_dir()
        .join("meeting-blobs")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
    assert_eq!(
        std::fs::read_to_string(note).unwrap(),
        "Connected note remains"
    );
}

#[test]
fn native_open_segment_is_visible_to_workspace_reader_before_audio_arrives() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host", workspace).unwrap();
    let producer = ServicePrincipal::full("native-producer", "team");
    let reader = ServicePrincipal::scoped(
        "bb-reader",
        ["team".to_string()],
        [OP_SESSION_READ.to_string()],
    );
    service
        .reserve_session(&producer, create("visible"))
        .unwrap();
    assert!(service
        .sessions(&reader, None, 10)
        .unwrap()
        .sessions
        .is_empty());
    let storage =
        margins_store::SqliteMeetingRuntimeStorage::open(&captures.join(".margins")).unwrap();
    storage.open_native_segment("visible", 0, 0).unwrap();
    let sessions = service.sessions(&reader, None, 10).unwrap().sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id.as_ref(), "visible");
    assert_eq!(sessions[0].segment_count, 1);
    assert!(!sessions[0].input_finalized);
}

#[test]
fn native_runtime_audio_is_listed_as_exportable_and_downloads_as_wav() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host", workspace).unwrap();
    let producer = ServicePrincipal::full("native-producer", "team");
    let reader = ServicePrincipal::scoped(
        "bb-reader",
        ["team".to_string()],
        [OP_SESSION_READ.to_string()],
    );
    let mut create = create("tui-a");
    let ClientMessageBodyV1::CreateSession(ref mut session) = create.body else {
        unreachable!()
    };
    session.provenance.hops[0].producer = "margins-tui".into();
    for lane in &mut session.lanes {
        lane.format.sample_rate_hz = 16_000;
    }
    let reservation = service.reserve_session(&producer, create).unwrap();
    for lane in ["mic", "system"] {
        for sequence in 0..2 {
            let mut message = chunk("tui-a", &format!("{lane}-{sequence}"), lane, sequence);
            let ClientMessageBodyV1::AudioChunk(ref mut audio) = message.body else {
                unreachable!()
            };
            audio.segment_id = "tui-a-seg-0".into();
            audio.payload.truncate(1_600 * 2);
            audio.payload_digest.hex = format!("{:x}", Sha256::digest(&audio.payload));
            service
                .execute_capture(&producer, &reservation.producer_token, message)
                .unwrap();
        }
    }
    let mut close = close("tui-a");
    let ClientMessageBodyV1::CloseSegment(ref mut segment) = close.body else {
        unreachable!()
    };
    segment.segment_id = "tui-a-seg-0".into();
    service
        .execute_capture(&producer, &reservation.producer_token, close)
        .unwrap();
    let mut finish = finalize("tui-a");
    let ClientMessageBodyV1::FinalizeSession(ref mut session) = finish.body else {
        unreachable!()
    };
    session.segment_closes[0].segment_id = "tui-a-seg-0".into();
    service
        .execute_capture(&producer, &reservation.producer_token, finish)
        .unwrap();

    let margins = captures.join(".margins");
    let wav = margins.join("tui-a_seg0.wav");
    assert!(!wav.exists());
    let registered = margins_store::canonical::list_session_artifacts(&margins, "tui-a").unwrap();
    assert_eq!(registered.len(), 2);
    // Simulate a TUI capture finalized before artifact rows were registered.
    for artifact in registered {
        margins_store::canonical::delete_session_artifact_registry_row(
            &margins,
            "tui-a",
            &artifact.kind,
            artifact.ordinal,
        )
        .unwrap();
    }
    let views = margins_workflows::artifacts::list_artifacts(&captures, &margins, "tui-a").unwrap();
    assert_eq!(views.len(), 2);
    assert!(views
        .iter()
        .all(|view| view.available && !view.exists && view.disk_path == wav));
    let artifacts = service.artifacts(&reader, "tui-a").unwrap();
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts
        .iter()
        .all(|artifact| artifact.size_bytes == Some(6_444)));
    assert!(!wav.exists(), "listing must not double stored audio");
    for artifact in &artifacts {
        let bytes = service
            .artifact_content(&reader, artifact.artifact_id.as_ref())
            .unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
        assert_eq!(bytes.len(), 6_444);
        let expected = if artifact.kind == "audio_mic_runtime" {
            0x1100
        } else {
            0x2200
        };
        assert_eq!(
            i16::from_le_bytes(bytes[44..46].try_into().unwrap()),
            expected
        );
        assert!(
            !wav.exists(),
            "artifact download must not leave a derived WAV"
        );
    }
    let exported = margins_store::SqliteMeetingRuntimeStorage::open(&margins)
        .unwrap()
        .export_native_wav("tui-a", 0)
        .unwrap();
    assert_eq!(exported, wav);
    let stereo = std::fs::read(wav).unwrap();
    assert_eq!(u16::from_le_bytes(stereo[22..24].try_into().unwrap()), 2);
    assert_eq!(stereo.len(), 12_844);
}

#[test]
fn workspace_reader_can_follow_an_unclosed_capture_without_producer_access() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    let captures = temp.path().join("captures");
    std::fs::create_dir_all(&notes).unwrap();
    let workspace =
        ensure_service_workspace(&temp.path().join("state"), "team", None, &notes, &captures)
            .unwrap();
    let service = WorkspaceService::open("host-a", workspace).unwrap();
    let producer = ServicePrincipal::full("native-producer", "team");
    let reader = ServicePrincipal::scoped(
        "codex-reader",
        ["team".to_string()],
        [OP_SESSION_READ.to_string()],
    );
    let denied = ServicePrincipal::scoped(
        "wrong-workspace",
        ["other".to_string()],
        [OP_SESSION_READ.to_string()],
    );
    let reservation = service
        .reserve_session(&producer, create("live-a"))
        .unwrap();
    assert_eq!(service.current(&reader).unwrap(), None);
    assert!(service
        .sessions(&reader, None, 10)
        .unwrap()
        .sessions
        .is_empty());
    assert!(service.active_sessions(&denied).is_err());
    let active = service.active_sessions(&reader).unwrap();
    assert_eq!(active.sessions.len(), 1);
    assert_eq!(active.sessions[0].session_id.as_ref(), "live-a");
    assert_eq!(active.sessions[0].segment_count, 0);
    assert!(!active.sessions[0].input_finalized);
    assert_eq!(
        service
            .session(&reader, &SessionId("live-a".into()))
            .unwrap(),
        active.sessions[0]
    );
    let checkpoint = serde_json::json!({
        "version": 2,
        "terminal": false,
        "decoded_until_ms": 1400,
        "committed_until_ms": 1200,
        "transcripts": [{"words": [{
            "channel": 0, "start_ms": 100, "end_ms": 500, "text": " hello"
        }]}]
    });
    assert!(service
        .publish_live_checkpoint(
            &reader,
            &reservation.producer_token,
            &SessionId("live-a".into()),
            &checkpoint
        )
        .is_err());
    assert!(service
        .publish_live_checkpoint(&producer, "wrong", &SessionId("live-a".into()), &checkpoint)
        .is_err());
    service
        .publish_live_checkpoint(
            &producer,
            &reservation.producer_token,
            &SessionId("live-a".into()),
            &checkpoint,
        )
        .unwrap();
    let interim = service.transcript(&reader, "live-a").unwrap();
    assert!(!interim.terminal);
    assert!(interim.live);
    assert!(interim.body.contains("hello"));
    let old = serde_json::json!({"version":2,"terminal":false,"decoded_until_ms":1000,"committed_until_ms":800,"transcripts":[{"words":[]}]});
    assert!(service
        .publish_live_checkpoint(
            &producer,
            &reservation.producer_token,
            &SessionId("live-a".into()),
            &old
        )
        .is_err());

    for lane in ["mic", "system"] {
        for sequence in 0..2 {
            service
                .execute_capture(
                    &producer,
                    &reservation.producer_token,
                    chunk("live-a", &format!("{lane}-{sequence}"), lane, sequence),
                )
                .unwrap();
        }
    }
    assert_eq!(
        service.active_sessions(&reader).unwrap().sessions[0].segment_count,
        0
    );
    service
        .execute_capture(&producer, &reservation.producer_token, close("live-a"))
        .unwrap();
    service
        .execute_capture(&producer, &reservation.producer_token, finalize("live-a"))
        .unwrap();
    assert!(service
        .publish_live_checkpoint(
            &producer,
            &reservation.producer_token,
            &SessionId("live-a".into()),
            &checkpoint
        )
        .is_err());
    assert!(service
        .active_sessions(&reader)
        .unwrap()
        .sessions
        .is_empty());
    assert!(
        service
            .session(&reader, &SessionId("live-a".into()))
            .unwrap()
            .input_finalized
    );
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
    assert_eq!(capabilities.capture_formats.len(), 2);
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
        bb_thread_id: Some("thr-distill".into()),
        distilled_memo_revision: Some("memo-v1".into()),
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
    assert_eq!(associated.bb_thread_ids, vec!["thr-distill"]);
    assert_eq!(
        associated.distilled_memo_revision.as_deref(),
        Some("memo-v1")
    );
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
    let direct_pending = margins_workflows::transcript_view::load_transcript_view(
        &captures,
        &captures.join(".margins"),
        "latest",
    )
    .unwrap();
    assert_eq!(direct_pending.view, "pending");
    assert!(!direct_pending.terminal);
    let pending = restarted.transcript(&owner, "latest").unwrap();
    assert_eq!(pending.session_id.as_ref(), "capture-a");
    assert!(pending.body.contains("Transcript pending"));
    assert_eq!(pending.view, "pending");
    assert!(!pending.terminal);
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
        WorkspaceService::open_with_capabilities("host-a", workspace.clone(), false, false)
            .unwrap();
    service.enable_deferred_asr();
    let owner = ServicePrincipal::full("client-a", "team");
    assert!(!service.capabilities(&owner).unwrap().asr_available);
    let observer = service.clone();
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
    service.set_asr_available(true);
    assert!(observer.capabilities(&owner).unwrap().asr_available);

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
        vec![admitted.clone()]
    );
    let read_only =
        ServicePrincipal::scoped("reader", ["team".to_string()], ["session.read".to_string()]);
    assert!(restarted
        .request_transcription_job(&read_only, &SessionId("asr-a".into()))
        .is_err());
    assert_eq!(
        restarted
            .request_transcription_job(&owner, &SessionId("asr-a".into()))
            .unwrap(),
        admitted
    );
    restarted
        .update_transcription_job(
            &admitted.job_id,
            admitted.attempt,
            "failed",
            None,
            None,
            Some("model unavailable"),
        )
        .unwrap();
    let retried = restarted
        .request_transcription_job(&owner, &SessionId("asr-a".into()))
        .unwrap();
    assert_eq!(retried.attempt, admitted.attempt + 1);
    assert_eq!(retried.status, "queued");
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
    let margins_dir = captures.join(".margins");
    let pending_path = margins_dir.join("capture-a_capture_context.md");
    assert!(pending_path.is_file());
    let connection = rusqlite::Connection::open(margins_dir.join("sessions.sqlite")).unwrap();
    let lifecycle: String = connection
        .query_row(
            "SELECT lifecycle_state FROM sessions WHERE name = 'capture-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(lifecycle, "ended");

    let request = WorkspaceAttachV1 {
        request_id: "8cc97936-9cb3-4d27-853f-9818cbe9ea72".into(),
        prior_finalize_message_id: "finalize-1".into(),
        requested_at_unix_ms: UnixMillis(BASE + 200),
        started_at_ms: SessionMillis(200),
    };
    let second = service
        .attach_session(&owner, &SessionId("capture-a".into()), &request)
        .unwrap();
    let lifecycle: String = connection
        .query_row(
            "SELECT lifecycle_state FROM sessions WHERE name = 'capture-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let repository_lifecycle: String = connection
        .query_row(
            "SELECT lifecycle FROM session_repository_state WHERE session_name = 'capture-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (lifecycle.as_str(), repository_lifecycle.as_str()),
        ("active", "active")
    );
    assert!(!pending_path.exists());
    assert!(service.transcript(&owner, "latest").is_err());
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
                bb_thread_id: None,
                distilled_memo_revision: None,
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
