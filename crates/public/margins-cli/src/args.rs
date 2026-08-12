use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "margins",
    about = "Record meetings and work with their notes and transcripts"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Prepare local transcription for this machine
    Setup,
    /// Start a new current session and open the recorder
    New {
        /// Optional display title; Margins generates the stable session id
        #[arg(long)]
        title: Option<String>,
    },
    /// Open the recorder for the current session, adding a new segment
    Attach {
        /// Make this existing session current before attaching
        session: Option<String>,
    },
    /// Show the current recording session
    Current,
    /// List recording sessions
    #[command(alias = "list")]
    Ls,
    /// Change the current session's display title
    Rename { title: String },
    /// List recent Margins meetings as XML
    Recent,
    /// Print the complete transcript for a meeting as XML (every utterance
    /// plus memo timeline; falls back to the memo-aligned artifact)
    Transcript {
        /// Stable meeting id from `margins recent`, or `latest`
        meeting_id: String,
    },
    /// List registered artifacts for a meeting as XML
    Artifacts {
        /// Stable meeting id from `margins recent`, or `latest`
        meeting_id: String,
    },
    /// Delete expired temporary registered artifacts
    ArtifactsPrune,
    /// Transcribe/import-prep an existing audio file
    Transcribe {
        /// Audio file to decode in Rust, such as WAV, M4A, MP3, FLAC, or AAC
        audio_path: PathBuf,
        /// Session name. Defaults to a slug derived from the audio filename.
        #[arg(long)]
        name: Option<String>,
        /// Optional memo/context markdown captured at the same time.
        #[arg(long)]
        memo: Option<PathBuf>,
        /// Speaker count for diarizing the downmixed mono audio. Default is 1.
        #[arg(long)]
        speakers: Option<usize>,
    },
    /// Process every segment of an existing recording session
    Process {
        /// Stable session id from `margins recent`, or `current`/`latest`
        session: String,
        /// Speaker count for mono diarization; stereo recordings keep channels
        #[arg(long)]
        speakers: Option<usize>,
        /// Rebuild alignment from the existing transcript without running ASR
        #[arg(long)]
        align_only: bool,
    },
    /// Import external meeting exports
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    /// Install Margins usage instructions into project agent files
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Inspect or switch the default Margins project used by the CLI
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Manage Margins projects (list, add, init)
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Validate and import a Granola export file into .margins
    Granola {
        /// Granola JSON or CSV export file
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// Create or update AGENTS.md and CLAUDE.md with Margins CLI guidance
    Install,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// List projects configured in Margins Desktop as XML
    List,
    /// Show the current active project as XML
    Current,
    /// Set the default CLI/app project to an already configured Margins Desktop project
    Use {
        /// Project id, name, root path, or work-dir path from `margins project list`
        project: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectsCommand {
    /// List configured Margins projects
    List {
        /// Output as a JSON array
        #[arg(long)]
        json: bool,
    },
    /// Register a folder as a Margins project (idempotent)
    Add {
        /// Project folder path; relative paths resolve from the invocation directory
        path: String,
        /// Display name for the project (defaults to folder name)
        #[arg(long)]
        name: Option<String>,
        /// Subfolder for new meeting notes (defaults to "meetings")
        #[arg(long)]
        inbox_folder: Option<String>,
        /// Also run `enzyme -p <path> init` after registering
        #[arg(long)]
        init: bool,
    },
    /// Initialize enzyme in the active (or specified) project folder
    Init {
        /// Path to the project folder (defaults to active project root)
        path: Option<PathBuf>,
    },
}

/// Preserve the historical global `--project value` and `--project=value`
/// preprocessing even when the flag appears after a subcommand.
pub fn strip_project_arg(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(Option<String>, Vec<OsString>), String> {
    let mut output = Vec::new();
    let mut project = None;
    let mut args = args.into_iter();
    if let Some(binary) = args.next() {
        output.push(binary);
    }
    while let Some(arg) = args.next() {
        if arg == "--project" {
            let value = args.next().ok_or_else(|| {
                "--project requires a configured project id, name, or path".to_string()
            })?;
            project = Some(value.to_string_lossy().into_owned());
        } else if let Some(value) = arg
            .to_str()
            .and_then(|text| text.strip_prefix("--project="))
        {
            project = Some(value.to_string());
        } else {
            output.push(arg);
        }
    }
    Ok((project, output))
}
