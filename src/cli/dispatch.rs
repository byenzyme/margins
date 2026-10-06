/// Entry point used by the production binary. Only interactive capture is
/// intercepted; every other command retains the public parser/dispatcher.
pub fn main_entry<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    main_entry_with(args, &NativeInteractiveSession)
}

pub fn main_entry_from_env() -> i32 {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    margins_media::providers::coreml::set_coreml_diagnostic_sink(|message| {
        crate::cli_log::event("coreml_preprocessor_fallback", message);
    });
    main_entry(std::env::args_os())
}

const RELEASE_SMOKE_COMMAND: &str = "__release-smoke";

fn official_capabilities_json() -> serde_json::Value {
    let mut distillation_inputs = vec!["transcript", "memo"];
    if cfg!(any(feature = "coreml-asr", feature = "parakeet-asr")) {
        distillation_inputs.push("audio");
    }
    let catalyst = margins_workflows::workspace::margins_home()
        .map(|home| crate::hosted_credentials::redacted_status(&home))
        .unwrap_or_else(|_| {
            serde_json::json!({
                "mode": "none",
                "usable": false,
                "reason": "config_unreadable",
                "expiry": {"state": "unknown", "bucket": "unknown"},
            })
        });
    serde_json::json!({
        "schema": 1,
        "product": "margins",
        "composition": "official",
        "official": true,
        "autonomous": true,
        "oauth_client": crate::google_oauth_client::status(),
        "build": margins_cli::build_info::get(),
        "recall": {
            "available": cfg!(feature = "recall"),
            "scan": cfg!(feature = "recall"),
            "indexing": cfg!(feature = "recall"),
            "lookup": cfg!(feature = "recall"),
            "local_model": cfg!(feature = "recall-local-model"),
        },
        "catalyst": catalyst,
        "capture": {
            "available": cfg!(feature = "audio-capture"),
            "provider": if cfg!(feature = "audio-capture") {
                "native-recorder"
            } else {
                "unavailable"
            },
        },
        "workspace": {
            "setup": true,
        },
        "audio_import": {
            "available": cfg!(any(feature = "coreml-asr", feature = "parakeet-asr")),
            "decoder": true,
        },
        "note_skill": {
            "available": true,
        },
        // Additive public workflow description used by the portable note skill.
        "distillation": {
            "available": true,
            "workflow": "connected_note",
            "inputs": distillation_inputs,
        },
        "tui": {
            "available": true,
        },
        // Backward-compatible fields consumed by the release smoke script.
        "capture_available": cfg!(feature = "audio-capture"),
        "capture_provider": if cfg!(feature = "audio-capture") {
            "native-recorder"
        } else {
            "unavailable"
        },
        "tui_available": true,
    })
}

/// Side-effect-free packaged-binary probe used by the private release pipeline.
///
/// Keeping this command in the private composition (rather than the public CLI
/// parser) makes a public-only binary with its no-capture service unable to
/// satisfy the release contract.
fn release_smoke(args: &[OsString]) -> Option<i32> {
    let [_, command] = args else {
        return None;
    };
    if command != RELEASE_SMOKE_COMMAND {
        return None;
    }

    println!("{}", official_capabilities_json());
    Some(0)
}

