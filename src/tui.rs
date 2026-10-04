use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::app::{App, AppMode, GUTTER_WIDTH};

/// Show silence beside the speaker meter without interrupting the mic meter.
const SYSTEM_AUDIO_SILENCE_MARKER_SECS: u64 = 3;
const MIC_NO_AUDIO_THRESHOLD_SECS: u64 = 3;
const SYSTEM_NO_AUDIO_THRESHOLD_SECS: u64 = 10;
const WATERMARK_HINT: &str = "  |  agent /watermark: live read";

fn watermark_hint(available_width: u16, status_width: usize) -> Option<&'static str> {
    (status_width.saturating_add(WATERMARK_HINT.len()) <= usize::from(available_width))
        .then_some(WATERMARK_HINT)
}

/// Observe only the tap's startup window. A denied macOS process tap commonly
/// starts IO successfully but then supplies no frames or only exact-zero frames.
const TAP_PERMISSION_NUDGE_THRESHOLD_SECS: u64 = 10;
const SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX: &str =
    "macOS system-audio IO started but delivered only empty buffers";
const SYSTEM_AUDIO_SILENCE_WARNING_PREFIX: &str = "Computer audio silent ";

fn should_nudge_system_audio_permission(
    io_started: bool,
    elapsed: std::time::Duration,
    sample_rate: u32,
    frame_count: u64,
    silent_samples: u64,
) -> bool {
    if !io_started
        || sample_rate == 0
        || elapsed < std::time::Duration::from_secs(TAP_PERMISSION_NUDGE_THRESHOLD_SECS)
    {
        return false;
    }
    let snapshot = margins_core::CaptureLaneSnapshot {
        lane: margins_core::AudioLane::System,
        state: margins_core::CaptureLaneState::Active,
        generation: 0,
        delivered_frames: frame_count,
        durable_frames: frame_count,
        observed_signal: frame_count > silent_samples,
        dropped_live_frames: 0,
        dropped_durable_frames: 0,
        last_error_code: None,
    };
    matches!(
        margins_core::capture_lane_health(&snapshot),
        margins_core::CaptureLaneHealth::NoFrames | margins_core::CaptureLaneHealth::Silent
    )
}

fn clear_system_audio_warning_after_recovery(
    message: &mut Option<String>,
    frame_count: u64,
    silent_samples: u64,
) -> bool {
    let recovered = frame_count > 0 && silent_samples == 0;
    let warning_is_ours = message.as_deref().is_some_and(|message| {
        message.starts_with(SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX)
            || message.starts_with(SYSTEM_AUDIO_SILENCE_WARNING_PREFIX)
    });
    if recovered && warning_is_ours {
        *message = None;
        return true;
    }
    false
}

#[derive(Default)]
struct SpoolProgress {
    last_frames: u64,
    last_progress_at: std::time::Duration,
}

impl SpoolProgress {
    fn missing(&mut self, elapsed: std::time::Duration, frames: u64, threshold_secs: u64) -> bool {
        if frames != self.last_frames {
            self.last_frames = frames;
            self.last_progress_at = elapsed;
            return false;
        }
        elapsed.saturating_sub(self.last_progress_at)
            >= std::time::Duration::from_secs(threshold_secs)
    }
}

