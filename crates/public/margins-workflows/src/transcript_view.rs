//! Portable transcript artifact selection and rendering inputs.

use crate::artifacts::{artifact_registry_disk_path, confined_session_artifact_access_disk_path};
use anyhow::{bail, Context, Result};
use margins_store::canonical;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptView {
    pub session_name: String,
    pub source_path: PathBuf,
    pub body: String,
    pub view: &'static str,
    pub decoded_until_ms: u64,
    pub committed_until_ms: u64,
    /// Filesystem-derived snapshot time. This is deliberately not persisted in
    /// the transcript schema: the source artifact's mtime is the freshness
    /// signal.
    pub updated_at_unix_ms: u64,
    pub live: bool,
    pub terminal: bool,
    pub speaker_alias: Option<String>,
    pub started_at: String,
    pub created_at: String,
    pub title: String,
    pub calendar_event: String,
    pub people: Vec<String>,
    pub memo_path: String,
    pub saved_note_path: Option<String>,
}

#[derive(Debug)]
struct TranscriptSource {
    source_path: PathBuf,
    body: String,
    decoded_until_ms: u64,
    committed_until_ms: u64,
    terminal: bool,
    live_checkpoint: bool,
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
    if let Some(path) = canonical::list_session_artifacts(margins_dir, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT)
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
    freshest_live_transcript_path(margins_dir, name)
}

