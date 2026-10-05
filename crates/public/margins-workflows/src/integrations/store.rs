//! SQLite integrations datastore and reconciliation engine.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, TimeZone, Utc};
use fs4::fs_std::FileExt;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{
    params, params_from_iter, Connection, OpenFlags, OptionalExtension, TransactionBehavior,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::types::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx,
    CurationObservation, EmailThreadSnapshotCounts, ExternalDocumentDelta,
    ExternalDocumentEvidence, ExternalDocumentParticipant, HealthReport, HealthStatus,
    ParticipantThread, RankedParticipant, RawItemDraft, ReconcileResult, RetentionApplyReceipt,
    RetentionCounts, RetentionCutoffs, RetentionMutationError, RetentionPreview, RetentionScope,
    RetentionTarget, RunManifest, SurveyRange, ThreadEvidence, RETENTION_APPLY_SCHEMA,
    RETENTION_PREVIEW_SCHEMA,
};
use crate::workspace::{
    lock_workspace_revision, normalize_google_account, normalize_granola_account,
    resolve_state_dir, validate_mutation_request_id, workspace_revision, ResolvedWorkspace,
    WorkspaceMutationError,
};
const SCHEMA_VERSION: i32 = 9;
const DB_NAME: &str = "ledger.db";
const EMAIL_CONNECTOR_ID: &str = "email";

/// SQLite store at `<MARGINS_HOME>/workspaces/<id>/ledger.db`.
#[derive(Debug, Clone)]
pub struct IntegrationsStore {
    workspace_state_dir: PathBuf,
    db_path: PathBuf,
    read_only: bool,
}

