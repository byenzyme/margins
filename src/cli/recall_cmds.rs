/// Resolve the vault git-style and run associative recall against it. People
/// get the readable catalyst tree; `--json` emits the margins.recall.v1
/// envelope for agents and scripts. Recall fails closed when catalysts cannot
/// serve the request; callers receive a non-zero command result and an
/// explicit hint.
#[cfg(feature = "recall")]
fn run_recall(
    workspace_selector: Option<&str>,
    query: &str,
    source: Option<&str>,
    json: bool,
) -> i32 {
    // Read-only: never creates a Workspace.
    let workspace = match read_only_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_cli_error(error),
    };
    match crate::recall::recall(&workspace, query, source) {
        Ok(output) => match crate::recall::render_recall_for_stdout(&output, !json) {
            Ok(rendered) => {
                print!("{rendered}");
                0
            }
            Err(error) => report_error(&format!("rendering recall: {error:#}")),
        },
        Err(error) => report_error(&format!("{error:#}")),
    }
}

#[cfg(feature = "recall")]
fn run_sync(workspace_selector: Option<&str>, source_filter: Option<&str>, json: bool) -> i32 {
    // Sync refreshes an existing Workspace; it never creates one.
    let workspace = match read_only_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) if json => return report_json_cli_error(error),
        Err(error) => return report_cli_error(error),
    };
    if let Some(source) = source_filter {
        if !workspace.config.bindings.contains_key(source) {
            return report_sync_error(
                json,
                "source_not_configured",
                &format!(
                    "workspace '{}' has no declared source '{}'",
                    workspace.config.id, source
                ),
            );
        }
    }
    let revision = match margins_workflows::workspace::workspace_revision(&workspace) {
        Ok(revision) => revision,
        Err(error) => return report_error(&format!("reading workspace revision: {error:#}")),
    };
    let request_id = generated_sync_request_id();
    let mut sources = local_sync_rows(&workspace, source_filter);
    let external_rows =
        match sync_external_sources(&workspace, source_filter, &revision, &request_id) {
            Ok(rows) => rows,
            Err(error) => {
                return report_sync_error(
                    json,
                    error.code(),
                    &margins_user_message(error.message()),
                )
            }
        };
    sources.extend(external_rows.into_iter().map(sync_connector_row_json));

    let recall = match crate::recall::refresh_workspace(&workspace) {
        Ok(status) => serde_json::json!({
            "ok": status.status != "catalysts_pending",
            "status": status.status,
            "reason": status.reason,
            "readiness": {
                "entities_curated": status.readiness.entities_curated,
                "catalysts_present": status.readiness.catalysts_present,
                "items_pending": status.readiness.items_pending,
                "reasons": status.readiness.reasons,
            },
        }),
        Err(error) => serde_json::json!({
            "ok": false,
            "status": "error",
            "reason": "index_refresh_failed",
            "error": margins_user_message(&format!("{error:#}")),
        }),
    };
    let ok = sources
        .iter()
        .all(|source| source.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        && recall.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    let envelope = serde_json::json!({
        "schema_version": "margins.sync.v1",
        "ok": ok,
        "request_id": request_id,
        "workspace": {
            "id": workspace.config.id,
            "path": workspace.state_dir,
        },
        "revision": {
            "before": revision,
            "after": revision,
        },
        "sources": sources,
        "recall": recall,
    });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string())
        );
    } else {
        println!("Sync workspace {}", workspace.config.id);
        for source in envelope
            .get("sources")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let binding = source
                .get("binding")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let status = source
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            println!("- {binding}: {status}");
        }
        let recall_status = envelope
            .get("recall")
            .and_then(|recall| recall.get("status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        println!("- recall: {recall_status}");
    }
    if ok {
        0
    } else {
        1
    }
}

#[cfg(feature = "recall")]
fn sync_external_sources(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    source_filter: Option<&str>,
    revision: &str,
    request_id: &str,
) -> Result<Vec<margins_cli::commands::integrations::SyncConnectorRow>, margins_cli::CliError> {
    if let Some(base) = std::env::var("MARGINS_GOOGLE_NATIVE_E2E_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let native = margins_workflows::integrations::NativeGoogleClient::with_bases(
            None,
            Some("fixture-access-token".to_string()),
            &base,
            &base,
            &base,
            &base,
            &base,
        );
        return margins_cli::commands::integrations::sync_declared_with_native_google_client(
            &workspace.state_dir,
            source_filter,
            revision,
            request_id,
            &native,
        );
    }
    let credential = crate::google_oauth_client::load().map_err(|error| {
        margins_cli::CliError::new("google_credential_unavailable", error.to_string())
    })?;
    margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        source_filter,
        revision,
        request_id,
        credential.as_deref(),
    )
}

