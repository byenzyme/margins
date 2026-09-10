//! Standalone public parser and command dispatcher for Margins.

#![forbid(unsafe_code)]

pub mod args;
pub mod build_info;
pub mod commands;
pub mod error;
pub mod output;
pub mod services;
pub mod vault_guard;

pub use error::CliError;
pub use services::{
    standalone_services, CliServices, Clock, LocalSessionStore, ProcessRunner, ProjectService,
    SessionStore, SystemClock, SystemProcessRunner, SystemProjectService,
};

use args::{
    AgentsCommand, ArchiveCommand, Args, Command, ConnectCommand, ConnectionServiceArg,
    DisconnectCommand, GuideCommand, ImportCommand, IntegrationsCommand, RetentionCommand,
    SetupLocalModelPolicyArg, SourceCommand, WorkspaceCommand,
};
use clap::Parser;
use commands::projects::absolute_from;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn run<I, T>(
    services: &CliServices,
    invocation_dir: &Path,
    args: I,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let json_errors = args.iter().any(|value| value == OsStr::new("--json"));
    let result = run_inner(services, invocation_dir, args, stdout, stderr);
    if let Err(error) = &result {
        if !error.is_reported() {
            let _ = if json_errors {
                output::write_json_error(stderr, error)
            } else {
                output::write_error(stderr, error)
            };
        }
    }
    result
}

