use chrono::{Local, TimeZone};
use clap::Parser;
use margins_cli::args::Args;
use margins_cli::run;
use margins_cli::services::{CliServices, Clock, ProjectService};
use margins_core::{
    AsrBackend, AsrRequest, AsrResult, AudioLane, CaptureCapabilities, CaptureCommand,
    CaptureCommandResult, CaptureCommandStatus, CaptureDevice, CaptureError, CaptureHandle,
    CaptureLaneSnapshot, CaptureLaneState, CaptureObserver, CaptureProvider, CaptureRequest,
    CaptureSnapshot, CaptureState, PermissionState, TranscriptError,
};
use margins_store::legacy;
use margins_workflows::integrations::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx, HealthStatus,
    IntegrationsStore,
};
use margins_workflows::project::{ProjectSource, ResolvedProject};
use margins_workflows::workspace::{self, GmailCollectionSelector, WorkspaceBinding};
use std::io::{Read, Write as IoWrite};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn seed_context_fixture(root: &Path, stale: bool) {
    let margins_dir = root.join("captures/.margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::create_dir_all(root.join("people")).unwrap();
    std::fs::create_dir_all(root.join("meetings")).unwrap();
    std::fs::create_dir_all(root.join("exports")).unwrap();

    let notes = [
        (
            "people/Ada Lovelace.md",
            "---\ntitle: Ada Lovelace\naliases: [Ada]\nemail: ada@example.com\n---\n# Ada Lovelace\n",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        (
            "people/Kevin Stone.md",
            "---\ntitle: Kevin Stone\naliases: [Kevin]\nemail: kevin.stone@example.com\n---\n# Kevin Stone\n",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        (
            "people/Kevin Chen.md",
            "---\ntitle: Kevin Chen\naliases: [Kevin]\nemail: kevin.chen@example.com\n---\n# Kevin Chen\n",
            serde_json::json!({}),
            Vec::<&str>::new(),
        ),
        (
            "meetings/ada-call.md",
            "---\npeople: ['[[Ada Lovelace]]']\n---\n# Customer call\nAgreed on the migration sequence.\n- [ ] Send Ada the migration plan\n",
            serde_json::json!({
                "source": "granola",
                "source_id": "episode-ada-1",
                "kind": "meeting_note",
                "occurred_at": "2026-08-01T10:00:00Z",
                "provenance": root.join("exports/granola-ada.json").to_string_lossy(),
            }),
            vec!["people/ada lovelace"],
        ),
        (
            "meetings/after-cutoff.md",
            "---\npeople: ['[[Ada Lovelace]]']\n---\n# Later call\nThis must not leak backward.\n",
            serde_json::json!({
                "source": "folder",
                "source_id": "episode-ada-later",
                "kind": "meeting_note",
                "occurred_at": "2026-08-12T10:00:00Z",
                "provenance": root.join("meetings/after-cutoff.md").to_string_lossy(),
            }),
            vec!["ada lovelace"],
        ),
    ];
    std::fs::write(root.join("exports/granola-ada.json"), "source receipt").unwrap();

    let recall_path = root.join("index.db");
    let conn = rusqlite::Connection::open(&recall_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE docs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_ref TEXT NOT NULL UNIQUE,
            title TEXT,
            content TEXT,
            content_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            indexed_at INTEGER NOT NULL,
            metadata TEXT DEFAULT '{}'
         );
         CREATE TABLE doc_links (
            doc_id INTEGER NOT NULL,
            link TEXT NOT NULL,
            PRIMARY KEY (doc_id, link)
         );",
    )
    .unwrap();
    let indexed_at = chrono::Utc::now().timestamp_millis();
    for (source_ref, content, metadata, links) in notes {
        let path = root.join(source_ref);
        std::fs::write(&path, content).unwrap();
        let modified_at = if stale {
            0
        } else {
            path.metadata()
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
        };
        conn.execute(
            "INSERT INTO docs (source_ref, title, content, content_hash, created_at, modified_at, indexed_at, metadata)
             VALUES (?1, NULL, ?2, 'fixture-hash', ?3, ?4, ?5, ?6)",
            rusqlite::params![
                source_ref,
                content,
                chrono::DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                    .unwrap()
                    .timestamp_millis(),
                modified_at,
                indexed_at,
                metadata.to_string(),
            ],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        for link in links {
            conn.execute(
                "INSERT INTO doc_links (doc_id, link) VALUES (?1, ?2)",
                rusqlite::params![doc_id, link],
            )
            .unwrap();
        }
    }
    drop(conn);

    let next = chrono::DateTime::parse_from_rfc3339("2026-08-11T09:00:00Z")
        .unwrap()
        .with_timezone(&Local);
    legacy::create_session(
        &margins_dir,
        "upcoming-ada",
        &next,
        ".margins/upcoming-ada.md",
    )
    .unwrap();
    legacy::set_calendar_event(
        &margins_dir,
        "upcoming-ada",
        legacy::CalendarEventMeta {
            title: "Ada planning".into(),
            start: Some("2026-08-11T09:00:00Z".into()),
            end: Some("2026-08-11T09:30:00Z".into()),
            calendar_id: Some("primary".into()),
            event_id: Some("calendar-1".into()),
        },
        vec!["Ada Lovelace".into(), "Kevin".into()],
    )
    .unwrap();
}

fn seed_calendar_meeting(root: &Path, source_id: &str, starts_at: &str) -> PathBuf {
    let store = IntegrationsStore::open(root).unwrap();
    let ctx = ConnectorCtx {
        vault_root: root.to_path_buf(),
        connector_id: "gcal".into(),
        account: "ada@example.com".into(),
        command_path: None,
    };
    let selector = margins_workflows::workspace::CalendarCollectionSelector::default_declaration();
    let scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
        &selector,
        chrono::Utc::now(),
    )
    .unwrap()
    .as_range();
    let occurred_from = chrono::DateTime::parse_from_rfc3339(starts_at)
        .unwrap()
        .with_timezone(&chrono::Utc);
    store
        .apply_calendar_event_delta(
            &ctx,
            CalendarEventDelta {
                events: vec![CalendarEventEvidence {
                    source_id: source_id.into(),
                    calendar_id: "primary".into(),
                    occurred_from,
                    occurred_to: Some(occurred_from + chrono::Duration::minutes(30)),
                    title: "Ada planning from calendar".into(),
                    body_text: "Calendar fixture".into(),
                    href: Some(format!("https://calendar.google.com/event?eid={source_id}")),
                }],
                attendees: vec![
                    CalendarEventAttendee {
                        source_id: source_id.into(),
                        attendee_key: "email:ada@example.com".into(),
                        position: 0,
                        display_name: "Ada Lovelace".into(),
                        email: Some("ada@example.com".into()),
                        response_status: Some("accepted".into()),
                        is_self: false,
                        organizer: false,
                    },
                    CalendarEventAttendee {
                        source_id: source_id.into(),
                        attendee_key: "name:kevin".into(),
                        position: 1,
                        display_name: "Kevin".into(),
                        email: None,
                        response_status: None,
                        is_self: false,
                        organizer: false,
                    },
                ],
                tombstone_source_ids: Vec::new(),
                raw_items: Vec::new(),
                scope,
                complete_snapshot: false,
                materialization_fingerprint: selector.materialization_fingerprint().unwrap(),
                next_cursor: None,
            },
            None,
        )
        .unwrap();
    store.db_path().to_path_buf()
}

fn seed_materialized_email(root: &Path) -> PathBuf {
    let store = IntegrationsStore::open(root).unwrap();
    let ctx = ConnectorCtx {
        vault_root: root.to_path_buf(),
        connector_id: "email".into(),
        account: "owner@example.com".into(),
        command_path: None,
    };
    let occurred = chrono::DateTime::parse_from_rfc3339("2026-08-09T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let selector = margins_workflows::workspace::GmailCollectionSelector::default_declaration();
    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &ctx,
            vec![margins_workflows::integrations::ThreadEvidence {
                thread_id: "thread-materialized".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text:
                    "Alice confirmed the migration follow-up from the materialized thread."
                        .into(),
                href: Some(
                    "https://mail.google.com/mail/?authuser=owner%40example.com#all/thread-materialized"
                        .into(),
                ),
            }],
            vec![margins_workflows::integrations::ParticipantThread {
                participant: "alice@acme.test".into(),
                thread_id: "thread-materialized".into(),
                last_interaction: occurred,
                sampling_score: Some(1),
            }],
            &selector.materialization_fingerprint().unwrap(),
        )
        .unwrap();
    store
        .update_health(&ctx, HealthStatus::Fresh, None)
        .unwrap();
    store.db_path().to_path_buf()
}

fn assert_context_v2_schema(value: &serde_json::Value) {
    assert_eq!(value["schema_version"], "margins.context.v2");
    assert!(value["query"].is_object());
    assert!(value["query"]["kind"].is_string());
    assert!(value["query"]["value"].is_string());
    assert!(value["query"]["cutoff"].is_string());
    assert!(value["resolved_identities"].is_array());
    assert!(value["identity_omissions"].is_array());
    assert!(value["episodes"].is_array());
    assert!(value["open_items"].is_array());
    assert!(value["source_manifest"].is_object());
    assert_eq!(value["source_manifest"]["freshness"]["status"], "fresh");
    assert_eq!(value["source_manifest"]["freshness"]["stale"], false);
    assert!(value["source_manifest"]["sources"].is_array());
    if value["meeting"].is_object() {
        assert!(matches!(
            value["meeting"]["source"].as_str(),
            Some("session" | "calendar_event")
        ));
    }
    for identity in value["resolved_identities"].as_array().unwrap() {
        assert!(identity["id"].is_string());
        assert!(identity["display_name"].is_string());
        assert!(identity["aliases"].is_array());
        assert!(identity["emails"].is_array());
        assert!(identity["total_evidence_count"].is_u64());
        assert!(identity["resolution_evidence"].is_array());
        assert!(identity["resolution_evidence"].as_array().unwrap().len() <= 5);
        for evidence in identity["resolution_evidence"].as_array().unwrap() {
            assert!(evidence["provenance"]["evidence"]["kind"].is_string());
        }
    }
    for episode in value["episodes"].as_array().unwrap() {
        for key in ["id", "kind", "occurred_at", "excerpt"] {
            assert!(episode[key].is_string(), "episode.{key} must be a string");
        }
        assert!(episode["evidence"]["kind"].is_string());
        assert!(episode.get("excerpt_path").is_none());
        assert!(episode["provenance"]["source"].is_string());
        assert!(episode["provenance"]["source_id"].is_string());
        assert!(episode["provenance"]["evidence"]["kind"].is_string());
    }
    for item in value["open_items"].as_array().unwrap() {
        for key in ["id", "text", "status", "episode_id"] {
            assert!(item[key].is_string(), "open_items.{key} must be a string");
        }
        assert!(item["provenance"]["evidence"]["kind"].is_string());
        assert!(item["provenance"]["anchor"].is_string());
    }
}

#[derive(Clone)]
struct FixedProject(PathBuf);

impl FixedProject {
    fn resolved(&self) -> ResolvedProject {
        ResolvedProject {
            project: ProjectSource {
                id: "test".into(),
                name: "Test".into(),
                path: self.0.to_string_lossy().into_owned(),
                inbox_folder: "meetings".into(),
                people_folder: "people".into(),
                readiness: "ready".into(),
            },
            root_dir: self.0.clone(),
            work_dir: self.0.clone(),
        }
    }
}

impl ProjectService for FixedProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        Ok(vec![self.resolved()])
    }
    fn resolve(&self, _selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn set_active(&self, _selector: &str) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn add(
        &self,
        _path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 10, 12, 0, 0).unwrap()
    }
}

fn services(root: &Path) -> CliServices {
    let mut services = CliServices::default();
    services.projects = Arc::new(FixedProject(root.to_path_buf()));
    services.clock = Arc::new(FixedClock);
    services
}

fn invoke(
    services: &CliServices,
    invocation_dir: &Path,
    args: &[&str],
) -> (Result<(), margins_cli::CliError>, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        services,
        invocation_dir,
        args.iter().copied(),
        &mut stdout,
        &mut stderr,
    );
    (
        result,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn project_preprocessing_accepts_both_historical_spellings_anywhere() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    for args in [
        vec!["margins", "--project", "test", "recent"],
        vec!["margins", "recent", "--project=test"],
    ] {
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{stderr}");
        assert_eq!(stdout, "<margins_recent />\n");
    }
}

