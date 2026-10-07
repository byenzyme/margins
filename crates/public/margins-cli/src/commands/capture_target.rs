//! Where session commands read and record.
//!
//! Recordings belong to a Workspace: an explicit `--workspace` or
//! `MARGINS_WORKSPACE`, else the Workspace that declares the cwd, else the
//! machine default. Nothing here creates a Workspace or a per-folder
//! `.margins/` store.
//!
//! Per-folder `.margins/` stores written by earlier releases stay usable for
//! the sessions they already hold: commands that work on existing sessions
//! (`ls`, `current`, `transcript`, `process`, …) fall back to such a store when
//! no Workspace declares the cwd, or use it when `--project` names it. They are
//! never copied, moved, or adopted, so nothing in them can be lost; new
//! recordings always go to a Workspace.

use crate::error::CliError;
use crate::services::CliServices;
use margins_workflows::project::ResolvedProject;
use margins_workflows::workspace::{self, ResolvedWorkspace};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Shown when a session command finds no Workspace to use.
pub const NO_CAPTURE_WORKSPACE_MESSAGE: &str = "No Margins Workspace covers this folder and no default Workspace is set, so there is nowhere to keep this recording. \
Run `margins init` in your notes folder first, or pass `--workspace <id>`.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureIntent {
    /// Starts or continues a recording, or imports audio as a new session.
    Record,
    /// Reads or maintains sessions that already exist.
    Existing,
}

pub enum CaptureTarget {
    /// The Workspace's declared capture store.
    Workspace {
        workspace: Box<ResolvedWorkspace>,
        capture_root: PathBuf,
    },
    /// A per-folder `.margins/` store from an earlier release. Only existing
    /// sessions are used; nothing new is recorded into it.
    Legacy(ResolvedProject),
}

/// Choose the store for a session command. See the module docs for the order.
pub fn resolve(
    services: &CliServices,
    workspace_selector: Option<&str>,
    project_selector: Option<&str>,
    cwd: &Path,
    intent: CaptureIntent,
    stderr: &mut dyn Write,
) -> Result<CaptureTarget, CliError> {
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let explicit_workspace =
        workspace_selector.is_some_and(|value| !value.trim().is_empty()) || env_workspace.is_some();
    if explicit_workspace && project_selector.is_some() {
        return Err(CliError::new(
            "invalid_arguments",
            "`--project` cannot be combined with an explicit Workspace selection",
        ));
    }

    if let Some(selector) = project_selector {
        if intent == CaptureIntent::Record {
            return Err(CliError::new(
                "legacy_store_read_only",
                "`--project` selects a per-folder .margins store from an earlier release; new recordings go to a Workspace. Pass `--workspace <id>`, or run `margins init` in your notes folder.",
            ));
        }
        let project = match services.projects.resolve_vault(Some(selector), cwd) {
            Ok(project) => project,
            // A folder holding an unregistered store is selected by its path.
            Err(error) => {
                let folder = crate::commands::projects::absolute_from(cwd, Path::new(selector));
                if !folder.join(".margins").is_dir() {
                    return Err(CliError::from_anyhow(error));
                }
                services
                    .projects
                    .resolve_vault(None, &folder)
                    .map_err(CliError::from_anyhow)?
            }
        };
        return Ok(CaptureTarget::Legacy(project));
    }

    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    if explicit_workspace {
        let selected =
            workspace::resolve_workspace_or_default(&margins_home, workspace_selector, cwd)
                .map_err(CliError::from_anyhow)?
                .ok_or_else(|| CliError::new("workspace_required", NO_CAPTURE_WORKSPACE_MESSAGE))?;
        return workspace_target(selected.workspace);
    }

    let legacy = legacy_store(services, cwd);
    let covering =
        workspace::covering_workspace_id(&margins_home, cwd).map_err(CliError::from_anyhow)?;
    if covering.is_none() && intent == CaptureIntent::Existing {
        if let Some(project) = legacy {
            return Ok(CaptureTarget::Legacy(project));
        }
    }
    let selected = workspace::resolve_workspace_or_default(&margins_home, None, cwd)
        .map_err(CliError::from_anyhow)?
        .ok_or_else(|| CliError::new("workspace_required", NO_CAPTURE_WORKSPACE_MESSAGE))?;
    if selected.via_default {
        writeln!(
            stderr,
            "Using Workspace {} (default)",
            selected.workspace.config.id
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    }
    let target = workspace_target(selected.workspace)?;
    if let (Some(project), CaptureTarget::Workspace { capture_root, .. }) = (&legacy, &target) {
        if !same_dir(&project.work_dir, capture_root) {
            writeln!(
                stderr,
                "Earlier recordings in {} stay readable with `margins --project {} ls`.",
                project.work_dir.join(".margins").display(),
                project.root_dir.display()
            )
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        }
    }
    Ok(target)
}

fn workspace_target(workspace: ResolvedWorkspace) -> Result<CaptureTarget, CliError> {
    let capture_root = workspace
        .capture_store_dir()
        .map_err(CliError::from_anyhow)?;
    Ok(CaptureTarget::Workspace {
        workspace: Box::new(workspace),
        capture_root,
    })
}

/// The per-folder store at or above `cwd`, if one already exists. Margins'
/// machine directory is never a per-folder store.
fn legacy_store(services: &CliServices, cwd: &Path) -> Option<ResolvedProject> {
    let project = services.projects.resolve_vault(None, cwd).ok()?;
    let store = project.work_dir.join(".margins");
    (store.is_dir() && !margins_workflows::project::is_margins_machine_state_dir(&store))
        .then_some(project)
}

fn same_dir(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical(left) == canonical(right)
}
