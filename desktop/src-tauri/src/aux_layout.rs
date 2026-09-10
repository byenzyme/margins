//! Presentation-only placement math and persistence for auxiliary windows.
//!
//! Geometry is stored in logical points so a saved position survives display
//! scale changes. A corrupt or missing layout is never load-bearing: callers
//! simply fall back to a default placement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct WindowPlacement {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl WindowPlacement {
    pub(crate) fn from_physical(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Self {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        Self {
            x: x / scale,
            y: y / scale,
            width: width / scale,
            height: height / scale,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MonitorRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl MonitorRect {
    pub(crate) fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn from_physical(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Self {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        Self::new(x / scale, y / scale, width / scale, height / scale)
    }
}

const GRAB_STRIP: f64 = 28.0;

fn overlap_len(a0: f64, a_len: f64, b0: f64, b_len: f64) -> f64 {
    ((a0 + a_len).min(b0 + b_len) - a0.max(b0)).max(0.0)
}

pub(crate) fn is_reachable(
    placement: &WindowPlacement,
    monitors: &[MonitorRect],
    min_visible: f64,
) -> bool {
    monitors.iter().any(|monitor| {
        overlap_len(placement.x, placement.width, monitor.x, monitor.width) >= min_visible
            && placement.y >= monitor.y
            && placement.y <= monitor.y + monitor.height - GRAB_STRIP
    })
}

fn distance_to_monitor(placement: &WindowPlacement, monitor: &MonitorRect) -> f64 {
    let center_x = placement.x + placement.width / 2.0;
    let center_y = placement.y + placement.height / 2.0;
    let dx = (monitor.x - center_x)
        .max(0.0)
        .max(center_x - (monitor.x + monitor.width));
    let dy = (monitor.y - center_y)
        .max(0.0)
        .max(center_y - (monitor.y + monitor.height));
    dx * dx + dy * dy
}

pub(crate) fn clamp_to_monitors(
    placement: WindowPlacement,
    monitors: &[MonitorRect],
    min_visible: f64,
) -> WindowPlacement {
    if monitors.is_empty() || is_reachable(&placement, monitors, min_visible) {
        return placement;
    }
    let monitor = monitors
        .iter()
        .min_by(|left, right| {
            distance_to_monitor(&placement, left)
                .partial_cmp(&distance_to_monitor(&placement, right))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("empty monitors returned above");
    let width = placement.width.min(monitor.width);
    let height = placement.height.min(monitor.height);
    WindowPlacement {
        x: placement.x.clamp(
            monitor.x,
            (monitor.x + monitor.width - width).max(monitor.x),
        ),
        y: placement.y.clamp(
            monitor.y,
            (monitor.y + monitor.height - height).max(monitor.y),
        ),
        width,
        height,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AuxLayout {
    #[serde(default)]
    windows: BTreeMap<String, WindowPlacement>,
}

impl AuxLayout {
    pub(crate) fn get(&self, label: &str) -> Option<WindowPlacement> {
        self.windows.get(label).copied()
    }

    pub(crate) fn set(&mut self, label: &str, placement: WindowPlacement) {
        self.windows.insert(label.to_string(), placement);
    }
}

fn layout_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".margins").join("aux-layout.json")
}

pub(crate) fn load_layout(config_dir: &Path) -> AuxLayout {
    std::fs::read(layout_path(config_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_layout(config_dir: &Path, layout: &AuxLayout) -> Result<(), String> {
    let path = layout_path(config_dir);
    let parent = path
        .parent()
        .ok_or("Auxiliary layout path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(layout).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_VISIBLE: f64 = 64.0;

    fn primary() -> MonitorRect {
        MonitorRect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    #[test]
    fn reachable_placement_is_unchanged() {
        let placement = WindowPlacement {
            x: 1600.0,
            y: 40.0,
            width: 260.0,
            height: 360.0,
        };
        assert_eq!(
            clamp_to_monitors(placement, &[primary()], MIN_VISIBLE),
            placement
        );
    }

    #[test]
    fn removed_display_placement_is_recovered() {
        let placement = WindowPlacement {
            x: 5000.0,
            y: 4000.0,
            width: 340.0,
            height: 460.0,
        };
        let clamped = clamp_to_monitors(placement, &[primary()], MIN_VISIBLE);
        assert!(is_reachable(&clamped, &[primary()], MIN_VISIBLE));
    }

    #[test]
    fn physical_geometry_becomes_logical_geometry() {
        assert_eq!(
            WindowPlacement::from_physical(200.0, 100.0, 520.0, 720.0, 2.0),
            WindowPlacement {
                x: 100.0,
                y: 50.0,
                width: 260.0,
                height: 360.0,
            }
        );
    }

    #[test]
    fn layout_round_trips_and_corruption_degrades_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = AuxLayout::default();
        let placement = WindowPlacement {
            x: 100.0,
            y: 120.0,
            width: 340.0,
            height: 460.0,
        };
        layout.set("pad-example", placement);
        save_layout(dir.path(), &layout).unwrap();
        assert_eq!(load_layout(dir.path()).get("pad-example"), Some(placement));

        std::fs::write(layout_path(dir.path()), b"{not json").unwrap();
        assert!(load_layout(dir.path()).get("pad-example").is_none());
    }
}
