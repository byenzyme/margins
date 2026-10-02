//! Cold model setup and durable batch transcription for finalized Workspace audio.

use anyhow::{bail, Context, Result};
use margins_core::{AsrBackend, AsrRequest};
use margins_media::{model_registry, transcript::TranscriptWordEntry};
use margins_meeting_protocol::{
    AudioCodecV1, AudioContainerV1, SessionId, WorkspaceProcessingJobV1,
};
use margins_store::canonical;
use margins_workflows::{
    alignment::{interleave_timeline, parse_timed_memo_lines, TimelineEvent},
    remote_workspace::{remote_opus_packet_stream_for_asr, remote_pcm_s16le_for_asr},
    transcript_view::transcript_artifact_path,
    workspace_service::{ServicePrincipal, WorkspaceService},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
};

/// Set the managed Linux locations before creating the async runtime or ASR workers.
pub fn configure_model_environment() -> Result<()> {
    #[cfg(all(feature = "parakeet-asr", target_os = "linux"))]
    model_registry::parakeet::configure_env()?;
    Ok(())
}

pub const fn supported() -> bool {
    cfg!(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))
}

pub fn runtime_available() -> bool {
    probe_model().is_ok()
}

fn model_kind() -> model_registry::ModelKind {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        return model_registry::ModelKind::CoreMl;
    }
    model_registry::ModelKind::ParakeetOnnx
}

fn probe_model() -> Result<()> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        let path = model_registry::resolve_model(model_kind())?
            .context("CoreML model is not installed")?;
        margins_media::providers::coreml::CoreMlAsrBackend::from_dir_auto(path)?;
        return Ok(());
    }
    #[cfg(all(
        feature = "parakeet-asr",
        not(all(feature = "coreml-asr", target_os = "macos"))
    ))]
    {
        let (path, _) =
            model_registry::resolve_parakeet_model()?.context("Parakeet model is not installed")?;
        let _ = path;
        margins_media::providers::parakeet::ParakeetAsr::runtime_available()?;
        return Ok(());
    }
    #[cfg(not(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    )))]
    bail!("ASR is not compiled into this server")
}

fn prepare_model(progress: &(dyn Fn(String, Option<f32>) + Send + Sync)) -> Result<()> {
    model_registry::prepare_model(model_kind(), progress)?;
    probe_model()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SpeechSetupSnapshot {
    pub state: &'static str,
    pub message: String,
    pub progress: Option<f32>,
}

#[derive(Clone)]
pub struct SpeechSetup {
    snapshot: Arc<Mutex<SpeechSetupSnapshot>>,
    running: Arc<AtomicBool>,
}

impl SpeechSetup {
    pub fn new(ready: bool, supported: bool) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(SpeechSetupSnapshot {
                state: if ready {
                    "ready"
                } else if supported {
                    "preparing"
                } else {
                    "unavailable"
                },
                message: if ready {
                    "Transcription ready"
                } else if supported {
                    "Preparing transcription model"
                } else {
                    "This server cannot transcribe recordings"
                }
                .into(),
                progress: ready.then_some(1.0),
            })),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn snapshot(&self) -> SpeechSetupSnapshot {
        self.snapshot
            .lock()
            .expect("speech setup state poisoned")
            .clone()
    }

    pub fn start(
        &self,
        service: Arc<WorkspaceService>,
        principal: ServicePrincipal,
        jobs: RemoteAsrJobs,
    ) -> Result<()> {
        if !supported() {
            bail!("This server cannot transcribe recordings");
        }
        if service.asr_available() {
            jobs.schedule_pending(service, principal)?;
            return Ok(());
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        *self.snapshot.lock().expect("speech setup state poisoned") = SpeechSetupSnapshot {
            state: "preparing",
            message: "Preparing transcription model".into(),
            progress: None,
        };
        let state = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("margins-speech-setup".into())
            .spawn(move || {
                let result = prepare_model(&|message, progress| {
                    *state.snapshot.lock().expect("speech setup state poisoned") =
                        SpeechSetupSnapshot {
                            state: "preparing",
                            message,
                            progress,
                        };
                });
                match result {
                    Ok(()) => {
                        service.set_asr_available(true);
                        if let Err(error) = jobs.schedule_pending(service, principal) {
                            eprintln!("[margins-server] pending ASR scan failed: {error:#}");
                        }
                        *state.snapshot.lock().expect("speech setup state poisoned") =
                            SpeechSetupSnapshot {
                                state: "ready",
                                message: "Transcription ready".into(),
                                progress: Some(1.0),
                            };
                    }
                    Err(error) => {
                        eprintln!("[margins-server] speech setup needs attention: {error:#}");
                        *state.snapshot.lock().expect("speech setup state poisoned") =
                            SpeechSetupSnapshot {
                                state: "failed",
                                message: error.to_string().chars().take(500).collect(),
                                progress: None,
                            };
                    }
                }
                state.running.store(false, Ordering::Release);
            })
        {
            self.running.store(false, Ordering::Release);
            return Err(error.into());
        }
        Ok(())
    }
}

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
        Self::new(None)
    }
}

