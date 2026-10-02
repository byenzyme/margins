use crate::canonical;
use anyhow::{bail, Context, Result};
use margins_core::{MemoMoment, TimedMemoDocument, TimedMemoLine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorityMemoReceipt {
    pub revision: String,
    pub lines: Vec<TimedMemoLine>,
    pub replayed: bool,
    /// SQLite committed the memo, but its repairable Markdown projection is
    /// missing, differs, or could not be refreshed. Native TUI writes use this
    /// authority, so refreshes no longer clobber TUI-owned Markdown edits.
    pub mirror_stale: bool,
}

/// The caller edited an older memo snapshot. Read `memo` again before retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoRevisionConflict {
    pub expected: String,
    pub current: String,
}

impl fmt::Display for MemoRevisionConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "memo revision conflict: current revision is {}",
            self.current
        )
    }
}

impl Error for MemoRevisionConflict {}

/// One revision-fenced memo mutation. A producer must retain the revision it
/// actually observed, including across retries and reconnects.
#[derive(Debug, Clone)]
pub enum MemoWrite<'a> {
    PlainText {
        observed_at_ms: u64,
        paused: bool,
        text: &'a str,
    },
    ReplaceLines(&'a [TimedMemoLine]),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportReceipt {
    pub upload_id: String,
    pub session_id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub stored_path: String,
    pub processing_state: String,
    pub replayed: bool,
}

#[derive(Debug, Clone)]
pub struct SqliteWorkspaceAuthorityStorage {
    directory: PathBuf,
}