fn update_no_audio_guard(
    app: &mut App,
    progress: &mut [SpoolProgress; 2],
    elapsed: std::time::Duration,
) {
    if app.capture_paused {
        return;
    }
    for (index, (lane, callbacks, spooled, warning, threshold)) in [
        (
            "mic",
            &app.mic_frames,
            &app.mic_real_spool_frames,
            &app.mic_no_audio_received,
            MIC_NO_AUDIO_THRESHOLD_SECS,
        ),
        (
            "system",
            &app.spk_frames,
            &app.spk_real_spool_frames,
            &app.spk_no_audio_received,
            SYSTEM_NO_AUDIO_THRESHOLD_SECS,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let real_frames = spooled.load(Ordering::Acquire);
        let missing = progress[index].missing(elapsed, real_frames, threshold);
        if missing && !warning.swap(true, Ordering::AcqRel) {
            crate::cli_log::event(
                "capture_no_audio_received",
                format!(
                    "lane={lane} callback_frames={} real_spool_frames={real_frames} elapsed_s={}",
                    callbacks.load(Ordering::Relaxed),
                    elapsed.as_secs(),
                ),
            );
        } else if !missing {
            warning.store(false, Ordering::Release);
        }
    }
    let silent = app.mic_rate != 0
        && app.mic_frames.load(Ordering::Acquire) > 0
        && app.mic_silence.load(Ordering::Acquire)
            >= u64::from(app.mic_rate) * MIC_NO_AUDIO_THRESHOLD_SECS;
    if silent && !app.mic_silent {
        let suggestion = suggested_device_index(
            &app.devices,
            &app.device_uids,
            &app.current_mic_name,
            app.current_mic_uid.as_deref(),
            app.preferred_mic_name.as_deref(),
            app.preferred_mic_uid.as_deref(),
        );
        app.suggested_mic_name = suggestion.map(|index| app.devices[index].clone());
        crate::cli_log::event(
            "capture_lane_silent",
            format!(
                "lane=mic name={:?} uid={:?} callback_frames={} consecutive_exact_zero_frames={} suggestion={:?}",
                app.current_mic_name,
                app.current_mic_uid,
                app.mic_frames.load(Ordering::Relaxed),
                app.mic_silence.load(Ordering::Relaxed),
                app.suggested_mic_name,
            ),
        );
    } else if !silent {
        app.suggested_mic_name = None;
    }
    app.mic_silent = silent;
}

fn is_virtual_loopback(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "blackhole",
        "loopback",
        "soundflower",
        "virtual",
        "vb-cable",
        "cable input",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn suggested_device_index(
    names: &[String],
    uids: &[Option<String>],
    current_name: &str,
    current_uid: Option<&str>,
    preferred_name: Option<&str>,
    preferred_uid: Option<&str>,
) -> Option<usize> {
    let is_current = |index: usize| {
        if let Some(uid) = current_uid {
            uids.get(index).and_then(Option::as_deref) == Some(uid)
        } else {
            names[index] == current_name
        }
    };
    let preferred = if let Some(uid) = preferred_uid {
        uids.iter()
            .position(|candidate| candidate.as_deref() == Some(uid))
    } else {
        preferred_name.and_then(|name| names.iter().position(|candidate| candidate == name))
    };
    if let Some(index) = preferred.filter(|index| !is_current(*index)) {
        return Some(index);
    }
    names
        .iter()
        .enumerate()
        .find(|(index, name)| !is_current(*index) && !is_virtual_loopback(name))
        .map(|(index, _)| index)
}

fn picker_preselection(app: &App) -> usize {
    let suggestion = app
        .mic_silent
        .then(|| {
            suggested_device_index(
                &app.devices,
                &app.device_uids,
                &app.current_mic_name,
                app.current_mic_uid.as_deref(),
                app.preferred_mic_name.as_deref(),
                app.preferred_mic_uid.as_deref(),
            )
        })
        .flatten();
    suggestion
        .or_else(|| {
            app.current_mic_uid.as_deref().and_then(|uid| {
                app.device_uids
                    .iter()
                    .position(|candidate| candidate.as_deref() == Some(uid))
            })
        })
        .or_else(|| app.devices.iter().position(|n| *n == app.current_mic_name))
        .unwrap_or(0)
}

pub enum TuiAction {
    Quit,
    Pause,
    Resume,
    SwitchDevice(usize),
}

/// Run the TUI event loop. Returns `TuiAction` indicating quit or device switch.
/// Sets `stop_flag` to signal the recorder thread to stop.
pub fn run_tui(
    app: &mut App,
    stop_flag: Arc<AtomicBool>,
) -> Result<TuiAction, Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, app, &stop_flag);

    // The producer sees Pause/Stop before terminal cleanup, memo persistence,
    // ASR finalization, or any network work performed by the caller.
    stop_flag.store(true, Ordering::SeqCst);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    stop_flag: &AtomicBool,
) -> Result<TuiAction, Box<dyn std::error::Error>> {
    // Reaching the TUI means system-audio IO returned Started. Probe once after
    // sustained startup silence; later quiet periods use the meter marker.
    let system_audio_io_started_at = std::time::Instant::now();
    let capture_started_at = std::time::Instant::now();
    let mut spool_progress = [SpoolProgress::default(), SpoolProgress::default()];
    let mut system_audio_permission_probe_pending = cfg!(target_os = "macos");

    loop {
        terminal.draw(|f| render(f, app))?;

        if app.native_spool_overflow.load(Ordering::Acquire) {
            app.message = Some(
                "Audio storage fell over 60s behind. Capture is stopping; run margins attach to resume."
                    .into(),
            );
            terminal.draw(|f| render(f, app))?;
            std::thread::sleep(std::time::Duration::from_millis(750));
            return Err(Box::new(io::Error::other(
                "audio storage fell over 60 seconds behind; recording stopped. Run margins attach to resume this session",
            )));
        }

        if event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    app.message = None;

                    if let Some(action) = handle_key(app, key) {
                        return Ok(action);
                    }
                }
                Event::Mouse(mouse) => {
                    if app.mode == AppMode::Normal {
                        handle_mouse(app, mouse);
                    }
                }
                _ => {}
            }
        }

        if system_audio_permission_probe_pending
            && system_audio_io_started_at.elapsed()
                >= std::time::Duration::from_secs(TAP_PERMISSION_NUDGE_THRESHOLD_SECS)
        {
            system_audio_permission_probe_pending = false;
            let frame_count = app.spk_frames.load(Ordering::Relaxed);
            let silent_samples = app.spk_silence.load(Ordering::Relaxed);
            if should_nudge_system_audio_permission(
                true,
                system_audio_io_started_at.elapsed(),
                app.spk_rate,
                frame_count,
                silent_samples,
            ) {
                app.message = Some(
                    margins_cli::error::macos_system_audio_permission_likely_message(
                        SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX,
                    ),
                );
            }
        }

        let frame_count = app.spk_frames.load(Ordering::Relaxed);
        let silent_samples = app.spk_silence.load(Ordering::Relaxed);
        clear_system_audio_warning_after_recovery(&mut app.message, frame_count, silent_samples);

        update_no_audio_guard(app, &mut spool_progress, capture_started_at.elapsed());

        // Also check if stop was requested externally
        if stop_flag.load(Ordering::SeqCst) {
            return Ok(TuiAction::Quit);
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<TuiAction> {
    match app.mode {
        AppMode::DeviceSelect => handle_key_device_select(app, key),
        AppMode::Normal => handle_key_normal(app, key),
    }
}

fn handle_key_device_select(app: &mut App, key: KeyEvent) -> Option<TuiAction> {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Up => {
            if app.selected_device > 0 {
                app.selected_device -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_device + 1 < app.devices.len() {
                app.selected_device += 1;
            }
        }
        KeyCode::Enter => {
            let idx = app.selected_device;
            app.mode = AppMode::Normal;
            return Some(TuiAction::SwitchDevice(idx));
        }
        _ => {}
    }
    None
}

fn handle_key_normal(app: &mut App, key: KeyEvent) -> Option<TuiAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let sup = key.modifiers.contains(KeyModifiers::SUPER);
    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;

    if key.code == KeyCode::Char('g') && ctrl && app.conflict_draft_path().is_some() {
        app.toggle_conflict_draft();
        app.message = None;
        return None;
    }
    if app.viewing_conflict_draft() {
        let page = app.editor_area.height.saturating_sub(2).max(1) as usize;
        match key.code {
            KeyCode::Char('c') if ctrl => return Some(TuiAction::Quit),
            KeyCode::Up => app.scroll_conflict_draft(-1, page),
            KeyCode::Down => app.scroll_conflict_draft(1, page),
            KeyCode::PageUp => app.scroll_conflict_draft(-(page as isize), page),
            KeyCode::PageDown => app.scroll_conflict_draft(page as isize, page),
            _ => {}
        }
        return None;
    }

    match key.code {
        // Quit
        KeyCode::Char('c') if ctrl => return Some(TuiAction::Quit),

        // Pause/resume. The caller retires native devices before re-entering
        // the editor in paused mode.
        KeyCode::Char('p') if ctrl => {
            return Some(if app.capture_paused {
                TuiAction::Resume
            } else {
                TuiAction::Pause
            })
        }

        // Save
        KeyCode::Char('s') if ctrl => {
            if let Err(error) = app.save() {
                if app.message.is_none() {
                    app.message = Some(format!("Memo save failed: {error}"));
                }
            }
        }

        // Device select
        KeyCode::Char('d') if ctrl => {
            let devices = crate::recorder::list_input_devices();
            app.devices = devices.iter().map(|(name, _)| name.clone()).collect();
            app.device_uids = crate::recorder::input_device_uid_snapshot(&app.devices);
            app.selected_device = picker_preselection(app);
            app.mode = AppMode::DeviceSelect;
        }

        // Delete to start of line: Cmd+Backspace or Ctrl+U
        KeyCode::Backspace if sup => app.delete_to_line_start(),
        KeyCode::Char('u') if ctrl => app.delete_to_line_start(),

        // Delete word back: Alt/Option+Backspace
        KeyCode::Backspace if alt => app.delete_word_back(),

        // Delete word forward: Alt/Option+Delete
        KeyCode::Delete if alt => app.delete_word_forward(),

        // Word navigation: Alt/Option+Arrow
        KeyCode::Left if alt => app.move_word_left(),
        KeyCode::Right if alt => app.move_word_right(),

        // Home/End with Cmd or standalone
        KeyCode::Left if sup => app.home(),
        KeyCode::Right if sup => app.end(),
        KeyCode::Home => app.home(),
        KeyCode::End => app.end(),

        // Basic navigation
        KeyCode::Left if plain => app.move_left(),
        KeyCode::Right if plain => app.move_right(),
        KeyCode::Up if plain => app.move_up(),
        KeyCode::Down if plain => app.move_down(),

        // Editing
        KeyCode::Enter => app.enter(),
        KeyCode::Backspace if plain => app.backspace(),
        KeyCode::Delete if plain => app.delete(),
        KeyCode::Tab => {
            app.insert_char(' ');
            app.insert_char(' ');
        }

        // Character input (no ctrl modifier)
        KeyCode::Char(c) if !ctrl => app.insert_char(c),

        _ => {}
    }

    None
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    if app.viewing_conflict_draft() {
        let page = app.editor_area.height.saturating_sub(2).max(1) as usize;
        match mouse.kind {
            MouseEventKind::ScrollUp => app.scroll_conflict_draft(-1, page),
            MouseEventKind::ScrollDown => app.scroll_conflict_draft(1, page),
            _ => {}
        }
        return;
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            app.handle_click(mouse.column, mouse.row);
        }
        MouseEventKind::ScrollUp => {
            if app.scroll > 0 {
                app.scroll -= 1;
            }
        }
        MouseEventKind::ScrollDown => {
            app.scroll += 1;
        }
        _ => {}
    }
}

