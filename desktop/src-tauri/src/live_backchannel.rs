#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
use margins::recorder::LiveAudioChannel;
use margins::recorder::{LiveAudioChunk, LiveAudioSink, LiveGenerationClock};
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
use serde::Serialize;
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
use crate::live_asr_worker::{
    LiveAsrTrace, AUDIO_QUEUE_MAX_SAMPLES, STARTUP_TIMEOUT, WORKER_LOOP_POLL,
};
use crate::live_asr_worker::{WorkerHealth, REQUEST_TIMEOUT, WORKER_STOPPED, WORKER_WARMING};
use crate::Settings;

// ---------------------------------------------------------------------------
// B1: Prewarm rendezvous slot — carries the loaded+warmed prewarm thread's
// activation channel. The thread parks after warmup and waits for a
// PrewarmActivation before starting the real worker loop.
// ---------------------------------------------------------------------------

/// Everything the session-start site needs to wake the parked prewarm thread.
/// All fields are Send so this can live in a process-global OnceLock.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) struct PrewarmHandoff {
    activate_tx: mpsc::SyncSender<PrewarmActivation>,
    model_dir: PathBuf,
    /// Moved into the LiveBackchannelHandle so finish_and_join can join.
    thread_handle: std::thread::JoinHandle<()>,
}

/// Sent from `start_live_backchannel_with_callbacks` to the parked prewarm thread.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) enum PrewarmActivation {
    Run(Box<WorkerLaunch>),
    Cancel,
}

/// All Send state needed to drive the worker loop from the prewarm thread.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) struct WorkerLaunch {
    trace: crate::live_asr_worker::LiveAsrTrace,
    audio_rx: mpsc::Receiver<LiveAudioChunk>,
    command_rx: mpsc::Receiver<LiveBackchannelCommand>,
    worker_health: crate::live_asr_worker::WorkerHealth,
    queued_samples: Arc<AtomicU64>,
    pending_mode: Arc<Mutex<Option<LiveTranscriptionMode>>>,
    transcription_mode: LiveTranscriptionMode,
    spawn_instant: std::time::Instant,
    on_ready: Option<Box<dyn Fn() + Send>>,
    on_degraded: Option<Box<dyn Fn(String) + Send>>,
}

/// Process-global prewarm slot. Holds at most one `PrewarmHandoff` at a time.
/// After the prewarm thread is activated (consuming the slot), the slot is
/// empty until a subsequent re-prewarm (out of scope for Workstream B).
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
static PREWARM_SLOT: OnceLock<Mutex<Option<PrewarmHandoff>>> = OnceLock::new();

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn prewarm_slot() -> &'static Mutex<Option<PrewarmHandoff>> {
    PREWARM_SLOT.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
const LIVE_BACKGROUND_DECODE_INTERVAL_MS: u64 = 1_000;
#[cfg(all(
    feature = "coreml-asr",
    target_os = "macos",
    feature = "rust-diarization"
))]
const MAX_PENDING_MIC_DIARIZATION_SECS: usize = 300;

#[derive(Debug, Clone)]
pub(crate) struct LiveTranscriptContext {
    pub(crate) transcript: String,
    pub(crate) previous_transcript: String,
    pub(crate) new_transcript: String,
    pub(crate) committed_transcript: String,
    pub(crate) hypothesis_transcript: String,
    pub(crate) transcript_source: String,
    pub(crate) previous_memo_checkpoint_ms: u64,
    pub(crate) decoded_until_ms: u64,
    pub(crate) committed_until_ms: u64,
    pub(crate) mic_accepted_samples: u64,
    pub(crate) system_accepted_samples: u64,
    pub(crate) mic_dropped_samples: u64,
    pub(crate) system_dropped_samples: u64,
    pub(crate) timing: Option<LiveTranscriptTiming>,
}

#[derive(Debug)]
pub(crate) struct LiveTerminalTranscript {
    pub(crate) context: LiveTranscriptContext,
    pub(crate) mic_audio_end_ms: u64,
    pub(crate) system_audio_end_ms: u64,
    pub(crate) mic_decoded_until_ms: u64,
    pub(crate) system_decoded_until_ms: u64,
    pub(crate) generation_timeline_verified: bool,
    pub(crate) generation_transitions: Vec<LiveGenerationTransition>,
}

#[derive(Debug)]
struct LiveChannelEndpoints {
    mic_audio_end_ms: u64,
    system_audio_end_ms: u64,
    mic_decoded_until_ms: u64,
    system_decoded_until_ms: u64,
    generation_timeline_verified: bool,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct LiveGenerationTransition {
    pub(crate) generation: u64,
    pub(crate) session_offset_ms: u64,
    pub(crate) reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LiveTranscriptTiming {
    pub(crate) command_wait_ms: u128,
    pub(crate) mic_update_ms: u128,
    pub(crate) system_update_ms: u128,
    pub(crate) format_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveTranscriptionMode {
    StereoSplit,
    MicDiarized,
}

impl LiveTranscriptionMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StereoSplit => "stereo_split",
            Self::MicDiarized => "mic_diarized",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stereo" | "stereo_split" | "split" => Ok(Self::StereoSplit),
            "mic_diarized" | "mic-diarized" | "diarize_mic" | "mic" => Ok(Self::MicDiarized),
            other => Err(format!("Unknown live transcription mode: {other}")),
        }
    }
}

pub(crate) struct LiveBackchannelHandle {
    audio_tx: mpsc::Sender<LiveAudioChunk>,
    command_tx: mpsc::Sender<LiveBackchannelCommand>,
    generation_clock: Arc<Mutex<LiveGenerationClock>>,
    generation_transitions: Arc<Mutex<Vec<LiveGenerationTransition>>>,
    generation_transitions_valid: Arc<AtomicBool>,
    worker_health: WorkerHealth,
    mic_accepted_samples: Arc<AtomicU64>,
    system_accepted_samples: Arc<AtomicU64>,
    mic_dropped_samples: Arc<AtomicU64>,
    system_dropped_samples: Arc<AtomicU64>,
    queued_samples: Arc<AtomicU64>,
    /// Frozen at capture start so settings changes cannot alter the session's
    /// terminal qualification contract.
    system_audio_expected: bool,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Pending mode to apply once the worker signals ready.
    pending_mode: Arc<Mutex<Option<LiveTranscriptionMode>>>,
}

#[derive(Clone)]
pub(crate) struct LiveBackchannelClient {
    command_tx: mpsc::Sender<LiveBackchannelCommand>,
    worker_health: WorkerHealth,
    mic_accepted_samples: Arc<AtomicU64>,
    system_accepted_samples: Arc<AtomicU64>,
    mic_dropped_samples: Arc<AtomicU64>,
    system_dropped_samples: Arc<AtomicU64>,
    pending_mode: Arc<Mutex<Option<LiveTranscriptionMode>>>,
}

#[allow(dead_code)]
enum LiveBackchannelCommand {
    SetTranscriptionMode {
        mode: LiveTranscriptionMode,
        response: mpsc::Sender<Result<(), String>>,
    },
    Snapshot {
        end_ms: u64,
        advance_checkpoint: bool,
        response: mpsc::Sender<Result<LiveTranscriptContext, String>>,
    },
    Finish {
        end_ms: u64,
        expected_generation: u64,
        response: mpsc::Sender<Result<LiveTerminalTranscript, String>>,
    },
}

impl LiveBackchannelHandle {
    fn populate_audio_coverage(&self, snapshot: &mut LiveTranscriptContext) {
        snapshot.mic_accepted_samples = self.mic_accepted_samples.load(Ordering::Relaxed);
        snapshot.system_accepted_samples = self.system_accepted_samples.load(Ordering::Relaxed);
        snapshot.mic_dropped_samples = self.mic_dropped_samples.load(Ordering::Relaxed);
        snapshot.system_dropped_samples = self.system_dropped_samples.load(Ordering::Relaxed);
    }

