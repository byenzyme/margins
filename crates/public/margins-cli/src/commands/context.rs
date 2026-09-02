//! Read-only `margins context` evidence retrieval.

use crate::error::CliError;
use crate::output::line;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use margins_workflows::integrations::{
    EvidenceFreshness, EvidenceHandle, FreshnessStatus, PersonIdentity,
    GOOGLE_MEET_MATERIALIZATION_FINGERPRINT,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const SCHEMA_VERSION: &str = "margins.context.v2";
const MAX_EPISODES: usize = 100;
const MAX_EXCERPT_CHARS: usize = 600;
const MAX_IDENTITY_EVIDENCE: usize = 5;
const MEETING_DEDUPE_WINDOW_SECS: i64 = 5 * 60;

#[derive(Debug, Serialize)]
struct ContextOutput {
    schema_version: &'static str,
    query: ContextQuery,
    meeting: Option<MeetingRef>,
    resolved_identities: Vec<ResolvedIdentity>,
    identity_omissions: Vec<IdentityOmission>,
    episodes: Vec<Episode>,
    open_items: Vec<OpenItem>,
    source_manifest: SourceManifest,
}

#[derive(Debug, Serialize)]
struct ContextQuery {
    kind: &'static str,
    value: String,
    cutoff: String,
}

#[derive(Debug, Serialize)]
struct MeetingRef {
    id: String,
    title: Option<String>,
    starts_at: String,
    source: &'static str,
    provenance: Provenance,
}

#[derive(Debug, Clone, Serialize)]
struct ResolvedIdentity {
    id: String,
    display_name: String,
    aliases: Vec<String>,
    emails: Vec<String>,
    total_evidence_count: usize,
    resolution_evidence: Vec<IdentityEvidence>,
}

#[derive(Debug, Clone, Serialize)]
struct IdentityEvidence {
    kind: &'static str,
    value: String,
    provenance: Provenance,
}

#[derive(Debug, Serialize)]
struct IdentityOmission {
    query: String,
    reason: &'static str,
    candidates: Vec<OmissionCandidate>,
}

#[derive(Debug, Serialize)]
struct OmissionCandidate {
    display_name: String,
    provenance: Provenance,
}

#[derive(Debug, Clone, Serialize)]
struct Episode {
    id: String,
    kind: String,
    occurred_at: String,
    provenance: Provenance,
    evidence: EvidenceHandle,
    excerpt: String,
}

#[derive(Debug, Serialize)]
struct OpenItem {
    id: String,
    text: String,
    status: &'static str,
    episode_id: String,
    provenance: Provenance,
}

#[derive(Debug, Clone, Serialize)]
struct Provenance {
    source: String,
    source_id: String,
    evidence: EvidenceHandle,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<String>,
}

#[derive(Debug, Serialize)]
struct SourceManifest {
    generated_at: String,
    freshness: EvidenceFreshness,
    sources: Vec<ManifestSource>,
}

#[derive(Debug, Serialize)]
struct ManifestSource {
    id: &'static str,
    kind: &'static str,
    freshness: EvidenceFreshness,
    item_count: usize,
}

#[derive(Debug, Clone)]
struct RecallDocument {
    id: i64,
    source_ref: String,
    title: Option<String>,
    content: String,
    created_at: i64,
    modified_at: i64,
    indexed_at: i64,
    metadata: JsonValue,
    links: Vec<String>,
}

#[derive(Debug, Clone)]
struct MeetingRecord {
    id: String,
    source: MeetingSource,
    starts_at: DateTime<Utc>,
    title: Option<String>,
    note_path: Option<String>,
    people: Vec<MeetingAttendee>,
    evidence: EvidenceHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingSource {
    Session,
    CalendarEvent,
    ExternalDocument,
}

impl MeetingSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::CalendarEvent => "calendar_event",
            Self::ExternalDocument => "external_document",
        }
    }

    fn provenance_source(self) -> &'static str {
        match self {
            Self::Session => "session_index",
            Self::CalendarEvent | Self::ExternalDocument => "integrations_ledger",
        }
    }
}

#[derive(Debug, Clone)]
struct MeetingAttendee {
    position: usize,
    display_name: String,
    email: Option<String>,
    source: MeetingSource,
    meeting_id: String,
    evidence: EvidenceHandle,
}

#[derive(Debug, Clone)]
struct IntegrationEpisode {
    connector_id: String,
    source_account: String,
    source_id: String,
    occurred_at: DateTime<Utc>,
    kind: String,
    body: String,
    people: Vec<PersonIdentity>,
    source_href: Option<String>,
}

impl IntegrationEpisode {
    fn ledger_id(&self) -> String {
        format!(
            "{}:{}:{}",
            self.connector_id, self.source_account, self.source_id
        )
    }

    fn context_episode_id(&self) -> String {
        format!("{}:{}", self.connector_id, self.source_id)
    }

    fn evidence_anchor(&self) -> &'static str {
        match self.connector_id.as_str() {
            "email" => "thread_evidence",
            "gcal" => "calendar_event_evidence",
            "google_meet" => "external_document_evidence",
            other => unreachable!("unsupported integration connector {other}"),
        }
    }

    fn identity_anchor(&self, position: usize) -> String {
        match self.connector_id.as_str() {
            "email" => format!("participant_threads:{position}"),
            "gcal" => format!("calendar_event_attendees:{position}"),
            "google_meet" => {
                format!("external_document_participants:{position}")
            }
            other => unreachable!("unsupported integration connector {other}"),
        }
    }

    fn evidence_handle(&self) -> EvidenceHandle {
        EvidenceHandle::ExternalRecord {
            connector_id: self.connector_id.clone(),
            source_account: self.source_account.clone(),
            source_id: self.source_id.clone(),
            href: self.source_href.clone(),
        }
    }
}

impl MeetingAttendee {
    fn resolution_query(&self) -> &str {
        self.email.as_deref().unwrap_or(&self.display_name)
    }

    fn identity_values(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.display_name.as_str()).chain(self.email.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone)]
struct PersonNote {
    doc_id: i64,
    source_ref: String,
    absolute_path: String,
    display_name: String,
    aliases: Vec<String>,
    emails: Vec<String>,
}

#[derive(Debug)]
struct RecallStore {
    path: PathBuf,
    documents: Vec<RecallDocument>,
    last_indexed_at: i64,
}

type IntegrationKey = (String, String);

#[derive(Debug, Clone, Default)]
struct ConfiguredIntegration {
    materialization_fingerprint: Option<String>,
    scope: Option<margins_workflows::integrations::SurveyRange>,
    external_occurred_from: Option<DateTime<Utc>>,
}

type ConfiguredIntegrations = BTreeMap<IntegrationKey, ConfiguredIntegration>;

