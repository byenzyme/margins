use anyhow::{bail, Context, Result};
use margins_meeting_protocol::{
    AudioCodecV1, AudioContainerV1, SessionId, WorkspaceProcessingJobV1,
};
use margins_workflows::{
    remote_workspace::remote_pcm_s16le_for_asr,
    workspace_service::{ServicePrincipal, WorkspaceService},
};
use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};

struct JobRequest {
    service: Arc<WorkspaceService>,
    principal: ServicePrincipal,
    job: WorkspaceProcessingJobV1,
}

#[derive(Clone)]
pub struct RemoteAsrJobs {
    in_flight: Arc<Mutex<HashSet<String>>>,
    sender: mpsc::SyncSender<JobRequest>,
}

impl Default for RemoteAsrJobs {
    fn default() -> Self {
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let (sender, receiver) = mpsc::sync_channel::<JobRequest>(16);
        let worker_in_flight = in_flight.clone();
        std::thread::Builder::new()
            .name("margins-remote-asr".to_string())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    drain_service_jobs(request, &worker_in_flight);
                }
            })
            .expect("failed to start bounded remote ASR worker");
        Self { in_flight, sender }
    }
}

impl RemoteAsrJobs {
    pub fn schedule(
        &self,
        service: Arc<WorkspaceService>,
        principal: ServicePrincipal,
        job: WorkspaceProcessingJobV1,
    ) {
        if !matches!(job.status.as_str(), "queued" | "running") {
            return;
        }
        let key = format!("{}:{}", job.job_id, job.attempt);
        if !self
            .in_flight
            .lock()
            .expect("remote ASR job lock poisoned")
            .insert(key.clone())
        {
            return;
        }
        if let Err(error) = self.sender.try_send(JobRequest {
            service,
            principal,
            job,
        }) {
            self.in_flight
                .lock()
                .expect("remote ASR job lock poisoned")
                .remove(&key);
            eprintln!("[margins-server] remote ASR queue deferred durable job: {error}");
        }
    }
}

fn drain_service_jobs(mut request: JobRequest, in_flight: &Mutex<HashSet<String>>) {
    loop {
        let key = format!("{}:{}", request.job.job_id, request.job.attempt);
        let result = run_job(&request.service, &request.principal, &request.job);
        if let Err(error) = result {
            let _ = request.service.update_transcription_job(
                &request.job.job_id,
                request.job.attempt,
                "failed",
                None,
                None,
                Some(&error.to_string()),
            );
        }
        in_flight
            .lock()
            .expect("remote ASR job lock poisoned")
            .remove(&key);
        let next = request
            .service
            .pending_transcription_jobs()
            .unwrap_or_default()
            .into_iter()
            .find(|job| {
                in_flight
                    .lock()
                    .expect("remote ASR job lock poisoned")
                    .insert(format!("{}:{}", job.job_id, job.attempt))
            });
        let Some(job) = next else {
            break;
        };
        request.job = job;
    }
}

fn run_job(
    service: &WorkspaceService,
    principal: &ServicePrincipal,
    job: &WorkspaceProcessingJobV1,
) -> Result<()> {
    service.update_transcription_job(
        &job.job_id,
        job.attempt,
        "running",
        Some(0.05),
        None,
        None,
    )?;
    let result_ref = transcribe_remote_session(service, principal, &job.session_id)?;
    service.update_transcription_job(
        &job.job_id,
        job.attempt,
        "complete",
        Some(1.0),
        Some(&result_ref),
        None,
    )?;
    Ok(())
}

fn transcribe_remote_session(
    service: &WorkspaceService,
    principal: &ServicePrincipal,
    session_id: &SessionId,
) -> Result<String> {
    let (model_dir, kind) = margins::offline_asr::resolve_parakeet_model_dir()?;
    let mut backend = margins::asr::parakeet::ParakeetAsr::from_dir(&model_dir, kind)
        .with_context(|| format!("failed to load ASR model from {}", model_dir.display()))?;
    let record = service
        .repository_record(principal, session_id)?
        .context("finalized session has no canonical repository record")?;
    let artifacts = service.artifacts(principal, session_id.as_ref())?;
    let mut timeline = Vec::<(u64, u8, String)>::new();
    for artifact in artifacts {
        let Some(lane_id) = artifact
            .kind
            .strip_prefix("audio_")
            .and_then(|kind| kind.strip_suffix("_pcm"))
        else {
            continue;
        };
        let format = service.capture_lane_format(session_id, lane_id)?;
        if format.codec != AudioCodecV1::PcmS16Le
            || format.container != AudioContainerV1::Raw
            || format.channel_count != 1
        {
            bail!("remote ASR supports only mono raw PCM s16le lanes");
        }
        let segment = record
            .segments
            .iter()
            .find(|segment| segment.ordinal == artifact.ordinal.max(0) as u64)
            .context("audio artifact has no matching segment timeline")?;
        let bytes = service.artifact_content(principal, artifact.artifact_id.as_ref())?;
        let mono_16k = remote_pcm_s16le_for_asr(&bytes, format.sample_rate_hz)?;
        let words = backend.transcribe_words(&mono_16k)?;
        let channel_order = if lane_id == "mic" { 0 } else { 1 };
        let label = if lane_id == "mic" {
            "you (mic)"
        } else if lane_id == "system" {
            "them (system)"
        } else {
            lane_id
        };
        for word in words {
            let text = word.text.trim();
            if !text.is_empty() {
                timeline.push((
                    segment.start_offset_ms.saturating_add(word.start_ms),
                    channel_order,
                    format!(
                        "[{}] {label}: {text}",
                        elapsed(segment.start_offset_ms.saturating_add(word.start_ms))
                    ),
                ));
            }
        }
    }
    timeline.sort_by_key(|(at, channel, _)| (*at, *channel));
    let timeline = timeline
        .into_iter()
        .map(|(_, _, line)| line)
        .collect::<Vec<_>>()
        .join("\n");
    let memo = std::fs::read_to_string(
        service
            .margins_dir()
            .join(format!("{}.md", session_id.as_ref())),
    )
    .unwrap_or_default();
    let content = crate::render_aligned_markdown(
        session_id.as_ref(),
        "Margins remote offline Parakeet TDT ONNX transcript and memo context.",
        &memo,
        &timeline,
    );
    if !crate::is_valid_terminal_transcript_checkpoint(&content, session_id.as_ref()) {
        bail!("remote transcript artifact did not validate for its session");
    }
    let path = crate::session_transcript_artifact_path(service.margins_dir(), session_id.as_ref());
    crate::write_atomic_utf8(&path, &content).map_err(anyhow::Error::msg)?;
    crate::register_transcript_artifact(service.margins_dir(), session_id.as_ref())
        .map_err(anyhow::Error::msg)?;
    Ok(crate::session_transcript_artifact_registry_path(
        session_id.as_ref(),
    ))
}