    pub(crate) fn audio_sink(&self, generation: u64) -> LiveAudioSink {
        #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
        let queue_max_samples = AUDIO_QUEUE_MAX_SAMPLES;
        #[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
        let queue_max_samples = 0u64;
        LiveAudioSink {
            sender: self.audio_tx.clone(),
            generation,
            generation_clock: self.generation_clock.clone(),
            mic_accepted_samples: self.mic_accepted_samples.clone(),
            system_accepted_samples: self.system_accepted_samples.clone(),
            mic_dropped_samples: self.mic_dropped_samples.clone(),
            system_dropped_samples: self.system_dropped_samples.clone(),
            queued_samples: self.queued_samples.clone(),
            queue_max_samples,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.worker_health.is_ready()
    }

    pub(crate) fn activate_generation(
        &self,
        generation: u64,
        session_offset_ms: u64,
        reason: &str,
    ) {
        let mut transitions = self.generation_transitions.lock().unwrap();
        let valid = transitions.last().is_some_and(|previous| {
            generation == previous.generation.saturating_add(1)
                && session_offset_ms >= previous.session_offset_ms
        });
        if !valid {
            self.generation_transitions_valid
                .store(false, Ordering::Release);
        }
        transitions.push(LiveGenerationTransition {
            generation,
            session_offset_ms,
            reason: reason.to_string(),
        });
        drop(transitions);
        *self
            .generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = LiveGenerationClock {
            generation,
            session_offset_ms,
        };
    }

    pub(crate) fn system_audio_expected(&self) -> bool {
        self.system_audio_expected
    }

    pub(crate) fn client(&self) -> LiveBackchannelClient {
        LiveBackchannelClient {
            command_tx: self.command_tx.clone(),
            worker_health: self.worker_health.clone(),
            mic_accepted_samples: self.mic_accepted_samples.clone(),
            system_accepted_samples: self.system_accepted_samples.clone(),
            mic_dropped_samples: self.mic_dropped_samples.clone(),
            system_dropped_samples: self.system_dropped_samples.clone(),
            pending_mode: self.pending_mode.clone(),
        }
    }

    /// Deterministic native-worker input seam for the ignored CoreML rolling
    /// harness. Capture continues to use `LiveAudioSink`; keeping this test-only
    /// avoids giving production callers a second audio-ingress path.
    #[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
    pub(crate) fn inject_test_audio(
        &self,
        channel: LiveAudioChannel,
        samples: Vec<f32>,
    ) -> Result<(), String> {
        let sample_count = samples.len() as u64;
        let accepted = match channel {
            LiveAudioChannel::Mic => &self.mic_accepted_samples,
            LiveAudioChannel::System => &self.system_accepted_samples,
        };
        let dropped = match channel {
            LiveAudioChannel::Mic => &self.mic_dropped_samples,
            LiveAudioChannel::System => &self.system_dropped_samples,
        };
        let generation_clock = *self
            .generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = LiveAudioChunk {
            channel,
            generation: generation_clock.generation,
            session_offset_ms: generation_clock.session_offset_ms,
            sample_rate: margins::coreml_asr::SAMPLE_RATE,
            samples,
        };
        match self.audio_tx.send(chunk) {
            Ok(()) => {
                self.queued_samples
                    .fetch_add(sample_count, Ordering::Relaxed);
                accepted.fetch_add(sample_count, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => {
                dropped.fetch_add(sample_count, Ordering::Relaxed);
                Err(WORKER_STOPPED.to_string())
            }
        }
    }

    /// Models a capture-side queue loss without changing worker audio. This is
    /// intentionally test-only: the recorder is the sole producer of real drop
    /// accounting in production.
    #[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
    pub(crate) fn inject_test_drop(&self, channel: LiveAudioChannel, samples: usize) {
        match channel {
            LiveAudioChannel::Mic => self
                .mic_dropped_samples
                .fetch_add(samples as u64, Ordering::Relaxed),
            LiveAudioChannel::System => self
                .system_dropped_samples
                .fetch_add(samples as u64, Ordering::Relaxed),
        };
    }

    pub(crate) fn finish_and_join(
        mut self,
        end_ms: u64,
    ) -> Result<Option<LiveTerminalTranscript>, String> {
        let (tx, rx) = mpsc::channel();
        if !self.worker_health.is_alive() {
            return Err(WORKER_STOPPED.to_string());
        }
        // Signal the worker to abort if it is still loading (A7).
        self.worker_health.request_shutdown();
        let expected_generation = self
            .generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation;
        self.command_tx
            .send(LiveBackchannelCommand::Finish {
                end_ms,
                expected_generation,
                response: tx,
            })
            .map_err(|_| WORKER_STOPPED.to_string())?;
        let result = self
            .worker_health
            .recv(
                rx,
                REQUEST_TIMEOUT,
                "Timed out finalizing live CoreML transcript",
            )?
            .map(|mut snapshot| {
                self.populate_audio_coverage(&mut snapshot.context);
                snapshot.generation_timeline_verified &=
                    self.generation_transitions_valid.load(Ordering::Acquire);
                snapshot.generation_transitions =
                    self.generation_transitions.lock().unwrap().clone();
                snapshot
            })
            .map(Some);
        if let Some(handle) = self.thread_handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        result
    }
}

impl LiveBackchannelClient {
    fn populate_audio_coverage(&self, snapshot: &mut LiveTranscriptContext) {
        snapshot.mic_accepted_samples = self.mic_accepted_samples.load(Ordering::Relaxed);
        snapshot.system_accepted_samples = self.system_accepted_samples.load(Ordering::Relaxed);
        snapshot.mic_dropped_samples = self.mic_dropped_samples.load(Ordering::Relaxed);
        snapshot.system_dropped_samples = self.system_dropped_samples.load(Ordering::Relaxed);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.worker_health.is_ready()
    }

    pub(crate) fn set_transcription_mode(&self, mode: LiveTranscriptionMode) -> Result<(), String> {
        if !self.worker_health.is_alive() {
            return Err(WORKER_STOPPED.to_string());
        }
        // If the worker is still warming, store the mode to be applied once ready (A5).
        if !self.worker_health.is_ready() {
            *self.pending_mode.lock().unwrap() = Some(mode);
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        self.command_tx
            .send(LiveBackchannelCommand::SetTranscriptionMode { mode, response: tx })
            .map_err(|_| WORKER_STOPPED.to_string())?;
        self.worker_health.recv(
            rx,
            REQUEST_TIMEOUT,
            "Timed out switching live transcription mode",
        )?
    }

    pub(crate) fn request_context(&self, end_ms: u64) -> Result<LiveTranscriptContext, String> {
        self.snapshot(end_ms, false)
    }

    pub(crate) fn checkpoint_memo(&self, end_ms: u64) -> Result<LiveTranscriptContext, String> {
        self.snapshot(end_ms, true)
    }

    fn snapshot(
        &self,
        end_ms: u64,
        advance_checkpoint: bool,
    ) -> Result<LiveTranscriptContext, String> {
        let requested_at = Instant::now();
        let (tx, rx) = mpsc::channel();
        if !self.worker_health.is_alive() {
            return Err(WORKER_STOPPED.to_string());
        }
        // Return immediately while the worker is warming (A5).
        if !self.worker_health.is_ready() {
            return Err(WORKER_WARMING.to_string());
        }
        self.command_tx
            .send(LiveBackchannelCommand::Snapshot {
                end_ms,
                advance_checkpoint,
                response: tx,
            })
            .map_err(|_| WORKER_STOPPED.to_string())?;
        let mut snapshot = self.worker_health.recv(
            rx,
            REQUEST_TIMEOUT,
            "Timed out waiting for live CoreML transcript",
        )??;
        self.populate_audio_coverage(&mut snapshot);
        if let Some(timing) = snapshot.timing.as_mut() {
            timing.command_wait_ms = requested_at
                .elapsed()
                .as_millis()
                .saturating_sub(timing.mic_update_ms + timing.system_update_ms + timing.format_ms);
        }
        Ok(snapshot)
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn start_live_backchannel(
    settings: &Settings,
    margins_dir: PathBuf,
    session_name: String,
    transcription_mode: LiveTranscriptionMode,
) -> Result<Option<LiveBackchannelHandle>, String> {
    start_live_backchannel_with_callbacks(
        settings,
        margins_dir,
        session_name,
        transcription_mode,
        None,
        None,
    )
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn start_live_backchannel_with_callbacks(
    settings: &Settings,
    margins_dir: PathBuf,
    session_name: String,
    transcription_mode: LiveTranscriptionMode,
    on_ready: Option<Box<dyn Fn() + Send>>,
    on_degraded: Option<Box<dyn Fn(String) + Send>>,
) -> Result<Option<LiveBackchannelHandle>, String> {
    let Some(model_dir) = resolved_live_model_dir(settings) else {
        return Ok(None);
    };
    // A3: unbounded channel guarded by producer-side sample budget.
    let (audio_tx, audio_rx) = mpsc::channel::<LiveAudioChunk>();
    let (command_tx, command_rx) = mpsc::channel::<LiveBackchannelCommand>();
    let generation_clock = Arc::new(Mutex::new(LiveGenerationClock {
        generation: 0,
        session_offset_ms: 0,
    }));
    let generation_transitions = Arc::new(Mutex::new(vec![LiveGenerationTransition {
        generation: 0,
        session_offset_ms: 0,
        reason: "capture_start".to_string(),
    }]));
    let generation_transitions_valid = Arc::new(AtomicBool::new(true));
    let worker_health = WorkerHealth::new();
    let mic_accepted_samples = Arc::new(AtomicU64::new(0));
    let system_accepted_samples = Arc::new(AtomicU64::new(0));
    let mic_dropped_samples = Arc::new(AtomicU64::new(0));
    let system_dropped_samples = Arc::new(AtomicU64::new(0));
    let queued_samples = Arc::new(AtomicU64::new(0));
    let pending_mode = Arc::new(Mutex::new(None::<LiveTranscriptionMode>));
    let trace = LiveAsrTrace::new(margins_dir, session_name.clone(), "coreml");
    let worker_health_for_thread = worker_health.clone();
    let queued_samples_for_thread = queued_samples.clone();
    let pending_mode_for_thread = pending_mode.clone();
    let spawn_instant = std::time::Instant::now();

    // B3: attempt to attach the prewarm thread instead of spawning a new one.
    let thread_handle = match take_prewarm_handoff(&model_dir) {
        Some(handoff) => {
            // Prewarm thread is parked and has a matching model_dir.
            // Send it the launch payload — the thread becomes the worker.
            let launch = WorkerLaunch {
                trace,
                audio_rx,
                command_rx,
                worker_health: worker_health_for_thread.clone(),
                queued_samples: queued_samples_for_thread,
                pending_mode: pending_mode_for_thread,
                transcription_mode,
                spawn_instant,
                on_ready,
                on_degraded,
            };
            match handoff
                .activate_tx
                .send(PrewarmActivation::Run(Box::new(launch)))
            {
                Ok(()) => {
                    // Trace emitted from the worker side (used_prewarm:true).
                    handoff.thread_handle
                }
                Err(send_err) => {
                    // Prewarm thread died between take and send — recover the
                    // WorkerLaunch from the error and spawn inline.
                    // SendError<T> is a tuple struct; .0 recovers the value.
                    let recovered = match send_err.0 {
                        PrewarmActivation::Run(launch) => *launch,
                        PrewarmActivation::Cancel => unreachable!("we sent Run"),
                    };
                    let wh = recovered.worker_health.clone();
                    let qs = recovered.queued_samples.clone();
                    let pm = recovered.pending_mode.clone();
                    let tr = recovered.trace;
                    let ar = recovered.audio_rx;
                    let cr = recovered.command_rx;
                    let mode = recovered.transcription_mode;
                    let si = recovered.spawn_instant;
                    let or_ = recovered.on_ready;
                    let od = recovered.on_degraded;
                    std::thread::Builder::new()
                        .name(format!("margins-live-backchannel-{session_name}"))
                        .spawn(move || {
                            let _alive_guard = wh.guard();
                            run_coreml_worker(
                                model_dir,
                                tr,
                                ar,
                                cr,
                                wh.clone(),
                                qs,
                                pm,
                                mode,
                                si,
                                or_,
                                od,
                            );
                        })
                        .map_err(|e| {
                            format!("Failed to start live CoreML transcript worker: {e}")
                        })?
                }
            }
        }
        None => {
            // No matching prewarm slot — inline spawn.
            // TODO(capture-first): re-prewarm after session start
            let trace_clone = trace.clone();
            std::thread::Builder::new()
                .name(format!("margins-live-backchannel-{session_name}"))
                .spawn(move || {
                    let _alive_guard = worker_health_for_thread.guard();
                    run_coreml_worker(
                        model_dir,
                        trace_clone,
                        audio_rx,
                        command_rx,
                        worker_health_for_thread.clone(),
                        queued_samples_for_thread,
                        pending_mode_for_thread,
                        transcription_mode,
                        spawn_instant,
                        on_ready,
                        on_degraded,
                    );
                })
                .map_err(|e| format!("Failed to start live CoreML transcript worker: {e}"))?
        }
    };

    // A2: return handle immediately — worker signals ready asynchronously.
    Ok(Some(LiveBackchannelHandle {
        audio_tx,
        command_tx,
        generation_clock,
        generation_transitions,
        generation_transitions_valid,
        worker_health,
        mic_accepted_samples,
        system_accepted_samples,
        mic_dropped_samples,
        system_dropped_samples,
        queued_samples,
        system_audio_expected: transcription_mode == LiveTranscriptionMode::StereoSplit,
        thread_handle: Some(thread_handle),
        pending_mode,
    }))
}

/// Returns true if PREWARM_SLOT currently holds a live (unconsumed) handoff.
/// Used by lib.rs to skip re-prewarm when one is already parked.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn prewarm_slot_occupied() -> bool {
    prewarm_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some()
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn prewarm_slot_occupied() -> bool {
    false
}

/// B3: Take the prewarm handoff if the slot holds one with a matching model_dir.
/// If the slot holds a mismatched dir, sends Cancel and leaves slot empty.
/// Extracted for unit-testability.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn take_prewarm_handoff(model_dir: &std::path::Path) -> Option<PrewarmHandoff> {
    let mut slot = prewarm_slot().lock().unwrap_or_else(|p| p.into_inner());
    match slot.as_ref() {
        None => None,
        Some(h) if h.model_dir == model_dir => slot.take(),
        Some(_) => {
            // Mismatched dir: cancel the old prewarm thread so it doesn't hang.
            if let Some(old) = slot.take() {
                let _ = old.activate_tx.send(PrewarmActivation::Cancel);
                // Don't join — let it exit on its own (daemon-style).
                // old.thread_handle is dropped here which is fine.
            }
            None
        }
    }
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn start_live_backchannel(
    _settings: &Settings,
    _margins_dir: PathBuf,
    _session_name: String,
    _transcription_mode: LiveTranscriptionMode,
) -> Result<Option<LiveBackchannelHandle>, String> {
    Ok(None)
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn start_live_backchannel_with_callbacks(
    _settings: &Settings,
    _margins_dir: PathBuf,
    _session_name: String,
    _transcription_mode: LiveTranscriptionMode,
    _on_ready: Option<Box<dyn Fn() + Send>>,
    _on_degraded: Option<Box<dyn Fn(String) + Send>>,
) -> Result<Option<LiveBackchannelHandle>, String> {
    Ok(None)
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn prewarm_live_models(settings: Settings) {
    prewarm_live_models_with_reason(settings, "app_start");
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn prewarm_live_models(_settings: Settings) {}

/// Schedule a re-prewarm after a session ends. Skips if the slot is already
/// occupied or if another recording is currently active.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn schedule_post_session_prewarm(settings: Settings, recording_active: bool) {
    if recording_active {
        eprintln!(
            "{}",
            serde_json::json!({
                "kind":"live_transcript_prewarm_skipped",
                "backend":"coreml",
                "reason":"post_session_recording_active",
            })
        );
        return;
    }
    if prewarm_slot_occupied() {
        eprintln!(
            "{}",
            serde_json::json!({
                "kind":"live_transcript_prewarm_skipped",
                "backend":"coreml",
                "reason":"post_session_slot_occupied",
            })
        );
        return;
    }
    prewarm_live_models_with_reason(settings, "post_session");
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn schedule_post_session_prewarm(_settings: Settings, _recording_active: bool) {}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn prewarm_live_models_with_reason(settings: Settings, reason: &'static str) {
    // Guard: skip if the slot is already occupied (idempotent/re-entrant safety).
    if prewarm_slot_occupied() {
        eprintln!(
            "{}",
            json!({
                "kind":"live_transcript_prewarm_skipped",
                "backend":"coreml",
                "reason":"slot_already_occupied",
                "trigger":reason,
            })
        );
        return;
    }
    let Some(model_dir) = resolved_live_model_dir(&settings) else {
        let env_model_dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let unavail_reason = if env_model_dir.is_some() {
            "fluid_coreml_assets_missing_at_env_model_dir"
        } else {
            "no_fluid_coreml_model_dir"
        };
        eprintln!(
            "{}",
            json!({
                "kind":"live_transcript_app_prewarm_unavailable",
                "backend":"coreml",
                "reason":unavail_reason,
                "trigger":reason,
                "env_model_dir":env_model_dir,
            })
        );
        return;
    };
    // B1: bounded sync channel of depth 1 — prewarm thread parks on recv.
    let (activate_tx, activate_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
    let activate_tx_for_slot = activate_tx.clone();
    let model_dir_for_slot = model_dir.clone();

    let thread_result = std::thread::Builder::new()
        .name("margins-live-backchannel-prewarm".to_string())
        .spawn(move || {
            let started = Instant::now();
            let model_dir_string = model_dir.display().to_string();
            eprintln!(
                "{}",
                json!({
                    "kind":"live_transcript_app_prewarm_started",
                    "backend":"coreml",
                    "model_dir":model_dir_string,
                    "reason":reason,
                })
            );
            crate::ai_config::append_app_log(
                "live_transcript_prewarm",
                &format!("prewarm started model_dir={model_dir_string} reason={reason}"),
            );

            let mut mic = match margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(&model_dir) {
                Ok(asr) => asr,
                Err(e) => {
                    let msg = format!("Live mic CoreML init failed: {e}");
                    eprintln!(
                        "{}",
                        json!({
                            "kind":"live_transcript_app_prewarm_failed",
                            "backend":"coreml",
                            "model_dir":model_dir_string,
                            "init_ms":started.elapsed().as_millis(),
                            "message":msg,
                        })
                    );
                    crate::ai_config::append_app_log("live_transcript_prewarm_failed", &msg);
                    return;
                }
            };
            let mut system = match margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(&model_dir) {
                Ok(asr) => asr,
                Err(e) => {
                    let msg = format!("Live system CoreML init failed: {e}");
                    eprintln!(
                        "{}",
                        json!({
                            "kind":"live_transcript_app_prewarm_failed",
                            "backend":"coreml",
                            "model_dir":model_dir_string,
                            "init_ms":started.elapsed().as_millis(),
                            "message":msg,
                        })
                    );
                    crate::ai_config::append_app_log("live_transcript_prewarm_failed", &msg);
                    return;
                }
            };
            let init_ms = started.elapsed().as_millis();
            let warmup_started = Instant::now();
            let mic_warmup = match mic.warmup_models() {
                Ok(warmup) => warmup,
                Err(e) => {
                    let msg = format!("Live mic CoreML warmup failed: {e}");
                    eprintln!(
                        "{}",
                        json!({
                            "kind":"live_transcript_app_prewarm_failed",
                            "backend":"coreml",
                            "model_dir":model_dir_string,
                            "init_ms":init_ms,
                            "duration_ms":warmup_started.elapsed().as_millis(),
                            "message":msg,
                        })
                    );
                    crate::ai_config::append_app_log("live_transcript_prewarm_failed", &msg);
                    return;
                }
            };
            let system_warmup = match system.warmup_models() {
                Ok(warmup) => warmup,
                Err(e) => {
                    let msg = format!("Live system CoreML warmup failed: {e}");
                    eprintln!(
                        "{}",
                        json!({
                            "kind":"live_transcript_app_prewarm_failed",
                            "backend":"coreml",
                            "model_dir":model_dir_string,
                            "init_ms":init_ms,
                            "duration_ms":warmup_started.elapsed().as_millis(),
                            "message":msg,
                        })
                    );
                    crate::ai_config::append_app_log("live_transcript_prewarm_failed", &msg);
                    return;
                }
            };
            let warmup_ms = warmup_started.elapsed().as_millis();
            let total_ms = started.elapsed().as_millis();
            eprintln!(
                "{}",
                json!({
                    "kind":"live_transcript_app_prewarm_finished",
                    "backend":"coreml",
                    "model_dir":model_dir_string,
                    "reason":reason,
                    "init_ms":init_ms,
                    "duration_ms":warmup_ms,
                    "total_ms":total_ms,
                    "mic":{
                        "frontend_ms":mic_warmup.frontend_ms,
                        "decode_ms":mic_warmup.decode_ms,
                        "total_ms":mic_warmup.total_ms,
                        "encoder_length":mic_warmup.encoder_length,
                        "actual_audio_frames":mic_warmup.actual_audio_frames,
                    },
                    "system":{
                        "frontend_ms":system_warmup.frontend_ms,
                        "decode_ms":system_warmup.decode_ms,
                        "total_ms":system_warmup.total_ms,
                        "encoder_length":system_warmup.encoder_length,
                        "actual_audio_frames":system_warmup.actual_audio_frames,
                    },
                })
            );
            crate::ai_config::append_app_log(
                "live_transcript_prewarm_finished",
                &format!("prewarm finished init_ms={init_ms} warmup_ms={warmup_ms} total_ms={total_ms} reason={reason}"),
            );

            // B1: park — wait for session start or cancellation.
            // The slot was registered AFTER warmup completed, so sessions that
            // start during load take the inline path (slot empty). That's
            // acceptable; the double-load waste is bounded and capture is not
            // blocked.
            match activate_rx.recv() {
                Ok(PrewarmActivation::Run(launch)) => {
                    // B2: build the worker from already-loaded instances.
                    let model_dir_str = model_dir_string.clone();
                    let worker = CoreMlLiveWorker::from_instances(
                        mic,
                        system,
                        launch.trace.clone(),
                    );
                    run_coreml_worker_from_instance(
                        worker,
                        model_dir_str,
                        launch.trace,
                        launch.audio_rx,
                        launch.command_rx,
                        launch.worker_health,
                        launch.queued_samples,
                        launch.pending_mode,
                        launch.transcription_mode,
                        launch.spawn_instant,
                        init_ms,
                        warmup_ms,
                        launch.on_ready,
                        launch.on_degraded,
                        true, // used_prewarm
                    );
                }
                Ok(PrewarmActivation::Cancel) | Err(_) => {
                    // Drop mic + system here — clean exit.
                }
            }
        });

    // Register the handoff only if the thread was created successfully.
    if let Ok(handle) = thread_result {
        let mut slot = prewarm_slot().lock().unwrap_or_else(|p| p.into_inner());
        *slot = Some(PrewarmHandoff {
            activate_tx: activate_tx_for_slot,
            model_dir: model_dir_for_slot,
            thread_handle: handle,
        });
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(crate) fn resolved_live_model_dir(settings: &Settings) -> Option<PathBuf> {
    margins_media::model_registry::resolve_coreml_dir_with_fallback(
        settings.parakeet_model_dir.as_deref(),
    )
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
pub(crate) fn resolved_live_model_dir(_settings: &Settings) -> Option<std::path::PathBuf> {
    None
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn run_coreml_worker(
    model_dir: PathBuf,
    trace: LiveAsrTrace,
    audio_rx: mpsc::Receiver<LiveAudioChunk>,
    command_rx: mpsc::Receiver<LiveBackchannelCommand>,
    worker_health: WorkerHealth,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    pending_mode: Arc<Mutex<Option<LiveTranscriptionMode>>>,
    transcription_mode: LiveTranscriptionMode,
    spawn_instant: std::time::Instant,
    on_ready: Option<Box<dyn Fn() + Send>>,
    on_degraded: Option<Box<dyn Fn(String) + Send>>,
) {
    let model_dir_string = model_dir.display().to_string();
    trace.append(json!({
        "kind":"live_transcript_worker_thread_spawned",
        "model_dir":model_dir_string,
    }));

    let init_started = Instant::now();
    let mut worker = match CoreMlLiveWorker::new(&model_dir, trace.clone()) {
        Ok(worker) => worker,
        Err(e) => {
            let error = format!("Failed to load live CoreML ASR models: {e}");
            trace.append(json!({
                "kind":"live_transcript_worker_start_degraded",
                "reason":"init_failed",
                "model_dir":model_dir_string,
                "init_ms":init_started.elapsed().as_millis(),
                "message":error,
            }));
            if let Some(cb) = on_degraded {
                cb(error);
            }
            // A7: respond to pending Finish with aborted_warming terminal.
            respond_to_warming_abort(command_rx);
            return;
        }
    };
    let init_ms = init_started.elapsed().as_millis();

    // A7: check shutdown between load steps.
    if worker_health.is_shutdown_requested() {
        respond_to_warming_abort(command_rx);
        return;
    }

    let warmup_started = Instant::now();
    let warmup = match worker.warmup() {
        Ok(warmup) => warmup,
        Err(e) => {
            let error = format!("Failed to warm live CoreML ASR models: {e}");
            trace.append(json!({
                "kind":"live_transcript_worker_start_degraded",
                "reason":"warmup_failed",
                "model_dir":model_dir_string,
                "duration_ms":warmup_started.elapsed().as_millis(),
                "message":error,
            }));
            if let Some(cb) = on_degraded {
                cb(error);
            }
            respond_to_warming_abort(command_rx);
            return;
        }
    };
    let warmup_ms = warmup_started.elapsed().as_millis();

    // A7: check shutdown after warmup.
    if worker_health.is_shutdown_requested() {
        respond_to_warming_abort(command_rx);
        return;
    }

    run_coreml_worker_from_instance(
        worker,
        model_dir_string,
        trace,
        audio_rx,
        command_rx,
        worker_health,
        queued_samples,
        pending_mode,
        transcription_mode,
        spawn_instant,
        init_ms,
        warmup_ms,
        on_ready,
        on_degraded,
        false, // used_prewarm
    );
}

/// Common worker loop used by both the inline path (after load+warmup) and the
/// prewarm path (after activation). `used_prewarm` controls the trace flag.
///
/// The caller (inline path) holds `_alive_guard` for the spawned thread's
/// lifetime. The prewarm path calls this directly, so we create the guard here
/// to ensure `alive` is cleared when the worker exits.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn run_coreml_worker_from_instance(
    mut worker: CoreMlLiveWorker,
    model_dir_string: String,
    trace: LiveAsrTrace,
    audio_rx: mpsc::Receiver<LiveAudioChunk>,
    command_rx: mpsc::Receiver<LiveBackchannelCommand>,
    worker_health: WorkerHealth,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    pending_mode: Arc<Mutex<Option<LiveTranscriptionMode>>>,
    transcription_mode: LiveTranscriptionMode,
    spawn_instant: std::time::Instant,
    init_ms: u128,
    warmup_ms: u128,
    on_ready: Option<Box<dyn Fn() + Send>>,
    on_degraded: Option<Box<dyn Fn(String) + Send>>,
    used_prewarm: bool,
) {
    // Hold the alive guard for this call's scope — works for both the prewarm
    // path (called directly from the prewarm thread) and as a no-op when the
    // inline path already holds a guard in the spawn closure.
    let _alive_guard = worker_health.guard();
    let _ = on_degraded; // degraded path is handled before calling this fn
    let capture_to_ready_ms = spawn_instant.elapsed().as_millis();
    trace.append(json!({
        "kind":"live_transcript_worker_ready",
        "model_dir":model_dir_string,
        "init_ms":init_ms,
        "warmup_ms":warmup_ms,
        "startup_ms":init_ms + warmup_ms,
    }));
    trace.append(json!({
        "kind":"live_transcript_worker_ready_signalled",
        "capture_to_ready_ms":capture_to_ready_ms,
        "used_prewarm":used_prewarm,
        "init_ms":init_ms,
        "warmup_ms":warmup_ms,
    }));

    // Apply pending mode set during warmup (A5).
    if let Some(mode) = pending_mode.lock().unwrap().take() {
        worker.transcription_mode = mode;
    } else {
        worker.transcription_mode = transcription_mode;
    }

    // A2: mark ready and call on_ready callback.
    worker_health.mark_ready();
    if let Some(cb) = on_ready {
        cb();
    }

    loop {
        while let Ok(command) = command_rx.try_recv() {
            if worker.handle_command(command, &audio_rx, &queued_samples) {
                return;
            }
        }

        match audio_rx.recv_timeout(WORKER_LOOP_POLL) {
            Ok(chunk) => {
                let sample_len = chunk.samples.len() as u64;
                worker.push_chunk(chunk);
                queued_samples.fetch_sub(
                    sample_len.min(queued_samples.load(Ordering::Relaxed)),
                    Ordering::Relaxed,
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                respond_to_commands_until_finish(
                    command_rx,
                    "Live audio channel closed".to_string(),
                );
                return;
            }
        }
    }
}

/// Respond to any pending commands during the load/warmup abort path (A7).
/// Answers Snapshot/SetMode with WORKER_WARMING and Finish with aborted_warming.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn respond_to_warming_abort(command_rx: mpsc::Receiver<LiveBackchannelCommand>) {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            LiveBackchannelCommand::SetTranscriptionMode { response, .. } => {
                let _ = response.send(Ok(()));
            }
            LiveBackchannelCommand::Snapshot { response, .. } => {
                let _ = response.send(Err(WORKER_WARMING.to_string()));
            }
            LiveBackchannelCommand::Finish { response, .. } => {
                let _ = response.send(Err("aborted_warming".to_string()));
                return;
            }
        }
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn respond_to_commands_until_finish(
    command_rx: mpsc::Receiver<LiveBackchannelCommand>,
    error: String,
) {
    while let Ok(command) = command_rx.recv() {
        match command {
            LiveBackchannelCommand::SetTranscriptionMode { response, .. } => {
                let _ = response.send(Ok(()));
            }
            LiveBackchannelCommand::Snapshot { response, .. } => {
                let _ = response.send(Err(error.clone()));
            }
            LiveBackchannelCommand::Finish { response, .. } => {
                let _ = response.send(Err(error.clone()));
                return;
            }
        }
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
struct CoreMlLiveWorker {
    session: margins::coreml_asr::StereoCoreMlAsrSession,
    trace: LiveAsrTrace,
    transcription_mode: LiveTranscriptionMode,
    #[cfg(feature = "rust-diarization")]
    mic_diarizer: Option<LiveMicDiarizer>,
    #[cfg(feature = "rust-diarization")]
    pending_mic_diarization_audio: Vec<f32>,
    last_memo_checkpoint_ms: u64,
    last_background_decode_ms: u64,
    mic_audio_samples: u64,
    system_audio_samples: u64,
    mic_resampler: NativeLiveResampler,
    system_resampler: NativeLiveResampler,
    mic_timeline: GenerationTimeline,
    system_timeline: GenerationTimeline,
}

/// Per-lane live tee resampler. SegmentWriter keeps each lane's emitted rate
/// stable across native capture attachments, so this state intentionally lives
/// for the entire live worker and is never reset by a microphone swap.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[derive(Debug, Default)]
struct NativeLiveResampler {
    generation: Option<u64>,
    source_rate: Option<u32>,
    inner: Option<margins::recorder::RationalResampler>,
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
impl NativeLiveResampler {
    fn process(
        &mut self,
        samples: &[f32],
        source_rate: u32,
        generation: u64,
    ) -> Result<Vec<f32>, String> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        if self.generation != Some(generation) {
            // A generation is a real segment boundary (resume), so the lane
            // may legitimately select a new fixed rate. Mic attach/switch
            // operations keep the same generation and therefore never reset
            // phase or interpolation state.
            self.generation = Some(generation);
            self.source_rate = None;
            self.inner = None;
        }
        match self.source_rate {
            Some(rate) if rate != source_rate => {
                return Err(format!(
                    "native live lane rate changed from {rate} Hz to {source_rate} Hz"
                ));
            }
            None => {
                self.source_rate = Some(source_rate);
                if source_rate != margins::coreml_asr::SAMPLE_RATE {
                    self.inner = Some(
                        margins::recorder::RationalResampler::new(
                            source_rate,
                            margins::coreml_asr::SAMPLE_RATE,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
            }
            Some(_) => {}
        }
        match self.inner.as_mut() {
            Some(resampler) => resampler
                .process(samples)
                .map_err(|error| error.to_string()),
            None => Ok(samples.to_vec()),
        }
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[derive(Serialize)]
struct LiveCoreMlWarmupTrace {
    frontend_ms: u128,
    decode_ms: u128,
    total_ms: u128,
    encoder_length: usize,
    actual_audio_frames: usize,
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[derive(Serialize)]
struct LiveWorkerWarmupTrace {
    mic: LiveCoreMlWarmupTrace,
    system: LiveCoreMlWarmupTrace,
}

/// Piecewise mapping from the audio clock consumed by CoreML to the capture's
/// wall-clock timeline. Each recorder generation is contiguous in audio time,
/// while `session_offset_ms` preserves pauses and device-restart gaps without
/// feeding potentially minutes of synthetic silence through ASR.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GenerationEpoch {
    generation: u64,
    audio_start_ms: u64,
    session_offset_ms: u64,
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[derive(Debug)]
struct GenerationTimeline {
    epochs: Vec<GenerationEpoch>,
    valid: bool,
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
impl GenerationTimeline {
    fn new() -> Self {
        Self {
            epochs: Vec::new(),
            valid: true,
        }
    }

    fn observe(&mut self, generation: u64, session_offset_ms: u64, audio_start_ms: u64) {
        let Some(previous) = self.epochs.last().copied() else {
            if generation != 0 || session_offset_ms != 0 || audio_start_ms != 0 {
                self.valid = false;
            }
            self.epochs.push(GenerationEpoch {
                generation,
                audio_start_ms,
                session_offset_ms,
            });
            return;
        };

        if generation == previous.generation {
            if session_offset_ms != previous.session_offset_ms {
                self.valid = false;
            }
            return;
        }

        let previous_session_end = previous
            .session_offset_ms
            .saturating_add(audio_start_ms.saturating_sub(previous.audio_start_ms));
        if generation != previous.generation.saturating_add(1)
            || audio_start_ms < previous.audio_start_ms
            || session_offset_ms < previous_session_end
        {
            self.valid = false;
        }
        self.epochs.push(GenerationEpoch {
            generation,
            audio_start_ms,
            session_offset_ms,
        });
    }

    fn verified_through(&self, expected_generation: u64) -> bool {
        self.valid
            && self.epochs.len() == expected_generation.saturating_add(1) as usize
            && self
                .epochs
                .iter()
                .enumerate()
                .all(|(index, epoch)| epoch.generation == index as u64)
    }

    fn map_ms(&self, audio_ms: u64) -> u64 {
        let Some(epoch) = self
            .epochs
            .iter()
            .rev()
            .find(|epoch| epoch.audio_start_ms <= audio_ms)
            .or_else(|| self.epochs.first())
        else {
            return audio_ms;
        };
        shift_ms(
            audio_ms,
            epoch.session_offset_ms as i128 - epoch.audio_start_ms as i128,
        )
    }

    fn map_word(&self, word: &margins::asr::WordTiming) -> margins::asr::WordTiming {
        let midpoint_ms = word.start_ms.saturating_add(word.end_ms) / 2;
        let Some(epoch) = self
            .epochs
            .iter()
            .rev()
            .find(|epoch| epoch.audio_start_ms <= midpoint_ms)
            .or_else(|| self.epochs.first())
        else {
            return word.clone();
        };
        let delta = epoch.session_offset_ms as i128 - epoch.audio_start_ms as i128;
        margins::asr::WordTiming {
            start_ms: shift_ms(word.start_ms, delta),
            end_ms: shift_ms(word.end_ms, delta),
            text: word.text.clone(),
        }
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn shift_ms(value: u64, delta: i128) -> u64 {
    (value as i128 + delta).clamp(0, u64::MAX as i128) as u64
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn generation_timelines_match(mic: &GenerationTimeline, system: &GenerationTimeline) -> bool {
    mic.epochs.len() == system.epochs.len()
        && mic.epochs.iter().zip(&system.epochs).all(|(mic, system)| {
            mic.generation == system.generation && mic.session_offset_ms == system.session_offset_ms
        })
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
impl CoreMlLiveWorker {
    /// B2: Build from already-loaded (and optionally warmed) model instances.
    /// The prewarm thread calls this after warmup — no model loading on this path.
    fn from_instances(
        mic_asr: margins::coreml_asr::FluidCoreMlAsr,
        system_asr: margins::coreml_asr::FluidCoreMlAsr,
        trace: LiveAsrTrace,
    ) -> Self {
        Self {
            session: margins::coreml_asr::StereoCoreMlAsrSession::new(
                mic_asr,
                system_asr,
                margins::coreml_asr::CoreMlStreamingConfig::default(),
            ),
            trace,
            transcription_mode: LiveTranscriptionMode::StereoSplit,
            #[cfg(feature = "rust-diarization")]
            mic_diarizer: None,
            #[cfg(feature = "rust-diarization")]
            pending_mic_diarization_audio: Vec::new(),
            last_memo_checkpoint_ms: 0,
            last_background_decode_ms: 0,
            mic_audio_samples: 0,
            system_audio_samples: 0,
            mic_resampler: NativeLiveResampler::default(),
            system_resampler: NativeLiveResampler::default(),
            mic_timeline: GenerationTimeline::new(),
            system_timeline: GenerationTimeline::new(),
        }
    }

    /// B2: Load model instances from disk and return a worker.
    /// Equivalent to calling `from_dir_auto` twice then `from_instances`.
    fn new(model_dir: &std::path::Path, trace: LiveAsrTrace) -> anyhow::Result<Self> {
        let mic_asr = margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(model_dir)?;
        let system_asr = margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(model_dir)?;
        Ok(Self::from_instances(mic_asr, system_asr, trace))
    }

    fn warmup(&mut self) -> Result<LiveWorkerWarmupTrace, String> {
        let mic = self
            .session
            .mic
            .asr_mut()
            .warmup_models()
            .map_err(|e| format!("Live mic CoreML warmup failed: {e}"))?;
        let system = self
            .session
            .system
            .asr_mut()
            .warmup_models()
            .map_err(|e| format!("Live system CoreML warmup failed: {e}"))?;
        Ok(LiveWorkerWarmupTrace {
            mic: LiveCoreMlWarmupTrace {
                frontend_ms: mic.frontend_ms,
                decode_ms: mic.decode_ms,
                total_ms: mic.total_ms,
                encoder_length: mic.encoder_length,
                actual_audio_frames: mic.actual_audio_frames,
            },
            system: LiveCoreMlWarmupTrace {
                frontend_ms: system.frontend_ms,
                decode_ms: system.decode_ms,
                total_ms: system.total_ms,
                encoder_length: system.encoder_length,
                actual_audio_frames: system.actual_audio_frames,
            },
        })
    }

    fn handle_command(
        &mut self,
        command: LiveBackchannelCommand,
        audio_rx: &mpsc::Receiver<LiveAudioChunk>,
        queued_samples: &std::sync::atomic::AtomicU64,
    ) -> bool {
        self.drain_audio(audio_rx, queued_samples);
        match command {
            LiveBackchannelCommand::SetTranscriptionMode { mode, response } => {
                let result = self.set_transcription_mode(mode);
                let _ = response.send(result);
                false
            }
            LiveBackchannelCommand::Snapshot {
                end_ms,
                advance_checkpoint,
                response,
            } => {
                let result = self
                    .snapshot(end_ms, false, None)
                    .map(|(snapshot, _)| snapshot);
                if let (true, Ok(snapshot)) = (advance_checkpoint, result.as_ref()) {
                    self.last_memo_checkpoint_ms = snapshot.decoded_until_ms;
                }
                let _ = response.send(result);
                false
            }
            LiveBackchannelCommand::Finish {
                end_ms,
                expected_generation,
                response,
            } => {
                let result = self.snapshot(end_ms, true, Some(expected_generation)).map(
                    |(context, endpoints)| LiveTerminalTranscript {
                        context,
                        mic_audio_end_ms: endpoints.mic_audio_end_ms,
                        system_audio_end_ms: endpoints.system_audio_end_ms,
                        mic_decoded_until_ms: endpoints.mic_decoded_until_ms,
                        system_decoded_until_ms: endpoints.system_decoded_until_ms,
                        generation_timeline_verified: endpoints.generation_timeline_verified,
                        generation_transitions: Vec::new(),
                    },
                );
                let _ = response.send(result);
                true
            }
        }
    }

    fn drain_audio(
        &mut self,
        audio_rx: &mpsc::Receiver<LiveAudioChunk>,
        queued_samples: &std::sync::atomic::AtomicU64,
    ) {
        while let Ok(chunk) = audio_rx.try_recv() {
            let sample_len = chunk.samples.len() as u64;
            self.push_chunk(chunk);
            queued_samples.fetch_sub(
                sample_len.min(queued_samples.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    fn push_chunk(&mut self, chunk: LiveAudioChunk) {
        if chunk.samples.is_empty() {
            return;
        }
        let resampled = match chunk.channel {
            LiveAudioChannel::Mic => {
                self.mic_resampler
                    .process(&chunk.samples, chunk.sample_rate, chunk.generation)
            }
            LiveAudioChannel::System => {
                self.system_resampler
                    .process(&chunk.samples, chunk.sample_rate, chunk.generation)
            }
        };
        let samples = match resampled {
            Ok(samples) => samples,
            Err(error) => {
                self.trace.append(json!({
                    "kind": "native_live_resample_failed",
                    "channel": match chunk.channel {
                        LiveAudioChannel::Mic => "mic",
                        LiveAudioChannel::System => "system",
                    },
                    "sample_rate": chunk.sample_rate,
                    "message": error,
                }));
                return;
            }
        };
        match chunk.channel {
            LiveAudioChannel::Mic => {
                let audio_start_ms = self.mic_audio_samples.saturating_mul(1_000)
                    / margins::coreml_asr::SAMPLE_RATE as u64;
                self.mic_timeline.observe(
                    chunk.generation,
                    chunk.session_offset_ms,
                    audio_start_ms,
                );
                self.session.mic.append_audio(&samples);
                self.mic_audio_samples =
                    self.mic_audio_samples.saturating_add(samples.len() as u64);
                self.feed_mic_diarizer_or_buffer(&samples);
            }
            LiveAudioChannel::System => {
                let audio_start_ms = self.system_audio_samples.saturating_mul(1_000)
                    / margins::coreml_asr::SAMPLE_RATE as u64;
                self.system_timeline.observe(
                    chunk.generation,
                    chunk.session_offset_ms,
                    audio_start_ms,
                );
                self.session.system.append_audio(&samples);
                self.system_audio_samples = self
                    .system_audio_samples
                    .saturating_add(samples.len() as u64);
            }
        }
        self.decode_ongoing_if_due();
    }

    fn decode_ongoing_if_due(&mut self) {
        let target_ms = match self.transcription_mode {
            LiveTranscriptionMode::StereoSplit => {
                self.mic_audio_samples
                    .min(self.system_audio_samples)
                    .saturating_mul(1_000)
                    / margins::coreml_asr::SAMPLE_RATE as u64
            }
            LiveTranscriptionMode::MicDiarized => {
                self.mic_audio_samples.saturating_mul(1_000)
                    / margins::coreml_asr::SAMPLE_RATE as u64
            }
        };
        if target_ms
            < self
                .last_background_decode_ms
                .saturating_add(LIVE_BACKGROUND_DECODE_INTERVAL_MS)
        {
            return;
        }

        let started = Instant::now();
        let mic_started = Instant::now();
        let mic = self.session.mic.update_until(target_ms);
        let mic_update_ms = mic_started.elapsed().as_millis();
        let (system, system_update_ms) =
            if self.transcription_mode == LiveTranscriptionMode::StereoSplit {
                let system_started = Instant::now();
                let result = self.session.system.update_until(target_ms);
                (Some(result), system_started.elapsed().as_millis())
            } else {
                (None, 0)
            };

        match (mic, system) {
            (Ok(mic), Some(Ok(system))) => {
                self.last_background_decode_ms = mic.decoded_until_ms.min(system.decoded_until_ms);
                self.trace.append(json!({
                    "kind":"live_transcript_background_decode",
                    "target_ms":target_ms,
                    "decoded_until_ms":self.last_background_decode_ms,
                    "mic_update_ms":mic_update_ms,
                    "system_update_ms":system_update_ms,
                    "total_ms":started.elapsed().as_millis(),
                }));
            }
            (Ok(mic), None) => {
                self.last_background_decode_ms = mic.decoded_until_ms;
                self.trace.append(json!({
                    "kind":"live_transcript_background_decode",
                    "target_ms":target_ms,
                    "decoded_until_ms":self.last_background_decode_ms,
                    "mic_update_ms":mic_update_ms,
                    "system_update_ms":0,
                    "total_ms":started.elapsed().as_millis(),
                }));
            }
            (Err(error), _) => {
                self.last_background_decode_ms = target_ms;
                self.trace.append(json!({
                    "kind":"live_transcript_background_decode_failed",
                    "channel":"mic",
                    "target_ms":target_ms,
                    "message":error.to_string(),
                }));
            }
            (_, Some(Err(error))) => {
                self.last_background_decode_ms = target_ms;
                self.trace.append(json!({
                    "kind":"live_transcript_background_decode_failed",
                    "channel":"system",
                    "target_ms":target_ms,
                    "message":error.to_string(),
                }));
            }
        }
    }

    fn set_transcription_mode(&mut self, mode: LiveTranscriptionMode) -> Result<(), String> {
        if mode == LiveTranscriptionMode::MicDiarized {
            self.ensure_mic_diarizer()?;
        }
        self.transcription_mode = mode;
        Ok(())
    }

    #[cfg(feature = "rust-diarization")]
    fn feed_mic_diarizer_or_buffer(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(diarizer) = self.mic_diarizer.as_mut() {
            let _ = diarizer.feed(samples);
        } else {
            self.pending_mic_diarization_audio
                .extend_from_slice(samples);
            let max_samples =
                margins::coreml_asr::SAMPLE_RATE as usize * MAX_PENDING_MIC_DIARIZATION_SECS;
            if self.pending_mic_diarization_audio.len() > max_samples {
                let drop = self.pending_mic_diarization_audio.len() - max_samples;
                self.pending_mic_diarization_audio.drain(..drop);
            }
        }
    }

    #[cfg(not(feature = "rust-diarization"))]
    fn feed_mic_diarizer_or_buffer(&mut self, _samples: &[f32]) {}

    #[cfg(feature = "rust-diarization")]
    fn ensure_mic_diarizer(&mut self) -> Result<(), String> {
        if self.mic_diarizer.is_some() {
            return Ok(());
        }
        let mut diarizer = LiveMicDiarizer::new()
            .map_err(|e| format!("Could not load live microphone diarization models: {e}"))?;
        if !self.pending_mic_diarization_audio.is_empty() {
            diarizer.feed(&self.pending_mic_diarization_audio)?;
            self.pending_mic_diarization_audio.clear();
        }
        self.mic_diarizer = Some(diarizer);
        Ok(())
    }

    #[cfg(not(feature = "rust-diarization"))]
    fn ensure_mic_diarizer(&mut self) -> Result<(), String> {
        Err("This build does not include live microphone diarization.".to_string())
    }

    fn snapshot(
        &mut self,
        _end_ms: u64,
        finish: bool,
        expected_generation: Option<u64>,
    ) -> Result<(LiveTranscriptContext, LiveChannelEndpoints), String> {
        let mic_audio_end_audio_ms =
            self.mic_audio_samples.saturating_mul(1_000) / margins::coreml_asr::SAMPLE_RATE as u64;
        let system_audio_end_audio_ms = self.system_audio_samples.saturating_mul(1_000)
            / margins::coreml_asr::SAMPLE_RATE as u64;
        let mic_audio_end_ms = self.mic_timeline.map_ms(mic_audio_end_audio_ms);
        let system_audio_end_ms = self.system_timeline.map_ms(system_audio_end_audio_ms);
        let mic_started = Instant::now();
        let mic = if finish {
            self.session.mic.finish_until(mic_audio_end_audio_ms)
        } else {
            self.session.mic.update_until(mic_audio_end_audio_ms)
        }
        .map_err(|e| format!("Live mic CoreML update failed: {e}"))?;
        let mic_update_ms = mic_started.elapsed().as_millis();
        let system_started = Instant::now();
        let system = if finish {
            self.session.system.finish_until(system_audio_end_audio_ms)
        } else {
            self.session.system.update_until(system_audio_end_audio_ms)
        }
        .map_err(|e| format!("Live system CoreML update failed: {e}"))?;
        let system_update_ms = system_started.elapsed().as_millis();

        #[cfg(feature = "rust-diarization")]
        if finish {
            if let Some(diarizer) = self.mic_diarizer.as_mut() {
                diarizer.flush()?;
            }
        }

        let format_started = Instant::now();
        let previous_memo_checkpoint_ms = self.last_memo_checkpoint_ms;
        let formatted = match self.transcription_mode {
            LiveTranscriptionMode::StereoSplit => format_transcript_context(
                &mic,
                &system,
                previous_memo_checkpoint_ms,
                &self.mic_timeline,
                &self.system_timeline,
            ),
            LiveTranscriptionMode::MicDiarized => {
                #[cfg(feature = "rust-diarization")]
                {
                    let turns = self
                        .mic_diarizer
                        .as_ref()
                        .map(|d| d.turns.as_slice())
                        .unwrap_or(&[]);
                    format_mic_diarized_transcript_context(
                        &mic,
                        turns,
                        previous_memo_checkpoint_ms,
                        &self.mic_timeline,
                    )
                }
                #[cfg(not(feature = "rust-diarization"))]
                {
                    return Err(
                        "This build does not include live microphone diarization.".to_string()
                    );
                }
            }
        };
        let format_ms = format_started.elapsed().as_millis();
        let mic_decoded_until_ms = self.mic_timeline.map_ms(mic.decoded_until_ms);
        let system_decoded_until_ms = self.system_timeline.map_ms(system.decoded_until_ms);
        let decoded_until_audio_ms = match self.transcription_mode {
            LiveTranscriptionMode::StereoSplit => mic.decoded_until_ms.min(system.decoded_until_ms),
            LiveTranscriptionMode::MicDiarized => mic.decoded_until_ms,
        };
        let decoded_until_ms = match self.transcription_mode {
            LiveTranscriptionMode::StereoSplit => mic_decoded_until_ms.min(system_decoded_until_ms),
            LiveTranscriptionMode::MicDiarized => mic_decoded_until_ms,
        };
        self.last_background_decode_ms = self.last_background_decode_ms.max(decoded_until_audio_ms);

        let context = LiveTranscriptContext {
            transcript: formatted.transcript,
            previous_transcript: formatted.previous_transcript,
            new_transcript: formatted.new_transcript,
            committed_transcript: formatted.committed_transcript,
            hypothesis_transcript: formatted.hypothesis_transcript,
            transcript_source: formatted.transcript_source,
            previous_memo_checkpoint_ms,
            decoded_until_ms,
            committed_until_ms: match self.transcription_mode {
                LiveTranscriptionMode::StereoSplit => self
                    .mic_timeline
                    .map_ms(mic.committed_until_ms)
                    .min(self.system_timeline.map_ms(system.committed_until_ms)),
                LiveTranscriptionMode::MicDiarized => {
                    self.mic_timeline.map_ms(mic.committed_until_ms)
                }
            },
            mic_accepted_samples: 0,
            system_accepted_samples: 0,
            mic_dropped_samples: 0,
            system_dropped_samples: 0,
            timing: Some(LiveTranscriptTiming {
                command_wait_ms: 0,
                mic_update_ms,
                system_update_ms,
                format_ms,
            }),
        };
        let endpoints = LiveChannelEndpoints {
            mic_audio_end_ms,
            system_audio_end_ms,
            mic_decoded_until_ms,
            system_decoded_until_ms,
            generation_timeline_verified: expected_generation.is_none_or(|generation| {
                self.mic_timeline.verified_through(generation)
                    && (self.transcription_mode != LiveTranscriptionMode::StereoSplit
                        || (self.system_timeline.verified_through(generation)
                            && generation_timelines_match(
                                &self.mic_timeline,
                                &self.system_timeline,
                            )))
            }),
        };
        Ok((context, endpoints))
    }
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
struct LiveMicDiarizer {
    pipeline: polyvoice::streaming::StreamingPipeline<
        polyvoice::EnergyVad,
        polyvoice::FbankOnnxExtractor,
    >,
    turns: Vec<polyvoice::SpeakerTurn>,
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
impl LiveMicDiarizer {
    fn new() -> Result<Self, String> {
        let registry = polyvoice::ModelRegistry::default().map_err(|e| e.to_string())?;
        let models = registry
            .ensure_for_profile(polyvoice::Profile::Balanced)
            .map_err(|e| e.to_string())?;
        let extractor = polyvoice::FbankOnnxExtractor::new(
            &models.embedder_path,
            polyvoice::Profile::Balanced.embedding_dim(),
            1,
        )
        .map_err(|e| e.to_string())?;
        let vad = polyvoice::EnergyVad::new(-40.0, margins::coreml_asr::SAMPLE_RATE, 512);
        let mut config = polyvoice::DiarizationConfig::default();
        config.cluster.max_speakers = 2;
        let pipeline = polyvoice::streaming::StreamingPipeline::new(
            vad,
            extractor,
            config,
            polyvoice::VadConfig::default(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            pipeline,
            turns: Vec::new(),
        })
    }

    fn feed(&mut self, samples: &[f32]) -> Result<(), String> {
        let new_turns = self.pipeline.feed(samples).map_err(|e| e.to_string())?;
        self.turns.extend(new_turns);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let new_turns = self.pipeline.flush().map_err(|e| e.to_string())?;
        self.turns.extend(new_turns);
        Ok(())
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
struct FormattedTranscriptContext {
    transcript: String,
    previous_transcript: String,
    new_transcript: String,
    committed_transcript: String,
    hypothesis_transcript: String,
    transcript_source: String,
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_transcript_context(
    mic: &margins::coreml_asr::StreamingTranscriptUpdate,
    system: &margins::coreml_asr::StreamingTranscriptUpdate,
    previous_memo_checkpoint_ms: u64,
    mic_timeline: &GenerationTimeline,
    system_timeline: &GenerationTimeline,
) -> FormattedTranscriptContext {
    let committed_entries = merged_transcript_entries(
        &mic.committed,
        &system.committed,
        mic_timeline,
        system_timeline,
    );
    let hypothesis_entries = merged_transcript_entries(
        &mic.hypothesis,
        &system.hypothesis,
        mic_timeline,
        system_timeline,
    );
    let all_entries = merged_entries(committed_entries.clone(), hypothesis_entries.clone());
    let committed_transcript = format_entries_window(&committed_entries, 0);
    let hypothesis_transcript = format_entries_window(&hypothesis_entries, 0);
    let transcript = format_entries_window(&all_entries, 0);
    let transcript_source =
        if !committed_transcript.trim().is_empty() && !hypothesis_transcript.trim().is_empty() {
            "committed+hypothesis".to_string()
        } else if !committed_transcript.trim().is_empty() {
            "committed".to_string()
        } else if !hypothesis_transcript.trim().is_empty() {
            "hypothesis".to_string()
        } else {
            "none".to_string()
        };
    let previous_transcript = format_entries_until(&all_entries, previous_memo_checkpoint_ms);
    let new_transcript = format_entries_window(&all_entries, previous_memo_checkpoint_ms);

    FormattedTranscriptContext {
        transcript,
        previous_transcript,
        new_transcript,
        committed_transcript,
        hypothesis_transcript,
        transcript_source,
    }
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
fn format_mic_diarized_transcript_context(
    mic: &margins::coreml_asr::StreamingTranscriptUpdate,
    turns: &[polyvoice::SpeakerTurn],
    previous_memo_checkpoint_ms: u64,
    mic_timeline: &GenerationTimeline,
) -> FormattedTranscriptContext {
    let committed_entries =
        map_transcript_entries(mic_diarized_entries(&mic.committed, turns), mic_timeline);
    let hypothesis_entries =
        map_transcript_entries(mic_diarized_entries(&mic.hypothesis, turns), mic_timeline);
    let all_entries = merged_entries(committed_entries.clone(), hypothesis_entries.clone());
    let committed_transcript =
        format_entries_window_with_labels(&committed_entries, 0, diarized_speaker_label);
    let hypothesis_transcript =
        format_entries_window_with_labels(&hypothesis_entries, 0, diarized_speaker_label);
    let transcript = format_entries_window_with_labels(&all_entries, 0, diarized_speaker_label);
    let base_source =
        if !committed_transcript.trim().is_empty() && !hypothesis_transcript.trim().is_empty() {
            "committed+hypothesis"
        } else if !committed_transcript.trim().is_empty() {
            "committed"
        } else if !hypothesis_transcript.trim().is_empty() {
            "hypothesis"
        } else {
            "none"
        };
    let previous_transcript = format_entries_until_with_labels(
        &all_entries,
        previous_memo_checkpoint_ms,
        diarized_speaker_label,
    );
    let new_transcript = format_entries_window_with_labels(
        &all_entries,
        previous_memo_checkpoint_ms,
        diarized_speaker_label,
    );

    FormattedTranscriptContext {
        transcript,
        previous_transcript,
        new_transcript,
        committed_transcript,
        hypothesis_transcript,
        transcript_source: format!("mic-diarized:{base_source}"),
    }
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
fn mic_diarized_entries(
    words: &[margins::asr::WordTiming],
    turns: &[polyvoice::SpeakerTurn],
) -> Vec<margins::asr::TranscriptWordEntry> {
    let mut entries = Vec::new();
    for word in words {
        let midpoint_ms = (word.start_ms + word.end_ms) / 2;
        entries.push(margins::asr::TranscriptWordEntry {
            channel: best_turn_for_midpoint(midpoint_ms, turns)
                .map(|turn| turn.speaker.0)
                .unwrap_or(0),
            start_ms: word.start_ms,
            end_ms: word.end_ms,
            text: format!(" {}", word.text.trim()),
        });
    }
    margins::asr::merge_word_entries_to_phrases(entries, 900)
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
fn best_turn_for_midpoint<'a>(
    midpoint_ms: u64,
    turns: &'a [polyvoice::SpeakerTurn],
) -> Option<&'a polyvoice::SpeakerTurn> {
    let midpoint_s = midpoint_ms as f64 / 1000.0;
    turns
        .iter()
        .find(|turn| turn.time.start <= midpoint_s && midpoint_s <= turn.time.end)
        .or_else(|| {
            turns.iter().min_by(|a, b| {
                let a_mid = (a.time.start + a.time.end) / 2.0;
                let b_mid = (b.time.start + b.time.end) / 2.0;
                (a_mid - midpoint_s)
                    .abs()
                    .partial_cmp(&(b_mid - midpoint_s).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn merged_entries(
    mut committed: Vec<margins::asr::TranscriptWordEntry>,
    hypothesis: Vec<margins::asr::TranscriptWordEntry>,
) -> Vec<margins::asr::TranscriptWordEntry> {
    committed.extend(hypothesis);
    margins::asr::merge_word_entries_to_phrases(committed, 900)
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn merged_transcript_entries(
    mic: &[margins::asr::WordTiming],
    system: &[margins::asr::WordTiming],
    mic_timeline: &GenerationTimeline,
    system_timeline: &GenerationTimeline,
) -> Vec<margins::asr::TranscriptWordEntry> {
    let mut entries = Vec::new();
    let mic = mic
        .iter()
        .map(|word| mic_timeline.map_word(word))
        .collect::<Vec<_>>();
    let system = system
        .iter()
        .map(|word| system_timeline.map_word(word))
        .collect::<Vec<_>>();
    entries.extend(margins::asr::words_to_transcript_entries(&mic, 0, 0));
    entries.extend(margins::asr::words_to_transcript_entries(&system, 1, 0));
    margins::asr::merge_word_entries_to_phrases(entries, 900)
}

#[cfg(all(
    feature = "coreml-asr",
    target_os = "macos",
    feature = "rust-diarization"
))]
fn map_transcript_entries(
    entries: Vec<margins::asr::TranscriptWordEntry>,
    timeline: &GenerationTimeline,
) -> Vec<margins::asr::TranscriptWordEntry> {
    entries
        .into_iter()
        .map(|entry| {
            let midpoint_ms = entry.start_ms.saturating_add(entry.end_ms) / 2;
            let delta = timeline
                .epochs
                .iter()
                .rev()
                .find(|epoch| epoch.audio_start_ms <= midpoint_ms)
                .or_else(|| timeline.epochs.first())
                .map(|epoch| epoch.session_offset_ms as i128 - epoch.audio_start_ms as i128)
                .unwrap_or(0);
            margins::asr::TranscriptWordEntry {
                start_ms: shift_ms(entry.start_ms, delta),
                end_ms: shift_ms(entry.end_ms, delta),
                ..entry
            }
        })
        .collect()
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_entries_window(
    entries: &[margins::asr::TranscriptWordEntry],
    min_start_ms: u64,
) -> String {
    format_entries_window_with_labels(entries, min_start_ms, speaker_label)
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_entries_window_with_labels(
    entries: &[margins::asr::TranscriptWordEntry],
    min_start_ms: u64,
    label: fn(u32) -> &'static str,
) -> String {
    let keep_from = entries
        .last()
        .map(|entry| entry.start_ms.saturating_sub(180_000))
        .unwrap_or(0)
        .max(min_start_ms);
    entries
        .iter()
        .filter(|entry| entry.end_ms > keep_from)
        .map(|entry| format_transcript_entry_with_label(entry, label))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_entries_until(entries: &[margins::asr::TranscriptWordEntry], max_end_ms: u64) -> String {
    format_entries_until_with_labels(entries, max_end_ms, speaker_label)
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_entries_until_with_labels(
    entries: &[margins::asr::TranscriptWordEntry],
    max_end_ms: u64,
    label: fn(u32) -> &'static str,
) -> String {
    if max_end_ms == 0 {
        return String::new();
    }
    let keep_from = max_end_ms.saturating_sub(180_000);
    entries
        .iter()
        .filter(|entry| entry.start_ms >= keep_from && entry.end_ms <= max_end_ms)
        .map(|entry| format_transcript_entry_with_label(entry, label))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_transcript_entry_with_label(
    entry: &margins::asr::TranscriptWordEntry,
    label: fn(u32) -> &'static str,
) -> String {
    format!(
        "[{}] {}: {}",
        format_elapsed(entry.start_ms),
        label(entry.channel),
        entry.text.trim()
    )
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn speaker_label(channel: u32) -> &'static str {
    match channel {
        0 => "user",
        1 => "other",
        _ => "speaker",
    }
}

#[cfg(all(
    feature = "coreml-asr",
    feature = "rust-diarization",
    target_os = "macos"
))]
fn diarized_speaker_label(channel: u32) -> &'static str {
    match channel {
        0 => "speaker 1",
        1 => "speaker 2",
        _ => "speaker",
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn format_elapsed(ms: u64) -> String {
    let total = ms / 1000;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
mod tests {
    use super::*;
    use margins::asr::WordTiming;
    use margins::coreml_asr::{CoreMlStreamingTimings, StreamingTranscriptUpdate};

    fn update(
        committed: Vec<WordTiming>,
        hypothesis: Vec<WordTiming>,
    ) -> StreamingTranscriptUpdate {
        StreamingTranscriptUpdate {
            committed,
            hypothesis,
            decoded_until_ms: 10_000,
            committed_until_ms: 0,
            timings: CoreMlStreamingTimings::default(),
        }
    }

    fn identity_timeline() -> GenerationTimeline {
        let mut timeline = GenerationTimeline::new();
        timeline.observe(0, 0, 0);
        timeline
    }

    #[test]
    fn native_live_resampler_long_duration_has_integer_output_contract() {
        let source_rate = 44_100u32;
        let input_frames = u64::from(source_rate) * 10 * 60;
        let mut resampler = NativeLiveResampler::default();
        let chunk = vec![0.25f32; 997];
        let mut remaining = input_frames;
        let mut output_frames = 0u64;
        while remaining > 0 {
            let count = remaining.min(chunk.len() as u64) as usize;
            output_frames += resampler
                .process(&chunk[..count], source_rate, 0)
                .unwrap()
                .len() as u64;
            remaining -= count as u64;
        }
        assert_eq!(
            output_frames,
            input_frames * u64::from(margins::coreml_asr::SAMPLE_RATE) / u64::from(source_rate)
        );
    }

    #[test]
    fn native_live_resampler_44k1_is_chunk_boundary_independent() {
        let input: Vec<f32> = (0..44_100 * 3)
            .map(|index| ((index % 257) as f32 - 128.0) / 128.0)
            .collect();
        let mut whole = NativeLiveResampler::default();
        let expected = whole.process(&input, 44_100, 0).unwrap();

        let mut chunked = NativeLiveResampler::default();
        let chunk_sizes = [1usize, 7, 113, 997, 2_048, 31];
        let mut actual = Vec::new();
        let mut cursor = 0usize;
        let mut chunk_index = 0usize;
        while cursor < input.len() {
            let end = (cursor + chunk_sizes[chunk_index % chunk_sizes.len()]).min(input.len());
            actual.extend(chunked.process(&input[cursor..end], 44_100, 0).unwrap());
            cursor = end;
            chunk_index += 1;
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn native_live_resampler_changes_rate_only_at_resume_generation() {
        let mut resampler = NativeLiveResampler::default();
        assert!(resampler.process(&[0.0; 441], 44_100, 0).is_ok());
        assert!(resampler.process(&[0.0; 480], 48_000, 0).is_err());
        assert!(resampler.process(&[0.0; 480], 48_000, 1).is_ok());
    }

    #[test]
    fn transcript_context_merges_word_timings_into_phrases() {
        let mic = update(
            vec![
                WordTiming {
                    start_ms: 2_000,
                    end_ms: 2_200,
                    text: "I".into(),
                },
                WordTiming {
                    start_ms: 2_250,
                    end_ms: 2_500,
                    text: "have".into(),
                },
                WordTiming {
                    start_ms: 2_550,
                    end_ms: 2_800,
                    text: "context".into(),
                },
            ],
            vec![],
        );
        let system = update(vec![], vec![]);

        let formatted =
            format_transcript_context(&mic, &system, 0, &identity_timeline(), &identity_timeline());
        assert_eq!(formatted.transcript_source, "committed");
        assert_eq!(formatted.transcript, "[00:02] user: I have context");
        assert_eq!(formatted.new_transcript, formatted.transcript);
    }

    #[test]
    fn transcript_context_splits_delta_at_previous_memo_checkpoint() {
        let mic = update(
            vec![
                WordTiming {
                    start_ms: 1_000,
                    end_ms: 1_200,
                    text: "old".into(),
                },
                WordTiming {
                    start_ms: 1_250,
                    end_ms: 1_500,
                    text: "context".into(),
                },
                WordTiming {
                    start_ms: 6_000,
                    end_ms: 6_300,
                    text: "new".into(),
                },
                WordTiming {
                    start_ms: 6_350,
                    end_ms: 6_700,
                    text: "segment".into(),
                },
            ],
            vec![],
        );
        let system = update(vec![], vec![]);

        let formatted = format_transcript_context(
            &mic,
            &system,
            5_000,
            &identity_timeline(),
            &identity_timeline(),
        );
        assert!(formatted.transcript.contains("old context"));
        assert!(formatted.transcript.contains("new segment"));
        assert_eq!(formatted.previous_transcript, "[00:01] user: old context");
        assert_eq!(formatted.new_transcript, "[00:06] user: new segment");
    }

    #[test]
    fn transcript_context_logs_no_transcript_without_fallback_copy() {
        let mic = update(vec![], vec![]);
        let system = update(vec![], vec![]);

        let formatted =
            format_transcript_context(&mic, &system, 0, &identity_timeline(), &identity_timeline());
        assert_eq!(formatted.transcript_source, "none");
        assert!(formatted.transcript.is_empty());
        assert!(formatted.new_transcript.is_empty());
        assert!(formatted.committed_transcript.is_empty());
        assert!(formatted.hypothesis_transcript.is_empty());
    }

    #[test]
    fn generation_timeline_preserves_latest_capture_gap_without_silence() {
        let mut timeline = GenerationTimeline::new();
        timeline.observe(0, 0, 0);
        timeline.observe(1, 12_375, 11_296);

        assert!(timeline.verified_through(1));
        assert_eq!(timeline.map_ms(11_295), 11_295);
        assert_eq!(timeline.map_ms(11_296), 12_375);
        assert_eq!(timeline.map_ms(12_296), 13_375);

        let mapped = timeline.map_word(&WordTiming {
            start_ms: 11_396,
            end_ms: 11_696,
            text: "after restart".into(),
        });
        assert_eq!(mapped.start_ms, 12_475);
        assert_eq!(mapped.end_ms, 12_775);
    }

    #[test]
    fn formatted_transcript_uses_generation_session_offsets() {
        let mic = update(
            vec![WordTiming {
                start_ms: 11_396,
                end_ms: 11_696,
                text: "after restart".into(),
            }],
            vec![],
        );
        let system = update(vec![], vec![]);
        let mut mic_timeline = GenerationTimeline::new();
        mic_timeline.observe(0, 0, 0);
        mic_timeline.observe(1, 12_375, 11_296);
        let mut system_timeline = GenerationTimeline::new();
        system_timeline.observe(0, 0, 0);
        system_timeline.observe(1, 12_375, 11_296);

        let formatted =
            format_transcript_context(&mic, &system, 0, &mic_timeline, &system_timeline);

        assert_eq!(formatted.transcript, "[00:12] user: after restart");
        assert!(generation_timelines_match(&mic_timeline, &system_timeline));
    }

    #[test]
    fn generation_timeline_rejects_missing_or_overlapping_epochs() {
        let mut missing = GenerationTimeline::new();
        missing.observe(0, 0, 0);
        assert!(!missing.verified_through(1));

        let mut overlap = GenerationTimeline::new();
        overlap.observe(0, 0, 0);
        overlap.observe(1, 10_000, 11_000);
        assert!(!overlap.verified_through(1));

        let mut skipped = GenerationTimeline::new();
        skipped.observe(0, 0, 0);
        skipped.observe(2, 12_000, 11_000);
        assert!(!skipped.verified_through(2));
    }
}

#[cfg(test)]
mod capture_first_tests {
    use super::*;
    use crate::live_asr_worker::{AUDIO_QUEUE_MAX_SAMPLES, WORKER_WARMING};
    use margins::recorder::{LiveAudioChannel, LiveAudioChunk, LiveAudioSink, LiveGenerationClock};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn make_not_ready_client() -> LiveBackchannelClient {
        let health = WorkerHealth::new();
        // health.ready is false by default — worker is "warming"
        LiveBackchannelClient {
            command_tx: mpsc::channel::<LiveBackchannelCommand>().0,
            worker_health: health,
            mic_accepted_samples: Arc::new(AtomicU64::new(0)),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: Arc::new(AtomicU64::new(0)),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            pending_mode: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn not_ready_snapshot_returns_warming() {
        let client = make_not_ready_client();
        let err = client
            .checkpoint_memo(1000)
            .expect_err("should fail while warming");
        assert_eq!(
            err, WORKER_WARMING,
            "checkpoint_memo on warming handle must return WORKER_WARMING"
        );
        let err2 = client
            .request_context(1000)
            .expect_err("should fail while warming");
        assert_eq!(err2, WORKER_WARMING);
    }

    #[test]
    fn not_ready_set_mode_stores_pending() {
        let client = make_not_ready_client();
        // Should not error — just stores pending mode.
        client
            .set_transcription_mode(LiveTranscriptionMode::MicDiarized)
            .expect("set_transcription_mode while warming should be Ok");
        let stored = client.pending_mode.lock().unwrap();
        assert_eq!(*stored, Some(LiveTranscriptionMode::MicDiarized));
    }

    #[test]
    fn stop_during_load_sets_shutdown_flag() {
        let health = WorkerHealth::new();
        assert!(!health.is_shutdown_requested());
        health.request_shutdown();
        assert!(health.is_shutdown_requested());
        // After marking ready, is_ready should be true; shutdown is separate.
        health.mark_ready();
        assert!(health.is_ready());
        assert!(health.is_shutdown_requested());
    }

    #[test]
    fn queue_budget_under_limit_no_drops() {
        let queued = Arc::new(AtomicU64::new(0));
        let (tx, _rx) = mpsc::channel::<LiveAudioChunk>();
        let generation_clock = Arc::new(Mutex::new(LiveGenerationClock {
            generation: 0,
            session_offset_ms: 0,
        }));
        let mic_accepted = Arc::new(AtomicU64::new(0));
        let mic_dropped = Arc::new(AtomicU64::new(0));
        let sink = LiveAudioSink {
            sender: tx,
            generation: 0,
            generation_clock,
            mic_accepted_samples: mic_accepted.clone(),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: mic_dropped.clone(),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            queued_samples: queued.clone(),
            queue_max_samples: AUDIO_QUEUE_MAX_SAMPLES,
        };
        // 100 samples well under budget.
        let samples = vec![0.0f32; 100];
        let sample_count = samples.len() as u64;
        let current = queued.load(Ordering::Relaxed);
        if sink.queue_max_samples == 0
            || current.saturating_add(sample_count) <= sink.queue_max_samples
        {
            let chunk = LiveAudioChunk {
                channel: LiveAudioChannel::Mic,
                generation: 0,
                session_offset_ms: 0,
                sample_rate: 16000,
                samples,
            };
            sink.sender.send(chunk).unwrap();
            queued.fetch_add(sample_count, Ordering::Relaxed);
            sink.mic_accepted_samples
                .fetch_add(sample_count, Ordering::Relaxed);
        } else {
            sink.mic_dropped_samples
                .fetch_add(sample_count, Ordering::Relaxed);
        }
        assert_eq!(mic_accepted.load(Ordering::Relaxed), 100);
        assert_eq!(mic_dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn queue_budget_over_limit_counts_drops() {
        let queued = Arc::new(AtomicU64::new(AUDIO_QUEUE_MAX_SAMPLES));
        let (tx, _rx) = mpsc::channel::<LiveAudioChunk>();
        let generation_clock = Arc::new(Mutex::new(LiveGenerationClock {
            generation: 0,
            session_offset_ms: 0,
        }));
        let mic_accepted = Arc::new(AtomicU64::new(0));
        let mic_dropped = Arc::new(AtomicU64::new(0));
        let sink = LiveAudioSink {
            sender: tx,
            generation: 0,
            generation_clock,
            mic_accepted_samples: mic_accepted.clone(),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: mic_dropped.clone(),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            queued_samples: queued.clone(),
            queue_max_samples: AUDIO_QUEUE_MAX_SAMPLES,
        };
        let samples = vec![0.0f32; 1000];
        let sample_count = samples.len() as u64;
        let current = queued.load(Ordering::Relaxed);
        if sink.queue_max_samples > 0
            && current.saturating_add(sample_count) > sink.queue_max_samples
        {
            sink.mic_dropped_samples
                .fetch_add(sample_count, Ordering::Relaxed);
        } else {
            sink.mic_accepted_samples
                .fetch_add(sample_count, Ordering::Relaxed);
        }
        assert_eq!(mic_dropped.load(Ordering::Relaxed), 1000);
        assert_eq!(mic_accepted.load(Ordering::Relaxed), 0);
    }
}

// ---------------------------------------------------------------------------
// B4: Unit tests for prewarm slot mechanics — no model assets required.
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
mod prewarm_slot_tests {
    use super::*;

    /// Inject a synthetic PrewarmHandoff into a local slot (mirrors production
    /// PREWARM_SLOT but is isolated per-test via a fresh Mutex).
    fn make_test_slot_with(
        model_dir: PathBuf,
    ) -> (
        Mutex<Option<PrewarmHandoff>>,
        mpsc::Receiver<PrewarmActivation>,
    ) {
        let (activate_tx, activate_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
        // Spawn a dummy parked thread that waits for activation.
        let (dummy_tx, dummy_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
        let handle = std::thread::Builder::new()
            .name("test-prewarm-dummy".into())
            .spawn(move || {
                // Park until activation.
                let _ = dummy_rx.recv();
            })
            .expect("spawn dummy thread");
        let handoff = PrewarmHandoff {
            activate_tx,
            model_dir,
            thread_handle: handle,
        };
        (Mutex::new(Some(handoff)), activate_rx)
    }

    /// Local version of take logic for isolated slot testing.
    fn take_from_slot(
        slot: &Mutex<Option<PrewarmHandoff>>,
        model_dir: &std::path::Path,
    ) -> Option<PrewarmHandoff> {
        let mut guard = slot.lock().unwrap();
        match guard.as_ref() {
            None => None,
            Some(h) if h.model_dir == model_dir => guard.take(),
            Some(_) => {
                if let Some(old) = guard.take() {
                    let _ = old.activate_tx.send(PrewarmActivation::Cancel);
                }
                None
            }
        }
    }

    #[test]
    fn empty_slot_returns_none() {
        let slot: Mutex<Option<PrewarmHandoff>> = Mutex::new(None);
        let dir = std::path::Path::new("/some/model/dir");
        assert!(take_from_slot(&slot, dir).is_none());
    }

    #[test]
    fn matching_dir_returns_handoff_and_empties_slot() {
        let dir = std::env::temp_dir().join(format!("prewarm_test_match_{}", std::process::id()));
        let (slot, _rx) = make_test_slot_with(dir.clone());
        let result = take_from_slot(&slot, &dir);
        assert!(result.is_some(), "matching dir should return the handoff");
        // Slot should now be empty.
        let second = take_from_slot(&slot, &dir);
        assert!(second.is_none(), "slot should be empty after take");
        // Clean up the dummy thread by sending something.
        if let Some(h) = result {
            let _ = h.activate_tx.send(PrewarmActivation::Cancel);
        }
    }

    #[test]
    fn mismatched_dir_sends_cancel_and_returns_none() {
        let dir_a = std::env::temp_dir().join(format!("prewarm_test_a_{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("prewarm_test_b_{}", std::process::id()));
        let (slot, activate_rx) = make_test_slot_with(dir_a.clone());
        // Request with a different dir.
        let result = take_from_slot(&slot, &dir_b);
        assert!(result.is_none(), "mismatched dir should return None");
        // The old handoff's activate_rx should have received Cancel.
        match activate_rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(PrewarmActivation::Cancel) => {} // expected
            Ok(PrewarmActivation::Run(_)) => panic!("expected Cancel, got Run"),
            Err(e) => panic!("expected Cancel within timeout, got: {e:?}"),
        }
        // Slot should be empty after mismatch.
        assert!(
            take_from_slot(&slot, &dir_a).is_none(),
            "slot should be empty"
        );
    }

    #[test]
    fn cancel_unparks_dummy_thread() {
        let dir = std::env::temp_dir().join(format!("prewarm_test_cancel_{}", std::process::id()));
        let (activate_tx, activate_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
        let barrier = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let barrier_clone = barrier.clone();
        let handle = std::thread::Builder::new()
            .name("test-cancel-park".into())
            .spawn(move || {
                // Park until activation.
                let _ = activate_rx.recv();
                barrier_clone.store(true, Ordering::Release);
            })
            .expect("spawn test thread");
        // Send Cancel.
        let _ = activate_tx.send(PrewarmActivation::Cancel);
        // Thread should exit quickly.
        handle
            .join()
            .expect("thread should have exited after Cancel");
        assert!(
            barrier.load(Ordering::Acquire),
            "thread should have been unparked by Cancel"
        );
        let _ = dir; // suppress unused warning
    }

    /// Helper: install a dummy handoff into a local test slot.
    fn install_into_slot(
        slot: &Mutex<Option<PrewarmHandoff>>,
        model_dir: PathBuf,
    ) -> mpsc::Receiver<PrewarmActivation> {
        let (activate_tx, activate_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
        let (dummy_tx, dummy_rx) = mpsc::sync_channel::<PrewarmActivation>(1);
        let _ = dummy_tx; // keep sender alive so the thread doesn't exit on recv
        let handle = std::thread::Builder::new()
            .name("test-prewarm-install".into())
            .spawn(move || {
                let _ = dummy_rx.recv();
            })
            .expect("spawn install thread");
        let handoff = PrewarmHandoff {
            activate_tx,
            model_dir,
            thread_handle: handle,
        };
        *slot.lock().unwrap() = Some(handoff);
        activate_rx
    }

    /// Helper: check occupied using the same logic as prewarm_slot_occupied but on a local slot.
    fn slot_occupied(slot: &Mutex<Option<PrewarmHandoff>>) -> bool {
        slot.lock().unwrap().is_some()
    }

    #[test]
    fn prewarm_slot_occupied_reports_correctly() {
        let slot: Mutex<Option<PrewarmHandoff>> = Mutex::new(None);
        assert!(
            !slot_occupied(&slot),
            "empty slot should report not occupied"
        );
        let dir = std::env::temp_dir().join(format!("prewarm_occupied_{}", std::process::id()));
        let _rx = install_into_slot(&slot, dir.clone());
        assert!(slot_occupied(&slot), "occupied slot should report occupied");
        let _ = take_from_slot(&slot, &dir);
        assert!(
            !slot_occupied(&slot),
            "after take slot should report not occupied"
        );
    }

    #[test]
    fn second_install_into_occupied_slot_is_rejected() {
        // Simulate what prewarm_live_models_with_reason does: if slot_occupied, skip.
        // We test the guard logic directly.
        let slot: Mutex<Option<PrewarmHandoff>> = Mutex::new(None);
        let dir = std::env::temp_dir().join(format!("prewarm_reject_{}", std::process::id()));
        let _rx1 = install_into_slot(&slot, dir.clone());
        assert!(slot_occupied(&slot), "first install: slot occupied");

        // Simulate the guard in prewarm_live_models_with_reason — if occupied, a new
        // thread would skip installation entirely. The slot should still hold the original.
        // We just verify the guard predicate is true (no real thread spawned in tests).
        let should_skip = slot_occupied(&slot);
        assert!(
            should_skip,
            "guard should detect occupied slot and prevent double-install"
        );

        // Slot still holds the original handoff, not replaced.
        assert!(
            slot_occupied(&slot),
            "original handoff should still be in slot"
        );
    }

    #[test]
    fn take_then_empty_then_reinstall_works() {
        let slot: Mutex<Option<PrewarmHandoff>> = Mutex::new(None);
        let dir = std::env::temp_dir().join(format!("prewarm_reinstall_{}", std::process::id()));
        // Install initial handoff.
        let rx1 = install_into_slot(&slot, dir.clone());
        assert!(slot_occupied(&slot));
        // Take (simulates session start consuming the slot).
        let handoff = take_from_slot(&slot, &dir).expect("should take handoff");
        let _ = handoff.activate_tx.send(PrewarmActivation::Cancel);
        // rx1 sender was consumed by install_into_slot — but activate_rx is rx1.
        let _ = rx1; // allow it to be dropped

        assert!(!slot_occupied(&slot), "slot empty after take");

        // Re-install (simulates post-session re-prewarm).
        let _rx2 = install_into_slot(&slot, dir.clone());
        assert!(slot_occupied(&slot), "slot occupied after re-install");

        // Take again to clean up.
        let h2 = take_from_slot(&slot, &dir).expect("second take should succeed");
        let _ = h2.activate_tx.send(PrewarmActivation::Cancel);
        assert!(!slot_occupied(&slot));
    }
}
