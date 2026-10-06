//! Process-wide diagnostic logging for the `margins` binaries.

use std::io::Write as _;

/// Only the Workspace configuration warnings are on unless `RUST_LOG` says otherwise.
const DEFAULT_FILTER: &str = "margins_workflows=warn";

/// Route `log` records to stderr: Workspace warnings by default (other crates
/// stay silent, as before, so nothing new interleaves with the TUI), overridable
/// with `RUST_LOG`. Stdout, including JSON output, is never written. A second
/// call is a no-op.
pub fn init_stderr_logger() {
    let _ =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER))
            .target(env_logger::Target::Stderr)
            .format(|buf, record| {
                let level = match record.level() {
                    log::Level::Error => "error",
                    log::Level::Warn => "warning",
                    log::Level::Info => "info",
                    log::Level::Debug => "debug",
                    log::Level::Trace => "trace",
                };
                writeln!(buf, "margins: {level}: {}", record.args())
            })
            .try_init();
}
