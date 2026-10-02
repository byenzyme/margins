use chrono::{DateTime, Local};
use margins_core::{MemoMoment, TimedMemoDocument, TimedMemoLine};
use ratatui::layout::Rect;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;

use crate::text_helpers::*;

pub const GUTTER_WIDTH: u16 = 9;
pub const LIVE_TRANSCRIPTION_OFF: u8 = 0;
pub const LIVE_TRANSCRIPTION_WARMING: u8 = 1;
pub const LIVE_TRANSCRIPTION_READY: u8 = 2;
pub const LIVE_TRANSCRIPTION_DEGRADED: u8 = 3;
pub const REMOTE_DELIVERY_LOCAL: u8 = 0;
pub const REMOTE_DELIVERY_CURRENT: u8 = 1;
pub const REMOTE_DELIVERY_PENDING: u8 = 2;

#[derive(PartialEq, Eq)]
pub enum AppMode {
    Normal,
    DeviceSelect,
}

pub struct App {
    pub memo: TimedMemoDocument,
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
    pub capture_paused: bool,
    pub remote_delivery_state: Arc<AtomicU8>,
    pub remote_pending_chunks: Arc<AtomicU64>,
    pub remote_pending_bytes: Arc<AtomicU64>,
    pause_block_ordinal: u32,
    workspace_authority: Option<(PathBuf, String)>,
    observed_memo: Option<(String, Vec<TimedMemoLine>)>,
}

impl App {
    pub fn new(output_path: String, start_time: DateTime<Local>, mic_name: String) -> Self {
        Self {
            memo: TimedMemoDocument::new(MemoMoment::recording(0.0)),
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
            capture_paused: false,
            remote_delivery_state: Arc::new(AtomicU8::new(REMOTE_DELIVERY_LOCAL)),
            remote_pending_chunks: Arc::new(AtomicU64::new(0)),
            remote_pending_bytes: Arc::new(AtomicU64::new(0)),
            pause_block_ordinal: 0,
            workspace_authority: None,
            observed_memo: None,
        }
    }

    /// Reconstruct App state from parsed markdown lines (for --resume).
    pub fn from_memo(
        parsed: TimedMemoDocument,
        output_path: String,
        start_time: DateTime<Local>,
        mic_name: String,
    ) -> Self {
        let memo = TimedMemoDocument::resume(
            parsed.into_lines(),
            MemoMoment::recording(elapsed_between(start_time, Local::now())),
        );
        let cursor_line = memo.len() - 1;

        Self {
            memo,
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
            capture_paused: false,
            remote_delivery_state: Arc::new(AtomicU8::new(REMOTE_DELIVERY_LOCAL)),
            remote_pending_chunks: Arc::new(AtomicU64::new(0)),
            remote_pending_bytes: Arc::new(AtomicU64::new(0)),
            pause_block_ordinal: 0,
            workspace_authority: None,
            observed_memo: None,
        }
    }

    pub fn bind_workspace_authority(&mut self, margins_dir: PathBuf, session_id: String) {
        self.workspace_authority = Some((margins_dir, session_id));
    }

    pub fn observe_memo(&mut self, revision: String, lines: Vec<TimedMemoLine>) {
        self.observed_memo = Some((revision, lines));
    }

    pub fn mark_edited(&mut self, line: usize) {
        self.mark_edited_at(line, Local::now());
    }

    pub fn mark_edited_at(&mut self, line: usize, ts: DateTime<Local>) {
        self.memo.mark_edited(line, self.moment(ts));
    }

    pub fn commit_line_at(&mut self, line: usize, ts: DateTime<Local>) {
        self.memo.commit_line(line, self.moment(ts));
    }

    pub fn commit_uncommitted_at(&mut self, ts: DateTime<Local>) {
        self.memo.commit_all(self.moment(ts));
    }

    /// Mark the old line as settled when cursor moves to a different line.
    pub fn settle_on_move(&mut self, old_line: usize) {
        self.memo.settle(old_line);
    }

    pub fn elapsed_secs(&self) -> i64 {
        (Local::now() - self.start_time).num_seconds()
    }

    fn moment(&self, ts: DateTime<Local>) -> MemoMoment {
        let elapsed = elapsed_between(self.start_time, ts);
        if self.capture_paused {
            MemoMoment::paused(elapsed, self.pause_block_ordinal)
        } else {
            MemoMoment::recording(elapsed)
        }
    }

    pub fn set_capture_paused(&mut self, paused: bool) {
        if self.capture_paused == paused {
            return;
        }
        self.commit_uncommitted_at(Local::now());
        if paused {
            self.pause_block_ordinal = self.pause_block_ordinal.saturating_add(1);
        }
        self.capture_paused = paused;
    }

