// ---------------------------------------------------------------------------
// events.rs — WebSocket broadcast sink for WP2
// ---------------------------------------------------------------------------

use crate::ctx::EventSink;
use tokio::sync::broadcast;

/// A broadcast-based EventSink that serialises events to JSON strings and
/// sends them over a tokio broadcast channel.  Individual WebSocket handlers
/// subscribe to the receiver end.
pub struct WsSink {
    pub tx: broadcast::Sender<String>,
}

impl WsSink {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

impl EventSink for WsSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let msg = serde_json::json!({
            "event": event,
            "payload": payload,
        });
        // Ignore the error — no receivers is normal when no WS clients are connected.
        let _ = self.tx.send(msg.to_string());
    }

    fn is_web(&self) -> bool {
        true
    }
}
