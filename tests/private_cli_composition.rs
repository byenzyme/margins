use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(feature = "recall")]
#[path = "support/fixture_generator.rs"]
mod fixture_generator;

#[cfg(feature = "recall")]
#[path = "support/enzyme_bin.rs"]
mod enzyme_bin;

fn source(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

#[test]
fn production_binary_uses_private_composition_not_public_standalone_entrypoint() {
    let main = source("src/main.rs");
    assert!(main.contains("margins::cli::main_entry_from_env"));
    assert!(!main.contains("margins_cli::main_entry_from_env"));
}

#[test]
fn production_new_composes_native_recorder_and_memo_tui() {
    let composition = source("src/cli/capture_local.rs");
    assert!(composition.contains("crate::recorder::RecorderHandle::start"));
    assert!(composition.contains("crate::tui::run_tui"));
    assert!(composition.contains("stop_and_flush"));
    assert!(!composition.contains("UnavailableCaptureProvider"));
}

#[test]
fn standalone_hosted_credentials_do_not_call_macos_keychain_apis() {
    for path in ["src/hosted_credentials.rs", "src/cli.rs"] {
        let contents = source(path);
        for forbidden in [
            "keychain_get_service",
            "keychain_set_service",
            "keychain_delete_service",
            "IncludedLeaseKeychain",
            "SystemIncludedLeaseKeychain",
            "keyring::Entry",
            "Command::new(\"security\")",
        ] {
            assert!(
                !contents.contains(forbidden),
                "{path} must not use macOS Keychain API {forbidden}"
            );
        }
    }
}

#[test]
fn standalone_connection_routes_pin_file_credentials_without_indirect_keychain_fallbacks() {
    let connect = source("crates/public/margins-cli/src/commands/connect.rs");
    let connect = connect.split("#[cfg(test)]").next().unwrap();
    assert!(connect.contains("GoogleCredentialBackendKind::File0600"));
    assert!(connect.contains("GranolaCredentialBackendKind::File0600"));
    assert!(!connect.contains("GoogleCredentialBackendKind::OsKeyring"));
    assert!(!connect.contains("GranolaCredentialBackendKind::OsKeyring"));
    assert!(!connect.contains("GoogleAccountStore::new("));
    assert!(!connect.contains("GranolaAccountStore::new("));
    assert!(!connect.contains("usable_connection_metadata"));

    let integrations = source("crates/public/margins-cli/src/commands/integrations.rs");
    assert!(integrations.contains("GoogleTokenProvider::new_with_backend"));
    assert!(integrations.contains("GoogleCredentialBackendKind::File0600"));
    assert!(!integrations.contains("GoogleTokenProvider::new("));

}

#[test]
fn production_source_add_help_exposes_granola_workspace_source() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["source", "add", "--help"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", temp.path().join("margins-home"))
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("granola"), "{stdout}");
    assert!(stdout.contains("last_30_days"), "{stdout}");
}

#[test]
fn production_binary_logs_migration_warnings_to_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    let legacy_dir = margins_home.join("workspaces/odd");
    fs::create_dir_all(&legacy_dir).unwrap();
    fs::write(
        legacy_dir.join("config.toml"),
        format!(
            "id = \"odd\"\n\n[policy]\nentities = [\"person:ada\"]\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n",
            notes.canonicalize().unwrap()
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["workspace", "migrate", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let migration: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(migration["warnings"].as_array().unwrap().len(), 1);
    assert!(
        stderr.lines().any(
            |line| line.starts_with("margins: warning: migrated Workspace 'odd': ")
                && line.contains("person:ada")
        ),
        "{stderr}"
    );
}

#[test]
fn production_capture_preflights_and_uses_one_native_start_path() {
    let composition = source("src/cli/capture_local.rs");
    let interactive = composition
        .split("impl InteractiveSession for NativeInteractiveSession")
        .nth(1)
        .unwrap();
    assert!(interactive.contains("ensure_capture_permissions(&NativeCapturePermissionSource)?"));

    let local = source("src/cli/capture_local.rs");
    let create = local
        .split("fn create_native_session")
        .nth(1)
        .unwrap()
        .split("fn attach_native_session")
        .next()
        .unwrap();
    assert!(
        create.find("LocalMeetingProducer::reserve").unwrap()
            < create.find("run_segment(").unwrap()
    );
    assert!(
        create.find("prepare_initial_native_input").unwrap()
            < create.find("LocalMeetingProducer::reserve").unwrap()
    );
    assert!(create.find("write_current_session(").unwrap() < create.find("run_segment(").unwrap());

    let attach = local
        .split("fn attach_native_session")
        .nth(1)
        .unwrap()
        .split("fn run_segment")
        .next()
        .unwrap();
    let recover = attach.find("LocalMeetingProducer::recover").unwrap();
    assert!(recover < attach.find("meeting.recover_pending_segment").unwrap());
    assert!(
        attach.find("meeting.recover_pending_segment").unwrap()
            < attach.find("run_segment(").unwrap()
    );

    let open = local
        .split("fn open_native_recorder")
        .nth(1)
        .unwrap()
        .split("fn prepare_initial_native_input")
        .next()
        .unwrap();
    assert!(open.contains("RecorderHandle::start_with_selected_audio"));
    let bind = local
        .split("fn bind_native_segment")
        .nth(1)
        .unwrap()
        .split("fn run_segment")
        .next()
        .unwrap();
    assert!(bind.contains("meeting.start_stream"));
    assert!(local
        .split("fn run_segment")
        .nth(1)
        .unwrap()
        .contains("bind_native_segment("));
}

#[test]
fn packaged_note_print_resolves_agent_and_does_not_launch_it() {
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    let cursor_home = temp.path().join(".cursor");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir(&cursor_home).unwrap();
    let cursor_agent = bin_dir.join("cursor-agent");
    fs::write(&cursor_agent, "print mode must not execute this fixture").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&cursor_agent).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cursor_agent, permissions).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["note", "--print"])
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", &bin_dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Agent: Cursor\nCommand: cursor-agent \"distill my latest margins session\"\n"
    );
}