#[cfg(feature = "recall")]
fn local_sync_rows(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    source_filter: Option<&str>,
) -> Vec<serde_json::Value> {
    workspace
        .config
        .bindings
        .iter()
        .filter(|(name, _)| source_filter.is_none_or(|filter| filter == name.as_str()))
        .filter_map(|(name, binding)| {
            let kind = match binding {
                margins_workflows::workspace::WorkspaceBinding::NativeMarkdown { .. } => "notes",
                margins_workflows::workspace::WorkspaceBinding::Captures { .. } => "captures",
                _ => return None,
            };
            Some(serde_json::json!({
                "binding": name,
                "connector_id": kind,
                "account": null,
                "ok": true,
                "status": "synced",
                "records_written": 0,
                "records_updated": 0,
                "records_unchanged": 0,
                "tombstones": 0,
            }))
        })
        .collect()
}

#[cfg(feature = "recall")]
fn sync_connector_row_json(
    row: margins_cli::commands::integrations::SyncConnectorRow,
) -> serde_json::Value {
    serde_json::to_value(row).unwrap_or_else(|error| {
        serde_json::json!({
            "binding": "unknown",
            "connector_id": "unknown",
            "account": "",
            "ok": false,
            "status": "error",
            "error": {
                "code": "sync_result_encoding_failed",
                "message": error.to_string(),
                "retryable": false,
            },
        })
    })
}

#[cfg(feature = "recall")]
fn report_sync_error(json: bool, code: &str, message: &str) -> i32 {
    if json {
        eprintln!(
            "{}",
            serde_json::json!({
                "schema_version": "margins.sync.v1",
                "ok": false,
                "error": {"code": code, "message": message}
            })
        );
        1
    } else {
        report_error(message)
    }
}

#[cfg(feature = "recall")]
fn generated_sync_request_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("sync-{millis}-{}", std::process::id())
}

/// `workspace plan --preset`: the engine fills the preset for the Workspace's
/// notes folder, Margins drops folder readings for folders that do not exist
/// and adds the rest to the current program, and the result is planned like any
/// desired program. The plan JSON also names the program and what the preset
/// kept and skipped.
#[cfg(feature = "recall")]
fn run_workspace_plan_preset(workspace_selector: Option<&str>, preset: &str, json: bool) -> i32 {
    let plan = match workspace_plan_preset(workspace_selector, preset) {
        Ok(plan) => plan,
        Err(error) if json => return report_json_cli_error(error),
        Err(error) => return report_cli_error(error),
    };
    let text = serde_json::to_string_pretty(&plan).unwrap_or_else(|_| plan.to_string());
    if json {
        println!("{text}");
        return 0;
    }
    match workspace_plan_preset_text(workspace_selector, &plan, &format!("{text}\n")) {
        Ok(()) => 0,
        Err(error) => report_cli_error(error),
    }
}

