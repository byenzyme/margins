use crate::args::{GranolaTimeRangeArg, SourceKindArg, SourceRoleArg};
use crate::error::CliError;
use anyhow::Context;
use margins_workflows::catalyst::{selected_status, CatalystStatus};
use margins_workflows::workspace::{
    self, CalendarCollectionSelector, GmailCollectionSelector, IndexPolicy, ResolvedWorkspace,
    SourceKind, SourceRole, WorkspaceBinding, WorkspaceConfig, WorkspacePlan, WorkspaceProgram,
};
use margins_workflows::workspace_program::derive_view;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize)]
struct PublicWorkspaceView<'a> {
    id: &'a str,
    revision: String,
    name: &'a Option<String>,
    state_dir: String,
    home: String,
    config: String,
    recall: margins_workflows::local_recall::LocalRecallStatus,
    build: crate::build_info::BuildInfo,
}

#[derive(Serialize)]
struct RuntimeWorkspaceView<'a> {
    id: &'a str,
    revision: String,
    name: &'a Option<String>,
    state_dir: String,
    home: String,
    config: String,
    index: String,
    ledger: String,
    catalyst: CatalystStatus,
    recall: margins_workflows::local_recall::LocalRecallStatus,
    source_refresh_staleness: &'a BTreeMap<String, SourceRefreshStalenessView>,
    build: crate::build_info::BuildInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRefreshStalenessView {
    pub last_refresh_ms: Option<i64>,
    pub stale: bool,
    pub stale_reason: Option<String>,
}

#[derive(Serialize)]
struct SourceView<'a> {
    name: &'a str,
    #[serde(flatten)]
    binding: &'a WorkspaceBinding,
    cache_raw_payload: bool,
    project_to_home: bool,
    indexed_how: &'static str,
}

pub fn new(
    id: &str,
    name: Option<&str>,
    home: &Path,
    json: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let resolved = workspace::create_workspace(&margins_home, id, name, home)
        .map_err(CliError::from_anyhow)?;
    render_workspace(&resolved, json, &BTreeMap::new(), stdout)
}

