use crate::error::CliError;
use std::io::Write;

pub fn public(stdout: &mut dyn Write) -> Result<(), CliError> {
    let report = serde_json::json!({
        "schema": 1,
        "product": "margins",
        "composition": "public-open-core",
        "official": false,
        "recall": {
            "available": false,
            "scan": false,
            "indexing": false,
            "lookup": false,
            "local_model": false,
        },
        "capture": {
            "available": false,
            "provider": "unavailable",
        },
        "tui": {
            "available": false,
        },
    });
    writeln!(stdout, "{report}").map_err(|error| CliError::from_anyhow(error.into()))
}
