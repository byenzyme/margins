use clap::{Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "margins",
    about = "Record meetings and work with the notes they connect to",
    after_help = "Start here: run `margins init` in your notes folder (or `margins init <folder>`).\n\
Running `margins` with no command opens the recorder for the current session.\n\
Your Workspace is one editable program, $MARGINS_HOME/configs/<id>.enzyme: it says which notes \
Margins learns from, what it leaves out, and where new notes go.\n  \
See what it learns: margins status --explain\n  \
Change it:          margins edit\n\
New to the words? Run `margins guide glossary`."
)]
pub struct Args {
    /// Select an opt-in remote Margins instance (ssh://alias or https://host)
    #[arg(long, global = true, conflicts_with = "local")]
    pub remote: Option<String>,
    /// Force the direct local adapter even when MARGINS_REMOTE is set
    #[arg(long, global = true, conflicts_with = "remote", hide = true)]
    pub local: bool,
    /// Print the version, build commit, and composition
    #[arg(short = 'V', long)]
    pub version: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Commands are listed by journey: set up and refine a Workspace, use it,
/// then record. Plumbing for agents, plugins, and recovery hints stays
/// callable (argv, flags, and JSON unchanged) but hidden from help.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Make a Workspace for your notes folder and index it, or refresh the
    /// Workspace that already covers it
    Init {
        /// Notes folder (default: the current folder). Naming a folder also
        /// allows an empty or temporary one.
        path: Option<PathBuf>,
        /// Workspace id for a new Workspace (default: from the folder name)
        #[arg(long)]
        id: Option<String>,
        /// Start from a minimal program instead of the margins-meetings preset
        #[arg(long)]
        no_preset: bool,
        /// Emit the margins.init.v1 JSON receipt instead of readable output
        #[arg(long)]
        json: bool,
    },
    /// Show your Workspace: what Margins learns about, its index, its
    /// catalysts (questions Margins prepares from your notes), and its Sources
    Status {
        /// Also show, per reading, what it picked and why some are skipped
        #[arg(long)]
        explain: bool,
        /// List every automatic pick and skipped entity instead of a summary
        #[arg(long)]
        all: bool,
        /// Emit the margins.status.v1 JSON contract
        #[arg(long)]
        json: bool,
    },
    /// Change your Workspace program in $VISUAL or $EDITOR, with a preview of
    /// the effect before anything is applied
    Edit {
        /// Print the program text instead of opening an editor
        #[arg(long, conflicts_with = "path")]
        print: bool,
        /// Print only the program's path
        #[arg(long)]
        path: bool,
        /// Emit the id, program path, revision, and text as JSON
        #[arg(long, conflicts_with = "path")]
        json: bool,
        /// Colour the reviewed change: auto (only on a terminal), always, never
        #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
        color: ColorArg,
    },
    /// Refresh your notes and connected Sources, then say what changed in
    /// what Margins learns about
    Sync {
        /// Narrow sync to one declared source binding
        #[arg(long)]
        source: Option<String>,
        /// Emit the stable margins.sync.v1 JSON contract
        #[arg(long)]
        json: bool,
    },
    /// Search your notes for what relates to a query
    Recall {
        /// What to look for, in the vault's own language where possible
        query: String,
        /// Restrict results to one declared source name
        #[arg(long)]
        source: Option<String>,
        /// Emit the margins.recall.v1 JSON envelope instead of readable results
        #[arg(long)]
        json: bool,
    },
    /// Connect Margins to Google or Granola
    Connect {
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// Disconnect Margins from an external service
    Disconnect {
        #[command(subcommand)]
        command: DisconnectCommand,
    },
    /// Set up everything at once: catalysts, agent skills, and the speech model
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
    /// Print embedded Margins guides, and a glossary of Margins words
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
    /// Print the complete transcript for a meeting (every utterance plus memo
    /// timeline; falls back to the memo-aligned artifact)
    Transcript {
        /// Stable meeting id from `margins ls`; defaults to `latest`
        meeting_id: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: TranscriptFormat,
    },
    /// Transcribe an existing recording as a new session
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
    /// Open your coding agent ready to distill the latest session
    Note {
        /// Print the selected agent and command without launching it
        #[arg(long)]
        print: bool,
    },
    /// Inspect and recover opt-in remote capture transfers
    #[command(hide = true)]
    Transfers {
        #[command(subcommand)]
        command: TransfersCommand,
    },
    /// Administer or discover the local Workspace service
    #[command(hide = true)]
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Workspace plumbing for agents and plugins (people use init, status,
    /// and edit)
    #[command(hide = true)]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Declare and inspect workspace sources
    #[command(hide = true)]
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Change the current session's display title
    #[command(hide = true)]
    Rename { title: String },
    /// Read or revision-check an edit to the remote timed memo
    #[command(hide = true)]
    Memo {
        /// Stable meeting id; defaults to the client-scoped current session
        meeting_id: Option<String>,
        /// Whole notepad text to reconcile at the observed capture time
        #[arg(long, requires = "expected_revision")]
        text: Option<String>,
        /// Revision returned by the preceding memo read
        #[arg(long)]
        expected_revision: Option<String>,
        /// Stable retry identity; generated when omitted
        #[arg(long)]
        request_id: Option<String>,
        /// Capture-timeline observation time for this edit
        #[arg(long)]
        observed_at_ms: Option<u64>,
        /// Record this edit as occurring while capture was paused
        #[arg(long)]
        paused: bool,
    },
    /// Read, link, or unlink a Source-relative session note reference
    #[command(hide = true)]
    NoteAssociation {
        /// Stable meeting id; defaults to the client-scoped current session
        meeting_id: Option<String>,
        /// Declared logical Source id for a new association
        #[arg(long, requires = "path")]
        source: Option<String>,
        /// Source-relative Markdown path; note bytes stay in native sync
        #[arg(long, requires = "source")]
        path: Option<String>,
        /// Optional hash observed by the client that wrote the note
        #[arg(long)]
        hash: Option<String>,
        /// Remove the current association
        #[arg(long, conflicts_with_all = ["source", "path", "hash"])]
        unlink: bool,
        /// Revision returned by the preceding association read
        #[arg(long)]
        expected_revision: Option<u64>,
        /// Stable retry identity for a link operation
        #[arg(long)]
        request_id: Option<String>,
        /// bb distillation thread id recorded with a linked note
        #[arg(long, requires_all = ["memo_revision", "source", "path"], conflicts_with = "unlink")]
        bb_thread_id: Option<String>,
        /// Memo revision used to produce the linked note
        #[arg(long, requires = "bb_thread_id", conflicts_with = "unlink")]
        memo_revision: Option<String>,
    },
    /// Show the latest Margins processing job independently of note links
    #[command(hide = true)]
    ProcessingStatus {
        /// Stable meeting id; defaults to the client-scoped current session
        meeting_id: Option<String>,
    },
    /// List recent Margins meetings as XML
    #[command(hide = true)]
    Recent {
        /// List meetings across every registered vault, not just this one
        #[arg(long)]
        all: bool,
    },
    /// Materialize scriptable WAV files for a meeting from its durable audio
    #[command(name = "audio-export", hide = true)]
    AudioExport {
        /// Meeting id from `margins recent`; defaults to `latest`
        meeting_id: Option<String>,
    },
    /// List registered artifacts for a meeting as XML
    #[command(hide = true)]
    Artifacts {
        /// Stable meeting id from `margins recent`, or `latest`
        meeting_id: String,
    },
    /// Delete expired temporary registered artifacts
    #[command(hide = true)]
    ArtifactsPrune,
    /// Keep aligned transcripts in the visible `_margins/` vault folder
    #[command(hide = true)]
    Archive {
        #[command(subcommand)]
        command: ArchiveCommand,
    },
    /// Process every segment of an existing recording session
    #[command(hide = true)]
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
    #[command(hide = true)]
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    /// Install Margins usage instructions into project agent files
    #[command(hide = true)]
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Reconcile and inspect source integrations declared by the Workspace
    #[command(hide = true)]
    Integrations {
        #[command(subcommand)]
        command: IntegrationsCommand,
    },
    /// Preview and apply explicit Workspace integration retention
    #[command(hide = true)]
    Retention {
        #[command(subcommand)]
        command: RetentionCommand,
    },
    /// Print this installation's machine-readable capabilities as JSON
    #[command(hide = true)]
    Capabilities,
}

#[derive(Debug, Subcommand)]
pub enum TransfersCommand {
    /// List locally recoverable remote transfers
    List {
        #[arg(long)]
        json: bool,
    },
    /// Retry delivery of one recoverable transfer
    Retry { transfer_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Print SSH-safe discovery metadata and a short-lived scoped credential
    Discover {
        #[arg(long, required = true)]
        json: bool,
        /// Explicit service state directory for isolated provisioning/verification
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Issue a revocable upload-only credential for a Shortcut installation
    PairShortcut {
        #[arg(long)]
        principal: String,
        #[arg(long, required = true)]
        json: bool,
    },
    /// Revoke all credentials for a principal
    Revoke {
        principal: String,
        #[arg(long, required = true)]
        json: bool,
    },
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
    /// Remove a Workspace declaration only when it has no stored data
    Remove {
        /// Workspace id to remove; declared Source folders are preserved
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// List Workspaces and the machine default
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read or set the machine's default Workspace
    Default {
        /// Set the default to an existing Workspace id
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read the selected Workspace's reviewed Home note destination
    Destination {
        #[arg(long, required = true)]
        json: bool,
    },
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
    /// Rename a Workspace whose id is now reserved (settings, profiles,
    /// margins-sources), keeping its notes and state
    Rename {
        /// The reserved id the Workspace has today
        old: String,
        /// The new Workspace id
        new: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the path of the selected Workspace's program
    Show {
        /// Print the program text instead of its path
        #[arg(long)]
        text: bool,
        /// Emit the id, program path, and revision (and text with --text) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Edit the selected Workspace's program in $VISUAL or $EDITOR, then review
    /// the change and apply it through plan/apply (interactive terminals only)
    Edit {
        /// Colour the reviewed change: auto (only on a terminal), always, never
        #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
        color: ColorArg,
    },
    /// Compile a complete desired Workspace program, or the setup preset, into
    /// a revisioned plan
    Plan {
        /// `.enzyme` file with the complete desired `workspace "<id>" { … }`
        /// program (a legacy `.toml` desired config is still accepted)
        #[arg(long, required_unless_present = "preset", conflicts_with = "preset")]
        desired: Option<PathBuf>,
        /// Fill a preset for the Workspace's notes folder and add it to the
        /// current program: `margins-meetings` (the setup preset) or the path
        /// to a `.enzyme.in` template. Folder readings for folders the notes
        /// folder does not have are dropped.
        #[arg(long, value_name = "NAME|PATH")]
        preset: Option<String>,
        /// Emit margins.workspace.plan.v2 JSON instead of a readable summary
        /// (without it, the plan is saved to a file for `workspace apply`)
        #[arg(long)]
        json: bool,
        /// Colour the readable change: auto (only on a terminal), always,
        /// never. JSON is never coloured.
        #[arg(long, value_enum, default_value_t = ColorArg::Auto)]
        color: ColorArg,
    },
    /// Atomically apply an exact workspace plan
    Apply {
        /// JSON plan emitted by `workspace plan`
        #[arg(long)]
        plan: PathBuf,
        /// Emit margins.workspace.apply.v2 JSON instead of a readable receipt
        #[arg(long)]
        json: bool,
    },
}

/// `--color`: `auto` colours only output written to a terminal (and honours
/// `NO_COLOR` and `TERM=dumb`), so piped output is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Auto,
    Always,
    Never,
}

impl ColorArg {
    /// Whether to colour output for a stream that is (`terminal`) or is not a
    /// terminal.
    pub fn enabled(self, terminal: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                terminal
                    && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                    && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
            }
        }
    }
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
    /// Explain the words Margins uses: Workspace, program, reading, catalyst, …
    Glossary,
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
