//! Portable transcript artifact selection and rendering inputs.

use crate::artifacts::{
    artifact_registry_disk_path, confined_session_artifact_access_disk_path,
};
use anyhow::{bail, Context, Result};
use margins_store::legacy;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptView {
    pub session_name: String,
    pub source_path: PathBuf,
    pub body: String,
    pub view: &'static str,
    pub speaker_alias: Option<String>,
    pub started_at: String,
    pub created_at: String,
    pub title: String,
    pub calendar_event: String,
    pub people: Vec<String>,
    pub memo_path: String,
    pub saved_note_path: Option<String>,
}

pub fn transcript_artifact_path(margins_dir: &Path, name: &str) -> PathBuf {
    margins_dir
        .join("artifacts")
        .join(name)
        .join("transcript.md")
}

pub fn preferred_transcript_path(margins_dir: &Path, name: &str) -> Option<PathBuf> {
    let artifact = transcript_artifact_path(margins_dir, name);
    if artifact.exists() {
        return Some(artifact);
    }
    if let Some(path) = legacy::list_session_artifacts(margins_dir, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT)
        .find_map(|artifact| {
            confined_session_artifact_access_disk_path(margins_dir, name, &artifact.path)
        })
        .filter(|path| path.exists())
    {
        return Some(path);
    }
    let aligned = margins_dir.join(format!("{name}_aligned.md"));
    if aligned.exists() {
        return Some(aligned);
    }
    let capture = margins_dir.join(format!("{name}_capture_context.md"));
    if capture.exists() {
        return Some(capture);
    }
    live_transcript_source_path(margins_dir, name)
}

pub fn resolve_session_name(margins_dir: &Path, requested: &str) -> Result<String> {
    if requested != "latest" {
        return Ok(requested.to_string());
    }
    if let Some(name) = legacy::list_sessions(margins_dir)?
        .first()
        .map(|s| s.name.clone())
    {
        return Ok(name);
    }
    artifact_session_names(margins_dir)
        .into_iter()
        .next()
        .context("No sessions found.")
}

pub fn load_transcript_view(
    work_dir: &Path,
    margins_dir: &Path,
    requested: &str,
) -> Result<TranscriptView> {
    let name = resolve_session_name(margins_dir, requested)?;
    let meta = legacy::get_session_meta(margins_dir, &name).ok();
    let (source_path, mut body, view) =
        if let Some((path, body)) = read_live_transcript_body(work_dir, margins_dir, &name)? {
            (path, body, "full")
        } else if let Some((path, body)) =
            read_terminal_checkpoint_body(work_dir, margins_dir, &name, meta.as_ref())?
        {
            (path, body, "full")
        } else {
            let (path, body) = read_registered_or_fallback(work_dir, margins_dir, &name)?;
            (path, body, "aligned")
        };
    let speaker_alias = speaker_alias_from_meta(meta.as_ref());
    body = apply_speaker_alias(&body, speaker_alias.as_deref());
    let saved_note_path = saved_note_path_for_session(margins_dir, &name);
    Ok(TranscriptView {
        session_name: name,
        source_path,
        body,
        view,
        speaker_alias,
        started_at: meta
            .as_ref()
            .map(|m| m.start_time.clone())
            .unwrap_or_default(),
        created_at: meta
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_default(),
        title: meta
            .as_ref()
            .and_then(|m| m.title.clone())
            .unwrap_or_default(),
        calendar_event: meta
            .as_ref()
            .and_then(|m| m.calendar_event.as_ref().map(|e| e.title.clone()))
            .unwrap_or_default(),
        people: meta.as_ref().map(|m| m.people.clone()).unwrap_or_default(),
        memo_path: meta
            .as_ref()
            .map(|m| m.notes_path.clone())
            .unwrap_or_default(),
        saved_note_path,
    })
}

fn read_terminal_checkpoint_body(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: Option<&legacy::SessionMeta>,
) -> Result<Option<(PathBuf, String)>> {
    let checkpoint = legacy::list_session_artifacts(margins_dir, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT)
        .filter(|artifact| artifact.path.ends_with(".live-transcript.json"))
        .find_map(|artifact| {
            confined_session_artifact_access_disk_path(margins_dir, name, &artifact.path)
        });
    let Some(path) = checkpoint else {
        return Ok(None);
    };
    let entries = margins_media::transcript::merge_word_entries_to_phrases(
        crate::processing::read_transcript_entries(&path)?,
        2_000,
    );
    let Some(meta) = meta else {
        return Ok(None);
    };
    let started_at = legacy::get_session_start_time(margins_dir, name)?;
    let memo_path = artifact_registry_disk_path(work_dir, margins_dir, &meta.notes_path);
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    let body = crate::alignment::render_aligned_markdown(name, &started_at, &memo, &entries);
    Ok(Some((path, body)))
}

