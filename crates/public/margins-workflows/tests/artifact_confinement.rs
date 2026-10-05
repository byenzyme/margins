use chrono::{Duration, Local};
use margins_store::canonical;
use margins_workflows::artifacts::{
    confined_artifact_registry_disk_path, confined_session_artifact_access_disk_path,
    list_artifacts, prune_expired_artifacts,
};

#[test]
fn confinement_rejects_absolute_traversal_and_shallow_targets() {
    let root = std::path::Path::new("/project/.margins");
    assert_eq!(
        confined_artifact_registry_disk_path(root, ".margins/artifacts/meet/transcript.md"),
        Some(root.join("artifacts/meet/transcript.md"))
    );
    for path in [
        "/tmp/file",
        ".margins/artifacts",
        ".margins/artifacts/meet",
        ".margins/artifacts/../escape",
        ".margins/other/file",
    ] {
        assert!(
            confined_artifact_registry_disk_path(root, path).is_none(),
            "accepted {path}"
        );
    }
}

#[test]
fn listing_missing_storage_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let margins = temp.path().join(".margins");
    assert!(list_artifacts(temp.path(), &margins, "missing")
        .unwrap()
        .is_empty());
    assert!(!margins.exists());
}

#[test]
fn listing_legacy_storage_does_not_create_runtime_blob_directory_or_tables() {
    let temp = tempfile::tempdir().unwrap();
    let margins = temp.path().join(".margins");
    canonical::create_session(&margins, "meet", &Local::now(), ".margins/meet.md").unwrap();
    assert!(!margins.join("meeting-blobs").exists());
    assert!(list_artifacts(temp.path(), &margins, "meet")
        .unwrap()
        .is_empty());
    assert!(!margins.join("meeting-blobs").exists());
    let conn = rusqlite::Connection::open(margins.join("sessions.sqlite")).unwrap();
    let runtime_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meeting_sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_tables, 0);
}

#[test]
fn listing_does_not_wait_for_an_active_sqlite_writer() {
    let temp = tempfile::tempdir().unwrap();
    let margins = temp.path().join(".margins");
    canonical::create_session(&margins, "meet", &Local::now(), ".margins/meet.md").unwrap();
    let writer = rusqlite::Connection::open(margins.join("sessions.sqlite")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(list_artifacts(temp.path(), &margins, "meet")
        .unwrap()
        .is_empty());
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn prune_rejects_cross_session_targets() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let start = Local::now();
    for session in ["meet", "other"] {
        canonical::create_session(&dir, session, &start, &format!(".margins/{session}.md")).unwrap();
    }
    let other = dir.join("artifacts/other/tmp.bin");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, b"other session").unwrap();
    canonical::upsert_session_artifact(
        &dir,
        "meet",
        "tmp",
        0,
        ".margins/artifacts/other/tmp.bin",
        "temporary",
        Some(&(start - Duration::days(1)).to_rfc3339()),
    )
    .unwrap();

    let report = prune_expired_artifacts(&dir, start).unwrap();
    assert_eq!(
        report.rejected_paths,
        vec![".margins/artifacts/other/tmp.bin"]
    );
    assert!(other.exists());
    assert_eq!(
        canonical::list_session_artifacts(&dir, "meet").unwrap().len(),
        1
    );
}

#[test]
fn listing_preserves_exact_legacy_transcript_sidecars_without_probing_other_paths() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    canonical::create_session(&dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
    std::fs::write(dir.join("meet_aligned.md"), "legacy").unwrap();
    canonical::upsert_session_artifact(
        &dir,
        "meet",
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/meet_aligned.md",
        "durable",
        None,
    )
    .unwrap();
    assert!(list_artifacts(work, &dir, "meet").unwrap()[0].exists);

    canonical::upsert_session_artifact(
        &dir,
        "meet",
        "unsafe",
        0,
        &work.join("outside").to_string_lossy(),
        "durable",
        None,
    )
    .unwrap();
    let listed = list_artifacts(work, &dir, "meet").unwrap();
    assert!(
        !listed
            .iter()
            .find(|item| item.artifact.kind == "unsafe")
            .unwrap()
            .exists
    );
}

#[test]
fn listing_recovers_exact_terminal_capture_artifacts_only_for_their_session() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["meet_seg0.wav", "meet_seg0.live-transcript.json"] {
        std::fs::write(dir.join(file), "intact").unwrap();
        assert_eq!(
            confined_session_artifact_access_disk_path(&dir, "meet", &format!(".margins/{file}")),
            Some(dir.join(file))
        );
    }
    for path in [
        ".margins/other_seg0.wav",
        ".margins/meet_segx.wav",
        ".margins/meet_seg0.txt",
        ".margins/nested/meet_seg0.wav",
    ] {
        assert_eq!(
            confined_session_artifact_access_disk_path(&dir, "meet", path),
            None,
            "accepted {path}"
        );
    }
}

#[cfg(unix)]
#[test]
fn prune_rejects_symlinked_session_ancestors() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let outside = temp.path().join("outside");
    let start = Local::now();
    canonical::create_session(&dir, "meet", &start, ".margins/meet.md").unwrap();
    std::fs::create_dir_all(dir.join("artifacts")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("tmp.bin"), b"must survive").unwrap();
    symlink(&outside, dir.join("artifacts/meet")).unwrap();
    canonical::upsert_session_artifact(
        &dir,
        "meet",
        "tmp",
        0,
        ".margins/artifacts/meet/tmp.bin",
        "temporary",
        Some(&(start - Duration::days(1)).to_rfc3339()),
    )
    .unwrap();

    let report = prune_expired_artifacts(&dir, start).unwrap();
    assert_eq!(
        report.rejected_paths,
        vec![".margins/artifacts/meet/tmp.bin"]
    );
    assert_eq!(
        std::fs::read(outside.join("tmp.bin")).unwrap(),
        b"must survive"
    );
    assert_eq!(
        canonical::list_session_artifacts(&dir, "meet").unwrap().len(),
        1
    );
}

#[test]
fn prune_deletes_confined_file_before_registry_row_and_keeps_rejected_row() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let start = Local::now();
    canonical::create_session(&dir, "meet", &start, ".margins/meet.md").unwrap();
    let valid = dir.join("artifacts/meet/tmp.bin");
    std::fs::create_dir_all(valid.parent().unwrap()).unwrap();
    std::fs::write(&valid, b"temporary").unwrap();
    let expired = (start - Duration::days(1)).to_rfc3339();
    canonical::upsert_session_artifact(
        &dir,
        "meet",
        "tmp",
        0,
        ".margins/artifacts/meet/tmp.bin",
        "temporary",
        Some(&expired),
    )
    .unwrap();
    canonical::upsert_session_artifact(
        &dir,
        "meet",
        "tmp",
        1,
        "../outside",
        "temporary",
        Some(&expired),
    )
    .unwrap();

    let report = prune_expired_artifacts(&dir, start).unwrap();
    assert_eq!(report.deleted, 1);
    assert_eq!(report.registry_rows, 1);
    assert_eq!(report.rejected_paths, vec!["../outside"]);
    assert!(!valid.exists());
    let remaining = canonical::list_session_artifacts(&dir, "meet").unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, "../outside");
}
