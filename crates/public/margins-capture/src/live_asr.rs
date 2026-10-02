#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
use anyhow::Context;
use anyhow::Result;
use std::io::{self, IsTerminal, Write};
#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
use std::path::Path;
#[cfg(feature = "audio-capture")]
use std::path::PathBuf;
#[cfg(any(test, feature = "audio-capture", all(feature = "coreml-asr", target_os = "macos")))]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(feature = "audio-capture")]
use std::sync::Mutex;
use std::sync::{mpsc, Arc};

#[cfg(feature = "audio-capture")]
pub fn start_live_transcript_worker(
    checkpoint_path: PathBuf,
    offset_ms: u64,
    status: Arc<AtomicU8>,
) -> Option<LiveTranscriptWorker> {
    match LiveTranscriptWorker::start(checkpoint_path, offset_ms, status.clone()) {
        Ok(worker) => worker,
        Err(error) => {
            // The TUI surfaces LIVE_TRANSCRIPTION_DEGRADED visually; keep stderr
            // clean and record the detail in diagnostics.
            crate::cli_log::event(
                "live_worker_unavailable",
                crate::cli_log::error_summary(&error),
            );
            status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            None
        }
    }
}

// ── Live transcription ────────────────────────────────────────────────────────

// The recorder sends native-rate samples before the worker resamples to 16 kHz.
// Budget 30 seconds for two lanes at up to 96 kHz.
#[cfg(any(test, feature = "audio-capture"))]
pub const LIVE_QUEUE_MAX_SAMPLES: u64 = 96_000 * 2 * 30;

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn live_checkpoint_complete(captured_until_ms: u64, decoded_until_ms: u64, dropped: u64) -> bool {
    dropped == 0 && decoded_until_ms.saturating_add(1_000) >= captured_until_ms
}

#[cfg(feature = "audio-capture")]
pub struct LiveTranscriptWorker {
    tx: mpsc::Sender<crate::recorder::LiveAudioChunk>,
    generation_clock: Arc<Mutex<crate::recorder::LiveGenerationClock>>,
    mic_accepted_samples: Arc<std::sync::atomic::AtomicU64>,
    system_accepted_samples: Arc<std::sync::atomic::AtomicU64>,
    mic_dropped_samples: Arc<std::sync::atomic::AtomicU64>,
    system_dropped_samples: Arc<std::sync::atomic::AtomicU64>,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    status: Arc<AtomicU8>,
    finish_tx: mpsc::Sender<u64>,
    join: std::thread::JoinHandle<Result<()>>,
}

#[cfg(any(test, feature = "audio-capture"))]
pub struct LiveTranscriptFinalizer {
    status: Arc<AtomicU8>,
    send_result: std::result::Result<(), mpsc::SendError<u64>>,
    join: std::thread::JoinHandle<Result<()>>,
}

#[cfg(feature = "audio-capture")]
impl LiveTranscriptWorker {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn start(checkpoint: PathBuf, offset_ms: u64, status: Arc<AtomicU8>) -> Result<Option<Self>> {
        let Some(model_dir) = margins_media::model_registry::resolve_coreml_dir() else {
            // Degraded state is shown in the TUI; record the cause quietly.
            crate::cli_log::event(
                "live_worker_unavailable",
                "category=setup details=coreml_model_dir_not_found",
            );
            status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            return Ok(None);
        };
        let (tx, rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let queued_samples = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mic_dropped_samples = Arc::new(AtomicU64::new(0));
        let system_dropped_samples = Arc::new(AtomicU64::new(0));
        let queued_for_thread = queued_samples.clone();
        let mic_dropped_for_thread = mic_dropped_samples.clone();
        let system_dropped_for_thread = system_dropped_samples.clone();
        let status_for_thread = status.clone();
        let join = std::thread::Builder::new()
            .name("margins-cli-live-transcription".into())
            .spawn(move || {
                use margins_media::providers::coreml::{
                    CoreMlStreamingConfig, FluidCoreMlAsr, StereoCoreMlAsrSession,
                };
                let warm_started = std::time::Instant::now();
                crate::cli_log::event("live_worker_warmup_started", "channels=2 backend=coreml");
                let prepare = || -> Result<StereoCoreMlAsrSession> {
                    let mut mic = FluidCoreMlAsr::from_dir_auto(&model_dir)?;
                    let mut system = FluidCoreMlAsr::from_dir_auto(&model_dir)?;
                    mic.warmup_models()?;
                    system.warmup_models()?;
                    Ok(StereoCoreMlAsrSession::new(
                        mic,
                        system,
                        CoreMlStreamingConfig::default(),
                    ))
                };
                let rolling = match prepare() {
                    Ok(rolling) => {
                        crate::cli_log::event(
                            "live_worker_ready",
                            format!("warmup_ms={}", warm_started.elapsed().as_millis()),
                        );
                        status_for_thread
                            .store(crate::app::LIVE_TRANSCRIPTION_READY, Ordering::Release);
                        rolling
                    }
                    Err(error) => {
                        crate::cli_log::event(
                            "live_worker_warmup_failed",
                            crate::cli_log::error_summary(&error),
                        );
                        status_for_thread
                            .store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
                        return Err(error);
                    }
                };
                run_live_worker(
                    rolling,
                    rx,
                    finish_rx,
                    checkpoint,
                    offset_ms,
                    queued_for_thread,
                    mic_dropped_for_thread,
                    system_dropped_for_thread,
                )
            })?;
        Ok(Some(Self {
            tx,
            generation_clock: Arc::new(Mutex::new(crate::recorder::LiveGenerationClock {
                generation: 0,
                session_offset_ms: offset_ms,
            })),
            mic_accepted_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            system_accepted_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mic_dropped_samples,
            system_dropped_samples,
            queued_samples,
            status,
            finish_tx,
            join,
        }))
    }

