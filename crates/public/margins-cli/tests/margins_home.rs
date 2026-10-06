//! The Margins home is an Enzyme home, through the real binary in temp homes:
//! legacy machine config and index names migrate, and `workspace show|edit`
//! read and edit the Workspace program through plan/apply.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    root: tempfile::TempDir,
    margins_home: PathBuf,
    notes: PathBuf,
    tmp: PathBuf,
}

impl Home {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let margins_home = base.join("margins-home");
        let notes = base.join("notes");
        let tmp = base.join("tmp");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        let home = Self {
            root,
            margins_home,
            notes,
            tmp,
        };
        let created = home.run(
            &["workspace", "new", "practice", "--home", home.notes.to_str().unwrap()],
            &[],
            "",
        );
        assert!(created.status.success(), "{}", stderr(&created));
        home
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(relative)
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)], stdin: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_margins-public"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.path("user-home"))
            .env("MARGINS_HOME", &self.margins_home)
            .env("TMPDIR", &self.tmp)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.notes)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn program_path(&self) -> PathBuf {
        self.margins_home.join("configs/practice.enzyme")
    }

    fn program(&self) -> String {
        std::fs::read_to_string(self.program_path()).unwrap()
    }

    /// An `$EDITOR` that replaces the file with `bodies[n]` on its n-th run.
    fn editor(&self, name: &str, bodies: &[&str]) -> String {
        let dir = self.path(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut script = String::from("#!/bin/sh\nset -e\n");
        script.push_str(&format!("count_file='{}'\n", dir.join("count").display()));
        script.push_str("n=$(cat \"$count_file\" 2>/dev/null || echo 0)\necho $((n + 1)) > \"$count_file\"\n");
        for (index, body) in bodies.iter().enumerate() {
            let body_path = dir.join(format!("body-{index}"));
            std::fs::write(&body_path, body).unwrap();
            script.push_str(&format!(
                "if [ \"$n\" = {index} ]; then cp '{}' \"$1\"; fi\n",
                body_path.display()
            ));
        }
        let script_path = dir.join("editor.sh");
        std::fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        script_path.to_str().unwrap().to_string()
    }

    fn edit(&self, editor: &str, answers: &str) -> Output {
        self.run(
            &["workspace", "edit"],
            &[
                ("MARGINS_WORKSPACE_EDIT_ASSUME_TERMINAL", "1"),
                ("EDITOR", editor),
            ],
            answers,
        )
    }

    fn leftover_edits(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.tmp)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("margins-practice-"))
            })
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn with_learning(program: &str) -> String {
    program.replace(
        "  remember in folder",
        "  learn questions from folder \"people\" about relationships {\n    sample by time\n  }\n\n  remember in folder",
    )
}

#[test]
fn legacy_machine_config_and_index_name_migrate_on_first_write() {
    let home = Home::new();
    let legacy = "# machine\n[workspace]\ndefault = \"practice\"\n\n[workspace.names]\npractice = \"Practice\"\n\n[retention]\nraw_cache_max_age_days = 30\n\n[llm]\nmode = \"local\"\nlocal_model = \"fixture-model\"\n\n[cli]\nnote_agent = \"codex\"\n";
    std::fs::write(home.margins_home.join("config.toml"), legacy).unwrap();
    let state = home.margins_home.join("workspaces/practice");
    std::fs::write(state.join("index.db"), b"index bytes").unwrap();
    std::fs::write(state.join("index.identity"), b"identity\n").unwrap();

    let listed = home.run(&["workspace", "list", "--json"], &[], "");
    assert!(listed.status.success(), "{}", stderr(&listed));
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["default_workspace"], "practice");
    assert_eq!(
        listed["workspaces"],
        serde_json::json!([{ "id": "practice", "name": "Practice" }]),
        "settings.enzyme is not a Workspace"
    );
    // Listing is read-only: it shows the migrated values without writing.
    assert_eq!(
        std::fs::read_to_string(home.margins_home.join("config.toml")).unwrap(),
        legacy
    );
    assert!(!home.margins_home.join("margins.toml").exists());
    assert!(!home.margins_home.join("configs/settings.enzyme").exists());
    assert!(state.join("index.db").exists());

    // The first command that writes the home migrates it.
    let set = home.run(&["workspace", "default", "--set", "practice", "--json"], &[], "");
    assert!(set.status.success(), "{}", stderr(&set));

    assert!(!home.margins_home.join("config.toml").exists());
    assert_eq!(
        std::fs::read_to_string(home.margins_home.join("config.toml.migrated")).unwrap(),
        legacy
    );
    let config = std::fs::read_to_string(home.margins_home.join("margins.toml")).unwrap();
    assert!(config.contains("default = \"practice\""), "{config}");
    assert!(config.contains("raw_cache_max_age_days = 30"), "{config}");
    assert!(config.contains("note_agent = \"codex\""), "{config}");
    assert!(!config.contains("[llm]"), "{config}");
    let settings =
        std::fs::read_to_string(home.margins_home.join("configs/settings.enzyme")).unwrap();
    assert!(settings.contains("generation local"), "{settings}");
    assert!(settings.contains("model \"fixture-model\""), "{settings}");
    assert!(settings.contains("updates disabled"), "{settings}");

    assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");
    assert!(!state.join("index.db").exists());
    assert_eq!(std::fs::read(state.join("index.identity")).unwrap(), b"identity\n");

    let status = home.run(&["workspace", "status", "--json"], &[], "");
    assert!(status.status.success(), "{}", stderr(&status));
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["name"], "Practice");

    // Idempotent: nothing more to migrate, nothing rewritten.
    let again = home.run(&["workspace", "default", "--json"], &[], "");
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(
        std::fs::read_to_string(home.margins_home.join("margins.toml")).unwrap(),
        config
    );
    assert!(!home.margins_home.join("config.toml.migrated.1").exists());
}

