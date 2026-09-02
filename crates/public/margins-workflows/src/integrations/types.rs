//! Serializable integration contract types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const RETENTION_PREVIEW_SCHEMA: &str = "margins.retention.preview.v1";
pub const RETENTION_APPLY_SCHEMA: &str = "margins.retention.apply.v1";

/// Closed Margins ledger target. Connector validation and account
/// normalization happen before a preview is produced; arbitrary tables,
/// provider instructions, and filesystem paths are never accepted as targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionTarget {
    pub connector_id: String,
    pub source_account: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionScope {
    Expired,
    RawCache,
    Tombstones,
    Materialization,
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCutoffs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_cache_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_before: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCounts {
    pub raw_items: u64,
    pub active_evidence: u64,
    pub tombstoned_evidence: u64,
    pub associations: u64,
    pub connector_state: u64,
    pub curation_observations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPreview {
    pub schema_version: String,
    pub workspace_id: String,
    pub revision: String,
    pub plan_id: String,
    pub target: RetentionTarget,
    pub scope: RetentionScope,
    pub cutoffs: RetentionCutoffs,
    pub counts: RetentionCounts,
    pub ledger_fingerprint: String,
    pub destructive: bool,
    pub index_refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionApplyReceipt {
    pub schema_version: String,
    pub ok: bool,
    pub workspace_id: String,
    pub revision: String,
    pub request_id: String,
    pub request_hash: String,
    pub plan_id: String,
    pub target: RetentionTarget,
    pub scope: RetentionScope,
    pub deleted: RetentionCounts,
    pub replayed: bool,
    pub index_refresh_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetentionMutationError {
    InvalidPlan(String),
    PlanStale {
        expected_fingerprint: String,
        actual_fingerprint: String,
    },
}

impl std::fmt::Display for RetentionMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(detail) => write!(formatter, "invalid retention plan: {detail}"),
            Self::PlanStale {
                expected_fingerprint,
                actual_fingerprint,
            } => write!(
                formatter,
                "retention plan is stale: expected ledger fingerprint {expected_fingerprint}, actual {actual_fingerprint}"
            ),
        }
    }
}

impl std::error::Error for RetentionMutationError {}

/// Closed, non-executable locator for authoritative evidence surfaced by
/// Margins. Only native Markdown is represented by a filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceHandle {
    NativeMarkdown {
        path: String,
    },
    ExternalRecord {
        connector_id: String,
        source_account: String,
        source_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        href: Option<String>,
    },
    SessionRecord {
        session_id: String,
    },
}

/// Machine-readable freshness state shared by retrieval and context surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessStatus {
    Fresh,
    Stale,
    Error,
    NeedsAuth,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceFreshness {
    pub status: FreshnessStatus,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_refresh: Option<DateTime<Utc>>,
}

/// Connector execution context passed to reconcile and health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCtx {
    /// Margins-owned workspace directory containing ledger/index/capture state.
    pub vault_root: PathBuf,
    pub connector_id: String,
    pub account: String,
    /// Optional connector executable selected by setup. Connectors which wrap
    /// an external CLI must not discover a different binary during reconcile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_path: Option<PathBuf>,
}

/// Raw identity evidence for a person before or after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonIdentity {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// The connector found conflicting or insufficient identity evidence.
    /// Ambiguous people remain evidence-only and never qualify for backlinks
    /// or resolved identity links.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ambiguous: bool,
    /// Wikilink target when vault resolution passed. Absence alone does not
    /// imply ambiguity because structured name/email evidence can be unique
    /// before a corresponding person note exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wikilink: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Counts grouped by source-native record kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindCounts {
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

impl KindCounts {
    pub fn increment_named(&mut self, kind: &str, by: u64) {
        *self.counts.entry(kind.to_string()).or_default() += by;
    }
}

/// Inferred entity with supporting evidence strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredEntity {
    pub label: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub ambiguous: bool,
}

/// A closed time range observed by an integration connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurveyRange {
    pub occurred_from: DateTime<Utc>,
    pub occurred_to: DateTime<Utc>,
}

/// Observable connector evidence supplied to Margins KnowledgePolicy.
///
/// This record never authorizes materialization or narrows collection
/// membership. Gmail currently produces it while reconciling the complete
/// workspace-declared collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurationObservation {
    pub connector_id: String,
    pub account: String,
    pub item_counts: KindCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_to: Option<DateTime<Utc>>,
    /// The range the connector asked the upstream service to observe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_window: Option<SurveyRange>,
    /// The range actually represented by the returned source items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_range: Option<SurveyRange>,
    /// Additive ranges for connectors that combine multiple survey samples.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sample_observed_ranges: BTreeMap<String, SurveyRange>,
    /// Exact upstream command invocations used to produce the observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_call_count: Option<u64>,
    #[serde(default)]
    pub detected_accounts: Vec<String>,
    #[serde(default)]
    pub inferred_people: Vec<InferredEntity>,
    #[serde(default)]
    pub inferred_orgs: Vec<InferredEntity>,
    #[serde(default)]
    pub proposed_include: Vec<String>,
    #[serde(default)]
    pub proposed_exclude: Vec<String>,
    /// Additive machine-readable flags keyed by an exact proposed include.
    /// Email surveys use `likely-automated` so callers can rank or filter
    /// noisy mailbox candidates without guessing from display text.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub proposal_flags: BTreeMap<String, Vec<String>>,
    /// Additive structured evidence keyed by an exact proposed include.
    /// Connectors populate this from observations already collected by the
    /// survey so consumers can apply their own thresholds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub proposal_evidence: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub decisions_required: Vec<String>,
}

