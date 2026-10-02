//! The public CLI has no recording device. The official native TUI composes
//! capture through the in-process meeting runtime.

use crate::error::CliError;
use crate::services::CliServices;
use std::path::Path;

pub fn run(
    _services: &CliServices,
    _work_dir: &Path,
    _selected: Option<&str>,
    _new_title: Option<&str>,
    _create_new: bool,
    _create_if_missing: bool,
) -> Result<(), CliError> {
    Err(CliError::capture_unavailable())
}
