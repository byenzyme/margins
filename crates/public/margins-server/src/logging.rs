//! Process-wide diagnostic logging for `margins-server`.

use std::io::Write as _;

/// Route `log` records to stderr: warnings and errors by default, overridable
/// with `RUST_LOG`. Stdout, including JSON output, is never written. A second
/// call is a no-op.
pub fn init_stderr_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
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
