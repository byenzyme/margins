//! Associative vault search backing `margins recall`.
//!
//! This is the private composition of the subtree-linked recall engine (see
//! `crates/private/recall-engine`). Recall only opens a pre-built index; vault
//! establishment and catalyst generation belong to `margins init`.
//!
//! The primary lookup follows thematic bridges generated at index time, so a
//! query can surface notes that share a theme without sharing words. Querying
//! never resolves credentials, provisions models, generates, or uses network.
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
use recall_engine::kernel::{Bridged, SearchIndex};
use recall_engine::search::context::TopCatalyst;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::workspace_recall::{CatalogEntry, WorkspaceCorpus};
use margins_workflows::integrations::{
    EvidenceFreshness, EvidenceHandle, FreshnessStatus, GoogleCalendarScope,
    GOOGLE_MEET_MATERIALIZATION_FINGERPRINT,
};
use margins_workflows::workspace::{ResolvedWorkspace, SourceKind, WorkspaceBinding};
use margins_workflows::workspace_lowering::sqlite_document_ref_prefix;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

/// A vault with fewer than this many notes is not worth indexing: recall would
/// have too little material to draw real associations from, so we return thin
/// results without paying to build an index.
#[cfg(test)]
const USEFULNESS_THRESHOLD: usize = 5;

/// How many related passages to return. A fixed, sensible default keeps the
/// command flag-free.
const RESULT_LIMIT: usize = 8;
/// Cap on entities given thematic bridges per build, to bound LLM spend. Recall
/// generates bridges for the most frequent entities. A bounded literal phrase
/// lookup remains available across the declared document boundary so curation
/// cannot make indexed Sources unreachable.
const MAX_BRIDGED_ENTITIES: usize = crate::workspace_recall::ENGINE_ENTITY_LIMIT;

