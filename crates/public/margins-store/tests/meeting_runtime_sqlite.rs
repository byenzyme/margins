use margins_meeting_protocol::*;
use margins_meeting_runtime::test_support::{
    assert_recorder_conformance, RecorderConformanceAdapter,
};
use margins_meeting_runtime::{MeetingRuntime, MeetingRuntimeStorage};
use margins_store::SqliteMeetingRuntimeStorage;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

struct SqliteRecorderAdapter {
    directory: PathBuf,
    runtime: MeetingRuntime<SqliteMeetingRuntimeStorage>,
    use_facade: bool,
}

impl RecorderConformanceAdapter for SqliteRecorderAdapter {
    type Error = anyhow::Error;

    fn send(
        &mut self,
        message: ClientMessageV1,
    ) -> Result<margins_meeting_runtime::RuntimeResponseV1, Self::Error> {
        if !self.use_facade {
            return self
                .runtime
                .handle(message)
                .map_err(|error| anyhow::anyhow!(error.to_string()));
        }
        let recorder = self.runtime.recorder();
        let ClientMessageV1 {
            session_id,
            message_id,
            sent_at_unix_ms,
            body,
            ..
        } = message;
        let result = match body {
            ClientMessageBodyV1::CreateSession(create) => {
                recorder.reserve(&session_id, message_id, sent_at_unix_ms, create)
            }
            ClientMessageBodyV1::AudioChunk(chunk) => {
                let lane = recorder
                    .open_lane(&session_id, chunk.segment_id, chunk.lane_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                recorder.append_chunk(
                    &lane,
                    message_id,
                    sent_at_unix_ms,
                    chunk.sequence,
                    chunk.starts_at_ms,
                    chunk.duration_ms,
                    chunk.payload,
                )
            }
            ClientMessageBodyV1::CloseSegment(close)
                if close.reason == SegmentCloseReasonV1::Pause =>
            {
                recorder.pause(&session_id, message_id, sent_at_unix_ms, close)
            }
            ClientMessageBodyV1::CloseSegment(close) => {
                recorder.close_segment(&session_id, message_id, sent_at_unix_ms, close)
            }
            ClientMessageBodyV1::FinalizeSession(finalize) => {
                recorder.finish(&session_id, message_id, sent_at_unix_ms, finalize)
            }
            ClientMessageBodyV1::BeginCaptureGeneration(begin) => {
                recorder.start_generation(&session_id, message_id, sent_at_unix_ms, begin)
            }
            ClientMessageBodyV1::ResumeSession(resume) => {
                return recorder
                    .recover(
                        &session_id,
                        message_id,
                        sent_at_unix_ms,
                        resume.after_server_sequence,
                    )
                    .map(|(_, response)| response)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            }
            _ => panic!("conformance suite sent an unsupported command"),
        };
        result.map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn restart(&mut self) {
        self.runtime =
            MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(&self.directory).unwrap());
    }
}

#[test]
fn sqlite_recorder_conformance_projects_one_canonical_session_and_two_audio_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let mut adapter = SqliteRecorderAdapter {
        directory: temp.path().to_path_buf(),
        runtime: MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap()),
        use_facade: false,
    };
    assert_recorder_conformance(&mut adapter, "conform");
    adapter.use_facade = true;
    assert_recorder_conformance(&mut adapter, "conform-facade");
    let connection = rusqlite::Connection::open(adapter.runtime.storage().database_path()).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE name = 'conform'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let segment_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_segments WHERE session_name = 'conform'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let artifact_count: i64 = connection.query_row("SELECT COUNT(*) FROM session_artifacts WHERE session_name = 'conform' AND kind = 'audio_mic_pcm'", [], |row| row.get(0)).unwrap();
    let lifecycle: String = connection
        .query_row(
            "SELECT lifecycle_state FROM sessions WHERE name = 'conform'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((count, segment_count, artifact_count), (1, 2, 2));
    assert_eq!(lifecycle, "active");
    assert!(!temp.path().join("conform_capture_context.md").exists());
}

#[test]
fn runtime_storage_rejects_new_session_ids_that_escape_artifact_paths() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    assert!(runtime.handle(create("../escape")).is_err());
    assert!(!temp
        .path()
        .parent()
        .unwrap()
        .join("escape_capture_context.md")
        .exists());
}

#[test]
fn pending_context_requires_finalized_audio_and_normal_finish() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(temp.path()).unwrap());
    runtime.handle(create("empty")).unwrap();
    runtime
        .handle(message(
            "empty",
            "finish-empty",
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(0),
                segment_closes: vec![],
                reason: SessionFinalizeReasonV1::Completed,
            }),
        ))
        .unwrap();
    assert!(!temp.path().join("empty_capture_context.md").exists());

    runtime.handle(create("cancelled")).unwrap();
    runtime
        .handle(chunk("cancelled", "chunk-0", 0, 9_600))
        .unwrap();
    runtime.handle(close("cancelled", 1)).unwrap();
    runtime
        .handle(message(
            "cancelled",
            "finish-cancelled",
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(20),
                segment_closes: vec![SegmentCloseReferenceV1 {
                    segment_id: "part".into(),
                    close_message_id: "close".into(),
                }],
                reason: SessionFinalizeReasonV1::Cancelled,
            }),
        ))
        .unwrap();
    assert!(!temp.path().join("cancelled_capture_context.md").exists());
}

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

