use chrono::{Local, TimeZone};
use clap::Parser;
use margins_cli::args::Args;
use margins_cli::run;
use margins_cli::services::{CliServices, Clock, ProjectService};
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError};
use margins_meeting_protocol::{SessionId, SessionMillis, WorkspaceMemoUpdateV1};
use margins_store::canonical;
use margins_workflows::project::{ProjectSource, ResolvedProject};
use margins_workflows::workspace::{
    self, GmailCollectionSelector, GranolaTimeRange, WorkspaceBinding,
};
use margins_workflows::workspace_service::{ServicePrincipal, WorkspaceService};
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

#[test]
fn workspace_default_and_destination_are_explicit_json_reads() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let machine = temp.path().join("machine");
    let vault = temp.path().join("vault");
    let code = temp.path().join("code");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&code).unwrap();
    let old = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &machine);
    let service = services(&code);
    let (missing, _, _) = invoke(
        &service,
        &code,
        &["margins", "workspace", "destination", "--json"],
    );
    assert!(missing.is_err());
    let workspace = workspace::create_workspace(&machine, "practice", None, &vault).unwrap();
    let (listed, output, _) = invoke(&service, &code, &["margins", "workspace", "list", "--json"]);
    assert!(listed.is_ok(), "{output}");
    let listing: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(listing["default_workspace"], serde_json::Value::Null);
    assert_eq!(listing["workspaces"][0]["id"], "practice");
    let mut desired = workspace.config.clone();
    let WorkspaceBinding::NativeMarkdown { note_folder, .. } =
        desired.bindings.get_mut("home").unwrap()
    else {
        panic!()
    };
    *note_folder = Some(PathBuf::from("inbox"));
    let plan = workspace::plan_workspace_config(&workspace, desired).unwrap();
    let mut workspace = workspace;
    workspace::apply_workspace_plan(&mut workspace, &plan).unwrap();
    let (set, output, _) = invoke(
        &service,
        &code,
        &[
            "margins",
            "workspace",
            "default",
            "--set",
            "practice",
            "--json",
        ],
    );
    assert!(set.is_ok(), "{output}");
    let (read, output, _) = invoke(
        &service,
        &code,
        &["margins", "workspace", "destination", "--json"],
    );
    assert!(read.is_ok(), "{output}");
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["workspace_id"], "practice");
    assert_eq!(json["home_source_id"], "home");
    assert_eq!(json["note_folder"], "inbox");
    assert_eq!(
        json["destination"],
        workspace.home_dir.join("inbox").to_string_lossy().as_ref()
    );
    restore_env("MARGINS_HOME", old.as_ref());
}

