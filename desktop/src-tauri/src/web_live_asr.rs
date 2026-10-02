//! Headless live-ASR worker used by the HTTP/agent-browser harness.
//!
//! Browser WebM chunks remain the durable recording format. The latency harness
//! additionally injects channel-ordered f32 PCM here so Linux exercises the same
//! ongoing-decode -> memo checkpoint -> backchannel shape as the native app.
//! Accepted chunks form a contiguous channel-local timeline; any rejected
//! chunk increments that channel's drop counter and disqualifies reuse.

#[cfg(any(feature = "parakeet-asr", test))]
use crate::live_asr_worker::{LiveAsrTrace, STARTUP_TIMEOUT, WORKER_LOOP_POLL};
use crate::live_asr_worker::{WorkerHealth, REQUEST_TIMEOUT, WORKER_STOPPED, WORKER_WARMING};
use crate::live_backchannel::{LiveTranscriptContext, LiveTranscriptTiming};
use margins::asr::WordTiming;
use margins::recorder::LiveAudioChannel;
#[cfg(any(feature = "parakeet-asr", test))]
use margins_core::{AsrChunkDecoder, AsrRequest};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(test)]
macro_rules! test_chunk_decoder {
    ($decoder:ty) => {
        impl margins_core::AsrChunkDecoder for $decoder {
            fn decode_chunk(
                &mut self,
                request: margins_core::AsrRequest,
            ) -> Result<margins_core::AsrResult, margins_core::TranscriptError> {
                self.transcribe_words(&request.samples)
                    .map(|words| margins_core::AsrResult {
                        words: words
                            .into_iter()
                            .map(|word| margins_core::TranscriptWord {
                                start_ms: word.start_ms.saturating_add(request.session_offset_ms),
                                end_ms: word.end_ms.saturating_add(request.session_offset_ms),
                                text: word.text,
                                speaker: None,
                                confidence_per_mille: None,
                            })
                            .collect(),
                        detected_language: None,
                    })
                    .map_err(|error| margins_core::TranscriptError {
                        code: margins_core::TranscriptErrorCode::InferenceFailed,
                        message: error.to_string(),
                        retryable: false,
                    })
            }
        }
    };
}

const SAMPLE_RATE: u32 = 16_000;
// Hosted ONNX model construction can take several minutes on an I/O-contended
// server. Keep capture-first startup lossless through that cold-load window
// without applying HTTP backpressure to the browser's durable recorder. Five
// minutes of normalized mono f32 PCM is about 18.3 MiB total; the budget is shared
// across channels and is released continuously once the worker starts draining.
#[cfg(any(feature = "parakeet-asr", test))]
const WEB_AUDIO_QUEUE_MAX_SAMPLES: u64 = SAMPLE_RATE as u64 * 60 * 5;
// Parakeet is an offline ONNX graph rather than a stateful streaming decoder.
// Five-second advances keep it ahead of real-time without repeatedly paying
// frontend/encoder cost for every one-second injected transport chunk.
const BACKGROUND_DECODE_INTERVAL_MS: u64 = 5_000;
const IDLE_FLUSH_DELAY_MS: u64 = 1_500;
const IDLE_FLUSH_MIN_RESIDUE_MS: u64 = 1_000;
const IDLE_FLUSH_MIN_INTERVAL_MS: u64 = 5_000;
const DECODE_RETRY_BASE_MS: u64 = 1_000;
const DECODE_RETRY_MAX_MS: u64 = 8_000;
const DECODE_FAILURE_LIMIT: u32 = 4;
const ROLLING_WINDOW_MS: u64 = 15_000;
const ROLLING_OVERLAP_MS: u64 = 1_000;

#[derive(Clone)]
pub(crate) struct WebLiveAsrClient {
    audio_tx: mpsc::Sender<InjectedAudio>,
    command_tx: mpsc::Sender<Command>,
    worker_health: WorkerHealth,
    cancelled: Arc<AtomicBool>,
    dropped_samples: Arc<ChannelDropCounters>,
    /// Tracks total normalized samples queued to the unbounded audio channel.
    /// Decremented by the worker on drain. Injections over budget are dropped.
    #[cfg(any(feature = "parakeet-asr", test))]
    queued_samples: Arc<AtomicU64>,
    request_timeout: Duration,
}

pub(crate) struct WebLiveAsrHandle {
    client: WebLiveAsrClient,
    thread: Option<std::thread::JoinHandle<()>>,
    reaper_tx: mpsc::Sender<std::thread::JoinHandle<()>>,
}

#[derive(Debug)]
pub(crate) struct WebLiveAsrTerminalSnapshot {
    pub(crate) context: LiveTranscriptContext,
    pub(crate) mic_decoded_samples: u64,
    pub(crate) system_decoded_samples: u64,
}

#[derive(Debug)]
struct InjectedAudio {
    channel: LiveAudioChannel,
    sample_rate: u32,
    samples: Vec<f32>,
}

#[derive(Default)]
struct ChannelDropCounters {
    mic: AtomicU64,
    system: AtomicU64,
}

impl ChannelDropCounters {
    fn add(&self, channel: LiveAudioChannel, samples: u64) {
        match channel {
            LiveAudioChannel::Mic => self.mic.fetch_add(samples, Ordering::Relaxed),
            LiveAudioChannel::System => self.system.fetch_add(samples, Ordering::Relaxed),
        };
    }

    fn get(&self, channel: LiveAudioChannel) -> u64 {
        match channel {
            LiveAudioChannel::Mic => self.mic.load(Ordering::Relaxed),
            LiveAudioChannel::System => self.system.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct ChannelState {
    audio: Vec<f32>,
    words: Vec<WordTiming>,
    decoded_until_samples: usize,
    checkpoint_samples: usize,
}

impl ChannelState {
    fn available_ms(&self) -> u64 {
        audio_duration_ms(&self.audio)
    }

    fn residue_ms(&self) -> u64 {
        samples_to_ms_floor(self.audio.len().saturating_sub(self.decoded_until_samples))
    }

    fn decoded_until_ms(&self) -> u64 {
        samples_to_ms_floor(self.decoded_until_samples)
    }

    fn checkpoint_ms(&self) -> u64 {
        samples_to_ms_floor(self.checkpoint_samples)
    }

    fn is_present(&self) -> bool {
        !self.audio.is_empty()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DecodeRetryState {
    consecutive_failures: u32,
    retry_not_before_ms: u64,
    unhealthy: bool,
}

impl DecodeRetryState {
    fn can_attempt(self, now_ms: u64) -> bool {
        !self.unhealthy && now_ms >= self.retry_not_before_ms
    }

    fn record_success(&mut self) {
        *self = Self::default();
    }

    fn record_failure(&mut self, now_ms: u64) -> RetryFailure {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= DECODE_FAILURE_LIMIT {
            self.unhealthy = true;
            return RetryFailure::Unhealthy;
        }
        let exponent = self.consecutive_failures.saturating_sub(1).min(31);
        let delay_ms = DECODE_RETRY_BASE_MS
            .saturating_mul(1_u64 << exponent)
            .min(DECODE_RETRY_MAX_MS);
        self.retry_not_before_ms = now_ms.saturating_add(delay_ms);
        RetryFailure::Backoff { delay_ms }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryFailure {
    Backoff { delay_ms: u64 },
    Unhealthy,
}

enum Command {
    Snapshot {
        end_ms: u64,
        advance_checkpoint: bool,
        queued_at: Instant,
        response: mpsc::Sender<Result<WebLiveAsrTerminalSnapshot, String>>,
    },
    Finish {
        end_ms: u64,
        queued_at: Instant,
        response: mpsc::Sender<Result<WebLiveAsrTerminalSnapshot, String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeTrigger {
    Cadence,
    Idle,
}

impl DecodeTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cadence => "cadence",
            Self::Idle => "idle",
        }
    }
}

#[cfg(any(feature = "parakeet-asr", test))]
trait WorkerClock: Send + Sync + 'static {
    fn now_ms(&self) -> u64;
}

#[cfg(any(feature = "parakeet-asr", test))]
type WorkerExitHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(any(feature = "parakeet-asr", test))]
struct WorkerExitGuard(Option<WorkerExitHook>);

#[cfg(any(feature = "parakeet-asr", test))]
impl Drop for WorkerExitGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            hook();
        }
    }
}

#[cfg(any(feature = "parakeet-asr", test))]
struct SystemWorkerClock(Instant);

#[cfg(any(feature = "parakeet-asr", test))]
impl SystemWorkerClock {
    fn new() -> Self {
        Self(Instant::now())
    }
}

#[cfg(any(feature = "parakeet-asr", test))]
impl WorkerClock for SystemWorkerClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

impl WebLiveAsrHandle {
    pub(crate) fn client(&self) -> WebLiveAsrClient {
        self.client.clone()
    }

