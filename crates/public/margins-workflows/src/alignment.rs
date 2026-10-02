use chrono::{DateTime, Local};
use margins_media::transcript::{merge_word_entries_to_phrases, TranscriptWordEntry};

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
    Transcript(TranscriptWordEntry),
    Memo(TimedMemo),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedMemo {
    pub at_ms: u64,
    pub edited_at_ms: Option<u64>,
    pub text: String,
}

/// Group words into phrases within memo-bounded windows, then place each memo
/// after the speech it bookmarked. Equal timestamps belong before the memo.
pub fn interleave_timeline(
    words: &[TranscriptWordEntry],
    memos: &[TimedMemo],
    max_gap_ms: u64,
) -> Vec<TimelineEvent> {
    let mut words = words.to_vec();
    for word in &mut words {
        let Some(first) = word.text.chars().next() else {
            continue;
        };
        if !first.is_whitespace() && !matches!(first, '.' | ',' | '!' | '?' | ':' | ';' | ')' | ']')
        {
            word.text.insert(0, ' ');
        }
    }
    words.sort_by_key(|word| (word.start_ms, word.channel));
    let mut words = words.into_iter().peekable();
    let mut memos = memos.to_vec();
    memos.sort_by_key(|memo| memo.at_ms);
    let mut events = Vec::new();
    for memo in memos {
        let mut window = Vec::new();
        while words.peek().is_some_and(|word| word.start_ms <= memo.at_ms) {
            window.push(words.next().expect("peeked transcript word"));
        }
        events.extend(
            merge_word_entries_to_phrases(window, max_gap_ms)
                .into_iter()
                .map(TimelineEvent::Transcript),
        );
        events.push(TimelineEvent::Memo(memo));
    }
    events.extend(
        merge_word_entries_to_phrases(words.collect(), max_gap_ms)
            .into_iter()
            .map(TimelineEvent::Transcript),
    );
    events
}

