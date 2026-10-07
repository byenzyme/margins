//! Associative vault search backing `margins recall`.
//!
//! Recall runs the shipped `enzyme` CLI ([`crate::enzyme_cli`]) against the
//! Workspace's index at `$MARGINS_HOME/workspaces/<id>/enzyme.db`. Querying
//! only opens a pre-built index (`enzyme search --json`, `status --json`);
//! index establishment and catalyst generation belong to `margins init`
//! (`enzyme init`) and to the refresh after a sync (`enzyme refresh`).
//! Querying never resolves credentials, provisions models, generates, or uses
//! network.
//!
//! The primary lookup follows thematic bridges generated at index time, so a
//! query can surface notes that share a theme without sharing words.
//!
//! Interactive output mirrors the `enzyme catalyze` tree. Piped output is the
//! Margins-owned typed result envelope: Enzyme's canonical catalyst text passes
//! through, while Margins resolves evidence identity and freshness.
//!
//! The user- and skill-visible command remains `margins recall "query"`; its
//! result contract intentionally distinguishes opaque document references from
//! native Markdown paths and external records.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::enzyme_cli::{Engine, EngineError, Generator, SearchHit, StatusEnvelope, TopCatalyst};
use crate::workspace_recall::CatalogEntry;
use margins_workflows::integrations::{
    EvidenceFreshness, EvidenceHandle, FreshnessStatus, GoogleCalendarScope,
    GOOGLE_MEET_MATERIALIZATION_FINGERPRINT,
};
use margins_workflows::source_kinds::{ledger_record_id, sqlite_document_ref_prefix};
use margins_workflows::workspace::{ResolvedWorkspace, SourceKind, WorkspaceBinding};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

/// A vault with fewer than this many notes is not worth indexing: recall would
/// have too little material to draw real associations from, so we return thin
/// results without paying to build an index.
#[cfg(test)]
const USEFULNESS_THRESHOLD: usize = 5;

/// How many related passages to return. A fixed, sensible default keeps the
/// command flag-free.
const RESULT_LIMIT: usize = 8;

/// The engine for this Margins home, with the home ready for it.
fn engine() -> Result<Engine> {
    Engine::for_home(&margins_home()?)
}

/// Engine status of a Workspace whose index exists; `None` when it was never
/// built, without running the engine.
fn indexed_status(workspace: &ResolvedWorkspace) -> Result<Option<(Engine, StatusEnvelope)>> {
    indexed_status_with(workspace, engine)
}

/// [`indexed_status`] for status reporting: the engine is only queried, and
/// the Margins home is not prepared or migrated (see
/// [`Engine::for_inspection`]).
fn inspected_status(workspace: &ResolvedWorkspace) -> Result<Option<StatusEnvelope>> {
    Ok(indexed_status_with(workspace, || Engine::for_inspection(&margins_home()?))?
        .map(|(_, status)| status))
}

fn indexed_status_with(
    workspace: &ResolvedWorkspace,
    engine: impl FnOnce() -> Result<Engine>,
) -> Result<Option<(Engine, StatusEnvelope)>> {
    if !workspace.recall_path().is_file() {
        return Ok(None);
    }
    let engine = engine()?;
    let status = engine
        .status(&workspace.config.id)
        .context("reading the Workspace index status")?;
    Ok(status.initialized.then_some((engine, status)))
}

pub fn workspace_source_refresh_staleness(
    workspace: &ResolvedWorkspace,
) -> Result<BTreeMap<String, margins_cli::commands::workspace::SourceRefreshStalenessView>> {
    let status = inspected_status(workspace)?;
    source_refresh_staleness(workspace, status.as_ref())
}

/// Per ledger source: the engine's refresh state of its SQLite source, made
/// stale again when Margins' own materialization receipt says the ledger no
/// longer matches the declaration.
fn source_refresh_staleness(
    workspace: &ResolvedWorkspace,
    status: Option<&StatusEnvelope>,
) -> Result<BTreeMap<String, margins_cli::commands::workspace::SourceRefreshStalenessView>> {
    use margins_cli::commands::workspace::SourceRefreshStalenessView;

    let ledger = workspace
        .ledger_path()
        .is_file()
        .then(|| {
            Connection::open_with_flags(workspace.ledger_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        })
        .transpose()?;
    workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| {
            let receipt = match binding {
                WorkspaceBinding::Gmail { account, gmail } => (
                    "email",
                    account.as_str(),
                    gmail.materialization_fingerprint(),
                    Ok(None),
                ),
                WorkspaceBinding::GoogleCalendar { account, calendar } => (
                    "gcal",
                    account.as_str(),
                    calendar.materialization_fingerprint(),
                    GoogleCalendarScope::for_selector(calendar, Utc::now()).and_then(|scope| {
                        serde_json::to_string(&scope.as_range())
                            .context("failed to fingerprint Calendar rolling boundary")
                            .map(Some)
                    }),
                ),
                WorkspaceBinding::GoogleMeet { account } => (
                    "google_meet",
                    account.as_str(),
                    Ok(GOOGLE_MEET_MATERIALIZATION_FINGERPRINT.to_string()),
                    Ok(None),
                ),
                WorkspaceBinding::Granola {
                    account,
                    collection,
                } => (
                    "granola",
                    account.as_str(),
                    collection.materialization_fingerprint(),
                    Ok(None),
                ),
                _ => return None,
            };
            Some((name, receipt))
        })
        .map(|(name, (connector, account, fingerprint, scope))| {
            let expected_fingerprint = fingerprint?;
            let expected_scope = scope?;
            let source = status.and_then(|status| {
                status
                    .sources
                    .iter()
                    .find(|source| source.name == *name)
            });
            let (last_refresh_ms, mut stale, mut stale_reason) = match source {
                Some(source) => (
                    source.last_refresh_ms,
                    source.stale,
                    source.stale_reason.clone(),
                ),
                None => (None, true, Some("never_refreshed".to_string())),
            };
            let materialization = ledger
                .as_ref()
                .map(|ledger| {
                    ledger
                        .query_row(
                            "SELECT materialization_fingerprint, health_status, scope_boundary_json
                             FROM connectors WHERE connector_id = ?1 AND account = ?2",
                            [connector, account],
                            |row| {
                                Ok((
                                    row.get::<_, Option<String>>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, Option<String>>(2)?,
                                ))
                            },
                        )
                        .optional()
                })
                .transpose()?
                .flatten();
            match materialization {
                None => {
                    stale = true;
                    stale_reason = Some("never_refreshed".to_string());
                }
                Some((_, status, _)) if status == "error" => {
                    stale = true;
                    stale_reason = Some("refresh_failed".to_string());
                }
                Some((_, status, _)) if status == "needs-auth" => {
                    stale = true;
                    stale_reason = Some("credentials_unavailable".to_string());
                }
                Some((stored_fingerprint, status, stored_scope))
                    if status == "stale"
                        || status == "unknown"
                        || stored_fingerprint.as_deref() != Some(expected_fingerprint.as_str())
                        || expected_scope.as_ref().is_some_and(|expected| {
                            stored_scope.as_deref() != Some(expected.as_str())
                        }) =>
                {
                    stale = true;
                    stale_reason = Some("refresh_required".to_string());
                }
                Some((_, status, _)) if status == "fresh" => {}
                Some((_, status, _)) => {
                    anyhow::bail!("unknown connector health status: {status}")
                }
            }
            Ok((
                name.clone(),
                SourceRefreshStalenessView {
                    last_refresh_ms,
                    stale,
                    stale_reason,
                },
            ))
        })
        .collect()
}

