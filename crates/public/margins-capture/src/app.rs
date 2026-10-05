use chrono::{DateTime, Local};
use margins_core::{MemoMoment, TimedMemoDocument, TimedMemoLine};
use ratatui::layout::Rect;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
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
    pub device_uids: Vec<Option<String>>,
    pub selected_device: usize,
    pub current_mic_name: String,
    pub current_mic_uid: Option<String>,
    pub preferred_mic_name: Option<String>,
    pub preferred_mic_uid: Option<String>,
    pub suggested_mic_name: Option<String>,
    pub mic_silent: bool,
    pub mic_level: Arc<AtomicU32>,
    pub spk_level: Arc<AtomicU32>,
    pub mic_drops: Arc<AtomicU64>,
    pub spk_drops: Arc<AtomicU64>,
    pub mic_frames: Arc<AtomicU64>,
    pub mic_silence: Arc<AtomicU64>,
    pub mic_rate: u32,
    pub spk_silence: Arc<AtomicU64>,
    pub spk_frames: Arc<AtomicU64>,
    pub mic_real_spool_frames: Arc<AtomicU64>,
    pub spk_real_spool_frames: Arc<AtomicU64>,
    pub mic_no_audio_received: Arc<AtomicBool>,
    pub spk_no_audio_received: Arc<AtomicBool>,
    pub spk_rate: u32,
    pub native_spool_overflow: Arc<AtomicBool>,
    pub native_store_retrying: Arc<AtomicBool>,
    pub live_transcription_status: Arc<AtomicU8>,
    pub live_mic_dropped_samples: Arc<AtomicU64>,
    pub live_system_dropped_samples: Arc<AtomicU64>,
    pub capture_paused: bool,
    pub remote_delivery_state: Arc<AtomicU8>,
    pub remote_pending_chunks: Arc<AtomicU64>,
    pub remote_pending_bytes: Arc<AtomicU64>,
    pause_block_ordinal: u32,
    workspace_authority: Option<(PathBuf, String)>,
    observed_memo: Option<(String, Vec<TimedMemoLine>)>,
    pending_conflict: Option<PendingMemoConflict>,
}