fn render(f: &mut ratatui::Frame, app: &mut App) {
    let input_notice = app.message.as_deref().is_some_and(|message| {
        message.starts_with("Saved mic '") || message.starts_with("Could not read saved mic choice")
    });
    let status_height = if app.mic_silent || input_notice { 2 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(status_height)])
        .split(f.area());

    let editor_area = chunks[0];
    let status_area = chunks[1];

    // Store for mouse click handling
    app.editor_area = editor_area;

    let visible_lines = editor_area.height.saturating_sub(2) as usize;
    if !app.viewing_conflict_draft() {
        app.ensure_cursor_visible(visible_lines);
    }

    // Build visible lines
    let mut display_lines: Vec<Line> = Vec::new();
    let draft_lines = app.conflict_draft_lines();
    let lines = draft_lines.unwrap_or_else(|| app.memo.lines());
    let scroll = if draft_lines.is_some() {
        app.conflict_draft_scroll()
    } else {
        app.scroll
    };
    let end = (scroll + visible_lines).min(lines.len());

    for i in scroll..end {
        if draft_lines.is_some() {
            display_lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>8} ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(&lines[i].text, Style::default().fg(Color::Yellow)),
            ]));
            continue;
        }
        let (gutter, edited) = app.gutter_label(i);

        let gutter_style = if edited {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let text_style = if i == app.cursor_line {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };

        display_lines.push(Line::from(vec![
            Span::styled(gutter, gutter_style),
            Span::styled(&lines[i].text, text_style),
        ]));
    }

    let title = if draft_lines.is_some() {
        " local draft (read only) "
    } else {
        " margins "
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(display_lines).block(block);
    f.render_widget(paragraph, editor_area);

    // Cursor
    if draft_lines.is_none() {
        let cursor_x = editor_area.x + 1 + GUTTER_WIDTH + app.cursor_col as u16;
        let cursor_y = editor_area.y + 1 + (app.cursor_line - app.scroll) as u16;
        if cursor_x < editor_area.x + editor_area.width - 1
            && cursor_y < editor_area.y + editor_area.height - 1
        {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }

    // Status bar
    let elapsed = app.elapsed_secs();
    let time = if elapsed >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            elapsed / 3600,
            (elapsed % 3600) / 60,
            elapsed % 60
        )
    } else {
        format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
    };

    let mic_peak = app.mic_level.swap(0, Ordering::Relaxed);
    let spk_peak = app.spk_level.swap(0, Ordering::Relaxed);
    let mic_drops = app.mic_drops.load(Ordering::Relaxed);
    let spk_drops = app.spk_drops.load(Ordering::Relaxed);

    let speaker_silent = app.spk_rate > 0
        && app.spk_silence.load(Ordering::Relaxed) / app.spk_rate as u64
            >= SYSTEM_AUDIO_SILENCE_MARKER_SECS;
    let meter_text = format!(
        "mic {}{} spk {}{}{}",
        level_meter(mic_peak, 8),
        if app.mic_silent {
            " SILENT"
        } else if app.mic_no_audio_received.load(Ordering::Acquire) {
            " NO AUDIO"
        } else {
            ""
        },
        level_meter(spk_peak, 8),
        if app.spk_no_audio_received.load(Ordering::Acquire) {
            " NO AUDIO"
        } else {
            ""
        },
        if speaker_silent { " silent" } else { "" },
    );
    let status_prefix = format!(
        " {}{} | ",
        time,
        if app.capture_paused { " PAUSED" } else { "" }
    );

    let remote_delivery = match app.remote_delivery_state.load(Ordering::Relaxed) {
        crate::app::REMOTE_DELIVERY_CURRENT => " | delivery current".to_string(),
        crate::app::REMOTE_DELIVERY_PENDING => format!(
            " | connection lost; {} chunks / {} KiB saved locally",
            app.remote_pending_chunks.load(Ordering::Relaxed),
            app.remote_pending_bytes
                .load(Ordering::Relaxed)
                .div_ceil(1024)
        ),
        _ => String::new(),
    };
    let mut detail = if app.viewing_conflict_draft() {
        "CONFLICTING LOCAL LINES read only | ^G return to merged memo | ^C stop".to_string()
    } else if app.native_store_retrying.load(Ordering::Acquire) {
        "Audio storage busy, retrying; capture is buffered locally".to_string()
    } else if let Some(path) = app.conflict_draft_path() {
        format!(
            "CONFLICT: merged memo shown | ^G view conflicting local lines ({})",
            path.display()
        )
    } else if let Some(ref msg) = app.message {
        msg.clone()
    } else if app.capture_paused {
        format!(
            "^C stop ^P resume ^S save | {} lines{}",
            app.memo.len(),
            remote_delivery,
        )
    } else {
        format!(
            "^C stop ^P pause ^S save ^D device | {} lines{}",
            app.memo.len(),
            remote_delivery,
        )
    };
    if mic_drops > 0 || spk_drops > 0 {
        detail = format!("DROPS mic:{mic_drops} spk:{spk_drops} | {detail}");
    }
    detail = format!("input: {} | {detail}", app.current_mic_name);

    // The terminal clips only the trailing detail when a notice is long.
    // Time and both meters occupy the fixed prefix in every status state.
    let status_text = format!("{status_prefix}{meter_text} | {detail}");
    let hint = watermark_hint(status_area.width, Line::from(status_text.as_str()).width());
    let mut status_spans = vec![
        Span::raw(status_prefix),
        Span::styled(
            meter_text,
            if app.capture_paused {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
            },
        ),
        Span::raw(format!(" | {detail}")),
    ];
    if let Some(hint) = hint {
        status_spans.push(Span::styled(
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::White)
                .remove_modifier(Modifier::BOLD)
                .add_modifier(Modifier::DIM),
        ));
    }

    let mut status_lines = vec![Line::from(status_spans)];
    if app.mic_silent {
        let mut warning = format!("mic silent: {} — ^D choose input", app.current_mic_name);
        if let Some(suggested) = &app.suggested_mic_name {
            warning = format!(
                "mic silent: {} — ^D (suggest: {suggested})",
                app.current_mic_name
            );
        }
        status_lines.push(Line::from(Span::styled(
            warning,
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )));
    } else if input_notice {
        status_lines.push(Line::from(Span::styled(
            app.message.as_deref().unwrap_or_default(),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )));
    }
    let status = Paragraph::new(status_lines).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(status, status_area);

    // Device select overlay
    if app.mode == AppMode::DeviceSelect {
        render_device_overlay(f, app);
    }
}

