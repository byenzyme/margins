//! Workspace and source configuration owned by Margins.
//!
//! A workspace is the id-named memory boundary of one practice. Its whole
//! configuration is one `.enzyme` program at `<MARGINS_HOME>/configs/<id>.enzyme`
//! (`workspace "<id>" { … }`), parsed by `enzyme-spec`; see
//! [`crate::workspace_program`]. The program records the Sources, the one
//! create-note folder (Home), and the attention corrections the runtime must
//! remember. Richer understanding stays grounded in the Sources and in the
//! setup conversation. Machine state lives at `<MARGINS_HOME>/workspaces/<id>`
//! and holds no configuration; content sources remain where their owners put
//! them. A command may create the first, implicit home Source from a
//! notes-bearing working directory under a strict isolation deny-list. Every
//! Source beyond that home remains explicitly declared.
//!
//! A retired `workspaces/<id>/config.toml` is migrated to the program on first
//! resolution (`config.toml.migrated` keeps the original).

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub use crate::workspace_program::{program_sha256, WorkspaceProgram};
use crate::machine_config;
use crate::workspace_program::{self as program_lang, FolderQualification};

pub const WORKSPACES_DIR: &str = "workspaces";
/// Directory of Workspace programs under `MARGINS_HOME`.
pub const CONFIGS_DIR: &str = "configs";
/// Optional shared custom profiles for every Workspace program.
pub const SHARED_PROFILES_PROGRAM: &str = "profiles.enzyme";
/// Programs under `configs/` that are not Workspaces: shared profiles, engine
/// settings, and the source kinds Margins defines.
pub const RESERVED_PROGRAMS: [&str; 3] = [
    SHARED_PROFILES_PROGRAM,
    machine_config::SETTINGS_PROGRAM,
    "margins-sources.enzyme",
];
/// The retired per-Workspace TOML config, migrated on first resolution.
pub const LEGACY_WORKSPACE_CONFIG: &str = "config.toml";
pub const LEGACY_WORKSPACE_CONFIG_MIGRATED: &str = "config.toml.migrated";
/// The engine index of a Workspace: the engine's named-workspace index path
/// `$ENZYME_HOME/workspaces/<id>/enzyme.db`, with `ENZYME_HOME=$MARGINS_HOME`.
pub const INDEX_DB: &str = "enzyme.db";
/// The index name before the Margins home became an Enzyme home; renamed to
/// [`INDEX_DB`] on first resolution, without reindexing.
pub const LEGACY_INDEX_DB: &str = "index.db";
/// SQLite files that travel with a database file.
const SQLITE_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];
pub const WORKSPACE_PLAN_SCHEMA: &str = "margins.workspace.plan.v2";
pub const WORKSPACE_APPLY_SCHEMA: &str = "margins.workspace.apply.v2";
pub const WORKSPACE_MIGRATE_SCHEMA: &str = "margins.workspace.migrate.v1";
const WORKSPACE_LOCK: &str = "config.lock";
const WORKSPACE_RECEIPTS_DIR: &str = "workspace-receipts";
const WORKSPACE_TRANSACTION: &str = "workspace-transaction.json";
/// Test/harness-only JSON override for process-derived implicit-home deny roots.
/// The schema requires `home_dir`, `enzyme_home`, and non-empty
/// `config_state_roots` and `temp_roots` arrays of absolute paths.
pub const IMPLICIT_WORKSPACE_DENY_ROOTS_ENV: &str = "MARGINS_IMPLICIT_WORKSPACE_DENY_ROOTS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Notes,
    Captures,
    GoogleMail,
    GoogleCalendar,
    GoogleMeet,
    Granola,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePolicy {
    pub cache_raw_payload: bool,
    pub project_to_home: bool,
    pub index: IndexPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPolicy {
    Native,
    Ledger,
    Registry,
}

impl SourceKind {
    /// The one policy table for source persistence and indexing behavior.
    pub const fn policy(self) -> SourcePolicy {
        match self {
            Self::Notes => SourcePolicy {
                cache_raw_payload: false,
                project_to_home: false,
                index: IndexPolicy::Native,
            },
            Self::Captures => SourcePolicy {
                cache_raw_payload: false,
                project_to_home: false,
                index: IndexPolicy::Registry,
            },
            Self::GoogleMail | Self::GoogleCalendar => SourcePolicy {
                cache_raw_payload: true,
                project_to_home: false,
                index: IndexPolicy::Ledger,
            },
            Self::GoogleMeet | Self::Granola => SourcePolicy {
                cache_raw_payload: true,
                project_to_home: false,
                index: IndexPolicy::Ledger,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRole {
    Home,
    Reference,
}

pub const DEFAULT_GMAIL_QUERY: &str = "-in:spam -in:trash";
pub const DEFAULT_GMAIL_BACKFILL_DAYS: u32 = 365;
pub const DEFAULT_CALENDAR_LOOKBACK_DAYS: u32 = 365;
pub const DEFAULT_CALENDAR_LOOKAHEAD_DAYS: u32 = 180;

/// Gmail import scope declared in workspace desired state. Temporal bounds live
/// only in `backfill_days`; `query` must remain non-temporal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailCollectionSelector {
    pub query: String,
    pub backfill_days: u32,
}

impl GmailCollectionSelector {
    pub fn default_declaration() -> Self {
        Self {
            query: DEFAULT_GMAIL_QUERY.to_string(),
            backfill_days: DEFAULT_GMAIL_BACKFILL_DAYS,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.backfill_days == 0 {
            bail!("Gmail backfill_days must be greater than zero");
        }
        validate_gmail_query(&self.query)
    }

    /// Stable receipt for the exact desired-state selector used to build an
    /// authoritative Gmail materialization snapshot.
    pub fn materialization_fingerprint(&self) -> Result<String> {
        Ok(format!(
            "gmail-selector-v1:{}",
            serde_json::to_string(self)
                .context("failed to fingerprint Gmail collection selector")?
        ))
    }
}

fn validate_gmail_query(query: &str) -> Result<()> {
    let lower = query.to_ascii_lowercase();
    for forbidden in [
        "newer_than:",
        "older_than:",
        "newer:",
        "older:",
        "after:",
        "before:",
    ] {
        if lower.contains(forbidden) {
            bail!(
                "Gmail query cannot contain temporal operator {forbidden:?}; use backfill_days instead"
            );
        }
    }
    Ok(())
}

/// Google Calendar import scope declared in workspace desired state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarCollectionSelector {
    pub lookback_days: u32,
    pub lookahead_days: u32,
}

impl CalendarCollectionSelector {
    pub fn default_declaration() -> Self {
        Self {
            lookback_days: DEFAULT_CALENDAR_LOOKBACK_DAYS,
            lookahead_days: DEFAULT_CALENDAR_LOOKAHEAD_DAYS,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.lookback_days == 0 {
            bail!("Calendar lookback_days must be greater than zero");
        }
        if self.lookahead_days == 0 {
            bail!("Calendar lookahead_days must be greater than zero");
        }
        Ok(())
    }

    /// Stable receipt for the exact desired-state selector used to build an
    /// authoritative Calendar materialization snapshot.
    pub fn materialization_fingerprint(&self) -> Result<String> {
        Ok(format!(
            "calendar-selector-v1:{}",
            serde_json::to_string(self)
                .context("failed to fingerprint Calendar collection selector")?
        ))
    }
}

/// Granola import scope declared in workspace desired state. The provider
/// currently proves a bounded rolling window, not an all-history collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GranolaTimeRange {
    #[serde(rename = "last_30_days")]
    Last30Days,
}

impl GranolaTimeRange {
    pub const fn as_provider_value(self) -> &'static str {
        match self {
            Self::Last30Days => "last_30_days",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GranolaCollectionSelector {
    pub time_range: GranolaTimeRange,
    pub workspace_only: bool,
}

impl GranolaCollectionSelector {
    pub fn default_declaration() -> Self {
        Self {
            time_range: GranolaTimeRange::Last30Days,
            workspace_only: false,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self.time_range {
            GranolaTimeRange::Last30Days => Ok(()),
        }
    }

    pub fn materialization_fingerprint(&self) -> Result<String> {
        Ok(format!(
            "granola-selector-v1:{}",
            serde_json::to_string(self)
                .context("failed to fingerprint Granola collection selector")?
        ))
    }

    pub fn occurred_from(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self.time_range {
            GranolaTimeRange::Last30Days => now - ChronoDuration::days(30),
        }
    }
}

/// Concrete workspace binding declared in desired state. Each variant carries
/// only the fields valid for its kind; there are no cross-kind optional slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkspaceBinding {
    #[serde(rename = "notes")]
    NativeMarkdown {
        path: PathBuf,
        role: SourceRole,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note_folder: Option<PathBuf>,
    },
    Captures {
        path: PathBuf,
    },
    #[serde(rename = "google-mail")]
    Gmail {
        account: String,
        gmail: GmailCollectionSelector,
    },
    #[serde(rename = "google-calendar")]
    GoogleCalendar {
        account: String,
        calendar: CalendarCollectionSelector,
    },
    #[serde(rename = "google-meet")]
    GoogleMeet {
        account: String,
    },
    Granola {
        account: String,
        collection: GranolaCollectionSelector,
    },
}

impl WorkspaceBinding {
    pub fn kind(&self) -> SourceKind {
        match self {
            Self::NativeMarkdown { .. } => SourceKind::Notes,
            Self::Captures { .. } => SourceKind::Captures,
            Self::Gmail { .. } => SourceKind::GoogleMail,
            Self::GoogleCalendar { .. } => SourceKind::GoogleCalendar,
            Self::GoogleMeet { .. } => SourceKind::GoogleMeet,
            Self::Granola { .. } => SourceKind::Granola,
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::NativeMarkdown { path, .. } | Self::Captures { path } => Some(path),
            Self::Gmail { .. }
            | Self::GoogleCalendar { .. }
            | Self::GoogleMeet { .. }
            | Self::Granola { .. } => None,
        }
    }

    pub fn google_account(&self) -> Option<&str> {
        match self {
            Self::Gmail { account, .. }
            | Self::GoogleCalendar { account, .. }
            | Self::GoogleMeet { account }
            | Self::Granola { account, .. } => Some(account.as_str()),
            _ => None,
        }
    }

    pub fn native_markdown_path(&self) -> Option<&Path> {
        match self {
            Self::NativeMarkdown { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn native_markdown_role(&self) -> Option<SourceRole> {
        match self {
            Self::NativeMarkdown { role, .. } => Some(*role),
            _ => None,
        }
    }

    pub fn gmail_account(&self) -> Option<&str> {
        match self {
            Self::Gmail { account, .. } => Some(account.as_str()),
            _ => None,
        }
    }

    pub fn gmail_selector(&self) -> Option<&GmailCollectionSelector> {
        match self {
            Self::Gmail { gmail, .. } => Some(gmail),
            _ => None,
        }
    }

    pub fn calendar_selector(&self) -> Option<&CalendarCollectionSelector> {
        match self {
            Self::GoogleCalendar { calendar, .. } => Some(calendar),
            _ => None,
        }
    }

    pub fn granola_selector(&self) -> Option<&GranolaCollectionSelector> {
        match self {
            Self::Granola { collection, .. } => Some(collection),
            _ => None,
        }
    }
}

const NATIVE_COLLECTION_DOMAIN: &[u8] = b"margins:workspace:native-markdown:v1\0";
const GMAIL_COLLECTION_DOMAIN: &[u8] = b"margins:workspace:gmail:v1\0";
const CALENDAR_COLLECTION_DOMAIN: &[u8] = b"margins:workspace:google-calendar:v1\0";
const MEET_COLLECTION_DOMAIN: &[u8] = b"margins:workspace:google-meet:v1\0";
const GRANOLA_COLLECTION_DOMAIN: &[u8] = b"margins:workspace:granola:v1\0";

fn collection_hash_hex(domain: &[u8], payload: &[u8]) -> String {
    let mut input = domain.to_vec();
    input.extend_from_slice(payload);
    format!("{:x}", Sha256::digest(input))
}

/// Deterministic Margins document namespace for one native Markdown root.
/// Depends only on the declared absolute path; binding display names and Gmail
/// selectors never enter this key.
pub fn native_markdown_collection_namespace(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        bail!(
            "native markdown collection namespace requires an absolute path: {}",
            path.display()
        );
    }
    Ok(format!(
        "markdown_{}",
        collection_hash_hex(
            NATIVE_COLLECTION_DOMAIN,
            path.as_os_str().as_encoded_bytes()
        )
    ))
}

/// Deterministic Margins document namespace for one Gmail account binding.
/// Depends only on the normalized account; display names and query/backfill do
/// not enter this key.
pub fn gmail_collection_namespace(account: &str) -> Result<String> {
    let account = normalize_google_account(account)?;
    Ok(format!(
        "gmail_{}",
        collection_hash_hex(GMAIL_COLLECTION_DOMAIN, account.as_bytes())
    ))
}

/// Deterministic Margins document namespace for one Google Calendar account
/// binding. Depends only on the normalized account; display names and selector
/// windows do not enter this key.
pub fn calendar_collection_namespace(account: &str) -> Result<String> {
    let account = normalize_google_account(account)?;
    Ok(format!(
        "calendar_{}",
        collection_hash_hex(CALENDAR_COLLECTION_DOMAIN, account.as_bytes())
    ))
}

/// Deterministic Margins document namespace for one Google Meet account
/// binding. Depends only on the normalized account.
pub fn meet_collection_namespace(account: &str) -> Result<String> {
    let account = normalize_google_account(account)?;
    Ok(format!(
        "meet_{}",
        collection_hash_hex(MEET_COLLECTION_DOMAIN, account.as_bytes())
    ))
}

/// Deterministic Margins document namespace for one Granola account binding.
/// Depends only on the normalized account; display names and rolling-window
/// selectors do not enter this key.
pub fn granola_collection_namespace(account: &str) -> Result<String> {
    let account = normalize_granola_account(account)?;
    Ok(format!(
        "granola_{}",
        collection_hash_hex(GRANOLA_COLLECTION_DOMAIN, account.as_bytes())
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceEntity {
    Simple(String),
    WithOptions(BTreeMap<String, WorkspaceEntityOptions>),
}

impl WorkspaceEntity {
    pub fn simple(entity_ref: impl Into<String>) -> Self {
        Self::Simple(entity_ref.into())
    }

    pub fn entries(&self) -> Vec<(&str, Option<&WorkspaceEntityOptions>)> {
        match self {
            Self::Simple(entity_ref) => vec![(entity_ref.as_str(), None)],
            Self::WithOptions(entries) => entries
                .iter()
                .map(|(entity_ref, options)| (entity_ref.as_str(), Some(options)))
                .collect(),
        }
    }

    pub fn with_options(entity_ref: impl Into<String>, options: WorkspaceEntityOptions) -> Self {
        Self::WithOptions(BTreeMap::from([(entity_ref.into(), options)]))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntityOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default)]
    pub expandable: bool,
    #[serde(default, skip_serializing)]
    pub children: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePolicy {
    /// Folders left out of every Markdown root, beyond Enzyme's implicit
    /// exclusions (`.git`, `node_modules`, …), which are never written.
    #[serde(default)]
    pub excluded_folders: Vec<String>,
    #[serde(default)]
    pub excluded_tags: Vec<String>,
    /// The curated catalyst entity surface. This is the same shape and meaning
    /// as enzyme-rust's workspace `entities`: a simple entity ref, or an entity
    /// ref with a catalyst profile and/or explicit folder expansion.
    ///
    #[serde(default)]
    pub entities: Vec<WorkspaceEntity>,
    /// Link, tag, or folder entity refs that must never receive catalysts.
    /// Evidence mentioning an excluded entity remains indexed and may support
    /// another entity.
    #[serde(default)]
    pub excluded_entities: Vec<String>,
}

/// Workspace-owned eligibility policy for independently purgeable integration
/// state. `None` retains that class until an explicit destructive purge. A
/// configured age only makes rows eligible for `retention preview/apply`; it
/// never schedules or performs background deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_cache_max_age_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_max_age_days: Option<u32>,
}

impl RetentionPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.raw_cache_max_age_days == Some(0) {
            bail!("retention raw_cache_max_age_days must be greater than zero");
        }
        if self.tombstone_max_age_days == Some(0) {
            bail!("retention tombstone_max_age_days must be greater than zero");
        }
        Ok(())
    }
}

impl Default for WorkspacePolicy {
    fn default() -> Self {
        Self {
            excluded_folders: Vec::new(),
            excluded_tags: Vec::new(),
            entities: Vec::new(),
            excluded_entities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub policy: WorkspacePolicy,
    #[serde(default)]
    pub retention: RetentionPolicy,
    #[serde(default)]
    pub bindings: BTreeMap<String, WorkspaceBinding>,
}

/// One human-readable change in a Workspace plan, derived from the typed view
/// diff. `update_program` covers changes the view does not model (learning
/// settings, profiles, agent policies, formatting); the plan's `diff` is exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WorkspacePlanAction {
    SetPolicy {
        summary: String,
        before: WorkspacePolicy,
        after: WorkspacePolicy,
    },
    AddBinding {
        summary: String,
        name: String,
        binding: WorkspaceBinding,
    },
    UpdateBinding {
        summary: String,
        name: String,
        before: WorkspaceBinding,
        after: WorkspaceBinding,
    },
    RemoveBinding {
        summary: String,
        name: String,
        binding: WorkspaceBinding,
    },
    UpdateProgram {
        summary: String,
    },
}

impl WorkspacePlanAction {
    pub fn summary(&self) -> &str {
        match self {
            Self::SetPolicy { summary, .. }
            | Self::AddBinding { summary, .. }
            | Self::UpdateBinding { summary, .. }
            | Self::RemoveBinding { summary, .. }
            | Self::UpdateProgram { summary } => summary,
        }
    }
}

/// A reviewed change from the current Workspace program to `desired_program`.
/// `apply` writes exactly `desired_program`, and only while the current program
/// still hashes to `base_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePlan {
    pub schema_version: String,
    pub workspace_id: String,
    pub base_revision: String,
    pub plan_id: String,
    pub actions: Vec<WorkspacePlanAction>,
    pub desired_program: String,
    pub desired_sha256: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceActionReceipt {
    pub position: usize,
    pub status: String,
    #[serde(flatten)]
    pub action: WorkspacePlanAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceApplyReceipt {
    pub schema_version: String,
    pub ok: bool,
    pub workspace_id: String,
    pub request_id: String,
    pub request_hash: String,
    pub plan_id: String,
    pub before_revision: String,
    pub after_revision: String,
    pub replayed: bool,
    pub actions: Vec<WorkspaceActionReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceTransaction {
    receipt: WorkspaceApplyReceipt,
    desired_program: String,
}

/// Holds the workspace config lock across an authoritative ledger revision
/// check and its commit, closing the check/commit race with config mutations.
pub struct WorkspaceRevisionGuard {
    _lock: File,
}

#[derive(Debug)]
pub enum WorkspaceMutationError {
    RevisionConflict { expected: String, actual: String },
    IdempotencyConflict { request_id: String },
    InvalidPlan(String),
}

impl std::fmt::Display for WorkspaceMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "workspace revision conflict: expected {expected}, found {actual}"
            ),
            Self::IdempotencyConflict { request_id } => write!(
                formatter,
                "request id '{request_id}' was already used for a different workspace plan"
            ),
            Self::InvalidPlan(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for WorkspaceMutationError {}

/// One resolved Workspace. `program` is the source of truth; `config` is its
/// derived, read-only typed view (plus machine-owned name and retention).
/// Never persist `config`: mutate through [`add_source`], [`remove_source`],
/// [`update_policy`], or a reviewed plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspace {
    pub config: WorkspaceConfig,
    pub program: WorkspaceProgram,
    pub state_dir: PathBuf,
    /// The program file, `<MARGINS_HOME>/configs/<id>.enzyme`.
    pub config_path: PathBuf,
    pub home_dir: PathBuf,
}

impl ResolvedWorkspace {
    /// The reviewed Home destination. The folder is never inferred from cwd.
    pub fn note_destination(&self) -> Result<PathBuf> {
        let folder = self
            .config
            .bindings
            .values()
            .find_map(|binding| match binding {
                WorkspaceBinding::NativeMarkdown {
                    role: SourceRole::Home,
                    note_folder,
                    ..
                } => Some(note_folder),
                _ => None,
            })
            .context("workspace has no Home binding")?;
        folder.as_ref().map_or_else(
            || Ok(self.home_dir.clone()),
            |relative| safe_note_destination(&self.home_dir, relative),
        )
    }
}

fn safe_note_destination(home: &Path, folder: &Path) -> Result<PathBuf> {
    let canonical_home = home
        .canonicalize()
        .with_context(|| format!("reading Home root {}", home.display()))?;
    let mut destination = canonical_home.clone();
    for part in folder.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            bail!("note_folder must stay within the Home root");
        }
        destination.push(part);
        if destination.exists() {
            destination = destination.canonicalize()?;
            if !destination.starts_with(&canonical_home) {
                bail!("note_folder resolves outside the Home root");
            }
        }
    }
    Ok(destination)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceResolution {
    pub workspace: ResolvedWorkspace,
    pub created_implicitly: bool,
}

/// Process-specific roots that implicit Workspace creation must never treat as
/// a notes home. Keeping these inputs explicit makes the isolation policy
/// testable without coupling fixtures to the checkout's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitWorkspaceRoots {
    /// The process user's home directory, when the platform exposes one.
    pub home_dir: Option<PathBuf>,
    /// Margins machine configuration and Workspace state root.
    pub margins_home: PathBuf,
    /// Enzyme configuration and state root, including an environment override.
    pub enzyme_home: Option<PathBuf>,
    /// Platform config/data/cache roots owned by Margins or Enzyme.
    pub config_state_roots: Vec<PathBuf>,
    /// System and environment-selected temporary directory roots.
    pub temp_roots: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ImplicitWorkspaceDenyRootsOverride {
    home_dir: PathBuf,
    enzyme_home: PathBuf,
    config_state_roots: Vec<PathBuf>,
    temp_roots: Vec<PathBuf>,
}

impl ImplicitWorkspaceRoots {
    /// Derive the isolation roots used by a real CLI process.
    pub fn from_process(margins_home: &Path) -> Result<Self> {
        if let Some(raw) = std::env::var_os(IMPLICIT_WORKSPACE_DENY_ROOTS_ENV) {
            let raw = raw.into_string().map_err(|_| {
                anyhow::anyhow!("{IMPLICIT_WORKSPACE_DENY_ROOTS_ENV} must contain UTF-8 JSON")
            })?;
            return Self::from_override(margins_home, &raw);
        }
        let home_dir = dirs::home_dir();
        let enzyme_home = std::env::var_os("ENZYME_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir.as_ref().map(|home| home.join(".enzyme")));
        let config_state_roots = [
            dirs::config_dir(),
            dirs::data_dir(),
            dirs::data_local_dir(),
            dirs::cache_dir(),
            dirs::state_dir(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|base| [base.join("margins"), base.join("enzyme")])
        .collect();
        let mut temp_roots = vec![
            PathBuf::from("/tmp"),
            PathBuf::from("/private/tmp"),
            PathBuf::from("/var/tmp"),
            std::env::temp_dir(),
        ];
        if let Some(tmpdir) = std::env::var_os("TMPDIR").filter(|value| !value.is_empty()) {
            temp_roots.push(PathBuf::from(tmpdir));
        }
        Ok(Self {
            home_dir,
            margins_home: margins_home.to_path_buf(),
            enzyme_home,
            config_state_roots,
            temp_roots,
        })
    }

    fn from_override(margins_home: &Path, raw: &str) -> Result<Self> {
        let override_roots: ImplicitWorkspaceDenyRootsOverride = serde_json::from_str(raw)
            .with_context(|| format!("invalid {IMPLICIT_WORKSPACE_DENY_ROOTS_ENV} JSON"))?;
        if override_roots.config_state_roots.is_empty() || override_roots.temp_roots.is_empty() {
            bail!(
                "{IMPLICIT_WORKSPACE_DENY_ROOTS_ENV} requires non-empty config_state_roots and temp_roots"
            );
        }
        let all_roots = std::iter::once(&override_roots.home_dir)
            .chain(std::iter::once(&override_roots.enzyme_home))
            .chain(override_roots.config_state_roots.iter())
            .chain(override_roots.temp_roots.iter());
        if let Some(path) = all_roots.into_iter().find(|path| !path.is_absolute()) {
            bail!(
                "{IMPLICIT_WORKSPACE_DENY_ROOTS_ENV} paths must be absolute: {}",
                path.display()
            );
        }
        Ok(Self {
            home_dir: Some(override_roots.home_dir),
            margins_home: margins_home.to_path_buf(),
            enzyme_home: Some(override_roots.enzyme_home),
            config_state_roots: override_roots.config_state_roots,
            temp_roots: override_roots.temp_roots,
        })
    }
}

impl ResolvedWorkspace {
    pub fn ledger_path(&self) -> PathBuf {
        self.state_dir.join("ledger.db")
    }

    pub fn recall_path(&self) -> PathBuf {
        self.state_dir.join(INDEX_DB)
    }

    pub fn captures_dir(&self) -> PathBuf {
        self.state_dir.join("captures")
    }

    /// Resolve the one declared writable capture destination for this
    /// Workspace. Capture routing is declaration-owned: callers must not fall
    /// back to their cwd or silently choose between multiple stores.
    pub fn capture_store_dir(&self) -> Result<PathBuf> {
        let mut captures =
            self.config
                .bindings
                .iter()
                .filter_map(|(name, binding)| match binding {
                    WorkspaceBinding::Captures { path } => Some((name.as_str(), path)),
                    _ => None,
                });
        let Some((name, path)) = captures.next() else {
            bail!(
                "workspace '{}' has no declared captures source",
                self.config.id
            );
        };
        if let Some((other, _)) = captures.next() {
            bail!(
                "workspace '{}' has multiple writable capture destinations ('{}' and '{}'); remove one before recording",
                self.config.id,
                name,
                other
            );
        }
        if !path.is_absolute() {
            bail!(
                "workspace '{}' capture destination is not absolute: {}",
                self.config.id,
                path.display()
            );
        }
        Ok(path.clone())
    }

    /// Machine-level Google transport state for one declared account.
    pub fn google_dir(&self, account: &str) -> Result<PathBuf> {
        let margins_home = self
            .state_dir
            .parent()
            .and_then(Path::parent)
            .context("workspace state directory has no Margins home")?;
        google_account_dir(margins_home, account)
    }
}

/// Machine preference used by clients outside any declared Source folder.
pub fn default_workspace(margins_home: &Path) -> Result<Option<String>> {
    machine_config::ensure_migrated(margins_home)?;
    default_workspace_locked(margins_home)
}

/// [`default_workspace`] for a caller holding the machine lock.
fn default_workspace_locked(margins_home: &Path) -> Result<Option<String>> {
    let config = read_machine_config_locked(margins_home)?;
    let selected = config
        .get("workspace")
        .and_then(|value| value.get("default"));
    match selected {
        None => Ok(None),
        Some(toml::Value::String(id)) if is_reserved_id(id) => Err(anyhow::anyhow!(
            "the machine default Workspace is '{id}': {}",
            reserved_id_error(id)
        )),
        Some(toml::Value::String(id)) => {
            validate_id(id)?;
            Ok(Some(id.clone()))
        }
        Some(_) => bail!("workspace.default must be a Workspace id"),
    }
}

pub fn set_default_workspace(margins_home: &Path, id: &str) -> Result<()> {
    resolve_at(margins_home, id)?;
    update_machine_config(margins_home, |table| {
        machine_table(table, "workspace")?
            .insert("default".to_string(), toml::Value::String(id.to_string()));
        Ok(())
    })
}

fn read_machine_config(margins_home: &Path) -> Result<toml::Table> {
    machine_config::ensure_migrated(margins_home)?;
    read_machine_config_locked(margins_home)
}

/// Machine `margins.toml` for a caller holding the machine lock (or after
/// [`machine_config::ensure_migrated`]).
fn read_machine_config_locked(margins_home: &Path) -> Result<toml::Table> {
    match machine_config::read_machine_config_text_locked(margins_home)? {
        Some(raw) if !raw.trim().is_empty() => {
            toml::from_str(&raw).context("invalid machine config")
        }
        _ => Ok(toml::Table::new()),
    }
}

/// Read-modify-write the machine `margins.toml` under the machine config lock.
fn update_machine_config(
    margins_home: &Path,
    mutate: impl FnOnce(&mut toml::Table) -> Result<()>,
) -> Result<()> {
    let _lock = machine_config::lock_machine(margins_home)?;
    machine_config::migrate_locked(margins_home)?;
    let mut config = read_machine_config_locked(margins_home)?;
    mutate(&mut config)?;
    let rendered = toml::to_string_pretty(&config)?;
    atomic_write(&machine_config::machine_config_path(margins_home), rendered.as_bytes())
}

fn machine_table<'a>(table: &'a mut toml::Table, key: &str) -> Result<&'a mut toml::Table> {
    table
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .with_context(|| format!("machine config [{key}] must be a table"))
}

/// Optional display name of a Workspace, kept in machine config
/// `[workspace.names]` because the program names the Workspace by id only.
pub fn workspace_display_name(margins_home: &Path, id: &str) -> Result<Option<String>> {
    let config = read_machine_config(margins_home)?;
    match config
        .get("workspace")
        .and_then(|workspace| workspace.get("names"))
        .and_then(|names| names.get(id))
    {
        None => Ok(None),
        Some(toml::Value::String(name)) => Ok(Some(name.clone())),
        Some(_) => bail!("workspace.names.{id} must be a string"),
    }
}

pub fn set_workspace_display_name(margins_home: &Path, id: &str, name: Option<&str>) -> Result<()> {
    validate_id(id)?;
    update_machine_config(margins_home, |table| {
        let workspace = machine_table(table, "workspace")?;
        match name {
            Some(name) => {
                machine_table(workspace, "names")?
                    .insert(id.to_string(), toml::Value::String(name.to_string()));
            }
            None => {
                if let Some(names) = workspace.get_mut("names").and_then(toml::Value::as_table_mut) {
                    names.remove(id);
                }
            }
        }
        Ok(())
    })
}

/// Effective retention for one Workspace from machine config: `[retention.<id>]`
/// when present, otherwise the global `[retention]` keys.
pub fn retention_policy(margins_home: &Path, id: &str) -> Result<RetentionPolicy> {
    let config = read_machine_config(margins_home)?;
    let Some(retention) = config.get("retention") else {
        return Ok(RetentionPolicy::default());
    };
    let table = retention
        .as_table()
        .context("machine config [retention] must be a table")?;
    let selected = match table.get(id) {
        Some(toml::Value::Table(specific)) => specific.clone(),
        Some(_) => bail!("machine config retention.{id} must be a table"),
        None => table
            .iter()
            .filter(|(_, value)| !value.is_table())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    };
    let policy: RetentionPolicy = toml::Value::Table(selected)
        .try_into()
        .context("invalid machine retention policy")?;
    policy.validate()?;
    Ok(policy)
}

/// Record a Workspace-specific retention override in machine config. A default
/// policy removes the override so the global `[retention]` applies.
pub fn set_workspace_retention(
    margins_home: &Path,
    id: &str,
    policy: &RetentionPolicy,
) -> Result<()> {
    validate_id(id)?;
    policy.validate()?;
    update_machine_config(margins_home, |table| {
        let retention = machine_table(table, "retention")?;
        if *policy == RetentionPolicy::default() {
            retention.remove(id);
        } else {
            retention.insert(id.to_string(), toml::Value::try_from(policy)?);
        }
        if retention.is_empty() {
            table.remove("retention");
        }
        Ok(())
    })
}

pub fn margins_home() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("MARGINS_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".margins"))
}

/// Resolve the machine-level connection home for a normalized Google account.
/// The account remains visible in the path so operators can inspect ownership;
/// path separators and non-email identifiers are refused.
pub fn google_account_dir(margins_home: &Path, account: &str) -> Result<PathBuf> {
    let account = normalize_google_account(account)?;
    Ok(margins_home.join("google").join(account))
}

pub fn normalize_google_account(account: &str) -> Result<String> {
    normalize_email_account(account, "Google connections require a valid account email")
}

pub fn normalize_granola_account(account: &str) -> Result<String> {
    normalize_email_account(account, "Granola sources require a valid account email")
}

fn normalize_email_account(account: &str, invalid_message: &str) -> Result<String> {
    let account = account.trim().to_ascii_lowercase();
    let Some((local, domain)) = account.split_once('@') else {
        bail!("{invalid_message}");
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !account
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '_' | '+' | '-'))
    {
        bail!("{invalid_message}");
    }
    Ok(account)
}

/// Return every Workspace id that declares any Google source for `account`.
pub fn workspaces_using_google_account(margins_home: &Path, account: &str) -> Result<Vec<String>> {
    let account = normalize_google_account(account)?;
    Ok(list_workspaces(margins_home)?
        .into_iter()
        .filter(|workspace| {
            workspace.config.bindings.values().any(|binding| {
                matches!(
                    binding.kind(),
                    SourceKind::GoogleMail | SourceKind::GoogleCalendar | SourceKind::GoogleMeet
                ) && binding
                    .google_account()
                    .and_then(|declared| normalize_google_account(declared).ok())
                    .is_some_and(|declared| declared == account)
            })
        })
        .map(|workspace| workspace.config.id)
        .collect())
}

/// Return every Workspace id that declares any Granola source for `account`.
pub fn workspaces_using_granola_account(margins_home: &Path, account: &str) -> Result<Vec<String>> {
    let account = normalize_granola_account(account)?;
    Ok(list_workspaces(margins_home)?
        .into_iter()
        .filter(|workspace| {
            workspace.config.bindings.values().any(|binding| {
                matches!(binding.kind(), SourceKind::Granola)
                    && binding
                        .google_account()
                        .and_then(|declared| normalize_granola_account(declared).ok())
                        .is_some_and(|declared| declared == account)
            })
        })
        .map(|workspace| workspace.config.id)
        .collect())
}

pub fn workspace_state_dir(home: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(home.join(WORKSPACES_DIR).join(id))
}

/// The Workspace program, `<MARGINS_HOME>/configs/<id>.enzyme`.
pub fn workspace_program_path(home: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(home.join(CONFIGS_DIR).join(format!("{id}.enzyme")))
}

fn margins_home_of_state_dir(state_dir: &Path) -> Result<&Path> {
    let workspaces = state_dir
        .parent()
        .context("workspace state directory has no parent")?;
    if workspaces.file_name().and_then(|value| value.to_str()) != Some(WORKSPACES_DIR) {
        bail!("workspace state directory must live under {WORKSPACES_DIR}/<id>");
    }
    workspaces
        .parent()
        .context("workspace state directory has no Margins home")
}

fn state_dir_id(state_dir: &Path) -> Result<&str> {
    state_dir
        .file_name()
        .and_then(|value| value.to_str())
        .context("workspace state directory has no valid id")
}

fn program_path_of_state_dir(state_dir: &Path) -> Result<PathBuf> {
    workspace_program_path(margins_home_of_state_dir(state_dir)?, state_dir_id(state_dir)?)
}

/// The program of a new Workspace with one Home and one captures store.
fn initial_config(id: &str, home_notes: &Path, capture_store: &Path) -> WorkspaceConfig {
    WorkspaceConfig {
        id: id.to_string(),
        name: None,
        policy: WorkspacePolicy::default(),
        retention: RetentionPolicy::default(),
        bindings: BTreeMap::from([
            (
                "home".to_string(),
                WorkspaceBinding::NativeMarkdown {
                    path: home_notes.to_path_buf(),
                    role: SourceRole::Home,
                    note_folder: None,
                },
            ),
            (
                "captures".to_string(),
                WorkspaceBinding::Captures {
                    path: capture_store.to_path_buf(),
                },
            ),
        ]),
    }
}

/// Write a brand-new Workspace program; refuses to replace an existing one.
fn write_new_program(margins_home: &Path, config: &WorkspaceConfig) -> Result<()> {
    let path = workspace_program_path(margins_home, &config.id)?;
    if path.exists() {
        bail!("workspace '{}' already exists at {}", config.id, path.display());
    }
    let program = program_from_config(config, &program_lang::empty_program(&config.id), FolderQualification::Program)?;
    validate_program(margins_home, &program)?;
    atomic_write(&path, program.text().as_bytes())
}

pub fn create_workspace(
    margins_home: &Path,
    id: &str,
    name: Option<&str>,
    home_notes: &Path,
) -> Result<ResolvedWorkspace> {
    validate_id(id)?;
    let home_notes = home_notes
        .canonicalize()
        .with_context(|| format!("home notes folder does not exist: {}", home_notes.display()))?;
    if !home_notes.is_dir() {
        bail!(
            "home notes source is not a directory: {}",
            home_notes.display()
        );
    }
    let state_dir = workspace_state_dir(margins_home, id)?;
    let program_path = workspace_program_path(margins_home, id)?;
    if state_dir.exists() || program_path.exists() {
        bail!("workspace '{id}' already exists at {}", state_dir.display());
    }
    std::fs::create_dir_all(state_dir.join("captures"))
        .with_context(|| format!("creating workspace state at {}", state_dir.display()))?;
    let config = initial_config(id, &home_notes, &state_dir.join("captures"));
    write_new_program(margins_home, &config)?;
    if let Some(name) = name {
        set_workspace_display_name(margins_home, id, Some(name))?;
    }
    resolve_at(margins_home, id)
}

/// Provision an explicitly routed service Workspace without inferring either
/// its notes home or capture authority from the server process cwd.
///
/// Reopening is data preserving: an existing declaration is accepted only
/// when both paths still match. The caller must use the reviewed plan/apply
/// workflow to change either binding.
pub fn ensure_service_workspace(
    margins_home: &Path,
    id: &str,
    name: Option<&str>,
    home_notes: &Path,
    capture_store: &Path,
) -> Result<ResolvedWorkspace> {
    validate_id(id)?;
    let home_notes = home_notes
        .canonicalize()
        .with_context(|| format!("notes folder does not exist: {}", home_notes.display()))?;
    let capture_store = if capture_store.exists() {
        capture_store
            .canonicalize()
            .with_context(|| format!("capture store does not exist: {}", capture_store.display()))?
    } else {
        if !capture_store.is_absolute() {
            bail!(
                "capture store must be absolute: {}",
                capture_store.display()
            );
        }
        std::fs::create_dir_all(capture_store)?;
        capture_store.canonicalize()?
    };
    if let Ok(existing) = resolve_at(margins_home, id) {
        let declared_capture = existing.capture_store_dir()?;
        if existing.home_dir != home_notes || declared_capture != capture_store {
            bail!(
                "workspace '{id}' is already mapped to different paths; use workspace plan/apply to change it"
            );
        }
        return Ok(existing);
    }
    let state_dir = workspace_state_dir(margins_home, id)?;
    if state_dir.exists() || workspace_program_path(margins_home, id)?.exists() {
        bail!(
            "workspace '{id}' has incomplete state at {}; inspect it before retrying",
            state_dir.display()
        );
    }
    std::fs::create_dir_all(&state_dir)?;
    let config = initial_config(id, &home_notes, &capture_store);
    write_new_program(margins_home, &config)?;
    if let Some(name) = name {
        set_workspace_display_name(margins_home, id, Some(name))?;
    }
    resolve_at(margins_home, id)
}

pub fn resolve_workspace(
    margins_home: &Path,
    selector: Option<&str>,
    cwd: &Path,
) -> Result<ResolvedWorkspace> {
    let env_selector = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if let Some(id) = selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| env_selector.as_deref())
    {
        return resolve_at(margins_home, id);
    }

    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut matches = list_workspaces(margins_home)?
        .into_iter()
        .filter(|workspace| {
            cwd.starts_with(&workspace.home_dir)
                || workspace
                    .config
                    .bindings
                    .values()
                    .filter_map(|binding| binding.local_path())
                    .any(|path| cwd.starts_with(path))
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!(
            "no workspace selected; pass --workspace <id>, set MARGINS_WORKSPACE, or run inside a declared source folder"
        ),
        _ => bail!(
            "multiple workspaces declare this folder; pass --workspace <id> explicitly"
        ),
    }
}

/// Resolve an explicit or cwd-bound Workspace, creating the single-folder
/// implicit case only after the cwd passes the isolation deny-list.
pub fn resolve_or_create_workspace(
    roots: &ImplicitWorkspaceRoots,
    selector: Option<&str>,
    cwd: &Path,
) -> Result<WorkspaceResolution> {
    let margins_home = &roots.margins_home;
    let env_selector = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if selector.is_some_and(|value| !value.trim().is_empty()) || env_selector.is_some() {
        return Ok(WorkspaceResolution {
            workspace: resolve_workspace(margins_home, selector, cwd)?,
            created_implicitly: false,
        });
    }

    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("current directory does not exist: {}", cwd.display()))?;
    let workspaces = list_workspaces(margins_home)?;
    let mut matches = workspaces
        .iter()
        .filter(|workspace| {
            cwd.starts_with(&workspace.home_dir)
                || workspace
                    .config
                    .bindings
                    .values()
                    .filter_map(|binding| binding.local_path())
                    .any(|path| cwd.starts_with(path))
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => {
            return Ok(WorkspaceResolution {
                workspace: matches.remove(0).clone(),
                created_implicitly: false,
            });
        }
        count if count > 1 => {
            bail!("multiple workspaces declare this folder; pass --workspace <id> explicitly")
        }
        _ => {}
    }

    let candidate_id = available_implicit_id(margins_home, &cwd)?;
    refuse_unsafe_implicit_home(&cwd, &workspaces, &candidate_id, roots)?;
    let workspace = create_workspace(margins_home, &candidate_id, None, &cwd)?;
    Ok(WorkspaceResolution {
        workspace,
        created_implicitly: true,
    })
}

fn refuse_unsafe_implicit_home(
    cwd: &Path,
    workspaces: &[ResolvedWorkspace],
    candidate_id: &str,
    roots: &ImplicitWorkspaceRoots,
) -> Result<()> {
    let explicit = || {
        format!(
            "margins workspace new {candidate_id} --home {}",
            cwd.display()
        )
    };
    let refuse = |reason: &str| -> Result<()> {
        bail!(
            "cannot create implicit workspace: {reason}; use explicit setup instead: {}",
            explicit()
        )
    };

    if cwd.parent().is_none() {
        return refuse("current directory is the filesystem root");
    }
    if roots
        .home_dir
        .as_deref()
        .map(|path| canonical_or_absolute(path, cwd))
        .is_some_and(|home| cwd == home)
    {
        return refuse("current directory is the user home directory");
    }

    let canonical_margins_home = canonical_or_absolute(&roots.margins_home, cwd);
    if cwd.starts_with(&canonical_margins_home) {
        return refuse("current directory is Margins configuration or state storage");
    }
    for state_name in [".margins", ".enzyme"] {
        if cwd
            .components()
            .any(|component| component.as_os_str() == state_name)
        {
            return refuse("current directory is Margins or Enzyme configuration or state storage");
        }
    }
    if let Some(enzyme_home) = roots.enzyme_home.as_deref() {
        if cwd.starts_with(canonical_or_absolute(enzyme_home, cwd)) {
            return refuse("current directory is Enzyme configuration or state storage");
        }
    }
    if roots
        .config_state_roots
        .iter()
        .map(|root| canonical_or_absolute(root, cwd))
        .any(|root| cwd.starts_with(root))
    {
        return refuse("current directory is Margins or Enzyme configuration or state storage");
    }

    if workspaces
        .iter()
        .any(|workspace| workspace.home_dir.starts_with(cwd) && workspace.home_dir != cwd)
    {
        return refuse("current directory is above a declared workspace home");
    }

    if roots
        .temp_roots
        .iter()
        .map(|root| canonical_or_absolute(root, cwd))
        .any(|root| cwd.starts_with(root))
    {
        return refuse("current directory is inside a temporary directory");
    }

    let has_markdown = WalkDir::new(cwd)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | ".margins" | ".enzyme")
                )
        })
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("md")
                            || extension.eq_ignore_ascii_case("markdown")
                    })
        });
    if !has_markdown {
        return refuse("current directory contains no Markdown note evidence");
    }
    Ok(())
}

