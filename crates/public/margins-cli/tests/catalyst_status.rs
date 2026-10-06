use std::process::Command;

#[test]
fn public_workspace_status_reports_only_live_local_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    margins_workflows::workspace::create_workspace(&margins_home, "status-fixture", None, &notes)
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margins-public"))
        .args([
            "--workspace",
            "status-fixture",
            "workspace",
            "status",
            "--json",
        ])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["recall"]["mode"], "live_lexical");
    assert!(status.get("catalyst").is_none());
    assert!(status.get("index").is_none());
    assert!(status.get("ledger").is_none());
    assert!(status.get("source_refresh_staleness").is_none());
}
