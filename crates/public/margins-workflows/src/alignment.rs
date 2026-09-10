use chrono::{DateTime, Local};
use margins_media::transcript::TranscriptWordEntry;

#[derive(Debug, Clone, PartialEq)]
enum TimelineEvent {
    Transcript(TranscriptWordEntry),
    Memo {
        at_ms: u64,
        edited_at_ms: Option<u64>,
        text: String,
    },
}

impl TimelineEvent {
    fn at_ms(&self) -> u64 {
        match self {
            Self::Transcript(entry) => entry.start_ms,
            Self::Memo { at_ms, .. } => *at_ms,
        }
    }
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
    let mut events = entries
        .iter()
        .cloned()
        .map(TimelineEvent::Transcript)
        .collect::<Vec<_>>();
    events.extend(parse_markdown(memo, session_start).into_iter().map(|line| {
        TimelineEvent::Memo {
            at_ms: (line.created_at - *session_start).num_milliseconds().max(0) as u64,
            edited_at_ms: line
                .edited_at
                .map(|edited| (edited - *session_start).num_milliseconds().max(0) as u64),
            text: line.text,
        }
    }));
    events.sort_by_key(|event| event.at_ms());

    let memo_count = events
        .iter()
        .filter(|event| matches!(event, TimelineEvent::Memo { .. }))
        .count();
    let transcript_count = events.len().saturating_sub(memo_count);
    let mut windows = Vec::<Vec<TimelineEvent>>::new();
    let mut current = Vec::new();
    for event in events {
        let closes_window = matches!(event, TimelineEvent::Memo { .. });
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
                TimelineEvent::Memo {
                    at_ms,
                    edited_at_ms,
                    text,
                } => {
                    let stamp = match edited_at_ms {
                        Some(edited) => format!(
                            "{} ~{}",
                            format_timestamp(*at_ms),
                            format_timestamp(*edited)
                        ),
                        None => format_timestamp(*at_ms),
                    };
                    out.push_str(&format!("**[{stamp} memo]** {}\n\n", text.trim()));
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
}
