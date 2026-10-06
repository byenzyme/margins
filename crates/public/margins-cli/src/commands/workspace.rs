use crate::args::{GranolaTimeRangeArg, SourceKindArg, SourceRoleArg};
use crate::error::CliError;
use super::workspace_text;
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
use std::path::Path;

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
    let workspaces = workspace::inspect_workspace_entries(&home).map_err(CliError::from_anyhow)?;
    let default = workspace::default_workspace(&home).map_err(CliError::from_anyhow)?;
    if json {
        let entries = workspaces
            .iter()
            .map(|(id, resolved)| match resolved {
                Ok(item) => serde_json::json!({
                    "id": item.config.id, "name": item.config.name,
                }),
                Err(error) => serde_json::json!({
                    "id": id, "error": format!("{error:#}"),
                }),
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
        for (id, resolved) in workspaces {
            match resolved {
                Ok(item) => writeln!(
                    stdout,
                    "{}\t{}",
                    item.config.id,
                    item.config.name.as_deref().unwrap_or("")
                ),
                Err(error) => writeln!(stdout, "{id}\t(invalid: {error:#})"),
            }
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
    let resolved = inspect_existing(Some(&selected), cwd)?;
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
    let workspace = resolve_read_only(selector, cwd, stderr)?;
    render_workspace(&workspace, json, &BTreeMap::new(), stdout)
}

pub fn plan(
    selector: Option<&str>,
    cwd: &Path,
    desired_path: &Path,
    json: bool,
    color: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let selector = require_explicit_workspace(selector)?;
    let workspace = inspect_existing(Some(selector), cwd)?;
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
    let plan = plan_desired(&workspace, &body, legacy_toml)?;
    let mut bytes = serde_json::to_vec_pretty(&plan)
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    bytes.push(b'\n');
    if json {
        return stdout
            .write_all(&bytes)
            .map_err(|error| CliError::new("output_failed", error.to_string()));
    }
    write_plan_text(&workspace, &plan, None, &bytes, color, stdout)
}

/// Human `workspace plan`: save the exact plan JSON (what `--json` prints) for
/// `workspace apply --plan`, then describe it in plain language.
pub fn write_plan_text(
    workspace: &ResolvedWorkspace,
    plan: &WorkspacePlan,
    preset: Option<&workspace_text::PresetOutcome>,
    plan_json: &[u8],
    color: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    let noop = plan.actions.is_empty() && plan.diff.is_empty();
    let saved = if noop {
        None
    } else {
        let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
        let path = workspace_text::save_plan(&margins_home, &plan.workspace_id, plan_json)
            .map_err(|error| {
                CliError::new(
                    "workspace_plan_unwritable",
                    format!(
                        "could not save the plan in {}: {error}",
                        margins_home.join(workspace_text::PLANS_DIR).display()
                    ),
                )
            })?;
        Some(path)
    };
    workspace_text::write_plan(
        stdout,
        &workspace.config,
        &workspace.config_path,
        plan,
        preset,
        saved.as_deref(),
        color,
    )
    .map_err(output)
}

/// Plan a complete desired program (or legacy TOML config) against the
/// Workspace, refusing folder readings that do not resolve under Home.
fn plan_desired(
    workspace: &ResolvedWorkspace,
    body: &str,
    legacy_toml: bool,
) -> Result<WorkspacePlan, CliError> {
    let plan = if legacy_toml {
        // Older setup callers still write the retired config.toml shape.
        let desired: WorkspaceConfig = toml::from_str(body).map_err(|error| {
            CliError::new(
                "workspace_desired_invalid",
                format!("invalid desired workspace config: {error}"),
            )
        })?;
        workspace::plan_legacy_workspace_config(workspace, desired)
    } else {
        workspace::plan_workspace_program(workspace, body)
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
    Ok(plan)
}

/// Rename a Workspace whose id became reserved; see
/// [`workspace::rename_reserved_workspace`].
pub fn rename(old: &str, new: &str, json: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let renamed = workspace::rename_reserved_workspace(&home, old, new)
        .map_err(|error| CliError::new("workspace_rename_failed", format!("{error:#}")))?;
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    if json {
        serde_json::to_writer_pretty(&mut *stdout, &renamed)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        writeln!(stdout).map_err(output)
    } else {
        writeln!(
            stdout,
            "Renamed Workspace {old} to {new}: {}. The previous declaration is kept at {}.",
            renamed.program_path.display(),
            renamed.retired.display()
        )
        .map_err(output)?;
        writeln!(
            stdout,
            "Use --workspace {new} from now on; update any saved selection of '{old}' (for example in the bb plugin)."
        )
        .map_err(output)
    }
}

/// Test/harness-only: treat the process as an interactive terminal for
/// `workspace edit`, so a scripted `$EDITOR` and piped answers can drive it.
pub const EDIT_ASSUME_TERMINAL_ENV: &str = "MARGINS_WORKSPACE_EDIT_ASSUME_TERMINAL";

#[derive(Serialize)]
struct ProgramView<'a> {
    workspace_id: &'a str,
    program_path: String,
    revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    program: Option<&'a str>,
}

/// Print where the selected Workspace's program lives (or its text).
pub fn show(
    selector: Option<&str>,
    cwd: &Path,
    text: bool,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let workspace = inspect_existing(selector, cwd)?;
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    if json {
        let view = ProgramView {
            workspace_id: &workspace.config.id,
            program_path: workspace.config_path.to_string_lossy().into_owned(),
            revision: workspace.program.sha256(),
            program: text.then(|| workspace.program.text()),
        };
        serde_json::to_writer_pretty(&mut *stdout, &view)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        writeln!(stdout).map_err(output)
    } else if text {
        write!(stdout, "{}", workspace.program.text()).map_err(output)
    } else {
        // The path stays the only stdout line, for `$(margins workspace show)`.
        writeln!(stdout, "{}", workspace.config_path.display()).map_err(output)?;
        let id = &workspace.config.id;
        let margins = workspace_text::margins_for(id, workspace_text::is_machine_default(id));
        writeln!(
            stderr,
            "That file is the program for Workspace {id}. Read it with `{margins} workspace show --text`; change it with `{margins} workspace edit`."
        )
        .map_err(output)
    }
}

/// Open the Workspace program in `$VISUAL`/`$EDITOR` on a copy, then show the
/// change and apply it through the same plan/apply path as `workspace plan`
/// and `workspace apply`. An edit that does not validate is never applied;
/// the user's text stays in a kept file they can reopen or plan from.
pub fn edit(
    selector: Option<&str>,
    cwd: &Path,
    color: bool,
    stdin: &mut dyn std::io::BufRead,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    use std::io::IsTerminal;

    let assume_terminal = std::env::var_os(EDIT_ASSUME_TERMINAL_ENV).is_some_and(|value| value == "1");
    if !assume_terminal && !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        let margins = match inspect_existing(selector, cwd) {
            Ok(workspace) => {
                let id = &workspace.config.id;
                workspace_text::margins_for(id, workspace_text::is_machine_default(id))
            }
            Err(_) => "margins --workspace <id>".to_string(),
        };
        return Err(CliError::new(
            "workspace_edit_requires_terminal",
            format!(
                "workspace edit opens an editor and asks before applying, so it needs an interactive terminal. \
                 Without one: print the program with `{margins} workspace show --text`, \
                 save a changed copy as program.enzyme, review it with \
                 `{margins} workspace plan --desired program.enzyme`, \
                 then run the `margins workspace apply --plan …` command it prints."
            ),
        ));
    }
    let editor = editor_command().ok_or_else(|| {
        CliError::new(
            "workspace_edit_no_editor",
            "set $VISUAL or $EDITOR to the editor to open the Workspace program with",
        )
    })?;
    let mut workspace = resolve_existing(selector, cwd)?;
    let id = workspace.config.id.clone();
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    let original = workspace.program.text().to_string();
    let (_, edit_path) = tempfile::Builder::new()
        .prefix(&format!("margins-{id}-"))
        .suffix(".enzyme")
        .tempfile()
        .and_then(|file| file.keep().map_err(|error| error.error))
        .map_err(|error| CliError::new("workspace_edit_failed", format!("creating an edit file: {error}")))?;
    std::fs::write(&edit_path, &original).map_err(|error| {
        CliError::new("workspace_edit_failed", format!("writing {}: {error}", edit_path.display()))
    })?;
    let margins = workspace_text::margins_for(&id, workspace_text::is_machine_default(&id));
    let kept = |message: String| {
        CliError::new(
            "workspace_edit_not_applied",
            format!(
                "{message}\nYour edit is kept at {path}. Reopen it with your editor, then review it with\n  \
                 {margins} workspace plan --desired {path}\n\
                 and run the apply command it prints, or start over with `{margins} workspace edit`.",
                path = edit_path.display()
            ),
        )
    };
    writeln!(stderr, "Editing Workspace {id} ({})", workspace.config_path.display()).map_err(output)?;
    loop {
        run_editor(&editor, &edit_path)
            .map_err(|error| kept(format!("the editor did not finish: {error:#}")))?;
        let edited = std::fs::read_to_string(&edit_path)
            .map_err(|error| kept(format!("could not read the edit: {error}")))?;
        if edited == original {
            let _ = std::fs::remove_file(&edit_path);
            writeln!(stdout, "No changes to Workspace {id}.").map_err(output)?;
            return Ok(());
        }
        let plan = match plan_desired(&workspace, &edited, false) {
            Ok(plan) => plan,
            Err(error) => {
                writeln!(stderr, "The edited program is not valid: {}", error.message()).map_err(output)?;
                if ask(stdin, stderr, "Reopen the editor to fix it? [Y/n] ", true).map_err(output)? {
                    continue;
                }
                return Err(kept(format!("The edited program is not valid: {}", error.message())));
            }
        };
        for action in &plan.actions {
            for summary in workspace_text::plain_summaries(action) {
                writeln!(stdout, "  • {summary}").map_err(output)?;
            }
        }
        workspace_text::write_diff(stdout, &plan.diff, color).map_err(output)?;
        if !ask(stdin, stderr, &format!("Apply this change to Workspace {id}? [y/N] "), false)
            .map_err(output)?
        {
            return Err(kept("Not applied.".to_string()));
        }
        let receipt = workspace::apply_workspace_plan(&mut workspace, &plan).map_err(|error| {
            kept(format!("The change could not be applied: {error:#}"))
        })?;
        let _ = std::fs::remove_file(&edit_path);
        writeln!(
            stdout,
            "Applied. Workspace {id} is at revision {}.",
            receipt.after_revision
        )
        .map_err(output)?;
        return Ok(());
    }
}

fn editor_command() -> Option<String> {
    ["VISUAL", "EDITOR"].into_iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Run the editor command with the file as its last argument. The command is
/// interpreted by the shell, so values such as `code --wait` work.
fn run_editor(editor: &str, path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg("sh")
        .arg(path)
        .status();
    #[cfg(not(unix))]
    let status = {
        let mut parts = editor.split_whitespace();
        std::process::Command::new(parts.next().unwrap_or(editor))
            .args(parts)
            .arg(path)
            .status()
    };
    let status = status.with_context(|| format!("starting editor `{editor}`"))?;
    if !status.success() {
        anyhow::bail!("editor `{editor}` exited with {status}");
    }
    Ok(())
}

/// Ask a yes/no question on `stderr`; an empty answer takes the default and
/// end of input is always "no", so piped input can never loop.
fn ask(
    stdin: &mut dyn std::io::BufRead,
    stderr: &mut dyn Write,
    question: &str,
    default: bool,
) -> std::io::Result<bool> {
    write!(stderr, "{question}")?;
    stderr.flush()?;
    let mut answer = String::new();
    if stdin.read_line(&mut answer)? == 0 {
        writeln!(stderr)?;
        return Ok(false);
    }
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    })
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

/// Folder readings name folders under the Workspace home. Refuse a plan that
/// cannot resolve one of them before the user reviews and
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
            let resolved = margins_workflows::workspace_preset::resolve_home_folder(home, folder)
                .map_err(|error| {
                    CliError::new("workspace_desired_invalid", format!("{error:#}"))
                })?;
            if resolved.is_none() {
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

pub fn apply(
    selector: Option<&str>,
    cwd: &Path,
    plan_path: &Path,
    json: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
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
    // The plan names its Workspace; an explicit `--workspace` must agree.
    if let Some(selector) = selector.filter(|selector| *selector != plan.workspace_id) {
        return Err(CliError::new(
            "workspace_mismatch",
            format!(
                "plan workspace '{}' does not match explicit workspace '{}'",
                plan.workspace_id, selector
            ),
        ));
    }
    let mut workspace = resolve_existing(Some(&plan.workspace_id), cwd)?;
    let receipt =
        workspace::apply_workspace_plan(&mut workspace, &plan).map_err(CliError::from_anyhow)?;
    if !json {
        let is_default = workspace_text::is_machine_default(&receipt.workspace_id);
        return workspace_text::write_applied(stdout, &receipt, &workspace.config_path, is_default)
            .map_err(|error| CliError::new("output_failed", error.to_string()));
    }
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
    let output = |error: std::io::Error| CliError::from_anyhow(error.into());
    let mut failed = Vec::new();
    for id in ids {
        let migration = match if dry_run {
            workspace::preview_workspace_migration(&home, &id)
        } else {
            workspace::migrate_workspace(&home, &id)
        } {
            Ok(migration) => migration,
            Err(error) => {
                // One Workspace that cannot migrate must not block the others;
                // its legacy file stays in place.
                if json {
                    serde_json::to_writer(
                        &mut *stdout,
                        &serde_json::json!({
                            "schema_version": workspace::WORKSPACE_MIGRATE_SCHEMA,
                            "workspace_id": id, "status": "failed", "error": format!("{error:#}"),
                        }),
                    )
                    .map_err(|error| CliError::from_anyhow(error.into()))?;
                    writeln!(stdout).map_err(output)?;
                } else {
                    writeln!(stdout, "{id}: failed: {error:#}").map_err(output)?;
                }
                failed.push((id, error));
                continue;
            }
        };
        if json {
            serde_json::to_writer(&mut *stdout, &migration)
                .map_err(|error| CliError::from_anyhow(error.into()))?;
            writeln!(stdout).map_err(output)?;
        } else {
            writeln!(
                stdout,
                "{}: {} -> {}",
                migration.workspace_id,
                migration.status,
                migration.program_path.display()
            )
            .map_err(output)?;
            for warning in &migration.warnings {
                writeln!(stdout, "  warning: {warning}").map_err(output)?;
            }
            if dry_run {
                write!(stdout, "{}", migration.program).map_err(output)?;
            }
        }
    }
    match failed.len() {
        0 => Ok(()),
        1 => Err(CliError::from_anyhow(failed.remove(0).1)),
        n => Err(CliError::new(
            "workspace_migration_failed",
            format!(
                "{n} Workspaces could not migrate: {}",
                failed.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
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
    let workspace = resolve_read_only(selector, cwd, stderr)?;
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

/// Why a read-like command found no Workspace; nothing was created.
pub const NO_WORKSPACE_MESSAGE: &str = "No Margins Workspace covers this folder, and Margins did not create one. \
Pick one with `margins --workspace <id> …` (`margins workspace list` shows them; \
`margins workspace default --set <id>` makes one the default), or create one for your notes with \
`margins workspace new <id> --home /path/to/notes`.";

/// Resolution for read-only commands (recall, status, source list,
/// integrations status): never creates or migrates a Workspace. Uses the
/// selected one, else the one declaring `cwd`, else the machine default
/// (announced on `stderr`), and otherwise fails with `workspace_required`.
/// Only `margins init` establishes a new Workspace for the current folder.
pub fn resolve_read_only(
    selector: Option<&str>,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    announce(
        workspace::inspect_workspace_or_default(&margins_home, selector, cwd)
            .map_err(CliError::from_anyhow)?,
        stderr,
    )
}

/// [`resolve_read_only`] for commands that write (sync, imports, reconcile):
/// the Workspace is migrated as needed, like `resolve_at`, but never created.
pub fn resolve_for_write(
    selector: Option<&str>,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    announce(
        workspace::resolve_workspace_or_default(&margins_home, selector, cwd)
            .map_err(CliError::from_anyhow)?,
        stderr,
    )
}

fn announce(
    selected: Option<workspace::SelectedWorkspace>,
    stderr: &mut dyn Write,
) -> Result<ResolvedWorkspace, CliError> {
    let selected =
        selected.ok_or_else(|| CliError::new("workspace_required", NO_WORKSPACE_MESSAGE))?;
    if selected.via_default {
        writeln!(stderr, "Using Workspace {} (default)", selected.workspace.config.id)
            .map_err(|error| CliError::from_anyhow(error.into()))?;
    }
    Ok(selected.workspace)
}

pub fn resolve_existing(selector: Option<&str>, cwd: &Path) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    workspace::resolve_workspace(&margins_home, selector, cwd).map_err(CliError::from_anyhow)
}

/// [`resolve`] for read-only commands: an existing Workspace is inspected
/// without migrating or writing anything (only creating a new implicit
/// Workspace writes).
pub fn inspect(
    selector: Option<&str>,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let roots = workspace::ImplicitWorkspaceRoots::from_process(&margins_home)
        .map_err(CliError::from_anyhow)?;
    let resolution = workspace::inspect_or_create_workspace(&roots, selector, cwd)
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

/// [`resolve_existing`] for read-only commands; writes nothing.
pub fn inspect_existing(selector: Option<&str>, cwd: &Path) -> Result<ResolvedWorkspace, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    workspace::inspect_workspace(&margins_home, selector, cwd).map_err(CliError::from_anyhow)
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
        writeln!(stdout, "Program: {}", view.config)
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
        .map_err(|error| CliError::from_anyhow(error.into()))?;
        let margins = workspace_text::margins_for(view.id, workspace_text::is_machine_default(view.id));
        writeln!(
            stdout,
            "Read the program with `{margins} workspace show --text`; change it with `{margins} workspace edit`."
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
        writeln!(stdout, "Program: {}", view.config)
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
        .map_err(|error| CliError::from_anyhow(error.into()))?;
        let margins = workspace_text::margins_for(view.id, workspace_text::is_machine_default(view.id));
        writeln!(
            stdout,
            "Read the program with `{margins} workspace show --text`; change it with `{margins} workspace edit`."
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