#[test]
fn workspace_plan_apply_and_migrate_use_enzyme_programs() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let machine = temp.path().join("machine");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(vault.join("people")).unwrap();
    let old = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &machine);
    let service = services(&vault);
    let workspace = workspace::create_workspace(&machine, "practice", None, &vault).unwrap();
    let program_path = machine.join("configs/practice.enzyme");
    assert_eq!(workspace.config_path, program_path);

    let (status, output, _) = invoke(
        &service,
        &vault,
        &["margins", "--workspace", "practice", "workspace", "status", "--json"],
    );
    assert!(status.is_ok(), "{output}");
    let status: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(status["config"], program_path.to_string_lossy().as_ref());
    assert_eq!(status["revision"], workspace.program.sha256());

    let desired = workspace.program.text().replace(
        "  remember in folder",
        "  learn questions from folder \"people\" about relationships {\n    sample by time\n  }\n\n  remember in folder",
    );
    let desired_path = temp.path().join("desired.enzyme");
    std::fs::write(&desired_path, &desired).unwrap();
    let (planned, output, stderr) = invoke(
        &service,
        &vault,
        &[
            "margins",
            "--workspace",
            "practice",
            "workspace",
            "plan",
            "--desired",
            desired_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(planned.is_ok(), "{stderr}");
    let plan: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(plan["schema_version"], "margins.workspace.plan.v2");
    assert_eq!(plan["workspace_id"], "practice");
    assert_eq!(plan["base_revision"], workspace.program.sha256());
    assert_eq!(plan["desired_program"], desired.as_str());
    assert_eq!(
        plan["desired_sha256"],
        margins_workflows::workspace::program_sha256(&desired)
    );
    assert!(plan["plan_id"].as_str().unwrap().len() == 64);
    assert!(plan["diff"]
        .as_str()
        .unwrap()
        .contains("+  learn questions from folder \"people\" about relationships {"));
    let actions = plan["actions"].as_array().unwrap();
    assert!(actions.iter().any(|action| action["action"] == "set_policy"));
    assert!(actions.iter().any(|action| action["action"] == "update_program"));
    assert!(actions
        .iter()
        .all(|action| action["summary"].as_str().is_some_and(|summary| !summary.is_empty())));
    let plan_path = temp.path().join("plan.json");
    std::fs::write(&plan_path, &output).unwrap();

    for replayed in [false, true] {
        let (applied, output, stderr) = invoke(
            &service,
            &vault,
            &[
                "margins",
                "--workspace",
                "practice",
                "workspace",
                "apply",
                "--plan",
                plan_path.to_str().unwrap(),
                "--json",
            ],
        );
        assert!(applied.is_ok(), "{stderr}");
        let receipt: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(receipt["schema_version"], "margins.workspace.apply.v2");
        assert_eq!(receipt["after_revision"], plan["desired_sha256"]);
        assert_eq!(receipt["replayed"], replayed);
    }
    assert_eq!(std::fs::read_to_string(&program_path).unwrap(), desired);

    // A desired file in the retired TOML shape is still accepted and keeps
    // the program's learning settings.
    let mut legacy = workspace::resolve_at(&machine, "practice").unwrap().config;
    legacy.policy.excluded_folders = vec!["archive".to_string()];
    let legacy_path = temp.path().join("desired.toml");
    std::fs::write(&legacy_path, toml::to_string_pretty(&legacy).unwrap()).unwrap();
    let (planned, output, stderr) = invoke(
        &service,
        &vault,
        &[
            "margins",
            "--workspace",
            "practice",
            "workspace",
            "plan",
            "--desired",
            legacy_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(planned.is_ok(), "{stderr}");
    let plan: serde_json::Value = serde_json::from_str(&output).unwrap();
    let program = plan["desired_program"].as_str().unwrap();
    assert!(program.contains("leave out folders [\"archive\"]"), "{program}");
    assert!(program.contains("sample by time"), "{program}");

    // An invalid desired program is refused with a stable code.
    std::fs::write(&desired_path, desired.replace("remember in folder", "remember in folders")).unwrap();
    let (refused, _, _) = invoke(
        &service,
        &vault,
        &[
            "margins",
            "--workspace",
            "practice",
            "workspace",
            "plan",
            "--desired",
            desired_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(refused.unwrap_err().code(), "workspace_desired_invalid");

    // Migration of a retired config.toml: dry run, then write, then nothing left.
    let legacy_dir = machine.join("workspaces/old");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("config.toml"),
        format!(
            "id = \"old\"\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n",
            vault.canonicalize().unwrap()
        ),
    )
    .unwrap();
    let (dry, output, stderr) = invoke(
        &service,
        &vault,
        &["margins", "workspace", "migrate", "--dry-run", "--json"],
    );
    assert!(dry.is_ok(), "{stderr}");
    let preview: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(preview["schema_version"], "margins.workspace.migrate.v1");
    assert_eq!(preview["status"], "would_migrate");
    assert!(!machine.join("configs/old.enzyme").exists());
    let (migrated, output, stderr) = invoke(
        &service,
        &vault,
        &["margins", "workspace", "migrate", "--json"],
    );
    assert!(migrated.is_ok(), "{stderr}");
    let migration: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(migration["status"], "migrated");
    assert_eq!(migration["program"], preview["program"]);
    assert_eq!(
        std::fs::read_to_string(machine.join("configs/old.enzyme")).unwrap(),
        preview["program"].as_str().unwrap()
    );
    assert!(legacy_dir.join("config.toml.migrated").is_file());
    let (again, output, _) = invoke(
        &service,
        &vault,
        &["margins", "workspace", "migrate", "--json"],
    );
    assert!(again.is_ok());
    assert!(output.is_empty());
    restore_env("MARGINS_HOME", old.as_ref());
}

#[test]
fn workspace_remove_only_accepts_a_non_default_config_only_workspace() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let machine = temp.path().join("machine");
    let vault = temp.path().join("vault");
    let code = temp.path().join("code");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&code).unwrap();
    let source_note = vault.join("keep.md");
    std::fs::write(&source_note, "source stays").unwrap();
    let old = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &machine);
    let service = services(&code);
    workspace::create_workspace(&machine, "default", None, &vault).unwrap();
    workspace::set_default_workspace(&machine, "default").unwrap();
    let candidate = workspace::create_workspace(&machine, "unused", None, &vault).unwrap();

    let (default_result, _, _) = invoke(
        &service,
        &code,
        &["margins", "workspace", "remove", "default", "--json"],
    );
    assert!(default_result.is_err());
    assert!(machine.join("configs/default.enzyme").exists());

    let (data_result, _, _) = invoke(
        &service,
        &code,
        &["margins", "workspace", "remove", "unused", "--json"],
    );
    assert!(data_result.is_err());
    assert!(candidate.config_path.exists());

    std::fs::remove_dir(candidate.state_dir.join("captures")).unwrap();
    let (removed, output, _) = invoke(
        &service,
        &code,
        &["margins", "workspace", "remove", "unused", "--json"],
    );
    assert!(removed.is_ok(), "{output}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["removed_workspace"],
        "unused"
    );
    assert!(!candidate.state_dir.exists());
    assert_eq!(
        std::fs::read_to_string(source_note).unwrap(),
        "source stays"
    );
    assert_eq!(
        workspace::default_workspace(&machine).unwrap().as_deref(),
        Some("default")
    );
    restore_env("MARGINS_HOME", old.as_ref());
}

#[test]
fn embedded_build_commit_matches_checkout_head_when_git_is_available() {
    let output = match std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return,
    };
    let head = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        margins_cli::build_info::get().commit,
        head.trim(),
        "Cargo reused build metadata from a different checkout revision"
    );
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
fn unsupported_remote_command_fails_before_transport_or_local_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "--remote",
            "http://127.0.0.1:9",
            "--workspace",
            "practice",
            "process",
            "must-not-connect",
        ],
    );
    let error = result.unwrap_err();
    assert_eq!(error.code(), "remote_command_unsupported");
    assert!(stdout.is_empty());
    assert!(stderr.contains("not supported by the remote adapter"));
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn unsupported_remote_flags_fail_before_transport_or_file_intake() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    let audio = temp.path().join("must-not-read.wav");
    std::fs::write(&audio, b"not audio").unwrap();
    let base = [
        "margins",
        "--remote",
        "http://127.0.0.1:9",
        "--workspace",
        "practice",
    ];
    let cases = [
        (vec!["recent", "--all"], "remote_option_unsupported"),
        (
            vec!["memo", "session-a", "--expected-revision", "memo-1"],
            "usage",
        ),
        (
            vec![
                "note-association",
                "session-a",
                "--source",
                "notes",
                "--path",
                "meeting.md",
            ],
            "usage",
        ),
        (
            vec!["transcribe", audio.to_str().unwrap(), "--speakers", "2"],
            "remote_option_unsupported",
        ),
    ];
    for (suffix, expected) in cases {
        let args = base.into_iter().chain(suffix).collect::<Vec<_>>();
        let (result, stdout, _stderr) = invoke(&services, temp.path(), &args);
        assert_eq!(result.unwrap_err().code(), expected);
        assert!(stdout.is_empty());
    }
    assert_eq!(std::fs::read(&audio).unwrap(), b"not audio");
}

