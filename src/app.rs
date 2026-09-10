use chrono::{DateTime, Local};
use ratatui::layout::Rect;
use std::io;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;

use crate::parser::ParsedLine;
use crate::text_helpers::*;

pub const GUTTER_WIDTH: u16 = 9;
pub const LIVE_TRANSCRIPTION_OFF: u8 = 0;
pub const LIVE_TRANSCRIPTION_WARMING: u8 = 1;
pub const LIVE_TRANSCRIPTION_READY: u8 = 2;
pub const LIVE_TRANSCRIPTION_DEGRADED: u8 = 3;

#[derive(PartialEq, Eq)]
pub enum AppMode {
    Normal,
    DeviceSelect,
}

pub struct App {
    pub lines: Vec<String>,
    /// The alignment timestamp for each line. A line receives this when it is
    /// committed (normally by pressing Enter), not when the blank line is
    /// created.
    pub created_at: Vec<DateTime<Local>>,
    pub edited_at: Vec<Option<DateTime<Local>>>,
    pub committed: Vec<bool>,
    /// A line is "settled" once the cursor has left it.
    /// Only settled lines get edited_at timestamps on modification.
    pub settled: Vec<bool>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll: usize,
    pub start_time: DateTime<Local>,
    pub output_path: String,
    pub message: Option<String>,
    pub editor_area: Rect,
    pub mode: AppMode,
    pub devices: Vec<String>,
    pub selected_device: usize,
    pub current_mic_name: String,
    pub mic_level: Arc<AtomicU32>,
    pub spk_level: Arc<AtomicU32>,
    pub mic_drops: Arc<AtomicU64>,
    pub spk_drops: Arc<AtomicU64>,
    pub spk_silence: Arc<AtomicU64>,
    pub spk_frames: Arc<AtomicU64>,
    pub spk_rate: u32,
    pub live_transcription_status: Arc<AtomicU8>,
}