#[test]
fn clap_help_preserves_the_prior_argument_contract() {
    let error = Args::try_parse_from(["margins", "transcribe", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Audio file to decode in Rust, such as WAV, M4A, MP3, FLAC, or AAC"));
    assert!(help.contains("Session name. Defaults to a slug derived"));
    assert!(help.contains("Speaker count for diarizing the downmixed mono audio"));

    let error = Args::try_parse_from(["margins", "process", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Stable session id from `margins recent`, or `current`/`latest`"));
    assert!(help.contains("Rebuild alignment from the existing transcript without running ASR"));
}

#[test]
fn context_help_documents_the_stable_agent_contract() {
    let error = Args::try_parse_from(["margins", "context", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("margins.context.v2"));
    assert!(help.contains("resolved_identities"));
    assert!(help.contains("identity_omissions"));
    assert!(help.contains("Every claim-bearing item carries a closed evidence handle"));
    assert!(help.contains("--person <PERSON>"));
    assert!(help.contains("--meeting <MEETING>"));
    assert!(help.contains("--json"));
}

#[test]
fn recall_parser_accepts_declared_source_filter() {
    let parsed = Args::try_parse_from([
        "margins",
        "recall",
        "relationship context",
        "--source",
        "mail",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Recall { query, source })
            if query == "relationship context" && source.as_deref() == Some("mail")
    ));
}

#[test]
fn context_person_returns_schema_valid_source_backed_evidence() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    let services = services(temp.path());
    let recall_path = temp.path().join("index.db");
    let sessions_path = temp.path().join("captures/.margins/sessions.sqlite");
    let recall_before = std::fs::read(&recall_path).unwrap();
    let sessions_before = std::fs::read(&sessions_path).unwrap();

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "context",
            "--person",
            "ada@example.com",
            "--cutoff",
            "2026-08-10",
            "--json",
        ],
    );

    assert!(result.is_ok(), "{stderr}");
    assert!(stderr.is_empty());
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_context_v2_schema(&value);
    assert_eq!(value["query"]["kind"], "person");
    assert_eq!(
        value["resolved_identities"][0]["display_name"],
        "Ada Lovelace"
    );
    assert_eq!(
        value["resolved_identities"][0]["emails"][0],
        "ada@example.com"
    );
    assert!(value["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|episode| episode["id"] == "granola:episode-ada-1"));
    assert!(!value["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|episode| episode["id"] == "folder:episode-ada-later"));
    assert_eq!(
        value["open_items"][0]["text"],
        "Send Ada the migration plan"
    );
    assert_eq!(value["open_items"][0]["status"], "open");
    assert_eq!(value["source_manifest"]["freshness"]["status"], "fresh");
    assert_eq!(std::fs::read(recall_path).unwrap(), recall_before);
    assert_eq!(std::fs::read(sessions_path).unwrap(), sessions_before);
}

#[test]
fn context_never_merges_people_sharing_a_first_name() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--person", "Kevin", "--json"],
    );

    let error = result.unwrap_err();
    assert_eq!(error.code(), "context_identity_ambiguous");
    assert!(stdout.is_empty());
    assert!(stderr.contains("Kevin Stone"));
    assert!(stderr.contains("Kevin Chen"));
    assert!(stderr.contains("will not merge"));
}

#[test]
fn granola_import_cli_emits_native_note_without_session_or_source_state() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &vault).unwrap();
    let export = temp.path().join("granola-export.json");
    std::fs::write(
        &export,
        r#"{"documents":[{"id":"cli-granola-1","title":"Migration review","created_at":"2026-08-20T14:00:00Z","notes":"Keep the source notes.","transcript":"Ada: Keep the source transcript.","attendees":[{"name":"Ada Lovelace","email":"ada@example.com"}]}]}"#,
    )
    .unwrap();
    let services = services(&vault);

    let (result, stdout, stderr) = invoke(
        &services,
        &vault,
        &[
            "margins",
            "--workspace",
            "practice",
            "import",
            "granola",
            export.to_string_lossy().as_ref(),
        ],
    );

    assert!(result.is_ok(), "{stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("<margins_import_granola status=\"ok\" imported=\"1\""));
    assert!(!stdout.contains("receipt="));
    assert!(stdout.contains("<note>"));
    assert!(!stdout.contains("<meeting"));
    let note_path = vault.join("Meetings/2026-08-20 Migration review.md");
    let note = std::fs::read_to_string(note_path).unwrap();
    assert!(note.contains("occurred_at: 2026-08-20T14:00:00Z"));
    assert!(note.contains("## Notes\n\nKeep the source notes."));
    assert!(note.contains("## Transcript\n\nAda: Keep the source transcript."));
    for forbidden in [
        "source:",
        "granola_id:",
        "source_account:",
        "source_id:",
        "margins_session:",
        "participants_unresolved:",
        "content_hash:",
        "imported_at:",
    ] {
        assert!(!note.contains(forbidden), "found {forbidden} in {note}");
    }
    assert!(!vault.join("organizations/Example.md").exists());
    assert!(!vault.join(".margins/sessions.sqlite").exists());
    assert!(!vault.join(".margins/integrations").exists());
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn context_meeting_next_omits_ambiguous_attendees_and_uses_start_cutoff() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--meeting", "next", "--json"],
    );

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_context_v2_schema(&value);
    assert_eq!(value["meeting"]["id"], "upcoming-ada");
    assert_eq!(value["meeting"]["source"], "session");
    assert_eq!(value["query"]["cutoff"], "2026-08-11T09:00:00+00:00");
    assert_eq!(value["resolved_identities"].as_array().unwrap().len(), 1);
    assert_eq!(value["identity_omissions"].as_array().unwrap().len(), 1);
    assert_eq!(value["identity_omissions"][0]["query"], "Kevin");
    assert_eq!(
        value["identity_omissions"][0]["reason"],
        "ambiguous_identity"
    );
    assert_eq!(
        value["identity_omissions"][0]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!value["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|episode| episode["id"] == "folder:episode-ada-later"));
}

#[test]
fn context_meeting_next_uses_calendar_episode_in_calendar_only_vault() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    std::fs::remove_file(temp.path().join("captures/.margins/sessions.sqlite")).unwrap();
    seed_calendar_meeting(temp.path(), "calendar-later", "2026-08-11T10:30:00Z");
    let integrations_path =
        seed_calendar_meeting(temp.path(), "calendar-only", "2026-08-11T08:30:00Z");
    let integrations_before = std::fs::read(&integrations_path).unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--meeting", "next", "--json"],
    );

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_context_v2_schema(&value);
    assert_eq!(value["meeting"]["id"], "gcal:ada@example.com:calendar-only");
    assert_eq!(value["meeting"]["source"], "calendar_event");
    assert_eq!(value["query"]["cutoff"], "2026-08-11T08:30:00+00:00");
    assert_eq!(value["resolved_identities"].as_array().unwrap().len(), 1);
    assert!(value["resolved_identities"][0]["resolution_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|evidence| evidence["kind"] == "calendar_attendee_email"));
    assert_eq!(value["identity_omissions"][0]["query"], "Kevin");
    assert_eq!(
        value["identity_omissions"][0]["reason"],
        "ambiguous_identity"
    );
    assert!(value["source_manifest"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["id"] == "margins_integrations"));
    assert_eq!(
        std::fs::read(integrations_path).unwrap(),
        integrations_before
    );

    let store = IntegrationsStore::open(temp.path()).unwrap();
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: "gcal".into(),
        account: "ada@example.com".into(),
        command_path: None,
    };
    store
        .record_failed_reconcile(&ctx, "simulated Calendar refresh failure")
        .unwrap();
    let (stale_result, stale_stdout, stale_stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--meeting", "next", "--json"],
    );
    assert!(stale_result.is_ok(), "{stale_stderr}");
    let stale: serde_json::Value = serde_json::from_str(&stale_stdout).unwrap();
    assert_eq!(stale["meeting"]["id"], "gcal:ada@example.com:calendar-only");
    assert_eq!(stale["source_manifest"]["freshness"]["status"], "error");
    assert_eq!(stale["source_manifest"]["freshness"]["stale"], true);
    assert_eq!(
        stale["source_manifest"]["freshness"]["reason"],
        "refresh_failed"
    );
}

#[test]
fn context_meeting_next_dedupes_overlapping_session_and_calendar_episode() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    let integrations_path =
        seed_calendar_meeting(temp.path(), "overlapping-calendar", "2026-08-11T09:03:00Z");
    seed_calendar_meeting(temp.path(), "distinct-later", "2026-08-11T10:00:00Z");
    let integrations_before = std::fs::read(&integrations_path).unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--meeting", "next", "--json"],
    );

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_context_v2_schema(&value);
    assert_eq!(value["meeting"]["id"], "upcoming-ada");
    assert_eq!(value["meeting"]["source"], "session");
    assert_eq!(value["query"]["cutoff"], "2026-08-11T09:00:00+00:00");
    assert_eq!(value["resolved_identities"].as_array().unwrap().len(), 1);
    assert_eq!(value["identity_omissions"].as_array().unwrap().len(), 1);
    assert!(value["resolved_identities"][0]["resolution_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|evidence| evidence["kind"] == "calendar_attendee_email"));
    assert_eq!(
        std::fs::read(integrations_path).unwrap(),
        integrations_before
    );
}

#[test]
fn context_meeting_caps_identity_evidence_and_reports_total_count() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    for index in 0..8 {
        seed_calendar_meeting(
            temp.path(),
            &format!("calendar-evidence-{index}"),
            &format!("2026-08-{:02}T10:00:00Z", 12 + index),
        );
    }
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "context", "--meeting", "next", "--json"],
    );

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let identity = &value["resolved_identities"][0];
    assert_eq!(identity["resolution_evidence"].as_array().unwrap().len(), 5);
    assert!(identity["total_evidence_count"].as_u64().unwrap() > 5);
    let unique = identity["resolution_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|evidence| {
            (
                evidence["kind"].as_str().unwrap(),
                evidence["value"].as_str().unwrap(),
                evidence["provenance"]["source_id"].as_str().unwrap(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), 5);
    assert!(
        stdout.len() < 40_000,
        "bounded context JSON was {} bytes",
        stdout.len()
    );
}

#[test]
fn context_person_reads_materialized_email_with_gmail_provenance() {
    let temp = tempfile::tempdir().unwrap();
    seed_context_fixture(temp.path(), false);
    let ledger_path = seed_materialized_email(temp.path());
    let ledger_before = std::fs::read(&ledger_path).unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "context",
            "--person",
            "alice@acme.test",
            "--cutoff",
            "2026-08-10",
            "--json",
        ],
    );

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_context_v2_schema(&value);
    assert_eq!(
        value["resolved_identities"][0]["display_name"],
        "alice@acme.test"
    );
    let email = value["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|episode| episode["id"] == "email:thread-materialized")
        .expect("materialized email is returned as context");
    assert_eq!(email["kind"], "email");
    assert!(email["excerpt"]
        .as_str()
        .unwrap()
        .contains("materialized thread"));
    assert_eq!(email["provenance"]["evidence"]["kind"], "external_record");
    assert_eq!(email["provenance"]["evidence"]["connector_id"], "email");
    assert_eq!(
        email["provenance"]["evidence"]["source_id"],
        "thread-materialized"
    );
    assert!(email["provenance"]["evidence"]["href"]
        .as_str()
        .unwrap()
        .starts_with("https://mail.google.com/mail/"));
    assert_eq!(email["provenance"]["anchor"], "thread_evidence");
    assert_eq!(email["evidence"], email["provenance"]["evidence"]);
    assert!(email.get("excerpt_path").is_none());
    assert!(value["resolved_identities"][0]["resolution_evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|evidence| evidence["provenance"]["anchor"] == "participant_threads:0"));
    assert_eq!(std::fs::read(ledger_path).unwrap(), ledger_before);

    let store = IntegrationsStore::open(temp.path()).unwrap();
    let ctx = ConnectorCtx {
        vault_root: temp.path().to_path_buf(),
        connector_id: "email".into(),
        account: "owner@example.com".into(),
        command_path: None,
    };
    store
        .record_failed_reconcile(&ctx, "simulated refresh failure")
        .unwrap();
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "context",
            "--person",
            "alice@acme.test",
            "--cutoff",
            "2026-08-10",
            "--json",
        ],
    );
    assert!(result.is_ok(), "{stderr}");
    let stale: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stale["source_manifest"]["freshness"]["status"], "error");
    assert_eq!(stale["source_manifest"]["freshness"]["stale"], true);
    assert_eq!(
        stale["source_manifest"]["freshness"]["reason"],
        "refresh_failed"
    );
    assert!(stale["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|episode| episode["id"] == "email:thread-materialized"));
}

#[test]
fn context_missing_or_stale_product_store_is_a_clear_error() {
    let missing = tempfile::tempdir().unwrap();
    let missing_services = services(missing.path());
    let (result, stdout, stderr) = invoke(
        &missing_services,
        missing.path(),
        &["margins", "context", "--person", "Ada Lovelace", "--json"],
    );
    assert_eq!(result.unwrap_err().code(), "context_store_missing");
    assert!(stdout.is_empty());
    assert!(stderr.contains("index.db"));
    assert!(stderr.contains("margins init"));

    let stale = tempfile::tempdir().unwrap();
    seed_context_fixture(stale.path(), true);
    let stale_services = services(stale.path());
    let (result, stdout, stderr) = invoke(
        &stale_services,
        stale.path(),
        &["margins", "context", "--person", "Ada Lovelace", "--json"],
    );
    assert_eq!(result.unwrap_err().code(), "context_store_stale");
    assert!(stdout.is_empty());
    assert!(stderr.contains("source files changed after indexing"));
    assert!(stderr.contains("Refresh the product recall index"));
}

#[test]
fn parser_accepts_workspace_setup_guide_command() {
    let parsed = Args::try_parse_from(["margins", "guide", "workspace-setup"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Guide {
            command: margins_cli::args::GuideCommand::WorkspaceSetup
        })
    ));
}

#[test]
fn parser_accepts_source_add_gmail_selector_flags() {
    Args::try_parse_from([
        "margins",
        "source",
        "add",
        "google-mail",
        "--name",
        "mail",
        "--account",
        "owner@example.com",
        "--query",
        "-in:spam -in:trash",
        "--backfill-days",
        "365",
    ])
    .unwrap();
}

#[test]
fn parser_accepts_source_add_calendar_selector_flags() {
    Args::try_parse_from([
        "margins",
        "source",
        "add",
        "google-calendar",
        "--name",
        "calendar",
        "--account",
        "owner@example.com",
        "--lookback-days",
        "365",
        "--lookahead-days",
        "180",
    ])
    .unwrap();
}