fn elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_meeting_protocol::{
        ClientMessageBodyV1, SegmentCloseReasonV1, SessionFinalizeReasonV1,
    };
    use margins_workflows::{
        remote_workspace::{
            native_create_session_command, DurableTransferSpool, NativePcmLane, NativePcmTransfer,
        },
        workspace::ensure_service_workspace,
    };
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires pinned Parakeet TDT v2 ONNX assets and spoken PCM fixtures"]
    fn finalized_remote_pcm_runs_durable_asr_job_and_retains_exact_audio() {
        let spoken_path = std::env::var_os("MARGINS_REMOTE_ASR_SPOKEN_PCM")
            .map(PathBuf::from)
            .expect("MARGINS_REMOTE_ASR_SPOKEN_PCM is required");
        let silence_path = std::env::var_os("MARGINS_REMOTE_ASR_SILENCE_PCM")
            .map(PathBuf::from)
            .expect("MARGINS_REMOTE_ASR_SILENCE_PCM is required");
        let spoken = std::fs::read(spoken_path).unwrap();
        let silence = std::fs::read(silence_path).unwrap();
        assert_eq!(spoken.len(), silence.len());

        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        let captures = temp.path().join("captures");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = ensure_service_workspace(
            &temp.path().join("home"),
            "asr-test",
            Some("ASR test"),
            &notes,
            &captures,
        )
        .unwrap();
        let service = Arc::new(
            WorkspaceService::open_with_capabilities("asr-host", workspace, true, false).unwrap(),
        );
        let principal = ServicePrincipal::full("test-client", "asr-test");
        let session = SessionId("remote-asr-fixture".into());
        let reservation = service
            .reserve_session(
                &principal,
                native_create_session_command(
                    session.as_ref(),
                    "remote-asr-fixture-reservation",
                    Some("Remote ASR fixture".into()),
                    "test",
                ),
            )
            .unwrap();
        let spool = DurableTransferSpool::create(
            &temp.path().join("spool"),
            "remote-asr-fixture-transfer",
            "asr-host",
            "https://fixture.invalid",
            "asr-test",
            session.as_ref(),
            &reservation.producer_token,
            0,
        )
        .unwrap();
        let mut transfer = NativePcmTransfer::new(spool);
        transfer.begin_segment("segment-1".into(), 0).unwrap();
        transfer
            .append_s16le(NativePcmLane::Microphone, 16_000, &spoken)
            .unwrap();
        transfer
            .append_s16le(NativePcmLane::System, 16_000, &silence)
            .unwrap();
        let close = transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
        let ended_at_ms = match &close.body {
            ClientMessageBodyV1::CloseSegment(close) => close.ended_at_ms.0,
            _ => unreachable!(),
        };
        let finalize = transfer
            .seal_session(ended_at_ms, SessionFinalizeReasonV1::Completed)
            .unwrap();
        for chunk in transfer.spool().pending_chunks().unwrap() {
            service
                .execute_capture(&principal, &reservation.producer_token, chunk.command)
                .unwrap();
        }
        service
            .execute_capture(&principal, &reservation.producer_token, close)
            .unwrap();
        service
            .execute_capture(&principal, &reservation.producer_token, finalize)
            .unwrap();
        let job = service
            .latest_job(&principal, &session)
            .unwrap()
            .expect("finalize must durably admit ASR");
        run_job(&service, &principal, &job).unwrap();

        let completed = service.latest_job(&principal, &session).unwrap().unwrap();
        assert_eq!(completed.status, "complete");
        assert_eq!(completed.attempt, 1);
        let record = service
            .repository_record(&principal, &session)
            .unwrap()
            .unwrap();
        assert_eq!(record.segments[0].audio.format.sample_rate_hz, 16_000);
        let transcript = service.transcript(&principal, session.as_ref()).unwrap();
        let lower = transcript.body.to_ascii_lowercase();
        for expected in [
            "margins",
            "verification",
            "alpha",
            "437",
            "system",
            "913",
            "stop",
            "now",
        ] {
            assert!(lower.contains(expected), "transcript omitted `{expected}`");
        }
        let artifacts = service.artifacts(&principal, session.as_ref()).unwrap();
        let mic = artifacts
            .iter()
            .find(|artifact| artifact.kind == "audio_mic_pcm")
            .unwrap();
        assert_eq!(
            service
                .artifact_content(&principal, mic.artifact_id.as_ref())
                .unwrap(),
            spoken
        );
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.kind == "transcript"));
    }
}
