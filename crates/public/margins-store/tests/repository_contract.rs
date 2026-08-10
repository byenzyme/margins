use chrono::{TimeZone, Utc};
use margins_core::{
    ArtifactDescriptor, ArtifactId, AudioFormat, DurationMillis, NewSegment, NewSession,
    SampleFormat, SegmentId, SessionArtifact, SessionErrorCode, SessionId, SessionLifecycle,
    SessionQuery, SessionRepository, UnixMillis,
};
use margins_store::{legacy, SqliteSessionRepository};
use rusqlite::{params, Connection};
use std::sync::{Arc, Barrier};
use tempfile::tempdir;

fn new_session(id: &str) -> NewSession {
    NewSession {
        id: SessionId::from(id),
        started_at_ms: UnixMillis(1_767_225_600_000),
        created_at_ms: UnixMillis(1_767_225_600_123),
        title: Some("Compatibility call".to_string()),
        note_uri: Some("notes/compatibility.md".to_string()),
    }
}

fn new_segment(id: &str, ordinal: u64, offset_ms: u64) -> NewSegment {
    let segment_id = SegmentId::from(id);
    NewSegment {
        id: segment_id.clone(),
        ordinal,
        start_offset_ms: offset_ms,
        duration_ms: DurationMillis(2_500),
        started_at_ms: UnixMillis(1_767_225_600_500 + offset_ms),
        audio: ArtifactDescriptor {
            id: ArtifactId::from(format!("audio-{id}")),
            segment_id,
            lane: None,
            uri: format!(".margins/{id}.wav"),
            format: AudioFormat {
                sample_rate_hz: 48_000,
                channel_count: 2,
                sample_format: SampleFormat::Signed16,
            },
            duration_ms: DurationMillis(2_500),
            frame_count: 120_000,
            byte_length: Some(480_044),
        },
        dropped_live_frames: 7,
        dropped_durable_frames: 0,
        timeline_reusable: true,
    }
}

#[test]
fn revisions_are_durable_cas_and_legacy_rows_remain_readable() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    let repository = SqliteSessionRepository::open(&margins_dir).unwrap();

    let created = repository.create(new_session("session-a")).unwrap();
    assert_eq!(created.revision, 0);
    assert_eq!(created.lifecycle, SessionLifecycle::Active);

    let appended = repository
        .append_segment(&created.id, 0, new_segment("segment-0", 0, 0))
        .unwrap();
    assert_eq!(appended.revision, 1);
    assert_eq!(appended.segments.len(), 1);
    assert_eq!(appended.segments[0].dropped_live_frames, 7);

    let stale = repository
        .append_segment(&created.id, 0, new_segment("segment-1", 1, 2_500))
        .unwrap_err();
    assert_eq!(stale.code, SessionErrorCode::Conflict);
    assert_eq!(
        legacy::get_session_meta(&margins_dir, "session-a")
            .unwrap()
            .segments
            .len(),
        1
    );

    let paused = repository
        .transition(&created.id, 1, SessionLifecycle::Paused)
        .unwrap();
    assert_eq!(paused.revision, 2);
    let unchanged = repository
        .transition(&created.id, 2, SessionLifecycle::Paused)
        .unwrap();
    assert_eq!(unchanged.revision, 2);
    assert!(repository
        .list(SessionQuery {
            limit: Some(0),
            ..SessionQuery::default()
        })
        .unwrap()
        .is_empty());

    let with_artifact = repository
        .upsert_artifact(
            2,
            SessionArtifact {
                id: ArtifactId::from("artifact-transcript"),
                session_id: created.id.clone(),
                kind: "transcript".to_string(),
                ordinal: 0,
                uri: ".margins/session-a-transcript.md".to_string(),
                retention_class: "durable".to_string(),
                created_at_ms: UnixMillis(1_767_225_700_321),
                expires_at_ms: None,
            },
        )
        .unwrap();
    assert_eq!(with_artifact.revision, 3);
    assert_eq!(
        with_artifact.artifacts[0].id.as_ref(),
        "artifact-transcript"
    );

    let stale_artifact = repository
        .upsert_artifact(
            2,
            SessionArtifact {
                id: ArtifactId::from("stale"),
                session_id: created.id.clone(),
                kind: "transcript".to_string(),
                ordinal: 1,
                uri: "stale.md".to_string(),
                retention_class: "temporary".to_string(),
                created_at_ms: UnixMillis(1_767_225_700_322),
                expires_at_ms: None,
            },
        )
        .unwrap_err();
    assert_eq!(stale_artifact.code, SessionErrorCode::Conflict);
    assert_eq!(
        legacy::list_session_artifacts(&margins_dir, "session-a")
            .unwrap()
            .len(),
        1
    );

    drop(repository);
    let reopened = SqliteSessionRepository::open(&margins_dir).unwrap();
    let durable = reopened.get(&created.id).unwrap().unwrap();
    assert_eq!(durable.revision, 3);
    assert_eq!(
        durable.artifacts[0].created_at_ms,
        UnixMillis(1_767_225_700_321)
    );
    assert_eq!(durable.segments[0].audio.frame_count, 120_000);

    reopened.tombstone(&created.id, 3).unwrap();
    let tombstoned = reopened.get(&created.id).unwrap().unwrap();
    assert_eq!(tombstoned.revision, 4);
    assert_eq!(tombstoned.lifecycle, SessionLifecycle::Tombstoned);
    assert!(legacy::list_sessions(&margins_dir).unwrap().is_empty());
    assert!(legacy::is_session_tombstoned(&margins_dir, "session-a").unwrap());
    assert!(reopened.create(new_session("session-a")).is_err());
}