    pub(crate) fn finish(mut self, end_ms: u64) -> Result<WebLiveAsrTerminalSnapshot, String> {
        let wait = self.client.finish_wait(end_ms);
        let Some(thread) = self.thread.take() else {
            return match wait {
                FinishWait::Responded(result) => result,
                FinishWait::Unavailable(error) => Err(error),
            };
        };
        match wait {
            FinishWait::Responded(result) => {
                join_responded_worker(thread)?;
                result
            }
            FinishWait::Unavailable(error) => {
                self.client.cancelled.store(true, Ordering::SeqCst);
                if thread.is_finished() {
                    let _ = thread.join();
                } else {
                    reap_cancelled_worker(thread, &self.reaper_tx);
                }
                Err(error)
            }
        }
    }
}

impl Drop for WebLiveAsrHandle {
    fn drop(&mut self) {
        self.client.cancelled.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                reap_cancelled_worker(thread, &self.reaper_tx);
            }
        }
    }
}

fn join_responded_worker(thread: std::thread::JoinHandle<()>) -> Result<(), String> {
    thread
        .join()
        .map_err(|_| "Live transcript worker panicked during shutdown".to_string())
}

enum FinishWait {
    Responded(Result<WebLiveAsrTerminalSnapshot, String>),
    Unavailable(String),
}

fn reap_cancelled_worker(
    thread: std::thread::JoinHandle<()>,
    reaper_tx: &mpsc::Sender<std::thread::JoinHandle<()>>,
) {
    let unowned = match reaper_tx.send(thread) {
        Ok(()) => return,
        Err(error) => error.0,
    };
    // The reaper is initialized before a live worker starts. Retain the handle
    // if it nevertheless becomes unavailable; this leaks bounded ownership but
    // does not silently detach or permit external session mutation.
    static CANCELLED_WORKERS: OnceLock<Mutex<Vec<std::thread::JoinHandle<()>>>> = OnceLock::new();
    CANCELLED_WORKERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .push(unowned);
}

fn worker_reaper() -> Result<&'static mpsc::Sender<std::thread::JoinHandle<()>>, String> {
    struct WorkerReaper {
        tx: mpsc::Sender<std::thread::JoinHandle<()>>,
        _thread: std::thread::JoinHandle<()>,
    }
    static REAPER: OnceLock<Result<WorkerReaper, String>> = OnceLock::new();
    REAPER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<std::thread::JoinHandle<()>>();
            let thread = std::thread::Builder::new()
                .name("margins-web-live-asr-reaper".to_string())
                .spawn(move || {
                    while let Ok(worker) = rx.recv() {
                        let _ = worker.join();
                    }
                })
                .map_err(|error| format!("Failed to start live ASR worker reaper: {error}"))?;
            Ok(WorkerReaper {
                tx,
                _thread: thread,
            })
        })
        .as_ref()
        .map(|reaper| &reaper.tx)
        .map_err(Clone::clone)
}

impl WebLiveAsrClient {
    pub(crate) fn is_ready(&self) -> bool {
        self.worker_health.is_ready()
    }