pub fn list(json: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let workspaces = workspace::list_workspaces(&home).map_err(CliError::from_anyhow)?;
    let default = workspace::default_workspace(&home).map_err(CliError::from_anyhow)?;
    if json {
        let entries = workspaces
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.config.id, "name": item.config.name,
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_writer(
            &mut *stdout,
            &serde_json::json!({
                "default_workspace": default, "workspaces": entries,
            }),
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        for item in workspaces {
            writeln!(
                stdout,
                "{}\t{}",
                item.config.id,
                item.config.name.as_deref().unwrap_or("")
            )
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        }
        Ok(())
    }
}

pub fn remove(id: &str, json: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    workspace::remove_empty_workspace(&home, id).map_err(CliError::from_anyhow)?;
    if json {
        serde_json::to_writer(
            &mut *stdout,
            &serde_json::json!({ "removed_workspace": id }),
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        writeln!(
            stdout,
            "Removed Workspace {id}; Source folders were preserved."
        )
        .map_err(|error| CliError::from_anyhow(error.into()))
    }
}

pub fn default(set: Option<&str>, json: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    if let Some(id) = set {
        workspace::set_default_workspace(&home, id).map_err(CliError::from_anyhow)?;
    }
    let selected = workspace::default_workspace(&home).map_err(CliError::from_anyhow)?;
    if json {
        serde_json::to_writer(
            &mut *stdout,
            &serde_json::json!({ "default_workspace": selected }),
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        writeln!(
            stdout,
            "Default Workspace: {}",
            selected.as_deref().unwrap_or("none")
        )
        .map_err(|error| CliError::from_anyhow(error.into()))
    }
}

pub fn destination(
    selector: Option<&str>,
    cwd: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let selected = selector
        .map(str::to_string)
        .or(workspace::default_workspace(&home).map_err(CliError::from_anyhow)?)
        .ok_or_else(|| {
            CliError::new(
                "workspace_required",
                "choose a Workspace with --workspace or set a machine default",
            )
        })?;
    let resolved = resolve_existing(Some(&selected), cwd)?;
    let (source_id, folder) = resolved
        .config
        .bindings
        .iter()
        .find_map(|(source_id, binding)| match binding {
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                note_folder,
                ..
            } => Some((source_id, note_folder)),
            _ => None,
        })
        .ok_or_else(|| CliError::new("workspace_invalid", "Workspace has no Home binding"))?;
    let destination = resolved.note_destination().map_err(CliError::from_anyhow)?;
    serde_json::to_writer(
        &mut *stdout,
        &serde_json::json!({
            "workspace_id": resolved.config.id, "home_root": resolved.home_dir,
            "home_source_id": source_id, "note_folder": folder, "destination": destination,
        }),
    )
    .map_err(|error| CliError::from_anyhow(error.into()))?;
    writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn status(
    selector: Option<&str>,
    cwd: &Path,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let workspace = resolve(selector, cwd, stderr)?;
    render_workspace(&workspace, json, &BTreeMap::new(), stdout)
}

pub fn plan(
    selector: Option<&str>,
    cwd: &Path,
    desired_path: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let selector = require_explicit_workspace(selector)?;
    let workspace = resolve_existing(Some(selector), cwd)?;
    let body = std::fs::read_to_string(desired_path).map_err(|error| {
        CliError::new(
            "workspace_desired_unreadable",
            format!("could not read {}: {error}", desired_path.display()),
        )
    })?;
    let legacy_toml = desired_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"));
    let plan = if legacy_toml {
        // Older setup callers still write the retired config.toml shape.
        let desired: WorkspaceConfig = toml::from_str(&body).map_err(|error| {
            CliError::new(
                "workspace_desired_invalid",
                format!("invalid desired workspace config: {error}"),
            )
        })?;
        workspace::plan_legacy_workspace_config(&workspace, desired)
    } else {
        workspace::plan_workspace_program(&workspace, &body)
    }
    .map_err(|error| desired_error(error))?;
    let desired_view = WorkspaceProgram::parse(&plan.desired_program)
        .and_then(|program| {
            derive_view(
                &program,
                workspace.config.name.clone(),
                workspace.config.retention.clone(),
            )
        })
        .map_err(|error| desired_error(error))?;
    validate_desired_home_folder_entities(&desired_view)?;
    serde_json::to_writer_pretty(&mut *stdout, &plan)
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    writeln!(stdout).map_err(|error| CliError::new("output_failed", error.to_string()))
}

/// Typed mutation errors keep their codes; anything else in a desired program
/// is the caller's input to fix.
fn desired_error(error: anyhow::Error) -> CliError {
    if error
        .downcast_ref::<workspace::WorkspaceMutationError>()
        .is_some()
    {
        return CliError::from_anyhow(error);
    }
    CliError::new("workspace_desired_invalid", format!("{error:#}"))
}

/// Scan emits unscoped `folder:<path>` specs for the Workspace home. Refuse a
/// plan that cannot resolve one of those specs before the user reviews and
/// consents to a policy that would fail later during init. With several
/// Markdown sources, folder readings are root-qualified; only those of the
/// Home source are checked here.
fn validate_desired_home_folder_entities(desired: &WorkspaceConfig) -> Result<(), CliError> {
    let (home_name, home) = desired
        .bindings
        .iter()
        .find_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown {
                path,
                role: SourceRole::Home,
                ..
            } => Some((name.as_str(), path.as_path())),
            _ => None,
        })
        .ok_or_else(|| {
            CliError::new(
                "workspace_desired_invalid",
                "desired workspace must declare one home notes source",
            )
        })?;
    let markdown_sources = desired
        .bindings
        .values()
        .filter(|binding| matches!(binding, WorkspaceBinding::NativeMarkdown { .. }))
        .count();

    for configured in &desired.policy.entities {
        for (entity_ref, _) in configured.entries() {
            let Some(folder) = strip_ascii_case_prefix(entity_ref.trim(), "folder:") else {
                continue;
            };
            let folder = if markdown_sources > 1 {
                let (root, rest) = folder.split_once('/').unwrap_or((folder, "."));
                if !root.eq_ignore_ascii_case(home_name) {
                    continue;
                }
                rest
            } else {
                folder
            };
            if resolve_scan_folder(home, folder)?.is_none() {
                return Err(CliError::new(
                    "workspace_desired_invalid",
                    format!(
                        "configured entity {entity_ref:?} does not resolve to a folder in the Workspace home"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn strip_ascii_case_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn resolve_scan_folder(home: &Path, relative: &str) -> Result<Option<PathBuf>, CliError> {
    let relative = relative.trim();
    if relative == "." {
        return Ok(home.is_dir().then(|| home.to_path_buf()));
    }
    let components = Path::new(relative).components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CliError::new(
            "workspace_desired_invalid",
            format!("folder entity path must be relative to the Workspace home: {relative:?}"),
        ));
    }

    let mut current = home.to_path_buf();
    for component in components {
        let wanted = component.as_os_str().to_string_lossy();
        let entries = std::fs::read_dir(&current).map_err(|error| {
            CliError::new(
                "workspace_desired_invalid",
                format!(
                    "could not inspect Workspace home folder {}: {error}",
                    current.display()
                ),
            )
        })?;
        let mut matches = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&wanted)
            })
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.path());
        let Some(next) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(CliError::new(
                "workspace_desired_invalid",
                format!("folder entity path {relative:?} is ambiguous under the Workspace home"),
            ));
        }
        current = next;
    }
    Ok(Some(current))
}

pub fn apply(
    selector: Option<&str>,
    cwd: &Path,
    plan_path: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let selector = require_explicit_workspace(selector)?;
    let mut workspace = resolve_existing(Some(selector), cwd)?;
    let bytes = std::fs::read(plan_path).map_err(|error| {
        CliError::new(
            "workspace_plan_unreadable",
            format!("could not read {}: {error}", plan_path.display()),
        )
    })?;
    let plan: WorkspacePlan = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::new(
            "workspace_plan_invalid",
            format!("invalid workspace plan JSON: {error}"),
        )
    })?;
    if plan.workspace_id != selector {
        return Err(CliError::new(
            "workspace_mismatch",
            format!(
                "plan workspace '{}' does not match explicit workspace '{}'",
                plan.workspace_id, selector
            ),
        ));
    }
    let receipt =
        workspace::apply_workspace_plan(&mut workspace, &plan).map_err(CliError::from_anyhow)?;
    serde_json::to_writer_pretty(&mut *stdout, &receipt)
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    writeln!(stdout).map_err(|error| CliError::new("output_failed", error.to_string()))
}

