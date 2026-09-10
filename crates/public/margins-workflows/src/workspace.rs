//! Workspace and source configuration owned by Margins.
//!
//! A workspace is the id-named memory boundary of one practice. Its config is
//! deliberately not a serialized interpretation of that practice: it records
//! only the Sources, read/write roles, retention, and attention corrections the
//! runtime must remember. Richer understanding stays grounded in the Sources
//! and in the setup conversation. Machine state lives at
//! `<MARGINS_HOME>/workspaces/<id>`; content sources remain where their owners
//! put them. A command may create the first, implicit home Source from a
//! notes-bearing working directory under a strict isolation deny-list. Every
//! Source beyond that home remains explicitly declared.

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

pub const WORKSPACES_DIR: &str = "workspaces";
pub const WORKSPACE_CONFIG: &str = "config.toml";
pub const WORKSPACE_PLAN_SCHEMA: &str = "margins.workspace.plan.v1";
pub const WORKSPACE_APPLY_SCHEMA: &str = "margins.workspace.apply.v1";
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
    #[serde(default = "default_excluded_folders")]
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
            excluded_folders: default_excluded_folders(),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WorkspacePlanAction {
    SetName {
        before: Option<String>,
        after: Option<String>,
    },
    SetPolicy {
        before: WorkspacePolicy,
        after: WorkspacePolicy,
    },
    SetRetentionPolicy {
        before: RetentionPolicy,
        after: RetentionPolicy,
    },
    AddBinding {
        name: String,
        binding: WorkspaceBinding,
    },
    UpdateBinding {
        name: String,
        before: WorkspaceBinding,
        after: WorkspaceBinding,
    },
    RemoveBinding {
        name: String,
        binding: WorkspaceBinding,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePlan {
    pub schema_version: String,
    pub workspace_id: String,
    pub base_revision: String,
    pub plan_id: String,
    pub actions: Vec<WorkspacePlanAction>,
    pub desired: WorkspaceConfig,
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
    desired: WorkspaceConfig,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspace {
    pub config: WorkspaceConfig,
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
    pub home_dir: PathBuf,
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
        self.state_dir.join("index.db")
    }

    pub fn captures_dir(&self) -> PathBuf {
        self.state_dir.join("captures")
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
    if state_dir.exists() {
        bail!("workspace '{id}' already exists at {}", state_dir.display());
    }
    std::fs::create_dir_all(state_dir.join("captures"))
        .with_context(|| format!("creating workspace state at {}", state_dir.display()))?;
    let mut bindings = BTreeMap::new();
    bindings.insert(
        "home".to_string(),
        WorkspaceBinding::NativeMarkdown {
            path: home_notes.clone(),
            role: SourceRole::Home,
        },
    );
    bindings.insert(
        "captures".to_string(),
        WorkspaceBinding::Captures {
            path: state_dir.join("captures"),
        },
    );
    let config = WorkspaceConfig {
        id: id.to_string(),
        name: name.map(str::to_string),
        policy: WorkspacePolicy::default(),
        retention: RetentionPolicy::default(),
        bindings,
    };
    write_config(&state_dir.join(WORKSPACE_CONFIG), &config)?;
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
    let state_dir = workspace_state_dir(margins_home, id)?;
    let config_path = state_dir.join(WORKSPACE_CONFIG);
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("workspace '{id}' not found at {}", config_path.display()))?;
    let config: WorkspaceConfig = toml::from_str(&raw)
        .with_context(|| format!("invalid workspace config {}", config_path.display()))?;
    if config.id != id {
        bail!(
            "workspace directory '{id}' contains config for '{}'",
            config.id
        );
    }
    validate_config(&config)?;
    let home_dir = config
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::NativeMarkdown {
                path,
                role: SourceRole::Home,
            } => Some(path.clone()),
            _ => None,
        })
        .context("workspace must declare exactly one notes source with role = 'home'")?;
    Ok(ResolvedWorkspace {
        config,
        state_dir,
        config_path,
        home_dir,
    })
}

