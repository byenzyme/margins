// ---------------------------------------------------------------------------
// margins-server — headless axum HTTP/WebSocket server entry point (WP2)
//
// All startup logic lives in margins_desktop::server::run() so it can access
// pub(crate) items from the library crate.
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--prepare-asr") {
        return margins_desktop::server::prepare_asr();
    }
    margins_desktop::server::run()
}