pub fn workspace_status_recall(
    workspace: &ResolvedWorkspace,
) -> Result<margins_workflows::local_recall::LocalRecallStatus> {
    let Some(status) = inspected_status(workspace)? else {
        return margins_workflows::local_recall::status(workspace);
    };
    if status.needs_rebuild() {
        anyhow::bail!(
            "the recall index at {} was built by an older engine; run `margins init`",
            workspace.recall_path().display()
        );
    }
    Ok(margins_workflows::local_recall::LocalRecallStatus {
        schema_version: "margins.indexed-recall.v1".to_string(),
        available: true,
        mode: "indexed".to_string(),
        documents: status.documents,
    })
}

pub const RECALL_UNAVAILABLE_MESSAGE: &str =
    "Recall unavailable: no usable generator is configured. Run `margins setup`.";

pub struct RecallOutput {
    pub query: String,
    /// Internal availability state: `ok` — searched via thematic bridges;
    /// `thin` — below threshold;
    /// `off` — opted out; `no_policy` — missing explicit Margins policy;
    /// `no_vault` — not a vault; `unavailable` — cannot search as-is.
    pub status: &'static str,
    /// Fine-grained machine reason within a status. Examples: `catalyst`,
    /// `no_entities`, `not_established`, `missing_policy`.
    pub reason: &'static str,
    pub freshness: RecallFreshness,
    /// Exact Margins-owned machine config path used for this workspace.
    pub config_path: Option<PathBuf>,
    /// Documents recall indexed and searched. Internal (not in stdout JSON).
    pub note_count: usize,
    pub results: Vec<RecallResult>,
    pub top_contributing_catalysts: Vec<TopCatalyst>,
    pub search_strategy: &'static str,
    pub processing_time: f64,
}