#[test]
fn removed_context_subcommand_is_rejected() {
    let error = Args::try_parse_from([
        "margins",
        "context",
        "--person",
        "ada@example.com",
        "--json",
    ])
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("unrecognized subcommand 'context'"));
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
fn parser_and_help_expose_granola_workspace_source_binding() {
    Args::try_parse_from([
        "margins",
        "source",
        "add",
        "granola",
        "--name",
        "granola",
        "--account",
        "owner@example.com",
        "--time-range",
        "last_30_days",
    ])
    .unwrap();

    let error = Args::try_parse_from(["margins", "source", "add", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("granola"));
    assert!(help.contains("last_30_days"));
}

#[test]
fn workspace_source_add_persists_granola_account_and_time_range() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let margins_home = temp.path().join("margins-home");
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &vault).unwrap();
    let services = services(&vault);

    let (result, stdout, stderr) = invoke(
        &services,
        &vault,
        &[
            "margins",
            "--workspace",
            "practice",
            "source",
            "add",
            "granola",
            "--name",
            "granola",
            "--account",
            "Owner@Example.COM",
            "--time-range",
            "last_30_days",
            "--json",
        ],
    );

    assert!(result.is_ok(), "{stderr}");
    assert!(stderr.is_empty());
    let sources: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(sources
        .as_array()
        .unwrap()
        .iter()
        .any(|source| { source["name"] == "granola" && source["kind"] == "granola" }));
    let workspace = workspace::resolve_at(&margins_home, "practice").unwrap();
    assert!(matches!(
        &workspace.config.bindings["granola"],
        WorkspaceBinding::Granola { account, collection }
            if account == "owner@example.com"
                && collection.time_range == GranolaTimeRange::Last30Days
                && !collection.workspace_only
    ));
    restore_env("MARGINS_HOME", old_margins_home.as_ref());
}