#[test]
fn parser_accepts_workspace_plan_apply_and_integrations_reconcile() {
    Args::try_parse_from(["margins", "workspace", "propose", "--json"]).unwrap();
    Args::try_parse_from([
        "margins",
        "workspace",
        "plan",
        "--desired",
        "desired.toml",
        "--json",
    ])
    .unwrap();
    Args::try_parse_from([
        "margins",
        "workspace",
        "apply",
        "--plan",
        "plan.json",
        "--if-revision",
        "abc",
        "--request-id",
        "req-1",
        "--json",
    ])
    .unwrap();
    for args in [
        vec![
            "margins",
            "integrations",
            "reconcile",
            "--if-revision",
            "abc",
            "--request-id",
            "req-1",
            "--json",
        ],
        vec![
            "margins",
            "integrations",
            "reconcile",
            "--if-revision",
            "abc",
            "--request-id",
            "req-2",
            "--connector",
            "email",
            "--account",
            "owner@example.com",
            "--json",
        ],
        vec!["margins", "integrations", "status", "--json"],
    ] {
        Args::try_parse_from(args).unwrap();
    }
}

#[test]
fn parser_accepts_top_level_sync_and_workspace_alias() {
    let parsed = Args::try_parse_from(["margins", "sync"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Sync {
            source: None,
            json: false,
        })
    ));

    let parsed = Args::try_parse_from(["margins", "sync", "--source", "mail", "--json"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Sync {
            source: Some(source),
            json: true,
        }) if source == "mail"
    ));

    let (workspace, stripped) = margins_cli::args::strip_workspace_arg(
        [
            "margins",
            "sync",
            "--workspace",
            "customer-vault",
            "--source",
            "mail",
        ]
        .into_iter()
        .map(std::ffi::OsString::from),
    )
    .unwrap();
    assert_eq!(workspace.as_deref(), Some("customer-vault"));
    assert_eq!(
        stripped,
        ["margins", "sync", "--source", "mail"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
}

#[test]
fn parser_accepts_retention_preview_and_apply_commands() {
    for args in [
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "email",
            "--account",
            "owner@example.com",
            "--scope",
            "raw-cache",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "gcal",
            "--account",
            "owner@example.com",
            "--scope",
            "all",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "google_meet",
            "--account",
            "owner@example.com",
            "--scope",
            "materialization",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "apply",
            "--plan",
            "plan.json",
            "--if-revision",
            "abc",
            "--request-id",
            "req-1",
            "--json",
        ],
    ] {
        Args::try_parse_from(args).unwrap();
    }
}

#[test]
fn parser_rejects_retention_missing_apply_fields_and_unknown_connectors_or_scopes() {
    for args in [
        vec![
            "margins",
            "retention",
            "apply",
            "--if-revision",
            "abc",
            "--request-id",
            "req-1",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "apply",
            "--plan",
            "plan.json",
            "--request-id",
            "req-1",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "apply",
            "--plan",
            "plan.json",
            "--if-revision",
            "abc",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "folder",
            "--account",
            "owner@example.com",
            "--scope",
            "raw-cache",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "email",
            "--account",
            "owner@example.com",
            "--scope",
            "everything",
            "--json",
        ],
        vec![
            "margins",
            "retention",
            "preview",
            "--connector",
            "email",
            "--scope",
            "raw-cache",
            "--json",
        ],
    ] {
        assert!(
            Args::try_parse_from(args.clone()).is_err(),
            "expected parser rejection for {}",
            args.join(" ")
        );
    }
}

#[test]
fn parser_rejects_removed_integrations_survey_approve_and_pull_commands() {
    for args in [
        vec!["margins", "integrations", "survey", "--json"],
        vec!["margins", "integrations", "approve", "--connector", "email"],
        vec!["margins", "integrations", "pull"],
        vec![
            "margins",
            "integrations",
            "approve",
            "--connector",
            "gcal",
            "--from",
            "today",
            "--to",
            "+7d",
        ],
    ] {
        let command = args.join(" ");
        assert!(
            Args::try_parse_from(args).is_err(),
            "expected parser rejection for {command}"
        );
    }
}

#[test]
fn source_list_json_serializes_typed_bindings_without_irrelevant_nulls() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let reference = temp.path().join("reference");
    std::fs::create_dir_all(&reference).unwrap();
    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
            .unwrap();
    use margins_workflows::workspace::{
        CalendarCollectionSelector, GmailCollectionSelector, SourceRole, WorkspaceBinding,
    };
    margins_workflows::workspace::add_source(
        &mut workspace,
        "reference",
        WorkspaceBinding::NativeMarkdown {
            path: reference,
            role: SourceRole::Reference,
        },
    )
    .unwrap();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "mail",
        WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: GmailCollectionSelector::default_declaration(),
        },
    )
    .unwrap();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "calendar",
        WorkspaceBinding::GoogleCalendar {
            account: "owner@example.com".to_string(),
            calendar: CalendarCollectionSelector::default_declaration(),
        },
    )
    .unwrap();

    let services = services(&notes);
    let (result, stdout, stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "source",
            "list",
            "--json",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
    assert!(result.is_ok(), "{stderr}");
    let entries: Vec<serde_json::Value> = serde_json::from_str(stdout.trim()).unwrap();
    let home = entries
        .iter()
        .find(|entry| entry["name"] == "home")
        .expect("home binding");
    assert_eq!(home["kind"], "notes");
    assert!(home["path"].is_string());
    assert_eq!(home["role"], "home");
    assert!(home.get("account").is_none());
    assert!(home.get("gmail").is_none());
    assert_eq!(home["indexed_how"], "native");

    let mail = entries
        .iter()
        .find(|entry| entry["name"] == "mail")
        .expect("mail binding");
    assert_eq!(mail["kind"], "google-mail");
    assert_eq!(mail["account"], "owner@example.com");
    assert!(mail["gmail"]["query"].is_string());
    assert_eq!(mail["gmail"]["backfill_days"], 365);
    assert!(mail.get("path").is_none());
    assert!(mail.get("role").is_none());
    assert_eq!(mail["indexed_how"], "ledger");

    let calendar = entries
        .iter()
        .find(|entry| entry["name"] == "calendar")
        .expect("calendar binding");
    assert_eq!(calendar["kind"], "google-calendar");
    assert_eq!(calendar["account"], "owner@example.com");
    assert_eq!(calendar["calendar"]["lookback_days"], 365);
    assert_eq!(calendar["calendar"]["lookahead_days"], 180);
    assert!(calendar.get("path").is_none());
    assert!(calendar.get("gmail").is_none());
    assert_eq!(calendar["indexed_how"], "ledger");
}

#[test]
fn source_add_rejects_fields_owned_by_other_binding_variants() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
        .unwrap();
    let services = services(&notes);

    let (calendar, _, calendar_stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "source",
            "add",
            "google-calendar",
            "--name",
            "calendar",
            "--account",
            "owner@example.com",
            "--path",
            notes.to_str().unwrap(),
        ],
    );
    assert_eq!(calendar.unwrap_err().code(), "invalid_source_flags");
    assert!(calendar_stderr.contains("--path applies only"));

    let (gmail_calendar_flags, _, gmail_calendar_stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "source",
            "add",
            "google-mail",
            "--name",
            "mail",
            "--account",
            "owner@example.com",
            "--lookback-days",
            "30",
        ],
    );
    assert_eq!(
        gmail_calendar_flags.unwrap_err().code(),
        "invalid_calendar_flags"
    );
    assert!(gmail_calendar_stderr.contains("--lookback-days"));

    let (reference, _, reference_stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "source",
            "add",
            "notes",
            "--name",
            "reference",
            "--path",
            notes.to_str().unwrap(),
            "--role",
            "reference",
            "--account",
            "owner@example.com",
        ],
    );
    assert_eq!(reference.unwrap_err().code(), "invalid_source_flags");
    assert!(reference_stderr.contains("--account applies only"));

    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn parser_accepts_portable_granola_connection_lifecycle() {
    Args::try_parse_from([
        "margins",
        "connect",
        "granola",
        "--account",
        "owner@example.com",
        "--headless",
        "--json",
    ])
    .unwrap();
    Args::try_parse_from([
        "margins",
        "connect",
        "status",
        "--service",
        "granola",
        "--json",
    ])
    .unwrap();
    Args::try_parse_from([
        "margins",
        "disconnect",
        "granola",
        "--account",
        "owner@example.com",
        "--json",
    ])
    .unwrap();
}

#[test]
fn granola_synthetic_token_failure_crosses_real_json_error_boundary() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    let old_fault = std::env::var_os("MARGINS_GRANOLA_NATIVE_E2E_TOKEN_ERROR");
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::set_var("MARGINS_GRANOLA_NATIVE_E2E_TOKEN_ERROR", "transport");

    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "connect", "granola", "--headless", "--json"],
    );

    restore_env("MARGINS_HOME", old_margins_home.as_ref());
    restore_env("MARGINS_GRANOLA_NATIVE_E2E_TOKEN_ERROR", old_fault.as_ref());
    let error = result.unwrap_err();
    assert_eq!(error.code(), "granola_oauth_token_exchange_failed");
    assert!(stdout.is_empty());
    let json = parse_cli_json_error(&stderr);
    assert_eq!(
        json["error"],
        serde_json::json!({
            "code": "granola_oauth_token_exchange_failed",
            "message": "Granola authorization did not complete at browser_token_exchange (transport). Run `margins connect granola` again.",
            "retryable": true,
            "details": {
                "stage": "browser_token_exchange",
                "reason": "transport",
                "next_action": "run_connect_granola_again"
            }
        })
    );
    assert!(!margins_home.join("granola").exists());
}

#[test]
fn parser_accepts_google_connection_lifecycle() {
    for args in [
        vec!["margins", "connect", "google"],
        vec!["margins", "connect", "google", "--json"],
        vec![
            "margins",
            "connect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ],
        vec![
            "margins",
            "connect",
            "google",
            "--headless",
            "--account",
            "owner@example.com",
            "--json",
        ],
        vec!["margins", "connect", "status", "--json"],
        vec![
            "margins",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ],
    ] {
        margins_cli::args::Args::try_parse_from(args).unwrap();
    }
}

#[test]
fn parser_rejects_removed_google_connection_bundle_compatibility_surface() {
    for args in [
        vec!["margins", "connect", "google", "--manual"],
        vec![
            "margins",
            "connect",
            "google",
            "--credential-backend",
            "file",
        ],
        vec![
            "margins",
            "connect",
            "google",
            "--headless",
            "--credential-backend",
            "file",
        ],
        vec![
            "margins",
            "connect",
            "google",
            "export",
            "--account",
            "owner@example.com",
            "--out",
            "connection.margins-google",
        ],
        vec![
            "margins",
            "connect",
            "google",
            "import",
            "connection.margins-google",
        ],
    ] {
        let command = args.join(" ");
        assert!(
            margins_cli::args::Args::try_parse_from(args).is_err(),
            "expected parser rejection for {command}"
        );
    }
}

#[test]
fn parser_rejects_bare_google_disconnect_and_legacy_forget_flags() {
    for args in [
        vec!["margins", "disconnect", "google", "--json"],
        vec![
            "margins",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--forget",
        ],
        vec![
            "margins",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--force",
        ],
    ] {
        let command = args.join(" ");
        assert!(
            margins_cli::args::Args::try_parse_from(args).is_err(),
            "expected parser rejection for {command}"
        );
    }
}

#[test]
fn disconnect_rejects_explicit_workspace_scope_before_forgetting_credentials() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let account_home =
        margins_workflows::workspace::google_account_dir(&margins_home, "owner@example.com")
            .unwrap();
    std::fs::create_dir_all(&account_home).unwrap();
    let sentinel = account_home.join("credential-sentinel");
    std::fs::write(&sentinel, "must survive rejected command").unwrap();

    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "--workspace",
            "alpha",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());

    assert!(result.is_err(), "{stdout}");
    assert!(stdout.is_empty());
    assert!(stderr.contains("machine-scoped"), "{stderr}");
    assert!(
        sentinel.is_file(),
        "rejected command must not mutate credentials"
    );
}

#[test]
fn connect_status_requires_durable_token_not_account_metadata_only() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_backend = std::env::var_os("MARGINS_GOOGLE_CREDENTIAL_BACKEND");
    std::env::set_var("MARGINS_GOOGLE_CREDENTIAL_BACKEND", "file");
    let store = margins_workflows::integrations::GoogleAccountStore::new(
        &margins_home,
        "owner@example.com",
    )
    .unwrap();
    store
        .write_metadata(margins_workflows::integrations::REQUIRED_GOOGLE_SCOPES)
        .unwrap();

    let mut stdout = Vec::new();
    margins_cli::commands::connect::status(&margins_home, true, &mut stdout).unwrap();
    restore_env("MARGINS_GOOGLE_CREDENTIAL_BACKEND", old_backend.as_ref());
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    let connection = &value["connections"][0];
    assert_eq!(connection["connected"], false);
    assert_eq!(connection["status"], "needs_attention");
    assert_eq!(connection["reason"], "google_credentials_unavailable");
    assert_eq!(connection["access"].as_array().unwrap().len(), 0);
}

#[test]
fn connect_status_reports_corrupt_file_token_as_needs_attention() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_backend = std::env::var_os("MARGINS_GOOGLE_CREDENTIAL_BACKEND");
    std::env::set_var("MARGINS_GOOGLE_CREDENTIAL_BACKEND", "file");
    let store = margins_workflows::integrations::GoogleAccountStore::new(
        &margins_home,
        "owner@example.com",
    )
    .unwrap();
    store
        .write_metadata(margins_workflows::integrations::REQUIRED_GOOGLE_SCOPES)
        .unwrap();
    let token_path = margins_home
        .join("google")
        .join("owner@example.com")
        .join("token-cache.json");
    std::fs::write(&token_path, "not-json").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let mut stdout = Vec::new();
    margins_cli::commands::connect::status(&margins_home, true, &mut stdout).unwrap();
    restore_env("MARGINS_GOOGLE_CREDENTIAL_BACKEND", old_backend.as_ref());
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    let connection = &value["connections"][0];
    assert_eq!(connection["connected"], false);
    assert_eq!(connection["status"], "needs_attention");
    assert_eq!(connection["reason"], "google_connection_needs_attention");
    assert_eq!(connection["access"].as_array().unwrap().len(), 0);
    assert!(connection["detail"]
        .as_str()
        .unwrap()
        .contains("failed to parse Google token store"));
}