pub fn run(
    state_dir: &Path,
    home_dir: &Path,
    people_folder: &str,
    person: Option<&str>,
    meeting: Option<&str>,
    cutoff: Option<&str>,
    json: bool,
    now: DateTime<Local>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    if !json {
        return Err(CliError::usage("margins context v2 requires --json"));
    }

    let store = load_recall_store(state_dir)?;
    ensure_store_fresh(home_dir, &store)?;
    let (sessions, session_path) = load_sessions_read_only(state_dir)?;
    let configured_integrations = configured_integration_keys(state_dir)?;
    let (calendar_meetings, integration_episodes, integrations_path) =
        load_integration_episodes_read_only(state_dir, configured_integrations.as_ref())?;
    let integration_count = integration_episodes.len();
    let meetings = merge_meeting_sources(&sessions, calendar_meetings);
    let people_notes = people_notes(home_dir, people_folder, &store.documents);
    let now_utc = now.with_timezone(&Utc);

    let (query, selected_meeting, identities, omissions, effective_cutoff) = if let Some(
        person_query,
    ) = person
    {
        let cutoff = parse_cutoff(cutoff, now_utc)?;
        let identity = resolve_person(
            person_query,
            &people_notes,
            &sessions,
            &integration_episodes,
            ResolutionMode::Direct,
        )?
        .resolved
        .ok_or_else(|| identity_not_found(person_query))?;
        (
            ContextQuery {
                kind: "person",
                value: person_query.to_string(),
                cutoff: cutoff.to_rfc3339(),
            },
            None,
            vec![identity],
            Vec::new(),
            cutoff,
        )
    } else {
        let requested = meeting
            .ok_or_else(|| CliError::usage("exactly one of --person or --meeting is required"))?;
        let selected = select_meeting(requested, &meetings, now_utc)?;
        let mut identities = Vec::new();
        let mut omissions = Vec::new();
        for attendee in &selected.people {
            match resolve_person(
                attendee.resolution_query(),
                &people_notes,
                &sessions,
                &integration_episodes,
                ResolutionMode::Meeting,
            )? {
                ResolutionResult {
                    resolved: Some(identity),
                    ..
                } => push_identity(&mut identities, identity),
                ResolutionResult { candidates, .. } => omissions.push(IdentityOmission {
                    query: attendee.display_name.clone(),
                    reason: if candidates.is_empty() {
                        "unresolved_identity"
                    } else {
                        "ambiguous_identity"
                    },
                    candidates,
                }),
            }
        }
        if identities.is_empty() {
            return Err(CliError::new(
                    "context_identity_unresolved",
                    format!(
                        "Meeting '{}' has no attendee identity that can be resolved without guessing. Add a full name, email, or people-note alias.",
                        selected.id
                    ),
                ));
        }
        let meeting_ref = MeetingRef {
            id: selected.id.clone(),
            title: selected.title.clone(),
            starts_at: selected.starts_at.to_rfc3339(),
            source: selected.source.as_str(),
            provenance: meeting_provenance(
                selected,
                Some(match selected.source {
                    MeetingSource::Session => "sessions",
                    MeetingSource::CalendarEvent => "calendar_event_evidence",
                    MeetingSource::ExternalDocument => "external_document_evidence",
                }),
            ),
        };
        (
            ContextQuery {
                kind: "meeting",
                value: requested.to_string(),
                cutoff: selected.starts_at.to_rfc3339(),
            },
            Some(meeting_ref),
            identities,
            omissions,
            selected.starts_at,
        )
    };
    let mut episodes = collect_episodes(
        home_dir,
        &store.documents,
        &sessions,
        session_path.as_deref(),
        &integration_episodes,
        &identities,
        effective_cutoff,
        selected_meeting.as_ref().map(|meeting| meeting.id.as_str()),
    );
    episodes.sort_by(|left, right| {
        right
            .occurred_at
            .cmp(&left.occurred_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    episodes.truncate(MAX_EPISODES);
    let open_items = derive_open_items(&episodes, &store.documents, home_dir);

    let recall_count = episodes
        .iter()
        .filter(|episode| episode.provenance.source != "session_index")
        .count();
    let session_count = episodes.len().saturating_sub(recall_count);
    let mut sources = vec![ManifestSource {
        id: "margins_recall",
        kind: "recall_index",
        freshness: fresh_at(DateTime::from_timestamp_millis(store.last_indexed_at)),
        item_count: recall_count,
    }];
    if let Some(path) = session_path {
        sources.push(ManifestSource {
            id: "margins_sessions",
            kind: "session_index",
            freshness: fresh_at(file_modified_datetime(&path)),
            item_count: session_count,
        });
    }
    if let Some(path) = integrations_path {
        sources.push(ManifestSource {
            id: "margins_integrations",
            kind: "integrations_ledger",
            freshness: integration_materialization_freshness(
                &path,
                configured_integrations.as_ref(),
            )?,
            item_count: integration_count,
        });
    }

    let least_fresh_source = sources
        .iter()
        .max_by_key(|source| freshness_severity(source.freshness.status));
    let manifest_status = least_fresh_source
        .map(|source| source.freshness.status)
        .unwrap_or(FreshnessStatus::Fresh);
    let manifest_freshness = EvidenceFreshness {
        status: manifest_status,
        stale: manifest_status != FreshnessStatus::Fresh,
        reason: least_fresh_source.and_then(|source| source.freshness.reason.clone()),
        last_successful_refresh: sources
            .iter()
            .filter_map(|source| source.freshness.last_successful_refresh)
            .max(),
    };

    let output = ContextOutput {
        schema_version: SCHEMA_VERSION,
        query,
        meeting: selected_meeting,
        resolved_identities: identities,
        identity_omissions: omissions,
        episodes,
        open_items,
        source_manifest: SourceManifest {
            generated_at: now_utc.to_rfc3339(),
            freshness: manifest_freshness,
            sources,
        },
    };
    let encoded = serde_json::to_string_pretty(&output)
        .map_err(|error| CliError::new("context_output_failed", error.to_string()))?;
    line(stdout, format_args!("{encoded}")).map_err(CliError::from_anyhow)
}

pub(crate) fn meeting_exists_read_only(
    work_dir: &Path,
    meeting_id: &str,
) -> Result<bool, CliError> {
    let path = work_dir.join("captures/.margins/sessions.sqlite");
    if !path.is_file() {
        return Ok(false);
    }
    let conn = open_read_only(&path, "context_session_store_invalid")?;
    conn.query_row(
        "SELECT 1 FROM sessions WHERE name = ?1 AND COALESCE(lifecycle_state, 'active') = 'active'",
        [meeting_id],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(|error| context_db_error("context_session_store_invalid", &path, error))
}

fn load_recall_store(work_dir: &Path) -> Result<RecallStore, CliError> {
    let path = work_dir.join("index.db");
    if !path.is_file() {
        return Err(CliError::new(
            "context_store_missing",
            format!(
                "Margins context store is missing at '{}'. Run `margins init` (or refresh the desktop recall index) before requesting context.",
                path.display()
            ),
        ));
    }
    let conn = open_read_only(&path, "context_store_invalid")?;
    for table in ["docs", "doc_links"] {
        let exists = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| context_db_error("context_store_invalid", &path, error))?
            .is_some();
        if !exists {
            return Err(CliError::new(
                "context_store_invalid",
                format!(
                    "Margins context store '{}' is not a compatible product recall index (missing {table}).",
                    path.display()
                ),
            ));
        }
    }

    let mut statement = conn
        .prepare(
            "SELECT id, source_ref, title, COALESCE(content, ''), created_at, modified_at, indexed_at, COALESCE(metadata, '{}') FROM docs ORDER BY id",
        )
        .map_err(|error| context_db_error("context_store_invalid", &path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(|error| context_db_error("context_store_invalid", &path, error))?;
    let mut documents = Vec::new();
    for row in rows {
        let (id, source_ref, title, content, created_at, modified_at, indexed_at, metadata) =
            row.map_err(|error| context_db_error("context_store_invalid", &path, error))?;
        let mut links_stmt = conn
            .prepare("SELECT link FROM doc_links WHERE doc_id = ?1 ORDER BY link")
            .map_err(|error| context_db_error("context_store_invalid", &path, error))?;
        let links = links_stmt
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(|error| context_db_error("context_store_invalid", &path, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| context_db_error("context_store_invalid", &path, error))?;
        documents.push(RecallDocument {
            id,
            source_ref,
            title,
            content,
            created_at,
            modified_at,
            indexed_at,
            metadata: serde_json::from_str(&metadata).unwrap_or(JsonValue::Null),
            links,
        });
    }
    let last_indexed_at = documents
        .iter()
        .map(|document| document.indexed_at)
        .max()
        .unwrap_or(0);
    if documents.is_empty() || last_indexed_at <= 0 {
        return Err(CliError::new(
            "context_store_stale",
            format!(
                "Margins context store '{}' has no indexed documents. Refresh the product recall index before requesting context.",
                path.display()
            ),
        ));
    }
    Ok(RecallStore {
        path,
        documents,
        last_indexed_at,
    })
}

fn ensure_store_fresh(work_dir: &Path, store: &RecallStore) -> Result<(), CliError> {
    let mut stale = Vec::new();
    for document in &store.documents {
        let path = absolute_source_path(work_dir, &document.source_ref);
        let Some(modified) = file_modified_millis(&path) else {
            continue;
        };
        if modified > document.modified_at.saturating_add(1_000) {
            stale.push(document.source_ref.clone());
            if stale.len() == 3 {
                break;
            }
        }
    }
    if stale.is_empty() {
        return Ok(());
    }
    Err(CliError::new(
        "context_store_stale",
        format!(
            "Margins context store '{}' is stale: source files changed after indexing ({}). Refresh the product recall index before requesting context.",
            store.path.display(),
            stale.join(", ")
        ),
    ))
}

fn load_sessions_read_only(
    work_dir: &Path,
) -> Result<(Vec<MeetingRecord>, Option<PathBuf>), CliError> {
    let path = work_dir.join("captures/.margins/sessions.sqlite");
    if !path.is_file() {
        return Ok((Vec::new(), None));
    }
    let conn = open_read_only(&path, "context_session_store_invalid")?;
    let mut statement = conn
        .prepare(
            "SELECT s.name, s.start_time, COALESCE(s.title, e.title), s.vault_note_path
             FROM sessions s
             LEFT JOIN session_calendar_events e ON e.session_name = s.name
             WHERE COALESCE(s.lifecycle_state, 'active') = 'active'
             ORDER BY s.start_time DESC, s.name",
        )
        .map_err(|error| context_db_error("context_session_store_invalid", &path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| context_db_error("context_session_store_invalid", &path, error))?;
    let mut sessions = Vec::new();
    for row in rows {
        let (id, starts_at, title, note_path) =
            row.map_err(|error| context_db_error("context_session_store_invalid", &path, error))?;
        let starts_at = DateTime::parse_from_rfc3339(&starts_at)
            .map_err(|_| {
                CliError::new(
                    "context_session_store_invalid",
                    format!("Session '{id}' has invalid start_time '{starts_at}'."),
                )
            })?
            .with_timezone(&Utc);
        let evidence = note_path
            .as_deref()
            .map(|note| EvidenceHandle::NativeMarkdown {
                path: path_from_store(work_dir, note)
                    .to_string_lossy()
                    .into_owned(),
            })
            .unwrap_or_else(|| EvidenceHandle::SessionRecord {
                session_id: id.clone(),
            });
        let mut people_stmt = conn
            .prepare(
                "SELECT position, person FROM session_people WHERE session_name = ?1 ORDER BY position",
            )
            .map_err(|error| context_db_error("context_session_store_invalid", &path, error))?;
        let people = people_stmt
            .query_map([&id], |row| {
                Ok(MeetingAttendee {
                    position: row.get::<_, usize>(0)?,
                    display_name: row.get::<_, String>(1)?,
                    email: None,
                    source: MeetingSource::Session,
                    meeting_id: id.clone(),
                    evidence: evidence.clone(),
                })
            })
            .map_err(|error| context_db_error("context_session_store_invalid", &path, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| context_db_error("context_session_store_invalid", &path, error))?;
        sessions.push(MeetingRecord {
            id,
            source: MeetingSource::Session,
            starts_at,
            title,
            note_path,
            people,
            evidence,
        });
    }
    Ok((sessions, Some(path)))
}

fn load_integration_episodes_read_only(
    work_dir: &Path,
    configured: Option<&ConfiguredIntegrations>,
) -> Result<(Vec<MeetingRecord>, Vec<IntegrationEpisode>, Option<PathBuf>), CliError> {
    if configured.is_some_and(BTreeMap::is_empty) {
        return Ok((Vec::new(), Vec::new(), None));
    }
    let path = work_dir.join("ledger.db");
    if !path.is_file() {
        return Ok((Vec::new(), Vec::new(), None));
    }
    let conn = open_read_only(&path, "context_integrations_store_invalid")?;
    let retrieval_configured = configured;
    let (meetings, mut episodes) =
        load_calendar_integration_episodes(&conn, &path, retrieval_configured)?;
    let (external_meetings, external_episodes) =
        load_external_document_episodes(&conn, &path, retrieval_configured)?;
    let meetings = merge_meeting_sources(&meetings, external_meetings);
    episodes.extend(external_episodes);
    episodes.extend(load_email_integration_episodes(
        &conn,
        &path,
        retrieval_configured,
    )?);
    Ok((meetings, episodes, Some(path)))
}

fn configured_integration_keys(
    state_dir: &Path,
) -> Result<Option<ConfiguredIntegrations>, CliError> {
    let is_workspace_state = state_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        == Some("workspaces");
    if !is_workspace_state {
        return Ok(None);
    }
    let workspace = margins_workflows::workspace::resolve_state_dir(state_dir)
        .map_err(CliError::from_anyhow)?;
    let mut keys = BTreeMap::new();
    for binding in workspace.config.bindings.values() {
        use margins_workflows::workspace::WorkspaceBinding;
        let entry = match binding {
            WorkspaceBinding::Gmail { account, gmail } => Some((
                ("email".to_string(), account.clone()),
                ConfiguredIntegration {
                    materialization_fingerprint: Some(
                        gmail
                            .materialization_fingerprint()
                            .map_err(CliError::from_anyhow)?,
                    ),
                    scope: None,
                    external_occurred_from: None,
                },
            )),
            WorkspaceBinding::GoogleCalendar { account, calendar } => {
                let scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
                    calendar,
                    Utc::now(),
                )
                .map_err(CliError::from_anyhow)?
                .as_range();
                Some((
                    ("gcal".to_string(), account.clone()),
                    ConfiguredIntegration {
                        materialization_fingerprint: Some(
                            calendar
                                .materialization_fingerprint()
                                .map_err(CliError::from_anyhow)?,
                        ),
                        scope: Some(scope),
                        external_occurred_from: None,
                    },
                ))
            }
            WorkspaceBinding::GoogleMeet { account } => Some((
                ("google_meet".to_string(), account.clone()),
                ConfiguredIntegration {
                    materialization_fingerprint: Some(
                        GOOGLE_MEET_MATERIALIZATION_FINGERPRINT.to_string(),
                    ),
                    scope: None,
                    external_occurred_from: None,
                },
            )),
            WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. } => None,
        };
        if let Some((key, configuration)) = entry {
            keys.insert(key, configuration);
        }
    }
    Ok(Some(keys))
}

fn integration_is_configured(
    configured: Option<&ConfiguredIntegrations>,
    connector_id: &str,
    source_account: &str,
) -> bool {
    configured.is_none_or(|configured| {
        configured.contains_key(&(connector_id.to_string(), source_account.to_string()))
    })
}

fn load_calendar_integration_episodes(
    conn: &Connection,
    path: &Path,
    configured: Option<&ConfiguredIntegrations>,
) -> Result<(Vec<MeetingRecord>, Vec<IntegrationEpisode>), CliError> {
    let mut statement = conn
        .prepare(
            "SELECT connector_id, source_account, source_id, occurred_from,
                    title, body_text, href
             FROM calendar_event_evidence
             WHERE connector_id = 'gcal' AND tombstoned_at IS NULL
             ORDER BY occurred_from, source_account, source_id",
        )
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let mut meetings = Vec::new();
    let mut episodes = Vec::new();
    for row in rows {
        let (connector_id, source_account, source_id, occurred_from, title, body, source_href) =
            row.map_err(|error| {
                context_db_error("context_integrations_store_invalid", &path, error)
            })?;
        if !integration_is_configured(configured, &connector_id, &source_account) {
            continue;
        }
        let starts_at = DateTime::parse_from_rfc3339(&occurred_from)
            .map_err(|error| {
                CliError::new(
                    "context_integrations_store_invalid",
                    format!(
                        "Calendar event '{connector_id}:{source_account}:{source_id}' has invalid occurred_from: {error}."
                    ),
                )
            })?
            .with_timezone(&Utc);
        let key = (connector_id.clone(), source_account.clone());
        if configured
            .and_then(|configured| configured.get(&key))
            .and_then(|configuration| configuration.scope.as_ref())
            .is_some_and(|scope| starts_at < scope.occurred_from || starts_at > scope.occurred_to)
        {
            continue;
        }
        let mut attendee_statement = conn
            .prepare(
                "SELECT position, display_name, email
                 FROM calendar_event_attendees
                 WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                 ORDER BY position, attendee_key",
            )
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        let attendee_rows = attendee_statement
            .query_map([&connector_id, &source_account, &source_id], |row| {
                Ok((
                    row.get::<_, usize>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        let attendee_rows = attendee_rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        let evidence = EvidenceHandle::ExternalRecord {
            connector_id: connector_id.clone(),
            source_account: source_account.clone(),
            source_id: source_id.clone(),
            href: source_href.clone(),
        };
        let people = attendee_rows
            .iter()
            .map(|(position, display_name, email)| MeetingAttendee {
                position: *position,
                display_name: display_name.clone(),
                email: email.clone(),
                source: MeetingSource::CalendarEvent,
                meeting_id: format!("{connector_id}:{source_account}:{source_id}"),
                evidence: evidence.clone(),
            })
            .collect::<Vec<_>>();
        meetings.push(MeetingRecord {
            id: format!("{connector_id}:{source_account}:{source_id}"),
            source: MeetingSource::CalendarEvent,
            starts_at,
            title: Some(title),
            note_path: None,
            people: people.clone(),
            evidence,
        });
        episodes.push(IntegrationEpisode {
            connector_id,
            source_account,
            source_id: source_id.clone(),
            occurred_at: starts_at,
            kind: "calendar_event".to_string(),
            body,
            people: people
                .into_iter()
                .map(|person| PersonIdentity {
                    display_name: person.display_name,
                    email: person.email,
                    aliases: Vec::new(),
                    ambiguous: false,
                    wikilink: None,
                })
                .collect(),
            source_href,
        });
    }
    Ok((meetings, episodes))
}

fn load_external_document_episodes(
    conn: &Connection,
    path: &Path,
    configured: Option<&ConfiguredIntegrations>,
) -> Result<(Vec<MeetingRecord>, Vec<IntegrationEpisode>), CliError> {
    let mut statement = conn
        .prepare(
            "SELECT connector_id, source_account, source_id, occurred_at,
                    title, body_text, href
             FROM external_document_evidence
             WHERE connector_id = 'google_meet'
               AND tombstoned_at IS NULL
             ORDER BY occurred_at, connector_id, source_account, source_id",
        )
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let mut meetings = Vec::new();
    let mut episodes = Vec::new();
    for row in rows {
        let (connector_id, source_account, source_id, occurred_at, title, body, source_href) = row
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        if !integration_is_configured(configured, &connector_id, &source_account) {
            continue;
        }
        let occurred_at = DateTime::parse_from_rfc3339(&occurred_at)
            .map_err(|error| {
                CliError::new(
                    "context_integrations_store_invalid",
                    format!(
                        "External evidence '{connector_id}:{source_account}:{source_id}' has invalid occurred_at: {error}."
                    ),
                )
            })?
            .with_timezone(&Utc);
        if configured
            .and_then(|configured| configured.get(&(connector_id.clone(), source_account.clone())))
            .and_then(|configuration| configuration.external_occurred_from)
            .is_some_and(|boundary| occurred_at < boundary)
        {
            continue;
        }
        let mut participant_statement = conn
            .prepare(
                "SELECT position, display_name, email, ambiguous
                 FROM external_document_participants
                 WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                 ORDER BY position, participant_key",
            )
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        let participant_rows = participant_statement
            .query_map([&connector_id, &source_account, &source_id], |row| {
                Ok((
                    row.get::<_, usize>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        let evidence = EvidenceHandle::ExternalRecord {
            connector_id: connector_id.clone(),
            source_account: source_account.clone(),
            source_id: source_id.clone(),
            href: source_href.clone(),
        };
        let meeting_id = format!("{connector_id}:{source_account}:{source_id}");
        let people = participant_rows
            .iter()
            .filter(|(_, _, _, ambiguous)| !ambiguous)
            .map(|(position, display_name, email, _)| MeetingAttendee {
                position: *position,
                display_name: display_name.clone(),
                email: email.clone(),
                source: MeetingSource::ExternalDocument,
                meeting_id: meeting_id.clone(),
                evidence: evidence.clone(),
            })
            .collect::<Vec<_>>();
        meetings.push(MeetingRecord {
            id: meeting_id,
            source: MeetingSource::ExternalDocument,
            starts_at: occurred_at,
            title: Some(title),
            note_path: None,
            people: people.clone(),
            evidence,
        });
        episodes.push(IntegrationEpisode {
            kind: if connector_id == "google_meet" {
                "transcript".to_string()
            } else {
                "meeting_note".to_string()
            },
            connector_id,
            source_account,
            source_id,
            occurred_at,
            body,
            people: participant_rows
                .into_iter()
                .map(|(_, display_name, email, ambiguous)| PersonIdentity {
                    display_name,
                    email,
                    aliases: Vec::new(),
                    ambiguous,
                    wikilink: None,
                })
                .collect(),
            source_href,
        });
    }
    Ok((meetings, episodes))
}

fn load_email_integration_episodes(
    conn: &Connection,
    path: &Path,
    configured: Option<&ConfiguredIntegrations>,
) -> Result<Vec<IntegrationEpisode>, CliError> {
    let mut statement = conn
        .prepare(
            "SELECT te.connector_id, te.source_account, te.thread_id, te.occurred_to,
                    te.body_text, te.href,
                    COALESCE((
                        SELECT json_group_array(ordered.participant)
                        FROM (
                            SELECT DISTINCT pt.participant
                            FROM participant_threads pt
                            WHERE pt.connector_id = te.connector_id
                              AND pt.source_account = te.source_account
                              AND pt.thread_id = te.thread_id
                            ORDER BY pt.participant
                        ) ordered
                    ), '[]') AS participants
             FROM thread_evidence te
             WHERE te.tombstoned_at IS NULL
             ORDER BY te.occurred_to, te.connector_id, te.source_account, te.thread_id",
        )
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let mut episodes = Vec::new();
    for row in rows {
        let (
            connector_id,
            source_account,
            source_id,
            occurred_at,
            body_text,
            href,
            participants_json,
        ) = row
            .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
        if !integration_is_configured(configured, &connector_id, &source_account) {
            continue;
        }
        let occurred_at = DateTime::parse_from_rfc3339(&occurred_at)
            .map_err(|error| {
                CliError::new(
                    "context_integrations_store_invalid",
                    format!(
                        "Email thread '{connector_id}:{source_account}:{source_id}' has invalid occurred_to: {error}."
                    ),
                )
            })?
            .with_timezone(&Utc);
        let participants: Vec<String> = serde_json::from_str(&participants_json).map_err(|error| {
            CliError::new(
                "context_integrations_store_invalid",
                format!(
                    "Email thread '{connector_id}:{source_account}:{source_id}' has invalid participant associations: {error}."
                ),
            )
        })?;
        let people = participants
            .into_iter()
            .map(|email| PersonIdentity {
                display_name: email.clone(),
                email: Some(email),
                aliases: Vec::new(),
                ambiguous: false,
                wikilink: None,
            })
            .collect();
        let source_href = href.clone();
        episodes.push(IntegrationEpisode {
            connector_id,
            source_account,
            source_id: source_id.clone(),
            occurred_at,
            kind: "email".to_string(),
            body: body_text,
            people,
            source_href,
        });
    }
    Ok(episodes)
}

fn merge_meeting_sources(
    sessions: &[MeetingRecord],
    calendar_meetings: Vec<MeetingRecord>,
) -> Vec<MeetingRecord> {
    let mut meetings = sessions.to_vec();
    for calendar in calendar_meetings {
        let duplicate = meetings.iter_mut().find(|candidate| {
            candidate.source == MeetingSource::Session
                && (candidate.starts_at - calendar.starts_at)
                    .num_seconds()
                    .abs()
                    <= MEETING_DEDUPE_WINDOW_SECS
                && attendees_overlap(&candidate.people, &calendar.people)
        });
        if let Some(session) = duplicate {
            merge_attendees(&mut session.people, &calendar.people);
        } else {
            meetings.push(calendar);
        }
    }
    meetings
}

fn attendees_overlap(left: &[MeetingAttendee], right: &[MeetingAttendee]) -> bool {
    let left_keys = left
        .iter()
        .flat_map(MeetingAttendee::identity_values)
        .map(normalize_identity)
        .collect::<BTreeSet<_>>();
    right.iter().any(|attendee| {
        attendee
            .identity_values()
            .map(normalize_identity)
            .any(|key| left_keys.contains(&key))
    })
}

fn merge_attendees(target: &mut Vec<MeetingAttendee>, additional: &[MeetingAttendee]) {
    for attendee in additional {
        let matching = target.iter_mut().find(|existing| {
            attendees_overlap(
                std::slice::from_ref(*existing),
                std::slice::from_ref(attendee),
            )
        });
        if let Some(existing) = matching {
            if existing.email.is_none() && attendee.email.is_some() {
                let position = existing.position;
                *existing = attendee.clone();
                existing.position = position;
            }
        } else {
            let mut attendee = attendee.clone();
            attendee.position = target.len();
            target.push(attendee);
        }
    }
}

fn people_notes(
    work_dir: &Path,
    people_folder: &str,
    documents: &[RecallDocument],
) -> Vec<PersonNote> {
    let folder = normalize_path(people_folder);
    documents
        .iter()
        .filter_map(|document| {
            let source = normalize_path(&document.source_ref);
            if !source.starts_with(&format!("{folder}/")) || !source.ends_with(".md") {
                return None;
            }
            let frontmatter = yaml_frontmatter(&document.content);
            let stem = Path::new(&document.source_ref)
                .file_stem()
                .and_then(|stem| stem.to_str())?
                .to_string();
            let display_name = yaml_strings(frontmatter.as_ref(), &["name", "title"])
                .into_iter()
                .next()
                .or_else(|| document.title.clone())
                .unwrap_or(stem);
            let aliases = yaml_strings(frontmatter.as_ref(), &["aliases", "alias"]);
            let emails = yaml_strings(frontmatter.as_ref(), &["email", "emails"])
                .into_iter()
                .filter(|value| looks_like_email(value))
                .collect();
            Some(PersonNote {
                doc_id: document.id,
                source_ref: document.source_ref.clone(),
                absolute_path: absolute_source_path(work_dir, &document.source_ref)
                    .to_string_lossy()
                    .into_owned(),
                display_name,
                aliases,
                emails,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum ResolutionMode {
    Direct,
    Meeting,
}

struct ResolutionResult {
    resolved: Option<ResolvedIdentity>,
    candidates: Vec<OmissionCandidate>,
}

fn resolve_person(
    query: &str,
    people_notes: &[PersonNote],
    sessions: &[MeetingRecord],
    integration_episodes: &[IntegrationEpisode],
    mode: ResolutionMode,
) -> Result<ResolutionResult, CliError> {
    let query = query.trim();
    let normalized = normalize_identity(query);
    if normalized.is_empty() {
        return Err(CliError::usage("--person cannot be empty"));
    }
    let email_query = looks_like_email(query);
    let bare_first_name = !email_query && query.split_whitespace().count() == 1;
    let mut exact_notes = Vec::new();
    for note in people_notes {
        let kind = if email_query
            && note
                .emails
                .iter()
                .any(|email| normalize_identity(email) == normalized)
        {
            Some("people_note_email")
        } else if normalize_identity(&note.display_name) == normalized {
            Some("people_note_exact_name")
        } else if note
            .aliases
            .iter()
            .any(|alias| normalize_identity(alias) == normalized)
        {
            Some("people_note_alias")
        } else {
            None
        };
        if let Some(kind) = kind {
            exact_notes.push((note, kind));
        }
    }

    if exact_notes.len() == 1 {
        let (note, kind) = exact_notes[0];
        let mut evidence = vec![IdentityEvidence {
            kind,
            value: query.to_string(),
            provenance: Provenance {
                source: "recall_index".to_string(),
                source_id: format!("doc:{}", note.doc_id),
                evidence: EvidenceHandle::NativeMarkdown {
                    path: note.absolute_path.clone(),
                },
                anchor: Some("frontmatter".to_string()),
            },
        }];
        add_session_evidence(&mut evidence, note, sessions);
        add_integration_evidence(&mut evidence, note, integration_episodes);
        return Ok(ResolutionResult {
            resolved: Some(identity_from_note(note, evidence)),
            candidates: Vec::new(),
        });
    }
    if exact_notes.len() > 1 {
        return ambiguous_result(query, exact_notes.iter().map(|(note, _)| *note), mode);
    }

    let mut exact_attendees = BTreeMap::<String, (String, Vec<IdentityEvidence>)>::new();
    for session in sessions {
        for attendee in &session.people {
            if !attendee
                .identity_values()
                .any(|value| normalize_identity(value) == normalized)
            {
                continue;
            }
            let entry = exact_attendees
                .entry(normalized.clone())
                .or_insert_with(|| (attendee.display_name.clone(), Vec::new()));
            entry.1.push(IdentityEvidence {
                kind: attendee_evidence_kind(attendee.source, email_query),
                value: query.to_string(),
                provenance: attendee_provenance(attendee),
            });
        }
    }
    if let Some((display_name, evidence)) = exact_attendees.remove(&normalized) {
        if !bare_first_name || email_query {
            return Ok(ResolutionResult {
                resolved: Some(finalize_identity(ResolvedIdentity {
                    id: format!("attendee:{}", stable_identity_key(&display_name)),
                    display_name,
                    aliases: Vec::new(),
                    emails: if email_query {
                        vec![query.to_string()]
                    } else {
                        Vec::new()
                    },
                    total_evidence_count: 0,
                    resolution_evidence: evidence,
                })),
                candidates: Vec::new(),
            });
        }
    }

    if !bare_first_name || email_query {
        let mut matches = BTreeMap::<String, (String, String, Vec<IdentityEvidence>)>::new();
        for episode in integration_episodes {
            for (position, person) in episode.people.iter().enumerate() {
                if person.ambiguous {
                    continue;
                }
                let Some(email) = person
                    .email
                    .as_deref()
                    .filter(|email| looks_like_email(email))
                else {
                    continue;
                };
                let name_matches = !email_query
                    && person.display_name.split_whitespace().count() >= 2
                    && normalize_identity(&person.display_name) == normalized;
                let email_matches = email_query && normalize_identity(email) == normalized;
                if !name_matches && !email_matches {
                    continue;
                }
                let key = normalize_identity(email);
                let entry = matches.entry(key).or_insert_with(|| {
                    (person.display_name.clone(), email.to_string(), Vec::new())
                });
                entry.2.push(IdentityEvidence {
                    kind: "episode_identity",
                    value: format!("{} <{}>", person.display_name, email),
                    provenance: integration_identity_provenance(episode, position),
                });
            }
        }
        if matches.len() == 1 {
            let (_, (display_name, email, evidence)) = matches.pop_first().unwrap();
            return Ok(ResolutionResult {
                resolved: Some(finalize_identity(ResolvedIdentity {
                    id: format!("episode_identity:{}", stable_identity_key(&email)),
                    display_name,
                    aliases: Vec::new(),
                    emails: vec![email],
                    total_evidence_count: 0,
                    resolution_evidence: evidence,
                })),
                candidates: Vec::new(),
            });
        }
        if matches.len() > 1 {
            let candidates = matches
                .into_values()
                .filter_map(|(display_name, _, evidence)| {
                    Some(OmissionCandidate {
                        display_name,
                        provenance: evidence.into_iter().next()?.provenance,
                    })
                })
                .collect::<Vec<_>>();
            if matches!(mode, ResolutionMode::Direct) {
                return Err(CliError::new(
                    "context_identity_ambiguous",
                    format!(
                        "Person '{query}' matches multiple integration Episode identities: {}. Pass a unique full name or email; Margins will not merge these identities.",
                        candidates
                            .iter()
                            .map(|candidate| candidate.display_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            return Ok(ResolutionResult {
                resolved: None,
                candidates,
            });
        }
    }

    if bare_first_name {
        let mut first_name_candidates = BTreeMap::<String, OmissionCandidate>::new();
        for note in people_notes {
            if first_token(&note.display_name) == normalized
                && normalize_identity(&note.display_name) != normalized
            {
                first_name_candidates
                    .entry(normalize_identity(&note.display_name))
                    .or_insert_with(|| OmissionCandidate {
                        display_name: note.display_name.clone(),
                        provenance: Provenance {
                            source: "recall_index".to_string(),
                            source_id: format!("doc:{}", note.doc_id),
                            evidence: EvidenceHandle::NativeMarkdown {
                                path: note.absolute_path.clone(),
                            },
                            anchor: Some("identity_candidate".to_string()),
                        },
                    });
            }
        }
        for session in sessions {
            for attendee in &session.people {
                if first_token(&attendee.display_name) == normalized
                    && normalize_identity(&attendee.display_name) != normalized
                {
                    first_name_candidates
                        .entry(normalize_identity(&attendee.display_name))
                        .or_insert_with(|| OmissionCandidate {
                            display_name: attendee.display_name.clone(),
                            provenance: attendee_provenance(attendee),
                        });
                }
            }
        }
        for episode in integration_episodes {
            for (position, person) in episode.people.iter().enumerate() {
                if first_token(&person.display_name) == normalized
                    && normalize_identity(&person.display_name) != normalized
                {
                    first_name_candidates
                        .entry(normalize_identity(&person.display_name))
                        .or_insert_with(|| OmissionCandidate {
                            display_name: person.display_name.clone(),
                            provenance: integration_identity_provenance(episode, position),
                        });
                }
            }
        }
        let candidates = first_name_candidates.into_values().collect::<Vec<_>>();
        if matches!(mode, ResolutionMode::Direct) && !candidates.is_empty() {
            return Err(CliError::new(
                "context_identity_ambiguous",
                format!(
                    "Person '{query}' is only a bare first-name match for: {}. Pass a full name or email; Margins will not merge these identities.",
                    candidates.iter().map(|candidate| candidate.display_name.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        return Ok(ResolutionResult {
            resolved: None,
            candidates,
        });
    }

    Ok(ResolutionResult {
        resolved: None,
        candidates: Vec::new(),
    })
}

fn ambiguous_result<'a>(
    query: &str,
    notes: impl Iterator<Item = &'a PersonNote>,
    mode: ResolutionMode,
) -> Result<ResolutionResult, CliError> {
    let candidates = notes
        .map(|note| OmissionCandidate {
            display_name: note.display_name.clone(),
            provenance: Provenance {
                source: "recall_index".to_string(),
                source_id: format!("doc:{}", note.doc_id),
                evidence: EvidenceHandle::NativeMarkdown {
                    path: note.absolute_path.clone(),
                },
                anchor: Some("identity_candidate".to_string()),
            },
        })
        .collect::<Vec<_>>();
    if matches!(mode, ResolutionMode::Direct) {
        return Err(CliError::new(
            "context_identity_ambiguous",
            format!(
                "Person '{query}' matches multiple people-note identities: {}. Pass a unique full name or email; Margins will not merge these identities.",
                candidates.iter().map(|candidate| candidate.display_name.as_str()).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Ok(ResolutionResult {
        resolved: None,
        candidates,
    })
}

fn identity_from_note(note: &PersonNote, evidence: Vec<IdentityEvidence>) -> ResolvedIdentity {
    let mut aliases = note.aliases.clone();
    aliases.sort_by_key(|value| normalize_identity(value));
    aliases.dedup_by(|left, right| normalize_identity(left) == normalize_identity(right));
    let mut emails = note.emails.clone();
    emails.sort_by_key(|value| normalize_identity(value));
    emails.dedup_by(|left, right| normalize_identity(left) == normalize_identity(right));
    finalize_identity(ResolvedIdentity {
        id: format!("person_note:{}", normalize_path(&note.source_ref)),
        display_name: note.display_name.clone(),
        aliases,
        emails,
        total_evidence_count: 0,
        resolution_evidence: evidence,
    })
}

fn finalize_identity(mut identity: ResolvedIdentity) -> ResolvedIdentity {
    let mut seen = BTreeSet::new();
    identity.resolution_evidence.retain(|evidence| {
        seen.insert((
            evidence.kind,
            evidence.value.clone(),
            evidence.provenance.source_id.clone(),
        ))
    });
    identity.total_evidence_count = identity.resolution_evidence.len();
    identity.resolution_evidence.truncate(MAX_IDENTITY_EVIDENCE);
    identity
}

fn add_session_evidence(
    evidence: &mut Vec<IdentityEvidence>,
    note: &PersonNote,
    sessions: &[MeetingRecord],
) {
    let mut keys = vec![normalize_identity(&note.display_name)];
    keys.extend(note.aliases.iter().map(|value| normalize_identity(value)));
    keys.extend(note.emails.iter().map(|value| normalize_identity(value)));
    for session in sessions {
        for attendee in &session.people {
            let matching_value = attendee
                .email
                .as_deref()
                .filter(|value| keys.contains(&normalize_identity(value)))
                .or_else(|| {
                    keys.contains(&normalize_identity(&attendee.display_name))
                        .then_some(attendee.display_name.as_str())
                });
            if let Some(value) = matching_value {
                evidence.push(IdentityEvidence {
                    kind: match attendee.source {
                        MeetingSource::Session => "session_attendee_exact_identity",
                        MeetingSource::CalendarEvent => {
                            attendee_evidence_kind(attendee.source, looks_like_email(value))
                        }
                        MeetingSource::ExternalDocument => {
                            attendee_evidence_kind(attendee.source, looks_like_email(value))
                        }
                    },
                    value: value.to_string(),
                    provenance: attendee_provenance(attendee),
                });
            }
        }
    }
}

fn add_integration_evidence(
    evidence: &mut Vec<IdentityEvidence>,
    note: &PersonNote,
    episodes: &[IntegrationEpisode],
) {
    let mut keys = vec![normalize_identity(&note.display_name)];
    keys.extend(note.aliases.iter().map(|value| normalize_identity(value)));
    keys.extend(note.emails.iter().map(|value| normalize_identity(value)));
    for episode in episodes {
        for (position, person) in episode.people.iter().enumerate() {
            let matching_value = person
                .email
                .as_deref()
                .filter(|value| keys.contains(&normalize_identity(value)))
                .or_else(|| {
                    keys.contains(&normalize_identity(&person.display_name))
                        .then_some(person.display_name.as_str())
                });
            if let Some(value) = matching_value {
                evidence.push(IdentityEvidence {
                    kind: if episode.kind == "calendar_event" {
                        if looks_like_email(value) {
                            "calendar_attendee_email"
                        } else {
                            "calendar_attendee_exact_name"
                        }
                    } else {
                        "episode_identity"
                    },
                    value: value.to_string(),
                    provenance: integration_identity_provenance(episode, position),
                });
            }
        }
    }
}

fn select_meeting<'a>(
    requested: &str,
    sessions: &'a [MeetingRecord],
    now: DateTime<Utc>,
) -> Result<&'a MeetingRecord, CliError> {
    if requested == "next" {
        return sessions
            .iter()
            .filter(|session| session.starts_at >= now)
            .min_by_key(|session| session.starts_at)
            .ok_or_else(|| {
                CliError::new(
                    "context_meeting_not_found",
                    "No future meeting exists in the Margins session index. Pass a concrete session id or sync an upcoming meeting.",
                )
            });
    }
    sessions
        .iter()
        .find(|session| session.id == requested)
        .ok_or_else(|| {
            CliError::new(
                "context_meeting_not_found",
                format!("Meeting '{requested}' does not exist in the Margins session index."),
            )
        })
}

fn collect_episodes(
    work_dir: &Path,
    documents: &[RecallDocument],
    sessions: &[MeetingRecord],
    session_path: Option<&Path>,
    integration_episodes: &[IntegrationEpisode],
    identities: &[ResolvedIdentity],
    cutoff: DateTime<Utc>,
    excluded_meeting: Option<&str>,
) -> Vec<Episode> {
    let keys = identity_keys(identities);
    let people_note_paths = identities
        .iter()
        .filter_map(|identity| identity.id.strip_prefix("person_note:"))
        .map(normalize_path)
        .collect::<BTreeSet<_>>();
    let mut people_note_targets = BTreeSet::new();
    for path in &people_note_paths {
        people_note_targets.insert(normalize_identity(path.trim_end_matches(".md")));
        if let Some(stem) = Path::new(path).file_stem().and_then(|stem| stem.to_str()) {
            people_note_targets.insert(normalize_identity(stem));
        }
    }
    let matching_sessions = sessions
        .iter()
        .filter(|session| {
            excluded_meeting != Some(session.id.as_str())
                && session.starts_at <= cutoff
                && session
                    .people
                    .iter()
                    .flat_map(MeetingAttendee::identity_values)
                    .any(|attendee| keys.contains(&normalize_identity(attendee)))
        })
        .collect::<Vec<_>>();
    let session_note_paths = matching_sessions
        .iter()
        .filter_map(|session| session.note_path.as_deref())
        .map(|path| normalize_path(&path_from_store(work_dir, path).to_string_lossy()))
        .collect::<BTreeSet<_>>();

    let mut episodes = Vec::new();
    let mut represented_paths = BTreeSet::new();
    for document in documents {
        if document.source_ref.starts_with("sqlite:") {
            continue;
        }
        let occurred_at = document_occurred_at(document);
        if occurred_at > cutoff {
            continue;
        }
        let normalized_source = normalize_path(&document.source_ref);
        let exact_person_note = people_note_paths.contains(&normalized_source);
        let session_note = session_note_paths.contains(&normalize_path(
            &absolute_source_path(work_dir, &document.source_ref).to_string_lossy(),
        ));
        let linked = document.links.iter().any(|link| {
            let link = normalize_identity(link.trim_end_matches(".md"));
            people_note_targets.contains(&link) || keys.contains(&link)
        });
        if !(exact_person_note || session_note || linked) {
            continue;
        }
        let path = absolute_source_path(work_dir, &document.source_ref);
        let provenance = document_provenance(document, &path);
        represented_paths.insert(normalize_path(&path.to_string_lossy()));
        episodes.push(Episode {
            id: episode_id(document),
            kind: document_kind(document, exact_person_note),
            occurred_at: occurred_at.to_rfc3339(),
            provenance,
            evidence: EvidenceHandle::NativeMarkdown {
                path: path.to_string_lossy().into_owned(),
            },
            excerpt: deterministic_excerpt(&document.content),
        });
    }

    if let Some(session_db) = session_path {
        for session in matching_sessions {
            let note_path = session
                .note_path
                .as_deref()
                .map(|path| path_from_store(work_dir, path))
                .unwrap_or_else(|| session_db.to_path_buf());
            if represented_paths.contains(&normalize_path(&note_path.to_string_lossy())) {
                continue;
            }
            episodes.push(Episode {
                id: format!("session:{}", session.id),
                kind: "meeting_note".to_string(),
                occurred_at: session.starts_at.to_rfc3339(),
                provenance: meeting_provenance(session, Some("sessions")),
                evidence: session.evidence.clone(),
                excerpt: std::fs::read_to_string(&note_path)
                    .map(|content| deterministic_excerpt(&content))
                    .unwrap_or_default(),
            });
        }
    }
    let mut represented_ids = episodes
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<BTreeSet<_>>();
    for integration in integration_episodes {
        if integration.occurred_at > cutoff
            || excluded_meeting == Some(integration.ledger_id().as_str())
            || !integration.people.iter().any(|person| {
                std::iter::once(person.display_name.as_str())
                    .chain(person.email.iter().map(String::as_str))
                    .any(|value| keys.contains(&normalize_identity(value)))
            })
        {
            continue;
        }
        let id = integration.context_episode_id();
        if !represented_ids.insert(id.clone()) {
            continue;
        }
        episodes.push(Episode {
            id,
            kind: integration.kind.clone(),
            occurred_at: integration.occurred_at.to_rfc3339(),
            provenance: Provenance {
                source: "integrations_ledger".to_string(),
                source_id: integration.ledger_id(),
                evidence: integration.evidence_handle(),
                anchor: Some(integration.evidence_anchor().to_string()),
            },
            evidence: integration.evidence_handle(),
            excerpt: deterministic_excerpt(&integration.body),
        });
    }
    episodes
}

fn derive_open_items(
    episodes: &[Episode],
    documents: &[RecallDocument],
    work_dir: &Path,
) -> Vec<OpenItem> {
    let mut by_path = BTreeMap::new();
    for document in documents {
        by_path.insert(
            normalize_path(&absolute_source_path(work_dir, &document.source_ref).to_string_lossy()),
            document.content.as_str(),
        );
    }
    let mut items = Vec::new();
    for episode in episodes {
        let EvidenceHandle::NativeMarkdown { path } = &episode.evidence else {
            continue;
        };
        let content = by_path
            .get(&normalize_path(path))
            .copied()
            .map(str::to_string)
            .or_else(|| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        for (index, raw) in content.lines().enumerate() {
            let trimmed = raw.trim_start();
            let text = trimmed
                .strip_prefix("- [ ] ")
                .or_else(|| trimmed.strip_prefix("* [ ] "))
                .map(str::trim)
                .filter(|text| !text.is_empty());
            let Some(text) = text else { continue };
            let line_number = index + 1;
            items.push(OpenItem {
                id: format!("{}:task:{line_number}", episode.id),
                text: text.to_string(),
                status: "open",
                episode_id: episode.id.clone(),
                provenance: Provenance {
                    source: episode.provenance.source.clone(),
                    source_id: episode.provenance.source_id.clone(),
                    evidence: episode.evidence.clone(),
                    anchor: Some(format!("L{line_number}")),
                },
            });
        }
    }
    items
}

fn parse_cutoff(value: Option<&str>, now: DateTime<Utc>) -> Result<DateTime<Utc>, CliError> {
    let Some(value) = value else { return Ok(now) };
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let next_day = date
            .succ_opt()
            .ok_or_else(|| CliError::usage(format!("invalid --cutoff date '{value}'")))?;
        return Ok(
            Utc.from_utc_datetime(&next_day.and_hms_opt(0, 0, 0).unwrap())
                - chrono::Duration::milliseconds(1),
        );
    }
    Err(CliError::usage(format!(
        "invalid --cutoff '{value}'; use YYYY-MM-DD or an RFC 3339 timestamp"
    )))
}

fn identity_not_found(query: &str) -> CliError {
    CliError::new(
        "context_identity_not_found",
        format!(
            "Person '{query}' could not be resolved by exact email, full name, or people-note alias. Margins will not guess from text similarity."
        ),
    )
}

fn push_identity(identities: &mut Vec<ResolvedIdentity>, identity: ResolvedIdentity) {
    if !identities.iter().any(|current| current.id == identity.id) {
        identities.push(identity);
    }
}

fn identity_keys(identities: &[ResolvedIdentity]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for identity in identities {
        keys.insert(normalize_identity(&identity.display_name));
        keys.extend(
            identity
                .aliases
                .iter()
                .map(|value| normalize_identity(value)),
        );
        keys.extend(
            identity
                .emails
                .iter()
                .map(|value| normalize_identity(value)),
        );
    }
    keys
}

fn document_occurred_at(document: &RecallDocument) -> DateTime<Utc> {
    for key in ["occurred_at", "date", "created_at", "created"] {
        if let Some(value) = metadata_string(&document.metadata, key) {
            if let Ok(parsed) = DateTime::parse_from_rfc3339(&value) {
                return parsed.with_timezone(&Utc);
            }
            if let Ok(date) = NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
                return Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap());
            }
        }
    }
    Utc.timestamp_millis_opt(document.created_at)
        .single()
        .unwrap_or(DateTime::UNIX_EPOCH)
}

fn document_provenance(document: &RecallDocument, path: &Path) -> Provenance {
    Provenance {
        source: metadata_string(&document.metadata, "source")
            .unwrap_or_else(|| "vault".to_string()),
        source_id: metadata_string(&document.metadata, "source_id")
            .unwrap_or_else(|| format!("doc:{}", document.id)),
        evidence: EvidenceHandle::NativeMarkdown {
            path: path.to_string_lossy().into_owned(),
        },
        anchor: None,
    }
}

fn document_kind(document: &RecallDocument, person_note: bool) -> String {
    metadata_string(&document.metadata, "kind").unwrap_or_else(|| {
        if person_note {
            "person_note".to_string()
        } else {
            "document".to_string()
        }
    })
}

fn episode_id(document: &RecallDocument) -> String {
    match (
        metadata_string(&document.metadata, "source"),
        metadata_string(&document.metadata, "source_id"),
    ) {
        (Some(source), Some(id)) => format!("{source}:{id}"),
        _ => format!("recall_doc:{}", document.id),
    }
}

fn metadata_string(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn deterministic_excerpt(content: &str) -> String {
    let body = strip_frontmatter(content);
    let collapsed = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() <= MAX_EXCERPT_CHARS {
        return collapsed;
    }
    let mut excerpt = collapsed
        .chars()
        .take(MAX_EXCERPT_CHARS)
        .collect::<String>();
    excerpt.push('…');
    excerpt
}

fn strip_frontmatter(content: &str) -> &str {
    let normalized = content.trim_start_matches('\u{feff}');
    if !normalized.starts_with("---") {
        return normalized;
    }
    let Some(rest) = normalized.strip_prefix("---") else {
        return normalized;
    };
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    for delimiter in ["\n---\r\n", "\n---\n"] {
        if let Some(index) = rest.find(delimiter) {
            return &rest[index + delimiter.len()..];
        }
    }
    normalized
}

fn yaml_frontmatter(content: &str) -> Option<YamlValue> {
    let normalized = content.trim_start_matches('\u{feff}');
    let rest = normalized.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    let end = rest.find("\n---")?;
    serde_yaml::from_str(&rest[..end]).ok()
}

fn yaml_strings(frontmatter: Option<&YamlValue>, keys: &[&str]) -> Vec<String> {
    let Some(YamlValue::Mapping(mapping)) = frontmatter else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for key in keys {
        let Some(value) = mapping.get(YamlValue::String((*key).to_string())) else {
            continue;
        };
        match value {
            YamlValue::String(value) => values.push(value.trim().to_string()),
            YamlValue::Sequence(sequence) => values.extend(sequence.iter().filter_map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })),
            _ => {}
        }
    }
    values
}

fn open_read_only(path: &Path, code: &'static str) -> Result<Connection, CliError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| context_db_error(code, path, error))
}

fn context_db_error(code: &'static str, path: &Path, error: rusqlite::Error) -> CliError {
    CliError::new(
        code,
        format!("Could not read '{}': {error}", path.display()),
    )
}

fn attendee_evidence_kind(source: MeetingSource, email: bool) -> &'static str {
    match (source, email) {
        (MeetingSource::Session, true) => "session_attendee_email",
        (MeetingSource::Session, false) => "session_attendee_exact_name",
        (MeetingSource::CalendarEvent, true) => "calendar_attendee_email",
        (MeetingSource::CalendarEvent, false) => "calendar_attendee_exact_name",
        (MeetingSource::ExternalDocument, true) => "external_document_participant_email",
        (MeetingSource::ExternalDocument, false) => "external_document_participant_exact_name",
    }
}

fn meeting_provenance(meeting: &MeetingRecord, anchor: Option<&str>) -> Provenance {
    Provenance {
        source: meeting.source.provenance_source().to_string(),
        source_id: meeting.id.clone(),
        evidence: meeting.evidence.clone(),
        anchor: anchor.map(str::to_string),
    }
}

fn attendee_provenance(attendee: &MeetingAttendee) -> Provenance {
    Provenance {
        source: attendee.source.provenance_source().to_string(),
        source_id: attendee.meeting_id.clone(),
        evidence: attendee.evidence.clone(),
        anchor: Some(format!(
            "{}:{}",
            match attendee.source {
                MeetingSource::Session => "session_people",
                MeetingSource::CalendarEvent => "calendar_event_attendees",
                MeetingSource::ExternalDocument => "external_document_participants",
            },
            attendee.position
        )),
    }
}

fn integration_identity_provenance(episode: &IntegrationEpisode, position: usize) -> Provenance {
    Provenance {
        source: "integrations_ledger".to_string(),
        source_id: episode.ledger_id(),
        evidence: episode.evidence_handle(),
        anchor: Some(episode.identity_anchor(position)),
    }
}

fn absolute_source_path(work_dir: &Path, source_ref: &str) -> PathBuf {
    let path = PathBuf::from(source_ref);
    if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    }
}

fn path_from_store(work_dir: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_lowercase()
}

fn normalize_identity(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| matches!(character, '[' | ']' | '"' | '\''))
        .split('|')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn first_token(value: &str) -> String {
    normalize_identity(value)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

fn stable_identity_key(value: &str) -> String {
    normalize_identity(value)
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn looks_like_email(value: &str) -> bool {
    let value = value.trim();
    value.contains('@') && !value.starts_with('@') && !value.ends_with('@')
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn file_modified_datetime(path: &Path) -> Option<DateTime<Utc>> {
    file_modified_millis(path).and_then(DateTime::from_timestamp_millis)
}

fn fresh_at(last_successful_refresh: Option<DateTime<Utc>>) -> EvidenceFreshness {
    EvidenceFreshness {
        status: FreshnessStatus::Fresh,
        stale: false,
        reason: None,
        last_successful_refresh,
    }
}

fn integration_materialization_freshness(
    path: &Path,
    configured: Option<&ConfiguredIntegrations>,
) -> Result<EvidenceFreshness, CliError> {
    let connection = open_read_only(path, "context_integrations_store_invalid")?;
    let mut statement = connection
        .prepare(
            "SELECT connector_id, account, health_status, last_sync_at,
                    materialization_fingerprint, scope_boundary_json
             FROM connectors ORDER BY connector_id, account",
        )
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?
        .filter_map(|row| match row {
            Ok((
                connector_id,
                account,
                status,
                last_sync_at,
                materialization_fingerprint,
                scope_boundary_json,
            )) if integration_is_configured(configured, &connector_id, &account) => Some(Ok((
                connector_id,
                account,
                status,
                last_sync_at,
                materialization_fingerprint,
                scope_boundary_json,
            ))),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| context_db_error("context_integrations_store_invalid", path, error))?;
    if rows.is_empty() {
        return Ok(EvidenceFreshness {
            status: FreshnessStatus::Stale,
            stale: true,
            reason: Some("never_refreshed".to_string()),
            last_successful_refresh: None,
        });
    }
    let observed = rows
        .iter()
        .map(|(connector_id, account, _, _, _, _)| (connector_id.clone(), account.clone()))
        .collect::<BTreeSet<_>>();
    let missing_configured_source = configured.is_some_and(|configured| {
        !configured
            .keys()
            .all(|configured| observed.contains(configured))
    });
    let mut status = if missing_configured_source {
        FreshnessStatus::Stale
    } else {
        FreshnessStatus::Fresh
    };
    let mut reason = missing_configured_source.then(|| "never_refreshed".to_string());
    let mut last_successful_refresh = None;
    for (
        connector_id,
        account,
        raw_status,
        last_sync_at,
        materialization_fingerprint,
        scope_boundary_json,
    ) in rows
    {
        if let Some(value) = last_sync_at {
            let parsed = DateTime::parse_from_rfc3339(&value)
                .map_err(|error| {
                    CliError::new(
                        "context_integrations_store_invalid",
                        format!("Integration refresh timestamp '{value}' is invalid: {error}."),
                    )
                })?
                .with_timezone(&Utc);
            last_successful_refresh = Some(
                last_successful_refresh
                    .map_or(parsed, |current: DateTime<Utc>| current.max(parsed)),
            );
        }
        let integration_key = (connector_id.clone(), account.clone());
        let expected_fingerprint = configured
            .and_then(|configured| configured.get(&integration_key))
            .and_then(|configuration| configuration.materialization_fingerprint.as_deref());
        let selector_mismatch = expected_fingerprint
            .is_some_and(|expected| materialization_fingerprint.as_deref() != Some(expected));
        let scope_mismatch = configured
            .and_then(|configured| configured.get(&integration_key))
            .and_then(|configuration| configuration.scope.as_ref())
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| CliError::from_anyhow(error.into()))?
            .is_some_and(|expected| scope_boundary_json.as_deref() != Some(expected.as_str()));
        let (candidate, candidate_reason) = match raw_status.as_str() {
            "fresh" if selector_mismatch || scope_mismatch => {
                (FreshnessStatus::Stale, Some("refresh_required".to_string()))
            }
            "fresh" => (FreshnessStatus::Fresh, None),
            "stale" | "unknown" => (FreshnessStatus::Stale, Some("refresh_required".to_string())),
            "needs-auth" => (
                FreshnessStatus::NeedsAuth,
                Some("credentials_unavailable".to_string()),
            ),
            "error" => (FreshnessStatus::Error, Some("refresh_failed".to_string())),
            other => {
                return Err(CliError::new(
                    "context_integrations_store_invalid",
                    format!("Unknown connector health status '{other}'."),
                ));
            }
        };
        if freshness_severity(candidate) > freshness_severity(status) {
            status = candidate;
            reason = candidate_reason;
        }
    }
    Ok(EvidenceFreshness {
        status,
        stale: status != FreshnessStatus::Fresh,
        reason,
        last_successful_refresh,
    })
}

fn freshness_severity(status: FreshnessStatus) -> u8 {
    match status {
        FreshnessStatus::Fresh | FreshnessStatus::NotApplicable => 0,
        FreshnessStatus::Stale => 1,
        FreshnessStatus::NeedsAuth => 2,
        FreshnessStatus::Error => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::integrations::{
        ConnectorCtx, ExternalDocumentDelta, ExternalDocumentEvidence, ExternalDocumentParticipant,
        HealthStatus, IntegrationsStore, ParticipantThread, ThreadEvidence,
    };
    use margins_workflows::workspace::{self, GmailCollectionSelector, WorkspaceBinding};

    #[test]
    fn removed_workspace_source_excludes_retained_email_evidence_from_context() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace =
            workspace::create_workspace(temp.path(), "practice", None, &notes).unwrap();
        let initial_selector = GmailCollectionSelector::default_declaration();
        workspace::add_source(
            &mut workspace,
            "work-mail",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: initial_selector.clone(),
            },
        )
        .unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: "email".to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        let occurred_at = Utc::now();
        store
            .replace_email_thread_snapshot_with_materialization_fingerprint(
                &ctx,
                vec![ThreadEvidence {
                    thread_id: "thread-1".to_string(),
                    occurred_from: occurred_at,
                    occurred_to: occurred_at,
                    body_text: "Retained authoritative email evidence".to_string(),
                    href: None,
                }],
                vec![ParticipantThread {
                    participant: "alice@example.com".to_string(),
                    thread_id: "thread-1".to_string(),
                    last_interaction: occurred_at,
                    sampling_score: None,
                }],
                &initial_selector.materialization_fingerprint().unwrap(),
            )
            .unwrap();
        store
            .update_health(&ctx, HealthStatus::Fresh, None)
            .unwrap();

        let configured = configured_integration_keys(&workspace.state_dir)
            .unwrap()
            .unwrap();
        let freshness = integration_materialization_freshness(
            &workspace.state_dir.join("ledger.db"),
            Some(&configured),
        )
        .unwrap();
        assert_eq!(freshness.status, FreshnessStatus::Fresh);
        let (_, episodes, ledger) =
            load_integration_episodes_read_only(&workspace.state_dir, Some(&configured)).unwrap();
        assert_eq!(episodes.len(), 1);
        assert!(ledger.is_some());

        workspace::remove_source(&mut workspace, "work-mail").unwrap();
        let changed_selector = GmailCollectionSelector {
            query: "label:important".to_string(),
            backfill_days: 30,
        };
        workspace::add_source(
            &mut workspace,
            "work-mail-renamed",
            WorkspaceBinding::Gmail {
                account: "owner@example.com".to_string(),
                gmail: changed_selector.clone(),
            },
        )
        .unwrap();
        let configured = configured_integration_keys(&workspace.state_dir)
            .unwrap()
            .unwrap();
        let freshness = integration_materialization_freshness(
            &workspace.state_dir.join("ledger.db"),
            Some(&configured),
        )
        .unwrap();
        assert_eq!(freshness.status, FreshnessStatus::Stale);
        assert_eq!(freshness.reason.as_deref(), Some("refresh_required"));
        assert_eq!(store.thread_evidence(&ctx).unwrap().len(), 1);

        store
            .replace_email_thread_snapshot_with_materialization_fingerprint(
                &ctx,
                store.thread_evidence(&ctx).unwrap(),
                store.participant_threads(&ctx).unwrap(),
                &changed_selector.materialization_fingerprint().unwrap(),
            )
            .unwrap();
        let freshness = integration_materialization_freshness(
            &workspace.state_dir.join("ledger.db"),
            Some(&configured),
        )
        .unwrap();
        assert_eq!(freshness.status, FreshnessStatus::Fresh);

        workspace::remove_source(&mut workspace, "work-mail-renamed").unwrap();
        let configured = configured_integration_keys(&workspace.state_dir)
            .unwrap()
            .unwrap();
        assert!(configured.is_empty());
        let (_, episodes, ledger) =
            load_integration_episodes_read_only(&workspace.state_dir, Some(&configured)).unwrap();
        assert!(episodes.is_empty());
        assert!(ledger.is_none());
        assert_eq!(store.thread_evidence(&ctx).unwrap().len(), 1);
        assert_eq!(store.participant_threads(&ctx).unwrap().len(), 1);
    }

    #[test]
    fn ambiguous_external_participants_are_evidence_only_for_meeting_context() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace =
            workspace::create_workspace(temp.path(), "practice", None, &notes).unwrap();
        workspace::add_source(
            &mut workspace,
            "meet",
            WorkspaceBinding::GoogleMeet {
                account: "owner@example.com".to_string(),
            },
        )
        .unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: "google_meet".to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        store
            .apply_external_document_delta(
                &ctx,
                ExternalDocumentDelta {
                    documents: vec![ExternalDocumentEvidence {
                        source_id: "meet-1".to_string(),
                        occurred_at: Utc::now(),
                        title: "Identity review".to_string(),
                        body_text: "Authoritative transcript".to_string(),
                        href: None,
                        attributes: serde_json::json!({}),
                    }],
                    participants: vec![
                        ExternalDocumentParticipant {
                            source_id: "meet-1".to_string(),
                            participant_key: "alice@example.com".to_string(),
                            position: 0,
                            display_name: "Alice Example".to_string(),
                            email: Some("alice@example.com".to_string()),
                            ambiguous: false,
                        },
                        ExternalDocumentParticipant {
                            source_id: "meet-1".to_string(),
                            participant_key: "position:1:sam".to_string(),
                            position: 1,
                            display_name: "Sam".to_string(),
                            email: None,
                            ambiguous: true,
                        },
                    ],
                    tombstone_source_ids: Vec::new(),
                    raw_items: Vec::new(),
                    snapshot_scope: None,
                    complete_snapshot: true,
                    materialization_fingerprint: GOOGLE_MEET_MATERIALIZATION_FINGERPRINT
                        .to_string(),
                    next_cursor: None,
                },
                None,
            )
            .unwrap();

        let configured = configured_integration_keys(&workspace.state_dir)
            .unwrap()
            .unwrap();
        let (meetings, episodes, _) =
            load_integration_episodes_read_only(&workspace.state_dir, Some(&configured)).unwrap();
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].people.len(), 1);
        assert_eq!(meetings[0].people[0].display_name, "Alice Example");
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].people.len(), 2);
        assert!(episodes[0].people[1].ambiguous);
    }
}
