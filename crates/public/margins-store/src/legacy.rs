//! Behavior-preserving access to the original Margins `sessions.sqlite` schema.
//!
//! This module intentionally retains the path-based API used by the transitional
//! root crate and desktop. New code should prefer [`crate::SqliteSessionRepository`]
//! when its richer `margins-core` aggregate can be represented losslessly.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub name: String,
    pub start_time: String,
    pub notes_path: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub segments: Vec<SegmentMeta>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<CalendarEventMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_note_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_stage: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CalendarEventMeta {
    pub title: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub calendar_id: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SegmentMeta {
    pub segment_index: i64,
    pub wav_path: String,
    pub offset_ms: i64,
    pub duration_secs: Option<f64>,
    pub started_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionGrounding {
    pub memo_ids: Vec<String>,
    pub note_quote: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
}

pub struct SessionInfo {
    pub name: String,
    pub start_time: String,
    pub notes_path: String,
    pub segment_count: i64,
}

pub const SESSION_ARTIFACT_KIND_TRANSCRIPT: &str = "transcript";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionArtifact {
    pub session_name: String,
    pub kind: String,
    pub ordinal: i64,
    pub path: String,
    pub retention_class: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionArtifactPathUpdate {
    pub session_name: String,
    pub kind: String,
    pub ordinal: i64,
    pub old_path: String,
    pub new_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteAssociation {
    pub session_name: String,
    pub source_id: String,
    pub relative_path: String,
    pub observed_content_hash: Option<String>,
    pub revision: u64,
    #[serde(default)]
    pub bb_thread_ids: Vec<String>,
    #[serde(default)]
    pub distilled_memo_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProcessingJob {
    pub job_id: String,
    pub session_name: String,
    pub operation: String,
    pub input_revision: String,
    pub attempt: u64,
    pub status: String,
    pub progress: Option<f64>,
    pub result_ref: Option<String>,
    pub failure: Option<String>,
    pub failed_stage: Option<String>,
}

pub fn database_path(dir: &Path) -> PathBuf {
    dir.join("sessions.sqlite")
}

pub(crate) fn open_db(dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {:?}", dir))?;
    let path = database_path(dir);
    let mut conn = Connection::open(&path).with_context(|| format!("failed to open {:?}", path))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    init_schema(&conn)?;
    migrate_json_metadata_if_needed(&mut conn, dir)?;
    migrate_application_records_if_needed(&mut conn, dir)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            name TEXT PRIMARY KEY NOT NULL,
            start_time TEXT NOT NULL,
            notes_path TEXT NOT NULL,
            created_at TEXT NOT NULL,
            title TEXT,
            vault_note_path TEXT,
            note_error TEXT,
            note_error_at TEXT,
            processing_state TEXT NOT NULL DEFAULT 'none',
            failed_stage TEXT,
            lifecycle_state TEXT NOT NULL DEFAULT 'active',
            lifecycle_updated_at TEXT
        );

        CREATE TABLE IF NOT EXISTS session_segments (
            session_name TEXT NOT NULL,
            segment_index INTEGER NOT NULL,
            wav_path TEXT NOT NULL,
            offset_ms INTEGER NOT NULL,
            duration_secs REAL,
            started_at TEXT NOT NULL,
            PRIMARY KEY (session_name, segment_index),
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_people (
            session_name TEXT NOT NULL,
            position INTEGER NOT NULL,
            person TEXT NOT NULL,
            PRIMARY KEY (session_name, position),
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_calendar_events (
            session_name TEXT PRIMARY KEY NOT NULL,
            title TEXT NOT NULL,
            start TEXT,
            end TEXT,
            calendar_id TEXT,
            event_id TEXT,
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS vault_notes (
            id TEXT PRIMARY KEY NOT NULL,
            absolute_path TEXT UNIQUE NOT NULL,
            procured_by TEXT NOT NULL,
            source_session_name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS session_grounding (
            session_name TEXT NOT NULL,
            position INTEGER NOT NULL,
            memo_ids TEXT NOT NULL,
            note_quote TEXT NOT NULL,
            section_id TEXT,
            disposition TEXT,
            PRIMARY KEY (session_name, position),
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_artifacts (
            session_name TEXT NOT NULL,
            kind TEXT NOT NULL,
            ordinal INTEGER NOT NULL DEFAULT 0,
            path TEXT NOT NULL,
            retention_class TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT,
            PRIMARY KEY (session_name, kind, ordinal),
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS session_tombstones (
            name TEXT PRIMARY KEY NOT NULL,
            state TEXT NOT NULL,
            deleted_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS session_note_associations (
            session_name TEXT PRIMARY KEY NOT NULL,
            source_id TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            observed_content_hash TEXT,
            revision INTEGER NOT NULL,
            linked_at TEXT NOT NULL,
            bb_thread_ids TEXT NOT NULL DEFAULT '[]',
            distilled_memo_revision TEXT,
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE,
            UNIQUE (source_id, relative_path)
        );

        CREATE TABLE IF NOT EXISTS session_processing_jobs (
            job_id TEXT PRIMARY KEY NOT NULL,
            session_name TEXT NOT NULL,
            operation TEXT NOT NULL,
            input_revision TEXT NOT NULL,
            attempt INTEGER NOT NULL,
            status TEXT NOT NULL,
            progress REAL,
            result_ref TEXT,
            failure TEXT,
            failed_stage TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (session_name) REFERENCES sessions(name) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_session_processing_jobs_session
            ON session_processing_jobs(session_name, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_session_segments_session
            ON session_segments(session_name, segment_index);
        CREATE INDEX IF NOT EXISTS idx_session_people_session
            ON session_people(session_name, position);
        CREATE INDEX IF NOT EXISTS idx_sessions_vault_note_path
            ON sessions(vault_note_path);
        CREATE INDEX IF NOT EXISTS idx_vault_notes_path
            ON vault_notes(absolute_path);
        CREATE INDEX IF NOT EXISTS idx_vault_notes_source_session
            ON vault_notes(source_session_name);
        CREATE INDEX IF NOT EXISTS idx_session_grounding_session
            ON session_grounding(session_name, position);
        "#,
    )?;
    ensure_column(conn, "sessions", "note_error", "TEXT")?;
    ensure_column(conn, "sessions", "note_error_at", "TEXT")?;
    ensure_column(
        conn,
        "sessions",
        "processing_state",
        "TEXT NOT NULL DEFAULT 'none'",
    )?;
    ensure_column(conn, "sessions", "failed_stage", "TEXT")?;
    ensure_column(
        conn,
        "sessions",
        "lifecycle_state",
        "TEXT NOT NULL DEFAULT 'active'",
    )?;
    ensure_column(conn, "sessions", "lifecycle_updated_at", "TEXT")?;
    ensure_column(
        conn,
        "session_note_associations",
        "bb_thread_ids",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    ensure_column(
        conn,
        "session_note_associations",
        "distilled_memo_revision",
        "TEXT",
    )?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row?.eq_ignore_ascii_case(column) {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
        [],
    )?;
    Ok(())
}

fn migrate_json_metadata_if_needed(conn: &mut Connection, dir: &Path) -> Result<()> {
    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    if existing > 0 {
        return Ok(());
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };

    let mut json_paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.ends_with(".meta.json") {
            json_paths.push(path);
        }
    }
    json_paths.sort();
    if json_paths.is_empty() {
        return Ok(());
    }

    let tx = conn.transaction()?;
    let mut imported_paths = Vec::new();
    for path in json_paths {
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<SessionMeta>(&data) else {
            continue;
        };
        upsert_session_meta_tx(&tx, &meta)?;
        imported_paths.push(path);
    }
    tx.commit()?;

    // The app no longer reads JSON metadata after this one-time import. Keep a
    // migrated backup instead of leaving active-looking .meta.json files behind.
    for path in imported_paths {
        let backup = PathBuf::from(format!("{}.migrated", path.to_string_lossy()));
        let _ = std::fs::rename(&path, backup);
    }

    Ok(())
}

/// Additively projects pre-refactor overloaded session columns into their new
/// independent records. Original columns remain as a recoverable source copy.
fn migrate_application_records_if_needed(conn: &mut Connection, dir: &Path) -> Result<()> {
    let Some(workspace) = dir.parent() else {
        return Ok(());
    };
    let rows = {
        let mut statement = conn.prepare(
            "SELECT name, vault_note_path, note_error, processing_state, failed_stage, created_at
             FROM sessions
             WHERE typeof(name) = 'text'
               AND (vault_note_path IS NULL OR typeof(vault_note_path) = 'text')
               AND (note_error IS NULL OR typeof(note_error) = 'text')
               AND typeof(processing_state) = 'text'
               AND (failed_stage IS NULL OR typeof(failed_stage) = 'text')
               AND typeof(created_at) = 'text'",
        )?;
        let mapped = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let tx = conn.transaction()?;
    for (name, note_path, error, processing_state, failed_stage, created_at) in rows {
        if let Some(note_path) = note_path {
            let path = Path::new(&note_path);
            if let Ok(relative) = path.strip_prefix(workspace) {
                if let Some(relative) = relative.to_str() {
                    if validate_note_reference("workspace", relative).is_ok() {
                        tx.execute(
                            "INSERT OR IGNORE INTO session_note_associations (session_name, source_id, relative_path, observed_content_hash, revision, linked_at) VALUES (?1, 'workspace', ?2, NULL, 1, ?3)",
                            params![name, relative, created_at],
                        )?;
                    }
                }
            }
        }
        if processing_state != "none"
            || error
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            let status = if error.is_some() || processing_state == "failed" {
                "failed"
            } else if processing_state == "done" {
                "complete"
            } else {
                "running"
            };
            tx.execute(
                "INSERT OR IGNORE INTO session_processing_jobs (job_id, session_name, operation, input_revision, attempt, status, progress, result_ref, failure, failed_stage, updated_at) VALUES (?1, ?2, 'legacy_note', 'legacy-session-columns', 0, ?3, ?4, NULL, ?5, ?6, ?7)",
                params![format!("legacy:{name}"), name, status, (status == "complete").then_some(1.0), error, failed_stage, created_at],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn vault_note_id_for_path(path: &str) -> String {
    format!("note-{:016x}", stable_hash(path))
}

fn stable_hash(text: &str) -> u64 {
    // FNV-1a keeps note ids stable across processes and Rust releases.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn upsert_vault_note_tx(
    tx: &rusqlite::Transaction<'_>,
    path: &str,
    source_session_name: Option<&str>,
) -> Result<String> {
    let id = vault_note_id_for_path(path);
    let now = Local::now().to_rfc3339();
    tx.execute(
        r#"
        INSERT INTO vault_notes
            (id, absolute_path, procured_by, source_session_name, created_at, updated_at)
        VALUES (?1, ?2, 'margins', ?3, ?4, ?4)
        ON CONFLICT(absolute_path) DO UPDATE SET
            procured_by = 'margins',
            source_session_name = COALESCE(excluded.source_session_name, vault_notes.source_session_name),
            updated_at = excluded.updated_at
        "#,
        params![id, path, source_session_name, now],
    )?;
    Ok(id)
}

fn upsert_session_meta_tx(tx: &rusqlite::Transaction<'_>, meta: &SessionMeta) -> Result<()> {
    if session_tombstone_exists_tx(tx, &meta.name)? {
        anyhow::bail!("session '{}' has been deleted", meta.name);
    }
    tx.execute(
        r#"
        INSERT INTO sessions (
            name, start_time, notes_path, created_at, title, vault_note_path,
            note_error, processing_state, failed_stage
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, 'none'), ?9)
        ON CONFLICT(name) DO UPDATE SET
            start_time = excluded.start_time,
            notes_path = excluded.notes_path,
            created_at = excluded.created_at,
            title = excluded.title,
            vault_note_path = excluded.vault_note_path,
            note_error = excluded.note_error,
            processing_state = excluded.processing_state,
            failed_stage = excluded.failed_stage
        "#,
        params![
            meta.name,
            meta.start_time,
            meta.notes_path,
            meta.created_at,
            meta.title,
            meta.vault_note_path,
            meta.note_error,
            meta.processing_state,
            meta.failed_stage,
        ],
    )?;
    tx.execute(
        "DELETE FROM session_segments WHERE session_name = ?1",
        params![meta.name],
    )?;
    for seg in &meta.segments {
        tx.execute(
            r#"
            INSERT INTO session_segments
                (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                meta.name,
                seg.segment_index,
                seg.wav_path,
                seg.offset_ms,
                seg.duration_secs,
                seg.started_at
            ],
        )?;
    }
    replace_people_tx(tx, &meta.name, &meta.people)?;
    tx.execute(
        "DELETE FROM session_calendar_events WHERE session_name = ?1",
        params![meta.name],
    )?;
    if let Some(event) = &meta.calendar_event {
        upsert_calendar_event_tx(tx, &meta.name, event)?;
    }
    if let Some(path) = &meta.vault_note_path {
        upsert_vault_note_tx(tx, path, Some(&meta.name))?;
    }
    Ok(())
}

fn replace_people_tx(
    tx: &rusqlite::Transaction<'_>,
    session_name: &str,
    people: &[String],
) -> Result<()> {
    tx.execute(
        "DELETE FROM session_people WHERE session_name = ?1",
        params![session_name],
    )?;
    for (position, person) in people.iter().enumerate() {
        tx.execute(
            "INSERT INTO session_people (session_name, position, person) VALUES (?1, ?2, ?3)",
            params![session_name, position as i64, person],
        )?;
    }
    Ok(())
}

fn upsert_calendar_event_tx(
    tx: &rusqlite::Transaction<'_>,
    session_name: &str,
    event: &CalendarEventMeta,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO session_calendar_events
            (session_name, title, start, end, calendar_id, event_id)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(session_name) DO UPDATE SET
            title = excluded.title,
            start = excluded.start,
            end = excluded.end,
            calendar_id = excluded.calendar_id,
            event_id = excluded.event_id
        "#,
        params![
            session_name,
            event.title,
            event.start,
            event.end,
            event.calendar_id,
            event.event_id
        ],
    )?;
    Ok(())
}

pub fn create_session(
    dir: &Path,
    name: &str,
    start_time: &DateTime<Local>,
    notes_path: &str,
) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = Local::now().to_rfc3339();
    if session_tombstone_exists_tx(&tx, name)? {
        anyhow::bail!("session '{name}' has been deleted");
    }
    tx.execute(
        r#"
        INSERT INTO sessions (name, start_time, notes_path, created_at)
        VALUES (?1, ?2, ?3, ?4)
        "#,
        params![name, start_time.to_rfc3339(), notes_path, now],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn begin_delete_session(dir: &Path, name: &str) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    let now = Local::now().to_rfc3339();
    tx.execute(
        "UPDATE sessions SET lifecycle_state = 'deleting', lifecycle_updated_at = ?1 WHERE name = ?2",
        params![now, name],
    )?;
    upsert_session_tombstone_tx(&tx, name, "deleting", &now)?;
    tx.commit()?;
    Ok(())
}

pub fn finalize_delete_session(dir: &Path, name: &str) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    let now = Local::now().to_rfc3339();
    upsert_session_tombstone_tx(&tx, name, "deleted", &now)?;
    tx.execute("DELETE FROM sessions WHERE name = ?1", params![name])?;
    tx.commit()?;
    Ok(())
}

fn upsert_session_tombstone_tx(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    state: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO session_tombstones (name, state, deleted_at, updated_at)
        VALUES (?1, ?2, ?3, ?3)
        ON CONFLICT(name) DO UPDATE SET
            state = excluded.state,
            updated_at = excluded.updated_at
        "#,
        params![name, state, now],
    )?;
    Ok(())
}

pub fn clear_session_tombstone(dir: &Path, name: &str) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "DELETE FROM session_tombstones WHERE name = ?1",
        params![name],
    )?;
    tx.execute(
        "UPDATE sessions SET lifecycle_state = 'active', lifecycle_updated_at = ?1 WHERE name = ?2",
        params![Local::now().to_rfc3339(), name],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn is_session_tombstoned(dir: &Path, name: &str) -> Result<bool> {
    let conn = open_db(dir)?;
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM session_tombstones WHERE name = ?1 LIMIT 1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub fn list_session_tombstone_names(dir: &Path) -> Result<Vec<String>> {
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare("SELECT name FROM session_tombstones")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

pub fn session_exists(dir: &Path, name: &str) -> Result<bool> {
    let conn = open_db(dir)?;
    let found: Option<i64> = conn
        .query_row(
            r#"
            SELECT 1 FROM sessions WHERE name = ?1
            UNION
            SELECT 1 FROM session_tombstones WHERE name = ?1
            LIMIT 1
            "#,
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub fn get_session_start_time(dir: &Path, name: &str) -> Result<DateTime<Local>> {
    let meta = get_session_meta(dir, name)?;
    let dt = DateTime::parse_from_rfc3339(&meta.start_time)
        .context("invalid start_time in session database")?
        .with_timezone(&Local);
    Ok(dt)
}

pub fn next_segment_index(dir: &Path, session_name: &str) -> Result<i64> {
    let conn = open_db(dir)?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session_segments WHERE session_name = ?1",
        params![session_name],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn add_segment(
    dir: &Path,
    session_name: &str,
    segment_index: i64,
    wav_path: &str,
    offset_ms: i64,
    duration_secs: Option<f64>,
) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        r#"
        INSERT INTO session_segments
            (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            session_name,
            segment_index,
            wav_path,
            offset_ms,
            duration_secs,
            Local::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

pub fn update_segment_duration(
    dir: &Path,
    session_name: &str,
    segment_index: i64,
    duration_secs: f64,
) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        r#"
        UPDATE session_segments
        SET duration_secs = ?1
        WHERE session_name = ?2 AND segment_index = ?3
        "#,
        params![duration_secs, session_name, segment_index],
    )?;
    Ok(())
}

/// Mark a stopped capture as ended once its durable segment is finalized.
/// The legacy lifecycle also tracks deletion, so ended sessions remain listable.
pub fn mark_session_ended(dir: &Path, name: &str) -> Result<()> {
    // The legacy store may be opened without the additive repository tables.
    // Install them before updating the two lifecycle views together.
    crate::SqliteSessionRepository::open(dir).map_err(|error| anyhow::anyhow!(error))?;
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let finalized: i64 = tx.query_row(
        "SELECT COUNT(*) FROM session_segments WHERE session_name = ?1 AND duration_secs IS NOT NULL",
        params![name],
        |row| row.get(0),
    )?;
    if finalized == 0 {
        anyhow::bail!("session '{name}' has no finalized audio segment");
    }
    let changed = tx.execute(
        "UPDATE sessions SET lifecycle_state = 'ended', lifecycle_updated_at = ?1 WHERE name = ?2 AND lifecycle_state = 'active'",
        params![Local::now().to_rfc3339(), name],
    )?;
    if changed == 0 {
        let state: Option<String> = tx
            .query_row(
                "SELECT lifecycle_state FROM sessions WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        if state.as_deref() != Some("ended") {
            anyhow::bail!("session '{name}' cannot be marked ended from {state:?}");
        }
    } else {
        tx.execute(
            "UPDATE session_repository_state SET lifecycle = 'processing' WHERE session_name = ?1 AND lifecycle = 'active'",
            params![name],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn list_sessions(dir: &Path) -> Result<Vec<SessionInfo>> {
    // Listing must not materialize the vault: an absent DB means no sessions
    // yet, so return empty instead of letting open_db create_dir_all the vault.
    // (Clean first-run launch reads sessions for the default project before any
    // capture; creating the dir here would recreate ~/Documents/margins.)
    if !database_path(dir).exists() {
        return Ok(Vec::new());
    }
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT s.name, s.start_time, s.notes_path, COUNT(seg.segment_index) AS segment_count
        FROM sessions s
        LEFT JOIN session_segments seg ON seg.session_name = s.name
        WHERE COALESCE(s.lifecycle_state, 'active') IN ('active', 'ended')
        GROUP BY s.name, s.start_time, s.notes_path
        ORDER BY s.start_time DESC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionInfo {
            name: row.get(0)?,
            start_time: row.get(1)?,
            notes_path: row.get(2)?,
            segment_count: row.get(3)?,
        })
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        let session = row.context("corrupt session row")?;
        let started_at = DateTime::parse_from_rfc3339(&session.start_time)
            .with_context(|| format!("invalid start_time for '{}'", session.name))?;
        sessions.push((started_at, session));
    }
    sessions.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(sessions.into_iter().map(|(_, session)| session).collect())
}

pub fn get_session_meta(dir: &Path, name: &str) -> Result<SessionMeta> {
    let conn = open_db(dir)?;
    let mut meta = conn
        .query_row(
            r#"
            SELECT
                name, start_time, notes_path, created_at, title, vault_note_path,
                note_error, processing_state, failed_stage
            FROM sessions
            WHERE name = ?1
            "#,
            params![name],
            |row| {
                Ok(SessionMeta {
                    name: row.get(0)?,
                    start_time: row.get(1)?,
                    notes_path: row.get(2)?,
                    created_at: row.get(3)?,
                    title: row.get(4)?,
                    vault_note_path: row.get(5)?,
                    note_error: row.get(6)?,
                    processing_state: row.get(7)?,
                    failed_stage: row.get(8)?,
                    segments: Vec::new(),
                    people: Vec::new(),
                    calendar_event: None,
                })
            },
        )
        .with_context(|| format!("session '{name}' not found"))?;

    let mut segment_stmt = conn.prepare(
        r#"
        SELECT segment_index, wav_path, offset_ms, duration_secs, started_at
        FROM session_segments
        WHERE session_name = ?1
        ORDER BY segment_index ASC
        "#,
    )?;
    let segment_rows = segment_stmt.query_map(params![name], |row| {
        Ok(SegmentMeta {
            segment_index: row.get(0)?,
            wav_path: row.get(1)?,
            offset_ms: row.get(2)?,
            duration_secs: row.get(3)?,
            started_at: row.get(4)?,
        })
    })?;
    for row in segment_rows {
        meta.segments.push(row?);
    }

    let mut people_stmt = conn.prepare(
        r#"
        SELECT person
        FROM session_people
        WHERE session_name = ?1
        ORDER BY position ASC
        "#,
    )?;
    let people_rows = people_stmt.query_map(params![name], |row| row.get::<_, String>(0))?;
    for row in people_rows {
        meta.people.push(row?);
    }

    meta.calendar_event = conn
        .query_row(
            r#"
            SELECT title, start, end, calendar_id, event_id
            FROM session_calendar_events
            WHERE session_name = ?1
            "#,
            params![name],
            |row| {
                Ok(CalendarEventMeta {
                    title: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                    calendar_id: row.get(3)?,
                    event_id: row.get(4)?,
                })
            },
        )
        .optional()?;

    if let Some(association) = get_note_association(dir, name)? {
        if association.source_id == "workspace" {
            if let Some(root) = dir.parent() {
                meta.vault_note_path = Some(
                    root.join(association.relative_path)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }
    if let Some(job) = latest_processing_job(dir, name)? {
        meta.processing_state = Some(
            match job.status.as_str() {
                "queued" => "none",
                "running" if job.failed_stage.as_deref() == Some("transcribe") => "transcribing",
                "running" => "distilling",
                "complete" => "done",
                "failed" => "failed",
                "cancelled" => "none",
                other => other,
            }
            .to_string(),
        );
        meta.note_error = job.failure;
        meta.failed_stage = job.failed_stage;
    }

    Ok(meta)
}

pub fn upsert_session_artifact(
    dir: &Path,
    session_name: &str,
    kind: &str,
    ordinal: i64,
    path: &str,
    retention_class: &str,
    expires_at: Option<&str>,
) -> Result<()> {
    let conn = open_db(dir)?;
    let now = Local::now().to_rfc3339();
    conn.execute(
        r#"
        INSERT INTO session_artifacts
            (session_name, kind, ordinal, path, retention_class, created_at, expires_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(session_name, kind, ordinal) DO UPDATE SET
            path = excluded.path,
            retention_class = excluded.retention_class,
            expires_at = excluded.expires_at
        "#,
        params![
            session_name,
            kind,
            ordinal,
            path,
            retention_class,
            now,
            expires_at
        ],
    )?;
    Ok(())
}

/// Rewrite a set of registered artifact paths as one all-or-nothing change.
///
/// The old path is part of each predicate so a concurrent writer cannot be
/// silently overwritten. A missing or changed row rolls back the transaction.
pub fn rewrite_session_artifact_paths(
    dir: &Path,
    updates: &[SessionArtifactPathUpdate],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for update in updates {
        let changed = tx.execute(
            r#"
            UPDATE session_artifacts
            SET path = ?1
            WHERE session_name = ?2 AND kind = ?3 AND ordinal = ?4 AND path = ?5
            "#,
            params![
                update.new_path,
                update.session_name,
                update.kind,
                update.ordinal,
                update.old_path
            ],
        )?;
        if changed != 1 {
            anyhow::bail!(
                "artifact path changed while migrating {}:{}:{}",
                update.session_name,
                update.kind,
                update.ordinal
            );
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn list_session_artifacts(dir: &Path, session_name: &str) -> Result<Vec<SessionArtifact>> {
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT session_name, kind, ordinal, path, retention_class, created_at, expires_at
        FROM session_artifacts
        WHERE session_name = ?1
        ORDER BY kind ASC, ordinal ASC
        "#,
    )?;
    let rows = stmt.query_map(params![session_name], |row| {
        Ok(SessionArtifact {
            session_name: row.get(0)?,
            kind: row.get(1)?,
            ordinal: row.get(2)?,
            path: row.get(3)?,
            retention_class: row.get(4)?,
            created_at: row.get(5)?,
            expires_at: row.get(6)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row.context("corrupt session artifact row")?);
    }
    Ok(artifacts)
}

pub fn list_expired_session_artifacts(
    dir: &Path,
    before: DateTime<Local>,
) -> Result<Vec<SessionArtifact>> {
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT session_name, kind, ordinal, path, retention_class, created_at, expires_at
        FROM session_artifacts
        WHERE expires_at IS NOT NULL
          AND retention_class = 'temporary'
        ORDER BY session_name ASC, kind ASC, ordinal ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionArtifact {
            session_name: row.get(0)?,
            kind: row.get(1)?,
            ordinal: row.get(2)?,
            path: row.get(3)?,
            retention_class: row.get(4)?,
            created_at: row.get(5)?,
            expires_at: row.get(6)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        let artifact = row.context("corrupt expiring session artifact row")?;
        let Some(expires_at) = artifact.expires_at.as_deref() else {
            continue;
        };
        let expires_at = DateTime::parse_from_rfc3339(expires_at).with_context(|| {
            format!(
                "invalid expires_at for artifact '{}:{}:{}'",
                artifact.session_name, artifact.kind, artifact.ordinal
            )
        })?;
        if expires_at.with_timezone(&Local) < before {
            artifacts.push(artifact);
        }
    }
    Ok(artifacts)
}

pub fn delete_session_artifact_registry_row(
    dir: &Path,
    session_name: &str,
    kind: &str,
    ordinal: i64,
) -> Result<usize> {
    let conn = open_db(dir)?;
    let changed = conn.execute(
        "DELETE FROM session_artifacts WHERE session_name = ?1 AND kind = ?2 AND ordinal = ?3",
        params![session_name, kind, ordinal],
    )?;
    Ok(changed)
}

pub fn delete_session_artifacts_registry_rows(
    dir: &Path,
    session_name: &str,
    kind: Option<&str>,
) -> Result<usize> {
    let conn = open_db(dir)?;
    let changed = if let Some(kind) = kind {
        conn.execute(
            "DELETE FROM session_artifacts WHERE session_name = ?1 AND kind = ?2",
            params![session_name, kind],
        )?
    } else {
        conn.execute(
            "DELETE FROM session_artifacts WHERE session_name = ?1",
            params![session_name],
        )?
    };
    Ok(changed)
}

fn validate_note_reference(source_id: &str, relative_path: &str) -> Result<()> {
    if source_id.trim().is_empty() {
        anyhow::bail!("note source id cannot be empty");
    }
    let path = Path::new(relative_path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        anyhow::bail!("note path must be source-relative and cannot contain traversal");
    }
    Ok(())
}

pub fn get_note_association(dir: &Path, name: &str) -> Result<Option<NoteAssociation>> {
    let conn = open_db(dir)?;
    conn.query_row(
        "SELECT session_name, source_id, relative_path, observed_content_hash, revision, bb_thread_ids, distilled_memo_revision FROM session_note_associations WHERE session_name = ?1",
        params![name],
        |row| {
            let revision: i64 = row.get(4)?;
            Ok(NoteAssociation {
                session_name: row.get(0)?,
                source_id: row.get(1)?,
                relative_path: row.get(2)?,
                observed_content_hash: row.get(3)?,
                revision: revision.max(0) as u64,
                bb_thread_ids: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                distilled_memo_revision: row.get(6)?,
            })
        },
    ).optional().map_err(Into::into)
}

pub fn link_note(
    dir: &Path,
    name: &str,
    source_id: &str,
    relative_path: &str,
    observed_content_hash: Option<&str>,
    expected_revision: u64,
) -> Result<NoteAssociation> {
    link_note_with_distillation(
        dir,
        name,
        source_id,
        relative_path,
        observed_content_hash,
        expected_revision,
        None,
        None,
    )
}

pub fn link_note_with_distillation(
    dir: &Path,
    name: &str,
    source_id: &str,
    relative_path: &str,
    observed_content_hash: Option<&str>,
    expected_revision: u64,
    bb_thread_id: Option<&str>,
    distilled_memo_revision: Option<&str>,
) -> Result<NoteAssociation> {
    validate_note_reference(source_id, relative_path)?;
    if bb_thread_id.is_some() != distilled_memo_revision.is_some() {
        anyhow::bail!("bb thread id and distilled memo revision must be supplied together");
    }
    if bb_thread_id.is_some_and(|value| value.trim().is_empty())
        || distilled_memo_revision.is_some_and(|value| value.trim().is_empty())
    {
        anyhow::bail!("distillation link values cannot be empty");
    }
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if session_tombstone_exists_tx(&tx, name)? {
        anyhow::bail!("session '{name}' has been deleted");
    }
    let exists: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM sessions WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        anyhow::bail!("session '{name}' not found");
    }
    let current: Option<(String, String, Option<String>, i64, String, Option<String>)> = tx.query_row(
        "SELECT source_id, relative_path, observed_content_hash, revision, bb_thread_ids, distilled_memo_revision FROM session_note_associations WHERE session_name = ?1",
        params![name],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    ).optional()?;
    let mut thread_ids: Vec<String> = current
        .as_ref()
        .map(|value| serde_json::from_str(&value.4).unwrap_or_default())
        .unwrap_or_default();
    if let Some(thread_id) = bb_thread_id {
        if !thread_ids.iter().any(|value| value == thread_id) {
            thread_ids.push(thread_id.to_string());
        }
    }
    let memo_revision = distilled_memo_revision
        .map(str::to_owned)
        .or_else(|| current.as_ref().and_then(|value| value.5.clone()));
    if let Some((current_source, current_path, current_hash, revision, _, _)) = &current {
        if current_source == source_id
            && current_path == relative_path
            && current_hash.as_deref() == observed_content_hash
            && serde_json::from_str::<Vec<String>>(&current.as_ref().unwrap().4).unwrap_or_default()
                == thread_ids
            && current.as_ref().unwrap().5 == memo_revision
        {
            return Ok(NoteAssociation {
                session_name: name.to_string(),
                source_id: current_source.clone(),
                relative_path: current_path.clone(),
                observed_content_hash: current_hash.clone(),
                revision: (*revision).max(0) as u64,
                bb_thread_ids: thread_ids,
                distilled_memo_revision: memo_revision,
            });
        }
    }
    let current_revision = current.as_ref().map_or(0, |value| value.3.max(0) as u64);
    if current_revision != expected_revision {
        anyhow::bail!("note association revision conflict: expected {expected_revision}, found {current_revision}");
    }
    let revision = current_revision + 1;
    tx.execute(
        "INSERT INTO session_note_associations (session_name, source_id, relative_path, observed_content_hash, revision, linked_at, bb_thread_ids, distilled_memo_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(session_name) DO UPDATE SET source_id = excluded.source_id, relative_path = excluded.relative_path, observed_content_hash = excluded.observed_content_hash, revision = excluded.revision, linked_at = excluded.linked_at, bb_thread_ids = excluded.bb_thread_ids, distilled_memo_revision = excluded.distilled_memo_revision",
        params![name, source_id, relative_path, observed_content_hash, revision as i64, Local::now().to_rfc3339(), serde_json::to_string(&thread_ids)?, memo_revision],
    )?;
    tx.commit()?;
    Ok(NoteAssociation {
        session_name: name.to_string(),
        source_id: source_id.to_string(),
        relative_path: relative_path.to_string(),
        observed_content_hash: observed_content_hash.map(ToOwned::to_owned),
        revision,
        bb_thread_ids: thread_ids,
        distilled_memo_revision: memo_revision,
    })
}

pub fn unlink_note(dir: &Path, name: &str, expected_revision: u64) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: Option<i64> = tx
        .query_row(
            "SELECT revision FROM session_note_associations WHERE session_name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(());
    };
    if current.max(0) as u64 != expected_revision {
        anyhow::bail!(
            "note association revision conflict: expected {expected_revision}, found {}",
            current.max(0)
        );
    }
    tx.execute(
        "DELETE FROM session_note_associations WHERE session_name = ?1",
        params![name],
    )?;
    tx.commit()?;
    Ok(())
}

fn session_tombstone_exists_tx(tx: &rusqlite::Transaction<'_>, name: &str) -> Result<bool> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM session_tombstones WHERE name = ?1 LIMIT 1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub fn set_session_grounding(dir: &Path, name: &str, grounding: &[SessionGrounding]) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM session_grounding WHERE session_name = ?1",
        params![name],
    )?;
    for (position, item) in grounding.iter().enumerate() {
        let memo_ids = item.memo_ids.join(",");
        let quote = item.note_quote.trim();
        if item.memo_ids.is_empty() || quote.is_empty() {
            continue;
        }
        tx.execute(
            r#"
            INSERT INTO session_grounding
                (session_name, position, memo_ids, note_quote, section_id, disposition)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                name,
                position as i64,
                memo_ids,
                quote,
                item.section_id.as_deref(),
                item.disposition.as_deref()
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn get_session_grounding(dir: &Path, name: &str) -> Result<Vec<SessionGrounding>> {
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT memo_ids, note_quote, section_id, disposition
        FROM session_grounding
        WHERE session_name = ?1
        ORDER BY position ASC
        "#,
    )?;
    let rows = stmt.query_map(params![name], |row| {
        let memo_ids_raw: String = row.get(0)?;
        Ok(SessionGrounding {
            memo_ids: memo_ids_raw
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string)
                .collect(),
            note_quote: row.get(1)?,
            section_id: row.get(2)?,
            disposition: row.get(3)?,
        })
    })?;

    let mut grounding = Vec::new();
    for row in rows {
        grounding.push(row?);
    }
    Ok(grounding)
}

pub fn move_vault_note_path_by_id(
    dir: &Path,
    id: &str,
    old_path: &str,
    new_path: &str,
) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        "UPDATE vault_notes SET absolute_path = ?1, updated_at = ?2 WHERE id = ?3 AND absolute_path = ?4 AND procured_by = 'margins'",
        params![new_path, Local::now().to_rfc3339(), id, old_path],
    )?;
    Ok(())
}

fn load_processing_job(conn: &Connection, job_id: &str) -> Result<Option<ProcessingJob>> {
    conn.query_row(
        "SELECT job_id, session_name, operation, input_revision, attempt, status, progress, result_ref, failure, failed_stage FROM session_processing_jobs WHERE job_id = ?1",
        params![job_id],
        |row| {
            let attempt: i64 = row.get(4)?;
            Ok(ProcessingJob {
                job_id: row.get(0)?, session_name: row.get(1)?, operation: row.get(2)?,
                input_revision: row.get(3)?, attempt: attempt.max(0) as u64,
                status: row.get(5)?, progress: row.get(6)?, result_ref: row.get(7)?,
                failure: row.get(8)?, failed_stage: row.get(9)?,
            })
        },
    ).optional().map_err(Into::into)
}

pub fn begin_processing_job(
    dir: &Path,
    name: &str,
    job_id: &str,
    operation: &str,
    input_revision: &str,
) -> Result<ProcessingJob> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if session_tombstone_exists_tx(&tx, name)? {
        anyhow::bail!("session '{name}' has been deleted");
    }
    let current = load_processing_job(&tx, job_id)?;
    if let Some(current) = &current {
        if current.session_name != name || current.operation != operation {
            anyhow::bail!("processing job id was already used for different input");
        }
        if current.input_revision == input_revision
            && matches!(current.status.as_str(), "queued" | "running")
        {
            return Ok(current.clone());
        }
    }
    let attempt = current.as_ref().map_or(1, |job| job.attempt + 1);
    tx.execute(
        "INSERT INTO session_processing_jobs (job_id, session_name, operation, input_revision, attempt, status, progress, result_ref, failure, failed_stage, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 0.0, NULL, NULL, NULL, ?6) ON CONFLICT(job_id) DO UPDATE SET input_revision = excluded.input_revision, attempt = excluded.attempt, status = 'queued', progress = 0.0, result_ref = NULL, failure = NULL, failed_stage = NULL, updated_at = excluded.updated_at",
        params![job_id, name, operation, input_revision, attempt as i64, Local::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(ProcessingJob {
        job_id: job_id.to_string(),
        session_name: name.to_string(),
        operation: operation.to_string(),
        input_revision: input_revision.to_string(),
        attempt,
        status: "queued".to_string(),
        progress: Some(0.0),
        result_ref: None,
        failure: None,
        failed_stage: None,
    })
}

pub fn get_processing_job(dir: &Path, job_id: &str) -> Result<Option<ProcessingJob>> {
    let conn = open_db(dir)?;
    load_processing_job(&conn, job_id)
}

pub fn latest_processing_job(dir: &Path, name: &str) -> Result<Option<ProcessingJob>> {
    let conn = open_db(dir)?;
    let job_id: Option<String> = conn.query_row(
        "SELECT job_id FROM session_processing_jobs WHERE session_name = ?1 ORDER BY updated_at DESC, job_id DESC LIMIT 1",
        params![name], |row| row.get(0),
    ).optional()?;
    job_id.map_or(Ok(None), |job_id| load_processing_job(&conn, &job_id))
}

pub fn pending_processing_jobs(dir: &Path, operation: &str) -> Result<Vec<ProcessingJob>> {
    let conn = open_db(dir)?;
    let mut statement = conn.prepare(
        "SELECT job_id FROM session_processing_jobs WHERE operation = ?1 AND status IN ('queued', 'running') ORDER BY updated_at, job_id",
    )?;
    let ids = statement
        .query_map(params![operation], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|job_id| {
            load_processing_job(&conn, &job_id)?
                .ok_or_else(|| anyhow::anyhow!("pending processing job disappeared"))
        })
        .collect()
}

pub fn update_processing_job(
    dir: &Path,
    job_id: &str,
    attempt: u64,
    status: &str,
    progress: Option<f64>,
    result_ref: Option<&str>,
    failure: Option<&str>,
    failed_stage: Option<&str>,
) -> Result<ProcessingJob> {
    if !matches!(
        status,
        "queued" | "running" | "failed" | "cancelled" | "complete"
    ) {
        anyhow::bail!("invalid processing job status '{status}'");
    }
    if progress.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
        anyhow::bail!("processing progress must be between zero and one");
    }
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_processing_job(&tx, job_id)?
        .ok_or_else(|| anyhow::anyhow!("processing job '{job_id}' not found"))?;
    if current.attempt != attempt {
        anyhow::bail!("processing job attempt was superseded");
    }
    if session_tombstone_exists_tx(&tx, &current.session_name)? {
        anyhow::bail!("session '{}' has been deleted", current.session_name);
    }
    if matches!(current.status.as_str(), "failed" | "cancelled" | "complete")
        && current.status != status
    {
        anyhow::bail!("late processing result rejected after {}", current.status);
    }
    tx.execute(
        "UPDATE session_processing_jobs SET status = ?1, progress = ?2, result_ref = ?3, failure = ?4, failed_stage = ?5, updated_at = ?6 WHERE job_id = ?7 AND attempt = ?8",
        params![status, progress, result_ref, failure, failed_stage, Local::now().to_rfc3339(), job_id, attempt as i64],
    )?;
    tx.commit()?;
    get_processing_job(dir, job_id)?
        .ok_or_else(|| anyhow::anyhow!("updated processing job disappeared"))
}

/// Publish a generated note and complete the exact processing attempt in one
/// transaction. This is the coordinator seam between otherwise-independent
/// job and note-association records: a cancelled, superseded, or deleted job
/// can never resurrect an association with a late result.
pub fn complete_processing_job_with_note(
    dir: &Path,
    job_id: &str,
    attempt: u64,
    source_id: &str,
    relative_path: &str,
    observed_content_hash: Option<&str>,
    expected_association_revision: u64,
) -> Result<NoteAssociation> {
    validate_note_reference(source_id, relative_path)?;
    let mut conn = open_db(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_processing_job(&tx, job_id)?
        .ok_or_else(|| anyhow::anyhow!("processing job '{job_id}' not found"))?;
    if current.attempt != attempt {
        anyhow::bail!("processing job attempt was superseded");
    }
    if session_tombstone_exists_tx(&tx, &current.session_name)? {
        anyhow::bail!("session '{}' has been deleted", current.session_name);
    }

    let association: Option<(String, String, Option<String>, i64, String, Option<String>)> = tx
        .query_row(
            "SELECT source_id, relative_path, observed_content_hash, revision, bb_thread_ids, distilled_memo_revision FROM session_note_associations WHERE session_name = ?1",
            params![current.session_name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()?;
    let exact_association = association.as_ref().is_some_and(
        |(current_source, current_path, current_hash, _, _, _)| {
            current_source == source_id
                && current_path == relative_path
                && current_hash.as_deref() == observed_content_hash
        },
    );
    if current.status == "complete" && exact_association {
        let (_, _, _, revision, thread_ids, memo_revision) = association.expect("checked above");
        return Ok(NoteAssociation {
            session_name: current.session_name,
            source_id: source_id.to_string(),
            relative_path: relative_path.to_string(),
            observed_content_hash: observed_content_hash.map(ToOwned::to_owned),
            revision: revision.max(0) as u64,
            bb_thread_ids: serde_json::from_str(&thread_ids).unwrap_or_default(),
            distilled_memo_revision: memo_revision,
        });
    }
    if !matches!(current.status.as_str(), "queued" | "running") {
        anyhow::bail!("late processing result rejected after {}", current.status);
    }

    let current_revision = association
        .as_ref()
        .map_or(0, |value| value.3.max(0) as u64);
    let revision = if exact_association {
        current_revision
    } else {
        if current_revision != expected_association_revision {
            anyhow::bail!(
                "note association revision conflict: expected {expected_association_revision}, found {current_revision}"
            );
        }
        let revision = current_revision + 1;
        tx.execute(
            "INSERT INTO session_note_associations (session_name, source_id, relative_path, observed_content_hash, revision, linked_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(session_name) DO UPDATE SET source_id = excluded.source_id, relative_path = excluded.relative_path, observed_content_hash = excluded.observed_content_hash, revision = excluded.revision, linked_at = excluded.linked_at",
            params![current.session_name, source_id, relative_path, observed_content_hash, revision as i64, Local::now().to_rfc3339()],
        )?;
        revision
    };
    tx.execute(
        "UPDATE session_processing_jobs SET status = 'complete', progress = 1.0, result_ref = ?1, failure = NULL, failed_stage = NULL, updated_at = ?2 WHERE job_id = ?3 AND attempt = ?4",
        params![relative_path, Local::now().to_rfc3339(), job_id, attempt as i64],
    )?;
    tx.commit()?;
    Ok(NoteAssociation {
        session_name: current.session_name,
        source_id: source_id.to_string(),
        relative_path: relative_path.to_string(),
        observed_content_hash: observed_content_hash.map(ToOwned::to_owned),
        revision,
        bb_thread_ids: association
            .as_ref()
            .map(|value| serde_json::from_str(&value.4).unwrap_or_default())
            .unwrap_or_default(),
        distilled_memo_revision: association.as_ref().and_then(|value| value.5.clone()),
    })
}

pub fn cancel_processing_job(dir: &Path, job_id: &str, attempt: u64) -> Result<ProcessingJob> {
    update_processing_job(dir, job_id, attempt, "cancelled", None, None, None, None)
}

pub fn dismiss_processing_job_failure(dir: &Path, job_id: &str) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        "UPDATE session_processing_jobs SET failure = NULL, updated_at = ?1 WHERE job_id = ?2 AND status = 'failed'",
        params![Local::now().to_rfc3339(), job_id],
    )?;
    Ok(())
}

pub fn set_people(dir: &Path, name: &str, people: Vec<String>) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    replace_people_tx(&tx, name, &people)?;
    tx.commit()?;
    Ok(())
}

pub fn set_title(dir: &Path, name: &str, title: Option<String>) -> Result<Option<String>> {
    let conn = open_db(dir)?;
    let saved = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    conn.execute(
        "UPDATE sessions SET title = ?1 WHERE name = ?2",
        params![saved, name],
    )?;
    Ok(saved)
}

pub fn set_calendar_event(
    dir: &Path,
    name: &str,
    event: CalendarEventMeta,
    people: Vec<String>,
) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    upsert_calendar_event_tx(&tx, name, &event)?;
    replace_people_tx(&tx, name, &people)?;
    tx.commit()?;
    Ok(())
}

pub fn delete_session(dir: &Path, name: &str) -> Result<()> {
    finalize_delete_session(dir, name)
}

pub fn delete_session_row(dir: &Path, name: &str) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute("DELETE FROM sessions WHERE name = ?1", params![name])?;
    Ok(())
}

pub struct VaultNoteInfo {
    pub id: String,
    pub absolute_path: String,
    pub source_session_name: Option<String>,
    pub updated_at: String,
}

pub fn list_vault_notes(dir: &Path) -> Result<Vec<VaultNoteInfo>> {
    // Listing must not materialize the vault: an absent DB means no notes yet,
    // so return empty instead of letting open_db create_dir_all the vault.
    // (Clean first-run launch fingerprints known notes for the default project
    // before any capture; creating the dir here would recreate ~/Documents/margins.)
    if !database_path(dir).exists() {
        return Ok(Vec::new());
    }
    let conn = open_db(dir)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, absolute_path, source_session_name, updated_at
        FROM vault_notes
        WHERE procured_by = 'margins'
        ORDER BY updated_at DESC, id ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VaultNoteInfo {
            id: row.get(0)?,
            absolute_path: row.get(1)?,
            source_session_name: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;

    let mut notes = Vec::new();
    for row in rows {
        notes.push(row.context("corrupt vault note row")?);
    }
    Ok(notes)
}

pub fn vault_note_path_by_id(dir: &Path, id: &str) -> Result<Option<String>> {
    let conn = open_db(dir)?;
    conn.query_row(
        "SELECT absolute_path FROM vault_notes WHERE id = ?1 AND procured_by = 'margins'",
        params![id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Register a standalone note discovered outside a session association.
/// Session-owned notes use [`link_note`] instead.
pub fn register_capture_note(
    dir: &Path,
    path: &str,
    source_session_name: Option<&str>,
) -> Result<String> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    let id = upsert_vault_note_tx(&tx, path, source_session_name)?;
    tx.commit()?;
    Ok(id)
}

pub fn remove_vault_note_by_id(dir: &Path, id: &str) -> Result<bool> {
    let conn = open_db(dir)?;
    let changed = conn.execute(
        "DELETE FROM vault_notes WHERE id = ?1 AND procured_by = 'margins'",
        params![id],
    )?;
    Ok(changed > 0)
}

pub fn remove_vault_note_by_path(dir: &Path, path: &str) -> Result<bool> {
    let conn = open_db(dir)?;
    let changed = conn.execute(
        "DELETE FROM vault_notes WHERE absolute_path = ?1 AND procured_by = 'margins'",
        params![path],
    )?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn finalized_capture_is_ended_and_remains_listed() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        create_session(&margins_dir, "meet", &Local::now(), "meet.md").unwrap();
        add_segment(&margins_dir, "meet", 0, "meet.wav", 0, None).unwrap();
        assert!(mark_session_ended(&margins_dir, "meet").is_err());
        update_segment_duration(&margins_dir, "meet", 0, 12.5).unwrap();
        mark_session_ended(&margins_dir, "meet").unwrap();
        mark_session_ended(&margins_dir, "meet").unwrap();
        assert_eq!(list_sessions(&margins_dir).unwrap().len(), 1);
        let conn = open_db(&margins_dir).unwrap();
        let lifecycle: String = conn
            .query_row(
                "SELECT lifecycle_state FROM sessions WHERE name = 'meet'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let repository: String = conn
            .query_row(
                "SELECT lifecycle FROM session_repository_state WHERE session_name = 'meet'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lifecycle, "ended");
        assert_eq!(repository, "processing");
    }

    #[test]
    fn stores_session_metadata_in_sqlite() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        add_segment(&margins_dir, "meet", 0, ".margins/meet_seg0.wav", 0, None).unwrap();
        update_segment_duration(&margins_dir, "meet", 0, 12.5).unwrap();
        set_people(
            &margins_dir,
            "meet",
            vec!["Ada".to_string(), "Grace".to_string()],
        )
        .unwrap();
        set_title(&margins_dir, "meet", Some("Weekly sync".to_string())).unwrap();
        link_note(
            &margins_dir,
            "meet",
            "workspace",
            "inbox/Weekly sync.md",
            None,
            0,
        )
        .unwrap();

        let meta = get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(meta.name, "meet");
        assert_eq!(meta.title.as_deref(), Some("Weekly sync"));
        assert_eq!(meta.people, vec!["Ada", "Grace"]);
        assert_eq!(meta.segments.len(), 1);
        assert_eq!(meta.segments[0].duration_secs, Some(12.5));
        assert_eq!(
            meta.vault_note_path.as_deref(),
            Some(
                dir.path()
                    .join("inbox/Weekly sync.md")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert!(database_path(&margins_dir).exists());
        assert!(!margins_dir.join("meet.meta.json").exists());
    }

    #[test]
    fn stores_session_grounding_in_sqlite() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        set_session_grounding(
            &margins_dir,
            "meet",
            &[SessionGrounding {
                memo_ids: vec!["m001".to_string(), "m002".to_string()],
                note_quote: "pricing risk became rollout risk".to_string(),
                section_id: Some("rollout-risk".to_string()),
                disposition: Some("folded_into_section".to_string()),
            }],
        )
        .unwrap();

        let grounding = get_session_grounding(&margins_dir, "meet").unwrap();
        assert_eq!(grounding.len(), 1);
        assert_eq!(grounding[0].memo_ids, vec!["m001", "m002"]);
        assert_eq!(grounding[0].note_quote, "pricing risk became rollout risk");
        assert_eq!(grounding[0].section_id.as_deref(), Some("rollout-risk"));
    }

    #[test]
    fn upserts_and_lists_session_artifacts() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            ".margins/meet_transcript_old.md",
            "durable",
            None,
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            "audio_segment",
            0,
            ".margins/meet_seg0.wav",
            "source",
            Some("2026-02-01T00:00:00Z"),
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            ".margins/meet_transcript.md",
            "durable",
            None,
        )
        .unwrap();

        let artifacts = list_session_artifacts(&margins_dir, "meet").unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].kind, "audio_segment");
        assert_eq!(artifacts[0].path, ".margins/meet_seg0.wav");
        assert_eq!(
            artifacts[0].expires_at.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        assert_eq!(artifacts[1].kind, SESSION_ARTIFACT_KIND_TRANSCRIPT);
        assert_eq!(artifacts[1].ordinal, 0);
        assert_eq!(artifacts[1].path, ".margins/meet_transcript.md");
        assert_eq!(artifacts[1].retention_class, "durable");
        assert!(!artifacts[1].created_at.is_empty());
    }

    #[test]
    fn artifact_path_rewrites_are_transactional_and_compare_old_values() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();
        for session in ["one", "two"] {
            create_session(&margins_dir, session, &start, &format!("{session}.md")).unwrap();
            upsert_session_artifact(
                &margins_dir,
                session,
                SESSION_ARTIFACT_KIND_TRANSCRIPT,
                0,
                &format!(".margins/{session}_aligned.md"),
                "durable",
                None,
            )
            .unwrap();
        }
        let updates = [
            SessionArtifactPathUpdate {
                session_name: "one".to_string(),
                kind: SESSION_ARTIFACT_KIND_TRANSCRIPT.to_string(),
                ordinal: 0,
                old_path: ".margins/one_aligned.md".to_string(),
                new_path: "_margins/one_aligned.md".to_string(),
            },
            SessionArtifactPathUpdate {
                session_name: "two".to_string(),
                kind: SESSION_ARTIFACT_KIND_TRANSCRIPT.to_string(),
                ordinal: 0,
                old_path: ".margins/stale.md".to_string(),
                new_path: "_margins/two_aligned.md".to_string(),
            },
        ];

        assert!(rewrite_session_artifact_paths(&margins_dir, &updates).is_err());
        assert_eq!(
            list_session_artifacts(&margins_dir, "one").unwrap()[0].path,
            ".margins/one_aligned.md"
        );
        assert_eq!(
            list_session_artifacts(&margins_dir, "two").unwrap()[0].path,
            ".margins/two_aligned.md"
        );
    }

    #[test]
    fn session_artifacts_can_be_deleted_and_cascade_with_session() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        create_session(&margins_dir, "other", &start, "other.md").unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            ".margins/meet_transcript.md",
            "durable",
            None,
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "other",
            SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            ".margins/other_transcript.md",
            "durable",
            None,
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "other",
            "audio_segment",
            0,
            ".margins/other_seg0.wav",
            "source",
            None,
        )
        .unwrap();

        let deleted =
            delete_session_artifacts_registry_rows(&margins_dir, "other", Some("audio_segment"))
                .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(
            list_session_artifacts(&margins_dir, "other").unwrap().len(),
            1
        );

        delete_session(&margins_dir, "meet").unwrap();
        assert!(list_session_artifacts(&margins_dir, "meet")
            .unwrap()
            .is_empty());
        let remaining = list_session_artifacts(&margins_dir, "other").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, ".margins/other_transcript.md");
    }

    #[test]
    fn deleted_session_leaves_tombstone_and_is_not_active() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        assert!(session_exists(&margins_dir, "meet").unwrap());
        begin_delete_session(&margins_dir, "meet").unwrap();
        assert!(is_session_tombstoned(&margins_dir, "meet").unwrap());
        assert!(list_sessions(&margins_dir).unwrap().is_empty());
        finalize_delete_session(&margins_dir, "meet").unwrap();

        assert!(is_session_tombstoned(&margins_dir, "meet").unwrap());
        assert!(session_exists(&margins_dir, "meet").unwrap());
        assert!(create_session(&margins_dir, "meet", &start, "meet.md").is_err());
    }

    #[test]
    fn legacy_database_schema_migrates_lifecycle_columns_and_tombstones() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        let db_path = database_path(&margins_dir);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE sessions (
                    name TEXT PRIMARY KEY NOT NULL,
                    start_time TEXT NOT NULL,
                    notes_path TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    title TEXT,
                    vault_note_path TEXT,
                    note_error TEXT,
                    note_error_at TEXT
                );
                CREATE TABLE session_segments (
                    session_name TEXT NOT NULL,
                    segment_index INTEGER NOT NULL,
                    wav_path TEXT NOT NULL,
                    offset_ms INTEGER NOT NULL,
                    duration_secs REAL,
                    started_at TEXT NOT NULL,
                    PRIMARY KEY (session_name, segment_index)
                );
                INSERT INTO sessions (name, start_time, notes_path, created_at)
                VALUES ('legacy', '2026-01-01T00:00:00Z', 'legacy.md', '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let sessions = list_sessions(&margins_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "legacy");

        let conn = Connection::open(&db_path).unwrap();
        let lifecycle_state: Option<String> = conn
            .query_row(
                "SELECT lifecycle_state FROM sessions WHERE name = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lifecycle_state.as_deref(), Some("active"));
        let processing_state: Option<String> = conn
            .query_row(
                "SELECT processing_state FROM sessions WHERE name = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let failed_stage: Option<String> = conn
            .query_row(
                "SELECT failed_stage FROM sessions WHERE name = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(processing_state.as_deref(), Some("none"));
        assert_eq!(failed_stage, None);
        let tombstone_table: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'session_tombstones'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstone_table, "session_tombstones");
    }

    #[test]
    fn processing_failure_remains_independent_when_a_note_is_linked() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        let job =
            begin_processing_job(&margins_dir, "meet", "note:meet", "distill_note", "r1").unwrap();
        update_processing_job(
            &margins_dir,
            &job.job_id,
            job.attempt,
            "failed",
            None,
            None,
            Some("AI note distillation failed"),
            Some("distill"),
        )
        .unwrap();

        let failed = get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(failed.processing_state.as_deref(), Some("failed"));
        assert_eq!(failed.failed_stage.as_deref(), Some("distill"));
        assert_eq!(
            failed.note_error.as_deref(),
            Some("AI note distillation failed")
        );

        link_note(&margins_dir, "meet", "workspace", "meet.md", None, 0).unwrap();
        let still_failed = get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(still_failed.processing_state.as_deref(), Some("failed"));
        assert_eq!(still_failed.failed_stage.as_deref(), Some("distill"));
        assert_eq!(
            still_failed.note_error.as_deref(),
            Some("AI note distillation failed")
        );
    }

    #[test]
    fn tombstoned_session_cannot_be_linked_to_note() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        begin_delete_session(&margins_dir, "meet").unwrap();

        let err = link_note(&margins_dir, "meet", "workspace", "meet.md", None, 0)
            .expect_err("deleted sessions must not accept note links");
        assert!(err.to_string().contains("deleted"));
    }

    #[test]
    fn lists_expired_session_artifacts_and_deletes_exact_row() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            "capture_context",
            0,
            ".margins/artifacts/meet/scratch/capture-context.md",
            "temporary",
            Some("2026-01-01T00:00:00Z"),
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            "capture_context",
            1,
            ".margins/artifacts/meet/scratch/future.md",
            "temporary",
            Some("2999-01-01T00:00:00Z"),
        )
        .unwrap();
        upsert_session_artifact(
            &margins_dir,
            "meet",
            SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            ".margins/artifacts/meet/transcript.md",
            "durable",
            Some("2026-01-01T00:00:00Z"),
        )
        .unwrap();

        let expired = list_expired_session_artifacts(&margins_dir, Local::now()).unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].kind, "capture_context");
        assert_eq!(expired[0].ordinal, 0);

        assert_eq!(
            delete_session_artifact_registry_row(&margins_dir, "meet", "capture_context", 0)
                .unwrap(),
            1
        );
        let remaining = list_session_artifacts(&margins_dir, "meet").unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(remaining
            .iter()
            .any(|artifact| artifact.kind == "capture_context" && artifact.ordinal == 1));
        assert!(remaining
            .iter()
            .any(|artifact| artifact.kind == SESSION_ARTIFACT_KIND_TRANSCRIPT));
    }

    #[test]
    fn migrates_legacy_json_once() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        std::fs::write(
            margins_dir.join("legacy.meta.json"),
            r#"{
              "name":"legacy",
              "start_time":"2026-01-01T10:00:00-08:00",
              "notes_path":"legacy.md",
              "created_at":"2026-01-01T10:00:00-08:00",
              "title":"Legacy meeting",
              "segments":[{"segment_index":0,"wav_path":".margins/legacy_seg0.wav","offset_ms":0,"duration_secs":3.0,"started_at":"2026-01-01T10:00:00-08:00"}],
              "people":["Ada"],
              "calendar_event":{"title":"Calendar title","start":null,"end":null,"calendar_id":null,"event_id":null},
              "vault_note_path":"/vault/inbox/Legacy meeting.md"
            }"#,
        )
        .unwrap();

        let meta = get_session_meta(&margins_dir, "legacy").unwrap();
        assert_eq!(meta.title.as_deref(), Some("Legacy meeting"));
        assert_eq!(meta.people, vec!["Ada"]);
        assert_eq!(
            meta.calendar_event.as_ref().unwrap().title,
            "Calendar title"
        );
        assert_eq!(
            meta.vault_note_path.as_deref(),
            Some("/vault/inbox/Legacy meeting.md")
        );
        assert!(!margins_dir.join("legacy.meta.json").exists());
        assert!(margins_dir.join("legacy.meta.json.migrated").exists());
    }

    #[test]
    fn list_sessions_rejects_malformed_rows() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();
        create_session(&margins_dir, "good", &start, "good.md").unwrap();

        let conn = open_db(&margins_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO sessions (name, start_time, notes_path, created_at)
            VALUES (x'ff', '2026-01-01T10:00:00-08:00', 'bad.md', '2026-01-01T10:00:00-08:00')
            "#,
            [],
        )
        .unwrap();

        let error = match list_sessions(&margins_dir) {
            Ok(_) => panic!("malformed rows must fail the query"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("corrupt session row"));
    }

    #[test]
    fn list_vault_notes_rejects_malformed_rows() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let conn = open_db(&margins_dir).unwrap();
        conn.execute(
            r#"
            INSERT INTO vault_notes
                (id, absolute_path, procured_by, source_session_name, created_at, updated_at)
            VALUES
                ('bad-note', x'ff', 'margins', NULL, '2026-01-01T10:00:00-08:00', '2026-01-01T10:00:00-08:00')
            "#,
            [],
        )
        .unwrap();

        let error = match list_vault_notes(&margins_dir) {
            Ok(_) => panic!("malformed rows must fail the query"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("corrupt vault note row"));
    }
}