#[test]
fn connect_status_reports_unrefreshable_file_token_as_needs_attention() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_backend = std::env::var_os("MARGINS_GOOGLE_CREDENTIAL_BACKEND");
    std::env::set_var("MARGINS_GOOGLE_CREDENTIAL_BACKEND", "file");
    let store = margins_workflows::integrations::GoogleAccountStore::new(
        &margins_home,
        "owner@example.com",
    )
    .unwrap();
    store
        .write_metadata(margins_workflows::integrations::REQUIRED_GOOGLE_SCOPES)
        .unwrap();
    let token_path = margins_home
        .join("google")
        .join("owner@example.com")
        .join("token-cache.json");
    std::fs::write(
        &token_path,
        serde_json::json!({
            "schema_version": "margins.google-token-cache.v1",
            "account": "owner@example.com",
            "tokens": [{
                "scopes": margins_workflows::integrations::REQUIRED_GOOGLE_SCOPES,
                "token": {
                    "access_token": "fixture-access",
                    "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let mut stdout = Vec::new();
    margins_cli::commands::connect::status(&margins_home, true, &mut stdout).unwrap();
    restore_env("MARGINS_GOOGLE_CREDENTIAL_BACKEND", old_backend.as_ref());
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    let connection = &value["connections"][0];
    assert_eq!(connection["connected"], false);
    assert_eq!(connection["status"], "needs_attention");
    assert_eq!(connection["reason"], "google_credentials_unavailable");
    assert_eq!(connection["access"].as_array().unwrap().len(), 0);
}

#[test]
fn forget_preserves_workspace_declarations_and_reports_retained_workspaces() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    for id in ["alpha", "beta"] {
        let notes = temp.path().join(format!("notes-{id}"));
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, id, None, &notes)
                .unwrap();
        use margins_workflows::workspace::{
            CalendarCollectionSelector, GmailCollectionSelector, SourceKind, WorkspaceBinding,
        };
        for (name, kind, gmail, calendar) in [
            (
                "work-mail",
                SourceKind::GoogleMail,
                Some(GmailCollectionSelector::default_declaration()),
                None,
            ),
            (
                "work-calendar",
                SourceKind::GoogleCalendar,
                None,
                Some(CalendarCollectionSelector::default_declaration()),
            ),
            ("work-meet", SourceKind::GoogleMeet, None, None),
        ] {
            let binding = match kind {
                SourceKind::GoogleMail => WorkspaceBinding::Gmail {
                    account: "owner@example.com".to_string(),
                    gmail: gmail.unwrap(),
                },
                SourceKind::GoogleCalendar => WorkspaceBinding::GoogleCalendar {
                    account: "owner@example.com".to_string(),
                    calendar: calendar.unwrap(),
                },
                SourceKind::GoogleMeet => WorkspaceBinding::GoogleMeet {
                    account: "owner@example.com".to_string(),
                },
                _ => unreachable!(),
            };
            margins_workflows::workspace::add_source(&mut workspace, name, binding).unwrap();
        }
    }

    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["forgotten"], true);
    assert_eq!(value["account"], "owner@example.com");
    assert_eq!(
        value["retained_workspaces"],
        serde_json::json!(["alpha", "beta"])
    );
    assert!(value.get("detached_workspaces").is_none());
    for id in ["alpha", "beta"] {
        let workspace = margins_workflows::workspace::resolve_at(&margins_home, id).unwrap();
        assert!(workspace.config.bindings.contains_key("work-mail"));
    }
}

#[test]
fn forget_leaves_notes_source_named_google_mail_untouched() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
            .unwrap();
    use margins_workflows::workspace::{SourceKind, SourceRole, WorkspaceBinding};
    margins_workflows::workspace::add_source(
        &mut workspace,
        "google-mail",
        WorkspaceBinding::NativeMarkdown {
            path: temp.path().join("reference"),
            role: SourceRole::Reference,
        },
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("reference")).unwrap();

    let services = services(temp.path());
    let (result, stdout, _stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
    assert!(result.is_ok());
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["forgotten"], true);
    assert_eq!(value["retained_workspaces"], serde_json::json!([]));
    let workspace = margins_workflows::workspace::resolve_at(&margins_home, "practice").unwrap();
    assert_eq!(
        workspace.config.bindings["google-mail"].kind(),
        SourceKind::Notes
    );
}

#[test]
fn source_remove_on_unbound_folder_does_not_create_workspace() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let notes = temp.path().join("Eligible Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("welcome.md"), "# Welcome\nA real note.").unwrap();

    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        &notes,
        &["margins", "source", "remove", "work-mail", "--json"],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
    assert!(result.is_err(), "{stdout}");
    assert!(stdout.is_empty());
    assert!(stderr.contains("workspace_required"));
    assert!(stderr.contains("literal --workspace <id>"));
    assert!(!margins_home.join("workspaces").exists());
}

fn restore_env(key: &str, value: Option<&std::ffi::OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

fn ledger_count(ledger_path: &Path, query: &str) -> i64 {
    let ledger = rusqlite::Connection::open(ledger_path).unwrap();
    ledger.query_row(query, [], |row| row.get(0)).unwrap()
}

fn parse_cli_json_error(stderr: &str) -> serde_json::Value {
    serde_json::from_str(stderr.trim()).expect("expected margins.error.v1 on stderr")
}

fn invoke_reconcile(
    services: &CliServices,
    dir: &Path,
    revision: &str,
    request_id: &str,
    extra: &[&str],
) -> (Result<(), margins_cli::CliError>, String, String) {
    let mut args = vec![
        "margins",
        "--workspace",
        "test-practice",
        "integrations",
        "reconcile",
        "--if-revision",
        revision,
        "--request-id",
        request_id,
        "--json",
    ];
    args.extend_from_slice(extra);
    invoke(services, dir, &args)
}

fn invoke_retention_preview(
    services: &CliServices,
    dir: &Path,
    connector: &str,
    account: &str,
    scope: &str,
) -> (Result<(), margins_cli::CliError>, String, String) {
    invoke(
        services,
        dir,
        &[
            "margins",
            "--workspace",
            "retention-practice",
            "retention",
            "preview",
            "--connector",
            connector,
            "--account",
            account,
            "--scope",
            scope,
            "--json",
        ],
    )
}

fn invoke_retention_apply(
    services: &CliServices,
    dir: &Path,
    plan: &Path,
    revision: &str,
    request_id: &str,
) -> (Result<(), margins_cli::CliError>, String, String) {
    invoke(
        services,
        dir,
        &[
            "margins",
            "--workspace",
            "retention-practice",
            "retention",
            "apply",
            "--plan",
            &plan.to_string_lossy(),
            "--if-revision",
            revision,
            "--request-id",
            request_id,
            "--json",
        ],
    )
}

struct NativeGoogleHttpFixture {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl NativeGoogleHttpFixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let should_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !should_stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let mut request = [0_u8; 8192];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body) = native_google_fixture_response(target);
                let reason = if status == 200 { "OK" } else { "Error" };
                let headers = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            base_url,
            stop,
            handle: Some(handle),
        }
    }

    fn client(&self) -> margins_workflows::integrations::NativeGoogleClient {
        margins_workflows::integrations::NativeGoogleClient::with_bases(
            None,
            Some("fixture-access-token".to_string()),
            &self.base_url,
            &self.base_url,
            &self.base_url,
            &self.base_url,
            &self.base_url,
        )
    }
}

impl Drop for NativeGoogleHttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn native_google_fixture_response(target: &str) -> (u16, String) {
    let path = target.split('?').next().unwrap_or(target);
    let body = if path.ends_with("/gmail/v1/users/me/profile") {
        serde_json::json!({"emailAddress": "owner@example.com"})
    } else if path.ends_with("/threads") && path.contains("/gmail/v1/users/") {
        serde_json::json!({"threads": [{"id": "thread-e2e-1"}]})
    } else if path.ends_with("/threads/thread-e2e-1") {
        serde_json::json!({
            "id": "thread-e2e-1",
            "historyId": "9001",
            "messages": [{
                "id": "msg-e2e-1",
                "threadId": "thread-e2e-1",
                "internalDate": "1787245200000",
                "payload": {
                    "mimeType": "text/plain",
                    "headers": [
                        {"name": "From", "value": "Alice Client <alice@acme.test>"},
                        {"name": "To", "value": "Owner <owner@example.com>"},
                        {"name": "Subject", "value": "Fixture kickoff checkpoint"},
                        {"name": "Date", "value": "Thu, 20 Aug 2026 17:00:00 +0000"}
                    ],
                    "body": {"data": "QWxpY2UgQ2xpZW50IGFza2VkIGZvciB0aGUgVHVlc2RheSBwaWxvdCBraWNrb2ZmIGNoZWNrcG9pbnQgYW5kIHJldmlzZWQgcGlsb3QgcGxhbi4"}
                }
            }]
        })
    } else if path.ends_with("/v3/users/me/calendarList") {
        serde_json::json!({"items": [{"id": "primary", "summary": "Primary"}]})
    } else if path.contains("/v3/calendars/primary/events") {
        serde_json::json!({
            "items": [{
                "id": "evt-client-kickoff",
                "calendarId": "primary",
                "status": "confirmed",
                "summary": "Tuesday pilot kickoff checkpoint",
                "description": "Review the revised pilot plan and follow-up owner.",
                "htmlLink": "https://calendar.google.com/event?eid=fixture",
                "start": {"dateTime": "2026-08-20T17:00:00Z"},
                "end": {"dateTime": "2026-08-20T17:30:00Z"},
                "attendees": [
                    {"displayName": "Alice Client", "email": "alice@acme.test", "responseStatus": "accepted"},
                    {"displayName": "Owner", "email": "owner@example.com", "self": true, "responseStatus": "accepted"}
                ],
                "organizer": {"displayName": "Owner", "email": "owner@example.com"}
            }],
            "nextSyncToken": "sync-token-1"
        })
    } else if path.ends_with("/v3/files") && target.contains("Meet+Recordings") {
        serde_json::json!({"files": [{"id": "folder-meet-recordings", "name": "Meet Recordings"}]})
    } else if path.ends_with("/v3/files") {
        serde_json::json!({"files": [{
            "id": "doc-meet-e2e-1",
            "name": "Tuesday pilot kickoff transcript",
            "createdTime": "2026-08-20T17:31:00Z",
            "modifiedTime": "2026-08-20T17:45:00Z",
            "webViewLink": "https://docs.google.com/document/d/fixture/edit"
        }]})
    } else if path.ends_with("/v1/documents/doc-meet-e2e-1") {
        serde_json::json!({"body": {"content": [
            {"paragraph": {"elements": [{"textRun": {"content": "Alice Client asked for the revised pilot plan.\n"}}]}},
            {"paragraph": {"elements": [{"textRun": {"content": "Owner confirmed the Friday follow-up.\n"}}]}}
        ]}})
    } else if path.ends_with("/v2/conferenceRecords") {
        serde_json::json!({"conferenceRecords": [{
            "name": "conferenceRecords/conf-e2e",
            "startTime": "2026-08-20T17:00:00Z",
            "endTime": "2026-08-20T17:30:00Z"
        }]})
    } else if path.ends_with("/v2/conferenceRecords/conf-e2e/transcripts") {
        serde_json::json!({"transcripts": [{
            "name": "conferenceRecords/conf-e2e/transcripts/transcript-e2e",
            "docsDestination": {"document": "documents/doc-meet-e2e-1"},
            "startTime": "2026-08-20T17:00:00Z",
            "endTime": "2026-08-20T17:30:00Z"
        }]})
    } else if path.ends_with("/v2/conferenceRecords/conf-e2e/participants") {
        serde_json::json!({"participants": [{
            "name": "conferenceRecords/conf-e2e/participants/participant-e2e",
            "signedinUser": {"displayName": "Alice Client", "user": "users/alice"}
        }]})
    } else {
        return (
            404,
            serde_json::json!({"error": format!("unexpected fixture request: {target}")})
                .to_string(),
        );
    };
    (200, body.to_string())
}

