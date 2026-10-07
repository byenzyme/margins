use crate::error::CliError;
use std::io::{self, Write};

pub fn xml_escape_text(value: &str) -> String {
    value
        .chars()
        .filter_map(xml_char)
        .flat_map(|character| match character {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            _ => vec![character],
        })
        .collect()
}

pub fn xml_escape_attr(value: &str) -> String {
    xml_escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_char(character: char) -> Option<char> {
    (character == '\n' || character == '\r' || character == '\t' || !character.is_control())
        .then_some(character)
}

/// Whether errors are for a person at a terminal: plain text instead of the
/// `<margins_error>` element that agents, scripts, and the bb plugin parse.
static PLAIN_ERRORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called once by a process entry point: plain errors when stderr is a
/// terminal and the command did not ask for `--json`. Anything else (pipes,
/// agents, the plugin, tests) keeps the XML and JSON forms unchanged.
pub fn choose_error_form(args: &[std::ffi::OsString]) {
    use std::io::IsTerminal;
    let json = args.iter().any(|arg| arg == "--json");
    PLAIN_ERRORS.store(
        !json && io::stderr().is_terminal(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

pub fn plain_errors() -> bool {
    PLAIN_ERRORS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn write_error(stderr: &mut dyn Write, error: &CliError) -> io::Result<()> {
    write_error_parts(stderr, error.code(), error.message())
}

/// [`write_error`] for a code and message without a [`CliError`].
pub fn write_error_parts(stderr: &mut dyn Write, code: &str, message: &str) -> io::Result<()> {
    if plain_errors() {
        return writeln!(stderr, "margins: {message}");
    }
    writeln!(
        stderr,
        "<margins_error code=\"{}\">{}</margins_error>",
        xml_escape_attr(code),
        xml_escape_text(message)
    )
}

pub fn write_json_error(stderr: &mut dyn Write, error: &CliError) -> io::Result<()> {
    serde_json::to_writer(
        &mut *stderr,
        &serde_json::json!({
            "schema_version": "margins.error.v1",
            "ok": false,
            "error": {
                "code": error.code(),
                "message": error.message(),
                "retryable": error.retryable_value(),
                "details": error.details(),
            }
        }),
    )?;
    writeln!(stderr)
}

pub fn line(output: &mut dyn Write, args: std::fmt::Arguments<'_>) -> anyhow::Result<()> {
    output.write_fmt(args)?;
    output.write_all(b"\n")?;
    Ok(())
}
