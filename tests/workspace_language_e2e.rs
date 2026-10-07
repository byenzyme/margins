#![cfg(feature = "recall")]
//! End-to-end proof of the Workspace language through the real `margins` and
//! `enzyme` binaries: program authoring, plan/apply, indexing by `enzyme`
//! from the program in `$MARGINS_HOME/configs`, recall provenance, source
//! kinds, legacy upgrade, and edit round-trips. Every run is hermetic: temp
//! `HOME` and `MARGINS_HOME`, a poisoned `ENZYME_HOME` in the environment, a
//! local fixture generator, no network, no real credentials. Each test
//! asserts that neither `ENZYME_HOME` nor `~/.enzyme` is touched.

#[path = "support/fixture_generator.rs"]
mod fixture_generator;

#[path = "support/enzyme_bin.rs"]
mod enzyme_bin;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::Utc;
use margins_workflows::integrations::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx,
    GoogleCalendarScope, HealthStatus, IntegrationsStore, ParticipantThread, ThreadEvidence,
    EMAIL_CONNECTOR_ID, GOOGLE_CALENDAR_CONNECTOR_ID,
};
use margins_workflows::workspace::{CalendarCollectionSelector, GmailCollectionSelector};

const BIN: &str = env!("CARGO_BIN_EXE_margins-private");
const ACCOUNT: &str = "owner@example.com";

struct Hermetic {
    root: tempfile::TempDir,
    home: PathBuf,
    margins_home: PathBuf,
    enzyme_home: PathBuf,
    enzyme_before: BTreeMap<PathBuf, Vec<u8>>,
}

