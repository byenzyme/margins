use crate::legacy;
use chrono::{DateTime, SecondsFormat, Utc};
use margins_core::{
    ArtifactId, NewSegment, NewSession, SegmentRecord, SessionArtifact, SessionError,
    SessionErrorCode, SessionId, SessionLifecycle, SessionQuery, SessionRecord, SessionRepository,
    SessionSummary, UnixMillis,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};

/// SQLite implementation of the public session repository port.
#[derive(Debug, Clone)]
pub struct SqliteSessionRepository {
    directory: PathBuf,
}

impl SqliteSessionRepository {
    /// Opens the existing `sessions.sqlite` store and installs only additive,
    /// backwards-compatible metadata tables and triggers.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, SessionError> {
        let directory = directory.as_ref().to_path_buf();
        let connection = legacy::open_db(&directory).map_err(internal)?;
        init_repository_schema(&connection).map_err(internal)?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn connection(&self) -> Result<Connection, SessionError> {
        let connection = legacy::open_db(&self.directory).map_err(internal)?;
        init_repository_schema(&connection).map_err(internal)?;
        Ok(connection)
    }
}

impl SessionRepository for SqliteSessionRepository {
    fn create(&self, new: NewSession) -> Result<SessionRecord, SessionError> {
        if !valid_session_id(new.id.as_ref()) {
            return Err(invalid(
                SessionErrorCode::CorruptData,
                "session id is empty or contains a path separator",
            ));
        }
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        if tombstone_exists(&tx, new.id.as_ref()).map_err(internal)? {
            return Err(invalid(
                SessionErrorCode::AlreadyExists,
                format!("session '{}' has been tombstoned", new.id.as_ref()),
            ));
        }

        let started_at = millis_to_rfc3339(new.started_at_ms)?;
        let created_at = millis_to_rfc3339(new.created_at_ms)?;
        let notes_path = new
            .note_uri
            .clone()
            .unwrap_or_else(|| format!("{}.md", new.id.as_ref()));
        tx.execute(
            r#"
            INSERT INTO sessions
                (name, start_time, notes_path, created_at, title, vault_note_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                new.id.as_ref(),
                started_at,
                notes_path,
                created_at,
                new.title,
                new.note_uri
            ],
        )
        .map_err(|error| {
            if is_constraint(&error) {
                invalid(
                    SessionErrorCode::AlreadyExists,
                    format!("session '{}' already exists", new.id.as_ref()),
                )
            } else {
                internal(error)
            }
        })?;
        tx.execute(
            r#"
            UPDATE session_repository_state
            SET revision = 0, lifecycle = 'active', updated_at_ms = ?1
            WHERE session_name = ?2
            "#,
            params![
                u64_to_i64(new.created_at_ms.0, "created_at_ms")?,
                new.id.as_ref()
            ],
        )
        .map_err(internal)?;
        let record = load_record(&tx, &new.id)?
            .ok_or_else(|| invalid(SessionErrorCode::Internal, "created session disappeared"))?;
        tx.commit().map_err(internal)?;
        Ok(record)
    }

    fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, SessionError> {
        let connection = self.connection()?;
        load_record(&connection, id)
    }

    fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let connection = self.connection()?;
        if query.limit == Some(0) {
            return Ok(Vec::new());
        }
        let mut statement = connection
            .prepare(
                r#"
                SELECT s.name, r.revision, r.lifecycle, s.start_time, s.title,
                       COUNT(seg.segment_index)
                FROM sessions s
                JOIN session_repository_state r ON r.session_name = s.name
                LEFT JOIN session_segments seg ON seg.session_name = s.name
                GROUP BY s.name, r.revision, r.lifecycle, s.start_time, s.title
                ORDER BY s.start_time DESC, s.name ASC
                "#,
            )
            .map_err(internal)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(internal)?;