impl IntegrationsStore {
    pub fn open(workspace_state_dir: impl AsRef<Path>) -> Result<Self> {
        let workspace_state_dir = workspace_state_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&workspace_state_dir).with_context(|| {
            format!(
                "failed to create workspace state {}",
                workspace_state_dir.display()
            )
        })?;
        let db_path = workspace_state_dir.join(DB_NAME);
        let store = Self {
            workspace_state_dir,
            db_path,
            read_only: false,
        };
        store.initialize()?;
        Ok(store)
    }

    /// Open an already-created ledger without creating or migrating workspace
    /// state. Read-only product/status surfaces use this path so observation
    /// never becomes a materialization mutation.
    pub fn open_existing_read_only(workspace_state_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let workspace_state_dir = workspace_state_dir.as_ref().to_path_buf();
        let db_path = workspace_state_dir.join(DB_NAME);
        if !db_path.is_file() {
            return Ok(None);
        }
        let store = Self {
            workspace_state_dir,
            db_path,
            read_only: true,
        };
        let connection = store.connect()?;
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("failed to read ledger schema version")?;
        if version != SCHEMA_VERSION {
            bail!(
                "unsupported ledger schema version {version}; delete and recreate the workspace ledger"
            );
        }
        Ok(Some(store))
    }

    pub fn workspace_state_dir(&self) -> &Path {
        &self.workspace_state_dir
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn connect(&self) -> Result<Connection> {
        let connection = if self.read_only {
            Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        } else {
            Connection::open(&self.db_path)
        }
        .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .context("failed to set SQLite busy timeout")?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .context("failed to enable foreign keys")?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<()> {
        let connection = self.connect()?;
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0);
        if version == 0 {
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE connectors (
                        connector_id TEXT NOT NULL,
                        account TEXT NOT NULL,
                        cursor_json TEXT,
                        last_sync_at TEXT,
                        scope_boundary_json TEXT,
                        materialization_fingerprint TEXT,
                        health_status TEXT NOT NULL DEFAULT 'unknown',
                        health_detail_json TEXT,
                        counts_json TEXT NOT NULL DEFAULT '{}',
                        updated_at TEXT NOT NULL,
                        PRIMARY KEY (connector_id, account)
                    );

                    CREATE TABLE raw_items (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        source_id TEXT NOT NULL,
                        payload_json TEXT NOT NULL,
                        fetched_at TEXT NOT NULL,
                        PRIMARY KEY (connector_id, source_account, source_id)
                    );

                    CREATE TABLE runs (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        connector_id TEXT NOT NULL,
                        account TEXT NOT NULL,
                        started_at TEXT NOT NULL,
                        finished_at TEXT NOT NULL,
                        manifest_json TEXT NOT NULL
                    );

                    CREATE TABLE curation_observations (
                        connector_id TEXT NOT NULL,
                        account TEXT NOT NULL,
                        report_json TEXT NOT NULL,
                        command_path TEXT,
                        materialization_fingerprint TEXT NOT NULL,
                        observed_at TEXT NOT NULL,
                        PRIMARY KEY (connector_id, account)
                    );

                    CREATE TABLE reconcile_receipts (
                        request_id TEXT PRIMARY KEY,
                        request_hash TEXT NOT NULL,
                        result_json TEXT NOT NULL,
                        created_at TEXT NOT NULL
                    );

                    CREATE TABLE purge_receipts (
                        request_id TEXT PRIMARY KEY,
                        request_hash TEXT NOT NULL,
                        result_json TEXT NOT NULL,
                        created_at TEXT NOT NULL
                    );

                    CREATE TABLE thread_evidence (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        thread_id TEXT NOT NULL,
                        occurred_from TEXT NOT NULL,
                        occurred_to TEXT NOT NULL,
                        body_text TEXT NOT NULL,
                        href TEXT,
                        tombstoned_at TEXT,
                        PRIMARY KEY (connector_id, source_account, thread_id)
                    );

                    CREATE TABLE participant_threads (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        participant TEXT NOT NULL,
                        thread_id TEXT NOT NULL,
                        last_interaction TEXT NOT NULL,
                        sampling_score INTEGER,
                        PRIMARY KEY (connector_id, source_account, participant, thread_id),
                        FOREIGN KEY (connector_id, source_account, thread_id)
                            REFERENCES thread_evidence(connector_id, source_account, thread_id)
                            ON DELETE CASCADE
                    );

                    CREATE INDEX idx_participant_threads_lookup
                        ON participant_threads(connector_id, source_account, participant);
                    CREATE INDEX idx_participant_threads_recent
                        ON participant_threads(connector_id, source_account, last_interaction DESC);
                    CREATE INDEX idx_thread_evidence_active
                        ON thread_evidence(
                            connector_id, source_account, occurred_to, tombstoned_at
                        );

                    CREATE TABLE calendar_event_evidence (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        source_id TEXT NOT NULL,
                        calendar_id TEXT NOT NULL,
                        occurred_from TEXT NOT NULL,
                        occurred_to TEXT,
                        title TEXT NOT NULL,
                        body_text TEXT NOT NULL,
                        href TEXT,
                        imported_at TEXT NOT NULL,
                        tombstoned_at TEXT,
                        PRIMARY KEY (connector_id, source_account, source_id)
                    );

                    CREATE TABLE calendar_event_attendees (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        source_id TEXT NOT NULL,
                        attendee_key TEXT NOT NULL,
                        position INTEGER NOT NULL,
                        display_name TEXT NOT NULL,
                        email TEXT,
                        response_status TEXT,
                        is_self INTEGER NOT NULL DEFAULT 0,
                        organizer INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY (
                            connector_id, source_account, source_id, attendee_key
                        ),
                        FOREIGN KEY (connector_id, source_account, source_id)
                            REFERENCES calendar_event_evidence(
                                connector_id, source_account, source_id
                            ) ON DELETE CASCADE
                    );

                    CREATE INDEX idx_calendar_event_evidence_active
                        ON calendar_event_evidence(
                            connector_id, source_account, occurred_from, tombstoned_at
                        );
                    CREATE INDEX idx_calendar_event_attendees_event
                        ON calendar_event_attendees(
                            connector_id, source_account, source_id, position
                        );

                    CREATE TABLE external_document_evidence (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        source_id TEXT NOT NULL,
                        occurred_at TEXT NOT NULL,
                        title TEXT NOT NULL,
                        body_text TEXT NOT NULL,
                        href TEXT,
                        attributes_json TEXT NOT NULL DEFAULT '{}',
                        imported_at TEXT NOT NULL,
                        tombstoned_at TEXT,
                        PRIMARY KEY (connector_id, source_account, source_id)
                    );

                    CREATE TABLE external_document_participants (
                        connector_id TEXT NOT NULL,
                        source_account TEXT NOT NULL,
                        source_id TEXT NOT NULL,
                        participant_key TEXT NOT NULL,
                        position INTEGER NOT NULL,
                        display_name TEXT NOT NULL,
                        email TEXT,
                        ambiguous INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY (
                            connector_id, source_account, source_id, participant_key
                        ),
                        FOREIGN KEY (connector_id, source_account, source_id)
                            REFERENCES external_document_evidence(
                                connector_id, source_account, source_id
                            ) ON DELETE CASCADE
                    );

                    CREATE INDEX idx_external_document_evidence_active
                        ON external_document_evidence(
                            connector_id, source_account, occurred_at, tombstoned_at
                        );
                    CREATE INDEX idx_external_document_participants_document
                        ON external_document_participants(
                            connector_id, source_account, source_id, position
                        );

                    CREATE INDEX idx_runs_connector ON runs(connector_id, account, started_at DESC);
                    "#,
                )
                .context("failed to initialize integrations schema")?;
            connection
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .context("failed to set schema version")?;
        } else if version != SCHEMA_VERSION {
            bail!(
                "unsupported ledger schema version {version}; delete and recreate the workspace ledger"
            );
        }
        Ok(())
    }

    /// Return the current connector observation used only as KnowledgePolicy
    /// input. It never authorizes materialization or changes corpus membership.
    pub fn latest_curation_observation(
        &self,
        connector_id: &str,
        account: &str,
        materialization_fingerprint: &str,
    ) -> Result<Option<CurationObservation>> {
        let connection = self.connect()?;
        let report_json = connection
            .query_row(
                "SELECT report_json FROM curation_observations
                 WHERE connector_id = ?1 AND account = ?2
                   AND materialization_fingerprint = ?3",
                params![connector_id, account, materialization_fingerprint],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to load current curation observation")?;
        report_json
            .map(|json| {
                serde_json::from_str(&json).context("failed to decode curation observation")
            })
            .transpose()
    }

    pub fn latest_curation_observation_for_account(
        &self,
        connector_id: &str,
        account: &str,
    ) -> Result<Option<CurationObservation>> {
        let connection = self.connect()?;
        let report_json = connection
            .query_row(
                "SELECT report_json FROM curation_observations
                 WHERE connector_id = ?1 AND account = ?2",
                params![connector_id, account],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to load curation observation")?;
        report_json
            .map(|json| {
                serde_json::from_str(&json).context("failed to decode curation observation")
            })
            .transpose()
    }

    pub fn acquire_reconcile_lock(&self) -> Result<File> {
        let path = self.workspace_state_dir.join("reconcile.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening reconcile lock {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("locking reconcile state {}", path.display()))?;
        Ok(file)
    }

    pub fn load_reconcile_receipt(
        &self,
        request_id: &str,
    ) -> Result<Option<(String, serde_json::Value)>> {
        let connection = self.connect()?;
        let row = connection
            .query_row(
                "SELECT request_hash, result_json FROM reconcile_receipts WHERE request_id = ?1",
                params![request_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .context("failed to load reconcile receipt")?;
        row.map(|(request_hash, result_json)| {
            Ok((
                request_hash,
                serde_json::from_str(&result_json).context("failed to decode reconcile receipt")?,
            ))
        })
        .transpose()
    }

    pub fn store_reconcile_receipt(
        &self,
        request_id: &str,
        request_hash: &str,
        result: &serde_json::Value,
    ) -> Result<()> {
        let result_json = serde_json::to_string(result).context("serializing reconcile receipt")?;
        self.connect()?
            .execute(
                "INSERT INTO reconcile_receipts (request_id, request_hash, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    request_id,
                    request_hash,
                    result_json,
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
                ],
            )
            .context("failed to persist reconcile receipt")?;
        Ok(())
    }

    /// Load the durable checkpoint a connector should use for its next fetch.
    pub fn load_cursor(
        &self,
        connector_id: &str,
        account: &str,
    ) -> Result<Option<serde_json::Value>> {
        let connection = self.connect()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT cursor_json FROM connectors WHERE connector_id = ?1 AND account = ?2",
                params![connector_id, account],
                |row| row.get(0),
            )
            .optional()
            .context("failed to load connector cursor")?
            .flatten();
        json.map(|value| serde_json::from_str(&value).context("failed to decode connector cursor"))
            .transpose()
    }

    /// Idempotently retain optional source-native transport rows without
    /// projecting any human-facing vault notes.
    pub fn ingest_raw_connector_items(
        &self,
        ctx: &ConnectorCtx,
        raw_items: Vec<RawItemDraft>,
    ) -> Result<()> {
        self.ingest_raw_connector_items_with_cursor(ctx, raw_items, None)
    }

    /// Atomically retain optional transport rows and the upstream checkpoint
    /// from which the next incremental refresh must continue.
    pub fn ingest_raw_connector_items_with_cursor(
        &self,
        ctx: &ConnectorCtx,
        raw_items: Vec<RawItemDraft>,
        cursor: Option<&serde_json::Value>,
    ) -> Result<()> {
        if !matches!(
            ctx.connector_id.as_str(),
            "email" | "gcal" | "google_meet" | "granola"
        ) {
            bail!("{} sources do not retain raw payloads", ctx.connector_id);
        }
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin raw connector ingest transaction")?;
        let fetched_at = Utc::now();
        for raw in raw_items {
            upsert_raw_item(
                &tx,
                &ctx.connector_id,
                &ctx.account,
                &raw.source_id,
                &raw.payload,
                fetched_at,
            )?;
        }
        if let Some(cursor) = cursor {
            let cursor_json =
                serde_json::to_string(cursor).context("failed to serialize connector cursor")?;
            tx.execute(
                r#"
                INSERT INTO connectors (connector_id, account, cursor_json, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(connector_id, account) DO UPDATE SET
                    cursor_json = excluded.cursor_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    cursor_json,
                    fetched_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                ],
            )
            .context("failed to persist connector cursor")?;
        }
        tx.commit()
            .context("failed to commit raw connector ingest")?;
        Ok(())
    }

    /// Load the durable raw corpus for one connector account in stable order.
    pub fn raw_items(&self, ctx: &ConnectorCtx) -> Result<Vec<RawItemDraft>> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT source_id, payload_json FROM raw_items \
                 WHERE connector_id = ?1 AND source_account = ?2 ORDER BY source_id",
            )
            .context("failed to prepare raw item query")?;
        let rows = statement
            .query_map(params![ctx.connector_id, ctx.account], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("failed to query raw items")?;
        rows.map(|row| {
            let (source_id, payload_json) = row.context("failed to read raw item")?;
            Ok(RawItemDraft {
                source_id,
                payload: serde_json::from_str(&payload_json)
                    .context("failed to decode raw item payload")?,
            })
        })
        .collect()
    }

    pub fn latest_run_manifest(
        &self,
        connector_id: &str,
        account: &str,
    ) -> Result<Option<RunManifest>> {
        let connection = self.connect()?;
        let json: Option<String> = connection
            .query_row(
                r#"
                SELECT manifest_json FROM runs
                WHERE connector_id = ?1 AND account = ?2
                ORDER BY id DESC LIMIT 1
                "#,
                params![connector_id, account],
                |row| row.get(0),
            )
            .optional()
            .context("failed to load latest run manifest")?;
        json.map(|value| serde_json::from_str(&value).context("failed to decode run manifest"))
            .transpose()
    }

    pub fn run_count(&self, connector_id: &str, account: &str) -> Result<u64> {
        let connection = self.connect()?;
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE connector_id = ?1 AND account = ?2",
                params![connector_id, account],
                |row| row.get(0),
            )
            .context("failed to count connector runs")?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    pub fn update_health(
        &self,
        ctx: &ConnectorCtx,
        status: HealthStatus,
        detail: Option<&str>,
    ) -> Result<HealthReport> {
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin health transaction")?;
        let now = Utc::now();
        update_connector_health(&tx, ctx, status, detail, now)?;
        tx.commit().context("failed to commit health update")?;
        self.build_health_report(ctx, status, detail, now)
    }

    /// Persist a failed reconcile without disturbing the last successful cursor or
    /// any previously materialized authoritative records.
    pub fn record_failed_reconcile(&self, ctx: &ConnectorCtx, detail: &str) -> Result<()> {
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin failed-reconcile transaction")?;
        let now = Utc::now();
        let manifest = RunManifest {
            errors: vec![detail.to_string()],
            ..RunManifest::default()
        };
        record_run(&tx, ctx, now, now, &manifest)?;
        update_connector_health(&tx, ctx, HealthStatus::Error, Some(detail), now)?;
        tx.commit()
            .context("failed to commit failed-reconcile status")?;
        Ok(())
    }

    /// Mark materialized Google snapshots unavailable for refresh after their
    /// machine credential is explicitly forgotten. This updates only existing
    /// connector rows: it never creates a ledger, connector declaration, or
    /// evidence row, and it preserves the last successful sync timestamp.
    pub fn mark_google_connection_needs_auth(&self, account: &str) -> Result<usize> {
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin Google connection health transaction")?;
        let updated = tx
            .execute(
                r#"
                UPDATE connectors
                SET health_status = 'needs-auth',
                    health_detail_json = ?1,
                    updated_at = ?2
                WHERE account = ?3
                  AND connector_id IN ('email', 'gcal', 'google_meet')
                "#,
                params![
                    serde_json::json!({
                        "detail": "machine Google connection was forgotten"
                    })
                    .to_string(),
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                    account
                ],
            )
            .context("failed to mark Google connector health needs-auth")?;
        tx.commit()
            .context("failed to commit Google connection health transaction")?;
        Ok(updated)
    }

    /// Mark materialized Granola snapshots unavailable for refresh after their
    /// machine credential is explicitly forgotten. This updates only existing
    /// connector rows and preserves the last successful sync timestamp.
    pub fn mark_granola_connection_needs_auth(&self, account: &str) -> Result<usize> {
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin Granola connection health transaction")?;
        let updated = tx
            .execute(
                r#"
                UPDATE connectors
                SET health_status = 'needs-auth',
                    health_detail_json = ?1,
                    updated_at = ?2
                WHERE account = ?3
                  AND connector_id = 'granola'
                "#,
                params![
                    serde_json::json!({
                        "detail": "machine Granola connection was forgotten"
                    })
                    .to_string(),
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                    account
                ],
            )
            .context("failed to mark Granola connector health needs-auth")?;
        tx.commit()
            .context("failed to commit Granola connection health transaction")?;
        Ok(updated)
    }

    pub fn health_report(&self, ctx: &ConnectorCtx) -> Result<HealthReport> {
        let connection = self.connect()?;
        let row: Option<(String, Option<String>, Option<String>)> = connection
            .query_row(
                "SELECT health_status, health_detail_json, last_sync_at FROM connectors WHERE connector_id = ?1 AND account = ?2",
                params![ctx.connector_id, ctx.account],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .context("failed to load connector health")?;
        let Some((status, detail_json, last_sync_at)) = row else {
            return Ok(HealthReport {
                status: HealthStatus::Stale,
                reason: Some("never_refreshed".to_string()),
                last_successful_sync: None,
                cursor_age_secs: None,
                detail: Some("connector has not completed a successful reconcile".to_string()),
            });
        };
        let (status, reason) = match status.as_str() {
            "fresh" => (HealthStatus::Fresh, None),
            "stale" | "unknown" => (HealthStatus::Stale, Some("refresh_required".to_string())),
            "needs-auth" => (
                HealthStatus::NeedsAuth,
                Some("credentials_unavailable".to_string()),
            ),
            "error" => (HealthStatus::Error, Some("refresh_failed".to_string())),
            other => bail!("unknown connector health status: {other}"),
        };
        let last_successful_sync = last_sync_at
            .as_deref()
            .map(|value| parse_timestamp(value, "last_sync_at"))
            .transpose()?;
        let detail = detail_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
            .and_then(|value| value.get("detail")?.as_str().map(str::to_string));
        Ok(HealthReport {
            status,
            reason,
            last_successful_sync,
            cursor_age_secs: last_successful_sync.map(|sync| (Utc::now() - sync).num_seconds()),
            detail,
        })
    }

    /// Return the desired-state receipt stored atomically with the last
    /// coherent authoritative materialization snapshot.
    pub fn materialization_fingerprint(&self, ctx: &ConnectorCtx) -> Result<Option<String>> {
        self.connect()?
            .query_row(
                "SELECT materialization_fingerprint FROM connectors WHERE connector_id = ?1 AND account = ?2",
                params![ctx.connector_id, ctx.account],
                |row| row.get(0),
            )
            .optional()
            .context("failed to load connector materialization fingerprint")
            .map(Option::flatten)
    }

    /// Health with desired-state materialization freshness applied. Transport
    /// failures retain their stronger status; a coherent but differently
    /// scoped snapshot becomes stale until refresh succeeds.
    pub fn health_report_for_materialization(
        &self,
        ctx: &ConnectorCtx,
        expected_fingerprint: &str,
    ) -> Result<HealthReport> {
        let mut report = self.health_report(ctx)?;
        if report.status == HealthStatus::Fresh
            && self.materialization_fingerprint(ctx)?.as_deref() != Some(expected_fingerprint)
        {
            report.status = HealthStatus::Stale;
            report.reason = Some("refresh_required".to_string());
            report.detail = Some(if ctx.connector_id == "email" {
                "workspace Gmail selector changed; refresh required".to_string()
            } else {
                format!(
                    "workspace {} collection selector changed; refresh required",
                    ctx.connector_id
                )
            });
        }
        Ok(report)
    }

    /// Replace the complete email thread evidence snapshot for one connector account.
    pub fn replace_email_thread_snapshot(
        &self,
        ctx: &ConnectorCtx,
        threads: Vec<ThreadEvidence>,
        associations: Vec<ParticipantThread>,
    ) -> Result<EmailThreadSnapshotCounts> {
        self.replace_email_thread_snapshot_inner(ctx, threads, associations, None, None, None)
    }

    /// Replace an authoritative Gmail snapshot and atomically receipt the
    /// desired-state selector that produced it.
    pub fn replace_email_thread_snapshot_with_materialization_fingerprint(
        &self,
        ctx: &ConnectorCtx,
        threads: Vec<ThreadEvidence>,
        associations: Vec<ParticipantThread>,
        materialization_fingerprint: &str,
    ) -> Result<EmailThreadSnapshotCounts> {
        self.replace_email_thread_snapshot_inner(
            ctx,
            threads,
            associations,
            Some(materialization_fingerprint),
            None,
            None,
        )
    }

    /// Atomically replace authoritative Gmail evidence and its observable
    /// KnowledgePolicy input report for the same coherent snapshot.
    pub fn replace_email_thread_snapshot_with_policy_report(
        &self,
        ctx: &ConnectorCtx,
        threads: Vec<ThreadEvidence>,
        associations: Vec<ParticipantThread>,
        report: &CurationObservation,
        command_path: Option<&Path>,
        materialization_fingerprint: &str,
        observed_at: DateTime<Utc>,
        next_cursor: Option<&serde_json::Value>,
        expected_workspace_revision: Option<&str>,
    ) -> Result<EmailThreadSnapshotCounts> {
        self.replace_email_thread_snapshot_inner(
            ctx,
            threads,
            associations,
            Some(materialization_fingerprint),
            Some((
                report,
                command_path,
                materialization_fingerprint,
                observed_at,
                next_cursor,
            )),
            expected_workspace_revision,
        )
    }

    fn replace_email_thread_snapshot_inner(
        &self,
        ctx: &ConnectorCtx,
        threads: Vec<ThreadEvidence>,
        associations: Vec<ParticipantThread>,
        materialization_fingerprint: Option<&str>,
        policy_report: Option<(
            &CurationObservation,
            Option<&Path>,
            &str,
            DateTime<Utc>,
            Option<&serde_json::Value>,
        )>,
        expected_workspace_revision: Option<&str>,
    ) -> Result<EmailThreadSnapshotCounts> {
        if ctx.connector_id != EMAIL_CONNECTOR_ID {
            bail!("thread evidence replacement is email-only");
        }
        if materialization_fingerprint.is_some_and(str::is_empty) {
            bail!("email materialization fingerprint cannot be empty");
        }
        let policy_cache = policy_report
            .map(
                |(report, command_path, materialization_fingerprint, observed_at, next_cursor)| {
                    if report.connector_id != ctx.connector_id || report.account != ctx.account {
                        bail!("email policy report does not match connector context");
                    }
                    Ok::<_, anyhow::Error>((
                        serde_json::to_string(report).context("failed to serialize observation")?,
                        command_path.map(|path| path.to_string_lossy().into_owned()),
                        materialization_fingerprint.to_string(),
                        observed_at,
                        next_cursor
                            .map(serde_json::to_string)
                            .transpose()
                            .context("failed to serialize email cursor")?,
                    ))
                },
            )
            .transpose()?;
        for thread in &threads {
            if thread.thread_id.trim().is_empty() {
                bail!("thread evidence requires a non-empty thread_id");
            }
            if thread.occurred_from > thread.occurred_to {
                bail!(
                    "thread evidence {} has occurred_from after occurred_to",
                    thread.thread_id
                );
            }
        }
        let thread_ids: BTreeSet<_> = threads
            .iter()
            .map(|thread| thread.thread_id.as_str())
            .collect();
        if thread_ids.len() != threads.len() {
            bail!("duplicate thread_id in email thread snapshot");
        }
        for association in &associations {
            if association.participant.trim().is_empty() || association.thread_id.trim().is_empty()
            {
                bail!("participant association requires non-empty participant and thread_id");
            }
            if !thread_ids.contains(association.thread_id.as_str()) {
                bail!(
                    "participant association references absent thread {}",
                    association.thread_id
                );
            }
            if let Some(score) = association.sampling_score {
                i64::try_from(score).with_context(|| {
                    format!(
                        "sampling score for {} in {} exceeds SQLite integer range",
                        association.participant, association.thread_id
                    )
                })?;
            }
        }

        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin email snapshot transaction")?;

        let mut existing: BTreeMap<
            String,
            (String, String, String, Option<String>, Option<String>),
        > = BTreeMap::new();
        {
            let mut statement = tx.prepare(
                "SELECT thread_id, occurred_from, occurred_to, body_text, href, tombstoned_at
                 FROM thread_evidence
                 WHERE connector_id = ?1 AND source_account = ?2",
            )?;
            let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?;
            for row in rows {
                let (thread_id, occurred_from, occurred_to, body_text, href, tombstoned_at) =
                    row.context("failed to read thread evidence")?;
                existing.insert(
                    thread_id,
                    (occurred_from, occurred_to, body_text, href, tombstoned_at),
                );
            }
        }

        let mut counts = EmailThreadSnapshotCounts::default();
        let snapshot_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        for thread in &threads {
            let href = thread.href.as_deref();
            let occurred_from = thread
                .occurred_from
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            let occurred_to = thread
                .occurred_to
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            if let Some((existing_from, existing_to, existing_body, existing_href, tombstoned_at)) =
                existing.remove(&thread.thread_id)
            {
                if existing_from == occurred_from
                    && existing_to == occurred_to
                    && existing_body == thread.body_text
                    && existing_href.as_deref() == href
                    && tombstoned_at.is_none()
                {
                    counts.threads_unchanged += 1;
                    continue;
                }
                tx.execute(
                    r#"
                    UPDATE thread_evidence
                    SET occurred_from = ?4, occurred_to = ?5, body_text = ?6, href = ?7,
                        tombstoned_at = NULL
                    WHERE connector_id = ?1 AND source_account = ?2 AND thread_id = ?3
                    "#,
                    params![
                        ctx.connector_id,
                        ctx.account,
                        thread.thread_id,
                        occurred_from,
                        occurred_to,
                        thread.body_text,
                        href,
                    ],
                )
                .context("failed to update thread evidence")?;
                counts.threads_updated += 1;
            } else {
                tx.execute(
                    r#"
                    INSERT INTO thread_evidence (
                        connector_id, source_account, thread_id,
                        occurred_from, occurred_to, body_text, href, tombstoned_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
                    "#,
                    params![
                        ctx.connector_id,
                        ctx.account,
                        thread.thread_id,
                        occurred_from,
                        occurred_to,
                        thread.body_text,
                        href,
                    ],
                )
                .context("failed to insert thread evidence")?;
                counts.threads_written += 1;
            }
        }

        for thread_id in existing.keys() {
            tx.execute(
                "UPDATE thread_evidence
                 SET tombstoned_at = ?4
                 WHERE connector_id = ?1 AND source_account = ?2 AND thread_id = ?3
                   AND tombstoned_at IS NULL",
                params![ctx.connector_id, ctx.account, thread_id, snapshot_at],
            )
            .context("failed to tombstone absent thread evidence")?;
            if existing
                .get(thread_id)
                .is_some_and(|(_, _, _, _, tombstoned_at)| tombstoned_at.is_none())
            {
                counts.threads_deleted += 1;
            }
        }

        tx.execute(
            "DELETE FROM participant_threads
             WHERE connector_id = ?1 AND source_account = ?2
               AND thread_id IN (
                   SELECT thread_id FROM thread_evidence
                   WHERE connector_id = ?1 AND source_account = ?2
                     AND tombstoned_at IS NULL
               )",
            params![ctx.connector_id, ctx.account],
        )
        .context("failed to clear active participant associations")?;

        for association in associations {
            let sampling_score = association
                .sampling_score
                .map(i64::try_from)
                .transpose()
                .context("validated sampling score exceeded SQLite integer range")?;
            tx.execute(
                r#"
                INSERT INTO participant_threads (
                    connector_id, source_account, participant, thread_id,
                    last_interaction, sampling_score
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    association.participant,
                    association.thread_id,
                    association
                        .last_interaction
                        .to_rfc3339_opts(SecondsFormat::Secs, true),
                    sampling_score,
                ],
            )
            .context("failed to insert participant association")?;
        }

        if let Some(materialization_fingerprint) = materialization_fingerprint {
            tx.execute(
                r#"
                INSERT INTO connectors (
                    connector_id, account, materialization_fingerprint, updated_at
                ) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(connector_id, account) DO UPDATE SET
                    materialization_fingerprint = excluded.materialization_fingerprint,
                    updated_at = excluded.updated_at
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    materialization_fingerprint,
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                ],
            )
            .context("failed to receipt email materialization selector")?;
        } else {
            tx.execute(
                "UPDATE connectors
                 SET materialization_fingerprint = NULL, updated_at = ?3
                 WHERE connector_id = ?1 AND account = ?2",
                params![
                    ctx.connector_id,
                    ctx.account,
                    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                ],
            )
            .context("failed to clear absent email materialization receipt")?;
        }

        if let Some((
            report_json,
            command_path,
            materialization_fingerprint,
            observed_at,
            cursor_json,
        )) = policy_cache
        {
            let observed_at_text = observed_at.to_rfc3339_opts(SecondsFormat::Secs, true);
            tx.execute(
                r#"
                INSERT INTO curation_observations (
                    connector_id, account, report_json, command_path,
                    materialization_fingerprint, observed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(connector_id, account) DO UPDATE SET
                    report_json = excluded.report_json,
                    command_path = excluded.command_path,
                    materialization_fingerprint = excluded.materialization_fingerprint,
                    observed_at = excluded.observed_at
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    report_json,
                    command_path,
                    materialization_fingerprint,
                    observed_at_text,
                ],
            )
            .context("failed to store email curation evidence")?;

            tx.execute(
                r#"
                UPDATE connectors
                SET cursor_json = ?3,
                    last_sync_at = ?4,
                    health_status = 'fresh',
                    health_detail_json = NULL,
                    updated_at = ?4
                WHERE connector_id = ?1 AND account = ?2
                "#,
                params![ctx.connector_id, ctx.account, cursor_json, observed_at_text,],
            )
            .context("failed to receipt successful email reconciliation")?;
            let manifest = RunManifest {
                records_written: counts.threads_written,
                records_updated: counts.threads_updated,
                records_unchanged: counts.threads_unchanged,
                tombstones: counts.threads_deleted,
                ..RunManifest::default()
            };
            record_run(&tx, ctx, observed_at, observed_at, &manifest)?;
            update_connector_counts(&tx, ctx, &manifest)?;
        }

        let _revision_guard = expected_workspace_revision
            .map(|expected| lock_workspace_revision(&self.workspace_state_dir, expected))
            .transpose()?;
        tx.commit().context("failed to commit email snapshot")?;
        Ok(counts)
    }

    /// Atomically reconcile authoritative Calendar evidence, its attendee
    /// associations, the optional raw transport cache, cursor, freshness
    /// receipt, health, and run receipt. No Markdown projection row is
    /// created by this path.
    pub fn apply_calendar_event_delta(
        &self,
        ctx: &ConnectorCtx,
        delta: CalendarEventDelta,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult> {
        if ctx.connector_id != "gcal" {
            bail!("Calendar event reconciliation requires connector_id=gcal");
        }
        if delta.materialization_fingerprint.is_empty() {
            bail!("Calendar materialization fingerprint cannot be empty");
        }
        if delta.scope.occurred_from > delta.scope.occurred_to {
            bail!("Calendar materialization scope starts after it ends");
        }

        let event_ids = delta
            .events
            .iter()
            .map(|event| event.source_id.as_str())
            .collect::<BTreeSet<_>>();
        if event_ids.len() != delta.events.len() {
            bail!("duplicate source_id in Calendar event delta");
        }
        for event in &delta.events {
            if event.source_id.trim().is_empty()
                || event.calendar_id.trim().is_empty()
                || event.title.trim().is_empty()
                || event.body_text.trim().is_empty()
            {
                bail!("Calendar evidence requires source_id, calendar_id, title, and body_text");
            }
            if event
                .occurred_to
                .is_some_and(|occurred_to| occurred_to < event.occurred_from)
            {
                bail!("Calendar event {} ends before it starts", event.source_id);
            }
            if event.occurred_from < delta.scope.occurred_from
                || event.occurred_from > delta.scope.occurred_to
            {
                bail!(
                    "Calendar event {} falls outside the materialization scope",
                    event.source_id
                );
            }
        }
        let mut attendee_keys = BTreeSet::new();
        for attendee in &delta.attendees {
            if attendee.source_id.trim().is_empty()
                || attendee.attendee_key.trim().is_empty()
                || attendee.display_name.trim().is_empty()
            {
                bail!(
                    "Calendar attendee association requires source_id, attendee_key, and display_name"
                );
            }
            if !event_ids.contains(attendee.source_id.as_str()) {
                bail!(
                    "Calendar attendee references absent event {}",
                    attendee.source_id
                );
            }
            if !attendee_keys.insert((attendee.source_id.as_str(), attendee.attendee_key.as_str()))
            {
                bail!(
                    "duplicate attendee {} for Calendar event {}",
                    attendee.attendee_key,
                    attendee.source_id
                );
            }
        }

        let started_at = Utc::now();
        let imported_at = Utc::now();
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin Calendar reconciliation transaction")?;
        let mut manifest = RunManifest::default();

        for raw in &delta.raw_items {
            upsert_raw_item(
                &tx,
                &ctx.connector_id,
                &ctx.account,
                &raw.source_id,
                &raw.payload,
                imported_at,
            )?;
        }

        let attendees_by_event = delta.attendees.iter().fold(
            BTreeMap::<&str, Vec<&CalendarEventAttendee>>::new(),
            |mut grouped, attendee| {
                grouped
                    .entry(attendee.source_id.as_str())
                    .or_default()
                    .push(attendee);
                grouped
            },
        );

        for event in &delta.events {
            let occurred_from = timestamp_text(event.occurred_from);
            let occurred_to = event.occurred_to.map(timestamp_text);
            let existing = tx
                .query_row(
                    r#"
                    SELECT calendar_id, occurred_from, occurred_to, title, body_text, href,
                           tombstoned_at
                    FROM calendar_event_evidence
                    WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                    "#,
                    params![ctx.connector_id, ctx.account, event.source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .optional()
                .context("failed to load existing Calendar event")?;

            let mut existing_attendees = Vec::new();
            {
                let mut statement = tx.prepare(
                    r#"
                    SELECT attendee_key, position, display_name, email, response_status,
                           is_self, organizer
                    FROM calendar_event_attendees
                    WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                    ORDER BY position, attendee_key
                    "#,
                )?;
                let rows = statement.query_map(
                    params![ctx.connector_id, ctx.account, event.source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, bool>(5)?,
                            row.get::<_, bool>(6)?,
                        ))
                    },
                )?;
                for row in rows {
                    existing_attendees.push(row.context("failed to read Calendar attendee")?);
                }
            }
            let mut incoming_attendees = attendees_by_event
                .get(event.source_id.as_str())
                .cloned()
                .unwrap_or_default();
            incoming_attendees.sort_by_key(|attendee| (attendee.position, &attendee.attendee_key));
            let attendees_equal = existing_attendees.len() == incoming_attendees.len()
                && existing_attendees.iter().zip(&incoming_attendees).all(
                    |(existing, incoming)| {
                        existing.0 == incoming.attendee_key
                            && u32::try_from(existing.1).ok() == Some(incoming.position)
                            && existing.2 == incoming.display_name
                            && existing.3 == incoming.email
                            && existing.4 == incoming.response_status
                            && existing.5 == incoming.is_self
                            && existing.6 == incoming.organizer
                    },
                );
            let event_equal = existing.as_ref().is_some_and(|existing| {
                existing.0 == event.calendar_id
                    && existing.1 == occurred_from
                    && existing.2 == occurred_to
                    && existing.3 == event.title
                    && existing.4 == event.body_text
                    && existing.5 == event.href
                    && existing.6.is_none()
            });

            tx.execute(
                r#"
                INSERT INTO calendar_event_evidence (
                    connector_id, source_account, source_id, calendar_id,
                    occurred_from, occurred_to, title, body_text, href,
                    imported_at, tombstoned_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)
                ON CONFLICT(connector_id, source_account, source_id) DO UPDATE SET
                    calendar_id = excluded.calendar_id,
                    occurred_from = excluded.occurred_from,
                    occurred_to = excluded.occurred_to,
                    title = excluded.title,
                    body_text = excluded.body_text,
                    href = excluded.href,
                    imported_at = excluded.imported_at,
                    tombstoned_at = NULL
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    event.source_id,
                    event.calendar_id,
                    occurred_from,
                    occurred_to,
                    event.title,
                    event.body_text,
                    event.href,
                    timestamp_text(imported_at),
                ],
            )
            .context("failed to upsert Calendar event evidence")?;

            if !attendees_equal {
                tx.execute(
                    "DELETE FROM calendar_event_attendees
                     WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3",
                    params![ctx.connector_id, ctx.account, event.source_id],
                )?;
                for attendee in incoming_attendees {
                    tx.execute(
                        r#"
                        INSERT INTO calendar_event_attendees (
                            connector_id, source_account, source_id, attendee_key,
                            position, display_name, email, response_status, is_self, organizer
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                        "#,
                        params![
                            ctx.connector_id,
                            ctx.account,
                            attendee.source_id,
                            attendee.attendee_key,
                            i64::from(attendee.position),
                            attendee.display_name,
                            attendee.email,
                            attendee.response_status,
                            attendee.is_self,
                            attendee.organizer,
                        ],
                    )
                    .context("failed to insert Calendar attendee association")?;
                }
            }

            if existing.is_none() {
                manifest.records_written += 1;
            } else if event_equal && attendees_equal {
                manifest.records_unchanged += 1;
            } else {
                manifest.records_updated += 1;
            }
        }

        let mut tombstone_ids = delta
            .tombstone_source_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        if delta.complete_snapshot {
            let mut statement = tx.prepare(
                r#"
                SELECT source_id, occurred_from FROM calendar_event_evidence
                WHERE connector_id = ?1 AND source_account = ?2
                  AND tombstoned_at IS NULL
                "#,
            )?;
            let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source_id, occurred_from) =
                    row.context("failed to read Calendar snapshot member")?;
                let occurred_from = parse_timestamp(&occurred_from, "occurred_from")?;
                let in_scope = occurred_from >= delta.scope.occurred_from
                    && occurred_from <= delta.scope.occurred_to;
                if in_scope && !event_ids.contains(source_id.as_str()) {
                    tombstone_ids.insert(source_id);
                }
            }
        }
        for source_id in tombstone_ids {
            let changed = tx
                .execute(
                    r#"
                    UPDATE calendar_event_evidence
                    SET tombstoned_at = ?1, imported_at = ?1
                    WHERE connector_id = ?2 AND source_account = ?3 AND source_id = ?4
                      AND tombstoned_at IS NULL
                    "#,
                    params![
                        timestamp_text(imported_at),
                        ctx.connector_id,
                        ctx.account,
                        source_id,
                    ],
                )
                .context("failed to tombstone Calendar event")?;
            manifest.tombstones += u64::try_from(changed).unwrap_or(0);
        }

        let cursor_json = delta
            .next_cursor
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize Calendar cursor")?;
        let scope_json = serde_json::to_string(&delta.scope)
            .context("failed to serialize Calendar materialization scope")?;
        let finished_at = Utc::now();
        tx.execute(
            r#"
            INSERT INTO connectors (
                connector_id, account, cursor_json, last_sync_at,
                scope_boundary_json, materialization_fingerprint,
                health_status, health_detail_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'fresh', NULL, ?4)
            ON CONFLICT(connector_id, account) DO UPDATE SET
                cursor_json = excluded.cursor_json,
                last_sync_at = excluded.last_sync_at,
                scope_boundary_json = excluded.scope_boundary_json,
                materialization_fingerprint = excluded.materialization_fingerprint,
                health_status = excluded.health_status,
                health_detail_json = excluded.health_detail_json,
                updated_at = excluded.updated_at
            "#,
            params![
                ctx.connector_id,
                ctx.account,
                cursor_json,
                timestamp_text(finished_at),
                scope_json,
                delta.materialization_fingerprint,
            ],
        )
        .context("failed to receipt Calendar materialization")?;
        record_run(&tx, ctx, started_at, finished_at, &manifest)?;
        update_connector_counts(&tx, ctx, &manifest)?;
        let _revision_guard = expected_workspace_revision
            .map(|expected| lock_workspace_revision(&self.workspace_state_dir, expected))
            .transpose()?;
        tx.commit()
            .context("failed to commit Calendar reconciliation")?;

        Ok(ReconcileResult {
            records_written: manifest.records_written,
            records_updated: manifest.records_updated,
            records_unchanged: manifest.records_unchanged,
            tombstones: manifest.tombstones,
            next_cursor: delta.next_cursor,
            manifest,
        })
    }

    /// Atomically reconcile authoritative external document evidence for the
    /// two concrete document-like connectors: Google Meet and Granola. Raw
    /// transport cache, when supplied, is independent; no Markdown
    /// projection is created by this transaction.
    pub fn apply_external_document_delta(
        &self,
        ctx: &ConnectorCtx,
        delta: ExternalDocumentDelta,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult> {
        if !matches!(ctx.connector_id.as_str(), "google_meet" | "granola") {
            bail!(
                "external document reconciliation does not support connector {}",
                ctx.connector_id
            );
        }
        if delta.materialization_fingerprint.trim().is_empty() {
            bail!("external document materialization fingerprint cannot be empty");
        }
        if delta
            .snapshot_scope
            .as_ref()
            .is_some_and(|scope| scope.occurred_from > scope.occurred_to)
        {
            bail!("external document snapshot scope starts after it ends");
        }

        let document_ids = delta
            .documents
            .iter()
            .map(|document| document.source_id.as_str())
            .collect::<BTreeSet<_>>();
        if document_ids.len() != delta.documents.len() {
            bail!("duplicate source_id in external document delta");
        }
        for document in &delta.documents {
            if document.source_id.trim().is_empty()
                || document.title.trim().is_empty()
                || document.body_text.trim().is_empty()
            {
                bail!("external document evidence requires source_id, title, and body_text");
            }
            if delta.snapshot_scope.as_ref().is_some_and(|scope| {
                document.occurred_at < scope.occurred_from
                    || document.occurred_at > scope.occurred_to
            }) {
                bail!(
                    "external document {} falls outside the snapshot scope",
                    document.source_id
                );
            }
        }
        let mut participant_keys = BTreeSet::new();
        for participant in &delta.participants {
            if participant.source_id.trim().is_empty()
                || participant.participant_key.trim().is_empty()
                || participant.display_name.trim().is_empty()
            {
                bail!(
                    "external document participant requires source_id, participant_key, and display_name"
                );
            }
            if !document_ids.contains(participant.source_id.as_str()) {
                bail!(
                    "external document participant references absent document {}",
                    participant.source_id
                );
            }
            if !participant_keys.insert((
                participant.source_id.as_str(),
                participant.participant_key.as_str(),
            )) {
                bail!(
                    "duplicate participant {} for external document {}",
                    participant.participant_key,
                    participant.source_id
                );
            }
        }
        for raw in &delta.raw_items {
            if !document_ids.contains(raw.source_id.as_str()) {
                bail!(
                    "external raw item references absent document {}",
                    raw.source_id
                );
            }
        }

        let started_at = Utc::now();
        let imported_at = Utc::now();
        let mut connection = self.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("failed to begin external document reconciliation transaction")?;
        let mut manifest = RunManifest::default();

        for raw in &delta.raw_items {
            upsert_raw_item(
                &tx,
                &ctx.connector_id,
                &ctx.account,
                &raw.source_id,
                &raw.payload,
                imported_at,
            )?;
        }

        let participants_by_document = delta.participants.iter().fold(
            BTreeMap::<&str, Vec<&ExternalDocumentParticipant>>::new(),
            |mut grouped, participant| {
                grouped
                    .entry(participant.source_id.as_str())
                    .or_default()
                    .push(participant);
                grouped
            },
        );

        for document in &delta.documents {
            let occurred_at = timestamp_text(document.occurred_at);
            let attributes_json = serde_json::to_string(&document.attributes)
                .context("failed to serialize external document attributes")?;
            let existing = tx
                .query_row(
                    r#"
                    SELECT occurred_at, title, body_text, href, attributes_json,
                           tombstoned_at
                    FROM external_document_evidence
                    WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                    "#,
                    params![ctx.connector_id, ctx.account, document.source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()
                .context("failed to load existing external document")?;

            let mut existing_participants = Vec::new();
            {
                let mut statement = tx.prepare(
                    r#"
                    SELECT participant_key, position, display_name, email, ambiguous
                    FROM external_document_participants
                    WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3
                    ORDER BY position, participant_key
                    "#,
                )?;
                let rows = statement.query_map(
                    params![ctx.connector_id, ctx.account, document.source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, bool>(4)?,
                        ))
                    },
                )?;
                for row in rows {
                    existing_participants
                        .push(row.context("failed to read external document participant")?);
                }
            }
            let mut incoming_participants = participants_by_document
                .get(document.source_id.as_str())
                .cloned()
                .unwrap_or_default();
            incoming_participants
                .sort_by_key(|participant| (participant.position, &participant.participant_key));
            let participants_equal = existing_participants.len() == incoming_participants.len()
                && existing_participants
                    .iter()
                    .zip(&incoming_participants)
                    .all(|(existing, incoming)| {
                        existing.0 == incoming.participant_key
                            && u32::try_from(existing.1).ok() == Some(incoming.position)
                            && existing.2 == incoming.display_name
                            && existing.3 == incoming.email
                            && existing.4 == incoming.ambiguous
                    });
            let document_equal = existing.as_ref().is_some_and(|existing| {
                existing.0 == occurred_at
                    && existing.1 == document.title
                    && existing.2 == document.body_text
                    && existing.3 == document.href
                    && existing.4 == attributes_json
                    && existing.5.is_none()
            });

            tx.execute(
                r#"
                INSERT INTO external_document_evidence (
                    connector_id, source_account, source_id, occurred_at,
                    title, body_text, href, attributes_json, imported_at,
                    tombstoned_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)
                ON CONFLICT(connector_id, source_account, source_id) DO UPDATE SET
                    occurred_at = excluded.occurred_at,
                    title = excluded.title,
                    body_text = excluded.body_text,
                    href = excluded.href,
                    attributes_json = excluded.attributes_json,
                    imported_at = excluded.imported_at,
                    tombstoned_at = NULL
                "#,
                params![
                    ctx.connector_id,
                    ctx.account,
                    document.source_id,
                    occurred_at,
                    document.title,
                    document.body_text,
                    document.href,
                    attributes_json,
                    timestamp_text(imported_at),
                ],
            )
            .context("failed to upsert external document evidence")?;

            if !participants_equal {
                tx.execute(
                    "DELETE FROM external_document_participants
                     WHERE connector_id = ?1 AND source_account = ?2 AND source_id = ?3",
                    params![ctx.connector_id, ctx.account, document.source_id],
                )?;
                for participant in incoming_participants {
                    tx.execute(
                        r#"
                        INSERT INTO external_document_participants (
                            connector_id, source_account, source_id, participant_key,
                            position, display_name, email, ambiguous
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                        "#,
                        params![
                            ctx.connector_id,
                            ctx.account,
                            participant.source_id,
                            participant.participant_key,
                            i64::from(participant.position),
                            participant.display_name,
                            participant.email,
                            participant.ambiguous,
                        ],
                    )
                    .context("failed to insert external document participant")?;
                }
            }

            if existing.is_none() {
                manifest.records_written += 1;
            } else if document_equal && participants_equal {
                manifest.records_unchanged += 1;
            } else {
                manifest.records_updated += 1;
            }
        }

        let mut tombstone_ids = delta
            .tombstone_source_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        if delta.complete_snapshot {
            let mut statement = tx.prepare(
                r#"
                SELECT source_id, occurred_at FROM external_document_evidence
                WHERE connector_id = ?1 AND source_account = ?2
                  AND tombstoned_at IS NULL
                "#,
            )?;
            let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source_id, occurred_at) =
                    row.context("failed to read external document snapshot member")?;
                let in_scope = match delta.snapshot_scope.as_ref() {
                    None => true,
                    Some(scope) => {
                        let occurred_at = parse_timestamp(&occurred_at, "occurred_at")?;
                        occurred_at >= scope.occurred_from && occurred_at <= scope.occurred_to
                    }
                };
                if in_scope && !document_ids.contains(source_id.as_str()) {
                    tombstone_ids.insert(source_id);
                }
            }
        }
        for source_id in tombstone_ids {
            let changed = tx
                .execute(
                    r#"
                    UPDATE external_document_evidence
                    SET tombstoned_at = ?1, imported_at = ?1
                    WHERE connector_id = ?2 AND source_account = ?3 AND source_id = ?4
                      AND tombstoned_at IS NULL
                    "#,
                    params![
                        timestamp_text(imported_at),
                        ctx.connector_id,
                        ctx.account,
                        source_id,
                    ],
                )
                .context("failed to tombstone external document")?;
            manifest.tombstones += u64::try_from(changed).unwrap_or(0);
        }

        let cursor_json = delta
            .next_cursor
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize external document cursor")?;
        let scope_json = delta
            .snapshot_scope
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize external document snapshot scope")?;
        let finished_at = Utc::now();
        tx.execute(
            r#"
            INSERT INTO connectors (
                connector_id, account, cursor_json, last_sync_at,
                scope_boundary_json, materialization_fingerprint,
                health_status, health_detail_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'fresh', NULL, ?4)
            ON CONFLICT(connector_id, account) DO UPDATE SET
                cursor_json = excluded.cursor_json,
                last_sync_at = excluded.last_sync_at,
                scope_boundary_json = excluded.scope_boundary_json,
                materialization_fingerprint = excluded.materialization_fingerprint,
                health_status = excluded.health_status,
                health_detail_json = excluded.health_detail_json,
                updated_at = excluded.updated_at
            "#,
            params![
                ctx.connector_id,
                ctx.account,
                cursor_json,
                timestamp_text(finished_at),
                scope_json,
                delta.materialization_fingerprint,
            ],
        )
        .context("failed to receipt external document materialization")?;
        record_run(&tx, ctx, started_at, finished_at, &manifest)?;
        update_connector_counts(&tx, ctx, &manifest)?;
        let _revision_guard = expected_workspace_revision
            .map(|expected| lock_workspace_revision(&self.workspace_state_dir, expected))
            .transpose()?;
        tx.commit()
            .context("failed to commit external document reconciliation")?;

        Ok(ReconcileResult {
            records_written: manifest.records_written,
            records_updated: manifest.records_updated,
            records_unchanged: manifest.records_unchanged,
            tombstones: manifest.tombstones,
            next_cursor: delta.next_cursor,
            manifest,
        })
    }

    pub fn external_document_evidence(
        &self,
        ctx: &ConnectorCtx,
    ) -> Result<Vec<ExternalDocumentEvidence>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT source_id, occurred_at, title, body_text, href, attributes_json
            FROM external_document_evidence
            WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL
            ORDER BY occurred_at, source_id
            "#,
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (source_id, occurred_at, title, body_text, href, attributes_json) =
                row.context("failed to read external document evidence")?;
            Ok(ExternalDocumentEvidence {
                source_id,
                occurred_at: parse_timestamp(&occurred_at, "occurred_at")?,
                title,
                body_text,
                href,
                attributes: serde_json::from_str(&attributes_json)
                    .context("failed to decode external document attributes")?,
            })
        })
        .collect()
    }

    pub fn external_document_participants(
        &self,
        ctx: &ConnectorCtx,
    ) -> Result<Vec<ExternalDocumentParticipant>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT p.source_id, p.participant_key, p.position, p.display_name,
                   p.email, p.ambiguous
            FROM external_document_participants p
            JOIN external_document_evidence e
              ON e.connector_id = p.connector_id
             AND e.source_account = p.source_account
             AND e.source_id = p.source_id
            WHERE p.connector_id = ?1 AND p.source_account = ?2
              AND e.tombstoned_at IS NULL
            ORDER BY p.source_id, p.position, p.participant_key
            "#,
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok(ExternalDocumentParticipant {
                source_id: row.get(0)?,
                participant_key: row.get(1)?,
                position: row.get(2)?,
                display_name: row.get(3)?,
                email: row.get(4)?,
                ambiguous: row.get(5)?,
            })
        })?;
        rows.map(|row| row.context("failed to read external document participant"))
            .collect()
    }

    pub fn calendar_event_evidence(
        &self,
        ctx: &ConnectorCtx,
    ) -> Result<Vec<CalendarEventEvidence>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT source_id, calendar_id, occurred_from, occurred_to, title, body_text, href
            FROM calendar_event_evidence
            WHERE connector_id = ?1 AND source_account = ?2 AND tombstoned_at IS NULL
            ORDER BY occurred_from, source_id
            "#,
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (source_id, calendar_id, occurred_from, occurred_to, title, body_text, href) =
                row.context("failed to read Calendar event evidence")?;
            Ok(CalendarEventEvidence {
                source_id,
                calendar_id,
                occurred_from: parse_timestamp(&occurred_from, "occurred_from")?,
                occurred_to: occurred_to
                    .map(|value| parse_timestamp(&value, "occurred_to"))
                    .transpose()?,
                title,
                body_text,
                href,
            })
        })
        .collect()
    }

    pub fn calendar_event_attendees(
        &self,
        ctx: &ConnectorCtx,
    ) -> Result<Vec<CalendarEventAttendee>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT a.source_id, a.attendee_key, a.position, a.display_name, a.email,
                   a.response_status, a.is_self, a.organizer
            FROM calendar_event_attendees a
            JOIN calendar_event_evidence e
              ON e.connector_id = a.connector_id
             AND e.source_account = a.source_account
             AND e.source_id = a.source_id
            WHERE a.connector_id = ?1 AND a.source_account = ?2
              AND e.tombstoned_at IS NULL
            ORDER BY a.source_id, a.position, a.attendee_key
            "#,
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok(CalendarEventAttendee {
                source_id: row.get(0)?,
                attendee_key: row.get(1)?,
                position: row.get::<_, u32>(2)?,
                display_name: row.get(3)?,
                email: row.get(4)?,
                response_status: row.get(5)?,
                is_self: row.get(6)?,
                organizer: row.get(7)?,
            })
        })?;
        rows.map(|row| row.context("failed to read Calendar attendee association"))
            .collect()
    }

    pub fn calendar_materialization_scope(
        &self,
        ctx: &ConnectorCtx,
    ) -> Result<Option<SurveyRange>> {
        let scope_json = self
            .connect()?
            .query_row(
                "SELECT scope_boundary_json FROM connectors
                 WHERE connector_id = ?1 AND account = ?2",
                params![ctx.connector_id, ctx.account],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .context("failed to load Calendar materialization scope")?
            .flatten();
        scope_json
            .map(|json| {
                serde_json::from_str(&json)
                    .context("failed to decode Calendar materialization scope")
            })
            .transpose()
    }

    pub fn health_report_for_calendar_materialization(
        &self,
        ctx: &ConnectorCtx,
        expected_fingerprint: &str,
        expected_scope: &SurveyRange,
    ) -> Result<HealthReport> {
        let mut report = self.health_report_for_materialization(ctx, expected_fingerprint)?;
        if report.status == HealthStatus::Fresh
            && self.calendar_materialization_scope(ctx)?.as_ref() != Some(expected_scope)
        {
            report.status = HealthStatus::Stale;
            report.reason = Some("refresh_required".to_string());
            report.detail =
                Some("workspace Calendar rolling boundary changed; refresh required".to_string());
        }
        Ok(report)
    }

    pub fn thread_evidence(&self, ctx: &ConnectorCtx) -> Result<Vec<ThreadEvidence>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT thread_id, occurred_from, occurred_to, body_text, href
             FROM thread_evidence
             WHERE connector_id = ?1 AND source_account = ?2
               AND tombstoned_at IS NULL
             ORDER BY thread_id",
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (thread_id, occurred_from, occurred_to, body_text, href) =
                row.context("failed to read thread evidence")?;
            Ok(ThreadEvidence {
                thread_id,
                occurred_from: parse_timestamp(&occurred_from, "occurred_from")?,
                occurred_to: parse_timestamp(&occurred_to, "occurred_to")?,
                body_text,
                href,
            })
        })
        .collect()
    }

    pub fn participant_threads(&self, ctx: &ConnectorCtx) -> Result<Vec<ParticipantThread>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT participant, thread_id, last_interaction, sampling_score
             FROM participant_threads
             WHERE connector_id = ?1 AND source_account = ?2
             ORDER BY participant, thread_id",
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (participant, thread_id, last_interaction, sampling_score) =
                row.context("failed to read participant association")?;
            Ok(ParticipantThread {
                participant,
                thread_id,
                last_interaction: parse_timestamp(&last_interaction, "last_interaction")?,
                sampling_score: sampling_score
                    .map(u64::try_from)
                    .transpose()
                    .context("participant sampling score is negative")?,
            })
        })
        .collect()
    }

    pub fn ranked_participants(&self, ctx: &ConnectorCtx) -> Result<Vec<RankedParticipant>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT participant,
                   COALESCE(SUM(COALESCE(sampling_score, 0) / 1000000), 0) AS replied,
                   COALESCE(SUM((COALESCE(sampling_score, 0) / 1000) % 1000), 0) AS sent,
                   COUNT(*) AS thread_count,
                   MAX(last_interaction) AS last_interaction
            FROM participant_threads
            WHERE connector_id = ?1 AND source_account = ?2
            GROUP BY participant
            ORDER BY replied DESC, sent DESC, thread_count DESC,
                     last_interaction DESC, participant ASC
            "#,
        )?;
        let rows = statement.query_map(params![ctx.connector_id, ctx.account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (participant, replied, sent, thread_count, last_interaction) =
                row.context("failed to read ranked participant")?;
            let replied =
                u64::try_from(replied).context("aggregate participant reply count is negative")?;
            let sent =
                u64::try_from(sent).context("aggregate participant sent count is negative")?;
            let thread_count = u64::try_from(thread_count)
                .context("aggregate participant thread count is negative")?;
            Ok(RankedParticipant {
                participant,
                aggregate_sampling_score: replied.min(999) * 1_000_000
                    + sent.min(999) * 1_000
                    + thread_count.min(999),
                last_interaction: parse_timestamp(&last_interaction, "last_interaction")?,
            })
        })
        .collect()
    }

    pub fn top_recent_participants(
        &self,
        ctx: &ConnectorCtx,
        limit: usize,
    ) -> Result<Vec<RankedParticipant>> {
        Ok(self
            .ranked_participants(ctx)?
            .into_iter()
            .take(limit)
            .collect())
    }

    fn build_health_report(
        &self,
        ctx: &ConnectorCtx,
        status: HealthStatus,
        detail: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<HealthReport> {
        let connection = self.connect()?;
        let row: Option<(Option<String>, Option<String>)> = connection
            .query_row(
                "SELECT last_sync_at, cursor_json FROM connectors WHERE connector_id = ?1 AND account = ?2",
                params![ctx.connector_id, ctx.account],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("failed to load connector health row")?;
        let (last_sync_at, _cursor_json) = row.unwrap_or((None, None));
        let last_successful_sync = last_sync_at
            .as_deref()
            .map(|value| parse_timestamp(value, "last_sync_at"))
            .transpose()?;
        let cursor_age_secs = last_successful_sync.map(|sync| (now - sync).num_seconds());
        Ok(HealthReport {
            status,
            reason: match status {
                HealthStatus::Fresh => None,
                HealthStatus::Stale => Some("refresh_required".to_string()),
                HealthStatus::NeedsAuth => Some("credentials_unavailable".to_string()),
                HealthStatus::Error => Some("refresh_failed".to_string()),
            },
            last_successful_sync,
            cursor_age_secs,
            detail: detail.map(str::to_owned),
        })
    }
}

/// Compile a deterministic, read-only destructive preview for one exact
/// connector account. Removed bindings remain valid targets because the ledger
/// identity is connector + normalized account, not a display name.
pub fn preview_retention(
    workspace: &ResolvedWorkspace,
    target: &RetentionTarget,
    scope: RetentionScope,
) -> Result<RetentionPreview> {
    validate_retention_target(target)?;
    let current = resolve_state_dir(&workspace.state_dir)?;
    let revision = workspace_revision(&current)?;
    let cutoffs = retention_cutoffs(&current, scope)?;
    let db_path = workspace.state_dir.join(DB_NAME);
    if !db_path.is_file() {
        bail!(
            "retention preview requires an existing workspace ledger at {}",
            db_path.display()
        );
    }
    let mut connection = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open {} read-only", db_path.display()))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .context("failed to set SQLite busy timeout")?;
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("failed to read ledger schema version")?;
    if version != SCHEMA_VERSION {
        bail!(
            "unsupported ledger schema version {version}; delete and recreate the workspace ledger"
        );
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .context("failed to begin read-only retention snapshot")?;
    let (counts, ledger_fingerprint) = retention_snapshot(&tx, target, scope, &cutoffs)?;
    let mut preview = RetentionPreview {
        schema_version: RETENTION_PREVIEW_SCHEMA.to_string(),
        workspace_id: current.config.id,
        revision,
        plan_id: String::new(),
        target: target.clone(),
        scope,
        cutoffs,
        counts,
        ledger_fingerprint,
        destructive: true,
        index_refresh_required: matches!(
            scope,
            RetentionScope::Materialization | RetentionScope::All
        ),
    };
    preview.plan_id = retention_plan_id(&preview)?;
    Ok(preview)
}

/// Atomically apply one exact preview and persist its replay receipt in the
/// same SQLite transaction as the destructive writes.
pub fn apply_retention(
    workspace: &ResolvedWorkspace,
    preview: &RetentionPreview,
    expected_revision: &str,
    request_id: &str,
) -> Result<RetentionApplyReceipt> {
    validate_mutation_request_id(request_id)?;
    if let Err(error) = validate_retention_target(&preview.target) {
        return Err(RetentionMutationError::InvalidPlan(error.to_string()).into());
    }
    let expected_plan_id = retention_plan_id(preview)?;
    let expected_index_refresh = matches!(
        preview.scope,
        RetentionScope::Materialization | RetentionScope::All
    );
    if preview.schema_version != RETENTION_PREVIEW_SCHEMA
        || preview.plan_id != expected_plan_id
        || !preview.destructive
        || preview.index_refresh_required != expected_index_refresh
    {
        return Err(RetentionMutationError::InvalidPlan(
            "schema, plan_id, destructive marker, or index refresh requirement is invalid"
                .to_string(),
        )
        .into());
    }
    if preview.workspace_id != workspace.config.id {
        return Err(RetentionMutationError::InvalidPlan(format!(
            "plan workspace '{}' does not match '{}'",
            preview.workspace_id, workspace.config.id
        ))
        .into());
    }
    if preview.revision != expected_revision {
        return Err(RetentionMutationError::InvalidPlan(
            "--if-revision must equal the preview revision".to_string(),
        )
        .into());
    }

    let request_hash = retention_request_hash(preview, expected_revision)?;
    let store = IntegrationsStore::open(&workspace.state_dir)?;
    let _reconcile_lock = store.acquire_reconcile_lock()?;
    if let Some((stored_hash, mut receipt)) = load_purge_receipt(&store, request_id)? {
        if stored_hash != request_hash {
            return Err(WorkspaceMutationError::IdempotencyConflict {
                request_id: request_id.to_string(),
            }
            .into());
        }
        receipt.replayed = true;
        return Ok(receipt);
    }

    let current = resolve_state_dir(&workspace.state_dir)?;
    let actual_revision = workspace_revision(&current)?;
    if actual_revision != expected_revision {
        return Err(WorkspaceMutationError::RevisionConflict {
            expected: expected_revision.to_string(),
            actual: actual_revision,
        }
        .into());
    }
    let expected_cutoffs = retention_cutoffs(&current, preview.scope)?;
    if preview.cutoffs != expected_cutoffs {
        return Err(RetentionMutationError::InvalidPlan(
            "retention cutoffs do not match the Workspace policy at the preview revision"
                .to_string(),
        )
        .into());
    }

    let mut connection = store.connect()?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin retention transaction")?;
    let (actual_counts, actual_fingerprint) =
        retention_snapshot(&tx, &preview.target, preview.scope, &preview.cutoffs)?;
    if actual_counts != preview.counts || actual_fingerprint != preview.ledger_fingerprint {
        return Err(RetentionMutationError::PlanStale {
            expected_fingerprint: preview.ledger_fingerprint.clone(),
            actual_fingerprint,
        }
        .into());
    }

    apply_retention_deletes(&tx, &preview.target, preview.scope, &preview.cutoffs)?;
    let receipt = RetentionApplyReceipt {
        schema_version: RETENTION_APPLY_SCHEMA.to_string(),
        ok: true,
        workspace_id: current.config.id,
        revision: expected_revision.to_string(),
        request_id: request_id.to_string(),
        request_hash,
        plan_id: preview.plan_id.clone(),
        target: preview.target.clone(),
        scope: preview.scope,
        deleted: preview.counts.clone(),
        replayed: false,
        index_refresh_required: preview.index_refresh_required,
    };
    let result_json = serde_json::to_string(&receipt).context("serializing purge receipt")?;
    tx.execute(
        "INSERT INTO purge_receipts (request_id, request_hash, result_json, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            request_id,
            receipt.request_hash,
            result_json,
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        ],
    )
    .context("failed to persist purge receipt")?;
    let _revision_guard = lock_workspace_revision(&workspace.state_dir, expected_revision)?;
    tx.commit().context("failed to commit retention apply")?;
    Ok(receipt)
}

fn load_purge_receipt(
    store: &IntegrationsStore,
    request_id: &str,
) -> Result<Option<(String, RetentionApplyReceipt)>> {
    let connection = store.connect()?;
    let row = connection
        .query_row(
            "SELECT request_hash, result_json FROM purge_receipts WHERE request_id = ?1",
            [request_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .context("failed to load purge receipt")?;
    row.map(|(request_hash, result_json)| {
        Ok((
            request_hash,
            serde_json::from_str(&result_json).context("failed to decode purge receipt")?,
        ))
    })
    .transpose()
}

fn retention_cutoffs(
    workspace: &ResolvedWorkspace,
    scope: RetentionScope,
) -> Result<RetentionCutoffs> {
    if scope != RetentionScope::Expired {
        return Ok(RetentionCutoffs::default());
    }
    let today = Utc::now().date_naive();
    let midnight = Utc.from_utc_datetime(
        &today
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always a valid naive time"),
    );
    Ok(RetentionCutoffs {
        raw_cache_before: workspace
            .config
            .retention
            .raw_cache_max_age_days
            .map(|days| midnight - ChronoDuration::days(i64::from(days))),
        tombstone_before: workspace
            .config
            .retention
            .tombstone_max_age_days
            .map(|days| midnight - ChronoDuration::days(i64::from(days))),
    })
}

fn retention_plan_id(preview: &RetentionPreview) -> Result<String> {
    #[derive(Serialize)]
    struct Plan<'a> {
        schema_version: &'a str,
        workspace_id: &'a str,
        revision: &'a str,
        target: &'a RetentionTarget,
        scope: RetentionScope,
        cutoffs: &'a RetentionCutoffs,
        counts: &'a RetentionCounts,
        ledger_fingerprint: &'a str,
        destructive: bool,
        index_refresh_required: bool,
    }
    let bytes = serde_json::to_vec(&Plan {
        schema_version: &preview.schema_version,
        workspace_id: &preview.workspace_id,
        revision: &preview.revision,
        target: &preview.target,
        scope: preview.scope,
        cutoffs: &preview.cutoffs,
        counts: &preview.counts,
        ledger_fingerprint: &preview.ledger_fingerprint,
        destructive: preview.destructive,
        index_refresh_required: preview.index_refresh_required,
    })
    .context("serializing retention preview")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn retention_request_hash(preview: &RetentionPreview, expected_revision: &str) -> Result<String> {
    let bytes = serde_json::to_vec(&(preview, expected_revision))
        .context("serializing retention apply request")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_retention_target(target: &RetentionTarget) -> Result<()> {
    if target.source_account.trim().is_empty()
        || target.source_account != target.source_account.trim()
    {
        bail!("retention target requires a normalized non-empty source_account");
    }
    if !matches!(
        target.connector_id.as_str(),
        "email" | "gcal" | "google_meet" | "granola"
    ) {
        bail!(
            "unsupported retention connector_id {}; expected email, gcal, google_meet, or granola",
            target.connector_id
        );
    }
    let normalized = if target.connector_id == "granola" {
        normalize_granola_account(&target.source_account)?
    } else {
        normalize_google_account(&target.source_account)?
    };
    if normalized != target.source_account {
        bail!("retention target source_account must already be normalized");
    }
    Ok(())
}

fn retention_snapshot(
    connection: &Connection,
    target: &RetentionTarget,
    scope: RetentionScope,
    cutoffs: &RetentionCutoffs,
) -> Result<(RetentionCounts, String)> {
    let mut hasher = Sha256::new();
    hasher.update(b"margins-retention-ledger-v1\0");
    hasher.update(target.connector_id.as_bytes());
    hasher.update([0]);
    hasher.update(target.source_account.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(&(scope, cutoffs))?);

    let mut counts = RetentionCounts::default();
    if matches!(scope, RetentionScope::RawCache | RetentionScope::All)
        || (scope == RetentionScope::Expired && cutoffs.raw_cache_before.is_some())
    {
        let mut sql = "SELECT connector_id, source_account, source_id, payload_json, fetched_at
                       FROM raw_items WHERE connector_id = ?1 AND source_account = ?2"
            .to_string();
        let mut values = target_values(target);
        if let Some(cutoff) = (scope == RetentionScope::Expired)
            .then_some(cutoffs.raw_cache_before)
            .flatten()
        {
            sql.push_str(" AND fetched_at < ?3");
            values.push(SqlValue::Text(timestamp_text(cutoff)));
        }
        sql.push_str(" ORDER BY source_id");
        counts.raw_items = hash_query_rows(connection, &mut hasher, &sql, &values)?;
    }

    let layout = retention_layout(&target.connector_id)?;
    if matches!(scope, RetentionScope::Materialization | RetentionScope::All) {
        let sql = format!(
            "SELECT * FROM {} WHERE connector_id = ?1 AND source_account = ?2
             AND tombstoned_at IS NULL ORDER BY {}",
            layout.evidence_table, layout.evidence_id
        );
        counts.active_evidence =
            hash_query_rows(connection, &mut hasher, &sql, &target_values(target))?;
    }
    if matches!(
        scope,
        RetentionScope::Tombstones | RetentionScope::Materialization | RetentionScope::All
    ) || (scope == RetentionScope::Expired && cutoffs.tombstone_before.is_some())
    {
        let mut sql = format!(
            "SELECT * FROM {} WHERE connector_id = ?1 AND source_account = ?2
             AND tombstoned_at IS NOT NULL",
            layout.evidence_table
        );
        let mut values = target_values(target);
        if let Some(cutoff) = (scope == RetentionScope::Expired)
            .then_some(cutoffs.tombstone_before)
            .flatten()
        {
            sql.push_str(" AND tombstoned_at < ?3");
            values.push(SqlValue::Text(timestamp_text(cutoff)));
        }
        sql.push_str(&format!(" ORDER BY {}", layout.evidence_id));
        counts.tombstoned_evidence = hash_query_rows(connection, &mut hasher, &sql, &values)?;
    }

    if matches!(scope, RetentionScope::Materialization | RetentionScope::All) {
        let sql = format!(
            "SELECT * FROM {} WHERE connector_id = ?1 AND source_account = ?2 ORDER BY {}",
            layout.association_table, layout.association_order
        );
        counts.associations =
            hash_query_rows(connection, &mut hasher, &sql, &target_values(target))?;
        counts.connector_state = hash_query_rows(
            connection,
            &mut hasher,
            "SELECT * FROM connectors WHERE connector_id = ?1 AND account = ?2",
            &target_values(target),
        )?;
        counts.curation_observations = hash_query_rows(
            connection,
            &mut hasher,
            "SELECT * FROM curation_observations WHERE connector_id = ?1 AND account = ?2",
            &target_values(target),
        )?;
    } else if matches!(scope, RetentionScope::Tombstones | RetentionScope::Expired)
        && counts.tombstoned_evidence > 0
    {
        let Some(cutoff) = (scope == RetentionScope::Expired)
            .then_some(cutoffs.tombstone_before)
            .flatten()
        else {
            let sql = tombstone_association_query(layout, false);
            counts.associations =
                hash_query_rows(connection, &mut hasher, &sql, &target_values(target))?;
            return Ok((counts, format!("{:x}", hasher.finalize())));
        };
        let sql = tombstone_association_query(layout, true);
        let mut values = target_values(target);
        values.push(SqlValue::Text(timestamp_text(cutoff)));
        counts.associations = hash_query_rows(connection, &mut hasher, &sql, &values)?;
    }

    Ok((counts, format!("{:x}", hasher.finalize())))
}

fn apply_retention_deletes(
    tx: &rusqlite::Transaction<'_>,
    target: &RetentionTarget,
    scope: RetentionScope,
    cutoffs: &RetentionCutoffs,
) -> Result<()> {
    if matches!(scope, RetentionScope::RawCache | RetentionScope::All) {
        tx.execute(
            "DELETE FROM raw_items WHERE connector_id = ?1 AND source_account = ?2",
            params![target.connector_id, target.source_account],
        )?;
    } else if scope == RetentionScope::Expired {
        if let Some(cutoff) = cutoffs.raw_cache_before {
            tx.execute(
                "DELETE FROM raw_items WHERE connector_id = ?1 AND source_account = ?2
                 AND fetched_at < ?3",
                params![
                    target.connector_id,
                    target.source_account,
                    timestamp_text(cutoff)
                ],
            )?;
        }
    }

    let layout = retention_layout(&target.connector_id)?;
    if matches!(scope, RetentionScope::Materialization | RetentionScope::All) {
        tx.execute(
            &format!(
                "DELETE FROM {} WHERE connector_id = ?1 AND source_account = ?2",
                layout.evidence_table
            ),
            params![target.connector_id, target.source_account],
        )?;
        tx.execute(
            "DELETE FROM connectors WHERE connector_id = ?1 AND account = ?2",
            params![target.connector_id, target.source_account],
        )?;
        tx.execute(
            "DELETE FROM curation_observations WHERE connector_id = ?1 AND account = ?2",
            params![target.connector_id, target.source_account],
        )?;
    } else if scope == RetentionScope::Tombstones {
        tx.execute(
            &format!(
                "DELETE FROM {} WHERE connector_id = ?1 AND source_account = ?2
                 AND tombstoned_at IS NOT NULL",
                layout.evidence_table
            ),
            params![target.connector_id, target.source_account],
        )?;
    } else if scope == RetentionScope::Expired {
        if let Some(cutoff) = cutoffs.tombstone_before {
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE connector_id = ?1 AND source_account = ?2
                     AND tombstoned_at IS NOT NULL AND tombstoned_at < ?3",
                    layout.evidence_table
                ),
                params![
                    target.connector_id,
                    target.source_account,
                    timestamp_text(cutoff)
                ],
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RetentionLayout {
    evidence_table: &'static str,
    evidence_id: &'static str,
    association_table: &'static str,
    association_order: &'static str,
    association_evidence_id: &'static str,
}

fn retention_layout(connector_id: &str) -> Result<RetentionLayout> {
    match connector_id {
        "email" => Ok(RetentionLayout {
            evidence_table: "thread_evidence",
            evidence_id: "thread_id",
            association_table: "participant_threads",
            association_order: "participant, thread_id",
            association_evidence_id: "thread_id",
        }),
        "gcal" => Ok(RetentionLayout {
            evidence_table: "calendar_event_evidence",
            evidence_id: "source_id",
            association_table: "calendar_event_attendees",
            association_order: "source_id, position, attendee_key",
            association_evidence_id: "source_id",
        }),
        "google_meet" | "granola" => Ok(RetentionLayout {
            evidence_table: "external_document_evidence",
            evidence_id: "source_id",
            association_table: "external_document_participants",
            association_order: "source_id, position, participant_key",
            association_evidence_id: "source_id",
        }),
        other => bail!("unsupported retention connector_id {other}"),
    }
}

fn tombstone_association_query(layout: RetentionLayout, with_cutoff: bool) -> String {
    format!(
        "SELECT a.* FROM {} a JOIN {} e
         ON e.connector_id = a.connector_id AND e.source_account = a.source_account
         AND e.{} = a.{}
         WHERE e.connector_id = ?1 AND e.source_account = ?2
         AND e.tombstoned_at IS NOT NULL{}
         ORDER BY a.{}",
        layout.association_table,
        layout.evidence_table,
        layout.evidence_id,
        layout.association_evidence_id,
        if with_cutoff {
            " AND e.tombstoned_at < ?3"
        } else {
            ""
        },
        layout.association_order,
    )
}

fn target_values(target: &RetentionTarget) -> Vec<SqlValue> {
    vec![
        SqlValue::Text(target.connector_id.clone()),
        SqlValue::Text(target.source_account.clone()),
    ]
}

fn hash_query_rows(
    connection: &Connection,
    hasher: &mut Sha256,
    sql: &str,
    values: &[SqlValue],
) -> Result<u64> {
    hasher.update(sql.as_bytes());
    hasher.update([0]);
    let mut statement = connection.prepare(sql)?;
    let column_count = statement.column_count();
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        count += 1;
        for index in 0..column_count {
            match row.get_ref(index)? {
                ValueRef::Null => hasher.update([0]),
                ValueRef::Integer(value) => {
                    hasher.update([1]);
                    hasher.update(value.to_le_bytes());
                }
                ValueRef::Real(value) => {
                    hasher.update([2]);
                    hasher.update(value.to_le_bytes());
                }
                ValueRef::Text(value) => {
                    hasher.update([3]);
                    hasher.update((value.len() as u64).to_le_bytes());
                    hasher.update(value);
                }
                ValueRef::Blob(value) => {
                    hasher.update([4]);
                    hasher.update((value.len() as u64).to_le_bytes());
                    hasher.update(value);
                }
            }
        }
    }
    Ok(count)
}

fn upsert_raw_item(
    tx: &rusqlite::Transaction<'_>,
    connector_id: &str,
    source_account: &str,
    source_id: &str,
    payload: &serde_json::Value,
    fetched_at: DateTime<Utc>,
) -> Result<()> {
    let payload_json = serde_json::to_string(payload).context("failed to serialize raw item")?;
    tx.execute(
        r#"
        INSERT INTO raw_items (
            connector_id, source_account, source_id, payload_json, fetched_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(connector_id, source_account, source_id) DO UPDATE SET
            payload_json = excluded.payload_json,
            fetched_at = excluded.fetched_at
        "#,
        params![
            connector_id,
            source_account,
            source_id,
            payload_json,
            fetched_at.to_rfc3339_opts(SecondsFormat::Secs, true)
        ],
    )
    .context("failed to upsert raw item")?;
    Ok(())
}

fn record_run(
    tx: &rusqlite::Transaction<'_>,
    ctx: &ConnectorCtx,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    manifest: &RunManifest,
) -> Result<()> {
    let manifest_json =
        serde_json::to_string(manifest).context("failed to serialize run manifest")?;
    tx.execute(
        r#"
        INSERT INTO runs (connector_id, account, started_at, finished_at, manifest_json)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            ctx.connector_id,
            ctx.account,
            started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            finished_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            manifest_json
        ],
    )
    .context("failed to record run")?;
    Ok(())
}

fn update_connector_counts(
    tx: &rusqlite::Transaction<'_>,
    ctx: &ConnectorCtx,
    manifest: &RunManifest,
) -> Result<()> {
    let counts = serde_json::to_string(manifest).context("failed to serialize connector counts")?;
    tx.execute(
        "UPDATE connectors SET counts_json = ?1 WHERE connector_id = ?2 AND account = ?3",
        params![counts, ctx.connector_id, ctx.account],
    )
    .context("failed to persist connector counts")?;
    Ok(())
}

fn update_connector_health(
    tx: &rusqlite::Transaction<'_>,
    ctx: &ConnectorCtx,
    status: HealthStatus,
    detail: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    let detail_json = detail.map(|value| serde_json::json!({ "detail": value }).to_string());
    let last_sync_at = if matches!(status, HealthStatus::Fresh) {
        Some(now.to_rfc3339_opts(SecondsFormat::Secs, true))
    } else {
        None
    };
    tx.execute(
        r#"
        INSERT INTO connectors (
            connector_id, account, health_status, health_detail_json, last_sync_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(connector_id, account) DO UPDATE SET
            health_status = excluded.health_status,
            health_detail_json = excluded.health_detail_json,
            last_sync_at = COALESCE(excluded.last_sync_at, connectors.last_sync_at),
            updated_at = excluded.updated_at
        "#,
        params![
            ctx.connector_id,
            ctx.account,
            status.as_str(),
            detail_json,
            last_sync_at,
            now.to_rfc3339_opts(SecondsFormat::Secs, true)
        ],
    )
    .context("failed to update connector health")?;
    Ok(())
}

fn parse_timestamp(value: &str, field: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid RFC3339 timestamp in {field}"))
        .map(|value| value.with_timezone(&Utc))
}

fn timestamp_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn connector_ctx(root: &Path, connector_id: &str, account: &str) -> ConnectorCtx {
        ConnectorCtx {
            vault_root: root.to_path_buf(),
            connector_id: connector_id.to_string(),
            account: account.to_string(),
            command_path: None,
        }
    }

    #[test]
    fn schema_initializes_at_expected_version() {
        let temp = tempfile::tempdir().unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let connection = Connection::open(store.db_path()).unwrap();
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn retention_preview_never_creates_or_mutates_workspace_state() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("home.md"), "# Home\n").unwrap();
        let margins_home = temp.path().join("margins-home");
        let workspace =
            crate::workspace::create_workspace(&margins_home, "preview", None, &notes).unwrap();
        let target = RetentionTarget {
            connector_id: "email".to_string(),
            source_account: "owner@example.com".to_string(),
        };

        let missing = preview_retention(&workspace, &target, RetentionScope::RawCache)
            .expect_err("preview must not create a missing ledger");
        assert!(missing
            .to_string()
            .contains("requires an existing workspace ledger"));
        assert!(!workspace.state_dir.join(DB_NAME).exists());
        assert!(!workspace.state_dir.join("reconcile.lock").exists());

        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        let before = std::fs::read(store.db_path()).unwrap();
        let preview = preview_retention(&workspace, &target, RetentionScope::RawCache).unwrap();
        assert_eq!(preview.counts, RetentionCounts::default());
        assert_eq!(std::fs::read(store.db_path()).unwrap(), before);
        assert!(!workspace.state_dir.join("reconcile.lock").exists());
    }

    #[test]
    fn retention_apply_rederives_closed_safety_fields() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("home.md"), "# Home\n").unwrap();
        let margins_home = temp.path().join("margins-home");
        let mut workspace =
            crate::workspace::create_workspace(&margins_home, "safety", None, &notes).unwrap();
        IntegrationsStore::open(&workspace.state_dir).unwrap();
        let target = RetentionTarget {
            connector_id: "email".to_string(),
            source_account: "owner@example.com".to_string(),
        };

        let mut materialization =
            preview_retention(&workspace, &target, RetentionScope::Materialization).unwrap();
        materialization.index_refresh_required = false;
        materialization.plan_id = retention_plan_id(&materialization).unwrap();
        let error = apply_retention(
            &workspace,
            &materialization,
            &materialization.revision,
            "tampered-refresh",
        )
        .expect_err("materialization cannot suppress its required index refresh");
        assert!(error
            .downcast_ref::<RetentionMutationError>()
            .is_some_and(|error| matches!(error, RetentionMutationError::InvalidPlan(_))));

        workspace.config.retention.raw_cache_max_age_days = Some(30);
        std::fs::write(
            &workspace.config_path,
            toml::to_string(&workspace.config).unwrap(),
        )
        .unwrap();
        let mut expired = preview_retention(&workspace, &target, RetentionScope::Expired).unwrap();
        expired.cutoffs.raw_cache_before = expired
            .cutoffs
            .raw_cache_before
            .map(|cutoff| cutoff + ChronoDuration::days(1));
        expired.plan_id = retention_plan_id(&expired).unwrap();
        let error = apply_retention(&workspace, &expired, &expired.revision, "tampered-cutoff")
            .expect_err("expired cutoff must be derived from Workspace policy");
        assert!(error
            .downcast_ref::<RetentionMutationError>()
            .is_some_and(|error| matches!(error, RetentionMutationError::InvalidPlan(_))));
    }

    #[test]
    fn external_document_reconciliation_uses_only_dedicated_authority() {
        let temp = tempfile::tempdir().unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let ctx = connector_ctx(temp.path(), "google_meet", "owner@example.com");
        let occurred_at: DateTime<Utc> = "2026-08-20T10:00:00Z".parse().unwrap();
        let document = |source_id: &str, title: &str| ExternalDocumentEvidence {
            source_id: source_id.to_string(),
            occurred_at,
            title: title.to_string(),
            body_text: format!("Transcript for {title}"),
            href: Some(format!("https://example.test/{source_id}")),
            attributes: serde_json::json!({"provider_field": source_id}),
        };
        let first = store
            .apply_external_document_delta(
                &ctx,
                ExternalDocumentDelta {
                    documents: vec![document("doc-1", "First"), document("doc-2", "Second")],
                    participants: vec![ExternalDocumentParticipant {
                        source_id: "doc-1".to_string(),
                        participant_key: "alice@example.com".to_string(),
                        position: 0,
                        display_name: "Alice".to_string(),
                        email: Some("alice@example.com".to_string()),
                        ambiguous: false,
                    }],
                    tombstone_source_ids: Vec::new(),
                    raw_items: vec![RawItemDraft {
                        source_id: "doc-1".to_string(),
                        payload: serde_json::json!({"raw": true}),
                    }],
                    snapshot_scope: None,
                    complete_snapshot: true,
                    materialization_fingerprint: "meet-v1".to_string(),
                    next_cursor: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(first.records_written, 2);
        assert_eq!(store.external_document_evidence(&ctx).unwrap().len(), 2);
        assert_eq!(
            store.external_document_participants(&ctx).unwrap()[0]
                .email
                .as_deref(),
            Some("alice@example.com")
        );
        let second = store
            .apply_external_document_delta(
                &ctx,
                ExternalDocumentDelta {
                    documents: vec![document("doc-1", "First updated")],
                    participants: Vec::new(),
                    tombstone_source_ids: Vec::new(),
                    raw_items: Vec::new(),
                    snapshot_scope: None,
                    complete_snapshot: true,
                    materialization_fingerprint: "meet-v1".to_string(),
                    next_cursor: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(second.records_updated, 1);
        assert_eq!(second.tombstones, 1);
        let remaining = store.external_document_evidence(&ctx).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "First updated");
        assert!(store
            .external_document_participants(&ctx)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .health_report_for_materialization(&ctx, "meet-v1")
                .unwrap()
                .status,
            HealthStatus::Fresh
        );
    }

    #[test]
    fn granola_external_reconciliation_retains_raw_transport_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let ctx = connector_ctx(temp.path(), "granola", "owner@example.com");
        let occurred_at: DateTime<Utc> = "2026-08-20T10:00:00Z".parse().unwrap();
        let result = store
            .apply_external_document_delta(
                &ctx,
                ExternalDocumentDelta {
                    documents: vec![ExternalDocumentEvidence {
                        source_id: "meeting-1".to_string(),
                        occurred_at,
                        title: "Granola meeting".to_string(),
                        body_text: "Granola evidence".to_string(),
                        href: None,
                        attributes: serde_json::json!({}),
                    }],
                    participants: Vec::new(),
                    tombstone_source_ids: Vec::new(),
                    raw_items: vec![RawItemDraft {
                        source_id: "meeting-1".to_string(),
                        payload: serde_json::json!({"upstream_shape": "meeting"}),
                    }],
                    snapshot_scope: None,
                    complete_snapshot: false,
                    materialization_fingerprint: "granola-v1".to_string(),
                    next_cursor: None,
                },
                None,
            )
            .expect("Granola reconciliation should retain its transport cache");

        assert_eq!(result.records_written, 1);
        assert_eq!(store.raw_items(&ctx).unwrap().len(), 1);
        assert_eq!(store.external_document_evidence(&ctx).unwrap().len(), 1);
    }

    #[test]
    fn schema_version_pins_every_create_statement() {
        use sha2::{Digest, Sha256};

        let temp = tempfile::tempdir().unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let connection = Connection::open(store.db_path()).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT type, name, sql FROM sqlite_schema \
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
                 ORDER BY type, name",
            )
            .unwrap();
        let schema = statement
            .query_map([], |row| {
                Ok(format!(
                    "{}\n{}\n{}\n",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<String>>()
            .unwrap();
        let actual = format!("{:x}", Sha256::digest(schema.as_bytes()));

        // A schema edit must deliberately bump SCHEMA_VERSION and replace the
        // matching fingerprint. Updating only the CREATE text fails here.
        let expected = match SCHEMA_VERSION {
            9 => "38419ea8fd512d288d51a95a264531b09536c98222c59e7f206a37bbc74dfd8c",
            other => panic!("schema version {other} has no pinned CREATE fingerprint"),
        };
        assert_eq!(actual, expected, "full SQLite CREATE schema changed");
    }

    #[test]
    fn incompatible_schema_is_refused_before_any_ledger_queries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_NAME);
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 7).unwrap();
        connection
            .execute_batch("CREATE TABLE deliberately_incompatible (value TEXT);")
            .unwrap();
        drop(connection);

        let error = IntegrationsStore::open(temp.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "unsupported ledger schema version 7; delete and recreate the workspace ledger"
        );
    }

    #[test]
    fn forgotten_google_connection_marks_only_existing_google_rows_needs_auth() {
        let temp = tempfile::tempdir().unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let account = "owner@example.com";
        let email = connector_ctx(temp.path(), "email", account);
        let calendar = connector_ctx(temp.path(), "gcal", account);
        let meet = connector_ctx(temp.path(), "google_meet", account);
        let granola = connector_ctx(temp.path(), "granola", account);
        let unrelated = connector_ctx(temp.path(), "email", "other@example.com");
        for ctx in [&email, &calendar, &meet, &granola, &unrelated] {
            store
                .update_health(ctx, HealthStatus::Fresh, Some("fixture refresh"))
                .unwrap();
        }
        let last_successful = [&email, &calendar, &meet, &granola]
            .map(|ctx| store.health_report(ctx).unwrap().last_successful_sync);

        assert_eq!(store.mark_google_connection_needs_auth(account).unwrap(), 3);
        for (ctx, previous_sync) in [&email, &calendar, &meet]
            .into_iter()
            .zip(last_successful.iter())
        {
            let report = store.health_report(ctx).unwrap();
            assert_eq!(report.status, HealthStatus::NeedsAuth);
            assert_eq!(&report.last_successful_sync, previous_sync);
            assert_eq!(
                report.detail.as_deref(),
                Some("machine Google connection was forgotten")
            );
        }
        assert_eq!(
            store.health_report(&granola).unwrap().status,
            HealthStatus::Fresh
        );
        assert_eq!(
            store.health_report(&unrelated).unwrap().status,
            HealthStatus::Fresh
        );

        assert_eq!(
            store.mark_granola_connection_needs_auth(account).unwrap(),
            1
        );
        let report = store.health_report(&granola).unwrap();
        assert_eq!(report.status, HealthStatus::NeedsAuth);
        assert_eq!(report.last_successful_sync, last_successful[3]);
        assert_eq!(
            report.detail.as_deref(),
            Some("machine Granola connection was forgotten")
        );
        assert_eq!(
            store.health_report(&unrelated).unwrap().status,
            HealthStatus::Fresh
        );
    }
}
