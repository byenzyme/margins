use crate::live_backchannel;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const LIVE_TRANSCRIPT_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(3);
static LIVE_TRANSCRIPT_SNAPSHOT_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct AssembledLiveTranscript {
    pub(crate) transcript: String,
    pub(crate) transcript_source: String,
    pub(crate) decoded_until_ms: u64,
    pub(crate) committed_until_ms: u64,
}

/// Compact freshness sidecar updated on every durable live-transcript append.
/// Cheap enough to read on each `get_recording_status` poll, unlike the
/// segments journal whose rows each carry a cumulative full transcript.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LiveTranscriptWatermark {
    pub(crate) decoded_until_ms: u64,
    pub(crate) committed_until_ms: u64,
    /// Wall-clock time of the last durable append. Bumps even when the
    /// watermarks hold still, because a snapshot can revise earlier text.
    pub(crate) updated_unix_ms: u64,
}

fn live_transcript_watermark_path(margins_dir: &Path, session_name: &str) -> std::path::PathBuf {
    margins_dir.join(format!("{}_live_watermark.json", session_name))
}

fn live_transcript_snapshot_path(margins_dir: &Path, session_name: &str) -> std::path::PathBuf {
    margins_dir.join(format!("{}_live_transcript_snapshot.json", session_name))
}

/// Whether the bounded latest-view snapshot should be refreshed. Freshness is
/// derived from mtime so the snapshot schema needs no wall-clock field.
pub(crate) fn live_transcript_snapshot_due(margins_dir: &Path, session_name: &str) -> bool {
    live_transcript_snapshot_path(margins_dir, session_name)
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age >= LIVE_TRANSCRIPT_SNAPSHOT_INTERVAL)
}

