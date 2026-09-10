//! Rust-owned lifecycle for the floating capture mark and one Pad window per
//! active capture identity. These windows reflect recording state; closing,
//! hiding, moving, or crashing one never owns or mutates capture lifecycle.

use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::aux_layout::{MonitorRect, WindowPlacement};
use crate::recording::RecordingPhase;

pub(crate) const CIRCLE_LABEL: &str = "capture-circle";
pub(crate) const CIRCLE_WIDTH: f64 = 112.0;
pub(crate) const CIRCLE_HEIGHT: f64 = 104.0;
pub(crate) const PAD_WIDTH: f64 = 340.0;
pub(crate) const PAD_HEIGHT: f64 = 460.0;
const MIN_VISIBLE: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CirclePhase {
    Idle,
    Starting,
    Recording,
    Paused,
    Finalizing,
    NeedsAttention,
}

pub(crate) fn circle_should_show(phase: CirclePhase) -> bool {
    phase != CirclePhase::Idle
}

pub(crate) fn circle_phase_from(recording: Option<RecordingPhase>, starting: bool) -> CirclePhase {
    match recording {
        Some(RecordingPhase::Recording) => CirclePhase::Recording,
        Some(RecordingPhase::Pausing) | Some(RecordingPhase::Paused) => CirclePhase::Paused,
        Some(RecordingPhase::Finalizing) => CirclePhase::Finalizing,
        Some(RecordingPhase::NeedsAttention) => CirclePhase::NeedsAttention,
        None if starting => CirclePhase::Starting,
        None => CirclePhase::Idle,
    }
}

pub(crate) fn circle_phase_slug(phase: CirclePhase) -> &'static str {
    match phase {
        CirclePhase::Idle => "idle",
        CirclePhase::Starting => "starting",
        CirclePhase::Recording => "recording",
        CirclePhase::Paused => "paused",
        CirclePhase::Finalizing => "finalizing",
        CirclePhase::NeedsAttention => "needs_attention",
    }
}

