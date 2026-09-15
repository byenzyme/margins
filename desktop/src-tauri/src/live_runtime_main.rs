fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("margins-live {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("capabilities") => {
            println!(
                "{}",
                serde_json::json!({
                    "schema": 1,
                    "product": "margins-live",
                    "protocol_version": 1,
                    "recording": true,
                    "editable_notepad": true
                })
            );
            Ok(())
        }
        Some("--help" | "-h") => {
            println!(
                "Margins recorder\n\nUsage: margins-live [--help | --version | capabilities]\n\nWith no command, keeps local meeting capture available to apps such as bb."
            );
            Ok(())
        }
        Some(command) => anyhow::bail!("unknown command: {command}"),
        None => margins_desktop::run_live_runtime(),
    }
}