/// Atomically replace the latest committed transcript view. Unlike the durable
/// event journal this file is bounded: it carries only the text Watermark can
/// safely reason from and the two progress boundaries needed to qualify it.
pub(crate) fn write_live_transcript_snapshot(
    margins_dir: &Path,
    session_name: &str,
    snapshot: &live_backchannel::LiveTranscriptContext,
    terminal: bool,
) -> Result<(), String> {
    let _write_guard = LIVE_TRANSCRIPT_SNAPSHOT_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(margins_dir).map_err(|error| error.to_string())?;
    let path = live_transcript_snapshot_path(margins_dir, session_name);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(previous) = serde_json::from_str::<Value>(&raw) {
            let previous_terminal = previous
                .get("terminal")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let previous_decoded = previous
                .get("decoded_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let previous_committed = previous
                .get("committed_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if (previous_terminal && !terminal)
                || previous_decoded > snapshot.decoded_until_ms
                || previous_committed > snapshot.committed_until_ms
            {
                return Ok(());
            }
        }
    }
    let temp = path.with_extension(format!("json.{}.tmp", rand::random::<u64>()));
    let value = json!({
        "version": 1,
        "terminal": terminal,
        "decoded_until_ms": snapshot.decoded_until_ms,
        "committed_until_ms": snapshot.committed_until_ms.min(snapshot.decoded_until_ms),
        "transcript_source": snapshot.transcript_source,
        "transcript": snapshot.committed_transcript,
    });
    let raw = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::write(&temp, raw).and_then(|_| std::fs::rename(&temp, &path)) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

pub(crate) fn read_live_transcript_watermark(
    margins_dir: &Path,
    session_name: &str,
) -> Option<LiveTranscriptWatermark> {
    let raw =
        std::fs::read_to_string(live_transcript_watermark_path(margins_dir, session_name)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn update_live_transcript_watermark(
    margins_dir: &Path,
    session_name: &str,
    decoded_until_ms: u64,
    committed_until_ms: u64,
) {
    let previous = read_live_transcript_watermark(margins_dir, session_name).unwrap_or_default();
    let next = LiveTranscriptWatermark {
        // Journal rows can land slightly out of order (a periodic append can
        // race a terminal one); keep the watermarks monotonic regardless.
        decoded_until_ms: previous.decoded_until_ms.max(decoded_until_ms),
        committed_until_ms: previous.committed_until_ms.max(committed_until_ms),
        updated_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    };
    let path = live_transcript_watermark_path(margins_dir, session_name);
    let tmp = path.with_extension("json.tmp");
    let Ok(raw) = serde_json::to_string(&next) else {
        return;
    };
    // Best effort: status readers fall back to "no watermark" on failure.
    if std::fs::write(&tmp, raw).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub(crate) fn append_live_transcript_segment(
    margins_dir: &Path,
    session_name: &str,
    kind: &str,
    memo_index: Option<usize>,
    memo_time: Option<&str>,
    snapshot: &live_backchannel::LiveTranscriptContext,
) -> Result<(), String> {
    std::fs::create_dir_all(margins_dir).map_err(|e| e.to_string())?;
    // The append-only journal remains the crash/reconstruction history. The
    // replaceable snapshot serves latency-sensitive readers without making
    // that history grow with every rolling update.
    let _ = write_live_transcript_snapshot(margins_dir, session_name, snapshot, kind == "final");
    let path = margins_dir.join(format!("{}_live_transcript_segments.jsonl", session_name));
    let event = json!({
        "schema": 1,
        "kind": kind,
        "memo_index": memo_index,
        "memo_time": memo_time,
        "start_ms": snapshot.previous_memo_checkpoint_ms,
        "decoded_until_ms": snapshot.decoded_until_ms,
        "committed_until_ms": snapshot.committed_until_ms,
        "transcript_source": snapshot.transcript_source,
        "mic_accepted_samples": snapshot.mic_accepted_samples,
        "system_accepted_samples": snapshot.system_accepted_samples,
        "mic_dropped_samples": snapshot.mic_dropped_samples,
        "system_dropped_samples": snapshot.system_dropped_samples,
        "segment_transcript": snapshot.new_transcript,
        "full_transcript": snapshot.transcript,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())
    )
    .map_err(|e| e.to_string())?;
    update_live_transcript_watermark(
        margins_dir,
        session_name,
        snapshot.decoded_until_ms,
        snapshot.committed_until_ms,
    );
    Ok(())
}

pub(crate) fn assembled_live_transcript(
    margins_dir: &Path,
    session_name: &str,
) -> Result<Option<AssembledLiveTranscript>, String> {
    let events = live_transcript_segment_events(margins_dir, session_name)?;
    if !events.is_empty() {
        return Ok(assemble_live_transcript_from_segment_events(&events));
    }
    let legacy_events = legacy_live_transcript_events(margins_dir, session_name)?;
    Ok(assemble_live_transcript_from_legacy_events(&legacy_events))
}

fn live_transcript_segment_events(
    margins_dir: &Path,
    session_name: &str,
) -> Result<Vec<Value>, String> {
    let path = margins_dir.join(format!("{}_live_transcript_segments.jsonl", session_name));
    read_jsonl_events(&path)
}

fn legacy_live_transcript_events(
    margins_dir: &Path,
    session_name: &str,
) -> Result<Vec<Value>, String> {
    let trace_path = margins_dir.join(format!("{}_backchannel_trace.jsonl", session_name));
    Ok(read_jsonl_events(&trace_path)?
        .into_iter()
        .filter(|event| {
            matches!(
                event.get("kind").and_then(Value::as_str),
                Some("memo_transcript_checkpoint") | Some("live_transcript_finalized")
            )
        })
        .collect())
}

fn read_jsonl_events(path: &Path) -> Result<Vec<Value>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    Ok(raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect())
}

fn assemble_live_transcript_from_segment_events(
    events: &[Value],
) -> Option<AssembledLiveTranscript> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut next_seq = 0usize;
    let mut transcript_source = "live_segments".to_string();
    let mut decoded_until_ms = 0u64;
    let mut committed_until_ms = 0u64;

    // A periodic worker can finish decoding just before Finish but append its
    // journal row just after the terminal row. Fold that terminal snapshot last
    // so its decoder revisions still win within its window.
    let terminal_index = events.iter().rposition(|event| {
        event.get("kind").and_then(Value::as_str) == Some("final")
            && event
                .get("full_transcript")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
    });
    let ordered_events = events
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != terminal_index)
        .map(|(_, event)| event)
        .chain(terminal_index.map(|index| &events[index]));

    for event in ordered_events {
        let full_lines = event
            .get("full_transcript")
            .and_then(Value::as_str)
            .into_iter()
            .flat_map(str::lines)
            .map(str::trim)
            .filter_map(|line| parse_context_line_ms(line).map(|ms| (ms, line.to_string())))
            .collect::<Vec<_>>();
        let segment = event
            .get("segment_transcript")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if full_lines.is_empty() && segment.is_empty() {
            continue;
        }
        if let Some(source) = event.get("transcript_source").and_then(Value::as_str) {
            if !source.trim().is_empty() {
                transcript_source = source.to_string();
            }
        } else if !full_lines.is_empty() {
            transcript_source = "live_snapshot".to_string();
        }
        let start_ms = event.get("start_ms").and_then(Value::as_u64).unwrap_or(0);
        let event_decoded_until_ms = event
            .get("decoded_until_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        decoded_until_ms = decoded_until_ms.max(event_decoded_until_ms);
        committed_until_ms = committed_until_ms.max(
            event
                .get("committed_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );

        if let Some((window_start_ms, latest_line_ms)) = full_lines
            .iter()
            .map(|(ms, _)| *ms)
            .min()
            .zip(full_lines.iter().map(|(ms, _)| *ms).max())
        {
            // Live decoder snapshots are bounded sliding windows, not cumulative
            // transcripts. Preserve the older prefix while replacing the
            // overlapping window so later hypothesis revisions remain canonical.
            let window_end_ms = latest_line_ms.max(event_decoded_until_ms);
            entries.retain(|(ms, _, _)| *ms < window_start_ms || *ms > window_end_ms);
            for (ms, line) in full_lines {
                seen.insert(line.clone());
                entries.push((ms, next_seq, line));
                next_seq += 1;
            }
        } else {
            append_segment_transcript_lines(
                &mut entries,
                &mut seen,
                &mut next_seq,
                start_ms,
                segment,
            );
        }
    }

    if entries.is_empty() {
        return None;
    }
    entries.sort_by_key(|(start_ms, seq, _)| (*start_ms, *seq));
    Some(AssembledLiveTranscript {
        transcript: entries
            .into_iter()
            .map(|(_, _, segment)| segment)
            .collect::<Vec<_>>()
            .join("\n"),
        transcript_source,
        decoded_until_ms,
        committed_until_ms,
    })
}

fn append_segment_transcript_lines(
    entries: &mut Vec<(u64, usize, String)>,
    seen: &mut HashSet<String>,
    next_seq: &mut usize,
    start_ms: u64,
    transcript: &str,
) {
    for line in transcript.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        let ms = parse_context_line_ms(trimmed).unwrap_or(start_ms);
        entries.push((ms, *next_seq, trimmed.to_string()));
        *next_seq += 1;
    }
}

fn assemble_live_transcript_from_legacy_events(
    events: &[Value],
) -> Option<AssembledLiveTranscript> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut next_seq = 0usize;
    let mut transcript_source = "legacy_live_trace".to_string();
    let mut decoded_until_ms = 0u64;
    let mut committed_until_ms = 0u64;

    for event in events {
        if let Some(source) = event.get("transcript_source").and_then(Value::as_str) {
            if !source.trim().is_empty() {
                transcript_source = source.to_string();
            }
        }
        decoded_until_ms = decoded_until_ms.max(
            event
                .get("decoded_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        committed_until_ms = committed_until_ms.max(
            event
                .get("committed_until_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );

        for key in [
            "new_transcript",
            "previous_transcript",
            "full_transcript",
            "transcript",
        ] {
            if let Some(transcript) = event.get(key).and_then(Value::as_str) {
                append_legacy_transcript_lines(&mut entries, &mut seen, &mut next_seq, transcript);
            }
        }
    }

    if entries.is_empty() {
        return None;
    }
    entries.sort_by_key(|(ms, seq, _)| (*ms, *seq));
    Some(AssembledLiveTranscript {
        transcript: entries
            .into_iter()
            .map(|(_, _, line)| line)
            .collect::<Vec<_>>()
            .join("\n"),
        transcript_source,
        decoded_until_ms,
        committed_until_ms,
    })
}

fn append_legacy_transcript_lines(
    entries: &mut Vec<(u64, usize, String)>,
    seen: &mut HashSet<String>,
    next_seq: &mut usize,
    transcript: &str,
) {
    for line in transcript.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        let Some(ms) = parse_context_line_ms(trimmed) else {
            continue;
        };
        entries.push((ms, *next_seq, trimmed.to_string()));
        *next_seq += 1;
    }
}

fn parse_context_line_ms(line: &str) -> Option<u64> {
    let end = line.find(']')?;
    if !line.starts_with('[') || end <= 1 {
        return None;
    }
    parse_elapsed_to_ms(&line[1..end])
}

fn parse_elapsed_to_ms(value: &str) -> Option<u64> {
    let parts = value.split(':').collect::<Vec<_>>();
    let seconds = match parts.as_slice() {
        [m, s] => m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?,
        [h, m, s] => {
            h.parse::<u64>().ok()? * 3600 + m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(seconds * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        decoded_until_ms: u64,
        committed_until_ms: u64,
    ) -> live_backchannel::LiveTranscriptContext {
        live_backchannel::LiveTranscriptContext {
            transcript: "[00:01] user: hello".to_string(),
            previous_transcript: String::new(),
            new_transcript: "[00:01] user: hello".to_string(),
            committed_transcript: "[00:01] user: hello".to_string(),
            hypothesis_transcript: String::new(),
            transcript_source: "committed".to_string(),
            previous_memo_checkpoint_ms: 0,
            decoded_until_ms,
            committed_until_ms,
            mic_accepted_samples: 0,
            system_accepted_samples: 0,
            mic_dropped_samples: 0,
            system_dropped_samples: 0,
            timing: None,
        }
    }

    #[test]
    fn rolling_snapshot_is_bounded_to_committed_text_and_progress() {
        let dir = std::env::temp_dir().join(format!(
            "margins-live-snapshot-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(live_transcript_snapshot_due(&dir, "s1"));

        let mut context = snapshot(4_000, 3_000);
        context.transcript = "[00:01] user: hello\n[00:04] user: unstable".to_string();
        context.hypothesis_transcript = "[00:04] user: unstable".to_string();
        write_live_transcript_snapshot(&dir, "s1", &context, false).unwrap();

        let path = dir.join("s1_live_transcript_snapshot.json");
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["terminal"], false);
        assert_eq!(value["decoded_until_ms"], 4_000);
        assert_eq!(value["committed_until_ms"], 3_000);
        assert_eq!(value["transcript"], "[00:01] user: hello");
        assert!(value.get("updated_unix_ms").is_none());
        assert!(!live_transcript_snapshot_due(&dir, "s1"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durable_append_updates_monotonic_watermark_sidecar() {
        let dir =
            std::env::temp_dir().join(format!("margins-watermark-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(read_live_transcript_watermark(&dir, "s1").is_none());

        append_live_transcript_segment(&dir, "s1", "checkpoint", None, None, &snapshot(4000, 3000))
            .unwrap();
        let first = read_live_transcript_watermark(&dir, "s1").unwrap();
        assert_eq!(first.decoded_until_ms, 4000);
        assert_eq!(first.committed_until_ms, 3000);
        assert!(first.updated_unix_ms > 0);

        // A late out-of-order append must not regress the watermarks.
        append_live_transcript_segment(&dir, "s1", "periodic", None, None, &snapshot(2000, 1000))
            .unwrap();
        let second = read_live_transcript_watermark(&dir, "s1").unwrap();
        assert_eq!(second.decoded_until_ms, 4000);
        assert_eq!(second.committed_until_ms, 3000);

        append_live_transcript_segment(&dir, "s1", "final", None, None, &snapshot(9000, 9000))
            .unwrap();
        let third = read_live_transcript_watermark(&dir, "s1").unwrap();
        assert_eq!(third.committed_until_ms, 9000);

        append_live_transcript_segment(&dir, "s1", "periodic", None, None, &snapshot(7000, 6000))
            .unwrap();
        let latest: Value = serde_json::from_slice(
            &std::fs::read(dir.join("s1_live_transcript_snapshot.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(latest["terminal"], true);
        assert_eq!(latest["decoded_until_ms"], 9000);
    }

    #[test]
    fn segment_assembly_dedupes_replayed_live_lines() {
        let events = vec![
            json!({
                "segment_transcript": "[00:01] user: hello\n[00:03] user: repeated",
                "transcript_source": "committed",
                "decoded_until_ms": 3000,
                "committed_until_ms": 3000,
            }),
            json!({
                "segment_transcript": "[00:03] user: repeated\n[00:05] user: later",
                "transcript_source": "committed",
                "decoded_until_ms": 5000,
                "committed_until_ms": 5000,
            }),
        ];

        let assembled = assemble_live_transcript_from_segment_events(&events).unwrap();

        assert_eq!(
            assembled.transcript,
            "[00:01] user: hello\n[00:03] user: repeated\n[00:05] user: later"
        );
        assert_eq!(assembled.transcript_source, "committed");
        assert_eq!(assembled.decoded_until_ms, 5000);
        assert_eq!(assembled.committed_until_ms, 5000);
    }

    #[test]
    fn latest_full_snapshot_replaces_revised_hypothesis() {
        let events = vec![
            json!({
                "segment_transcript": "[00:01] user: wreck a nice beach",
                "full_transcript": "[00:01] user: wreck a nice beach",
                "decoded_until_ms": 2000,
                "committed_until_ms": 1000,
            }),
            json!({
                "segment_transcript": "[00:01] user: recognize speech",
                "full_transcript": "[00:01] user: recognize speech",
                "decoded_until_ms": 3000,
                "committed_until_ms": 3000,
            }),
        ];

        let assembled = assemble_live_transcript_from_segment_events(&events).unwrap();
        assert_eq!(assembled.transcript, "[00:01] user: recognize speech");
        assert!(!assembled.transcript.contains("wreck a nice beach"));
    }

    #[test]
    fn sliding_window_snapshots_preserve_prefix_and_replace_overlap() {
        let events = vec![
            json!({
                "full_transcript": concat!(
                    "[00:00] user: opening\n",
                    "[01:30] user: old overlap wording\n",
                    "[02:00] assistant: first window tail"
                ),
                "decoded_until_ms": 120_000,
                "committed_until_ms": 120_000,
            }),
            json!({
                "full_transcript": concat!(
                    "[01:30] user: revised overlap wording\n",
                    "[02:00] assistant: revised first window tail\n",
                    "[03:00] user: later\n",
                    "[04:00] assistant: latest"
                ),
                "decoded_until_ms": 240_000,
                "committed_until_ms": 240_000,
            }),
        ];

        let assembled = assemble_live_transcript_from_segment_events(&events).unwrap();

        assert_eq!(
            assembled.transcript,
            concat!(
                "[00:00] user: opening\n",
                "[01:30] user: revised overlap wording\n",
                "[02:00] assistant: revised first window tail\n",
                "[03:00] user: later\n",
                "[04:00] assistant: latest"
            )
        );
        assert!(!assembled.transcript.contains("old overlap wording"));
    }

    #[test]
    fn terminal_sliding_window_does_not_drop_earlier_prefix() {
        let events = vec![
            json!({
                "kind": "checkpoint",
                "full_transcript": "[00:00] user: meeting starts\n[20:00] assistant: middle",
                "decoded_until_ms": 1_200_000,
                "committed_until_ms": 1_200_000,
            }),
            json!({
                "kind": "checkpoint",
                "full_transcript": "[20:00] assistant: revised middle\n[42:11] user: old tail",
                "decoded_until_ms": 2_531_000,
                "committed_until_ms": 2_531_000,
            }),
            json!({
                "kind": "final",
                "full_transcript": "[42:11] user: final tail\n[45:00] assistant: meeting ends",
                "decoded_until_ms": 2_700_000,
                "committed_until_ms": 2_700_000,
            }),
        ];

        let assembled = assemble_live_transcript_from_segment_events(&events).unwrap();

        assert_eq!(
            assembled.transcript,
            concat!(
                "[00:00] user: meeting starts\n",
                "[20:00] assistant: revised middle\n",
                "[42:11] user: final tail\n",
                "[45:00] assistant: meeting ends"
            )
        );
        assert_eq!(assembled.committed_until_ms, 2_700_000);
    }

    #[test]
    fn terminal_snapshot_wins_over_late_periodic_append() {
        let events = vec![
            json!({
                "kind": "final",
                "full_transcript": "[00:01] user: final stable words",
                "decoded_until_ms": 3000,
                "committed_until_ms": 3000,
            }),
            json!({
                "kind": "periodic",
                "full_transcript": "[00:01] user: stale hypoth",
                "decoded_until_ms": 2800,
                "committed_until_ms": 2000,
            }),
        ];

        let assembled = assemble_live_transcript_from_segment_events(&events).unwrap();
        assert_eq!(assembled.transcript, "[00:01] user: final stable words");
        assert_eq!(assembled.committed_until_ms, 3000);
    }
}
