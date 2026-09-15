use chrono::Local;
use margins_store::legacy;
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempdir().unwrap();
    let margins = temporary.path().join(".margins");
    legacy::create_session(&margins, "session", &Local::now(), "session.md").unwrap();
    legacy::add_segment(
        &margins,
        "session",
        0,
        "recordings/session.wav",
        0,
        Some(3.0),
    )
    .unwrap();
    (temporary, margins)
}

#[test]
fn note_association_is_revisioned_source_relative_and_has_no_job_side_effects() {
    let (_temporary, margins) = store();
    let failed = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    legacy::update_processing_job(
        &margins,
        &failed.job_id,
        failed.attempt,
        "failed",
        Some(0.5),
        None,
        Some("model unavailable"),
        Some("distill"),
    )
    .unwrap();

    let linked = legacy::link_note(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        0,
    )
    .unwrap();
    assert_eq!(linked.revision, 1);
    let job = legacy::get_processing_job(&margins, "note:session")
        .unwrap()
        .unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.failure.as_deref(), Some("model unavailable"));
    assert_eq!(
        legacy::get_session_meta(&margins, "session")
            .unwrap()
            .segments
            .len(),
        1
    );

    let replay = legacy::link_note(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        0,
    )
    .unwrap();
    assert_eq!(replay.revision, 1, "exact retry is idempotent");
    assert!(legacy::link_note(
        &margins,
        "session",
        "workspace",
        "inbox/changed.md",
        None,
        0,
    )
    .unwrap_err()
    .to_string()
    .contains("revision conflict"));
    assert!(legacy::link_note(&margins, "session", "workspace", "../escape.md", None, 1,).is_err());
}

#[test]
fn cancelled_or_superseded_job_attempt_rejects_late_success_after_reopen() {
    let (_temporary, margins) = store();
    let first = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    legacy::cancel_processing_job(&margins, &first.job_id, first.attempt).unwrap();
    assert!(legacy::update_processing_job(
        &margins,
        &first.job_id,
        first.attempt,
        "complete",
        Some(1.0),
        Some("inbox/session.md"),
        None,
        None,
    )
    .unwrap_err()
    .to_string()
    .contains("late processing result"));

    let second = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    assert_eq!(second.attempt, first.attempt + 1);
    assert!(legacy::update_processing_job(
        &margins,
        &first.job_id,
        first.attempt,
        "complete",
        Some(1.0),
        Some("stale.md"),
        None,
        None,
    )
    .unwrap_err()
    .to_string()
    .contains("superseded"));
    let reopened = legacy::get_processing_job(&margins, "note:session")
        .unwrap()
        .unwrap();
    assert_eq!(reopened.attempt, second.attempt);
    assert_eq!(reopened.status, "queued");

    let third = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-2",
    )
    .unwrap();
    assert_eq!(third.attempt, second.attempt + 1);
    assert_eq!(third.input_revision, "input-2");
    assert!(legacy::update_processing_job(
        &margins,
        &second.job_id,
        second.attempt,
        "complete",
        Some(1.0),
        Some("stale.md"),
        None,
        None,
    )
    .unwrap_err()
    .to_string()
    .contains("superseded"));
}

#[test]
fn link_unlink_never_remove_audio_or_complete_processing() {
    let (temporary, margins) = store();
    let audio = temporary.path().join("recordings/session.wav");
    std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
    std::fs::write(&audio, b"durable audio").unwrap();
    let job = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    let linked = legacy::link_note(
        &margins,
        "session",
        "workspace",
        "notes/session.md",
        None,
        0,
    )
    .unwrap();
    legacy::unlink_note(&margins, "session", linked.revision).unwrap();
    assert!(audio.exists());
    assert_eq!(
        legacy::get_processing_job(&margins, &job.job_id)
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
}

#[test]
fn note_publication_and_exact_job_completion_are_atomic_and_retryable() {
    let (_temporary, margins) = store();
    let job = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    let linked = legacy::complete_processing_job_with_note(
        &margins,
        &job.job_id,
        job.attempt,
        "workspace",
        "notes/session.md",
        Some("sha256:note"),
        0,
    )
    .unwrap();
    assert_eq!(linked.revision, 1);
    assert_eq!(
        legacy::get_processing_job(&margins, &job.job_id)
            .unwrap()
            .unwrap()
            .status,
        "complete"
    );

    let retry = legacy::complete_processing_job_with_note(
        &margins,
        &job.job_id,
        job.attempt,
        "workspace",
        "notes/session.md",
        Some("sha256:note"),
        0,
    )
    .unwrap();
    assert_eq!(retry, linked);
}

#[test]
fn cancelled_job_cannot_publish_a_late_note_association() {
    let (_temporary, margins) = store();
    let job = legacy::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    legacy::cancel_processing_job(&margins, &job.job_id, job.attempt).unwrap();
    assert!(legacy::complete_processing_job_with_note(
        &margins,
        &job.job_id,
        job.attempt,
        "workspace",
        "notes/late.md",
        None,
        0,
    )
    .unwrap_err()
    .to_string()
    .contains("late processing result"));
    assert!(legacy::get_note_association(&margins, "session")
        .unwrap()
        .is_none());
}