#[test]
fn sync_declared_sources_default_and_narrow_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("note.md"), "# Local").unwrap();
    let mut workspace =
        workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    workspace::add_source(
        &mut workspace,
        "mail",
        WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: GmailCollectionSelector::default_declaration(),
        },
    )
    .unwrap();
    let revision = workspace::workspace_revision(&workspace.config).unwrap();

    let all = margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        None,
        &revision,
        "sync-contract-1",
        None,
    )
    .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].binding, "mail");
    assert_eq!(
        all[0].status,
        margins_cli::commands::integrations::SyncSourceStatus::Error
    );

    let narrowed = margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        Some("mail"),
        &revision,
        "sync-contract-2",
        None,
    )
    .unwrap();
    assert_eq!(narrowed.len(), 1);
    assert_eq!(narrowed[0].binding, "mail");
    assert_eq!(
        narrowed[0].status,
        margins_cli::commands::integrations::SyncSourceStatus::Error
    );
    let stale_revision = margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        Some("mail"),
        &"0".repeat(64),
        "sync-contract-3",
        None,
    )
    .unwrap_err();
    assert_eq!(stale_revision.code(), "workspace_revision_conflict");
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn integrations_cli_reconciles_native_google_bindings_and_replays_idempotently() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path();
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    std::fs::create_dir_all(vault.join("transcripts")).unwrap();
    std::fs::write(
        vault.join("transcripts/2026-08-20-client-call.md"),
        "---\ntitle: Client call\ndate: 2026-08-20\npeople:\n  - name: Alice Client\n    email: alice@acme.test\n---\n# Client call\n- [ ] Send Alice the pilot update\n",
    )
    .unwrap();

    let old_margins_home = std::env::var_os("MARGINS_HOME");
    let margins_home = temp.path().join("margins-home");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "test-practice", None, vault)
            .unwrap();
    use margins_workflows::workspace::{
        CalendarCollectionSelector, GmailCollectionSelector, WorkspaceBinding,
    };
    for (name, binding) in [
        (
            "mail",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        ),
        (
            "calendar",
            WorkspaceBinding::GoogleCalendar {
                account: "owner@example.com".to_string(),
                calendar: CalendarCollectionSelector::default_declaration(),
            },
        ),
        (
            "meet",
            WorkspaceBinding::GoogleMeet {
                account: "owner@example.com".to_string(),
            },
        ),
    ] {
        margins_workflows::workspace::add_source(&mut workspace, name, binding).unwrap();
    }

    let cli_services = services(vault);
    let integrations_db = margins_home.join("workspaces/test-practice/ledger.db");

    let (pre_status_result, pre_status_stdout, pre_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "integrations",
            "status",
            "--json",
        ],
    );
    assert!(pre_status_result.is_ok(), "{pre_status_stderr}");
    assert!(
        !integrations_db.exists(),
        "read-only status must not create the integrations ledger"
    );
    let pre_status: serde_json::Value = serde_json::from_str(&pre_status_stdout).unwrap();
    assert!(pre_status["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["status"] == "stale" && row["reason"] == "never_refreshed"));

    let (workspace_status_result, workspace_status_stdout, workspace_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "workspace",
            "status",
            "--json",
        ],
    );
    assert!(workspace_status_result.is_ok(), "{workspace_status_stderr}");
    let workspace_status: serde_json::Value =
        serde_json::from_str(&workspace_status_stdout).unwrap();
    let revision = workspace_status["revision"].as_str().unwrap().to_string();

    let (stale_result, stale_stdout, stale_stderr) = invoke_reconcile(
        &cli_services,
        vault,
        "0000000000000000000000000000000000000000",
        "stale-revision",
        &[],
    );
    assert!(stale_result.is_err());
    assert!(stale_stdout.is_empty());
    let stale_error = parse_cli_json_error(&stale_stderr);
    assert_eq!(stale_error["schema_version"], "margins.error.v1");
    assert_eq!(stale_error["error"]["code"], "workspace_revision_conflict");

    let fixture = NativeGoogleHttpFixture::start();
    let native = fixture.client();
    let mut reconcile_stdout = Vec::new();
    margins_cli::commands::integrations::reconcile_with_native_google_client(
        &workspace.state_dir,
        None,
        None,
        &revision,
        "reconcile-1",
        &native,
        &mut reconcile_stdout,
    )
    .unwrap();
    let reconcile: serde_json::Value = serde_json::from_slice(&reconcile_stdout).unwrap();
    assert_eq!(
        reconcile["schema_version"],
        "margins.integrations.reconcile.v1"
    );
    assert_eq!(reconcile["workspace_id"], "test-practice");
    assert_eq!(reconcile["revision"], revision);
    assert_eq!(reconcile["request_id"], "reconcile-1");
    assert_eq!(reconcile["replayed"], false);
    assert_eq!(reconcile["ok"], true);
    for connector in ["email", "gcal", "google_meet"] {
        let row = reconcile["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["connector_id"] == connector)
            .unwrap_or_else(|| panic!("reconcile omitted {connector}"));
        assert_eq!(row["status"], "applied");
        assert!(row["error"].is_null());
    }
    assert!(ledger_count(&integrations_db, "SELECT COUNT(*) FROM raw_items") > 0);
    assert!(ledger_count(&integrations_db, "SELECT COUNT(*) FROM thread_evidence") > 0);
    assert!(
        ledger_count(
            &integrations_db,
            "SELECT COUNT(*) FROM calendar_event_evidence"
        ) > 0
    );
    assert!(
        ledger_count(
            &integrations_db,
            "SELECT COUNT(*) FROM external_document_evidence WHERE connector_id = 'google_meet'"
        ) > 0
    );
    assert_eq!(
        ledger_count(
            &integrations_db,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('episodes', 'recall_items')"
        ),
        0
    );

    let runs_before = ledger_count(&integrations_db, "SELECT COUNT(*) FROM runs");
    let receipts_before = ledger_count(&integrations_db, "SELECT COUNT(*) FROM reconcile_receipts");
    let threads_before = ledger_count(&integrations_db, "SELECT COUNT(*) FROM thread_evidence");

    let mut replay_stdout = Vec::new();
    margins_cli::commands::integrations::reconcile_with_native_google_client(
        &workspace.state_dir,
        None,
        None,
        &revision,
        "reconcile-1",
        &native,
        &mut replay_stdout,
    )
    .unwrap();
    let replay: serde_json::Value = serde_json::from_slice(&replay_stdout).unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        ledger_count(&integrations_db, "SELECT COUNT(*) FROM runs"),
        runs_before
    );
    assert_eq!(
        ledger_count(&integrations_db, "SELECT COUNT(*) FROM reconcile_receipts"),
        receipts_before
    );
    assert_eq!(
        ledger_count(
            &integrations_db,
            "SELECT COUNT(*) FROM thread_evidence WHERE tombstoned_at IS NULL"
        ),
        threads_before
    );

    let mut conflict_stdout = Vec::new();
    let conflict = margins_cli::commands::integrations::reconcile_with_native_google_client(
        &workspace.state_dir,
        Some("email"),
        None,
        &revision,
        "reconcile-1",
        &native,
        &mut conflict_stdout,
    )
    .unwrap_err();
    assert_eq!(conflict.code(), "idempotency_conflict");
    assert!(conflict_stdout.is_empty());

    let (integration_status_result, integration_status_stdout, integration_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "integrations",
            "status",
            "--json",
        ],
    );
    assert!(
        integration_status_result.is_ok(),
        "{integration_status_stderr}"
    );
    let integration_status: serde_json::Value =
        serde_json::from_str(&integration_status_stdout).unwrap();
    assert_eq!(
        integration_status["schema_version"],
        "margins.integrations.status.v1"
    );
    for connector in ["email", "gcal", "google_meet"] {
        let row = integration_status["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["connector_id"] == connector)
            .unwrap_or_else(|| panic!("status omitted {connector}"));
        assert_eq!(row["status"], "fresh");
    }

    let current_mail = margins_workflows::workspace::remove_source(&mut workspace, "mail").unwrap();
    let retained_threads_before =
        ledger_count(&integrations_db, "SELECT COUNT(*) FROM thread_evidence");
    let (removed_status_result, removed_status_stdout, removed_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "integrations",
            "status",
            "--json",
        ],
    );
    assert!(removed_status_result.is_ok(), "{removed_status_stderr}");
    let removed_status: serde_json::Value = serde_json::from_str(&removed_status_stdout).unwrap();
    assert!(removed_status["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["connector_id"] != "email"));
    assert_eq!(
        ledger_count(&integrations_db, "SELECT COUNT(*) FROM thread_evidence"),
        retained_threads_before
    );

    let current_mail = match current_mail {
        WorkspaceBinding::Gmail { account, .. } => WorkspaceBinding::Gmail {
            account,
            gmail: GmailCollectionSelector {
                query: "label:new-noisy".to_string(),
                backfill_days: 7,
            },
        },
        other => other,
    };
    margins_workflows::workspace::add_source(&mut workspace, "mail", current_mail).unwrap();
    let (selector_status_result, selector_status_stdout, selector_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "integrations",
            "status",
            "--json",
        ],
    );
    assert!(selector_status_result.is_ok(), "{selector_status_stderr}");
    let selector_status: serde_json::Value = serde_json::from_str(&selector_status_stdout).unwrap();
    let email_status = selector_status["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["connector_id"] == "email")
        .expect("rebound Gmail binding remains visible in status");
    assert_eq!(email_status["status"], "stale");
    assert_eq!(email_status["reason"], "refresh_required");

    let (refreshed_revision_result, refreshed_revision_stdout, refreshed_revision_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "workspace",
            "status",
            "--json",
        ],
    );
    assert!(
        refreshed_revision_result.is_ok(),
        "{refreshed_revision_stderr}"
    );
    let refreshed_revision: serde_json::Value =
        serde_json::from_str(&refreshed_revision_stdout).unwrap();
    let refreshed_revision = refreshed_revision["revision"].as_str().unwrap().to_string();

    let mut selector_reconcile_stdout = Vec::new();
    margins_cli::commands::integrations::reconcile_with_native_google_client(
        &workspace.state_dir,
        None,
        None,
        &refreshed_revision,
        "reconcile-selector-refresh",
        &native,
        &mut selector_reconcile_stdout,
    )
    .unwrap();
    assert_eq!(
        ledger_count(
            &integrations_db,
            "SELECT COUNT(*) FROM thread_evidence WHERE tombstoned_at IS NULL"
        ),
        1
    );
    let (fresh_status_result, fresh_status_stdout, fresh_status_stderr) = invoke(
        &cli_services,
        vault,
        &[
            "margins",
            "--workspace",
            "test-practice",
            "integrations",
            "status",
            "--json",
        ],
    );
    assert!(fresh_status_result.is_ok(), "{fresh_status_stderr}");
    let fresh_status: serde_json::Value = serde_json::from_str(&fresh_status_stdout).unwrap();
    let email_status = fresh_status["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["connector_id"] == "email")
        .unwrap();
    assert_eq!(email_status["status"], "fresh");

    let absent = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(absent.path().join(".obsidian")).unwrap();
    let absent_services = services(absent.path());
    let (absent_result, absent_stdout, absent_stderr) = invoke(
        &absent_services,
        absent.path(),
        &[
            "margins",
            "integrations",
            "reconcile",
            "--if-revision",
            "0000000000000000000000000000000000000000",
            "--request-id",
            "absent-workspace",
            "--json",
        ],
    );
    assert!(absent_result.is_err(), "{absent_stdout}");
    let absent_error = parse_cli_json_error(&absent_stderr);
    assert_eq!(absent_error["schema_version"], "margins.error.v1");
    assert_eq!(absent_error["error"]["code"], "usage");
    assert!(absent_error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("integrations reconcile requires literal --workspace"));

    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn parser_accepts_read_only_and_write_config_scan_commands() {
    let parsed = Args::try_parse_from(["margins", "scan"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan {
            write_config: false,
            update: false,
        })
    ));

    let parsed = Args::try_parse_from(["margins", "scan", "--write-config"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan {
            write_config: true,
            update: false,
        })
    ));

    let parsed = Args::try_parse_from(["margins", "scan", "--write-config", "--update"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan {
            write_config: true,
            update: true,
        })
    ));
}

#[test]
fn setup_handoff_never_uses_invocation_dir_without_a_selected_workspace() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("note.md"), "real note").unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "setup"]);

    assert!(result.is_ok(), "{stderr}");
    assert_eq!(
        stdout,
        "Paste into your agent:\nGo to my notes folder, then help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
    );
    assert!(stderr.is_empty());
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn public_setup_rejects_official_machine_setup_flags() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "setup", "--only", "skills"],
    );

    assert_eq!(result.unwrap_err().code(), "setup_option_unavailable");
    assert!(stdout.is_empty());
    assert!(stderr.contains("public setup uses the single workspace guide"));
}

#[test]
fn parser_accepts_repeatable_setup_only_and_speech_skip() {
    let parsed = Args::try_parse_from([
        "margins", "setup", "--only", "catalyst", "--only", "skills", "--skip", "speech",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Setup {
            only,
            skip,
            local_model: margins_cli::args::SetupLocalModelPolicyArg::Fallback,
        })
            if only == vec![
                margins_cli::args::SetupStepArg::Catalyst,
                margins_cli::args::SetupStepArg::Skills,
            ] && skip == Some(margins_cli::args::SetupSkipArg::Speech)
    ));

    let parsed = Args::try_parse_from([
        "margins",
        "setup",
        "--only",
        "catalyst",
        "--local-model",
        "always",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Setup {
            local_model: margins_cli::args::SetupLocalModelPolicyArg::Always,
            ..
        })
    ));
}

