use crate::commands::sessions::{read_current_session, unique_session_name, write_current_session};
use crate::error::CliError;
use crate::services::CliServices;
use margins_core::{
    ArtifactDestination, AudioLane, CaptureAction, CaptureCommand, CaptureObserver,
    CaptureOperationId, CaptureRequest, CaptureState, EventEnvelope, PcmChunk, PermissionState,
    SegmentId, SessionId,
};
use std::path::Path;
use std::sync::Arc;

struct EventObserver {
    sink: Arc<dyn margins_core::EventSink>,
}

impl CaptureObserver for EventObserver {
    fn on_audio(&self, _chunk: PcmChunk) {}
    fn on_event(&self, event: EventEnvelope) {
        let _ = self.sink.publish(&event);
    }
}

pub fn run(
    services: &CliServices,
    work_dir: &Path,
    selected: Option<&str>,
    new_title: Option<&str>,
    create_new: bool,
) -> Result<(), CliError> {
    preflight(services)?;

    let margins_dir = work_dir.join(".margins");
    let now = services.clock.now();
    let name = if create_new {
        unique_session_name(
            services,
            &margins_dir,
            &now.format("%Y-%m-%d-%H-%M-%S").to_string(),
        )?
    } else if let Some(name) = selected {
        if !services
            .sessions
            .exists(&margins_dir, name)
            .map_err(CliError::from_anyhow)?
        {
            return Err(CliError::new(
                "session_not_found",
                format!("Session '{name}' not found. Run `margins ls` to choose one."),
            ));
        }
        name.to_string()
    } else {
        read_current_session(services, &margins_dir)?
    };

    let (ordinal, offset_ms) = if create_new {
        (0, 0)
    } else {
        let started = services
            .sessions
            .start_time(&margins_dir, &name)
            .map_err(CliError::from_anyhow)?;
        (
            services
                .sessions
                .next_segment_ordinal(&margins_dir, &name)
                .map_err(CliError::from_anyhow)?,
            (now - started).num_milliseconds().max(0),
        )
    };
    let segment_id = SegmentId::from(format!("{name}-segment-{ordinal}"));
    let operation_id = CaptureOperationId::new(format!("{name}-segment-{ordinal}-start"));
    let audio_uri = format!(".margins/{name}_seg{ordinal}.wav");
    let capabilities = services.capture.capabilities();
    let lanes = if capabilities.supported_lanes.is_empty() {
        vec![AudioLane::Microphone, AudioLane::System]
    } else {
        capabilities.supported_lanes
    };
    for lane in &lanes {
        ensure_permission(services, *lane)?;
    }

    // Start before durable reservation. A failed provider therefore cannot
    // create a session, segment, memo, or current pointer.
    let handle = services
        .capture
        .start(
            CaptureRequest {
                session_id: SessionId::from(name.clone()),
                segment_id: segment_id.clone(),
                operation_id: operation_id.clone(),
                lanes,
                input_device_id: None,
                deliver_live_pcm: true,
                destination: ArtifactDestination {
                    uri: audio_uri.clone(),
                },
            },
            Arc::new(EventObserver {
                sink: services.events.clone(),
            }),
        )
        .map_err(CliError::capture)?;

    std::fs::create_dir_all(&margins_dir)
        .map_err(|error| CliError::new("store_failed", error.to_string()))?;
    if create_new {
        let memo_uri = format!(".margins/{name}.md");
        std::fs::write(work_dir.join(&memo_uri), "")
            .map_err(|error| CliError::new("store_failed", error.to_string()))?;
        services
            .sessions
            .create(&margins_dir, &name, &now, &memo_uri)
            .map_err(CliError::from_anyhow)?;
        if new_title.is_some() {
            services
                .sessions
                .set_title(&margins_dir, &name, new_title.map(str::to_string))
                .map_err(CliError::from_anyhow)?;
        }
    }
    services
        .sessions
        .append_segment(&margins_dir, &name, ordinal, &audio_uri, offset_ms)
        .map_err(CliError::from_anyhow)?;
    write_current_session(services, &margins_dir, &name)?;

    // The public composition never reaches this point. Embedders may replace
    // the unavailable provider; until the native terminal adapter is composed,
    // finish a successful injected capture deterministically.
    let result = handle
        .command(CaptureCommand {
            operation_id: CaptureOperationId::new(format!("{name}-segment-{ordinal}-finish")),
            expected_segment_id: segment_id,
            action: CaptureAction::Finish,
        })
        .map_err(CliError::capture)?;
    if result.snapshot.state != CaptureState::Finished {
        return Err(CliError::new(
            "capture_open_failed",
            "capture provider did not finish the segment",
        ));
    }
    let duration_secs = result
        .completed_artifacts
        .iter()
        .map(|artifact| artifact.duration_ms.0 as f64 / 1000.0)
        .fold(0.0, f64::max);
    services
        .sessions
        .set_segment_duration(&margins_dir, &name, ordinal, duration_secs)
        .map_err(CliError::from_anyhow)?;
    for (artifact_ordinal, artifact) in result.completed_artifacts.iter().enumerate() {
        let kind = match artifact.lane {
            Some(AudioLane::Microphone) => "audio_microphone",
            Some(AudioLane::System) => "audio_system",
            Some(_) | None => "audio",
        };
        services
            .sessions
            .register_artifact(
                &margins_dir,
                &name,
                kind,
                ordinal.saturating_mul(10) + artifact_ordinal as i64,
                &artifact.uri,
            )
            .map_err(CliError::from_anyhow)?;
    }
    Ok(())
}

fn preflight(services: &CliServices) -> Result<(), CliError> {
    if services.capture.capabilities().available {
        Ok(())
    } else {
        Err(CliError::capture_unavailable())
    }
}

fn ensure_permission(services: &CliServices, lane: AudioLane) -> Result<(), CliError> {
    let state = services
        .capture
        .permission(lane)
        .map_err(CliError::capture)?;
    let state = if state == PermissionState::NotDetermined {
        services
            .capture
            .request_permission(lane)
            .map_err(CliError::capture)?
    } else {
        state
    };
    match state {
        PermissionState::Granted => Ok(()),
        PermissionState::Unavailable => Err(CliError::capture_unavailable()),
        PermissionState::Denied | PermissionState::Restricted => Err(CliError::new(
            "capture_permission_denied",
            "capture permission was denied",
        )),
        _ => Err(CliError::new(
            "capture_open_failed",
            "capture permission was not granted",
        )),
    }
}