/// Human `workspace plan --preset`: the same plan, saved for `apply --plan`
/// and described with what the preset kept and skipped.
#[cfg(feature = "recall")]
fn workspace_plan_preset_text(
    workspace_selector: Option<&str>,
    value: &serde_json::Value,
    plan_json: &str,
) -> Result<(), margins_cli::CliError> {
    use margins_cli::commands::workspace_text::PresetOutcome;
    use margins_cli::CliError;
    let invalid = |error: serde_json::Error| CliError::new("output_failed", error.to_string());
    let plan: margins_workflows::workspace::WorkspacePlan =
        serde_json::from_value(value.clone()).map_err(invalid)?;
    let preset = &value["preset"];
    let outcome = PresetOutcome {
        template: preset["template"].as_str().unwrap_or_default().to_string(),
        skipped_readings: serde_json::from_value(preset["skipped_readings"].clone())
            .map_err(invalid)?,
        skip_reasons: serde_json::from_value(preset["skip_reasons"].clone()).map_err(invalid)?,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let workspace = margins_cli::commands::workspace::inspect_existing(workspace_selector, &cwd)?;
    margins_cli::commands::workspace::write_plan_text(
        &workspace,
        &plan,
        Some(&outcome),
        plan_json.as_bytes(),
        &mut io::stdout(),
    )
}

#[cfg(feature = "recall")]
fn workspace_plan_preset(
    workspace_selector: Option<&str>,
    preset: &str,
) -> Result<serde_json::Value, margins_cli::CliError> {
    use margins_cli::CliError;
    use margins_workflows::workspace_preset;

    let Some(selector) = workspace_selector else {
        return Err(CliError::new(
            "workspace_required",
            "workspace plan needs an explicit --workspace <id>",
        ));
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    // A preview writes nothing to the Margins home: the Workspace is inspected
    // (not migrated), and the engine fills the preset in a throwaway home.
    // Migration and managed files happen at `workspace apply`.
    let workspace = margins_cli::commands::workspace::inspect_existing(Some(selector), &cwd)?;
    let invalid = |error: anyhow::Error| CliError::new("workspace_preset_invalid", format!("{error:#}"));
    let margins_home = margins_workflows::workspace::margins_home().map_err(CliError::from_anyhow)?;
    let scratch = tempfile::tempdir()
        .map_err(|error| CliError::new("engine_unavailable", format!("creating a scratch engine home: {error}")))?;
    let (template, template_label) = if preset == workspace_preset::MEETINGS_PRESET {
        let path = scratch.path().join(format!("{preset}.enzyme.in"));
        std::fs::write(&path, workspace_preset::MEETINGS_PRESET_TEXT)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        (path, std::path::PathBuf::from(preset))
    } else if Path::new(preset).is_file() {
        (cwd.join(preset), cwd.join(preset))
    } else {
        return Err(CliError::new(
            "workspace_preset_unknown",
            format!(
                "unknown preset {preset:?}; use {} or the path to a .enzyme.in template",
                workspace_preset::MEETINGS_PRESET
            ),
        ));
    };
    let home = workspace
        .config
        .bindings
        .values()
        .find_map(|binding| match binding {
            margins_workflows::workspace::WorkspaceBinding::NativeMarkdown {
                path,
                role: margins_workflows::workspace::SourceRole::Home,
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .ok_or_else(|| CliError::new("workspace_preset_invalid", "Workspace has no Home notes source"))?;
    let engine = crate::enzyme_cli::Engine::for_scratch_home(&margins_home, &scratch.path().join("home"))
        .map_err(|error| CliError::new("engine_unavailable", format!("{error:#}")))?;
    let filled = engine
        .compile_preset(&workspace.config.id, &template, &home)
        .map_err(|error| CliError::new("workspace_preset_invalid", error.to_string()))?;
    let proposal = workspace_preset::propose(&workspace.config, &filled).map_err(invalid)?;
    let plan = margins_workflows::workspace::plan_workspace_config(&workspace, proposal.desired.clone())
        .map_err(CliError::from_anyhow)?;
    let mut value = serde_json::to_value(&plan)
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    value["program_path"] = serde_json::json!(workspace.config_path);
    value["preset"] = serde_json::json!({
        "template": template_label,
        "readings": proposal.readings,
        "skipped_readings": proposal.skipped_readings,
        "skip_reasons": proposal.skip_reasons,
        "note_folder": proposal.note_folder,
    });
    Ok(value)
}

#[cfg(feature = "recall")]
fn run_init(workspace_selector: Option<&str>) -> i32 {
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    let margins_home = match crate::hosted_credentials::margins_home() {
        Ok(home) => home,
        Err(error) => return report_error(&error.to_string()),
    };
    eprintln!("Building or refreshing recall index and catalysts…");
    match crate::recall::provision_workspace_for_init(&workspace) {
        Ok(status) => {
            if status.status == "catalysts_pending" {
                return report_error(&status.readiness.message());
            }
            let catalyst = margins_workflows::catalyst::selected_status(&margins_home);
            if let Err(error) = margins_cli::commands::projects::write_init(
                &mut io::stdout(),
                &workspace.home_dir,
                status.status,
                Some(&workspace.config_path),
                Some(&catalyst),
            ) {
                return report_error(&error.to_string());
            }
            // stdout stays the one-line init receipt; the reminder is for people.
            let _ = margins_cli::commands::workspace_text::write_program_block(
                &mut io::stderr(),
                &workspace.config.id,
                &workspace.config_path,
            );
            0
        }
        Err(error) => report_error(&format!("indexing vault: {error:#}")),
    }
}

/// [`margins_cli::commands::workspace::resolve_read_only`] from the process cwd.
#[cfg(feature = "recall")]
fn read_only_workspace(
    selector: Option<&str>,
) -> Result<margins_workflows::workspace::ResolvedWorkspace, margins_cli::CliError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    margins_cli::commands::workspace::resolve_read_only(selector, &cwd)
}

fn resolve_workspace(
    selector: Option<&str>,
) -> anyhow::Result<margins_workflows::workspace::ResolvedWorkspace> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home = margins_workflows::workspace::margins_home()?;
    let roots = margins_workflows::workspace::ImplicitWorkspaceRoots::from_process(&home)?;
    let resolution =
        margins_workflows::workspace::resolve_or_create_workspace(&roots, selector, &cwd)?;
    if resolution.created_implicitly {
        eprintln!(
            "Created workspace {} with home {}",
            resolution.workspace.config.id,
            resolution.workspace.home_dir.display()
        );
    }
    Ok(resolution.workspace)
}

/// `margins enzyme <args>`: the bundled engine on the Margins home, never
/// `~/.enzyme` or an inherited `ENZYME_HOME`.
#[cfg(feature = "recall")]
fn run_enzyme(workspace_selector: Option<&str>, args: &[OsString]) -> i32 {
    let run = || -> anyhow::Result<i32> {
        let margins_home = margins_workflows::workspace::margins_home()?;
        let env_workspace = std::env::var("MARGINS_WORKSPACE")
            .ok()
            .filter(|value| !value.trim().is_empty());
        // The selected Workspace, else the one whose notes folder holds the
        // cwd, else the machine default; never the engine's own cwd vault.
        let workspace = match workspace_selector.or(env_workspace.as_deref()) {
            Some(workspace) => Some(workspace.to_string()),
            None => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
                match margins_workflows::workspace::inspect_workspace(&margins_home, None, &cwd) {
                    Ok(workspace) => Some(workspace.config.id),
                    Err(_) => margins_workflows::workspace::default_workspace(&margins_home)?,
                }
            }
        };
        let passthrough = crate::enzyme_cli::passthrough_args(workspace.as_deref(), args)
            .map_err(anyhow::Error::msg)?;
        let engine = crate::enzyme_cli::Engine::for_home(&margins_home)?;
        let generator = if passthrough.needs_generator {
            Some(crate::enzyme_cli::selected_generator(&engine, &margins_home)?)
        } else {
            None
        };
        let status = engine.passthrough(&passthrough.args, generator.as_ref())?;
        Ok(status.code().unwrap_or(1))
    };
    match run() {
        Ok(code) => code,
        Err(error) => report_error(&format!("{error:#}")),
    }
}

#[cfg(feature = "recall")]
fn run_workspace_status(selector: Option<&str>, json: bool) -> i32 {
    // Status is read-only: an existing Workspace is inspected, never migrated.
    let workspace = match read_only_workspace(selector) {
        Ok(workspace) => workspace,
        Err(error) if json => return report_json_cli_error(error),
        Err(error) => return report_cli_error(error),
    };
    let source_refresh_staleness =
        match crate::recall::workspace_source_refresh_staleness(&workspace) {
            Ok(status) => status,
            Err(error) => {
                return report_error(&format!("reading recall source freshness: {error:#}"))
            }
        };
    let recall = match crate::recall::workspace_status_recall(&workspace) {
        Ok(status) => status,
        Err(error) => return report_error(&format!("reading recall index status: {error:#}")),
    };
    match margins_cli::commands::workspace::render_status(
        &workspace,
        json,
        recall,
        &source_refresh_staleness,
        &mut io::stdout(),
    ) {
        Ok(()) => 0,
        Err(error) => report_error(&error.to_string()),
    }
}

fn report_error(message: &str) -> i32 {
    let message = margins_user_message(message);
    eprintln!(
        "<margins_error code=\"command_failed\">{}</margins_error>",
        margins_cli::output::xml_escape_text(&message)
    );
    1
}

#[cfg(feature = "recall")]
fn report_cli_error(error: margins_cli::CliError) -> i32 {
    let exit_code = error.exit_code();
    let _ = margins_cli::output::write_error(&mut io::stderr(), &error);
    exit_code
}

fn report_json_cli_error(error: margins_cli::CliError) -> i32 {
    let exit_code = error.exit_code();
    let _ = margins_cli::output::write_json_error(&mut io::stderr(), &error);
    exit_code
}

fn margins_user_message(message: &str) -> String {
    let mut result = message.to_string();
    for (internal, replacement) in [
        ("enzyme login", "margins setup"),
        ("--use-env-llm", "margins setup"),
        ("api.enzyme.garden", "hosted catalyst service"),
        ("openrouter", "hosted catalyst provider"),
    ] {
        while let Some(start) = result.to_ascii_lowercase().find(internal) {
            result.replace_range(start..start + internal.len(), replacement);
        }
    }
    result
}