fn available_implicit_id(margins_home: &Path, cwd: &Path) -> Result<String> {
    let base = implicit_id(cwd);
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while workspace_state_dir(margins_home, &candidate)?.exists() {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(candidate)
}

fn implicit_id(cwd: &Path) -> String {
    let name = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("notes");
    let mut id = String::new();
    let mut pending_hyphen = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !id.is_empty() {
                id.push('-');
            }
            id.push(ch);
            pending_hyphen = false;
        } else {
            pending_hyphen = !id.is_empty();
        }
    }
    if id.is_empty() {
        "notes".to_string()
    } else {
        id
    }
}

fn canonical_or_absolute(path: &Path, cwd: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    })
}

pub fn resolve_at(margins_home: &Path, id: &str) -> Result<ResolvedWorkspace> {
    if is_reserved_id(id) {
        return Err(reserved_id_error(id));
    }
    let state_dir = workspace_state_dir(margins_home, id)?;
    let program_path = workspace_program_path(margins_home, id)?;
    if state_dir.join(LEGACY_WORKSPACE_CONFIG).is_file() {
        // Migrate, or retire a legacy file left beside an existing program.
        migrate_workspace(margins_home, id)?;
    }
    adopt_engine_index_name(&state_dir)?;
    let text = std::fs::read_to_string(&program_path)
        .with_context(|| format!("workspace '{id}' not found at {}", program_path.display()))?;
    let program = WorkspaceProgram::parse(&text)
        .with_context(|| format!("invalid workspace program {}", program_path.display()))?;
    if program.id() != id {
        bail!(
            "workspace program {} declares workspace '{}', expected '{id}'",
            program_path.display(),
            program.id()
        );
    }
    resolved_from_program(margins_home, state_dir, program_path, program)
}