#[test]
fn workspace_setup_guide_is_embedded_margins_native_and_read_only() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("note.md"), "real note").unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "guide", "workspace-setup"],
    );

    assert!(result.is_ok(), "{stderr}");
    assert_eq!(
        stdout,
        margins_workflows::resources::MARGINS_WORKSPACE_SETUP_GUIDE
    );
    assert!(stdout.contains("margins workspace new practice"));
    assert!(stdout.contains("margins init"));
    assert!(stdout.contains("margins sync --json"));
    assert!(stdout.contains("margins recall \"an exact phrase from these notes\""));
    assert!(stdout.contains("source add notes"));
    assert!(stdout.contains("Run these commands from the notes folder"));
    assert!(stdout.contains("cd \"/absolute/path/to/notes\""));
    assert!(stdout.contains("`workspace.propose: true`"));
    assert!(stdout.contains("writes its explanation to stderr"));
    assert!(stdout.contains("Present the understanding first"));
    assert!(stdout.contains("If `actions` is empty"));
    assert!(stdout.contains("consent and `workspace apply`"));
    assert!(stdout.contains("exact plan actions and ask for explicit consent"));
    assert!(stdout.contains("apply the saved plan unchanged"));
    assert!(stdout.contains("what is wrong or missing"));
    assert!(stdout.contains("workspace plan"));
    assert!(stdout.contains("Do not hand-edit the plan JSON"));
    assert!(stdout.contains("Use only an item the review surfaced"));
    assert!(stdout.contains("folder:<displayed path>"));
    assert!(stdout.contains("Do not begin connected-note distillation as part of setup"));
    let declaration = stdout.find("margins workspace new practice").unwrap();
    let proposal = stdout
        .find("margins --workspace practice workspace propose")
        .unwrap();
    let apply = stdout
        .find("margins --workspace practice workspace apply")
        .unwrap();
    let initialize = stdout.find("margins --workspace practice init").unwrap();
    assert!(declaration < proposal && proposal < apply && apply < initialize);
    assert!(!stdout.contains("transcribe"));
    assert!(!stdout.contains("audio"));
    assert!(!stdout.contains("scan --write-config"));
    assert!(!stdout.contains("fallback"));
    assert!(!stdout.contains("official composition"));
    assert!(!stdout.contains("private reveal"));
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn public_capabilities_report_only_supported_workflows() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "capabilities"]);

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["product"], "margins");
    assert_eq!(value["composition"], "public");
    assert_eq!(
        value["build"]["commit"],
        margins_cli::build_info::get().commit
    );
    let mut top_level = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    top_level.sort();
    assert_eq!(
        top_level,
        [
            "build",
            "composition",
            "distillation",
            "product",
            "recall",
            "schema",
            "workspace",
        ]
    );
    assert_eq!(value["workspace"]["setup"], true);
    assert_eq!(
        value["workspace"]["declarations"],
        serde_json::json!(["workspace", "source"])
    );
    assert_eq!(
        value["workspace"]["lifecycle"],
        serde_json::json!(["init", "sync"])
    );
    assert_eq!(
        value["workspace"]["automation"],
        serde_json::json!(["plan", "apply"])
    );
    assert!(value["workspace"].get("propose").is_none());
    assert_eq!(value["recall"]["lookup"], true);
    assert_eq!(value["recall"]["mode"], "live_local_markdown");
    assert_eq!(value["distillation"]["available"], true);
    assert_eq!(value["distillation"]["workflow"], "connected_note");
    let inputs = value["distillation"]["inputs"].as_array().unwrap();
    assert!(inputs.contains(&serde_json::json!("transcript")));
    assert!(inputs.contains(&serde_json::json!("memo")));
    assert_eq!(
        inputs.contains(&serde_json::json!("audio")),
        cfg!(any(feature = "coreml-asr", feature = "parakeet-onnx"))
    );
    for omitted in [
        "official",
        "autonomous",
        "audio_import",
        "note_skill",
        "capture",
        "tui",
    ] {
        assert!(value.get(omitted).is_none(), "unexpected field {omitted}");
    }
    for omitted in ["scan", "indexing", "local_model"] {
        assert!(
            value["recall"].get(omitted).is_none(),
            "unexpected recall field {omitted}"
        );
    }
}

#[test]
fn guided_onboarding_ends_setup_before_distillation() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) =
        invoke(&services, temp.path(), &["margins", "guide", "onboarding"]);

    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("Setup ends when"));
    assert!(stdout.contains("Connected-note distillation is a separate workflow"));
    assert!(stdout.contains("from that folder"));
    assert!(stdout.contains("an explanation to stderr"));
    assert!(stdout.contains("When the plan has no actions, skip consent"));
    assert!(stdout.contains("actions, obtain explicit consent"));
    assert!(stdout.contains("apply the emitted plan"));
    assert!(stdout.contains("reconstruct how"));
    assert!(stdout.contains("preserve the surfaced item's displayed spelling"));
    let declaration = stdout.find("3. Otherwise create").unwrap();
    let proposal = stdout.find("4. When `workspace.propose: true`").unwrap();
    let initialize = stdout.find("5. Run `init`").unwrap();
    assert!(declaration < proposal && proposal < initialize);
    assert!(!stdout.contains("transcribe"));
    assert!(!stdout.contains("audio"));
    assert!(!stdout.contains("fallback"));
    assert!(!stdout.contains("official composition"));
    assert!(!stdout.contains("private reveal"));
}

#[test]
fn public_scan_is_not_the_product_workspace_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "scan"]);

    assert!(result.is_err());
    assert!(stdout.is_empty());
    assert!(stderr.contains("composition_unavailable"));
    assert!(stderr.contains("official Margins CLI"));
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn public_init_recall_and_sync_form_an_autonomous_local_loop() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("decision.md"),
        "# Decision\nThe phosphorescent handoff preserves the evidence boundary.",
    )
    .unwrap();
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    let services = services(&notes);

    let (init, init_stdout, init_stderr) = invoke(
        &services,
        &notes,
        &["margins", "--workspace", "practice", "init"],
    );
    assert!(init.is_ok(), "{init_stderr}");
    assert!(init_stdout.contains("mode=\"live_lexical\""));

    let (recall, recall_stdout, recall_stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "recall",
            "phosphorescent handoff",
        ],
    );
    assert!(recall.is_ok(), "{recall_stderr}");
    let recall: serde_json::Value = serde_json::from_str(&recall_stdout).unwrap();
    assert_eq!(recall["search_strategy"], "live_local_markdown");
    assert_eq!(recall["total_results"], 1);

    let (sync, sync_stdout, sync_stderr) = invoke(
        &services,
        &notes,
        &["margins", "--workspace", "practice", "sync", "--json"],
    );
    assert!(sync.is_ok(), "{sync_stderr}");
    let sync: serde_json::Value = serde_json::from_str(&sync_stdout).unwrap();
    assert_eq!(sync["schema_version"], "margins.sync.v1");
    assert_eq!(sync["ok"], true);
    assert_eq!(sync["recall"]["mode"], "live_lexical");
    assert!(sync["sources"]
        .as_array()
        .unwrap()
        .iter()
        .all(|source| source["status"] == "ready"));

    let (status, status_stdout, status_stderr) = invoke(
        &services,
        &notes,
        &[
            "margins",
            "--workspace",
            "practice",
            "workspace",
            "status",
            "--json",
        ],
    );
    assert!(status.is_ok(), "{status_stderr}");
    let status: serde_json::Value = serde_json::from_str(&status_stdout).unwrap();
    assert!(status.get("index").is_none());
    assert!(status.get("ledger").is_none());
    assert!(status.get("catalyst").is_none());
    assert!(status.get("source_refresh_staleness").is_none());
    assert!(!notes.join(".margins").exists());
    assert!(!margins_home.join("workspaces/practice/index.db").exists());

    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn public_sync_returns_a_typed_failure_for_unavailable_sources() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let mut workspace =
        workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    workspace::add_source(
        &mut workspace,
        "mail",
        WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: GmailCollectionSelector::default_declaration(),
        },
    )
    .unwrap();
    let services = services(&notes);

    let (result, stdout, stderr) = invoke(
        &services,
        &notes,
        &["margins", "--workspace", "practice", "sync", "--json"],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());

    assert_eq!(result.unwrap_err().code(), "sync_incomplete");
    let sync: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sync["ok"], false);
    assert!(sync["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["name"] == "mail" && source["status"] == "requires_provider"));
    assert!(stderr.contains("sync_incomplete"));
}

#[test]
fn init_xml_can_report_effective_config_path() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("home/.margins/config.toml");
    let mut stdout = Vec::new();

    let catalyst = margins_workflows::catalyst::CatalystStatus {
        mode: margins_workflows::catalyst::CatalystMode::None,
        reason: "setup_required",
    };
    margins_cli::commands::projects::write_init(
        &mut stdout,
        temp.path(),
        "ok",
        Some(&config),
        Some(&catalyst),
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(stdout).unwrap(),
        format!(
            "<margins_init path=\"{}\" status=\"ok\" build_commit=\"{}\" config_path=\"{}\" catalyst=\"none\" reason=\"setup_required\" />\n",
            temp.path().display(),
            margins_cli::build_info::get().commit,
            config.display()
        )
    );
}

struct RecordingProject {
    root: PathBuf,
    added_paths: Mutex<Vec<String>>,
    resolved_selectors: Mutex<Vec<Option<String>>>,
}

struct MultiProject {
    active_id: String,
    vaults: Vec<ResolvedProject>,
    list_calls: Mutex<usize>,
}

impl ProjectService for MultiProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        *self.list_calls.lock().unwrap() += 1;
        Ok(self.vaults.clone())
    }

    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        let id = selector.unwrap_or(&self.active_id);
        self.vaults
            .iter()
            .find(|vault| vault.project.id == id || vault.project.path == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown test vault {id}"))
    }

    fn set_active(&self, selector: &str) -> anyhow::Result<ResolvedProject> {
        self.resolve(Some(selector))
    }

    fn add(
        &self,
        path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        self.resolve(Some(path))
    }
}

fn resolved_vault(id: &str, root: &Path) -> ResolvedProject {
    ResolvedProject {
        project: ProjectSource {
            id: id.into(),
            name: id.into(),
            path: root.to_string_lossy().into_owned(),
            inbox_folder: "meetings".into(),
            people_folder: "people".into(),
            readiness: "ready".into(),
        },
        root_dir: root.to_path_buf(),
        work_dir: root.to_path_buf(),
    }
}

fn seed_inspectable_session(vault: &Path, meeting_id: &str, marker: &str) {
    let margins_dir = vault.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::write(margins_dir.join(format!("{meeting_id}.md")), marker).unwrap();
    std::fs::write(
        margins_dir.join(format!("{meeting_id}_aligned.md")),
        format!("# {marker}"),
    )
    .unwrap();
    legacy::create_session(
        &margins_dir,
        meeting_id,
        &Local::now(),
        &format!(".margins/{meeting_id}.md"),
    )
    .unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        meeting_id,
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &format!(".margins/{meeting_id}_aligned.md"),
        "durable",
        None,
    )
    .unwrap();
}

fn seed_checkpoint_session(vault: &Path, meeting_id: &str, terminal: bool, decoded: u64) {
    let margins_dir = vault.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::write(margins_dir.join(format!("{meeting_id}.md")), "[00:01] memo").unwrap();
    legacy::create_session(
        &margins_dir,
        meeting_id,
        &Local::now(),
        &format!(".margins/{meeting_id}.md"),
    )
    .unwrap();
    legacy::add_segment(
        &margins_dir,
        meeting_id,
        0,
        &format!(".margins/{meeting_id}_seg0.wav"),
        0,
        None,
    )
    .unwrap();
    let checkpoint = margins_dir.join(format!("{meeting_id}_seg0.live-transcript.json"));
    let payload = serde_json::json!({"terminal": terminal, "decoded_until_ms": decoded, "transcripts": [{"words": [{"channel": 0, "start_ms": 500, "end_ms": 900, "text": " hello"}]}]});
    std::fs::write(checkpoint, serde_json::to_vec(&payload).unwrap()).unwrap();
    std::fs::write(margins_dir.join("current"), format!("{meeting_id}\n")).unwrap();
}

fn multi_project_services(projects: Arc<MultiProject>) -> CliServices {
    let mut services = services(&projects.vaults[0].root_dir);
    services.projects = projects;
    services
}

impl RecordingProject {
    fn resolved(&self) -> ResolvedProject {
        FixedProject(self.root.clone()).resolved()
    }
}

impl ProjectService for RecordingProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        Ok(vec![self.resolved()])
    }
    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        self.resolved_selectors
            .lock()
            .unwrap()
            .push(selector.map(str::to_string));
        Ok(self.resolved())
    }
    fn set_active(&self, _selector: &str) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn add(
        &self,
        path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        self.added_paths.lock().unwrap().push(path.to_string());
        Ok(self.resolved())
    }
}

#[test]
fn selected_project_is_forwarded_without_changing_process_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let project_root = temp.path().join("selected-project");
    std::fs::create_dir_all(&project_root).unwrap();
    let projects = Arc::new(RecordingProject {
        root: project_root,
        added_paths: Mutex::new(Vec::new()),
        resolved_selectors: Mutex::new(Vec::new()),
    });
    let mut services = services(&projects.root);
    services.projects = projects.clone();
    let cwd_before = std::env::current_dir().unwrap();

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "recent", "--project=selected"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert_eq!(stdout, "<margins_recent />\n");
    assert_eq!(
        projects.resolved_selectors.lock().unwrap().as_slice(),
        &[Some("selected".into())]
    );
    assert_eq!(std::env::current_dir().unwrap(), cwd_before);
}

#[test]
fn concrete_meeting_commands_find_the_unique_owning_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let owner = temp.path().join("owner");
    std::fs::create_dir_all(active.join(".margins")).unwrap();
    seed_inspectable_session(&owner, "cross-vault", "owner transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("owner", &owner),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    for args in [
        vec!["margins", "artifacts", "cross-vault"],
        vec!["margins", "transcript", "cross-vault"],
    ] {
        let (result, stdout, stderr) = invoke(&services, &active, &args);
        assert!(result.is_ok(), "{args:?}: {stderr}");
        assert!(stdout.contains(&owner.to_string_lossy().to_string()));
        assert!(!stdout.contains("<exists>false</exists>"));
    }
}