#[test]
fn invalid_legacy_machine_config_is_left_in_place() {
    let home = Home::new();
    let legacy = "[llm]\nmode = \"cloud\"\n";
    std::fs::write(home.margins_home.join("config.toml"), legacy).unwrap();
    let listed = home.run(&["workspace", "default", "--json"], &[], "");
    assert!(!listed.status.success());
    assert!(stderr(&listed).contains("unsupported [llm] mode"), "{}", stderr(&listed));
    assert_eq!(
        std::fs::read_to_string(home.margins_home.join("config.toml")).unwrap(),
        legacy
    );
    assert!(!home.margins_home.join("margins.toml").exists());
    assert!(!home.margins_home.join("configs/settings.enzyme").exists());
}

#[test]
fn workspace_show_prints_the_program_path_text_and_revision() {
    let home = Home::new();
    let selected = ["--workspace", "practice", "workspace", "show"];

    let shown = home.run(&selected, &[], "");
    assert!(shown.status.success(), "{}", stderr(&shown));
    assert_eq!(stdout(&shown), format!("{}\n", home.program_path().display()));

    let text = home.run(&[&selected[..], &["--text"]].concat(), &[], "");
    assert_eq!(stdout(&text), home.program());

    let json = home.run(&[&selected[..], &["--json", "--text"]].concat(), &[], "");
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["workspace_id"], "practice");
    assert_eq!(json["program_path"], home.program_path().to_str().unwrap());
    assert_eq!(json["program"], home.program());
    assert_eq!(json["revision"].as_str().unwrap().len(), 64);
}

#[test]
fn workspace_edit_refuses_without_a_terminal_or_an_editor() {
    let home = Home::new();
    let before = home.program();
    let editor = home.editor("editor", &[&with_learning(&before)]);

    let refused = home.run(&["workspace", "edit"], &[("EDITOR", &editor)], "y\n");
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("needs an interactive terminal"),
        "{}",
        stderr(&refused)
    );

    let no_editor = home.run(
        &["workspace", "edit"],
        &[("MARGINS_WORKSPACE_EDIT_ASSUME_TERMINAL", "1")],
        "y\n",
    );
    assert!(!no_editor.status.success());
    assert!(stderr(&no_editor).contains("$VISUAL or $EDITOR"), "{}", stderr(&no_editor));
    assert_eq!(home.program(), before);
    assert!(home.leftover_edits().is_empty());
}

#[test]
fn workspace_edit_shows_the_diff_and_applies_a_valid_edit() {
    let home = Home::new();
    let desired = with_learning(&home.program());
    let editor = home.editor("editor", &[&desired]);

    let edited = home.edit(&editor, "y\n");

    assert!(edited.status.success(), "{}", stderr(&edited));
    let out = stdout(&edited);
    assert!(out.contains("+  learn questions from folder \"people\""), "{out}");
    assert!(out.contains("Applied. Workspace practice is at revision"), "{out}");
    assert!(stderr(&edited).contains("Apply this change to Workspace practice?"));
    assert_eq!(home.program(), desired);
    assert!(home.leftover_edits().is_empty());

    // An unchanged edit is a no-op.
    let unchanged = home.edit("true", "");
    assert!(unchanged.status.success(), "{}", stderr(&unchanged));
    assert!(stdout(&unchanged).contains("No changes"));
    assert_eq!(home.program(), desired);
}