/// Render a memo and complete transcript on one session-relative timeline.
/// A memo closes the current context window, matching the way capture notes
/// bookmark the conversation that immediately preceded them.
pub fn render_aligned_markdown(
    session_name: &str,
    session_start: &DateTime<Local>,
    memo: &str,
    entries: &[TranscriptWordEntry],
) -> String {
    let memos = parse_markdown(memo, session_start)
        .into_iter()
        .map(|line| TimedMemo {
            at_ms: (line.created_at - *session_start).num_milliseconds().max(0) as u64,
            edited_at_ms: line
                .edited_at
                .map(|edited| (edited - *session_start).num_milliseconds().max(0) as u64),
            text: line.text,
        })
        .collect::<Vec<_>>();
    let events = interleave_timeline(entries, &memos, 2_000);

    let memo_count = events
        .iter()
        .filter(|event| matches!(event, TimelineEvent::Memo(_)))
        .count();
    let transcript_count = events.len().saturating_sub(memo_count);
    let mut windows = Vec::<Vec<TimelineEvent>>::new();
    let mut current = Vec::new();
    for event in events {
        let closes_window = matches!(event, TimelineEvent::Memo(_));
        current.push(event);
        if closes_window {
            windows.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        windows.push(current);
    }

    let mut out = format!("# Aligned transcript\n\nSession: `{session_name}`\n\n## Timeline\n\n");
    for window in &windows {
        for event in window {
            match event {
                TimelineEvent::Transcript(entry) => out.push_str(&format!(
                    "> [transcript ch{}] {}\n\n",
                    entry.channel,
                    entry.text.trim()
                )),
                TimelineEvent::Memo(memo) => {
                    let stamp = match memo.edited_at_ms {
                        Some(edited) => format!(
                            "{} ~{}",
                            format_timestamp(memo.at_ms),
                            format_timestamp(edited)
                        ),
                        None => format_timestamp(memo.at_ms),
                    };
                    out.push_str(&format!("**[{stamp} memo]** {}\n\n", memo.text.trim()));
                }
            }
        }
        out.push_str("---\n\n");
    }
    out.push_str("## Session metadata\n\n");
    out.push_str(&format!("- Memo lines: {memo_count}\n"));
    out.push_str(&format!("- Transcript entries: {transcript_count}\n"));
    out.push_str(&format!("- Windows: {}\n", windows.len()));
    out
}

fn format_timestamp(ms: u64) -> String {
    let seconds = ms / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[derive(Debug)]
pub struct ParsedLine {
    pub text: String,
    pub created_at: DateTime<Local>,
    pub edited_at: Option<DateTime<Local>>,
}

fn parse_time_str(value: &str) -> Option<i64> {
    let parts = value.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [minutes, seconds] => {
            Some(minutes.parse::<i64>().ok()? * 60 + seconds.parse::<i64>().ok()?)
        }
        [hours, minutes, seconds] => Some(
            hours.parse::<i64>().ok()? * 3_600
                + minutes.parse::<i64>().ok()? * 60
                + seconds.parse::<i64>().ok()?,
        ),
        _ => None,
    }
}

/// Read the timestamped memo rows used by remote capture. Untimed lines stay
/// separate because they are reflections rather than points on the timeline.
pub fn parse_timed_memo_lines(memo: &str) -> (Vec<TimedMemo>, Vec<String>) {
    let mut timed = Vec::new();
    let mut untimed = Vec::new();
    for line in memo.lines().map(str::trim) {
        if line.is_empty() || line == "---" || line.starts_with('#') {
            continue;
        }
        let parsed = line
            .strip_prefix('[')
            .and_then(|tail| tail.split_once(']'))
            .and_then(|(stamp, text)| {
                let (created, edited) = match stamp.split_once('~') {
                    Some((created, edited)) => (created.trim(), Some(edited.trim())),
                    None => (stamp.trim(), None),
                };
                Some(TimedMemo {
                    at_ms: u64::try_from(parse_time_str(created)?)
                        .ok()?
                        .checked_mul(1_000)?,
                    edited_at_ms: edited
                        .and_then(parse_time_str)
                        .and_then(|seconds| u64::try_from(seconds).ok())
                        .and_then(|seconds| seconds.checked_mul(1_000)),
                    text: text.trim().to_string(),
                })
            });
        if let Some(row) = parsed {
            timed.push(row);
        } else {
            untimed.push(line.to_string());
        }
    }
    (timed, untimed)
}

/// Parse persisted memo markdown without depending on the root capture crate.
/// The root `parser` module is a facade over this implementation so resume and
/// offline alignment cannot drift.
pub fn parse_markdown(content: &str, start_time: &DateTime<Local>) -> Vec<ParsedLine> {
    let edited = regex::Regex::new(r"^\[(\d+:\d{2}(?::\d{2})?) ~(\d+:\d{2}(?::\d{2})?)\] (.*)$")
        .expect("valid memo regex");
    let simple = regex::Regex::new(r"^\[(\d+:\d{2}(?::\d{2})?)\] (.*)$").expect("valid memo regex");
    let mut lines: Vec<ParsedLine> = Vec::new();
    for raw_line in content.lines() {
        if let Some(captures) = edited.captures(raw_line) {
            if let (Some(created), Some(changed)) =
                (parse_time_str(&captures[1]), parse_time_str(&captures[2]))
            {
                lines.push(ParsedLine {
                    text: captures[3].to_string(),
                    created_at: *start_time + chrono::Duration::seconds(created),
                    edited_at: Some(*start_time + chrono::Duration::seconds(changed)),
                });
                continue;
            }
        }
        if let Some(captures) = simple.captures(raw_line) {
            if let Some(created) = parse_time_str(&captures[1]) {
                lines.push(ParsedLine {
                    text: captures[2].to_string(),
                    created_at: *start_time + chrono::Duration::seconds(created),
                    edited_at: None,
                });
                continue;
            }
        }
        if let Some(last) = lines.last_mut() {
            last.text.push('\n');
            last.text.push_str(raw_line);
        } else {
            lines.push(ParsedLine {
                text: raw_line.to_string(),
                created_at: *start_time,
                edited_at: None,
            });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn memo_appears_after_preceding_transcript_and_closes_window() {
        let start = Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let entries = vec![
            TranscriptWordEntry {
                start_ms: 10_000,
                end_ms: 12_000,
                text: "context".into(),
                channel: 1,
            },
            TranscriptWordEntry {
                start_ms: 20_000,
                end_ms: 22_000,
                text: "after".into(),
                channel: 1,
            },
        ];
        let text = render_aligned_markdown("session", &start, "[00:15] my note", &entries);
        assert!(text.find("context").unwrap() < text.find("[00:15 memo]").unwrap());
        assert!(text.find("[00:15 memo]").unwrap() < text.find("after").unwrap());
        assert!(text.contains("- Windows: 2"));
    }

    #[test]
    fn memo_parser_preserves_resume_formats_and_continuations() {
        let start = Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let lines = parse_markdown(
            "orphan\n[01:30] simple\ncontinuation\n[01:00:00 ~01:02:03] edited",
            &start,
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "orphan");
        assert_eq!(lines[1].text, "simple\ncontinuation");
        assert_eq!((lines[1].created_at - start).num_seconds(), 90);
        assert_eq!((lines[2].created_at - start).num_seconds(), 3_600);
        assert_eq!((lines[2].edited_at.unwrap() - start).num_seconds(), 3_723);
    }

    #[test]
    fn shared_timeline_groups_words_without_crossing_memo_or_channel() {
        let words = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 1_000,
                end_ms: 1_100,
                text: " Nice".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 2_000,
                end_ms: 2_000,
                text: ".".into(),
            },
            TranscriptWordEntry {
                channel: 1,
                start_ms: 2_000,
                end_ms: 2_200,
                text: " Yes".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 3_000,
                end_ms: 3_100,
                text: " Next".into(),
            },
        ];
        let (memos, untimed) = parse_timed_memo_lines("# Memo\n[00:02] decision\nLater reflection");
        assert_eq!(untimed, ["Later reflection"]);
        let rows = interleave_timeline(&words, &memos, 2_000);
        assert!(matches!(&rows[0], TimelineEvent::Transcript(word) if word.text == " Nice."));
        assert!(matches!(&rows[1], TimelineEvent::Transcript(word) if word.channel == 1));
        assert!(matches!(&rows[2], TimelineEvent::Memo(memo) if memo.text == "decision"));
        assert!(matches!(&rows[3], TimelineEvent::Transcript(word) if word.text == " Next"));
    }

    #[test]
    fn shared_timeline_spaces_bare_cli_words_but_attaches_punctuation() {
        let words = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: "Nice".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 110,
                end_ms: 120,
                text: ".".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 130,
                end_ms: 200,
                text: "recording".into(),
            },
        ];
        let rows = interleave_timeline(&words, &[], 2_000);
        assert!(
            matches!(&rows[0], TimelineEvent::Transcript(word) if word.text.trim() == "Nice. recording")
        );
    }
}
