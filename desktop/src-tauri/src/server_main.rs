// ---------------------------------------------------------------------------
// margins-server — headless axum HTTP/WebSocket server entry point (WP2)
//
// All startup logic lives in margins_desktop::server::run() so it can access
// pub(crate) items from the library crate.
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    margins_desktop::server::run()
}