    pub fn display_ts(&self, i: usize) -> Option<(f64, bool)> {
        if !self.memo.is_committed(i) {
            return None;
        }
        let line = self.memo.line(i)?;
        match line.edited_secs {
            Some(edited) => Some((edited, true)),
            None => Some((line.created_secs, false)),
        }
    }

    pub fn gutter_label(&self, i: usize) -> (String, bool) {
        // Don't show timestamps for empty or uncommitted lines. The sidebar is
        // the alignment timestamp, so it appears after Enter commits the line.
        let Some((ts, edited)) = self.display_ts(i) else {
            return (" ".repeat(GUTTER_WIDTH as usize), false);
        };
        if self.memo.line(i).unwrap().text.trim().is_empty() {
            return (" ".repeat(GUTTER_WIDTH as usize), false);
        }

        // Collapse if same second + same edit status as previous committed line.
        if i > 0 {
            if let Some((prev_ts, prev_edited)) = self.display_ts(i - 1) {
                if (ts as i64 - prev_ts as i64).abs() == 0 && edited == prev_edited {
                    return (" ".repeat(GUTTER_WIDTH as usize), edited);
                }
            }
        }

        let time_str = margins_core::format_elapsed(ts);
        let prefix = if edited { "~" } else { " " };
        (
            format!("{}{:<w$}", prefix, time_str, w = GUTTER_WIDTH as usize - 1),
            edited,
        )
    }

    // --- Editing ---

    pub fn insert_char(&mut self, c: char) {
        let moment = self.moment(Local::now());
        self.memo
            .insert_char(self.cursor_line, self.cursor_col, c, moment);
        self.cursor_col += c.len_utf8();
    }

    pub fn enter(&mut self) {
        self.enter_at(Local::now());
    }