impl App {
    pub fn new(output_path: String, start_time: DateTime<Local>, mic_name: String) -> Self {
        Self {
            lines: vec![String::new()],
            created_at: vec![start_time],
            edited_at: vec![None],
            committed: vec![false],
            settled: vec![false],
            cursor_line: 0,
            cursor_col: 0,
            scroll: 0,
            start_time,
            output_path,
            message: None,
            editor_area: Rect::default(),
            mode: AppMode::Normal,
            devices: Vec::new(),
            selected_device: 0,
            current_mic_name: mic_name,
            mic_level: Arc::new(AtomicU32::new(0)),
            spk_level: Arc::new(AtomicU32::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            spk_drops: Arc::new(AtomicU64::new(0)),
            spk_silence: Arc::new(AtomicU64::new(0)),
            spk_frames: Arc::new(AtomicU64::new(0)),
            spk_rate: 0,
            live_transcription_status: Arc::new(AtomicU8::new(LIVE_TRANSCRIPTION_OFF)),
        }
    }

    /// Reconstruct App state from parsed markdown lines (for --resume).
    pub fn from_parsed(
        parsed: Vec<ParsedLine>,
        output_path: String,
        start_time: DateTime<Local>,
        mic_name: String,
    ) -> Self {
        let mut lines = Vec::new();
        let mut created_at = Vec::new();
        let mut edited_at = Vec::new();
        let mut committed = Vec::new();
        let mut settled = Vec::new();

        for p in parsed {
            lines.push(p.text);
            created_at.push(p.created_at);
            edited_at.push(p.edited_at);
            committed.push(true);
            settled.push(true); // All resumed lines are settled
        }

        // Append an empty line for the user to continue typing. It is not
        // timestamped until committed.
        lines.push(String::new());
        created_at.push(start_time);
        edited_at.push(None);
        committed.push(false);
        settled.push(false);

        let cursor_line = lines.len() - 1;

        Self {
            lines,
            created_at,
            edited_at,
            committed,
            settled,
            cursor_line,
            cursor_col: 0,
            scroll: cursor_line.saturating_sub(10),
            start_time,
            output_path,
            message: None,
            editor_area: Rect::default(),
            mode: AppMode::Normal,
            devices: Vec::new(),
            selected_device: 0,
            current_mic_name: mic_name,
            mic_level: Arc::new(AtomicU32::new(0)),
            spk_level: Arc::new(AtomicU32::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            spk_drops: Arc::new(AtomicU64::new(0)),
            spk_silence: Arc::new(AtomicU64::new(0)),
            spk_frames: Arc::new(AtomicU64::new(0)),
            spk_rate: 0,
            live_transcription_status: Arc::new(AtomicU8::new(LIVE_TRANSCRIPTION_OFF)),
        }
    }

    pub fn mark_edited(&mut self, line: usize) {
        self.mark_edited_at(line, Local::now());
    }

    pub fn mark_edited_at(&mut self, line: usize, ts: DateTime<Local>) {
        if line < self.settled.len() && self.committed[line] && self.settled[line] {
            self.edited_at[line] = Some(ts);
            self.settled[line] = false;
        }
    }

    pub fn commit_line_at(&mut self, line: usize, ts: DateTime<Local>) {
        if line < self.committed.len()
            && !self.committed[line]
            && !self.lines[line].trim().is_empty()
        {
            self.created_at[line] = ts;
            self.committed[line] = true;
        }
    }

    pub fn commit_uncommitted_at(&mut self, ts: DateTime<Local>) {
        for i in 0..self.lines.len() {
            self.commit_line_at(i, ts);
        }
    }

    /// Mark the old line as settled when cursor moves to a different line.
    pub fn settle_on_move(&mut self, old_line: usize) {
        if old_line < self.settled.len() {
            self.settled[old_line] = true;
        }
    }

    pub fn elapsed_secs(&self) -> i64 {
        (Local::now() - self.start_time).num_seconds()
    }

    pub fn format_time(&self, ts: &DateTime<Local>) -> String {
        let elapsed = (*ts - self.start_time).num_seconds().max(0);
        let h = elapsed / 3600;
        let m = (elapsed % 3600) / 60;
        let s = elapsed % 60;
        if h > 0 {
            format!("{:02}:{:02}:{:02}", h, m, s)
        } else {
            format!("{:02}:{:02}", m, s)
        }
    }

    pub fn display_ts(&self, i: usize) -> Option<(&DateTime<Local>, bool)> {
        if !self.committed[i] {
            return None;
        }
        match self.edited_at[i] {
            Some(ref et) => Some((et, true)),
            None => Some((&self.created_at[i], false)),
        }
    }

    pub fn gutter_label(&self, i: usize) -> (String, bool) {
        // Don't show timestamps for empty or uncommitted lines. The sidebar is
        // the alignment timestamp, so it appears after Enter commits the line.
        let Some((ts, edited)) = self.display_ts(i) else {
            return (" ".repeat(GUTTER_WIDTH as usize), false);
        };
        if self.lines[i].trim().is_empty() {
            return (" ".repeat(GUTTER_WIDTH as usize), false);
        }

        // Collapse if same second + same edit status as previous committed line.
        if i > 0 {
            if let Some((prev_ts, prev_edited)) = self.display_ts(i - 1) {
                if (*ts - *prev_ts).num_seconds().abs() == 0 && edited == prev_edited {
                    return (" ".repeat(GUTTER_WIDTH as usize), edited);
                }
            }
        }

        let time_str = self.format_time(ts);
        let prefix = if edited { "~" } else { " " };
        (
            format!("{}{:<w$}", prefix, time_str, w = GUTTER_WIDTH as usize - 1),
            edited,
        )
    }

    // --- Editing ---

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.cursor_line].insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
        self.mark_edited(self.cursor_line);
    }

    pub fn enter(&mut self) {
        self.enter_at(Local::now());
    }

