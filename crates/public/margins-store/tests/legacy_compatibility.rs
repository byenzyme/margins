use chrono::{Local, TimeZone};
use margins_store::{legacy, SqliteSessionRepository};
use rusqlite::Connection;
use std::path::Path;
use tempfile::tempdir;

fn logical_snapshot(path: &Path) -> Vec<(String, String)> {
    let connection = Connection::open(path).unwrap();
    let mut snapshot = Vec::new();
    let mut schema = connection
        .prepare(
            "SELECT type || ':' || name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name",
        )
        .unwrap();
    for row in schema
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
    {
        snapshot.push(row.unwrap());
    }
    for (table, query) in [
        ("sessions", "SELECT quote(name)||'|'||quote(start_time)||'|'||quote(notes_path)||'|'||quote(created_at)||'|'||quote(title)||'|'||quote(vault_note_path)||'|'||quote(note_error)||'|'||quote(processing_state)||'|'||quote(failed_stage)||'|'||quote(lifecycle_state) FROM sessions ORDER BY name"),
        ("session_segments", "SELECT quote(session_name)||'|'||segment_index||'|'||quote(wav_path)||'|'||offset_ms||'|'||quote(duration_secs)||'|'||quote(started_at) FROM session_segments ORDER BY session_name, segment_index"),
        ("session_artifacts", "SELECT quote(session_name)||'|'||quote(kind)||'|'||ordinal||'|'||quote(path)||'|'||quote(retention_class)||'|'||quote(created_at)||'|'||quote(expires_at) FROM session_artifacts ORDER BY session_name, kind, ordinal"),
        ("session_grounding", "SELECT quote(session_name)||'|'||position||'|'||quote(memo_ids)||'|'||quote(note_quote)||'|'||quote(section_id)||'|'||quote(disposition) FROM session_grounding ORDER BY session_name, position"),
        ("session_tombstones", "SELECT quote(name)||'|'||quote(state)||'|'||quote(deleted_at)||'|'||quote(updated_at) FROM session_tombstones ORDER BY name"),
        ("session_repository_state", "SELECT quote(session_name)||'|'||revision||'|'||quote(lifecycle)||'|'||updated_at_ms FROM session_repository_state ORDER BY session_name"),
    ] {
        let mut statement = connection.prepare(query).unwrap();
        for row in statement.query_map([], |row| row.get::<_, String>(0)).unwrap() {
            snapshot.push((format!("row:{table}"), row.unwrap()));
        }
    }
    snapshot
}

#[test]
fn opening_a_populated_legacy_database_is_additive_and_idempotent() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    let start = Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    legacy::create_session(&margins_dir, "kept", &start, "kept.md").unwrap();
    legacy::add_segment(&margins_dir, "kept", 0, "kept.wav", 17, Some(2.25)).unwrap();
    legacy::set_title(&margins_dir, "kept", Some("Exact title".to_string())).unwrap();
    legacy::set_note_failure(&margins_dir, "kept", "retry me", Some("distill")).unwrap();
    legacy::set_session_grounding(
        &margins_dir,
        "kept",
        &[legacy::SessionGrounding {
            memo_ids: vec!["m-1".to_string()],
            note_quote: "quoted fact".to_string(),
            section_id: Some("decision".to_string()),
            disposition: Some("kept".to_string()),
        }],
    )
    .unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        "kept",
        "transcript",
        0,
        "transcript.md",
        "durable",
        None,
    )
    .unwrap();

    let before = legacy::get_session_meta(&margins_dir, "kept").unwrap();
    SqliteSessionRepository::open(&margins_dir).unwrap();
    let first = logical_snapshot(&legacy::database_path(&margins_dir));
    SqliteSessionRepository::open(&margins_dir).unwrap();
    let second = logical_snapshot(&legacy::database_path(&margins_dir));
    let after = legacy::get_session_meta(&margins_dir, "kept").unwrap();

    assert_eq!(first, second, "second open must not change schema or rows");
    assert_eq!(before.name, after.name);
    assert_eq!(before.start_time, after.start_time);
    assert_eq!(before.notes_path, after.notes_path);
    assert_eq!(before.title, after.title);
    assert_eq!(before.note_error, after.note_error);
    assert_eq!(before.processing_state, after.processing_state);
    assert_eq!(before.failed_stage, after.failed_stage);
    assert_eq!(before.segments.len(), after.segments.len());
    assert_eq!(before.segments[0].offset_ms, after.segments[0].offset_ms);
    assert_eq!(
        legacy::get_session_grounding(&margins_dir, "kept").unwrap()[0].note_quote,
        "quoted fact"
    );
    assert_eq!(
        legacy::list_session_artifacts(&margins_dir, "kept").unwrap()[0].path,
        "transcript.md"
    );
}