fn main_entry_with<I, T>(args: I, interactive: &dyn InteractiveSession) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    if let Err(error) = crate::initialize_sqlite_runtime() {
        return report_error(&format!("SQLite runtime initialization failed: {error:#}"));
    }
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if let Some(code) = release_smoke(&args) {
        return code;
    }
    #[cfg(feature = "audio-capture")]
    if let Some(code) = native_bridge::maybe_main(&args) {
        return code;
    }
    let (workspace_selector, workspace_args) =
        match margins_cli::args::strip_workspace_arg(args.clone()) {
            Ok(value) => value,
            Err(error) => return report_error(&error),
        };
    let (project_selector, parse_args) = match margins_cli::args::strip_project_arg(workspace_args)
    {
        Ok(value) => value,
        Err(error) => return report_error(&error),
    };
    let parsed = match Args::try_parse_from(parse_args) {
        Ok(value) => value,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    let remote_selected = !parsed.local
        && (parsed.remote.is_some()
            || std::env::var("MARGINS_REMOTE")
                .ok()
                .is_some_and(|value| !value.trim().is_empty()));
    if remote_selected {
        #[cfg(feature = "audio-capture")]
        if matches!(
            parsed.command,
            Some(Command::New { .. } | Command::Attach { .. })
        ) {
            let env_remote = std::env::var("MARGINS_REMOTE").ok();
            let env_workspace = std::env::var("MARGINS_WORKSPACE")
                .ok()
                .filter(|value| !value.trim().is_empty());
            let remote = parsed
                .remote
                .as_deref()
                .or(env_remote.as_deref())
                .map(str::to_string)
                .expect("remote selection was already established");
            let workspace = workspace_selector
                .as_deref()
                .or(env_workspace.as_deref())
                .map(str::to_string);
            let Some(workspace) = workspace else {
                return report_error(
                    "remote capture requires --workspace <id> or MARGINS_WORKSPACE",
                );
            };
            return match run_remote_native_capture(
                &remote,
                &workspace,
                &parsed.command,
                None,
                None,
                None,
                None,
                false,
            ) {
                Ok(()) => 0,
                Err(error) => {
                    let message = format!("{error:#}");
                    let _ = report_error(&message);
                    if message.contains("upload pending in transfer") {
                        75
                    } else {
                        1
                    }
                }
            };
        }
        return margins_cli::main_entry(args);
    }

    // Intercept Setup before the interactive dispatch.
    if let Some(Command::Setup {
        only,
        skip,
        local_model,
    }) = &parsed.command
    {
        return run_setup(workspace_selector.as_deref(), only, *skip, *local_model);
    }

    if matches!(parsed.command, Some(Command::Capabilities)) {
        println!("{}", official_capabilities_json());
        return 0;
    }

    #[cfg(feature = "recall")]
    if matches!(
        parsed.command,
        Some(Command::Workspace {
            command: margins_cli::args::WorkspaceCommand::Status { json: true }
        })
    ) {
        return run_workspace_status(workspace_selector.as_deref());
    }

    if let Some(Command::Connect {
        command:
            margins_cli::args::ConnectCommand::Google {
                account,
                headless,
                json,
            },
    }) = &parsed.command
    {
        let mut stdout = std::io::stdout().lock();
        let credential = match crate::google_oauth_client::load() {
            Ok(credential) => credential,
            Err(error) => return report_error(&error.to_string()),
        };
        let home = match margins_workflows::workspace::margins_home() {
            Ok(home) => home,
            Err(error) => return report_error(&error.to_string()),
        };
        let result = margins_cli::commands::connect::google(
            &home,
            account.as_deref(),
            *headless,
            *json,
            credential.as_deref(),
            &mut stdout,
            &mut std::io::stderr().lock(),
        );
        return match result {
            Ok(()) => 0,
            Err(error) => {
                let message = margins_user_message(error.message());
                if *json {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "error": {"code": error.code(), "message": message}
                        })
                    );
                } else {
                    eprintln!(
                        "<margins_error code=\"{}\">{}</margins_error>",
                        error.code(),
                        message
                    );
                }
                error.exit_code()
            }
        };
    }

    if let Some(Command::Integrations {
        command:
            margins_cli::args::IntegrationsCommand::Reconcile {
                connector,
                account,
                if_revision,
                request_id,
                json,
            },
    }) = &parsed.command
    {
        debug_assert!(*json);
        if workspace_selector
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return report_json_cli_error(margins_cli::CliError::usage(
                "integrations reconcile requires literal --workspace <id>",
            ));
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let workspace = match margins_cli::commands::workspace::resolve(
            workspace_selector.as_deref(),
            &cwd,
            &mut stderr,
        ) {
            Ok(workspace) => workspace,
            Err(error) => return report_json_cli_error(error),
        };
        let fixture_base = std::env::var("MARGINS_GOOGLE_NATIVE_E2E_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let result = if let Some(base) = fixture_base {
            let native = margins_workflows::integrations::NativeGoogleClient::with_bases(
                None,
                Some("fixture-access-token".to_string()),
                &base,
                &base,
                &base,
                &base,
                &base,
            );
            margins_cli::commands::integrations::reconcile_with_native_google_client(
                &workspace.state_dir,
                connector.as_deref(),
                account.as_deref(),
                if_revision,
                request_id,
                &native,
                &mut stdout,
            )
        } else {
            let credential = match crate::google_oauth_client::load() {
                Ok(Some(credential)) => credential,
                Ok(None) => return report_json_cli_error(margins_cli::CliError::new(
                    "google_credential_unavailable",
                    "Supply MARGINS_GOOGLE_OAUTH_CLIENT_FILE or MARGINS_GOOGLE_OAUTH_CLIENT_JSON to use Google integration in a source build.",
                )),
                Err(error) => return report_json_cli_error(margins_cli::CliError::new(
                    "google_credential_unavailable",
                    error.to_string(),
                )),
            };
            margins_cli::commands::integrations::reconcile_with_google_credential(
                &workspace.state_dir,
                connector.as_deref(),
                account.as_deref(),
                if_revision,
                request_id,
                &credential,
                &mut stdout,
            )
        };
        let code = result
            .as_ref()
            .map_or_else(|error| error.exit_code(), |_| 0);
        #[cfg(feature = "recall")]
        if code == 0 && workspace.ledger_path().is_file() {
            if let Err(error) = crate::recall::refresh_workspace(&workspace) {
                return report_json_cli_error(
                    margins_cli::CliError::new(
                        "integration_index_refresh_failed",
                        margins_user_message(&format!(
                            "authoritative ledger mutation committed, but refreshing workspace recall failed: {error:#}"
                        )),
                    )
                    .with_details(serde_json::json!({
                        "ledger_committed": true,
                        "replay_safe": true,
                        "mutation_exit_code": code,
                    }))
                    .retryable(true),
                );
            }
        }
        let _ = io::stderr().write_all(&stderr);
        let _ = io::stdout().write_all(&stdout);
        return match result {
            Ok(()) => code,
            Err(error) => {
                if !error.is_reported() {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "error": {"code": error.code(), "message": margins_user_message(error.message())}
                        })
                    );
                }
                code
            }
        };
    }

    if let Some(Command::Note { print }) = &parsed.command {
        return match crate::note::run(*print) {
            Ok(()) => 0,
            Err(error) => report_error(&format!("{error:#}")),
        };
    }

    // The official binary owns vault indexing because it composes the private
    // recall engine. Establish first, then publish success only after the index
    // and catalysts have been refreshed.
    #[cfg(feature = "recall")]
    if matches!(parsed.command, Some(Command::Init)) {
        return run_init(workspace_selector.as_deref());
    }

    // Intercept Recall: the official binary composes the vendored associative
    // search engine, so it never falls through to the public degradation path.
    #[cfg(feature = "recall")]
    if let Some(Command::Recall { query, source }) = &parsed.command {
        return run_recall(workspace_selector.as_deref(), query, source.as_deref());
    }

    #[cfg(feature = "recall")]
    if let Some(Command::Sync { source, json }) = &parsed.command {
        return run_sync(workspace_selector.as_deref(), source.as_deref(), *json);
    }

    // Scan is filesystem-only discovery. It shares Margins' explicit config
    // path but never opens or creates the recall database.
    #[cfg(feature = "recall")]
    if matches!(&parsed.command, Some(Command::Scan)) {
        return run_scan(workspace_selector.as_deref());
    }

    #[cfg(feature = "recall")]
    if let Some(Command::Workspace {
        command: margins_cli::args::WorkspaceCommand::Compile { note_folder, .. },
    }) = &parsed.command
    {
        return run_workspace_compile(workspace_selector.as_deref(), note_folder.as_deref());
    }

    let interactive_command = match &parsed.command {
        None => Some((false, None, None, true)),
        Some(Command::New { title }) => Some((true, title.as_deref(), None, false)),
        Some(Command::Attach { session }) => Some((false, None, session.as_deref(), false)),
        _ => None,
    };
    let Some((create, title, selected, create_if_missing)) = interactive_command else {
        let retention_apply = matches!(
            &parsed.command,
            Some(Command::Retention {
                command: margins_cli::args::RetentionCommand::Apply { .. }
            })
        );
        let granola_import = matches!(
            &parsed.command,
            Some(Command::Import {
                command: margins_cli::args::ImportCommand::Granola { .. }
            })
        );
        // Hold public mutation output until successful ledger writes have had
        // any required recall refresh. This preserves typed errors while never
        // publishing a materialization purge as complete ahead of Enzyme's
        // generic source reconciliation.
        let buffer_mutation = retention_apply || granola_import;
        let (code, mutation_stdout, mutation_stderr) = if buffer_mutation {
            let services = margins_cli::standalone_services();
            let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let result = margins_cli::run(&services, &cwd, args.clone(), &mut stdout, &mut stderr);
            (
                result
                    .as_ref()
                    .map_or_else(|error| error.exit_code(), |_| 0),
                stdout,
                stderr,
            )
        } else {
            (margins_cli::main_entry(args), Vec::new(), Vec::new())
        };
        #[cfg(feature = "recall")]
        if granola_import && code == 0 {
            let workspace = match resolve_workspace(workspace_selector.as_deref()) {
                Ok(workspace) => workspace,
                Err(error) => return report_error(&error.to_string()),
            };
            if let Err(error) = crate::recall::refresh_workspace(&workspace) {
                return report_json_cli_error(
                    margins_cli::CliError::new(
                        "granola_index_refresh_failed",
                        margins_user_message(&format!(
                            "Granola notes were imported, but refreshing Margins recall failed: {error:#}"
                        )),
                    )
                    .with_details(serde_json::json!({
                        "notes_committed": true,
                        "replay_safe": false,
                    })),
                );
            }
        }
        #[cfg(feature = "recall")]
        if retention_apply && workspace_selector.is_some() {
            let retention_requires_refresh = retention_apply
                && serde_json::from_slice::<serde_json::Value>(&mutation_stdout)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("index_refresh_required")
                            .and_then(|v| v.as_bool())
                    })
                    == Some(true);
            let workspace = match resolve_workspace(workspace_selector.as_deref()) {
                Ok(workspace) => workspace,
                Err(error) => {
                    return report_json_cli_error(margins_cli::CliError::new(
                        "workspace_resolution_failed",
                        margins_user_message(&error.to_string()),
                    ))
                }
            };
            if retention_requires_refresh && workspace.ledger_path().is_file() {
                if let Err(error) = crate::recall::refresh_workspace(&workspace) {
                    return report_json_cli_error(
                        margins_cli::CliError::new(
                            "retention_index_refresh_failed",
                            margins_user_message(&format!(
                                "authoritative ledger mutation committed, but refreshing workspace recall failed: {error:#}"
                            )),
                        )
                        .with_details(serde_json::json!({
                            "ledger_committed": true,
                            "replay_safe": true,
                            "mutation_exit_code": code,
                        }))
                        .retryable(true),
                    );
                }
            }
        }
        if buffer_mutation {
            let _ = io::stdout().write_all(&mutation_stdout);
        }
        let _ = io::stderr().write_all(&mutation_stderr);
        return code;
    };

    let services = margins_cli::standalone_services();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let selected_workspace = workspace_selector.as_deref().or(env_workspace.as_deref());
    if selected_workspace.is_some() && project_selector.is_some() {
        return report_error("`--project` cannot be combined with an explicit Workspace selection");
    }
    let capture_root = if selected_workspace.is_some() {
        let workspace = match resolve_workspace(selected_workspace) {
            Ok(workspace) => workspace,
            Err(error) => return report_error(&error.to_string()),
        };
        match workspace.capture_store_dir() {
            Ok(path) => path,
            Err(error) => return report_error(&error.to_string()),
        }
    } else {
        match services
            .projects
            .resolve_vault(project_selector.as_deref(), &cwd)
        {
            Ok(project) => project.work_dir,
            Err(error) => return report_error(&error.to_string()),
        }
    };
    let create = if create_if_missing {
        let margins_dir = capture_root.join(".margins");
        let current = match services.sessions.current(&margins_dir) {
            Ok(current) => current,
            Err(error) => return report_error(&error.to_string()),
        };
        let current_exists = match current.as_deref().filter(|name| !name.trim().is_empty()) {
            Some(name) => match services.sessions.exists(&margins_dir, name) {
                Ok(exists) => exists,
                Err(error) => return report_error(&error.to_string()),
            },
            None => false,
        };
        bare_capture_creates(current.as_deref(), current_exists)
    } else {
        create
    };
    let result = if create {
        interactive.create(&capture_root, title)
    } else {
        interactive.attach(&capture_root, selected)
    };
    match result {
        Ok(()) => 0,
        Err(error) => report_error(&format!("{error:#}")),
    }
}

fn bare_capture_creates(current: Option<&str>, current_exists: bool) -> bool {
    current
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .is_none()
        || !current_exists
}