#[test]
fn parser_accepts_workspace_plan_apply_and_integrations_reconcile() {
    Args::try_parse_from([
        "margins",
        "workspace",
        "plan",
        "--desired",
        "desired.enzyme",
        "--json",
    ])
    .unwrap();
    Args::try_parse_from(["margins", "workspace", "migrate", "--dry-run", "--json"]).unwrap();
    Args::try_parse_from([
        "margins",
        "workspace",
        "apply",
        "--plan",
        "plan.json",
        "--json",
    ])
    .unwrap();
    assert!(Args::try_parse_from([
        "margins",
        "workspace",
        "apply",
        "--plan",
        "plan.json",
        "--if-revision",
        "abc",
        "--json",
    ])
    .is_err());
    assert!(Args::try_parse_from([
        "margins",
        "workspace",
        "apply",
        "--plan",
        "plan.json",
        "--request-id",
        "req-1",
        "--json",
    ])
    .is_err());
    assert!(Args::try_parse_from(["margins", "workspace", "propose", "--json"]).is_err());
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
            note_folder: None,
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
            note_folder: None,
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
    let revision = workspace::workspace_revision(&workspace).unwrap();

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
fn parser_accepts_only_read_only_scan() {
    let parsed = Args::try_parse_from(["margins", "scan"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan)
    ));

    assert!(Args::try_parse_from(["margins", "scan", "--write-config"]).is_err());
    assert!(Args::try_parse_from(["margins", "scan", "--update"]).is_err());
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
fn workspace_setup_guide_exposes_coverage_and_entity_curation_and_is_read_only() {
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
        margins_workflows::resources::margins_workspace_setup_guide()
    );
    assert!(stdout.contains("# Knowledge Practice Review Contract"));
    assert!(stdout.contains("margins workspace new practice"));
    assert!(stdout.contains("margins init"));
    assert!(stdout.contains("margins sync --json"));
    assert!(stdout.contains("margins recall \"an exact phrase from these notes\""));
    assert!(stdout.contains("source add notes"));
    assert!(stdout.contains("Run these commands from the notes folder"));
    assert!(stdout.contains("cd \"/absolute/path/to/notes\""));
    assert!(stdout.contains("`recall.scan: true`"));
    assert!(stdout.contains("scan as soon as the explicit named Workspace"));
    assert!(stdout.contains("There is no `margins workspace scan` subcommand"));
    assert!(stdout.contains("Read the complete saved `scan.v2` result"));
    assert!(stdout.contains("Show the user an understanding, not scan output"));
    assert!(stdout.contains("The field names below are for your analysis"));
    assert!(stdout.contains("match how you work, and what did it miss?"));
    assert!(stdout.contains("Never open, cat, print, or summarize credential bundles"));
    assert!(stdout.contains("Use only redacted Margins product status"));
    assert!(stdout.contains("Do not run recall before `margins init`"));
    assert!(stdout.contains("margins setup --only catalyst"));
    assert!(stdout.contains("If `actions` is empty"));
    assert!(stdout.contains("apply the saved plan unchanged"));
    assert!(stdout.contains("workspace plan"));
    assert!(stdout.contains("Never hand-edit plan JSON"));
    assert!(stdout.contains("Do not ask for a second “apply this plan” confirmation"));
    assert!(stdout.contains("machine-level catalyst mode"));
    assert!(stdout.contains("not an exact-phrase boundary proof"));
    assert!(stdout.contains("contiguous, verbatim phrase"));
    assert!(stdout.contains("one universal discovery question"));
    assert!(stdout.contains("universal pause."));
    assert!(stdout.contains("Do not ask the user to design `[policy].entities`"));
    assert!(stdout.contains("Never declare setup complete while"));
    assert!(stdout.contains("Use exactly the spellings surfaced by scan"));
    assert!(stdout.contains("folder:<path>"));
    assert!(stdout.contains("`[policy].entities`"));
    assert!(stdout.contains("profile = \"relational\""));
    assert!(stdout.contains("expandable = true"));
    for field in [
        "summary",
        "instructions",
        "coverage_entities",
        "entity_curation_candidates",
        "top_entities",
        "top_folders",
        "top_tags",
        "top_links",
        "entity_samples",
        "representative_samples",
        "sample_files",
        "folder_stats",
        "folder_page_entities",
        "folder_children",
        "tag_children",
        "frontmatter_samples",
        "current_config",
        "available_profiles",
    ] {
        assert!(
            stdout.contains(&format!("`{field}`")),
            "missing scan field {field}"
        );
    }
    assert!(stdout.contains("`current_config.config_path`"));
    for profile in [
        "relational",
        "operational",
        "decision_trace",
        "resonance_trace",
        "reflective",
        "tension_trace",
        "preference_evidence",
    ] {
        assert!(
            stdout.contains(&format!("`{profile}`")),
            "missing profile {profile}"
        );
    }
    let normalized_guide = stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized_guide.contains("A fallback policy change after failure is not part"));
    assert!(normalized_guide.contains("Do not run unsupported discovery commands"));
    assert!(normalized_guide.contains("Do not provision a hosted lease at the start"));
    assert!(normalized_guide.contains("short-lived lease should begin as late as possible"));
    assert!(normalized_guide.contains("do not run `init` repeatedly"));
    assert!(normalized_guide.contains("earlier setup attempts as hypotheses"));
    assert!(normalized_guide.contains("preserve that policy"));
    assert!(normalized_guide.contains("`live_lexical` status only confirms that an index exists"));
    assert!(normalized_guide.contains("portable live Markdown coverage"));
    assert!(normalized_guide.contains("Never relabel that number as “indexed documents.”"));
    assert!(normalized_guide.contains("`mode = \"indexed\"` reports the persisted engine index"));
    assert!(normalized_guide.contains("`entity_curation_candidates[].spec`"));
    assert!(normalized_guide.contains("`entity_curation_candidates[].expansion`"));
    assert!(normalized_guide
        .contains("`expands_automatically = true` means `expandable = true` is redundant"));
    assert!(
        normalized_guide.contains("`mode = \"explicit_available\"` means real child pages exist")
    );
    assert!(normalized_guide.contains("frequency alone does not establish importance"));
    assert!(normalized_guide.contains("not a weight or an importance score"));
    assert!(normalized_guide.contains("Leave an ambiguous entity without a profile"));
    assert!(normalized_guide.contains("one note per person is an optional practice"));
    assert!(normalized_guide.contains("Do not create, reorganize, or configure those notes"));
    assert!(normalized_guide.contains("at most two future capture habits"));
    assert!(normalized_guide.contains("name the question that habit would make answerable"));
    assert!(normalized_guide.contains("Do not prescribe a generic folder taxonomy"));
    assert!(normalized_guide.contains("Lead the final handoff with what the proof revealed"));
    assert!(normalized_guide.contains("Do not mistake a successful command"));
    assert!(normalized_guide.contains("operational receipt"));
    assert!(normalized_guide.contains("revision hashes, similarity scores"));
    assert!(stdout.contains("Do not begin connected-note distillation as part of setup"));
    let declaration = stdout.find("margins workspace new practice").unwrap();
    let scan = stdout.find("margins --workspace practice scan").unwrap();
    let understanding = stdout
        .find("## 4. Show the user an understanding, not scan output")
        .unwrap();
    let plan = stdout
        .find("margins --workspace practice workspace plan")
        .unwrap();
    let apply = stdout
        .find("margins --workspace practice workspace apply")
        .unwrap();
    let initialize = stdout.find("margins --workspace practice init").unwrap();
    assert!(
        declaration < scan
            && scan < understanding
            && understanding < plan
            && plan < apply
            && apply < initialize
    );
    assert!(!stdout.contains("margins transcribe"));
    assert!(!stdout.contains("scan --write-config"));
    assert!(!stdout.contains("workspace propose"));
    assert!(!stdout.contains("--if-revision"));
    assert!(!stdout.contains("--request-id"));
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
fn guided_onboarding_routes_without_duplicating_setup_protocol() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) =
        invoke(&services, temp.path(), &["margins", "guide", "onboarding"]);

    assert!(result.is_ok(), "{stderr}");
    assert_eq!(
        stdout,
        margins_workflows::resources::MARGINS_GUIDED_ONBOARDING
    );
    let normalized = stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(stdout.contains("`margins guide workspace-setup`"));
    assert!(normalized.contains("sole source of truth for setup"));
    assert!(stdout.contains("Setup and distillation are separate"));
    assert!(
        normalized.contains("Speak about their notes, work, and questions in ordinary language")
    );
    assert!(normalized.contains("End with the useful thing Margins surfaced"));
    assert!(normalized.contains("setup result brief and secondary"));
    assert!(stdout.split_whitespace().count() < 300);
    for duplicated_detail in [
        "scan.v2",
        "workspace plan",
        "workspace apply",
        "current_config",
        "catalyst",
        "profile =",
        "margins init",
        "margins sync",
    ] {
        assert!(
            !stdout.contains(duplicated_detail),
            "onboarding duplicated setup detail: {duplicated_detail}"
        );
    }
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
    canonical::create_session(
        &margins_dir,
        meeting_id,
        &Local::now(),
        &format!(".margins/{meeting_id}.md"),
    )
    .unwrap();
    canonical::upsert_session_artifact(
        &margins_dir,
        meeting_id,
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
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
    canonical::create_session(
        &margins_dir,
        meeting_id,
        &Local::now(),
        &format!(".margins/{meeting_id}.md"),
    )
    .unwrap();
    canonical::add_segment(
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
    assert!(default_xml.starts_with(
        "<margins_transcript meeting_id=\"latest-json\" view=\"aligned\" terminal=\"true\" incomplete=\"false\""
    ));

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
        "captured_until_ms",
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
fn transcript_xml_marks_finished_short_live_checkpoint_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let meeting_id = "short-live";
    seed_checkpoint_session(temp.path(), meeting_id, true, 11);
    let dir = temp.path().join(".margins");
    canonical::update_segment_duration(&dir, meeting_id, 0, 48.5).unwrap();
    canonical::mark_session_ended(&dir, meeting_id).unwrap();
    std::fs::remove_file(dir.join("current")).unwrap();
    let (result, xml, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", meeting_id],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(xml.contains("view=\"incomplete\" terminal=\"false\" incomplete=\"true\""));
    assert!(xml.contains("captured_until_ms=\"48500\" decoded_until_ms=\"11\""));
}

