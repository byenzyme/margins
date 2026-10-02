use crate::{canonical, sqlite::init_repository_schema};
use anyhow::{Context, Result};
use margins_core::{
    ArtifactDescriptor, ArtifactId, AudioFormat, AudioLane, DurationMillis as CoreDurationMillis,
    NewSegment, SampleFormat, SegmentId as CoreSegmentId, UnixMillis as CoreUnixMillis,
};
use margins_meeting_protocol::{
    decode_opus_packet_blocks_v1, AudioChunkV1, AudioCodecV1, AudioContainerV1, AudioFormatV1,
    LaneId, MessageId, SequenceRangeV1, ServerMessageBodyV1, ServerMessageV1, SessionId,
    MAX_SAFE_JSON_INTEGER, OPUS_PACKET_FRAME_SAMPLES_V1, OPUS_PACKET_STREAM_HEADER_BYTES_V1,
    OPUS_PACKET_STREAM_MAX_BLOCK_BYTES_V1,
};
use margins_meeting_runtime::{
    MeetingRuntimeStorage, SessionDeltaV1, StorageCommit, StoredCommandReceiptV1, StoredSessionV1,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

static TEMPORARY_BLOB_ID: AtomicU64 = AtomicU64::new(0);
const PENDING_CAPTURE_CONTEXT: &str = "<!-- margins:transcript-pending-v1 -->\n# Capture saved\n\nTranscript pending. Audio is saved in this session's artifacts.\n";

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
    read_only: bool,
    fail_before_metadata_commit: Arc<AtomicBool>,
    adopt_legacy_once: Arc<AtomicBool>,
}