    #[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
    fn start(_checkpoint: PathBuf, _offset_ms: u64, status: Arc<AtomicU8>) -> Result<Option<Self>> {
        status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
        Ok(None)
    }

    pub fn sink_for_offset(&self, session_offset_ms: u64) -> crate::recorder::LiveAudioSink {
        let generation = {
            let mut clock = self
                .generation_clock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            clock.generation = clock.generation.saturating_add(1);
            clock.session_offset_ms = session_offset_ms;
            clock.generation
        };
        crate::recorder::LiveAudioSink {
            sender: self.tx.clone(),
            generation,
            generation_clock: self.generation_clock.clone(),
            mic_accepted_samples: self.mic_accepted_samples.clone(),
            system_accepted_samples: self.system_accepted_samples.clone(),
            mic_dropped_samples: self.mic_dropped_samples.clone(),
            system_dropped_samples: self.system_dropped_samples.clone(),
            queued_samples: self.queued_samples.clone(),
            queue_max_samples: LIVE_QUEUE_MAX_SAMPLES,
        }
    }

    pub fn dropped_counters(&self) -> (Arc<AtomicU64>, Arc<AtomicU64>) {
        (
            self.mic_dropped_samples.clone(),
            self.system_dropped_samples.clone(),
        )
    }

    pub fn begin_finish(self, duration_ms: u64) -> LiveTranscriptFinalizer {
        crate::cli_log::event(
            "live_worker_finish_requested",
            format!(
                "duration_ms={duration_ms} mic_accepted={} system_accepted={} mic_dropped={} system_dropped={} queued={}",
                self.mic_accepted_samples.load(Ordering::Acquire),
                self.system_accepted_samples.load(Ordering::Acquire),
                self.mic_dropped_samples.load(Ordering::Acquire),
                self.system_dropped_samples.load(Ordering::Acquire),
                self.queued_samples.load(Ordering::Acquire),
            ),
        );
        let send_result = self.finish_tx.send(duration_ms);
        drop(self.tx);
        LiveTranscriptFinalizer {
            status: self.status,
            send_result,
            join: self.join,
        }
    }
}

#[cfg(any(test, feature = "audio-capture"))]
impl LiveTranscriptFinalizer {
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    pub fn complete(self) -> Result<bool> {
        let finish_error = self.send_result.err().map(|error| error.to_string());
        let join_error = match self.join.join() {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(crate::cli_log::error_summary(&error)),
            Err(_) => Some("live transcription worker panicked".to_string()),
        };
        if let Some(reason) = optional_worker_failure_reason(
            self.status.load(Ordering::Acquire) == crate::app::LIVE_TRANSCRIPTION_DEGRADED,
            finish_error.as_deref(),
            join_error.as_deref(),
        ) {
            self.status
                .store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            crate::cli_log::event(
                "live_worker_finished",
                format!("status=unavailable reason={reason}"),
            );
            return Ok(false);
        }
        crate::cli_log::event("live_worker_finished", "status=ok");
        Ok(true)
    }

