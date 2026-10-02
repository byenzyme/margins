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
    let revision = match margins_workflows::workspace::workspace_revision(&workspace.config) {
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

    let recall = match crate::recall::provision_workspace_for_init(&workspace) {
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
    let credential = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/private/margins-google/resources/google-oauth-client.json"
    ))
    .as_slice();
    margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        source_filter,
        revision,
        request_id,
        Some(credential),
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

#[cfg(feature = "recall")]
fn run_scan(workspace_selector: Option<&str>) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let workspace =
        match margins_cli::commands::workspace::resolve_existing(workspace_selector, &cwd) {
            Ok(workspace) => workspace,
            Err(error) => return report_error(&error.to_string()),
        };
    match crate::scan::run_scan(&workspace) {
        Ok(()) => 0,
        Err(error) => report_error(&format!("scanning vault: {error:#}")),
    }
}

#[cfg(feature = "recall")]
fn run_workspace_compile(workspace_selector: Option<&str>, note_folder: Option<&str>) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let workspace =
        match margins_cli::commands::workspace::resolve_existing(workspace_selector, &cwd) {
            Ok(workspace) => workspace,
            Err(error) => return report_error(&error.to_string()),
        };
    match crate::setup_compile::compile(&workspace, note_folder) {
        Ok(result) => {
            println!("{result}");
            0
        }
        Err(error) => report_error(&format!("compiling Workspace setup: {error:#}")),
    }
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
    let workspace = match resolve_workspace(selector) {
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