fn render_device_overlay(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let max_name_len = app
        .devices
        .iter()
        .map(|n| n.len())
        .max()
        .unwrap_or(20)
        .max(20);
    let width = (max_name_len as u16 + 6).min(area.width.saturating_sub(4));
    let height = (app.devices.len() as u16 + 2).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay_area = Rect::new(x, y, width, height);

    f.render_widget(Clear, overlay_area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, name) in app.devices.iter().enumerate() {
        let is_current = *name == app.current_mic_name;
        let marker = if is_current { "*" } else { " " };
        let prefix = if i == app.selected_device { ">" } else { " " };
        let label = format!("{}{} {}", prefix, marker, name);

        let style = if i == app.selected_device {
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(label, style)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Select mic (↑↓ Enter Esc) ");
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, overlay_area);
}

fn level_meter(peak_bits: u32, width: usize) -> String {
    let peak = f32::from_bits(peak_bits);
    if peak <= 0.0 {
        return "░".repeat(width);
    }
    let db = 20.0 * peak.log10();
    // Map -72 dB .. 0 dB onto 0 .. width bars
    let normalized = ((db + 72.0) / 72.0).clamp(0.0, 1.0);
    let filled = (normalized * width as f32).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    const RATE: u32 = 48_000;

    #[test]
    fn zero_callback_guard_shows_a_lane_marker_and_recovers_on_real_spool_frames() {
        let mut app = App::new("meeting.md".into(), chrono::Local::now(), "mic".into());
        let mut progress = [SpoolProgress::default(), SpoolProgress::default()];
        app.mic_frames.store(3 * RATE as u64, Ordering::Release);
        update_no_audio_guard(&mut app, &mut progress, Duration::from_millis(2_999));
        assert!(!app.mic_no_audio_received.load(Ordering::Acquire));
        update_no_audio_guard(&mut app, &mut progress, Duration::from_secs(3));
        assert!(app.mic_no_audio_received.load(Ordering::Acquire));
        let status = rendered_status(&mut app, 80);
        assert!(status.contains("mic █"), "{status}");
        assert!(status.contains("NO AUDIO"), "{status}");
        assert!(status.contains("spk ░░░░░░░░"), "{status}");
        app.mic_real_spool_frames.store(1, Ordering::Release);
        update_no_audio_guard(&mut app, &mut progress, Duration::from_secs(4));
        assert!(!app.mic_no_audio_received.load(Ordering::Acquire));
        update_no_audio_guard(&mut app, &mut progress, Duration::from_secs(7));
        assert!(app.mic_no_audio_received.load(Ordering::Acquire));
    }

    #[test]
    fn exact_zero_mic_callbacks_name_the_input_and_suggest_a_physical_alternative() {
        let mut app = App::new(
            "meeting.md".into(),
            chrono::Local::now(),
            "MacBook Pro Microphone".into(),
        );
        app.current_mic_uid = Some("built-in".into());
        app.devices = vec![
            "MacBook Pro Microphone".into(),
            "BlackHole 2ch".into(),
            "Yeti Stereo Microphone".into(),
        ];
        app.device_uids = vec![
            Some("built-in".into()),
            Some("blackhole".into()),
            Some("yeti".into()),
        ];
        app.mic_rate = RATE;
        let mut progress = [SpoolProgress::default(), SpoolProgress::default()];
        // Fake callbacks delivered three seconds of exact-zero PCM. They did
        // reach the spool, so the missing-callback guard must stay clear.
        app.mic_frames.store(3 * RATE as u64, Ordering::Release);
        app.mic_real_spool_frames
            .store(3 * RATE as u64, Ordering::Release);
        app.mic_silence.store(3 * RATE as u64, Ordering::Release);
        update_no_audio_guard(&mut app, &mut progress, Duration::from_secs(3));
        assert!(app.mic_silent);
        assert!(!app.mic_no_audio_received.load(Ordering::Acquire));
        assert_eq!(
            app.suggested_mic_name.as_deref(),
            Some("Yeti Stereo Microphone")
        );
        assert_eq!(picker_preselection(&app), 2);
        for width in [80, 120] {
            let status = rendered_status_block(&mut app, width, 2);
            assert!(status.contains("mic █"), "{width}: {status}");
            assert!(status.contains("spk ░░░░░░░░"), "{width}: {status}");
            assert!(
                status.contains("mic silent: MacBook Pro Microphone"),
                "{width}: {status}"
            );
            assert!(
                status.contains("suggest: Yeti Stereo Microphone"),
                "{width}: {status}"
            );
        }
        app.mic_silence.store(0, Ordering::Release);
        update_no_audio_guard(&mut app, &mut progress, Duration::from_secs(4));
        assert!(!app.mic_silent);
    }

    #[test]
    fn suggestion_prefers_saved_uid_then_skips_virtual_loopbacks() {
        let names = vec![
            "MacBook Pro Microphone".into(),
            "BlackHole 2ch".into(),
            "Yeti Stereo Microphone".into(),
            "USB Digital Audio".into(),
        ];
        let uids = vec![
            Some("built-in".into()),
            Some("blackhole".into()),
            Some("yeti".into()),
            Some("usb".into()),
        ];
        assert_eq!(
            suggested_device_index(
                &names,
                &uids,
                &names[0],
                Some("built-in"),
                Some(&names[3]),
                Some("usb")
            ),
            Some(3)
        );
        assert_eq!(
            suggested_device_index(&names, &uids, &names[0], Some("built-in"), None, None),
            Some(2)
        );
    }

    #[test]
    fn missing_saved_mic_note_remains_visible_with_active_input_and_meters() {
        let mut app = App::new(
            "meeting.md".into(),
            chrono::Local::now(),
            "MacBook Pro Microphone".into(),
        );
        app.message = Some(
            "Saved mic 'Yeti Stereo Microphone' is unavailable; using system default. ^D to choose input"
                .into(),
        );
        for width in [80, 120] {
            let status = rendered_status_block(&mut app, width, 2);
            assert!(status.contains("mic █"), "{width}: {status}");
            assert!(status.contains("spk ░░░░░░░░"), "{width}: {status}");
            assert!(
                status.contains("input: MacBook Pro Microphone"),
                "{width}: {status}"
            );
            assert!(
                status.contains("Saved mic 'Yeti Stereo Microphone'"),
                "{width}: {status}"
            );
        }
    }

    #[test]
    fn startup_permission_nudge_waits_for_started_io_and_threshold() {
        assert!(!should_nudge_system_audio_permission(
            false,
            Duration::from_secs(10),
            RATE,
            0,
            0,
        ));
        assert!(!should_nudge_system_audio_permission(
            true,
            Duration::from_millis(9_999),
            RATE,
            0,
            0,
        ));
        assert!(should_nudge_system_audio_permission(
            true,
            Duration::from_secs(10),
            RATE,
            0,
            0,
        ));
    }

    #[test]
    fn startup_permission_nudge_requires_every_observed_frame_to_be_exact_zero() {
        let observed_frames = u64::from(RATE);

        assert!(!should_nudge_system_audio_permission(
            true,
            Duration::from_secs(10),
            RATE,
            observed_frames,
            observed_frames - 1,
        ));
        assert!(should_nudge_system_audio_permission(
            true,
            Duration::from_secs(10),
            RATE,
            observed_frames,
            observed_frames,
        ));
    }

    #[test]
    fn startup_permission_nudge_does_not_flag_a_live_noise_floor() {
        let observed_frames = u64::from(RATE) * TAP_PERMISSION_NUDGE_THRESHOLD_SECS;
        assert!(!should_nudge_system_audio_permission(
            true,
            Duration::from_secs(10),
            RATE,
            observed_frames,
            0,
        ));
    }

    fn rendered_status(app: &mut App, width: u16) -> String {
        app.mic_level.store(0.5f32.to_bits(), Ordering::Relaxed);
        let backend = TestBackend::new(width, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .last()
            .unwrap()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn rendered_status_block(app: &mut App, width: u16, lines: usize) -> String {
        app.mic_level.store(0.5f32.to_bits(), Ordering::Relaxed);
        let backend = TestBackend::new(width, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let rows = terminal.backend().buffer().content.chunks(width as usize);
        rows.skip(8 - lines)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn status_keeps_mic_meter_beside_each_system_audio_warning() {
        for width in [80, 120] {
            for (warning, visible_notice) in [
                (
                    margins_cli::error::macos_system_audio_permission_likely_message(
                        SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX,
                    ),
                    "macOS system-audio",
                ),
                (
                    "Computer audio silent 60s — mic still recording".to_string(),
                    "Computer audio silent",
                ),
            ] {
                let mut app = App::new("meeting.md".into(), chrono::Local::now(), "mic".into());
                app.spk_rate = RATE;
                app.spk_silence
                    .store(u64::from(RATE) * 60, Ordering::Relaxed);
                app.message = Some(warning);
                let status = rendered_status(&mut app, width);
                assert!(status.contains("mic █"), "{width}: {status}");
                assert!(status.contains("spk ░░░░░░░░ silent"), "{width}: {status}");
                assert!(status.contains(visible_notice), "{width}: {status}");
            }
        }
    }

    #[test]
    fn status_keeps_meters_in_normal_paused_storage_and_message_states() {
        for width in [80, 120] {
            let mut app = App::new("meeting.md".into(), chrono::Local::now(), "mic".into());
            for state in ["normal", "message", "paused", "storage"] {
                app.message = (state == "message").then(|| "Memo saved".into());
                app.capture_paused = state == "paused";
                app.native_store_retrying
                    .store(state == "storage", Ordering::Relaxed);
                let status = rendered_status(&mut app, width);
                assert!(status.contains("mic █"), "{width} {state}: {status}");
                assert!(status.contains("spk ░░░░░░░░"), "{width} {state}: {status}");
                if state == "paused" {
                    assert!(status.contains("PAUSED"), "{width}: {status}");
                }
                if state == "normal" || state == "paused" {
                    assert!(status.contains("^C stop"), "{width}: {status}");
                }
            }
        }
    }

    #[test]
    fn status_keeps_meters_in_merged_and_draft_conflict_views() {
        use margins_core::{MemoMoment, TimedMemoDocument, TimedMemoLine};

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".margins");
        margins_store::canonical::create_session(
            &dir,
            "meeting",
            &chrono::Local::now(),
            ".margins/meeting.md",
        )
        .unwrap();
        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
        let initial = authority.memo("meeting").unwrap();
        let line = |text| TimedMemoLine::at(text, MemoMoment::recording(1.0));
        let base = authority
            .replace_memo_lines("meeting", "tui", "base", &initial.revision, &[line("base")])
            .unwrap();
        let mut app = App::from_memo(
            TimedMemoDocument::from_committed(base.lines.clone()),
            dir.join("meeting.md").to_string_lossy().into_owned(),
            chrono::Local::now(),
            "mic".into(),
        );
        app.bind_workspace_authority(dir, "meeting".into());
        app.observe_memo(base.revision.clone(), base.lines);
        app.memo = TimedMemoDocument::from_committed(vec![line("local")]);
        authority
            .replace_memo_lines("meeting", "bb", "remote", &base.revision, &[line("remote")])
            .unwrap();
        assert!(app.save().is_err());

        for width in [80, 120] {
            let merged = rendered_status(&mut app, width);
            assert!(merged.contains("mic █"), "{width}: {merged}");
            assert!(merged.contains("CONFLICT:"), "{width}: {merged}");
            app.toggle_conflict_draft();
            let draft = rendered_status(&mut app, width);
            assert!(draft.contains("mic █"), "{width}: {draft}");
            assert!(draft.contains("CONFLICTING"), "{width}: {draft}");
            app.toggle_conflict_draft();
        }
    }

    #[test]
    fn recovered_system_audio_clears_owned_warnings_only() {
        for warning in [
            margins_cli::error::macos_system_audio_permission_likely_message(
                SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX,
            ),
            "Computer audio silent 60s — mic still recording".to_string(),
        ] {
            let mut message = Some(warning);
            assert!(clear_system_audio_warning_after_recovery(
                &mut message,
                u64::from(RATE),
                0,
            ));
            assert_eq!(message, None);
        }

        let mut unrelated = Some("Saved 3 lines".to_string());
        assert!(!clear_system_audio_warning_after_recovery(
            &mut unrelated,
            u64::from(RATE),
            0,
        ));
        assert_eq!(unrelated.as_deref(), Some("Saved 3 lines"));
    }

    #[test]
    fn empty_or_still_silent_tap_keeps_warning_visible() {
        for (frames, silence) in [(0, 0), (u64::from(RATE), u64::from(RATE))] {
            let mut message = Some(
                margins_cli::error::macos_system_audio_permission_likely_message(
                    SYSTEM_AUDIO_PERMISSION_WARNING_PREFIX,
                ),
            );
            assert!(!clear_system_audio_warning_after_recovery(
                &mut message,
                frames,
                silence,
            ));
            assert!(message.is_some());
        }
    }

    #[test]
    fn watermark_hint_only_renders_when_it_fits() {
        let status_width = 55;
        let combined_width = status_width + WATERMARK_HINT.len();

        assert_eq!(
            watermark_hint(combined_width as u16, status_width),
            Some(WATERMARK_HINT)
        );
        assert_eq!(
            watermark_hint((combined_width - 1) as u16, status_width),
            None
        );
    }
}
