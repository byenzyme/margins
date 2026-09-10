use clap::{Parser, Subcommand, ValueEnum};
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
    /// Define and inspect the memory boundary for one practice
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Declare and inspect workspace sources
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Set up the capabilities available in this Margins installation
    Setup {
        /// Run only this setup area; repeat to select more than one
        #[arg(long, value_enum)]
        only: Vec<SetupStepArg>,
        /// Skip local speech model preparation
        #[arg(long, value_enum)]
        skip: Option<SetupSkipArg>,
        /// Local catalyst model policy when hosted credentials are available
        #[arg(long, value_enum, default_value_t = SetupLocalModelPolicyArg::Fallback)]
        local_model: SetupLocalModelPolicyArg,
    },
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
    /// Open your coding agent ready to distill the latest session
    Note {
        /// Print the selected agent and command without launching it
        #[arg(long)]
        print: bool,
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
    /// Print the complete transcript for a meeting (every utterance plus memo
    /// timeline; falls back to the memo-aligned artifact)
    Transcript {
        /// Stable meeting id from `margins recent`; defaults to `latest`
        meeting_id: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: TranscriptFormat,
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
        /// Restrict results to one declared source name
        #[arg(long)]
        source: Option<String>,
    },
    /// Refresh declared workspace sources and the recall snapshot
    Sync {
        /// Narrow sync to one declared source binding
        #[arg(long)]
        source: Option<String>,
        /// Emit the stable margins.sync.v1 JSON contract
        #[arg(long)]
        json: bool,
    },
    /// Reconcile and inspect source integrations declared by the Workspace
    Integrations {
        #[command(subcommand)]
        command: IntegrationsCommand,
    },
    /// Preview and apply explicit Workspace integration retention
    Retention {
        #[command(subcommand)]
        command: RetentionCommand,
    },
    /// Connect Margins to external services
    Connect {
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// Disconnect Margins from an external service
    Disconnect {
        #[command(subcommand)]
        command: DisconnectCommand,
    },
    /// Inspect a vault and emit grounded setup evidence without changing it
    Scan,
    /// Print this installation's machine-readable capabilities as JSON
    Capabilities,
    /// Establish or refresh a Margins vault in this folder
    Init,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupStepArg {
    Catalyst,
    Skills,
    Speech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupSkipArg {
    Speech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupLocalModelPolicyArg {
    Fallback,
    Always,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Create a workspace with one writable home notes source
    New {
        /// Stable lowercase workspace id
        id: String,
        /// Existing folder for writable notes and projected human-readable artifacts
        #[arg(long)]
        home: PathBuf,
        /// Optional display name
        #[arg(long)]
        name: Option<String>,
        /// Emit machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Show the selected workspace and its state paths
    Status {
        /// Emit machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Compile complete desired Workspace settings into a revisioned plan
    Plan {
        /// TOML file containing the complete desired Workspace settings
        #[arg(long)]
        desired: PathBuf,
        /// Emit margins.workspace.plan.v1 JSON
        #[arg(long, required = true)]
        json: bool,
    },
    /// Atomically apply an exact workspace plan
    Apply {
        /// JSON plan emitted by `workspace plan`
        #[arg(long)]
        plan: PathBuf,
        /// Emit margins.workspace.apply.v1 JSON
        #[arg(long, required = true)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceKindArg {
    Notes,
    Captures,
    GoogleMail,
    GoogleCalendar,
    GoogleMeet,
    Granola,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GranolaTimeRangeArg {
    #[value(name = "last_30_days")]
    Last30Days,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceRoleArg {
    Home,
    Reference,
}

#[derive(Debug, Subcommand)]
pub enum SourceCommand {
    /// Declare a source through the same locked workspace mutation path
    Add {
        kind: SourceKindArg,
        /// Stable source name within the workspace
        #[arg(long)]
        name: String,
        /// Absolute path for notes or captures sources
        #[arg(long)]
        path: Option<PathBuf>,
        /// Notes role; only reference may be added because home is created with the workspace
        #[arg(long)]
        role: Option<SourceRoleArg>,
        /// Remote account identity for Google sources
        #[arg(long)]
        account: Option<String>,
        /// Non-temporal Gmail search query (google-mail only)
        #[arg(long, allow_hyphen_values = true)]
        query: Option<String>,
        /// Gmail history backfill window in days (google-mail only)
        #[arg(long)]
        backfill_days: Option<u32>,
        /// Calendar history lookback window in days (google-calendar only)
        #[arg(long)]
        lookback_days: Option<u32>,
        /// Calendar future lookahead window in days (google-calendar only)
        #[arg(long)]
        lookahead_days: Option<u32>,
        /// Granola bounded collection window (granola only)
        #[arg(long)]
        time_range: Option<GranolaTimeRangeArg>,
        /// Emit machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// List declared sources and their fixed policies
    List {
        /// Emit machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Remove a declared non-required source
    Remove {
        name: String,
        /// Emit machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConnectCommand {
    /// Connect Google mail, calendar, and files read-only
    Google {
        /// Optional expected Google account email safety guard
        #[arg(long)]
        account: Option<String>,
        /// Headless/SSH presentation: print the consent URL to stderr (CLI credentials always use an owner-only 0600 file)
        #[arg(long)]
        headless: bool,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
    /// Connect Granola meeting access over its public MCP authorization flow
    Granola {
        /// Optional expected Granola account email safety guard
        #[arg(long)]
        account: Option<String>,
        /// Headless/SSH presentation: use PKCE with a private callback prompt (CLI credentials always use an owner-only 0600 file)
        #[arg(long)]
        headless: bool,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
    /// Show connection status
    Status {
        /// Restrict status to one machine connection service
        #[arg(long, value_enum)]
        service: Option<ConnectionServiceArg>,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DisconnectCommand {
    /// Forget a machine-level Google connection credential
    Google {
        /// Connected Google account email to forget on this machine
        #[arg(long)]
        account: String,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
    /// Forget a machine-level Granola connection credential
    Granola {
        /// Connected Granola account email to forget on this machine
        #[arg(long)]
        account: String,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConnectionServiceArg {
    Google,
    Granola,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TranscriptFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Import a Granola export as native notes in the selected Workspace
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
    /// Print the agent guide for understanding and setting up a Workspace
    WorkspaceSetup,
    /// Print the guided onboarding experience for an orchestrating agent
    Onboarding,
}

#[derive(Debug, Subcommand)]
pub enum IntegrationsCommand {
    /// Reconcile materialized evidence directly from declared bindings
    Reconcile {
        /// Reconcile only this connector id
        #[arg(long)]
        connector: Option<String>,
        /// Reconcile only this exact connector account
        #[arg(long, requires = "connector")]
        account: Option<String>,
        /// Exact workspace revision being reconciled
        #[arg(long)]
        if_revision: String,
        /// Stable idempotency key for this reconcile attempt
        #[arg(long)]
        request_id: String,
        /// Emit margins.integrations.reconcile.v1 JSON
        #[arg(long, required = true)]
        json: bool,
    },
    /// Show ledger-backed health and freshness for declared connectors
    Status {
        /// Emit machine-readable status results
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RetentionScopeArg {
    Expired,
    RawCache,
    Tombstones,
    Materialization,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RetentionConnectorArg {
    #[value(name = "email")]
    Email,
    #[value(name = "gcal")]
    Gcal,
    #[value(name = "google_meet")]
    GoogleMeet,
}

#[derive(Debug, Subcommand)]
pub enum RetentionCommand {
    /// Produce a deterministic read-only destructive preview
    Preview {
        /// Closed connector id: email, gcal, or google_meet
        #[arg(long, value_enum)]
        connector: RetentionConnectorArg,
        /// Exact normalized connector account
        #[arg(long)]
        account: String,
        /// Independently purgeable state class
        #[arg(long, value_enum)]
        scope: RetentionScopeArg,
        /// Emit margins.retention.preview.v1 JSON
        #[arg(long, required = true)]
        json: bool,
    },
    /// Atomically apply an exact destructive preview
    Apply {
        /// JSON preview emitted by `retention preview`
        #[arg(long)]
        plan: PathBuf,
        /// Exact Workspace revision the preview was compiled against
        #[arg(long)]
        if_revision: String,
        /// Stable idempotency key for this destructive mutation
        #[arg(long)]
        request_id: String,
        /// Emit margins.retention.apply.v1 JSON
        #[arg(long, required = true)]
        json: bool,
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

/// Extract the global `--workspace` selector wherever it appears.
pub fn strip_workspace_arg(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(Option<String>, Vec<OsString>), String> {
    let mut output = Vec::new();
    let mut workspace = None;
    let mut args = args.into_iter();
    if let Some(binary) = args.next() {
        output.push(binary);
    }
    while let Some(arg) = args.next() {
        if arg == "--workspace" {
            let value = args
                .next()
                .ok_or_else(|| "--workspace requires an id".to_string())?;
            workspace = Some(value.to_string_lossy().into_owned());
        } else if let Some(value) = arg
            .to_str()
            .and_then(|text| text.strip_prefix("--workspace="))
        {
            workspace = Some(value.to_string());
        } else {
            output.push(arg);
        }
    }
    Ok((workspace, output))
}
