use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProcessingTrack {
    Capture,
    Transcript,
    Note,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProcessingEvent {
    pub(crate) stage: String,
    pub(crate) message: String,
    pub(crate) progress: Option<f32>,
    /// Session this lifecycle event belongs to. Older trace rows do not have it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    /// Coarse lifecycle track for reducing capture/transcript/note state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) track: Option<ProcessingTrack>,
    /// Track-specific lifecycle phase, serialized as snake_case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    /// Unix epoch milliseconds when the event was emitted. Older traces may
    /// not have this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) emitted_at_ms: Option<u64>,
    /// Milliseconds since this processing/refine run started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) elapsed_ms: Option<u64>,
    /// Transcript entry streamed during transcription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) entry: Option<TranscriptEntry>,
}

impl ProcessingEvent {
    pub(crate) fn new(stage: &str, message: &str, progress: Option<f32>) -> Self {
        Self::with_entry(stage, message, progress, None, None, None, None, None)
    }

    pub(crate) fn timed(
        run_started_at: &Instant,
        stage: &str,
        message: &str,
        progress: Option<f32>,
    ) -> Self {
        Self::with_entry(
            stage,
            message,
            progress,
            Some(run_started_at),
            None,
            None,
            None,
            None,
        )
    }

    pub(crate) fn timed_lifecycle(
        run_started_at: &Instant,
        session: &str,
        track: ProcessingTrack,
        phase: &str,
        stage: &str,
        message: &str,
        progress: Option<f32>,
    ) -> Self {
        Self::with_entry(
            stage,
            message,
            progress,
            Some(run_started_at),
            None,
            Some(session.to_string()),
            Some(track),
            Some(phase.to_string()),
        )
    }

    pub(crate) fn with_entry(
        stage: &str,
        message: &str,
        progress: Option<f32>,
        run_started_at: Option<&Instant>,
        entry: Option<TranscriptEntry>,
        session: Option<String>,
        track: Option<ProcessingTrack>,
        phase: Option<String>,
    ) -> Self {
        let emitted_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64);
        let elapsed_ms = run_started_at.map(|started| started.elapsed().as_millis() as u64);
        Self {
            stage: stage.to_string(),
            message: message.to_string(),
            progress,
            session,
            track,
            phase,
            emitted_at_ms,
            elapsed_ms,
            entry,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TranscriptEntry {
    pub(crate) channel: u32,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) text: String,
}

pub(crate) fn write_processing_trace(
    trace_file: &Option<Arc<Mutex<std::fs::File>>>,
    event: &ProcessingEvent,
) {
    let Some(file) = trace_file else {
        return;
    };
    let Ok(mut guard) = file.lock() else {
        return;
    };
    let Ok(line) = serde_json::to_string(event) else {
        return;
    };
    let _ = writeln!(guard, "{line}");
}

pub(crate) fn write_processing_steps_markdown(
    steps_file: &Option<Arc<Mutex<std::fs::File>>>,
    event: &ProcessingEvent,
) {
    if event.stage == "note_stream" || event.stage == "transcript" {
        return;
    }
    let Some(file) = steps_file else {
        return;
    };
    let Ok(mut guard) = file.lock() else {
        return;
    };
    let progress = event
        .progress
        .map(|p| format!("{}% · ", (p.clamp(0.0, 1.0) * 100.0).round() as u32))
        .unwrap_or_default();
    let message = event.message.replace('\n', " ").trim().to_string();
    if message.is_empty() {
        return;
    }
    let _ = writeln!(guard, "- {progress}**{}** — {message}", event.stage);
}