fn run_inner(
    services: &CliServices,
    invocation_dir: &Path,
    args: Vec<OsString>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let (workspace_selector, args) = args::strip_workspace_arg(args).map_err(CliError::usage)?;
    let (project_selector, args) = args::strip_project_arg(args).map_err(CliError::usage)?;
    let args = Args::try_parse_from(args).map_err(|error| CliError::usage(error.to_string()))?;
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let workspace_selected = workspace_selector.is_some() || env_workspace.is_some();

    match args.command {
        Some(Command::Retention { command }) => {
            let selector =
                commands::workspace::require_explicit_workspace(workspace_selector.as_deref())?;
            let workspace = commands::workspace::resolve_existing(Some(selector), invocation_dir)?;
            return match command {
                RetentionCommand::Preview {
                    connector,
                    account,
                    scope,
                    json,
                } => {
                    debug_assert!(json);
                    commands::retention::preview(&workspace, connector, &account, scope, stdout)
                }
                RetentionCommand::Apply {
                    plan,
                    if_revision,
                    request_id,
                    json,
                } => {
                    debug_assert!(json);
                    commands::retention::apply(
                        &workspace,
                        &absolute_from(invocation_dir, &plan),
                        &if_revision,
                        &request_id,
                        stdout,
                    )
                }
            };
        }
        Some(Command::Integrations { command }) => {
            if matches!(command, IntegrationsCommand::Reconcile { .. })
                && workspace_selector
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Err(CliError::usage(
                    "integrations reconcile requires literal --workspace <id>",
                ));
            }
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return match command {
                IntegrationsCommand::Reconcile {
                    connector,
                    account,
                    if_revision,
                    request_id,
                    json,
                } => {
                    debug_assert!(json);
                    commands::integrations::reconcile(
                        &workspace.state_dir,
                        connector.as_deref(),
                        account.as_deref(),
                        &if_revision,
                        &request_id,
                        stdout,
                    )
                }
                IntegrationsCommand::Status { json } => {
                    commands::integrations::status(&workspace.state_dir, json, stdout)
                }
            };
        }
        Some(Command::Workspace {
            command:
                WorkspaceCommand::New {
                    id,
                    home,
                    name,
                    json,
                },
        }) => {
            let home = absolute_from(invocation_dir, &home);
            return commands::workspace::new(&id, name.as_deref(), &home, json, stdout);
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Status { json },
        }) => {
            return commands::workspace::status(
                workspace_selector.as_deref(),
                invocation_dir,
                json,
                stdout,
                stderr,
            );
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Plan { desired, .. },
        }) => {
            return commands::workspace::plan(
                workspace_selector.as_deref(),
                invocation_dir,
                &absolute_from(invocation_dir, &desired),
                stdout,
            );
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Apply { plan, .. },
        }) => {
            return commands::workspace::apply(
                workspace_selector.as_deref(),
                invocation_dir,
                &absolute_from(invocation_dir, &plan),
                stdout,
            );
        }
        Some(Command::Source {
            command:
                SourceCommand::Add {
                    kind,
                    name,
                    path,
                    role,
                    account,
                    query,
                    backfill_days,
                    lookback_days,
                    lookahead_days,
                    time_range,
                    json,
                },
        }) => {
            return commands::workspace::add_source(
                workspace_selector.as_deref(),
                invocation_dir,
                &name,
                kind,
                path.as_deref(),
                role,
                account.as_deref(),
                query.as_deref(),
                backfill_days,
                lookback_days,
                lookahead_days,
                time_range,
                json,
                stdout,
                stderr,
            );
        }
        Some(Command::Source {
            command: SourceCommand::List { json },
        }) => {
            return commands::workspace::list_sources(
                workspace_selector.as_deref(),
                invocation_dir,
                json,
                stdout,
                stderr,
            );
        }
        Some(Command::Source {
            command: SourceCommand::Remove { name, json },
        }) => {
            return commands::workspace::remove_source(
                workspace_selector.as_deref(),
                invocation_dir,
                &name,
                json,
                stdout,
                stderr,
            );
        }
        // A Workspace owns both recall state and captured-session state. Keep
        // read-only session resolution on that same selector: removing native
        // capture from a build must not turn `latest` into a lookup against an
        // unrelated legacy project directory.
        Some(Command::Recent { all }) if workspace_selected => {
            if all {
                return Err(CliError::usage(
                    "`recent --all` is a cross-project discovery command and cannot be combined with a Workspace selection",
                ));
            }
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::transcript::recent(&workspace.captures_dir(), stdout);
        }
        Some(Command::Transcript { meeting_id, format }) if workspace_selected => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::transcript::transcript(
                &workspace.captures_dir(),
                meeting_id.as_deref().unwrap_or("latest"),
                format,
                stdout,
            );
        }
        Some(Command::Artifacts { meeting_id }) if workspace_selected => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::artifacts::list(&workspace.captures_dir(), &meeting_id, stdout);
        }
        Some(Command::Import {
            command: ImportCommand::Granola { path },
        }) => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            let project = workspace_project_adapter(&workspace);
            return commands::import::granola(
                &workspace.state_dir,
                &project,
                &absolute_from(invocation_dir, &path),
                stdout,
            );
        }
        Some(Command::Init) => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::recall::init(&workspace, stdout);
        }
        Some(Command::Setup {
            only,
            skip,
            local_model,
        }) => {
            if !only.is_empty()
                || skip.is_some()
                || local_model != SetupLocalModelPolicyArg::Fallback
            {
                return Err(CliError::unavailable(
                    "setup_option_unavailable",
                    "setup selection flags require the official Margins composition; public setup uses the single workspace guide",
                ));
            }
            let env_workspace = std::env::var("MARGINS_WORKSPACE")
                .ok()
                .filter(|value| !value.trim().is_empty());
            let workspace = workspace_selector.as_deref().or(env_workspace.as_deref());
            return commands::guide::setup_handoff(workspace, stdout);
        }
        Some(Command::Guide {
            command: GuideCommand::WorkspaceSetup,
        }) => {
            return commands::guide::workspace_setup(stdout);
        }
        Some(Command::Guide {
            command: GuideCommand::Onboarding,
        }) => {
            return commands::guide::onboarding(stdout);
        }
        Some(Command::Capabilities) => {
            return commands::capabilities::public(stdout);
        }
        Some(Command::Connect {
            command: ConnectCommand::Google { .. },
        }) => {
            return Err(CliError::unavailable(
                "google_connect_unavailable",
                "Google connect requires the official Margins build.",
            ));
        }
        Some(Command::Connect {
            command:
                ConnectCommand::Granola {
                    account,
                    headless,
                    json,
                },
        }) => {
            let home =
                margins_workflows::workspace::margins_home().map_err(CliError::from_anyhow)?;
            return commands::connect::granola(
                &home,
                account.as_deref(),
                headless,
                json,
                stdout,
                stderr,
            );
        }
        Some(Command::Connect {
            command: ConnectCommand::Status { service, json },
        }) => {
            let home =
                margins_workflows::workspace::margins_home().map_err(CliError::from_anyhow)?;
            return match service {
                Some(ConnectionServiceArg::Granola) => {
                    commands::connect::granola_status(&home, json, stdout)
                }
                Some(ConnectionServiceArg::Google) | None => {
                    commands::connect::status(&home, json, stdout)
                }
            };
        }
        Some(Command::Disconnect {
            command: DisconnectCommand::Google { account, json },
        }) => {
            if workspace_selector.is_some() {
                return Err(CliError::usage(
                    "`disconnect google` is machine-scoped and does not accept `--workspace`; use `source remove NAME` to remove a Workspace source declaration",
                ));
            }
            let home =
                margins_workflows::workspace::margins_home().map_err(CliError::from_anyhow)?;
            return commands::connect::forget(&home, &account, json, stdout);
        }
        Some(Command::Disconnect {
            command: DisconnectCommand::Granola { account, json },
        }) => {
            if workspace_selector.is_some() {
                return Err(CliError::usage(
                    "`disconnect granola` is machine-scoped and does not accept `--workspace`; use `source remove NAME` to remove a Workspace source declaration",
                ));
            }
            let home =
                margins_workflows::workspace::margins_home().map_err(CliError::from_anyhow)?;
            return commands::connect::forget_granola(&home, &account, json, stdout);
        }
        Some(Command::Recall { query, source }) => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::recall::run(&workspace, &query, source.as_deref(), stdout);
        }
        Some(Command::Sync { source, json }) => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::recall::sync(&workspace, source.as_deref(), json, stdout);
        }
        Some(Command::Transcribe {
            audio_path,
            name,
            memo,
            speakers,
        }) if workspace_selected => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            let audio_path = absolute_from(invocation_dir, &audio_path);
            let memo_path = memo
                .as_deref()
                .map(|path| absolute_from(invocation_dir, path));
            let started_at = audio_start_time(&audio_path).unwrap_or_else(|| services.clock.now());
            return commands::process::transcribe(
                services,
                &workspace.captures_dir(),
                &audio_path,
                name.as_deref(),
                memo_path.as_deref(),
                speakers.unwrap_or(1),
                started_at,
                stdout,
            );
        }
        Some(Command::Scan) => {
            return Err(CliError::new(
                "composition_unavailable",
                "This public development CLI cannot scan a Margins recall workspace. Install the official Margins CLI (`./install.sh` or a release artifact).",
            ));
        }
        Some(Command::Note { print }) => {
            let workspace = workspace_selector.as_deref().or(env_workspace.as_deref());
            return commands::guide::note_handoff(workspace, print, stdout);
        }
        _ => {}
    }

    let project = services
        .projects
        .resolve_vault(project_selector.as_deref(), invocation_dir)
        .map_err(CliError::from_anyhow)?;
    if matches!(args.command, Some(Command::Integrations { .. })) {
        vault_guard::require_evidenced_vault(&project)?;
    }
    let project = match &args.command {
        Some(Command::Transcript { meeting_id, .. }) => resolve_meeting_owner(
            services,
            project,
            project_selector.is_some(),
            meeting_id.as_deref().unwrap_or("latest"),
        )?,
        Some(Command::Artifacts { meeting_id }) => {
            resolve_meeting_owner(services, project, project_selector.is_some(), meeting_id)?
        }
        _ => project,
    };
    let work_dir = &project.work_dir;
    match args.command {
        Some(Command::Connect { .. }) | Some(Command::Disconnect { .. }) => {
            unreachable!("connection commands return before vault resolution")
        }
        None => commands::capture::run(services, work_dir, None, None, false, true),
        Some(Command::New { title }) => {
            commands::capture::run(services, work_dir, None, title.as_deref(), true, false)
        }
        Some(Command::Attach { session }) => {
            commands::capture::run(services, work_dir, session.as_deref(), None, false, false)
        }
        Some(Command::Current) => commands::sessions::show_current(services, work_dir, stdout),
        Some(Command::Ls) => commands::sessions::list(services, work_dir, stderr),
        Some(Command::Rename { title }) => {
            commands::sessions::rename(services, work_dir, &title, stdout)
        }
        Some(Command::Recent { all }) => {
            if all {
                let vaults = services.projects.list().map_err(CliError::from_anyhow)?;
                commands::transcript::recent_all(&vaults, stdout)
            } else {
                commands::transcript::recent(work_dir, stdout)
            }
        }
        Some(Command::Transcript { meeting_id, format }) => commands::transcript::transcript(
            work_dir,
            meeting_id.as_deref().unwrap_or("latest"),
            format,
            stdout,
        ),
        Some(Command::Integrations { command }) => match command {
            IntegrationsCommand::Reconcile {
                connector,
                account,
                if_revision,
                request_id,
                json,
            } => {
                debug_assert!(json);
                commands::integrations::reconcile(
                    work_dir,
                    connector.as_deref(),
                    account.as_deref(),
                    &if_revision,
                    &request_id,
                    stdout,
                )
            }
            IntegrationsCommand::Status { json } => {
                commands::integrations::status(work_dir, json, stdout)
            }
        },
        Some(Command::Artifacts { meeting_id }) => {
            commands::artifacts::list(work_dir, &meeting_id, stdout)
        }
        Some(Command::ArtifactsPrune) => {
            commands::artifacts::prune(work_dir, services.clock.now(), stdout)
        }
        Some(Command::Archive { command }) => match command {
            ArchiveCommand::On => commands::archive::set(work_dir, true, stdout),
            ArchiveCommand::Off => commands::archive::set(work_dir, false, stdout),
            ArchiveCommand::Status => commands::archive::status(work_dir, stdout),
        },
        Some(Command::Transcribe {
            audio_path,
            name,
            memo,
            speakers,
        }) => {
            let audio_path = absolute_from(invocation_dir, &audio_path);
            let memo_path = memo
                .as_deref()
                .map(|path| absolute_from(invocation_dir, path));
            let started_at = audio_start_time(&audio_path).unwrap_or_else(|| services.clock.now());
            commands::process::transcribe(
                services,
                work_dir,
                &audio_path,
                name.as_deref(),
                memo_path.as_deref(),
                speakers.unwrap_or(1),
                started_at,
                stdout,
            )
        }
        Some(Command::Process {
            session,
            speakers,
            align_only,
        }) => commands::process::process_session(
            services,
            work_dir,
            &session,
            speakers.unwrap_or(1),
            align_only,
            stdout,
        ),
        Some(Command::Import { .. }) => unreachable!("imports return after workspace resolution"),
        Some(Command::Agents { command }) => match command {
            AgentsCommand::Install => commands::projects::install_agents(work_dir, stdout),
        },
        Some(Command::Recall { .. }) => unreachable!("handled before project resolution"),
        Some(Command::Sync { .. }) => unreachable!("handled before project resolution"),
        Some(Command::Scan) => unreachable!("handled before project resolution"),
        Some(Command::Capabilities) => unreachable!("handled before project resolution"),
        Some(Command::Init) => unreachable!("handled before project resolution"),
        Some(Command::Note { .. }) => unreachable!("handled before project resolution"),
        Some(Command::Setup { .. }) | Some(Command::Guide { .. }) => {
            unreachable!("handled before project resolution")
        }
        Some(Command::Workspace { .. })
        | Some(Command::Source { .. })
        | Some(Command::Retention { .. }) => {
            unreachable!("handled before project resolution")
        }
    }
}

