//! Session commands record into a Workspace, never a per-folder `.margins/`,
//! and per-folder stores from earlier releases stay readable. Runs the shipped
//! binary against temp homes only.

use chrono::{Local, TimeZone};
use margins_store::canonical;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Machine {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("user/.config")).unwrap();
        Self { _temp: temp, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("margins-home")
    }

    fn folder(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_margins-private"))
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("user"))
            .env("XDG_CONFIG_HOME", self.root.join("user/.config"))
            .env("MARGINS_HOME", self.home())
            .output()
            .unwrap()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A per-folder store as earlier releases wrote it.
fn seed_session(store_root: &Path, name: &str) {
    let margins_dir = store_root.join(".margins");
    fs::create_dir_all(&margins_dir).unwrap();
    let started = Local.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap();
    canonical::create_session(&margins_dir, name, &started, &format!(".margins/{name}.md"))
        .unwrap();
    fs::write(
        margins_dir.join(format!("{name}.md")),
        "[00:01] earlier memo\n",
    )
    .unwrap();
    fs::write(margins_dir.join("current"), format!("{name}\n")).unwrap();
}

fn create_workspace(machine: &Machine, id: &str, notes: &Path) -> PathBuf {
    let output = machine.run(
        &machine.root,
        &[
            "workspace",
            "new",
            id,
            "--home",
            notes.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    machine.home().join(format!("workspaces/{id}/captures"))
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                entries.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn recording_without_a_workspace_points_at_init_and_creates_nothing() {
    let machine = Machine::new();
    let folder = machine.folder("folder");
    fs::write(folder.join("input.wav"), b"not audio").unwrap();

    for args in [
        vec!["new", "--title", "Nowhere"],
        vec!["attach"],
        vec!["transcribe", "input.wav"],
        vec!["ls"],
        vec!["current"],
    ] {
        let output = machine.run(&folder, &args);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(&output).contains("margins init"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    assert!(!folder.join(".margins").exists());
    assert!(!machine.home().join("workspaces").exists());
    assert!(!machine.root.join("user/.config/margins").exists());
}

#[test]
fn old_layout_store_stays_readable_and_untouched() {
    let machine = Machine::new();
    let old = machine.folder("old-vault");
    seed_session(&old, "earlier-meeting");
    let before = snapshot(&old.join(".margins"));

    // No Workspace anywhere: the folder's own sessions are read in place.
    let output = machine.run(&old, &["current"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("earlier-meeting"));
    let output = machine.run(&old, &["ls"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("earlier-meeting"));
    let output = machine.run(&old, &["new"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("margins init"),
        "{}",
        stderr(&output)
    );

    // A default Workspace elsewhere takes new recordings; the old sessions are
    // still the ones this folder shows.
    let notes = machine.folder("notes");
    let captures = create_workspace(&machine, "practice", &notes);
    let output = machine.run(
        &machine.root,
        &["workspace", "default", "--set", "practice"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let output = machine.run(&old, &["current"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("earlier-meeting"));
    let output = machine.run(&old, &["transcribe", "missing.wav"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Using Workspace practice (default)"),
        "{}",
        stderr(&output)
    );

    // `--project <path>` reads the old store from anywhere, but never records.
    let old_arg = old.to_str().unwrap();
    let output = machine.run(&notes, &["--project", old_arg, "current"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("earlier-meeting"));
    let output = machine.run(&notes, &["--project", old_arg, "new"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("legacy_store_read_only"),
        "{}",
        stderr(&output)
    );

    assert_eq!(snapshot(&old.join(".margins")), before, "old store changed");
    assert!(!captures.join(".margins").exists(), "nothing was adopted");
    assert!(!notes.join(".margins").exists());
}

#[test]
fn covering_workspace_wins_and_names_the_earlier_store() {
    let machine = Machine::new();
    let notes = machine.folder("notes");
    seed_session(&notes, "earlier-meeting");
    let captures = create_workspace(&machine, "practice", &notes);
    seed_session(&captures, "workspace-meeting");
    let inbox = machine.folder("notes/inbox");

    let output = machine.run(&inbox, &["current"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("workspace-meeting"),
        "{}",
        stdout(&output)
    );
    assert!(!stderr(&output).contains("(default)"));
    assert!(
        stderr(&output).contains("stay readable with `margins --project"),
        "{}",
        stderr(&output)
    );

    let output = machine.run(&inbox, &["--project", notes.to_str().unwrap(), "current"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("earlier-meeting"));
    assert!(!inbox.join(".margins").exists());
}
