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
    /// Print a short agent handoff for setting up this folder
    Setup,
    /// Print embedded Margins guides for agents
    Guide {
        #[command(subcommand)]
        command: GuideCommand,
    },
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
    Recent {
        /// List meetings across every registered vault, not just this one
        #[arg(long)]
        all: bool,
    },
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
    /// Keep aligned transcripts in the visible `_margins/` vault folder
    Archive {
        #[command(subcommand)]
        command: ArchiveCommand,
    },
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
    /// Search the vault for notes and connections related to a query
    Recall {
        /// What to look for, in the vault's own language where possible
        query: String,
    },
    /// Inspect a vault and suggest folders, tags, links, logs, and exclusions
    Scan {
        /// Write the initial suggested policy when this workspace has none
        #[arg(long)]
        write_config: bool,
    },
    /// Print this binary's machine-readable composition capabilities as JSON
    Capabilities,
    /// Establish or refresh a Margins vault in this folder
    Init,
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
pub enum ArchiveCommand {
    /// Move aligned transcripts into `_margins/` and use it for new output
    On,
    /// Move aligned transcripts back into `.margins/`
    Off,
    /// Show the current archive location and transcript count
    Status,
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// Create or update AGENTS.md and CLAUDE.md with Margins CLI guidance
    Install,
}

#[derive(Debug, Subcommand)]
pub enum GuideCommand {
    /// Print the complete Margins workspace setup guide
    WorkspaceSetup,
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