    pub fn enter_at(&mut self, ts: DateTime<Local>) {
        let old_line = self.cursor_line;
        let rest = self.lines[self.cursor_line].split_off(self.cursor_col);
        if !rest.is_empty() && self.cursor_col > 0 {
            self.mark_edited_at(self.cursor_line, ts);
        }
        self.commit_line_at(old_line, ts);
        self.settle_on_move(old_line);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_line, rest);
        self.created_at.insert(self.cursor_line, ts);
        self.edited_at.insert(self.cursor_line, None);
        self.committed.insert(self.cursor_line, false);
        self.settled.insert(self.cursor_line, false);
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let prev = prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(prev..self.cursor_col, "");
            self.cursor_col = prev;
            self.mark_edited(self.cursor_line);
        } else if self.cursor_line > 0 {
            let old_line = self.cursor_line;
            let current = self.lines.remove(self.cursor_line);
            self.created_at.remove(self.cursor_line);
            self.edited_at.remove(self.cursor_line);
            self.committed.remove(self.cursor_line);
            self.settled.remove(self.cursor_line);
            self.settle_on_move(old_line.min(self.lines.len().saturating_sub(1)));
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
            self.mark_edited(self.cursor_line);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor_col < self.lines[self.cursor_line].len() {
            let next = next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].replace_range(self.cursor_col..next, "");
            self.mark_edited(self.cursor_line);
        } else if self.cursor_line + 1 < self.lines.len() {
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.created_at.remove(self.cursor_line + 1);
            self.edited_at.remove(self.cursor_line + 1);
            self.committed.remove(self.cursor_line + 1);
            self.settled.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next_line);
            self.mark_edited(self.cursor_line);
        }
    }

    pub fn delete_word_back(&mut self) {
        if self.cursor_col == 0 {
            self.backspace();
            return;
        }
        let boundary = prev_word_boundary(&self.lines[self.cursor_line], self.cursor_col);
        self.lines[self.cursor_line].replace_range(boundary..self.cursor_col, "");
        self.cursor_col = boundary;
        self.mark_edited(self.cursor_line);
    }

    pub fn delete_to_line_start(&mut self) {
        if self.cursor_col == 0 {
            return;
        }
        self.lines[self.cursor_line].replace_range(..self.cursor_col, "");
        self.cursor_col = 0;
        self.mark_edited(self.cursor_line);
    }

    pub fn delete_word_forward(&mut self) {
        if self.cursor_col >= self.lines[self.cursor_line].len() {
            self.delete();
            return;
        }
        let boundary = next_word_boundary(&self.lines[self.cursor_line], self.cursor_col);
        self.lines[self.cursor_line].replace_range(self.cursor_col..boundary, "");
        self.mark_edited(self.cursor_line);
    }

    // --- Navigation ---

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col = prev_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        } else if self.cursor_line > 0 {
            let old = self.cursor_line;
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.settle_on_move(old);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_col < self.lines[self.cursor_line].len() {
            self.cursor_col = next_char_boundary(&self.lines[self.cursor_line], self.cursor_col);
        } else if self.cursor_line + 1 < self.lines.len() {
            let old = self.cursor_line;
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.settle_on_move(old);
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor_line > 0 {
            let old = self.cursor_line;
            self.cursor_line -= 1;
            self.snap_cursor_to_line();
            self.settle_on_move(old);
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor_line + 1 < self.lines.len() {
            let old = self.cursor_line;
            self.cursor_line += 1;
            self.snap_cursor_to_line();
            self.settle_on_move(old);
        }
    }

    pub fn move_word_left(&mut self) {
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                let old = self.cursor_line;
                self.cursor_line -= 1;
                self.cursor_col = self.lines[self.cursor_line].len();
                self.settle_on_move(old);
            }
            return;
        }
        self.cursor_col = prev_word_boundary(&self.lines[self.cursor_line], self.cursor_col);
    }

    pub fn move_word_right(&mut self) {
        if self.cursor_col >= self.lines[self.cursor_line].len() {
            if self.cursor_line + 1 < self.lines.len() {
                let old = self.cursor_line;
                self.cursor_line += 1;
                self.cursor_col = 0;
                self.settle_on_move(old);
            }
            return;
        }
        self.cursor_col = next_word_end(&self.lines[self.cursor_line], self.cursor_col);
    }

    pub fn home(&mut self) {
        self.cursor_col = 0;
    }

    pub fn end(&mut self) {
        self.cursor_col = self.lines[self.cursor_line].len();
    }

    pub fn snap_cursor_to_line(&mut self) {
        let len = self.lines[self.cursor_line].len();
        if self.cursor_col > len {
            self.cursor_col = len;
        }
        // Snap to char boundary
        while self.cursor_col > 0 && !self.lines[self.cursor_line].is_char_boundary(self.cursor_col)
        {
            self.cursor_col -= 1;
        }
    }

    pub fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        if visible_lines == 0 {
            return;
        }
        if self.cursor_line < self.scroll {
            self.scroll = self.cursor_line;
        } else if self.cursor_line >= self.scroll + visible_lines {
            self.scroll = self.cursor_line - visible_lines + 1;
        }
    }

    // --- Mouse ---

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let area = self.editor_area;
        let border: u16 = 1;

        if col < area.x + border + GUTTER_WIDTH || col >= area.x + area.width - border {
            return;
        }
        if row < area.y + border || row >= area.y + area.height - border {
            return;
        }

        let click_line = (row - area.y - border) as usize + self.scroll;
        let click_col = (col - area.x - border - GUTTER_WIDTH) as usize;

        if click_line < self.lines.len() {
            let old = self.cursor_line;
            self.cursor_line = click_line;
            self.cursor_col = click_col.min(self.lines[self.cursor_line].len());
            self.snap_cursor_to_line();
            if old != self.cursor_line {
                self.settle_on_move(old);
            }
        }
    }

    // --- Export ---

    pub fn export(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if line.trim().is_empty() || !self.committed[i] {
                continue;
            }
            let created = self.format_time(&self.created_at[i]);
            match self.edited_at[i] {
                Some(ref et) => {
                    let edited = self.format_time(et);
                    out.push_str(&format!("[{} ~{}] {}\n", created, edited, line));
                }
                None => {
                    out.push_str(&format!("[{}] {}\n", created, line));
                }
            }
        }
        out
    }

    pub fn save(&mut self) -> io::Result<()> {
        self.commit_uncommitted_at(Local::now());
        let content = self.export();
        std::fs::write(&self.output_path, &content)?;
        let count = content.lines().count();
        self.message = Some(format!("Saved {} lines to {}", count, self.output_path));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_start() -> DateTime<Local> {
        Local.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap()
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.insert_char(c);
        }
    }

    #[test]
    fn uncommitted_line_has_no_gutter_or_export() {
        let start = make_start();
        let mut app = App::new("/tmp/margins-test.md".into(), start, "mic".into());

        type_text(&mut app, "not done yet");

        assert_eq!(app.export(), "");
        assert_eq!(
            app.gutter_label(0),
            (" ".repeat(GUTTER_WIDTH as usize), false)
        );
    }

    #[test]
    fn enter_commits_line_at_enter_time() {
        let start = make_start();
        let mut app = App::new("/tmp/margins-test.md".into(), start, "mic".into());

        type_text(&mut app, "first memo");
        app.enter_at(start + chrono::Duration::seconds(65));
        type_text(&mut app, "second memo");
        app.enter_at(start + chrono::Duration::seconds(120));

        assert_eq!(app.export(), "[01:05] first memo\n[02:00] second memo\n");
        assert!(app.committed[0]);
        assert!(app.committed[1]);
        assert!(!app.committed[2]);
    }

    #[test]
    fn gutter_uses_commit_timestamp() {
        let start = make_start();
        let mut app = App::new("/tmp/margins-test.md".into(), start, "mic".into());

        type_text(&mut app, "memo");
        app.enter_at(start + chrono::Duration::seconds(42));

        let (gutter, edited) = app.gutter_label(0);
        assert!(!edited);
        assert_eq!(gutter.trim(), "00:42");
    }

    #[test]
    fn save_commits_uncommitted_line() {
        let start = make_start();
        let path =
            std::env::temp_dir().join(format!("margins-save-test-{}.md", std::process::id()));
        let mut app = App::new(path.to_string_lossy().into_owned(), start, "mic".into());

        type_text(&mut app, "final memo");
        app.save().unwrap();

        assert!(app.committed[0]);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("final memo"));
        let _ = std::fs::remove_file(path);
    }
}