/// Convert retired `workspaces/<id>/config.toml` files to programs. With
/// `--workspace`, only that Workspace; otherwise every one still to migrate.
pub fn migrate(
    selector: Option<&str>,
    dry_run: bool,
    json: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let ids = match selector {
        Some(id) => vec![id.to_string()],
        None => workspace::legacy_workspace_ids(&home).map_err(CliError::from_anyhow)?,
    };
    for id in ids {
        let migration = if dry_run {
            workspace::preview_workspace_migration(&home, &id)
        } else {
            workspace::migrate_workspace(&home, &id)
        }
        .map_err(CliError::from_anyhow)?;
        if json {
            serde_json::to_writer(&mut *stdout, &migration)
                .map_err(|error| CliError::from_anyhow(error.into()))?;
            writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))?;
        } else {
            writeln!(
                stdout,
                "{}: {} -> {}",
                migration.workspace_id,
                migration.status,
                migration.program_path.display()
            )
            .map_err(|error| CliError::from_anyhow(error.into()))?;
            if dry_run {
                write!(stdout, "{}", migration.program)
                    .map_err(|error| CliError::from_anyhow(error.into()))?;
            }
        }
    }
    Ok(())
}

pub fn require_explicit_workspace(selector: Option<&str>) -> Result<&str, CliError> {
    selector.ok_or_else(|| {
        CliError::new(
            "workspace_required",
            "this mutation requires literal --workspace <id>",
        )
    })
}

pub fn render_status(
    workspace: &ResolvedWorkspace,
    json: bool,
    recall: margins_workflows::local_recall::LocalRecallStatus,
    source_refresh_staleness: &BTreeMap<String, SourceRefreshStalenessView>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    render_runtime_workspace(workspace, json, recall, source_refresh_staleness, stdout)
}

