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
    _temp: tempfile::TempDir,
    /// The temp directory with symlinks resolved (macOS `/var` is
    /// `/private/var`), so it matches the paths Margins stores and prints.
    root: std::path::PathBuf,
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
        let root = temp.path().canonicalize().unwrap();
        let old_home = std::env::var_os("MARGINS_HOME");
        std::env::set_var("MARGINS_HOME", root.join("machine"));
        std::env::remove_var("MARGINS_WORKSPACE");
        std::fs::create_dir_all(root.join("vault/people")).unwrap();
        std::fs::write(root.join("vault/people/ada.md"), "# Ada\n").unwrap();
        workspace::create_workspace(
            &root.join("machine"),
            "practice",
            None,
            &root.join("vault"),
        )
        .unwrap();
        Self {
            _guard: guard,
            _temp: temp,
            root,
            old_home,
        }
    }

    fn root(&self) -> &Path {
        &self.root
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
    let saved = saved_plan(&readable);
    let plans = fixture.root().join("machine/plans");
    assert_eq!(saved.parent(), Some(plans.as_path()), "{readable}");
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
            "\nNothing is applied yet. To apply exactly this plan:\n  margins workspace apply --plan {}\n",
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
             See what it learns: margins --workspace practice status\n  \
             Change it:          margins --workspace practice edit\n",
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
    assert_eq!(std::fs::read_dir(&plans).unwrap().count(), 0);
}

/// The plan path in a readable plan's apply command.
fn saved_plan(readable: &str) -> std::path::PathBuf {
    readable
        .lines()
        .find_map(|line| line.trim().strip_prefix("margins workspace apply --plan "))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("no apply command in:\n{readable}"))
}

#[cfg(unix)]
#[test]
fn readable_plans_are_private_files_with_random_names_and_never_follow_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let desired = fixture.desired();
    let args = [
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
    ];
    let plans = fixture.root().join("machine/plans");
    let (first, _) = fixture.ok(&args);
    let (second, _) = fixture.ok(&args);
    let (first, second) = (saved_plan(&first), saved_plan(&second));
    assert_ne!(first, second, "each plan gets its own random name");
    assert_eq!(
        plans.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    for saved in [&first, &second] {
        assert_eq!(
            saved.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    // A pre-planted `plans` symlink is refused and its target is untouched.
    std::fs::remove_dir_all(&plans).unwrap();
    let elsewhere = fixture.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &plans).unwrap();
    let (refused, stdout, stderr) = fixture.invoke(&args);
    assert_eq!(
        refused.unwrap_err().code(),
        "workspace_plan_unwritable",
        "{stderr}"
    );
    assert!(stdout.is_empty());
    assert!(stderr.contains("is not a private directory"), "{stderr}");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(plans.symlink_metadata().unwrap().file_type().is_symlink());
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
        "Read the program with `margins --workspace practice edit --print`; change it with `margins --workspace practice edit`.\n"
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
        "That file is the program for Workspace practice. Read it with `margins --workspace practice edit --print`; change it with `margins --workspace practice edit`.\n"
    );
    let (text, quiet) = fixture.ok(&["--workspace", "practice", "workspace", "show", "--text"]);
    assert_eq!(text, std::fs::read_to_string(&program).unwrap());
    assert!(quiet.is_empty());

    // `edit --print`, `--path`, and `--json` are `workspace show` for people.
    assert_eq!(
        fixture.ok(&["--workspace", "practice", "edit", "--print"]).0,
        text
    );
    assert_eq!(
        fixture.ok(&["--workspace", "practice", "edit", "--path"]).0,
        path
    );
    assert_eq!(
        fixture.ok(&["--workspace", "practice", "edit", "--json"]).0,
        fixture
            .ok(&["--workspace", "practice", "workspace", "show", "--text", "--json"])
            .0
    );

    // Tests never run with a terminal on both stdin and stdout: edit (and its
    // hidden `workspace edit` alias) changes nothing and says how to.
    for args in [
        &["--workspace", "practice", "edit"][..],
        &["--workspace", "practice", "workspace", "edit"],
    ] {
        let (refused, _, stderr) = fixture.invoke(args);
        assert_eq!(
            refused.unwrap_err().code(),
            "workspace_edit_requires_terminal"
        );
        for expected in [
            "nothing was changed",
            &format!("The program is {}", program.display()),
            "save the output of `margins --workspace practice edit --print` as program.enzyme",
            "margins --workspace practice workspace plan --desired program.enzyme",
            "margins workspace apply --plan",
        ] {
            assert!(stderr.contains(expected), "{expected}: {stderr}");
        }
        assert!(!stderr.contains("--json"), "{stderr}");
    }
}

