use crate::{canonical, sqlite::init_repository_schema};
use anyhow::{Context, Result};
use margins_core::{
    ArtifactDescriptor, ArtifactId, AudioFormat, AudioLane, DurationMillis as CoreDurationMillis,
    NewSegment, SampleFormat, SegmentId as CoreSegmentId, UnixMillis as CoreUnixMillis,
};
use margins_meeting_protocol::{
    validate_opus_packet_stream_v1, AudioChunkV1, AudioCodecV1, AudioContainerV1, AudioFormatV1,
    LaneId, MessageId, SequenceRangeV1, ServerMessageBodyV1, ServerMessageV1, SessionId,
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
        canonical::database_path(&self.directory)
    }
    fn blob_dir(&self) -> PathBuf {
        self.directory.join("meeting-blobs")
    }

    /// Remove immutable chunks for one discarded session while its DB rows
    /// still identify the files. The identity hash includes the session ID.
    pub fn delete_session_blobs(&self, session_id: &str) -> Result<()> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT blob_path FROM meeting_chunks WHERE session_id = ?1")?;
        let paths = statement
            .query_map([session_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let blob_dir = self.blob_dir();
        let metadata = std::fs::symlink_metadata(&blob_dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!("meeting blob directory is not a regular directory");
        }
        for name in paths {
            let valid = name.len() == 70
                && name.ends_with(".chunk")
                && name[..64].bytes().all(|byte| byte.is_ascii_hexdigit());
            if !valid {
                anyhow::bail!("invalid meeting chunk path");
            }
            match std::fs::remove_file(blob_dir.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Inject one storage failure after immutable blob publication but before
    /// metadata/receipt commit. Used by crash-boundary contract tests.
    #[doc(hidden)]
    pub fn fail_before_metadata_commit_once(&self) {
        self.fail_before_metadata_commit
            .store(true, Ordering::Release);
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
        let connection = canonical::open_db(&self.directory)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        init_repository_schema(&connection)?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meeting_sessions (
                session_id TEXT PRIMARY KEY NOT NULL,
                create_key TEXT UNIQUE NOT NULL,
                revision INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(name) ON DELETE CASCADE
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
            CREATE TABLE IF NOT EXISTS meeting_segment_projection (
                session_id TEXT NOT NULL,
                segment_id TEXT NOT NULL,
                canonical_ordinal INTEGER NOT NULL,
                PRIMARY KEY (session_id, segment_id),
                UNIQUE (session_id, canonical_ordinal),
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

    fn assemble_finalized_segment(
        &self,
        delta: &SessionDeltaV1,
    ) -> Result<Option<FinalizedProjection>> {
        let Some(finalized) = delta.events.iter().find_map(|event| match &event.body {
            ServerMessageBodyV1::SegmentFinalized(value) => Some(value),
            _ => None,
        }) else {
            return Ok(None);
        };
        let session_id = delta.session.session_id();
        let declared_lanes = &delta.session.create().lanes;
        let artifacts = self.directory.join("artifacts").join(session_id.as_ref());
        std::fs::create_dir_all(&artifacts)?;
        let mut lanes = Vec::new();
        for boundary in &finalized.lane_boundaries {
            if boundary.next_sequence == 0 {
                continue;
            }
            let mut bytes = Vec::new();
            for sequence in 0..boundary.next_sequence {
                let payload = if delta.audio_chunk.as_ref().is_some_and(|chunk| {
                    chunk.segment_id == finalized.segment_id
                        && chunk.lane_id == boundary.lane_id
                        && chunk.sequence == sequence
                }) {
                    delta.audio_chunk.as_ref().unwrap().payload.clone()
                } else {
                    self.load_audio_chunk(
                        session_id,
                        finalized.segment_id.as_ref(),
                        &boundary.lane_id,
                        sequence,
                    )?
                    .with_context(|| {
                        format!(
                            "finalized lane {} is missing sequence {sequence}",
                            boundary.lane_id.as_ref()
                        )
                    })?
                    .payload
                };
                bytes.extend_from_slice(&payload);
            }
            let format = declared_lanes
                .iter()
                .find(|lane| lane.lane_id == boundary.lane_id)
                .context("finalized lane was not declared by the session")?
                .format
                .clone();
            let (extension, artifact_suffix, frame_count) = match (format.codec, format.container) {
                (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw) => {
                    if bytes.len() % 2 != 0 {
                        anyhow::bail!("finalized PCM lane has a partial s16 sample");
                    }
                    ("pcm", "pcm", bytes.len() as u64 / 2)
                }
                (AudioCodecV1::Opus, AudioContainerV1::PacketStream) => {
                    let summary =
                        validate_opus_packet_stream_v1(&bytes).map_err(anyhow::Error::msg)?;
                    ("mopus", "opus", summary.source_frame_count)
                }
                _ => anyhow::bail!("finalized remote audio uses an unsupported durable format"),
            };
            let file_name = format!(
                "{}_{}_{}.{}",
                safe_file_component(session_id.as_ref()),
                safe_file_component(finalized.segment_id.as_ref()),
                safe_file_component(boundary.lane_id.as_ref()),
                extension,
            );
            let path = artifacts.join(&file_name);
            atomic_replace(&path, &bytes)?;
            lanes.push((
                boundary.lane_id.clone(),
                format!(".margins/artifacts/{}/{file_name}", session_id.as_ref()),
                bytes.len() as u64,
                frame_count,
                artifact_suffix.to_string(),
                format,
            ));
        }
        if lanes.is_empty() {
            return Ok(None);
        }
        Ok(Some(FinalizedProjection {
            segment_id: finalized.segment_id.as_ref().to_string(),
            duration_secs: finalized.duration_ms.0 as f64 / 1000.0,
            lanes,
        }))
    }
}

struct FinalizedProjection {
    segment_id: String,
    duration_secs: f64,
    lanes: Vec<(LaneId, String, u64, u64, String, AudioFormatV1)>,
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
        let name = delta.session.session_id().as_ref();
        if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            anyhow::bail!("session id cannot be used as a storage path component");
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session_id = delta.session.session_id();
        let create = delta.session.create();
        let started_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            i64::try_from(create.started_at_unix_ms.0)
                .context("session start time exceeds SQLite range")?,
        )
        .context("session start time is invalid")?
        .to_rfc3339();
        let created_at = chrono::Utc::now().to_rfc3339();
        let note_path = format!(".margins/{}.md", session_id.as_ref());
        let canonical_created = tx.execute(
            "INSERT OR IGNORE INTO sessions (name, start_time, notes_path, created_at, title) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id.as_ref(), started_at, note_path, created_at, create.title],
        )?;
        if canonical_created == 0 {
            let existing_create_key: Option<String> = tx
                .query_row(
                    "SELECT create_key FROM meeting_sessions WHERE session_id = ?1",
                    params![session_id.as_ref()],
                    |row| row.get(0),
                )
                .optional()?;
            if existing_create_key.as_deref() != Some(create.idempotency_key.as_str()) {
                return Ok(StorageCommit::Conflict);
            }
        }
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
        if staged.is_some()
            && self
                .fail_before_metadata_commit
                .swap(false, Ordering::AcqRel)
        {
            anyhow::bail!("injected failure before meeting metadata commit");
        }
        let projection = self.assemble_finalized_segment(&delta)?;
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
        if let Some(projection) = projection {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT canonical_ordinal FROM meeting_segment_projection WHERE session_id = ?1 AND segment_id = ?2",
                    params![session_id.as_ref(), projection.segment_id],
                    |row| row.get(0),
                )
                .optional()?;
            let ordinal = if let Some(value) = existing {
                value
            } else {
                let next: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(segment_index) + 1, 0) FROM session_segments WHERE session_name = ?1",
                    params![session_id.as_ref()],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO meeting_segment_projection (session_id, segment_id, canonical_ordinal) VALUES (?1, ?2, ?3)",
                    params![session_id.as_ref(), projection.segment_id, next],
                )?;
                next
            };
            let offset_ms: i64 = tx.query_row(
                "SELECT COALESCE(MAX(offset_ms + CAST(ROUND(COALESCE(duration_secs, 0) * 1000.0) AS INTEGER)), 0) FROM session_segments WHERE session_name = ?1",
                params![session_id.as_ref()],
                |row| row.get(0),
            )?;
            let representative = projection
                .lanes
                .first()
                .context("finalized segment has no lanes")?;
            let started_at_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
            let started_at =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(started_at_ms as i64)
                    .context("segment timestamp is outside the supported range")?
                    .to_rfc3339();
            tx.execute(
                "INSERT OR IGNORE INTO session_segments (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![session_id.as_ref(), ordinal, representative.1, offset_ms, projection.duration_secs, started_at],
            )?;
            let duration_ms = (projection.duration_secs * 1000.0).round().max(0.0) as u64;
            let lane = match representative.0.as_ref() {
                "mic" | "microphone" => Some(AudioLane::Microphone),
                "system" => Some(AudioLane::System),
                _ => None,
            };
            let contract = NewSegment {
                id: CoreSegmentId::from(projection.segment_id.clone()),
                ordinal: ordinal.max(0) as u64,
                start_offset_ms: offset_ms.max(0) as u64,
                duration_ms: CoreDurationMillis(duration_ms),
                started_at_ms: CoreUnixMillis(started_at_ms),
                audio: ArtifactDescriptor {
                    id: ArtifactId::from(format!(
                        "{}:{}:{}",
                        session_id.as_ref(),
                        projection.segment_id,
                        representative.0.as_ref()
                    )),
                    segment_id: CoreSegmentId::from(projection.segment_id.clone()),
                    lane,
                    uri: representative.1.clone(),
                    format: core_audio_format(&representative.5)?,
                    duration_ms: CoreDurationMillis(duration_ms),
                    frame_count: representative.3,
                    byte_length: Some(representative.2),
                },
                dropped_live_frames: 0,
                dropped_durable_frames: 0,
                timeline_reusable: true,
            };
            tx.execute(
                "INSERT OR IGNORE INTO session_segment_contracts (session_name, segment_index, segment_id, contract_json) VALUES (?1, ?2, ?3, ?4)",
                params![session_id.as_ref(), ordinal, projection.segment_id, serde_json::to_string(&contract)?],
            )?;
            for (lane, path, _, _, artifact_suffix, _) in projection.lanes {
                let kind = format!(
                    "audio_{}_{}",
                    safe_file_component(lane.as_ref()),
                    artifact_suffix
                );
                tx.execute(
                    "INSERT INTO session_artifacts (session_name, kind, ordinal, path, retention_class, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, 'durable', ?5, NULL) ON CONFLICT(session_name, kind, ordinal) DO UPDATE SET path = excluded.path, retention_class = excluded.retention_class",
                    params![session_id.as_ref(), kind, ordinal, path, chrono::Utc::now().to_rfc3339()],
                )?;
            }
        }
        if delta
            .events
            .iter()
            .any(|event| matches!(event.body, ServerMessageBodyV1::SessionFinalized(_)))
        {
            tx.execute(
                "UPDATE sessions SET lifecycle_state = 'ended', lifecycle_updated_at = ?1 WHERE name = ?2 AND lifecycle_state = 'active'",
                params![chrono::Utc::now().to_rfc3339(), session_id.as_ref()],
            )?;
            // Existing transcript readers use this fallback until processing
            // publishes a real transcript. It explicitly reports pending ASR.
            let path = self
                .directory
                .join(format!("{}_capture_context.md", session_id.as_ref()));
            if !path.exists() {
                atomic_replace(&path, b"# Capture saved\n\nTranscript pending. Audio is saved in this session's artifacts.\n")?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO session_artifacts (session_name, kind, ordinal, path, retention_class, created_at, expires_at) VALUES (?1, 'capture_context', 0, ?2, 'durable', ?3, NULL)",
                params![session_id.as_ref(), format!(".margins/{}_capture_context.md", session_id.as_ref()), chrono::Utc::now().to_rfc3339()],
            )?;
        }
        tx.commit()?;
        Ok(StorageCommit::Committed)
    }
}

fn core_audio_format(format: &AudioFormatV1) -> Result<AudioFormat> {
    let supported = matches!(
        (format.codec, format.container),
        (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw)
            | (AudioCodecV1::Opus, AudioContainerV1::PacketStream)
    );
    if !supported {
        anyhow::bail!("finalized remote audio uses an unsupported durable format");
    }
    if format.channel_count != 1 || format.sample_rate_hz == 0 {
        anyhow::bail!("finalized remote audio must be mono with a nonzero sample rate");
    }
    Ok(AudioFormat {
        sample_rate_hz: format.sample_rate_hz,
        channel_count: format.channel_count,
        sample_format: SampleFormat::Signed16,
    })
}

fn safe_file_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(100)
        .collect()
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        if std::fs::read(path)? == bytes {
            return Ok(());
        }
        anyhow::bail!("finalized lane projection conflicts with existing bytes");
    }
    let parent = path.parent().context("projection path has no parent")?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio"),
        std::process::id(),
        TEMPORARY_BLOB_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    match std::fs::hard_link(&temporary, path) {
        Ok(()) => std::fs::remove_file(&temporary)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(&temporary)?;
            if std::fs::read(path)? != bytes {
                anyhow::bail!("finalized lane projection conflicts with existing bytes");
            }
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}
