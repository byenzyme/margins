use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use chrono::{DateTime, Local};
use std::io::Write;
use std::path::Path;

pub fn list(work_dir: &Path, meeting_id: &str, stdout: &mut dyn Write) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        return Err(CliError::new(
            "store_not_found",
            "No .margins/ directory found.",
        ));
    }
    let meeting_id =
        margins_workflows::transcript_view::resolve_session_name(&margins_dir, meeting_id)
            .map_err(CliError::from_anyhow)?;
    let artifacts =
        margins_workflows::artifacts::list_artifacts(work_dir, &margins_dir, &meeting_id)
            .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_artifacts meeting_id=\"{}\">",
            xml_escape_attr(&meeting_id)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    for view in artifacts {
        let artifact = view.artifact;
        line(
            stdout,
            format_args!(
                "  <artifact kind=\"{}\" ordinal=\"{}\">",
                xml_escape_attr(&artifact.kind),
                artifact.ordinal
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <retention_class>{}</retention_class>",
                xml_escape_text(&artifact.retention_class)
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <expires_at>{}</expires_at>",
                xml_escape_text(artifact.expires_at.as_deref().unwrap_or(""))
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <path>{}</path>",
                xml_escape_text(&view.disk_path.to_string_lossy())
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("    <exists>{}</exists>", view.exists))
            .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("  </artifact>")).map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("</margins_artifacts>")).map_err(CliError::from_anyhow)
}

pub fn prune(
    work_dir: &Path,
    now: DateTime<Local>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        line(
            stdout,
            format_args!("<margins_artifacts_prune deleted=\"0\" rows=\"0\" />"),
        )
        .map_err(CliError::from_anyhow)?;
        return Ok(());
    }
    let report = margins_workflows::artifacts::prune_expired_artifacts(&margins_dir, now)
        .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("<margins_artifacts_prune>")).map_err(CliError::from_anyhow)?;
    for item in report.artifacts {
        let artifact = item.artifact;
        line(
            stdout,
            format_args!(
                "  <artifact session_id=\"{}\" kind=\"{}\" ordinal=\"{}\" deleted=\"{}\" registry_rows=\"{}\">",
                xml_escape_attr(&artifact.session_name),
                xml_escape_attr(&artifact.kind),
                artifact.ordinal,
                item.deleted,
                item.registry_rows
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <path>{}</path>", xml_escape_text(&artifact.path)),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <expires_at>{}</expires_at>",
                xml_escape_text(artifact.expires_at.as_deref().unwrap_or(""))
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("  </artifact>")).map_err(CliError::from_anyhow)?;
    }
    line(
        stdout,
        format_args!(
            "  <summary deleted=\"{}\" rows=\"{}\" />",
            report.deleted, report.registry_rows
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("</margins_artifacts_prune>")).map_err(CliError::from_anyhow)
}