#[test]
fn transcript_xml_keeps_pending_remote_sessions_out_of_offline_processing() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["remote-pending", "remote-active"] {
        canonical::create_session(&dir, name, &Local::now(), &format!(".margins/{name}.md"))
            .unwrap();
        canonical::add_segment(
            &dir,
            name,
            0,
            &format!("{name}_seg0.wav"),
            0,
            (name == "remote-pending").then_some(1.0),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("{name}_capture_context.md")),
            "<!-- margins:transcript-pending-v1 -->\nRemote ASR pending.\n",
        )
        .unwrap();
    }
    canonical::mark_session_ended(&dir, "remote-pending").unwrap();
    let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
    authority
        .reserve_producer("remote-active", "browser", "token")
        .unwrap();
    for (name, live) in [("remote-pending", false), ("remote-active", true)] {
        let (result, xml, stderr) = invoke(
            &services(temp.path()),
            temp.path(),
            &["margins", "transcript", name],
        );
        assert!(result.is_ok(), "{stderr}");
        assert!(xml.contains("view=\"pending\" terminal=\"false\" incomplete=\"false\""));
        assert!(xml.contains(&format!("live=\"{live}\"")));
    }
}

#[test]
fn active_remote_checkpoint_is_live_in_default_xml() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    std::fs::create_dir_all(&dir).unwrap();
    canonical::create_session(
        &dir,
        "remote-live",
        &Local::now(),
        ".margins/remote-live.md",
    )
    .unwrap();
    canonical::add_segment(&dir, "remote-live", 0, "remote-live_seg0.wav", 0, None).unwrap();
    std::fs::write(
        dir.join("remote-live_remote.live-transcript.json"),
        serde_json::json!({
            "version": 2, "terminal": false, "decoded_until_ms": 500,
            "transcripts": [{"words": [{"channel": 0, "start_ms": 100,
                "end_ms": 500, "text": "hello"}]}]
        })
        .to_string(),
    )
    .unwrap();
    let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
    authority
        .reserve_producer("remote-live", "browser", "token")
        .unwrap();
    let (result, xml, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", "remote-live"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(xml.contains("view=\"full\" terminal=\"false\" incomplete=\"false\" live=\"true\""));
    authority
        .release_producer("remote-live", "browser", "token")
        .unwrap();
    let (result, xml, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", "remote-live"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(xml.contains("view=\"pending\" terminal=\"false\" incomplete=\"false\" live=\"false\""));
    let (result, json, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", "remote-live", "--format", "json"],
    );
    assert!(result.is_ok(), "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(report["view"], "pending");
    assert_eq!(report["live"], false);
}