struct PendingMemoConflict {
    path: PathBuf,
    lines: Vec<TimedMemoLine>,
    viewing_draft: bool,
    draft_scroll: usize,
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
            device_uids: Vec::new(),
            selected_device: 0,
            current_mic_name: mic_name,
            current_mic_uid: None,
            preferred_mic_name: None,
            preferred_mic_uid: None,
            suggested_mic_name: None,
            mic_silent: false,
            mic_level: Arc::new(AtomicU32::new(0)),
            spk_level: Arc::new(AtomicU32::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            spk_drops: Arc::new(AtomicU64::new(0)),
            mic_frames: Arc::new(AtomicU64::new(0)),
            mic_silence: Arc::new(AtomicU64::new(0)),
            mic_rate: 0,
            spk_silence: Arc::new(AtomicU64::new(0)),
            spk_frames: Arc::new(AtomicU64::new(0)),
            mic_real_spool_frames: Arc::new(AtomicU64::new(0)),
            spk_real_spool_frames: Arc::new(AtomicU64::new(0)),
            mic_no_audio_received: Arc::new(AtomicBool::new(false)),
            spk_no_audio_received: Arc::new(AtomicBool::new(false)),
            spk_rate: 0,
            native_spool_overflow: Arc::new(AtomicBool::new(false)),
            native_store_retrying: Arc::new(AtomicBool::new(false)),
            live_transcription_status: Arc::new(AtomicU8::new(LIVE_TRANSCRIPTION_OFF)),
            live_mic_dropped_samples: Arc::new(AtomicU64::new(0)),
            live_system_dropped_samples: Arc::new(AtomicU64::new(0)),
            capture_paused: false,
            remote_delivery_state: Arc::new(AtomicU8::new(REMOTE_DELIVERY_LOCAL)),
            remote_pending_chunks: Arc::new(AtomicU64::new(0)),
            remote_pending_bytes: Arc::new(AtomicU64::new(0)),
            pause_block_ordinal: 0,
            workspace_authority: None,
            observed_memo: None,
            pending_conflict: None,
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
            device_uids: Vec::new(),
            selected_device: 0,
            current_mic_name: mic_name,
            current_mic_uid: None,
            preferred_mic_name: None,
            preferred_mic_uid: None,
            suggested_mic_name: None,
            mic_silent: false,
            mic_level: Arc::new(AtomicU32::new(0)),
            spk_level: Arc::new(AtomicU32::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            spk_drops: Arc::new(AtomicU64::new(0)),
            mic_frames: Arc::new(AtomicU64::new(0)),
            mic_silence: Arc::new(AtomicU64::new(0)),
            mic_rate: 0,
            spk_silence: Arc::new(AtomicU64::new(0)),
            spk_frames: Arc::new(AtomicU64::new(0)),
            mic_real_spool_frames: Arc::new(AtomicU64::new(0)),
            spk_real_spool_frames: Arc::new(AtomicU64::new(0)),
            mic_no_audio_received: Arc::new(AtomicBool::new(false)),
            spk_no_audio_received: Arc::new(AtomicBool::new(false)),
            spk_rate: 0,
            native_spool_overflow: Arc::new(AtomicBool::new(false)),
            native_store_retrying: Arc::new(AtomicBool::new(false)),
            live_transcription_status: Arc::new(AtomicU8::new(LIVE_TRANSCRIPTION_OFF)),
            live_mic_dropped_samples: Arc::new(AtomicU64::new(0)),
            live_system_dropped_samples: Arc::new(AtomicU64::new(0)),
            capture_paused: false,
            remote_delivery_state: Arc::new(AtomicU8::new(REMOTE_DELIVERY_LOCAL)),
            remote_pending_chunks: Arc::new(AtomicU64::new(0)),
            remote_pending_bytes: Arc::new(AtomicU64::new(0)),
            pause_block_ordinal: 0,
            workspace_authority: None,
            observed_memo: None,
            pending_conflict: None,
        }
    }

    pub fn bind_workspace_authority(&mut self, margins_dir: PathBuf, session_id: String) {
        self.workspace_authority = Some((margins_dir, session_id));
    }

    pub fn observe_memo(&mut self, revision: String, lines: Vec<TimedMemoLine>) {
        self.observed_memo = Some((revision, lines));
        self.pending_conflict = None;
    }

    pub fn conflict_draft_path(&self) -> Option<&std::path::Path> {
        self.pending_conflict
            .as_ref()
            .map(|draft| draft.path.as_path())
    }

    pub fn viewing_conflict_draft(&self) -> bool {
        self.pending_conflict
            .as_ref()
            .is_some_and(|draft| draft.viewing_draft)
    }

    pub fn conflict_draft_lines(&self) -> Option<&[TimedMemoLine]> {
        self.pending_conflict
            .as_ref()
            .filter(|draft| draft.viewing_draft)
            .map(|draft| draft.lines.as_slice())
    }

    pub fn conflict_draft_scroll(&self) -> usize {
        self.pending_conflict
            .as_ref()
            .map_or(0, |draft| draft.draft_scroll)
    }

    pub fn scroll_conflict_draft(&mut self, delta: isize, visible_lines: usize) {
        if let Some(draft) = &mut self.pending_conflict {
            let last = draft.lines.len().saturating_sub(visible_lines.max(1));
            draft.draft_scroll = draft.draft_scroll.saturating_add_signed(delta).min(last);
        }
    }