pub fn read_registered_or_fallback(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
) -> Result<(PathBuf, String)> {
    let registered = legacy::list_session_artifacts(margins_dir, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT)
        .filter(|artifact| !artifact.path.ends_with(".live-transcript.json"))
        .filter_map(|artifact| {
            confined_session_artifact_access_disk_path(margins_dir, name, &artifact.path)
        });
    let fallbacks = [
        transcript_artifact_path(margins_dir, name),
        margins_dir.join(format!("{name}_aligned.md")),
        margins_dir.join(format!("{name}_capture_context.md")),
    ];
    for path in registered.chain(fallbacks) {
        if let Ok(body) = std::fs::read_to_string(&path) {
            if !body.trim().is_empty() {
                return Ok((path, body));
            }
        }
    }
    if let Some((path, body)) = read_live_transcript_body(work_dir, margins_dir, name)? {
        return Ok((path, body));
    }
    bail!("No aligned transcript or capture context found for '{name}'.")
}

fn live_transcript_source_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let segments = dir.join(format!("{name}_live_transcript_segments.jsonl"));
    if jsonl_has_event(&segments) {
        return Some(segments);
    }
    let legacy = dir.join(format!("{name}_backchannel_trace.jsonl"));
    jsonl_has_event(&legacy).then_some(legacy)
}

fn jsonl_has_event(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|raw| {
        raw.lines()
            .any(|line| serde_json::from_str::<Value>(line).is_ok())
    })
}

fn read_live_transcript_body(
    work_dir: &Path,
    dir: &Path,
    name: &str,
) -> Result<Option<(PathBuf, String)>> {
    let Some(path) = live_transcript_source_path(dir, name) else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let events = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            segment_mode_for_path(&path)
                || matches!(
                    event.get("kind").and_then(Value::as_str),
                    Some("memo_transcript_checkpoint") | Some("live_transcript_finalized")
                )
        })
        .collect::<Vec<_>>();
    let segment_mode = segment_mode_for_path(&path);
    let mut timeline = Vec::<(u64, usize, String)>::new();
    let mut seen = HashSet::new();
    let mut source = if segment_mode {
        "live_segments"
    } else {
        "legacy_live_trace"
    }
    .to_string();
    let mut decoded = 0;
    let mut committed = 0;
    for event in events {
        if let Some(value) = event
            .get("transcript_source")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            source = value.to_string();
        }
        decoded = decoded.max(
            event
                .get("decoded_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        committed = committed.max(
            event
                .get("committed_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        let fallback = event.get("start_ms").and_then(Value::as_u64).unwrap_or(0);
        let keys: &[&str] = if segment_mode {
            &["segment_transcript"]
        } else {
            &[
                "new_transcript",
                "previous_transcript",
                "full_transcript",
                "transcript",
            ]
        };
        for key in keys {
            if let Some(text) = event.get(*key).and_then(Value::as_str) {
                for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    if !seen.insert(line.to_string()) {
                        continue;
                    }
                    if let Some(ms) =
                        elapsed_line_ms(line).or_else(|| segment_mode.then_some(fallback))
                    {
                        timeline.push((ms, timeline.len(), line.to_string()));
                    }
                }
            }
        }
    }
    if timeline.is_empty() {
        return Ok(None);
    }
    let memo = legacy::get_session_meta(dir, name)
        .ok()
        .and_then(|m| {
            let path = PathBuf::from(m.notes_path);
            std::fs::read_to_string(if path.is_absolute() {
                path
            } else {
                work_dir.join(path)
            })
            .ok()
        })
        .unwrap_or_default();
    let mut rows = timeline
        .into_iter()
        .map(|(ms, seq, text)| (ms, 0u8, seq, text))
        .collect::<Vec<_>>();
    let mut untimed = Vec::new();
    for (seq, line) in memo.lines().map(str::trim).enumerate() {
        if line.is_empty() || line == "---" || line.starts_with('#') {
            continue;
        }
        if let Some((ms, text)) = memo_line(line) {
            rows.push((
                ms,
                1,
                seq,
                format!("[{}] memo: {}", format_elapsed(ms), text),
            ));
        } else {
            untimed.push(line.to_string());
        }
    }
    rows.sort_by_key(|row| (row.0, row.1, row.2));
    let mut body = format!("# Capture Context\n\nSession: `{name}`\nSource: Margins CLI memo and live transcript state.\nTranscript source: `{source}`\nDecoded until: {}\nCommitted until: {}\n\n## Timeline\n\n", format_elapsed(decoded), format_elapsed(committed));
    for (_, _, _, line) in rows {
        body.push_str(&line);
        body.push('\n');
    }
    if !untimed.is_empty() {
        body.push_str("\n## Untimed memo / reflection lines\n\n");
        for line in untimed {
            body.push_str("- memo: ");
            body.push_str(&line);
            body.push('\n');
        }
    }
    Ok(Some((path, body)))
}

fn segment_mode_for_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("segments"))
}

