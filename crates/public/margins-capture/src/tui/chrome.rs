//! Editor frame chrome: the one-shot ignition sweep around the border and the
//! recording indicator in the title.
//!
//! Every function here is a pure function of elapsed time so the animation can
//! be asserted without a real clock.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::time::Duration;

/// The bright head travels the full perimeter in this window.
pub(super) const SWEEP: Duration = Duration::from_millis(600);
/// After the sweep, the title types in one character per step.
pub(super) const TITLE_CHAR: Duration = Duration::from_millis(15);
/// Resuming from pause flashes the recording dot for this long.
pub(super) const RESUME_PULSE: Duration = Duration::from_millis(300);
/// One full bright→dim→bright breath of the recording dot.
const DOT_CYCLE: Duration = Duration::from_millis(2000);
/// Redraw cadence while an animation is in flight.
pub(super) const ANIMATION_FRAME: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColorMode {
    TrueColor,
    Basic,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Chrome {
    pub color: ColorMode,
    pub animate: bool,
}

impl Chrome {
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var("NO_COLOR").ok().as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("MARGINS_NO_ANIMATION").ok().as_deref(),
        )
    }

    /// `NO_COLOR` follows no-color.org: any non-empty value disables color.
    /// Without color the sweep would be invisible, so it disables motion too.
    pub fn from_vars(
        no_color: Option<&str>,
        colorterm: Option<&str>,
        no_anim: Option<&str>,
    ) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            return Self {
                color: ColorMode::None,
                animate: false,
            };
        }
        let color = match colorterm {
            Some(value)
                if value.eq_ignore_ascii_case("truecolor")
                    || value.eq_ignore_ascii_case("24bit") =>
            {
                ColorMode::TrueColor
            }
            _ => ColorMode::Basic,
        };
        let animate = !no_anim.is_some_and(|value| !value.is_empty() && value != "0");
        Self { color, animate }
    }

    /// Static chrome for render tests that only inspect text.
    #[cfg(test)]
    pub fn still() -> Self {
        Self {
            color: ColorMode::Basic,
            animate: false,
        }
    }

    fn rgb_or(&self, rgb: (u8, u8, u8), basic: Color) -> Option<Color> {
        match self.color {
            ColorMode::TrueColor => Some(Color::Rgb(rgb.0, rgb.1, rgb.2)),
            ColorMode::Basic => Some(basic),
            ColorMode::None => None,
        }
    }

    fn fg(color: Option<Color>) -> Style {
        color.map_or_else(Style::default, |color| Style::default().fg(color))
    }

    /// The settled frame color: the desktop app's dark-theme accent.
    pub fn accent(&self) -> Style {
        Self::fg(self.rgb_or(ACCENT, Color::Cyan))
    }

    /// The border before the head has reached it.
    fn faint(&self) -> Style {
        Self::fg(self.rgb_or(FAINT, Color::DarkGray))
    }

    /// The sweep head, and the tail color `distance` (0.0 head .. 1.0 settled).
    fn tail(&self, distance: f32) -> Style {
        match self.color {
            ColorMode::TrueColor => {
                let (r, g, b) = lerp_rgb(HIGHLIGHT, ACCENT, distance);
                let style = Style::default().fg(Color::Rgb(r, g, b));
                if distance == 0.0 {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                }
            }
            ColorMode::Basic => {
                let color = if distance < 0.34 {
                    Color::White
                } else if distance < 0.67 {
                    Color::LightCyan
                } else {
                    Color::Cyan
                };
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            }
            ColorMode::None => Style::default(),
        }
    }

    /// How long the intro (sweep plus title typing) lasts for this title.
    pub fn intro_length(&self, title_chars: usize) -> Duration {
        if !self.animate {
            return Duration::ZERO;
        }
        SWEEP + TITLE_CHAR * title_chars as u32
    }

    /// Whether the event loop should redraw at animation cadence.
    pub fn animating(
        &self,
        since_intro: Option<Duration>,
        since_resume: Option<Duration>,
        title_chars: usize,
    ) -> bool {
        self.animate
            && (since_intro.is_some_and(|t| t < self.intro_length(title_chars))
                || since_resume.is_some_and(|t| t < RESUME_PULSE))
    }
}

const ACCENT: (u8, u8, u8) = (0x8f, 0xa7, 0xb8);
const HIGHLIGHT: (u8, u8, u8) = (0xee, 0xf5, 0xf9);
const FAINT: (u8, u8, u8) = (0x3a, 0x46, 0x50);
const REC: (u8, u8, u8) = (0xe5, 0x53, 0x4b);
const REC_DIM: (u8, u8, u8) = (0x7a, 0x2e, 0x2a);
const PAUSED: (u8, u8, u8) = (0xa9, 0x95, 0x7d);

