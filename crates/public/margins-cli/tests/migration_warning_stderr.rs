//! Migration is automatic: the first command that resolves a retired
//! Workspace for writing (here `init --json`) migrates it. Its warnings
//! travel through `log` to the binary's stderr logger, while stdout keeps
//! only the command's JSON.

use std::path::Path;
use std::process::{Command, Output};

fn legacy_home_with_ignored_entity(root: &Path) -> std::path::PathBuf {
    let margins_home = root.join("margins-home");
    let notes = root.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let legacy_dir = margins_home.join("workspaces/odd");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("config.toml"),
        format!(
            "id = \"odd\"\n\n[policy]\nentities = [\"person:ada\", \"#craft\"]\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n",
            notes.canonicalize().unwrap()
        ),
    )
    .unwrap();
    margins_home
}

fn migrate(margins_home: &Path, rust_log: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_margins-public"));
    command
        .args(["--workspace", "odd", "init", "--json"])
        .env_clear()
        .env("HOME", margins_home.parent().unwrap())
        .env("MARGINS_HOME", margins_home);
    if let Some(filter) = rust_log {
        command.env("RUST_LOG", filter);
    }
    command.output().unwrap()
}

#[test]
fn automatic_migration_warning_reaches_stderr_and_stdout_stays_json() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = legacy_home_with_ignored_entity(temp.path());

    let output = migrate(&margins_home, None);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["schema_version"], "margins.init.v1");
    assert_eq!(receipt["workspace"]["id"], "odd");
    assert!(margins_home.join("configs/odd.enzyme").is_file());
    let line = stderr
        .lines()
        .find(|line| line.starts_with("margins: warning: migrated Workspace 'odd': "))
        .unwrap_or_else(|| panic!("no migration warning on stderr: {stderr:?}"));
    assert!(line.contains("person:ada"), "{line}");
}

#[test]
fn rust_log_overrides_the_default_warning_filter() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = legacy_home_with_ignored_entity(temp.path());

    let output = migrate(&margins_home, Some("off"));
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["workspace"]["id"], "odd");
}