impl Hermetic {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let home = root_path.join("home");
        let margins_home = root_path.join("margins-home");
        let enzyme_home = root_path.join("enzyme-home");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&margins_home).unwrap();
        // A poisoned Enzyme home: any read would fail to parse, any write
        // would change the snapshot.
        fs::create_dir_all(enzyme_home.join("configs")).unwrap();
        fs::write(enzyme_home.join("config.toml"), "this is = = not toml\n").unwrap();
        fs::write(
            enzyme_home.join("configs/poison.enzyme"),
            "workspace {{ unparseable\n",
        )
        .unwrap();
        let enzyme_before = snapshot(&enzyme_home);
        Self {
            root,
            home,
            margins_home,
            enzyme_home,
            enzyme_before,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(relative)
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_bin(Path::new(BIN), args)
    }

    fn run_bin(&self, bin: &Path, args: &[&str]) -> Output {
        self.command(bin, args).output().unwrap()
    }

    fn command(&self, bin: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(bin);
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("MARGINS_HOME", &self.margins_home)
            .env("ENZYME_HOME", &self.enzyme_home)
            .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("MARGINS_RECALL_DEBUG", "1")
            .current_dir(&self.home);
        command
    }

    /// Run the engine directly on the Margins home, with the hosted bundle the
    /// fixture generator installed as its `--llm env` endpoint.
    fn enzyme(&self, args: &[&str]) -> Output {
        let bundle: serde_json::Value = serde_json::from_slice(
            &fs::read(self.margins_home.join("llm-config-cache.json")).unwrap(),
        )
        .unwrap();
        Command::new(enzyme_bin::enzyme_bin())
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("ENZYME_HOME", &self.margins_home)
            .env("OPENAI_API_KEY", bundle["api_key"].as_str().unwrap())
            .env("OPENAI_BASE_URL", bundle["base_url"].as_str().unwrap())
            .env("OPENAI_MODEL", bundle["model"].as_str().unwrap())
            .current_dir(&self.home)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "margins {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.ok(args);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "margins {args:?} stdout is not JSON ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn program(&self, id: &str) -> String {
        fs::read_to_string(self.margins_home.join(format!("configs/{id}.enzyme"))).unwrap()
    }

    /// Plan a desired program file and apply the exact plan; returns the plan.
    fn plan_and_apply(&self, id: &str, desired: &str) -> serde_json::Value {
        let desired_path = self.path(&format!("{id}-desired.enzyme"));
        fs::write(&desired_path, desired).unwrap();
        let plan = self.ok(&[
            "--workspace",
            id,
            "workspace",
            "plan",
            "--desired",
            desired_path.to_str().unwrap(),
            "--json",
        ]);
        let plan_path = self.path(&format!("{id}-plan.json"));
        fs::write(&plan_path, &plan.stdout).unwrap();
        let receipt = self.json(&[
            "--workspace",
            id,
            "workspace",
            "apply",
            "--plan",
            plan_path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(receipt["ok"], true, "{receipt}");
        serde_json::from_slice(&plan.stdout).unwrap()
    }

    fn assert_enzyme_untouched(&self) {
        assert_eq!(
            snapshot(&self.enzyme_home),
            self.enzyme_before,
            "ENZYME_HOME must be neither read-modified nor written"
        );
        assert!(
            !self.home.join(".enzyme").exists(),
            "~/.enzyme must never be created"
        );
    }
}

fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.insert(path.clone(), Vec::new());
                stack.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn recall(env: &Hermetic, id: &str, query: &str, source: Option<&str>) -> serde_json::Value {
    let mut args = vec!["--workspace", id, "recall", "--json"];
    if let Some(source) = source {
        args.extend(["--source", source]);
    }
    args.push(query);
    env.json(&args)
}

fn result_refs(recalled: &serde_json::Value) -> Vec<String> {
    recalled["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["document_ref"].as_str().unwrap().to_string())
        .collect()
}

fn indexed_refs(state_dir: &Path) -> Vec<String> {
    let index = rusqlite::Connection::open(state_dir.join("enzyme.db")).unwrap();
    let mut statement = index
        .prepare("SELECT source_ref FROM docs ORDER BY source_ref")
        .unwrap();
    let refs = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    refs
}

fn notes_fixture(env: &Hermetic) -> PathBuf {
    let notes = env.path("notes");
    fs::create_dir_all(notes.join("inbox")).unwrap();
    write(
        &notes.join("projects/harbor.md"),
        "# Harbor\n\nThe quartz harbor ledger records every crossing tonight.\n",
    );
    write(
        &notes.join("archive/secret.md"),
        "# Sealed\n\nThe obsidian lantern archive phrase must never be recalled.\n",
    );
    notes
}

/// Four tagged planning notes no preset reading covers: material for the
/// engine's automatic selection.
fn planning_fixture(notes: &Path) {
    for index in 0..4 {
        write(
            &notes.join(format!("Planning/plan-{index}.md")),
            &format!(
                "# Plan {index}\n\n#roadmap planning record {index} weighs launch sequencing, \
                 staffing, budget tradeoffs, vendor dependencies, migration risk, pricing \
                 experiments, support load, hiring timing, quarterly milestones, partner \
                 commitments, analytics instrumentation, and rollout communication.\n"
            ),
        );
    }
}

/// Four substantive people notes, enough evidence for a folder catalyst.
fn people_fixture(notes: &Path) {
    for (index, name) in ["Ada Chen", "Ben Patel", "Cara Jones", "Diego Ruiz"]
        .into_iter()
        .enumerate()
    {
        write(
            &notes.join(format!("people/note-{index}.md")),
            &format!(
                "# {name}\n\n{name} relationship context {index} covers trust cadence decision \
                 memory planning followup customer signal roadmap ambiguity delivery ownership \
                 repair feedback commitments priorities constraints introductions support \
                 escalation notes strategy collaboration review learning questions history \
                 meeting tone risks outcomes alignment handoff next steps.\n"
            ),
        );
    }
}

/// Fresh setup: author a program, plan and apply it, index, and recall a
/// planted phrase with its real path; excluded folders are unreachable; the
/// note destination resolves inside the declared folder; a destination that
/// escapes the source is refused at plan time.
#[test]
fn fresh_program_setup_plans_applies_indexes_and_recalls() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    env.ok(&[
        "workspace",
        "new",
        "practice",
        "--home",
        notes.to_str().unwrap(),
        "--json",
    ]);
    let desired = format!(
        r#"workspace "practice" {{
  source markdown "notes" {{ path "{}" }}
  remember in folder "inbox" create note
  leave out folders ["archive"]
}}
"#,
        notes.display()
    );
    let plan = env.plan_and_apply("practice", &desired);
    assert_eq!(plan["schema_version"], "margins.workspace.plan.v2");
    assert_eq!(env.program("practice"), plan["desired_program"].as_str().unwrap());
    assert!(!env
        .margins_home
        .join("workspaces/practice/config.toml")
        .exists());

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    let init = env.ok(&["--workspace", "practice", "init"]);
    let stderr = String::from_utf8_lossy(&init.stderr);
    assert!(
        stderr.contains("engine index first_build"),
        "{stderr}"
    );
    let state = env.margins_home.join("workspaces/practice");
    // One Markdown source: root-relative identity, archive never indexed.
    assert_eq!(indexed_refs(&state), ["projects/harbor.md"]);

    let recalled = recall(
        &env,
        "practice",
        "The quartz harbor ledger records every crossing",
        None,
    );
    assert_eq!(recalled["status"], "ok", "{recalled}");
    let hit = recalled["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["document_ref"] == "projects/harbor.md")
        .unwrap_or_else(|| panic!("planted phrase not recalled: {recalled}"));
    assert_eq!(hit["source"], "notes");
    assert_eq!(hit["evidence"]["kind"], "native_markdown");
    assert_eq!(
        hit["evidence"]["path"],
        notes.join("projects/harbor.md").to_str().unwrap()
    );

    let excluded = recall(
        &env,
        "practice",
        "The obsidian lantern archive phrase must never be recalled",
        None,
    );
    assert!(
        result_refs(&excluded)
            .iter()
            .all(|reference| !reference.contains("archive")),
        "leave out folders content must not be recallable: {excluded}"
    );

    let destination = env.json(&["--workspace", "practice", "workspace", "destination", "--json"]);
    let destination_text = destination.to_string();
    assert!(
        destination_text.contains(notes.join("inbox").to_str().unwrap()),
        "{destination}"
    );

    // A destination outside the declaring source is refused before apply.
    let before = env.program("practice");
    let escaping = desired.replace(r#"folder "inbox""#, r#"folder "../outside""#);
    let escaping_path = env.path("escaping.enzyme");
    fs::write(&escaping_path, escaping).unwrap();
    let refused = env.run(&[
        "--workspace",
        "practice",
        "workspace",
        "plan",
        "--desired",
        escaping_path.to_str().unwrap(),
        "--json",
    ]);
    assert!(!refused.status.success());
    let refusal = String::from_utf8_lossy(&refused.stdout).to_string()
        + &String::from_utf8_lossy(&refused.stderr);
    assert!(refusal.contains("outside"), "{refusal}");
    assert_eq!(env.program("practice"), before);

    env.assert_enzyme_untouched();
}

fn seed_mail_and_calendar(state_dir: &Path) {
    let store = IntegrationsStore::open(state_dir).unwrap();
    let mail = ConnectorCtx {
        vault_root: state_dir.to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: ACCOUNT.to_string(),
        command_path: None,
    };
    let now = Utc::now();
    let mut threads = Vec::new();
    let mut associations = Vec::new();
    for index in 0..4 {
        let thread_id = format!("thread-{index}");
        let at = now - chrono::Duration::days(index + 1);
        threads.push(ThreadEvidence {
            thread_id: thread_id.clone(),
            occurred_from: at,
            occurred_to: at,
            body_text: format!(
                "Marigold procurement thread {index}: Ada Chen confirms the vendor shortlist, \
                 budget owner, delivery window, review cadence, risk register, acceptance \
                 criteria, escalation path, and the next decision checkpoint."
            ),
            href: Some(format!("https://mail.example/thread-{index}")),
        });
        associations.push(ParticipantThread {
            participant: "ada@client.test".to_string(),
            thread_id: thread_id.clone(),
            last_interaction: at,
            sampling_score: Some(1_001_001),
        });
        // The account itself takes part in every thread.
        associations.push(ParticipantThread {
            participant: ACCOUNT.to_string(),
            thread_id,
            last_interaction: at,
            sampling_score: Some(1_001_001),
        });
    }
    let gmail = GmailCollectionSelector::default_declaration();
    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &mail,
            threads,
            associations,
            &gmail.materialization_fingerprint().unwrap(),
        )
        .unwrap();
    store.update_health(&mail, HealthStatus::Fresh, None).unwrap();

    let calendar = ConnectorCtx {
        connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.to_string(),
        ..mail
    };
    let selector = CalendarCollectionSelector::default_declaration();
    let scope = GoogleCalendarScope::for_selector(&selector, now).unwrap();
    let occurred_from = now + chrono::Duration::hours(2);
    store
        .apply_calendar_event_delta(
            &calendar,
            CalendarEventDelta {
                events: vec![CalendarEventEvidence {
                    source_id: "primary:kickoff".to_string(),
                    calendar_id: "primary".to_string(),
                    occurred_from,
                    occurred_to: Some(occurred_from + chrono::Duration::minutes(30)),
                    title: "Marigold kickoff".to_string(),
                    body_text: "Kickoff agenda for the marigold vendor review.".to_string(),
                    href: Some("https://calendar.google.com/event?eid=kickoff".to_string()),
                }],
                attendees: vec![CalendarEventAttendee {
                    source_id: "primary:kickoff".to_string(),
                    attendee_key: "email:ada@client.test".to_string(),
                    position: 0,
                    display_name: "Ada Chen".to_string(),
                    email: Some("ada@client.test".to_string()),
                    response_status: Some("accepted".to_string()),
                    is_self: false,
                    organizer: false,
                }],
                tombstone_source_ids: Vec::new(),
                raw_items: Vec::new(),
                scope: scope.as_range(),
                complete_snapshot: true,
                materialization_fingerprint: selector.materialization_fingerprint().unwrap(),
                next_cursor: None,
            },
            None,
        )
        .unwrap();
}

/// Host sources declared in the program lower to SQLite sources over the
/// Workspace ledger under their own names; mail content is recallable with
/// external-record provenance and `learn questions from source "mail"`
/// resolves to the lowered collection.
#[test]
fn mixed_host_sources_lower_to_ledger_sources_and_resolve_source_readings() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    env.ok(&[
        "workspace",
        "new",
        "mixed",
        "--home",
        notes.to_str().unwrap(),
        "--json",
    ]);
    let desired = format!(
        r#"workspace "mixed" {{
  source markdown "notes" {{ path "{}" }}
  source google-mail "mail" {{ account "{ACCOUNT}" }}
  source google-calendar "calendar" {{ account "{ACCOUNT}" }}
  remember in folder "inbox" create note
  leave out folders ["archive"]
  learn questions from source "mail"
}}
"#,
        notes.display()
    );
    env.plan_and_apply("mixed", &desired);
    let state = env.margins_home.join("workspaces/mixed");
    seed_mail_and_calendar(&state);

    let generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "mixed", "init"]);
    let refs = indexed_refs(&state);
    assert!(refs.contains(&"projects/harbor.md".to_string()), "{refs:?}");
    assert_eq!(
        refs.iter().filter(|r| r.starts_with("sqlite:mail/")).count(),
        4,
        "{refs:?}"
    );
    assert_eq!(
        refs.iter()
            .filter(|r| r.starts_with("sqlite:calendar/"))
            .count(),
        1,
        "{refs:?}"
    );
    assert!(refs.iter().all(|r| !r.contains("gmail_") && !r.contains("markdown_")));

    // The source reading resolved to the lowered SQLite collection.
    let index = rusqlite::Connection::open(state.join("enzyme.db")).unwrap();
    let collection: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM entities WHERE name = 'sqlite:mail' AND type = 'collection'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(collection, 1);
    // Mail people are linked; the account owner is not one of them.
    let people = |name: &str| -> i64 {
        index
            .query_row(
                "SELECT COUNT(*) FROM entities WHERE lower(name) LIKE ?1",
                [format!("%{name}%")],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert!(people("ada@client.test") > 0);
    assert_eq!(people(ACCOUNT), 0, "the mail account owner must not be a link entity");
    let mail_catalysts: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM catalysts WHERE entity = 'sqlite:mail'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        mail_catalysts > 0 && generator.request_count() > 0,
        "learn questions from source \"mail\" must select the mail collection"
    );
    drop(index);

    let recalled = recall(&env, "mixed", "Marigold procurement vendor shortlist", Some("mail"));
    assert_eq!(recalled["status"], "ok", "{recalled}");
    let hits = recalled["results"].as_array().unwrap();
    assert!(!hits.is_empty(), "mail content must be recallable: {recalled}");
    for hit in hits {
        assert_eq!(hit["source"], "mail", "{hit}");
        assert_eq!(hit["source_kind"], "google-mail", "{hit}");
        assert_eq!(hit["evidence"]["kind"], "external_record", "{hit}");
        assert_eq!(hit["evidence"]["source_account"], ACCOUNT);
        assert!(hit["document_ref"]
            .as_str()
            .unwrap()
            .starts_with("sqlite:mail/"));
    }
    // The program file itself never carries ledger SQL or hidden rules.
    let program = env.program("mixed");
    assert!(!program.contains("sqlite") && !program.contains("SELECT"));
    assert!(!program.contains("margins-managed-projection"));

    env.assert_enzyme_untouched();
}

