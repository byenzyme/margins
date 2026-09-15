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

/// Duration of continuous speaker dead-silence (exact zeros) before warning about
/// the computer-audio tap. A live tap with nobody talking often still produces a
/// noise floor, but exact silence is not enough evidence to interrupt capture.
const TAP_WARNING_THRESHOLD_SECS: u64 = 60;
const WATERMARK_HINT: &str = "  |  agent /watermark: live read";

fn watermark_hint(available_width: u16, status_width: usize) -> Option<&'static str> {
    (status_width.saturating_add(WATERMARK_HINT.len()) <= usize::from(available_width))
        .then_some(WATERMARK_HINT)
}

/// Observe only the tap's startup window. A denied macOS process tap commonly
/// starts IO successfully but then supplies no frames or only exact-zero frames.
const TAP_PERMISSION_NUDGE_THRESHOLD_SECS: u64 = 3;
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
    // Reaching the TUI means system-audio IO returned Started. Probe once near
    // startup; later quiet periods remain under the existing 60-second warning.
    let system_audio_io_started_at = std::time::Instant::now();
    let mut system_audio_permission_probe_pending = cfg!(target_os = "macos");

    loop {
        terminal.draw(|f| render(f, app))?;

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

        // Warn about sustained exact silence after the one-shot permission
        // probe, but do not restart the whole capture. Mic audio and memo
        // timing should remain continuous.
        if app.spk_rate > 0 {
            let silent_samples = app.spk_silence.load(Ordering::Relaxed);
            let silent_secs = silent_samples / app.spk_rate as u64;
            if silent_secs >= TAP_WARNING_THRESHOLD_SECS {
                app.message = Some(format!(
                    "Computer audio silent {}s — mic still recording",
                    silent_secs
                ));
            }
        }

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
            let _ = app.save();
        }

        // Device select
        KeyCode::Char('d') if ctrl => {
            let devices = crate::recorder::list_input_devices();
            app.devices = devices.iter().map(|(name, _)| name.clone()).collect();
            // Pre-select the current mic
            app.selected_device = app
                .devices
                .iter()
                .position(|n| *n == app.current_mic_name)
                .unwrap_or(0);
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    let editor_area = chunks[0];
    let status_area = chunks[1];

    // Store for mouse click handling
    app.editor_area = editor_area;

    let visible_lines = editor_area.height.saturating_sub(2) as usize;
    app.ensure_cursor_visible(visible_lines);

    // Build visible lines
    let mut display_lines: Vec<Line> = Vec::new();
    let end = (app.scroll + visible_lines).min(app.memo.len());

    for i in app.scroll..end {
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
            Span::styled(&app.memo.line(i).unwrap().text, text_style),
        ]));
    }

    let block = Block::default().borders(Borders::ALL).title(" margins ");
    let paragraph = Paragraph::new(display_lines).block(block);
    f.render_widget(paragraph, editor_area);

    // Cursor
    let cursor_x = editor_area.x + 1 + GUTTER_WIDTH + app.cursor_col as u16;
    let cursor_y = editor_area.y + 1 + (app.cursor_line - app.scroll) as u16;
    if cursor_x < editor_area.x + editor_area.width - 1
        && cursor_y < editor_area.y + editor_area.height - 1
    {
        f.set_cursor_position((cursor_x, cursor_y));
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

    // Build warning suffix for drops or silence
    let mut warnings = String::new();
    if mic_drops > 0 || spk_drops > 0 {
        warnings.push_str(&format!(" DROPS mic:{} spk:{}", mic_drops, spk_drops));
    }
    if app.spk_rate > 0 {
        let silent_samples = app.spk_silence.load(Ordering::Relaxed);
        let silent_secs = silent_samples / app.spk_rate as u64;
        if silent_secs >= 3 {
            warnings.push_str(&format!(" SPK SILENT {}s", silent_secs));
        }
    }

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
    let status_text = if let Some(ref msg) = app.message {
        format!(" {} | {}", time, msg)
    } else if app.capture_paused {
        format!(
            " {} | PAUSED{} | {} lines |  ^P resume  ^S save  ^C stop",
            time,
            remote_delivery,
            app.memo.len(),
        )
    } else {
        format!(
            " {} | {} lines{} | mic {} spk {} |{}  ^P pause  ^D device  ^S save  ^C stop",
            time,
            app.memo.len(),
            remote_delivery,
            level_meter(mic_peak, 8),
            level_meter(spk_peak, 8),
            warnings,
        )
    };

    let hint = watermark_hint(status_area.width, Line::from(status_text.as_str()).width());
    let mut status_spans = vec![Span::raw(status_text)];
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

    let status = Paragraph::new(Line::from(status_spans)).style(
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
    use std::time::Duration;

    const RATE: u32 = 48_000;

    #[test]
    fn startup_permission_nudge_waits_for_started_io_and_threshold() {
        assert!(!should_nudge_system_audio_permission(
            false,
            Duration::from_secs(3),
            RATE,
            0,
            0,
        ));
        assert!(!should_nudge_system_audio_permission(
            true,
            Duration::from_millis(2_999),
            RATE,
            0,
            0,
        ));
        assert!(should_nudge_system_audio_permission(
            true,
            Duration::from_secs(3),
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
            Duration::from_secs(3),
            RATE,
            observed_frames,
            observed_frames - 1,
        ));
        assert!(should_nudge_system_audio_permission(
            true,
            Duration::from_secs(3),
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
            Duration::from_secs(3),
            RATE,
            observed_frames,
            0,
        ));
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