pub fn add_source(
    selector: Option<&str>,
    cwd: &Path,
    name: &str,
    kind: SourceKindArg,
    path: Option<&Path>,
    role: Option<SourceRoleArg>,
    account: Option<&str>,
    query: Option<&str>,
    backfill_days: Option<u32>,
    lookback_days: Option<u32>,
    lookahead_days: Option<u32>,
    time_range: Option<GranolaTimeRangeArg>,
    json: bool,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let selector = require_explicit_workspace(selector)?;
    let mut workspace = resolve_existing(Some(selector), cwd)?;
    let kind = source_kind(kind);
    let local_path_kind = matches!(kind, SourceKind::Notes | SourceKind::Captures);
    let google_kind = matches!(
        kind,
        SourceKind::GoogleMail | SourceKind::GoogleCalendar | SourceKind::GoogleMeet
    );
    let granola_kind = kind == SourceKind::Granola;
    if path.is_some() && !local_path_kind {
        return Err(CliError::new(
            "invalid_source_flags",
            "--path applies only to notes and captures sources",
        ));
    }
    if role.is_some() && kind != SourceKind::Notes {
        return Err(CliError::new(
            "invalid_source_flags",
            "--role applies only to notes sources",
        ));
    }
    if account.is_some() && !(google_kind || granola_kind) {
        return Err(CliError::new(
            "invalid_source_flags",
            "--account applies only to Google and Granola sources",
        ));
    }
    if (query.is_some() || backfill_days.is_some()) && kind != SourceKind::GoogleMail {
        return Err(CliError::new(
            "invalid_gmail_flags",
            "--query and --backfill-days apply only to google-mail sources",
        ));
    }
    if (lookback_days.is_some() || lookahead_days.is_some()) && kind != SourceKind::GoogleCalendar {
        return Err(CliError::new(
            "invalid_calendar_flags",
            "--lookback-days and --lookahead-days apply only to google-calendar sources",
        ));
    }
    if time_range.is_some() && kind != SourceKind::Granola {
        return Err(CliError::new(
            "invalid_granola_flags",
            "--time-range applies only to granola sources",
        ));
    }
    let path = path
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        })
        .map(|path| path.canonicalize().unwrap_or(path));
    let account = if google_kind || granola_kind {
        if granola_kind {
            account
                .map(workspace::normalize_granola_account)
                .transpose()
                .map_err(CliError::from_anyhow)?
        } else {
            account
                .map(workspace::normalize_google_account)
                .transpose()
                .map_err(CliError::from_anyhow)?
        }
    } else {
        None
    };
    let gmail = if kind == SourceKind::GoogleMail {
        let selector = GmailCollectionSelector {
            query: query
                .map(str::to_string)
                .unwrap_or_else(|| workspace::DEFAULT_GMAIL_QUERY.to_string()),
            backfill_days: backfill_days.unwrap_or(workspace::DEFAULT_GMAIL_BACKFILL_DAYS),
        };
        selector.validate().map_err(CliError::from_anyhow)?;
        Some(selector)
    } else {
        None
    };
    let calendar = if kind == SourceKind::GoogleCalendar {
        let selector = CalendarCollectionSelector {
            lookback_days: lookback_days.unwrap_or(workspace::DEFAULT_CALENDAR_LOOKBACK_DAYS),
            lookahead_days: lookahead_days.unwrap_or(workspace::DEFAULT_CALENDAR_LOOKAHEAD_DAYS),
        };
        selector.validate().map_err(CliError::from_anyhow)?;
        Some(selector)
    } else {
        None
    };
    let binding = match kind {
        SourceKind::Notes => WorkspaceBinding::NativeMarkdown {
            path: path
                .with_context(|| "notes sources require --path")
                .map_err(CliError::from_anyhow)?,
            role: role
                .map(source_role)
                .with_context(|| "notes sources require --role home or reference")
                .map_err(CliError::from_anyhow)?,
            note_folder: None,
        },
        SourceKind::Captures => WorkspaceBinding::Captures {
            path: path
                .with_context(|| "captures sources require --path")
                .map_err(CliError::from_anyhow)?,
        },
        SourceKind::GoogleMail => WorkspaceBinding::Gmail {
            account: account
                .with_context(|| "google-mail sources require --account")
                .map_err(CliError::from_anyhow)?,
            gmail: gmail.unwrap(),
        },
        SourceKind::GoogleCalendar => WorkspaceBinding::GoogleCalendar {
            account: account
                .with_context(|| "google-calendar sources require --account")
                .map_err(CliError::from_anyhow)?,
            calendar: calendar.unwrap(),
        },
        SourceKind::GoogleMeet => WorkspaceBinding::GoogleMeet {
            account: account
                .with_context(|| "google-meet sources require --account")
                .map_err(CliError::from_anyhow)?,
        },
        SourceKind::Granola => WorkspaceBinding::Granola {
            account: account
                .with_context(|| "granola sources require --account")
                .map_err(CliError::from_anyhow)?,
            collection: margins_workflows::workspace::GranolaCollectionSelector {
                time_range: match time_range.unwrap_or(GranolaTimeRangeArg::Last30Days) {
                    GranolaTimeRangeArg::Last30Days => {
                        margins_workflows::workspace::GranolaTimeRange::Last30Days
                    }
                },
                workspace_only: false,
            },
        },
    };
    workspace::add_source(&mut workspace, name, binding).map_err(CliError::from_anyhow)?;
    render_sources(&workspace, json, stdout)
}

