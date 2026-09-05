//! Parsed memo values shared by command and workflow implementations.

use serde::{Deserialize, Serialize};

/// One parsed memo line anchored to the session timeline when available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// The durable form of one line in the live meeting notepad.
///
/// Times are relative to the meeting start. `block_ordinal` is present while
/// the meeting clock is stopped, so those lines remain ordered without
/// pretending they align to audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedMemoLine {
    pub text: String,
    pub created_secs: f64,
    pub edited_secs: Option<f64>,
    #[serde(default)]
    pub draft_started_secs: Option<f64>,
    #[serde(default)]
    pub audio_pending_at_mark: bool,
    #[serde(default)]
    pub block_ordinal: Option<u32>,
}

impl TimedMemoLine {
    pub fn at(text: impl Into<String>, moment: MemoMoment) -> Self {
        Self {
            text: text.into(),
            created_secs: moment.elapsed_secs,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: moment.block_ordinal,
        }
    }
}

/// Where a notepad mutation happened in the meeting timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoMoment {
    pub elapsed_secs: f64,
    pub block_ordinal: Option<u32>,
}

impl MemoMoment {
    pub fn recording(elapsed_secs: f64) -> Self {
        Self {
            elapsed_secs,
            block_ordinal: None,
        }
    }

    pub fn paused(elapsed_secs: f64, block_ordinal: u32) -> Self {
        Self {
            elapsed_secs,
            block_ordinal: Some(block_ordinal),
        }
    }
}

/// The shared editing model used by the terminal notepad and local live API.
///
/// `committed` and `settled` are transient editor state. They are kept beside
/// the durable lines so frontends can render cursors differently without
/// inventing their own timestamp rules.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedMemoDocument {
    lines: Vec<TimedMemoLine>,
    committed: Vec<bool>,
    settled: Vec<bool>,
}

impl Default for TimedMemoDocument {
    fn default() -> Self {
        Self::new(MemoMoment::recording(0.0))
    }
}

impl TimedMemoDocument {
    /// Start an editable document with one uncommitted line.
    pub fn new(moment: MemoMoment) -> Self {
        Self {
            lines: vec![TimedMemoLine::at(String::new(), moment)],
            committed: vec![false],
            settled: vec![false],
        }
    }

    /// Wrap durable lines, as used by the local API and persisted desktop pad.
    pub fn from_committed(lines: Vec<TimedMemoLine>) -> Self {
        let len = lines.len();
        Self {
            lines,
            committed: vec![true; len],
            settled: vec![true; len],
        }
    }

    /// Resume editing durable lines and append a fresh, untimestamped draft.
    pub fn resume(mut lines: Vec<TimedMemoLine>, moment: MemoMoment) -> Self {
        let durable_len = lines.len();
        lines.push(TimedMemoLine::at(String::new(), moment));
        let mut committed = vec![true; durable_len];
        committed.push(false);
        let mut settled = vec![true; durable_len];
        settled.push(false);
        Self {
            lines,
            committed,
            settled,
        }
    }

    /// Parse the persisted line-oriented memo format. Unknown lines continue
    /// the preceding entry so older free-form notes remain readable.
    pub fn parse_markdown(content: &str) -> Self {
        let mut lines: Vec<TimedMemoLine> = Vec::new();
        for raw in content.lines() {
            if let Some(line) = parse_persisted_line(raw) {
                lines.push(line);
            } else if let Some(last) = lines.last_mut() {
                last.text.push('\n');
                last.text.push_str(raw);
            } else {
                lines.push(TimedMemoLine::at(raw, MemoMoment::recording(0.0)));
            }
        }
        Self::from_committed(lines)
    }

    pub fn lines(&self) -> &[TimedMemoLine] {
        &self.lines
    }