    pub fn wait_with_spinner(self, distill_requested: bool) -> Result<bool> {
        if self.is_finished() {
            return self.complete();
        }

        let message = if distill_requested {
            "Finishing transcription before creating the note…"
        } else {
            "Finishing transcription…"
        };
        let mut stderr = io::stderr().lock();
        let terminal = stderr.is_terminal();
        if terminal {
            const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
            let mut frame = 0usize;
            while !self.is_finished() {
                write!(stderr, "\r{} {message}", FRAMES[frame % FRAMES.len()])?;
                stderr.flush()?;
                frame = frame.wrapping_add(1);
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            write!(stderr, "\r\x1b[2K")?;
            stderr.flush()?;
        } else {
            writeln!(stderr, "{message}")?;
        }
        drop(stderr);

        let completed = self.complete()?;
        if terminal && completed {
            eprintln!("✓ Transcript ready.");
        }
        Ok(completed)
    }
}

#[cfg(any(test, feature = "audio-capture"))]
fn optional_worker_failure_reason(
    status_degraded: bool,
    finish_error: Option<&str>,
    join_error: Option<&str>,
) -> Option<String> {
    if let Some(error) = finish_error {
        Some(format!("finish_channel_closed: {error}"))
    } else if let Some(error) = join_error {
        Some(format!("worker_failed: {error}"))
    } else if status_degraded {
        Some("worker_reported_degraded".to_string())
    } else {
        None
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn run_live_worker(
    mut rolling: margins_media::providers::coreml::StereoCoreMlAsrSession,
    rx: mpsc::Receiver<crate::recorder::LiveAudioChunk>,
    finish_rx: mpsc::Receiver<u64>,
    checkpoint: PathBuf,
    offset_ms: u64,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    mic_dropped_samples: Arc<AtomicU64>,
    system_dropped_samples: Arc<AtomicU64>,
) -> Result<()> {
    use crate::recorder::LiveAudioChannel;
    use margins_core::AsrStreamDecoder;
    let mut mic_resampler = None;
    let mut system_resampler = None;
    let mut last_update_local_ms = 0;
    let mut mic_samples = 0u64;
    let mut system_samples = 0u64;
    let mut active_generation = None;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) => {
                let consumed = chunk.samples.len() as u64;
                debit_queued_samples(&queued_samples, consumed);
                if active_generation != Some(chunk.generation) {
                    let current_local_ms =
                        mic_samples.max(system_samples).saturating_mul(1_000) / 16_000;
                    let gap_samples =
                        timeline_gap_samples(chunk.session_offset_ms, offset_ms, current_local_ms);
                    if gap_samples > 0 {
                        let silence = vec![0.0; gap_samples as usize];
                        AsrStreamDecoder::append_audio(&mut rolling.mic, &silence);
                        AsrStreamDecoder::append_audio(&mut rolling.system, &silence);
                        mic_samples = mic_samples.saturating_add(gap_samples);
                        system_samples = system_samples.saturating_add(gap_samples);
                    }
                    active_generation = Some(chunk.generation);
                }
                let slot = match chunk.channel {
                    LiveAudioChannel::Mic => &mut mic_resampler,
                    LiveAudioChannel::System => &mut system_resampler,
                };
                if slot
                    .as_ref()
                    .is_none_or(|(rate, _)| *rate != chunk.sample_rate)
                {
                    *slot = Some((
                        chunk.sample_rate,
                        margins_media::timeline::RationalResampler::new(chunk.sample_rate, 16_000)?,
                    ));
                }
                let samples = slot
                    .as_mut()
                    .expect("resampler initialized")
                    .1
                    .process(&chunk.samples)?;
                match chunk.channel {
                    LiveAudioChannel::Mic => {
                        mic_samples = mic_samples.saturating_add(samples.len() as u64);
                        AsrStreamDecoder::append_audio(&mut rolling.mic, &samples);
                    }
                    LiveAudioChannel::System => {
                        system_samples = system_samples.saturating_add(samples.len() as u64);
                        AsrStreamDecoder::append_audio(&mut rolling.system, &samples);
                    }
                }
                let local_end_ms = mic_samples.max(system_samples).saturating_mul(1_000) / 16_000;
                if local_end_ms.saturating_sub(last_update_local_ms) >= 3_000 {
                    let mic = AsrStreamDecoder::update_until(&mut rolling.mic, local_end_ms)?;
                    let system = AsrStreamDecoder::update_until(&mut rolling.system, local_end_ms)?;
                    write_checkpoint(&checkpoint, &mic, &system, offset_ms, false, None, 0)?;
                    last_update_local_ms = local_end_ms;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let duration_ms = finish_rx
                    .recv()
                    .context("live transcription ended without a final duration")?;
                let local_duration_ms =
                    duration_ms.min(mic_samples.max(system_samples).saturating_mul(1_000) / 16_000);
                let mic = AsrStreamDecoder::finish_until(&mut rolling.mic, local_duration_ms)?;
                let system =
                    AsrStreamDecoder::finish_until(&mut rolling.system, local_duration_ms)?;
                let dropped_samples = mic_dropped_samples.load(Ordering::Acquire)
                    + system_dropped_samples.load(Ordering::Acquire);
                let captured_until_ms = offset_ms.saturating_add(duration_ms);
                let decoded_until_ms =
                    offset_ms.saturating_add(mic.decoded_until_ms.max(system.decoded_until_ms));
                let terminal =
                    live_checkpoint_complete(captured_until_ms, decoded_until_ms, dropped_samples);
                write_checkpoint(
                    &checkpoint,
                    &mic,
                    &system,
                    offset_ms,
                    terminal,
                    Some(captured_until_ms),
                    dropped_samples,
                )?;
                if !terminal {
                    bail!(
                        "live transcript incomplete: captured_until_ms={captured_until_ms} decoded_until_ms={decoded_until_ms} dropped_samples={dropped_samples}; run margins process for an offline transcript"
                    );
                }
                return Ok(());
            }
        }
    }
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn debit_queued_samples(queued_samples: &AtomicU64, consumed: u64) {
    // Recorder send currently publishes to the channel immediately before it
    // increments this accounting counter. A fast consumer can therefore win
    // that tiny race; wait for the producer's publication instead of leaving a
    // phantom queued balance that would eventually disable live transcription.
    for _ in 0..128 {
        if queued_samples
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued >= consumed).then(|| queued - consumed)
            })
            .is_ok()
        {
            return;
        }
        std::thread::yield_now();
    }
    let _ = queued_samples.fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
        Some(queued.saturating_sub(consumed))
    });
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn timeline_gap_samples(
    session_offset_ms: u64,
    initial_offset_ms: u64,
    current_local_ms: u64,
) -> u64 {
    session_offset_ms
        .saturating_sub(initial_offset_ms)
        .saturating_sub(current_local_ms)
        .saturating_mul(16_000)
        / 1_000
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn write_checkpoint(
    path: &Path,
    mic: &margins_core::AsrStreamUpdate,
    system: &margins_core::AsrStreamUpdate,
    offset_ms: u64,
    terminal: bool,
    captured_until_ms: Option<u64>,
    dropped_samples: u64,
) -> Result<()> {
    let to_entries = |words: &[margins_core::TranscriptWord], channel| {
        let timings = words
            .iter()
            .map(crate::asr::WordTiming::from)
            .collect::<Vec<_>>();
        margins_media::transcript::words_to_transcript_entries(&timings, channel, offset_ms)
    };
    let mut entries = to_entries(&mic.committed, 0);
    entries.extend(to_entries(&system.committed, 1));
    let decoded_until_ms = mic
        .decoded_until_ms
        .max(system.decoded_until_ms)
        .saturating_add(offset_ms);
    let committed_until_ms = mic
        .committed_until_ms
        .min(system.committed_until_ms)
        .saturating_add(offset_ms)
        .min(decoded_until_ms);
    let value = serde_json::json!({
        "version": 2,
        "terminal": terminal,
        "start_offset_ms": offset_ms,
        "decoded_until_ms": decoded_until_ms,
        "committed_until_ms": committed_until_ms,
        "captured_until_ms": captured_until_ms,
        "live_dropped_samples": dropped_samples,
        "transcripts": [{ "words": entries }],
    });
    write_checkpoint_value(path, &value)?;
    crate::cli_log::event(
        "live_checkpoint_written",
        format!(
            "terminal={terminal} decoded_until_ms={} committed_until_ms={} mic_words={} system_words={}",
            decoded_until_ms,
            committed_until_ms,
            mic.committed.len(),
            system.committed.len(),
        ),
    );
    Ok(())
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn write_checkpoint_value(path: &Path, value: &serde_json::Value) -> Result<()> {
    let parent = path.parent().context("checkpoint has no parent")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, &value)?;
    use std::io::Write as _;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("live_asr_tests.rs");
}
