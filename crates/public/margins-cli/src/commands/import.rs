use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use margins_workflows::granola_import::GranolaVaultTarget;
use margins_workflows::project::ResolvedProject;
use std::io::Write;
use std::path::Path;

pub fn granola(
    work_dir: &Path,
    project: &ResolvedProject,
    source_path: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    margins_workflows::granola_import::validate_granola_file(source_path)
        .map_err(CliError::from_anyhow)?;
    let margins_dir = work_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir)
        .map_err(|error| CliError::new("store_failed", error.to_string()))?;
    let target = GranolaVaultTarget {
        vault_root: project.root_dir.clone(),
        notes_folder: project.project.inbox_folder.clone(),
        people_folder: project.project.people_folder.clone(),
        organizations_folder: "organizations".to_string(),
    };
    let (result, used_vault) = margins_workflows::granola_import::import_granola_with_vault_target(
        source_path,
        &margins_dir,
        Some(&target),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_import_granola status=\"ok\" imported=\"{}\" destination=\"{}\">",
            result.imported_count,
            if used_vault { "vault" } else { "margins" }
        ),
    )
    .map_err(CliError::from_anyhow)?;
    if !used_vault {
        line(stdout, format_args!("  <hint>No .margins/config.toml vault was configured; imported into .margins only.</hint>"))
            .map_err(CliError::from_anyhow)?;
    }
    for (index, (session_id, note_path)) in result
        .session_ids
        .iter()
        .zip(result.note_paths.iter())
        .enumerate()
    {
        line(
            stdout,
            format_args!("  <meeting id=\"{}\">", xml_escape_attr(session_id)),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <memo_path>{}</memo_path>", xml_escape_text(note_path)),
        )
        .map_err(CliError::from_anyhow)?;
        if let Some(path) = result.transcript_paths.get(index) {
            line(
                stdout,
                format_args!(
                    "    <transcript_path>{}</transcript_path>",
                    xml_escape_text(path)
                ),
            )
            .map_err(CliError::from_anyhow)?;
        }
        line(stdout, format_args!("  </meeting>")).map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("</margins_import_granola>")).map_err(CliError::from_anyhow)
}