fn resolved_from_program(
    margins_home: &Path,
    state_dir: PathBuf,
    config_path: PathBuf,
    program: WorkspaceProgram,
) -> Result<ResolvedWorkspace> {
    let config = view_of(margins_home, &program)
        .with_context(|| format!("invalid workspace program {}", config_path.display()))?;
    let home_dir = config
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::NativeMarkdown {
                path,
                role: SourceRole::Home,
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .context("workspace must declare exactly one Home Markdown source")?;
    Ok(ResolvedWorkspace {
        config,
        program,
        state_dir,
        config_path,
        home_dir,
    })
}

/// Rename a Workspace index from `index.db` to `enzyme.db` (with its SQLite
/// sidecars) under the Workspace lock. The database and the `index.identity`
/// marker are untouched, so nothing is reindexed. Sidecars move before the
/// main file: a crash in between leaves `index.db` in place and the rename
/// resumes on the next resolution. No step ever replaces an existing file:
/// when both names exist, `enzyme.db` is the index and `index.db` is left for
/// the user; a sidecar whose destination already exists stops the rename.
///
/// An older Margins (CLI or `margins-server`) that still has `index.db` open
/// keeps writing to the renamed files, so the CLI and the server must be
/// upgraded together.
fn adopt_engine_index_name(state_dir: &Path) -> Result<()> {
    let legacy = state_dir.join(LEGACY_INDEX_DB);
    if !legacy.exists() {
        return Ok(());
    }
    let _lock = lock_workspace(state_dir)?;
    let current = state_dir.join(INDEX_DB);
    if !legacy.exists() {
        return Ok(());
    }
    if current.exists() {
        log::warn!(
            "{} is the Workspace index; leaving the older {} in place",
            current.display(),
            legacy.display()
        );
        return Ok(());
    }
    for suffix in SQLITE_SIDECARS {
        let from = state_dir.join(format!("{LEGACY_INDEX_DB}{suffix}"));
        if from.exists() {
            rename_no_replace(&from, &state_dir.join(format!("{INDEX_DB}{suffix}")))?;
        }
    }
    rename_no_replace(&legacy, &current)?;
    sync_parent_directory(&current)
}

/// Rename `from` to `to`, refusing to replace an existing `to`. A hard link
/// fails atomically when `to` exists; the source is removed only after the
/// link succeeded. Filesystems without hard links fall back to a checked
/// rename under the caller's lock.
fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => std::fs::remove_file(from)
            .with_context(|| format!("removing {} after linking {}", from.display(), to.display())),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists || to.exists() => bail!(
            "cannot rename {} to {}: the destination already exists; move one of them aside",
            from.display(),
            to.display()
        ),
        Err(_) => std::fs::rename(from, to)
            .with_context(|| format!("renaming {} to {}", from.display(), to.display())),
    }
}

/// Derive and validate the typed view of a program, including machine-owned
/// display name and retention, and validate the program language itself.
fn view_of(margins_home: &Path, program: &WorkspaceProgram) -> Result<WorkspaceConfig> {
    let id = program.id();
    validate_id(id)?;
    let config = program_lang::derive_view(
        program,
        workspace_display_name(margins_home, id)?,
        retention_policy(margins_home, id)?,
    )?;
    validate_config(&config)?;
    program_lang::validate_language(
        program,
        &margins_home.join(WORKSPACES_DIR).join(id).join("ledger.db"),
        shared_profiles(margins_home)?.as_ref(),
    )?;
    Ok(config)
}

fn validate_program(margins_home: &Path, program: &WorkspaceProgram) -> Result<WorkspaceConfig> {
    view_of(margins_home, program)
}

/// The optional shared `configs/profiles.enzyme` program (profiles only).
pub fn shared_profiles(margins_home: &Path) -> Result<Option<enzyme_spec::Program>> {
    let path = margins_home.join(CONFIGS_DIR).join(SHARED_PROFILES_PROGRAM);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let program = enzyme_spec::parse(&text)
        .with_context(|| format!("invalid shared profiles {}", path.display()))?;
    if !program.workspaces.is_empty()
        || !program.vaults.is_empty()
        || program.settings != enzyme_spec::Settings::default()
        || program.learning != enzyme_spec::Learning::default()
        || program.retrieval.is_some()
    {
        bail!("{} may only define profiles", path.display());
    }
    Ok(Some(program))
}

/// Render the program whose view is `config`, starting from `base` and
/// preserving everything the view does not model.
fn program_from_config(
    config: &WorkspaceConfig,
    base: &enzyme_spec::Program,
    qualification: FolderQualification,
) -> Result<WorkspaceProgram> {
    validate_config(config)?;
    let program = program_lang::reconcile(base, config, qualification)?;
    WorkspaceProgram::from_program(program)
}

/// Convert a retired `config.toml` Workspace config to its program,
/// deterministically. Name and retention are machine-owned and not part of the
/// program; see [`migrate_workspace`].
/// Returns the program and what the previous engine ignored and is therefore
/// not migrated (see [`program_lang::legacy_view`]).
pub fn program_from_legacy_config(config: &WorkspaceConfig) -> Result<(WorkspaceProgram, Vec<String>)> {
    let legacy = program_lang::legacy_view(config)?;
    let program = program_from_config(
        &legacy.config,
        &program_lang::empty_program(&config.id),
        FolderQualification::Legacy,
    )?;
    Ok((program, legacy.warnings))
}

/// Result of migrating one retired `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceMigration {
    pub schema_version: String,
    pub workspace_id: String,
    /// `migrated`, `would_migrate` (dry run), or `already_migrated`.
    pub status: String,
    pub program_path: PathBuf,
    pub program: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_backup: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_override: Option<RetentionPolicy>,
    /// Legacy settings the previous engine ignored and that are therefore not
    /// part of the program.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn read_legacy_config(path: &Path) -> Result<WorkspaceConfig> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading workspace config {}", path.display()))?;
    toml::from_str(&body).with_context(|| format!("invalid workspace config {}", path.display()))
}

/// Preview the program a retired `config.toml` migrates to, without writing.
pub fn preview_workspace_migration(margins_home: &Path, id: &str) -> Result<WorkspaceMigration> {
    let state_dir = workspace_state_dir(margins_home, id)?;
    let program_path = workspace_program_path(margins_home, id)?;
    if program_path.exists() {
        let program = std::fs::read_to_string(&program_path)?;
        return Ok(WorkspaceMigration {
            schema_version: WORKSPACE_MIGRATE_SCHEMA.to_string(),
            workspace_id: id.to_string(),
            status: "already_migrated".to_string(),
            program_path,
            revision: program_sha256(&program),
            program,
            legacy_backup: None,
            retention_override: None,
            warnings: Vec::new(),
        });
    }
    let legacy = read_legacy_config(&state_dir.join(LEGACY_WORKSPACE_CONFIG))?;
    if legacy.id != id {
        bail!("workspace directory '{id}' contains config for '{}'", legacy.id);
    }
    let (program, warnings) = program_from_legacy_config(&legacy)?;
    validate_program(margins_home, &program)?;
    Ok(WorkspaceMigration {
        schema_version: WORKSPACE_MIGRATE_SCHEMA.to_string(),
        workspace_id: id.to_string(),
        status: "would_migrate".to_string(),
        program_path,
        revision: program.sha256(),
        program: program.text().to_string(),
        legacy_backup: Some(legacy_backup_path(&state_dir)),
        retention_override: (legacy.retention != RetentionPolicy::default())
            .then_some(legacy.retention),
        warnings,
    })
}

/// The first free retirement name: `config.toml.migrated`, then
/// `config.toml.migrated.1`, `.2`, ….
fn legacy_backup_path(state_dir: &Path) -> PathBuf {
    let first = state_dir.join(LEGACY_WORKSPACE_CONFIG_MIGRATED);
    if !first.exists() {
        return first;
    }
    (1u32..)
        .map(|n| state_dir.join(format!("{LEGACY_WORKSPACE_CONFIG_MIGRATED}.{n}")))
        .find(|path| !path.exists())
        .expect("a free retirement name")
}

/// Whether `name` is a retired legacy config (`config.toml.migrated[.<n>]`).
fn is_legacy_backup_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name == LEGACY_WORKSPACE_CONFIG_MIGRATED
            || name
                .strip_prefix(LEGACY_WORKSPACE_CONFIG_MIGRATED)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Rename a legacy `config.toml` out of the way; the caller holds the lock.
fn retire_legacy_config(state_dir: &Path) -> Result<PathBuf> {
    let legacy_path = state_dir.join(LEGACY_WORKSPACE_CONFIG);
    let backup = legacy_backup_path(state_dir);
    std::fs::rename(&legacy_path, &backup)
        .with_context(|| format!("retiring {}", legacy_path.display()))?;
    sync_parent_directory(&backup)?;
    Ok(backup)
}

/// Migrate a retired `workspaces/<id>/config.toml` to `configs/<id>.enzyme`.
///
/// Under the Workspace lock: convert deterministically, validate, move the
/// display name and a non-default retention policy to machine config, write
/// the program atomically, then rename the old file to `config.toml.migrated`.
/// Idempotent: an existing program is never replaced.
pub fn migrate_workspace(margins_home: &Path, id: &str) -> Result<WorkspaceMigration> {
    let state_dir = workspace_state_dir(margins_home, id)?;
    let _lock = lock_workspace(&state_dir)?;
    let legacy_path = state_dir.join(LEGACY_WORKSPACE_CONFIG);
    let program_path = workspace_program_path(margins_home, id)?;
    if program_path.exists() {
        // A concurrent resolver migrated first, a crash came between the
        // program write and the rename, or the program was authored directly.
        // Retire a leftover legacy file without reinterpreting it.
        let mut migration = preview_workspace_migration(margins_home, id)?;
        if legacy_path.is_file() {
            migration.legacy_backup = Some(retire_legacy_config(&state_dir)?);
        }
        return Ok(migration);
    }
    let legacy = read_legacy_config(&legacy_path)?;
    if legacy.id != id {
        bail!("workspace directory '{id}' contains config for '{}'", legacy.id);
    }
    // Convert and validate before anything is written, so a Workspace that
    // cannot migrate leaves its legacy file and machine config untouched.
    let (program, warnings) = program_from_legacy_config(&legacy)
        .with_context(|| format!("migrating {}", legacy_path.display()))?;
    validate_program(margins_home, &program)
        .with_context(|| format!("migrating {}", legacy_path.display()))?;
    // Machine-owned settings next: a crash before the program write leaves
    // the legacy file authoritative and the migration simply runs again.
    if legacy.name.is_some() && workspace_display_name(margins_home, id)? != legacy.name {
        set_workspace_display_name(margins_home, id, legacy.name.as_deref())?;
    }
    if legacy.retention != RetentionPolicy::default() {
        set_workspace_retention(margins_home, id, &legacy.retention)?;
    }
    atomic_write(&program_path, program.text().as_bytes())?;
    let backup = retire_legacy_config(&state_dir)?;
    for warning in &warnings {
        log::warn!("migrated Workspace '{id}': {warning}");
    }
    Ok(WorkspaceMigration {
        schema_version: WORKSPACE_MIGRATE_SCHEMA.to_string(),
        workspace_id: id.to_string(),
        status: "migrated".to_string(),
        program_path,
        revision: program.sha256(),
        program: program.text().to_string(),
        legacy_backup: Some(backup),
        retention_override: (legacy.retention != RetentionPolicy::default())
            .then_some(legacy.retention),
        warnings,
    })
}