impl SqliteMeetingRuntimeStorage {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let storage = Self {
            directory: directory.into(),
            read_only: false,
            fail_before_metadata_commit: Arc::new(AtomicBool::new(false)),
            adopt_legacy_once: Arc::new(AtomicBool::new(false)),
        };
        std::fs::create_dir_all(storage.blob_dir())?;
        storage.connection()?;
        Ok(storage)
    }

    /// Open existing runtime tables for inventory without creating a directory,
    /// blob store, schema, or a writer connection.
    pub fn open_read_only(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            read_only: true,
            fail_before_metadata_commit: Arc::new(AtomicBool::new(false)),
            adopt_legacy_once: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn next_native_ordinal(&self, session_id: &str) -> Result<i64> {
        let connection = self.connection()?;
        let mut ordinal: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM (SELECT segment_index AS ordinal FROM session_segments WHERE session_name = ?1 UNION ALL SELECT canonical_ordinal AS ordinal FROM meeting_segment_projection WHERE session_id = ?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(anyhow::Error::from)?;
        while self
            .directory
            .join(format!("{session_id}_seg{ordinal}.wav"))
            .exists()
        {
            ordinal = ordinal
                .checked_add(1)
                .context("native segment ordinal overflow")?;
        }
        Ok(ordinal)
    }

    pub fn pending_native_segment(&self, session_id: &str) -> Result<Option<(i64, u64)>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT segment_index, offset_ms FROM session_segments WHERE session_name = ?1 AND duration_secs IS NULL ORDER BY segment_index DESC LIMIT 1",
                [session_id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)?.max(0) as u64)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn native_chunk_boundaries(
        &self,
        session_id: &str,
        ordinal: i64,
    ) -> Result<[(u64, u64); 2]> {
        let connection = self.connection()?;
        let segment_id = format!("{session_id}-seg-{ordinal}");
        let mut results = [(0u64, 0u64); 2];
        for (index, lane) in ["mic", "system"].iter().enumerate() {
            let mut query = connection.prepare(
                "SELECT sequence, metadata_json FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 ORDER BY sequence",
            )?;
            let mut rows = query.query(params![session_id, segment_id, lane])?;
            while let Some(row) = rows.next()? {
                let sequence: i64 = row.get(0)?;
                if sequence != results[index].0 as i64 {
                    anyhow::bail!("native runtime lane has a missing chunk");
                }
                let metadata: AudioChunkV1 = serde_json::from_str(&row.get::<_, String>(1)?)?;
                results[index].0 += 1;
                results[index].1 = results[index]
                    .1
                    .max(metadata.starts_at_ms.0 + metadata.duration_ms.0);
            }
        }
        Ok(results)
    }

    /// Export a scriptable stereo WAV from the canonical runtime chunks.
    /// Existing WAVs, including pre-runtime user recordings, are never changed.
    pub fn export_native_wav(&self, session_id: &str, ordinal: i64) -> Result<PathBuf> {
        if session_id.contains('/') || session_id.contains('\\') || ordinal < 0 {
            anyhow::bail!("invalid native audio export identity");
        }
        let path = self
            .directory
            .join(format!("{session_id}_seg{ordinal}.wav"));
        if path.is_file() {
            return Ok(path);
        }
        let connection = self.connection()?;
        let segment_id: String = connection.query_row(
            "SELECT p.segment_id FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal WHERE p.session_id = ?1 AND p.canonical_ordinal = ?2 AND s.duration_secs IS NOT NULL",
            params![session_id, ordinal],
            |row| row.get(0),
        )?;
        let lane_paths = |lane: &str| -> Result<Vec<PathBuf>> {
            let mut query = connection.prepare(
                "SELECT blob_path FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 ORDER BY sequence",
            )?;
            let rows = query.query_map(params![session_id, segment_id, lane], |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|row| Ok(self.blob_dir().join(row?))).collect()
        };
        let mut mic = PcmChunkSamples::new(lane_paths("mic")?);
        let mut system = PcmChunkSamples::new(lane_paths("system")?);
        let temporary = self.directory.join(format!(
            ".{session_id}_seg{ordinal}.{}.{}.wav.tmp",
            std::process::id(),
            TEMPORARY_BLOB_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut writer = hound::WavWriter::create(
            &temporary,
            hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        loop {
            let left = mic.next_sample()?;
            let right = system.next_sample()?;
            if left.is_none() && right.is_none() {
                break;
            }
            writer.write_sample(left.unwrap_or(0))?;
            writer.write_sample(right.unwrap_or(0))?;
        }
        writer.finalize()?;
        File::open(&temporary)?.sync_all()?;
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        std::fs::remove_file(&temporary)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(path)
    }

    /// Derive just one lane into an anonymous temporary WAV. Closing the file
    /// removes it; HTTP can keep it open until its response finishes streaming.
    pub fn temporary_native_lane_wav(
        &self,
        session_id: &str,
        ordinal: i64,
        lane: &str,
    ) -> Result<File> {
        if session_id.is_empty()
            || session_id == "."
            || session_id == ".."
            || session_id.contains('/')
            || session_id.contains('\\')
            || ordinal < 0
            || !matches!(lane, "mic" | "system")
        {
            anyhow::bail!("invalid native audio export identity");
        }
        let connection = self.connection()?;
        let segment_id: String = connection.query_row(
            "SELECT p.segment_id FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal WHERE p.session_id = ?1 AND p.canonical_ordinal = ?2 AND s.duration_secs IS NOT NULL",
            params![session_id, ordinal],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare(
            "SELECT blob_path FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 ORDER BY sequence",
        )?;
        let paths = statement
            .query_map(params![session_id, segment_id, lane], |row| {
                row.get::<_, String>(0)
            })?
            .map(|row| Ok(self.blob_dir().join(row?)))
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(!paths.is_empty(), "native audio lane has no chunks");
        let mut samples = PcmChunkSamples::new(paths);
        let mut file = tempfile::tempfile()?;
        let mut writer = hound::WavWriter::new(
            &mut file,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        while let Some(sample) = samples.next_sample()? {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    /// Size of the WAV that can be derived from a finalized native segment,
    /// without writing the derived file merely to list its audio artifacts.
    pub fn native_wav_export_size(&self, session_id: &str, ordinal: i64) -> Result<Option<u64>> {
        if session_id.is_empty()
            || session_id == "."
            || session_id == ".."
            || session_id.contains('/')
            || session_id.contains('\\')
            || ordinal < 0
        {
            anyhow::bail!("invalid native audio export identity");
        }
        let connection = self.connection()?;
        let segment_id: Option<String> = connection
            .query_row(
                "SELECT p.segment_id FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal WHERE p.session_id = ?1 AND p.canonical_ordinal = ?2 AND s.duration_secs IS NOT NULL",
                params![session_id, ordinal],
                |row| row.get(0),
            )
            .optional()?;
        let Some(segment_id) = segment_id else {
            return Ok(None);
        };
        let mut statement = connection.prepare(
            "SELECT lane_id, blob_path FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 ORDER BY lane_id, sequence",
        )?;
        let mut rows = statement.query(params![session_id, segment_id])?;
        let mut mic_bytes = 0u64;
        let mut system_bytes = 0u64;
        while let Some(row) = rows.next()? {
            let lane: String = row.get(0)?;
            let blob: String = row.get(1)?;
            let size = std::fs::metadata(self.blob_dir().join(blob))?.len();
            let total = match lane.as_str() {
                "mic" => &mut mic_bytes,
                "system" => &mut system_bytes,
                _ => continue,
            };
            *total = total
                .checked_add(size)
                .context("native audio size overflow")?;
        }
        if mic_bytes == 0 && system_bytes == 0 {
            return Ok(None);
        }
        if mic_bytes % 2 != 0 || system_bytes % 2 != 0 {
            anyhow::bail!("native runtime PCM has a partial s16 sample");
        }
        let frames = mic_bytes.max(system_bytes) / 2;
        let data_bytes = frames.checked_mul(4).context("native WAV size overflow")?;
        if data_bytes > u32::MAX as u64 {
            return Ok(None);
        }
        Ok(Some(
            data_bytes
                .checked_add(44)
                .context("native WAV size overflow")?,
        ))
    }

    /// Size of one derived mono lane, without materializing its WAV.
    pub fn native_lane_wav_size(
        &self,
        session_id: &str,
        ordinal: i64,
        lane: &str,
    ) -> Result<Option<u64>> {
        anyhow::ensure!(
            matches!(lane, "mic" | "system"),
            "invalid native audio lane"
        );
        if session_id.is_empty()
            || session_id == "."
            || session_id == ".."
            || session_id.contains('/')
            || session_id.contains('\\')
            || ordinal < 0
        {
            anyhow::bail!("invalid native audio export identity");
        }
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT c.blob_path FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal JOIN meeting_chunks c ON c.session_id = p.session_id AND c.segment_id = p.segment_id WHERE p.session_id = ?1 AND p.canonical_ordinal = ?2 AND s.duration_secs IS NOT NULL AND c.lane_id = ?3 ORDER BY c.sequence",
        )?;
        let mut bytes = 0u64;
        for row in statement.query_map(params![session_id, ordinal, lane], |row| {
            row.get::<_, String>(0)
        })? {
            bytes = bytes
                .checked_add(std::fs::metadata(self.blob_dir().join(row?))?.len())
                .context("native audio size overflow")?;
        }
        if bytes == 0 {
            return Ok(None);
        }
        anyhow::ensure!(
            bytes % 2 == 0,
            "native runtime PCM has a partial s16 sample"
        );
        if bytes > u32::MAX as u64 {
            return Ok(None);
        }
        Ok(Some(
            bytes.checked_add(44).context("native WAV size overflow")?,
        ))
    }

    /// Read-time inventory for TUI sessions finalized before audio artifact
    /// registry rows were added. It does not export or duplicate any audio.
    pub fn native_runtime_artifact_lanes(
        &self,
        session_id: &str,
    ) -> Result<Vec<(i64, String, String)>> {
        if session_id.is_empty()
            || session_id == "."
            || session_id == ".."
            || session_id.contains('/')
            || session_id.contains('\\')
        {
            anyhow::bail!("invalid native session id");
        }
        let connection = self.connection()?;
        let has_projection: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meeting_segment_projection')",
            [],
            |row| row.get(0),
        )?;
        if !has_projection {
            return Ok(Vec::new());
        }
        let mut statement = connection.prepare(
            "SELECT DISTINCT p.canonical_ordinal, p.segment_id, s.wav_path, s.started_at, c.lane_id FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal JOIN meeting_chunks c ON c.session_id = p.session_id AND c.segment_id = p.segment_id WHERE p.session_id = ?1 AND s.duration_secs IS NOT NULL ORDER BY p.canonical_ordinal, c.lane_id",
        )?;
        let mut rows = statement.query([session_id])?;
        let mut lanes = Vec::new();
        while let Some(row) = rows.next()? {
            let ordinal: i64 = row.get(0)?;
            let segment: String = row.get(1)?;
            let wav_path: String = row.get(2)?;
            let started_at: String = row.get(3)?;
            let lane: String = row.get(4)?;
            if ordinal >= 0
                && segment == format!("{session_id}-seg-{ordinal}")
                && wav_path == format!(".margins/{session_id}_seg{ordinal}.wav")
                && matches!(lane.as_str(), "mic" | "system")
            {
                lanes.push((ordinal, lane, started_at));
            }
        }
        Ok(lanes)
    }

    /// Project an open native segment immediately so session readers can see
    /// a recording before its first audio chunk has finalized.
    pub fn open_native_segment(
        &self,
        session_id: &str,
        ordinal: i64,
        offset_ms: u64,
    ) -> Result<()> {
        if ordinal < 0 || session_id.contains('/') || session_id.contains('\\') {
            anyhow::bail!("invalid native segment identity");
        }
        let segment_id = format!("{session_id}-seg-{ordinal}");
        let uri = format!(".margins/{session_id}_seg{ordinal}.wav");
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO meeting_segment_projection (session_id, segment_id, canonical_ordinal) VALUES (?1, ?2, ?3) ON CONFLICT(session_id, segment_id) DO NOTHING",
            params![session_id, segment_id, ordinal],
        )?;
        tx.execute(
            "INSERT INTO session_segments (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5) ON CONFLICT(session_name, segment_index) DO NOTHING",
            params![session_id, ordinal, uri, offset_ms as i64, chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn discard_empty_native_segment(&self, session_id: &str, ordinal: i64) -> Result<()> {
        let segment_id = format!("{session_id}-seg-{ordinal}");
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let chunks: i64 = tx.query_row(
            "SELECT COUNT(*) FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2",
            params![session_id, segment_id],
            |row| row.get(0),
        )?;
        if chunks != 0 {
            anyhow::bail!("cannot discard a native segment with durable audio");
        }
        tx.execute(
            "DELETE FROM session_segments WHERE session_name = ?1 AND segment_index = ?2 AND duration_secs IS NULL",
            params![session_id, ordinal],
        )?;
        tx.execute(
            "DELETE FROM meeting_segment_projection WHERE session_id = ?1 AND segment_id = ?2",
            params![session_id, segment_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The native TUI may attach a pre-runtime session, including one that
    /// crashed before its first finalized segment. This only
    /// authorizes one reservation against that existing canonical row; it does
    /// not rewrite earlier segments or backfill historical runtime events.
    pub fn allow_legacy_adoption_once(&self) {
        self.adopt_legacy_once.store(true, Ordering::Release);
    }

    /// Keep the native TUI's familiar stereo WAV as the legacy processing
    /// input after the runtime has durably finalized both PCM lanes. The
    /// runtime projection remains the authority for segment identity and
    /// duration; this only supplies its compatible WAV view.
    pub fn link_native_wav(&self, session_id: &str, segment_id: &str, ordinal: i64) -> Result<()> {
        let filename = format!("{session_id}_seg{ordinal}.wav");
        if session_id.contains('/')
            || session_id.contains('\\')
            || session_id == "."
            || session_id == ".."
        {
            anyhow::bail!("invalid native session id");
        }
        if !self.directory.join(&filename).is_file() {
            anyhow::bail!("native WAV is not present for finalized segment");
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let projected: Option<i64> = tx.query_row(
            "SELECT canonical_ordinal FROM meeting_segment_projection WHERE session_id = ?1 AND segment_id = ?2",
            params![session_id, segment_id], |row| row.get(0),
        ).optional()?;
        if projected != Some(ordinal) {
            anyhow::bail!("native WAV does not match a finalized runtime segment");
        }
        let uri = format!(".margins/{filename}");
        tx.execute(
            "UPDATE session_segments SET wav_path = ?1 WHERE session_name = ?2 AND segment_index = ?3",
            params![uri, session_id, ordinal],
        )?;
        tx.execute(
            "INSERT INTO session_artifacts (session_name, kind, ordinal, path, retention_class, created_at, expires_at) VALUES (?1, 'audio', ?2, ?3, 'durable', ?4, NULL) ON CONFLICT(session_name, kind, ordinal) DO UPDATE SET path = excluded.path, retention_class = excluded.retention_class",
            params![session_id, ordinal, uri, chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Complete the native WAV compatibility link if a process stopped after
    /// runtime finalization but before the link transaction committed.
    pub fn reconcile_native_wavs(&self, session_id: &str) -> Result<()> {
        if session_id.contains('/')
            || session_id.contains('\\')
            || session_id == "."
            || session_id == ".."
        {
            anyhow::bail!("invalid native session id");
        }
        let connection = self.connection()?;
        let mut query = connection.prepare(
            "SELECT p.segment_id, p.canonical_ordinal, s.wav_path FROM meeting_segment_projection p JOIN session_segments s ON s.session_name = p.session_id AND s.segment_index = p.canonical_ordinal WHERE p.session_id = ?1",
        )?;
        let rows = query.query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let pending = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        drop(connection);
        for (segment_id, ordinal, path) in pending {
            let native_path = format!(".margins/{session_id}_seg{ordinal}.wav");
            if path != native_path
                && self
                    .directory
                    .join(format!("{session_id}_seg{ordinal}.wav"))
                    .is_file()
            {
                self.link_native_wav(session_id, &segment_id, ordinal)?;
            }
        }
        Ok(())
    }

    pub fn register_native_transcript(&self, session_id: &str, ordinal: i64) -> Result<()> {
        if session_id.contains('/')
            || session_id.contains('\\')
            || session_id == "."
            || session_id == ".."
        {
            anyhow::bail!("invalid native session id");
        }
        let filename = format!("{session_id}_seg{ordinal}.live-transcript.json");
        if !self.directory.join(&filename).is_file() {
            anyhow::bail!("native transcript checkpoint is not present");
        }
        canonical::upsert_session_artifact(
            &self.directory,
            session_id,
            "transcript",
            ordinal,
            &format!(".margins/{filename}"),
            "durable",
            None,
        )
    }
    pub fn database_path(&self) -> PathBuf {
        canonical::database_path(&self.directory)
    }

    /// Last durable audio endpoint before a lane sequence. This reads only
    /// committed metadata, so a missing-upload marker never uses a later
    /// out-of-order chunk as its time anchor.
    pub fn latest_chunk_end_before(
        &self,
        session_id: &SessionId,
        segment_id: &str,
        lane_id: &LaneId,
        sequence: u64,
    ) -> Result<Option<u64>> {
        let connection = self.connection()?;
        let metadata: Option<String> = connection
            .query_row(
                "SELECT metadata_json FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 AND sequence < ?4 ORDER BY sequence DESC LIMIT 1",
                params![session_id.as_ref(), segment_id, lane_id.as_ref(), i64::try_from(sequence)?],
                |row| row.get(0),
            )
            .optional()?;
        metadata
            .map(|value| {
                let chunk: AudioChunkV1 = serde_json::from_str(&value)?;
                chunk
                    .starts_at_ms
                    .0
                    .checked_add(chunk.duration_ms.0)
                    .context("durable browser chunk time overflows")
            })
            .transpose()
    }

    pub fn audio_chunk_metadata(
        &self,
        session_id: &SessionId,
        segment_id: &str,
        lane_id: &LaneId,
        sequence: u64,
    ) -> Result<Option<AudioChunkV1>> {
        let connection = self.connection()?;
        let metadata: Option<String> = connection
            .query_row(
                "SELECT metadata_json FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 AND sequence = ?4",
                params![session_id.as_ref(), segment_id, lane_id.as_ref(), i64::try_from(sequence)?],
                |row| row.get(0),
            )
            .optional()?;
        metadata
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
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
        if self.read_only {
            let connection = Connection::open_with_flags(
                canonical::database_path(&self.directory),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            connection.busy_timeout(Duration::from_secs(5))?;
            return Ok(connection);
        }
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
        if delta
            .session
            .create()
            .provenance
            .hops
            .iter()
            .any(|hop| hop.producer == "margins-tui")
        {
            let ordinal: i64 = finalized
                .segment_id
                .as_ref()
                .strip_prefix(&format!("{}-seg-", session_id.as_ref()))
                .context("native runtime segment id does not match its session")?
                .parse()
                .context("native runtime segment id has no ordinal")?;
            let connection = self.connection()?;
            let mut lanes = Vec::new();
            let mut first_start_ms: Option<u64> = None;
            for boundary in &finalized.lane_boundaries {
                if boundary.next_sequence == 0 {
                    continue;
                }
                let format = declared_lanes
                    .iter()
                    .find(|lane| lane.lane_id == boundary.lane_id)
                    .context("native finalized lane was not declared")?
                    .format
                    .clone();
                if format.codec != AudioCodecV1::PcmS16Le
                    || format.container != AudioContainerV1::Raw
                    || format.sample_rate_hz != 16_000
                    || format.channel_count != 1
                {
                    anyhow::bail!("native runtime lane is not 16 kHz mono PCM");
                }
                let mut query = connection.prepare(
                    "SELECT sequence, metadata_json, blob_path FROM meeting_chunks WHERE session_id = ?1 AND segment_id = ?2 AND lane_id = ?3 ORDER BY sequence",
                )?;
                let mut rows = query.query(params![
                    session_id.as_ref(),
                    finalized.segment_id.as_ref(),
                    boundary.lane_id.as_ref()
                ])?;
                let mut count = 0u64;
                let mut bytes = 0u64;
                while let Some(row) = rows.next()? {
                    let sequence: i64 = row.get(0)?;
                    if sequence != count as i64 {
                        anyhow::bail!("native runtime lane has a missing chunk");
                    }
                    let metadata: AudioChunkV1 = serde_json::from_str(&row.get::<_, String>(1)?)?;
                    let blob_path: String = row.get(2)?;
                    let length = std::fs::metadata(self.blob_dir().join(blob_path))?.len();
                    if length % 2 != 0 {
                        anyhow::bail!("native runtime PCM chunk has a partial s16 sample");
                    }
                    bytes += length;
                    first_start_ms = Some(first_start_ms.map_or(metadata.starts_at_ms.0, |old| {
                        old.min(metadata.starts_at_ms.0)
                    }));
                    count += 1;
                }
                if count != boundary.next_sequence {
                    anyhow::bail!("native runtime lane boundary exceeds durable chunks");
                }
                lanes.push((
                    boundary.lane_id.clone(),
                    format!(
                        "meeting-runtime://{}/{}/{}",
                        session_id.as_ref(),
                        finalized.segment_id.as_ref(),
                        boundary.lane_id.as_ref()
                    ),
                    bytes,
                    bytes / 2,
                    "runtime".to_owned(),
                    format,
                ));
            }
            if lanes.is_empty() {
                return Ok(None);
            }
            let start = first_start_ms.unwrap_or(finalized.duration_ms.0);
            return Ok(Some(FinalizedProjection {
                segment_id: finalized.segment_id.as_ref().to_owned(),
                offset_ms: start as i64,
                duration_secs: finalized.duration_ms.0.saturating_sub(start) as f64 / 1000.0,
                lanes,
                native_wav_path: Some(format!(".margins/{}_seg{ordinal}.wav", session_id.as_ref())),
            }));
        }
        let artifacts = self.directory.join("artifacts").join(session_id.as_ref());
        std::fs::create_dir_all(&artifacts)?;
        let mut lanes = Vec::new();
        let mut first_start_ms: Option<u64> = None;
        for boundary in &finalized.lane_boundaries {
            if boundary.next_sequence == 0 {
                continue;
            }
            let format = declared_lanes
                .iter()
                .find(|lane| lane.lane_id == boundary.lane_id)
                .context("finalized lane was not declared by the session")?
                .format
                .clone();
            let (extension, artifact_suffix) = match (format.codec, format.container) {
                (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw) => ("pcm", "pcm"),
                (AudioCodecV1::Opus, AudioContainerV1::PacketStream) => ("mopus", "opus"),
                (AudioCodecV1::Opus, AudioContainerV1::Webm) => ("webm", "webm"),
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
            let mut staged = tempfile::NamedTempFile::new_in(&artifacts)?;
            let mut byte_count = 0u64;
            for sequence in 0..boundary.next_sequence {
                let chunk = if delta.audio_chunk.as_ref().is_some_and(|chunk| {
                    chunk.segment_id == finalized.segment_id
                        && chunk.lane_id == boundary.lane_id
                        && chunk.sequence == sequence
                }) {
                    Some(delta.audio_chunk.as_ref().unwrap().clone())
                } else {
                    self.load_audio_chunk(
                        session_id,
                        finalized.segment_id.as_ref(),
                        &boundary.lane_id,
                        sequence,
                    )?
                };
                let Some(chunk) = chunk else {
                    // Incomplete finalization may explicitly declare a lost
                    // sequence. The runtime has already accepted the gap;
                    // ordinary missing storage is still a hard error.
                    if delta.session.discontinuities().any(|gap| {
                        gap.segment_id == finalized.segment_id
                            && gap.lane_id == boundary.lane_id
                            && gap.sequence_range.contains(sequence)
                    }) {
                        continue;
                    }
                    anyhow::bail!(
                        "finalized lane {} is missing sequence {sequence}",
                        boundary.lane_id.as_ref()
                    );
                };
                first_start_ms = Some(
                    first_start_ms
                        .map_or(chunk.starts_at_ms.0, |old| old.min(chunk.starts_at_ms.0)),
                );
                staged.write_all(&chunk.payload)?;
                byte_count = byte_count
                    .checked_add(chunk.payload.len() as u64)
                    .context("finalized lane byte count overflow")?;
            }
            if byte_count == 0 {
                continue;
            }
            staged.as_file_mut().sync_all()?;
            let frame_count = match (format.codec, format.container) {
                (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw) => {
                    if byte_count % 2 != 0 {
                        anyhow::bail!("finalized PCM lane has a partial s16 sample");
                    }
                    byte_count / 2
                }
                (AudioCodecV1::Opus, AudioContainerV1::PacketStream) => {
                    validate_opus_packet_file(staged.path())?
                }
                // MediaRecorder emits one WebM byte stream across its dataavailable
                // blobs. Preserve those bytes in order; parsing and decoding belong
                // to the media layer after the durable artifact is committed.
                (AudioCodecV1::Opus, AudioContainerV1::Webm) => 0,
                _ => anyhow::bail!("finalized remote audio uses an unsupported durable format"),
            };
            install_staged_projection(staged, &path)?;
            lanes.push((
                boundary.lane_id.clone(),
                format!(".margins/artifacts/{}/{file_name}", session_id.as_ref()),
                byte_count,
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
            offset_ms: first_start_ms.unwrap_or(finalized.duration_ms.0) as i64,
            duration_secs: first_start_ms
                .map(|start| finalized.duration_ms.0.saturating_sub(start) as f64 / 1000.0)
                .unwrap_or(0.0),
            lanes,
            native_wav_path: None,
        }))
    }
}

struct FinalizedProjection {
    segment_id: String,
    offset_ms: i64,
    duration_secs: f64,
    lanes: Vec<(LaneId, String, u64, u64, String, AudioFormatV1)>,
    native_wav_path: Option<String>,
}

struct PcmChunkSamples {
    paths: std::vec::IntoIter<PathBuf>,
    bytes: Vec<u8>,
    index: usize,
}

impl PcmChunkSamples {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: paths.into_iter(),
            bytes: Vec::new(),
            index: 0,
        }
    }

    fn next_sample(&mut self) -> Result<Option<i16>> {
        loop {
            if self.index + 1 < self.bytes.len() {
                let sample =
                    i16::from_le_bytes([self.bytes[self.index], self.bytes[self.index + 1]]);
                self.index += 2;
                return Ok(Some(sample));
            }
            let Some(path) = self.paths.next() else {
                return Ok(None);
            };
            self.bytes = std::fs::read(path)?;
            if self.bytes.len() % 2 != 0 {
                anyhow::bail!("native runtime PCM chunk has a partial s16 sample");
            }
            self.index = 0;
        }
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
            let legacy_session: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE name = ?1 AND lifecycle_state IN ('active', 'ended'))",
                [session_id.as_ref()], |row| row.get(0),
            )?;
            let can_adopt = existing_create_key.is_none()
                && legacy_session
                && self.adopt_legacy_once.swap(false, Ordering::AcqRel);
            if !can_adopt && existing_create_key.as_deref() != Some(create.idempotency_key.as_str())
            {
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
        let finalized = delta
            .events
            .iter()
            .any(|event| matches!(event.body, ServerMessageBodyV1::SessionFinalized(_)));
        let generation_started = delta
            .events
            .iter()
            .any(|event| matches!(event.body, ServerMessageBodyV1::CaptureGenerationStarted(_)));
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
            // Runtime chunk timestamps are session-relative. Summing prior
            // segment durations erases real pause time and shifts later ASR
            // words ahead of memo observations.
            let offset_ms = projection.offset_ms;
            let representative = projection
                .lanes
                .first()
                .context("finalized segment has no lanes")?;
            let canonical_audio_path = projection
                .native_wav_path
                .clone()
                .unwrap_or_else(|| representative.1.clone());
            let started_at_ms = delta
                .session
                .create()
                .started_at_unix_ms
                .0
                .checked_add(u64::try_from(offset_ms).context("negative segment start offset")?)
                .context("segment wall-clock timestamp overflow")?;
            let started_at =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(started_at_ms as i64)
                    .context("segment timestamp is outside the supported range")?
                    .to_rfc3339();
            tx.execute(
                "INSERT INTO session_segments (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(session_name, segment_index) DO UPDATE SET wav_path = excluded.wav_path, offset_ms = excluded.offset_ms, duration_secs = excluded.duration_secs",
                params![session_id.as_ref(), ordinal, canonical_audio_path, offset_ms, projection.duration_secs, started_at],
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
        let finalized_audio: i64 = if finalized {
            tx.query_row(
                "SELECT COUNT(*) FROM session_segments WHERE session_name = ?1 AND duration_secs IS NOT NULL",
                [session_id.as_ref()],
                |row| row.get(0),
            )?
        } else {
            0
        };
        if finalized && finalized_audio > 0 {
            tx.execute("UPDATE sessions SET lifecycle_state = 'ended', lifecycle_updated_at = ?1 WHERE name = ?2 AND lifecycle_state = 'active'",
                params![chrono::Utc::now().to_rfc3339(), session_id.as_ref()])?;
        }
        if generation_started {
            tx.execute("UPDATE sessions SET lifecycle_state = 'active', lifecycle_updated_at = ?1 WHERE name = ?2 AND lifecycle_state = 'ended'",
                params![chrono::Utc::now().to_rfc3339(), session_id.as_ref()])?;
            tx.execute("UPDATE session_repository_state SET lifecycle = 'active' WHERE session_name = ?1 AND lifecycle = 'processing'",
                [session_id.as_ref()])?;
        }
        tx.commit()?;
        let pending_path = self
            .directory
            .join(format!("{}_capture_context.md", session_id.as_ref()));
        if generation_started && is_owned_pending_context(&pending_path) {
            if let Err(error) = std::fs::remove_file(&pending_path) {
                log::warn!(
                    "could not remove pending capture context for {}: {error}",
                    session_id.as_ref()
                );
            }
        }
        let normal_finish = matches!(
            delta.session.finalized_reason(),
            Some(
                margins_meeting_protocol::SessionFinalizeReasonV1::Completed
                    | margins_meeting_protocol::SessionFinalizeReasonV1::SourceEnded
            )
        );
        let still_finalized = if finalized && finalized_audio > 0 && normal_finish {
            match self.load_session(&session_id) {
                Ok(Some(state)) => state.input_finalized(),
                Ok(None) => false,
                Err(error) => {
                    log::warn!(
                        "could not inspect finalized capture {} for pending context: {error}",
                        session_id.as_ref()
                    );
                    false
                }
            }
        } else {
            false
        };
        if finalized
            && finalized_audio > 0
            && normal_finish
            && still_finalized
            && !pending_path.exists()
        {
            if let Err(error) = atomic_replace(&pending_path, PENDING_CAPTURE_CONTEXT.as_bytes()) {
                log::warn!(
                    "could not project pending capture context for {}: {error}",
                    session_id.as_ref()
                );
            }
        }
        Ok(StorageCommit::Committed)
    }
}

fn is_owned_pending_context(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|body| body.starts_with("<!-- margins:transcript-pending-v1 -->"))
}

fn core_audio_format(format: &AudioFormatV1) -> Result<AudioFormat> {
    let supported = matches!(
        (format.codec, format.container),
        (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw)
            | (AudioCodecV1::Opus, AudioContainerV1::PacketStream)
            | (AudioCodecV1::Opus, AudioContainerV1::Webm)
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

/// Validate the packet-block stream with at most one bounded block in memory.
fn validate_opus_packet_file(path: &Path) -> Result<u64> {
    let mut file = File::open(path)?;
    let mut expected_source_start = 0u64;
    let mut packet_count = 0u64;
    let mut pre_skip_48k = 0u16;
    let mut block_count = 0u64;
    let mut saw_end = false;
    loop {
        let mut header = [0u8; OPUS_PACKET_STREAM_HEADER_BYTES_V1];
        if file.read(&mut header[..1])? == 0 {
            break;
        }
        file.read_exact(&mut header[1..])?;
        let block_bytes = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if block_bytes < OPUS_PACKET_STREAM_HEADER_BYTES_V1
            || block_bytes > OPUS_PACKET_STREAM_MAX_BLOCK_BYTES_V1
        {
            anyhow::bail!("truncated or invalid Opus packet block length");
        }
        let mut bytes = vec![0u8; block_bytes];
        bytes[..header.len()].copy_from_slice(&header);
        file.read_exact(&mut bytes[header.len()..])?;
        let blocks = decode_opus_packet_blocks_v1(&bytes).map_err(anyhow::Error::msg)?;
        let block = &blocks[0];
        if saw_end {
            anyhow::bail!("Opus packet stream has bytes after END");
        }
        if block_count == 0 {
            if !block.stream_start || block.pre_skip_48k == 0 {
                anyhow::bail!("Opus packet stream is missing its START/pre-skip");
            }
            if block.pre_skip_48k % 3 != 0 {
                anyhow::bail!("16 kHz Opus pre-skip is not integral in the 48 kHz clock");
            }
            pre_skip_48k = block.pre_skip_48k;
        } else if block.stream_start || block.pre_skip_48k != 0 {
            anyhow::bail!("Opus packet stream has a repeated START/pre-skip");
        }
        if block.source_start_frame != expected_source_start {
            anyhow::bail!("Opus packet stream source frames contain a gap or overlap");
        }
        expected_source_start = expected_source_start
            .checked_add(u64::from(block.source_frame_count))
            .context("Opus packet stream source frame count overflowed")?;
        if expected_source_start > MAX_SAFE_JSON_INTEGER {
            anyhow::bail!("Opus packet stream source frame count exceeds V1");
        }
        packet_count = packet_count
            .checked_add(block.packets.len() as u64)
            .context("Opus packet count overflowed")?;
        if !block.stream_end
            && u64::from(block.source_frame_count)
                != block.packets.len() as u64 * u64::from(block.frame_samples)
        {
            anyhow::bail!("non-terminal Opus block does not fully represent its packets");
        }
        saw_end = block.stream_end;
        block_count += 1;
    }
    if block_count == 0 || !saw_end {
        anyhow::bail!("Opus packet stream is missing END");
    }
    let decoded_capacity = packet_count
        .checked_mul(u64::from(OPUS_PACKET_FRAME_SAMPLES_V1))
        .context("Opus decoded frame capacity overflowed")?;
    if decoded_capacity < expected_source_start.saturating_add(u64::from(pre_skip_48k / 3)) {
        anyhow::bail!("Opus packet stream tail cannot cover source frames after pre-skip");
    }
    Ok(expected_source_start)
}

fn install_staged_projection(staged: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    let parent = path.parent().context("projection path has no parent")?;
    match std::fs::hard_link(staged.path(), path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut current = File::open(path)?;
            let mut proposed = File::open(staged.path())?;
            if current.metadata()?.len() != proposed.metadata()?.len() {
                anyhow::bail!("finalized lane projection conflicts with existing bytes");
            }
            let mut left = [0u8; 64 * 1024];
            let mut right = [0u8; 64 * 1024];
            loop {
                let count = current.read(&mut left)?;
                if count == 0 {
                    break;
                }
                proposed.read_exact(&mut right[..count])?;
                if left[..count] != right[..count] {
                    anyhow::bail!("finalized lane projection conflicts with existing bytes");
                }
            }
        }
        Err(error) => return Err(error.into()),
    }
    File::open(parent)?.sync_all()?;
    Ok(())
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