impl SqliteWorkspaceAuthorityStorage {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let storage = Self {
            directory: directory.into(),
        };
        storage.connection()?;
        Ok(storage)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn connection(&self) -> Result<Connection> {
        let connection = canonical::open_db(&self.directory)?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workspace_session_producers (
                session_id TEXT PRIMARY KEY NOT NULL,
                principal_id TEXT NOT NULL,
                producer_token_hash TEXT NOT NULL,
                state TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS workspace_client_current (
                principal_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (principal_id, workspace_id),
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS workspace_memos (
                session_id TEXT PRIMARY KEY NOT NULL,
                revision TEXT NOT NULL,
                document_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS workspace_memo_receipts (
                session_id TEXT NOT NULL,
                principal_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                response_json TEXT NOT NULL,
                PRIMARY KEY (session_id, principal_id, request_id),
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS workspace_import_receipts (
                upload_id TEXT PRIMARY KEY NOT NULL,
                principal_id TEXT NOT NULL,
                digest TEXT NOT NULL,
                session_id TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                stored_path TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                processing_state TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS workspace_session_mutation_receipts (
                session_id TEXT NOT NULL,
                principal_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                request_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                response_json TEXT NOT NULL,
                PRIMARY KEY (session_id, principal_id, operation, request_id),
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
            );
            "#,
        )?;
        Ok(connection)
    }

    pub fn reserve_producer(
        &self,
        session_id: &str,
        principal_id: &str,
        producer_token: &str,
    ) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let token_hash = digest(producer_token.as_bytes());
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT principal_id, producer_token_hash, state FROM workspace_session_producers WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((owner, hash, state)) = existing {
            if owner == principal_id && hash == token_hash && state != "released" {
                return Ok(());
            }
            if state != "released" {
                bail!("session already has a capture producer");
            }
            tx.execute(
                "UPDATE workspace_session_producers SET principal_id = ?1, producer_token_hash = ?2, state = 'active', updated_at_ms = ?3 WHERE session_id = ?4 AND state = 'released'",
                params![principal_id, token_hash, now_ms(), session_id],
            )?;
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "INSERT INTO workspace_session_producers (session_id, principal_id, producer_token_hash, state, updated_at_ms) VALUES (?1, ?2, ?3, 'active', ?4)",
            params![session_id, principal_id, token_hash, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn authorize_producer(
        &self,
        session_id: &str,
        principal_id: &str,
        producer_token: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        let expected: Option<(String, String, String)> = connection
            .query_row(
                "SELECT principal_id, producer_token_hash, state FROM workspace_session_producers WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((owner, hash, state)) = expected else {
            bail!("capture producer is not reserved");
        };
        if owner != principal_id || hash != digest(producer_token.as_bytes()) {
            bail!("capture producer authorization failed");
        }
        if state == "abandoned" {
            bail!("browser capture owner lease expired");
        }
        // A released producer may repeat its exact final command after losing
        // the response. The runtime receipt decides whether it is an
        // idempotent replay and rejects any new post-finalization mutation.
        Ok(())
    }

    pub fn authorize_active_producer(
        &self,
        session_id: &str,
        principal_id: &str,
        producer_token: &str,
    ) -> Result<()> {
        self.authorize_producer(session_id, principal_id, producer_token)?;
        let connection = self.connection()?;
        let state: String = connection.query_row(
            "SELECT state FROM workspace_session_producers WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        if state != "active" {
            bail!("capture producer is no longer active");
        }
        Ok(())
    }

    pub fn rename_session(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        title: &str,
    ) -> Result<String> {
        validate_request_id(request_id)?;
        let title = title.trim();
        if title.is_empty() {
            bail!("title must not be empty");
        }
        let fingerprint = digest(title.as_bytes());
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint, response_json FROM workspace_session_mutation_receipts WHERE session_id = ?1 AND principal_id = ?2 AND operation = 'rename' AND request_id = ?3",
                params![session_id, principal_id, request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((prior_fingerprint, response)) = prior {
            if prior_fingerprint != fingerprint {
                bail!("rename request id was reused with different content");
            }
            return Ok(serde_json::from_str(&response)?);
        }
        let changed = tx.execute(
            "UPDATE sessions SET title = ?1 WHERE name = ?2",
            params![title, session_id],
        )?;
        if changed == 0 {
            bail!("session not found");
        }
        let response = serde_json::to_string(title)?;
        tx.execute(
            "INSERT INTO workspace_session_mutation_receipts (session_id, principal_id, operation, request_id, fingerprint, response_json) VALUES (?1, ?2, 'rename', ?3, ?4, ?5)",
            params![session_id, principal_id, request_id, fingerprint, response],
        )?;
        tx.commit()?;
        Ok(title.to_string())
    }

    pub fn link_note(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        source_id: &str,
        relative_path: &str,
        observed_hash: Option<&str>,
        expected_revision: u64,
        bb_thread_id: Option<&str>,
        distilled_memo_revision: Option<&str>,
    ) -> Result<canonical::NoteAssociation> {
        validate_request_id(request_id)?;
        let fingerprint = digest(
            format!(
                "{source_id}\0{relative_path}\0{}\0{expected_revision}\0{}\0{}",
                observed_hash.unwrap_or_default(),
                bb_thread_id.unwrap_or_default(),
                distilled_memo_revision.unwrap_or_default()
            )
            .as_bytes(),
        );
        if let Some(response) = self.mutation_receipt(
            session_id,
            principal_id,
            "note_link",
            request_id,
            &fingerprint,
        )? {
            return Ok(serde_json::from_str(&response)?);
        }
        let association = canonical::link_note_with_distillation(
            &self.directory,
            session_id,
            source_id,
            relative_path,
            observed_hash,
            expected_revision,
            bb_thread_id,
            distilled_memo_revision,
        )?;
        self.record_mutation_receipt(
            session_id,
            principal_id,
            "note_link",
            request_id,
            &fingerprint,
            &serde_json::to_string(&association)?,
        )?;
        Ok(association)
    }

    pub fn unlink_note(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        expected_revision: u64,
    ) -> Result<()> {
        validate_request_id(request_id)?;
        let fingerprint = digest(expected_revision.to_string().as_bytes());
        if self
            .mutation_receipt(
                session_id,
                principal_id,
                "note_unlink",
                request_id,
                &fingerprint,
            )?
            .is_some()
        {
            return Ok(());
        }
        canonical::unlink_note(&self.directory, session_id, expected_revision)?;
        self.record_mutation_receipt(
            session_id,
            principal_id,
            "note_unlink",
            request_id,
            &fingerprint,
            "null",
        )
    }

    fn mutation_receipt(
        &self,
        session_id: &str,
        principal_id: &str,
        operation: &str,
        request_id: &str,
        fingerprint: &str,
    ) -> Result<Option<String>> {
        let connection = self.connection()?;
        let stored: Option<(String, String)> = connection
            .query_row(
                "SELECT fingerprint, response_json FROM workspace_session_mutation_receipts WHERE session_id = ?1 AND principal_id = ?2 AND operation = ?3 AND request_id = ?4",
                params![session_id, principal_id, operation, request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((stored_fingerprint, response)) = stored {
            if stored_fingerprint != fingerprint {
                bail!("mutation request id was reused with different content");
            }
            Ok(Some(response))
        } else {
            Ok(None)
        }
    }

    fn record_mutation_receipt(
        &self,
        session_id: &str,
        principal_id: &str,
        operation: &str,
        request_id: &str,
        fingerprint: &str,
        response: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT OR IGNORE INTO workspace_session_mutation_receipts (session_id, principal_id, operation, request_id, fingerprint, response_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, principal_id, operation, request_id, fingerprint, response],
        )?;
        Ok(())
    }

    pub fn release_producer(
        &self,
        session_id: &str,
        principal_id: &str,
        producer_token: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE workspace_session_producers SET state = 'released', updated_at_ms = ?1 WHERE session_id = ?2 AND principal_id = ?3 AND producer_token_hash = ?4 AND state = 'active'",
            params![now_ms(), session_id, principal_id, digest(producer_token.as_bytes())],
        )?;
        if changed == 0 {
            // Exact finalize replays after the matching producer was already
            // released are successful; a stale generation must not release a
            // newer producer.
            self.authorize_producer(session_id, principal_id, producer_token)?;
        }
        Ok(())
    }

    /// Refresh the durable browser owner lease only for the active producer.
    pub fn touch_producer(
        &self,
        session_id: &str,
        principal_id: &str,
        producer_token: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE workspace_session_producers SET updated_at_ms = ?1 WHERE session_id = ?2 AND principal_id = ?3 AND producer_token_hash = ?4 AND state = 'active'",
            params![now_ms(), session_id, principal_id, digest(producer_token.as_bytes())],
        )?;
        if changed != 1 {
            self.authorize_producer(session_id, principal_id, producer_token)?;
            bail!("capture producer is no longer active");
        }
        Ok(())
    }

    /// Claim expired browser producers in the same durable authority row used
    /// by capture authorization. A claimed row is retried after server restart
    /// until incomplete finalization releases it.
    pub fn claim_expired_browser_producers(
        &self,
        principal_id: &str,
        cutoff_ms: i64,
    ) -> Result<Vec<String>> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE workspace_session_producers SET state = 'abandoned' WHERE principal_id = ?1 AND state = 'active' AND updated_at_ms <= ?2 AND session_id LIKE 'browser-%'",
            params![principal_id, cutoff_ms],
        )?;
        let sessions = {
            let mut statement = tx.prepare(
                "SELECT session_id FROM workspace_session_producers WHERE principal_id = ?1 AND state = 'abandoned' AND session_id LIKE 'browser-%' ORDER BY updated_at_ms ASC LIMIT 100",
            )?;
            let rows = statement
                .query_map(params![principal_id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            rows
        };
        tx.commit()?;
        Ok(sessions)
    }

    pub fn require_abandoned_producer(&self, session_id: &str, principal_id: &str) -> Result<()> {
        let connection = self.connection()?;
        let state: Option<String> = connection
            .query_row(
                "SELECT state FROM workspace_session_producers WHERE session_id = ?1 AND principal_id = ?2",
                params![session_id, principal_id],
                |row| row.get(0),
            )
            .optional()?;
        if state.as_deref() != Some("abandoned") {
            bail!("browser capture has no claimed expired owner lease");
        }
        Ok(())
    }

    pub fn release_abandoned_producer(&self, session_id: &str, principal_id: &str) -> Result<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE workspace_session_producers SET state = 'released', updated_at_ms = ?1 WHERE session_id = ?2 AND principal_id = ?3 AND state = 'abandoned'",
            params![now_ms(), session_id, principal_id],
        )?;
        if changed != 1 {
            bail!("browser capture expired owner lease was not claimed");
        }
        Ok(())
    }

    pub fn set_current(
        &self,
        principal_id: &str,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO workspace_client_current (principal_id, workspace_id, session_id, updated_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(principal_id, workspace_id) DO UPDATE SET session_id = excluded.session_id, updated_at_ms = excluded.updated_at_ms",
            params![principal_id, workspace_id, session_id, now_ms()],
        )?;
        Ok(())
    }

    pub fn current(&self, principal_id: &str, workspace_id: &str) -> Result<Option<String>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT session_id FROM workspace_client_current WHERE principal_id = ?1 AND workspace_id = ?2",
                params![principal_id, workspace_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Producer reservations are Workspace-local. Readers can discover their
    /// session IDs without receiving a producer identity or secret.
    pub fn active_session_ids(&self) -> Result<Vec<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT session_id FROM workspace_session_producers WHERE state = 'active' ORDER BY updated_at_ms DESC, session_id DESC LIMIT 100",
        )?;
        let sessions = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(sessions)
    }

    pub fn memo(&self, session_id: &str) -> Result<AuthorityMemoReceipt> {
        let connection = self.connection()?;
        let stored: Option<(String, String)> = connection
            .query_row(
                "SELECT revision, document_json FROM workspace_memos WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let document = if let Some((revision, json)) = stored {
            let lines: Vec<TimedMemoLine> = serde_json::from_str(&json)?;
            let mirror_stale = self
                .mirror_is_stale_for_lines(session_id, &lines)
                .unwrap_or_else(|error| {
                    log::warn!("could not inspect memo mirror for {session_id}: {error}");
                    true
                });
            return Ok(AuthorityMemoReceipt {
                revision,
                lines,
                replayed: false,
                mirror_stale,
            });
        } else {
            let markdown = std::fs::read_to_string(self.directory.join(format!("{session_id}.md")))
                .unwrap_or_default();
            TimedMemoDocument::parse_markdown(&markdown)
        };
        Ok(AuthorityMemoReceipt {
            revision: document.revision(),
            lines: document.into_lines(),
            replayed: false,
            mirror_stale: false,
        })
    }

    pub fn update_memo(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        expected_revision: &str,
        observed_at_ms: u64,
        paused: bool,
        text: &str,
    ) -> Result<AuthorityMemoReceipt> {
        self.write_memo(
            session_id,
            principal_id,
            request_id,
            expected_revision,
            MemoWrite::PlainText {
                observed_at_ms,
                paused,
                text,
            },
        )
    }

    /// Replace an already-timestamped memo through the same revisioned
    /// authority used by plain-text remote edits. Desktop capture owns the
    /// timestamps it supplies; SQLite owns CAS, retry receipts, and the
    /// Markdown projection.
    pub fn replace_memo_lines(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        expected_revision: &str,
        lines: &[TimedMemoLine],
    ) -> Result<AuthorityMemoReceipt> {
        self.write_memo(
            session_id,
            principal_id,
            request_id,
            expected_revision,
            MemoWrite::ReplaceLines(lines),
        )
    }

    /// SQLite is the memo authority. The Markdown file is a repairable,
    /// post-commit projection; a failed mirror refresh never reverses a commit.
    pub fn write_memo(
        &self,
        session_id: &str,
        principal_id: &str,
        request_id: &str,
        expected_revision: &str,
        write: MemoWrite<'_>,
    ) -> Result<AuthorityMemoReceipt> {
        validate_request_id(request_id)?;
        let fingerprint = match &write {
            MemoWrite::PlainText {
                observed_at_ms,
                paused,
                text,
            } => digest(
                format!("{expected_revision}\0{observed_at_ms}\0{paused}\0{text}").as_bytes(),
            ),
            MemoWrite::ReplaceLines(lines) => {
                digest(format!("{expected_revision}\0{}", serde_json::to_string(lines)?).as_bytes())
            }
        };
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint, response_json FROM workspace_memo_receipts WHERE session_id = ?1 AND principal_id = ?2 AND request_id = ?3",
                params![session_id, principal_id, request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((prior_fingerprint, response)) = prior {
            if prior_fingerprint != fingerprint {
                bail!("memo request id was reused with different content");
            }
            let mut receipt: AuthorityMemoReceiptWire = serde_json::from_str(&response)?;
            receipt.replayed = true;
            drop(tx);
            // A replay is read-only: the TUI may have edited the Markdown
            // mirror directly since the original request was committed.
            receipt.mirror_stale = self
                .memo_mirror_is_stale(session_id)
                .unwrap_or_else(|error| {
                    log::warn!("could not inspect memo mirror for {session_id}: {error}");
                    true
                });
            return Ok(receipt.into());
        }
        let current = self.memo_in_transaction(&tx, session_id)?;
        if current.revision != expected_revision {
            return Err(MemoRevisionConflict {
                expected: expected_revision.to_string(),
                current: current.revision,
            }
            .into());
        }
        let document = match write {
            MemoWrite::PlainText {
                observed_at_ms,
                paused,
                text,
            } => {
                let elapsed = observed_at_ms as f64 / 1000.0;
                let moment = if paused {
                    MemoMoment::paused(elapsed, 1)
                } else {
                    MemoMoment::recording(elapsed)
                };
                TimedMemoDocument::from_committed(current.lines).reconcile_plain_text(text, moment)
            }
            MemoWrite::ReplaceLines(lines) => TimedMemoDocument::from_committed(lines.to_vec()),
        };
        let receipt = AuthorityMemoReceipt {
            revision: document.revision(),
            lines: document.clone().into_lines(),
            replayed: false,
            mirror_stale: false,
        };
        let response = serde_json::to_string(&AuthorityMemoReceiptWire::from(&receipt))?;
        tx.execute(
            "INSERT INTO workspace_memos (session_id, revision, document_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(session_id) DO UPDATE SET revision = excluded.revision, document_json = excluded.document_json, updated_at_ms = excluded.updated_at_ms",
            params![session_id, receipt.revision, serde_json::to_string(&receipt.lines)?, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO workspace_memo_receipts (session_id, principal_id, request_id, fingerprint, response_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, principal_id, request_id, fingerprint, response],
        )?;
        tx.commit()?;
        let mut receipt = receipt;
        if let Err(error) = self.refresh_memo_mirror(session_id) {
            log::warn!("memo mirror projection failed for {session_id}: {error}");
            receipt.mirror_stale = true;
        }
        Ok(receipt)
    }

    fn memo_mirror_is_stale(&self, session_id: &str) -> Result<bool> {
        Ok(self.memo(session_id)?.mirror_stale)
    }

    fn mirror_is_stale_for_lines(&self, session_id: &str, lines: &[TimedMemoLine]) -> Result<bool> {
        let expected = TimedMemoDocument::from_committed(lines.to_vec()).export_markdown();
        match std::fs::read_to_string(self.directory.join(format!("{session_id}.md"))) {
            Ok(actual) => Ok(actual != expected),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    /// Rebuild the optional Markdown export from committed SQLite state.
    pub fn refresh_memo_mirror(&self, session_id: &str) -> Result<()> {
        // Serialize projections with memo writes, including other processes.
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = self.memo_in_transaction(&tx, session_id)?;
        let document = TimedMemoDocument::from_committed(receipt.lines);
        atomic_write(
            &self.directory.join(format!("{session_id}.md")),
            document.export_markdown().as_bytes(),
        )?;
        tx.commit()?;
        Ok(())
    }

    fn memo_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        session_id: &str,
    ) -> Result<AuthorityMemoReceipt> {
        let stored: Option<(String, String)> = tx
            .query_row(
                "SELECT revision, document_json FROM workspace_memos WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((revision, json)) = stored {
            return Ok(AuthorityMemoReceipt {
                revision,
                lines: serde_json::from_str(&json)?,
                replayed: false,
                mirror_stale: false,
            });
        }
        let markdown = std::fs::read_to_string(self.directory.join(format!("{session_id}.md")))
            .unwrap_or_default();
        let document = TimedMemoDocument::parse_markdown(&markdown);
        Ok(AuthorityMemoReceipt {
            revision: document.revision(),
            lines: document.into_lines(),
            replayed: false,
            mirror_stale: false,
        })
    }

    pub fn record_import(
        &self,
        upload_id: &str,
        principal_id: &str,
        session_id: &str,
        original_filename: &str,
        bytes: &[u8],
    ) -> Result<ImportReceipt> {
        validate_request_id(upload_id)?;
        let digest = digest(bytes);
        let existing = self.import_receipt(principal_id, upload_id)?;
        if let Some(mut receipt) = existing {
            if receipt.digest != digest || receipt.session_id != session_id {
                bail!("upload id was reused with different content");
            }
            receipt.replayed = true;
            return Ok(receipt);
        }
        let imports = self.directory.join("imports");
        std::fs::create_dir_all(&imports)?;
        let stored_name = format!("{upload_id}-{digest}.upload");
        atomic_write(&imports.join(&stored_name), bytes)?;
        let stored_path = format!(".margins/imports/{stored_name}");
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO workspace_import_receipts (upload_id, principal_id, digest, session_id, size_bytes, stored_path, original_filename, processing_state, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'durable_received', ?8)",
            params![upload_id, principal_id, digest, session_id, i64::try_from(bytes.len()).context("import too large")?, stored_path, sanitize_filename(original_filename), now_ms()],
        )?;
        Ok(ImportReceipt {
            upload_id: upload_id.to_string(),
            session_id: session_id.to_string(),
            digest,
            size_bytes: bytes.len() as u64,
            stored_path,
            processing_state: "durable_received".to_string(),
            replayed: false,
        })
    }

    pub fn import_receipt(
        &self,
        principal_id: &str,
        upload_id: &str,
    ) -> Result<Option<ImportReceipt>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT upload_id, session_id, digest, size_bytes, stored_path, processing_state FROM workspace_import_receipts WHERE upload_id = ?1 AND principal_id = ?2",
                params![upload_id, principal_id],
                |row| {
                    Ok(ImportReceipt {
                        upload_id: row.get(0)?,
                        session_id: row.get(1)?,
                        digest: row.get(2)?,
                        size_bytes: row.get::<_, i64>(3)?.max(0) as u64,
                        stored_path: row.get(4)?,
                        processing_state: row.get(5)?,
                        replayed: false,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthorityMemoReceiptWire {
    revision: String,
    lines: Vec<TimedMemoLine>,
    replayed: bool,
    #[serde(default)]
    mirror_stale: bool,
}

impl From<&AuthorityMemoReceipt> for AuthorityMemoReceiptWire {
    fn from(value: &AuthorityMemoReceipt) -> Self {
        Self {
            revision: value.revision.clone(),
            lines: value.lines.clone(),
            replayed: value.replayed,
            mirror_stale: value.mirror_stale,
        }
    }
}

impl From<AuthorityMemoReceiptWire> for AuthorityMemoReceipt {
    fn from(value: AuthorityMemoReceiptWire) -> Self {
        Self {
            revision: value.revision,
            lines: value.lines,
            replayed: value.replayed,
            mirror_stale: value.mirror_stale,
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_request_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("request id is invalid");
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn sanitize_filename(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("recording")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(160)
        .collect()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("authority path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("write"),
        now_ms()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