#[test]
fn duplicate_or_out_of_order_segments_fail_without_partial_writes() {
    let temporary = tempdir().unwrap();
    let repository = SqliteSessionRepository::open(temporary.path()).unwrap();
    let created = repository.create(new_session("ordered")).unwrap();
    let one = repository
        .append_segment(&created.id, 0, new_segment("first", 2, 200))
        .unwrap();

    for invalid_segment in [
        new_segment("first", 3, 300),
        new_segment("duplicate-ordinal", 2, 300),
        new_segment("backwards", 4, 100),
    ] {
        let error = repository
            .append_segment(&created.id, one.revision, invalid_segment)
            .unwrap_err();
        assert_eq!(error.code, SessionErrorCode::InvalidSegment);
    }
    assert_eq!(
        repository.get(&created.id).unwrap().unwrap().segments.len(),
        1
    );
}

#[test]
fn legacy_segments_remain_indexable_but_are_not_fabricated_as_core_contracts() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    let started = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "legacy", &started.into(), "legacy.md").unwrap();
    legacy::add_segment(&margins_dir, "legacy", 0, "legacy.wav", 0, Some(1.0)).unwrap();

    let repository = SqliteSessionRepository::open(&margins_dir).unwrap();
    let summaries = repository.list(SessionQuery::default()).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].segment_count, 1);
    let error = repository
        .get(&SessionId::from("legacy"))
        .expect_err("missing rich segment facts must not be invented");
    assert_eq!(error.code, SessionErrorCode::CorruptData);
    assert_eq!(
        legacy::get_session_meta(&margins_dir, "legacy")
            .unwrap()
            .segments
            .len(),
        1
    );
}

#[test]
fn tombstones_reject_artifact_writes_and_path_like_session_ids() {
    let temporary = tempdir().unwrap();
    let repository = SqliteSessionRepository::open(temporary.path()).unwrap();

    let invalid = repository.create(new_session("../escape")).unwrap_err();
    assert_eq!(invalid.code, SessionErrorCode::CorruptData);

    let created = repository.create(new_session("tombstoned")).unwrap();
    repository.tombstone(&created.id, 0).unwrap();
    let error = repository
        .upsert_artifact(
            1,
            SessionArtifact {
                id: ArtifactId::from("late-artifact"),
                session_id: created.id.clone(),
                kind: "transcript".to_string(),
                ordinal: 0,
                uri: "late.md".to_string(),
                retention_class: "durable".to_string(),
                created_at_ms: UnixMillis(1_767_225_700_321),
                expires_at_ms: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code, SessionErrorCode::InvalidTransition);
    assert!(
        legacy::list_session_artifacts(temporary.path(), "tombstoned")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn sidecar_divergence_is_reported_as_corrupt_data() {
    let temporary = tempdir().unwrap();
    let repository = SqliteSessionRepository::open(temporary.path()).unwrap();
    let created = repository.create(new_session("diverged")).unwrap();
    repository
        .append_segment(&created.id, 0, new_segment("segment", 0, 0))
        .unwrap();

    let connection = Connection::open(legacy::database_path(temporary.path())).unwrap();
    connection
        .execute(
            "UPDATE session_segment_contracts SET segment_id = 'wrong' WHERE session_name = ?1",
            params![created.id.as_ref()],
        )
        .unwrap();
    let error = repository.get(&created.id).unwrap_err();
    assert_eq!(error.code, SessionErrorCode::CorruptData);
}

#[test]
fn concurrent_compare_and_swap_has_exactly_one_winner() {
    let temporary = tempdir().unwrap();
    let repository = Arc::new(SqliteSessionRepository::open(temporary.path()).unwrap());
    let created = repository.create(new_session("contended")).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for lifecycle in [SessionLifecycle::Paused, SessionLifecycle::Processing] {
        let repository = Arc::clone(&repository);
        let barrier = Arc::clone(&barrier);
        let id = created.id.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            repository.transition(&id, 0, lifecycle)
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert_eq!(loser.code, SessionErrorCode::Conflict);
    assert_eq!(repository.get(&created.id).unwrap().unwrap().revision, 1);
}
