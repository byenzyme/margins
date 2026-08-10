use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use std::io::Write;
use std::path::Path;

pub fn recent(work_dir: &Path, stdout: &mut dyn Write) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        line(stdout, format_args!("<margins_recent />")).map_err(CliError::from_anyhow)?;
        return Ok(());
    }
    let meetings = margins_workflows::session_index::list_sessions_with_notes(work_dir, None, None)
        .map_err(|error| CliError::new("store_failed", error))?;
    line(stdout, format_args!("<margins_recent>")).map_err(CliError::from_anyhow)?;
    for meeting in meetings.iter().take(30) {
        let people = meeting.display_people().join(", ");
        let transcript_path = margins_workflows::transcript_view::preferred_transcript_path(
            &margins_dir,
            &meeting.name,
        )
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
        line(
            stdout,
            format_args!(
                "  <meeting id=\"{}\" started_at=\"{}\" segments=\"{}\">",
                xml_escape_attr(&meeting.name),
                xml_escape_attr(&meeting.start_time),
                meeting.segment_count
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <title>{}</title>",
                xml_escape_text(&meeting.display_title())
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <calendar_event>{}</calendar_event>",
                xml_escape_text(meeting.calendar_event_title.as_deref().unwrap_or(""))
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <people>{}</people>", xml_escape_text(&people)),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <memo_path>{}</memo_path>",
                xml_escape_text(&meeting.notes_path)
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <saved_note_path>{}</saved_note_path>",
                xml_escape_text(meeting.vault_note_path.as_deref().unwrap_or(""))
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!(
                "    <transcript_path>{}</transcript_path>",
                xml_escape_text(&transcript_path)
            ),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <status>{}</status>", xml_escape_text(&meeting.status)),
        )
        .map_err(CliError::from_anyhow)?;
        line(
            stdout,
            format_args!("    <source>{}</source>", xml_escape_text(&meeting.source)),
        )
        .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("  </meeting>")).map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("</margins_recent>")).map_err(CliError::from_anyhow)
}

pub fn transcript(
    work_dir: &Path,
    meeting_id: &str,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        return Err(CliError::new(
            "store_not_found",
            "No .margins/ directory found.",
        ));
    }
    let transcript = margins_workflows::transcript_view::load_transcript_view(
        work_dir,
        &margins_dir,
        meeting_id,
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_transcript meeting_id=\"{}\" view=\"{}\">",
            xml_escape_attr(&transcript.session_name),
            transcript.view
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("  <metadata>")).map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <started_at>{}</started_at>",
            xml_escape_text(&transcript.started_at)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <created_at>{}</created_at>",
            xml_escape_text(&transcript.created_at)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("    <title>{}</title>", xml_escape_text(&transcript.title)),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <calendar_event>{}</calendar_event>",
            xml_escape_text(&transcript.calendar_event)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <people>{}</people>",
            xml_escape_text(&transcript.people.join(", "))
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <memo_path>{}</memo_path>",
            xml_escape_text(&transcript.memo_path)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <saved_note_path>{}</saved_note_path>",
            xml_escape_text(transcript.saved_note_path.as_deref().unwrap_or(""))
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "    <transcript_path>{}</transcript_path>",
            xml_escape_text(&transcript.source_path.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    if let Some(alias) = &transcript.speaker_alias {
        line(
            stdout,
            format_args!(
                "    <speaker_alias channel=\"other\">{}</speaker_alias>",
                xml_escape_text(alias)
            ),
        )
        .map_err(CliError::from_anyhow)?;
    }
    line(stdout, format_args!("  </metadata>")).map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("  <body>")).map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("{}", xml_escape_text(&transcript.body)),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("  </body>")).map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("</margins_transcript>")).map_err(CliError::from_anyhow)
}
