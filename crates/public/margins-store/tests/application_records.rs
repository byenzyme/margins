use chrono::Local;
use margins_store::canonical;
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempdir().unwrap();
    let margins = temporary.path().join(".margins");
    canonical::create_session(&margins, "session", &Local::now(), "session.md").unwrap();
    canonical::add_segment(
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
    let failed = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    canonical::update_processing_job(
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

    let linked = canonical::link_note(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        0,
    )
    .unwrap();
    assert_eq!(linked.revision, 1);
    let job = canonical::get_processing_job(&margins, "note:session")
        .unwrap()
        .unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.failure.as_deref(), Some("model unavailable"));
    assert_eq!(
        canonical::get_session_meta(&margins, "session")
            .unwrap()
            .segments
            .len(),
        1
    );

    let replay = canonical::link_note(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        0,
    )
    .unwrap();
    assert_eq!(replay.revision, 1, "exact retry is idempotent");
    let distilled = canonical::link_note_with_distillation(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        1,
        Some("thr-first"),
        Some("memo-v1"),
    )
    .unwrap();
    assert_eq!(distilled.bb_thread_ids, vec!["thr-first"]);
    assert_eq!(
        distilled.distilled_memo_revision.as_deref(),
        Some("memo-v1")
    );
    let refined = canonical::link_note_with_distillation(
        &margins,
        "session",
        "workspace",
        "inbox/session.md",
        Some("sha256:abc"),
        2,
        Some("thr-second"),
        Some("memo-v2"),
    )
    .unwrap();
    assert_eq!(refined.bb_thread_ids, vec!["thr-first", "thr-second"]);
    assert_eq!(
        canonical::get_note_association(&margins, "session")
            .unwrap()
            .unwrap()
            .distilled_memo_revision
            .as_deref(),
        Some("memo-v2")
    );
    assert!(canonical::link_note(
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
    assert!(
        canonical::link_note(&margins, "session", "workspace", "../escape.md", None, 1,).is_err()
    );
}

#[test]
fn cancelled_or_superseded_job_attempt_rejects_late_success_after_reopen() {
    let (_temporary, margins) = store();
    let first = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    canonical::cancel_processing_job(&margins, &first.job_id, first.attempt).unwrap();
    assert!(canonical::update_processing_job(
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

    let second = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    assert_eq!(second.attempt, first.attempt + 1);
    assert!(canonical::update_processing_job(
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
    let reopened = canonical::get_processing_job(&margins, "note:session")
        .unwrap()
        .unwrap();
    assert_eq!(reopened.attempt, second.attempt);
    assert_eq!(reopened.status, "queued");

    let third = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-2",
    )
    .unwrap();
    assert_eq!(third.attempt, second.attempt + 1);
    assert_eq!(third.input_revision, "input-2");
    assert!(canonical::update_processing_job(
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
    let job = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    let linked = canonical::link_note(
        &margins,
        "session",
        "workspace",
        "notes/session.md",
        None,
        0,
    )
    .unwrap();
    canonical::unlink_note(&margins, "session", linked.revision).unwrap();
    assert!(audio.exists());
    assert_eq!(
        canonical::get_processing_job(&margins, &job.job_id)
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
}

#[test]
fn note_publication_and_exact_job_completion_are_atomic_and_retryable() {
    let (_temporary, margins) = store();
    let job = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    let linked = canonical::complete_processing_job_with_note(
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
        canonical::get_processing_job(&margins, &job.job_id)
            .unwrap()
            .unwrap()
            .status,
        "complete"
    );

    let retry = canonical::complete_processing_job_with_note(
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
    let job = canonical::begin_processing_job(
        &margins,
        "session",
        "note:session",
        "distill_note",
        "input-1",
    )
    .unwrap();
    canonical::cancel_processing_job(&margins, &job.job_id, job.attempt).unwrap();
    assert!(canonical::complete_processing_job_with_note(
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
    assert!(canonical::get_note_association(&margins, "session")
        .unwrap()
        .is_none());
}