        let mut summaries = Vec::new();
        for row in rows {
            let (id, revision, lifecycle, started_at, title, segment_count) =
                row.map_err(internal)?;
            let lifecycle = parse_lifecycle(&lifecycle)?;
            let started_at_ms = rfc3339_to_millis(&started_at, "start_time")?;
            if query
                .lifecycle
                .is_some_and(|expected| expected != lifecycle)
                || query
                    .started_after_ms
                    .is_some_and(|bound| started_at_ms.0 <= bound.0)
                || query
                    .started_before_ms
                    .is_some_and(|bound| started_at_ms.0 >= bound.0)
            {
                continue;
            }
            summaries.push(SessionSummary {
                id: SessionId::from(id),
                revision: nonnegative_u64(revision, "revision")?,
                lifecycle,
                started_at_ms,
                title,
                segment_count: nonnegative_u64(segment_count, "segment_count")?,
            });
        }
        summaries.sort_by(|left, right| {
            right
                .started_at_ms
                .0
                .cmp(&left.started_at_ms.0)
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(limit) = query.limit {
            summaries.truncate(limit as usize);
        }
        Ok(summaries)
    }

    fn append_segment(
        &self,
        id: &SessionId,
        expected_revision: u64,
        segment: NewSegment,
    ) -> Result<SessionRecord, SessionError> {
        validate_segment(&segment)?;
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        check_revision(&tx, id, expected_revision)?;
        let lifecycle = current_lifecycle(&tx, id)?;
        if lifecycle == SessionLifecycle::Tombstoned {
            return Err(invalid(
                SessionErrorCode::InvalidTransition,
                "cannot append a segment to a tombstoned session",
            ));
        }
        let duplicate: Option<i64> = tx
            .query_row(
                r#"
                SELECT 1
                FROM session_segments seg
                LEFT JOIN session_segment_contracts detail
                  ON detail.session_name = seg.session_name
                 AND detail.segment_index = seg.segment_index
                WHERE seg.session_name = ?1
                  AND (seg.segment_index = ?2 OR detail.segment_id = ?3)
                LIMIT 1
                "#,
                params![
                    id.as_ref(),
                    u64_to_i64(segment.ordinal, "segment ordinal")?,
                    segment.id.as_ref()
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?;
        if duplicate.is_some() {
            return Err(invalid(
                SessionErrorCode::InvalidSegment,
                "segment id and ordinal must be unique within a session",
            ));
        }
        let last: Option<(i64, i64)> = tx
            .query_row(
                r#"
                SELECT segment_index, offset_ms
                FROM session_segments
                WHERE session_name = ?1
                ORDER BY segment_index DESC
                LIMIT 1
                "#,
                params![id.as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(internal)?;
        if let Some((last_ordinal, last_offset)) = last {
            if u64_to_i64(segment.ordinal, "segment ordinal")? <= last_ordinal
                || u64_to_i64(segment.start_offset_ms, "segment offset")? < last_offset
            {
                return Err(invalid(
                    SessionErrorCode::InvalidSegment,
                    "segment ordinals must increase and offsets must not decrease",
                ));
            }
        }

        tx.execute(
            r#"
            INSERT INTO session_segments
                (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                id.as_ref(),
                u64_to_i64(segment.ordinal, "segment ordinal")?,
                segment.audio.uri,
                u64_to_i64(segment.start_offset_ms, "segment offset")?,
                segment.duration_ms.0 as f64 / 1000.0,
                millis_to_rfc3339(segment.started_at_ms)?
            ],
        )
        .map_err(internal)?;
        let contract_json = serde_json::to_string(&segment).map_err(internal)?;
        tx.execute(
            r#"
            INSERT INTO session_segment_contracts
                (session_name, segment_index, segment_id, contract_json)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                id.as_ref(),
                u64_to_i64(segment.ordinal, "segment ordinal")?,
                segment.id.as_ref(),
                contract_json
            ],
        )
        .map_err(internal)?;
        assert_revision_advanced_once(&tx, id, expected_revision)?;
        let record = load_record(&tx, id)?.ok_or_else(not_found)?;
        tx.commit().map_err(internal)?;
        Ok(record)
    }

    fn transition(
        &self,
        id: &SessionId,
        expected_revision: u64,
        next: SessionLifecycle,
    ) -> Result<SessionRecord, SessionError> {
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        check_revision(&tx, id, expected_revision)?;
        let current = current_lifecycle(&tx, id)?;
        if !current.can_transition_to(next) {
            return Err(invalid(
                SessionErrorCode::InvalidTransition,
                format!("cannot transition from {current:?} to {next:?}"),
            ));
        }
        if current == next {
            return load_record(&tx, id)?.ok_or_else(not_found);
        }
        if next == SessionLifecycle::Tombstoned {
            mark_tombstoned(&tx, id)?;
            assert_revision_advanced_once(&tx, id, expected_revision)?;
            let record = load_record(&tx, id)?.ok_or_else(not_found)?;
            tx.commit().map_err(internal)?;
            return Ok(record);
        }
        tx.execute(
            r#"
            UPDATE session_repository_state
            SET revision = revision + 1, lifecycle = ?1, updated_at_ms = ?2
            WHERE session_name = ?3 AND revision = ?4
            "#,
            params![
                lifecycle_name(next)?,
                now_millis_i64(),
                id.as_ref(),
                u64_to_i64(expected_revision, "expected revision")?
            ],
        )
        .map_err(internal)?;
        let record = load_record(&tx, id)?.ok_or_else(not_found)?;
        tx.commit().map_err(internal)?;
        Ok(record)
    }

    fn upsert_artifact(
        &self,
        expected_revision: u64,
        artifact: SessionArtifact,
    ) -> Result<SessionRecord, SessionError> {
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        check_revision(&tx, &artifact.session_id, expected_revision)?;
        if current_lifecycle(&tx, &artifact.session_id)? == SessionLifecycle::Tombstoned {
            return Err(invalid(
                SessionErrorCode::InvalidTransition,
                "cannot upsert an artifact for a tombstoned session",
            ));
        }
        validate_artifact(&artifact)?;
        let ordinal = u64_to_i64(artifact.ordinal, "artifact ordinal")?;
        tx.execute(
            r#"
            INSERT INTO session_artifacts
                (session_name, kind, ordinal, path, retention_class, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(session_name, kind, ordinal) DO UPDATE SET
                path = excluded.path,
                retention_class = excluded.retention_class,
                created_at = excluded.created_at,
                expires_at = excluded.expires_at
            "#,
            params![
                artifact.session_id.as_ref(),
                artifact.kind,
                ordinal,
                artifact.uri,
                artifact.retention_class,
                millis_to_rfc3339(artifact.created_at_ms)?,
                artifact.expires_at_ms.map(millis_to_rfc3339).transpose()?
            ],
        )
        .map_err(internal)?;
        tx.execute(
            r#"
            INSERT INTO session_artifact_ids (session_name, kind, ordinal, artifact_id)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(session_name, kind, ordinal) DO UPDATE SET
                artifact_id = excluded.artifact_id
            "#,
            params![
                artifact.session_id.as_ref(),
                artifact.kind,
                ordinal,
                artifact.id.as_ref()
            ],
        )
        .map_err(internal)?;
        assert_revision_advanced_once(&tx, &artifact.session_id, expected_revision)?;
        let record = load_record(&tx, &artifact.session_id)?.ok_or_else(not_found)?;
        tx.commit().map_err(internal)?;
        Ok(record)
    }

    fn tombstone(&self, id: &SessionId, expected_revision: u64) -> Result<(), SessionError> {
        let mut connection = self.connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        check_revision(&tx, id, expected_revision)?;
        if current_lifecycle(&tx, id)? == SessionLifecycle::Tombstoned {
            tx.commit().map_err(internal)?;
            return Ok(());
        }
        mark_tombstoned(&tx, id)?;
        assert_revision_advanced_once(&tx, id, expected_revision)?;
        tx.commit().map_err(internal)?;
        Ok(())
    }
}

fn mark_tombstoned(tx: &Transaction<'_>, id: &SessionId) -> Result<(), SessionError> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    tx.execute(
        r#"
        UPDATE sessions
        SET lifecycle_state = 'deleting', lifecycle_updated_at = ?1
        WHERE name = ?2
        "#,
        params![now, id.as_ref()],
    )
    .map_err(internal)?;
    tx.execute(
        r#"
        INSERT INTO session_tombstones (name, state, deleted_at, updated_at)
        VALUES (?1, 'deleting', ?2, ?2)
        ON CONFLICT(name) DO UPDATE SET state = 'deleting', updated_at = excluded.updated_at
        "#,
        params![id.as_ref(), now],
    )
    .map_err(internal)?;
    tx.execute(
        "UPDATE session_repository_state SET lifecycle = 'tombstoned' WHERE session_name = ?1",
        params![id.as_ref()],
    )
    .map_err(internal)?;
    Ok(())
}

fn init_repository_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        r#"
        BEGIN IMMEDIATE;

        CREATE TABLE IF NOT EXISTS session_repository_state (
            session_name TEXT PRIMARY KEY NOT NULL,
            revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
            lifecycle TEXT NOT NULL DEFAULT 'active',
            updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_segment_contracts (
            session_name TEXT NOT NULL,
            segment_index INTEGER NOT NULL,
            segment_id TEXT NOT NULL,
            contract_json TEXT NOT NULL,
            PRIMARY KEY (session_name, segment_index),
            UNIQUE (session_name, segment_id),
            FOREIGN KEY (session_name, segment_index)
                REFERENCES session_segments(session_name, segment_index) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_artifact_ids (
            session_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            artifact_id TEXT NOT NULL,
            PRIMARY KEY (session_name, kind, ordinal),
            UNIQUE (session_name, artifact_id),
            FOREIGN KEY (session_name, kind, ordinal)
                REFERENCES session_artifacts(session_name, kind, ordinal) ON DELETE CASCADE
        );

        INSERT OR IGNORE INTO session_repository_state
            (session_name, revision, lifecycle, updated_at_ms)
        SELECT s.name, 0,
               CASE WHEN t.name IS NULL THEN 'active' ELSE 'tombstoned' END,
               MAX(0, CAST(strftime('%s', COALESCE(s.lifecycle_updated_at, s.created_at)) AS INTEGER) * 1000)
        FROM sessions s
        LEFT JOIN session_tombstones t ON t.name = s.name;

        CREATE TRIGGER IF NOT EXISTS margins_repository_session_insert
        AFTER INSERT ON sessions
        BEGIN
            INSERT OR IGNORE INTO session_repository_state
                (session_name, revision, lifecycle, updated_at_ms)
            VALUES (
                NEW.name, 0, 'active',
                MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            );
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_session_update
        AFTER UPDATE ON sessions
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                lifecycle = CASE
                    WHEN NEW.lifecycle_state = 'deleting' THEN 'tombstoned'
                    WHEN OLD.lifecycle_state = 'deleting' AND NEW.lifecycle_state = 'active' THEN 'active'
                    ELSE lifecycle
                END,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = NEW.name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_segment_insert
        AFTER INSERT ON session_segments
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = NEW.session_name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_segment_update
        AFTER UPDATE ON session_segments
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = NEW.session_name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_segment_delete
        AFTER DELETE ON session_segments
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = OLD.session_name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_artifact_insert
        AFTER INSERT ON session_artifacts
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = NEW.session_name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_artifact_update
        AFTER UPDATE ON session_artifacts
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = NEW.session_name;
        END;

        CREATE TRIGGER IF NOT EXISTS margins_repository_artifact_delete
        AFTER DELETE ON session_artifacts
        BEGIN
            UPDATE session_repository_state
            SET revision = revision + 1,
                updated_at_ms = MAX(0, CAST(strftime('%s', 'now') AS INTEGER) * 1000)
            WHERE session_name = OLD.session_name;
        END;

        COMMIT;
        "#,
    )
}

fn load_record(
    connection: &Connection,
    id: &SessionId,
) -> Result<Option<SessionRecord>, SessionError> {
    let base = connection
        .query_row(
            r#"
            SELECT s.start_time, s.created_at, s.title, s.vault_note_path, s.note_error,
                   r.revision, r.lifecycle, r.updated_at_ms
            FROM sessions s
            JOIN session_repository_state r ON r.session_name = s.name
            WHERE s.name = ?1
            "#,
            params![id.as_ref()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(internal)?;
    let Some((start, created, title, note_uri, note_error, revision, lifecycle, updated)) = base
    else {
        return Ok(None);
    };

    let mut segments = Vec::new();
    let mut statement = connection
        .prepare(
            r#"
            SELECT seg.segment_index, seg.wav_path, seg.offset_ms, seg.duration_secs,
                   seg.started_at, detail.segment_id, detail.contract_json
            FROM session_segments seg
            LEFT JOIN session_segment_contracts detail
              ON detail.session_name = seg.session_name
             AND detail.segment_index = seg.segment_index
            WHERE seg.session_name = ?1
            ORDER BY seg.segment_index ASC
            "#,
        )
        .map_err(internal)?;
    let rows = statement
        .query_map(params![id.as_ref()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(internal)?;
    for row in rows {
        let (ordinal, path, offset, duration, started_at, sidecar_id, contract) =
            row.map_err(internal)?;
        let (Some(sidecar_id), Some(contract)) = (sidecar_id, contract) else {
            return Err(invalid(
                SessionErrorCode::CorruptData,
                format!(
                    "legacy segment {ordinal} for '{}' lacks lossless margins-core metadata; use the legacy/index API",
                    id.as_ref()
                ),
            ));
        };
        let segment: NewSegment = serde_json::from_str(&contract).map_err(|error| {
            invalid(
                SessionErrorCode::CorruptData,
                format!("invalid segment contract for '{}': {error}", id.as_ref()),
            )
        })?;
        let contract_is_valid = validate_segment(&segment).is_ok();
        if !contract_is_valid
            || sidecar_id != segment.id.as_ref()
            || segment.ordinal != nonnegative_u64(ordinal, "segment ordinal")?
            || segment.start_offset_ms != nonnegative_u64(offset, "segment offset")?
            || segment.audio.uri != path
            || rfc3339_to_millis(&started_at, "segment started_at")? != segment.started_at_ms
            || duration.is_none_or(|seconds| {
                !seconds.is_finite()
                    || (seconds * 1000.0 - segment.duration_ms.0 as f64).abs() > 0.5
            })
        {
            return Err(invalid(
                SessionErrorCode::CorruptData,
                format!(
                    "legacy and contract segment metadata diverged for '{}': {ordinal}",
                    id.as_ref()
                ),
            ));
        }
        segments.push(SegmentRecord::from(segment));
    }
    drop(statement);

    let mut artifacts = Vec::new();
    let mut statement = connection
        .prepare(
            r#"
            SELECT a.kind, a.ordinal, a.path, a.retention_class, a.created_at, a.expires_at,
                   ids.artifact_id
            FROM session_artifacts a
            LEFT JOIN session_artifact_ids ids
              ON ids.session_name = a.session_name
             AND ids.kind = a.kind
             AND ids.ordinal = a.ordinal
            WHERE a.session_name = ?1
            ORDER BY a.kind ASC, a.ordinal ASC
            "#,
        )
        .map_err(internal)?;
    let rows = statement
        .query_map(params![id.as_ref()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(internal)?;
    for row in rows {
        let (kind, ordinal, uri, retention_class, created_at, expires_at, artifact_id) =
            row.map_err(internal)?;
        let ordinal = nonnegative_u64(ordinal, "artifact ordinal")?;
        artifacts.push(SessionArtifact {
            id: ArtifactId::from(
                artifact_id.unwrap_or_else(|| format!("legacy:{}:{kind}:{ordinal}", id.as_ref())),
            ),
            session_id: id.clone(),
            kind,
            ordinal,
            uri,
            retention_class,
            created_at_ms: rfc3339_to_millis(&created_at, "artifact created_at")?,
            expires_at_ms: expires_at
                .as_deref()
                .map(|value| rfc3339_to_millis(value, "artifact expires_at"))
                .transpose()?,
        });
    }

    Ok(Some(SessionRecord {
        id: id.clone(),
        revision: nonnegative_u64(revision, "revision")?,
        lifecycle: parse_lifecycle(&lifecycle)?,
        started_at_ms: rfc3339_to_millis(&start, "start_time")?,
        created_at_ms: rfc3339_to_millis(&created, "created_at")?,
        updated_at_ms: UnixMillis(nonnegative_u64(updated, "updated_at_ms")?),
        title,
        note_uri,
        note_error,
        segments,
        artifacts,
    }))
}

fn validate_segment(segment: &NewSegment) -> Result<(), SessionError> {
    if segment.id.as_ref().trim().is_empty()
        || segment.audio.id.as_ref().trim().is_empty()
        || segment.audio.segment_id != segment.id
        || segment.audio.duration_ms != segment.duration_ms
        || segment.audio.uri.trim().is_empty()
        || segment.audio.format.sample_rate_hz == 0
        || segment.audio.format.channel_count == 0
    {
        return Err(invalid(
            SessionErrorCode::InvalidSegment,
            "segment and audio descriptor must be complete and internally consistent",
        ));
    }
    Ok(())
}

fn validate_artifact(artifact: &SessionArtifact) -> Result<(), SessionError> {
    if artifact.id.as_ref().trim().is_empty()
        || artifact.kind.trim().is_empty()
        || artifact.uri.trim().is_empty()
        || artifact.retention_class.trim().is_empty()
        || artifact
            .expires_at_ms
            .is_some_and(|expires| expires.0 < artifact.created_at_ms.0)
    {
        return Err(invalid(
            SessionErrorCode::CorruptData,
            "artifact must be complete and cannot expire before it is created",
        ));
    }
    Ok(())
}

fn valid_session_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

fn check_revision(tx: &Transaction<'_>, id: &SessionId, expected: u64) -> Result<(), SessionError> {
    let actual: Option<i64> = tx
        .query_row(
            "SELECT revision FROM session_repository_state WHERE session_name = ?1",
            params![id.as_ref()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let Some(actual) = actual else {
        return Err(not_found());
    };
    let actual = nonnegative_u64(actual, "revision")?;
    if actual != expected {
        return Err(SessionError::conflict(format!(
            "session '{}' revision is {actual}, expected {expected}",
            id.as_ref()
        )));
    }
    Ok(())
}

fn assert_revision_advanced_once(
    tx: &Transaction<'_>,
    id: &SessionId,
    previous: u64,
) -> Result<(), SessionError> {
    check_revision(
        tx,
        id,
        previous
            .checked_add(1)
            .ok_or_else(|| invalid(SessionErrorCode::Internal, "revision overflow"))?,
    )
}

fn current_lifecycle(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<SessionLifecycle, SessionError> {
    let lifecycle: Option<String> = tx
        .query_row(
            "SELECT lifecycle FROM session_repository_state WHERE session_name = ?1",
            params![id.as_ref()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    parse_lifecycle(&lifecycle.ok_or_else(not_found)?)
}

fn tombstone_exists(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT 1 FROM session_tombstones WHERE name = ?1 LIMIT 1",
        params![id],
        |_| Ok(()),
    )
    .optional()
    .map(|value| value.is_some())
}

fn parse_lifecycle(value: &str) -> Result<SessionLifecycle, SessionError> {
    match value {
        "active" => Ok(SessionLifecycle::Active),
        "paused" => Ok(SessionLifecycle::Paused),
        "processing" => Ok(SessionLifecycle::Processing),
        "ready" => Ok(SessionLifecycle::Ready),
        "needs_attention" => Ok(SessionLifecycle::NeedsAttention),
        "tombstoned" => Ok(SessionLifecycle::Tombstoned),
        other => Err(invalid(
            SessionErrorCode::CorruptData,
            format!("unknown repository lifecycle '{other}'"),
        )),
    }
}

fn lifecycle_name(value: SessionLifecycle) -> Result<&'static str, SessionError> {
    match value {
        SessionLifecycle::Active => Ok("active"),
        SessionLifecycle::Paused => Ok("paused"),
        SessionLifecycle::Processing => Ok("processing"),
        SessionLifecycle::Ready => Ok("ready"),
        SessionLifecycle::NeedsAttention => Ok("needs_attention"),
        SessionLifecycle::Tombstoned => Ok("tombstoned"),
        _ => Err(invalid(
            SessionErrorCode::InvalidTransition,
            "unsupported session lifecycle",
        )),
    }
}

fn rfc3339_to_millis(value: &str, field: &str) -> Result<UnixMillis, SessionError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|error| {
        invalid(
            SessionErrorCode::CorruptData,
            format!("invalid {field}: {error}"),
        )
    })?;
    let millis = parsed.timestamp_millis();
    if millis < 0 {
        return Err(invalid(
            SessionErrorCode::CorruptData,
            format!("{field} predates the Unix epoch"),
        ));
    }
    Ok(UnixMillis(millis as u64))
}

fn millis_to_rfc3339(value: UnixMillis) -> Result<String, SessionError> {
    let millis = i64::try_from(value.0).map_err(|_| {
        invalid(
            SessionErrorCode::CorruptData,
            "timestamp exceeds SQLite/chrono range",
        )
    })?;
    DateTime::<Utc>::from_timestamp_millis(millis)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| {
            invalid(
                SessionErrorCode::CorruptData,
                "timestamp exceeds SQLite/chrono range",
            )
        })
}

fn now_millis_i64() -> i64 {
    Utc::now().timestamp_millis().max(0)
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64, SessionError> {
    u64::try_from(value).map_err(|_| {
        invalid(
            SessionErrorCode::CorruptData,
            format!("{field} must be non-negative"),
        )
    })
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, SessionError> {
    i64::try_from(value).map_err(|_| {
        invalid(
            SessionErrorCode::CorruptData,
            format!("{field} exceeds SQLite integer range"),
        )
    })
}

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn invalid(code: SessionErrorCode, message: impl Into<String>) -> SessionError {
    SessionError {
        code,
        message: message.into(),
        retryable: false,
    }
}

fn not_found() -> SessionError {
    invalid(SessionErrorCode::NotFound, "session not found")
}

fn internal(error: impl std::fmt::Display) -> SessionError {
    SessionError {
        code: SessionErrorCode::Internal,
        message: error.to_string(),
        retryable: false,
    }
}
