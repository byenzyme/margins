use crate::commands::sessions::read_current_session;
use crate::error::CliError;
use crate::output::{line, xml_escape_attr, xml_escape_text};
use crate::services::CliServices;
use chrono::{DateTime, Local};
use std::io::Write;
use std::path::Path;

pub fn process_session(
    services: &CliServices,
    work_dir: &Path,
    requested_session: &str,
    speakers: usize,
    align_only: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        return Err(CliError::new(
            "store_not_found",
            "No .margins/ directory found.",
        ));
    }
    let name = if requested_session == "current" {
        read_current_session(services, &margins_dir)?
    } else {
        margins_workflows::transcript_view::resolve_session_name(&margins_dir, requested_session)
            .map_err(CliError::from_anyhow)?
    };
    if speakers > 1 && !services.diarization.is_available() {
        return Err(CliError::diarization_unavailable());
    }
    if !align_only && !services.asr.is_available() {
        return Err(CliError::asr_unavailable());
    }
    let result = margins_workflows::processing::process_session(
        margins_workflows::processing::ProcessRequest {
            work_dir,
            margins_dir: &margins_dir,
            session_name: &name,
            speakers,
            align_only,
        },
        services.asr.as_ref(),
        Some(services.diarization.as_ref()),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_process meeting_id=\"{}\" status=\"ok\">",
            xml_escape_attr(&result.session_name)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <segments>{}</segments>", result.segment_count),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <speakers>{}</speakers>", result.speakers),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <transcript_entries>{}</transcript_entries>",
            result.transcript_entries
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <transcript_json>{}</transcript_json>",
            xml_escape_text(&result.transcript_json.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <aligned_path>{}</aligned_path>",
            xml_escape_text(&result.aligned_path.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <asr_backends>{}</asr_backends>",
            xml_escape_text(&result.asr_backend)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("</margins_process>")).map_err(CliError::from_anyhow)
}

pub fn transcribe(
    services: &CliServices,
    work_dir: &Path,
    audio_path: &Path,
    requested_name: Option<&str>,
    memo_path: Option<&Path>,
    speakers: usize,
    started_at: DateTime<Local>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    if !audio_path.exists() {
        return Err(CliError::new(
            "audio_not_found",
            format!("Audio file not found: {}", audio_path.display()),
        ));
    }
    if speakers > 1 && !services.diarization.is_available() {
        return Err(CliError::diarization_unavailable());
    }
    if !services.asr.is_available() {
        return Err(CliError::asr_unavailable());
    }
    let margins_dir = work_dir.join(".margins");
    let result = margins_workflows::processing::transcribe_audio(
        margins_workflows::processing::TranscribeRequest {
            work_dir,
            margins_dir: &margins_dir,
            audio_path,
            requested_name,
            memo_path,
            speakers,
            started_at,
        },
        services.asr.as_ref(),
        Some(services.diarization.as_ref()),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_transcribe meeting_id=\"{}\" status=\"ok\">",
            xml_escape_attr(&result.session_name)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <audio_path>{}</audio_path>",
            xml_escape_text(&result.audio_path.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <memo_path>{}</memo_path>",
            xml_escape_text(&result.memo_path.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <transcript_json>{}</transcript_json>",
            xml_escape_text(&result.transcript_json.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <transcript_path>{}</transcript_path>",
            xml_escape_text(&result.transcript_path.to_string_lossy())
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <duration_secs>{:.3}</duration_secs>",
            result.duration_secs
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <audio_mode>{}</audio_mode>", result.audio_mode),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <speakers>{}</speakers>", result.speakers),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!("  <asr_backend>{}</asr_backend>", result.asr_backend),
    )
    .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "  <transcript_entries>{}</transcript_entries>",
            result.transcript_entries
        ),
    )
    .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("</margins_transcribe>")).map_err(CliError::from_anyhow)
}
