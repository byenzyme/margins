use crate::error::CliError;
use crate::output::{line, xml_escape_attr};
use std::io::Write;
use std::path::Path;

pub fn set(work_dir: &Path, enabled: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let report = margins_workflows::archive::set_enabled(work_dir, enabled)
        .map_err(CliError::from_anyhow)?;
    render(&report, stdout)
}

pub fn status(work_dir: &Path, stdout: &mut dyn Write) -> Result<(), CliError> {
    let report = margins_workflows::archive::status(work_dir).map_err(CliError::from_anyhow)?;
    render(&report, stdout)
}

fn render(
    report: &margins_workflows::archive::ArchiveReport,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    line(
        stdout,
        format_args!(
            "<margins_archive enabled=\"{}\" path=\"{}\" moved=\"{}\" transcripts=\"{}\" />",
            report.enabled,
            xml_escape_attr(&report.path.to_string_lossy()),
            report.moved,
            report.transcripts
        ),
    )
    .map_err(CliError::from_anyhow)
}