#[test]
fn concrete_meeting_lookup_deduplicates_the_active_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    seed_inspectable_session(&active, "active-meeting", "active transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        // The initial resolved vault is appended by the dispatcher; this list
        // entry must not make the active meeting look ambiguous.
        vaults: vec![resolved_vault("active", &active)],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    let (result, stdout, stderr) = invoke(
        &services,
        &active,
        &["margins", "transcript", "active-meeting"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("active transcript"));
}

#[test]
fn duplicate_concrete_meeting_ids_fail_closed_even_when_active_has_one() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "duplicate", "active transcript");
    seed_inspectable_session(&other, "duplicate", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    let (result, stdout, stderr) =
        invoke(&services, &active, &["margins", "artifacts", "duplicate"]);
    let error = result.unwrap_err();
    assert_eq!(error.code(), "ambiguous_meeting");
    assert!(stdout.is_empty());
    assert!(stderr.contains("active"));
    assert!(stderr.contains("other"));
    assert!(stderr.contains("Pass --project"));
}

#[test]
fn explicit_project_remains_authoritative_for_duplicate_meeting_ids() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "duplicate", "active transcript");
    seed_inspectable_session(&other, "duplicate", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects.clone());

    let (result, stdout, stderr) = invoke(
        &services,
        &active,
        &["margins", "transcript", "duplicate", "--project=other"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("other transcript"));
    assert!(!stdout.contains("active transcript"));
    assert_eq!(*projects.list_calls.lock().unwrap(), 0);
}

#[test]
fn latest_remains_scoped_to_the_active_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "active-latest", "active transcript");
    seed_inspectable_session(&other, "other-latest", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects.clone());

    let (result, stdout, stderr) = invoke(&services, &active, &["margins", "transcript", "latest"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("active transcript"));
    assert!(!stdout.contains("other transcript"));
    assert_eq!(*projects.list_calls.lock().unwrap(), 0);
}

#[test]
fn transcript_defaults_to_latest_json_and_preserves_default_xml() {
    let temp = tempfile::tempdir().unwrap();
    seed_inspectable_session(temp.path(), "latest-json", "latest transcript");
    let services = services(temp.path());

    let (result, default_xml, stderr) = invoke(&services, temp.path(), &["margins", "transcript"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(default_xml
        .starts_with("<margins_transcript meeting_id=\"latest-json\" view=\"aligned\">\n"));

    let (result, explicit_text, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcript", "--format", "text"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert_eq!(default_xml, explicit_text);

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcript", "--format", "json"],
    );
    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let object = value.as_object().unwrap();
    for field in [
        "meeting_id",
        "body",
        "view",
        "decoded_until_ms",
        "committed_until_ms",
        "updated_at_unix_ms",
        "live",
        "terminal",
    ] {
        assert!(object.contains_key(field), "missing JSON field {field}");
    }
    assert_eq!(value["meeting_id"], "latest-json");
    assert_eq!(value["body"], "# latest transcript");
    assert_eq!(value["view"], "aligned");
    assert_eq!(value["decoded_until_ms"], 0);
    assert_eq!(value["committed_until_ms"], 0);
    assert!(value["updated_at_unix_ms"].as_u64().unwrap() > 0);
    assert_eq!(value["live"], false);
    assert_eq!(value["terminal"], true);
}

#[test]
fn transcript_json_reports_live_and_terminal_checkpoint_state() {
    for (meeting_id, terminal, expected_live, decoded) in [
        ("live-meeting", false, true, 12_345),
        ("terminal-meeting", true, false, 67_890),
    ] {
        let temp = tempfile::tempdir().unwrap();
        seed_checkpoint_session(temp.path(), meeting_id, terminal, decoded);
        let services = services(temp.path());
        let (result, stdout, stderr) = invoke(
            &services,
            temp.path(),
            &["margins", "transcript", meeting_id, "--format", "json"],
        );
        assert!(result.is_ok(), "{meeting_id}: {stderr}");
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["meeting_id"], meeting_id);
        assert_eq!(value["view"], "full");
        assert_eq!(value["decoded_until_ms"], decoded);
        // Version-one compact checkpoints stored committed words but did not
        // carry a separate commit watermark.
        assert_eq!(value["committed_until_ms"], decoded);
        assert!(value["updated_at_unix_ms"].as_u64().unwrap() > 0);
        assert_eq!(value["terminal"], terminal);
        assert_eq!(value["live"], expected_live);
        assert!(value["body"].as_str().unwrap().contains("hello"));
    }
}

struct EmptyAsr;

impl AsrBackend for EmptyAsr {
    fn backend_name(&self) -> &'static str {
        "empty-fixture"
    }

    fn transcribe(&self, _request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        Ok(AsrResult {
            words: Vec::new(),
            detected_language: None,
        })
    }
}

#[test]
fn transcribe_resolves_audio_and_memo_relative_to_invocation() {
    let temp = tempfile::tempdir().unwrap();
    let invocation = temp.path().join("invocation");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&invocation).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    margins_media::audio::write_interleaved_wav(
        &invocation.join("input.wav"),
        &[0.0; 320],
        16_000,
        1,
    )
    .unwrap();
    std::fs::write(invocation.join("memo.md"), "[00:00] relative memo\n").unwrap();
    let mut services = services(&project);
    services.asr = Arc::new(EmptyAsr);

    let (result, stdout, stderr) = invoke(
        &services,
        &invocation,
        &[
            "margins",
            "transcribe",
            "input.wav",
            "--memo",
            "memo.md",
            "--name",
            "relative",
        ],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("meeting_id=\"relative\""));
    assert_eq!(
        std::fs::read_to_string(project.join(".margins/relative.md")).unwrap(),
        "[00:00] relative memo\n"
    );
    assert!(project.join(".margins/relative_seg0.wav").is_file());
    assert!(!invocation.join(".margins").exists());
}

#[test]
fn workspace_transcribe_keeps_generated_artifacts_out_of_notes_sources() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("notes");
    let invocation = temp.path().join("invocation");
    let unrelated_project = temp.path().join("legacy-project");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&invocation).unwrap();
    std::fs::create_dir_all(&unrelated_project).unwrap();
    margins_media::audio::write_interleaved_wav(
        &invocation.join("input.wav"),
        &[0.0; 320],
        16_000,
        1,
    )
    .unwrap();
    std::fs::write(invocation.join("memo.md"), "[00:00] public memo\n").unwrap();
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    let mut services = services(&unrelated_project);
    services.asr = Arc::new(EmptyAsr);

    let (result, stdout, stderr) = invoke(
        &services,
        &invocation,
        &[
            "margins",
            "--workspace",
            "practice",
            "transcribe",
            "input.wav",
            "--memo",
            "memo.md",
            "--name",
            "public-input",
        ],
    );
    let (transcript_result, transcript_stdout, transcript_stderr) = invoke(
        &services,
        &invocation,
        &[
            "margins",
            "--workspace",
            "practice",
            "transcript",
            "latest",
            "--format",
            "json",
        ],
    );
    let (artifacts_result, artifacts_stdout, artifacts_stderr) = invoke(
        &services,
        &invocation,
        &["margins", "--workspace", "practice", "artifacts", "latest"],
    );
    let (recent_result, recent_stdout, recent_stderr) = invoke(
        &services,
        &invocation,
        &["margins", "--workspace", "practice", "recent"],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());

    let capture_store = margins_home.join("workspaces/practice/captures/.margins");
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("meeting_id=\"public-input\""));
    assert!(transcript_result.is_ok(), "{transcript_stderr}");
    let transcript: serde_json::Value = serde_json::from_str(&transcript_stdout).unwrap();
    assert_eq!(transcript["meeting_id"], "public-input");
    assert_eq!(
        PathBuf::from(transcript["memo_path"].as_str().unwrap()),
        capture_store.join("public-input.md")
    );
    assert!(artifacts_result.is_ok(), "{artifacts_stderr}");
    assert!(artifacts_stdout.contains("meeting_id=\"public-input\""));
    assert!(artifacts_stdout.contains(capture_store.to_string_lossy().as_ref()));
    assert!(recent_result.is_ok(), "{recent_stderr}");
    assert!(recent_stdout.contains("id=\"public-input\""));
    assert!(capture_store.join("public-input.md").is_file());
    assert!(capture_store.join("public-input_transcript.json").is_file());
    assert!(!notes.join(".margins").exists());
    assert!(!unrelated_project.join(".margins").exists());
}

#[test]
fn unavailable_capture_is_stable_and_precedes_all_mutation() {
    for args in [
        vec!["margins"],
        vec!["margins", "new", "--title", "Never written"],
        vec!["margins", "attach", "missing"],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        let error = result.unwrap_err();
        assert_eq!(error.code(), "capture_unavailable");
        assert_eq!(error.exit_code(), 69);
        assert!(stdout.is_empty());
        let expected_message = if cfg!(target_os = "macos") {
            "capture is unavailable. On macOS, the most likely cause is missing \"Screen &amp; System Audio Recording\" permission. Grant it to your terminal in System Settings &gt; Privacy &amp; Security &gt; Screen &amp; System Audio Recording, then quit and reopen the terminal before running Margins again."
        } else {
            "capture is unavailable in this build"
        };
        assert_eq!(
            stderr,
            format!(
                "<margins_error code=\"capture_unavailable\">{expected_message}</margins_error>\n"
            )
        );
        assert!(!temp.path().join(".margins").exists());
    }
}

#[derive(Default)]
struct FakeCapture;

impl CaptureProvider for FakeCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            available: true,
            supported_lanes: vec![AudioLane::Microphone],
            supports_device_selection: false,
            supports_live_pcm: false,
            unavailable_reason: None,
        }
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn start(
        &self,
        request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Ok(Box::new(FakeHandle {
            snapshot: CaptureSnapshot {
                session_id: request.session_id,
                segment_id: request.segment_id,
                state: CaptureState::Capturing,
                lanes: vec![CaptureLaneSnapshot {
                    lane: AudioLane::Microphone,
                    state: CaptureLaneState::Active,
                    generation: 0,
                    delivered_frames: 0,
                    durable_frames: 0,
                    observed_signal: false,
                    dropped_live_frames: 0,
                    dropped_durable_frames: 0,
                    last_error_code: None,
                }],
                timeline_reusable: true,
            },
        }))
    }
}

struct FailingStartCapture;

impl CaptureProvider for FailingStartCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        FakeCapture.capabilities()
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn start(
        &self,
        _request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Err(CaptureError::unavailable("provider disappeared"))
    }
}

struct DeniedCapture;

impl CaptureProvider for DeniedCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        FakeCapture.capabilities()
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Denied)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Denied)
    }
    fn start(
        &self,
        _request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        unreachable!("permission denial must preflight before start")
    }
}

struct UnknownSystemCapture;

impl CaptureProvider for UnknownSystemCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            available: true,
            supported_lanes: vec![AudioLane::System],
            supports_device_selection: false,
            supports_live_pcm: false,
            unavailable_reason: None,
        }
    }

    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }

    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Unknown)
    }

    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        panic!("unknown system permission must be proven by capture health, not requested")
    }

    fn start(
        &self,
        request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Ok(Box::new(FakeHandle {
            snapshot: CaptureSnapshot {
                session_id: request.session_id,
                segment_id: request.segment_id,
                state: CaptureState::Capturing,
                lanes: vec![CaptureLaneSnapshot {
                    lane: AudioLane::System,
                    state: CaptureLaneState::Active,
                    generation: 0,
                    delivered_frames: 0,
                    durable_frames: 0,
                    observed_signal: false,
                    dropped_live_frames: 0,
                    dropped_durable_frames: 0,
                    last_error_code: None,
                }],
                timeline_reusable: true,
            },
        }))
    }
}

struct FakeHandle {
    snapshot: CaptureSnapshot,
}

impl CaptureHandle for FakeHandle {
    fn snapshot(&self) -> Result<CaptureSnapshot, CaptureError> {
        Ok(self.snapshot.clone())
    }
    fn command(&self, command: CaptureCommand) -> Result<CaptureCommandResult, CaptureError> {
        let mut snapshot = self.snapshot.clone();
        snapshot.state = CaptureState::Finished;
        Ok(CaptureCommandResult {
            operation_id: command.operation_id,
            status: CaptureCommandStatus::Applied,
            snapshot,
            completed_artifacts: Vec::new(),
        })
    }
}

#[test]
fn injected_capture_keeps_one_stable_id_across_attaches() {
    let temp = tempfile::tempdir().unwrap();
    let mut services = services(temp.path());
    services.capture = Arc::new(FakeCapture);
    for args in [
        vec!["margins", "new", "--title", "Stable"],
        vec!["margins", "attach"],
        vec!["margins", "attach"],
    ] {
        let (result, _, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{stderr}");
    }
    let margins_dir = temp.path().join(".margins");
    let current = std::fs::read_to_string(margins_dir.join("current")).unwrap();
    let id = current.trim();
    assert_eq!(id, "2026-08-10-12-00-00");
    let meta = legacy::get_session_meta(&margins_dir, id).unwrap();
    assert_eq!(meta.title.as_deref(), Some("Stable"));
    assert_eq!(
        meta.segments
            .iter()
            .map(|segment| segment.segment_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(legacy::list_sessions(&margins_dir).unwrap().len(), 1);
}

#[test]
fn permission_and_failed_start_leave_no_capture_mutation() {
    for (provider, expected) in [
        (
            Arc::new(DeniedCapture) as Arc<dyn CaptureProvider>,
            "capture_permission_denied",
        ),
        (
            Arc::new(FailingStartCapture) as Arc<dyn CaptureProvider>,
            "capture_unavailable",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut services = services(temp.path());
        services.capture = provider;
        let (result, stdout, _) = invoke(&services, temp.path(), &["margins", "new"]);
        assert_eq!(result.unwrap_err().code(), expected);
        assert!(stdout.is_empty());
        assert!(!temp.path().join(".margins").exists());
    }
}

#[test]
fn unknown_system_permission_is_deferred_to_delivery_health() {
    let temp = tempfile::tempdir().unwrap();
    let mut services = services(temp.path());
    services.capture = Arc::new(UnknownSystemCapture);

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "new"]);

    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.is_empty());
    assert!(temp.path().join(".margins/current").is_file());
}

#[test]
fn no_feature_asr_fails_before_transcribe_writes() {
    let temp = tempfile::tempdir().unwrap();
    let audio = temp.path().join("relative.wav");
    margins_media::audio::write_interleaved_wav(&audio, &[0.0; 320], 16_000, 1).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let services = services(&project);
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcribe", "relative.wav", "--name", "blocked"],
    );
    assert_eq!(result.unwrap_err().code(), "asr_unavailable");
    assert!(stdout.is_empty());
    assert_eq!(stderr, "<margins_error code=\"asr_unavailable\">ASR is unavailable in this build</margins_error>\n");
    assert!(!project.join(".margins").exists());
}

