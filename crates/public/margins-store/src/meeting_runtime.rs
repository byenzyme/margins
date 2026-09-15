use anyhow::{Context, Result};
use margins_meeting_protocol::{
    AudioChunkV1, LaneId, MessageId, SequenceRangeV1, ServerMessageV1, SessionId,
};
use margins_meeting_runtime::{
    MeetingRuntimeStorage, SessionDeltaV1, StorageCommit, StoredCommandReceiptV1, StoredSessionV1,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

static TEMPORARY_BLOB_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingRuntimeStorageStats {
    pub sessions: u64,
    pub receipts: u64,
    pub events: u64,
    pub chunks: u64,
    pub session_state_bytes: u64,
    pub chunk_metadata_bytes: u64,
    pub blob_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct SqliteMeetingRuntimeStorage {
    directory: PathBuf,
    fail_before_metadata_commit: Arc<AtomicBool>,
}

impl SqliteMeetingRuntimeStorage {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let storage = Self {
            directory: directory.into(),
            fail_before_metadata_commit: Arc::new(AtomicBool::new(false)),
        };
        std::fs::create_dir_all(storage.blob_dir())?;
        storage.connection()?;
        Ok(storage)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn database_path(&self) -> PathBuf {
        self.directory.join("meeting-runtime.sqlite")
    }
    fn blob_dir(&self) -> PathBuf {
        self.directory.join("meeting-blobs")
    }

    /// Inject one storage failure after immutable blob publication but before
    /// metadata/receipt commit. Used by crash-boundary contract tests.
    #[doc(hidden)]
    pub fn fail_before_metadata_commit_once(&self) {
        self.fail_before_metadata_commit.store(true, Ordering::Release);
    }

    /// Storage-level counters used by scaling and operational diagnostics.
    /// This intentionally reports sizes rather than loading any payload.
    pub fn stats(&self) -> Result<MeetingRuntimeStorageStats> {
        let connection = self.connection()?;
        let row = connection.query_row(
            r#"
            SELECT
              (SELECT COUNT(*) FROM meeting_sessions),
              (SELECT COUNT(*) FROM meeting_receipts),
              (SELECT COUNT(*) FROM meeting_events),
              (SELECT COUNT(*) FROM meeting_chunks),
              COALESCE((SELECT SUM(LENGTH(state_json)) FROM meeting_sessions), 0),
              COALESCE((SELECT SUM(LENGTH(metadata_json)) FROM meeting_chunks), 0)
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;
        let blob_bytes = std::fs::read_dir(self.blob_dir())?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .sum();
        Ok(MeetingRuntimeStorageStats {
            sessions: row.0.max(0) as u64,
            receipts: row.1.max(0) as u64,
            events: row.2.max(0) as u64,
            chunks: row.3.max(0) as u64,
            session_state_bytes: row.4.max(0) as u64,
            chunk_metadata_bytes: row.5.max(0) as u64,
            blob_bytes,
        })
    }

    fn connection(&self) -> Result<Connection> {
        std::fs::create_dir_all(&self.directory)?;
        let connection = Connection::open(self.database_path())?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meeting_sessions (
                session_id TEXT PRIMARY KEY NOT NULL,
                create_key TEXT UNIQUE NOT NULL,
                revision INTEGER NOT NULL,
                state_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meeting_receipts (
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                response_start INTEGER NOT NULL,
                response_end INTEGER NOT NULL,
                PRIMARY KEY (session_id, message_id, fingerprint),
                FOREIGN KEY (session_id) REFERENCES meeting_sessions(session_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS meeting_events (
                session_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY (session_id, sequence),
                FOREIGN KEY (session_id) REFERENCES meeting_sessions(session_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS meeting_chunks (
                session_id TEXT NOT NULL,
                segment_id TEXT NOT NULL,
                lane_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                metadata_json TEXT NOT NULL,
                blob_path TEXT NOT NULL,
                PRIMARY KEY (session_id, segment_id, lane_id, sequence),
                FOREIGN KEY (session_id) REFERENCES meeting_sessions(session_id) ON DELETE CASCADE
            );
            "#,
        )?;
        Ok(connection)
    }

    fn stage_chunk(
        &self,
        session_id: &SessionId,
        chunk: &AudioChunkV1,
    ) -> Result<(String, String)> {
        let mut identity = Sha256::new();
        identity.update(session_id.as_ref().as_bytes());
        identity.update([0]);
        identity.update(chunk.segment_id.as_ref().as_bytes());
        identity.update([0]);
        identity.update(chunk.lane_id.as_ref().as_bytes());
        identity.update(chunk.sequence.to_le_bytes());
        identity.update(chunk.payload_digest.hex.as_bytes());
        let name = format!("{:x}.chunk", identity.finalize());
        let final_path = self.blob_dir().join(&name);
        if final_path.exists() {
            let existing = std::fs::read(&final_path)?;
            if existing != chunk.payload {
                anyhow::bail!("durable chunk identity conflicts with existing bytes");
            }
        } else {
            let temporary = self.blob_dir().join(format!(
                ".{name}.{}.{}.tmp",
                std::process::id(),
                TEMPORARY_BLOB_ID.fetch_add(1, Ordering::Relaxed),
            ));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&chunk.payload)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &final_path)?;
            File::open(self.blob_dir())?.sync_all()?;
        }
        let mut metadata = chunk.clone();
        metadata.payload.clear();
        Ok((name, serde_json::to_string(&metadata)?))
    }

    fn insert_receipt(
        connection: &Connection,
        session_id: &SessionId,
        receipt: &StoredCommandReceiptV1,
    ) -> Result<()> {
        connection.execute(
            "INSERT INTO meeting_receipts (session_id, message_id, fingerprint, response_start, response_end) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id.as_ref(), receipt.message_id.as_ref(), receipt.fingerprint, receipt.response_range.start as i64, receipt.response_range.end_exclusive as i64],
        )?;
        Ok(())
    }

    fn insert_events(
        connection: &Connection,
        session_id: &SessionId,
        events: &[ServerMessageV1],
    ) -> Result<()> {
        for event in events {
            connection.execute(
                "INSERT INTO meeting_events (session_id, sequence, event_json) VALUES (?1, ?2, ?3)",
                params![
                    session_id.as_ref(),
                    event.sequence as i64,
                    serde_json::to_string(event)?
                ],
            )?;
        }
        Ok(())
    }
}

impl MeetingRuntimeStorage for SqliteMeetingRuntimeStorage {
    type Error = anyhow::Error;

    fn load_session(&self, session_id: &SessionId) -> Result<Option<StoredSessionV1>> {
        let connection = self.connection()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT state_json FROM meeting_sessions WHERE session_id = ?1",
                params![session_id.as_ref()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).context("invalid meeting session state"))
            .transpose()
    }

    fn load_session_by_create_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<StoredSessionV1>> {
        let connection = self.connection()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT state_json FROM meeting_sessions WHERE create_key = ?1",
                params![idempotency_key],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).context("invalid meeting session state"))
            .transpose()
    }

    fn load_command_receipts(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<Vec<StoredCommandReceiptV1>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT fingerprint, response_start, response_end FROM meeting_receipts WHERE session_id = ?1 AND message_id = ?2 ORDER BY rowid",
        )?;
        let rows =
            statement.query_map(params![session_id.as_ref(), message_id.as_ref()], |row| {
                Ok(StoredCommandReceiptV1 {
                    message_id: message_id.clone(),
                    fingerprint: row.get(0)?,
                    response_range: SequenceRangeV1 {
                        start: row.get::<_, i64>(1)?.max(0) as u64,
                        end_exclusive: row.get::<_, i64>(2)?.max(0) as u64,
                    },
                })
            })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn load_events(
        &self,
        session_id: &SessionId,
        range: SequenceRangeV1,
        limit: usize,
    ) -> Result<Vec<ServerMessageV1>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_json FROM meeting_events WHERE session_id = ?1 AND sequence >= ?2 AND sequence < ?3 ORDER BY sequence LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![
                session_id.as_ref(),
                range.start as i64,
                range.end_exclusive as i64,
                limit as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    fn load_audio_chunk(
        &self,
        session_id: &SessionId,
        segment_id: &str,
        lane_id: &LaneId,
        sequence: u64,
    ) -> Result<Option<AudioChunkV1>> {
        let connection = self.connection()?;
        let row: Option<(String, String)> = connection.query_row(
            "SELECT metadata_json, blob_path FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 AND sequence = ?4",
            params![session_id.as_ref(), segment_id, lane_id.as_ref(), sequence as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        row.map(|(metadata, blob)| {
            let mut chunk: AudioChunkV1 = serde_json::from_str(&metadata)?;
            chunk.payload = std::fs::read(self.blob_dir().join(blob))?;
            Ok(chunk)
        })
        .transpose()
    }

    fn create_session(&self, delta: SessionDeltaV1) -> Result<StorageCommit> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session_id = delta.session.session_id();
        let changed = tx.execute(
            "INSERT OR IGNORE INTO meeting_sessions (session_id, create_key, revision, state_json) VALUES (?1, ?2, ?3, ?4)",
            params![session_id.as_ref(), delta.session.create().idempotency_key, delta.session.revision() as i64, serde_json::to_string(&delta.session)?],
        )?;
        if changed == 0 {
            return Ok(StorageCommit::Conflict);
        }
        Self::insert_receipt(&tx, session_id, &delta.receipt)?;
        Self::insert_events(&tx, session_id, &delta.events)?;
        tx.commit()?;
        Ok(StorageCommit::Committed)
    }

    fn apply_delta(&self, delta: SessionDeltaV1) -> Result<StorageCommit> {
        let session_id = delta.session.session_id().clone();
        let staged = delta
            .audio_chunk
            .as_ref()
            .map(|chunk| self.stage_chunk(&session_id, chunk))
            .transpose()?;
        if staged.is_some() && self.fail_before_metadata_commit.swap(false, Ordering::AcqRel) {
            anyhow::bail!("injected failure before meeting metadata commit");
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision: Option<i64> = tx
            .query_row(
                "SELECT revision FROM meeting_sessions WHERE session_id = ?1",
                params![session_id.as_ref()],
                |row| row.get(0),
            )
            .optional()?;
        if revision.map(|value| value.max(0) as u64) != Some(delta.expected_revision) {
            return Ok(StorageCommit::Conflict);
        }
        if let (Some(chunk), Some((blob_path, metadata_json))) = (&delta.audio_chunk, staged) {
            tx.execute(
                "INSERT INTO meeting_chunks (session_id, segment_id, lane_id, sequence, metadata_json, blob_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![session_id.as_ref(), chunk.segment_id.as_ref(), chunk.lane_id.as_ref(), chunk.sequence as i64, metadata_json, blob_path],
            )?;
        }
        Self::insert_receipt(&tx, &session_id, &delta.receipt)?;
        Self::insert_events(&tx, &session_id, &delta.events)?;
        let changed = tx.execute(
            "UPDATE meeting_sessions SET revision = ?1, state_json = ?2 WHERE session_id = ?3 AND revision = ?4",
            params![delta.session.revision() as i64, serde_json::to_string(&delta.session)?, session_id.as_ref(), delta.expected_revision as i64],
        )?;
        if changed != 1 {
            return Ok(StorageCommit::Conflict);
        }
        tx.commit()?;
        Ok(StorageCommit::Committed)
    }
}