#[test]
#[cfg(feature = "recall")]
fn setup_output_redacts_credential_paths_and_secret_shaped_fixture_data() {
    use std::io::{Read, Write as _};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let body = r#"{"api_key":"sk-or-v1-secret-shaped-cli-fixture","base_url":"https://fixture.invalid/v1","model":"fixture-catalyst-model","expires_at":4102444800}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");

    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["setup", "--only", "catalyst"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env(
            "ENZYME_FREE_CONFIG_URL",
            format!("http://{address}/llm/free-config"),
        )
        .output()
        .unwrap();
    server.join().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap()
    );
    assert!(combined.contains("fixture-catalyst-model"));
    // Setup names where the Workspace program lives (under `configs/`); no
    // other Margins-home path, and nothing about credentials, is printed.
    let home_text = margins_home.to_string_lossy().to_string();
    let programs = format!("{home_text}/configs/");
    assert!(combined.contains(&programs), "{combined}");
    let combined = combined.replace(&programs, "<programs>/");
    for forbidden in [
        home_text,
        "llm-config-cache.json".to_string(),
        "sk-or-v1-secret-shaped-cli-fixture".to_string(),
        "fixture.invalid".to_string(),
        "bootstrap".to_string(),
    ] {
        assert!(
            !combined.contains(&forbidden),
            "setup output leaked {forbidden}: {combined}"
        );
    }
}

#[test]
fn capabilities_reports_setup_selected_status_and_ignores_ambient_provider_env() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["capabilities"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", temp.path().join("margins-home"))
        .env("OPENROUTER_API_KEY", "sk-or-v1-secret-shaped-env-fixture")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["catalyst"]["mode"], "none");
    assert_eq!(value["catalyst"]["usable"], false);
    assert_eq!(value["catalyst"]["reason"], "setup_required");
    assert_eq!(value["catalyst"]["expiry"]["state"], "not_applicable");
    assert!(!stdout.contains("explicit_env"));
    assert!(!stdout.contains("sk-or-v1-secret-shaped-env-fixture"));
    assert!(!stdout.contains(&temp.path().to_string_lossy().to_string()));
}

