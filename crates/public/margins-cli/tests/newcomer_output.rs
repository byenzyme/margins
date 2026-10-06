//! Human output for a CLI-only newcomer, and the `--json` contracts it must
//! leave byte for byte unchanged for the bb plugin, skills, and scripts.

use clap::Parser;
use margins_cli::args::Args;
use margins_cli::run;
use margins_cli::services::CliServices;
use margins_workflows::workspace;
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _guard: std::sync::MutexGuard<'static, ()>,
    temp: tempfile::TempDir,
    old_home: Option<std::ffi::OsString>,
}

impl Fixture {
    /// A Margins home with Workspace `practice` over a notes folder that has
    /// a `people` folder.
    fn new() -> Self {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let old_home = std::env::var_os("MARGINS_HOME");
        std::env::set_var("MARGINS_HOME", temp.path().join("machine"));
        std::env::remove_var("MARGINS_WORKSPACE");
        std::fs::create_dir_all(temp.path().join("vault/people")).unwrap();
        std::fs::write(temp.path().join("vault/people/ada.md"), "# Ada\n").unwrap();
        workspace::create_workspace(
            &temp.path().join("machine"),
            "practice",
            None,
            &temp.path().join("vault"),
        )
        .unwrap();
        Self {
            _guard: guard,
            temp,
            old_home,
        }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn program_path(&self) -> std::path::PathBuf {
        self.root().join("machine/configs/practice.enzyme")
    }

    fn invoke(&self, args: &[&str]) -> (Result<(), margins_cli::CliError>, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(
            &CliServices::default(),
            &self.root().join("vault"),
            std::iter::once("margins").chain(args.iter().copied()),
            &mut stdout,
            &mut stderr,
        );
        (
            result,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    fn ok(&self, args: &[&str]) -> (String, String) {
        let (result, stdout, stderr) = self.invoke(args);
        assert!(result.is_ok(), "margins {args:?}: {stderr}");
        (stdout, stderr)
    }

    /// A desired program that reads the people folder and leaves out
    /// `archive`.
    fn desired(&self) -> std::path::PathBuf {
        let current = std::fs::read_to_string(self.program_path()).unwrap();
        let desired = current.replace(
            "  remember in folder",
            "  learn questions from folder \"people\" about relationships\n  leave out folders [\"archive\"]\n\n  remember in folder",
        );
        assert_ne!(desired, current);
        let path = self.root().join("desired.enzyme");
        std::fs::write(&path, desired).unwrap();
        path
    }

    /// Replace the temp root and content hashes so output can be compared.
    fn normalize(&self, output: &str) -> String {
        let root = self.root().to_string_lossy().into_owned();
        let output = output.replace(&root, "<root>");
        let mut normalized = String::new();
        let mut run = String::new();
        for character in output.chars().chain(std::iter::once('\0')) {
            if character.is_ascii_hexdigit() && character.is_ascii_lowercase()
                || character.is_ascii_digit()
            {
                run.push(character);
                continue;
            }
            normalized.push_str(if run.len() == 64 { "<sha>" } else { &run });
            run.clear();
            if character != '\0' {
                normalized.push(character);
            }
        }
        normalized
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        match &self.old_home {
            Some(value) => std::env::set_var("MARGINS_HOME", value),
            None => std::env::remove_var("MARGINS_HOME"),
        }
    }
}

/// Captured from v0.4.17 (`origin/main` at cb45809) before human output existed.
const PLAN_JSON_SNAPSHOT: &str = r##"{
  "schema_version": "margins.workspace.plan.v2",
  "workspace_id": "practice",
  "base_revision": "<sha>",
  "plan_id": "<sha>",
  "actions": [
    {
      "action": "set_policy",
      "summary": "Attention policy: learn questions from folder:people; leave out folders archive",
      "before": {
        "excluded_folders": [],
        "excluded_tags": [],
        "entities": [],
        "excluded_entities": []
      },
      "after": {
        "excluded_folders": [
          "archive"
        ],
        "excluded_tags": [],
        "entities": [
          {
            "folder:people": {
              "profile": "relationships",
              "expandable": false
            }
          }
        ],
        "excluded_entities": []
      }
    }
  ],
  "desired_program": "// Enzyme reading configuration\n\nworkspace \"practice\" {\n  source margins-captures \"captures\" {\n    path \"<root>/machine/workspaces/practice/captures\"\n  }\n\n  source markdown \"home\" { path \"<root>/vault\" }\n\n\n  learn questions from folder \"people\" about relationships\n  leave out folders [\"archive\"]\n\n  remember in folder \".\" create note\n}\n\n",
  "desired_sha256": "<sha>",
  "diff": "--- a/practice.enzyme\n+++ b/practice.enzyme\n@@ -8,6 +8,9 @@\n   source markdown \"home\" { path \"<root>/vault\" }\n \n \n+  learn questions from folder \"people\" about relationships\n+  leave out folders [\"archive\"]\n+\n   remember in folder \".\" create note\n }\n \n",
  "program_plan": {
    "schema": "enzyme.plan.v1",
    "workspace": "practice",
    "target": "practice.enzyme",
    "base_revision": "<sha>",
    "desired": "// Enzyme reading configuration\n\nworkspace \"practice\" {\n  source margins-captures \"captures\" {\n    path \"<root>/machine/workspaces/practice/captures\"\n  }\n\n  source markdown \"home\" { path \"<root>/vault\" }\n\n\n  learn questions from folder \"people\" about relationships\n  leave out folders [\"archive\"]\n\n  remember in folder \".\" create note\n}\n\n",
    "desired_sha256": "<sha>",
    "changes": [
      {
        "statement": "reading",
        "action": "added",
        "workspace": "practice",
        "name": "folder:people",
        "summary": "Learn questions from folder:people about relationships"
      },
      {
        "statement": "exclusion",
        "action": "added",
        "workspace": "practice",
        "name": "folder archive",
        "summary": "Leave out folder \"archive\""
      }
    ],
    "diff": "--- a/practice.enzyme\n+++ b/practice.enzyme\n@@ -8,6 +8,9 @@\n   source markdown \"home\" { path \"<root>/vault\" }\n \n \n+  learn questions from folder \"people\" about relationships\n+  leave out folders [\"archive\"]\n+\n   remember in folder \".\" create note\n }\n \n",
    "plan_id": "<sha>"
  }
}
"##;
const APPLY_JSON_SNAPSHOT: &str = r##"{
  "schema_version": "margins.workspace.apply.v2",
  "ok": true,
  "workspace_id": "practice",
  "request_id": "workspace-apply-<sha>",
  "request_hash": "<sha>",
  "plan_id": "<sha>",
  "before_revision": "<sha>",
  "after_revision": "<sha>",
  "replayed": false,
  "actions": [
    {
      "position": 0,
      "status": "applied",
      "action": "set_policy",
      "summary": "Attention policy: learn questions from folder:people; leave out folders archive",
      "before": {
        "excluded_folders": [],
        "excluded_tags": [],
        "entities": [],
        "excluded_entities": []
      },
      "after": {
        "excluded_folders": [
          "archive"
        ],
        "excluded_tags": [],
        "entities": [
          {
            "folder:people": {
              "profile": "relationships",
              "expandable": false
            }
          }
        ],
        "excluded_entities": []
      }
    }
  ]
}
"##;

#[test]
fn plan_and_apply_json_stay_byte_identical() {
    let fixture = Fixture::new();
    let desired = fixture.desired();
    let (plan, _) = fixture.ok(&[
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        fixture.normalize(&plan),
        PLAN_JSON_SNAPSHOT,
        "plan --json changed:\n{}",
        fixture.normalize(&plan)
    );
    let plan_path = fixture.root().join("plan.json");
    std::fs::write(&plan_path, &plan).unwrap();
    let (receipt, _) = fixture.ok(&[
        "--workspace",
        "practice",
        "workspace",
        "apply",
        "--plan",
        plan_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        fixture.normalize(&receipt),
        APPLY_JSON_SNAPSHOT,
        "apply --json changed:\n{}",
        fixture.normalize(&receipt)
    );
}

#[test]
fn readable_plan_saves_the_exact_json_plan_and_apply_names_the_program() {
    let fixture = Fixture::new();
    let desired = fixture.desired();
    let plan_args = [
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
    ];
    let (readable, _) = fixture.ok(&plan_args);
    let (json, _) = fixture.ok(&[&plan_args[..], &["--json"]].concat());
    let parsed: workspace::WorkspacePlan = serde_json::from_str(&json).unwrap();
    let saved = margins_cli::commands::workspace_text::plan_file_path("practice", &parsed.plan_id);
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        json,
        "the saved plan is the --json plan"
    );

    let program = fixture.program_path();
    let vault = fixture.root().join("vault");
    let expected_head = format!(
        "Workspace practice: plan for {}\n\nChanges:\n  \
         • Learn from the people folder (new)\n  \
         • Leave out the archive folder\n\n\
         Once applied, Margins:\n  \
         Learns from people\n  \
         Leaves out archive\n  \
         Notes will go to {}\n\n\
         Exact change to the program:\n",
        program.display(),
        vault.display()
    );
    assert!(readable.starts_with(&expected_head), "{readable}");
    assert!(readable.contains(&parsed.diff), "{readable}");
    assert!(
        readable.ends_with(&format!(
            "\nNothing is applied yet. To apply exactly this plan:\n  margins --workspace practice workspace apply --plan {}\n",
            saved.display()
        )),
        "{readable}"
    );

    let (applied, _) = fixture.ok(&[
        "--workspace",
        "practice",
        "workspace",
        "apply",
        "--plan",
        saved.to_str().unwrap(),
    ]);
    let revision = &parsed.desired_sha256[..12];
    assert_eq!(
        applied,
        format!(
            "Applied to Workspace practice (revision {revision}):\n  \
             • Learn from the people folder (new)\n  \
             • Leave out the archive folder\n\n\
             Your Workspace is the program at {}\n  \
             Read it:   margins --workspace practice workspace show --text\n  \
             Change it: margins --workspace practice workspace edit\n",
            program.display()
        )
    );
    let _ = std::fs::remove_file(&saved);

    // Planning what the program already says changes and saves nothing.
    let (same, _) = fixture.ok(&plan_args);
    assert!(
        same.contains("No changes: the program already says this."),
        "{same}"
    );
    assert!(same.ends_with("\nNothing to apply.\n"), "{same}");
    assert!(!saved.exists());
}

#[test]
fn status_show_and_edit_point_at_the_program() {
    let fixture = Fixture::new();
    let program = fixture.program_path();

    let (status, _) = fixture.ok(&["--workspace", "practice", "workspace", "status"]);
    assert!(
        status.starts_with(&format!(
            "Workspace: practice\nProgram: {}\nHome: ",
            program.display()
        )),
        "{status}"
    );
    assert!(status.ends_with(
        "Read the program with `margins --workspace practice workspace show --text`; change it with `margins --workspace practice workspace edit`.\n"
    ), "{status}");
    // The JSON status is unchanged: no new keys.
    let (json, _) = fixture.ok(&["--workspace", "practice", "workspace", "status", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys = json
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "build",
            "config",
            "home",
            "id",
            "name",
            "recall",
            "revision",
            "state_dir"
        ]
    );

    let (path, hint) = fixture.ok(&["--workspace", "practice", "workspace", "show"]);
    assert_eq!(path, format!("{}\n", program.display()));
    assert_eq!(
        hint,
        "That file is the program for Workspace practice. Read it with `margins --workspace practice workspace show --text`; change it with `margins --workspace practice workspace edit`.\n"
    );
    let (text, quiet) = fixture.ok(&["--workspace", "practice", "workspace", "show", "--text"]);
    assert_eq!(text, std::fs::read_to_string(&program).unwrap());
    assert!(quiet.is_empty());

    // Tests never run with a terminal on both stdin and stdout.
    let (refused, _, stderr) = fixture.invoke(&["--workspace", "practice", "workspace", "edit"]);
    assert_eq!(
        refused.unwrap_err().code(),
        "workspace_edit_requires_terminal"
    );
    for expected in [
        "margins --workspace practice workspace show --text",
        "margins --workspace practice workspace plan --desired program.enzyme",
        "margins --workspace practice workspace apply --plan",
    ] {
        assert!(stderr.contains(expected), "{expected}: {stderr}");
    }
    assert!(!stderr.contains("--json"), "{stderr}");
}

#[test]
fn help_and_guide_explain_the_program_in_plain_words() {
    let help = Args::try_parse_from(["margins", "--help"])
        .unwrap_err()
        .to_string();
    assert!(
        help.contains("Your Workspace is one editable program, $MARGINS_HOME/configs/<id>.enzyme"),
        "{help}"
    );
    assert!(
        help.contains("margins --workspace <id> workspace show --text"),
        "{help}"
    );
    assert!(
        help.contains("margins --workspace <id> workspace edit"),
        "{help}"
    );
    assert!(help.contains("margins guide glossary"), "{help}");
    assert!(!help.contains("memory boundary"), "{help}");
    assert!(help.contains("enzyme"), "{help}");

    let workspace_help = Args::try_parse_from(["margins", "workspace", "--help"])
        .unwrap_err()
        .to_string();
    assert!(
        workspace_help.contains("Each Workspace is one editable program"),
        "{workspace_help}"
    );
    assert!(
        workspace_help.contains("workspace show --text"),
        "{workspace_help}"
    );

    // Plan and apply no longer require --json.
    Args::try_parse_from(["margins", "workspace", "plan", "--desired", "d.enzyme"]).unwrap();
    Args::try_parse_from(["margins", "workspace", "apply", "--plan", "p.json"]).unwrap();
    // `margins enzyme` passes everything through, help included.
    let parsed = Args::try_parse_from(["margins", "enzyme", "scan", "--json", "--help"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Enzyme { ref args }) if args.len() == 3
    ));

    let fixture = Fixture::new();
    let (glossary, _) = fixture.ok(&["guide", "glossary"]);
    for word in [
        "Workspace",
        "program",
        "reading",
        "catalyst",
        "profile",
        "learn questions",
        "enzyme",
    ] {
        assert!(
            glossary.contains(&format!("\n{word}")),
            "{word} missing:\n{glossary}"
        );
    }
    assert!(glossary.contains("never ~/.enzyme"));

    // The public build has no bundled engine to run.
    let (result, _, _) = fixture.invoke(&["enzyme", "status"]);
    assert_eq!(result.unwrap_err().code(), "composition_unavailable");
}