pub(crate) fn pad_window_label(work_dir: &Path, session_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(work_dir.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(session_name.as_bytes());
    let digest = hasher.finalize();
    let suffix: String = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("pad-{suffix}")
}

pub(crate) fn is_pad_label(label: &str) -> bool {
    label.starts_with("pad-")
}

pub(crate) fn is_aux_label(label: &str) -> bool {
    label == CIRCLE_LABEL || is_pad_label(label)
}

fn default_circle_placement(primary: MonitorRect) -> WindowPlacement {
    WindowPlacement {
        x: primary.x + primary.width - CIRCLE_WIDTH - 24.0,
        y: primary.y + 48.0,
        width: CIRCLE_WIDTH,
        height: CIRCLE_HEIGHT,
    }
}

fn default_pad_placement(primary: MonitorRect) -> WindowPlacement {
    WindowPlacement {
        x: primary.x + 120.0,
        y: primary.y + 120.0,
        width: PAD_WIDTH,
        height: PAD_HEIGHT,
    }
}

#[cfg(feature = "tauri-app")]
mod native {
    use std::sync::Arc;

    use tauri::{AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

    use super::*;
    use crate::aux_layout::{self, AuxLayout};
    use crate::AppState;

    /// Keep the capture circle above ordinary windows from other applications.
    /// Tauri's builder flag remains the portable baseline; on macOS, assert the
    /// actual AppKit floating level without changing Spaces/full-screen policy.
    fn apply_circle_window_level(window: &tauri::WebviewWindow) {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_always_on_top(true);
        }

        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::{NSFloatingWindowLevel, NSWindow};

            let native_window = window.clone();
            if let Err(error) = window.run_on_main_thread(move || {
                let Ok(ns_window) = native_window.ns_window() else {
                    eprintln!("margins: could not resolve capture-circle NSWindow");
                    return;
                };
                // Tauri owns this NSWindow for at least as long as the cloned
                // WebviewWindow captured by the main-thread closure.
                let ns_window: &NSWindow = unsafe { &*ns_window.cast() };
                ns_window.setLevel(NSFloatingWindowLevel);
            }) {
                eprintln!("margins: could not apply capture-circle window level ({error})");
            }
        }
    }

    fn monitors(app: &AppHandle) -> Vec<MonitorRect> {
        let Some(anchor) = app
            .get_webview_window("main")
            .or_else(|| app.webview_windows().into_values().next())
        else {
            return Vec::new();
        };
        anchor
            .available_monitors()
            .unwrap_or_default()
            .into_iter()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                MonitorRect::from_physical(
                    position.x as f64,
                    position.y as f64,
                    size.width as f64,
                    size.height as f64,
                    monitor.scale_factor(),
                )
            })
            .collect()
    }

    fn primary_monitor(app: &AppHandle) -> MonitorRect {
        monitors(app)
            .into_iter()
            .next()
            .unwrap_or_else(|| MonitorRect::new(0.0, 0.0, 1440.0, 900.0))
    }

    fn resolved_placement(
        app: &AppHandle,
        layout: &AuxLayout,
        label: &str,
        fallback: WindowPlacement,
    ) -> WindowPlacement {
        aux_layout::clamp_to_monitors(
            layout.get(label).unwrap_or(fallback),
            &monitors(app),
            MIN_VISIBLE,
        )
    }

    pub(crate) fn remember_placement(app: &AppHandle, label: &str, config_dir: &Path) {
        let Some(window) = app.get_webview_window(label) else {
            return;
        };
        let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
            return;
        };
        let scale = window.scale_factor().unwrap_or(1.0);
        let mut layout = aux_layout::load_layout(config_dir);
        layout.set(
            label,
            WindowPlacement::from_physical(
                position.x as f64,
                position.y as f64,
                size.width as f64,
                size.height as f64,
                scale,
            ),
        );
        if let Err(error) = aux_layout::save_layout(config_dir, &layout) {
            eprintln!("margins: auxiliary window placement was not saved ({error})");
        }
    }

    pub(crate) fn reconcile_circle(app: &AppHandle, phase: CirclePhase, config_dir: &Path) {
        let existing = app.get_webview_window(CIRCLE_LABEL);
        match (circle_should_show(phase), existing) {
            (false, Some(window)) => {
                remember_placement(app, CIRCLE_LABEL, config_dir);
                let _ = window.destroy();
            }
            (false, None) => {}
            (true, Some(window)) => {
                let _ = window.emit("circle-phase", circle_phase_slug(phase));
                apply_circle_window_level(&window);
                let _ = window.show();
            }
            (true, None) => {
                let layout = aux_layout::load_layout(config_dir);
                let placement = resolved_placement(
                    app,
                    &layout,
                    CIRCLE_LABEL,
                    default_circle_placement(primary_monitor(app)),
                );
                let url = format!("circle.html?phase={}", circle_phase_slug(phase));
                match WebviewWindowBuilder::new(app, CIRCLE_LABEL, WebviewUrl::App(url.into()))
                    .title("Margins capture")
                    .inner_size(placement.width, placement.height)
                    .position(placement.x, placement.y)
                    .resizable(false)
                    .decorations(false)
                    .transparent(true)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .visible(true)
                    .build()
                {
                    Ok(window) => {
                        let _ = window.set_position(LogicalPosition::new(placement.x, placement.y));
                        apply_circle_window_level(&window);
                    }
                    Err(error) => {
                        eprintln!("margins: could not open capture mark ({error})");
                    }
                }
            }
        }
    }

    pub(crate) fn open_or_focus_pad(
        app: &AppHandle,
        work_dir: &Path,
        session_name: &str,
        project_id: Option<&str>,
        config_dir: &Path,
    ) -> Result<String, String> {
        let canonical = std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
        let label = pad_window_label(&canonical, session_name);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.show();
            let _ = window.set_focus();
            return Ok(label);
        }

        let layout = aux_layout::load_layout(config_dir);
        let placement = resolved_placement(
            app,
            &layout,
            &label,
            default_pad_placement(primary_monitor(app)),
        );
        let mut url = format!("pad.html?session={}", urlencode(session_name));
        if let Some(project_id) = project_id {
            url.push_str(&format!("&project={}", urlencode(project_id)));
        }
        let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
            .title(format!("{session_name} — Pad"))
            .inner_size(placement.width, placement.height)
            .position(placement.x, placement.y)
            .min_inner_size(280.0, 320.0)
            .resizable(true)
            .decorations(false)
            .transparent(true)
            .skip_taskbar(true)
            .visible(true)
            .build()
            .map_err(|error| format!("Could not open Pad: {error}"))?;
        let _ = window.set_position(LogicalPosition::new(placement.x, placement.y));
        let _ = window.set_focus();
        Ok(label)
    }

    pub(crate) fn toggle_pad(
        app: &AppHandle,
        work_dir: &Path,
        session_name: &str,
        project_id: Option<&str>,
        config_dir: &Path,
    ) -> Result<Option<String>, String> {
        let canonical = std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
        let label = pad_window_label(&canonical, session_name);
        if let Some(window) = app.get_webview_window(&label) {
            if window.is_visible().unwrap_or(false) {
                remember_placement(app, &label, config_dir);
                let _ = window.hide();
                return Ok(None);
            }
        }
        open_or_focus_pad(app, work_dir, session_name, project_id, config_dir).map(Some)
    }

    pub(crate) fn mark_starting(app: &AppHandle) {
        reconcile_circle(
            app,
            CirclePhase::Starting,
            &crate::settings::default_work_dir(),
        );
    }

    pub(crate) fn mark_finalizing(app: &AppHandle) {
        reconcile_circle(
            app,
            CirclePhase::Finalizing,
            &crate::settings::default_work_dir(),
        );
    }

    fn close_pads(app: &AppHandle, config_dir: &Path) {
        let labels: Vec<String> = app
            .webview_windows()
            .into_keys()
            .filter(|label| is_pad_label(label))
            .collect();
        for label in labels {
            remember_placement(app, &label, config_dir);
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
        }
    }

    pub(crate) fn reconcile_from_state(app: &AppHandle, state: &Arc<AppState>) {
        let recording = state
            .recording
            .lock()
            .unwrap()
            .as_ref()
            .map(|recording| recording.phase);
        let starting = state.recording_startup_cancel.lock().unwrap().is_some();
        let phase = circle_phase_from(recording, starting);
        let config_dir = crate::settings::default_work_dir();
        reconcile_circle(app, phase, &config_dir);
        if phase == CirclePhase::Idle {
            close_pads(app, &config_dir);
        }
    }

    fn urlencode(value: &str) -> String {
        value
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (byte as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    }
}