/// Ids of every Workspace with a retired `config.toml` still to migrate.
pub fn legacy_workspace_ids(margins_home: &Path) -> Result<Vec<String>> {
    let root = margins_home.join(WORKSPACES_DIR);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(&root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry?;
        let Ok(id) = entry.file_name().into_string() else {
            continue;
        };
        // Reserved ids are listed too, so they surface with their rename path
        // instead of being skipped.
        if validate_id_format(&id).is_ok()
            && entry.path().join(LEGACY_WORKSPACE_CONFIG).is_file()
            && (is_reserved_id(&id)
                || !margins_home.join(CONFIGS_DIR).join(format!("{id}.enzyme")).exists())
        {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

pub fn resolve_state_dir(state_dir: &Path) -> Result<ResolvedWorkspace> {
    resolve_at(margins_home_of_state_dir(state_dir)?, state_dir_id(state_dir)?)
}

/// Every Workspace that resolves. A Workspace that cannot resolve (an invalid
/// program, or a legacy config that cannot migrate) is skipped with a warning
/// rather than hiding every other Workspace; see [`list_workspace_entries`].
pub fn list_workspaces(margins_home: &Path) -> Result<Vec<ResolvedWorkspace>> {
    Ok(list_workspace_entries(margins_home)?
        .into_iter()
        .filter_map(|(id, resolved)| match resolved {
            Ok(workspace) => Some(workspace),
            Err(error) => {
                log::warn!("skipping Workspace '{id}': {error:#}");
                None
            }
        })
        .collect())
}

/// Every declared Workspace id with its resolution, in id order.
pub fn list_workspace_entries(
    margins_home: &Path,
) -> Result<Vec<(String, Result<ResolvedWorkspace>)>> {
    let mut ids = std::collections::BTreeSet::new();
    let configs = margins_home.join(CONFIGS_DIR);
    if configs.exists() {
        for entry in
            std::fs::read_dir(&configs).with_context(|| format!("reading {}", configs.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("enzyme")
                || !path.is_file()
            {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|value| value.to_str()) {
                let file = path.file_name().and_then(|value| value.to_str());
                if !RESERVED_PROGRAMS.iter().any(|reserved| Some(*reserved) == file)
                    || reserved_program_declares_workspace(&path)
                {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    ids.extend(legacy_workspace_ids(margins_home)?);
    Ok(ids
        .into_iter()
        .map(|id| {
            let resolved = resolve_at(margins_home, &id);
            (id, resolved)
        })
        .collect())
}

/// Result of [`rename_reserved_workspace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceRename {
    pub old_id: String,
    pub new_id: String,
    pub program_path: PathBuf,
    pub state_dir: PathBuf,
    /// Where the previous declaration was kept.
    pub retired: PathBuf,
    /// `workspaces/<old>` now points at the moved state directory, so paths
    /// recorded inside it (captures, sessions) keep resolving.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility_link: Option<PathBuf>,
}

/// Rename a Workspace whose id became reserved (`settings`, `profiles`,
/// `margins-sources`) to `new_id`, keeping all of its data.
///
/// Under the machine lock, everything is read and validated first. Then the
/// state directory moves to `workspaces/<new>` (leaving a `workspaces/<old>`
/// symlink so absolute paths recorded in its stores keep resolving), the
/// renamed program is written, the old declaration is retired (never deleted:
/// `configs/<old>.enzyme.renamed-to-<new>` or `config.toml.migrated`), and the
/// machine default, display name, and retention override follow the new id.
/// Re-running after an interruption resumes.
///
/// Only reserved ids are renamed: other ids are referenced from places this
/// command cannot update (sessions, server and plugin selections), and those
/// Workspaces keep working under their id.
pub fn rename_reserved_workspace(
    margins_home: &Path,
    old_id: &str,
    new_id: &str,
) -> Result<WorkspaceRename> {
    if !is_reserved_id(old_id) {
        bail!("`workspace rename` only renames Workspaces whose id is now reserved (settings, profiles, margins-sources); '{old_id}' is not");
    }
    validate_id(new_id)?;
    let _machine = machine_config::lock_machine(margins_home)?;
    let configs = margins_home.join(CONFIGS_DIR);
    let old_program_path = configs.join(format!("{old_id}.enzyme"));
    let new_program_path = configs.join(format!("{new_id}.enzyme"));
    let old_state = margins_home.join(WORKSPACES_DIR).join(old_id);
    let new_state = margins_home.join(WORKSPACES_DIR).join(new_id);
    let state_moved = std::fs::symlink_metadata(&old_state)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && std::fs::read_link(&old_state).is_ok_and(|target| target == Path::new(new_id));
    let rewrite = |path: &Path| -> PathBuf {
        path.strip_prefix(&old_state)
            .map(|rest| new_state.join(rest))
            .unwrap_or_else(|_| path.to_path_buf())
    };

    // Read the declaration: a program in configs/, else a legacy config.
    let declared_program = std::fs::read_to_string(&old_program_path)
        .ok()
        .and_then(|text| WorkspaceProgram::parse(&text).ok())
        .filter(|program| program.id() == old_id);
    let legacy_path = old_state.join(LEGACY_WORKSPACE_CONFIG);
    let (program, retire): (WorkspaceProgram, PathBuf) = if let Some(old) = declared_program {
        let mut view = program_lang::derive_view(&old, None, RetentionPolicy::default())?;
        view.id = new_id.to_string();
        for binding in view.bindings.values_mut() {
            match binding {
                WorkspaceBinding::NativeMarkdown { path, .. } | WorkspaceBinding::Captures { path } => {
                    *path = rewrite(path);
                }
                _ => {}
            }
        }
        let mut base = old.program().clone();
        base.workspaces[0].name = new_id.to_string();
        let program = program_from_config(&view, &base, FolderQualification::Program)?;
        (program, old_program_path.clone())
    } else if legacy_path.is_file() {
        let mut legacy = read_legacy_config(&legacy_path)?;
        if legacy.id != old_id {
            bail!("{} declares workspace '{}', not '{old_id}'", legacy_path.display(), legacy.id);
        }
        legacy.id = new_id.to_string();
        for binding in legacy.bindings.values_mut() {
            match binding {
                WorkspaceBinding::NativeMarkdown { path, .. } | WorkspaceBinding::Captures { path } => {
                    *path = rewrite(path);
                }
                _ => {}
            }
        }
        let (program, warnings) = program_from_legacy_config(&legacy)?;
        for warning in &warnings {
            log::warn!("renamed Workspace '{new_id}': {warning}");
        }
        (program, new_state.join(LEGACY_WORKSPACE_CONFIG))
    } else if new_program_path.is_file()
        && (configs.join(format!("{old_id}.enzyme.renamed-to-{new_id}")).exists() || state_moved)
    {
        // An earlier run moved everything; only machine config may remain.
        rename_in_machine_config(margins_home, old_id, new_id)?;
        let retired = configs.join(format!("{old_id}.enzyme.renamed-to-{new_id}"));
        let retired = if retired.exists() {
            retired
        } else {
            new_state.join(LEGACY_WORKSPACE_CONFIG_MIGRATED)
        };
        return Ok(WorkspaceRename {
            old_id: old_id.to_string(),
            new_id: new_id.to_string(),
            program_path: new_program_path,
            state_dir: new_state,
            retired,
            compatibility_link: state_moved.then_some(old_state),
        });
    } else {
        bail!("there is no Workspace '{old_id}' to rename");
    };
    // Validate without machine config (it may name the old id, and a legacy
    // machine file cannot migrate while the old program holds a reserved
    // slot) and without the shared profiles when they are the old program.
    let view = program_lang::derive_view(&program, None, RetentionPolicy::default())?;
    validate_config(&view)?;
    program_lang::validate_language(
        &program,
        &new_state.join("ledger.db"),
        if old_id == "profiles" { None } else { shared_profiles(margins_home)? }.as_ref(),
    )?;
    match std::fs::read_to_string(&new_program_path) {
        Ok(existing) if existing != program.text() => {
            bail!("Workspace '{new_id}' already exists; choose another id")
        }
        _ => {}
    }
    if !state_moved && old_state.is_dir() && std::fs::symlink_metadata(&new_state).is_ok() {
        bail!("{} already exists; choose another id", new_state.display());
    }

    // Move the state, keeping a link for paths recorded under the old id.
    let mut compatibility_link = None;
    if !state_moved && old_state.is_dir() {
        let _lock = lock_workspace(&old_state)?;
        std::fs::rename(&old_state, &new_state).with_context(|| {
            format!("moving {} to {}", old_state.display(), new_state.display())
        })?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(new_id, &old_state)
            .with_context(|| format!("linking {}", old_state.display()))?;
        sync_parent_directory(&new_state)?;
    }
    if std::fs::symlink_metadata(&old_state).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        compatibility_link = Some(old_state.clone());
    }
    atomic_write(&new_program_path, program.text().as_bytes())?;

    // Retire the old declaration; it is kept, never deleted.
    let retired = if retire == old_program_path {
        let retired = configs.join(format!("{old_id}.enzyme.renamed-to-{new_id}"));
        if retire.exists() {
            rename_no_replace(&retire, &retired)?;
        }
        retired
    } else if retire.is_file() {
        retire_legacy_config(&new_state)?
    } else {
        legacy_backup_path(&new_state)
    };
    sync_parent_directory(&new_program_path)?;

    rename_in_machine_config(margins_home, old_id, new_id)?;
    Ok(WorkspaceRename {
        old_id: old_id.to_string(),
        new_id: new_id.to_string(),
        program_path: new_program_path,
        state_dir: new_state,
        retired,
        compatibility_link,
    })
}

/// Point the machine default, display name, and retention override at the
/// renamed id. Migrates a legacy machine file first (it may name the old id);
/// the caller holds the machine lock.
fn rename_in_machine_config(margins_home: &Path, old_id: &str, new_id: &str) -> Result<()> {
    machine_config::migrate_locked(margins_home)?;
    let mut config = read_machine_config_locked(margins_home)?;
    let mut changed = false;
    if let Some(workspace) = config.get_mut("workspace").and_then(toml::Value::as_table_mut) {
        if workspace.get("default").and_then(toml::Value::as_str) == Some(old_id) {
            workspace.insert("default".to_string(), toml::Value::String(new_id.to_string()));
            changed = true;
        }
        if let Some(names) = workspace.get_mut("names").and_then(toml::Value::as_table_mut) {
            if let Some(name) = names.remove(old_id) {
                names.insert(new_id.to_string(), name);
                changed = true;
            }
        }
    }
    if let Some(retention) = config.get_mut("retention").and_then(toml::Value::as_table_mut) {
        if let Some(policy) = retention.remove(old_id) {
            retention.insert(new_id.to_string(), policy);
            changed = true;
        }
    }
    if changed {
        atomic_write(
            &machine_config::machine_config_path(margins_home),
            toml::to_string_pretty(&config)?.as_bytes(),
        )?;
    }
    Ok(())
}

/// Whether a reserved program (for example a `configs/settings.enzyme`
/// written when `settings` was a legal Workspace id) declares a Workspace.
fn reserved_program_declares_workspace(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| enzyme_spec::parse(&text).ok())
        .is_some_and(|program| !program.workspaces.is_empty())
}

/// Remove an unused Workspace declaration without touching any declared Source.
/// A Workspace with state beyond its program needs an explicit migration first.
pub fn remove_empty_workspace(margins_home: &Path, id: &str) -> Result<()> {
    let state_dir = workspace_state_dir(margins_home, id)?;
    // Resolve (and migrate) before taking the machine lock: migration may
    // record machine-owned settings under that same lock.
    resolve_at(margins_home, id)?;
    let _lock = machine_config::lock_machine(margins_home)?;
    machine_config::migrate_locked(margins_home)?;
    if default_workspace_locked(margins_home)?.as_deref() == Some(id) {
        bail!("cannot remove the machine's default Workspace");
    }
    let program_path = workspace_program_path(margins_home, id)?;
    let program_metadata = std::fs::symlink_metadata(&program_path)
        .with_context(|| format!("Workspace {id} does not exist"))?;
    if !program_metadata.is_file() || program_metadata.file_type().is_symlink() {
        bail!("Workspace program is not a plain file");
    }
    match std::fs::symlink_metadata(&state_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!("Workspace state directory is not a plain directory");
            }
            for entry in std::fs::read_dir(&state_dir)? {
                let name = entry?.file_name();
                if name != WORKSPACE_LOCK && !is_legacy_backup_name(&name) {
                    bail!("Workspace has stored data; migrate or retain it before removal");
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", state_dir.display())),
    }
    std::fs::remove_file(&program_path)?;
    sync_parent_directory(&program_path)?;
    if state_dir.exists() {
        std::fs::remove_dir_all(&state_dir)?;
    }
    Ok(())
}

pub fn add_source(
    workspace: &mut ResolvedWorkspace,
    name: &str,
    binding: WorkspaceBinding,
) -> Result<()> {
    validate_source_name(name)?;
    validate_binding(&binding)?;
    mutate_workspace_config(workspace, |config| {
        if config.bindings.contains_key(name) {
            bail!("source '{name}' already exists");
        }
        if matches!(
            binding,
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                ..
            }
        ) {
            bail!("workspace already has its one home notes source");
        }
        config.bindings.insert(name.to_string(), binding);
        Ok(())
    })
}

pub fn remove_source(workspace: &mut ResolvedWorkspace, name: &str) -> Result<WorkspaceBinding> {
    let captures_dir = workspace.captures_dir();
    mutate_workspace_config(workspace, |config| {
        let binding = config
            .bindings
            .get(name)
            .with_context(|| format!("unknown source '{name}'"))?;
        if matches!(
            binding,
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                ..
            }
        ) {
            bail!("the required home notes source cannot be removed");
        }
        if matches!(binding, WorkspaceBinding::Captures { path } if path == &captures_dir) {
            bail!("the required captures source cannot be removed");
        }
        config
            .bindings
            .remove(name)
            .with_context(|| format!("unknown source '{name}'"))
    })
}

/// Replace the view-level attention policy. Readings that keep their entity
/// keep their learning settings and inline profiles.
pub fn update_policy(workspace: &mut ResolvedWorkspace, policy: WorkspacePolicy) -> Result<()> {
    mutate_workspace_config(workspace, |config| {
        config.policy = policy;
        Ok(())
    })
}

/// The Workspace revision: SHA-256 of the program bytes.
pub fn workspace_revision(workspace: &ResolvedWorkspace) -> Result<String> {
    Ok(workspace.program.sha256())
}

fn current_revision(state_dir: &Path) -> Result<String> {
    let path = program_path_of_state_dir(state_dir)?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading workspace program {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Re-read the workspace program and reject an authoritative commit whose
/// desired-state revision changed while provider transport was in flight.
pub fn require_workspace_revision(state_dir: &Path, expected_revision: &str) -> Result<()> {
    let actual = current_revision(state_dir)?;
    if actual != expected_revision {
        return Err(WorkspaceMutationError::RevisionConflict {
            expected: expected_revision.to_string(),
            actual,
        }
        .into());
    }
    Ok(())
}

pub fn lock_workspace_revision(
    state_dir: &Path,
    expected_revision: &str,
) -> Result<WorkspaceRevisionGuard> {
    let lock = lock_workspace_ready(state_dir)?;
    require_workspace_revision(state_dir, expected_revision)?;
    Ok(WorkspaceRevisionGuard { _lock: lock })
}

pub fn validate_mutation_request_id(request_id: &str) -> Result<()> {
    validate_request_id(request_id)
}

/// Plan the complete desired view `desired` against the current program,
/// preserving every program statement the view does not model.
pub fn plan_workspace_config(
    workspace: &ResolvedWorkspace,
    desired: WorkspaceConfig,
) -> Result<WorkspacePlan> {
    let program = desired_program_from_config(workspace, &desired, FolderQualification::Program)?;
    plan_workspace_program(workspace, program.text())
}

/// Plan a desired Workspace written in the retired `config.toml` shape. Older
/// setup callers still produce it; it is converted with the migration rules
/// onto the current program.
pub fn plan_legacy_workspace_config(
    workspace: &ResolvedWorkspace,
    desired: WorkspaceConfig,
) -> Result<WorkspacePlan> {
    if desired.retention != workspace.config.retention {
        return Err(WorkspaceMutationError::InvalidPlan(
            "retention is machine configuration: set [retention] or [retention.<id>] in MARGINS_HOME/margins.toml".to_string(),
        )
        .into());
    }
    if desired.name.is_some() && desired.name != workspace.config.name {
        return Err(WorkspaceMutationError::InvalidPlan(
            "a Workspace display name is machine configuration, not part of the desired program"
                .to_string(),
        )
        .into());
    }
    let legacy = program_lang::legacy_view(&desired)?;
    for warning in &legacy.warnings {
        log::warn!("desired Workspace '{}': {warning}", desired.id);
    }
    let program =
        desired_program_from_config(workspace, &legacy.config, FolderQualification::Legacy)?;
    plan_workspace_program(workspace, program.text())
}

fn desired_program_from_config(
    workspace: &ResolvedWorkspace,
    desired: &WorkspaceConfig,
    qualification: FolderQualification,
) -> Result<WorkspaceProgram> {
    if desired.id != workspace.config.id {
        return Err(WorkspaceMutationError::InvalidPlan(format!(
            "desired config id '{}' does not match workspace '{}'",
            desired.id, workspace.config.id
        ))
        .into());
    }
    program_from_config(desired, workspace.program.program(), qualification)
}

/// Plan replacing the current program with `desired_program`, verbatim.
pub fn plan_workspace_program(
    workspace: &ResolvedWorkspace,
    desired_program: &str,
) -> Result<WorkspacePlan> {
    let margins_home = margins_home_of_state_dir(&workspace.state_dir)?;
    let desired = WorkspaceProgram::parse(desired_program).context("invalid desired program")?;
    if desired.id() != workspace.config.id {
        return Err(WorkspaceMutationError::InvalidPlan(format!(
            "desired program declares workspace '{}', not '{}'",
            desired.id(),
            workspace.config.id
        ))
        .into());
    }
    let desired_view = validate_program(margins_home, &desired)?;
    let mut actions = view_actions(&workspace.config, &desired_view);
    if workspace.program.text() != desired_program {
        // The view actions explain the change when applying them to the
        // current program yields the same statements as the desired program;
        // layout and comments alone are visible in the diff.
        let explained = program_from_config(
            &desired_view,
            workspace.program.program(),
            FolderQualification::Program,
        )
        .is_ok_and(|reconciled| reconciled.program() == desired.program());
        if actions.is_empty() || !explained {
            actions.push(WorkspacePlanAction::UpdateProgram {
                summary: "Update program statements beyond sources and attention policy (learning settings, profiles, agent policies, or formatting); see diff".to_string(),
            });
        }
    }
    let base_revision = workspace.program.sha256();
    let desired_sha256 = program_sha256(desired_program);
    let diff = program_lang::unified_diff(
        workspace.program.text(),
        desired_program,
        &format!("{CONFIGS_DIR}/{}.enzyme", workspace.config.id),
    );
    let plan_id = workspace_plan_id(
        &workspace.config.id,
        &base_revision,
        &actions,
        &desired_sha256,
    )?;
    Ok(WorkspacePlan {
        schema_version: WORKSPACE_PLAN_SCHEMA.to_string(),
        workspace_id: workspace.config.id.clone(),
        base_revision,
        plan_id,
        actions,
        desired_program: desired_program.to_string(),
        desired_sha256,
        diff,
    })
}

fn view_actions(current: &WorkspaceConfig, desired: &WorkspaceConfig) -> Vec<WorkspacePlanAction> {
    let mut actions = Vec::new();
    if current.policy != desired.policy {
        actions.push(WorkspacePlanAction::SetPolicy {
            summary: policy_summary(&current.policy, &desired.policy),
            before: current.policy.clone(),
            after: desired.policy.clone(),
        });
    }
    for (name, binding) in &current.bindings {
        match desired.bindings.get(name) {
            None => actions.push(WorkspacePlanAction::RemoveBinding {
                summary: format!("Remove {} source \"{name}\"", binding_label(binding)),
                name: name.clone(),
                binding: binding.clone(),
            }),
            Some(after) if after != binding => actions.push(WorkspacePlanAction::UpdateBinding {
                summary: binding_change_summary(name, binding, after),
                name: name.clone(),
                before: binding.clone(),
                after: after.clone(),
            }),
            Some(_) => {}
        }
    }
    for (name, binding) in &desired.bindings {
        if !current.bindings.contains_key(name) {
            actions.push(WorkspacePlanAction::AddBinding {
                summary: format!(
                    "Add {} source \"{name}\" ({})",
                    binding_label(binding),
                    binding_target(binding)
                ),
                name: name.clone(),
                binding: binding.clone(),
            });
        }
    }
    actions
}

fn binding_label(binding: &WorkspaceBinding) -> &'static str {
    match binding {
        WorkspaceBinding::NativeMarkdown {
            role: SourceRole::Home,
            ..
        } => "Home markdown",
        WorkspaceBinding::NativeMarkdown { .. } => "markdown",
        WorkspaceBinding::Captures { .. } => program_lang::SOURCE_CAPTURES,
        WorkspaceBinding::Gmail { .. } => program_lang::SOURCE_GOOGLE_MAIL,
        WorkspaceBinding::GoogleCalendar { .. } => program_lang::SOURCE_GOOGLE_CALENDAR,
        WorkspaceBinding::GoogleMeet { .. } => program_lang::SOURCE_GOOGLE_MEET,
        WorkspaceBinding::Granola { .. } => program_lang::SOURCE_GRANOLA,
    }
}

fn binding_target(binding: &WorkspaceBinding) -> String {
    binding
        .local_path()
        .map(|path| path.display().to_string())
        .or_else(|| binding.google_account().map(str::to_string))
        .unwrap_or_default()
}

fn binding_change_summary(name: &str, before: &WorkspaceBinding, after: &WorkspaceBinding) -> String {
    let mut changes = Vec::new();
    if before.local_path() != after.local_path() || before.google_account() != after.google_account() {
        changes.push(format!("now {}", binding_target(after)));
    }
    if before.native_markdown_role() != after.native_markdown_role() {
        changes.push(match after.native_markdown_role() {
            Some(SourceRole::Home) => "becomes Home".to_string(),
            _ => "no longer Home".to_string(),
        });
    }
    if let (
        WorkspaceBinding::NativeMarkdown { note_folder: old, .. },
        WorkspaceBinding::NativeMarkdown { note_folder: new, role: SourceRole::Home, .. },
    ) = (before, after)
    {
        if old != new {
            changes.push(format!(
                "new notes go to \"{}\"",
                new.as_deref().map_or(".".to_string(), |folder| folder.display().to_string())
            ));
        }
    }
    if before.gmail_selector() != after.gmail_selector()
        || before.calendar_selector() != after.calendar_selector()
        || before.granola_selector() != after.granola_selector()
    {
        changes.push("collection window or query changes".to_string());
    }
    if changes.is_empty() {
        changes.push("declaration changes".to_string());
    }
    format!("Change source \"{name}\": {}", changes.join("; "))
}

fn policy_summary(before: &WorkspacePolicy, after: &WorkspacePolicy) -> String {
    fn entity_names(policy: &WorkspacePolicy) -> Vec<String> {
        policy
            .entities
            .iter()
            .flat_map(|entity| {
                entity
                    .entries()
                    .into_iter()
                    .map(|(name, _)| name.to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    fn delta(added_verb: &str, removed_verb: &str, before: &[String], after: &[String], parts: &mut Vec<String>) {
        let added: Vec<&str> = after.iter().filter(|item| !before.contains(item)).map(String::as_str).collect();
        let removed: Vec<&str> = before.iter().filter(|item| !after.contains(item)).map(String::as_str).collect();
        if !added.is_empty() {
            parts.push(format!("{added_verb} {}", added.join(", ")));
        }
        if !removed.is_empty() {
            parts.push(format!("{removed_verb} {}", removed.join(", ")));
        }
    }
    let mut parts = Vec::new();
    let (before_entities, after_entities) = (entity_names(before), entity_names(after));
    delta("learn questions from", "stop learning from", &before_entities, &after_entities, &mut parts);
    if before_entities == after_entities && before.entities != after.entities {
        parts.push("change reading profiles or linked-page expansion".to_string());
    }
    delta("leave out folders", "stop leaving out folders", &before.excluded_folders, &after.excluded_folders, &mut parts);
    delta("leave out tags", "stop leaving out tags", &before.excluded_tags, &after.excluded_tags, &mut parts);
    delta("leave out links", "stop leaving out links", &before.excluded_entities, &after.excluded_entities, &mut parts);
    if parts.is_empty() {
        parts.push("reorder readings or exclusions".to_string());
    }
    format!("Attention policy: {}", parts.join("; "))
}

pub fn apply_workspace_plan(
    workspace: &mut ResolvedWorkspace,
    plan: &WorkspacePlan,
) -> Result<WorkspaceApplyReceipt> {
    if program_sha256(&plan.desired_program) != plan.desired_sha256 {
        return Err(WorkspaceMutationError::InvalidPlan(
            "workspace plan desired_program does not match desired_sha256".to_string(),
        )
        .into());
    }
    let request_hash = workspace_apply_request_hash(plan)?;
    let request_id = format!("workspace-apply-{request_hash}");
    let margins_home = margins_home_of_state_dir(&workspace.state_dir)?.to_path_buf();
    let id = workspace.config.id.clone();
    let _lock = lock_workspace_ready(&workspace.state_dir)?;
    if let Some(mut receipt) = load_workspace_receipt(&workspace.state_dir, &request_id)? {
        if receipt.request_hash != request_hash {
            return Err(WorkspaceMutationError::IdempotencyConflict {
                request_id: request_id.clone(),
            }
            .into());
        }
        receipt.replayed = true;
        *workspace = load_resolved(&margins_home, &id)?;
        return Ok(receipt);
    }

    let current = load_resolved(&margins_home, &id)?;
    let actual_revision = current.program.sha256();
    if actual_revision != plan.base_revision {
        return Err(WorkspaceMutationError::RevisionConflict {
            expected: plan.base_revision.clone(),
            actual: actual_revision,
        }
        .into());
    }
    let rebuilt = plan_workspace_program(&current, &plan.desired_program)?;
    if rebuilt != *plan {
        return Err(WorkspaceMutationError::InvalidPlan(
            "workspace plan content or plan_id is invalid".to_string(),
        )
        .into());
    }

    let receipt = WorkspaceApplyReceipt {
        schema_version: WORKSPACE_APPLY_SCHEMA.to_string(),
        ok: true,
        workspace_id: id.clone(),
        request_id,
        request_hash,
        plan_id: plan.plan_id.clone(),
        before_revision: plan.base_revision.clone(),
        after_revision: plan.desired_sha256.clone(),
        replayed: false,
        actions: plan
            .actions
            .iter()
            .cloned()
            .enumerate()
            .map(|(position, action)| WorkspaceActionReceipt {
                position,
                status: "applied".to_string(),
                action,
            })
            .collect(),
    };
    store_workspace_transaction(
        &workspace.state_dir,
        &WorkspaceTransaction {
            receipt: receipt.clone(),
            desired_program: plan.desired_program.clone(),
        },
    )?;
    atomic_write(&current.config_path, plan.desired_program.as_bytes())?;
    store_workspace_receipt(&workspace.state_dir, &receipt)?;
    clear_workspace_transaction(&workspace.state_dir)?;
    *workspace = load_resolved(&margins_home, &id)?;
    Ok(receipt)
}

fn workspace_apply_request_hash(plan: &WorkspacePlan) -> Result<String> {
    let bytes = serde_json::to_vec(plan).context("serializing workspace apply request")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Resolve the declared Gmail selector for one normalized account.
pub fn gmail_selector_for_account(
    config: &WorkspaceConfig,
    account: &str,
) -> Result<GmailCollectionSelector> {
    let requested_account = normalize_google_account(account)?;
    config
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::Gmail {
                account: declared_account,
                gmail,
            } if normalize_google_account(declared_account)
                .ok()
                .is_some_and(|normalized| normalized == requested_account) =>
            {
                Some(gmail.clone())
            }
            _ => None,
        })
        .with_context(|| {
            format!("workspace has no google-mail source for account {requested_account}")
        })
}

pub fn calendar_selector_for_account(
    config: &WorkspaceConfig,
    account: &str,
) -> Result<CalendarCollectionSelector> {
    let requested_account = normalize_google_account(account)?;
    config
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::GoogleCalendar {
                account: declared_account,
                calendar,
            } if normalize_google_account(declared_account)
                .ok()
                .is_some_and(|normalized| normalized == requested_account) =>
            {
                Some(calendar.clone())
            }
            _ => None,
        })
        .with_context(|| {
            format!("workspace has no google-calendar source for account {requested_account}")
        })
}

pub fn granola_selector_for_account(
    config: &WorkspaceConfig,
    account: &str,
) -> Result<GranolaCollectionSelector> {
    let requested_account = normalize_granola_account(account)?;
    config
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::Granola {
                account: declared_account,
                collection,
            } if normalize_granola_account(declared_account)
                .ok()
                .is_some_and(|normalized| normalized == requested_account) =>
            {
                Some(collection.clone())
            }
            _ => None,
        })
        .with_context(|| format!("workspace has no granola source for account {requested_account}"))
}

fn validate_config(config: &WorkspaceConfig) -> Result<()> {
    validate_id(&config.id)?;
    config.retention.validate()?;
    let homes = config
        .bindings
        .values()
        .filter(|binding| {
            matches!(
                binding,
                WorkspaceBinding::NativeMarkdown {
                    role: SourceRole::Home,
                    ..
                }
            )
        })
        .count();
    if homes != 1 {
        bail!("workspace must declare exactly one notes source with role = 'home'");
    }
    let mut gmail_accounts = BTreeMap::new();
    let mut calendar_accounts = BTreeMap::new();
    let mut meet_accounts = BTreeMap::new();
    let mut granola_accounts = BTreeMap::new();
    let mut native_paths: Vec<&Path> = Vec::new();
    for (name, binding) in &config.bindings {
        validate_source_name(name)?;
        validate_binding(binding)?;
        if let WorkspaceBinding::NativeMarkdown { path, .. } = binding {
            for existing in &native_paths {
                if native_roots_overlap(existing, path) {
                    bail!(
                        "native markdown roots cannot overlap: {} and {}",
                        existing.display(),
                        path.display()
                    );
                }
            }
            native_paths.push(path.as_path());
        }
        if let WorkspaceBinding::Gmail { account, .. } = binding {
            let account = normalize_google_account(account)?;
            if gmail_accounts.insert(account, name.as_str()).is_some() {
                bail!("workspace cannot declare two Gmail sources for the same account");
            }
        }
        if let WorkspaceBinding::GoogleCalendar { account, .. } = binding {
            let account = normalize_google_account(account)?;
            if calendar_accounts.insert(account, name.as_str()).is_some() {
                bail!("workspace cannot declare two Calendar sources for the same account");
            }
        }
        if let WorkspaceBinding::GoogleMeet { account } = binding {
            let account = normalize_google_account(account)?;
            if meet_accounts.insert(account, name.as_str()).is_some() {
                bail!("workspace cannot declare two Google Meet sources for the same account");
            }
        }
        if let WorkspaceBinding::Granola { account, .. } = binding {
            let account = normalize_granola_account(account)?;
            if granola_accounts.insert(account, name.as_str()).is_some() {
                bail!("workspace cannot declare two Granola sources for the same account");
            }
        }
    }
    Ok(())
}

fn native_roots_overlap(left: &Path, right: &Path) -> bool {
    // A declared Home is canonicalized at creation, while later bindings may
    // arrive through an alias such as macOS /var -> /private/var.
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left.starts_with(&right) || right.starts_with(&left)
}

fn validate_binding(binding: &WorkspaceBinding) -> Result<()> {
    match binding {
        WorkspaceBinding::NativeMarkdown {
            path,
            role,
            note_folder,
        } => {
            if !path.is_absolute() {
                bail!("source paths must be absolute: {}", path.display());
            }
            if let Some(folder) = note_folder {
                if *role != SourceRole::Home
                    || folder.as_os_str().is_empty()
                    || folder
                        .components()
                        .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    bail!("note_folder must be a nonempty relative path on the Home binding");
                }
                safe_note_destination(path, folder)?;
            }
        }
        WorkspaceBinding::Captures { path } => {
            if !path.is_absolute() {
                bail!("source paths must be absolute: {}", path.display());
            }
        }
        WorkspaceBinding::Gmail { account, gmail } => {
            if normalize_google_account(account)? != account.as_str() {
                bail!("remote Google source accounts must be normalized email addresses");
            }
            gmail.validate()?;
        }
        WorkspaceBinding::GoogleCalendar { account, calendar } => {
            if normalize_google_account(account)? != account.as_str() {
                bail!("remote Google source accounts must be normalized email addresses");
            }
            calendar.validate()?;
        }
        WorkspaceBinding::GoogleMeet { account } => {
            if normalize_google_account(account)? != account.as_str() {
                bail!("remote Google source accounts must be normalized email addresses");
            }
        }
        WorkspaceBinding::Granola {
            account,
            collection,
        } => {
            if normalize_granola_account(account)? != account.as_str() {
                bail!("remote Granola source accounts must be normalized email addresses");
            }
            collection.validate()?;
        }
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    validate_id_format(id)?;
    if let Some(file) = reserved_program_for(id) {
        bail!("workspace id '{id}' is reserved for configs/{file}");
    }
    Ok(())
}

fn validate_id_format(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        || id.starts_with('-')
        || id.ends_with('-')
    {
        bail!("workspace id must use lowercase letters, digits, and internal hyphens");
    }
    Ok(())
}

fn reserved_program_for(id: &str) -> Option<&'static str> {
    RESERVED_PROGRAMS
        .iter()
        .copied()
        .find(|file| file.strip_suffix(".enzyme") == Some(id))
}

/// Whether `id` was a legal Workspace id before it became a reserved program
/// name (`settings`, `profiles`, `margins-sources`).
pub fn is_reserved_id(id: &str) -> bool {
    validate_id_format(id).is_ok() && reserved_program_for(id).is_some()
}

/// The error for an existing Workspace whose id is now reserved: it names the
/// rename that keeps its notes and state.
pub fn reserved_id_error(id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Workspace '{id}' uses an id that is now reserved for configs/{}; its notes and state are kept. \
         Rename it with `margins workspace rename {id} <new-id>`",
        reserved_program_for(id).unwrap_or("")
    )
}

fn validate_source_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        bail!("source name must use lowercase letters, digits, and hyphens");
    }
    Ok(())
}

