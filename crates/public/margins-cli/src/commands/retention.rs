use crate::args::{RetentionConnectorArg, RetentionScopeArg};
use crate::error::CliError;
use margins_workflows::integrations::{
    apply_retention, preview_retention, RetentionPreview, RetentionScope, RetentionTarget,
};
use margins_workflows::workspace::{normalize_google_account, ResolvedWorkspace};
use std::io::Write;
use std::path::Path;

pub fn preview(
    workspace: &ResolvedWorkspace,
    connector: RetentionConnectorArg,
    account: &str,
    scope: RetentionScopeArg,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let target = normalized_target(connector, account)?;
    let preview = preview_retention(workspace, &target, retention_scope(scope))
        .map_err(CliError::from_anyhow)?;
    write_json(stdout, &preview)
}

pub fn apply(
    workspace: &ResolvedWorkspace,
    plan_path: &Path,
    expected_revision: &str,
    request_id: &str,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let bytes = std::fs::read(plan_path).map_err(|error| {
        CliError::new(
            "retention_plan_unreadable",
            format!("could not read {}: {error}", plan_path.display()),
        )
    })?;
    let preview: RetentionPreview = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::new(
            "retention_plan_invalid",
            format!("invalid retention preview JSON: {error}"),
        )
    })?;
    if preview.workspace_id != workspace.config.id {
        return Err(CliError::new(
            "workspace_mismatch",
            format!(
                "retention plan workspace '{}' does not match explicit workspace '{}'",
                preview.workspace_id, workspace.config.id
            ),
        ));
    }
    let receipt = apply_retention(workspace, &preview, expected_revision, request_id)
        .map_err(CliError::from_anyhow)?;
    write_json(stdout, &receipt)
}

fn normalized_target(
    connector: RetentionConnectorArg,
    account: &str,
) -> Result<RetentionTarget, CliError> {
    let (connector_id, source_account) = match connector {
        RetentionConnectorArg::Email => (
            "email",
            normalize_google_account(account).map_err(CliError::from_anyhow)?,
        ),
        RetentionConnectorArg::Gcal => (
            "gcal",
            normalize_google_account(account).map_err(CliError::from_anyhow)?,
        ),
        RetentionConnectorArg::GoogleMeet => (
            "google_meet",
            normalize_google_account(account).map_err(CliError::from_anyhow)?,
        ),
    };
    Ok(RetentionTarget {
        connector_id: connector_id.to_string(),
        source_account,
    })
}

fn retention_scope(scope: RetentionScopeArg) -> RetentionScope {
    match scope {
        RetentionScopeArg::Expired => RetentionScope::Expired,
        RetentionScopeArg::RawCache => RetentionScope::RawCache,
        RetentionScopeArg::Tombstones => RetentionScope::Tombstones,
        RetentionScopeArg::Materialization => RetentionScope::Materialization,
        RetentionScopeArg::All => RetentionScope::All,
    }
}

fn write_json(stdout: &mut dyn Write, value: &impl serde::Serialize) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *stdout, value)
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    writeln!(stdout).map_err(|error| CliError::new("output_failed", error.to_string()))
}