    pub fn line(&self, index: usize) -> Option<&TimedMemoLine> {
        self.lines.get(index)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn is_committed(&self, index: usize) -> bool {
        self.committed.get(index).copied().unwrap_or(false)
    }

    pub fn into_lines(self) -> Vec<TimedMemoLine> {
        self.lines
    }

    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .filter(|line| !line.text.trim().is_empty())
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn insert_char(&mut self, line: usize, byte: usize, value: char, moment: MemoMoment) {
        self.lines[line].text.insert(byte, value);
        self.mark_edited(line, moment);
    }

    pub fn delete_range(&mut self, line: usize, range: std::ops::Range<usize>, moment: MemoMoment) {
        self.lines[line].text.replace_range(range, "");
        self.mark_edited(line, moment);
    }

    pub fn split_line(&mut self, line: usize, byte: usize, moment: MemoMoment) {
        let rest = self.lines[line].text.split_off(byte);
        if !rest.is_empty() && byte > 0 {
            self.mark_edited(line, moment);
        }
        self.commit_line(line, moment);
        self.settle(line);
        self.lines.insert(line + 1, TimedMemoLine::at(rest, moment));
        self.committed.insert(line + 1, false);
        self.settled.insert(line + 1, false);
    }

    /// Remove `line` and append its text to the preceding line.
    /// Returns the byte position of the join.
    pub fn join_with_previous(&mut self, line: usize, moment: MemoMoment) -> Option<usize> {
        if line == 0 || line >= self.lines.len() {
            return None;
        }
        let removed = self.remove_line(line);
        let previous = line - 1;
        let join = self.lines[previous].text.len();
        self.lines[previous].text.push_str(&removed.text);
        self.mark_edited(previous, moment);
        Some(join)
    }

    /// Remove the following line and append it to `line`.
    pub fn join_with_next(&mut self, line: usize, moment: MemoMoment) -> bool {
        if line + 1 >= self.lines.len() {
            return false;
        }
        let removed = self.remove_line(line + 1);
        self.lines[line].text.push_str(&removed.text);
        self.mark_edited(line, moment);
        true
    }

    pub fn settle(&mut self, line: usize) {
        if let Some(settled) = self.settled.get_mut(line) {
            *settled = true;
        }
    }

    pub fn commit_line(&mut self, line: usize, moment: MemoMoment) {
        if line < self.lines.len()
            && !self.committed[line]
            && !self.lines[line].text.trim().is_empty()
        {
            self.lines[line].created_secs = moment.elapsed_secs;
            self.lines[line].block_ordinal = moment.block_ordinal;
            self.committed[line] = true;
        }
    }

    pub fn commit_all(&mut self, moment: MemoMoment) {
        for line in 0..self.lines.len() {
            self.commit_line(line, moment);
        }
    }

    pub fn mark_edited(&mut self, line: usize, moment: MemoMoment) {
        if line < self.lines.len() && self.committed[line] && self.settled[line] {
            self.lines[line].edited_secs = Some(moment.elapsed_secs);
            self.settled[line] = false;
        }
    }

    /// Replace the visible text while preserving the anchors of unchanged
    /// lines. Changed lines keep their original anchor and receive an edit
    /// time; genuinely new lines receive the current meeting time.
    pub fn reconcile_plain_text(&self, text: &str, moment: MemoMoment) -> Self {
        let next = visible_lines(text);
        let old_len = self.lines.len();
        let new_len = next.len();
        let mut lcs = vec![vec![0usize; new_len + 1]; old_len + 1];
        for old in (0..old_len).rev() {
            for new in (0..new_len).rev() {
                lcs[old][new] = if self.lines[old].text == next[new] {
                    1 + lcs[old + 1][new + 1]
                } else {
                    lcs[old + 1][new].max(lcs[old][new + 1])
                };
            }
        }

        let mut matches = Vec::new();
        let (mut old, mut new) = (0usize, 0usize);
        while old < old_len && new < new_len {
            if self.lines[old].text == next[new] {
                matches.push((old, new));
                old += 1;
                new += 1;
            } else if lcs[old + 1][new] >= lcs[old][new + 1] {
                old += 1;
            } else {
                new += 1;
            }
        }

        let mut out = Vec::with_capacity(new_len);
        let (mut old_start, mut new_start) = (0usize, 0usize);
        for (old_match, new_match) in matches.into_iter().chain([(old_len, new_len)]) {
            let paired = (old_match - old_start).min(new_match - new_start);
            for offset in 0..paired {
                let mut line = self.lines[old_start + offset].clone();
                let replacement = &next[new_start + offset];
                if line.text != *replacement {
                    line.text = replacement.clone();
                    line.edited_secs = Some(moment.elapsed_secs);
                }
                out.push(line);
            }
            for replacement in &next[new_start + paired..new_match] {
                out.push(TimedMemoLine::at(replacement.clone(), moment));
            }
            if old_match < old_len {
                out.push(self.lines[old_match].clone());
            }
            old_start = old_match.saturating_add(1);
            new_start = new_match.saturating_add(1);
        }
        Self::from_committed(out)
    }

    /// Opaque optimistic-concurrency token covering text and hidden timing.
    pub fn revision(&self) -> String {
        let bytes = serde_json::to_vec(&self.lines).unwrap_or_default();
        let mut hash = 0xcbf29ce484222325u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("v1-{hash:016x}")
    }

    pub fn export_markdown(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if line.text.trim().is_empty() || !self.committed[index] {
                continue;
            }
            if let Some(ordinal) = line.block_ordinal {
                out.push_str(&format!("[block {ordinal}] {}\n", line.text));
                continue;
            }
            let created = format_elapsed(line.created_secs);
            let grounding = line
                .audio_pending_at_mark
                .then_some(" (audio not live yet)")
                .unwrap_or_default();
            if let Some(edited) = line.edited_secs {
                out.push_str(&format!(
                    "[{created} ~{}]{grounding} {}\n",
                    format_elapsed(edited),
                    line.text
                ));
            } else {
                out.push_str(&format!("[{created}]{grounding} {}\n", line.text));
            }
        }
        out
    }