#[test]
fn legacy_json_import_is_one_shot_and_preserves_invalid_input() {
    let temporary = tempdir().unwrap();
    let margins_dir = temporary.path().join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    let valid = margins_dir.join("valid.meta.json");
    let invalid = margins_dir.join("invalid.meta.json");
    std::fs::write(
        &valid,
        r#"{"name":"legacy-id","start_time":"2026-01-01T00:00:00Z","notes_path":"legacy.md","created_at":"2026-01-01T00:00:00Z","title":"Legacy","segments":[{"segment_index":0,"wav_path":"legacy.wav","offset_ms":42,"duration_secs":1.25,"started_at":"2026-01-01T00:00:00Z"}],"people":["Ada"],"calendar_event":null,"vault_note_path":"/vault/Legacy.md","note_error":"retry","processing_state":"failed","failed_stage":"distill"}"#,
    )
    .unwrap();
    std::fs::write(&invalid, "{ definitely not json").unwrap();

    let imported = legacy::get_session_meta(&margins_dir, "legacy-id").unwrap();
    assert_eq!(imported.name, "legacy-id");
    assert_eq!(imported.segments[0].offset_ms, 42);
    assert_eq!(imported.people, vec!["Ada"]);
    assert_eq!(imported.note_error.as_deref(), Some("retry"));
    assert!(!valid.exists());
    assert!(margins_dir.join("valid.meta.json.migrated").exists());
    assert!(invalid.exists());

    SqliteSessionRepository::open(&margins_dir).unwrap();
    let snapshot = logical_snapshot(&legacy::database_path(&margins_dir));
    let reopened = legacy::get_session_meta(&margins_dir, "legacy-id").unwrap();
    assert_eq!(reopened.start_time, imported.start_time);
    assert_eq!(
        snapshot,
        logical_snapshot(&legacy::database_path(&margins_dir))
    );
    assert!(invalid.exists());
}

#[test]
fn copied_pre_extraction_database_migrates_without_losing_failure_or_note_data() {
    let temporary = tempdir().unwrap();
    let source = temporary.path().join("legacy-source.sqlite");
    let margins_dir = temporary.path().join("copied");
    std::fs::create_dir_all(&margins_dir).unwrap();
    {
        let connection = Connection::open(&source).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE sessions (
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
                CREATE TABLE session_segments (
                    session_name TEXT NOT NULL,
                    segment_index INTEGER NOT NULL,
                    wav_path TEXT NOT NULL,
                    offset_ms INTEGER NOT NULL,
                    duration_secs REAL,
                    started_at TEXT NOT NULL,
                    PRIMARY KEY (session_name, segment_index)
                );
                INSERT INTO sessions
                    (name, start_time, notes_path, created_at, title, vault_note_path,
                     note_error, processing_state, failed_stage)
                VALUES
                    ('copied', '2026-01-01T00:00:00Z', 'copied.md',
                     '2026-01-01T00:00:00Z', 'Copied title', '/vault/Copied.md',
                     'preserve me', 'failed', 'distill');
                INSERT INTO session_segments
                    (session_name, segment_index, wav_path, offset_ms, duration_secs, started_at)
                VALUES
                    ('copied', 0, 'copied.wav', 0, 1.5, '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
    }
    std::fs::copy(&source, legacy::database_path(&margins_dir)).unwrap();

    SqliteSessionRepository::open(&margins_dir).unwrap();
    let meta = legacy::get_session_meta(&margins_dir, "copied").unwrap();
    assert_eq!(meta.note_error.as_deref(), Some("preserve me"));
    assert_eq!(meta.processing_state.as_deref(), Some("failed"));
    assert_eq!(meta.failed_stage.as_deref(), Some("distill"));
    assert_eq!(meta.vault_note_path.as_deref(), Some("/vault/Copied.md"));
    assert_eq!(meta.segments.len(), 1);

    let connection = Connection::open(legacy::database_path(&margins_dir)).unwrap();
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM session_repository_state WHERE session_name = 'copied'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, 0);
}
