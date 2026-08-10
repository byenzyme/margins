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
        WHERE COALESCE(s.lifecycle_state, 'active') = 'active'
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

pub fn set_vault_note_path(dir: &Path, name: &str, vault_path: &str) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    if session_tombstone_exists_tx(&tx, name)? {
        anyhow::bail!("session '{name}' has been deleted");
    }
    upsert_vault_note_tx(&tx, vault_path, Some(name))?;
    tx.execute(
        r#"
        UPDATE sessions
        SET vault_note_path = ?1,
            note_error = NULL,
            note_error_at = NULL,
            processing_state = 'done',
            failed_stage = NULL
        WHERE name = ?2
        "#,
        params![vault_path, name],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn sync_session_note_metadata(
    dir: &Path,
    name: &str,
    vault_path: &str,
    title: Option<&str>,
    people: &[String],
    sync_people: bool,
) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    if session_tombstone_exists_tx(&tx, name)? {
        anyhow::bail!("session '{name}' has been deleted");
    }
    upsert_vault_note_tx(&tx, vault_path, Some(name))?;
    let cleaned_title = title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    tx.execute(
        r#"
        UPDATE sessions
        SET vault_note_path = ?1,
            title = COALESCE(?2, title),
            note_error = NULL,
            note_error_at = NULL,
            processing_state = 'done',
            failed_stage = NULL
        WHERE name = ?3
        "#,
        params![vault_path, cleaned_title, name],
    )?;
    if sync_people {
        replace_people_tx(&tx, name, people)?;
    }
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

pub fn move_session_vault_note_path(
    dir: &Path,
    name: &str,
    old_path: &str,
    new_path: &str,
) -> Result<()> {
    let mut conn = open_db(dir)?;
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM vault_notes WHERE absolute_path = ?1 AND procured_by = 'margins'",
        params![old_path],
    )?;
    upsert_vault_note_tx(&tx, new_path, Some(name))?;
    tx.execute(
        "UPDATE sessions SET vault_note_path = ?1 WHERE name = ?2",
        params![new_path, name],
    )?;
    tx.commit()?;
    Ok(())
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

pub fn clear_vault_note_path(dir: &Path, name: &str) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        "UPDATE sessions SET vault_note_path = NULL WHERE name = ?1",
        params![name],
    )?;
    Ok(())
}

pub fn set_note_error(dir: &Path, name: &str, message: &str) -> Result<()> {
    set_note_failure(dir, name, message, None)
}

pub fn set_note_failure(
    dir: &Path,
    name: &str,
    message: &str,
    failed_stage: Option<&str>,
) -> Result<()> {
    let conn = open_db(dir)?;
    let now = Local::now().to_rfc3339();
    conn.execute(
        r#"
        UPDATE sessions
        SET note_error = ?1,
            note_error_at = ?2,
            processing_state = 'failed',
            failed_stage = ?3
        WHERE name = ?4
        "#,
        params![message, now, failed_stage, name],
    )?;
    Ok(())
}

pub fn clear_note_error(dir: &Path, name: &str) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        r#"
        UPDATE sessions
        SET note_error = NULL,
            note_error_at = NULL,
            failed_stage = NULL,
            processing_state = CASE
                WHEN processing_state = 'failed' THEN 'none'
                ELSE processing_state
            END
        WHERE name = ?1
        "#,
        params![name],
    )?;
    Ok(())
}

pub fn set_processing_state(
    dir: &Path,
    name: &str,
    processing_state: &str,
    failed_stage: Option<&str>,
) -> Result<()> {
    let conn = open_db(dir)?;
    conn.execute(
        "UPDATE sessions SET processing_state = ?1, failed_stage = ?2 WHERE name = ?3",
        params![processing_state, failed_stage, name],
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
        set_vault_note_path(&margins_dir, "meet", "/vault/inbox/Weekly sync.md").unwrap();

        let meta = get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(meta.name, "meet");
        assert_eq!(meta.title.as_deref(), Some("Weekly sync"));
        assert_eq!(meta.people, vec!["Ada", "Grace"]);
        assert_eq!(meta.segments.len(), 1);
        assert_eq!(meta.segments[0].duration_secs, Some(12.5));
        assert_eq!(
            meta.vault_note_path.as_deref(),
            Some("/vault/inbox/Weekly sync.md")
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
    fn processing_failure_stage_is_persisted_and_cleared_on_success() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        set_processing_state(&margins_dir, "meet", "distilling", None).unwrap();
        set_note_failure(
            &margins_dir,
            "meet",
            "AI note distillation failed",
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

        set_vault_note_path(&margins_dir, "meet", "/vault/meet.md").unwrap();
        let done = get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(done.processing_state.as_deref(), Some("done"));
        assert_eq!(done.failed_stage, None);
        assert_eq!(done.note_error, None);
    }

    #[test]
    fn tombstoned_session_cannot_be_linked_to_note() {
        let dir = tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let start = Local::now();

        create_session(&margins_dir, "meet", &start, "meet.md").unwrap();
        begin_delete_session(&margins_dir, "meet").unwrap();

        let err = set_vault_note_path(&margins_dir, "meet", "/vault/meet.md")
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
        let good_path = dir.path().join("good.md");
        set_vault_note_path(
            &margins_dir,
            "missing-session",
            &good_path.to_string_lossy(),
        )
        .unwrap();

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