/// Per-reconcile manifest stored in `runs` and returned to callers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    pub records_written: u64,
    pub records_updated: u64,
    pub records_unchanged: u64,
    pub tombstones: u64,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Authoritative catalyst/index record for one materialized email thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEvidence {
    pub thread_id: String,
    pub occurred_from: DateTime<Utc>,
    pub occurred_to: DateTime<Utc>,
    pub body_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Authoritative normalized Google Calendar event materialized by Margins.
/// This is provider evidence, not projected Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventEvidence {
    pub source_id: String,
    pub calendar_id: String,
    pub occurred_from: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_to: Option<DateTime<Utc>>,
    pub title: String,
    pub body_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Rebuildable, role-blind document association for one Calendar attendee.
/// Provider-observable fields remain available to Margins but are not engine
/// roles and never decide corpus membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventAttendee {
    pub source_id: String,
    pub attendee_key: String,
    pub position: u32,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_status: Option<String>,
    #[serde(default)]
    pub is_self: bool,
    #[serde(default)]
    pub organizer: bool,
}

/// Authoritative normalized document supplied by an external document-like
/// collection. Google Meet transcripts and Granola meetings are the first two
/// concrete consumers. `attributes` contains normalized source-specific fields
/// used by Margins hydration/projection; it is never passed to Enzyme as a
/// provider payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalDocumentEvidence {
    pub source_id: String,
    pub occurred_at: DateTime<Utc>,
    pub title: String,
    pub body_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default)]
    pub attributes: serde_json::Value,
}

/// Rebuildable, role-blind participant association for one authoritative
/// external document. Position is source order, not speaker ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalDocumentParticipant {
    pub source_id: String,
    pub participant_key: String,
    pub position: u32,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub ambiguous: bool,
}

/// Role-blind association between a normalized external participant and a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantThread {
    pub participant: String,
    pub thread_id: String,
    pub last_interaction: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_score: Option<u64>,
}

/// Counts returned by atomic email thread snapshot replacement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailThreadSnapshotCounts {
    pub threads_written: u64,
    pub threads_updated: u64,
    pub threads_unchanged: u64,
    pub threads_deleted: u64,
}

/// Aggregate participant ranking row used for curated entity selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedParticipant {
    pub participant: String,
    /// Aggregate activity importance with each component capped before packing.
    /// Ordering uses the uncapped reply/sent/thread tuple separately.
    pub aggregate_sampling_score: u64,
    pub last_interaction: DateTime<Utc>,
}

/// Connector output after reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileResult {
    pub records_written: u64,
    pub records_updated: u64,
    pub records_unchanged: u64,
    pub tombstones: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<serde_json::Value>,
    pub manifest: RunManifest,
}

/// Health status for connector status surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Fresh,
    Stale,
    #[serde(rename = "needs-auth")]
    NeedsAuth,
    Error,
}

impl HealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::NeedsAuth => "needs-auth",
            Self::Error => "error",
        }
    }
}

/// Cheap freshness report; failures must not invalidate the local index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_sync: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_age_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A fetched source-native record retained in the optional raw transport cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawItemDraft {
    pub source_id: String,
    pub payload: serde_json::Value,
}

/// Calendar-specific reconciliation input. A complete snapshot may tombstone
/// active in-boundary records absent from `events`; incrementals only apply
/// explicit cancellations and changed records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventDelta {
    #[serde(default)]
    pub events: Vec<CalendarEventEvidence>,
    #[serde(default)]
    pub attendees: Vec<CalendarEventAttendee>,
    #[serde(default)]
    pub tombstone_source_ids: Vec<String>,
    #[serde(default)]
    pub raw_items: Vec<RawItemDraft>,
    pub scope: SurveyRange,
    pub complete_snapshot: bool,
    pub materialization_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<serde_json::Value>,
}

/// Reconciliation input shared by the concrete Google Meet and Granola
/// external-document materializers. A complete snapshot tombstones absent
/// active records globally or, when `snapshot_scope` is present, only within
/// that observed time range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalDocumentDelta {
    #[serde(default)]
    pub documents: Vec<ExternalDocumentEvidence>,
    #[serde(default)]
    pub participants: Vec<ExternalDocumentParticipant>,
    #[serde(default)]
    pub tombstone_source_ids: Vec<String>,
    #[serde(default)]
    pub raw_items: Vec<RawItemDraft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_scope: Option<SurveyRange>,
    pub complete_snapshot: bool,
    pub materialization_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<serde_json::Value>,
}