#[test]
#[cfg(feature = "recall")]
fn workspace_commands_refuse_unsafe_home_and_state_cwds_with_exact_reasons() {
    let temp = tempfile::tempdir().unwrap();
    let machine_home = temp.path().join("home-with-notes");
    let margins_home = machine_home.join(".margins");
    fs::create_dir_all(machine_home.join(".obsidian")).unwrap();
    fs::create_dir_all(&margins_home).unwrap();
    let workspace_state_root = margins_home.join("workspaces");
    fs::create_dir_all(&workspace_state_root).unwrap();
    fs::write(
        machine_home.join("evidence.md"),
        "# Real-looking note evidence must never imply a Workspace.",
    )
    .unwrap();

    let commands: &[&[&str]] = &[
        &["workspace", "status"],
        &["source", "list"],
        &["source", "remove", "reference"],
        &[
            "source",
            "add",
            "notes",
            "--name",
            "reference",
            "--role",
            "reference",
            "--path",
            ".",
        ],
        &["integrations", "status", "--json"],
        &["recall", "relationship context"],
        &["init"],
    ];
    for (cwd, reason, id) in [
        (
            &machine_home,
            "current directory is the user home directory",
            "home-with-notes",
        ),
        (
            &margins_home,
            "current directory is Margins configuration or state storage",
            "margins",
        ),
        (
            &workspace_state_root,
            "current directory is Margins configuration or state storage",
            "workspaces",
        ),
    ] {
        for args in commands {
            let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
                .args(*args)
                .current_dir(cwd)
                .env_clear()
                .env("HOME", &machine_home)
                .env("MARGINS_HOME", &margins_home)
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "command unexpectedly inferred Workspace from {}: {args:?}",
                cwd.display()
            );
            let stderr = String::from_utf8(output.stderr).unwrap();
            let explicit_source_mutation =
                args.first() == Some(&"source") && matches!(args.get(1), Some(&"add" | &"remove"));
            // Only `init` establishes a Workspace for the cwd, so only it
            // reaches the unsafe-home refusal; read-like commands never try.
            let establishes = args.first() == Some(&"init");
            let message = if explicit_source_mutation {
                "this mutation requires literal --workspace <id>".to_string()
            } else if establishes {
                format!(
                    "cannot create implicit workspace: {reason}; use explicit setup instead: margins workspace new {id} --home {}",
                    cwd.canonicalize().unwrap().display()
                )
            } else {
                margins_cli::commands::workspace::NO_WORKSPACE_MESSAGE.to_string()
            };
            let code = if establishes {
                "command_failed"
            } else {
                "workspace_required"
            };
            let refusal = stderr.lines().last().unwrap_or("");
            if args.first() == Some(&"integrations") && args.contains(&"--json") {
                let error: serde_json::Value = serde_json::from_str(refusal).unwrap();
                assert_eq!(error["schema_version"], "margins.error.v1");
                assert_eq!(error["error"]["code"], code);
                assert_eq!(error["error"]["message"], message);
                continue;
            }
            let expected = format!(
                "<margins_error code=\"{code}\">{}</margins_error>\n",
                margins_cli::output::xml_escape_text(&message)
            );
            assert_eq!(
                refusal,
                expected.trim_end(),
                "unexpected refusal for {args:?} from {}",
                cwd.display()
            );
        }
    }
    assert_eq!(fs::read_dir(&workspace_state_root).unwrap().count(), 0);
    assert!(!margins_home.join("ledger.db").exists());
    assert!(!margins_home.join("index.db").exists());
}

#[test]
#[cfg(feature = "recall")]
fn integrations_reconcile_requires_literal_workspace_selector() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("welcome.md"), "# Welcome\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "integrations",
            "reconcile",
            "--if-revision",
            "0000000000000000000000000000000000000000",
            "--request-id",
            "needs-workspace",
            "--json",
        ])
        .current_dir(&notes)
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", temp.path().join("margins-home"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let error: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(error["schema_version"], "margins.error.v1");
    assert_eq!(error["error"]["code"], "usage");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("integrations reconcile requires literal --workspace"));
}