#[derive(Serialize)]
pub struct RecallResult {
    pub document_ref: String,
    pub content: String,
    pub similarity: f64,
    pub source: String,
    pub source_kind: SourceKind,
    pub evidence: EvidenceHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_catalyst_id: Option<String>,
    /// Full canonical catalyst chunk as stored by the engine. The versioned
    /// header and inline receipts intentionally pass through without Margins
    /// parsing or re-rendering them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_catalyst_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceMaterializationFreshness {
    pub source: String,
    #[serde(flatten)]
    pub freshness: EvidenceFreshness,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallFreshness {
    pub status: FreshnessStatus,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub index: EvidenceFreshness,
    pub materialization: Vec<SourceMaterializationFreshness>,
}

struct RecallHit {
    path: String,
    score: f64,
    content: String,
    via_catalyst_id: Option<String>,
    via_catalyst_text: Option<String>,
}

/// Run associative search against a pre-built index. Missing or stale indexes
/// are reported honestly and never repaired from the query path.
pub fn recall(
    workspace: &ResolvedWorkspace,
    query: &str,
    source_filter: Option<&str>,
) -> Result<RecallOutput> {
    let config_path = Some(workspace.config_path.clone());
    let materialization = materialization_freshness(workspace)?;
    let empty = |status: &'static str, reason: &'static str, note_count| RecallOutput {
        query: query.to_string(),
        status,
        reason,
        freshness: combined_freshness(
            EvidenceFreshness {
                status: FreshnessStatus::NotApplicable,
                stale: false,
                reason: None,
                last_successful_refresh: None,
            },
            materialization.clone(),
        ),
        config_path: config_path.clone(),
        note_count,
        results: Vec::new(),
        top_contributing_catalysts: Vec::new(),
        search_strategy: "catalyze",
        processing_time: 0.0,
    };

    if let Some(source) = source_filter {
        if !workspace.config.bindings.contains_key(source) {
            anyhow::bail!(
                "unknown source '{source}' in workspace '{}'",
                workspace.config.id
            );
        }
    }
    // Opt-out: skips the LLM catalyst layer and all spend.
    if recall_disabled() {
        return Ok(empty("off", "opt_out", 0));
    }

    let Some((engine, status)) = indexed_status(workspace)? else {
        return Ok(empty("unavailable", "not_established", 0));
    };
    if status.needs_rebuild() {
        anyhow::bail!(
            "recall_unavailable_index_outdated: the recall index was built by an older engine; run `margins init`"
        );
    }
    let freshness = require_retrieval_freshness(
        snapshot_index_freshness(workspace, &status)?,
        materialization,
    )?;

    let started = std::time::Instant::now();
    let (status_word, reason, search_strategy, hits, top_contributing_catalysts) =
        search_index(workspace, &engine, &status, query, source_filter)?;
    let results = hits
        .into_iter()
        .map(|hit| {
            let entry = catalog_entry_for_document_ref(workspace, &hit.path)?;
            Ok(RecallResult {
                document_ref: hit.path,
                similarity: hit.score,
                content: hit.content,
                source: entry.source,
                source_kind: entry.kind,
                evidence: entry.evidence,
                via_catalyst_id: hit.via_catalyst_id,
                via_catalyst_text: hit.via_catalyst_text,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(RecallOutput {
        query: query.to_string(),
        status: status_word,
        reason,
        freshness,
        config_path: Some(workspace.config_path.clone()),
        // The exact number of indexed documents recall just searched.
        note_count: status.documents,
        results,
        top_contributing_catalysts,
        search_strategy,
        processing_time: (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
    })
}

fn recall_hit(hit: SearchHit) -> RecallHit {
    RecallHit {
        path: hit.path,
        score: hit.score,
        content: hit.content,
        via_catalyst_id: hit.via_catalyst_id,
        via_catalyst_text: hit.via_catalyst_text,
    }
}

/// Search primarily through generated thematic bridges, while preserving a
/// bounded literal phrase path across the full declared document boundary.
fn search_index(
    workspace: &ResolvedWorkspace,
    engine: &Engine,
    status: &StatusEnvelope,
    query: &str,
    source_filter: Option<&str>,
) -> Result<(
    &'static str,
    &'static str,
    &'static str,
    Vec<RecallHit>,
    Vec<TopCatalyst>,
)> {
    let id = &workspace.config.id;
    let search_limit = source_filter
        .map(|_| status.documents.max(RESULT_LIMIT))
        .unwrap_or(RESULT_LIMIT);
    let phrase = is_distinctive_phrase(query);
    // A non-distinctive query gets no literal lookup: the engine's exact
    // search would otherwise match on the query itself.
    let response = engine.search(id, query, Some(if phrase { query } else { "" }), search_limit)?;
    let exact_hits = response
        .exact_hits
        .iter()
        .cloned()
        .map(recall_hit)
        .filter(|hit| native_markdown_hit(workspace, &hit.path))
        .collect::<Vec<_>>();
    if response.bridged == "ready" {
        let catalyst_hits = response
            .catalyst_hits
            .into_iter()
            .map(recall_hit)
            .collect::<Vec<_>>();
        let mut exact_hits = filter_recall_hits(workspace, exact_hits, source_filter);
        let catalyst_hits = filter_recall_hits(workspace, catalyst_hits, source_filter);
        // An exact hit that also has catalyst provenance below the result
        // cut keeps that provenance rather than surfacing as a bare literal.
        let missing = exact_hits
            .iter()
            .filter(|exact| !catalyst_hits.iter().any(|hit| hit.path == exact.path))
            .count();
        if missing > 0 && response.document_count > search_limit {
            let deep = engine.search(id, query, Some(""), response.document_count)?;
            for exact in &mut exact_hits {
                if catalyst_hits.iter().any(|hit| hit.path == exact.path) {
                    continue;
                }
                if let Some(hit) = deep.catalyst_hits.iter().find(|hit| hit.path == exact.path) {
                    exact.score = hit.score;
                    exact.content = hit.content.clone();
                    exact.via_catalyst_id = hit.via_catalyst_id.clone();
                    exact.via_catalyst_text = hit.via_catalyst_text.clone();
                }
            }
        }
        let hits = merge_exact_and_catalyst_hits(exact_hits, catalyst_hits);
        debug_strategy("catalyst");
        return Ok(("ok", "catalyst", "catalyze", hits, response.top_catalysts));
    }
    // An index without catalysts serves direct search only when no selected
    // entity could have catalysts (none selected, or all too thin);
    // otherwise catalysts are missing and recall fails closed.
    // Without a generator set up, the index serves direct and exact matches
    // and says so (`catalysts_not_set_up`). With one, missing catalysts mean
    // a build is incomplete, and recall fails closed until it finishes.
    let generator_ready = ensure_usable_generator().is_ok();
    let awaiting = status
        .selection
        .as_ref()
        .is_some_and(|selection| selection.entities.iter().any(awaits_catalysts));
    if generator_ready && awaiting {
        anyhow::bail!(
            "Recall is waiting for catalysts that are still being built for this Workspace; run `margins sync` to finish them."
        );
    }
    let reason = if generator_ready {
        "no_entities"
    } else {
        "catalysts_not_set_up"
    };
    let direct_hits = response
        .direct_hits
        .into_iter()
        .map(recall_hit)
        .collect::<Vec<_>>();
    let hits = merge_exact_and_catalyst_hits(
        filter_recall_hits(workspace, exact_hits, source_filter),
        filter_recall_hits(workspace, direct_hits, source_filter),
    );
    Ok(("ok", reason, "direct", hits, Vec::new()))
}

fn is_distinctive_phrase(query: &str) -> bool {
    let query = query.trim();
    query.len() >= 12 && query.split_whitespace().count() >= 3
}

fn native_markdown_hit(workspace: &ResolvedWorkspace, document_ref: &str) -> bool {
    source_name_for_document_ref(workspace, document_ref).is_some_and(|source| {
        matches!(
            workspace.config.bindings.get(&source),
            Some(WorkspaceBinding::NativeMarkdown { .. })
        )
    })
}

fn merge_exact_and_catalyst_hits(
    exact_hits: Vec<RecallHit>,
    mut catalyst_hits: Vec<RecallHit>,
) -> Vec<RecallHit> {
    let mut exact_paths = BTreeSet::new();
    let mut merged = Vec::new();
    for exact in exact_hits {
        if !exact_paths.insert(exact.path.clone()) {
            continue;
        }
        if let Some(index) = catalyst_hits
            .iter()
            .position(|candidate| candidate.path == exact.path)
        {
            merged.push(catalyst_hits.remove(index));
        } else {
            merged.push(exact);
        }
    }
    merged.extend(
        catalyst_hits
            .into_iter()
            .filter(|hit| !exact_paths.contains(&hit.path)),
    );
    merged.truncate(RESULT_LIMIT);
    merged
}

fn filter_recall_hits(
    workspace: &ResolvedWorkspace,
    hits: Vec<RecallHit>,
    source_filter: Option<&str>,
) -> Vec<RecallHit> {
    hits.into_iter()
        .filter(|hit| {
            source_filter.is_none_or(|source| {
                source_name_for_document_ref(workspace, &hit.path).as_deref() == Some(source)
            })
        })
        .take(RESULT_LIMIT)
        .collect()
}

fn catalog_entry_for_document_ref(
    workspace: &ResolvedWorkspace,
    document_ref: &str,
) -> Result<CatalogEntry> {
    if let Some((source, path)) = crate::workspace_recall::markdown_document(workspace, document_ref)
    {
        return Ok(CatalogEntry {
            source,
            kind: SourceKind::Notes,
            evidence: EvidenceHandle::NativeMarkdown {
                path: path.to_string_lossy().into_owned(),
            },
        });
    }
    for (name, binding) in &workspace.config.bindings {
        let (connector, account) = match binding {
            WorkspaceBinding::Gmail { account, .. } => ("email", account),
            WorkspaceBinding::GoogleCalendar { account, .. } => ("gcal", account),
            WorkspaceBinding::GoogleMeet { account } => ("google_meet", account),
            WorkspaceBinding::Granola { account, .. } => ("granola", account),
            WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. } => continue,
        };
        if document_ref_source_matches(document_ref, name) {
            let source_id = ledger_record_id(name, document_ref);
            return Ok(CatalogEntry {
                source: name.clone(),
                kind: binding.kind(),
                evidence: external_evidence_for_hit(
                    workspace,
                    connector,
                    account,
                    source_id.as_deref(),
                )?,
            });
        }
    }
    Ok(CatalogEntry {
        source: "unknown".to_string(),
        kind: SourceKind::Notes,
        evidence: EvidenceHandle::NativeMarkdown {
            path: document_ref.to_string(),
        },
    })
}

fn source_name_for_document_ref(
    workspace: &ResolvedWorkspace,
    document_ref: &str,
) -> Option<String> {
    if let Some((source, _)) = crate::workspace_recall::markdown_document(workspace, document_ref) {
        return Some(source);
    }
    workspace
        .config
        .bindings
        .iter()
        .find(|(name, binding)| {
            !matches!(
                binding,
                WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. }
            ) && document_ref_source_matches(document_ref, name)
        })
        .map(|(name, _)| name.clone())
}

fn document_ref_source_matches(document_ref: &str, source_name: &str) -> bool {
    document_ref.starts_with(&sqlite_document_ref_prefix(source_name))
}


fn external_evidence_for_hit(
    workspace: &ResolvedWorkspace,
    connector_id: &str,
    source_account: &str,
    source_id: Option<&str>,
) -> Result<EvidenceHandle> {
    let source_id = source_id.unwrap_or_default();
    let href = if source_id.is_empty() {
        None
    } else {
        lookup_external_href(workspace, connector_id, source_account, source_id)?
    };
    Ok(EvidenceHandle::ExternalRecord {
        connector_id: connector_id.to_string(),
        source_account: source_account.to_string(),
        source_id: source_id.to_string(),
        href,
    })
}

fn lookup_external_href(
    workspace: &ResolvedWorkspace,
    connector_id: &str,
    source_account: &str,
    source_id: &str,
) -> Result<Option<String>> {
    if !workspace.ledger_path().is_file() {
        return Ok(None);
    }
    let connection =
        Connection::open_with_flags(workspace.ledger_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let table = match connector_id {
        "email" => "thread_evidence",
        "gcal" => "calendar_event_evidence",
        "google_meet" | "granola" => "external_document_evidence",
        _ => return Ok(None),
    };
    let id_column = if connector_id == "email" {
        "thread_id"
    } else {
        "source_id"
    };
    let sql = format!(
        "SELECT href FROM {table}
         WHERE connector_id = ?1 AND source_account = ?2 AND {id_column} = ?3
         LIMIT 1"
    );
    connection
        .query_row(&sql, [connector_id, source_account, source_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()
        .map(|value| value.flatten())
        .map_err(Into::into)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalystReadiness {
    pub entities_curated: usize,
    pub catalysts_present: usize,
    pub items_pending: usize,
    pub reasons: BTreeMap<String, usize>,
}

impl CatalystReadiness {
    pub fn message(&self) -> String {
        let reasons = self
            .reasons
            .iter()
            .map(|(reason, count)| format!("{reason}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "status=catalysts_pending entities_curated={} catalysts_present={} items_pending={} reasons=[{}]; run `margins init`",
            self.entities_curated, self.catalysts_present, self.items_pending, reasons
        )
    }
}

/// Whether an entity the engine selected still waits for catalysts it can
/// generate. A `skipped` entity has too little evidence for a generation job,
/// so it does not hold recall back.
fn awaits_catalysts(entity: &crate::enzyme_cli::SelectedEntity) -> bool {
    matches!(entity.state.as_str(), "pending" | "unchecked")
}

/// Catalyst readiness from the engine's own selection report: what it would
/// select now and which of those entities have catalysts.
fn catalyst_readiness(status: &StatusEnvelope) -> CatalystReadiness {
    let selection = status.selection.clone().unwrap_or_default();
    let mut reasons = BTreeMap::new();
    let mut pending = 0;
    for entity in selection
        .entities
        .iter()
        .filter(|entity| entity.state != "ready")
    {
        if awaits_catalysts(entity) {
            pending += 1;
        }
        let reason = match entity.state.as_str() {
            "skipped" => entity
                .skip_reason
                .as_ref()
                .and_then(|reason| reason.get("kind"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("skipped")
                .replace('_', "-"),
            "pending" => "no-catalyst".to_string(),
            other => other.to_string(),
        };
        *reasons.entry(reason).or_insert(0) += 1;
    }
    CatalystReadiness {
        entities_curated: selection.selected,
        catalysts_present: status.catalysts,
        // Preserve partial recall when at least one selected entity has a
        // catalyst. Suppressed entities remain auditable in `reasons`; a fully
        // suppressed selection is still catalysts_pending.
        items_pending: if status.catalysts > 0 { 0 } else { pending },
        reasons,
    }
}

fn classify_init_status(status: &StatusEnvelope) -> InitStatus {
    let readiness = catalyst_readiness(status);
    if readiness.items_pending > 0 {
        return InitStatus {
            status: "catalysts_pending",
            reason: "catalysts_pending",
            readiness,
            documents: status.documents,
            catalysts: status.catalysts,
            attention: None,
            entities: Vec::new(),
        };
    }
    InitStatus {
        status: "ok",
        reason: if status.catalysts > 0 { "catalyst" } else { "no_entities" },
        readiness,
        documents: status.documents,
        catalysts: status.catalysts,
        attention: None,
        entities: Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitStatus {
    /// `ok`, `catalysts_pending`, or `index_only` (indexed and searchable
    /// directly; no catalyst generator is set up).
    pub status: &'static str,
    pub reason: &'static str,
    pub readiness: CatalystReadiness,
    /// Indexed documents and catalysts after this refresh.
    pub documents: usize,
    pub catalysts: usize,
    /// What changed in attention since the previous refresh.
    pub attention: Option<crate::attention::Diff>,
    /// What the engine selected after this refresh.
    pub entities: Vec<crate::attention::Entity>,
}

/// The one line that turns catalysts on, for output that reports
/// `index_only`.
pub const ENABLE_CATALYSTS_HINT: &str =
    "Catalysts are off, so search finds direct matches only. Turn them on with `margins setup --only catalyst`.";


/// Require the setup-selected generator to be usable without discovery,
/// provisioning, network access, or environment-key fallback.
pub fn ensure_usable_generator() -> Result<()> {
    let home = margins_home()?;
    ensure_usable_generator_at(&home)
}

pub(crate) fn ensure_usable_generator_at(home: &Path) -> Result<()> {
    let status = margins_workflows::catalyst::selected_status(home);
    match status.mode {
        margins_workflows::catalyst::CatalystMode::Hosted => {
            if crate::hosted_credentials::cached_bundle_for_generation(home)?.is_some() {
                Ok(())
            } else {
                anyhow::bail!(RECALL_UNAVAILABLE_MESSAGE)
            }
        }
        margins_workflows::catalyst::CatalystMode::Local if local_generator_installed() => Ok(()),
        margins_workflows::catalyst::CatalystMode::Local
        | margins_workflows::catalyst::CatalystMode::None => {
            debug_strategy(&format!("generator unavailable ({})", status.reason));
            anyhow::bail!(RECALL_UNAVAILABLE_MESSAGE)
        }
    }
}

#[cfg(feature = "recall-local-model")]
fn local_generator_installed() -> bool {
    crate::catalyst_model_setup::is_installed()
}

#[cfg(not(feature = "recall-local-model"))]
fn local_generator_installed() -> bool {
    false
}

fn margins_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("MARGINS_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".margins"))
}

/// Whether recall lookup is opted out via `MARGINS_RECALL=off` (also accepts
/// `0`/`false`/`disable`).
fn recall_disabled() -> bool {
    std::env::var("MARGINS_RECALL")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(
                v.as_str(),
                "off" | "0" | "false" | "no" | "disable" | "disabled"
            )
        })
        .unwrap_or(false)
}

/// Optional stderr trace of which search layer served the request. Off unless
/// `MARGINS_RECALL_DEBUG` is set; never touches stdout (the XML contract).
fn debug_strategy(strategy: &str) {
    if std::env::var_os("MARGINS_RECALL_DEBUG").is_some() {
        eprintln!("recall: {strategy}");
    }
}


/// Validate the external-source snapshot the index holds from the engine's
/// source refresh state and Margins' materialization receipts. Native
/// Markdown freshness does not block lookup; it is reported by `enzyme
/// status` and repaired by the next init or sync.
fn snapshot_index_freshness(
    workspace: &ResolvedWorkspace,
    status: &StatusEnvelope,
) -> Result<EvidenceFreshness> {
    let sources = source_refresh_staleness(workspace, Some(status))?;
    let Some(stale) = sources.values().find(|source| source.stale) else {
        return Ok(if sources.is_empty() {
            snapshot_unknown_freshness()
        } else {
            EvidenceFreshness {
                status: FreshnessStatus::Fresh,
                stale: false,
                reason: None,
                last_successful_refresh: None,
            }
        });
    };
    Ok(EvidenceFreshness {
        status: FreshnessStatus::Stale,
        stale: true,
        reason: stale.stale_reason.clone(),
        last_successful_refresh: None,
    })
}

fn snapshot_unknown_freshness() -> EvidenceFreshness {
    EvidenceFreshness {
        status: FreshnessStatus::NotApplicable,
        stale: false,
        reason: Some("snapshot_freshness_unknown".to_string()),
        last_successful_refresh: None,
    }
}

/// Convenience for the CLI interception: run recall and return the JSON string
/// to print to stdout.
pub fn recall_json(
    workspace: &ResolvedWorkspace,
    query: &str,
    source: Option<&str>,
) -> Result<String> {
    render_recall_json(&recall(workspace, query, source)?)
}

/// How Margins asks the engine to bring an index up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provision {
    /// `enzyme init`: index, embed, select, and generate catalysts now.
    Init,
    /// `enzyme refresh`: bring documents up to date now; a due catalyst epoch
    /// is built by a detached worker.
    Refresh,
}

/// Build or update the Workspace index and generate its catalysts in the
/// foreground (`margins init`).
pub fn provision_workspace_for_init(workspace: &ResolvedWorkspace) -> Result<InitStatus> {
    provision(workspace, Provision::Init)
}

/// Bring the Workspace index up to date after its sources changed (sync,
/// connector reconcile, imports). Catalyst epochs that become due are built in
/// the background by the engine.
pub fn refresh_workspace(workspace: &ResolvedWorkspace) -> Result<InitStatus> {
    provision(workspace, Provision::Refresh)
}

/// The marker an in-process Margins (before the engine ran as a process)
/// kept beside the index to record its document identity.
const LEGACY_IDENTITY_MARKER: &str = "index.identity";

fn provision(workspace: &ResolvedWorkspace, mode: Provision) -> Result<InitStatus> {
    std::fs::create_dir_all(&workspace.state_dir)
        .with_context(|| format!("creating workspace state {}", workspace.state_dir.display()))?;
    // Every Workspace declares a captures source, and ledger kinds read the
    // ledger: the engine requires the database to exist before it indexes.
    crate::workspace_recall::ensure_ledger(workspace)?;
    let margins_home = margins_home()?;
    let engine = Engine::for_home(&margins_home)?;
    let generator = crate::enzyme_cli::selected_generator(&engine, &margins_home)?;
    let id = &workspace.config.id;

    // An index another Margins built in-process stays in use when the engine
    // reads it (same `enzyme.db`, compatible schema); one it cannot read is
    // rebuilt once with `init --force`.
    let existing = if workspace.recall_path().is_file() {
        Some(engine.status(id).context("reading the Workspace index status")?)
    } else {
        None
    };
    let rebuild = existing.as_ref().is_some_and(StatusEnvelope::needs_rebuild);
    let legacy = workspace.state_dir.join(LEGACY_IDENTITY_MARKER);
    debug_strategy(&match &existing {
        None => "engine index first_build".to_string(),
        Some(_) if rebuild => "engine index rebuild reason=schema_outdated".to_string(),
        Some(_) if legacy.exists() => "engine index reuse reason=in_process_index".to_string(),
        Some(_) => "engine index reuse".to_string(),
    });

    // `margins init` waits for another build as long as the engine does; a
    // sync-triggered refresh waits briefly and reports busy instead.
    let lock_timeout = match mode {
        Provision::Init => None,
        Provision::Refresh => Some(REFRESH_LOCK_TIMEOUT_SECS),
    };
    let built = match mode {
        Provision::Refresh if !rebuild && existing.is_some() => engine
            .refresh(id, &generator, lock_timeout)
            .map(|summary| {
                debug_strategy(&format!(
                    "engine refresh background_spawned={}",
                    summary
                        .get("background_spawned")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                ));
                None
            }),
        _ => engine
            .init(id, &generator, rebuild, lock_timeout)
            .map(Some),
    };
    // Exit 5: the index is built and searchable, but catalyst generation
    // failed its quality gate. Report the catalysts as pending.
    let (generated, catalysts_failed) = match built {
        Ok(generated) => (generated, None),
        Err(EngineError::CatalystsFailed(message)) => (None, Some(message)),
        Err(error) => return Err(provision_error(error, mode)),
    };
    if legacy.exists() {
        std::fs::remove_file(&legacy)
            .with_context(|| format!("removing {}", legacy.display()))?;
    }
    let status = engine
        .status(id)
        .context("reading the Workspace index status")?;
    let mut init_status = classify_init_status(&status);
    if let Some(message) = &catalysts_failed {
        debug_strategy(&format!("engine catalysts failed: {message}"));
        init_status.status = "catalysts_pending";
        init_status.reason = "catalysts_pending";
    }
    debug_strategy(&format!(
        "workspace index provisioned mode={mode:?} generator={generator:?} documents={} catalysts={} hosted_generation_calls={} status={}",
        status.documents,
        status.catalysts,
        generated
            .as_ref()
            .filter(|_| matches!(generator, Generator::Env { .. }))
            .map_or(0, |summary| summary.entities_generated),
        init_status.status,
    ));
    // Without a generator the documents are indexed (`--llm none`) and
    // searchable directly; catalysts wait until setup chooses a generator.
    if !generator.generates() {
        init_status.status = "index_only";
        init_status.reason = "catalysts_not_set_up";
    }
    // Remember what Margins learns about, and say what changed since the
    // previous refresh. A failed write only costs the next diff a baseline.
    let revision = margins_workflows::workspace::workspace_revision(workspace)
        .unwrap_or_default();
    let next = crate::attention::snapshot(&status, &revision, crate::enzyme_cli::required_version());
    let casing = margins_workflows::workspace::name_casing(workspace);
    match crate::attention::record(&workspace.state_dir, &next, generator.generates(), &casing) {
        Ok(diff) => init_status.attention = Some(diff),
        Err(error) => debug_strategy(&format!("attention snapshot not saved: {error:#}")),
    }
    init_status.entities = next.entities;
    Ok(init_status)
}

/// Seconds a sync-triggered index update waits for another build of the same
/// Workspace before reporting it busy.
const REFRESH_LOCK_TIMEOUT_SECS: u64 = 10;

fn provision_error(error: EngineError, mode: Provision) -> anyhow::Error {
    let busy = matches!(error, EngineError::Busy(_));
    let error = anyhow::Error::new(error);
    if busy && mode == Provision::Refresh {
        return error.context(
            "another Margins command is building this Workspace's index; this update is retried by the next `margins sync` or `margins init`",
        );
    }
    error.context("indexing and generating recall catalysts")
}

fn combined_freshness(
    index: EvidenceFreshness,
    materialization: Vec<SourceMaterializationFreshness>,
) -> RecallFreshness {
    let least_fresh_materialization = materialization
        .iter()
        .max_by_key(|source| freshness_severity(source.freshness.status));
    let materialization_status = least_fresh_materialization
        .map(|source| source.freshness.status)
        .unwrap_or(FreshnessStatus::Fresh);
    let status = if freshness_severity(index.status) >= freshness_severity(materialization_status) {
        index.status
    } else {
        materialization_status
    };
    RecallFreshness {
        status,
        stale: index.stale || materialization.iter().any(|source| source.freshness.stale),
        reason: if freshness_severity(materialization_status) > freshness_severity(index.status) {
            least_fresh_materialization.and_then(|source| source.freshness.reason.clone())
        } else {
            index.reason.clone()
        },
        index,
        materialization,
    }
}

fn require_retrieval_freshness(
    index: EvidenceFreshness,
    materialization: Vec<SourceMaterializationFreshness>,
) -> Result<RecallFreshness> {
    let freshness = combined_freshness(index, materialization);
    if freshness.index.stale {
        anyhow::bail!(
            "recall_unavailable_stale_materialization: refresh the workspace index before semantic retrieval"
        );
    }
    Ok(freshness)
}

fn freshness_severity(status: FreshnessStatus) -> u8 {
    match status {
        FreshnessStatus::Fresh | FreshnessStatus::NotApplicable => 0,
        FreshnessStatus::Stale => 1,
        FreshnessStatus::NeedsAuth => 2,
        FreshnessStatus::Error => 3,
    }
}

fn materialization_freshness(
    workspace: &ResolvedWorkspace,
) -> Result<Vec<SourceMaterializationFreshness>> {
    let external = workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| {
            let (connector, account, expected_fingerprint, expected_scope) = match binding {
                WorkspaceBinding::Gmail { account, gmail } => (
                    "email",
                    account,
                    Some(gmail.materialization_fingerprint()),
                    Ok(None),
                ),
                WorkspaceBinding::GoogleCalendar { account, calendar } => {
                    let expected_scope = GoogleCalendarScope::for_selector(calendar, Utc::now())
                        .and_then(|scope| {
                            serde_json::to_string(&scope.as_range())
                                .context("failed to serialize Calendar rolling boundary")
                        });
                    (
                        "gcal",
                        account,
                        Some(calendar.materialization_fingerprint()),
                        expected_scope.map(Some),
                    )
                }
                WorkspaceBinding::GoogleMeet { account } => (
                    "google_meet",
                    account,
                    Some(Ok(GOOGLE_MEET_MATERIALIZATION_FINGERPRINT.to_string())),
                    Ok(None),
                ),
                WorkspaceBinding::Granola {
                    account,
                    collection,
                } => (
                    "granola",
                    account,
                    Some(collection.materialization_fingerprint()),
                    Ok(None),
                ),
                _ => return None,
            };
            Some(
                expected_fingerprint
                    .transpose()
                    .and_then(|expected_fingerprint| {
                        Ok((
                            name,
                            connector,
                            account,
                            expected_fingerprint,
                            expected_scope?,
                        ))
                    }),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    if external.is_empty() {
        return Ok(Vec::new());
    }
    let connection = workspace
        .ledger_path()
        .is_file()
        .then(|| {
            Connection::open_with_flags(workspace.ledger_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        })
        .transpose()?;
    external
        .into_iter()
        .map(
            |(name, connector, account, expected_fingerprint, expected_scope)| {
                let row: Option<(String, Option<String>, Option<String>, Option<String>)> =
                    connection
                        .as_ref()
                        .map(|connection| {
                            connection
                        .query_row(
                            "SELECT health_status, last_sync_at, materialization_fingerprint,
                                    scope_boundary_json
                             FROM connectors WHERE connector_id = ?1 AND account = ?2",
                            [connector, account],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                        )
                        .optional()
                        })
                        .transpose()?
                        .flatten();
                let (status, stale, reason, last_successful_refresh) = match row {
                    None => (
                        FreshnessStatus::Stale,
                        true,
                        Some("never_refreshed".to_string()),
                        None,
                    ),
                    Some((status, last_sync_at, stored_fingerprint, stored_scope)) => {
                        let last_successful_refresh = last_sync_at
                            .as_deref()
                            .map(DateTime::parse_from_rfc3339)
                            .transpose()?
                            .map(|value| value.with_timezone(&Utc));
                        match status.as_str() {
                            "fresh"
                                if expected_fingerprint.as_deref().is_some_and(|expected| {
                                    stored_fingerprint.as_deref() != Some(expected)
                                }) || expected_scope.as_ref().is_some_and(|expected| {
                                    stored_scope.as_deref() != Some(expected.as_str())
                                }) =>
                            {
                                (
                                    FreshnessStatus::Stale,
                                    true,
                                    Some("refresh_required".to_string()),
                                    last_successful_refresh,
                                )
                            }
                            "fresh" => {
                                (FreshnessStatus::Fresh, false, None, last_successful_refresh)
                            }
                            "stale" | "unknown" => (
                                FreshnessStatus::Stale,
                                true,
                                Some("refresh_required".to_string()),
                                last_successful_refresh,
                            ),
                            "needs-auth" => (
                                FreshnessStatus::NeedsAuth,
                                true,
                                Some("credentials_unavailable".to_string()),
                                last_successful_refresh,
                            ),
                            "error" => (
                                FreshnessStatus::Error,
                                true,
                                Some("refresh_failed".to_string()),
                                last_successful_refresh,
                            ),
                            other => anyhow::bail!("unknown connector health status: {other}"),
                        }
                    }
                };
                Ok(SourceMaterializationFreshness {
                    source: name.clone(),
                    freshness: EvidenceFreshness {
                        status,
                        stale,
                        reason,
                        last_successful_refresh,
                    },
                })
            },
        )
        .collect()
}


/// Render the Margins-owned machine result envelope. Enzyme's catalyst text is
/// passed through unchanged, while source identity and freshness are resolved
/// at this product boundary.
pub fn render_recall_json(output: &RecallOutput) -> Result<String> {
    // Catalyst text is the canonical transport. Keep the existing contributor
    // fields, but do not publish the engine's rebuildable struct projections;
    // provenance receipts remain inline in `text` exactly as stored.
    let catalysts = output
        .top_contributing_catalysts
        .iter()
        .map(|catalyst| {
            serde_json::json!({
                "id": catalyst.id,
                "text": catalyst.text,
                "entity": catalyst.entity,
                "topic_name": catalyst.topic_name,
                "relevance_score": catalyst.relevance_score,
                "contribution_count": catalyst.contribution_count,
            })
        })
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "schema_version": "margins.recall.v1",
        "status": output.status,
        "reason": output.reason,
        "freshness": output.freshness,
        "results": output.results,
        "top_contributing_catalysts": catalysts,
        "query": output.query,
        "search_strategy": output.search_strategy,
        "processing_time": output.processing_time,
        "total_results": output.results.len(),
    });
    Ok(format!("{}\n", serde_json::to_string(&value)?))
}

/// Render the human-facing tree used by interactive `enzyme catalyze` output.
/// A catalyst response may omit its contributor summary, so the surfaced-notes
/// branch remains valid even when no activated-bridge heading is available.
pub fn render_recall_tree(output: &RecallOutput, terminal_width: usize) -> String {
    let mut tree = String::new();
    let _ = writeln!(tree, "Catalyze \"{}\"", output.query);
    if output.reason == "catalysts_not_set_up" {
        let _ = writeln!(tree, "├─ {ENABLE_CATALYSTS_HINT}");
    }

    if output.top_contributing_catalysts.is_empty() {
        if output.results.is_empty() {
            tree.push_str("└─ no activated bridges\n");
            return tree;
        }
        tree.push_str("├─ no activated bridges\n");
    } else {
        tree.push_str("├─ activated bridges\n");
        for (idx, catalyst) in output.top_contributing_catalysts.iter().enumerate() {
            let bridge_no = idx + 1;
            let _ = writeln!(
                tree,
                "│  [{}] {} · {:.3} · {} {}",
                bridge_no,
                catalyst.entity,
                catalyst.relevance_score,
                catalyst.contribution_count,
                pluralize(catalyst.contribution_count, "hit", "hits")
            );
            // Preserve the canonical chunk's line boundaries. In particular,
            // receipt JSONL must not be reconstructed or reflowed by Margins.
            for line in catalyst.text.split('\n') {
                let _ = writeln!(tree, "│      {line}");
            }
            if idx + 1 != output.top_contributing_catalysts.len() {
                tree.push_str("│\n");
            }
        }
    }

    tree.push_str("└─ surfaced notes\n");
    if output.results.is_empty() {
        tree.push_str("   └─ no results\n");
        return tree;
    }

    for (idx, result) in output.results.iter().enumerate() {
        let is_last = idx + 1 == output.results.len();
        let connector = if is_last { "└─" } else { "├─" };
        let stem = if is_last { "   " } else { "│  " };
        let bridge_label = result
            .via_catalyst_id
            .as_deref()
            .and_then(|id| bridge_index(output, id))
            .map(|n| format!("[{n}] "))
            .unwrap_or_default();

        let _ = writeln!(
            tree,
            "   {connector} {bridge_label}{} · {:.3}",
            result.document_ref, result.similarity
        );
        let _ = writeln!(tree, "   {stem}");
        for line in wrap_preserving_blank_lines(
            &result.content,
            content_wrap_width(terminal_width),
            "   ",
            stem,
        ) {
            let _ = writeln!(tree, "{line}");
        }
        if !is_last {
            tree.push_str("   │\n");
        }
    }

    let _ = writeln!(
        tree,
        "\n{} results · {:.3} ms",
        output.results.len(),
        output.processing_time
    );
    tree
}

pub fn render_recall_for_stdout(output: &RecallOutput, terminal: bool) -> Result<String> {
    if terminal {
        let width = crossterm::terminal::size()
            .map(|(width, _)| width as usize)
            .unwrap_or(120);
        Ok(render_recall_tree(output, width))
    } else {
        render_recall_json(output)
    }
}

fn bridge_index(output: &RecallOutput, catalyst_id: &str) -> Option<usize> {
    output
        .top_contributing_catalysts
        .iter()
        .position(|catalyst| catalyst.id == catalyst_id)
        .map(|idx| idx + 1)
}

fn content_wrap_width(terminal_width: usize) -> usize {
    terminal_width.saturating_sub(9).clamp(96, 140)
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn wrap_preserving_blank_lines(
    text: &str,
    width: usize,
    result_prefix: &str,
    tree_stem: &str,
) -> Vec<String> {
    let prefix = format!("{result_prefix}{tree_stem}   ");
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.trim().is_empty() {
            lines.push(prefix.trim_end().to_string());
            continue;
        }
        lines.extend(wrap_text(raw_line, width, &prefix));
    }
    if text.ends_with('\n') {
        lines.push(prefix.trim_end().to_string());
    }
    lines
}

fn wrap_text(text: &str, width: usize, prefix: &str) -> Vec<String> {
    let available = width.max(1);
    let mut out = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= available {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(format!("{prefix}{current}"));
            current.clear();
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        out.push(format!("{prefix}{current}"));
    }
    if out.is_empty() {
        out.push(prefix.trim_end().to_string());
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn meet_connector_health_contributes_to_materialization_freshness() {
        use margins_workflows::integrations::{ConnectorCtx, HealthStatus, IntegrationsStore};
        use margins_workflows::workspace::{add_source, create_workspace, WorkspaceBinding};

        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace = create_workspace(&margins_home, "practice", None, &notes).unwrap();
        add_source(
            &mut workspace,
            "meet",
            WorkspaceBinding::GoogleMeet {
                account: "owner@example.com".to_string(),
            },
        )
        .unwrap();
        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: "google_meet".to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        store
            .update_health(&ctx, HealthStatus::Fresh, None)
            .unwrap();
        assert_eq!(
            store
                .mark_google_connection_needs_auth("owner@example.com")
                .unwrap(),
            1
        );

        let freshness = materialization_freshness(&workspace).unwrap();
        assert_eq!(freshness.len(), 1);
        assert_eq!(freshness[0].source, "meet");
        assert_eq!(freshness[0].freshness.status, FreshnessStatus::NeedsAuth);
        assert!(freshness[0].freshness.stale);
        assert_eq!(
            freshness[0].freshness.reason.as_deref(),
            Some("credentials_unavailable")
        );
        assert!(freshness[0].freshness.last_successful_refresh.is_some());
    }


    fn output(status: &'static str, results: Vec<RecallResult>) -> RecallOutput {
        RecallOutput {
            query: "q".into(),
            status,
            reason: "catalyst",
            freshness: combined_freshness(
                EvidenceFreshness {
                    status: FreshnessStatus::Fresh,
                    stale: false,
                    reason: None,
                    last_successful_refresh: None,
                },
                Vec::new(),
            ),
            config_path: None,
            note_count: 0,
            results,
            top_contributing_catalysts: Vec::new(),
            search_strategy: "catalyze",
            processing_time: 0.0,
        }
    }

    struct EnvRestore {
        variables: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn capture() -> Self {
            Self {
                variables: resolver_env_names()
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, previous) in self.variables.drain(..) {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    const fn resolver_env_names() -> &'static [&'static str] {
        &[
            "MARGINS_HOME",
            "ENZYME_HOME",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
            "ENZYME_FREE_CONFIG_URL",
            "ENZYME_API_CACHE_PATH",
            "ENZYME_LOCAL_ENGINE",
        ]
    }

    fn clear_resolver_env() {
        for name in resolver_env_names() {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn json_and_init_status_contracts_use_shared_vocabulary() {
        for status in ["thin", "off", "no_vault", "unavailable", "no_policy"] {
            let output = output(status, vec![]);
            let json: serde_json::Value =
                serde_json::from_str(&render_recall_json(&output).unwrap()).unwrap();
            assert_eq!(json["query"], "q");
            assert_eq!(json["results"], serde_json::json!([]));
            assert_eq!(json["top_contributing_catalysts"], serde_json::json!([]));
            assert_eq!(json["search_strategy"], "catalyze");
            assert_eq!(json["total_results"], 0);
        }
        let status = |catalysts: usize, entities: serde_json::Value| -> StatusEnvelope {
            let selected = entities.as_array().unwrap().len();
            serde_json::from_value(serde_json::json!({
                "schema": "enzyme.status.v1",
                "initialized": true,
                "documents": 4,
                "catalysts": catalysts,
                "selection": {"selected": selected, "entities": entities},
            }))
            .unwrap()
        };
        let ready = classify_init_status(&status(
            6,
            serde_json::json!([
                {"name": "projects", "type": "folder", "state": "ready", "catalysts": 6},
                {"name": "Marisol", "type": "link", "state": "skipped", "catalysts": 0,
                 "skip_reason": {"kind": "thin_context"}},
            ]),
        ));
        assert_eq!(ready.status, "ok");
        assert_eq!(ready.reason, "catalyst");
        assert_eq!(
            ready.readiness,
            CatalystReadiness {
                entities_curated: 2,
                catalysts_present: 6,
                items_pending: 0,
                reasons: BTreeMap::from([("thin-context".to_string(), 1)]),
            }
        );
        let pending = classify_init_status(&status(
            0,
            serde_json::json!([
                {"name": "projects", "type": "folder", "state": "pending", "catalysts": 0},
                {"name": "radio", "type": "tag", "state": "pending", "catalysts": 0},
            ]),
        ));
        assert_eq!(pending.status, "catalysts_pending");
        assert_eq!(pending.readiness.items_pending, 2);
        assert_eq!(
            pending.readiness.reasons,
            BTreeMap::from([("no-catalyst".to_string(), 2)])
        );
        let none = classify_init_status(&status(0, serde_json::json!([])));
        assert_eq!(none.status, "ok");
        assert_eq!(none.reason, "no_entities");
        // Entities too thin for a generation job do not hold recall back.
        let thin = classify_init_status(&status(
            0,
            serde_json::json!([
                {"name": "sqlite:mail", "type": "collection", "state": "skipped", "catalysts": 0,
                 "skip_reason": {"kind": "thin_context"}},
            ]),
        ));
        assert_eq!(thin.status, "ok");
        assert_eq!(thin.reason, "no_entities");
        assert_eq!(
            thin.readiness.reasons,
            BTreeMap::from([("thin-context".to_string(), 1)])
        );
    }
    #[test]
    fn json_uses_typed_margins_result_shape() {
        let mut output = output(
            "ok",
            vec![RecallResult {
                document_ref: "people/jane.md".into(),
                similarity: 0.831,
                content: "full passage".into(),
                source: "home".into(),
                source_kind: SourceKind::Notes,
                evidence: EvidenceHandle::NativeMarkdown {
                    path: "/notes/people/jane.md".into(),
                },
                via_catalyst_id: Some("bridge-1".into()),
                via_catalyst_text: Some("full stored catalyst chunk".into()),
            }],
        );
        output.top_contributing_catalysts = vec![TopCatalyst {
            id: "bridge-1".into(),
            text: "full stored catalyst chunk".into(),
            entity: "Person".into(),
            topic_name: Some("Decision".into()),
            relevance_score: 0.9,
            contribution_count: 1,
        }];
        let json: serde_json::Value =
            serde_json::from_str(&render_recall_json(&output).unwrap()).unwrap();
        assert_eq!(json["schema_version"], "margins.recall.v1");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["reason"], "catalyst");
        assert_eq!(json["freshness"]["status"], "fresh");
        assert_eq!(json["results"][0]["document_ref"], "people/jane.md");
        assert_eq!(json["results"][0]["evidence"]["kind"], "native_markdown");
        assert_eq!(
            json["results"][0]["evidence"]["path"],
            "/notes/people/jane.md"
        );
        assert!(json["results"][0].get("file_path").is_none());
        assert_eq!(json["results"][0]["content"], "full passage");
        assert_eq!(json["results"][0]["similarity"], 0.831);
        assert_eq!(json["results"][0]["via_catalyst_id"], "bridge-1");
        assert_eq!(
            json["results"][0]["via_catalyst_text"],
            "full stored catalyst chunk"
        );
        assert_eq!(
            json["top_contributing_catalysts"][0]["text"],
            "full stored catalyst chunk"
        );
        assert!(json["top_contributing_catalysts"][0]
            .get("evidence_anchors")
            .is_none());
        assert_eq!(json["total_results"], 1);
    }

    #[test]
    fn tree_matches_enzyme_catalyze_shape() {
        let mut output = output(
            "ok",
            vec![RecallResult {
                document_ref: "people/jane.md".into(),
                similarity: 0.831,
                content: "full passage".into(),
                source: "home".into(),
                source_kind: SourceKind::Notes,
                evidence: EvidenceHandle::NativeMarkdown {
                    path: "/notes/people/jane.md".into(),
                },
                via_catalyst_id: Some("bridge-1".into()),
                via_catalyst_text: Some("What changed?".into()),
            }],
        );
        output.processing_time = 1.234;
        output.top_contributing_catalysts = vec![TopCatalyst {
            id: "bridge-1".into(),
            text: "What changed?".into(),
            entity: "Person".into(),
            topic_name: None,
            relevance_score: 0.9,
            contribution_count: 1,
        }];

        assert_eq!(
            render_recall_tree(&output, 120),
            concat!(
                "Catalyze \"q\"\n",
                "├─ activated bridges\n",
                "│  [1] Person · 0.900 · 1 hit\n",
                "│      What changed?\n",
                "└─ surfaced notes\n",
                "   └─ [1] people/jane.md · 0.831\n",
                "      \n",
                "         full passage\n",
                "\n",
                "1 results · 1.234 ms\n",
            )
        );
    }

    #[test]
    fn tree_preserves_canonical_catalyst_jsonl_lines() {
        let receipt = r#"{"anchor_id":"E1","source_ref":"sqlite:mail/a-very-long-source-reference-that-must-not-be-reflowed","occurrence_ids":[7],"timestamp_ms":1,"quote":"verbatim"}"#;
        let chunk = format!(
            "```enzyme-catalyst-header\n{{\"format_version\":1}}\n```\n\nHypothesis prose.\n\n```receipts\n{receipt}\n```"
        );
        let mut output = output("ok", Vec::new());
        output.top_contributing_catalysts = vec![TopCatalyst {
            id: "bridge-1".into(),
            text: chunk,
            entity: "Person".into(),
            topic_name: None,
            relevance_score: 0.9,
            contribution_count: 1,
        }];

        let tree = render_recall_tree(&output, 20);
        assert!(tree.contains(&format!("│      {receipt}\n")));
        assert_eq!(tree.matches(receipt).count(), 1);
    }

    #[test]
    fn stdout_mode_selects_tree_for_terminals_and_json_for_pipes() {
        let output = output("thin", vec![]);

        assert_eq!(
            render_recall_for_stdout(&output, true).unwrap(),
            "Catalyze \"q\"\n└─ no activated bridges\n"
        );
        let piped: serde_json::Value =
            serde_json::from_str(&render_recall_for_stdout(&output, false).unwrap()).unwrap();
        assert_eq!(piped["query"], "q");
        assert_eq!(piped["results"], serde_json::json!([]));
    }

    #[test]
    fn recall_reports_missing_snapshot_for_tiny_vault_without_indexing() {
        let _guard = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture();
        let tmp = tempfile::tempdir().unwrap();
        let margins_home = tmp.path().join("margins-home");
        std::env::set_var("MARGINS_HOME", &margins_home);
        crate::hosted_credentials::install_bundle(
            &margins_home,
            "thin-fixture",
            "fixture-key",
            "http://127.0.0.1:9/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        for i in 0..2 {
            std::fs::write(notes.join(format!("note-{i}.md")), "# tiny\nbody").unwrap();
        }
        let workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "test", None, &notes)
                .unwrap();
        let out = recall(&workspace, "anything", None).unwrap();
        assert_eq!(out.status, "unavailable");
        assert_eq!(out.reason, "not_established");
        assert_eq!(out.note_count, 0);
        // No index built for a below-threshold vault.
        assert!(!workspace.recall_path().exists());
    }

    #[test]
    fn recall_reports_missing_index_without_provisioning() {
        let _guard = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture();
        let tmp = tempfile::tempdir().unwrap();
        let margins_home = tmp.path().join("margins-home");
        std::env::set_var("MARGINS_HOME", &margins_home);
        crate::hosted_credentials::install_included_bundle(
            &margins_home,
            "query-must-not-read-bootstrap",
            "fixture-query-key",
            Some(4_102_444_800),
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::env::set_var(
            "ENZYME_FREE_CONFIG_URL",
            format!("http://{}/llm/free-config", listener.local_addr().unwrap()),
        );
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        for i in 0..USEFULNESS_THRESHOLD {
            std::fs::write(
                notes.join(format!("note-{i}.md")),
                format!("# Note {i}\nSubstantive local material about project {i}."),
            )
            .unwrap();
        }
        let workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "test", None, &notes)
                .unwrap();

        let out = recall(&workspace, "project", None).unwrap();

        assert_eq!(out.status, "unavailable");
        assert_eq!(out.reason, "not_established");
        assert!(!workspace.recall_path().exists());
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn ledger_hits_map_back_to_external_evidence() {
        let _guard = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture();
        let tmp = tempfile::tempdir().unwrap();
        let margins_home = tmp.path().join("margins-home");
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("note.md"), "# Home").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "test", None, &notes)
                .unwrap();
        let account = "owner@example.com";
        margins_workflows::workspace::add_source(
            &mut workspace,
            "mail",
            WorkspaceBinding::Gmail {
                account: account.to_string(),
                gmail: margins_workflows::workspace::GmailCollectionSelector::default_declaration(),
            },
        )
        .unwrap();
        let ctx = margins_workflows::integrations::ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: "email".to_string(),
            account: account.to_string(),
            command_path: None,
        };
        margins_workflows::integrations::IntegrationsStore::open(&workspace.state_dir)
            .unwrap()
            .replace_email_thread_snapshot(
                &ctx,
                vec![margins_workflows::integrations::ThreadEvidence {
                    thread_id: "thread-123".to_string(),
                    occurred_from: Utc.with_ymd_and_hms(2026, 8, 20, 9, 0, 0).unwrap(),
                    occurred_to: Utc.with_ymd_and_hms(2026, 8, 20, 9, 30, 0).unwrap(),
                    body_text: "External evidence body".to_string(),
                    href: Some("https://mail.example/thread-123".to_string()),
                }],
                Vec::new(),
            )
            .unwrap();
        let document_ref =
            margins_workflows::source_kinds::ledger_document_ref("mail", "thread-123");

        let entry =
            catalog_entry_for_document_ref(&workspace, &document_ref).unwrap();

        assert_eq!(
            source_name_for_document_ref(&workspace, &document_ref).as_deref(),
            Some("mail")
        );
        assert_eq!(entry.source, "mail");
        assert_eq!(entry.kind, SourceKind::GoogleMail);
        assert_eq!(
            entry.evidence,
            EvidenceHandle::ExternalRecord {
                connector_id: "email".to_string(),
                source_account: account.to_string(),
                source_id: "thread-123".to_string(),
                href: Some("https://mail.example/thread-123".to_string()),
            }
        );
    }
}