#[test]
fn read_only_input_errors_precede_backend_unavailability() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let invocation = temp.path().join("invocation");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&invocation).unwrap();
    let services = services(&project);

    let (result, stdout, stderr) = invoke(
        &services,
        &invocation,
        &["margins", "transcribe", "missing.wav"],
    );
    assert_eq!(result.unwrap_err().code(), "audio_not_found");
    assert!(stdout.is_empty());
    assert!(stderr.contains("Audio file not found:"));
    assert!(!project.join(".margins").exists());

    let (result, stdout, stderr) =
        invoke(&services, &invocation, &["margins", "process", "missing"]);
    assert_eq!(result.unwrap_err().code(), "store_not_found");
    assert!(stdout.is_empty());
    assert_eq!(
        stderr,
        "<margins_error code=\"store_not_found\">No .margins/ directory found.</margins_error>\n"
    );
    assert!(!project.join(".margins").exists());
}

#[test]
fn unavailable_process_backends_do_not_replace_existing_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let margins_dir = project.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    let started = Local.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        None,
    )
    .unwrap();
    let transcript = margins_dir.join("meeting_transcript.json");
    std::fs::write(&transcript, "preserve-me").unwrap();
    let services = services(&project);

    for args in [
        vec!["margins", "process", "meeting"],
        vec!["margins", "process", "meeting", "--speakers", "2"],
    ] {
        let (result, stdout, _) = invoke(&services, temp.path(), &args);
        let error = result.unwrap_err();
        assert!(matches!(
            error.code(),
            "asr_unavailable" | "diarization_unavailable"
        ));
        assert!(stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&transcript).unwrap(), "preserve-me");
        assert!(!margins_dir.join("meeting_aligned.md").exists());
        assert!(!margins_dir.join("artifacts").exists());
    }
}

#[test]
fn xml_and_json_presenters_escape_user_controlled_values() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    let margins_dir = temp.path().join(".margins");
    let started = Local.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    legacy::set_title(
        &margins_dir,
        "meeting",
        Some("A <title> & \"quote\"".into()),
    )
    .unwrap();
    std::fs::write(margins_dir.join("current"), "meeting\n").unwrap();

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "current"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<title>A &lt;title&gt; &amp; \"quote\"</title>"));
    assert!(!stdout.contains(''));
}

#[test]
fn no_feature_diarization_fails_before_transcribe_writes() {
    let temp = tempfile::tempdir().unwrap();
    let audio = temp.path().join("relative.wav");
    margins_media::audio::write_interleaved_wav(&audio, &[0.0; 320], 16_000, 1).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let services = services(&project);
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcribe", "relative.wav", "--speakers", "2"],
    );
    assert_eq!(result.unwrap_err().code(), "diarization_unavailable");
    assert!(stdout.is_empty());
    assert_eq!(stderr, "<margins_error code=\"diarization_unavailable\">diarization is unavailable in this build</margins_error>\n");
    assert!(!project.join(".margins").exists());
}

#[test]
fn align_only_does_not_consult_unavailable_asr() {
    let temp = tempfile::tempdir().unwrap();
    let margins_dir = temp.path().join(".margins");
    let memo = temp.path().join("memo.md");
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        &memo.to_string_lossy(),
    )
    .unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        Some(1.0),
    )
    .unwrap();
    std::fs::write(
        margins_dir.join("meeting_transcript.json"),
        r#"{"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":"hello"}]}]}"#,
    )
    .unwrap();
    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "process", "meeting", "--align-only"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<margins_process meeting_id=\"meeting\" status=\"ok\">"));
    assert!(margins_dir.join("meeting_aligned.md").exists());
}

#[test]
fn archive_commands_route_new_aligned_output_to_visible_default_folder() {
    let temp = tempfile::tempdir().unwrap();
    let margins_dir = temp.path().join(".margins");
    let memo = margins_dir.join("meeting.md");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        Some(1.0),
    )
    .unwrap();
    std::fs::write(
        margins_dir.join("meeting_transcript.json"),
        r#"{"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":"hello"}]}]}"#,
    )
    .unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "archive", "on"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"true\""));
    assert!(stdout.contains("path=\"") && stdout.contains("_margins"));

    let (result, _, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "process", "meeting", "--align-only"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(temp.path().join("_margins/meeting_aligned.md").is_file());
    assert!(!margins_dir.join("meeting_aligned.md").exists());
    assert_eq!(
        legacy::list_session_artifacts(&margins_dir, "meeting").unwrap()[0].path,
        "_margins/meeting_aligned.md"
    );

    let (result, stdout, stderr) =
        invoke(&services, temp.path(), &["margins", "archive", "status"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"true\""));
    assert!(stdout.contains("transcripts=\"1\""));

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "archive", "off"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"false\""));
    assert!(margins_dir.join("meeting_aligned.md").is_file());
    assert!(!temp.path().join("_margins").exists());
}

#[test]
fn session_catalog_and_artifact_commands_emit_vault_anchored_paths() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let margins_dir = vault.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    let memo = margins_dir.join("meeting.md");
    let transcript = margins_dir.join("meeting_seg0.live-transcript.json");
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    std::fs::write(
        &transcript,
        r#"{"terminal":true,"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":" hello"}]}]}"#,
    )
    .unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        "meeting",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/meeting_seg0.live-transcript.json",
        "durable",
        None,
    )
    .unwrap();
    let services = services(&vault);

    for args in [
        vec!["margins", "recent"],
        vec!["margins", "artifacts", "meeting"],
        vec!["margins", "transcript", "meeting"],
    ] {
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{args:?}: {stderr}");
        assert!(
            stdout.contains(&vault.to_string_lossy().to_string()),
            "{args:?} returned a cwd-relative artifact path: {stdout}"
        );
    }
}
#[test]
fn parser_accepts_note_print_dry_run() {
    let parsed = Args::try_parse_from(["margins", "note", "--print"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Note { print: true })
    ));
}

#[test]
fn public_note_handoff_defaults_to_latest_session_in_selected_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "--workspace", "practice", "note", "--print"],
    );

    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("my latest Margins session"));
    assert!(stdout.contains("workspace practice"));
    assert!(!stdout.contains("I provide"));
}

#[test]
fn bare_margins_creates_without_a_current_session_then_resumes_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut services = services(temp.path());
    services.capture = Arc::new(FakeCapture);

    let (first, _, first_stderr) = invoke(&services, temp.path(), &["margins"]);
    assert!(first.is_ok(), "{first_stderr}");

    let margins_dir = temp.path().join(".margins");
    let current = std::fs::read_to_string(margins_dir.join("current")).unwrap();
    let id = current.trim();
    let first_meta = legacy::get_session_meta(&margins_dir, id).unwrap();
    assert_eq!(first_meta.segments.len(), 1);

    let (second, _, second_stderr) = invoke(&services, temp.path(), &["margins"]);
    assert!(second.is_ok(), "{second_stderr}");

    let second_current = std::fs::read_to_string(margins_dir.join("current")).unwrap();
    assert_eq!(second_current.trim(), id);
    let resumed_meta = legacy::get_session_meta(&margins_dir, id).unwrap();
    assert_eq!(
        resumed_meta
            .segments
            .iter()
            .map(|segment| segment.segment_index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(legacy::list_sessions(&margins_dir).unwrap().len(), 1);
}

#[test]
fn retention_cli_preview_is_deterministic_and_apply_replays_with_scope_effects() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path();
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    let margins_home = temp.path().join("margins-home");
    std::env::set_var("MARGINS_HOME", &margins_home);
    let old_margins_home = Some(std::ffi::OsString::from(margins_home.as_os_str()));
    let workspace = margins_workflows::workspace::create_workspace(
        &margins_home,
        "retention-practice",
        None,
        vault,
    )
    .unwrap();
    use margins_workflows::integrations::{
        ConnectorCtx, IntegrationsStore, RawItemDraft, ThreadEvidence, EMAIL_CONNECTOR_ID,
    };
    let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
    let ctx = ConnectorCtx {
        vault_root: workspace.state_dir.clone(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let occurred = chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    store
        .replace_email_thread_snapshot(
            &ctx,
            vec![ThreadEvidence {
                thread_id: "active".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "active".into(),
                href: None,
            }],
            Vec::new(),
        )
        .unwrap();
    store
        .ingest_raw_connector_items(
            &ctx,
            vec![RawItemDraft {
                source_id: "active".into(),
                payload: serde_json::json!({"cache": true}),
            }],
        )
        .unwrap();
    let services = services(vault);
    let (status_result, status_stdout, status_stderr) = invoke(
        &services,
        vault,
        &[
            "margins",
            "--workspace",
            "retention-practice",
            "workspace",
            "status",
            "--json",
        ],
    );
    assert!(status_result.is_ok(), "{status_stderr}");
    let status: serde_json::Value = serde_json::from_str(&status_stdout).unwrap();
    let revision = status["revision"].as_str().unwrap().to_string();

    let (preview_a, preview_stdout_a, preview_stderr_a) =
        invoke_retention_preview(&services, vault, "email", "owner@example.com", "raw-cache");
    assert!(preview_a.is_ok(), "{preview_stderr_a}\n{preview_stdout_a}");
    let plan_a: serde_json::Value = serde_json::from_str(&preview_stdout_a).unwrap();
    assert_eq!(plan_a["schema_version"], "margins.retention.preview.v1");
    assert_eq!(plan_a["destructive"], true);
    assert_eq!(plan_a["index_refresh_required"], false);
    assert_eq!(plan_a["target"]["connector_id"], "email");
    assert_eq!(plan_a["target"]["source_account"], "owner@example.com");
    assert_eq!(plan_a["counts"]["raw_items"], 1);

    let (preview_b, preview_stdout_b, _) =
        invoke_retention_preview(&services, vault, "email", "owner@example.com", "raw-cache");
    assert!(preview_b.is_ok());
    let plan_b: serde_json::Value = serde_json::from_str(&preview_stdout_b).unwrap();
    assert_eq!(plan_a["plan_id"], plan_b["plan_id"]);
    assert_eq!(plan_a["ledger_fingerprint"], plan_b["ledger_fingerprint"]);

    let plan_path = temp.path().join("retention-plan.json");
    std::fs::write(&plan_path, preview_stdout_a.trim()).unwrap();
    let (apply_result, apply_stdout, apply_stderr) =
        invoke_retention_apply(&services, vault, &plan_path, &revision, "retention-cli-1");
    assert!(apply_result.is_ok(), "{apply_stderr}\n{apply_stdout}");
    let applied: serde_json::Value = serde_json::from_str(&apply_stdout).unwrap();
    assert_eq!(applied["schema_version"], "margins.retention.apply.v1");
    assert_eq!(applied["replayed"], false);
    assert_eq!(store.raw_items(&ctx).unwrap().len(), 0);
    assert_eq!(store.thread_evidence(&ctx).unwrap().len(), 1);

    let (replay_result, replay_stdout, replay_stderr) =
        invoke_retention_apply(&services, vault, &plan_path, &revision, "retention-cli-1");
    assert!(replay_result.is_ok(), "{replay_stderr}");
    let replay: serde_json::Value = serde_json::from_str(&replay_stdout).unwrap();
    assert_eq!(replay["replayed"], true);

    let (materialization_preview, materialization_stdout, _) = invoke_retention_preview(
        &services,
        vault,
        "email",
        "owner@example.com",
        "materialization",
    );
    assert!(materialization_preview.is_ok());
    let materialization_path = temp.path().join("retention-materialization.json");
    std::fs::write(&materialization_path, materialization_stdout.trim()).unwrap();
    let (conflict_result, conflict_stdout, conflict_stderr) = invoke(
        &services,
        vault,
        &[
            "margins",
            "--workspace",
            "retention-practice",
            "retention",
            "apply",
            "--plan",
            &materialization_path.to_string_lossy(),
            "--if-revision",
            &revision,
            "--request-id",
            "retention-cli-1",
            "--json",
        ],
    );
    assert!(conflict_result.is_err(), "{conflict_stdout}");
    let conflict = parse_cli_json_error(&conflict_stderr);
    assert_eq!(conflict["schema_version"], "margins.error.v1");
    assert_eq!(conflict["error"]["code"], "idempotency_conflict");

    let (missing_workspace, missing_stdout, missing_stderr) = invoke(
        &services,
        vault,
        &[
            "margins",
            "retention",
            "apply",
            "--plan",
            &plan_path.to_string_lossy(),
            "--if-revision",
            &revision,
            "--request-id",
            "missing-workspace",
            "--json",
        ],
    );
    assert!(missing_workspace.is_err(), "{missing_stdout}");
    let missing = parse_cli_json_error(&missing_stderr);
    assert_eq!(missing["error"]["code"], "workspace_required");
    assert!(missing["error"]["message"]
        .as_str()
        .unwrap()
        .contains("this mutation requires literal --workspace"));

    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}