impl RemoteAsrJobs {
    /// Inject a backend into a server composition without loading model assets.
    pub fn with_backend(backend: Arc<dyn AsrBackend>) -> Self {
        Self::new(Some(backend))
    }

    fn new(backend: Option<Arc<dyn AsrBackend>>) -> Self {
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let (sender, receiver) = mpsc::sync_channel::<JobRequest>(16);
        let worker_in_flight = in_flight.clone();
        let worker_backend = backend.clone();
        std::thread::Builder::new()
            .name("margins-remote-asr".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    drain_service_jobs(request, &worker_in_flight, worker_backend.as_deref());
                }
            })
            .expect("failed to start remote ASR worker");
        Self { in_flight, sender }
    }
}

impl RemoteAsrJobs {
    pub fn schedule_pending(
        &self,
        service: Arc<WorkspaceService>,
        principal: ServicePrincipal,
    ) -> Result<()> {
        for job in service.pending_transcription_jobs()? {
            self.schedule(service.clone(), principal.clone(), job);
        }
        Ok(())
    }

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
            eprintln!("[margins-server] ASR queue deferred durable job: {error}");
        }
    }
}

fn drain_service_jobs(
    mut request: JobRequest,
    in_flight: &Mutex<HashSet<String>>,
    backend: Option<&dyn AsrBackend>,
) {
    loop {
        let key = format!("{}:{}", request.job.job_id, request.job.attempt);
        if let Err(error) = run_job(&request.service, &request.principal, &request.job, backend) {
            let _ = request.service.update_transcription_job(
                &request.job.job_id,
                request.job.attempt,
                "failed",
                None,
                None,
                Some(&format!("{error:#}")),
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
        let Some(job) = next else { break };
        request.job = job;
    }
}

fn run_job(
    service: &WorkspaceService,
    principal: &ServicePrincipal,
    job: &WorkspaceProcessingJobV1,
    injected_backend: Option<&dyn AsrBackend>,
) -> Result<()> {
    service.update_transcription_job(
        &job.job_id,
        job.attempt,
        "running",
        Some(0.05),
        None,
        None,
    )?;
    let loaded_backend = if injected_backend.is_none() {
        Some(load_backend()?)
    } else {
        None
    };
    let backend = injected_backend
        .unwrap_or_else(|| loaded_backend.as_ref().expect("loaded backend").as_ref());
    let result_ref = transcribe_remote_session(service, principal, &job.session_id, backend)?;
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

fn load_backend() -> Result<Box<dyn AsrBackend>> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        let path = model_registry::resolve_model(model_kind())?
            .context("CoreML model is not installed")?;
        return Ok(Box::new(
            margins_media::providers::coreml::CoreMlAsrBackend::from_dir_auto(path)?,
        ));
    }
    #[cfg(all(
        feature = "parakeet-asr",
        not(all(feature = "coreml-asr", target_os = "macos"))
    ))]
    {
        let (path, kind) =
            model_registry::resolve_parakeet_model()?.context("Parakeet model is not installed")?;
        return Ok(Box::new(
            margins_media::providers::parakeet::ParakeetOnnxBackend::from_dir(path, kind)?,
        ));
    }
    #[cfg(not(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    )))]
    bail!("ASR is not compiled into this server")
}

