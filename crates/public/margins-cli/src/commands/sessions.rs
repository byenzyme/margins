use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use crate::services::CliServices;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn current_pointer_path(margins_dir: &Path) -> PathBuf {
    margins_dir.join("current")
}

pub fn read_current_session(
    services: &CliServices,
    margins_dir: &Path,
) -> Result<String, CliError> {
    let name = services
        .sessions
        .current(margins_dir)
        .map_err(CliError::from_anyhow)?
        .ok_or_else(|| {
            CliError::new(
                "session_not_found",
                "No current session. Run `margins new` to start one.",
            )
        })?;
    let exists = !name.is_empty()
        && services
            .sessions
            .exists(margins_dir, &name)
            .map_err(CliError::from_anyhow)?;
    if !exists {
        return Err(CliError::new(
            "session_not_found",
            "Current session is missing or stale. Run `margins new` to start one.",
        ));
    }
    Ok(name)
}

pub fn write_current_session(
    services: &CliServices,
    margins_dir: &Path,
    name: &str,
) -> Result<(), CliError> {
    services
        .sessions
        .set_current(margins_dir, name)
        .map_err(CliError::from_anyhow)
}

pub fn show_current(
    services: &CliServices,
    work_dir: &Path,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    let name = read_current_session(services, &margins_dir)?;
    let meta = services
        .sessions
        .get(&margins_dir, &name)
        .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("<margins_current id=\"{}\">", xml_escape_attr(&name)),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <title>{}</title>",
            xml_escape_text(meta.title.as_deref().unwrap_or(""))
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <started_at>{}</started_at>",
            xml_escape_text(&meta.start_time)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <segments>{}</segments>", meta.segments.len()),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <memo_path>{}</memo_path>",
            xml_escape_text(&meta.notes_path)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("</margins_current>")).map_err(CliError::from_anyhow)
}

pub fn rename(
    services: &CliServices,
    work_dir: &Path,
    title: &str,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    let name = read_current_session(services, &margins_dir)?;
    let saved = services
        .sessions
        .set_title(&margins_dir, &name, Some(title.to_string()))
        .map_err(CliError::from_anyhow)?
        .ok_or_else(|| CliError::new("invalid_title", "Title cannot be empty"))?;
    line(
        stdout,
        format_args!(
            "<margins_renamed id=\"{}\"><title>{}</title></margins_renamed>",
            xml_escape_attr(&name),
            xml_escape_text(&saved)
        ),
    )
    .map_err(CliError::from_anyhow)
}

pub fn list(
    services: &CliServices,
    work_dir: &Path,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        line(
            stderr,
            format_args!("No sessions found (no .margins/ directory)."),
        )
        .map_err(CliError::from_anyhow)?;
        return Ok(());
    }
    let sessions = services
        .sessions
        .list(&margins_dir)
        .map_err(CliError::from_anyhow)?;
    if sessions.is_empty() {
        line(stderr, format_args!("No sessions found.")).map_err(CliError::from_anyhow)?;
        return Ok(());
    }
    line(
        stderr,
        format_args!("{:<20} {:<24} {:<8} {}", "NAME", "STARTED", "SEGS", "NOTES"),
    )
    .map_err(CliError::from_anyhow)?;
    line(stderr, format_args!("{}", "-".repeat(72))).map_err(CliError::from_anyhow)?;
    for session in sessions {
        line(
            stderr,
            format_args!(
                "{:<20} {:<24} {:<8} {}",
                session.name, session.start_time, session.segment_count, session.notes_path
            ),
        )
        .map_err(CliError::from_anyhow)?;
    }
    Ok(())
}

pub fn unique_session_name(
    services: &CliServices,
    margins_dir: &Path,
    base: &str,
) -> Result<String, CliError> {
    if !margins_dir.exists() {
        return Ok(base.to_string());
    }
    for ordinal in 1..1000 {
        let candidate = if ordinal == 1 {
            base.to_string()
        } else {
            format!("{base}-{ordinal}")
        };
        if !services
            .sessions
            .exists(margins_dir, &candidate)
            .map_err(CliError::from_anyhow)?
            && margins_workflows::transcript_view::preferred_transcript_path(
                margins_dir,
                &candidate,
            )
            .is_none()
            && !margins_dir.join(format!("{candidate}.md")).exists()
        {
            return Ok(candidate);
        }
    }
    Err(CliError::new(
        "session_name_exhausted",
        "could not allocate a unique session id",
    ))
}
