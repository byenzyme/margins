use chrono::Local;
use margins_store::legacy;
use margins_workflows::{archive, transcript_view};

#[test]
fn archive_round_trip_moves_registered_and_legacy_aligned_markdown() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let margins = work.join(".margins");
    let now = Local::now();

    legacy::create_session(&margins, "legacy", &now, ".margins/legacy.md").unwrap();
    std::fs::write(margins.join("legacy_aligned.md"), "legacy transcript").unwrap();
    legacy::upsert_session_artifact(
        &margins,
        "legacy",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/legacy_aligned.md",
        "durable",
        None,
    )
    .unwrap();

    legacy::create_session(&margins, "modern", &now, ".margins/modern.md").unwrap();
    let modern = margins.join("artifacts/modern/transcript.md");
    std::fs::create_dir_all(modern.parent().unwrap()).unwrap();
    std::fs::write(&modern, "modern transcript").unwrap();
    legacy::upsert_session_artifact(
        &margins,
        "modern",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/artifacts/modern/transcript.md",
        "durable",
        None,
    )
    .unwrap();
    std::fs::write(margins.join("orphan_aligned.md"), "orphan transcript").unwrap();

    let enabled = archive::set_enabled(work, true).unwrap();
    assert!(enabled.enabled);
    assert_eq!(enabled.moved, 3);
    assert_eq!(enabled.transcripts, 3);
    assert!(archive::is_enabled(work));
    for name in ["legacy", "modern", "orphan"] {
        assert!(work.join(format!("_margins/{name}_aligned.md")).is_file());
    }
    assert!(!margins.join("legacy_aligned.md").exists());
    assert!(!modern.exists());
    assert_eq!(
        legacy::list_session_artifacts(&margins, "modern").unwrap()[0].path,
        "_margins/modern_aligned.md"
    );
    assert_eq!(
        transcript_view::preferred_transcript_path(&margins, "modern"),
        Some(work.join("_margins/modern_aligned.md"))
    );

    let disabled = archive::set_enabled(work, false).unwrap();
    assert!(!disabled.enabled);
    assert_eq!(disabled.moved, 3);
    assert_eq!(disabled.transcripts, 0);
    assert!(!archive::is_enabled(work));
    assert!(!work.join("_margins").exists());
    for name in ["legacy", "modern", "orphan"] {
        assert!(margins.join(format!("{name}_aligned.md")).is_file());
    }
    assert_eq!(
        legacy::list_session_artifacts(&margins, "modern").unwrap()[0].path,
        ".margins/modern_aligned.md"
    );
}

#[test]
fn archive_conflict_aborts_before_moving_or_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let margins = work.join(".margins");
    let archive_dir = work.join("_margins");
    legacy::create_session(&margins, "meet", &Local::now(), ".margins/meet.md").unwrap();
    std::fs::write(margins.join("meet_aligned.md"), "local").unwrap();
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(archive_dir.join("meet_aligned.md"), "remote").unwrap();
    legacy::upsert_session_artifact(
        &margins,
        "meet",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/meet_aligned.md",
        "durable",
        None,
    )
    .unwrap();

    let error = archive::set_enabled(work, true).unwrap_err().to_string();
    assert!(error.contains("Refusing to overwrite existing transcript"));
    assert_eq!(
        std::fs::read_to_string(margins.join("meet_aligned.md")).unwrap(),
        "local"
    );
    assert_eq!(
        std::fs::read_to_string(archive_dir.join("meet_aligned.md")).unwrap(),
        "remote"
    );
    assert!(!archive::is_enabled(work));
    assert_eq!(
        legacy::list_session_artifacts(&margins, "meet").unwrap()[0].path,
        ".margins/meet_aligned.md"
    );
}