#[test]
#[cfg(feature = "recall")]
fn read_like_commands_never_create_a_workspace_and_only_init_establishes_one() {
    let temp = tempfile::Builder::new()
        .prefix("implicit-workspace-cli-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let machine_home = temp.path().join("machine-home");
    let margins_home = temp.path().join("state");
    // A folder with no Workspace, such as a code checkout: recall there must
    // not quietly make it a Workspace whose notes land in the repo.
    let notes = temp.path().join("Fresh Notes");
    fs::create_dir_all(&machine_home).unwrap();
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("welcome.md"), "# Welcome\nA real note.").unwrap();
    let enzyme_home = temp.path().join("fake-enzyme-home");
    let config_root = temp.path().join("fake-config");
    let harness_temp = temp.path().join("fake-temp");
    for root in [&enzyme_home, &config_root, &harness_temp] {
        fs::create_dir_all(root).unwrap();
    }
    let deny_roots = serde_json::json!({
        "home_dir": machine_home,
        "enzyme_home": enzyme_home,
        "config_state_roots": [
            config_root.join("margins"),
            config_root.join("enzyme"),
        ],
        "temp_roots": [harness_temp],
    })
    .to_string();
    let canonical_notes = notes.canonicalize().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_margins-private"))
            .args(args)
            .current_dir(&notes)
            .env_clear()
            .env("HOME", &machine_home)
            .env("MARGINS_HOME", &margins_home)
            .env(
                margins_workflows::workspace::IMPLICIT_WORKSPACE_DENY_ROOTS_ENV,
                &deny_roots,
            )
            .output()
            .unwrap()
    };

    for args in [
        &["recall", "a real note"][..],
        &["recall", "a real note", "--json"],
        &["workspace", "status"],
        &["workspace", "status", "--json"],
        &["source", "list", "--json"],
        &["sync", "--json"],
        &["integrations", "status", "--json"],
    ] {
        let output = run(args);
        assert!(!output.status.success(), "{args:?} succeeded without a Workspace");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("workspace_required"), "{args:?}: {stderr}");
        assert!(stderr.contains("did not create one"), "{args:?}: {stderr}");
        assert!(stderr.contains("margins workspace new"), "{args:?}: {stderr}");
        assert!(!stderr.contains("Created workspace"), "{args:?}: {stderr}");
        assert!(
            !margins_home.join("configs").exists() && !margins_home.join("workspaces").exists(),
            "{args:?} created Workspace state"
        );
    }

    // `init` is the one command that establishes the cwd as a Workspace.
    let init = run(&["init"]);
    assert!(
        String::from_utf8_lossy(&init.stderr).contains(&format!(
            "Created workspace fresh-notes with home {}",
            canonical_notes.display()
        )),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config_path = margins_home.join("configs/fresh-notes.enzyme");
    assert!(config_path.is_file());
    let config = margins_workflows::workspace::resolve_at(&margins_home, "fresh-notes")
        .unwrap()
        .config;
    assert!(matches!(
        &config.bindings["home"],
        margins_workflows::workspace::WorkspaceBinding::NativeMarkdown { path, .. }
            if path == &canonical_notes
    ));

    // Afterwards, read-like commands find it from the cwd and create nothing.
    let status = run(&["workspace", "status", "--json"]);
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    assert!(status.stderr.is_empty());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["id"], "fresh-notes");
    assert_eq!(
        status["build"]["commit"],
        margins_cli::build_info::get().commit
    );
    assert_eq!(status["source_refresh_staleness"], serde_json::json!({}));
    assert_eq!(
        fs::read_dir(margins_home.join("workspaces"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
#[cfg(feature = "recall")]
fn workspace_status_reports_engine_source_refresh_staleness() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    let mut workspace =
        margins_workflows::workspace::create_workspace(&margins_home, "freshness", None, &notes)
            .unwrap();
    let selector = margins_workflows::workspace::GmailCollectionSelector::default_declaration();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "google-mail",
        margins_workflows::workspace::WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: selector.clone(),
        },
    )
    .unwrap();
    let store =
        margins_workflows::integrations::IntegrationsStore::open(&workspace.state_dir).unwrap();
    let ctx = margins_workflows::integrations::ConnectorCtx {
        vault_root: workspace.state_dir.clone(),
        connector_id: "email".to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &ctx,
            Vec::new(),
            Vec::new(),
            &selector.materialization_fingerprint().unwrap(),
        )
        .unwrap();
    store
        .update_health(
            &ctx,
            margins_workflows::integrations::HealthStatus::Fresh,
            None,
        )
        .unwrap();
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);
    let init = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "freshness", "init"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    // The ledger changes after the engine last refreshed its SQLite source.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    store
        .replace_email_thread_snapshot_with_materialization_fingerprint(
            &ctx,
            vec![margins_workflows::integrations::ThreadEvidence {
                thread_id: "late".into(),
                occurred_from: chrono::Utc::now(),
                occurred_to: chrono::Utc::now(),
                body_text: "A thread synced after the last index refresh.".into(),
                href: None,
            }],
            Vec::new(),
            &selector.materialization_fingerprint().unwrap(),
        )
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "freshness", "workspace", "status", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mail = &status["source_refresh_staleness"]["google-mail"];
    assert!(mail["last_refresh_ms"].as_i64().is_some_and(|ms| ms > 0), "{status}");
    assert_eq!(mail["stale"], true, "{status}");
    assert_eq!(mail["stale_reason"], "source_modified_after_refresh", "{status}");

    margins_workflows::workspace::remove_source(&mut workspace, "google-mail").unwrap();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "renamed-mail",
        margins_workflows::workspace::WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: margins_workflows::workspace::GmailCollectionSelector {
                query: "label:important".to_string(),
                backfill_days: 30,
            },
        },
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "freshness", "workspace", "status", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["source_refresh_staleness"]["renamed-mail"]["stale_reason"],
        "refresh_required"
    );

    store
        .record_failed_reconcile(&ctx, "simulated Gmail refresh failure")
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "freshness", "workspace", "status", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["source_refresh_staleness"]["renamed-mail"]["stale_reason"], "refresh_failed",
        "connector errors outrank a desired-state selector mismatch"
    );

    store
        .mark_google_connection_needs_auth("owner@example.com")
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "freshness", "workspace", "status", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("MARGINS_HOME", &margins_home)
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["source_refresh_staleness"]["renamed-mail"]["stale_reason"],
        "credentials_unavailable",
        "credential loss outranks a desired-state selector mismatch"
    );
}