    pub fn toggle_conflict_draft(&mut self) {
        if let Some(draft) = &mut self.pending_conflict {
            draft.viewing_draft = !draft.viewing_draft;
        }
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
            let mut pending_draft: Option<(PathBuf, Vec<TimedMemoLine>)> = None;
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
                        if let Some((path, lines)) = pending_draft {
                            self.pending_conflict = Some(PendingMemoConflict {
                                path: path.clone(),
                                lines,
                                viewing_draft: false,
                                draft_scroll: 0,
                            });
                            self.message = Some(format!(
                                "Memo changed elsewhere. Non-conflicting edits were saved; press Ctrl+G to view conflicting local lines at {}.",
                                path.display()
                            ));
                            return Err(io::Error::other(format!(
                                "memo conflict; conflicting local lines saved to {}",
                                path.display()
                            )));
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
                        let merged = merge_memo_lines_partial(&base, &desired, &current.lines)
                            .ok_or_else(|| io::Error::other("could not compare memo lines"))?;
                        if !merged.conflicts.is_empty() {
                            let (path, lines) = pending_draft.get_or_insert_with(|| {
                                (
                                    margins_dir.join(format!(
                                        "{session_id}.memo-conflict-{}.md",
                                        uuid::Uuid::new_v4()
                                    )),
                                    Vec::new(),
                                )
                            });
                            lines.extend(merged.conflicts);
                            std::fs::write(
                                path,
                                TimedMemoDocument::from_committed(lines.clone()).export_markdown(),
                            )?;
                        }
                        desired = merged.merged;
                        revision = current.revision;
                        base = current.lines;
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

#[derive(Debug, Clone, PartialEq)]
struct MemoHunk {
    start: usize,
    end: usize,
    replacement: Vec<TimedMemoLine>,
}

/// Find changed spans against the common document. A bounded LCS keeps the
/// result stable for insertions and deletions without treating later lines as
/// edits merely because their indexes shifted.
fn memo_hunks(base: &[TimedMemoLine], changed: &[TimedMemoLine]) -> Option<Vec<MemoHunk>> {
    let rows = base.len().checked_add(1)?;
    let cols = changed.len().checked_add(1)?;
    if rows.checked_mul(cols)? > 4_000_000 {
        let old = base
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let new = changed
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let mut hunks = Vec::new();
        let mut pending: Option<(usize, usize, usize, usize)> = None;
        for op in similar::capture_diff_slices(similar::Algorithm::Patience, &old, &new) {
            if op.tag() == similar::DiffTag::Equal {
                if let Some((start, end, new_start, new_end)) = pending.take() {
                    hunks.push(MemoHunk {
                        start,
                        end,
                        replacement: changed[new_start..new_end].to_vec(),
                    });
                }
                continue;
            }
            let old_range = op.old_range();
            let new_range = op.new_range();
            match &mut pending {
                Some((_, end, _, new_end))
                    if *end == old_range.start && *new_end == new_range.start =>
                {
                    *end = old_range.end;
                    *new_end = new_range.end;
                }
                _ => {
                    if let Some((start, end, new_start, new_end)) = pending.take() {
                        hunks.push(MemoHunk {
                            start,
                            end,
                            replacement: changed[new_start..new_end].to_vec(),
                        });
                    }
                    pending = Some((
                        old_range.start,
                        old_range.end,
                        new_range.start,
                        new_range.end,
                    ));
                }
            }
        }
        if let Some((start, end, new_start, new_end)) = pending {
            hunks.push(MemoHunk {
                start,
                end,
                replacement: changed[new_start..new_end].to_vec(),
            });
        }
        return Some(hunks);
    }
    let mut lcs = vec![vec![0usize; cols]; rows];
    for i in (0..base.len()).rev() {
        for j in (0..changed.len()).rev() {
            lcs[i][j] = if base[i] == changed[j] {
                1 + lcs[i + 1][j + 1]
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let (mut from_base, mut from_changed) = (0usize, 0usize);
    let mut hunks = Vec::new();
    while i < base.len() && j < changed.len() {
        if base[i] == changed[j] {
            if from_base < i || from_changed < j {
                hunks.push(MemoHunk {
                    start: from_base,
                    end: i,
                    replacement: changed[from_changed..j].to_vec(),
                });
            }
            i += 1;
            j += 1;
            from_base = i;
            from_changed = j;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if from_base < base.len() || from_changed < changed.len() {
        hunks.push(MemoHunk {
            start: from_base,
            end: base.len(),
            replacement: changed[from_changed..].to_vec(),
        });
    }
    Some(hunks)
}

struct PartialMemoMerge {
    merged: Vec<TimedMemoLine>,
    conflicts: Vec<TimedMemoLine>,
}

/// Apply independent local hunks while leaving overlapping remote edits in
/// the working memo. Only the local hunks requiring a decision go to the draft.
fn merge_memo_lines_partial(
    base: &[TimedMemoLine],
    local: &[TimedMemoLine],
    remote: &[TimedMemoLine],
) -> Option<PartialMemoMerge> {
    let local_hunks = memo_hunks(base, local)?;
    let remote_hunks = memo_hunks(base, remote)?;
    let mut accepted = Vec::new();
    let mut conflicts = Vec::new();
    for local_hunk in local_hunks {
        let conflicts_with_remote = remote_hunks.iter().any(|remote_hunk| {
            if local_hunk.start == remote_hunk.start
                && local_hunk.end == remote_hunk.end
                && same_memo_text(&local_hunk.replacement, &remote_hunk.replacement)
            {
                return false;
            }
            match (
                local_hunk.start == local_hunk.end,
                remote_hunk.start == remote_hunk.end,
            ) {
                (true, true) => false,
                (true, false) => {
                    remote_hunk.start < local_hunk.start && local_hunk.start < remote_hunk.end
                }
                (false, true) => {
                    local_hunk.start < remote_hunk.start && remote_hunk.start < local_hunk.end
                }
                (false, false) => {
                    local_hunk.start < remote_hunk.end && remote_hunk.start < local_hunk.end
                }
            }
        });
        if conflicts_with_remote {
            if local_hunk.replacement.is_empty() {
                conflicts.extend(base[local_hunk.start..local_hunk.end].iter().map(|line| {
                    let mut line = line.clone();
                    line.text = format!("[local deletion] {}", line.text);
                    line
                }));
            } else {
                conflicts.extend(base[local_hunk.start..local_hunk.end].iter().map(|line| {
                    let mut line = line.clone();
                    line.text = format!("[replaced base] {}", line.text);
                    line
                }));
                conflicts.extend(local_hunk.replacement);
            }
        } else {
            accepted.push(local_hunk);
        }
    }
    Some(PartialMemoMerge {
        merged: merge_memo_hunks(base, &accepted, &remote_hunks)?,
        conflicts,
    })
}

/// Merge edits against the lines both clients observed. Different insertions
/// at one boundary are retained in remote-then-local order; identical ones
/// appear once. Overlapping replacements need a human decision.
#[cfg(test)]
fn merge_memo_lines(
    base: &[TimedMemoLine],
    local: &[TimedMemoLine],
    remote: &[TimedMemoLine],
) -> Option<Vec<TimedMemoLine>> {
    let local_hunks = memo_hunks(base, local)?;
    let remote_hunks = memo_hunks(base, remote)?;
    merge_memo_hunks(base, &local_hunks, &remote_hunks)
}

fn merge_memo_hunks(
    base: &[TimedMemoLine],
    local_hunks: &[MemoHunk],
    remote_hunks: &[MemoHunk],
) -> Option<Vec<TimedMemoLine>> {
    let mut merged = Vec::with_capacity(base.len() + local_hunks.len() + remote_hunks.len());
    let (mut position, mut li, mut ri) = (0usize, 0usize, 0usize);
    loop {
        let local_insert = local_hunks
            .get(li)
            .filter(|h| h.start == position && h.end == position);
        let remote_insert = remote_hunks
            .get(ri)
            .filter(|h| h.start == position && h.end == position);
        match (local_insert, remote_insert) {
            (Some(local), Some(remote)) => {
                merged.extend(remote.replacement.iter().cloned());
                if !same_memo_text(&local.replacement, &remote.replacement) {
                    merged.extend(local.replacement.iter().cloned());
                }
                li += 1;
                ri += 1;
            }
            (Some(local), None) => {
                merged.extend(local.replacement.iter().cloned());
                li += 1;
            }
            (None, Some(remote)) => {
                merged.extend(remote.replacement.iter().cloned());
                ri += 1;
            }
            (None, None) => {}
        }
        if position == base.len() {
            return Some(merged);
        }
        let local_edit = local_hunks
            .get(li)
            .filter(|h| h.start == position && h.end > position);
        let remote_edit = remote_hunks
            .get(ri)
            .filter(|h| h.start == position && h.end > position);
        match (local_edit, remote_edit) {
            (Some(local), Some(remote)) => {
                if local.end != remote.end
                    || !same_memo_text(&local.replacement, &remote.replacement)
                {
                    return None;
                }
                merged.extend(local.replacement.iter().cloned());
                position = local.end;
                li += 1;
                ri += 1;
            }
            (Some(local), None) => {
                if remote_hunks.get(ri).is_some_and(|h| h.start < local.end) {
                    return None;
                }
                merged.extend(local.replacement.iter().cloned());
                position = local.end;
                li += 1;
            }
            (None, Some(remote)) => {
                if local_hunks.get(li).is_some_and(|h| h.start < remote.end) {
                    return None;
                }
                merged.extend(remote.replacement.iter().cloned());
                position = remote.end;
                ri += 1;
            }
            (None, None) => {
                merged.push(base[position].clone());
                position += 1;
            }
        }
    }
}

fn same_memo_text(left: &[TimedMemoLine], right: &[TimedMemoLine]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.text == right.text)
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
            .replace_memo_lines(
                "meeting",
                "bb",
                "remote-2",
                &base.revision,
                &[
                    TimedMemoLine::at("remote", MemoMoment::recording(2.0)),
                    TimedMemoLine::at("remote-only", MemoMoment::recording(2.1)),
                ],
            )
            .unwrap();
        let error = app.save().unwrap_err();
        assert!(error.to_string().contains("memo conflict"));
        assert_eq!(authority.memo("meeting").unwrap().lines[0].text, "remote");
        assert_eq!(app.memo.line(0).unwrap().text, "remote");
        assert_eq!(app.memo.line(1).unwrap().text, "remote-only");
        assert!(app.conflict_draft_path().is_some());
        app.toggle_conflict_draft();
        assert_eq!(
            app.conflict_draft_lines().unwrap()[0].text,
            "[replaced base] base"
        );
        assert_eq!(app.conflict_draft_lines().unwrap()[1].text, "local");
        app.toggle_conflict_draft();
        let draft = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().contains("memo-conflict"))
            .unwrap();
        assert!(std::fs::read_to_string(draft).unwrap().contains("local"));
        let first_message = app.message.clone();
        app.enter();
        type_text(&mut app, "new local note");
        app.save().unwrap();
        let saved = authority.memo("meeting").unwrap();
        assert!(saved.lines.iter().any(|line| line.text == "remote"));
        assert!(saved.lines.iter().any(|line| line.text == "remote-only"));
        assert!(saved.lines.iter().any(|line| line.text == "new local note"));
        assert!(app.conflict_draft_path().is_some());
        assert!(first_message
            .as_deref()
            .unwrap()
            .contains("Non-conflicting edits were saved"));
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().to_string_lossy().contains("memo-conflict"))
                .count(),
            1
        );
    }

    #[test]
    fn conflict_saves_independent_local_edits_and_drafts_only_the_overlap() {
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
        let lines = |values: &[&str]| {
            values
                .iter()
                .enumerate()
                .map(|(index, text)| TimedMemoLine::at(*text, MemoMoment::recording(index as f64)))
                .collect::<Vec<_>>()
        };
        let base = authority
            .replace_memo_lines(
                "meeting",
                "bb",
                "base",
                &initial.revision,
                &lines(&["A", "B", "C", "D"]),
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
        app.memo = TimedMemoDocument::from_committed(lines(&["A", "local B", "C", "local D"]));
        authority
            .replace_memo_lines(
                "meeting",
                "bb",
                "remote",
                &base.revision,
                &lines(&["A", "remote B", "C", "D", "remote E"]),
            )
            .unwrap();
        assert!(app
            .save()
            .unwrap_err()
            .to_string()
            .contains("memo conflict"));
        let saved = authority.memo("meeting").unwrap();
        assert_eq!(
            saved
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["A", "remote B", "C", "local D", "remote E"]
        );
        app.toggle_conflict_draft();
        let draft = app.conflict_draft_lines().unwrap();
        assert_eq!(draft.len(), 2);
        assert_eq!(draft[0].text, "[replaced base] B");
        assert_eq!(draft[1].text, "local B");
        let contents = std::fs::read_to_string(app.conflict_draft_path().unwrap()).unwrap();
        assert!(contents.contains("local B"));
        assert!(!contents.contains("local D"));
    }

    #[test]
    fn line_merge_keeps_insertions_in_place_and_deduplicates_identical_appends() {
        let lines = |values: &[&str]| {
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    TimedMemoLine::at(*value, MemoMoment::recording(index as f64))
                })
                .collect::<Vec<_>>()
        };
        let base = lines(&["A", "B", "C"]);
        let mut local = base.clone();
        local.insert(1, TimedMemoLine::at("X", MemoMoment::recording(1.5)));
        let mut remote = base.clone();
        remote.push(TimedMemoLine::at("R", MemoMoment::recording(4.0)));
        let merged = merge_memo_lines(&base, &local, &remote).unwrap();
        assert_eq!(
            merged
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["A", "X", "B", "C", "R"]
        );

        let appended = TimedMemoLine::at("same", MemoMoment::recording(5.0));
        let mut left = base.clone();
        left.push(appended.clone());
        let mut right = base.clone();
        right.push(TimedMemoLine::at("same", MemoMoment::recording(6.0)));
        assert_eq!(merge_memo_lines(&base, &left, &right).unwrap().len(), 4);
    }

    #[test]
    fn large_memo_uses_patience_diff_instead_of_conflicting_by_size() {
        let base = (0..2_500)
            .map(|index| TimedMemoLine::at(format!("line {index}"), MemoMoment::recording(1.0)))
            .collect::<Vec<_>>();
        let mut local = base.clone();
        local.insert(
            1_000,
            TimedMemoLine::at("local insert", MemoMoment::recording(2.0)),
        );
        let mut remote = base.clone();
        remote.push(TimedMemoLine::at(
            "remote append",
            MemoMoment::recording(3.0),
        ));
        let merged = merge_memo_lines(&base, &local, &remote).unwrap();
        assert_eq!(merged[1_000].text, "local insert");
        assert_eq!(merged.last().unwrap().text, "remote append");
        assert_eq!(merged.len(), 2_502);
    }

    #[test]
    fn conflicting_local_deletion_is_visible_in_the_draft() {
        let base = vec![TimedMemoLine::at("remove me", MemoMoment::recording(1.0))];
        let remote = vec![TimedMemoLine::at("remote edit", MemoMoment::recording(2.0))];
        let merged = merge_memo_lines_partial(&base, &[], &remote).unwrap();
        assert_eq!(merged.merged[0].text, "remote edit");
        assert_eq!(merged.conflicts[0].text, "[local deletion] remove me");
    }

    #[test]
    fn conflicting_partial_replacement_shows_all_dropped_base_lines() {
        let lines = |values: &[&str]| {
            values
                .iter()
                .map(|value| TimedMemoLine::at(*value, MemoMoment::recording(1.0)))
                .collect::<Vec<_>>()
        };
        let base = lines(&["A", "B", "C", "D"]);
        let local = lines(&["A", "replacement", "D"]);
        let remote = lines(&["A", "remote B", "C", "D"]);
        let merged = merge_memo_lines_partial(&base, &local, &remote).unwrap();
        assert_eq!(
            merged
                .conflicts
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["[replaced base] B", "[replaced base] C", "replacement"]
        );
        assert_eq!(
            merged
                .merged
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["A", "remote B", "C", "D"]
        );
    }
}
