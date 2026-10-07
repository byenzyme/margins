//! Where session commands read and record.
//!
//! Recordings belong to a Workspace: an explicit `--workspace` or
//! `MARGINS_WORKSPACE`, else the Workspace that declares the cwd, else the
//! machine default. Nothing here creates a Workspace or a per-folder
//! `.margins/` store.
//!
//! Per-folder `.margins/` stores written by earlier releases are never copied,
//! moved, or adopted, so nothing in them can be lost. They stay usable:
//! - every command resolves the same way, so reads and recordings in one folder
//!   always meet the same store; an old store is used only when no Workspace
//!   covers the folder and no default is set, or when `--project` names it;
//! - `attach` continues a session that already lives in the old store (a named
//!   one, or the unfinished current one), so a recording started before an
//!   upgrade can be finished;
//! - a concrete meeting id missing from the Workspace is looked up in the old
//!   store (see [`CaptureTarget::Workspace::legacy`]).
//!
//! New recordings (`new`, bare `margins`, `transcribe`) always go to a
//! Workspace.

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
pub enum CaptureIntent<'a> {
    /// Starts a new session (`new`, bare `margins`, `transcribe`).
    Record,
    /// Continues a session: the named one, or the current one.
    Attach(Option<&'a str>),
    /// Reads or maintains sessions that already exist.
    Existing,
}

pub enum CaptureTarget {
    /// The Workspace's declared capture store.
    Workspace {
        workspace: Box<ResolvedWorkspace>,
        capture_root: PathBuf,
        /// An old per-folder store at or above the cwd that this Workspace
        /// shadows; a concrete meeting id the Workspace lacks may live there.
        legacy: Option<ResolvedProject>,
    },
    /// A per-folder `.margins/` store from an earlier release. Only existing
    /// sessions are used; nothing new is started in it.
    Legacy(ResolvedProject),
}

/// Choose the store for a session command. See the module docs for the order.
pub fn resolve(
    services: &CliServices,
    workspace_selector: Option<&str>,
    project_selector: Option<&str>,
    cwd: &Path,
    intent: CaptureIntent<'_>,
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
        let project = explicit_legacy_store(services, selector, cwd)?;
        let allowed = match intent {
            CaptureIntent::Record => false,
            CaptureIntent::Attach(session) => continues_legacy_session(services, &project, session),
            CaptureIntent::Existing => true,
        };
        if !allowed {
            return Err(CliError::new(
                "legacy_store_read_only",
                "`--project` selects a per-folder .margins store from an earlier release; it can continue its own sessions but new recordings go to a Workspace. Pass `--workspace <id>`, or run `margins init` in your notes folder.",
            ));
        }
        return Ok(CaptureTarget::Legacy(project));
    }

    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    if explicit_workspace {
        let selected =
            workspace::resolve_workspace_or_default(&margins_home, workspace_selector, cwd)
                .map_err(CliError::from_anyhow)?
                .ok_or_else(|| CliError::new("workspace_required", NO_CAPTURE_WORKSPACE_MESSAGE))?;
        return workspace_target(selected.workspace, None);
    }

    let legacy = legacy_store(services, cwd);
    let selected = workspace::resolve_workspace_or_default(&margins_home, None, cwd).map_err(
        |error| match &legacy {
            Some(project) => CliError::new(
                "command_failed",
                format!(
                    "{error:#}. Earlier recordings in this folder stay readable with `margins --project {} ls`",
                    project.root_dir.display()
                ),
            ),
            None => CliError::from_anyhow(error),
        },
    )?;
    // A session already recorded into the old store is continued there.
    if let (CaptureIntent::Attach(session), Some(project)) = (intent, &legacy) {
        if continues_legacy_session(services, project, session) {
            return Ok(CaptureTarget::Legacy(project.clone()));
        }
    }
    let Some(selected) = selected else {
        return match (intent, legacy) {
            (CaptureIntent::Existing, Some(project)) => Ok(CaptureTarget::Legacy(project)),
            _ => Err(CliError::new("workspace_required", NO_CAPTURE_WORKSPACE_MESSAGE)),
        };
    };
    if selected.via_default {
        writeln!(
            stderr,
            "Using Workspace {} (default)",
            selected.workspace.config.id
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    }
    let target = workspace_target(selected.workspace, legacy)?;
    if let CaptureTarget::Workspace {
        legacy: Some(project),
        ..
    } = &target
    {
        writeln!(
            stderr,
            "Earlier recordings in {} stay readable with `margins --project {} ls`.",
            project.work_dir.join(".margins").display(),
            project.root_dir.display()
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    }
    Ok(target)
}

fn workspace_target(
    workspace: ResolvedWorkspace,
    legacy: Option<ResolvedProject>,
) -> Result<CaptureTarget, CliError> {
    let capture_root = workspace
        .capture_store_dir()
        .map_err(CliError::from_anyhow)?;
    let legacy = legacy.filter(|project| !same_dir(&project.work_dir, &capture_root));
    Ok(CaptureTarget::Workspace {
        workspace: Box::new(workspace),
        capture_root,
        legacy,
    })
}

/// The store `--project` names: a registered vault, or a folder holding a
/// store by its path.
fn explicit_legacy_store(
    services: &CliServices,
    selector: &str,
    cwd: &Path,
) -> Result<ResolvedProject, CliError> {
    match services.projects.resolve_vault(Some(selector), cwd) {
        Ok(project) => Ok(project),
        Err(error) => {
            let folder = crate::commands::projects::absolute_from(cwd, Path::new(selector));
            if !folder.join(".margins").is_dir() {
                return Err(CliError::from_anyhow(error));
            }
            services
                .projects
                .resolve_vault(None, &folder)
                .map_err(CliError::from_anyhow)
        }
    }
}

/// Whether `attach` continues a session that lives in the old store: the named
/// session exists there, or (with no name) its current session is unfinished.
fn continues_legacy_session(
    services: &CliServices,
    project: &ResolvedProject,
    session: Option<&str>,
) -> bool {
    let margins_dir = project.work_dir.join(".margins");
    if !margins_dir.is_dir() {
        return false;
    }
    if let Some(name) = session {
        return services
            .sessions
            .exists(&margins_dir, name)
            .unwrap_or(false);
    }
    let Ok(Some(current)) = services.sessions.current(&margins_dir) else {
        return false;
    };
    let current = current.trim();
    !current.is_empty()
        && services.sessions.list(&margins_dir).is_ok_and(|sessions| {
            sessions
                .iter()
                .any(|info| info.name == current && info.lifecycle_state == "active")
        })
}

/// The per-folder store at or above `cwd`, if one already exists. Margins'
/// machine directory is never a per-folder store, and neither is a configured
/// vault that the launcher temp-dir exception of `resolve_vault` would pick
/// for an agent started outside it: the store must contain the cwd.
fn legacy_store(services: &CliServices, cwd: &Path) -> Option<ResolvedProject> {
    let project = services.projects.resolve_vault(None, cwd).ok()?;
    let store = project.work_dir.join(".margins");
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    (store.is_dir()
        && !margins_workflows::project::is_margins_machine_state_dir(&store)
        && canonical(cwd).starts_with(canonical(&project.root_dir)))
    .then_some(project)
}

fn same_dir(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical(left) == canonical(right)
}
