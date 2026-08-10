use chrono::{Duration, Local};
use margins_store::legacy;
use margins_workflows::artifacts::{
    confined_artifact_registry_disk_path, list_artifacts, prune_expired_artifacts,
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
fn prune_rejects_cross_session_targets() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let start = Local::now();
    for session in ["meet", "other"] {
        legacy::create_session(&dir, session, &start, &format!(".margins/{session}.md")).unwrap();
    }
    let other = dir.join("artifacts/other/tmp.bin");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, b"other session").unwrap();
    legacy::upsert_session_artifact(
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
        legacy::list_session_artifacts(&dir, "meet").unwrap().len(),
        1
    );
}

#[test]
fn listing_preserves_exact_legacy_transcript_sidecars_without_probing_other_paths() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    legacy::create_session(&dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
    std::fs::write(dir.join("meet_aligned.md"), "legacy").unwrap();
    legacy::upsert_session_artifact(
        &dir,
        "meet",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/meet_aligned.md",
        "durable",
        None,
    )
    .unwrap();
    assert!(list_artifacts(work, &dir, "meet").unwrap()[0].exists);

    legacy::upsert_session_artifact(
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

#[cfg(unix)]
#[test]
fn prune_rejects_symlinked_session_ancestors() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let outside = temp.path().join("outside");
    let start = Local::now();
    legacy::create_session(&dir, "meet", &start, ".margins/meet.md").unwrap();
    std::fs::create_dir_all(dir.join("artifacts")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("tmp.bin"), b"must survive").unwrap();
    symlink(&outside, dir.join("artifacts/meet")).unwrap();
    legacy::upsert_session_artifact(
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
        legacy::list_session_artifacts(&dir, "meet").unwrap().len(),
        1
    );
}

#[test]
fn prune_deletes_confined_file_before_registry_row_and_keeps_rejected_row() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    let start = Local::now();
    legacy::create_session(&dir, "meet", &start, ".margins/meet.md").unwrap();
    let valid = dir.join("artifacts/meet/tmp.bin");
    std::fs::create_dir_all(valid.parent().unwrap()).unwrap();
    std::fs::write(&valid, b"temporary").unwrap();
    let expired = (start - Duration::days(1)).to_rfc3339();
    legacy::upsert_session_artifact(
        &dir,
        "meet",
        "tmp",
        0,
        ".margins/artifacts/meet/tmp.bin",
        "temporary",
        Some(&expired),
    )
    .unwrap();
    legacy::upsert_session_artifact(
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
    let remaining = legacy::list_session_artifacts(&dir, "meet").unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, "../outside");
}