pub fn list_sources(
    selector: Option<&str>,
    cwd: &Path,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let workspace = resolve(selector, cwd, stderr)?;
    render_sources(&workspace, json, stdout)
}

pub fn remove_source(
    selector: Option<&str>,
    cwd: &Path,
    name: &str,
    json: bool,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let selector = require_explicit_workspace(selector)?;
    let mut workspace = resolve_existing(Some(selector), cwd)?;
    workspace::remove_source(&mut workspace, name).map_err(CliError::from_anyhow)?;
    render_sources(&workspace, json, stdout)
}

pub fn resolve(
    selector: Option<&str>,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let roots = workspace::ImplicitWorkspaceRoots::from_process(&margins_home)
        .map_err(CliError::from_anyhow)?;
    let resolution = workspace::resolve_or_create_workspace(&roots, selector, cwd)
        .map_err(CliError::from_anyhow)?;
    if resolution.created_implicitly {
        writeln!(
            stderr,
            "Created workspace {} with home {}",
            resolution.workspace.config.id,
            resolution.workspace.home_dir.display()
        )
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    }
    Ok(resolution.workspace)
}

pub fn resolve_existing(selector: Option<&str>, cwd: &Path) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    workspace::resolve_workspace(&margins_home, selector, cwd).map_err(CliError::from_anyhow)
}

fn render_workspace(
    workspace: &ResolvedWorkspace,
    json: bool,
    _source_refresh_staleness: &BTreeMap<String, SourceRefreshStalenessView>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let view = PublicWorkspaceView {
        id: &workspace.config.id,
        revision: workspace::workspace_revision(&workspace)
            .map_err(CliError::from_anyhow)?,
        name: &workspace.config.name,
        state_dir: workspace.state_dir.to_string_lossy().into_owned(),
        home: workspace.home_dir.to_string_lossy().into_owned(),
        config: workspace.config_path.to_string_lossy().into_owned(),
        recall: margins_workflows::local_recall::status(workspace)
            .map_err(CliError::from_anyhow)?,
        build: crate::build_info::get(),
    };
    if json {
        serde_json::to_writer_pretty(&mut *stdout, &view)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        writeln!(stdout, "Workspace: {}", view.id)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout, "Home: {}", view.home)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout, "State: {}", view.state_dir)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(
            stdout,
            "Recall: {} (available={}, documents={})",
            view.recall.mode, view.recall.available, view.recall.documents,
        )
        .map_err(|error| CliError::from_anyhow(error.into()))
    }
}

fn render_runtime_workspace(
    workspace: &ResolvedWorkspace,
    json: bool,
    recall: margins_workflows::local_recall::LocalRecallStatus,
    source_refresh_staleness: &BTreeMap<String, SourceRefreshStalenessView>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let view = RuntimeWorkspaceView {
        id: &workspace.config.id,
        revision: workspace::workspace_revision(&workspace)
            .map_err(CliError::from_anyhow)?,
        name: &workspace.config.name,
        state_dir: workspace.state_dir.to_string_lossy().into_owned(),
        home: workspace.home_dir.to_string_lossy().into_owned(),
        config: workspace.config_path.to_string_lossy().into_owned(),
        index: workspace.recall_path().to_string_lossy().into_owned(),
        ledger: workspace.ledger_path().to_string_lossy().into_owned(),
        catalyst: selected_status(&margins_home),
        recall,
        source_refresh_staleness,
        build: crate::build_info::get(),
    };
    if json {
        serde_json::to_writer_pretty(&mut *stdout, &view)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        writeln!(stdout, "Workspace: {}", view.id)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout, "Home: {}", view.home)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout, "State: {}", view.state_dir)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(
            stdout,
            "Recall: {} (available={}, documents={})",
            view.recall.mode, view.recall.available, view.recall.documents,
        )
        .map_err(|error| CliError::from_anyhow(error.into()))
    }
}