    pub(crate) fn inject(
        &self,
        channel: LiveAudioChannel,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<(), String> {
        let dropped_samples = normalized_sample_count(samples.len(), sample_rate);
        if sample_rate == 0 {
            self.dropped_samples.add(channel, dropped_samples);
            return Err("Live PCM sample rate must be greater than zero".to_string());
        }
        if self.cancelled.load(Ordering::SeqCst) || !self.worker_health.is_alive() {
            self.dropped_samples.add(channel, dropped_samples);
            return Err(WORKER_STOPPED.to_string());
        }
        enqueue_audio(
            &self.audio_tx,
            &self.dropped_samples,
            #[cfg(any(feature = "parakeet-asr", test))]
            &self.queued_samples,
            InjectedAudio {
                channel,
                sample_rate,
                samples,
            },
        )
    }

    pub(crate) fn checkpoint(&self, end_ms: u64) -> Result<LiveTranscriptContext, String> {
        self.snapshot(end_ms, true)
    }

    pub(crate) fn request_context(&self, end_ms: u64) -> Result<LiveTranscriptContext, String> {
        self.snapshot(end_ms, false)
    }

    fn snapshot(
        &self,
        end_ms: u64,
        advance_checkpoint: bool,
    ) -> Result<LiveTranscriptContext, String> {
        let (tx, rx) = mpsc::channel();
        if self.cancelled.load(Ordering::SeqCst) || !self.worker_health.is_alive() {
            return Err(WORKER_STOPPED.to_string());
        }
        // Fast-return while the worker is still loading models (capture-first A5 mirror).
        if !self.worker_health.is_ready() {
            return Err(WORKER_WARMING.to_string());
        }
        self.command_tx
            .send(Command::Snapshot {
                end_ms,
                advance_checkpoint,
                queued_at: Instant::now(),
                response: tx,
            })
            .map_err(|_| WORKER_STOPPED.to_string())?;
        match self.worker_health.recv(
            rx,
            self.request_timeout,
            "Timed out waiting for live ONNX transcript",
        ) {
            Ok(result) => result.map(|snapshot| snapshot.context),
            Err(error) => {
                self.cancelled.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    fn finish_wait(&self, end_ms: u64) -> FinishWait {
        let (tx, rx) = mpsc::channel();
        if self.cancelled.load(Ordering::SeqCst) || !self.worker_health.is_alive() {
            return FinishWait::Unavailable(WORKER_STOPPED.to_string());
        }
        // Signal the worker to abort if it is still loading (mirrors native A7).
        self.worker_health.request_shutdown();
        if self
            .command_tx
            .send(Command::Finish {
                end_ms,
                queued_at: Instant::now(),
                response: tx,
            })
            .is_err()
        {
            return FinishWait::Unavailable(WORKER_STOPPED.to_string());
        }
        match self.worker_health.recv(
            rx,
            self.request_timeout,
            "Timed out finalizing live ONNX transcript",
        ) {
            Ok(result) => FinishWait::Responded(result),
            Err(error) => FinishWait::Unavailable(error),
        }
    }
}

/// Enqueue audio into the unbounded channel with a producer-side sample budget
/// gate. While the worker is loading, up to `WEB_AUDIO_QUEUE_MAX_SAMPLES` of
/// normalized 16 kHz samples are buffered losslessly. Injections that would
/// exceed the budget are dropped and counted so `dropped_samples == 0` reuse
/// qualification is correctly invalidated. Once the worker is running it
/// decrements the counter as it drains, keeping the gate open indefinitely.
#[cfg(any(feature = "parakeet-asr", test))]
fn enqueue_audio(
    tx: &mpsc::Sender<InjectedAudio>,
    dropped_samples: &ChannelDropCounters,
    queued_samples: &AtomicU64,
    audio: InjectedAudio,
) -> Result<(), String> {
    let sample_count = normalized_sample_count(audio.samples.len(), audio.sample_rate);
    // Optimistically reserve budget; roll back on failure.
    let prev = queued_samples.fetch_add(sample_count, Ordering::Relaxed);
    if prev.saturating_add(sample_count) > WEB_AUDIO_QUEUE_MAX_SAMPLES {
        queued_samples.fetch_sub(sample_count, Ordering::Relaxed);
        dropped_samples.add(audio.channel, sample_count);
        return Err("Live transcript audio queue budget exceeded; PCM chunk dropped".to_string());
    }
    match tx.send(audio) {
        Ok(()) => Ok(()),
        Err(mpsc::SendError(audio)) => {
            queued_samples.fetch_sub(sample_count, Ordering::Relaxed);
            dropped_samples.add(audio.channel, sample_count);
            Err(WORKER_STOPPED.to_string())
        }
    }
}

/// Non-parakeet/non-test stub: audio enqueue is a no-op.
#[cfg(not(any(feature = "parakeet-asr", test)))]
fn enqueue_audio(
    tx: &mpsc::Sender<InjectedAudio>,
    dropped_samples: &ChannelDropCounters,
    audio: InjectedAudio,
) -> Result<(), String> {
    let sample_count = normalized_sample_count(audio.samples.len(), audio.sample_rate);
    match tx.send(audio) {
        Ok(()) => Ok(()),
        Err(mpsc::SendError(audio)) => {
            dropped_samples.add(audio.channel, sample_count);
            Err(WORKER_STOPPED.to_string())
        }
    }
}

#[cfg(feature = "parakeet-asr")]
pub(crate) fn start(
    margins_dir: PathBuf,
    session_name: String,
) -> Result<Option<WebLiveAsrHandle>, String> {
    // The managed first-run download can still be in progress when a browser
    // begins recording. WebM remains durable, and offline ASR will run after
    // the runtime is ready; live PCM decoding is optional for that capture.
    if (std::env::var_os("MARGINS_MANAGED_ASR_MODEL").is_some()
        || std::env::var_os("MARGINS_MANAGED_ASR_RUNTIME").is_some())
        && !crate::speech_models::transcription_runtime_available(&crate::settings::load_settings())
    {
        return Ok(None);
    }
    let Some((model_dir, kind)) = resolve_model()? else {
        return Ok(None);
    };
    let reaper_tx = worker_reaper()?.clone();
    let trace = LiveAsrTrace::new(margins_dir, session_name.clone(), "parakeet_onnx");
    let model_label = model_dir.clone();
    start_worker(
        session_name,
        trace,
        Some(model_label),
        STARTUP_TIMEOUT,
        REQUEST_TIMEOUT,
        reaper_tx,
        None,
        move || {
            margins_media::providers::parakeet::ParakeetOnnxBackend::from_dir(&model_dir, kind)
                .map_err(|error| format!("Failed to load live Parakeet ONNX models: {error}"))
        },
    )
    .map(Some)
}

#[cfg(any(feature = "parakeet-asr", test))]
#[allow(clippy::too_many_arguments)]
fn start_worker<D, Loader>(
    session_name: String,
    trace: LiveAsrTrace,
    model_label: Option<PathBuf>,
    _startup_timeout: Duration,
    request_timeout: Duration,
    reaper_tx: mpsc::Sender<std::thread::JoinHandle<()>>,
    worker_exit_hook: Option<WorkerExitHook>,
    loader: Loader,
) -> Result<WebLiveAsrHandle, String>
where
    D: AsrChunkDecoder + 'static,
    Loader: FnOnce() -> Result<D, String> + Send + 'static,
{
    // Capture-first: unbounded channel guarded by producer-side sample budget.
    let (audio_tx, audio_rx) = mpsc::channel::<InjectedAudio>();
    let (command_tx, command_rx) = mpsc::channel();
    let worker_health = WorkerHealth::new();
    let worker_health_for_thread = worker_health.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_thread = cancelled.clone();
    let dropped_samples = Arc::new(ChannelDropCounters::default());
    let dropped_samples_for_thread = dropped_samples.clone();
    let queued_samples = Arc::new(AtomicU64::new(0));
    let queued_samples_for_thread = queued_samples.clone();
    let thread = std::thread::Builder::new()
        .name(format!("margins-web-live-asr-{session_name}"))
        .spawn(move || {
            let _alive_guard = worker_health_for_thread.guard();
            let started = Instant::now();
            // Check for a stop-during-load before doing expensive model work.
            if worker_health_for_thread.is_shutdown_requested() {
                trace.append(json!({
                    "kind":"live_transcript_worker_aborted_warming",
                    "lane":"web",
                    "init_ms":started.elapsed().as_millis(),
                    "model_dir":model_label,
                }));
                return;
            }
            match loader() {
                Ok(decoder) => {
                    let init_ms = started.elapsed().as_millis();
                    // Check for stop-during-load between model construction and running.
                    if worker_health_for_thread.is_shutdown_requested() {
                        trace.append(json!({
                            "kind":"live_transcript_worker_aborted_warming",
                            "lane":"web",
                            "init_ms":init_ms,
                            "model_dir":model_label,
                        }));
                        return;
                    }
                    // Signal readiness — unblocks queued snapshot/checkpoint callers.
                    worker_health_for_thread.mark_ready();
                    trace.append(json!({
                        "kind":"live_transcript_worker_ready_signalled",
                        "lane":"web",
                        "init_ms":init_ms,
                        "warmup_ms":0,
                        "startup_ms":init_ms,
                        "model_dir":model_label,
                    }));
                    run_worker(
                        decoder,
                        trace,
                        audio_rx,
                        command_rx,
                        cancelled_for_thread,
                        dropped_samples_for_thread,
                        queued_samples_for_thread,
                        Arc::new(SystemWorkerClock::new()),
                        worker_exit_hook,
                    );
                }
                Err(error) => {
                    trace.append(json!({
                        "kind":"live_transcript_worker_init_failed",
                        "lane":"web",
                        "init_ms":started.elapsed().as_millis(),
                        "model_dir":model_label,
                        "message":error,
                    }));
                    // Worker exits without marking ready; inject/snapshot callers
                    // will see is_alive() == false → WORKER_STOPPED.
                }
            }
        })
        .map_err(|error| format!("Failed to start headless live ASR: {error}"))?;
    // Non-blocking: return the handle immediately. The worker signals readiness
    // asynchronously via worker_health.mark_ready() after model load.
    Ok(WebLiveAsrHandle {
        client: WebLiveAsrClient {
            audio_tx,
            command_tx,
            worker_health,
            cancelled,
            dropped_samples,
            queued_samples,
            request_timeout,
        },
        thread: Some(thread),
        reaper_tx,
    })
}

#[cfg(test)]
pub(crate) fn start_stalled_test_worker(
    margins_dir: PathBuf,
    session_name: String,
    loader_entered: mpsc::Sender<()>,
    loader_release: mpsc::Receiver<()>,
) -> Result<WebLiveAsrHandle, String> {
    struct EmptyDecoder;
    impl EmptyDecoder {
        fn transcribe_words(&mut self, _samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            Ok(Vec::new())
        }
    }
    test_chunk_decoder!(EmptyDecoder);
    start_worker(
        session_name.clone(),
        LiveAsrTrace::new(margins_dir, session_name, "stalled_test"),
        None,
        Duration::from_millis(10),
        Duration::from_millis(50),
        worker_reaper()?.clone(),
        None,
        move || {
            let _ = loader_entered.send(());
            let _ = loader_release.recv();
            Ok(EmptyDecoder)
        },
    )
}

#[cfg(not(feature = "parakeet-asr"))]
pub(crate) fn start(
    _margins_dir: PathBuf,
    _session_name: String,
) -> Result<Option<WebLiveAsrHandle>, String> {
    Ok(None)
}

#[cfg(feature = "parakeet-asr")]
fn resolve_model() -> Result<Option<(PathBuf, margins::asr::AsrModelKind)>, String> {
    margins_media::model_registry::resolve_parakeet_model().map_err(|error| error.to_string())
}

#[cfg(any(feature = "parakeet-asr", test))]
fn run_worker<D: AsrChunkDecoder>(
    mut decoder: D,
    trace: LiveAsrTrace,
    audio_rx: mpsc::Receiver<InjectedAudio>,
    command_rx: mpsc::Receiver<Command>,
    cancelled: Arc<AtomicBool>,
    dropped_samples: Arc<ChannelDropCounters>,
    queued_samples: Arc<AtomicU64>,
    clock: Arc<dyn WorkerClock>,
    worker_exit_hook: Option<WorkerExitHook>,
) {
    let _exit_guard = WorkerExitGuard(worker_exit_hook);
    let mut mic = ChannelState::default();
    let mut system = ChannelState::default();
    let mut retry = DecodeRetryState::default();
    let mut last_audio_received_at = None;
    let mut last_inference_started_at = None;

    loop {
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        while let Ok(command) = command_rx.try_recv() {
            if handle_command(
                command,
                &mut decoder,
                &audio_rx,
                &mut mic,
                &mut system,
                &mut retry,
                &cancelled,
                &dropped_samples,
                &queued_samples,
                clock.as_ref(),
                &trace,
            ) {
                return;
            }
        }

        match audio_rx.recv_timeout(WORKER_LOOP_POLL) {
            Ok(audio) => {
                last_audio_received_at = Some(clock.now_ms());
                let sample_count = normalized_sample_count(audio.samples.len(), audio.sample_rate);
                queued_samples.fetch_sub(
                    sample_count.min(queued_samples.load(Ordering::Relaxed)),
                    Ordering::Relaxed,
                );
                append_audio(audio, &mut mic, &mut system);
                let since_last_inference_ms =
                    last_inference_started_at.map(|started| clock.now_ms().saturating_sub(started));
                if let Some(trigger) = select_decode_trigger(
                    maximum_residue_ms(&mic, &system),
                    0,
                    since_last_inference_ms,
                    retry.can_attempt(clock.now_ms()),
                ) {
                    if run_incremental_decode(
                        &mut decoder,
                        &mut mic,
                        &mut system,
                        trigger,
                        0,
                        since_last_inference_ms,
                        &mut last_inference_started_at,
                        &mut retry,
                        &cancelled,
                        clock.as_ref(),
                        &trace,
                    ) {
                        return;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // A terminal command always wins over opportunistic idle work.
                if let Ok(command) = command_rx.try_recv() {
                    if handle_command(
                        command,
                        &mut decoder,
                        &audio_rx,
                        &mut mic,
                        &mut system,
                        &mut retry,
                        &cancelled,
                        &dropped_samples,
                        &queued_samples,
                        clock.as_ref(),
                        &trace,
                    ) {
                        return;
                    }
                    continue;
                }

                let idle_ms = last_audio_received_at
                    .map(|received| clock.now_ms().saturating_sub(received))
                    .unwrap_or(0);
                let since_last_inference_ms =
                    last_inference_started_at.map(|started| clock.now_ms().saturating_sub(started));
                if let Some(trigger) = select_decode_trigger(
                    maximum_residue_ms(&mic, &system),
                    idle_ms,
                    since_last_inference_ms,
                    retry.can_attempt(clock.now_ms()),
                ) {
                    if run_incremental_decode(
                        &mut decoder,
                        &mut mic,
                        &mut system,
                        trigger,
                        idle_ms,
                        since_last_inference_ms,
                        &mut last_inference_started_at,
                        &mut retry,
                        &cancelled,
                        clock.as_ref(),
                        &trace,
                    ) {
                        return;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn select_decode_trigger(
    residue_ms: u64,
    idle_ms: u64,
    since_last_inference_ms: Option<u64>,
    retry_ready: bool,
) -> Option<DecodeTrigger> {
    if !retry_ready {
        return None;
    }
    if residue_ms >= BACKGROUND_DECODE_INTERVAL_MS {
        return Some(DecodeTrigger::Cadence);
    }
    if residue_ms >= IDLE_FLUSH_MIN_RESIDUE_MS
        && idle_ms >= IDLE_FLUSH_DELAY_MS
        && since_last_inference_ms.map_or(true, |elapsed| elapsed >= IDLE_FLUSH_MIN_INTERVAL_MS)
    {
        return Some(DecodeTrigger::Idle);
    }
    None
}

#[cfg(any(feature = "parakeet-asr", test))]
#[allow(clippy::too_many_arguments)]
fn handle_command<D: AsrChunkDecoder>(
    command: Command,
    decoder: &mut D,
    audio_rx: &mpsc::Receiver<InjectedAudio>,
    mic: &mut ChannelState,
    system: &mut ChannelState,
    retry: &mut DecodeRetryState,
    cancelled: &AtomicBool,
    dropped_samples: &ChannelDropCounters,
    queued_samples: &AtomicU64,
    clock: &dyn WorkerClock,
    trace: &LiveAsrTrace,
) -> bool {
    let finish = matches!(command, Command::Finish { .. });
    let (kind, end_ms, advance, queued_at, response) = match command {
        Command::Snapshot {
            end_ms,
            advance_checkpoint,
            queued_at,
            response,
        } => ("snapshot", end_ms, advance_checkpoint, queued_at, response),
        Command::Finish {
            end_ms,
            queued_at,
            response,
        } => ("finish", end_ms, false, queued_at, response),
    };
    let command_wait_ms = queued_at.elapsed().as_millis();
    drain_audio(audio_rx, mic, system, queued_samples);
    trace.append(json!({
        "kind":"live_transcript_command_started",
        "command":kind,
        "requested_end_ms":end_ms,
        "mic_available_ms":mic.available_ms(),
        "system_available_ms":system.available_ms(),
        "mic_decoded_before_ms":mic.decoded_until_ms(),
        "system_decoded_before_ms":system.decoded_until_ms(),
        "mic_decoded_before_samples":mic.decoded_until_samples,
        "system_decoded_before_samples":system.decoded_until_samples,
        "mic_residue_ms":mic.residue_ms(),
        "system_residue_ms":system.residue_ms(),
        "command_wait_ms":command_wait_ms,
    }));
    if !finish && !retry.can_attempt(clock.now_ms()) {
        let error = format!(
            "Live transcript decode is backing off until worker millisecond {}",
            retry.retry_not_before_ms
        );
        trace.append(json!({
            "kind":"live_transcript_command_deferred",
            "command":kind,
            "requested_end_ms":end_ms,
            "retry_not_before_ms":retry.retry_not_before_ms,
            "consecutive_failures":retry.consecutive_failures,
        }));
        let _ = response.send(Err(error));
        return false;
    }
    let started = Instant::now();
    let mut result = decode_snapshot(
        decoder,
        mic,
        system,
        dropped_samples,
        cancelled,
        end_ms,
        finish,
    );
    if cancelled.load(Ordering::SeqCst) {
        return true;
    }
    if let Ok(snapshot) = &mut result {
        retry.record_success();
        if let Some(timing) = snapshot.context.timing.as_mut() {
            timing.command_wait_ms = command_wait_ms;
        }
        if advance {
            let requested_samples = ms_to_samples_floor(end_ms);
            mic.checkpoint_samples = requested_samples
                .min(mic.audio.len())
                .min(mic.decoded_until_samples);
            system.checkpoint_samples = requested_samples
                .min(system.audio.len())
                .min(system.decoded_until_samples);
        }
        let timing = snapshot.context.timing.as_ref();
        trace.append(json!({
            "kind":"live_transcript_command_completed",
            "command":kind,
            "requested_end_ms":end_ms,
            "decoded_until_ms":snapshot.context.decoded_until_ms,
            "mic_decoded_until_ms":mic.decoded_until_ms(),
            "system_decoded_until_ms":system.decoded_until_ms(),
            "mic_decoded_samples":snapshot.mic_decoded_samples,
            "system_decoded_samples":snapshot.system_decoded_samples,
            "command_wait_ms":command_wait_ms,
            "mic_decode_ms":timing.map(|value| value.mic_update_ms).unwrap_or(0),
            "system_decode_ms":timing.map(|value| value.system_update_ms).unwrap_or(0),
            "format_ms":timing.map(|value| value.format_ms).unwrap_or(0),
            "total_ms":started.elapsed().as_millis(),
            "ok":true,
        }));
    } else if let Err(error) = &result {
        let failure = retry.record_failure(clock.now_ms());
        let retry_delay_ms = match failure {
            RetryFailure::Backoff { delay_ms } => Some(delay_ms),
            RetryFailure::Unhealthy => None,
        };
        if failure == RetryFailure::Unhealthy {
            cancelled.store(true, Ordering::SeqCst);
        }
        trace.append(json!({
            "kind":"live_transcript_command_completed",
            "command":kind,
            "requested_end_ms":end_ms,
            "command_wait_ms":command_wait_ms,
            "total_ms":started.elapsed().as_millis(),
            "consecutive_failures":retry.consecutive_failures,
            "retry_delay_ms":retry_delay_ms,
            "retry_not_before_ms":retry.retry_not_before_ms,
            "worker_unhealthy":retry.unhealthy,
            "ok":false,
            "message":error,
        }));
    }
    let _ = response.send(result);
    finish || retry.unhealthy
}

#[cfg(any(feature = "parakeet-asr", test))]
#[allow(clippy::too_many_arguments)]
fn run_incremental_decode<D: AsrChunkDecoder>(
    decoder: &mut D,
    mic: &mut ChannelState,
    system: &mut ChannelState,
    trigger: DecodeTrigger,
    idle_ms: u64,
    since_last_inference_ms: Option<u64>,
    last_inference_started_at: &mut Option<u64>,
    retry: &mut DecodeRetryState,
    cancelled: &AtomicBool,
    clock: &dyn WorkerClock,
    trace: &LiveAsrTrace,
) -> bool {
    let mic_residue_before_ms = mic.residue_ms();
    let system_residue_before_ms = system.residue_ms();
    if trigger == DecodeTrigger::Idle {
        trace.append(json!({
            "kind":"live_transcript_idle_flush_triggered",
            "mic_available_ms":mic.available_ms(),
            "system_available_ms":system.available_ms(),
            "mic_decoded_before_ms":mic.decoded_until_ms(),
            "system_decoded_before_ms":system.decoded_until_ms(),
            "mic_decoded_before_samples":mic.decoded_until_samples,
            "system_decoded_before_samples":system.decoded_until_samples,
            "mic_residue_ms":mic_residue_before_ms,
            "system_residue_ms":system_residue_before_ms,
            "idle_ms":idle_ms,
            "since_last_inference_ms":since_last_inference_ms,
        }));
    }
    let started = Instant::now();
    *last_inference_started_at = Some(clock.now_ms());
    let target_samples = mic.audio.len().max(system.audio.len());
    match update_words_until(decoder, mic, system, target_samples) {
        Ok((mic_decode_ms, system_decode_ms)) => {
            if cancelled.load(Ordering::SeqCst) {
                return true;
            }
            retry.record_success();
            trace.append(json!({
                "kind":if trigger == DecodeTrigger::Idle {
                    "live_transcript_idle_flush_completed"
                } else {
                    "live_transcript_background_decode"
                },
                "trigger":trigger.as_str(),
                "mic_available_ms":mic.available_ms(),
                "system_available_ms":system.available_ms(),
                "mic_decoded_until_ms":mic.decoded_until_ms(),
                "system_decoded_until_ms":system.decoded_until_ms(),
                "mic_decoded_samples":mic.decoded_until_samples,
                "system_decoded_samples":system.decoded_until_samples,
                "mic_residue_before_ms":mic_residue_before_ms,
                "system_residue_before_ms":system_residue_before_ms,
                "mic_residue_after_ms":mic.residue_ms(),
                "system_residue_after_ms":system.residue_ms(),
                "idle_ms":idle_ms,
                "mic_decode_ms":mic_decode_ms,
                "system_decode_ms":system_decode_ms,
                "total_ms":started.elapsed().as_millis(),
            }));
            false
        }
        Err(error) => {
            if cancelled.load(Ordering::SeqCst) {
                return true;
            }
            let failure = retry.record_failure(clock.now_ms());
            let retry_delay_ms = match failure {
                RetryFailure::Backoff { delay_ms } => Some(delay_ms),
                RetryFailure::Unhealthy => None,
            };
            trace.append(json!({
                "kind":if trigger == DecodeTrigger::Idle {
                    "live_transcript_idle_flush_failed"
                } else {
                    "live_transcript_background_decode_failed"
                },
                "trigger":trigger.as_str(),
                "mic_available_ms":mic.available_ms(),
                "system_available_ms":system.available_ms(),
                "mic_decoded_until_ms":mic.decoded_until_ms(),
                "system_decoded_until_ms":system.decoded_until_ms(),
                "mic_decoded_samples":mic.decoded_until_samples,
                "system_decoded_samples":system.decoded_until_samples,
                "mic_residue_ms":mic.residue_ms(),
                "system_residue_ms":system.residue_ms(),
                "idle_ms":idle_ms,
                "total_ms":started.elapsed().as_millis(),
                "consecutive_failures":retry.consecutive_failures,
                "retry_delay_ms":retry_delay_ms,
                "retry_not_before_ms":retry.retry_not_before_ms,
                "worker_unhealthy":retry.unhealthy,
                "message":error,
            }));
            if failure == RetryFailure::Unhealthy {
                cancelled.store(true, Ordering::SeqCst);
                trace.append(json!({
                    "kind":"live_transcript_worker_unhealthy",
                    "consecutive_failures":retry.consecutive_failures,
                    "message":error,
                }));
                true
            } else {
                false
            }
        }
    }
}

fn append_audio(audio: InjectedAudio, mic: &mut ChannelState, system: &mut ChannelState) {
    let samples = resample(&audio.samples, audio.sample_rate);
    match audio.channel {
        LiveAudioChannel::Mic => mic.audio.extend(samples),
        LiveAudioChannel::System => system.audio.extend(samples),
    }
}

fn drain_audio(
    rx: &mpsc::Receiver<InjectedAudio>,
    mic: &mut ChannelState,
    system: &mut ChannelState,
    queued_samples: &AtomicU64,
) {
    while let Ok(audio) = rx.try_recv() {
        let sample_count = normalized_sample_count(audio.samples.len(), audio.sample_rate);
        queued_samples.fetch_sub(
            sample_count.min(queued_samples.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        append_audio(audio, mic, system);
    }
}

#[cfg(any(feature = "parakeet-asr", test))]
fn update_words_until<D: AsrChunkDecoder>(
    decoder: &mut D,
    mic: &mut ChannelState,
    system: &mut ChannelState,
    target_samples: usize,
) -> Result<(u128, u128), String> {
    let mic_decode_ms = update_channel_words(decoder, mic, target_samples)
        .map_err(|error| format!("Live mic Parakeet decode failed: {error}"))?;
    let system_decode_ms = update_channel_words(decoder, system, target_samples)
        .map_err(|error| format!("Live system Parakeet decode failed: {error}"))?;
    Ok((mic_decode_ms, system_decode_ms))
}

#[cfg(any(feature = "parakeet-asr", test))]
fn update_channel_words<D: AsrChunkDecoder>(
    decoder: &mut D,
    channel: &mut ChannelState,
    target_samples: usize,
) -> anyhow::Result<u128> {
    let target_samples = bounded_channel_target_samples(
        channel.decoded_until_samples,
        target_samples,
        channel.audio.len(),
    );
    if channel.audio.is_empty() || target_samples <= channel.decoded_until_samples {
        return Ok(0);
    }
    let started = Instant::now();
    let start_sample = rolling_decode_start_samples(channel.decoded_until_samples, target_samples);
    let start_ms = samples_to_ms_floor(start_sample);
    let replace_from_ms = if channel.decoded_until_samples == 0 {
        0
    } else {
        channel
            .decoded_until_ms()
            .saturating_sub(ROLLING_OVERLAP_MS / 2)
    };
    let replacement = decoder
        .decode_chunk(AsrRequest {
            samples: channel.audio[start_sample..target_samples].to_vec(),
            sample_rate_hz: SAMPLE_RATE,
            session_offset_ms: start_ms,
            language: None,
        })?
        .words
        .into_iter()
        .map(|word| WordTiming {
            start_ms: word.start_ms,
            end_ms: word.end_ms,
            text: word.text,
        })
        .collect::<Vec<_>>();
    channel.words.retain(|word| word.end_ms < replace_from_ms);
    channel.words.extend(
        replacement
            .into_iter()
            .filter(|word| word.end_ms >= replace_from_ms),
    );
    channel.words.sort_by_key(|word| word.start_ms);
    channel.decoded_until_samples = channel.decoded_until_samples.max(target_samples);
    Ok(started.elapsed().as_millis())
}

fn bounded_channel_target_samples(
    decoded_until_samples: usize,
    requested_samples: usize,
    available_samples: usize,
) -> usize {
    let rolling_window_samples = ms_to_samples_floor(ROLLING_WINDOW_MS);
    let rolling_overlap_samples = ms_to_samples_floor(ROLLING_OVERLAP_MS);
    let maximum_advance_samples = if decoded_until_samples == 0 {
        rolling_window_samples
    } else {
        rolling_window_samples.saturating_sub(rolling_overlap_samples)
    };
    requested_samples
        .min(available_samples)
        .min(decoded_until_samples.saturating_add(maximum_advance_samples))
}

fn rolling_decode_start_samples(decoded_until_samples: usize, target_samples: usize) -> usize {
    decoded_until_samples
        .saturating_sub(ms_to_samples_floor(ROLLING_OVERLAP_MS))
        .max(target_samples.saturating_sub(ms_to_samples_floor(ROLLING_WINDOW_MS)))
}

#[cfg(any(feature = "parakeet-asr", test))]
fn decode_snapshot<D: AsrChunkDecoder>(
    decoder: &mut D,
    mic: &mut ChannelState,
    system: &mut ChannelState,
    dropped_samples: &ChannelDropCounters,
    cancelled: &AtomicBool,
    requested_end_ms: u64,
    terminal: bool,
) -> Result<WebLiveAsrTerminalSnapshot, String> {
    let requested_end_samples = ms_to_samples_floor(requested_end_ms);
    // Finish consumes the exact accepted endpoints. `end_ms` remains useful for
    // display and checkpoint requests, but its integer precision must not omit
    // a sub-millisecond suffix from terminal inference.
    let mic_end_samples = if terminal {
        mic.audio.len()
    } else {
        requested_end_samples.min(mic.audio.len())
    };
    let system_end_samples = if terminal {
        system.audio.len()
    } else {
        requested_end_samples.min(system.audio.len())
    };
    let target_samples = mic_end_samples.max(system_end_samples);
    let mut mic_decode_ms = 0;
    let mut system_decode_ms = 0;
    while mic.decoded_until_samples < mic_end_samples
        || system.decoded_until_samples < system_end_samples
    {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Live transcript worker cancelled".to_string());
        }
        let mic_before_samples = mic.decoded_until_samples;
        let system_before_samples = system.decoded_until_samples;
        let (mic_step_ms, system_step_ms) =
            update_words_until(decoder, mic, system, target_samples)?;
        mic_decode_ms += mic_step_ms;
        system_decode_ms += system_step_ms;
        if mic.decoded_until_samples == mic_before_samples
            && system.decoded_until_samples == system_before_samples
        {
            return Err("Live transcript decode made no timeline progress".to_string());
        }
    }
    let decoded_until_ms = minimum_present_value(
        mic.is_present().then_some(mic.decoded_until_ms()),
        system.is_present().then_some(system.decoded_until_ms()),
    );
    let checkpoint_ms = minimum_present_value(
        mic.is_present().then_some(mic.checkpoint_ms()),
        system.is_present().then_some(system.checkpoint_ms()),
    );
    let mic_end_ms = samples_to_ms_floor(mic_end_samples);
    let system_end_ms = samples_to_ms_floor(system_end_samples);
    let format_started = Instant::now();
    let transcript =
        format_words_ranges(&mic.words, &system.words, 0, mic_end_ms, 0, system_end_ms);
    let previous = format_words_ranges(
        &mic.words,
        &system.words,
        0,
        mic.checkpoint_ms().min(mic_end_ms),
        0,
        system.checkpoint_ms().min(system_end_ms),
    );
    let new = format_words_ranges(
        &mic.words,
        &system.words,
        mic.checkpoint_ms().min(mic_end_ms),
        mic_end_ms,
        system.checkpoint_ms().min(system_end_ms),
        system_end_ms,
    );
    let format_ms = format_started.elapsed().as_millis();
    Ok(WebLiveAsrTerminalSnapshot {
        mic_decoded_samples: mic.decoded_until_samples as u64,
        system_decoded_samples: system.decoded_until_samples as u64,
        context: LiveTranscriptContext {
            transcript: transcript.clone(),
            previous_transcript: previous,
            new_transcript: new,
            committed_transcript: transcript,
            hypothesis_transcript: String::new(),
            transcript_source: "web_parakeet_onnx".to_string(),
            previous_memo_checkpoint_ms: checkpoint_ms,
            decoded_until_ms,
            committed_until_ms: decoded_until_ms,
            mic_accepted_samples: mic.audio.len() as u64,
            system_accepted_samples: system.audio.len() as u64,
            mic_dropped_samples: dropped_samples.get(LiveAudioChannel::Mic),
            system_dropped_samples: dropped_samples.get(LiveAudioChannel::System),
            timing: Some(LiveTranscriptTiming {
                command_wait_ms: 0,
                mic_update_ms: mic_decode_ms,
                system_update_ms: system_decode_ms,
                format_ms,
            }),
        },
    })
}

fn maximum_residue_ms(mic: &ChannelState, system: &ChannelState) -> u64 {
    mic.residue_ms().max(system.residue_ms())
}

fn minimum_present_value(mic: Option<u64>, system: Option<u64>) -> u64 {
    match (mic, system) {
        (Some(mic), Some(system)) => mic.min(system),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => 0,
    }
}

#[cfg(test)]
fn format_words(mic: &[WordTiming], system: &[WordTiming], after_ms: u64, end_ms: u64) -> String {
    format_words_ranges(mic, system, after_ms, end_ms, after_ms, end_ms)
}

fn format_words_ranges(
    mic: &[WordTiming],
    system: &[WordTiming],
    mic_after_ms: u64,
    mic_end_ms: u64,
    system_after_ms: u64,
    system_end_ms: u64,
) -> String {
    let mut words = mic
        .iter()
        .filter(|word| word.end_ms > mic_after_ms && word.start_ms <= mic_end_ms)
        .map(|word| (word.start_ms, "mic", word.text.trim()))
        .chain(
            system
                .iter()
                .filter(|word| word.end_ms > system_after_ms && word.start_ms <= system_end_ms)
                .map(|word| (word.start_ms, "system", word.text.trim())),
        )
        .filter(|(_, _, text)| !text.is_empty())
        .collect::<Vec<_>>();
    words.sort_by_key(|(start, _, _)| *start);
    let mut out = String::new();
    let mut channel = "";
    for (start_ms, next_channel, text) in words {
        if channel != next_channel {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push('[');
            out.push_str(&format_elapsed_ms(start_ms));
            out.push_str("] ");
            out.push_str(if next_channel == "mic" {
                "Mic: "
            } else {
                "System: "
            });
            channel = next_channel;
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(text);
    }
    out
}

fn format_elapsed_ms(ms: u64) -> String {
    let total_seconds = ms / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static NEXT_TEST_TRACE: AtomicU64 = AtomicU64::new(0);

    struct RecognizingDecoder {
        calls: Arc<AtomicUsize>,
    }

    impl RecognizingDecoder {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    impl RecognizingDecoder {
        fn transcribe_words(&mut self, samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(samples
                .iter()
                .enumerate()
                .filter_map(|(index, sample)| {
                    let text = if (*sample - 42.0).abs() < f32::EPSILON {
                        "lagging speech"
                    } else if (*sample - 7.0).abs() < f32::EPSILON {
                        "terminal marker"
                    } else {
                        return None;
                    };
                    let start_ms = samples_to_ms_floor(index);
                    Some(WordTiming {
                        start_ms,
                        end_ms: start_ms.saturating_add(100),
                        text: text.to_string(),
                    })
                })
                .collect())
        }
    }

    struct FailingDecoder {
        call_tx: mpsc::Sender<usize>,
        calls: usize,
    }

    struct RecoveringDecoder {
        call_tx: mpsc::Sender<usize>,
        first_release_rx: Option<mpsc::Receiver<()>>,
        calls: usize,
    }

    struct ControlledFailingDecoder {
        call_tx: mpsc::Sender<usize>,
        release_rx: mpsc::Receiver<()>,
        calls: usize,
    }

    impl ControlledFailingDecoder {
        fn transcribe_words(&mut self, _samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            self.calls += 1;
            let _ = self.call_tx.send(self.calls);
            let _ = self.release_rx.recv();
            anyhow::bail!("controlled permanent fake decode failure")
        }
    }

    impl RecoveringDecoder {
        fn transcribe_words(&mut self, _samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            self.calls += 1;
            let _ = self.call_tx.send(self.calls);
            if self.calls == 1 {
                if let Some(release_rx) = self.first_release_rx.take() {
                    let _ = release_rx.recv();
                }
                anyhow::bail!("transient fake decode failure");
            }
            Ok(Vec::new())
        }
    }

    impl FailingDecoder {
        fn transcribe_words(&mut self, _samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            self.calls += 1;
            let _ = self.call_tx.send(self.calls);
            anyhow::bail!("permanent fake decode failure")
        }
    }

    struct BlockingDecoder {
        entered_tx: Option<mpsc::Sender<()>>,
        release_rx: mpsc::Receiver<()>,
    }

    impl BlockingDecoder {
        fn transcribe_words(&mut self, _samples: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
            if let Some(entered_tx) = self.entered_tx.take() {
                let _ = entered_tx.send(());
            }
            let _ = self.release_rx.recv();
            Ok(Vec::new())
        }
    }

    test_chunk_decoder!(RecognizingDecoder);
    test_chunk_decoder!(ControlledFailingDecoder);
    test_chunk_decoder!(RecoveringDecoder);
    test_chunk_decoder!(FailingDecoder);
    test_chunk_decoder!(BlockingDecoder);

    #[derive(Default)]
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn set(&self, now_ms: u64) {
            self.0.store(now_ms, Ordering::SeqCst);
        }
    }

    impl WorkerClock for ManualClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn test_trace(name: &str) -> (LiveAsrTrace, PathBuf) {
        let id = NEXT_TEST_TRACE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "margins-web-live-asr-{name}-{}-{id}",
            std::process::id()
        ));
        (
            LiveAsrTrace::new(path.clone(), name.to_string(), "fake"),
            path,
        )
    }

    fn injected(channel: LiveAudioChannel, samples: usize) -> InjectedAudio {
        InjectedAudio {
            channel,
            sample_rate: SAMPLE_RATE,
            samples: vec![0.0; samples],
        }
    }

    #[test]
    fn terminal_web_transcript_uses_checkpoint_compatible_timestamps() {
        let mic = vec![
            WordTiming {
                start_ms: 1_200,
                end_ms: 1_500,
                text: "stable".to_string(),
            },
            WordTiming {
                start_ms: 1_600,
                end_ms: 1_900,
                text: "words".to_string(),
            },
        ];
        assert_eq!(
            format_words(&mic, &[], 0, 2_000),
            "[00:01] Mic: stable words"
        );
    }

    #[test]
    fn idle_flush_requires_residue_idle_delay_and_inference_cadence() {
        assert_eq!(select_decode_trigger(999, 10_000, None, true), None);
        assert_eq!(select_decode_trigger(1_000, 1_499, None, true), None);
        assert_eq!(select_decode_trigger(1_000, 1_500, Some(4_999), true), None);
        assert_eq!(
            select_decode_trigger(1_000, 1_500, Some(5_000), true),
            Some(DecodeTrigger::Idle)
        );
        assert_eq!(
            select_decode_trigger(5_000, 0, Some(0), true),
            Some(DecodeTrigger::Cadence)
        );
        assert_eq!(select_decode_trigger(5_000, 0, Some(0), false), None);
    }

    #[test]
    fn exact_end_never_schedules_an_inference() {
        assert_eq!(select_decode_trigger(0, 60_000, None, true), None);
        assert_eq!(
            rolling_decode_start_samples(ms_to_samples_floor(12_000), ms_to_samples_floor(12_000)),
            ms_to_samples_floor(11_000)
        );
    }

    #[test]
    fn terminal_decode_preserves_lagging_speech_beyond_overlap() {
        let (mut decoder, _) = RecognizingDecoder::new();
        let mut mic = ChannelState {
            audio: vec![0.0; ms_to_samples_floor(40_000)],
            ..ChannelState::default()
        };
        let mut system = ChannelState {
            audio: vec![0.0; ms_to_samples_floor(40_000)],
            ..ChannelState::default()
        };
        system.audio[ms_to_samples_floor(10_000)] = 42.0;

        while mic.decoded_until_samples < ms_to_samples_floor(30_000) {
            update_channel_words(&mut decoder, &mut mic, ms_to_samples_floor(30_000)).unwrap();
        }
        update_channel_words(&mut decoder, &mut system, ms_to_samples_floor(5_000)).unwrap();
        assert!(
            mic.decoded_until_samples
                .saturating_sub(system.decoded_until_samples)
                > ms_to_samples_floor(ROLLING_OVERLAP_MS)
        );

        let snapshot = decode_snapshot(
            &mut decoder,
            &mut mic,
            &mut system,
            &ChannelDropCounters::default(),
            &AtomicBool::new(false),
            40_000,
            true,
        )
        .unwrap();
        assert_eq!(snapshot.mic_decoded_samples, mic.audio.len() as u64);
        assert_eq!(snapshot.system_decoded_samples, system.audio.len() as u64);
        assert!(snapshot
            .context
            .transcript
            .contains("System: lagging speech"));
    }

    #[test]
    fn terminal_decode_covers_sub_millisecond_saved_endpoint_exactly() {
        let (mut decoder, _) = RecognizingDecoder::new();
        let mut mic = ChannelState {
            audio: vec![0.0; 537_970],
            ..ChannelState::default()
        };
        mic.audio[537_969] = 7.0;
        let mut system = ChannelState::default();
        let snapshot = decode_snapshot(
            &mut decoder,
            &mut mic,
            &mut system,
            &ChannelDropCounters::default(),
            &AtomicBool::new(false),
            33_623,
            true,
        )
        .unwrap();

        assert_eq!(samples_to_ms_floor(537_970), 33_623);
        assert_eq!(ms_to_samples_floor(33_623), 537_968);
        assert_eq!(snapshot.mic_decoded_samples, 537_970);
        assert_eq!(snapshot.context.decoded_until_ms, 33_623);
        assert!(snapshot.context.transcript.contains("terminal marker"));
    }

    #[test]
    fn worker_loop_throttles_new_audio_and_opens_circuit_on_fourth_failure() {
        let (audio_tx, audio_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let (call_tx, call_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(ManualClock::default());
        let (trace, trace_dir) = test_trace("backoff");
        let worker_cancelled = cancelled.clone();
        let worker_clock = clock.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                ControlledFailingDecoder {
                    call_tx,
                    release_rx,
                    calls: 0,
                },
                trace,
                audio_rx,
                command_rx,
                worker_cancelled,
                Arc::new(ChannelDropCounters::default()),
                Arc::new(AtomicU64::new(0)),
                worker_clock,
                None,
            );
        });

        audio_tx
            .send(injected(LiveAudioChannel::Mic, ms_to_samples_floor(5_000)))
            .unwrap();
        assert_eq!(call_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        release_tx.send(()).unwrap();
        let await_backoff = || {
            let (response_tx, response_rx) = mpsc::channel();
            command_tx
                .send(Command::Snapshot {
                    end_ms: 5_000,
                    advance_checkpoint: false,
                    queued_at: Instant::now(),
                    response: response_tx,
                })
                .unwrap();
            assert!(response_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err()
                .contains("backing off"));
        };
        await_backoff();

        for (before_ms, ready_ms, expected_call) in
            [(999, 1_000, 2), (2_999, 3_000, 3), (6_999, 7_000, 4)]
        {
            clock.set(before_ms);
            audio_tx.send(injected(LiveAudioChannel::Mic, 1)).unwrap();
            let (response_tx, response_rx) = mpsc::channel();
            command_tx
                .send(Command::Snapshot {
                    end_ms: 5_000,
                    advance_checkpoint: false,
                    queued_at: Instant::now(),
                    response: response_tx,
                })
                .unwrap();
            assert!(response_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err()
                .contains("backing off"));
            assert!(call_rx.try_recv().is_err());
            clock.set(ready_ms);
            audio_tx.send(injected(LiveAudioChannel::Mic, 1)).unwrap();
            assert_eq!(
                call_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected_call
            );
            release_tx.send(()).unwrap();
            if expected_call < 4 {
                await_backoff();
            }
        }

        worker.join().unwrap();
        assert!(cancelled.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn worker_loop_success_resets_failure_backoff() {
        let (audio_tx, audio_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let (call_tx, call_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::channel();
        let clock = Arc::new(ManualClock::default());
        let (trace, trace_dir) = test_trace("backoff-recovery");
        let worker_clock = clock.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                RecoveringDecoder {
                    call_tx,
                    first_release_rx: Some(first_release_rx),
                    calls: 0,
                },
                trace,
                audio_rx,
                command_rx,
                Arc::new(AtomicBool::new(false)),
                Arc::new(ChannelDropCounters::default()),
                Arc::new(AtomicU64::new(0)),
                worker_clock,
                None,
            );
        });

        audio_tx
            .send(injected(LiveAudioChannel::Mic, ms_to_samples_floor(5_000)))
            .unwrap();
        assert_eq!(call_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        first_release_tx.send(()).unwrap();
        let (deferred_tx, deferred_rx) = mpsc::channel();
        command_tx
            .send(Command::Snapshot {
                end_ms: 5_000,
                advance_checkpoint: false,
                queued_at: Instant::now(),
                response: deferred_tx,
            })
            .unwrap();
        assert!(deferred_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err()
            .contains("backing off"));
        clock.set(1_000);
        audio_tx.send(injected(LiveAudioChannel::Mic, 1)).unwrap();
        assert_eq!(call_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);

        // A success clears the failure history, so fresh cadence audio is not
        // subjected to what would otherwise be the second (2s) backoff.
        audio_tx
            .send(injected(LiveAudioChannel::Mic, ms_to_samples_floor(5_000)))
            .unwrap();
        assert_eq!(call_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 3);

        let (response_tx, response_rx) = mpsc::channel();
        command_tx
            .send(Command::Finish {
                end_ms: 10_001,
                queued_at: Instant::now(),
                response: response_tx,
            })
            .unwrap();
        response_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn hosted_budget_accepts_audio_through_four_minute_cold_start() {
        let (audio_tx, audio_rx) = mpsc::channel();
        let dropped = ChannelDropCounters::default();
        let queued = AtomicU64::new(SAMPLE_RATE as u64 * 60 * 4);

        enqueue_audio(
            &audio_tx,
            &dropped,
            &queued,
            injected(LiveAudioChannel::Mic, ms_to_samples_floor(1_000)),
        )
        .expect("hosted cold-start PCM must remain lossless past two minutes");

        assert!(audio_rx.try_recv().is_ok());
        assert_eq!(dropped.get(LiveAudioChannel::Mic), 0);
        assert_eq!(
            queued.load(Ordering::Relaxed),
            SAMPLE_RATE as u64 * 60 * 4 + SAMPLE_RATE as u64
        );
    }

    #[test]
    fn budget_gate_drops_audio_over_max_and_records_in_counters() {
        // Seed queued audio, then force the counter to the hosted ceiling.
        let (audio_tx, audio_rx) = mpsc::channel();
        let dropped = Arc::new(ChannelDropCounters::default());
        let queued = Arc::new(AtomicU64::new(0));
        // Pre-fill exactly to budget using direct channel sends (no gate needed).
        audio_tx
            .send(injected(
                LiveAudioChannel::System,
                ms_to_samples_floor(5_000),
            ))
            .unwrap();
        queued.fetch_add(ms_to_samples_floor(5_000) as u64, Ordering::Relaxed);
        audio_tx
            .send(injected(
                LiveAudioChannel::System,
                ms_to_samples_floor(5_000),
            ))
            .unwrap();
        queued.fetch_add(ms_to_samples_floor(5_000) as u64, Ordering::Relaxed);
        // At the hosted ceiling, the next chunk is rejected and accounted for.
        queued.store(WEB_AUDIO_QUEUE_MAX_SAMPLES, Ordering::Relaxed);
        assert!(enqueue_audio(
            &audio_tx,
            &dropped,
            &queued,
            injected(LiveAudioChannel::System, ms_to_samples_floor(1_000)),
        )
        .is_err());
        assert_eq!(
            dropped.get(LiveAudioChannel::System),
            ms_to_samples_floor(1_000) as u64
        );

        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        command_tx
            .send(Command::Finish {
                end_ms: 10_000,
                queued_at: Instant::now(),
                response: response_tx,
            })
            .unwrap();
        let (decoder, calls) = RecognizingDecoder::new();
        let (trace, trace_dir) = test_trace("budget-gate");
        let worker_dropped = dropped.clone();
        let worker_queued = queued.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                decoder,
                trace,
                audio_rx,
                command_rx,
                Arc::new(AtomicBool::new(false)),
                worker_dropped,
                worker_queued,
                Arc::new(ManualClock::default()),
                None,
            );
        });

        let snapshot = response_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(
            snapshot.system_decoded_samples,
            ms_to_samples_floor(10_000) as u64
        );
        assert_eq!(
            snapshot.context.system_dropped_samples,
            ms_to_samples_floor(1_000) as u64
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn actual_finish_failure_returns_soft_fallback_error() {
        let (audio_tx, audio_rx) = mpsc::channel();
        audio_tx
            .send(injected(LiveAudioChannel::Mic, ms_to_samples_floor(5_000)))
            .unwrap();
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        command_tx
            .send(Command::Finish {
                end_ms: 5_000,
                queued_at: Instant::now(),
                response: response_tx,
            })
            .unwrap();
        let (call_tx, _call_rx) = mpsc::channel();
        let (trace, trace_dir) = test_trace("finish-fallback");
        let worker = std::thread::spawn(move || {
            run_worker(
                FailingDecoder { call_tx, calls: 0 },
                trace,
                audio_rx,
                command_rx,
                Arc::new(AtomicBool::new(false)),
                Arc::new(ChannelDropCounters::default()),
                Arc::new(AtomicU64::new(0)),
                Arc::new(ManualClock::default()),
                None,
            );
        });
        let error = response_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        worker.join().unwrap();
        assert!(error.contains("fake decode failure"));
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn handle_finish_waits_for_worker_exit_after_successful_response() {
        let (decoder, _) = RecognizingDecoder::new();
        let (reaper_tx, _reaper_rx) = mpsc::channel();
        let (exit_entered_tx, exit_entered_rx) = mpsc::channel();
        let (exit_release_tx, exit_release_rx) = mpsc::channel();
        let exit_release_rx = Arc::new(Mutex::new(exit_release_rx));
        let exit_hook: WorkerExitHook = Arc::new(move || {
            let _ = exit_entered_tx.send(());
            let _ = exit_release_rx.lock().unwrap().recv();
        });
        let (trace, trace_dir) = test_trace("handle-success-join");
        let handle = start_worker(
            "handle-success-join".to_string(),
            trace,
            None,
            Duration::from_secs(1),
            Duration::from_secs(1),
            reaper_tx,
            Some(exit_hook),
            move || Ok(decoder),
        )
        .unwrap();
        handle
            .client()
            .inject(
                LiveAudioChannel::Mic,
                SAMPLE_RATE,
                vec![0.0; ms_to_samples_floor(1_000)],
            )
            .unwrap();

        let (finish_tx, finish_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = finish_tx.send(handle.finish(1_000));
        });
        exit_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(matches!(
            finish_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        exit_release_tx.send(()).unwrap();
        let snapshot = finish_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot.mic_decoded_samples,
            ms_to_samples_floor(1_000) as u64
        );
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn handle_finish_timeout_transfers_real_worker_to_injected_reaper() {
        let (decode_entered_tx, decode_entered_rx) = mpsc::channel();
        let (decode_release_tx, decode_release_rx) = mpsc::channel();
        let (reaper_tx, reaper_rx) = mpsc::channel();
        let (trace, trace_dir) = test_trace("handle-timeout-transfer");
        let handle = start_worker(
            "handle-timeout-transfer".to_string(),
            trace,
            None,
            Duration::from_secs(1),
            Duration::from_millis(5),
            reaper_tx,
            None,
            move || {
                Ok(BlockingDecoder {
                    entered_tx: Some(decode_entered_tx),
                    release_rx: decode_release_rx,
                })
            },
        )
        .unwrap();
        handle
            .client()
            .inject(
                LiveAudioChannel::Mic,
                SAMPLE_RATE,
                vec![0.0; ms_to_samples_floor(5_000)],
            )
            .unwrap();
        decode_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let error = handle.finish(5_000).unwrap_err();
        assert!(error.contains("Timed out finalizing"));
        let worker = reaper_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        decode_release_tx.send(()).unwrap();
        worker.join().unwrap();
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    /// Non-blocking start: start_worker returns immediately even when the loader
    /// takes a long time. The handle is valid and finish signals shutdown so the
    /// worker aborts loading and returns an empty terminal.
    #[test]
    fn nonblocking_start_returns_handle_before_loader_completes() {
        let (loader_entered_tx, loader_entered_rx) = mpsc::channel::<()>();
        let (loader_release_tx, loader_release_rx) = mpsc::channel::<()>();
        let (reaper_tx, _reaper_rx) = mpsc::channel();
        let (trace, trace_dir) = test_trace("nonblocking-start");
        let handle = start_worker(
            "nonblocking-start".to_string(),
            trace,
            None,
            Duration::from_millis(5),
            Duration::from_secs(1),
            reaper_tx,
            None,
            move || {
                let _ = loader_entered_tx.send(());
                let _ = loader_release_rx.recv();
                Ok(RecognizingDecoder::new().0)
            },
        )
        .expect("start_worker must succeed immediately (non-blocking)");
        // Verify loader is running but handle is already available.
        loader_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        // Worker is not ready yet.
        let client = handle.client();
        assert!(!client.is_ready());
        // snapshot fast-returns WORKER_WARMING.
        let snap_err = client.snapshot(0, false).unwrap_err();
        assert!(
            snap_err.contains("warming"),
            "expected warming, got: {snap_err}"
        );
        // Release the loader and wait for worker to become ready.
        loader_release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.is_ready() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(client.is_ready(), "worker should be ready after load");
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    /// Audio injected before the worker is ready is buffered losslessly up to
    /// AUDIO_QUEUE_MAX_SAMPLES; the budget gate drops over-budget chunks.
    #[test]
    fn lossless_buffer_while_loading() {
        let (loader_entered_tx, loader_entered_rx) = mpsc::channel::<()>();
        let (loader_release_tx, loader_release_rx) = mpsc::channel::<()>();
        let (reaper_tx, _reaper_rx) = mpsc::channel();
        let (trace, trace_dir) = test_trace("lossless-buffer");
        let handle = start_worker(
            "lossless-buffer".to_string(),
            trace,
            None,
            Duration::from_millis(5),
            Duration::from_secs(1),
            reaper_tx,
            None,
            move || {
                let _ = loader_entered_tx.send(());
                let _ = loader_release_rx.recv();
                Ok(RecognizingDecoder::new().0)
            },
        )
        .unwrap();
        loader_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let client = handle.client();
        // Inject 5 s of audio while the worker is still loading — must not drop.
        client
            .inject(
                LiveAudioChannel::Mic,
                SAMPLE_RATE,
                vec![0.0; ms_to_samples_floor(5_000)],
            )
            .unwrap();
        // inject() returned Ok — no drops counted.
        // Release loader; wait for worker to mark ready, then finish.
        loader_release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.is_ready() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(client.is_ready());
        let snapshot = handle.finish(5_000).unwrap();
        assert_eq!(
            snapshot.mic_decoded_samples,
            ms_to_samples_floor(5_000) as u64,
            "worker must have processed the buffered audio"
        );
        assert_eq!(snapshot.context.mic_dropped_samples, 0);
        let _ = std::fs::remove_dir_all(trace_dir);
    }

    #[test]
    fn channel_specific_checkpoint_ranges_do_not_hide_lagging_words() {
        let mic = vec![WordTiming {
            start_ms: 25_000,
            end_ms: 25_500,
            text: "ahead".to_string(),
        }];
        let system = vec![WordTiming {
            start_ms: 10_000,
            end_ms: 10_500,
            text: "lagging speech".to_string(),
        }];
        let new = format_words_ranges(&mic, &system, 30_000, 40_000, 5_000, 40_000);
        assert_eq!(new, "[00:10] System: lagging speech");
    }

    #[test]
    fn rolling_decode_keeps_overlap_while_bounding_the_window() {
        let target = bounded_channel_target_samples(
            ms_to_samples_floor(20_000),
            ms_to_samples_floor(40_000),
            ms_to_samples_floor(40_000),
        );
        assert_eq!(target, ms_to_samples_floor(34_000));
        assert_eq!(
            rolling_decode_start_samples(ms_to_samples_floor(20_000), target),
            ms_to_samples_floor(19_000)
        );
    }
}

fn audio_duration_ms(samples: &[f32]) -> u64 {
    samples_to_ms_floor(samples.len())
}

fn samples_to_ms_floor(samples: usize) -> u64 {
    (samples as u64)
        .saturating_mul(1_000)
        .saturating_div(SAMPLE_RATE as u64)
}

fn ms_to_samples_floor(ms: u64) -> usize {
    ms.saturating_mul(SAMPLE_RATE as u64).saturating_div(1_000) as usize
}

fn normalized_sample_count(samples: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return samples as u64;
    }
    (samples as u64)
        .saturating_mul(SAMPLE_RATE as u64)
        .saturating_div(sample_rate as u64)
}

fn resample(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = sample_rate as f64 / SAMPLE_RATE as f64;
    let output_len = (samples.len() as f64 / ratio).floor() as usize;
    (0..output_len)
        .map(|index| {
            let source = index as f64 * ratio;
            let left = source.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (source - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}