fn workspace_project_adapter(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
) -> margins_workflows::project::ResolvedProject {
    use margins_workflows::project::{ProjectSource, ResolvedProject};
    ResolvedProject {
        project: ProjectSource {
            id: workspace.config.id.clone(),
            name: workspace
                .config
                .name
                .clone()
                .unwrap_or_else(|| workspace.config.id.clone()),
            path: workspace.home_dir.to_string_lossy().into_owned(),
            inbox_folder: "Meetings".to_string(),
            people_folder: "People".to_string(),
            readiness: "ready".to_string(),
        },
        root_dir: workspace.home_dir.clone(),
        work_dir: workspace.state_dir.clone(),
    }
}

/// Resolve a concrete meeting id across registered vaults for read-only
/// inspection commands. Explicit project routing and vault-relative aliases
/// stay scoped to the initially resolved vault.
fn resolve_meeting_owner(
    services: &CliServices,
    initial: margins_workflows::project::ResolvedProject,
    has_explicit_project: bool,
    meeting_id: &str,
) -> Result<margins_workflows::project::ResolvedProject, CliError> {
    if has_explicit_project || meeting_id == "latest" {
        return Ok(initial);
    }

    let mut vaults = services.projects.list().map_err(CliError::from_anyhow)?;
    vaults.push(initial.clone());
    let mut seen = Vec::<PathBuf>::new();
    let mut owners = Vec::new();
    for vault in vaults {
        let root = vault
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| vault.root_dir.clone());
        if seen.iter().any(|seen_root| seen_root == &root) {
            continue;
        }
        seen.push(root);
        let margins_dir = vault.work_dir.join(".margins");
        if !margins_dir.is_dir() {
            continue;
        }
        let exists = services
            .sessions
            .exists(&margins_dir, meeting_id)
            .map_err(CliError::from_anyhow)?;
        if exists {
            owners.push(vault);
        }
    }

    match owners.len() {
        0 => Ok(initial),
        1 => Ok(owners.remove(0)),
        _ => {
            let candidates = owners
                .iter()
                .map(|vault| {
                    format!(
                        "{} ({})",
                        vault.project.id,
                        vault.root_dir.to_string_lossy()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::new(
                "ambiguous_meeting",
                format!(
                    "Meeting '{meeting_id}' exists in multiple Margins vaults: {candidates}. Pass --project <id-or-path> to choose one."
                ),
            ))
        }
    }
}

fn audio_start_time(path: &Path) -> Option<chrono::DateTime<chrono::Local>> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.created())
        .or_else(|_| std::fs::metadata(path).and_then(|metadata| metadata.modified()))
        .ok()
        .map(chrono::DateTime::<chrono::Local>::from)
}

/// Process entrypoint shared by the standalone binary and transitional private
/// composition binaries.
pub fn main_entry<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let services = standalone_services();
    let invocation_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "<margins_error code=\"command_failed\">{}</margins_error>",
                output::xml_escape_text(&error.to_string())
            );
            return 1;
        }
    };
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    match run(&services, &invocation_dir, args, &mut stdout, &mut stderr) {
        Ok(()) => 0,
        Err(error) => error.exit_code(),
    }
}

pub fn main_entry_from_env() -> i32 {
    main_entry(std::env::args_os())
}

#[allow(dead_code)]
fn _os_str_is_public(_: &OsStr) {}
