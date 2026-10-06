/// Resolve the vault git-style and run associative recall against it. Interactive
/// terminals receive Enzyme's catalyze tree; piped consumers receive the
/// compatible JSON envelope. Recall fails closed when catalysts cannot serve
/// the request; callers receive a non-zero command result and an explicit hint.
#[cfg(feature = "recall")]
fn run_recall(workspace_selector: Option<&str>, query: &str, source: Option<&str>) -> i32 {
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    let terminal = io::stdout().is_terminal();
    match crate::recall::recall(&workspace, query, source) {
        Ok(output) => match crate::recall::render_recall_for_stdout(&output, terminal) {
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
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
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
fn run_workspace_plan_preset(workspace_selector: Option<&str>, preset: &str) -> i32 {
    match workspace_plan_preset(workspace_selector, preset) {
        Ok(plan) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).unwrap_or_else(|_| plan.to_string())
            );
            0
        }
        Err(error) => report_json_cli_error(error),
    }
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
            match margins_cli::commands::projects::write_init(
                &mut io::stdout(),
                &workspace.home_dir,
                status.status,
                Some(&workspace.config_path),
                Some(&catalyst),
            ) {
                Ok(()) => 0,
                Err(error) => report_error(&error.to_string()),
            }
        }
        Err(error) => report_error(&format!("indexing vault: {error:#}")),
    }
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

#[cfg(feature = "recall")]
fn run_workspace_status(selector: Option<&str>) -> i32 {
    // Status is read-only: an existing Workspace is inspected, never migrated.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut stderr = io::stderr();
    let workspace = match margins_cli::commands::workspace::inspect(selector, &cwd, &mut stderr) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
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
        true,
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