    fn remove_line(&mut self, line: usize) -> TimedMemoLine {
        self.committed.remove(line);
        self.settled.remove(line);
        self.lines.remove(line)
    }
}

fn visible_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_persisted_line(raw: &str) -> Option<TimedMemoLine> {
    let close = raw.find(']')?;
    let header = raw.strip_prefix('[')?.get(..close - 1)?;
    let mut body = raw.get(close + 1..)?.strip_prefix(' ')?;

    if let Some(ordinal) = header.strip_prefix("block ") {
        return Some(TimedMemoLine::at(
            body,
            MemoMoment::paused(0.0, ordinal.parse().ok()?),
        ));
    }

    let (created, edited) = match header.split_once(" ~") {
        Some((created, edited)) => (parse_elapsed(created)?, Some(parse_elapsed(edited)?)),
        None => (parse_elapsed(header)?, None),
    };
    let audio_pending_at_mark = body.starts_with("(audio not live yet) ");
    if audio_pending_at_mark {
        body = body.strip_prefix("(audio not live yet) ")?;
    }
    Some(TimedMemoLine {
        text: body.to_string(),
        created_secs: created,
        edited_secs: edited,
        draft_started_secs: None,
        audio_pending_at_mark,
        block_ordinal: None,
    })
}

fn parse_elapsed(value: &str) -> Option<f64> {
    let parts = value.split(':').collect::<Vec<_>>();
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?,
        [hours, minutes, seconds] => {
            hours.parse::<u64>().ok()? * 3_600
                + minutes.parse::<u64>().ok()? * 60
                + seconds.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(seconds as f64)
}

pub fn format_elapsed(secs: f64) -> String {
    let total = secs.max(0.0) as i64;
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod timed_tests {
    use super::*;

    fn line(text: &str, secs: f64) -> TimedMemoLine {
        TimedMemoLine::at(text, MemoMoment::recording(secs))
    }

    #[test]
    fn editing_settled_lines_records_one_edit_time_until_the_cursor_leaves() {
        let mut doc =
            TimedMemoDocument::resume(vec![line("first", 5.0)], MemoMoment::recording(10.0));
        doc.insert_char(0, 5, '!', MemoMoment::recording(12.0));
        doc.insert_char(0, 6, '!', MemoMoment::recording(13.0));
        assert_eq!(doc.lines()[0].edited_secs, Some(12.0));
        doc.settle(0);
        doc.delete_range(0, 5..7, MemoMoment::recording(14.0));
        assert_eq!(doc.lines()[0].edited_secs, Some(14.0));
    }

    #[test]
    fn reconciliation_preserves_anchors_and_times_changes_and_insertions() {
        let doc = TimedMemoDocument::from_committed(vec![
            line("one", 1.0),
            line("two", 2.0),
            line("three", 3.0),
        ]);
        let next =
            doc.reconcile_plain_text("one\nchanged\ninserted\nthree", MemoMoment::recording(20.0));
        assert_eq!(next.lines()[0], line("one", 1.0));
        assert_eq!(next.lines()[1].created_secs, 2.0);
        assert_eq!(next.lines()[1].edited_secs, Some(20.0));
        assert_eq!(next.lines()[2].created_secs, 20.0);
        assert_eq!(next.lines()[3], line("three", 3.0));
    }

    #[test]
    fn revision_covers_hidden_timing_and_export_handles_paused_lines() {
        let first = TimedMemoDocument::from_committed(vec![line("same", 1.0)]);
        let second = TimedMemoDocument::from_committed(vec![line("same", 2.0)]);
        assert_ne!(first.revision(), second.revision());

        let paused = TimedMemoDocument::from_committed(vec![TimedMemoLine::at(
            "during pause",
            MemoMoment::paused(10.0, 2),
        )]);
        assert_eq!(paused.export_markdown(), "[block 2] during pause\n");
    }

    #[test]
    fn markdown_round_trips_timing_blocks_and_grounding() {
        let source = "[00:05 ~00:08] (audio not live yet) revised\n[block 2] paused\n";
        let parsed = TimedMemoDocument::parse_markdown(source);
        assert_eq!(parsed.export_markdown(), source);
        assert_eq!(parsed.lines()[0].edited_secs, Some(8.0));
        assert!(parsed.lines()[0].audio_pending_at_mark);
        assert_eq!(parsed.lines()[1].block_ordinal, Some(2));
    }
}
