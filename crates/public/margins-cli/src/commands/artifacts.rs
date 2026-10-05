use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use chrono::{DateTime, Local};
use margins_workflows::artifacts::ArtifactView;
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
    render_artifact_list(&meeting_id, artifacts, stdout)
}

fn render_artifact_list(
    meeting_id: &str,
    artifacts: Vec<ArtifactView>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
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
        line(
            stdout,
            format_args!("    <available>{}</available>", view.available),
        )
        .map_err(CliError::from_anyhow)?;
        if artifact.path.starts_with("meeting-runtime://") && view.available {
            line(
                stdout,
                format_args!("    <export_command>margins audio-export</export_command>"),
            )
            .map_err(CliError::from_anyhow)?;
            line(
                stdout,
                format_args!(
                    "    <export_meeting_id>{}</export_meeting_id>",
                    xml_escape_text(&meeting_id)
                ),
            )
            .map_err(CliError::from_anyhow)?;
        }
        line(stdout, format_args!("  </artifact>")).map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("</margins_artifacts>")).map_err(CliError::from_anyhow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_audio_list_exposes_export_instead_of_claiming_a_missing_file() {
        let artifact = margins_store::canonical::SessionArtifact {
            session_name: "meeting".into(),
            kind: "audio_mic_runtime".into(),
            ordinal: 0,
            path: "meeting-runtime://meeting/meeting-seg-0/mic".into(),
            retention_class: "durable".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        };
        let mut output = Vec::new();
        render_artifact_list(
            "meeting",
            vec![ArtifactView {
                artifact,
                disk_path: "/tmp/.margins/meeting_seg0.wav".into(),
                exists: false,
                available: true,
            }],
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("<available>true</available>"));
        assert!(output.contains("<exists>false</exists>"));
        assert!(output.contains("<export_command>margins audio-export</export_command>"));
        assert!(output.contains("<export_meeting_id>meeting</export_meeting_id>"));
    }
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
