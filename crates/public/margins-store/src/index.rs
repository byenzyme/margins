//! Storage-only session index queries suitable for a standalone CLI.

use crate::legacy;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Filters for the portable storage-backed index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndexQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// One storage-backed session row, without vault scanning or note mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub name: String,
    pub start_time: String,
    pub notes_path: String,
    pub title: Option<String>,
    pub segment_count: i64,
    pub duration_secs: f64,
    pub vault_note_path: Option<String>,
    pub note_error: Option<String>,
    pub processing_state: Option<String>,
    pub failed_stage: Option<String>,
    pub people: Vec<String>,
    pub calendar_event_title: Option<String>,
}

/// Lists persisted sessions without reading note files or materializing an
/// absent database directory.
pub fn list_session_index(
    margins_dir: &Path,
    query: SessionIndexQuery,
) -> anyhow::Result<Vec<SessionIndexEntry>> {
    if !legacy::database_path(margins_dir).exists() {
        return Ok(Vec::new());
    }

    let tombstoned = legacy::list_session_tombstone_names(margins_dir)?
        .into_iter()
        .collect::<HashSet<_>>();
    let after = parse_bound(query.started_after.as_deref(), "started_after")?;
    let before = parse_bound(query.started_before.as_deref(), "started_before")?;
    if query.limit == Some(0) {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();

    for session in legacy::list_sessions(margins_dir)? {
        if tombstoned.contains(&session.name) {
            continue;
        }
        let started = DateTime::parse_from_rfc3339(&session.start_time).map_err(|error| {
            anyhow::anyhow!("invalid start_time for '{}': {error}", session.name)
        })?;
        if after.is_some_and(|bound| started <= bound)
            || before.is_some_and(|bound| started >= bound)
        {
            continue;
        }
        let meta = legacy::get_session_meta(margins_dir, &session.name)?;
        entries.push((
            started,
            SessionIndexEntry {
                name: session.name,
                start_time: session.start_time,
                notes_path: session.notes_path,
                title: meta.title,
                segment_count: session.segment_count,
                duration_secs: meta
                    .segments
                    .iter()
                    .filter_map(|segment| segment.duration_secs)
                    .sum(),
                vault_note_path: meta.vault_note_path,
                note_error: meta.note_error,
                processing_state: meta.processing_state,
                failed_stage: meta.failed_stage,
                people: meta.people,
                calendar_event_title: meta.calendar_event.map(|event| event.title),
            },
        ));
    }
    entries.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut entries = entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    if let Some(limit) = query.limit {
        entries.truncate(limit);
    }
    Ok(entries)
}

fn parse_bound(value: Option<&str>, field: &str) -> anyhow::Result<Option<DateTime<FixedOffset>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map_err(|error| anyhow::anyhow!("invalid {field}: {error}"))
        })
        .transpose()
}