    pub fn enter_at(&mut self, ts: DateTime<Local>) {
        let old_line = self.cursor_line;
        self.memo
            .split_line(self.cursor_line, self.cursor_col, self.moment(ts));
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.settle_on_move(old_line);
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let prev = prev_char_boundary(
                &self.memo.line(self.cursor_line).unwrap().text,
                self.cursor_col,
            );
            let moment = self.moment(Local::now());
            self.memo
                .delete_range(self.cursor_line, prev..self.cursor_col, moment);
            self.cursor_col = prev;
        } else if self.cursor_line > 0 {
            let moment = self.moment(Local::now());
            let join = self
                .memo
                .join_with_previous(self.cursor_line, moment)
                .expect("cursor has a preceding line");
            self.cursor_line -= 1;
            self.cursor_col = join;
        }
    }

    pub fn delete(&mut self) {
        let line = &self.memo.line(self.cursor_line).unwrap().text;
        if self.cursor_col < line.len() {
            let next = next_char_boundary(line, self.cursor_col);
            let moment = self.moment(Local::now());
            self.memo
                .delete_range(self.cursor_line, self.cursor_col..next, moment);
        } else if self.cursor_line + 1 < self.memo.len() {
            let moment = self.moment(Local::now());
            self.memo.join_with_next(self.cursor_line, moment);
        }
    }

    pub fn delete_word_back(&mut self) {
        if self.cursor_col == 0 {
            self.backspace();
            return;
        }
        let boundary = prev_word_boundary(
            &self.memo.line(self.cursor_line).unwrap().text,
            self.cursor_col,
        );
        let moment = self.moment(Local::now());
        self.memo
            .delete_range(self.cursor_line, boundary..self.cursor_col, moment);
        self.cursor_col = boundary;
    }

    pub fn delete_to_line_start(&mut self) {
        if self.cursor_col == 0 {
            return;
        }
        let moment = self.moment(Local::now());
        self.memo
            .delete_range(self.cursor_line, 0..self.cursor_col, moment);
        self.cursor_col = 0;
    }

    pub fn delete_word_forward(&mut self) {
        if self.cursor_col >= self.memo.line(self.cursor_line).unwrap().text.len() {
            self.delete();
            return;
        }
        let boundary = next_word_boundary(
            &self.memo.line(self.cursor_line).unwrap().text,
            self.cursor_col,
        );
        let moment = self.moment(Local::now());
        self.memo
            .delete_range(self.cursor_line, self.cursor_col..boundary, moment);
    }

    // --- Navigation ---

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col = prev_char_boundary(
                &self.memo.line(self.cursor_line).unwrap().text,
                self.cursor_col,
            );
        } else if self.cursor_line > 0 {
            let old = self.cursor_line;
            self.cursor_line -= 1;
            self.cursor_col = self.memo.line(self.cursor_line).unwrap().text.len();
            self.settle_on_move(old);
        }
    }

    pub fn move_right(&mut self) {
        let line = &self.memo.line(self.cursor_line).unwrap().text;
        if self.cursor_col < line.len() {
            self.cursor_col = next_char_boundary(line, self.cursor_col);
        } else if self.cursor_line + 1 < self.memo.len() {
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
        if self.cursor_line + 1 < self.memo.len() {
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
                self.cursor_col = self.memo.line(self.cursor_line).unwrap().text.len();
                self.settle_on_move(old);
            }
            return;
        }
        self.cursor_col = prev_word_boundary(
            &self.memo.line(self.cursor_line).unwrap().text,
            self.cursor_col,
        );
    }

    pub fn move_word_right(&mut self) {
        let line = &self.memo.line(self.cursor_line).unwrap().text;
        if self.cursor_col >= line.len() {
            if self.cursor_line + 1 < self.memo.len() {
                let old = self.cursor_line;
                self.cursor_line += 1;
                self.cursor_col = 0;
                self.settle_on_move(old);
            }
            return;
        }
        self.cursor_col = next_word_end(line, self.cursor_col);
    }

    pub fn home(&mut self) {
        self.cursor_col = 0;
    }

    pub fn end(&mut self) {
        self.cursor_col = self.memo.line(self.cursor_line).unwrap().text.len();
    }

    pub fn snap_cursor_to_line(&mut self) {
        let len = self.memo.line(self.cursor_line).unwrap().text.len();
        if self.cursor_col > len {
            self.cursor_col = len;
        }
        // Snap to char boundary
        while self.cursor_col > 0
            && !self
                .memo
                .line(self.cursor_line)
                .unwrap()
                .text
                .is_char_boundary(self.cursor_col)
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

        if click_line < self.memo.len() {
            let old = self.cursor_line;
            self.cursor_line = click_line;
            self.cursor_col = click_col.min(self.memo.line(self.cursor_line).unwrap().text.len());
            self.snap_cursor_to_line();
            if old != self.cursor_line {
                self.settle_on_move(old);
            }
        }
    }

    // --- Export ---

    pub fn export(&self) -> String {
        self.memo.export_markdown()
    }

    pub fn save(&mut self) -> io::Result<()> {
        self.message = None;
        self.commit_uncommitted_at(Local::now());
        let content = self.export();
        if let Some((margins_dir, session_id)) = &self.workspace_authority {
            let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(margins_dir)
                .map_err(io::Error::other)?;
            let (mut revision, mut base) = match &self.observed_memo {
                Some(observed) => observed.clone(),
                None => {
                    let receipt = authority.memo(session_id).map_err(io::Error::other)?;
                    (receipt.revision, receipt.lines)
                }
            };
            let mut desired = self.memo.lines().to_vec();
            let mut saved = false;
            for _ in 0..3 {
                let request_id = format!("native-memo-{}", uuid::Uuid::new_v4());
                match authority.replace_memo_lines(
                    session_id,
                    "native-cli",
                    &request_id,
                    &revision,
                    &desired,
                ) {
                    Ok(receipt) => {
                        self.observed_memo = Some((receipt.revision, receipt.lines.clone()));
                        if receipt.lines != self.memo.lines() {
                            self.memo = TimedMemoDocument::resume(
                                receipt.lines,
                                MemoMoment::recording(elapsed_between(
                                    self.start_time,
                                    Local::now(),
                                )),
                            );
                            self.cursor_line = self.memo.len().saturating_sub(1);
                            self.cursor_col = 0;
                        }
                        if receipt.mirror_stale {
                            self.message =
                                Some("Memo saved in SQLite; Markdown mirror needs repair".into());
                        }
                        saved = true;
                        break;
                    }
                    Err(error)
                        if error
                            .downcast_ref::<margins_store::MemoRevisionConflict>()
                            .is_some() =>
                    {
                        let current = authority.memo(session_id).map_err(io::Error::other)?;
                        if let Some(merged) = merge_memo_lines(&base, &desired, &current.lines) {
                            desired = merged;
                            revision = current.revision;
                            base = current.lines;
                        } else {
                            let draft = margins_dir.join(format!(
                                "{session_id}.memo-conflict-{}.md",
                                uuid::Uuid::new_v4()
                            ));
                            std::fs::write(&draft, &content)?;
                            self.message = Some(format!(
                                "Memo changed elsewhere. Your draft is at {}. Review it and retry.",
                                draft.display()
                            ));
                            return Err(io::Error::other(format!(
                                "memo conflict; local draft saved to {}",
                                draft.display()
                            )));
                        }
                    }
                    Err(error) => return Err(io::Error::other(error)),
                }
            }
            if !saved {
                return Err(io::Error::other("memo changed repeatedly; retry save"));
            }
        } else {
            std::fs::write(&self.output_path, &content)?;
        }
        let count = content.lines().count();
        if self.message.is_none() {
            self.message = Some(format!("Saved {} lines to {}", count, self.output_path));
        }
        Ok(())
    }
}