/// A `MARGINS_HOME` laid out like the current release: legacy
/// `workspaces/<id>/config.toml` with a notes Home, a Gmail binding, policy
/// entities, and exclusions. When `MARGINS_E2E_LEGACY_BIN` names a release
/// binary, the index is built by that binary in the old identity layout;
/// otherwise the test starts from no index.
#[test]
fn legacy_release_home_migrates_and_reindexes_once() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    people_fixture(&notes);
    let state = env.margins_home.join("workspaces/legacy");
    fs::create_dir_all(&state).unwrap();
    // Exactly the shape the 2026-09 release writes.
    let legacy = format!(
        r#"id = "legacy"

[policy]
excluded_folders = ["archive"]
excluded_tags = ["private"]
entities = [{{ "folder:people" = {{ profile = "relational", expandable = true }} }}]
excluded_entities = ["[[Spam Sender]]"]

[retention]

[bindings.captures]
kind = "captures"
path = "{captures}"

[bindings.home]
kind = "notes"
path = "{notes}"
role = "home"
note_folder = "inbox"

[bindings.mail]
kind = "google-mail"
account = "{ACCOUNT}"

[bindings.mail.gmail]
query = "-in:spam -in:trash"
backfill_days = 365
"#,
        captures = state.join("captures").display(),
        notes = notes.display()
    );
    fs::write(state.join("config.toml"), &legacy).unwrap();

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    let legacy_bin = std::env::var_os("MARGINS_E2E_LEGACY_BIN").map(PathBuf::from);
    if let Some(legacy_bin) = &legacy_bin {
        let built = env.run_bin(legacy_bin, &["--workspace", "legacy", "init"]);
        assert!(
            built.status.success(),
            "legacy release init failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let old_refs = indexed_refs(&state);
        assert!(
            old_refs.iter().any(|r| r.starts_with("markdown_")),
            "the release binary builds hashed namespaces: {old_refs:?}"
        );
        assert!(state.join("config.toml").is_file());
    }

    // Status is read-only: it shows the Workspace without migrating it.
    let status = env.json(&["--workspace", "legacy", "workspace", "status", "--json"]);
    assert_eq!(status["id"], "legacy");
    assert_eq!(fs::read_to_string(state.join("config.toml")).unwrap(), legacy);

    // A command that writes the home migrates the program; there is no
    // migrate command.
    env.ok(&["workspace", "default", "--set", "legacy", "--json"]);
    assert!(!state.join("config.toml").exists());
    assert_eq!(fs::read_to_string(state.join("config.toml.migrated")).unwrap(), legacy);
    let program = env.program("legacy");
    for expected in [
        r#"source markdown "home""#,
        r#"source google-mail "mail""#,
        r#"remember in folder "inbox" create note"#,
        "archive",
        "private",
        "Spam Sender",
        r#"learn questions from folder "people""#,
        "including linked pages",
    ] {
        assert!(program.contains(expected), "missing {expected:?} in\n{program}");
    }

    // Re-planning the migrated program is a no-op.
    let replanned = env.ok(&[
        "--workspace",
        "legacy",
        "workspace",
        "plan",
        "--desired",
        env.margins_home
            .join("configs/legacy.enzyme")
            .to_str()
            .unwrap(),
        "--json",
    ]);
    let replanned: serde_json::Value = serde_json::from_slice(&replanned.stdout).unwrap();
    assert_eq!(replanned["actions"], serde_json::json!([]), "{replanned}");
    assert_eq!(replanned["diff"], "", "{replanned}");

    // Recall refuses a never-synced Gmail source; give it a fresh snapshot
    // (as `sync` would with real credentials).
    seed_mail_and_calendar(&state);

    // The engine reuses a release index it can read (re-identifying its
    // documents) or rebuilds one it cannot, once.
    let first = env.ok(&["--workspace", "legacy", "init"]);
    let first_stderr = String::from_utf8_lossy(&first.stderr);
    let expected_reason = if legacy_bin.is_some() {
        "engine index re"
    } else {
        "engine index first_build"
    };
    assert!(first_stderr.contains(expected_reason), "{first_stderr}");
    let refs = indexed_refs(&state);
    assert!(refs.contains(&"people/note-0.md".to_string()), "{refs:?}");
    assert!(refs.iter().all(|r| !r.starts_with("markdown_") && !r.contains("gmail_")));
    assert!(!refs.iter().any(|r| r.contains("archive")));
    if legacy_bin.is_some() {
        let index = rusqlite::Connection::open(state.join("enzyme.db")).unwrap();
        let stale: i64 = index
            .query_row(
                "SELECT COUNT(*) FROM catalysts WHERE entity LIKE 'markdown\\_%' ESCAPE '\\'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0, "old identity-bearing catalysts are dropped");
    }
    // `sync` would also refresh Gmail, which needs real credentials; `init`
    // is the same recall refresh without the network.
    let second = env.ok(&["--workspace", "legacy", "init"]);
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        second_stderr.contains("engine index reuse")
            && !second_stderr.contains("rebuild")
            && !second_stderr.contains("first_build")
            && !second_stderr.contains("in_process_index"),
        "the transition happens once: {second_stderr}"
    );

    let recalled = recall(
        &env,
        "legacy",
        "The quartz harbor ledger records every crossing",
        Some("home"),
    );
    assert!(
        result_refs(&recalled).contains(&"projects/harbor.md".to_string()),
        "{recalled}"
    );
    env.assert_enzyme_untouched();
}