#[test]
fn remote_session_with_stale_final_remains_pending_for_server_asr() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    std::fs::create_dir_all(&dir).unwrap();
    canonical::create_session(
        &dir,
        "remote-stale",
        &Local::now(),
        ".margins/remote-stale.md",
    )
    .unwrap();
    canonical::add_segment(&dir, "remote-stale", 0, "seg0.wav", 0, Some(1.0)).unwrap();
    std::fs::write(dir.join("remote-stale_aligned.md"), "old remote transcript").unwrap();
    let coverage = canonical::transcript_coverage(
        &canonical::get_session_meta(&dir, "remote-stale")
            .unwrap()
            .segments,
    )
    .unwrap();
    canonical::register_processed_transcript(
        &dir,
        "remote-stale",
        ".margins/remote-stale_aligned.md",
        coverage,
    )
    .unwrap();
    let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
    authority
        .reserve_producer("remote-stale", "browser", "token")
        .unwrap();
    authority
        .release_producer("remote-stale", "browser", "token")
        .unwrap();
    canonical::add_segment(&dir, "remote-stale", 1, "seg1.wav", 10_000, Some(1.0)).unwrap();
    let (result, xml, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", "remote-stale"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(xml.contains("view=\"pending\" terminal=\"false\" incomplete=\"false\" live=\"false\""));
    assert!(xml.contains("Remote audio is waiting for server transcription"));
    assert!(!xml.contains("old remote transcript"));
}

