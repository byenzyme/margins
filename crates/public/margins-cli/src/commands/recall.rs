//! Autonomous public Workspace initialization, sync, and local recall.

use crate::error::CliError;
use margins_workflows::local_recall;
use margins_workflows::workspace::ResolvedWorkspace;
use serde_json::json;
use std::io::Write;

pub fn init(workspace: &ResolvedWorkspace, stdout: &mut dyn Write) -> Result<(), CliError> {
    let status = local_recall::status(workspace).map_err(CliError::from_anyhow)?;
    writeln!(
        stdout,
        "<margins_init status=\"ok\" mode=\"{}\" documents=\"{}\" />",
        status.mode, status.documents
    )
    .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn run(
    workspace: &ResolvedWorkspace,
    query: &str,
    source: Option<&str>,
    json: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let output = local_recall::search(workspace, query, source).map_err(CliError::from_anyhow)?;
    let failed = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    if json {
        serde_json::to_writer_pretty(&mut *stdout, &output)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        return writeln!(stdout).map_err(failed);
    }
    if output.results.is_empty() {
        return writeln!(stdout, "No notes matched \"{}\".", output.query).map_err(failed);
    }
    writeln!(
        stdout,
        "{} {} for \"{}\":",
        output.total_results,
        if output.total_results == 1 { "match" } else { "matches" },
        output.query
    )
    .map_err(failed)?;
    for hit in &output.results {
        writeln!(stdout).map_err(failed)?;
        writeln!(stdout, "{} ({})", hit.document_ref, hit.source).map_err(failed)?;
        let excerpt = hit.content.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt = match excerpt.char_indices().nth(240) {
            Some((end, _)) => format!("{}…", &excerpt[..end]),
            None => excerpt,
        };
        writeln!(stdout, "  {excerpt}").map_err(failed)?;
    }
    Ok(())
}

pub fn sync(
    workspace: &ResolvedWorkspace,
    source_filter: Option<&str>,
    json_output: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    if let Some(source) = source_filter {
        if !workspace.config.bindings.contains_key(source) {
            return Err(CliError::new(
                "source_not_configured",
                format!(
                    "workspace '{}' has no declared source '{}'",
                    workspace.config.id, source
                ),
            ));
        }
    }
    let status = local_recall::status(workspace).map_err(CliError::from_anyhow)?;
    let sources = workspace
        .config
        .bindings
        .iter()
        .filter(|(name, _)| source_filter.is_none_or(|filter| filter == name.as_str()))
        .map(|(name, binding)| {
            let local_path = binding.local_path();
            let ready = local_path.is_some_and(|path| path.is_dir());
            let status = if ready {
                "ready"
            } else if local_path.is_some() {
                "missing_path"
            } else {
                "requires_provider"
            };
            json!({
                "name": name,
                "kind": binding.kind(),
                "ok": ready,
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    let ok = sources
        .iter()
        .all(|source| source.get("ok").and_then(serde_json::Value::as_bool) == Some(true));
    let envelope = json!({
        "schema_version": "margins.sync.v1",
        "ok": ok,
        "workspace": {"id": workspace.config.id, "path": workspace.state_dir},
        "sources": sources,
        "recall": status,
    });
    if json_output {
        serde_json::to_writer_pretty(&mut *stdout, &envelope)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        writeln!(stdout).map_err(|error| CliError::new("output_failed", error.to_string()))?;
    } else {
        writeln!(stdout, "Sync workspace {}", workspace.config.id)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        writeln!(
            stdout,
            "Local recall: {} Markdown documents available",
            status.documents
        )
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        for source in &sources {
            writeln!(
                stdout,
                "{}: {}",
                source["name"].as_str().unwrap_or("unknown"),
                source["status"].as_str().unwrap_or("unknown"),
            )
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        }
    }
    if ok {
        Ok(())
    } else {
        Err(CliError::new(
            "sync_incomplete",
            "one or more declared sources are not available in this composition",
        ))
    }
}