/// Source mutations re-render the program but keep every statement the typed
/// view cannot express.
#[test]
fn program_language_survives_source_mutations() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    people_fixture(&notes);
    let library = env.path("library");
    write(&library.join("book.md"), "# Book\n\nA library note.\n");
    env.ok(&[
        "workspace",
        "new",
        "edits",
        "--home",
        notes.to_str().unwrap(),
        "--json",
    ]);
    let desired = format!(
        r#"profile clients {{
  seek "what each client needs next"
  notice {{
    "commitments"
  }}
}}

workspace "edits" {{
  source markdown "notes" {{ path "{}" }}
  remember in folder "inbox" create note
  leave out folders ["archive"]
  learn questions from folder "people" about clients {{
    sample by time
  }}
  when asked {{
    "Use grep for exact names."
  }}
}}
"#,
        notes.display()
    );
    env.plan_and_apply("edits", &desired);
    let before = env.program("edits");

    env.ok(&[
        "--workspace",
        "edits",
        "source",
        "add",
        "notes",
        "--name",
        "library",
        "--path",
        library.to_str().unwrap(),
        "--role",
        "reference",
    ]);
    let after = env.program("edits");
    assert!(after.contains(r#"source markdown "library""#), "{after}");
    for kept in [
        "profile clients",
        r#"seek "what each client needs next""#,
        "sample by time",
        "when asked",
        r#""Use grep for exact names.""#,
        "about clients",
    ] {
        assert!(after.contains(kept), "lost {kept:?}:\n{after}");
    }
    // With two Markdown sources the folder reading and note policy become
    // qualified by source name; nothing else changes.
    assert!(
        after.contains(r#"learn questions from folder "notes/people""#),
        "{after}"
    );
    assert!(after.contains(r#"in source "notes""#), "{after}");

    // Removing it restores the same program; re-rendering may reformat
    // (comments are not preserved), so compare the parsed language.
    env.ok(&["--workspace", "edits", "source", "remove", "library"]);
    let parse = |text: &str| margins_workflows::enzyme_spec::parse(text).unwrap();
    assert_eq!(
        parse(&env.program("edits")),
        parse(&before),
        "remove restores the original program"
    );
    env.assert_enzyme_untouched();
}

/// One Markdown source indexes root-relative identities; adding a second
/// changes Home identities to `<source>/…`. The next index must leave no
/// stale root-relative documents or chunks, recall must return the prefixed
/// refs, and nothing may be indexed twice.
#[test]
fn adding_a_second_markdown_source_reindexes_home_identities() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    let library = env.path("library");
    write(
        &library.join("book.md"),
        "# Book\n\nThe vermilion orchard almanac lists every graft.\n",
    );
    env.ok(&["workspace", "new", "grow", "--home", notes.to_str().unwrap(), "--json"]);
    let desired = format!(
        "workspace \"grow\" {{\n  source markdown \"notes\" {{ path \"{}\" }}\n  remember in folder \"inbox\" create note\n  leave out folders [\"archive\"]\n}}\n",
        notes.display()
    );
    env.plan_and_apply("grow", &desired);
    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "grow", "init"]);
    let state = env.margins_home.join("workspaces/grow");
    assert_eq!(indexed_refs(&state), ["projects/harbor.md"]);

    env.ok(&[
        "--workspace", "grow", "source", "add", "notes", "--name", "library", "--path",
        library.to_str().unwrap(), "--role", "reference",
    ]);
    env.ok(&["--workspace", "grow", "init"]);

    let refs = indexed_refs(&state);
    assert_eq!(refs, ["library/book.md", "notes/projects/harbor.md"], "{refs:?}");
    let index = rusqlite::Connection::open(state.join("enzyme.db")).unwrap();
    let orphan_chunks: i64 = index
        .query_row(
            "SELECT count(*) FROM chunks WHERE doc_id NOT IN (SELECT id FROM docs)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphan_chunks, 0);

    let recalled = recall(&env, "grow", "The quartz harbor ledger records every crossing", None);
    let hits = result_refs(&recalled);
    assert!(hits.contains(&"notes/projects/harbor.md".to_string()), "{recalled}");
    assert!(!hits.contains(&"projects/harbor.md".to_string()), "{recalled}");
    let mut unique = hits.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), hits.len(), "duplicate hits: {recalled}");
    let book = recall(&env, "grow", "The vermilion orchard almanac lists every graft", None);
    assert!(result_refs(&book).contains(&"library/book.md".to_string()), "{book}");
    env.assert_enzyme_untouched();
}

