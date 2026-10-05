#![cfg(feature = "recall")]
//! End-to-end proof of the Workspace language through the real `margins`
//! binary: program authoring, plan/apply, indexing from the in-memory program,
//! recall provenance, host-source lowering, legacy upgrade, and edit
//! round-trips. Every run is hermetic: temp `HOME`, `MARGINS_HOME`, and
//! `ENZYME_HOME`, a local fixture generator, no network, no real credentials.
//! Each test asserts that neither `ENZYME_HOME` nor `~/.enzyme` is touched.

#[path = "support/fixture_generator.rs"]
mod fixture_generator;

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
        Command::new(bin)
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("MARGINS_HOME", &self.margins_home)
            .env("ENZYME_HOME", &self.enzyme_home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("MARGINS_RECALL_DEBUG", "1")
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
    let mut args = vec!["--workspace", id, "recall"];
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
    let index = rusqlite::Connection::open(state_dir.join("index.db")).unwrap();
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
        stderr.contains("full_reindex=true reason=first_build"),
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
    let index = rusqlite::Connection::open(state.join("index.db")).unwrap();
    let collection: i64 = index
        .query_row(
            "SELECT COUNT(*) FROM entities WHERE name = 'sqlite:mail' AND type = 'collection'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(collection, 1);
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

    // Any normal command migrates the program.
    let status = env.json(&["--workspace", "legacy", "workspace", "status", "--json"]);
    assert_eq!(status["id"], "legacy");
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

    // The first index run under the new identity is a full reindex, exactly once.
    let first = env.ok(&["--workspace", "legacy", "init"]);
    let first_stderr = String::from_utf8_lossy(&first.stderr);
    let expected_reason = if legacy_bin.is_some() {
        "full_reindex=true reason=document_identity"
    } else {
        "full_reindex=true reason=first_build"
    };
    assert!(first_stderr.contains(expected_reason), "{first_stderr}");
    let refs = indexed_refs(&state);
    assert!(refs.contains(&"people/note-0.md".to_string()), "{refs:?}");
    assert!(refs.iter().all(|r| !r.starts_with("markdown_") && !r.contains("gmail_")));
    assert!(!refs.iter().any(|r| r.contains("archive")));
    if legacy_bin.is_some() {
        let index = rusqlite::Connection::open(state.join("index.db")).unwrap();
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
        !second_stderr.contains("full_reindex=true"),
        "the identity reindex happens once: {second_stderr}"
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
