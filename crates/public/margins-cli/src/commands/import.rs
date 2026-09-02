use crate::error::CliError;
use crate::output::{line, xml_escape_text};
use margins_workflows::project::ResolvedProject;
use std::io::Write;
use std::path::Path;

pub fn granola(
    _work_dir: &Path,
    project: &ResolvedProject,
    source_path: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let options = margins_workflows::granola_import::default_options(
        &project.root_dir,
        &project.project.inbox_folder,
        &project.project.people_folder,
    );
    let paths = vec![source_path.to_string_lossy().into_owned()];
    let result = margins_workflows::granola_import::import(&paths, &project.root_dir, &options)
        .map_err(|error| CliError::new("granola_import_failed", error))?;
    line(
        stdout,
        format_args!(
            "<margins_import_granola status=\"ok\" imported=\"{}\" destination=\"vault\">",
            result.imported_count,
        ),
    )
    .map_err(CliError::from_anyhow)?;
    for note_path in &result.note_paths {
        line(stdout, format_args!("  <note>")).map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <path>{}</path>", xml_escape_text(note_path)),
        )
        .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("  </note>")).map_err(CliError::from_anyhow)?;
    }
    for warning in &result.warnings {
        line(
            stdout,
            format_args!("  <warning>{}</warning>", xml_escape_text(warning)),
        )
        .map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("</margins_import_granola>")).map_err(CliError::from_anyhow)
}
