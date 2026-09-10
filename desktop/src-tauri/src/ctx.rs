// ---------------------------------------------------------------------------
// Ctx — decoupled command context (WP1)
//
// Provides an abstraction over "how events are emitted" so that the ~52
// #[tauri::command] handlers can be extracted into `_impl` functions that
// compile and run without Tauri (e.g. inside an axum server, WP2).
// ---------------------------------------------------------------------------

use crate::AppState;
use std::sync::Arc;

/// Abstraction over the event-emission channel.
///
/// The Tauri desktop path implements this via `AppHandle::emit`; the axum web
/// path (WP2) will implement it via a per-connection WebSocket sender or SSE
/// stream.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);

    /// Returns true when the sink is connected to a web client rather than the
    /// Tauri WebView.  The `get_capabilities` command uses this to gate
    /// native-only features (audio tap, Obsidian, file dialogs, CLI install).
    fn is_web(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// TauriSink — wraps AppHandle for the desktop path
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
pub struct TauriSink(pub tauri::AppHandle);

// ---------------------------------------------------------------------------
// NoopSink — used when a command shim has no AppHandle (no events emitted)
// ---------------------------------------------------------------------------

pub struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

#[cfg(feature = "tauri-app")]
impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.0.emit(event, payload);
    }
}

// ---------------------------------------------------------------------------
// Ctx — command context passed to every `_impl` function
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Ctx {
    pub state: Arc<AppState>,
    pub sink: Arc<dyn EventSink>,
}

impl Ctx {
    pub fn emit(&self, event: &str, payload: impl serde::Serialize) {
        let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.sink.emit(event, value);
    }

    /// Build a Ctx for the Tauri desktop path with a TauriSink.
    #[cfg(feature = "tauri-app")]
    pub fn tauri(state: Arc<AppState>, app: tauri::AppHandle) -> Self {
        Self {
            state,
            sink: Arc::new(TauriSink(app)),
        }
    }

    /// Build a Ctx for commands that do not emit events (no-op sink).
    pub fn no_emit(state: Arc<AppState>) -> Self {
        Self {
            state,
            sink: Arc::new(NoopSink),
        }
    }
}