#[test]
fn status_shows_the_index_and_catalysts_separately() {
    let fixture = Fixture::new();
    let (status, _) = fixture.ok(&["--workspace", "practice", "status"]);
    assert!(
        status.starts_with(&format!(
            "Workspace practice (selected) — notes in {}\n  Program: {} · revision ",
            fixture.root().join("vault").display(),
            fixture.program_path().display()
        )),
        "{status}"
    );
    // The public build has no engine: it never claims catalysts.
    assert!(status.contains("\nIndex: none in this build; recall reads 1 note directly\n"), "{status}");
    assert!(
        status.contains("\nCatalysts (questions Margins prepares from your notes to find related ones): not set up — search finds direct matches only\n"),
        "{status}"
    );
    assert!(status.contains("\nLearns about:\n"), "{status}");
    assert!(status.contains("\nSources:\n  captures — recordings, "), "{status}");
    assert!(status.contains("\nCaptures: none yet — `margins new` starts one\n"), "{status}");
    assert!(!status.contains("Recall: available"), "{status}");

    let (json, _) = fixture.ok(&["--workspace", "practice", "status", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["schema_version"], "margins.status.v1");
    assert_eq!(json["workspace"]["id"], "practice");
    assert_eq!(json["workspace"]["selected_by"], "selection");
    assert_eq!(json["index"]["state"], "lexical");
    assert_eq!(json["catalysts"]["usable"], false);
    assert_eq!(json["sources"].as_array().unwrap().len(), 2);

    // Without a selection, the folder's Workspace; elsewhere, none.
    let (_, _) = fixture.ok(&["status"]);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let elsewhere = margins_cli::run(
        &CliServices::default(),
        &fixture.root().join("machine"),
        ["margins", "status"],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(elsewhere.unwrap_err().code(), "workspace_required");
    assert!(
        String::from_utf8(stderr).unwrap().contains("margins init"),
        "the way out is init"
    );
}

#[test]
fn init_makes_a_workspace_for_a_named_folder_and_refreshes_a_covered_one() {
    let fixture = Fixture::new();
    // A folder already in a Workspace is refreshed, not re-made.
    let (covered, _) = fixture.ok(&["init"]);
    assert!(
        covered.starts_with(&format!(
            "Already part of Workspace practice ({})\n",
            fixture.root().join("vault").display()
        )),
        "{covered}"
    );
    assert!(!covered.contains("<margins_init"), "{covered}");
    let (json, _) = fixture.ok(&["init", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["schema_version"], "margins.init.v1");
    assert_eq!(json["created"], false);
    assert_eq!(json["covered_by"], "home");

    // A named empty folder is allowed, with a note; it becomes the default
    // only when there is none.
    let empty = fixture.root().join("fresh notes");
    std::fs::create_dir_all(&empty).unwrap();
    let (created, _) = fixture.ok(&["init", empty.to_str().unwrap(), "--json"]);
    let created: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert_eq!(created["created"], true);
    assert_eq!(created["workspace"]["id"], "fresh-notes");
    assert_eq!(created["default_set"], true);
    assert!(created["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| note.as_str().unwrap().contains("no Markdown notes")));
    assert!(fixture
        .root()
        .join("machine/configs/fresh-notes.enzyme")
        .is_file());

    // Naming an existing Workspace's folder under another id is refused, as
    // is a folder above a Workspace's notes; nothing is created.
    let (refused, _, _) = fixture.invoke(&["init", empty.to_str().unwrap(), "--id", "other"]);
    assert_eq!(refused.unwrap_err().code(), "folder_already_in_workspace");
    let (refused, _, stderr) = fixture.invoke(&["init", fixture.root().to_str().unwrap()]);
    assert_eq!(refused.unwrap_err().code(), "folder_refused", "{stderr}");
    assert!(stderr.contains("contains the notes folder of Workspace"), "{stderr}");
    // --workspace refreshes; it cannot be combined with a folder.
    let (refused, _, _) =
        fixture.invoke(&["--workspace", "practice", "init", empty.to_str().unwrap()]);
    assert_eq!(refused.unwrap_err().code(), "usage");
    let (missing, _, stderr) = fixture.invoke(&["--workspace", "nope", "init"]);
    assert_eq!(missing.unwrap_err().code(), "workspace_not_found");
    assert!(stderr.contains("margins init /path/to/notes --id nope"), "{stderr}");
}

/// The commands `margins --help` lists, in order.
fn listed_commands(help: &str) -> Vec<String> {
    help.split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect()
}

#[test]
fn help_lists_journeys_and_hides_plumbing_that_stays_callable() {
    let help = Args::try_parse_from(["margins", "--help"])
        .unwrap_err()
        .to_string();
    assert_eq!(
        listed_commands(&help),
        [
            "init", "status", "edit", "sync", "recall", "connect", "disconnect", "setup", "guide",
            "new", "attach", "current", "ls", "transcript", "transcribe", "note", "help",
        ]
    );
    assert!(help.contains("Start here: run `margins init` in your notes folder"), "{help}");
    assert!(
        help.contains("Your Workspace is one editable program, $MARGINS_HOME/configs/<id>.enzyme"),
        "{help}"
    );
    assert!(help.contains("margins status --explain"), "{help}");
    assert!(help.contains("margins edit"), "{help}");
    assert!(help.contains("margins guide glossary"), "{help}");
    assert!(!help.contains("memory boundary"), "{help}");
    assert!(!help.contains("--local"), "{help}");

    // Hidden plumbing still parses exactly as before.
    for args in [
        &["margins", "workspace", "new", "x", "--home", "/n", "--name", "X", "--json"][..],
        &["margins", "workspace", "show", "--text", "--json"],
        &["margins", "workspace", "status", "--json"],
        &["margins", "workspace", "list", "--json"],
        &["margins", "workspace", "default", "--set", "x", "--json"],
        &["margins", "workspace", "destination", "--json"],
        &["margins", "workspace", "plan", "--preset", "margins-meetings", "--json"],
        &["margins", "workspace", "plan", "--desired", "d.enzyme"],
        &["margins", "workspace", "apply", "--plan", "p.json"],
        &["margins", "workspace", "edit", "--color", "never"],
        &["margins", "source", "list", "--json"],
        &["margins", "integrations", "status", "--json"],
        &["margins", "capabilities"],
        &["margins", "recent"],
        &["margins", "artifacts", "latest"],
        &["margins", "process", "latest", "--align-only"],
        &["margins", "import", "granola", "export.json"],
        &["margins", "service", "discover", "--json"],
        &["margins", "transfers", "list", "--json"],
        &["margins", "--local", "ls"],
    ] {
        Args::try_parse_from(args).unwrap_or_else(|error| panic!("{args:?}: {error}"));
    }
    assert!(Args::try_parse_from(["margins", "workspace", "edit", "--color", "sometimes"]).is_err());
    // `workspace migrate` (migration is automatic) and `margins enzyme`
    // (replaced by `status --explain`) are gone.
    assert!(Args::try_parse_from(["margins", "workspace", "migrate"]).is_err());
    assert!(Args::try_parse_from(["margins", "enzyme", "status"]).is_err());
    // The journey commands' flags.
    for args in [
        &["margins", "init", "/notes", "--id", "practice", "--no-preset", "--json"][..],
        &["margins", "status", "--explain", "--all", "--json"],
        &["margins", "edit", "--print"],
        &["margins", "edit", "--path"],
        &["margins", "edit", "--json"],
    ] {
        Args::try_parse_from(args).unwrap_or_else(|error| panic!("{args:?}: {error}"));
    }
    assert!(Args::try_parse_from(["margins", "edit", "--print", "--path"]).is_err());

    let fixture = Fixture::new();
    let (glossary, _) = fixture.ok(&["guide", "glossary"]);
    for word in [
        "Workspace",
        "program",
        "reading",
        "catalyst",
        "profile",
        "learn questions",
        "index",
        "enzyme",
    ] {
        assert!(
            glossary.contains(&format!("\n{word}")),
            "{word} missing:\n{glossary}"
        );
    }
    assert!(glossary.contains("never ~/.enzyme"));
    assert!(glossary.contains("margins status --explain"));
    assert!(!glossary.contains("margins enzyme"));
}

#[test]
fn apply_takes_the_workspace_from_the_plan_and_hints_drop_the_default_selector() {
    let fixture = Fixture::new();
    // A second Workspace, so a disagreeing --workspace exists.
    std::fs::create_dir_all(fixture.root().join("other")).unwrap();
    fixture.ok(&[
        "workspace",
        "new",
        "other",
        "--home",
        fixture.root().join("other").to_str().unwrap(),
    ]);
    let desired = fixture.desired();
    let (readable, _) = fixture.ok(&[
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
    ]);
    let saved = saved_plan(&readable);

    // An explicit --workspace that disagrees with the plan is refused.
    let (mismatch, _, stderr) = fixture.invoke(&[
        "--workspace",
        "other",
        "workspace",
        "apply",
        "--plan",
        saved.to_str().unwrap(),
    ]);
    assert_eq!(
        mismatch.unwrap_err().code(),
        "workspace_mismatch",
        "{stderr}"
    );
    assert!(std::fs::read_to_string(fixture.program_path())
        .unwrap()
        .contains("remember in folder"));

    // With practice as the machine default, apply needs only the plan and its
    // hints name no Workspace.
    fixture.ok(&["workspace", "default", "--set", "practice"]);
    let (applied, _) = fixture.ok(&["workspace", "apply", "--plan", saved.to_str().unwrap()]);
    assert!(
        applied.starts_with("Applied to Workspace practice"),
        "{applied}"
    );
    assert!(
        applied.ends_with(
            "  See what it learns: margins status\n  Change it:          margins edit\n"
        ),
        "{applied}"
    );
    let (_, hint) = fixture.ok(&["workspace", "show"]);
    assert!(
        hint.contains("Read it with `margins edit --print`"),
        "{hint}"
    );

    // Replaying the applied plan is idempotent; once the program changes
    // again, the same plan is stale and refused.
    let (replayed, _, stderr) =
        fixture.invoke(&["workspace", "apply", "--plan", saved.to_str().unwrap()]);
    assert!(replayed.is_ok(), "{stderr}");
    let current = std::fs::read_to_string(fixture.program_path()).unwrap();
    std::fs::write(fixture.program_path(), format!("{current}\n")).unwrap();
    let (refused, _, stderr) =
        fixture.invoke(&["workspace", "apply", "--plan", saved.to_str().unwrap()]);
    assert_eq!(
        refused.unwrap_err().code(),
        "workspace_revision_conflict",
        "{stderr}"
    );
}

/// The readable plan without its random plan-file name.
fn without_plan_file(readable: &str) -> String {
    let saved = saved_plan(readable);
    readable.replace(saved.to_str().unwrap(), "<plan>")
}

#[test]
fn plan_colours_the_diff_only_when_asked_or_on_a_terminal() {
    let fixture = Fixture::new();
    let desired = fixture.desired();
    let plan = |extra: &[&str]| {
        let mut args = vec![
            "--workspace",
            "practice",
            "workspace",
            "plan",
            "--desired",
            desired.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        fixture.ok(&args).0
    };
    // Tests write to a pipe: auto is plain and identical to never.
    let auto = plan(&[]);
    let never = plan(&["--color", "never"]);
    assert_eq!(without_plan_file(&auto), without_plan_file(&never));
    assert!(!auto.contains('\x1b'), "{auto}");

    let always = plan(&["--color", "always"]);
    assert!(
        always.contains(
            "\x1b[32m+  learn questions from folder \"people\" about relationships\x1b[0m\n"
        ),
        "{always}"
    );
    assert!(
        always.contains("\n--- a/practice.enzyme\n+++ b/practice.enzyme\n"),
        "{always}"
    );
    // Only the diff is coloured: stripping the codes gives the plain output.
    let stripped = always
        .replace("\x1b[32m", "")
        .replace("\x1b[31m", "")
        .replace("\x1b[0m", "");
    assert_eq!(without_plan_file(&stripped), without_plan_file(&auto));

    // JSON is never coloured.
    let (json, _) = fixture.ok(&[
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
        "--json",
        "--color",
        "always",
    ]);
    assert!(!json.contains('\x1b'));
    serde_json::from_str::<serde_json::Value>(&json).unwrap();
}

#[cfg(unix)]
#[test]
fn saving_a_plan_prunes_old_and_excess_plans_but_never_follows_symlinks() {
    use margins_cli::commands::workspace_text::{MAX_KEPT_PLANS, PLAN_MAX_AGE};
    let fixture = Fixture::new();
    let desired = fixture.desired();
    let args = [
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        desired.to_str().unwrap(),
    ];
    let (first, _) = fixture.ok(&args);
    let plans = saved_plan(&first).parent().unwrap().to_path_buf();

    let stale = plans.join("practice-stale.plan.json");
    std::fs::write(&stale, "{}").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(
            std::time::SystemTime::now() - PLAN_MAX_AGE - std::time::Duration::from_secs(60),
        )
        .unwrap();
    for index in 0..MAX_KEPT_PLANS + 5 {
        std::fs::write(
            plans.join(format!("practice-filler{index:02}.plan.json")),
            "{}",
        )
        .unwrap();
    }
    let canary = fixture.root().join("canary.json");
    std::fs::write(&canary, "keep me").unwrap();
    let link = plans.join("practice-link.plan.json");
    std::os::unix::fs::symlink(&canary, &link).unwrap();
    let unrelated = plans.join("notes.txt");
    std::fs::write(&unrelated, "not a plan").unwrap();

    let (second, _) = fixture.ok(&args);
    let newest = saved_plan(&second);
    assert!(newest.is_file());
    assert!(!stale.exists(), "a plan past the age limit is removed");
    let kept = std::fs::read_dir(&plans)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry.file_name().to_string_lossy().ends_with(".plan.json")
                && entry.file_type().unwrap().is_file()
        })
        .count();
    assert!(kept <= MAX_KEPT_PLANS, "{kept} plans kept");
    assert!(link.symlink_metadata().is_ok(), "symlinks are left alone");
    assert_eq!(std::fs::read_to_string(&canary).unwrap(), "keep me");
    assert!(unrelated.exists());
}

#[test]
fn listing_sessions_without_a_workspace_points_to_init() {
    let fixture = Fixture::new();
    let elsewhere = fixture.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    for command in ["ls", "current"] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = margins_cli::run(
            &CliServices::default(),
            &elsewhere,
            ["margins", command],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(result.unwrap_err().code(), "workspace_required", "{command}");
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("there are no recordings to show"), "{command}: {stderr}");
        assert!(stderr.contains("Run `margins init` in your notes folder first"), "{command}: {stderr}");
        assert!(!stderr.contains("this recording"), "{command}: {stderr}");
    }
}

#[test]
fn explain_lines_never_repeat_the_reading() {
    use margins_cli::commands::status::{explain_line, ExplainReading};
    let reading = |source: &str, learns: &[&str]| ExplainReading {
        reading: source.to_string(),
        learns: learns.iter().map(|learned| learned.to_string()).collect(),
        skipped: Vec::new(),
    };
    assert_eq!(
        explain_line(&reading("folder \"Meetings\"", &["the Meetings folder"])),
        "the Meetings folder: learned about"
    );
    assert_eq!(
        explain_line(&reading(
            "folder \"People\" including linked pages",
            &["the People folder", "Alice Chen (linked page)"]
        )),
        "the People folder and the pages it links: learned about, with Alice Chen (linked page)"
    );
    assert_eq!(
        explain_line(&reading("folder \"Projects\"", &[])),
        "the Projects folder: nothing yet"
    );
}