fn mutate_workspace_config<T>(
    workspace: &mut ResolvedWorkspace,
    mutate: impl FnOnce(&mut WorkspaceConfig) -> Result<T>,
) -> Result<T> {
    let margins_home = margins_home_of_state_dir(&workspace.state_dir)?.to_path_buf();
    let id = workspace.config.id.clone();
    let _lock = lock_workspace_ready(&workspace.state_dir)?;
    let current = load_resolved(&margins_home, &id)?;
    let mut desired = current.config.clone();
    let value = mutate(&mut desired)?;
    let program = program_from_config(
        &desired,
        current.program.program(),
        FolderQualification::Program,
    )?;
    validate_program(&margins_home, &program)?;
    if program != current.program {
        atomic_write(&current.config_path, program.text().as_bytes())?;
    }
    *workspace = load_resolved(&margins_home, &id)?;
    Ok(value)
}

/// Load an already-migrated Workspace program. Used under the Workspace lock,
/// where migration (which takes the same lock) must not run.
fn load_resolved(margins_home: &Path, id: &str) -> Result<ResolvedWorkspace> {
    let state_dir = workspace_state_dir(margins_home, id)?;
    let program_path = workspace_program_path(margins_home, id)?;
    let text = std::fs::read_to_string(&program_path)
        .with_context(|| format!("reading workspace program {}", program_path.display()))?;
    let program = WorkspaceProgram::parse(&text)
        .with_context(|| format!("invalid workspace program {}", program_path.display()))?;
    if program.id() != id {
        bail!(
            "workspace program {} declares workspace '{}', expected '{id}'",
            program_path.display(),
            program.id()
        );
    }
    resolved_from_program(margins_home, state_dir, program_path, program)
}

fn workspace_plan_id(
    workspace_id: &str,
    base_revision: &str,
    actions: &[WorkspacePlanAction],
    desired_sha256: &str,
) -> Result<String> {
    #[derive(Serialize)]
    struct Identity<'a> {
        workspace_id: &'a str,
        base_revision: &'a str,
        actions: &'a [WorkspacePlanAction],
        desired_sha256: &'a str,
    }
    let bytes = serde_json::to_vec(&Identity {
        workspace_id,
        base_revision,
        actions,
        desired_sha256,
    })
    .context("serializing canonical workspace plan identity")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_request_id(request_id: &str) -> Result<()> {
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(WorkspaceMutationError::InvalidPlan(
            "request id must use 1-128 ASCII letters, digits, periods, underscores, or hyphens"
                .to_string(),
        )
        .into());
    }
    Ok(())
}

fn lock_workspace(state_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("creating workspace state at {}", state_dir.display()))?;
    let path = state_dir.join(WORKSPACE_LOCK);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening workspace lock {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking workspace {}", state_dir.display()))?;
    Ok(file)
}

fn lock_workspace_ready(state_dir: &Path) -> Result<File> {
    let lock = lock_workspace(state_dir)?;
    recover_workspace_transaction(state_dir)?;
    Ok(lock)
}

fn workspace_receipt_path(state_dir: &Path, request_id: &str) -> PathBuf {
    state_dir
        .join(WORKSPACE_RECEIPTS_DIR)
        .join(format!("{request_id}.json"))
}

fn workspace_transaction_path(state_dir: &Path) -> PathBuf {
    state_dir.join(WORKSPACE_TRANSACTION)
}

