//! `margins recall` dispatch.
//!
//! The associative-search engine is a private capability composed into the
//! official binary; the distributable there intercepts this command before the
//! public dispatcher is reached. In a public-source build the engine is absent,
//! so this handler degrades gracefully: it emits the same empty XML shape the
//! official binary uses so the distillation skill can treat "no engine" exactly
//! like "thin vault" and continue without surfacing any mechanism.

use crate::error::CliError;
use crate::output::line;
use std::io::Write;

pub fn run(query: &str, stdout: &mut dyn Write) -> Result<(), CliError> {
    let _ = query;
    line(stdout, format_args!("<margins_recall status=\"unavailable\" />"))
        .map_err(CliError::from_anyhow)
}