/// Three-way merge of line edits. A line changed differently in both copies
/// needs a human decision; independent edits and appended lines are safe.
fn merge_memo_lines(
    base: &[TimedMemoLine],
    local: &[TimedMemoLine],
    remote: &[TimedMemoLine],
) -> Option<Vec<TimedMemoLine>> {
    if local.len() < base.len() || remote.len() < base.len() {
        return None;
    }
    let mut merged = Vec::with_capacity(local.len() + remote.len());
    for index in 0..base.len() {
        let local_changed = local[index] != base[index];
        let remote_changed = remote[index] != base[index];
        if local_changed && remote_changed && local[index] != remote[index] {
            return None;
        }
        merged.push(if local_changed {
            local[index].clone()
        } else {
            remote[index].clone()
        });
    }
    merged.extend_from_slice(&remote[base.len()..]);
    merged.extend_from_slice(&local[base.len()..]);
    Some(merged)
}

fn elapsed_between(start: DateTime<Local>, time: DateTime<Local>) -> f64 {
    (time - start).num_milliseconds().max(0) as f64 / 1_000.0
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
        assert!(app.memo.is_committed(0));
        assert!(app.memo.is_committed(1));
        assert!(!app.memo.is_committed(2));
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

        assert!(app.memo.is_committed(0));
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("final memo"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bound_native_memo_save_uses_sqlite_authority_and_markdown_projection() {
        let temp = tempfile::tempdir().unwrap();
        let margins_dir = temp.path().join(".margins");
        margins_store::canonical::create_session(
            &margins_dir,
            "native-session",
            &Local::now(),
            ".margins/native-session.md",
        )
        .unwrap();
        let output = margins_dir.join("native-session.md");
        let mut app = App::new(
            output.to_string_lossy().into_owned(),
            make_start(),
            "mic".into(),
        );
        app.bind_workspace_authority(margins_dir.clone(), "native-session".into());
        type_text(&mut app, "authoritative memo");
        app.save().unwrap();

        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&margins_dir).unwrap();
        let memo = authority.memo("native-session").unwrap();
        assert_eq!(memo.lines[0].text, "authoritative memo");
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            app.memo.export_markdown()
        );
    }

    #[test]
    fn remote_and_local_appends_merge_against_observed_revision() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".margins");
        margins_store::canonical::create_session(
            &dir,
            "meeting",
            &Local::now(),
            ".margins/meeting.md",
        )
        .unwrap();
        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
        let base = authority.memo("meeting").unwrap();
        let mut app = App::from_memo(
            TimedMemoDocument::from_committed(base.lines.clone()),
            dir.join("meeting.md").to_string_lossy().into_owned(),
            make_start(),
            "mic".into(),
        );
        app.bind_workspace_authority(dir.clone(), "meeting".into());
        app.observe_memo(base.revision.clone(), base.lines);
        type_text(&mut app, "local line");
        authority
            .update_memo(
                "meeting",
                "bb",
                "remote-1",
                &base.revision,
                1000,
                false,
                "remote line",
            )
            .unwrap();
        app.save().unwrap();
        let saved = authority.memo("meeting").unwrap();
        assert!(saved.lines.iter().any(|line| line.text == "remote line"));
        assert!(saved.lines.iter().any(|line| line.text == "local line"));
        assert!(!saved.mirror_stale);
    }

    #[test]
    fn divergent_line_keeps_remote_and_preserves_local_draft() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".margins");
        margins_store::canonical::create_session(
            &dir,
            "meeting",
            &Local::now(),
            ".margins/meeting.md",
        )
        .unwrap();
        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
        let initial = authority.memo("meeting").unwrap();
        let base = authority
            .update_memo(
                "meeting",
                "bb",
                "seed",
                &initial.revision,
                1000,
                false,
                "base",
            )
            .unwrap();
        let mut app = App::from_memo(
            TimedMemoDocument::from_committed(base.lines.clone()),
            dir.join("meeting.md").to_string_lossy().into_owned(),
            make_start(),
            "mic".into(),
        );
        app.bind_workspace_authority(dir.clone(), "meeting".into());
        app.observe_memo(base.revision.clone(), base.lines);
        app.memo = TimedMemoDocument::from_committed(vec![TimedMemoLine::at(
            "local",
            MemoMoment::recording(2.0),
        )]);
        authority
            .update_memo(
                "meeting",
                "bb",
                "remote-2",
                &base.revision,
                2000,
                false,
                "remote",
            )
            .unwrap();
        let error = app.save().unwrap_err();
        assert!(error.to_string().contains("memo conflict"));
        assert_eq!(authority.memo("meeting").unwrap().lines[0].text, "remote");
        let draft = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().contains("memo-conflict"))
            .unwrap();
        assert!(std::fs::read_to_string(draft).unwrap().contains("local"));
    }
}
