//! Standalone public parser and command dispatcher for Margins.

#![forbid(unsafe_code)]

pub mod args;
pub mod build_info;
pub mod commands;
pub mod error;
pub mod logging;
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
use commands::capture_target::{CaptureIntent, CaptureTarget};
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
    let mut args =
        Args::try_parse_from(args).map_err(|error| CliError::usage(error.to_string()))?;
    if args.version {
        return writeln!(
            stdout,
            "{}",
            build_info::version_line(env!("CARGO_PKG_VERSION"), "public")
        )
        .map_err(|error| CliError::from_anyhow(error.into()));
    }
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let workspace_selected = workspace_selector.is_some() || env_workspace.is_some();
    if workspace_selected && project_selector.is_some() {
        return Err(CliError::new(
            "invalid_arguments",
            "`--project` cannot be combined with an explicit Workspace selection",
        ));
    }

    let remote = if args.local {
        None
    } else {
        args.remote.clone().or_else(|| {
            std::env::var("MARGINS_REMOTE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    };

    if matches!(args.command, Some(Command::Service { .. })) {
        let Some(Command::Service { command }) = args.command.take() else {
            unreachable!()
        };
        return commands::remote::service(command, workspace_selector.as_deref(), stdout);
    }
    if matches!(args.command, Some(Command::Transfers { .. })) {
        let Some(Command::Transfers { command }) = args.command.take() else {
            unreachable!()
        };
        return commands::remote::transfers(command, stdout);
    }
    if let Some(remote) = remote {
        let workspace = workspace_selector
            .as_deref()
            .or(env_workspace.as_deref())
            .ok_or_else(|| {
                CliError::usage("remote commands require --workspace <id> or MARGINS_WORKSPACE")
            })?;
        let command = args.command.ok_or_else(|| {
            CliError::new(
                "remote_command_unsupported",
                "bare remote capture is not available; use `margins new`",
            )
        })?;
        return commands::remote::run(&remote, workspace, command, stdout);
    }

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
            let workspace = if matches!(command, IntegrationsCommand::Reconcile { .. }) {
                commands::workspace::resolve_for_write(
                    workspace_selector.as_deref(),
                    invocation_dir,
                    stderr,
                )?
            } else {
                commands::workspace::resolve_read_only(
                    workspace_selector.as_deref(),
                    invocation_dir,
                    stderr,
                )?
            };
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
            command: WorkspaceCommand::Remove { id, json },
        }) => {
            return commands::workspace::remove(&id, json, stdout);
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::List { json },
        }) => {
            return commands::workspace::list(json, stdout);
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Default { set, json },
        }) => {
            return commands::workspace::default(set.as_deref(), json, stdout);
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Destination { .. },
        }) => {
            return commands::workspace::destination(
                workspace_selector.as_deref(),
                invocation_dir,
                stdout,
            );
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
            command: WorkspaceCommand::Rename { old, new, json },
        }) => {
            return commands::workspace::rename(&old, &new, json, stdout);
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Show { text, json },
        }) => {
            return commands::workspace::show(
                workspace_selector.as_deref(),
                invocation_dir,
                text,
                json,
                stdout,
                stderr,
            );
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Edit { color },
        }) => {
            use std::io::IsTerminal;
            return commands::workspace::edit(
                workspace_selector.as_deref(),
                invocation_dir,
                color.enabled(std::io::stdout().is_terminal()),
                &mut std::io::stdin().lock(),
                stdout,
                stderr,
            );
        }
        Some(Command::Workspace {
            command:
                WorkspaceCommand::Plan {
                    desired: Some(desired),
                    json,
                    color,
                    ..
                },
        }) => {
            use std::io::IsTerminal;
            return commands::workspace::plan(
                workspace_selector.as_deref(),
                invocation_dir,
                &absolute_from(invocation_dir, &desired),
                json,
                color.enabled(std::io::stdout().is_terminal()),
                stdout,
            );
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Plan { desired: None, .. },
        }) => {
            return Err(CliError::new(
                "composition_unavailable",
                "this build cannot fill presets; install the official Margins CLI, which runs the enzyme engine",
            ));
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Migrate { dry_run, json },
        }) => {
            return commands::workspace::migrate(
                workspace_selector.as_deref(),
                dry_run,
                json,
                stdout,
            );
        }
        Some(Command::Workspace {
            command: WorkspaceCommand::Apply { plan, json },
        }) => {
            return commands::workspace::apply(
                workspace_selector.as_deref(),
                invocation_dir,
                &absolute_from(invocation_dir, &plan),
                json,
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
        Some(Command::Import {
            command: ImportCommand::Granola { path },
        }) => {
            // Importing writes notes; it never creates a Workspace to hold them.
            let workspace = commands::workspace::resolve_for_write(
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
        Some(Command::Guide {
            command: GuideCommand::Glossary,
        }) => {
            return commands::guide::glossary(stdout);
        }
        Some(Command::Enzyme { .. }) => {
            return Err(CliError::new(
                "composition_unavailable",
                "this build has no bundled enzyme engine; install the official Margins CLI",
            ));
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
        Some(Command::Recall {
            query,
            source,
            json,
        }) => {
            let workspace = commands::workspace::resolve_read_only(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::recall::run(&workspace, &query, source.as_deref(), json, stdout);
        }
        Some(Command::Sync { source, json }) => {
            let workspace = commands::workspace::resolve_for_write(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            return commands::recall::sync(&workspace, source.as_deref(), json, stdout);
        }
        Some(Command::Note { print }) => {
            let workspace = workspace_selector.as_deref().or(env_workspace.as_deref());
            return commands::guide::note_handoff(workspace, print, stdout);
        }
        _ => {}
    }

    if let Some(Command::Recent { all: true }) = &args.command {
        if workspace_selected {
            return Err(CliError::usage(
                "`recent --all` is a cross-project discovery command and cannot be combined with a Workspace selection",
            ));
        }
        let vaults = services.projects.list().map_err(CliError::from_anyhow)?;
        return commands::transcript::recent_all(&vaults, stdout);
    }

    if let Some(intent) = capture_intent(&args.command) {
        let target = commands::capture_target::resolve(
            services,
            workspace_selector.as_deref(),
            project_selector.as_deref(),
            invocation_dir,
            intent,
            stderr,
        )?;
        let root = match target {
            CaptureTarget::Workspace {
                capture_root,
                legacy,
                ..
            } => {
                // A concrete meeting id the Workspace lacks may predate it.
                let meeting_id = match &args.command {
                    Some(Command::Transcript { meeting_id, .. })
                    | Some(Command::AudioExport { meeting_id }) => meeting_id.as_deref(),
                    Some(Command::Artifacts { meeting_id }) => Some(meeting_id.as_str()),
                    _ => None,
                }
                .filter(|id| *id != "latest");
                match (meeting_id, legacy) {
                    (Some(id), Some(project))
                        if !session_exists(services, &capture_root, id) =>
                    {
                        let owner = resolve_meeting_owner(services, project, false, id)?;
                        if session_exists(services, &owner.work_dir, id) {
                            owner.work_dir
                        } else {
                            capture_root
                        }
                    }
                    _ => capture_root,
                }
            }
            CaptureTarget::Legacy(project) => {
                let project = match &args.command {
                    Some(Command::Transcript { meeting_id, .. })
                    | Some(Command::AudioExport { meeting_id }) => resolve_meeting_owner(
                        services,
                        project,
                        project_selector.is_some(),
                        meeting_id.as_deref().unwrap_or("latest"),
                    )?,
                    Some(Command::Artifacts { meeting_id }) => resolve_meeting_owner(
                        services,
                        project,
                        project_selector.is_some(),
                        meeting_id,
                    )?,
                    _ => project,
                };
                project.work_dir
            }
        };
        return run_session_command(
            services,
            invocation_dir,
            &root,
            args.command,
            stdout,
            stderr,
        );
    }

    match args.command {
        Some(
            Command::Memo { .. }
            | Command::NoteAssociation { .. }
            | Command::ProcessingStatus { .. },
        ) if workspace_selected => {
            let workspace = commands::workspace::resolve(
                workspace_selector.as_deref(),
                invocation_dir,
                stderr,
            )?;
            let command = args.command.expect("matched a command");
            commands::application::run(workspace, command, stdout)
        }
        Some(Command::Memo { .. })
        | Some(Command::NoteAssociation { .. })
        | Some(Command::ProcessingStatus { .. }) => Err(CliError::usage(
            "memo, note-association, and processing-status require an explicit Workspace",
        )),
        Some(Command::Agents { command }) => {
            if workspace_selected {
                return Err(CliError::usage(
                    "`agents install` writes into the current folder and does not accept a Workspace",
                ));
            }
            let project = services
                .projects
                .resolve_vault(project_selector.as_deref(), invocation_dir)
                .map_err(CliError::from_anyhow)?;
            match command {
                AgentsCommand::Install => {
                    commands::projects::install_agents(&project.work_dir, stdout)
                }
            }
        }
        other => unreachable!("{other:?} is handled before session resolution"),
    }
}

/// Whether `root/.margins` holds `id`, without creating the store.
fn session_exists(services: &CliServices, root: &Path, id: &str) -> bool {
    let margins_dir = root.join(".margins");
    margins_dir.is_dir() && services.sessions.exists(&margins_dir, id).unwrap_or(false)
}

/// Session commands, and whether they may start a recording.
fn capture_intent(command: &Option<Command>) -> Option<CaptureIntent<'_>> {
    match command {
        None | Some(Command::New { .. }) | Some(Command::Transcribe { .. }) => {
            Some(CaptureIntent::Record)
        }
        Some(Command::Attach { session }) => Some(CaptureIntent::Attach(session.as_deref())),
        Some(Command::Current)
        | Some(Command::Ls)
        | Some(Command::Rename { .. })
        | Some(Command::Process { .. })
        | Some(Command::ArtifactsPrune)
        | Some(Command::Archive { .. })
        | Some(Command::Recent { all: false })
        | Some(Command::Transcript { .. })
        | Some(Command::AudioExport { .. })
        | Some(Command::Artifacts { .. }) => Some(CaptureIntent::Existing),
        _ => None,
    }
}

/// Run a session command against one capture store (`root/.margins`).
fn run_session_command(
    services: &CliServices,
    invocation_dir: &Path,
    root: &Path,
    command: Option<Command>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    match command {
        None => commands::capture::run(services, root, None, None, false, true),
        Some(Command::New { title }) => {
            commands::capture::run(services, root, None, title.as_deref(), true, false)
        }
        Some(Command::Attach { session }) => {
            commands::capture::run(services, root, session.as_deref(), None, false, false)
        }
        Some(Command::Current) => commands::sessions::show_current(services, root, stdout),
        Some(Command::Ls) => commands::sessions::list(services, root, stderr),
        Some(Command::Rename { title }) => {
            commands::sessions::rename(services, root, &title, stdout)
        }
        Some(Command::Recent { .. }) => commands::transcript::recent(root, stdout),
        Some(Command::Transcript { meeting_id, format }) => commands::transcript::transcript(
            root,
            meeting_id.as_deref().unwrap_or("latest"),
            format,
            stdout,
        ),
        Some(Command::AudioExport { meeting_id }) => commands::sessions::export_audio(
            root,
            meeting_id.as_deref().unwrap_or("latest"),
            stdout,
        ),
        Some(Command::Artifacts { meeting_id }) => {
            commands::artifacts::list(root, &meeting_id, stdout)
        }
        Some(Command::ArtifactsPrune) => {
            commands::artifacts::prune(root, services.clock.now(), stdout)
        }
        Some(Command::Archive { command }) => match command {
            ArchiveCommand::On => commands::archive::set(root, true, stdout),
            ArchiveCommand::Off => commands::archive::set(root, false, stdout),
            ArchiveCommand::Status => commands::archive::status(root, stdout),
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
                root,
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
            root,
            &session,
            speakers.unwrap_or(1),
            align_only,
            stdout,
        ),
        Some(other) => unreachable!("{other:?} is not a session command"),
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