pub fn resolve_session_name(margins_dir: &Path, requested: &str) -> Result<String> {
    if requested != "latest" {
        return Ok(requested.to_string());
    }
    if let Some(name) = canonical::list_sessions(margins_dir)?
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
    let meta = canonical::get_session_meta(margins_dir, &name).ok();
    let final_path = transcript_artifact_path(margins_dir, &name);
    let final_source = if meta
        .as_ref()
        .and_then(|value| value.processing_state.as_deref())
        == Some("done")
    {
        std::fs::read_to_string(&final_path)
            .ok()
            .filter(|body| !body.trim().is_empty())
            .map(|body| TranscriptSource {
                source_path: final_path.clone(),
                body,
                decoded_until_ms: 0,
                committed_until_ms: 0,
                terminal: true,
                live_checkpoint: false,
            })
    } else {
        None
    };
    let (source, view) = if let Some(source) = final_source {
        (source, "aligned")
    } else if let Some(source) =
        read_remote_live_checkpoint(work_dir, margins_dir, &name, meta.as_ref())?
    {
        (source, "full")
    } else if let Some(source) = read_freshest_live_transcript(work_dir, margins_dir, &name)? {
        (source, "full")
    } else if let Some(source) =
        read_terminal_checkpoint_body(work_dir, margins_dir, &name, meta.as_ref())?
    {
        (source, "full")
    } else {
        let (path, body) = read_registered_or_fallback(work_dir, margins_dir, &name)?;
        (
            TranscriptSource {
                source_path: path,
                body,
                decoded_until_ms: 0,
                committed_until_ms: 0,
                terminal: true,
                live_checkpoint: false,
            },
            "aligned",
        )
    };
    // The public store has no process-level capture status. A current session
    // with a non-terminal live source is the strongest honest available signal.
    let live =
        source.live_checkpoint && !source.terminal && current_session_matches(margins_dir, &name);
    let speaker_alias = speaker_alias_from_meta(meta.as_ref());
    let body = if source.source_path == final_path {
        meta.as_ref()
            .and_then(|meta| {
                let path = artifact_registry_disk_path(work_dir, margins_dir, &meta.notes_path);
                std::fs::read_to_string(path)
                    .ok()
                    .and_then(|memo| render_remote_offline_transcript(&source.body, &memo))
            })
            .unwrap_or_else(|| source.body.clone())
    } else {
        source.body.clone()
    };
    let body = apply_speaker_alias(&body, speaker_alias.as_deref());
    let saved_note_path = saved_note_path_for_session(margins_dir, &name);
    let updated_at_unix_ms = source_modified_unix_ms(&source.source_path);
    Ok(TranscriptView {
        session_name: name,
        source_path: source.source_path,
        body,
        view,
        decoded_until_ms: source.decoded_until_ms,
        committed_until_ms: source.committed_until_ms,
        updated_at_unix_ms,
        live,
        terminal: source.terminal,
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

/// Remote ASR artifacts retain the spoken timeline. Rebuild their readable
/// view with the current memo so bb and `margins transcript` share the same
/// phrase grouping and memo placement, including for older word-per-row files.
fn render_remote_offline_transcript(body: &str, memo: &str) -> Option<String> {
    let (header, original_timeline) = body.split_once("## Timeline")?;
    if !header
        .contains("Source: Margins remote offline Parakeet TDT ONNX transcript and memo context.")
    {
        return None;
    }
    let mut channels = HashMap::<String, u32>::new();
    let mut labels = BTreeMap::<u32, String>::new();
    let mut words = Vec::new();
    for line in original_timeline.lines() {
        let Some(ms) = elapsed_line_ms(line) else {
            continue;
        };
        let rest = line.split_once(']')?.1.trim();
        let (speaker, text) = rest.split_once(':')?;
        if speaker.trim() == "memo" {
            continue;
        }
        let speaker = speaker.trim();
        let channel = match channels.get(speaker) {
            Some(channel) => *channel,
            None => {
                let channel = channels.len() as u32;
                channels.insert(speaker.to_string(), channel);
                labels.insert(channel, speaker.to_string());
                channel
            }
        };
        words.extend(margins_media::transcript::words_to_transcript_entries(
            &[margins_media::transcript::WordTiming {
                start_ms: ms,
                end_ms: ms,
                text: text.trim().to_string(),
            }],
            channel,
            0,
        ));
    }
    words.sort_by_key(|entry| (entry.start_ms, entry.channel));
    let mut spoken = words.into_iter().peekable();
    let mut rows = Vec::new();
    let mut timed_memos = Vec::new();
    let mut untimed = Vec::new();
    for (seq, line) in memo.lines().map(str::trim).enumerate() {
        if line.is_empty() || line == "---" || line.starts_with('#') {
            continue;
        }
        if let Some((ms, text)) = memo_line(line) {
            timed_memos.push((ms, seq, text));
        } else {
            untimed.push(line);
        }
    }
    timed_memos.sort_by_key(|row| (row.0, row.1));
    for (ms, seq, text) in timed_memos {
        let mut window = Vec::new();
        while spoken.peek().is_some_and(|entry| entry.start_ms <= ms) {
            window.push(spoken.next().expect("peeked spoken entry"));
        }
        append_spoken_rows(&mut rows, window, &labels);
        rows.push((ms, 1, seq, format!("[{}] memo: {text}", format_elapsed(ms))));
    }
    append_spoken_rows(&mut rows, spoken.collect(), &labels);
    let mut rendered = format!("{header}## Timeline\n\n");
    if rows.is_empty() {
        rendered.push_str("_No timestamped transcript or memo entries were available._\n");
    } else {
        for (_, _, _, line) in rows {
            rendered.push_str(&line);
            rendered.push('\n');
        }
    }
    if !untimed.is_empty() {
        rendered.push_str("\n## Untimed memo / reflection lines\n\n");
        for line in untimed {
            rendered.push_str("- memo: ");
            rendered.push_str(line);
            rendered.push('\n');
        }
    }
    Some(rendered)
}

fn append_spoken_rows(
    rows: &mut Vec<(u64, u8, usize, String)>,
    entries: Vec<margins_media::transcript::TranscriptWordEntry>,
    labels: &BTreeMap<u32, String>,
) {
    for entry in margins_media::transcript::merge_word_entries_to_phrases(entries, 2_000) {
        rows.push((
            entry.start_ms,
            0,
            rows.len(),
            format!(
                "[{}] {}: {}",
                format_elapsed(entry.start_ms),
                labels[&entry.channel],
                entry.text.trim()
            ),
        ));
    }
}

fn read_terminal_checkpoint_body(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: Option<&canonical::SessionMeta>,
) -> Result<Option<TranscriptSource>> {
    let checkpoint = meta
        .and_then(|meta| {
            meta.segments
                .iter()
                .filter_map(|segment| {
                    let path = margins_dir.join(format!(
                        "{name}_seg{}.live-transcript.json",
                        segment.segment_index
                    ));
                    path.is_file().then_some((segment.segment_index, path))
                })
                .max_by_key(|(ordinal, _)| *ordinal)
                .map(|(_, path)| path)
        })
        .or_else(|| {
            canonical::list_session_artifacts(margins_dir, name)
                .unwrap_or_default()
                .into_iter()
                .filter(|artifact| artifact.kind == canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT)
                .filter(|artifact| artifact.path.ends_with(".live-transcript.json"))
                .find_map(|artifact| {
                    confined_session_artifact_access_disk_path(margins_dir, name, &artifact.path)
                })
        });
    let Some(path) = checkpoint else {
        return Ok(None);
    };
    read_checkpoint_body_at(work_dir, margins_dir, name, meta, path, false)
}

fn read_remote_live_checkpoint(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: Option<&canonical::SessionMeta>,
) -> Result<Option<TranscriptSource>> {
    let path = margins_dir.join(format!("{name}_remote.live-transcript.json"));
    if !path.is_file() {
        return Ok(None);
    }
    read_checkpoint_body_at(work_dir, margins_dir, name, meta, path, true)
}

fn read_checkpoint_body_at(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: Option<&canonical::SessionMeta>,
    path: PathBuf,
    provisional: bool,
) -> Result<Option<TranscriptSource>> {
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let checkpoint: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let decoded_until_ms = checkpoint
        .get("decoded_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // Compact CLI checkpoints contain committed words only. Older checkpoint
    // schemas did not spell out their commit watermark, so decoded progress is
    // the honest compatibility default for those artifacts.
    let committed_until_ms = checkpoint
        .get("committed_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(decoded_until_ms)
        .min(decoded_until_ms);
    let terminal = !provisional
        && checkpoint
            .get("terminal")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let entries = margins_media::transcript::merge_word_entries_to_phrases(
        crate::processing::read_transcript_entries(&path)?,
        2_000,
    );
    let Some(meta) = meta else {
        return Ok(None);
    };
    let started_at = canonical::get_session_start_time(margins_dir, name)?;
    let memo_path = artifact_registry_disk_path(work_dir, margins_dir, &meta.notes_path);
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    let body = crate::alignment::render_aligned_markdown(name, &started_at, &memo, &entries);
    Ok(Some(TranscriptSource {
        source_path: path,
        body,
        decoded_until_ms,
        committed_until_ms,
        terminal,
        live_checkpoint: true,
    }))
}

pub fn read_registered_or_fallback(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
) -> Result<(PathBuf, String)> {
    let registered = canonical::list_session_artifacts(margins_dir, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT)
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
    if let Some(source) = read_freshest_live_transcript(work_dir, margins_dir, name)? {
        return Ok((source.source_path, source.body));
    }
    bail!("No aligned transcript or capture context found for '{name}'.")
}

fn freshest_live_transcript_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let snapshot = dir.join(format!("{name}_live_transcript_snapshot.json"));
    let journal = live_transcript_journal_path(dir, name);
    std::iter::once(snapshot)
        .chain(journal)
        .filter(|path| path.is_file())
        .max_by_key(|path| source_modified_unix_ms(path))
}

fn live_transcript_journal_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let segments = dir.join(format!("{name}_live_transcript_segments.jsonl"));
    if jsonl_has_event(&segments) {
        return Some(segments);
    }
    let legacy = dir.join(format!("{name}_backchannel_trace.jsonl"));
    jsonl_has_event(&legacy).then_some(legacy)
}

fn read_freshest_live_transcript(
    work_dir: &Path,
    dir: &Path,
    name: &str,
) -> Result<Option<TranscriptSource>> {
    let snapshot = read_live_transcript_snapshot(work_dir, dir, name)?;
    let journal = read_live_transcript_body(work_dir, dir, name)?;
    Ok(match (snapshot, journal) {
        (Some(snapshot), Some(journal)) => {
            if source_modified_unix_ms(&snapshot.source_path)
                >= source_modified_unix_ms(&journal.source_path)
            {
                Some(snapshot)
            } else {
                Some(journal)
            }
        }
        (snapshot, journal) => snapshot.or(journal),
    })
}

fn read_live_transcript_snapshot(
    work_dir: &Path,
    dir: &Path,
    name: &str,
) -> Result<Option<TranscriptSource>> {
    let path = dir.join(format!("{name}_live_transcript_snapshot.json"));
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let Ok(snapshot) = serde_json::from_str::<Value>(&raw) else {
        // A replaceable live view is opportunistic. Ignore an unreadable
        // snapshot and fall back to the durable journal/checkpoint chain.
        return Ok(None);
    };
    let decoded_until_ms = snapshot
        .get("decoded_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let committed_until_ms = snapshot
        .get("committed_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(decoded_until_ms)
        .min(decoded_until_ms);
    let source = snapshot
        .get("transcript_source")
        .and_then(Value::as_str)
        .filter(|source| !source.trim().is_empty())
        .unwrap_or("live_snapshot");
    let transcript = snapshot
        .get("transcript")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let memo = read_live_memo(work_dir, dir, name);
    let timeline = transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(seq, line)| {
            (
                elapsed_line_ms(line).unwrap_or(committed_until_ms),
                seq,
                line.to_string(),
            )
        })
        .collect();
    Ok(Some(TranscriptSource {
        source_path: path,
        body: render_live_capture_context(
            name,
            source,
            decoded_until_ms,
            committed_until_ms,
            timeline,
            &memo,
        ),
        decoded_until_ms,
        committed_until_ms,
        terminal: snapshot
            .get("terminal")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        live_checkpoint: true,
    }))
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
) -> Result<Option<TranscriptSource>> {
    let Some(path) = live_transcript_journal_path(dir, name) else {
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
    let terminal = events
        .iter()
        .rev()
        .find_map(checkpoint_event_terminal)
        .unwrap_or(false);
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
    let memo = read_live_memo(work_dir, dir, name);
    let body = render_live_capture_context(name, &source, decoded, committed, timeline, &memo);
    Ok(Some(TranscriptSource {
        source_path: path,
        body,
        decoded_until_ms: decoded,
        committed_until_ms: committed.min(decoded),
        terminal,
        live_checkpoint: true,
    }))
}

fn read_live_memo(work_dir: &Path, dir: &Path, name: &str) -> String {
    canonical::get_session_meta(dir, name)
        .ok()
        .and_then(|meta| {
            let path = PathBuf::from(meta.notes_path);
            std::fs::read_to_string(if path.is_absolute() {
                path
            } else {
                work_dir.join(path)
            })
            .ok()
        })
        .unwrap_or_default()
}

fn render_live_capture_context(
    name: &str,
    source: &str,
    decoded: u64,
    committed: u64,
    timeline: Vec<(u64, usize, String)>,
    memo: &str,
) -> String {
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
    body
}

fn source_modified_unix_ms(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn segment_mode_for_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("segments"))
}

fn checkpoint_event_terminal(event: &Value) -> Option<bool> {
    event.get("terminal").and_then(Value::as_bool).or_else(|| {
        match event.get("kind").and_then(Value::as_str) {
            Some("live_transcript_finalized") => Some(true),
            Some("memo_transcript_checkpoint") => Some(false),
            _ => None,
        }
    })
}

fn current_session_matches(dir: &Path, name: &str) -> bool {
    std::fs::read_to_string(dir.join("current")).is_ok_and(|current| current.trim() == name)
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

pub fn speaker_alias_from_meta(meta: Option<&canonical::SessionMeta>) -> Option<String> {
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
    if let Ok(meta) = canonical::get_session_meta(dir, id) {
        if let Some(path) = available(meta.vault_note_path.as_deref()) {
            return Some(path);
        }
    }
    if let Ok(Some(path)) = canonical::vault_note_path_by_id(dir, id) {
        if let Some(path) = available(Some(&path)) {
            return Some(path);
        }
    }
    canonical::list_vault_notes(dir)
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
    fn completed_remote_transcript_groups_words_and_interleaves_current_memo() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(
            margins_dir.join("meet.md"),
            "[00:02] Current memo\nA later reflection",
        )
        .unwrap();
        let final_path = transcript_artifact_path(&margins_dir, "meet");
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        std::fs::write(&final_path, "# Transcript\n\nSession: `meet`\nSource: Margins remote offline Parakeet TDT ONNX transcript and memo context.\nTranscript source: `offline`\n\n## Timeline\n\n[00:01] you (mic): Nice\n[00:02] you (mic): .\n[00:02] memo: Stale memo\n[00:03] you (mic): To\n[00:03] you (mic): find\n[00:04] you (mic): the\n[00:04] you (mic): recording\n[00:05] you (mic): suite\n[00:05] you (mic): .\n").unwrap();
        let job =
            canonical::begin_processing_job(&margins_dir, "meet", "job-1", "transcribe", "input-1")
                .unwrap();
        canonical::update_processing_job(
            &margins_dir,
            "job-1",
            job.attempt,
            "complete",
            Some(1.0),
            None,
            None,
            None,
        )
        .unwrap();

        let view = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();
        assert!(view.body.contains("[00:01] you (mic): Nice."));
        assert!(view
            .body
            .contains("[00:03] you (mic): To find the recording suite."));
        assert!(view.body.find("Nice.").unwrap() < view.body.find("Current memo").unwrap());
        assert!(view.body.find("Current memo").unwrap() < view.body.find("To find").unwrap());
        assert!(view.body.contains("- memo: A later reflection"));
        assert!(!view.body.contains("Stale memo"));
    }

    #[test]
    fn provisional_remote_words_yield_to_completed_final_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(margins_dir.join("meet.md"), "").unwrap();
        std::fs::write(
            margins_dir.join("meet_remote.live-transcript.json"),
            serde_json::json!({
                "version":2,"terminal":true,"decoded_until_ms":1000,"committed_until_ms":900,
                "transcripts":[{"words":[{"channel":0,"start_ms":100,"end_ms":500,"text":" provisional"}]}]
            }).to_string(),
        ).unwrap();
        let interim = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();
        assert!(interim.body.contains("provisional"));
        assert!(!interim.terminal);

        let final_path = transcript_artifact_path(&margins_dir, "meet");
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        std::fs::write(&final_path, "# Final\n\nrecognized remotely\n").unwrap();
        let job =
            canonical::begin_processing_job(&margins_dir, "meet", "job-1", "transcribe", "input-1")
                .unwrap();
        canonical::update_processing_job(
            &margins_dir,
            "job-1",
            job.attempt,
            "complete",
            Some(1.0),
            None,
            None,
            None,
        )
        .unwrap();
        let final_view = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();
        assert_eq!(final_view.view, "aligned");
        assert!(final_view.terminal);
        assert!(final_view.body.contains("recognized remotely"));
        assert!(!final_view.body.contains("provisional"));
    }

    #[test]
    fn empty_segment_journal_falls_back_to_filtered_legacy_checkpoints() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
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
    fn live_segment_view_exposes_only_committed_progress_and_derived_freshness() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(margins_dir.join("meet.md"), "[00:02] memo").unwrap();
        std::fs::write(margins_dir.join("current"), "meet\n").unwrap();
        std::fs::write(
            margins_dir.join("meet_live_transcript_segments.jsonl"),
            concat!(
                "{\"kind\":\"checkpoint\",\"start_ms\":0,",
                "\"decoded_until_ms\":5000,\"committed_until_ms\":3000,",
                "\"segment_transcript\":\"[00:01] user: hello\"}\n"
            ),
        )
        .unwrap();

        let view = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();

        assert_eq!(view.decoded_until_ms, 5_000);
        assert_eq!(view.committed_until_ms, 3_000);
        assert!(view.updated_at_unix_ms > 0);
        assert!(view.live);
        assert!(!view.terminal);
    }

    #[test]
    fn bounded_live_snapshot_is_a_complete_transcript_view_without_a_journal() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(margins_dir.join("meet.md"), "[00:02] memo").unwrap();
        std::fs::write(margins_dir.join("current"), "meet\n").unwrap();
        std::fs::write(
            margins_dir.join("meet_live_transcript_snapshot.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "terminal": false,
                "decoded_until_ms": 5_000,
                "committed_until_ms": 3_000,
                "transcript_source": "committed",
                "transcript": "[00:01] user: stable words"
            }))
            .unwrap(),
        )
        .unwrap();

        let view = load_transcript_view(work_dir, &margins_dir, "meet").unwrap();

        assert_eq!(
            view.source_path,
            margins_dir.join("meet_live_transcript_snapshot.json")
        );
        assert_eq!(view.decoded_until_ms, 5_000);
        assert_eq!(view.committed_until_ms, 3_000);
        assert!(view.body.contains("[00:01] user: stable words"));
        assert!(view.body.contains("[00:02] memo: memo"));
        assert!(view.live);
    }

    #[test]
    fn registered_transcript_cannot_escape_its_session_artifact_root() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path();
        let margins_dir = work_dir.join(".margins");
        let secret = work_dir.join("secret.md");
        canonical::create_session(&margins_dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
        std::fs::write(&secret, "secret outside root").unwrap();
        std::fs::write(margins_dir.join("meet_aligned.md"), "safe fallback").unwrap();
        canonical::upsert_session_artifact(
            &margins_dir,
            "meet",
            canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
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