#[test]
#[cfg(feature = "recall")]
fn connection_commands_do_not_implicitly_create_a_workspace() {
    let temp = tempfile::Builder::new()
        .prefix("machine-connection-cli-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let machine_home = temp.path().join("machine-home");
    let margins_home = temp.path().join("state");
    let notes = temp.path().join("Eligible Notes");
    fs::create_dir_all(&machine_home).unwrap();
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("welcome.md"), "# Welcome\nA real note.").unwrap();

    let connect = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["connect", "google", "--json"])
        .current_dir(&notes)
        .env_clear()
        .env("HOME", &machine_home)
        .env("MARGINS_HOME", &margins_home)
        .env(
            "MARGINS_GOOGLE_OAUTH_CLIENT_FILE",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/public/margins-cli/tests/fixtures/google-oauth-client.json"
            ),
        )
        .env("MARGINS_CONNECT_NO_BROWSER", "1")
        // A failed suppression guard must still be unable to find open/xdg-open.
        .env("PATH", temp.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&connect.stderr)
            .contains("Browser launch suppressed by MARGINS_CONNECT_NO_BROWSER=1."),
        "{}",
        String::from_utf8_lossy(&connect.stderr)
    );
    assert!(
        !margins_home.exists(),
        "connect must not create Workspace or machine state"
    );

    let disconnect = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "disconnect",
            "google",
            "--account",
            "owner@example.com",
            "--json",
        ])
        .current_dir(&notes)
        .env_clear()
        .env("HOME", &machine_home)
        .env("MARGINS_HOME", &margins_home)
        .output()
        .unwrap();
    assert!(
        disconnect.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnect.stderr)
    );
    let disconnected: serde_json::Value = serde_json::from_slice(&disconnect.stdout).unwrap();
    assert_eq!(disconnected["scope"], "machine");
    assert_eq!(disconnected["account"], "owner@example.com");
    assert_eq!(disconnected["forgotten"], true);
    assert_eq!(disconnected["retained_workspaces"], serde_json::json!([]));
    assert!(!margins_home.join("workspaces").exists());

    let bare_disconnect = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["disconnect", "google", "--json"])
        .current_dir(&notes)
        .env_clear()
        .env("HOME", &machine_home)
        .env("MARGINS_HOME", &margins_home)
        .output()
        .unwrap();
    assert!(!bare_disconnect.status.success());
    assert!(bare_disconnect.stdout.is_empty());
    assert!(!margins_home.join("workspaces").exists());
}

#[test]
#[cfg(feature = "recall")]
fn init_fails_closed_without_a_usable_generator() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    for index in 0..5 {
        fs::write(
            notes.join(format!("note-{index}.md")),
            format!("# Fixture {index}\nPortable catalyst status evidence."),
        )
        .unwrap();
    }
    margins_workflows::workspace::create_workspace(&margins_home, "init-status", None, &notes)
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "init-status", "init"])
        .env_clear()
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Building or refreshing recall index and catalysts…\n<margins_error code=\"command_failed\">indexing vault: Recall unavailable: no usable generator is configured. Run `margins setup`.</margins_error>\n"
    );
}

#[test]
#[cfg(feature = "recall")]
fn preset_plan_needs_an_existing_workspace_and_creates_none() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();

    for args in [
        &["workspace", "plan", "--preset", "margins-meetings", "--json"][..],
        &["--workspace", "practice", "workspace", "plan", "--preset", "margins-meetings", "--json"][..],
    ] {
        let plan = Command::new(env!("CARGO_BIN_EXE_margins-private"))
            .args(args)
            .current_dir(&notes)
            .env_clear()
            .env("HOME", temp.path())
            .env("MARGINS_HOME", &margins_home)
            .output()
            .unwrap();
        assert!(
            !plan.status.success(),
            "preset plan unexpectedly succeeded: {}",
            String::from_utf8_lossy(&plan.stdout)
        );
        assert!(plan.stdout.is_empty());
    }
    assert!(!margins_home.join("workspaces").exists());
    assert!(!margins_home.join("configs").exists());
}