fn render_sources(
    workspace: &ResolvedWorkspace,
    json: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let views = workspace
        .config
        .bindings
        .iter()
        .map(|(name, binding)| {
            let policy = binding.kind().policy();
            SourceView {
                name,
                binding,
                cache_raw_payload: policy.cache_raw_payload,
                project_to_home: policy.project_to_home,
                indexed_how: match policy.index {
                    IndexPolicy::Native => "native",
                    IndexPolicy::Ledger => "ledger",
                    IndexPolicy::Registry => "registry",
                },
            }
        })
        .collect::<Vec<_>>();
    if json {
        serde_json::to_writer_pretty(&mut *stdout, &views)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        writeln!(stdout).map_err(|error| CliError::from_anyhow(error.into()))
    } else {
        for source in views {
            writeln!(
                stdout,
                "{}\t{:?}\t{}",
                source.name,
                source.binding.kind(),
                source.indexed_how
            )
            .map_err(|error| CliError::from_anyhow(error.into()))?;
        }
        Ok(())
    }
}

fn source_kind(kind: SourceKindArg) -> SourceKind {
    match kind {
        SourceKindArg::Notes => SourceKind::Notes,
        SourceKindArg::Captures => SourceKind::Captures,
        SourceKindArg::GoogleMail => SourceKind::GoogleMail,
        SourceKindArg::GoogleCalendar => SourceKind::GoogleCalendar,
        SourceKindArg::GoogleMeet => SourceKind::GoogleMeet,
        SourceKindArg::Granola => SourceKind::Granola,
    }
}

fn source_role(role: SourceRoleArg) -> SourceRole {
    match role {
        SourceRoleArg::Home => SourceRole::Home,
        SourceRoleArg::Reference => SourceRole::Reference,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::workspace::{RetentionPolicy, WorkspaceEntity, WorkspacePolicy};
    use tempfile::TempDir;

    fn desired_with_folder(home: &Path, folder: &str) -> WorkspaceConfig {
        WorkspaceConfig {
            id: "fixture".to_string(),
            name: None,
            policy: WorkspacePolicy {
                entities: vec![WorkspaceEntity::simple(format!("folder:{folder}"))],
                ..WorkspacePolicy::default()
            },
            retention: RetentionPolicy::default(),
            bindings: BTreeMap::from([(
                "home".to_string(),
                WorkspaceBinding::NativeMarkdown {
                    path: home.to_path_buf(),
                    role: SourceRole::Home,
                    note_folder: None,
                },
            )]),
        }
    }

    #[test]
    fn desired_scan_folder_resolves_case_insensitively_within_home() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join("People")).unwrap();
        let desired = desired_with_folder(temp.path(), "people");

        validate_desired_home_folder_entities(&desired).unwrap();
    }

    #[test]
    fn desired_scan_folder_must_exist_in_home() {
        let home = TempDir::new().unwrap();
        let reference = TempDir::new().unwrap();
        std::fs::create_dir(reference.path().join("people")).unwrap();
        let mut desired = desired_with_folder(home.path(), "people");
        desired.bindings.insert(
            "reference".to_string(),
            WorkspaceBinding::NativeMarkdown {
                path: reference.path().to_path_buf(),
                role: SourceRole::Reference,
                note_folder: None,
            },
        );

        // Several Markdown roots: folder readings are root-qualified, and a
        // folder that exists only under the reference root is not Home's.
        desired.policy.entities = vec![WorkspaceEntity::simple("folder:home/people")];
        let error = validate_desired_home_folder_entities(&desired).unwrap_err();

        assert_eq!(error.code(), "workspace_desired_invalid");
        assert!(error.to_string().contains("does not resolve to a folder"));

        desired.policy.entities = vec![WorkspaceEntity::simple("folder:reference/people")];
        validate_desired_home_folder_entities(&desired).unwrap();
    }

    #[test]
    fn desired_scan_folder_rejects_parent_traversal() {
        let home = TempDir::new().unwrap();
        let desired = desired_with_folder(home.path(), "../people");

        let error = validate_desired_home_folder_entities(&desired).unwrap_err();

        assert_eq!(error.code(), "workspace_desired_invalid");
        assert!(error.to_string().contains("must be relative"));
    }
}
