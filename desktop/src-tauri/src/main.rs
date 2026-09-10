#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "tauri-app")]
fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "Margins desktop {}\n\nUsage: margins-desktop [--help] [--version]",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            "-V" | "--version" => {
                println!("margins-desktop {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {}
        }
    }

    margins_desktop::run()
}

#[cfg(not(feature = "tauri-app"))]
fn main() {
    eprintln!("margins-desktop requires the tauri-app feature; use margins-server instead");
}