#[cfg(feature = "tauri-app")]
pub(crate) use native::{
    mark_finalizing, mark_starting, reconcile_from_state, remember_placement, toggle_pad,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn circle_visibility_tracks_active_capture_phases() {
        assert!(!circle_should_show(CirclePhase::Idle));
        for phase in [
            CirclePhase::Starting,
            CirclePhase::Recording,
            CirclePhase::Paused,
            CirclePhase::Finalizing,
            CirclePhase::NeedsAttention,
        ] {
            assert!(circle_should_show(phase));
        }
    }

    #[test]
    fn pad_identity_is_stable_without_cross_meeting_collisions() {
        let first_dir = PathBuf::from("/tmp/vault-a");
        let second_dir = PathBuf::from("/tmp/vault-b");
        let label = pad_window_label(&first_dir, "customer-call");
        assert_eq!(label, pad_window_label(&first_dir, "customer-call"));
        assert_ne!(label, pad_window_label(&first_dir, "other-call"));
        assert_ne!(label, pad_window_label(&second_dir, "customer-call"));
        assert!(is_aux_label(&label));
        assert!(!is_aux_label("main"));
    }

    #[test]
    fn capability_is_scoped_to_auxiliary_window_mechanics() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities/aux-windows.json");
        let raw = std::fs::read_to_string(path).expect("auxiliary capability exists");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("valid capability JSON");
        let windows = json["windows"].as_array().unwrap();
        assert!(windows.iter().any(|value| value == CIRCLE_LABEL));
        assert!(windows.iter().any(|value| value == "pad-*"));
        let permissions = json["permissions"].as_array().unwrap();
        for required in [
            "core:event:default",
            "core:window:allow-start-dragging",
            "core:window:allow-set-size",
            "core:window:allow-cursor-position",
            "core:window:allow-outer-position",
            "core:window:allow-scale-factor",
            "core:window:allow-set-ignore-cursor-events",
        ] {
            assert!(permissions.iter().any(|value| value == required));
        }
        assert!(!permissions.iter().any(|value| value == "core:default"));
        assert!(!permissions
            .iter()
            .any(|value| value == "core:window:default"));
    }
}