fn indexed_documents(state_dir: &Path) -> Vec<(String, String)> {
    let index = rusqlite::Connection::open(state_dir.join("enzyme.db")).unwrap();
    let mut statement = index
        .prepare("SELECT source_ref, content_hash FROM docs ORDER BY source_ref")
        .unwrap();
    let documents = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    documents
}

fn catalyst_entities(state_dir: &Path) -> Vec<String> {
    let index = rusqlite::Connection::open(state_dir.join("enzyme.db")).unwrap();
    let mut statement = index
        .prepare("SELECT DISTINCT entity FROM catalysts ORDER BY entity")
        .unwrap();
    let entities = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    entities
}

/// `enzyme` run directly with `ENZYME_HOME=$MARGINS_HOME` reads the Workspace
/// program, `settings.enzyme`, and `margins-sources.enzyme` that Margins
/// wrote, and builds the same index Margins builds; `enzyme search` finds
/// what `margins recall` finds.
#[test]
fn enzyme_reads_margins_programs_and_kinds_and_builds_the_same_index() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    people_fixture(&notes);
    env.ok(&["workspace", "new", "direct", "--home", notes.to_str().unwrap(), "--json"]);
    let desired = format!(
        r#"workspace "direct" {{
  source markdown "notes" {{ path "{}" }}
  source google-mail "mail" {{ account "{ACCOUNT}" }}
  source google-calendar "calendar" {{ account "{ACCOUNT}" }}
  remember in folder "inbox" create note
  leave out folders ["archive"]
}}
"#,
        notes.display()
    );
    env.plan_and_apply("direct", &desired);
    let state = env.margins_home.join("workspaces/direct");
    seed_mail_and_calendar(&state);
    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "direct", "init"]);
    let documents = indexed_documents(&state);
    let entities = catalyst_entities(&state);
    assert!(documents.iter().any(|(r, _)| r == "projects/harbor.md"), "{documents:?}");
    assert!(documents.iter().any(|(r, _)| r.starts_with("sqlite:mail/")), "{documents:?}");
    assert!(!entities.is_empty());
    let sources = fs::read_to_string(env.margins_home.join("configs/margins-sources.enzyme"))
        .unwrap();
    assert_eq!(sources, margins_workflows::source_kinds::SOURCES_TEXT);
    let settings = fs::read_to_string(env.margins_home.join("configs/settings.enzyme")).unwrap();
    assert!(settings.contains("updates disabled"), "{settings}");

    let phrase = "The quartz harbor ledger records every crossing";
    let recalled = recall(&env, "direct", phrase, None);
    let searched = env.enzyme(&[
        "--workspace", "direct", "search", phrase, "--phrase", phrase, "-n", "8", "--json",
    ]);
    assert!(searched.status.success(), "{}", String::from_utf8_lossy(&searched.stderr));
    let searched: serde_json::Value = serde_json::from_slice(&searched.stdout).unwrap();
    let engine_refs = ["exact_hits", "catalyst_hits"]
        .iter()
        .flat_map(|list| searched[list].as_array().unwrap())
        .map(|hit| hit["path"].as_str().unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let margins_refs = result_refs(&recalled);
    assert!(!margins_refs.is_empty(), "{recalled}");
    assert!(
        margins_refs.iter().all(|reference| engine_refs.contains(reference)),
        "margins {margins_refs:?} vs enzyme {engine_refs:?}"
    );

    // Rebuild the index from scratch with plain `enzyme`.
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(state.join(format!("enzyme.db{suffix}")));
    }
    let built = env.enzyme(&["--workspace", "direct", "init", "--llm", "env", "--quiet"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    assert_eq!(indexed_documents(&state), documents);
    assert_eq!(catalyst_entities(&state), entities);
    env.assert_enzyme_untouched();
}

/// A second index build of the same Workspace while one runs: with no wait it
/// fails as busy, without changing anything; with the default wait it runs
/// after the first finishes. `margins sync` reports the busy refresh.
#[test]
fn concurrent_margins_commands_map_lock_busy() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    people_fixture(&notes);
    env.ok(&["workspace", "new", "busy", "--home", notes.to_str().unwrap(), "--json"]);
    let generator = fixture_generator::FixtureGenerator::start_with_delay(&env.margins_home, 1500);
    let mut first = env
        .command(Path::new(BIN), &["--workspace", "busy", "init"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Generation runs under the workspace lock.
    let started = std::time::Instant::now();
    while generator.request_count() == 0 {
        assert!(started.elapsed().as_secs() < 60, "the first init never reached generation");
        assert!(first.try_wait().unwrap().is_none(), "the first init exited early");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let busy = env
        .command(Path::new(BIN), &["--workspace", "busy", "init"])
        .env("MARGINS_ENZYME_LOCK_TIMEOUT", "0")
        .output()
        .unwrap();
    assert!(!busy.status.success());
    let stderr = String::from_utf8_lossy(&busy.stderr);
    assert!(stderr.contains("workspace busy"), "{stderr}");

    let sync = env
        .command(Path::new(BIN), &["--workspace", "busy", "sync", "--json"])
        .env("MARGINS_ENZYME_LOCK_TIMEOUT", "0")
        .output()
        .unwrap();
    let sync_json: serde_json::Value = serde_json::from_slice(&sync.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&sync.stderr)));
    assert_eq!(sync_json["recall"]["ok"], false, "{sync_json}");
    assert!(
        sync_json["recall"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("workspace busy")),
        "{sync_json}"
    );

    let waiting = env
        .command(Path::new(BIN), &["--workspace", "busy", "init"])
        .output()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(waiting.status.success(), "{}", String::from_utf8_lossy(&waiting.stderr));
    env.assert_enzyme_untouched();
}

/// Setup is preset-only: `workspace new`, then `workspace plan --preset
/// margins-meetings` fills Margins' managed preset through `enzyme compile`,
/// keeps only readings whose folders exist, and is applied unchanged; init
/// and an exact-phrase recall prove the notes folder, and new notes go to
/// Meetings. Running setup again proposes nothing.
#[test]
fn preset_setup_keeps_existing_readings_recalls_and_is_safe_to_rerun() {
    let env = Hermetic::new();
    let notes = env.path("notes");
    write(
        &notes.join("Meetings/2026-10-01 vendor sync.md"),
        "# Vendor sync\n\nThe cobalt orchard review moved the launch to Thursday.\n",
    );
    people_fixture(&notes);
    planning_fixture(&notes);
    write(&notes.join("templates/meeting.md"), "# {{title}}\n\nThe template sentinel phrase stays out.\n");
    env.ok(&["workspace", "new", "practice", "--home", notes.to_str().unwrap(), "--json"]);

    let plan_args = [
        "--workspace", "practice", "workspace", "plan", "--preset", "margins-meetings", "--json",
    ];
    let plan = env.ok(&plan_args);
    let plan_path = env.path("preset-plan.json");
    fs::write(&plan_path, &plan.stdout).unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(plan["schema_version"], "margins.workspace.plan.v2");
    assert_eq!(
        plan["program_path"],
        env.margins_home.join("configs/practice.enzyme").to_str().unwrap()
    );
    assert_eq!(plan["preset"]["readings"], serde_json::json!(["folder:Meetings", "folder:people"]));
    assert_eq!(plan["preset"]["skipped_readings"], serde_json::json!(["folder:Projects"]));
    assert_eq!(plan["preset"]["note_folder"], "Meetings");
    // The preview writes nothing into the Margins home, not even the preset.
    assert_eq!(plan["preset"]["template"], "margins-meetings");
    assert!(!env.margins_home.join("presets").exists());
    let receipt = env.json(&[
        "--workspace", "practice", "workspace", "apply", "--plan", plan_path.to_str().unwrap(), "--json",
    ]);
    assert_eq!(receipt["ok"], true, "{receipt}");

    let program = env.program("practice");
    assert_eq!(program, plan["desired_program"].as_str().unwrap());
    assert!(program.contains(r#"learn questions from folder "Meetings""#), "{program}");
    assert!(program.contains(r#"learn questions from folder "people""#), "{program}");
    assert!(!program.contains("Projects"), "{program}");
    assert!(program.contains(r#"leave out folders ["templates", "Attachments"]"#), "{program}");
    assert!(program.contains(r#"remember in folder "Meetings" create note"#), "{program}");
    assert!(program.contains(r#"source margins-captures "captures""#), "{program}");
    assert!(program.contains("learn questions automatically\n"), "{program}");
    let programs = fs::read_dir(env.margins_home.join("configs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".enzyme"))
        .collect::<std::collections::BTreeSet<_>>();
    // Only the program: the preview prepared nothing, and the engine's
    // settings and source kinds are written when the engine first runs (init).
    assert_eq!(programs, ["practice.enzyme"].map(String::from).into());

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "practice", "init"]);
    let state = env.margins_home.join("workspaces/practice");
    assert!(indexed_refs(&state).iter().all(|reference| !reference.starts_with("templates/")));
    let phrase = "The cobalt orchard review moved the launch to Thursday";
    let recalled = recall(&env, "practice", phrase, None);
    assert_eq!(recalled["status"], "ok", "{recalled}");
    assert!(
        result_refs(&recalled).contains(&"Meetings/2026-10-01 vendor sync.md".to_string()),
        "planted phrase not recalled: {recalled}"
    );
    let destination = env.json(&["--workspace", "practice", "workspace", "destination", "--json"]);
    assert_eq!(destination["destination"], notes.join("Meetings").to_str().unwrap(), "{destination}");
    // Automatic selection runs alongside the readings: besides the people
    // reading, the engine picks what the readings miss (the Planning notes and
    // their #roadmap tag), and nothing is written back.
    let entities = catalyst_entities(&state);
    assert!(entities.iter().any(|entity| entity == "people"), "{entities:?}");
    assert!(
        entities.iter().any(|entity| entity == "planning" || entity == "roadmap"),
        "no automatic pick alongside the readings: {entities:?}"
    );
    assert_eq!(env.program("practice"), program);

    // Setup again: the same preset proposes no change and no second program.
    let again: serde_json::Value = serde_json::from_slice(&env.ok(&plan_args).stdout).unwrap();
    assert_eq!(again["actions"], serde_json::json!([]), "{again}");
    assert_eq!(again["diff"], "");
    assert_eq!(again["desired_program"], plan["desired_program"]);
    assert_eq!(env.program("practice"), program);
    let programs_after = fs::read_dir(env.margins_home.join("configs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".enzyme"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        programs_after,
        ["margins-sources.enzyme", "practice.enzyme", "settings.enzyme"]
            .map(String::from)
            .into(),
        "init added only the engine's settings and source kinds"
    );

    // A program set up before the preset had the statement gains it, and
    // only it, when setup runs again.
    let older = program.replace("  learn questions automatically\n", "");
    assert_ne!(older, program);
    env.plan_and_apply("practice", &older);
    let upgrade: serde_json::Value = serde_json::from_slice(&env.ok(&plan_args).stdout).unwrap();
    assert_eq!(upgrade["desired_program"].as_str().unwrap(), program, "{upgrade}");
    let added: Vec<&str> = upgrade["diff"]
        .as_str()
        .unwrap()
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .collect();
    assert_eq!(added, ["+  learn questions automatically"], "{upgrade}");
    assert!(
        !upgrade["diff"].as_str().unwrap().lines().any(|line| line.starts_with('-') && !line.starts_with("---")),
        "{upgrade}"
    );

    env.assert_enzyme_untouched();
}

#[test]
fn a_program_without_automatic_selection_learns_only_its_readings_and_is_unchanged() {
    let env = Hermetic::new();
    let notes = env.path("notes");
    people_fixture(&notes);
    planning_fixture(&notes);
    env.ok(&["workspace", "new", "practice", "--home", notes.to_str().unwrap(), "--json"]);
    let desired = format!(
        "workspace \"practice\" {{\n  source markdown \"home\" {{ path \"{}\" }}\n  remember in folder \".\" create note\n  learn questions from folder \"people\"\n}}\n",
        notes.display()
    );
    env.plan_and_apply("practice", &desired);
    let program = env.program("practice");
    assert!(!program.contains("automatically"), "{program}");

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "practice", "init"]);
    env.ok(&["--workspace", "practice", "init"]);
    let entities = catalyst_entities(&env.margins_home.join("workspaces/practice"));
    assert_eq!(entities, ["people"], "readings are the complete set");
    assert_eq!(env.program("practice"), program);
    env.assert_enzyme_untouched();
}

#[test]
fn two_workspaces_share_one_notes_folder_and_index_independently() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    // `alpha` leaves the archive out; `beta` reads the whole folder.
    for (id, extra) in [("alpha", "\n  leave out folders [\"archive\"]"), ("beta", "")] {
        env.ok(&[
            "workspace",
            "new",
            id,
            "--home",
            notes.to_str().unwrap(),
            "--json",
        ]);
        let desired = format!(
            "workspace \"{id}\" {{\n  source markdown \"notes\" {{ path \"{}\" }}\n  remember in folder \"inbox\" create note{extra}\n}}\n",
            notes.display()
        );
        env.plan_and_apply(id, &desired);
    }

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    env.ok(&["--workspace", "alpha", "init"]);
    let alpha = env.margins_home.join("workspaces/alpha");
    let alpha_index = indexed_documents(&alpha);
    assert_eq!(indexed_refs(&alpha), ["projects/harbor.md"]);
    // Nothing of beta's exists until beta itself is initialized.
    assert!(!env.margins_home.join("workspaces/beta/enzyme.db").exists());

    env.ok(&["--workspace", "beta", "init"]);
    let beta = env.margins_home.join("workspaces/beta");
    assert_eq!(
        indexed_refs(&beta),
        ["archive/secret.md", "projects/harbor.md"]
    );
    // Building beta's index over the same folder leaves alpha's untouched,
    // and nothing is written into the shared notes folder.
    assert_eq!(indexed_documents(&alpha), alpha_index);
    assert!(!notes.join(".enzyme").exists());

    let harbor = "The quartz harbor ledger records every crossing";
    let archive = "The obsidian lantern archive phrase must never be recalled";
    for id in ["alpha", "beta"] {
        let recalled = recall(&env, id, harbor, None);
        assert_eq!(recalled["status"], "ok", "{id}: {recalled}");
        assert!(
            result_refs(&recalled).contains(&"projects/harbor.md".to_string()),
            "{id}: {recalled}"
        );
    }
    assert!(!result_refs(&recall(&env, "alpha", archive, None))
        .contains(&"archive/secret.md".to_string()));
    assert!(result_refs(&recall(&env, "beta", archive, None))
        .contains(&"archive/secret.md".to_string()));

    // Re-initializing one Workspace changes neither index.
    let beta_index = indexed_documents(&beta);
    env.ok(&["--workspace", "alpha", "init"]);
    assert_eq!(indexed_documents(&alpha), alpha_index);
    assert_eq!(indexed_documents(&beta), beta_index);

    env.assert_enzyme_untouched();
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A CLI-only newcomer, without the bb plugin: every step names the one
/// editable program, plan and apply read in plain language without `--json`,
/// the readable plan applies exactly what `--json` would, status reports the
/// engine's state word, and `margins enzyme` runs the bundled engine on the
/// Margins home, never a canary `~/.enzyme` or the inherited `ENZYME_HOME`.
#[test]
fn cli_only_newcomer_learns_the_program_and_sees_it_through_status() {
    let env = Hermetic::new();
    let canary = env.home.join(".enzyme");
    write(
        &canary.join("configs/practice.enzyme"),
        "workspace \"practice\" { canary that must never be read }\n",
    );
    write(&canary.join("config.toml"), "canary = = not toml\n");
    let canary_before = snapshot(&canary);
    let notes = env.path("notes");
    write(
        &notes.join("Meetings/2026-10-01 vendor sync.md"),
        "# Vendor sync\n\nThe cobalt orchard review moved the launch to Thursday.\n",
    );
    people_fixture(&notes);
    fs::create_dir_all(notes.join("templates")).unwrap();
    env.ok(&["workspace", "new", "practice", "--home", notes.to_str().unwrap()]);
    let program_path = env.margins_home.join("configs/practice.enzyme");
    let block = format!(
        "Your Workspace is the program at {}\n  See what it learns: margins --workspace practice status\n  Change it:          margins --workspace practice edit\n",
        program_path.display()
    );

    // Readable plan: consequences, the exact diff, and a saved plan to apply.
    let plan = text(
        &env.ok(&["--workspace", "practice", "workspace", "plan", "--preset", "margins-meetings"])
            .stdout,
    );
    for expected in [
        format!("Workspace practice: plan for {}", program_path.display()),
        "  • Learn from the Meetings folder (new)".to_string(),
        "  • Leave out the templates folder".to_string(),
        "  Learns from Meetings · people".to_string(),
        "  Leaves out templates · Attachments".to_string(),
        format!("  Notes will go to {}", notes.join("Meetings").display()),
        "  Skipped, not in your notes: Projects (no such folder)".to_string(),
        "Exact change to the program:\n--- a/practice.enzyme".to_string(),
        "+  learn questions from folder \"Meetings\"".to_string(),
        "Nothing is applied yet.".to_string(),
    ] {
        assert!(plan.contains(&expected), "missing {expected:?} in:\n{plan}");
    }
    assert!(serde_json::from_str::<serde_json::Value>(&plan).is_err());
    let saved = plan
        .lines()
        .find_map(|line| line.trim().strip_prefix("margins workspace apply --plan "))
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("no apply command in:\n{plan}"));
    // The saved plan is byte for byte what `--json` prints.
    let json_plan = env.ok(&[
        "--workspace", "practice", "workspace", "plan", "--preset", "margins-meetings", "--json",
    ]);
    assert_eq!(fs::read(&saved).unwrap(), json_plan.stdout);
    assert!(!env.margins_home.join("presets").exists());

    let applied = text(
        &env.ok(&["--workspace", "practice", "workspace", "apply", "--plan", saved.to_str().unwrap()])
            .stdout,
    );
    assert!(applied.starts_with("Applied to Workspace practice (revision "), "{applied}");
    assert!(applied.contains("  • Learn from the people folder (new)\n"), "{applied}");
    assert!(applied.ends_with(&format!("\n{block}")), "{applied}");
    let _ = fs::remove_file(&saved);

    // Init on an existing Workspace refreshes it and prints a readable
    // receipt; machines use `--json` (margins.init.v1).
    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    let init = env.ok(&["--workspace", "practice", "init"]);
    let init_stdout = text(&init.stdout);
    assert!(
        init_stdout.starts_with(&format!("Workspace practice ({})\n", notes.display())),
        "{init_stdout}"
    );
    assert!(!init_stdout.contains("<margins_init"), "{init_stdout}");
    assert!(init_stdout.contains("Margins learns about:\n  Meetings — operational\n"), "{init_stdout}");
    assert!(init_stdout.contains(" notes indexed · "), "{init_stdout}");
    assert!(
        init_stdout.ends_with(
            "See what it learned: `margins --workspace practice status` · Change it: `margins --workspace practice edit`\n"
        ),
        "{init_stdout}"
    );
    let receipt = env.json(&["--workspace", "practice", "init", "--json"]);
    assert_eq!(receipt["schema_version"], "margins.init.v1", "{receipt}");
    assert_eq!(receipt["recall"]["status"], "ok", "{receipt}");

    // `workspace status` keeps its JSON and no longer reads as full recall:
    // the index and catalysts are separate lines.
    let status = text(&env.ok(&["--workspace", "practice", "workspace", "status"]).stdout);
    let status_json = env.json(&["--workspace", "practice", "workspace", "status", "--json"]);
    assert_eq!(status_json["recall"]["mode"], "indexed", "{status_json}");
    assert!(
        status.contains(&format!("Index: indexed · {} documents\n", status_json["recall"]["documents"])),
        "{status}"
    );
    assert!(status.contains("\nCatalysts: "), "{status}");
    assert!(!status.contains("available=true"), "{status}");
    assert!(status.contains(&format!("Program: {}\n", program_path.display())), "{status}");

    // Show keeps the path alone on stdout, with the hint on stderr.
    let show = env.ok(&["--workspace", "practice", "workspace", "show"]);
    assert_eq!(text(&show.stdout), format!("{}\n", program_path.display()));
    assert!(text(&show.stderr).contains("edit --print"));

    // Recall reads as results for people, JSON on request.
    let phrase = "The cobalt orchard review moved the launch to Thursday";
    let readable = text(&env.ok(&["--workspace", "practice", "recall", phrase]).stdout);
    assert!(readable.contains("Meetings/2026-10-01 vendor sync.md"), "{readable}");
    assert!(serde_json::from_str::<serde_json::Value>(&readable).is_err(), "{readable}");
    assert_eq!(recall(&env, "practice", phrase, None)["status"], "ok");

    // Non-interactive edit points at the documented flag form.
    for args in [
        &["--workspace", "practice", "workspace", "edit"][..],
        &["--workspace", "practice", "edit"],
    ] {
        let edit = env.run(args);
        assert!(!edit.status.success());
        assert!(
            text(&edit.stderr).contains("margins --workspace practice workspace plan --desired program.enzyme"),
            "{}",
            text(&edit.stderr)
        );
    }

    // `status` is the refinement view: readings, what the engine picked, and
    // with --explain why, all from the bundled engine on the Margins home.
    let top = env.json(&["--workspace", "practice", "status", "--explain", "--json"]);
    assert_eq!(top["schema_version"], "margins.status.v1", "{top}");
    assert_eq!(top["index"]["state"], "indexed", "{top}");
    assert_eq!(top["index"]["documents"], status_json["recall"]["documents"]);
    assert_eq!(top["catalysts"]["usable"], true, "{top}");
    assert_eq!(top["attention"]["readings"][0]["reading"], "folder:Meetings", "{top}");
    assert!(top["attention"]["from_readings"].as_array().is_some_and(|e| !e.is_empty()), "{top}");
    assert!(top["explain"]["readings"].as_array().is_some_and(|r| !r.is_empty()), "{top}");
    let readable = text(&env.ok(&["--workspace", "practice", "status", "--explain"]).stdout);
    for expected in [
        " notes indexed",
        "\nLearns about:\n  Meetings — operational\n",
        "\nWhy (the next catalyst build: ",
        "  folder \"Meetings\"",
    ] {
        assert!(readable.contains(expected), "missing {expected:?} in:\n{readable}");
    }
    // Inside the notes folder the Workspace is found from the cwd.
    let inside = env
        .command(Path::new(BIN), &["status", "--json"])
        .current_dir(notes.join("Meetings"))
        .output()
        .unwrap();
    assert!(inside.status.success(), "{}", text(&inside.stderr));
    let inside: serde_json::Value = serde_json::from_slice(&inside.stdout).unwrap();
    assert_eq!(inside["workspace"]["selected_by"], "folder");
    // `margins enzyme` is gone; status --explain replaces it.
    let removed = env.run(&["enzyme", "status"]);
    assert_eq!(removed.status.code(), Some(2), "{}", text(&removed.stderr));
    // The machine default is used, and said, when nothing else applies.
    env.ok(&["workspace", "default", "--set", "practice"]);
    let defaulted = text(&env.ok(&["status"]).stdout);
    assert!(
        defaulted.starts_with("Workspace practice (your default)"),
        "{defaulted}"
    );

    // Setup names the program too.
    let setup = env.run(&["--workspace", "practice", "setup", "--only", "skills"]);
    assert!(setup.status.success(), "{}", text(&setup.stderr));
    // `practice` is now the machine default, so its commands need no selector.
    let default_block = block.replace("margins --workspace practice ", "margins ");
    assert!(text(&setup.stderr).contains(&default_block), "{}", text(&setup.stderr));
    assert!(text(&setup.stderr).contains("Setup finished.\n"), "{}", text(&setup.stderr));

    assert_eq!(snapshot(&canary), canary_before, "~/.enzyme must be neither read-modified nor written");
    assert_eq!(snapshot(&env.enzyme_home), env.enzyme_before, "ENZYME_HOME must be untouched");
    assert!(!notes.join(".enzyme").exists());
}

/// A Workspace from before programs, upgraded and synced before any `init`,
/// through the machine default: sync announces the default it used and
/// migrates the program as any writing command does, creating nothing else.
#[test]
fn sync_before_init_migrates_a_legacy_default_workspace_and_says_which_it_used() {
    let env = Hermetic::new();
    let notes = notes_fixture(&env);
    let state = env.margins_home.join("workspaces/legacy");
    fs::create_dir_all(&state).unwrap();
    let legacy = format!(
        "id = \"legacy\"\n\n[policy]\nexcluded_folders = [\"archive\"]\n\n[retention]\n\n\
         [bindings.captures]\nkind = \"captures\"\npath = \"{}\"\n\n\
         [bindings.home]\nkind = \"notes\"\npath = \"{}\"\nrole = \"home\"\n",
        state.join("captures").display(),
        notes.display()
    );
    fs::write(state.join("config.toml"), &legacy).unwrap();
    // The default as the release wrote it (setting it now would migrate).
    fs::write(env.margins_home.join("margins.toml"), "[workspace]\ndefault = \"legacy\"\n").unwrap();
    assert!(!env.margins_home.join("configs/legacy.enzyme").exists());

    let _generator = fixture_generator::FixtureGenerator::start(&env.margins_home);
    // The cwd (the temp HOME) is no Workspace's folder: the default is used.
    let synced = env.ok(&["sync", "--json"]);
    assert!(
        text(&synced.stderr).starts_with("Using Workspace legacy (default)\n"),
        "{}",
        text(&synced.stderr)
    );
    let sync: serde_json::Value = serde_json::from_slice(&synced.stdout).unwrap();
    assert_eq!(sync["workspace"]["id"], "legacy", "{sync}");
    assert!(env.program("legacy").contains("archive"), "the program was migrated");
    assert!(!state.join("config.toml").exists());
    assert!(state.join("enzyme.db").is_file(), "sync indexed the migrated program");
    let workspaces = fs::read_dir(env.margins_home.join("workspaces")).unwrap().count();
    assert_eq!(workspaces, 1, "no Workspace was created");
    env.assert_enzyme_untouched();
}