#[test]
fn workspace_edit_declined_keeps_the_text_and_applies_nothing() {
    let home = Home::new();
    let before = home.program();
    let desired = with_learning(&before);
    let editor = home.editor("editor", &[&desired]);

    let declined = home.edit(&editor, "n\n");

    assert!(!declined.status.success());
    assert_eq!(home.program(), before);
    let kept = home.leftover_edits();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), desired);
    let message = stderr(&declined);
    assert!(message.contains("Not applied."), "{message}");
    assert!(message.contains(kept[0].to_str().unwrap()), "{message}");
}

#[test]
fn workspace_edit_invalid_text_is_kept_and_can_be_fixed_in_the_editor() {
    let home = Home::new();
    let before = home.program();
    let invalid = format!("{before}this is not enzyme\n");

    // Refusing to reopen keeps the user's text and explains the way back.
    let editor = home.editor("refuse", &[&invalid]);
    let refused = home.edit(&editor, "n\n");
    assert!(!refused.status.success());
    let message = stderr(&refused);
    assert!(message.contains("The edited program is not valid"), "{message}");
    assert!(message.contains("margins --workspace practice workspace plan --desired"), "{message}");
    assert_eq!(home.program(), before);
    let kept = home.leftover_edits();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), invalid);
    std::fs::remove_file(&kept[0]).unwrap();

    // Reopening lets the user fix it; the fixed text is then applied.
    let desired = with_learning(&before);
    let editor = home.editor("fix", &[&invalid, &desired]);
    let fixed = home.edit(&editor, "y\ny\n");
    assert!(fixed.status.success(), "{}", stderr(&fixed));
    assert!(stderr(&fixed).contains("Reopen the editor to fix it?"));
    assert_eq!(home.program(), desired);
    assert!(home.leftover_edits().is_empty());
}

#[test]
fn workspace_edit_never_applies_a_program_for_another_workspace() {
    let home = Home::new();
    let before = home.program();
    let renamed = before.replace("workspace \"practice\"", "workspace \"other\"");
    assert_ne!(renamed, before);
    let editor = home.editor("editor", &[&renamed]);
    let refused = home.edit(&editor, "n\n");
    assert!(!refused.status.success());
    assert_eq!(home.program(), before);
    assert!(Path::new(&home.leftover_edits()[0]).is_file());
}

#[test]
fn workspace_edit_end_of_input_never_loops_or_applies() {
    let home = Home::new();
    let before = home.program();
    let editor = home.editor("editor", &[&format!("{before}not enzyme\n")]);
    let refused = home.edit(&editor, "");
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("The edited program is not valid"));
    assert_eq!(
        std::fs::read_to_string(home.path("editor/count")).unwrap().trim(),
        "1",
        "the editor opened once"
    );
    assert_eq!(home.program(), before);
    assert_eq!(home.leftover_edits().len(), 1);
}

#[test]
fn a_workspace_named_settings_is_renamed_through_the_binary() {
    let home = Home::new();
    let practice = home.program();
    let settings_program = practice
        .replace("workspace \"practice\"", "workspace \"settings\"")
        .replace("workspaces/practice/captures", "workspaces/settings/captures");
    std::fs::write(home.margins_home.join("configs/settings.enzyme"), &settings_program).unwrap();
    let state = home.margins_home.join("workspaces/settings");
    std::fs::create_dir_all(state.join("captures")).unwrap();
    std::fs::write(state.join("enzyme.db"), b"index").unwrap();
    std::fs::write(
        home.margins_home.join("margins.toml"),
        "[workspace]\ndefault = \"settings\"\n",
    )
    .unwrap();

    let refused = home.run(&["workspace", "list", "--json"], &[], "");
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("margins workspace rename settings <new-id>"),
        "{}",
        stderr(&refused)
    );

    let renamed = home.run(&["workspace", "rename", "settings", "home-notes", "--json"], &[], "");
    assert!(renamed.status.success(), "{}", stderr(&renamed));
    let renamed: serde_json::Value = serde_json::from_slice(&renamed.stdout).unwrap();
    assert_eq!(renamed["new_id"], "home-notes");

    let listed = home.run(&["workspace", "list", "--json"], &[], "");
    assert!(listed.status.success(), "{}", stderr(&listed));
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["default_workspace"], "home-notes");
    let ids = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["id"].as_str().unwrap(), entry.get("error").is_none()))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![("home-notes", true), ("practice", true)]);
    assert_eq!(
        std::fs::read(home.margins_home.join("workspaces/home-notes/enzyme.db")).unwrap(),
        b"index"
    );
    assert!(!home.margins_home.join("configs/settings.enzyme").exists());
    assert!(home
        .margins_home
        .join("configs/settings.enzyme.renamed-to-home-notes")
        .is_file());
}