fn load_workspace_transaction(state_dir: &Path) -> Result<Option<WorkspaceTransaction>> {
    let path = workspace_transaction_path(state_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid workspace transaction {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn store_workspace_transaction(state_dir: &Path, transaction: &WorkspaceTransaction) -> Result<()> {
    let path = workspace_transaction_path(state_dir);
    let body = serde_json::to_vec_pretty(transaction)
        .context("serializing workspace transaction journal")?;
    atomic_write(&path, &body)
}

fn clear_workspace_transaction(state_dir: &Path) -> Result<()> {
    let path = workspace_transaction_path(state_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => sync_parent_directory(&path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn recover_workspace_transaction(state_dir: &Path) -> Result<()> {
    let Some(transaction) = load_workspace_transaction(state_dir)? else {
        return Ok(());
    };
    WorkspaceProgram::parse(&transaction.desired_program)
        .context("pending workspace transaction has an invalid desired program")?;
    if program_sha256(&transaction.desired_program) != transaction.receipt.after_revision {
        bail!("pending workspace transaction has an invalid desired revision");
    }
    let program_path = program_path_of_state_dir(state_dir)?;
    let current_revision = current_revision(state_dir)?;
    if current_revision == transaction.receipt.before_revision {
        atomic_write(&program_path, transaction.desired_program.as_bytes())?;
    } else if current_revision != transaction.receipt.after_revision {
        bail!(
            "pending workspace transaction cannot be recovered: expected revision {} or {}, found {}",
            transaction.receipt.before_revision,
            transaction.receipt.after_revision,
            current_revision
        );
    }
    if let Some(existing) =
        load_workspace_receipt(state_dir, transaction.receipt.request_id.as_str())?
    {
        if existing.request_hash != transaction.receipt.request_hash {
            return Err(WorkspaceMutationError::IdempotencyConflict {
                request_id: transaction.receipt.request_id.clone(),
            }
            .into());
        }
    } else {
        store_workspace_receipt(state_dir, &transaction.receipt)?;
    }
    clear_workspace_transaction(state_dir)
}

fn load_workspace_receipt(
    state_dir: &Path,
    request_id: &str,
) -> Result<Option<WorkspaceApplyReceipt>> {
    let path = workspace_receipt_path(state_dir, request_id);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid workspace receipt {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn store_workspace_receipt(state_dir: &Path, receipt: &WorkspaceApplyReceipt) -> Result<()> {
    let directory = state_dir.join(WORKSPACE_RECEIPTS_DIR);
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let path = workspace_receipt_path(state_dir, &receipt.request_id);
    let body = serde_json::to_vec_pretty(receipt).context("serializing workspace receipt")?;
    atomic_write(&path, &body)
}

pub(crate) fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.permissions().readonly())
    {
        bail!("writing {}: destination is read-only", path.display());
    }
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary config in {}", parent.display()))?;
    temporary
        .write_all(body)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    sync_parent_directory(path)
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("syncing parent directory {}", parent.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn safe_tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("implicit-workspace-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
    }

    fn test_implicit_roots(base: &Path, margins_home: &Path) -> ImplicitWorkspaceRoots {
        ImplicitWorkspaceRoots {
            home_dir: Some(base.join("fake-user-home")),
            margins_home: margins_home.to_path_buf(),
            enzyme_home: Some(base.join("fake-enzyme-home")),
            config_state_roots: vec![
                base.join("fake-config/margins"),
                base.join("fake-config/enzyme"),
            ],
            temp_roots: vec![base.join("fake-temp")],
        }
    }

    #[test]
    fn process_deny_root_override_requires_complete_absolute_nonempty_roots() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let valid = serde_json::json!({
            "home_dir": temp.path().join("home"),
            "enzyme_home": temp.path().join("enzyme"),
            "config_state_roots": [temp.path().join("config/margins")],
            "temp_roots": [temp.path().join("temp")],
        })
        .to_string();
        let roots = ImplicitWorkspaceRoots::from_override(&margins_home, &valid).unwrap();
        assert_eq!(roots.margins_home, margins_home);

        let empty = serde_json::json!({
            "home_dir": temp.path().join("home"),
            "enzyme_home": temp.path().join("enzyme"),
            "config_state_roots": [],
            "temp_roots": [],
        })
        .to_string();
        assert!(ImplicitWorkspaceRoots::from_override(&margins_home, &empty)
            .unwrap_err()
            .to_string()
            .contains("requires non-empty"));

        let relative = serde_json::json!({
            "home_dir": "relative-home",
            "enzyme_home": temp.path().join("enzyme"),
            "config_state_roots": [temp.path().join("config/margins")],
            "temp_roots": [temp.path().join("temp")],
        })
        .to_string();
        assert!(
            ImplicitWorkspaceRoots::from_override(&margins_home, &relative)
                .unwrap_err()
                .to_string()
                .contains("paths must be absolute")
        );
    }

    #[test]
    fn creates_state_outside_home_content_and_resolves_declared_binding() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("client")).unwrap();
        let created = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        assert_eq!(created.state_dir, margins_home.join("workspaces/practice"));
        assert!(!notes.join(".margins").exists());
        let resolved = resolve_workspace(&margins_home, None, &notes.join("client")).unwrap();
        assert_eq!(resolved.config.id, "practice");
    }

    #[test]
    fn index_db_is_renamed_to_enzyme_db_once_without_touching_identity() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let created = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let state = created.state_dir.clone();
        std::fs::write(state.join("index.db"), b"index bytes").unwrap();
        std::fs::write(state.join("index.db-wal"), b"wal bytes").unwrap();
        std::fs::write(state.join("index.identity"), b"identity\n").unwrap();

        let resolved = resolve_at(&margins_home, "practice").unwrap();

        assert_eq!(resolved.recall_path(), state.join("enzyme.db"));
        assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");
        assert_eq!(std::fs::read(state.join("enzyme.db-wal")).unwrap(), b"wal bytes");
        assert!(!state.join("index.db").exists());
        assert!(!state.join("index.db-wal").exists());
        assert_eq!(std::fs::read(state.join("index.identity")).unwrap(), b"identity\n");
        resolve_at(&margins_home, "practice").unwrap();
        assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");

        // A crash after the sidecars moved resumes with the main file.
        std::fs::rename(state.join("enzyme.db"), state.join("index.db")).unwrap();
        resolve_at(&margins_home, "practice").unwrap();
        assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");
        assert_eq!(std::fs::read(state.join("enzyme.db-wal")).unwrap(), b"wal bytes");

        // A recreated legacy sidecar never replaces the adopted one.
        std::fs::rename(state.join("enzyme.db"), state.join("index.db")).unwrap();
        std::fs::write(state.join("index.db-wal"), b"stale wal").unwrap();
        let error = resolve_at(&margins_home, "practice").unwrap_err();
        assert!(format!("{error:#}").contains("already exists"), "{error:#}");
        assert_eq!(std::fs::read(state.join("enzyme.db-wal")).unwrap(), b"wal bytes");
        assert_eq!(std::fs::read(state.join("index.db-wal")).unwrap(), b"stale wal");
        assert_eq!(std::fs::read(state.join("index.db")).unwrap(), b"index bytes");
        std::fs::remove_file(state.join("index.db-wal")).unwrap();
        resolve_at(&margins_home, "practice").unwrap();
        assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");

        // Both names: enzyme.db is the index; index.db is left alone.
        std::fs::write(state.join("index.db"), b"older").unwrap();
        resolve_at(&margins_home, "practice").unwrap();
        assert_eq!(std::fs::read(state.join("enzyme.db")).unwrap(), b"index bytes");
        assert_eq!(std::fs::read(state.join("index.db")).unwrap(), b"older");
    }

    /// A home from when `settings` was a legal Workspace id: its program sits
    /// in the reserved `configs/settings.enzyme` slot, and the legacy machine
    /// file names it as the default.
    fn home_with_settings_workspace(temp: &Path) -> (PathBuf, PathBuf) {
        let margins_home = temp.join("state");
        let notes = temp.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let practice = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let text = practice
            .program
            .text()
            .replace("workspace \"practice\"", "workspace \"settings\"")
            .replace("workspaces/practice/captures", "workspaces/settings/captures");
        std::fs::write(margins_home.join("configs/settings.enzyme"), &text).unwrap();
        let state = margins_home.join("workspaces/settings");
        std::fs::create_dir_all(state.join("captures")).unwrap();
        std::fs::write(state.join("captures/one.wav"), b"audio").unwrap();
        std::fs::write(state.join("ledger.db"), b"ledger").unwrap();
        std::fs::write(
            margins_home.join("config.toml"),
            "[workspace]\ndefault = \"settings\"\n[workspace.names]\nsettings = \"My settings\"\n[retention.settings]\nraw_cache_max_age_days = 9\n[llm]\nmode = \"local\"\n",
        )
        .unwrap();
        (margins_home, state)
    }

    #[test]
    fn reserved_workspaces_fail_loudly_with_a_rename_path() {
        let temp = tempfile::tempdir().unwrap();
        let (margins_home, _) = home_with_settings_workspace(temp.path());
        let hint = "margins workspace rename settings <new-id>";

        let entries = list_workspace_entries(&margins_home).unwrap();
        let settings = entries.iter().find(|(id, _)| id == "settings").expect("listed, not hidden");
        let error = settings.1.as_ref().unwrap_err();
        assert!(format!("{error:#}").contains(hint), "{error:#}");
        let error = default_workspace(&margins_home).unwrap_err();
        assert!(format!("{error:#}").contains(hint), "{error:#}");
        // Nothing moved or migrated.
        assert!(margins_home.join("config.toml").is_file());
        assert!(margins_home.join("configs/settings.enzyme").is_file());

        // A legacy Workspace under a reserved id is listed with the same path.
        let legacy = margins_home.join("workspaces/margins-sources");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("config.toml"), "id = \"margins-sources\"\n").unwrap();
        let entries = list_workspace_entries(&margins_home).unwrap();
        let (_, resolved) = entries.iter().find(|(id, _)| id == "margins-sources").unwrap();
        assert!(format!("{:#}", resolved.as_ref().unwrap_err())
            .contains("margins workspace rename margins-sources <new-id>"));
    }

    #[test]
    fn renaming_a_reserved_workspace_keeps_its_data_and_machine_settings() {
        let temp = tempfile::tempdir().unwrap();
        let (margins_home, old_state) = home_with_settings_workspace(temp.path());
        let original = std::fs::read_to_string(margins_home.join("configs/settings.enzyme")).unwrap();

        assert!(rename_reserved_workspace(&margins_home, "practice", "x").is_err());
        assert!(rename_reserved_workspace(&margins_home, "settings", "practice").is_err());
        assert!(rename_reserved_workspace(&margins_home, "settings", "profiles").is_err());

        let renamed = rename_reserved_workspace(&margins_home, "settings", "my-settings").unwrap();

        let new_state = margins_home.join("workspaces/my-settings");
        assert_eq!(renamed.state_dir, new_state);
        assert_eq!(std::fs::read(new_state.join("captures/one.wav")).unwrap(), b"audio");
        assert_eq!(std::fs::read(new_state.join("ledger.db")).unwrap(), b"ledger");
        // Recorded absolute paths under the old id keep resolving.
        assert_eq!(std::fs::read(old_state.join("captures/one.wav")).unwrap(), b"audio");
        assert_eq!(renamed.compatibility_link.as_deref(), Some(old_state.as_path()));
        assert_eq!(
            std::fs::read_to_string(margins_home.join("configs/settings.enzyme.renamed-to-my-settings")).unwrap(),
            original
        );
        let workspace = resolve_at(&margins_home, "my-settings").unwrap();
        assert_eq!(workspace.config.name.as_deref(), Some("My settings"));
        assert_eq!(workspace.config.retention.raw_cache_max_age_days, Some(9));
        assert_eq!(workspace.capture_store_dir().unwrap(), new_state.join("captures"));
        assert_eq!(default_workspace(&margins_home).unwrap().as_deref(), Some("my-settings"));
        // The reserved slot now holds the engine settings migrated from config.toml.
        let settings = crate::machine_config::engine_settings(&margins_home).unwrap();
        assert_eq!(settings.generation.as_deref(), Some("local"));
        assert!(margins_home.join("config.toml.migrated").is_file());
        let ids = list_workspace_entries(&margins_home)
            .unwrap()
            .into_iter()
            .map(|(id, resolved)| (id, resolved.is_ok()))
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![("my-settings".to_string(), true), ("practice".to_string(), true)]);

        // Re-running is a no-op that reports the same rename.
        let again = rename_reserved_workspace(&margins_home, "settings", "my-settings").unwrap();
        assert_eq!(again.program_path, renamed.program_path);
    }

    #[test]
    fn renaming_resumes_after_an_interruption() {
        let temp = tempfile::tempdir().unwrap();
        let (margins_home, _) = home_with_settings_workspace(temp.path());
        let original = std::fs::read_to_string(margins_home.join("configs/settings.enzyme")).unwrap();
        rename_reserved_workspace(&margins_home, "settings", "kept").unwrap();
        // As if interrupted after the new program was written: the old
        // declaration is back in its slot and machine config is unchanged.
        std::fs::rename(
            margins_home.join("configs/settings.enzyme"),
            margins_home.join("configs/engine-settings.bak"),
        )
        .unwrap();
        std::fs::remove_file(margins_home.join("configs/settings.enzyme.renamed-to-kept")).unwrap();
        std::fs::write(margins_home.join("configs/settings.enzyme"), &original).unwrap();
        let resumed = rename_reserved_workspace(&margins_home, "settings", "kept").unwrap();
        assert!(resumed.retired.is_file());
        assert!(resolve_at(&margins_home, "kept").is_ok());
    }

    #[test]
    fn renaming_a_legacy_reserved_workspace_migrates_it_under_the_new_id() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let state = margins_home.join("workspaces/profiles");
        std::fs::create_dir_all(&state).unwrap();
        let legacy = format!(
            "id = \"profiles\"\nname = \"Profiles\"\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n",
            notes.canonicalize().unwrap()
        );
        std::fs::write(state.join("config.toml"), &legacy).unwrap();
        std::fs::write(state.join("enzyme.db"), b"index").unwrap();

        let renamed = rename_reserved_workspace(&margins_home, "profiles", "people").unwrap();

        let new_state = margins_home.join("workspaces/people");
        assert_eq!(std::fs::read(new_state.join("enzyme.db")).unwrap(), b"index");
        assert_eq!(std::fs::read_to_string(&renamed.retired).unwrap(), legacy);
        assert!(!new_state.join("config.toml").exists());
        assert!(resolve_at(&margins_home, "people").is_ok());
        assert!(!margins_home.join("configs/profiles.enzyme").exists());
    }

    #[test]
    fn engine_programs_in_configs_are_not_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        create_workspace(&margins_home, "practice", None, &notes).unwrap();
        crate::machine_config::set_generation(&margins_home, "local").unwrap();
        std::fs::write(margins_home.join("configs/profiles.enzyme"), "").unwrap();
        let ids = list_workspace_entries(&margins_home)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["practice".to_string()]);
        for reserved in ["settings", "profiles", "margins-sources"] {
            let error = create_workspace(&margins_home, reserved, None, &notes).unwrap_err();
            assert!(format!("{error:#}").contains("reserved"), "{error:#}");
        }
    }

    #[test]
    fn home_and_margins_home_without_declared_binding_are_clean_refusals() {
        let temp = tempfile::tempdir().unwrap();
        let machine_home = temp.path().join("machine-home");
        let margins_home = machine_home.join(".margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&machine_home).unwrap();
        std::fs::create_dir_all(&notes).unwrap();
        create_workspace(&margins_home, "practice", None, &notes).unwrap();
        for cwd in [&machine_home, &margins_home] {
            let error = resolve_workspace(&margins_home, None, cwd).unwrap_err();
            assert!(error.to_string().contains("no workspace selected"));
        }
    }

    #[test]
    fn fresh_notes_folder_creates_once_and_persists_home_config() {
        let temp = safe_tempdir();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("Client Notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("kickoff.md"), "# Kickoff").unwrap();
        let roots = test_implicit_roots(temp.path(), &margins_home);

        let first = resolve_or_create_workspace(&roots, None, &notes).unwrap();
        assert!(first.created_implicitly);
        assert_eq!(first.workspace.config.id, "client-notes");
        assert_eq!(first.workspace.home_dir, notes.canonicalize().unwrap());
        assert_eq!(
            first.workspace.config_path,
            margins_home.join("configs/client-notes.enzyme")
        );
        let raw = std::fs::read_to_string(&first.workspace.config_path).unwrap();
        assert!(raw.contains(&format!("path {:?}", notes.canonicalize().unwrap())));
        assert!(!first.workspace.state_dir.join("config.toml").exists());

        let second = resolve_or_create_workspace(&roots, None, &notes).unwrap();
        assert!(!second.created_implicitly);
        assert_eq!(second.workspace.config.id, "client-notes");
        assert_eq!(list_workspaces(&margins_home).unwrap().len(), 1);
    }

    #[test]
    fn nested_folder_resolves_declared_home_without_creation() {
        let temp = safe_tempdir();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        let nested = notes.join("clients/acme");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(notes.join("index.md"), "# Notes").unwrap();
        create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let roots = test_implicit_roots(temp.path(), &margins_home);

        let resolved = resolve_or_create_workspace(&roots, None, &nested).unwrap();
        assert!(!resolved.created_implicitly);
        assert_eq!(resolved.workspace.config.id, "practice");
        assert_eq!(list_workspaces(&margins_home).unwrap().len(), 1);
    }

    #[test]
    fn ancestor_of_declared_home_refuses_with_explicit_alternative() {
        let temp = safe_tempdir();
        let margins_home = temp.path().join("state");
        let ancestor = temp.path().join("notes-root");
        let notes = ancestor.join("practice");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(ancestor.join("overview.md"), "# Overview").unwrap();
        create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let roots = test_implicit_roots(temp.path(), &margins_home);

        let error = resolve_or_create_workspace(&roots, None, &ancestor).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "cannot create implicit workspace: current directory is above a declared workspace home; use explicit setup instead: margins workspace new notes-root --home {}",
                ancestor.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn temporary_and_empty_folders_refuse_with_exact_reasons() {
        let safe = safe_tempdir();
        let margins_home = safe.path().join("state");
        let roots = test_implicit_roots(safe.path(), &margins_home);
        let temporary = roots.temp_roots[0].join("notes");
        std::fs::create_dir_all(&temporary).unwrap();
        std::fs::write(temporary.join("note.md"), "# Note").unwrap();
        let temp_error = resolve_or_create_workspace(&roots, None, &temporary).unwrap_err();
        assert!(temp_error.to_string().contains(
            "cannot create implicit workspace: current directory is inside a temporary directory; use explicit setup instead: margins workspace new"
        ));

        let empty = safe.path().join("empty-notes");
        std::fs::create_dir_all(&empty).unwrap();
        let empty_error = resolve_or_create_workspace(&roots, None, &empty).unwrap_err();
        assert_eq!(
            empty_error.to_string(),
            format!(
                "cannot create implicit workspace: current directory contains no Markdown note evidence; use explicit setup instead: margins workspace new empty-notes --home {}",
                empty.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn implicit_id_uses_numeric_suffix_on_collision() {
        let temp = safe_tempdir();
        let margins_home = temp.path().join("state");
        let first = temp.path().join("first/notes");
        let second = temp.path().join("second/notes");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("one.md"), "# One").unwrap();
        std::fs::write(second.join("two.md"), "# Two").unwrap();
        create_workspace(&margins_home, "notes", None, &first).unwrap();
        let roots = test_implicit_roots(temp.path(), &margins_home);

        let created = resolve_or_create_workspace(&roots, None, &second).unwrap();
        assert!(created.created_implicitly);
        assert_eq!(created.workspace.config.id, "notes-2");
    }

    #[test]
    fn source_policy_is_per_kind_and_closed() {
        assert_eq!(SourceKind::GoogleMail.policy().index, IndexPolicy::Ledger);
        assert!(SourceKind::GoogleCalendar.policy().cache_raw_payload);
        assert!(!SourceKind::GoogleMeet.policy().project_to_home);
        assert_eq!(SourceKind::GoogleMeet.policy().index, IndexPolicy::Ledger);
        assert!(!SourceKind::Notes.policy().cache_raw_payload);
    }

    #[test]
    fn google_sources_add_and_remove_without_touching_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        std::fs::write(workspace.ledger_path(), b"ledger evidence").unwrap();

        for (name, kind, gmail, calendar) in [
            (
                "work-mail",
                SourceKind::GoogleMail,
                Some(GmailCollectionSelector::default_declaration()),
                None,
            ),
            (
                "work-calendar",
                SourceKind::GoogleCalendar,
                None,
                Some(CalendarCollectionSelector::default_declaration()),
            ),
            ("work-meet", SourceKind::GoogleMeet, None, None),
        ] {
            let binding = match kind {
                SourceKind::GoogleMail => WorkspaceBinding::Gmail {
                    account: "owner@example.com".to_string(),
                    gmail: gmail.unwrap(),
                },
                SourceKind::GoogleCalendar => WorkspaceBinding::GoogleCalendar {
                    account: "owner@example.com".to_string(),
                    calendar: calendar.unwrap(),
                },
                SourceKind::GoogleMeet => WorkspaceBinding::GoogleMeet {
                    account: "owner@example.com".to_string(),
                },
                _ => unreachable!(),
            };
            add_source(&mut workspace, name, binding).unwrap();
            let binding = &workspace.config.bindings[name];
            assert_eq!(binding.kind(), kind);
            assert_eq!(binding.google_account(), Some("owner@example.com"));
            if kind == SourceKind::GoogleMail {
                assert_eq!(
                    binding.gmail_selector(),
                    Some(&GmailCollectionSelector::default_declaration())
                );
            } else {
                assert!(binding.gmail_selector().is_none());
            }
            if kind == SourceKind::GoogleCalendar {
                assert_eq!(
                    binding.calendar_selector(),
                    Some(&CalendarCollectionSelector::default_declaration())
                );
            } else {
                assert!(binding.calendar_selector().is_none());
            }
        }
        let program = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(program.contains(
            "source google-mail \"work-mail\" { account \"owner@example.com\" }"
        ));
        assert!(program.contains(
            "source google-calendar \"work-calendar\" { account \"owner@example.com\" }"
        ));
        // Defaults equal default_declaration() and are omitted.
        assert!(!program.contains("backfill days"));
        assert!(!program.contains("lookback days"));
        assert_eq!(
            workspace.google_dir("owner@example.com").unwrap(),
            margins_home.join("google/owner@example.com")
        );

        for name in ["work-mail", "work-calendar", "work-meet"] {
            remove_source(&mut workspace, name).unwrap();
        }
        assert_eq!(
            std::fs::read(workspace.ledger_path()).unwrap(),
            b"ledger evidence"
        );
        for name in ["work-mail", "work-calendar", "work-meet"] {
            assert!(!workspace.config.bindings.contains_key(name));
        }
    }

    #[test]
    fn machine_google_connection_is_shared_by_declaring_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes_a = temp.path().join("notes-a");
        let notes_b = temp.path().join("notes-b");
        std::fs::create_dir_all(&notes_a).unwrap();
        std::fs::create_dir_all(&notes_b).unwrap();
        let mut a = create_workspace(&margins_home, "alpha", None, &notes_a).unwrap();
        let mut b = create_workspace(&margins_home, "beta", None, &notes_b).unwrap();
        for workspace in [&mut a, &mut b] {
            add_source(
                workspace,
                "work-mail",
                WorkspaceBinding::Gmail {
                    account: "owner@example.com".to_string(),
                    gmail: GmailCollectionSelector::default_declaration(),
                },
            )
            .unwrap();
        }

        assert_eq!(
            a.google_dir("owner@example.com").unwrap(),
            b.google_dir("owner@example.com").unwrap()
        );
        assert_eq!(
            workspaces_using_google_account(&margins_home, "owner@example.com").unwrap(),
            vec!["alpha", "beta"]
        );
    }

    #[test]
    fn gmail_selector_validation_rejects_zero_backfill_and_temporal_query_terms() {
        assert!(GmailCollectionSelector {
            query: DEFAULT_GMAIL_QUERY.to_string(),
            backfill_days: 0,
        }
        .validate()
        .unwrap_err()
        .to_string()
        .contains("backfill_days"));

        assert!(GmailCollectionSelector {
            query: "label:important -(after:2026/01/01)".to_string(),
            backfill_days: 30,
        }
        .validate()
        .unwrap_err()
        .to_string()
        .contains("temporal operator"));

        let selector = GmailCollectionSelector {
            query: String::new(),
            backfill_days: 30,
        };
        selector.validate().unwrap();
        assert_eq!(
            GmailCollectionSelector::default_declaration(),
            GmailCollectionSelector {
                query: "-in:spam -in:trash".into(),
                backfill_days: 365,
            }
        );
    }

    #[test]
    fn workspace_rejects_duplicate_gmail_accounts_and_overlapping_native_roots() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        add_source(
            &mut workspace,
            "mail-a",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        )
        .unwrap();
        let duplicate = add_source(
            &mut workspace,
            "mail-b",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        );
        assert!(duplicate
            .unwrap_err()
            .to_string()
            .contains("two Gmail sources"));

        let reference = temp.path().join("reference");
        std::fs::create_dir_all(&reference).unwrap();
        let foreign = add_source(
            &mut workspace,
            "calendar",
            WorkspaceBinding::GoogleCalendar {
                account: "owner@example.com".to_string(),
                calendar: CalendarCollectionSelector::default_declaration(),
            },
        );
        assert!(foreign.is_ok());
        let duplicate_calendar = add_source(
            &mut workspace,
            "calendar-b",
            WorkspaceBinding::GoogleCalendar {
                account: "owner@example.com".to_string(),
                calendar: CalendarCollectionSelector::default_declaration(),
            },
        );
        assert!(duplicate_calendar
            .unwrap_err()
            .to_string()
            .contains("two Calendar sources"));
        let overlap = add_source(
            &mut workspace,
            "ref",
            WorkspaceBinding::NativeMarkdown {
                path: notes.clone(),
                role: SourceRole::Reference,
                note_folder: None,
            },
        );
        assert!(overlap.unwrap_err().to_string().contains("overlap"));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_rejects_native_root_through_symlink_alias() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        let alias = temp.path().join("notes-alias");
        std::fs::create_dir_all(&notes).unwrap();
        std::os::unix::fs::symlink(&notes, &alias).unwrap();
        let mut workspace =
            create_workspace(&temp.path().join("state"), "practice", None, &notes).unwrap();

        let overlap = add_source(
            &mut workspace,
            "alias",
            WorkspaceBinding::NativeMarkdown {
                path: alias,
                role: SourceRole::Reference,
                note_folder: None,
            },
        );
        assert!(overlap.unwrap_err().to_string().contains("overlap"));
    }

    #[test]
    fn program_roundtrip_declares_sources_and_home_policy() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let raw = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(raw.contains("workspace \"practice\" {"), "{raw}");
        assert!(raw.contains("source markdown \"home\""), "{raw}");
        assert!(raw.contains("source margins-captures \"captures\""), "{raw}");
        assert!(raw.contains("remember in folder \".\" create note"), "{raw}");
        assert!(!raw.contains("[bindings"));

        let reparsed = WorkspaceProgram::parse(&raw).unwrap();
        assert_eq!(reparsed, workspace.program);
        assert_eq!(workspace_revision(&workspace).unwrap(), program_sha256(&raw));
        assert_eq!(
            program_lang::derive_view(&reparsed, None, RetentionPolicy::default()).unwrap(),
            workspace.config
        );
    }

    #[test]
    fn workspace_entity_curation_renders_readings() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let entities = vec![
            WorkspaceEntity::simple("#enzyme"),
            WorkspaceEntity::with_options(
                "folder:people",
                WorkspaceEntityOptions {
                    profile: Some("relational".to_string()),
                    expandable: true,
                    children: Vec::new(),
                },
            ),
        ];
        update_policy(
            &mut workspace,
            WorkspacePolicy {
                entities: entities.clone(),
                ..WorkspacePolicy::default()
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(raw.contains("learn questions from tag \"enzyme\""), "{raw}");
        assert!(
            raw.contains(
                "learn questions from folder \"people\"\n    including linked pages\n    about relational"
            ),
            "{raw}"
        );
        assert_eq!(workspace.config.policy.entities, entities);
    }

    #[test]
    fn config_rejects_fields_owned_by_another_binding_variant() {
        let invalid = r#"
id = "practice"

[bindings.home]
kind = "notes"
path = "/absolute/notes"
role = "home"
account = "owner@example.com"
"#;
        let error = toml::from_str::<WorkspaceConfig>(invalid).unwrap_err();
        assert!(error.to_string().contains("unknown field `account`"));
    }

    #[test]
    fn collection_namespaces_are_stable_and_selector_independent() {
        let path = PathBuf::from("/abs/practice/notes");
        let first = native_markdown_collection_namespace(&path).unwrap();
        let second = native_markdown_collection_namespace(&path).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("markdown_"));
        assert_eq!(first.len(), "markdown_".len() + 64);

        let gmail_first = gmail_collection_namespace("owner@example.com").unwrap();
        let gmail_second = gmail_collection_namespace("OWNER@example.com").unwrap();
        assert_eq!(gmail_first, gmail_second);
        assert!(gmail_first.starts_with("gmail_"));
        assert_eq!(gmail_first.len(), "gmail_".len() + 64);
        assert_ne!(gmail_first, first);
    }

    #[test]
    fn calendar_selector_validation_rejects_zero_windows_and_fingerprints_deterministically() {
        assert!(CalendarCollectionSelector {
            lookback_days: 0,
            lookahead_days: 180,
        }
        .validate()
        .unwrap_err()
        .to_string()
        .contains("lookback_days"));

        assert!(CalendarCollectionSelector {
            lookback_days: 365,
            lookahead_days: 0,
        }
        .validate()
        .unwrap_err()
        .to_string()
        .contains("lookahead_days"));

        let selector = CalendarCollectionSelector {
            lookback_days: 90,
            lookahead_days: 30,
        };
        selector.validate().unwrap();
        let first = selector.materialization_fingerprint().unwrap();
        let second = selector.materialization_fingerprint().unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("calendar-selector-v1:"));

        assert_eq!(
            CalendarCollectionSelector::default_declaration(),
            CalendarCollectionSelector {
                lookback_days: 365,
                lookahead_days: 180,
            }
        );
    }

    #[test]
    fn calendar_collection_namespace_is_stable_and_selector_independent() {
        let first = calendar_collection_namespace("owner@example.com").unwrap();
        let second = calendar_collection_namespace("OWNER@example.com").unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("calendar_"));
        assert_eq!(first.len(), "calendar_".len() + 64);
        assert_ne!(
            first,
            gmail_collection_namespace("owner@example.com").unwrap()
        );
    }

    #[test]
    fn gmail_selector_lookup_matches_the_requested_account() {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            "alpha-mail".to_string(),
            WorkspaceBinding::Gmail {
                account: "alpha@example.com".to_string(),
                gmail: GmailCollectionSelector {
                    query: "label:alpha".to_string(),
                    backfill_days: 30,
                },
            },
        );
        bindings.insert(
            "beta-mail".to_string(),
            WorkspaceBinding::Gmail {
                account: "beta@example.com".to_string(),
                gmail: GmailCollectionSelector {
                    query: "label:beta".to_string(),
                    backfill_days: 90,
                },
            },
        );
        let config = WorkspaceConfig {
            id: "practice".to_string(),
            name: None,
            policy: WorkspacePolicy::default(),
            retention: RetentionPolicy::default(),
            bindings,
        };

        assert_eq!(
            gmail_selector_for_account(&config, "BETA@example.com").unwrap(),
            GmailCollectionSelector {
                query: "label:beta".to_string(),
                backfill_days: 90,
            }
        );
    }

    #[test]
    fn failed_add_and_remove_do_not_mutate_config() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();

        add_source(
            &mut workspace,
            "mail",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        )
        .unwrap();

        let nested = notes.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let failed_add = add_source(
            &mut workspace,
            "ref",
            WorkspaceBinding::NativeMarkdown {
                path: nested,
                role: SourceRole::Reference,
                note_folder: None,
            },
        );
        assert!(failed_add.unwrap_err().to_string().contains("overlap"));
        assert!(!workspace.config.bindings.contains_key("ref"));
        let on_disk = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(on_disk.contains("source google-mail \"mail\""));
        assert!(!on_disk.contains("\"ref\""));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&workspace.config_path)
                .unwrap()
                .permissions();
            perms.set_mode(0o444);
            std::fs::set_permissions(&workspace.config_path, perms.clone()).unwrap();

            let failed_remove = remove_source(&mut workspace, "mail");
            assert!(failed_remove.is_err());
            assert!(workspace.config.bindings.contains_key("mail"));
            assert_eq!(
                std::fs::read_to_string(&workspace.config_path).unwrap(),
                on_disk
            );

            perms.set_mode(0o644);
            std::fs::set_permissions(&workspace.config_path, perms).unwrap();
        }
    }

    #[test]
    fn semantic_required_binding_removal_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();

        let home_name = workspace
            .config
            .bindings
            .iter()
            .find_map(|(name, binding)| match binding {
                WorkspaceBinding::NativeMarkdown {
                    role: SourceRole::Home,
                    ..
                } => Some(name.clone()),
                _ => None,
            })
            .unwrap();
        assert!(remove_source(&mut workspace, &home_name)
            .unwrap_err()
            .to_string()
            .contains("required home"));

        let captures_name = workspace
            .config
            .bindings
            .iter()
            .find_map(|(name, binding)| match binding {
                WorkspaceBinding::Captures { .. } => Some(name.clone()),
                _ => None,
            })
            .unwrap();
        assert!(remove_source(&mut workspace, &captures_name)
            .unwrap_err()
            .to_string()
            .contains("required captures"));
    }

    #[test]
    fn additional_captures_binding_remains_removable() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        let sessions = temp.path().join("sessions");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        add_source(
            &mut workspace,
            "sessions",
            WorkspaceBinding::Captures {
                path: sessions.clone(),
            },
        )
        .unwrap();

        let error = workspace.capture_store_dir().unwrap_err().to_string();
        assert!(error.contains("multiple writable capture destinations"));
        assert!(error.contains("captures"));
        assert!(error.contains("sessions"));

        assert_eq!(
            remove_source(&mut workspace, "sessions").unwrap(),
            WorkspaceBinding::Captures { path: sessions }
        );
        assert!(!workspace.config.bindings.contains_key("sessions"));
        assert!(workspace.config.bindings.values().any(|binding| {
            matches!(
                binding,
                WorkspaceBinding::Captures { path } if path == &workspace.captures_dir()
            )
        }));
    }

    #[test]
    fn declared_capture_store_is_stable_across_client_directories() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();

        assert_eq!(
            workspace.capture_store_dir().unwrap(),
            margins_home.join("workspaces/practice/captures")
        );
    }

    #[test]
    fn meet_collection_namespace_is_stable_and_account_normalized() {
        let meet_first = meet_collection_namespace("owner@example.com").unwrap();
        let meet_second = meet_collection_namespace("OWNER@example.com").unwrap();
        assert_eq!(meet_first, meet_second);
        assert!(meet_first.starts_with("meet_"));
        assert_eq!(meet_first.len(), "meet_".len() + 64);
    }

    #[test]
    fn workspace_rejects_duplicate_meet_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();

        add_source(
            &mut workspace,
            "meet-a",
            WorkspaceBinding::GoogleMeet {
                account: "owner@example.com".to_string(),
            },
        )
        .unwrap();
        let duplicate_meet = add_source(
            &mut workspace,
            "meet-b",
            WorkspaceBinding::GoogleMeet {
                account: "owner@example.com".to_string(),
            },
        );
        assert!(duplicate_meet
            .unwrap_err()
            .to_string()
            .contains("two Google Meet sources"));
    }

    fn mail_binding() -> WorkspaceBinding {
        WorkspaceBinding::Gmail {
            account: "owner@example.com".to_string(),
            gmail: GmailCollectionSelector::default_declaration(),
        }
    }

    #[test]
    fn workspace_plan_apply_derives_replay_identity_and_rejects_stale_plans() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let mut desired = workspace.config.clone();
        desired.policy.excluded_folders = vec!["archive".to_string()];
        desired.bindings.insert("mail".to_string(), mail_binding());

        let plan = plan_workspace_config(&workspace, desired).unwrap();
        assert_eq!(plan.schema_version, WORKSPACE_PLAN_SCHEMA);
        assert_eq!(plan.actions.len(), 2, "{:?}", plan.actions);
        assert_eq!(plan.base_revision, workspace.program.sha256());
        assert_eq!(plan.desired_sha256, program_sha256(&plan.desired_program));
        assert!(plan.diff.contains("+  source google-mail \"mail\""), "{}", plan.diff);
        assert!(plan
            .actions
            .iter()
            .any(|action| action.summary().contains("leave out folders archive")));
        let receipt = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(!receipt.replayed);
        assert_eq!(receipt.schema_version, WORKSPACE_APPLY_SCHEMA);
        assert_eq!(receipt.request_hash.len(), 64);
        assert_eq!(
            receipt.request_id,
            format!("workspace-apply-{}", receipt.request_hash)
        );
        assert_eq!(receipt.after_revision, plan.desired_sha256);
        assert_eq!(
            std::fs::read_to_string(&workspace.config_path).unwrap(),
            plan.desired_program
        );
        assert!(workspace.config.bindings.contains_key("mail"));

        let replay = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.after_revision, receipt.after_revision);

        // Re-planning the applied program is a no-op.
        let unchanged = plan_workspace_program(&workspace, workspace.program.text()).unwrap();
        assert!(unchanged.actions.is_empty());
        assert!(unchanged.diff.is_empty());

        let stale_plan = plan_workspace_config(&workspace, {
            let mut desired = workspace.config.clone();
            desired.policy.excluded_tags = vec!["private".to_string()];
            desired
        })
        .unwrap();
        let mut changed_policy = workspace.config.policy.clone();
        changed_policy
            .entities
            .push(WorkspaceEntity::simple("[[stale-plan@example.com]]"));
        update_policy(&mut workspace, changed_policy).unwrap();
        let stale = apply_workspace_plan(&mut workspace, &stale_plan).unwrap_err();
        assert!(stale
            .downcast_ref::<WorkspaceMutationError>()
            .is_some_and(|error| matches!(error, WorkspaceMutationError::RevisionConflict { .. })));
    }

    #[test]
    fn workspace_apply_refuses_altered_plans() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let mut desired = workspace.config.clone();
        desired.bindings.insert("mail".to_string(), mail_binding());
        let plan = plan_workspace_config(&workspace, desired).unwrap();
        let before = std::fs::read_to_string(&workspace.config_path).unwrap();
        let invalid_plan = |error: anyhow::Error| {
            error
                .downcast_ref::<WorkspaceMutationError>()
                .is_some_and(|error| matches!(error, WorkspaceMutationError::InvalidPlan(_)))
        };

        // Text altered without updating its digest.
        let mut altered = plan.clone();
        altered.desired_program = altered.desired_program.replace("owner@", "other@");
        assert!(invalid_plan(apply_workspace_plan(&mut workspace, &altered).unwrap_err()));

        // Text and digest altered consistently: the plan identity no longer matches.
        altered.desired_sha256 = program_sha256(&altered.desired_program);
        assert!(invalid_plan(apply_workspace_plan(&mut workspace, &altered).unwrap_err()));

        // Reviewed summary or diff edited after review.
        let mut altered = plan.clone();
        altered.diff.push_str("+ something else\n");
        assert!(invalid_plan(apply_workspace_plan(&mut workspace, &altered).unwrap_err()));
        let mut altered = plan.clone();
        altered.actions.clear();
        assert!(invalid_plan(apply_workspace_plan(&mut workspace, &altered).unwrap_err()));

        assert_eq!(std::fs::read_to_string(&workspace.config_path).unwrap(), before);
        apply_workspace_plan(&mut workspace, &plan).unwrap();
    }

    #[test]
    fn workspace_plan_keeps_language_features_and_reports_program_only_changes() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let program = workspace.program.text().replace(
            "  remember in folder",
            "  learn questions from folder \"people\" about relationships {\n    sample by time\n  }\n\n  when asked {\n    \"Use grep for exact names.\"\n  }\n\n  remember in folder",
        );
        let plan = plan_workspace_program(&workspace, &program).unwrap();
        assert!(plan
            .actions
            .iter()
            .any(|action| matches!(action, WorkspacePlanAction::SetPolicy { .. })));
        assert!(plan
            .actions
            .iter()
            .any(|action| matches!(action, WorkspacePlanAction::UpdateProgram { .. })));
        apply_workspace_plan(&mut workspace, &plan).unwrap();

        // A view-level mutation preserves the statements the view cannot express.
        add_source(&mut workspace, "mail", mail_binding()).unwrap();
        let text = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(text.contains("sample by time"), "{text}");
        assert!(text.contains("Use grep for exact names."), "{text}");
        assert!(text.contains("about relationships"), "{text}");
    }

    #[test]
    fn workspace_apply_recovers_a_durable_prepared_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let mut desired = workspace.config.clone();
        desired.bindings.insert("mail".to_string(), mail_binding());
        let plan = plan_workspace_config(&workspace, desired).unwrap();
        let request_hash = workspace_apply_request_hash(&plan).unwrap();
        let request_id = format!("workspace-apply-{request_hash}");
        let receipt = WorkspaceApplyReceipt {
            schema_version: WORKSPACE_APPLY_SCHEMA.to_string(),
            ok: true,
            workspace_id: plan.workspace_id.clone(),
            request_id: request_id.clone(),
            request_hash,
            plan_id: plan.plan_id.clone(),
            before_revision: plan.base_revision.clone(),
            after_revision: plan.desired_sha256.clone(),
            replayed: false,
            actions: plan
                .actions
                .iter()
                .cloned()
                .enumerate()
                .map(|(position, action)| WorkspaceActionReceipt {
                    position,
                    status: "applied".to_string(),
                    action,
                })
                .collect(),
        };
        // A crash after journaling the transaction, before the program write.
        store_workspace_transaction(
            &workspace.state_dir,
            &WorkspaceTransaction {
                receipt,
                desired_program: plan.desired_program.clone(),
            },
        )
        .unwrap();

        let recovered = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(recovered.replayed);
        assert!(workspace.config.bindings.contains_key("mail"));
        assert_eq!(
            std::fs::read_to_string(&workspace.config_path).unwrap(),
            plan.desired_program
        );
        assert!(!workspace_transaction_path(&workspace.state_dir).exists());
        assert!(workspace_receipt_path(&workspace.state_dir, &request_id).is_file());
    }

    fn write_legacy(margins_home: &Path, id: &str, body: &str) -> PathBuf {
        let state_dir = margins_home.join(WORKSPACES_DIR).join(id);
        std::fs::create_dir_all(&state_dir).unwrap();
        let path = state_dir.join(LEGACY_WORKSPACE_CONFIG);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn legacy_home(notes: &Path, extra: &str) -> String {
        format!(
            "id = \"legacy\"\n\n[bindings.home]\nkind = \"notes\"\npath = {:?}\nrole = \"home\"\n{extra}",
            notes
        )
    }

    /// One retired config per binding kind; each must migrate to a program
    /// whose view equals the legacy config's bindings.
    #[test]
    fn legacy_config_migrates_every_binding_kind() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        let reference = temp.path().join("reference");
        let captures = temp.path().join("captures");
        for path in [&notes, &reference, &captures] {
            std::fs::create_dir_all(path).unwrap();
        }
        let fixtures = [
            ("notes", format!("\n[bindings.ref]\nkind = \"notes\"\npath = {reference:?}\nrole = \"reference\"\n"), "source markdown \"ref\""),
            ("captures", format!("\n[bindings.captures]\nkind = \"captures\"\npath = {captures:?}\n"), "source margins-captures \"captures\""),
            ("google-mail", "\n[bindings.mail]\nkind = \"google-mail\"\naccount = \"me@example.com\"\n\n[bindings.mail.gmail]\nquery = \"label:clients\"\nbackfill_days = 90\n".to_string(), "backfill days 90"),
            ("google-calendar", "\n[bindings.calendar]\nkind = \"google-calendar\"\naccount = \"me@example.com\"\n\n[bindings.calendar.calendar]\nlookback_days = 365\nlookahead_days = 30\n".to_string(), "lookahead days 30"),
            ("google-meet", "\n[bindings.meet]\nkind = \"google-meet\"\naccount = \"me@example.com\"\n".to_string(), "source google-meet \"meet\" { account \"me@example.com\" }"),
            ("granola", "\n[bindings.granola]\nkind = \"granola\"\naccount = \"me@example.com\"\n\n[bindings.granola.collection]\ntime_range = \"last_30_days\"\nworkspace_only = true\n".to_string(), "workspace only true"),
        ];
        for (kind, extra, expected) in fixtures {
            let margins_home = temp.path().join(format!("home-{kind}"));
            let legacy_text = legacy_home(&notes, &extra);
            write_legacy(&margins_home, "legacy", &legacy_text);
            let legacy: WorkspaceConfig = toml::from_str(&legacy_text).unwrap();

            let preview = preview_workspace_migration(&margins_home, "legacy").unwrap();
            assert_eq!(preview.status, "would_migrate");
            assert!(preview.program.contains(expected), "{kind}: {}", preview.program);
            assert!(!workspace_program_path(&margins_home, "legacy").unwrap().exists());

            let workspace = resolve_at(&margins_home, "legacy").unwrap();
            assert_eq!(workspace.program.text(), preview.program, "{kind}: dry run differs");
            assert_eq!(workspace.config.bindings, legacy.bindings, "{kind}");
            assert!(workspace
                .state_dir
                .join(LEGACY_WORKSPACE_CONFIG_MIGRATED)
                .is_file());
            assert!(!workspace.state_dir.join(LEGACY_WORKSPACE_CONFIG).exists());
            let replanned = plan_workspace_program(&workspace, workspace.program.text()).unwrap();
            assert!(replanned.actions.is_empty(), "{kind}");
        }
    }

    #[test]
    fn mixed_legacy_config_migrates_policy_retention_and_name() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("machine");
        let notes = temp.path().join("notes");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        std::fs::create_dir_all(notes.join("inbox")).unwrap();
        std::fs::create_dir_all(&reference).unwrap();
        let reference_namespace = native_markdown_collection_namespace(&reference).unwrap();
        let legacy_text = format!(
            r##"id = "legacy"
name = "Client practice"

[policy]
excluded_folders = [".git", "node_modules", "archive"]
excluded_tags = ["private"]
excluded_entities = ["[[Noise Sender]]", "folder:old", "#draft"]
entities = [
  "#craft",
  {{ "folder:people" = {{ profile = "relational", expandable = true }} }},
  {{ "folder:reference/papers" = {{ profile = "decision_trace" }} }},
  {{ "folder:{reference_namespace}/papers" = {{ profile = "decision_trace" }} }},
]

[retention]
raw_cache_max_age_days = 30

[bindings.home]
kind = "notes"
path = {notes:?}
role = "home"
note_folder = "inbox"

[bindings.reference]
kind = "notes"
path = {reference:?}
role = "reference"

[bindings.mail]
kind = "google-mail"
account = "me@example.com"

[bindings.mail.gmail]
query = "-in:spam -in:trash"
backfill_days = 365
"##
        );
        write_legacy(&margins_home, "legacy", &legacy_text);

        let migration = migrate_workspace(&margins_home, "legacy").unwrap();
        assert_eq!(migration.status, "migrated");
        let text = &migration.program;
        assert!(text.contains("remember in folder \"inbox\" in source \"home\" create note"), "{text}");
        assert!(text.contains("learn questions from tag \"craft\""), "{text}");
        // Several Markdown roots: unqualified legacy folders meant the Home
        // root, even when their first segment names another source; the
        // internal identity of a reference root maps back to its source name.
        assert!(text.contains("learn questions from folder \"home/people\"\n    including linked pages\n    about relational"), "{text}");
        assert!(text.contains("learn questions from folder \"home/reference/papers\"\n    about decision_trace"), "{text}");
        assert!(text.contains("learn questions from folder \"reference/papers\"\n    about decision_trace"), "{text}");
        assert!(!text.contains("markdown_"), "{text}");
        assert!(migration.warnings.is_empty(), "{:?}", migration.warnings);
        assert!(text.contains("leave out folders [\"archive\", \"old\"]"), "{text}");
        assert!(text.contains("leave out tags [\"private\", \"draft\"]"), "{text}");
        assert!(text.contains("leave out links [\"Noise Sender\"]"), "{text}");
        assert!(!text.contains(".git"), "{text}");
        assert!(!text.contains("backfill days"), "{text}");

        let workspace = resolve_at(&margins_home, "legacy").unwrap();
        assert_eq!(workspace.config.name.as_deref(), Some("Client practice"));
        assert_eq!(workspace.config.retention.raw_cache_max_age_days, Some(30));
        let machine = std::fs::read_to_string(margins_home.join("margins.toml")).unwrap();
        assert!(machine.contains("[retention.legacy]"), "{machine}");
        assert_eq!(workspace.note_destination().unwrap(), notes.canonicalize().unwrap().join("inbox"));

        // Idempotent: a second migration neither rewrites nor reinterprets.
        let again = migrate_workspace(&margins_home, "legacy").unwrap();
        assert_eq!(again.status, "already_migrated");
        assert_eq!(again.program, migration.program);
        assert!(plan_workspace_program(&workspace, workspace.program.text())
            .unwrap()
            .actions
            .is_empty());
        // A legacy desired TOML for the same settings plans to no view change.
        let legacy: WorkspaceConfig = toml::from_str(&legacy_text).unwrap();
        let mut legacy = legacy;
        legacy.policy.excluded_folders.retain(|folder| folder != ".git" && folder != "node_modules");
        let legacy_plan = plan_legacy_workspace_config(&workspace, legacy).unwrap();
        assert!(
            legacy_plan.actions.iter().all(|action| !matches!(action, WorkspacePlanAction::AddBinding { .. } | WorkspacePlanAction::RemoveBinding { .. })),
            "{:?}",
            legacy_plan.actions
        );
    }

    /// Entity forms the previous engine ignored must not block migration (and
    /// so every Workspace); they are dropped and reported.
    #[test]
    fn legacy_entities_the_previous_engine_ignored_are_dropped_and_reported() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("machine");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        let legacy_text = legacy_home(
            &notes,
            r##"
[policy]
excluded_entities = ["person:ada", "Plain", "tag:x", "[[Noise]]", "#draft", "folder:old"]
entities = [
  "Project: Atlas",
  "person:ada",
  "tag:craft",
  "[[Noise]]",
  { "[[Project: Atlas]]" = { expandable = true } },
  { "#craft" = { profile = "relational", expandable = true } },
  "log:journal",
  { "folder:people" = { expandable = true } },
  "folder:people",
]
"##,
        );
        let legacy_path = write_legacy(&margins_home, "legacy", &legacy_text);

        let preview = preview_workspace_migration(&margins_home, "legacy").unwrap();
        let migration = migrate_workspace(&margins_home, "legacy").unwrap();
        assert_eq!(preview.program, migration.program);
        assert_eq!(preview.warnings, migration.warnings);
        assert!(!legacy_path.exists());
        let text = &migration.program;
        assert!(text.contains("learn questions from link \"Project: Atlas\"\n"), "{text}");
        assert!(text.contains("learn questions from tag \"craft\"\n    about relational"), "{text}");
        assert!(text.contains("learn questions from log \"journal\""), "{text}");
        assert!(text.contains("learn questions from folder \"people\"\n    including linked pages"), "{text}");
        assert!(text.contains("leave out folders [\"old\"]"), "{text}");
        assert!(text.contains("leave out tags [\"draft\"]"), "{text}");
        assert!(text.contains("leave out links [\"Noise\"]"), "{text}");
        for ignored in ["Atlas\"\n    including", "ada", "Plain", "\"x\"", "from link \"Noise\""] {
            assert!(!text.contains(ignored), "{ignored}: {text}");
        }
        assert_eq!(text.matches("folder \"people\"").count(), 1, "{text}");
        let warnings = migration.warnings.join("\n");
        for reported in [
            "excluded entity \"person:ada\"",
            "excluded entity \"Plain\"",
            "excluded entity \"tag:x\"",
            "entity \"Project: Atlas\" is not",
            "entity \"person:ada\" is not",
            "entity \"tag:craft\" is not",
            "entity \"[[Noise]]\" is also excluded",
            "entity \"[[Project: Atlas]]\" is expandable",
            "entity \"#craft\" is expandable",
            "entity \"folder:people\" repeats",
        ] {
            assert!(warnings.contains(reported), "{reported}: {warnings}");
        }
        assert_eq!(migration.warnings.len(), 10, "{warnings}");
        let json = serde_json::to_value(&migration).unwrap();
        assert_eq!(json["warnings"].as_array().unwrap().len(), 10);
    }

    /// One Workspace that cannot migrate leaves its legacy file untouched and
    /// does not hide the others.
    #[test]
    fn a_workspace_that_cannot_migrate_does_not_hide_the_others() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("machine");
        let notes = temp.path().join("notes");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        create_workspace(&margins_home, "good", Some("Good"), &other).unwrap();
        let broken = legacy_home(&notes, "")
            .replace("id = \"legacy\"", "id = \"broken\"\nname = \"Broken\"")
            .replace("role = \"home\"", "role = \"home\"\nnote_folder = \"../outside\"");
        let legacy_path = write_legacy(&margins_home, "broken", &broken);

        assert!(resolve_at(&margins_home, "broken").is_err());
        assert!(migrate_workspace(&margins_home, "broken").is_err());
        assert_eq!(std::fs::read_to_string(&legacy_path).unwrap(), broken);
        assert!(!workspace_program_path(&margins_home, "broken").unwrap().exists());
        assert!(!margins_home.join(WORKSPACES_DIR).join("broken").join(LEGACY_WORKSPACE_CONFIG_MIGRATED).exists());
        assert_eq!(workspace_display_name(&margins_home, "broken").unwrap(), None);

        let listed = list_workspaces(&margins_home).unwrap();
        assert_eq!(
            listed.iter().map(|workspace| workspace.config.id.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
        let entries = list_workspace_entries(&margins_home).unwrap();
        assert_eq!(entries.len(), 2);
        let (id, resolved) = &entries[0];
        assert_eq!(id, "broken");
        assert!(resolved.is_err());
        assert_eq!(std::fs::read_to_string(&legacy_path).unwrap(), broken);
    }

    /// A crash between the program write and the rename leaves both files; the
    /// next resolution retires the legacy file under a free name.
    #[test]
    fn a_legacy_config_beside_a_program_is_retired_on_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("machine");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let legacy_path = write_legacy(&margins_home, "legacy", &legacy_home(&notes, ""));
        let migrated = resolve_at(&margins_home, "legacy").unwrap();
        let state_dir = migrated.state_dir.clone();
        assert!(state_dir.join(LEGACY_WORKSPACE_CONFIG_MIGRATED).is_file());

        // Simulate the crash window, with an earlier retirement already present.
        let stale = legacy_home(&notes, "\n[policy]\nentities = [\"#stale\"]\n");
        std::fs::write(&legacy_path, &stale).unwrap();
        let workspace = resolve_at(&margins_home, "legacy").unwrap();
        assert_eq!(workspace.program.text(), migrated.program.text());
        assert!(!legacy_path.exists());
        let numbered = state_dir.join(format!("{LEGACY_WORKSPACE_CONFIG_MIGRATED}.1"));
        assert_eq!(std::fs::read_to_string(&numbered).unwrap(), stale);
        assert!(legacy_workspace_ids(&margins_home).unwrap().is_empty());

        std::fs::write(&legacy_path, &stale).unwrap();
        resolve_at(&margins_home, "legacy").unwrap();
        assert!(state_dir.join(format!("{LEGACY_WORKSPACE_CONFIG_MIGRATED}.2")).is_file());
        assert!(!legacy_path.exists());
        assert!(is_legacy_backup_name(std::ffi::OsStr::new("config.toml.migrated.2")));
        assert!(!is_legacy_backup_name(std::ffi::OsStr::new("config.toml.migrated.x")));
    }

    /// A hand-written program whose only change is attention policy plans as
    /// that policy change, without a redundant `update_program`.
    #[test]
    fn a_policy_only_program_change_plans_without_update_program() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        let workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        for statement in [
            "  learn questions from folder \"people\"\n\n",
            "  learn questions from folder \"people\" including linked pages about relationships\n\n",
            "  leave out folders { \"archive\" }\n  leave out tags { \"draft\" }\n  leave out links { \"Noise\" }\n\n",
        ] {
            let program = workspace
                .program
                .text()
                .replace("  remember in folder", &format!("{statement}  remember in folder"));
            let plan = plan_workspace_program(&workspace, &program).unwrap();
            assert!(
                matches!(plan.actions.as_slice(), [WorkspacePlanAction::SetPolicy { .. }]),
                "{statement}: {:?}",
                plan.actions
            );
        }
        // A view-modelled change plus a statement outside the view still says so.
        let program = workspace.program.text().replace(
            "  remember in folder",
            "  learn questions from folder \"people\" {\n    sample by time\n  }\n\n  remember in folder",
        );
        let plan = plan_workspace_program(&workspace, &program).unwrap();
        assert!(plan
            .actions
            .iter()
            .any(|action| matches!(action, WorkspacePlanAction::UpdateProgram { .. })));
    }

    #[test]
    fn concurrent_resolution_migrates_once() {
        use std::sync::{Arc, Barrier};

        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("machine");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        write_legacy(&margins_home, "legacy", &legacy_home(&notes, ""));
        let barrier = Arc::new(Barrier::new(4));
        let handles = (0..4)
            .map(|_| {
                let margins_home = margins_home.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    resolve_at(&margins_home, "legacy").unwrap().program.sha256()
                })
            })
            .collect::<Vec<_>>();
        let revisions = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(revisions.len(), 1);
        assert_eq!(list_workspaces(&margins_home).unwrap().len(), 1);
        assert!(margins_home
            .join("workspaces/legacy")
            .join(LEGACY_WORKSPACE_CONFIG_MIGRATED)
            .is_file());
    }

    #[test]
    fn concurrent_source_additions_do_not_lose_updates() {
        use std::sync::{Arc, Barrier};

        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        let first_notes = temp.path().join("first");
        let second_notes = temp.path().join("second");
        for path in [&notes, &first_notes, &second_notes] {
            std::fs::create_dir_all(path).unwrap();
        }
        create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles = [("first", first_notes), ("second", second_notes)]
            .into_iter()
            .map(|(name, path)| {
                let margins_home = margins_home.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut workspace = resolve_at(&margins_home, "practice").unwrap();
                    barrier.wait();
                    add_source(
                        &mut workspace,
                        name,
                        WorkspaceBinding::NativeMarkdown {
                            path,
                            role: SourceRole::Reference,
                            note_folder: None,
                        },
                    )
                    .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        let workspace = resolve_at(&margins_home, "practice").unwrap();
        assert!(workspace.config.bindings.contains_key("first"));
        assert!(workspace.config.bindings.contains_key("second"));
    }

    #[test]
    fn reviewed_home_note_folder_and_machine_default_resolve_without_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let machine = temp.path().join("machine");
        let home = temp.path().join("vault");
        std::fs::create_dir_all(&home).unwrap();
        let mut workspace = create_workspace(&machine, "practice", None, &home).unwrap();
        assert_eq!(workspace.note_destination().unwrap(), workspace.home_dir);
        assert_eq!(default_workspace(&machine).unwrap(), None);
        set_default_workspace(&machine, "practice").unwrap();
        assert_eq!(
            default_workspace(&machine).unwrap().as_deref(),
            Some("practice")
        );
        let mut desired = workspace.config.clone();
        let WorkspaceBinding::NativeMarkdown { note_folder, .. } =
            desired.bindings.get_mut("home").unwrap()
        else {
            panic!("home binding changed")
        };
        *note_folder = Some(PathBuf::from("inbox"));
        let plan = plan_workspace_config(&workspace, desired).unwrap();
        apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert_eq!(
            resolve_at(&machine, "practice")
                .unwrap()
                .note_destination()
                .unwrap(),
            workspace.home_dir.join("inbox")
        );
        assert!(plan_workspace_config(&workspace, {
            let mut invalid = workspace.config.clone();
            let WorkspaceBinding::NativeMarkdown { note_folder, .. } =
                invalid.bindings.get_mut("home").unwrap()
            else {
                panic!()
            };
            *note_folder = Some(PathBuf::from("../outside"));
            invalid
        })
        .is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path(), home.join("outside-link")).unwrap();
            let mut invalid = workspace.config.clone();
            let WorkspaceBinding::NativeMarkdown { note_folder, .. } =
                invalid.bindings.get_mut("home").unwrap()
            else {
                panic!()
            };
            *note_folder = Some(PathBuf::from("outside-link"));
            assert!(plan_workspace_config(&workspace, invalid).is_err());
        }
    }
}