fn elapsed_line_ms(line: &str) -> Option<u64> {
    let end = line.find(']')?;
    line.starts_with('[')
        .then(|| parse_elapsed(&line[1..end]))
        .flatten()
}
fn memo_line(line: &str) -> Option<(u64, String)> {
    let end = line.find(']')?;
    let ms = parse_elapsed(line.get(1..end)?.split('~').next()?.trim())?;
    Some((ms, line.get(end + 1..)?.trim().to_string()))
}
fn parse_elapsed(value: &str) -> Option<u64> {
    let p = value.split(':').collect::<Vec<_>>();
    Some(match p.as_slice() {
        [m, s] => m.parse::<u64>().ok()? * 60_000 + s.parse::<u64>().ok()? * 1_000,
        [h, m, s] => {
            h.parse::<u64>().ok()? * 3_600_000
                + m.parse::<u64>().ok()? * 60_000
                + s.parse::<u64>().ok()? * 1_000
        }
        _ => return None,
    })
}
fn format_elapsed(ms: u64) -> String {
    let total = ms / 1_000;
    let h = total / 3_600;
    let m = (total % 3_600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn speaker_alias_from_meta(meta: Option<&legacy::SessionMeta>) -> Option<String> {
    let people = &meta?.people;
    (people.len() == 1)
        .then(|| people[0].trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn apply_speaker_alias(body: &str, alias: Option<&str>) -> String {
    let Some(alias) = alias else {
        return body.to_string();
    };
    let pattern = regex::Regex::new(r"(?m)^(\[[0-9:]+\] )other:").expect("valid alias regex");
    pattern
        .replace_all(body, |caps: &regex::Captures| {
            format!("{}{}:", &caps[1], alias)
        })
        .into_owned()
}

pub fn saved_note_path_for_session(dir: &Path, id: &str) -> Option<String> {
    let available = |value: Option<&str>| {
        value
            .filter(|p| !p.trim().is_empty() && Path::new(p).is_file())
            .map(str::to_string)
    };
    if let Ok(meta) = legacy::get_session_meta(dir, id) {
        if let Some(path) = available(meta.vault_note_path.as_deref()) {
            return Some(path);
        }
    }
    if let Ok(Some(path)) = legacy::vault_note_path_by_id(dir, id) {
        if let Some(path) = available(Some(&path)) {
            return Some(path);
        }
    }
    legacy::list_vault_notes(dir)
        .ok()?
        .into_iter()
        .find(|note| note.source_session_name.as_deref() == Some(id))
        .and_then(|note| available(Some(&note.absolute_path)))
}

fn artifact_session_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(name) = file
                .strip_suffix("_aligned.md")
                .or_else(|| file.strip_suffix("_capture_context.md"))
            else {
                continue;
            };
            names.push((
                name.to_string(),
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            ));
        }
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("artifacts")) {
        for entry in entries.flatten().filter(|e| e.path().is_dir()) {
            if let Some(name) = entry.file_name().to_str() {
                names.push((
                    name.to_string(),
                    entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                ));
            }
        }
    }
    names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter_map(|(name, _)| seen.insert(name.clone()).then_some(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    #[test]
    fn empty_segment_journal_falls_back_to_filtered_legacy_checkpoints() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        legacy::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(margins_dir.join("meet.md"), "[00:02] memo").unwrap();
        std::fs::write(
            margins_dir.join("meet_live_transcript_segments.jsonl"),
            "not-json\n",
        )
        .unwrap();
        std::fs::write(
            margins_dir.join("meet_backchannel_trace.jsonl"),
            concat!(
                "{\"kind\":\"speculative\",\"transcript\":\"[00:01] user: leaked\"}\n",
                "{\"kind\":\"live_transcript_finalized\",\"transcript\":\"[00:03] user: kept\",\"transcript_source\":\"final\"}\n"
            ),
        )
        .unwrap();

        let view = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();
        assert_eq!(
            view.source_path,
            margins_dir.join("meet_backchannel_trace.jsonl")
        );
        assert_eq!(view.view, "full");
        assert!(view.body.contains("[00:02] memo: memo"));
        assert!(view.body.contains("[00:03] user: kept"));
        assert!(!view.body.contains("leaked"));
    }

    #[test]
    fn registered_transcript_cannot_escape_its_session_artifact_root() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        let secret = work_dir.join("secret.md");
        legacy::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(&secret, "secret outside root").unwrap();
        std::fs::write(margins_dir.join("meet_aligned.md"), "safe fallback").unwrap();
        legacy::upsert_session_artifact(
            &margins_dir,
            "meet",
            legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
            0,
            &secret.to_string_lossy(),
            "durable",
            None,
        )
        .unwrap();

        let (path, body) = read_registered_or_fallback(work_dir, &margins_dir, "meet").unwrap();
        assert_eq!(path, margins_dir.join("meet_aligned.md"));
        assert_eq!(body, "safe fallback");
    }
}