fn lerp_rgb(from: (u8, u8, u8), to: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// Cells of `area`'s border in clockwise order from the top-left corner.
fn perimeter(area: Rect) -> Vec<(u16, u16)> {
    if area.width < 2 || area.height < 2 {
        return Vec::new();
    }
    let (left, top) = (area.x, area.y);
    let (right, bottom) = (area.right() - 1, area.bottom() - 1);
    let mut cells = Vec::with_capacity(2 * (area.width + area.height) as usize);
    cells.extend((left..=right).map(|x| (x, top)));
    cells.extend((top + 1..=bottom).map(|y| (right, y)));
    cells.extend((left..right).rev().map(|x| (x, bottom)));
    cells.extend((top + 1..bottom).rev().map(|y| (left, y)));
    cells
}

/// Recolor the border of `area` for the sweep at `since_intro`. Returns false
/// (leaving the buffer untouched) once the sweep is over or disabled.
pub(super) fn paint_sweep(
    buf: &mut Buffer,
    area: Rect,
    chrome: &Chrome,
    since_intro: Duration,
) -> bool {
    if !chrome.animate || since_intro >= SWEEP {
        return false;
    }
    let cells = perimeter(area);
    if cells.is_empty() {
        return false;
    }
    let progress = since_intro.as_secs_f32() / SWEEP.as_secs_f32();
    let head = (progress * cells.len() as f32) as usize;
    let tail_len = (cells.len() / 6).max(4);
    for (index, &(x, y)) in cells.iter().enumerate() {
        let style = if index > head {
            chrome.faint()
        } else {
            let distance = head - index;
            if distance < tail_len {
                chrome.tail(distance as f32 / tail_len as f32)
            } else {
                chrome.accent()
            }
        };
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_style(style);
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Recording {
    Live,
    Paused,
}

/// Title text candidates, widest first. The recording indicator is always the
/// last thing standing.
fn title_candidates(state: Recording, session: &str, resumed: bool) -> [String; 3] {
    let (glyph, word) = match state {
        Recording::Live => ("●", "rec"),
        Recording::Paused => ("‖", "paused"),
    };
    let suffix = if resumed { " (resumed)" } else { "" };
    [
        format!(" {glyph} {word} · margins — {session}{suffix} "),
        format!(" {glyph} {word} · {session}{suffix} "),
        format!(" {glyph} {word} "),
    ]
}

/// The widest title that fits a border of `width` columns (corners excluded).
pub(super) fn title_text(state: Recording, session: &str, resumed: bool, width: u16) -> String {
    let room = usize::from(width.saturating_sub(2));
    title_candidates(state, session, resumed)
        .into_iter()
        .find(|title| Line::from(title.as_str()).width() <= room)
        .unwrap_or_default()
}

pub(super) struct TitleClock {
    /// Time since the intro began; `None` means it never ran (no animation).
    pub since_intro: Option<Duration>,
    pub since_resume: Option<Duration>,
    /// Phase source for the breathing dot.
    pub since_start: Duration,
}

/// Style the title, revealing only the characters typed so far.
pub(super) fn title_line(
    text: &str,
    state: Recording,
    chrome: &Chrome,
    clock: &TitleClock,
) -> Line<'static> {
    let visible = match clock.since_intro {
        Some(t) if chrome.animate => {
            if t < SWEEP {
                0
            } else {
                ((t - SWEEP).as_millis() / TITLE_CHAR.as_millis()) as usize + 1
            }
        }
        _ => usize::MAX,
    };
    let text: String = text.chars().take(visible).collect();
    if text.is_empty() {
        return Line::default();
    }

    // " ● rec · ..." — the leading space, glyph, and state word carry the
    // indicator style; the rest is ordinary title text.
    let indicator_end = text
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == ' ')
        .and_then(|(first_space, _)| {
            text[first_space + 1..]
                .find(' ')
                .map(|second| first_space + 1 + second)
        })
        .unwrap_or(text.len());
    let (indicator, rest) = text.split_at(indicator_end);
    let indicator_style = indicator_style(state, chrome, clock);
    Line::from(vec![
        Span::styled(indicator.to_string(), indicator_style),
        Span::raw(rest.to_string()),
    ])
}

fn indicator_style(state: Recording, chrome: &Chrome, clock: &TitleClock) -> Style {
    match state {
        Recording::Paused => match chrome.color {
            ColorMode::TrueColor => Style::default().fg(Color::Rgb(PAUSED.0, PAUSED.1, PAUSED.2)),
            ColorMode::Basic => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
            ColorMode::None => Style::default(),
        },
        Recording::Live => {
            let pulsing = chrome.animate && clock.since_resume.is_some_and(|t| t < RESUME_PULSE);
            if pulsing {
                return Chrome::fg(chrome.rgb_or(HIGHLIGHT, Color::White))
                    .add_modifier(Modifier::BOLD);
            }
            match chrome.color {
                ColorMode::None => Style::default().add_modifier(Modifier::BOLD),
                _ if !chrome.animate => {
                    Chrome::fg(chrome.rgb_or(REC, Color::Red)).add_modifier(Modifier::BOLD)
                }
                ColorMode::TrueColor => {
                    let (r, g, b) = lerp_rgb(REC, REC_DIM, breath(clock.since_start));
                    Style::default()
                        .fg(Color::Rgb(r, g, b))
                        .add_modifier(Modifier::BOLD)
                }
                ColorMode::Basic => {
                    let style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
                    if breath(clock.since_start) > 0.5 {
                        style.add_modifier(Modifier::DIM)
                    } else {
                        style
                    }
                }
            }
        }
    }
}

/// 0.0 at full brightness, 1.0 at the dimmest point of the cycle.
fn breath(t: Duration) -> f32 {
    let phase = (t.as_millis() % DOT_CYCLE.as_millis()) as f32 / DOT_CYCLE.as_millis() as f32;
    (1.0 - (phase * std::f32::consts::TAU).cos()) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRUE: Chrome = Chrome {
        color: ColorMode::TrueColor,
        animate: true,
    };

    #[test]
    fn env_selects_color_depth_and_honors_opt_outs() {
        assert_eq!(Chrome::from_vars(None, Some("truecolor"), None), TRUE);
        assert_eq!(
            Chrome::from_vars(None, Some("24bit"), None).color,
            ColorMode::TrueColor
        );
        assert_eq!(Chrome::from_vars(None, None, None).color, ColorMode::Basic);
        assert!(!Chrome::from_vars(None, None, Some("1")).animate);
        assert!(Chrome::from_vars(None, None, Some("0")).animate);
        assert_eq!(
            Chrome::from_vars(Some("1"), Some("truecolor"), None),
            Chrome {
                color: ColorMode::None,
                animate: false
            }
        );
        // An empty NO_COLOR is unset per no-color.org.
        assert_eq!(Chrome::from_vars(Some(""), Some("truecolor"), None), TRUE);
    }

    #[test]
    fn perimeter_runs_clockwise_from_top_left_without_repeats() {
        let cells = perimeter(Rect::new(2, 1, 4, 3));
        assert_eq!(
            cells,
            vec![
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 1),
                (5, 2),
                (5, 3),
                (4, 3),
                (3, 3),
                (2, 3),
                (2, 2)
            ]
        );
    }

    fn fg_at(buf: &Buffer, x: u16, y: u16) -> Color {
        buf.cell((x, y)).unwrap().fg
    }

    #[test]
    fn sweep_lights_the_path_behind_the_head_and_leaves_the_rest_faint() {
        let area = Rect::new(0, 0, 20, 6);
        let cells = perimeter(area);
        let mut buf = Buffer::empty(area);
        assert!(paint_sweep(&mut buf, area, &TRUE, SWEEP / 2));
        let head = cells.len() / 2;
        let faint = Color::Rgb(FAINT.0, FAINT.1, FAINT.2);
        let accent = Color::Rgb(ACCENT.0, ACCENT.1, ACCENT.2);
        let (hx, hy) = cells[head];
        assert_eq!(
            fg_at(&buf, hx, hy),
            Color::Rgb(HIGHLIGHT.0, HIGHLIGHT.1, HIGHLIGHT.2)
        );
        assert!(buf
            .cell((hx, hy))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD));
        let (ax, ay) = cells[0];
        assert_eq!(
            fg_at(&buf, ax, ay),
            accent,
            "far behind the head has settled"
        );
        let (fx, fy) = cells[head + 1];
        assert_eq!(
            fg_at(&buf, fx, fy),
            faint,
            "ahead of the head is not yet lit"
        );
        let (lx, ly) = *cells.last().unwrap();
        assert_eq!(fg_at(&buf, lx, ly), faint);
    }

    #[test]
    fn sweep_stops_at_its_deadline_and_when_animation_is_off() {
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        assert!(paint_sweep(&mut buf, area, &TRUE, Duration::ZERO));
        assert!(!paint_sweep(&mut buf, area, &TRUE, SWEEP));
        let still = Chrome {
            animate: false,
            ..TRUE
        };
        assert!(!paint_sweep(&mut buf, area, &still, Duration::ZERO));
    }

    #[test]
    fn title_shrinks_by_dropping_brand_then_name_but_keeps_the_indicator() {
        let full = " ● rec · margins — standup (resumed) ";
        assert_eq!(title_text(Recording::Live, "standup", true, 80), full);
        assert_eq!(
            title_text(Recording::Live, "standup", true, 30),
            " ● rec · standup (resumed) "
        );
        assert_eq!(title_text(Recording::Live, "standup", false, 12), " ● rec ");
        assert_eq!(
            title_text(Recording::Paused, "standup", false, 80),
            " ‖ paused · margins — standup "
        );
        assert_eq!(title_text(Recording::Live, "standup", false, 4), "");
    }

    fn clock(intro_ms: Option<u64>, resume_ms: Option<u64>, start_ms: u64) -> TitleClock {
        TitleClock {
            since_intro: intro_ms.map(Duration::from_millis),
            since_resume: resume_ms.map(Duration::from_millis),
            since_start: Duration::from_millis(start_ms),
        }
    }

    fn text_of(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn title_is_hidden_during_the_sweep_then_types_in() {
        let text = " ● rec · margins — standup ";
        let during = title_line(text, Recording::Live, &TRUE, &clock(Some(300), None, 300));
        assert_eq!(text_of(&during), "");
        let typed = title_line(
            text,
            Recording::Live,
            &TRUE,
            &clock(Some(600 + 15 * 3), None, 645),
        );
        assert_eq!(text_of(&typed), " ● r");
        let done = title_line(
            text,
            Recording::Live,
            &TRUE,
            &clock(Some(5_000), None, 5_000),
        );
        assert_eq!(text_of(&done), text);
        assert_eq!(done.spans[0].content, " ● rec");
        assert_eq!(done.spans[1].content, " · margins — standup ");
        let still = Chrome {
            animate: false,
            ..TRUE
        };
        let instant = title_line(text, Recording::Live, &still, &clock(Some(0), None, 0));
        assert_eq!(text_of(&instant), text);
    }

    #[test]
    fn rec_dot_breathes_pulses_on_resume_and_holds_when_paused() {
        let text = " ● rec · margins ";
        let style_at = |state, resume, start| {
            title_line(text, state, &TRUE, &clock(Some(10_000), resume, start)).spans[0].style
        };
        let rec = Color::Rgb(REC.0, REC.1, REC.2);
        let dim = Color::Rgb(REC_DIM.0, REC_DIM.1, REC_DIM.2);
        assert_eq!(style_at(Recording::Live, None, 0).fg, Some(rec));
        assert_eq!(style_at(Recording::Live, None, 1_000).fg, Some(dim));
        assert_eq!(style_at(Recording::Live, None, 2_000).fg, Some(rec));
        assert_eq!(
            style_at(Recording::Live, Some(100), 1_000).fg,
            Some(Color::Rgb(HIGHLIGHT.0, HIGHLIGHT.1, HIGHLIGHT.2))
        );
        assert_eq!(style_at(Recording::Live, Some(400), 1_000).fg, Some(dim));
        let paused = Color::Rgb(PAUSED.0, PAUSED.1, PAUSED.2);
        assert_eq!(style_at(Recording::Paused, None, 0).fg, Some(paused));
        assert_eq!(style_at(Recording::Paused, None, 1_000).fg, Some(paused));
    }

    #[test]
    fn no_color_keeps_a_bold_steady_indicator() {
        let none = Chrome {
            color: ColorMode::None,
            animate: false,
        };
        let line = title_line(
            " ● rec · margins ",
            Recording::Live,
            &none,
            &clock(None, None, 1_000),
        );
        assert_eq!(
            line.spans[0].style,
            Style::default().add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn animation_cadence_covers_intro_and_resume_pulse_only() {
        let chars = 20;
        let intro = TRUE.intro_length(chars);
        assert_eq!(intro, SWEEP + TITLE_CHAR * 20);
        assert!(TRUE.animating(Some(Duration::ZERO), None, chars));
        assert!(!TRUE.animating(Some(intro), None, chars));
        assert!(TRUE.animating(Some(intro), Some(Duration::from_millis(10)), chars));
        assert!(!TRUE.animating(None, Some(RESUME_PULSE), chars));
        let still = Chrome {
            animate: false,
            ..TRUE
        };
        assert!(!still.animating(Some(Duration::ZERO), Some(Duration::ZERO), chars));
    }
}