#[test]
fn crashed_local_session_with_partial_final_is_explicitly_incomplete() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".margins");
    std::fs::create_dir_all(&dir).unwrap();
    canonical::create_session(
        &dir,
        "crashed-local",
        &Local::now(),
        ".margins/crashed-local.md",
    )
    .unwrap();
    canonical::add_segment(&dir, "crashed-local", 0, "seg0.wav", 0, Some(1.0)).unwrap();
    canonical::add_segment(&dir, "crashed-local", 1, "seg1.wav", 10_000, None).unwrap();
    std::fs::write(dir.join("crashed-local_aligned.md"), "partial transcript").unwrap();
    let coverage = canonical::transcript_coverage(
        &canonical::get_session_meta(&dir, "crashed-local")
            .unwrap()
            .segments,
    )
    .unwrap();
    canonical::register_processed_transcript(
        &dir,
        "crashed-local",
        ".margins/crashed-local_aligned.md",
        coverage,
    )
    .unwrap();
    let (result, xml, stderr) = invoke(
        &services(temp.path()),
        temp.path(),
        &["margins", "transcript", "crashed-local"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(
        xml.contains("view=\"incomplete\" terminal=\"false\" incomplete=\"true\" live=\"false\"")
    );
    assert!(xml.contains("partial transcript"));
}

#[test]
fn transcript_json_reports_live_and_terminal_checkpoint_state() {
    for (meeting_id, terminal, expected_live, decoded) in [
        ("live-meeting", false, true, 12_345),
        ("terminal-meeting", true, false, 67_890),
    ] {
        let temp = tempfile::tempdir().unwrap();
        seed_checkpoint_session(temp.path(), meeting_id, terminal, decoded);
        if expected_live {
            margins_store::SqliteWorkspaceAuthorityStorage::open(temp.path().join(".margins"))
                .unwrap()
                .reserve_producer(meeting_id, "test-producer", "token")
                .unwrap();
        }
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
    let resolved =
        workspace::resolve_workspace(&margins_home, Some("practice"), &invocation).unwrap();
    let service = WorkspaceService::open("same-process-service", resolved).unwrap();
    let principal = ServicePrincipal::full("service-reader", "practice");
    let service_sessions = service.sessions(&principal, None, 10).unwrap();
    assert_eq!(
        service_sessions.sessions[0].session_id.as_ref(),
        "public-input"
    );
    let service_artifacts = service.artifacts(&principal, "public-input").unwrap();
    assert!(service_artifacts
        .iter()
        .any(|artifact| artifact.kind == "transcript"));
    let service_memo = service
        .memo(&principal, &SessionId("public-input".into()))
        .unwrap();
    let service_edit = service
        .update_memo(
            &principal,
            &SessionId("public-input".into()),
            &WorkspaceMemoUpdateV1 {
                request_id: "service-edit-after-cli-create".into(),
                expected_revision: service_memo.revision,
                observed_at_ms: SessionMillis(500),
                paused: false,
                text: "public memo\nservice-visible edit".into(),
            },
        )
        .unwrap();
    assert_eq!(service_edit.lines[1].text, "service-visible edit");
    assert_eq!(
        service
            .memo(&principal, &SessionId("public-input".into()))
            .unwrap()
            .revision,
        service_edit.revision
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
    assert!(
        std::fs::read_to_string(capture_store.join("public-input.md"))
            .unwrap()
            .contains("service-visible edit")
    );
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

#[test]
fn explicit_workspace_new_is_unavailable_without_side_effects() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("notes");
    let unrelated = temp.path().join("unrelated");
    let legacy_project = temp.path().join("legacy-project");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&unrelated).unwrap();
    std::fs::create_dir_all(&legacy_project).unwrap();
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    let services = services(&legacy_project);

    let (result, _stdout, stderr) = invoke(
        &services,
        &unrelated,
        &[
            "margins",
            "--workspace",
            "practice",
            "new",
            "--title",
            "Remote parity",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());

    assert_eq!(
        result.unwrap_err().code(),
        "capture_unavailable",
        "{stderr}"
    );
    let canonical = margins_home.join("workspaces/practice/captures/.margins");
    assert!(!canonical.exists());
    assert!(!unrelated.join(".margins").exists());
    assert!(!legacy_project.join(".margins").exists());
    assert!(!notes.join(".margins").exists());
}

#[test]
fn explicit_workspace_rejects_project_before_capture_side_effects() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let old_margins_home = std::env::var_os("MARGINS_HOME");
    std::env::set_var("MARGINS_HOME", &margins_home);
    workspace::create_workspace(&margins_home, "practice", None, &notes).unwrap();
    let services = services(temp.path());

    let (result, _stdout, stderr) = invoke(
        &services,
        temp.path(),
        &[
            "margins",
            "--workspace",
            "practice",
            "--project",
            "test",
            "new",
        ],
    );
    restore_env("MARGINS_HOME", old_margins_home.as_ref());

    assert_eq!(result.unwrap_err().code(), "invalid_arguments");
    assert!(stderr.contains("cannot be combined"));
    assert!(!margins_home
        .join("workspaces/practice/captures/.margins")
        .exists());
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
    canonical::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    canonical::add_segment(
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
    canonical::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    canonical::set_title(
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
    canonical::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        &memo.to_string_lossy(),
    )
    .unwrap();
    canonical::add_segment(
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
    canonical::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    canonical::add_segment(
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
        canonical::list_session_artifacts(&margins_dir, "meeting").unwrap()[0].path,
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
    canonical::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    canonical::upsert_session_artifact(
        &margins_dir,
        "meeting",
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
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
fn bare_public_cli_is_unavailable_without_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    for _ in 0..2 {
        let (result, _, _) = invoke(&services, temp.path(), &["margins"]);
        assert_eq!(result.unwrap_err().code(), "capture_unavailable");
    }
    assert!(!temp.path().join(".margins").exists());
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