mod memo_authority {
    use margins_core::{MemoMoment, TimedMemoLine};
    use margins_store::{
        canonical, MemoRevisionConflict, MemoWrite, SqliteWorkspaceAuthorityStorage,
    };
    use std::sync::{Arc, Barrier};

    fn store() -> (tempfile::TempDir, SqliteWorkspaceAuthorityStorage) {
        let temp = tempfile::tempdir().unwrap();
        canonical::create_session(temp.path(), "meeting", &chrono::Local::now(), "meeting.md")
            .unwrap();
        let storage = SqliteWorkspaceAuthorityStorage::open(temp.path()).unwrap();
        (temp, storage)
    }

    #[test]
    fn update_and_replace_require_the_observed_revision_and_preserve_observation_time() {
        let (_temp, storage) = store();
        let original = storage.memo("meeting").unwrap();
        let update = storage
            .write_memo(
                "meeting",
                "alice",
                "update-1",
                &original.revision,
                MemoWrite::PlainText {
                    observed_at_ms: 12_345,
                    paused: false,
                    text: "first",
                },
            )
            .unwrap();
        assert_eq!(update.lines[0].created_secs, 12.345);
        let replay = storage
            .write_memo(
                "meeting",
                "alice",
                "update-1",
                &original.revision,
                MemoWrite::PlainText {
                    observed_at_ms: 12_345,
                    paused: false,
                    text: "first",
                },
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.revision, update.revision);

        let stale = storage
            .write_memo(
                "meeting",
                "bob",
                "stale",
                &original.revision,
                MemoWrite::PlainText {
                    observed_at_ms: 20_000,
                    paused: false,
                    text: "lost",
                },
            )
            .unwrap_err();
        let conflict = stale.downcast_ref::<MemoRevisionConflict>().unwrap();
        assert_eq!(
            (&conflict.expected, &conflict.current),
            (&original.revision, &update.revision)
        );

        let lines = vec![TimedMemoLine::at(
            "replacement",
            MemoMoment::paused(25.0, 1),
        )];
        let replaced = storage
            .write_memo(
                "meeting",
                "bob",
                "replace-1",
                &update.revision,
                MemoWrite::ReplaceLines(&lines),
            )
            .unwrap();
        assert_eq!(replaced.lines, lines);
        assert_eq!(storage.memo("meeting").unwrap().revision, replaced.revision);
        assert!(storage
            .write_memo(
                "meeting",
                "alice",
                "replace-stale",
                &original.revision,
                MemoWrite::ReplaceLines(&lines)
            )
            .unwrap_err()
            .is::<MemoRevisionConflict>());
    }

    #[test]
    fn concurrent_writers_have_one_winner_and_a_typed_conflict() {
        let (_temp, storage) = store();
        let observed = storage.memo("meeting").unwrap().revision;
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let storage = storage.clone();
                let observed = observed.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    storage.write_memo(
                        "meeting",
                        &format!("writer-{index}"),
                        &format!("write-{index}"),
                        &observed,
                        MemoWrite::PlainText {
                            observed_at_ms: 1000 + index,
                            paused: false,
                            text: if index == 0 { "zero" } else { "one" },
                        },
                    )
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.is::<MemoRevisionConflict>()))
                .count(),
            1
        );
    }

    #[test]
    fn mirror_failure_after_commit_does_not_lose_write_and_can_be_repaired() {
        let (temp, storage) = store();
        let path = temp.path().join("meeting.md");
        std::fs::create_dir(&path).unwrap();
        let initial = storage.memo("meeting").unwrap();
        let written = storage
            .update_memo(
                "meeting",
                "alice",
                "write",
                &initial.revision,
                4_321,
                false,
                "committed",
            )
            .unwrap();
        assert!(written.mirror_stale);
        assert_eq!(storage.memo("meeting").unwrap().revision, written.revision);
        assert!(storage.memo("meeting").unwrap().mirror_stale);
        assert!(path.is_dir());
        std::fs::remove_dir(&path).unwrap();
        storage.refresh_memo_mirror("meeting").unwrap();
        assert!(!storage.memo("meeting").unwrap().mirror_stale);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("committed"));

        let lines = vec![TimedMemoLine::at("replacement", MemoMoment::recording(5.0))];
        let replaced = storage
            .replace_memo_lines("meeting", "bob", "replace", &written.revision, &lines)
            .unwrap();
        assert_eq!(storage.memo("meeting").unwrap().revision, replaced.revision);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("replacement"));
        std::fs::write(&path, "TUI wrote newer Markdown directly\n").unwrap();
        let replay = storage
            .replace_memo_lines("meeting", "bob", "replace", &written.revision, &lines)
            .unwrap();
        assert!(replay.replayed);
        assert!(replay.mirror_stale);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "TUI wrote newer Markdown directly\n"
        );
    }
}