#[test]
#[cfg(feature = "recall")]
fn retention_apply_materialization_refreshes_official_recall_index() {
    let temp = tempfile::tempdir().unwrap();
    let margins_home = temp.path().join("margins-home");
    let preprocessor_cache = temp.path().join("coreml-preprocessor-cache");
    // Mac builds exercise the CoreML fallback here; its diagnostic belongs in
    // the CLI log, leaving the JSON error as the only stderr output.
    let blocked_preprocessor_cache = temp.path().join("blocked-preprocessor-cache");
    fs::write(&blocked_preprocessor_cache, b"force fallback diagnostics").unwrap();
    let notes = temp.path().join("notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("home.md"), "# Home\n\nRetention fixture.").unwrap();
    let mut workspace = margins_workflows::workspace::create_workspace(
        &margins_home,
        "retention-cli",
        None,
        &notes,
    )
    .unwrap();
    margins_workflows::workspace::add_source(
        &mut workspace,
        "mail",
        margins_workflows::workspace::WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: margins_workflows::workspace::GmailCollectionSelector::default_declaration(),
        },
    )
    .unwrap();
    let store =
        margins_workflows::integrations::IntegrationsStore::open(&workspace.state_dir).unwrap();
    let ctx = margins_workflows::integrations::ConnectorCtx {
        vault_root: workspace.state_dir.clone(),
        connector_id: "email".to_string(),
        account: "owner@example.com".to_string(),
        command_path: None,
    };
    let occurred = chrono::DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    store
        .replace_email_thread_snapshot(
            &ctx,
            vec![margins_workflows::integrations::ThreadEvidence {
                thread_id: "purge-me".into(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "purge me from recall".into(),
                href: None,
            }],
            Vec::new(),
        )
        .unwrap();
    std::env::set_var("MARGINS_HOME", &margins_home);
    std::env::remove_var("ENZYME_HOME");
    let _generator = fixture_generator::FixtureGenerator::start(&margins_home);
    let initial_index = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args(["--workspace", "retention-cli", "init"])
        .env_clear()
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env("MARGINS_COREML_PREPROCESSOR_CACHE_DIR", &preprocessor_cache)
        .output()
        .unwrap();
    assert!(
        initial_index.status.success(),
        "{}",
        String::from_utf8_lossy(&initial_index.stderr)
    );
    let namespace = "mail";
    let before: i64 = rusqlite::Connection::open(workspace.recall_path())
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:{namespace}/%'"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, 1);

    let status = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "--workspace",
            "retention-cli",
            "workspace",
            "status",
            "--json",
        ])
        .env_clear()
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env("MARGINS_COREML_PREPROCESSOR_CACHE_DIR", &preprocessor_cache)
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_json = serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap();
    let indexed_documents: i64 = rusqlite::Connection::open(workspace.recall_path())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM docs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status_json["recall"]["mode"], "indexed");
    assert_eq!(status_json["recall"]["documents"], indexed_documents);
    let revision = status_json["revision"].as_str().unwrap().to_string();

    let preview = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "--workspace",
            "retention-cli",
            "retention",
            "preview",
            "--connector",
            "email",
            "--account",
            "owner@example.com",
            "--scope",
            "materialization",
            "--json",
        ])
        .env_clear()
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env("MARGINS_COREML_PREPROCESSOR_CACHE_DIR", &preprocessor_cache)
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let plan_path = temp.path().join("retention-plan.json");
    fs::write(&plan_path, preview.stdout).unwrap();

    let index_path = workspace.recall_path();
    let saved_index_path = workspace.state_dir.join("index-before-retention.db");
    fs::rename(&index_path, &saved_index_path).unwrap();
    fs::create_dir(&index_path).unwrap();

    let failed_apply = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "--workspace",
            "retention-cli",
            "retention",
            "apply",
            "--plan",
            plan_path.to_str().unwrap(),
            "--if-revision",
            &revision,
            "--request-id",
            "official-retention-1",
            "--json",
        ])
        .env_clear()
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env(
            "MARGINS_COREML_PREPROCESSOR_CACHE_DIR",
            &blocked_preprocessor_cache,
        )
        .output()
        .unwrap();
    assert!(!failed_apply.status.success());
    assert!(failed_apply.stdout.is_empty());
    let refresh_error: serde_json::Value = serde_json::from_slice(&failed_apply.stderr)
        .unwrap_or_else(|error| {
            panic!(
                "expected one typed JSON refresh error, got {:?}: {error}",
                String::from_utf8_lossy(&failed_apply.stderr)
            )
        });
    assert_eq!(refresh_error["schema_version"], "margins.error.v1");
    assert_eq!(
        refresh_error["error"]["code"],
        "retention_index_refresh_failed"
    );
    assert_eq!(refresh_error["error"]["retryable"], true);
    assert_eq!(refresh_error["error"]["details"]["ledger_committed"], true);
    assert_eq!(refresh_error["error"]["details"]["replay_safe"], true);
    let ledger = rusqlite::Connection::open(workspace.ledger_path()).unwrap();
    assert_eq!(
        ledger
            .query_row("SELECT COUNT(*) FROM thread_evidence", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        ledger
            .query_row("SELECT COUNT(*) FROM purge_receipts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(ledger);

    fs::remove_dir(&index_path).unwrap();
    fs::rename(&saved_index_path, &index_path).unwrap();
    let apply = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .args([
            "--workspace",
            "retention-cli",
            "retention",
            "apply",
            "--plan",
            plan_path.to_str().unwrap(),
            "--if-revision",
            &revision,
            "--request-id",
            "official-retention-1",
            "--json",
        ])
        .env_clear()
        .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
        .env("MARGINS_HOME", &margins_home)
        .env("HOME", temp.path())
        .env("MARGINS_COREML_PREPROCESSOR_CACHE_DIR", &preprocessor_cache)
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(receipt["schema_version"], "margins.retention.apply.v1");
    assert_eq!(receipt["index_refresh_required"], true);
    assert_eq!(receipt["replayed"], true);
    let after: i64 = rusqlite::Connection::open(workspace.recall_path())
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM docs WHERE source_ref LIKE 'sqlite:{namespace}/%'"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, 0,
        "official composition refresh removes purged sqlite-source docs"
    );
}

#[test]
#[cfg(feature = "audio-capture")]
fn packaged_binary_reports_private_native_composition() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_margins-private"))
        .arg("__release-smoke")
        .env_remove("MARGINS_PROJECT")
        .env("HOME", temp.path())
        .env("MARGINS_HOME", temp.path().join("margins-home"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let contract: serde_json::Value = serde_json::from_str(stdout.trim_end()).unwrap();
    assert_eq!(contract["schema"], 1);
    assert_eq!(contract["product"], "margins");
    assert_eq!(contract["composition"], "official");
    assert_eq!(contract["official"], true);
    assert!(contract["oauth_client"].is_string());
    assert_eq!(contract["capture_available"], true);
    assert_eq!(contract["capture_provider"], "native-recorder");
    assert_eq!(contract["tui_available"], true);
    for capability in ["available", "indexing", "lookup"] {
        assert_eq!(
            contract["recall"][capability],
            cfg!(feature = "recall"),
            "recall.{capability} must match the compiled feature matrix"
        );
    }
    assert_eq!(
        contract["recall"]["local_model"],
        cfg!(feature = "recall-local-model")
    );
}

/// Every file, directory, and symlink under `root` with its bytes, mode, and
/// modification time.
fn home_snapshot(root: &Path) -> std::collections::BTreeMap<String, (String, Vec<u8>, u32, u128)> {
    fn walk(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<String, (String, Vec<u8>, u32, u128)>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let metadata = fs::symlink_metadata(&path).unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions());
            #[cfg(not(unix))]
            let mode = 0;
            let mtime = metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                let entry = ("symlink".into(), target.display().to_string().into_bytes(), mode, mtime);
                out.insert(relative, entry);
            } else if metadata.is_dir() {
                out.insert(relative, ("dir".into(), Vec::new(), mode, mtime));
                walk(root, &path, out);
            } else {
                out.insert(relative, ("file".into(), fs::read(&path).unwrap(), mode, mtime));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Read-only and diagnostic commands never migrate or write user data, even
/// against a home that every migration would rewrite: a legacy machine
/// `config.toml`, a retired Workspace `config.toml`, and Workspace indexes
/// under their pre-engine name. The home must stay byte-identical, mtimes
/// included.
#[test]
fn read_only_commands_leave_a_legacy_home_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let margins_home = home.join(".margins");
    let notes = home.join("notes");
    let current_notes = home.join("current-notes");
    fs::create_dir_all(notes.join("Meetings")).unwrap();
    fs::create_dir_all(notes.join("People")).unwrap();
    fs::create_dir_all(&current_notes).unwrap();
    fs::write(notes.join("note.md"), "# Note\n").unwrap();
    fs::write(current_notes.join("note.md"), "# Current\n").unwrap();
    // A Workspace already declared as a program, beside the legacy ones.
    let current = margins_workflows::workspace::create_workspace(
        &margins_home,
        "current",
        None,
        &current_notes,
    )
    .unwrap();
    fs::write(current.state_dir.join("index.db"), b"not a database").unwrap();
    fs::write(
        margins_home.join("config.toml"),
        "# machine preferences\n[llm]\nmode = \"local\"\n\n[workspace]\ndefault = \"legacy\"\n",
    )
    .unwrap();
    let state = margins_home.join("workspaces/legacy");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("config.toml"),
        format!(
            "id = \"legacy\"\nname = \"Legacy\"\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n",
            notes.canonicalize().unwrap()
        ),
    )
    .unwrap();
    fs::write(state.join("index.db"), b"not a database").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(margins_home.join("config.toml"), fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    let before = home_snapshot(temp.path());

    let mut commands: Vec<Vec<&str>> = vec![
        vec!["__release-smoke"],
        vec!["capabilities"],
        vec!["--version"],
        vec!["--help"],
        vec!["guide", "workspace-setup"],
        vec!["guide", "onboarding"],
        vec!["workspace", "list", "--json"],
        vec!["workspace", "default", "--json"],
        vec!["connect", "status", "--json"],
    ];
    for id in ["legacy", "current"] {
        for command in [
            &["workspace", "show"][..],
            &["workspace", "show", "--text", "--json"],
            &["workspace", "status", "--json"],
            &["workspace", "status"],
            &["source", "list", "--json"],
            &["workspace", "destination", "--json"],
        ] {
            commands.push([&["--workspace", id][..], command].concat());
        }
        #[cfg(feature = "recall")]
        commands.push(vec![
            "--workspace", id, "workspace", "plan", "--preset", "margins-meetings", "--json",
        ]);
    }
    // Without a selector, the machine default and the cwd's Workspace.
    commands.push(vec!["workspace", "destination", "--json"]);
    commands.push(vec!["workspace", "status", "--json"]);
    commands.push(vec!["source", "list", "--json"]);
    for args in &commands {
        let mut command = Command::new(env!("CARGO_BIN_EXE_margins-private"));
        command
            .args(args)
            .current_dir(&notes)
            .env_clear()
            .env("HOME", &home)
            .env("MARGINS_HOME", &margins_home)
            .env("ENZYME_HOME", temp.path().join("poisoned-enzyme-home"));
        #[cfg(feature = "recall")]
        command.env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin());
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let after = home_snapshot(temp.path());
        assert!(
            before == after,
            "{args:?} changed the home: added {:?}, removed {:?}, modified {:?}",
            after.keys().filter(|key| !before.contains_key(*key)).collect::<Vec<_>>(),
            before.keys().filter(|key| !after.contains_key(*key)).collect::<Vec<_>>(),
            before
                .iter()
                .filter(|(key, value)| after.get(*key).is_some_and(|other| other != *value))
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
        );
        let json = || serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
        match args.as_slice() {
            ["workspace", "list", "--json"] => {
                let listed = json();
                assert_eq!(listed["default_workspace"], "legacy", "{listed}");
                assert_eq!(listed["workspaces"][1]["id"], "legacy", "{listed}");
                assert_eq!(listed["workspaces"][1]["name"], "Legacy", "{listed}");
            }
            ["capabilities"] => assert_eq!(json()["catalyst"]["mode"], "local"),
            ["--workspace", "legacy", "workspace", "status", "--json"] => {
                assert_eq!(json()["name"], "Legacy")
            }
            ["--workspace", "legacy", "workspace", "plan", ..] => {
                let plan = json();
                assert_eq!(plan["preset"]["note_folder"], "Meetings", "{plan}");
                assert!(!plan["actions"].as_array().unwrap().is_empty(), "{plan}");
            }
            _ => {}
        }
    }

    // A preview of the retired Workspace applies after the migration it
    // implies: the plan's base is the program the migration writes.
    #[cfg(feature = "recall")]
    {
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_margins-private"))
                .args(args)
                .current_dir(&notes)
                .env_clear()
                .env("HOME", &home)
                .env("MARGINS_HOME", &margins_home)
                .env("MARGINS_ENZYME_BIN", enzyme_bin::enzyme_bin())
                .output()
                .unwrap()
        };
        let plan = run(&["--workspace", "legacy", "workspace", "plan", "--preset", "margins-meetings", "--json"]);
        assert!(plan.status.success());
        let plan_path = temp.path().join("plan.json");
        fs::write(&plan_path, &plan.stdout).unwrap();
        let apply = run(&[
            "--workspace", "legacy", "workspace", "apply", "--plan", plan_path.to_str().unwrap(), "--json",
        ]);
        assert!(apply.status.success(), "{}", String::from_utf8_lossy(&apply.stderr));
        let program = fs::read_to_string(margins_home.join("configs/legacy.enzyme")).unwrap();
        assert!(program.contains("remember in folder \"Meetings\""), "{program}");
        assert!(margins_home.join("config.toml.migrated").is_file());
        assert!(state.join("config.toml.migrated").is_file());
    }
}