fn transcribe_remote_session(
    service: &WorkspaceService,
    principal: &ServicePrincipal,
    session_id: &SessionId,
    backend: &dyn AsrBackend,
) -> Result<String> {
    let record = service
        .repository_record(principal, session_id)?
        .context("finalized session has no canonical repository record")?;
    let artifacts = service.artifacts(principal, session_id.as_ref())?;
    let mut entries = Vec::new();
    let mut decoded_audio = false;
    let mut channel_labels = BTreeMap::from([
        (0, "you (mic)".to_string()),
        (1, "them (system)".to_string()),
    ]);
    let mut other_channels = HashMap::<String, u32>::new();
    for artifact in artifacts {
        let Some(kind) = artifact.kind.strip_prefix("audio_") else {
            continue;
        };
        let (lane_id, codec, container) = if let Some(lane) = kind.strip_suffix("_pcm") {
            (lane, AudioCodecV1::PcmS16Le, AudioContainerV1::Raw)
        } else if let Some(lane) = kind.strip_suffix("_opus") {
            (lane, AudioCodecV1::Opus, AudioContainerV1::PacketStream)
        } else if let Some(lane) = kind.strip_suffix("_webm") {
            (lane, AudioCodecV1::Opus, AudioContainerV1::Webm)
        } else {
            continue;
        };
        let format = service.capture_lane_format(session_id, lane_id)?;
        if format.codec != codec || format.container != container || format.channel_count != 1 {
            bail!("remote ASR artifact kind conflicts with declared lane format");
        }
        let segment = record
            .segments
            .iter()
            .find(|segment| segment.ordinal == artifact.ordinal.max(0) as u64)
            .context("audio artifact has no matching segment timeline")?;
        let bytes = service.artifact_content(principal, artifact.artifact_id.as_ref())?;
        let mono_16k = match (codec, container) {
            (AudioCodecV1::PcmS16Le, AudioContainerV1::Raw) => {
                remote_pcm_s16le_for_asr(&bytes, format.sample_rate_hz)?
            }
            (AudioCodecV1::Opus, AudioContainerV1::PacketStream)
                if format.sample_rate_hz == 16_000 =>
            {
                remote_opus_packet_stream_for_asr(&bytes)?
            }
            (AudioCodecV1::Opus, AudioContainerV1::Webm) if format.sample_rate_hz == 48_000 => {
                crate::webm::decode_webm_opus_to_mono_16k(&bytes).map_err(anyhow::Error::msg)?
            }
            _ => bail!("remote ASR lane uses an unsupported durable audio format"),
        };
        if mono_16k.is_empty() {
            continue;
        }
        decoded_audio = true;
        let channel = match lane_id {
            "mic" => 0,
            "system" => 1,
            _ => match other_channels.get(lane_id) {
                Some(channel) => *channel,
                None => {
                    let channel = 2 + other_channels.len() as u32;
                    other_channels.insert(lane_id.to_string(), channel);
                    channel
                }
            },
        };
        channel_labels
            .entry(channel)
            .or_insert_with(|| match lane_id {
                "mic" => "you (mic)".into(),
                "system" => "them (system)".into(),
                _ => lane_id.to_string(),
            });
        let result = backend.transcribe(AsrRequest {
            samples: mono_16k,
            sample_rate_hz: 16_000,
            session_offset_ms: segment.start_offset_ms,
            language: None,
        })?;
        entries.extend(
            result
                .words
                .into_iter()
                .filter(|word| !word.text.trim().is_empty())
                .map(|word| TranscriptWordEntry {
                    channel,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                    text: word.text,
                }),
        );
    }
    anyhow::ensure!(
        decoded_audio,
        "finalized session has no decodable audio artifacts"
    );
    entries.sort_by_key(|entry| (entry.start_ms, entry.channel));
    let memo = service.memo(principal, session_id)?;
    let memo_text = memo
        .lines
        .iter()
        .map(|line| {
            format!(
                "[{}] {}",
                elapsed((line.created_secs.max(0.0) * 1_000.0) as u64),
                line.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let timeline = render_remote_timeline(&entries, &channel_labels, &memo_text);
    let content = format!(
        "# Transcript\n\nSession: `{}`\nSource: Margins remote offline speech transcript and memo context.\nTranscript source: `offline`\n\n## Timeline\n\n{}\n",
        session_id.as_ref(),
        if timeline.is_empty() { "_No timestamped transcript or memo entries were available._" } else { &timeline },
    );
    let path = transcript_artifact_path(service.margins_dir(), session_id.as_ref());
    write_atomic_utf8(&path, &content)?;
    let registry_path = format!(".margins/artifacts/{}/transcript.md", session_id.as_ref());
    canonical::upsert_session_artifact(
        service.margins_dir(),
        session_id.as_ref(),
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &registry_path,
        "durable",
        None,
    )?;
    Ok(registry_path)
}

fn render_remote_timeline(
    entries: &[TranscriptWordEntry],
    labels: &BTreeMap<u32, String>,
    memo: &str,
) -> String {
    let (memos, _) = parse_timed_memo_lines(memo);
    interleave_timeline(entries, &memos, 2_000)
        .into_iter()
        .map(|row| match row {
            TimelineEvent::Transcript(entry) => format!(
                "[{}] {}: {}",
                elapsed(entry.start_ms),
                labels
                    .get(&entry.channel)
                    .map(String::as_str)
                    .unwrap_or("speaker"),
                entry.text.trim(),
            ),
            TimelineEvent::Memo(memo) => format!("[{}] memo: {}", elapsed(memo.at_ms), memo.text),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn write_atomic_utf8(path: &std::path::Path, body: &str) -> Result<()> {
    let parent = path.parent().context("transcript path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(body.as_bytes())?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_core::{AsrResult, TranscriptWord};

    struct FakeAsr;
    impl AsrBackend for FakeAsr {
        fn transcribe(
            &self,
            request: AsrRequest,
        ) -> std::result::Result<AsrResult, margins_core::TranscriptError> {
            Ok(AsrResult {
                words: vec![TranscriptWord {
                    start_ms: request.session_offset_ms + 1_000,
                    end_ms: request.session_offset_ms + 1_200,
                    text: "hello".into(),
                    speaker: None,
                    confidence_per_mille: None,
                }],
                detected_language: None,
            })
        }
    }

    #[test]
    fn fake_backend_keeps_memo_boundary_in_remote_timeline() {
        let result = FakeAsr
            .transcribe(AsrRequest {
                samples: vec![0.0; 16_000],
                sample_rate_hz: 16_000,
                session_offset_ms: 0,
                language: None,
            })
            .unwrap();
        let entries = result
            .words
            .into_iter()
            .map(|word| TranscriptWordEntry {
                channel: 0,
                start_ms: word.start_ms,
                end_ms: word.end_ms,
                text: word.text,
            })
            .collect::<Vec<_>>();
        let timeline = render_remote_timeline(
            &entries,
            &BTreeMap::from([(0, "you (mic)".into())]),
            "[00:02] Decision",
        );
        assert!(timeline.contains("[00:01] you (mic): hello"));
        assert!(timeline.contains("[00:02] memo: Decision"));
        assert!(timeline.find("hello").unwrap() < timeline.find("Decision").unwrap());
    }
}
