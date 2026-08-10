//! Standalone public parser and command dispatcher for Margins.

#![forbid(unsafe_code)]

pub mod args;
pub mod commands;
pub mod error;
pub mod output;
pub mod services;

pub use error::CliError;
pub use services::{
    standalone_services, CliServices, Clock, LocalSessionStore, ProcessRunner, ProjectService,
    SessionStore, SystemClock, SystemProcessRunner, SystemProjectService,
};

use args::{AgentsCommand, Args, Command, ImportCommand, ProjectCommand, ProjectsCommand};
use clap::Parser;
use commands::projects::absolute_from;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::Path;

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
    let result = run_inner(services, invocation_dir, args, stdout, stderr);
    if let Err(error) = &result {
        let _ = output::write_error(stderr, error);
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
    let (project_selector, args) = args::strip_project_arg(args).map_err(CliError::usage)?;
    let args = Args::try_parse_from(args).map_err(|error| CliError::usage(error.to_string()))?;

    match args.command {
        Some(Command::Project { command }) => {
            return match command {
                ProjectCommand::List => commands::projects::project_list(services, stdout),
                ProjectCommand::Current => commands::projects::project_current(services, stdout),
                ProjectCommand::Use { project } => {
                    commands::projects::project_use(services, &project, stdout)
                }
            };
        }
        Some(Command::Projects { command }) => {
            return match command {
                ProjectsCommand::List { json } => commands::projects::list(services, json, stdout),
                ProjectsCommand::Add {
                    path,
                    name,
                    inbox_folder,
                    init,
                } => commands::projects::add(
                    services,
                    invocation_dir,
                    &path,
                    name.as_deref(),
                    inbox_folder.as_deref(),
                    init,
                    stdout,
                    stderr,
                ),
                ProjectsCommand::Init { path } => {
                    commands::projects::init(services, invocation_dir, path.as_deref(), stdout)
                }
            };
        }
        _ => {}
    }

    let project = services
        .projects
        .resolve(project_selector.as_deref())
        .map_err(CliError::from_anyhow)?;
    let work_dir = &project.work_dir;
    match args.command {
        None => commands::capture::run(services, work_dir, None, None, false),
        Some(Command::New { title }) => {
            commands::capture::run(services, work_dir, None, title.as_deref(), true)
        }
        Some(Command::Attach { session }) => {
            commands::capture::run(services, work_dir, session.as_deref(), None, false)
        }
        Some(Command::Current) => commands::sessions::show_current(services, work_dir, stdout),
        Some(Command::Ls) => commands::sessions::list(services, work_dir, stderr),
        Some(Command::Rename { title }) => {
            commands::sessions::rename(services, work_dir, &title, stdout)
        }
        Some(Command::Recent) => commands::transcript::recent(work_dir, stdout),
        Some(Command::Transcript { meeting_id }) => {
            commands::transcript::transcript(work_dir, &meeting_id, stdout)
        }
        Some(Command::Artifacts { meeting_id }) => {
            commands::artifacts::list(work_dir, &meeting_id, stdout)
        }
        Some(Command::ArtifactsPrune) => {
            commands::artifacts::prune(work_dir, services.clock.now(), stdout)
        }
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
        Some(Command::Import { command }) => match command {
            ImportCommand::Granola { path } => commands::import::granola(
                work_dir,
                &project,
                &absolute_from(invocation_dir, &path),
                stdout,
            ),
        },
        Some(Command::Agents { command }) => match command {
            AgentsCommand::Install => commands::projects::install_agents(work_dir, stdout),
        },
        Some(Command::Project { .. }) | Some(Command::Projects { .. }) => unreachable!(),
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
