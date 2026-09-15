use crate::canonical;
use anyhow::{bail, Context, Result};
use margins_core::{MemoMoment, TimedMemoDocument, TimedMemoLine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorityMemoReceipt {
    pub revision: String,
    pub lines: Vec<TimedMemoLine>,
    pub replayed: bool,
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
            bail!("session already has a capture producer");
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
        if owner != principal_id || hash != digest(producer_token.as_bytes()) || state == "released"
        {
            bail!("capture producer authorization failed");
        }
        Ok(())
    }

    pub fn release_producer(&self, session_id: &str) -> Result<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE workspace_session_producers SET state = 'released', updated_at_ms = ?1 WHERE session_id = ?2",
            params![now_ms(), session_id],
        )?;
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
            return Ok(AuthorityMemoReceipt {
                revision,
                lines,
                replayed: false,
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
        validate_request_id(request_id)?;
        let fingerprint =
            digest(format!("{expected_revision}\0{observed_at_ms}\0{paused}\0{text}").as_bytes());
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
            return Ok(receipt.into());
        }
        let stored: Option<(String, String)> = tx
            .query_row(
                "SELECT revision, document_json FROM workspace_memos WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let current = if let Some((revision, json)) = stored {
            AuthorityMemoReceipt {
                revision,
                lines: serde_json::from_str(&json)?,
                replayed: false,
            }
        } else {
            let markdown = std::fs::read_to_string(self.directory.join(format!("{session_id}.md")))
                .unwrap_or_default();
            let document = TimedMemoDocument::parse_markdown(&markdown);
            AuthorityMemoReceipt {
                revision: document.revision(),
                lines: document.into_lines(),
                replayed: false,
            }
        };
        if current.revision != expected_revision {
            bail!(
                "memo revision conflict: current revision is {}",
                current.revision
            );
        }
        let elapsed = observed_at_ms as f64 / 1000.0;
        let moment = if paused {
            MemoMoment::paused(elapsed, 1)
        } else {
            MemoMoment::recording(elapsed)
        };
        let document =
            TimedMemoDocument::from_committed(current.lines).reconcile_plain_text(text, moment);
        let receipt = AuthorityMemoReceipt {
            revision: document.revision(),
            lines: document.clone().into_lines(),
            replayed: false,
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
        atomic_write(
            &self.directory.join(format!("{session_id}.md")),
            document.export_markdown().as_bytes(),
        )?;
        Ok(receipt)
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
        validate_request_id(request_id)?;
        let document = TimedMemoDocument::from_committed(lines.to_vec());
        let fingerprint =
            digest(format!("{expected_revision}\0{}", serde_json::to_string(lines)?).as_bytes());
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
            return Ok(receipt.into());
        }
        let current = self.memo_in_transaction(&tx, session_id)?;
        if current.revision != expected_revision {
            bail!(
                "memo revision conflict: current revision is {}",
                current.revision
            );
        }
        let receipt = AuthorityMemoReceipt {
            revision: document.revision(),
            lines: lines.to_vec(),
            replayed: false,
        };
        let response = serde_json::to_string(&AuthorityMemoReceiptWire::from(&receipt))?;
        tx.execute(
            "INSERT INTO workspace_memos (session_id, revision, document_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(session_id) DO UPDATE SET revision = excluded.revision, document_json = excluded.document_json, updated_at_ms = excluded.updated_at_ms",
            params![session_id, receipt.revision, serde_json::to_string(lines)?, now_ms()],
        )?;
        tx.execute(
            "INSERT INTO workspace_memo_receipts (session_id, principal_id, request_id, fingerprint, response_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, principal_id, request_id, fingerprint, response],
        )?;
        tx.commit()?;
        atomic_write(
            &self.directory.join(format!("{session_id}.md")),
            document.export_markdown().as_bytes(),
        )?;
        Ok(receipt)
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
            });
        }
        let markdown = std::fs::read_to_string(self.directory.join(format!("{session_id}.md")))
            .unwrap_or_default();
        let document = TimedMemoDocument::parse_markdown(&markdown);
        Ok(AuthorityMemoReceipt {
            revision: document.revision(),
            lines: document.into_lines(),
            replayed: false,
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
            "INSERT INTO workspace_import_receipts (upload_id, principal_id, digest, session_id, size_bytes, stored_path, original_filename, processing_state, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
            params![upload_id, principal_id, digest, session_id, i64::try_from(bytes.len()).context("import too large")?, stored_path, sanitize_filename(original_filename), now_ms()],
        )?;
        Ok(ImportReceipt {
            upload_id: upload_id.to_string(),
            session_id: session_id.to_string(),
            digest,
            size_bytes: bytes.len() as u64,
            stored_path,
            processing_state: "queued".to_string(),
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
}

impl From<&AuthorityMemoReceipt> for AuthorityMemoReceiptWire {
    fn from(value: &AuthorityMemoReceipt) -> Self {
        Self {
            revision: value.revision.clone(),
            lines: value.lines.clone(),
            replayed: value.replayed,
        }
    }
}

impl From<AuthorityMemoReceiptWire> for AuthorityMemoReceipt {
    fn from(value: AuthorityMemoReceiptWire) -> Self {
        Self {
            revision: value.revision,
            lines: value.lines,
            replayed: value.replayed,
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