pub fn resolve_state_dir(state_dir: &Path) -> Result<ResolvedWorkspace> {
    let id = state_dir
        .file_name()
        .and_then(|value| value.to_str())
        .context("workspace state directory has no valid id")?;
    let workspaces = state_dir
        .parent()
        .context("workspace state directory has no parent")?;
    if workspaces.file_name().and_then(|value| value.to_str()) != Some(WORKSPACES_DIR) {
        bail!("workspace state directory must live under {WORKSPACES_DIR}/<id>");
    }
    let margins_home = workspaces
        .parent()
        .context("workspace state directory has no Margins home")?;
    resolve_at(margins_home, id)
}

pub fn list_workspaces(margins_home: &Path) -> Result<Vec<ResolvedWorkspace>> {
    let root = margins_home.join(WORKSPACES_DIR);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = std::fs::read_dir(&root)
        .with_context(|| format!("reading {}", root.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    entries
        .into_iter()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .map(|id| resolve_at(margins_home, &id))
        .collect()
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

pub fn update_policy(workspace: &mut ResolvedWorkspace, policy: WorkspacePolicy) -> Result<()> {
    mutate_workspace_config(workspace, |config| {
        config.policy = policy;
        Ok(())
    })
}

pub fn workspace_revision(config: &WorkspaceConfig) -> Result<String> {
    let bytes = serde_json::to_vec(config).context("serializing canonical workspace revision")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Re-read the workspace config and reject an authoritative commit whose
/// desired-state revision changed while provider transport was in flight.
pub fn require_workspace_revision(state_dir: &Path, expected_revision: &str) -> Result<()> {
    let workspace = resolve_state_dir(state_dir)?;
    let actual = workspace_revision(&workspace.config)?;
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

pub fn plan_workspace_config(
    current: &WorkspaceConfig,
    desired: WorkspaceConfig,
) -> Result<WorkspacePlan> {
    validate_config(&desired)?;
    if desired.id != current.id {
        return Err(WorkspaceMutationError::InvalidPlan(format!(
            "desired config id '{}' does not match workspace '{}'",
            desired.id, current.id
        ))
        .into());
    }
    let mut actions = Vec::new();
    if current.name != desired.name {
        actions.push(WorkspacePlanAction::SetName {
            before: current.name.clone(),
            after: desired.name.clone(),
        });
    }
    if current.policy != desired.policy {
        actions.push(WorkspacePlanAction::SetPolicy {
            before: current.policy.clone(),
            after: desired.policy.clone(),
        });
    }
    if current.retention != desired.retention {
        actions.push(WorkspacePlanAction::SetRetentionPolicy {
            before: current.retention.clone(),
            after: desired.retention.clone(),
        });
    }
    for (name, binding) in &current.bindings {
        match desired.bindings.get(name) {
            None => actions.push(WorkspacePlanAction::RemoveBinding {
                name: name.clone(),
                binding: binding.clone(),
            }),
            Some(after) if after != binding => actions.push(WorkspacePlanAction::UpdateBinding {
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
                name: name.clone(),
                binding: binding.clone(),
            });
        }
    }
    let base_revision = workspace_revision(current)?;
    let plan_id = workspace_plan_id(&current.id, &base_revision, &actions, &desired)?;
    Ok(WorkspacePlan {
        schema_version: WORKSPACE_PLAN_SCHEMA.to_string(),
        workspace_id: current.id.clone(),
        base_revision,
        plan_id,
        actions,
        desired,
    })
}

pub fn apply_workspace_plan(
    workspace: &mut ResolvedWorkspace,
    plan: &WorkspacePlan,
) -> Result<WorkspaceApplyReceipt> {
    let request_hash = workspace_apply_request_hash(plan)?;
    let request_id = format!("workspace-apply-{request_hash}");
    let _lock = lock_workspace_ready(&workspace.state_dir)?;
    if let Some(mut receipt) = load_workspace_receipt(&workspace.state_dir, &request_id)? {
        if receipt.request_hash != request_hash {
            return Err(WorkspaceMutationError::IdempotencyConflict {
                request_id: request_id.clone(),
            }
            .into());
        }
        receipt.replayed = true;
        workspace.config = read_config(&workspace.config_path)?;
        return Ok(receipt);
    }

    let current = read_config(&workspace.config_path)?;
    let actual_revision = workspace_revision(&current)?;
    if actual_revision != plan.base_revision {
        return Err(WorkspaceMutationError::RevisionConflict {
            expected: plan.base_revision.clone(),
            actual: actual_revision,
        }
        .into());
    }
    let rebuilt = plan_workspace_config(&current, plan.desired.clone())?;
    if rebuilt != *plan {
        return Err(WorkspaceMutationError::InvalidPlan(
            "workspace plan content or plan_id is invalid".to_string(),
        )
        .into());
    }

    let after_revision = workspace_revision(&plan.desired)?;
    let receipt = WorkspaceApplyReceipt {
        schema_version: WORKSPACE_APPLY_SCHEMA.to_string(),
        ok: true,
        workspace_id: current.id,
        request_id,
        request_hash,
        plan_id: plan.plan_id.clone(),
        before_revision: plan.base_revision.clone(),
        after_revision,
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
            desired: plan.desired.clone(),
        },
    )?;
    write_config(&workspace.config_path, &plan.desired)?;
    store_workspace_receipt(&workspace.state_dir, &receipt)?;
    clear_workspace_transaction(&workspace.state_dir)?;
    workspace.config = plan.desired.clone();
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
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn validate_binding(binding: &WorkspaceBinding) -> Result<()> {
    match binding {
        WorkspaceBinding::NativeMarkdown { path, role: _ } => {
            if !path.is_absolute() {
                bail!("source paths must be absolute: {}", path.display());
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
    let _lock = lock_workspace_ready(&workspace.state_dir)?;
    let current = read_config(&workspace.config_path)?;
    let mut desired = current.clone();
    let value = mutate(&mut desired)?;
    let plan = plan_workspace_config(&current, desired)?;
    write_config(&workspace.config_path, &plan.desired)?;
    workspace.config = plan.desired;
    Ok(value)
}

fn workspace_plan_id(
    workspace_id: &str,
    base_revision: &str,
    actions: &[WorkspacePlanAction],
    desired: &WorkspaceConfig,
) -> Result<String> {
    #[derive(Serialize)]
    struct Identity<'a> {
        workspace_id: &'a str,
        base_revision: &'a str,
        actions: &'a [WorkspacePlanAction],
        desired: &'a WorkspaceConfig,
    }
    let bytes = serde_json::to_vec(&Identity {
        workspace_id,
        base_revision,
        actions,
        desired,
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

fn read_config(path: &Path) -> Result<WorkspaceConfig> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading workspace config {}", path.display()))?;
    let config: WorkspaceConfig = toml::from_str(&body)
        .with_context(|| format!("invalid workspace config {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
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
    validate_config(&transaction.desired)?;
    let expected_after = workspace_revision(&transaction.desired)?;
    if expected_after != transaction.receipt.after_revision {
        bail!("pending workspace transaction has an invalid desired revision");
    }
    let config_path = state_dir.join(WORKSPACE_CONFIG);
    let current = read_config(&config_path)?;
    let current_revision = workspace_revision(&current)?;
    if current_revision == transaction.receipt.before_revision {
        write_config(&config_path, &transaction.desired)?;
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

fn write_config(path: &Path, config: &WorkspaceConfig) -> Result<()> {
    let body = toml::to_string_pretty(config).context("serializing workspace config")?;
    atomic_write(path, body.as_bytes())
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
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

fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("syncing parent directory {}", parent.display()))?;
    Ok(())
}

fn default_excluded_folders() -> Vec<String> {
    vec![".git".to_string(), "node_modules".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

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
            margins_home.join("workspaces/client-notes/config.toml")
        );
        let raw = std::fs::read_to_string(&first.workspace.config_path).unwrap();
        assert!(raw.contains(&format!("path = {:?}", notes.canonicalize().unwrap())));

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
        let config_toml = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(config_toml.contains("query = \"-in:spam -in:trash\""));
        assert!(config_toml.contains("backfill_days = 365"));
        assert!(config_toml.contains("lookback_days = 365"));
        assert!(config_toml.contains("lookahead_days = 180"));
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
            },
        );
        assert!(overlap.unwrap_err().to_string().contains("overlap"));
    }

    #[test]
    fn config_roundtrip_uses_bindings_not_sources() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let raw = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(raw.contains("[bindings.home]"));
        assert!(raw.contains("kind = \"notes\""));
        assert!(raw.contains("[bindings.captures]"));
        assert!(!raw.contains("[sources."));

        let reparsed: WorkspaceConfig = toml::from_str(&raw).unwrap();
        assert_eq!(reparsed, workspace.config);
    }

    #[test]
    fn workspace_entity_curation_matches_enzyme_shape() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut config = create_workspace(&margins_home, "practice", None, &notes)
            .unwrap()
            .config;
        config.policy.entities = vec![
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

        let raw = toml::to_string_pretty(&config).unwrap();
        assert!(raw.contains("entities = ["), "{raw}");
        assert!(raw.contains("profile = \"relational\""), "{raw}");
        assert!(raw.contains("expandable = true"), "{raw}");
        assert_eq!(toml::from_str::<WorkspaceConfig>(&raw).unwrap(), config);
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
            },
        );
        assert!(failed_add.unwrap_err().to_string().contains("overlap"));
        assert!(!workspace.config.bindings.contains_key("ref"));
        let on_disk = std::fs::read_to_string(&workspace.config_path).unwrap();
        assert!(on_disk.contains("[bindings.mail]"));
        assert!(!on_disk.contains("[bindings.ref]"));

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

    #[test]
    fn workspace_plan_apply_derives_replay_identity_and_rejects_stale_plans() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let mut desired = workspace.config.clone();
        desired.name = Some("Client practice".to_string());
        desired.bindings.insert(
            "mail".to_string(),
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        );

        let plan = plan_workspace_config(&workspace.config, desired).unwrap();
        assert_eq!(plan.schema_version, WORKSPACE_PLAN_SCHEMA);
        assert_eq!(plan.actions.len(), 2);
        let receipt = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(!receipt.replayed);
        assert_eq!(receipt.schema_version, WORKSPACE_APPLY_SCHEMA);
        assert_eq!(receipt.request_hash.len(), 64);
        assert_eq!(
            receipt.request_id,
            format!("workspace-apply-{}", receipt.request_hash)
        );
        assert_eq!(workspace.config.name.as_deref(), Some("Client practice"));

        let replay = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.after_revision, receipt.after_revision);

        let mut changed = workspace.config.clone();
        changed.name = Some("Different plan".to_string());
        let changed_plan = plan_workspace_config(&workspace.config, changed).unwrap();
        let changed_receipt = apply_workspace_plan(&mut workspace, &changed_plan).unwrap();
        assert_ne!(changed_receipt.request_id, receipt.request_id);

        let stale_plan = plan_workspace_config(&workspace.config, {
            let mut desired = workspace.config.clone();
            desired.name = Some("Stale plan".to_string());
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
    fn workspace_apply_recovers_a_durable_prepared_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        let mut desired = workspace.config.clone();
        desired.name = Some("Recovered transaction".to_string());
        let plan = plan_workspace_config(&workspace.config, desired.clone()).unwrap();
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
            after_revision: workspace_revision(&desired).unwrap(),
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
            &WorkspaceTransaction { receipt, desired },
        )
        .unwrap();

        let recovered = apply_workspace_plan(&mut workspace, &plan).unwrap();
        assert!(recovered.replayed);
        assert_eq!(
            workspace.config.name.as_deref(),
            Some("Recovered transaction")
        );
        assert!(!workspace_transaction_path(&workspace.state_dir).exists());
        assert!(workspace_receipt_path(&workspace.state_dir, &request_id).is_file());
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
}
