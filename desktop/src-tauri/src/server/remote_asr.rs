use anyhow::{bail, Context, Result};
use margins_meeting_protocol::{
    AudioCodecV1, AudioContainerV1, SessionId, WorkspaceProcessingJobV1,
};
use margins_workflows::{
    remote_workspace::{remote_opus_packet_stream_for_asr, remote_pcm_s16le_for_asr},
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
        let Some(kind) = artifact.kind.strip_prefix("audio_") else {
            continue;
        };
        let (lane_id, artifact_codec) = if let Some(lane) = kind.strip_suffix("_pcm") {
            (lane, AudioCodecV1::PcmS16Le)
        } else if let Some(lane) = kind.strip_suffix("_opus") {
            (lane, AudioCodecV1::Opus)
        } else {
            continue;
        };
        let format = service.capture_lane_format(session_id, lane_id)?;
        if format.codec != artifact_codec || format.channel_count != 1 {
            bail!("remote ASR artifact kind conflicts with its declared lane format");
        }
        let segment = record
            .segments
            .iter()
            .find(|segment| segment.ordinal == artifact.ordinal.max(0) as u64)
            .context("audio artifact has no matching segment timeline")?;
        let bytes = service.artifact_content(principal, artifact.artifact_id.as_ref())?;
        let mono_16k = match (format.codec, format.container) {
            (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw) => {
                remote_pcm_s16le_for_asr(&bytes, format.sample_rate_hz)?
            }
            (AudioCodecV1::Opus, AudioContainerV1::PacketStream)
                if format.sample_rate_hz == 16_000 =>
            {
                remote_opus_packet_stream_for_asr(&bytes)?
            }
            _ => bail!("remote ASR lane uses an unsupported durable audio format"),
        };
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
            native_create_session_command, remote_opus_packet_stream_for_asr, DurableTransferSpool,
            NativeRemoteLane, NativeRemoteTransfer,
        },
        workspace::ensure_service_workspace,
    };
    use std::path::PathBuf;

    struct QualityCase {
        name: &'static str,
        pcm: &'static str,
        reference: &'static str,
        critical: &'static [&'static str],
    }

    fn normalized_words(value: &str) -> Vec<String> {
        value
            .to_lowercase()
            .chars()
            .map(|character| {
                if character.is_alphanumeric() {
                    character
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
        let reference = normalized_words(reference);
        let hypothesis = normalized_words(hypothesis);
        let mut previous = (0..=hypothesis.len()).collect::<Vec<_>>();
        for (row, expected) in reference.iter().enumerate() {
            let mut current = vec![row + 1];
            for (column, actual) in hypothesis.iter().enumerate() {
                current.push(
                    (previous[column + 1] + 1)
                        .min(current[column] + 1)
                        .min(previous[column] + usize::from(expected != actual)),
                );
            }
            previous = current;
        }
        previous[hypothesis.len()] as f64 / reference.len().max(1) as f64
    }

    fn encode_quality_lane(pcm: &[u8], bitrate: u32) -> (Vec<u8>, Vec<f32>, u128) {
        let temp = tempfile::tempdir().unwrap();
        let spool = DurableTransferSpool::create(
            temp.path(),
            "quality",
            "quality-instance",
            "https://fixture.invalid",
            "quality",
            "quality-session",
            "quality-producer",
            0,
        )
        .unwrap();
        let started = std::time::Instant::now();
        let mut transfer = NativeRemoteTransfer::new_with_bitrate(spool, bitrate);
        transfer.begin_segment("quality-segment".into(), 0).unwrap();
        transfer
            .append_s16le(NativeRemoteLane::Microphone, 16_000, pcm)
            .unwrap();
        transfer.close_segment(SegmentCloseReasonV1::Stop).unwrap();
        let encode_micros = started.elapsed().as_micros();
        let mut chunks = transfer
            .spool()
            .pending_chunks()
            .unwrap()
            .into_iter()
            .filter_map(|chunk| match chunk.command.body {
                ClientMessageBodyV1::AudioChunk(audio) if audio.lane_id.as_ref() == "mic" => {
                    Some((audio.sequence, audio.payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        chunks.sort_by_key(|(sequence, _)| *sequence);
        let encoded = chunks
            .into_iter()
            .flat_map(|(_, payload)| payload)
            .collect::<Vec<_>>();
        let decoded = remote_opus_packet_stream_for_asr(&encoded).unwrap();
        assert_eq!(decoded.len(), pcm.len() / 2);
        (encoded, decoded, encode_micros)
    }

    #[test]
    #[ignore = "requires pinned Parakeet TDT v2 ONNX assets and the synthetic Opus evaluation corpus"]
    fn pinned_v2_opus_quality_matrix() {
        let fixture_root = std::env::var_os("MARGINS_OPUS_EVAL_FIXTURES")
            .map(PathBuf::from)
            .expect("MARGINS_OPUS_EVAL_FIXTURES is required");
        let cases = [
            QualityCase {
                name: "us-names-numbers",
                pcm: "us-names-numbers.pcm",
                reference: "Margins codec evaluation. Priya Natarajan called José Alvarez at nine forty seven. Order Q seven four two costs one thousand two hundred thirty dollars. Meeting code alpha nine one three. Stop now.",
                critical: &[
                    "priya",
                    "natarajan",
                    "jose",
                    "alvarez",
                    "nine forty seven",
                    "q seven four two",
                    "alpha nine one three",
                ],
            },
            QualityCase {
                name: "quiet-us",
                pcm: "quiet-us.pcm",
                reference: "Margins codec evaluation. Priya Natarajan called José Alvarez at nine forty seven. Order Q seven four two costs one thousand two hundred thirty dollars. Meeting code alpha nine one three. Stop now.",
                critical: &[
                    "priya",
                    "natarajan",
                    "jose",
                    "alvarez",
                    "nine forty seven",
                    "q seven four two",
                    "alpha nine one three",
                ],
            },
            QualityCase {
                name: "noisy-us",
                pcm: "noisy-us.pcm",
                reference: "Margins codec evaluation. Priya Natarajan called José Alvarez at nine forty seven. Order Q seven four two costs one thousand two hundred thirty dollars. Meeting code alpha nine one three. Stop now.",
                critical: &[
                    "priya",
                    "natarajan",
                    "jose",
                    "alvarez",
                    "nine forty seven",
                    "q seven four two",
                    "alpha nine one three",
                ],
            },
            QualityCase {
                name: "in-names-numbers",
                pcm: "in-names-numbers.pcm",
                reference: "Anika Rao reported latency of one hundred twenty milliseconds. Ticket B seven three one closes on September twenty first. Please verify each number.",
                critical: &[
                    "anika",
                    "rao",
                    "one hundred twenty",
                    "b seven three one",
                    "september twenty first",
                ],
            },
            QualityCase {
                name: "gb-names-numbers",
                pcm: "gb-names-numbers.pcm",
                reference: "On Thursday, Eleanor Wright scheduled platform review at fourteen thirty. Reference six zero eight, checksum delta four five. Please confirm the Oxford address.",
                critical: &[
                    "eleanor",
                    "wright",
                    "fourteen thirty",
                    "six zero eight",
                    "delta four five",
                    "oxford",
                ],
            },
            QualityCase {
                name: "overlap",
                pcm: "overlap.pcm",
                reference: "Speaker one confirms project Atlas at nine fifteen. Budget code four two seven. Speaker two requests review on Friday at eleven thirty. Reference eight zero six.",
                critical: &[
                    "atlas",
                    "nine fifteen",
                    "four two seven",
                    "friday",
                    "eleven thirty",
                    "eight zero six",
                ],
            },
            QualityCase {
                name: "system-silence",
                pcm: "us-names-numbers-system-silence.pcm",
                reference: "",
                critical: &[],
            },
        ];
        let (model_dir, kind) = margins::offline_asr::resolve_parakeet_model_dir().unwrap();
        assert_eq!(kind, margins::asr::AsrModelKind::Tdt);
        assert!(
            model_dir.to_string_lossy().contains("v2"),
            "quality acceptance must use the configured Parakeet TDT v2 assets"
        );
        let mut backend = margins::asr::parakeet::ParakeetAsr::from_dir(&model_dir, kind).unwrap();
        let mut results = Vec::new();
        for case in cases {
            let pcm = std::fs::read(fixture_root.join("pcm").join(case.pcm)).unwrap();
            let baseline = remote_pcm_s16le_for_asr(&pcm, 16_000).unwrap();
            let mut variants = vec![("pcm".to_string(), pcm.len(), baseline, 0u128)];
            for bitrate in [16_000, 24_000, 32_000] {
                let (encoded, decoded, encode_micros) = encode_quality_lane(&pcm, bitrate);
                variants.push((
                    format!("opus-{}k", bitrate / 1_000),
                    encoded.len(),
                    decoded,
                    encode_micros,
                ));
            }
            let mut baseline_quality = None;
            for (variant, bytes, samples, encode_micros) in variants {
                let asr_started = std::time::Instant::now();
                let words = backend.transcribe_words(&samples).unwrap();
                let asr_millis = asr_started.elapsed().as_millis();
                let hypothesis = words
                    .iter()
                    .map(|word| word.text.trim())
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let normalized_hypothesis = normalized_words(&hypothesis).join(" ");
                let critical_hits = case
                    .critical
                    .iter()
                    .filter(|token| {
                        normalized_hypothesis.contains(&normalized_words(token).join(" "))
                    })
                    .count();
                let wer = word_error_rate(case.reference, &hypothesis);
                let word_count = normalized_words(&hypothesis).len();
                let first_word_ms = words.first().map(|word| word.start_ms);
                let last_word_ms = words.last().map(|word| word.end_ms);
                if variant == "pcm" {
                    baseline_quality = Some((wer, critical_hits, first_word_ms, last_word_ms));
                } else if variant == "opus-24k" {
                    let baseline = baseline_quality.expect("PCM baseline must run first");
                    assert!(
                        wer <= baseline.0 + f64::EPSILON,
                        "{} 24 kbps WER regressed from {} to {}",
                        case.name,
                        baseline.0,
                        wer
                    );
                    assert!(
                        critical_hits >= baseline.1,
                        "{} 24 kbps lost baseline critical tokens",
                        case.name
                    );
                    for (actual, expected) in
                        [(first_word_ms, baseline.2), (last_word_ms, baseline.3)]
                    {
                        if let (Some(actual), Some(expected)) = (actual, expected) {
                            assert!(
                                actual.abs_diff(expected) <= 160,
                                "{} 24 kbps word boundary shifted by more than 160 ms",
                                case.name
                            );
                        }
                    }
                    assert!(
                        bytes < pcm.len() * 15 / 100,
                        "{} 24 kbps transport exceeded 15% of PCM",
                        case.name
                    );
                }
                if case.reference.is_empty() {
                    assert_eq!(word_count, 0, "{} produced false speech", variant);
                }
                results.push(serde_json::json!({
                    "case": case.name,
                    "variant": variant,
                    "source_frames": pcm.len() / 2,
                    "transport_bytes": bytes,
                    "transport_ratio": bytes as f64 / pcm.len() as f64,
                    "wer": wer,
                    "hypothesis_word_count": word_count,
                    "critical_hits": critical_hits,
                    "critical_total": case.critical.len(),
                    "first_word_ms": first_word_ms,
                    "last_word_ms": last_word_ms,
                    "encode_micros": encode_micros,
                    "asr_millis": asr_millis,
                }));
            }
        }
        let sanitized = serde_json::to_vec_pretty(&results).unwrap();
        if let Some(path) = std::env::var_os("MARGINS_OPUS_EVAL_RESULTS") {
            std::fs::write(path, &sanitized).unwrap();
        }
        println!("{}", String::from_utf8(sanitized).unwrap());
    }

    #[test]
    #[ignore = "requires pinned Parakeet TDT v2 ONNX assets and spoken PCM fixtures"]
    fn finalized_remote_opus_runs_durable_asr_job_and_retains_encoded_audio() {
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
        let mut transfer = NativeRemoteTransfer::new(spool);
        transfer.begin_segment("segment-1".into(), 0).unwrap();
        transfer
            .append_s16le(NativeRemoteLane::Microphone, 16_000, &spoken)
            .unwrap();
        transfer
            .append_s16le(NativeRemoteLane::System, 16_000, &silence)
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
            .find(|artifact| artifact.kind == "audio_mic_opus")
            .unwrap();
        let encoded = service
            .artifact_content(&principal, mic.artifact_id.as_ref())
            .unwrap();
        assert_eq!(
            remote_opus_packet_stream_for_asr(&encoded).unwrap().len(),
            spoken.len() / 2
        );
        assert!(artifacts
            .iter()
            .any(|artifact| artifact.kind == "transcript"));
    }
}