pub fn workspace_source_refresh_staleness(
    workspace: &ResolvedWorkspace,
) -> Result<BTreeMap<String, margins_cli::commands::workspace::SourceRefreshStalenessView>> {
    use margins_cli::commands::workspace::SourceRefreshStalenessView;

    let database = workspace
        .recall_path()
        .is_file()
        .then(|| recall_engine::db::Database::open(workspace.recall_path()))
        .transpose()?;
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
            let (engine_source_name, materialization_receipt) = match binding {
                WorkspaceBinding::Gmail { account, gmail } => Some((
                    Ok::<_, anyhow::Error>(name.clone()),
                    Some((
                        "email",
                        account.as_str(),
                        gmail.materialization_fingerprint(),
                        Ok(None),
                    )),
                )),
                WorkspaceBinding::GoogleCalendar { account, calendar } => Some((
                    Ok::<_, anyhow::Error>(name.clone()),
                    Some((
                        "gcal",
                        account.as_str(),
                        calendar.materialization_fingerprint(),
                        GoogleCalendarScope::for_selector(calendar, Utc::now()).and_then(|scope| {
                            serde_json::to_string(&scope.as_range())
                                .context("failed to fingerprint Calendar rolling boundary")
                                .map(Some)
                        }),
                    )),
                )),
                WorkspaceBinding::GoogleMeet { account } => Some((
                    Ok::<_, anyhow::Error>(name.clone()),
                    Some((
                        "google_meet",
                        account.as_str(),
                        Ok(GOOGLE_MEET_MATERIALIZATION_FINGERPRINT.to_string()),
                        Ok(None),
                    )),
                )),
                WorkspaceBinding::Granola {
                    account,
                    collection,
                } => Some((
                    Ok::<_, anyhow::Error>(name.clone()),
                    Some((
                        "granola",
                        account.as_str(),
                        collection.materialization_fingerprint(),
                        Ok(None),
                    )),
                )),
                _ => None,
            }?;
            Some(engine_source_name.and_then(|engine_source_name| {
                let materialization_receipt = match materialization_receipt {
                    Some((connector, account, fingerprint, scope)) => {
                        Some((connector, account, fingerprint?, scope?))
                    }
                    None => None,
                };
                Ok((name, engine_source_name, materialization_receipt))
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|(name, engine_source_name, materialization_receipt)| {
            let (last_refresh_ms, mut stale, mut stale_reason) = if let Some(database) = &database {
                let last_refresh_ms = database
                    .get_source_refresh_status(&engine_source_name)?
                    .map(|(_, _, refreshed_at_ms)| refreshed_at_ms);
                let freshness = recall_engine::sqlite_source::source_refresh_staleness(
                    database,
                    &engine_source_name,
                    &workspace.ledger_path(),
                )?;
                (last_refresh_ms, freshness.stale, freshness.reason)
            } else {
                (None, true, Some("never_refreshed".to_string()))
            };
            if let Some((connector, account, expected_fingerprint, expected_scope)) =
                materialization_receipt
            {
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
                            || stored_fingerprint.as_deref()
                                != Some(expected_fingerprint.as_str())
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
    let index_path = workspace.recall_path();
    if !index_path.is_file() {
        return margins_workflows::local_recall::status(workspace);
    }
    let documents = crate::recall_engine_seam::indexed_document_count(&index_path)
        .with_context(|| format!("opening recall status index {}", index_path.display()))?;
    Ok(margins_workflows::local_recall::LocalRecallStatus {
        schema_version: "margins.indexed-recall.v1".to_string(),
        available: true,
        mode: "indexed".to_string(),
        documents,
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

    let db_path = workspace.recall_path();
    let Some(index) = crate::recall_engine_seam::open(&db_path)? else {
        return Ok(empty("unavailable", "not_established", 0));
    };
    let freshness =
        require_retrieval_freshness(snapshot_index_freshness(workspace)?, materialization)?;

    let started = std::time::Instant::now();
    let (status, reason, search_strategy, hits, top_contributing_catalysts) =
        search_index(workspace, &index, query, source_filter)?;

    let results = recall_results_for_workspace(hits, workspace)?;

    Ok(RecallOutput {
        query: query.to_string(),
        status,
        reason,
        freshness,
        config_path: Some(workspace.config_path.clone()),
        // The exact number of indexed documents recall just searched.
        note_count: index.document_count,
        results,
        top_contributing_catalysts,
        search_strategy,
        processing_time: (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
    })
}

fn recall_results(
    hits: Vec<RecallHit>,
    catalog: &BTreeMap<String, CatalogEntry>,
) -> Vec<RecallResult> {
    hits.into_iter()
        .filter_map(|hit| {
            let entry = catalog.get(&hit.path)?.clone();
            Some(RecallResult {
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
        .collect()
}

fn recall_results_for_workspace(
    hits: Vec<RecallHit>,
    workspace: &ResolvedWorkspace,
) -> Result<Vec<RecallResult>> {
    hits.into_iter()
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
        .collect()
}

/// Search primarily through generated thematic bridges, while preserving a
/// bounded literal phrase path across the full declared document boundary.
fn search_index(
    workspace: &ResolvedWorkspace,
    index: &SearchIndex,
    query: &str,
    source_filter: Option<&str>,
) -> Result<(
    &'static str,
    &'static str,
    &'static str,
    Vec<RecallHit>,
    Vec<TopCatalyst>,
)> {
    let search_limit = source_filter
        .map(|_| index.document_count.max(RESULT_LIMIT))
        .unwrap_or(RESULT_LIMIT);
    match index.bridged {
        Bridged::Ready => {
            let response = index.catalyst_search_response(query, search_limit)?;
            let catalyst_hits = response
                .results
                .into_iter()
                .map(|hit| RecallHit {
                    path: hit.file_path,
                    score: hit.similarity,
                    content: hit.content,
                    via_catalyst_id: hit.via_catalyst_id,
                    via_catalyst_text: hit.via_catalyst_text,
                })
                .collect::<Vec<_>>();
            let exact_hits = if is_distinctive_phrase(query) {
                index
                    .exact_phrase_search(query, search_limit)?
                    .into_iter()
                    .filter(|hit| native_markdown_hit(workspace, &hit.path))
                    .map(|hit| RecallHit {
                        path: hit.path,
                        score: hit.score,
                        content: hit.content,
                        via_catalyst_id: None,
                        via_catalyst_text: None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let exact_hits = filter_recall_hits(workspace, exact_hits, source_filter);
            let catalyst_hits = filter_recall_hits(workspace, catalyst_hits, source_filter);
            let hits = merge_exact_and_catalyst_hits(exact_hits, catalyst_hits);
            debug_strategy("catalyst");
            Ok((
                "ok",
                "catalyst",
                "catalyze",
                hits,
                response.top_contributing_catalysts,
            ))
        }
        Bridged::NoGenerator | Bridged::NoEntities => {
            // Reopening an index with zero catalysts reports NoGenerator even
            // when init had a usable generator and simply selected no link
            // entities. Keep the declared notes searchable in that case.
            let corpus = crate::workspace_recall::prepare(workspace, false)?;
            if !corpus.selected_entity_names.is_empty() {
                anyhow::bail!(RECALL_UNAVAILABLE_MESSAGE);
            }
            ensure_usable_generator()?;
            let direct_hits = index
                .direct_search(query, search_limit)?
                .into_iter()
                .map(|hit| RecallHit {
                    path: hit.path,
                    score: hit.score,
                    content: hit.content,
                    via_catalyst_id: None,
                    via_catalyst_text: None,
                })
                .collect::<Vec<_>>();
            let exact_hits = if is_distinctive_phrase(query) {
                index
                    .exact_phrase_search(query, search_limit)?
                    .into_iter()
                    .filter(|hit| native_markdown_hit(workspace, &hit.path))
                    .map(|hit| RecallHit {
                        path: hit.path,
                        score: hit.score,
                        content: hit.content,
                        via_catalyst_id: None,
                        via_catalyst_text: None,
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let hits = merge_exact_and_catalyst_hits(
                filter_recall_hits(workspace, exact_hits, source_filter),
                filter_recall_hits(workspace, direct_hits, source_filter),
            );
            Ok(("ok", "no_entities", "direct", hits, Vec::new()))
        }
    }
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
            let source_id = sqlite_document_ref_source_id(document_ref);
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

fn sqlite_document_ref_source_id(document_ref: &str) -> Option<String> {
    let encoded = document_ref.strip_prefix("sqlite:")?.split_once('/')?.1;
    if encoded.len() % 2 != 0 || !encoded.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|idx| u8::from_str_radix(&encoded[idx..idx + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    String::from_utf8(bytes).ok()
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

fn catalyst_readiness(
    index: &SearchIndex,
    selected_entities: &BTreeSet<String>,
) -> Result<CatalystReadiness> {
    let database = index.database();
    let catalyst_entities = database
        .query_read("SELECT DISTINCT entity FROM catalysts", Vec::new(), |row| {
            row.get::<String>(0)
        })?
        .into_iter()
        .map(|entity| entity.trim().to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let catalysts_present = database
        .query_read("SELECT COUNT(*) FROM catalysts", Vec::new(), |row| {
            row.get::<i64>(0)
        })?
        .into_iter()
        .next()
        .unwrap_or_default()
        .max(0) as usize;
    let pending = selected_entities
        .difference(&catalyst_entities)
        .cloned()
        .collect::<BTreeSet<_>>();
    let latest_skips = latest_generation_skip_reasons(database)?;
    let mut reasons = BTreeMap::new();
    for entity in &pending {
        let reason = latest_skips
            .get(entity)
            .map(String::as_str)
            .unwrap_or("no-occurrence");
        *reasons.entry(reason.to_string()).or_insert(0) += 1;
    }
    Ok(CatalystReadiness {
        entities_curated: selected_entities.len(),
        catalysts_present,
        // Preserve partial recall when at least one selected entity has a
        // catalyst. Suppressed entities remain auditable in `reasons`; a fully
        // suppressed selection is still catalysts_pending.
        items_pending: if catalysts_present > 0 {
            0
        } else {
            pending.len()
        },
        reasons,
    })
}

fn latest_generation_skip_reasons(
    database: &recall_engine::db::Database,
) -> Result<BTreeMap<String, String>> {
    let rows = database.query_read(
        "SELECT entity, reason_code
         FROM catalyst_generation_skips
         ORDER BY created_at_ms DESC, id DESC",
        Vec::new(),
        |row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)),
    )?;
    let mut latest = BTreeMap::new();
    for (entity, reason) in rows {
        latest
            .entry(entity.trim().to_ascii_lowercase())
            .or_insert(reason);
    }
    Ok(latest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitStatus {
    pub status: &'static str,
    pub reason: &'static str,
    pub readiness: CatalystReadiness,
}

fn classify_init_status(bridged: Bridged, readiness: CatalystReadiness) -> InitStatus {
    if readiness.items_pending > 0 {
        return InitStatus {
            status: "catalysts_pending",
            reason: "catalysts_pending",
            readiness,
        };
    }
    match bridged {
        Bridged::Ready => InitStatus {
            status: "ok",
            reason: "catalyst",
            readiness,
        },
        Bridged::NoGenerator => InitStatus {
            status: "catalysts_pending",
            reason: "catalysts_pending",
            readiness,
        },
        Bridged::NoEntities => InitStatus {
            status: "ok",
            reason: "no_entities",
            readiness,
        },
    }
}

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

fn snapshot_unknown_freshness() -> EvidenceFreshness {
    EvidenceFreshness {
        status: FreshnessStatus::NotApplicable,
        stale: false,
        reason: Some("snapshot_freshness_unknown".to_string()),
        last_successful_refresh: None,
    }
}

/// Validate the existing external-source snapshot from Margins-owned refresh
/// metadata without rediscovering or rebuilding the workspace corpus. Native
/// Markdown freshness remains intentionally unknown on this lookup-only path.
fn snapshot_index_freshness(workspace: &ResolvedWorkspace) -> Result<EvidenceFreshness> {
    let sources = workspace_source_refresh_staleness(workspace)?;
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

/// Compare discovered paths and filesystem modification times with the index's
/// per-document indexing timestamps. This reads local metadata and SQLite only;
/// it never invokes indexing, embeddings, generators, credentials, or network.
fn index_freshness(corpus: &WorkspaceCorpus, index: &SearchIndex) -> Result<EvidenceFreshness> {
    let stale = |reason: &str| EvidenceFreshness {
        status: FreshnessStatus::Stale,
        stale: true,
        reason: Some(reason.to_string()),
        last_successful_refresh: None,
    };
    let indexed: BTreeMap<String, i64> = index
        .database()
        .query_read(
            "SELECT source_ref, indexed_at FROM docs",
            Vec::new(),
            |row| Ok((row.get::<String>(0)?, row.get::<i64>(1)?)),
        )?
        .into_iter()
        .collect();
    let mut expected = corpus
        .filesystem_documents
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    expected.extend(
        corpus
            .catalog
            .iter()
            .filter(|(source_ref, entry)| {
                source_ref.starts_with("sqlite:") && source_kind_has_ledger_corpus(entry.kind)
            })
            .map(|(source_ref, _)| source_ref.clone()),
    );
    if indexed.keys().cloned().collect::<BTreeSet<_>>() != expected {
        debug_strategy(&format!(
            "workspace corpus mismatch: expected={expected:?} indexed={:?}",
            indexed.keys().collect::<Vec<_>>()
        ));
        return Ok(stale("document_set_changed"));
    }
    for (source_ref, path) in &corpus.filesystem_documents {
        let refreshed_at = indexed.get(source_ref);
        let Some(refreshed_at) = refreshed_at else {
            return Ok(stale("document_set_changed"));
        };
        if crate::workspace_recall::path_modified_ms(path)
            .is_some_and(|modified| modified > *refreshed_at)
        {
            debug_strategy(&format!(
                "workspace document newer than its refresh: source_ref={source_ref:?} modified_path={} refreshed_at_ms={refreshed_at}",
                path.display()
            ));
            return Ok(stale("native_document_changed_after_index"));
        }
    }
    for (source_name, source_path) in &corpus.sqlite_sources {
        let freshness = recall_engine::sqlite_source::source_refresh_staleness(
            index.database(),
            source_name,
            source_path,
        )?;
        let last_refresh_ms = index
            .database()
            .get_source_refresh_status(source_name)?
            .map(|(_, _, refreshed_at_ms)| refreshed_at_ms);
        if freshness.stale {
            debug_strategy(&format!(
                "workspace SQLite source stale: source={source_name:?} last_refresh_ms={last_refresh_ms:?} stale=true stale_reason={:?}",
                freshness.reason
            ));
            return Ok(stale(
                freshness
                    .reason
                    .as_deref()
                    .unwrap_or("materialization_changed_after_index"),
            ));
        }
    }
    Ok(EvidenceFreshness {
        status: FreshnessStatus::Fresh,
        stale: false,
        reason: None,
        last_successful_refresh: None,
    })
}

fn source_kind_has_ledger_corpus(kind: SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::GoogleMail
            | SourceKind::GoogleCalendar
            | SourceKind::GoogleMeet
            | SourceKind::Granola
    )
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

/// Convenience for the CLI interception: run recall and return the JSON string
/// to print to stdout.
pub fn recall_json(
    workspace: &ResolvedWorkspace,
    query: &str,
    source: Option<&str>,
) -> Result<String> {
    render_recall_json(&recall(workspace, query, source)?)
}

/// A reusable in-process product search handle. Enzyme remains the search
/// engine, but Margins owns result identity and freshness at this boundary.
pub struct SearchHandle {
    index: recall_engine::kernel::SearchIndex,
    selected_entity_names: BTreeSet<String>,
    catalog: BTreeMap<String, CatalogEntry>,
    freshness: RecallFreshness,
}

impl SearchHandle {
    /// Margins-owned typed catalyst result JSON.
    pub fn catalyze_json(&self, query: &str, limit: usize) -> Result<String> {
        let readiness = catalyst_readiness(&self.index, &self.selected_entity_names)?;
        if readiness.items_pending > 0 {
            anyhow::bail!(readiness.message());
        }
        let started = std::time::Instant::now();
        let (reason, search_strategy, hits, top_contributing_catalysts) = match self.index.bridged {
            Bridged::Ready => {
                let response = self.index.catalyst_search_response(query, limit)?;
                (
                    "catalyst",
                    "catalyze",
                    response
                        .results
                        .into_iter()
                        .map(|hit| RecallHit {
                            path: hit.file_path,
                            score: hit.similarity,
                            content: hit.content,
                            via_catalyst_id: hit.via_catalyst_id,
                            via_catalyst_text: hit.via_catalyst_text,
                        })
                        .collect::<Vec<_>>(),
                    response.top_contributing_catalysts,
                )
            }
            Bridged::NoGenerator | Bridged::NoEntities => {
                if !self.selected_entity_names.is_empty() {
                    anyhow::bail!(RECALL_UNAVAILABLE_MESSAGE);
                }
                ensure_usable_generator()?;
                let hits = self
                    .index
                    .direct_search(query, limit)?
                    .into_iter()
                    .map(|hit| RecallHit {
                        path: hit.path,
                        score: hit.score,
                        content: hit.content,
                        via_catalyst_id: None,
                        via_catalyst_text: None,
                    })
                    .collect();
                ("no_entities", "direct", hits, Vec::new())
            }
        };
        let output = RecallOutput {
            query: query.to_string(),
            status: "ok",
            reason,
            freshness: self.freshness.clone(),
            config_path: None,
            note_count: self.index.document_count,
            results: recall_results(hits, &self.catalog),
            top_contributing_catalysts,
            search_strategy,
            processing_time: (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        };
        render_recall_json(&output)
    }

    /// `enzyme petri`-compatible JSON (consumed-field subset).
    pub fn petri_json(&self, query: &str, top: usize, catalyst_budget: usize) -> Result<String> {
        let readiness = catalyst_readiness(&self.index, &self.selected_entity_names)?;
        if readiness.items_pending > 0 {
            anyhow::bail!(readiness.message());
        }
        self.index.petri_json(query, top, catalyst_budget)
    }
}

/// Build or refresh the unified index for every declared source in a Workspace.
pub fn provision_workspace(workspace: &ResolvedWorkspace) -> Result<SearchHandle> {
    Ok(provision_workspace_inner(workspace)?.0)
}

pub fn provision_workspace_for_init(workspace: &ResolvedWorkspace) -> Result<InitStatus> {
    Ok(provision_workspace_inner(workspace)?.1)
}

pub fn open_workspace(workspace: &ResolvedWorkspace) -> Result<Option<SearchHandle>> {
    let corpus = crate::workspace_recall::prepare(workspace, false)?;
    let Some(index) = crate::recall_engine_seam::open(&workspace.recall_path())? else {
        return Ok(None);
    };
    let freshness = require_retrieval_freshness(
        index_freshness(&corpus, &index)?,
        materialization_freshness(workspace)?,
    )?;
    ensure_usable_generator()?;
    let readiness = catalyst_readiness(&index, &corpus.selected_entity_names)?;
    if readiness.items_pending > 0 {
        anyhow::bail!(readiness.message());
    }
    Ok(Some(SearchHandle {
        index,
        selected_entity_names: corpus.selected_entity_names,
        catalog: corpus.catalog,
        freshness,
    }))
}

fn provision_workspace_inner(workspace: &ResolvedWorkspace) -> Result<(SearchHandle, InitStatus)> {
    std::fs::create_dir_all(&workspace.state_dir)
        .with_context(|| format!("creating workspace state {}", workspace.state_dir.display()))?;
    let corpus = crate::workspace_recall::prepare(workspace, true)?;
    let provisioned = crate::recall_engine_seam::provision(
        &corpus.engine,
        crate::recall_engine_seam::ProvisionRequest {
            max_bridged_entities: MAX_BRIDGED_ENTITIES
                .max(corpus.entity_selection_debug.selected.len()),
            selected_link_entities: corpus.selected_link_entities.clone(),
            excluded_link_entities: corpus.excluded_link_entities.clone(),
            generator_home: margins_home()?,
        },
    )?;
    let hosted_generation_calls = provisioned.hosted_generation_calls;
    let index = provisioned.index;
    let catalysts_after = catalyst_ids_from_index(&index)?;
    let materialization = entity_materialization_debug(&corpus, &index, &catalysts_after)?;
    let readiness = catalyst_readiness(&index, &corpus.selected_entity_names)?;
    let status = classify_init_status(index.bridged, readiness);
    debug_strategy(&format!(
        "workspace index provisioned ({:?})",
        index.bridged
    ));
    debug_correspondent_selection(&corpus, hosted_generation_calls, Some(&materialization));
    let freshness = combined_freshness(
        index_freshness(&corpus, &index)?,
        materialization_freshness(workspace)?,
    );
    Ok((
        SearchHandle {
            index,
            selected_entity_names: corpus.selected_entity_names.clone(),
            catalog: corpus.catalog.clone(),
            freshness,
        },
        status,
    ))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EntityMaterializationDebug {
    materialized_count: usize,
    selected_count: usize,
    non_materialized: Vec<(String, String)>,
}

fn entity_materialization_debug(
    corpus: &WorkspaceCorpus,
    index: &SearchIndex,
    catalysts: &BTreeMap<String, BTreeSet<String>>,
) -> Result<EntityMaterializationDebug> {
    let occurrences = index
        .database()
        .query_read(
            "SELECT DISTINCT e.name
             FROM entities e
             JOIN entity_occurrences eo ON eo.entity_id = e.id
             WHERE e.type = 'link'",
            Vec::new(),
            |row| row.get::<String>(0),
        )?
        .into_iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    Ok(entity_materialization_debug_from_sets(
        &corpus.selected_link_entities,
        &occurrences,
        &catalysts.keys().cloned().collect(),
        &latest_generation_skip_reasons(index.database())?,
    ))
}

fn entity_materialization_debug_from_sets(
    selected: &BTreeSet<String>,
    occurrences: &BTreeSet<String>,
    materialized: &BTreeSet<String>,
    generation_skips: &BTreeMap<String, String>,
) -> EntityMaterializationDebug {
    let non_materialized = selected
        .difference(materialized)
        .map(|entity| {
            let reason = if let Some(reason) = generation_skips.get(entity) {
                reason.clone()
            } else if occurrences.contains(entity) {
                "no-catalyst".to_string()
            } else {
                "no-occurrence".to_string()
            };
            (entity.clone(), reason)
        })
        .collect();
    EntityMaterializationDebug {
        materialized_count: selected.intersection(materialized).count(),
        selected_count: selected.len(),
        non_materialized,
    }
}

fn debug_correspondent_selection(
    corpus: &WorkspaceCorpus,
    hosted_generation_calls: usize,
    materialization: Option<&EntityMaterializationDebug>,
) {
    debug_strategy(&correspondent_selection_debug_message(
        &corpus.entity_selection_debug,
        hosted_generation_calls,
        materialization,
    ));
}

fn correspondent_selection_debug_message(
    debug: &crate::workspace_recall::EntitySelectionDebug,
    hosted_generation_calls: usize,
    materialization: Option<&EntityMaterializationDebug>,
) -> String {
    let selected = debug
        .selected
        .iter()
        .map(|(entity, weight)| match weight {
            Some(weight) => format!("{entity}={weight}"),
            None => format!("{entity}=selected"),
        })
        .collect::<Vec<_>>()
        .join(",");
    let (materialized, non_materialized) = materialization.map_or_else(
        || ("unavailable".to_string(), String::new()),
        |materialization| {
            let missing = materialization
                .non_materialized
                .iter()
                .map(|(entity, reason)| format!("[[{entity}]]={reason}"))
                .collect::<Vec<_>>()
                .join(",");
            (
                format!(
                    "{}/{}",
                    materialization.materialized_count, materialization.selected_count
                ),
                missing,
            )
        },
    );
    format!(
        "correspondent_selection selected=[{selected}] suppressed={} correspondents_considered={} hosted_generation_calls={hosted_generation_calls} materialized={materialized} non_materialized=[{non_materialized}]",
        debug.suppressed_count,
        debug.correspondents_considered,
    )
}

fn catalyst_ids_from_index(index: &SearchIndex) -> Result<BTreeMap<String, BTreeSet<String>>> {
    catalyst_ids(index.database())
}

fn catalyst_ids(
    database: &recall_engine::db::Database,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut result = BTreeMap::new();
    for catalyst in database.get_all_catalysts()? {
        result
            .entry(catalyst.entity)
            .or_insert_with(BTreeSet::new)
            .insert(catalyst.id);
    }
    Ok(result)
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

    #[test]
    fn correspondent_debug_line_reports_considered_counts() {
        let debug = crate::workspace_recall::EntitySelectionDebug {
            selected: vec![("[[dk@example.test]]".to_string(), Some(10_002_007))],
            suppressed_count: 52,
            correspondents_considered: 41,
        };
        let materialization = EntityMaterializationDebug {
            materialized_count: 1,
            selected_count: 2,
            non_materialized: vec![(
                "missing@example.test".to_string(),
                "no-occurrence".to_string(),
            )],
        };
        assert_eq!(
            correspondent_selection_debug_message(&debug, 1, Some(&materialization)),
            "correspondent_selection selected=[[[dk@example.test]]=10002007] suppressed=52 correspondents_considered=41 hosted_generation_calls=1 materialized=1/2 non_materialized=[[[missing@example.test]]=no-occurrence]"
        );
    }

    #[test]
    fn materialization_debug_distinguishes_missing_occurrences_from_generation_gaps() {
        let selected = BTreeSet::from([
            "dk@example.test".to_string(),
            "generated@example.test".to_string(),
            "missing@example.test".to_string(),
            "ungenerated@example.test".to_string(),
        ]);
        let occurrences = BTreeSet::from([
            "generated@example.test".to_string(),
            "ungenerated@example.test".to_string(),
        ]);
        let materialized = BTreeSet::from(["generated@example.test".to_string()]);
        let generation_skips =
            BTreeMap::from([("dk@example.test".to_string(), "thin_evidence".to_string())]);
        assert_eq!(
            entity_materialization_debug_from_sets(
                &selected,
                &occurrences,
                &materialized,
                &generation_skips,
            ),
            EntityMaterializationDebug {
                materialized_count: 1,
                selected_count: 4,
                non_materialized: vec![
                    ("dk@example.test".to_string(), "thin_evidence".to_string()),
                    (
                        "missing@example.test".to_string(),
                        "no-occurrence".to_string(),
                    ),
                    (
                        "ungenerated@example.test".to_string(),
                        "no-catalyst".to_string(),
                    ),
                ],
            }
        );
    }

    #[test]
    fn authoritative_link_selection_prunes_stale_and_suppressed_catalysts_only() {
        recall_engine::initialize_sqlite_runtime().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("index.db");
        drop(recall_engine::db::Database::open(&db_path).unwrap());
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        for (id, entity, entity_type) in [
            ("keep", "dk@example.test", "link"),
            ("stale", "newsletter@example.test", "link"),
            ("self", "owner@example.test", "link"),
            ("folder", "people", "folder"),
        ] {
            connection
                .execute(
                    "INSERT INTO catalysts (id, text, entity, metadata)
                     VALUES (?1, 'fixture', ?2, json_object('entity_type', ?3))",
                    rusqlite::params![id, entity, entity_type],
                )
                .unwrap();
        }
        drop(connection);

        crate::recall_engine_seam::reconcile_link_catalysts(
            &recall_engine::db::Database::open(&db_path).unwrap(),
            &BTreeSet::from(["dk@example.test".to_string()]),
            &BTreeSet::from(["owner@example.test".to_string()]),
        )
        .unwrap();

        let connection = rusqlite::Connection::open(&db_path).unwrap();
        let mut statement = connection
            .prepare("SELECT entity FROM catalysts ORDER BY entity")
            .unwrap();
        let entities = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(entities, vec!["dk@example.test", "people"]);
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
    fn hosted_generator_uses_setup_bundle_without_broker_or_bootstrap_access() {
        let _guard = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture();
        clear_resolver_env();
        let home = tempfile::tempdir().unwrap();
        crate::hosted_credentials::install_bundle(
            home.path(),
            "setup-machine-id",
            "fixture-cached-key",
            "https://fixture.invalid/v1",
            "fixture-cached-model",
            Some(4_102_444_800),
        )
        .unwrap();
        std::fs::write(home.path().join("bootstrap.json"), b"not valid json").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::env::set_var(
            "ENZYME_FREE_CONFIG_URL",
            format!("http://{}/llm/free-config", listener.local_addr().unwrap()),
        );

        let generator = crate::recall_engine_seam::resolve_generator(home.path()).unwrap();

        assert_eq!(generator.model(), "fixture-cached-model");
        assert!(matches!(
            generator.provider(),
            recall_engine::llm::ConfigProvider::Env
        ));
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[cfg(feature = "recall-local-model")]
    #[tokio::test]
    async fn configured_local_mode_wins_without_env_credentials() {
        let _guard = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture();
        clear_resolver_env();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("models")).unwrap();
        std::fs::write(
            home.path().join("models/configured-catalyst.gguf"),
            b"selection-fixture",
        )
        .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[llm]\nmode = \"local\"\nlocal_model = \"configured-catalyst\"\n",
        )
        .unwrap();

        let config = recall_engine::llm::resolve_api_config_in(home.path())
            .await
            .unwrap();

        assert!(matches!(
            config.provider,
            recall_engine::llm::ConfigProvider::Local
        ));
        assert_eq!(config.model, "configured-catalyst");
        assert!(!home.path().join("bootstrap.json").exists());
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
        let readiness = CatalystReadiness {
            entities_curated: 2,
            catalysts_present: 6,
            items_pending: 0,
            reasons: BTreeMap::new(),
        };
        assert_eq!(
            classify_init_status(Bridged::Ready, readiness.clone()),
            InitStatus {
                status: "ok",
                reason: "catalyst",
                readiness: readiness.clone(),
            }
        );
        assert_eq!(
            classify_init_status(
                Bridged::NoGenerator,
                CatalystReadiness {
                    items_pending: 2,
                    ..readiness.clone()
                }
            ),
            InitStatus {
                status: "catalysts_pending",
                reason: "catalysts_pending",
                readiness: CatalystReadiness {
                    items_pending: 2,
                    ..readiness.clone()
                },
            }
        );
        assert_eq!(
            classify_init_status(
                Bridged::NoEntities,
                CatalystReadiness {
                    entities_curated: 0,
                    catalysts_present: 0,
                    items_pending: 0,
                    reasons: BTreeMap::new(),
                }
            ),
            InitStatus {
                status: "ok",
                reason: "no_entities",
                readiness: CatalystReadiness {
                    entities_curated: 0,
                    catalysts_present: 0,
                    items_pending: 0,
                    reasons: BTreeMap::new(),
                },
            }
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
    fn snapshot_hydration_reconstructs_external_evidence_without_catalog_walk() {
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
            margins_workflows::workspace_lowering::sqlite_document_ref("mail", "thread-123");

        let entry = catalog_entry_for_document_ref(&workspace, &document_ref).unwrap();

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
