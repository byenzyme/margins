use chrono::Local;
use margins::recorder;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{
    device_registry::{DeviceRegistry, DeviceSnapshot},
    live_backchannel::{self, LiveTranscriptionMode},
    mic_control::{
        self, HoldingReason, OpenTarget, PolicyAction, PolicyEvent, PolicyMicRuntime,
        SessionMicTarget, SwapCause,
    },
    settings::InputDeviceMode,
    MemoLine,
};

/// Speaker peak high enough to count as real meeting/computer audio, not just
/// Core Audio tap noise floor. This gates scary "dropped" warnings until the
/// app has actually heard the other side once in this recording.
const SYSTEM_AUDIO_SEEN_THRESHOLD: f32 = 0.001;
pub(crate) const CAPTURE_DEVICE_CHANGED_EVENT: &str = "capture-device-changed";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingPhase {
    Recording,
    Pausing,
    Paused,
    Finalizing,
    NeedsAttention,
}

impl RecordingPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Finalizing => "finalizing",
            Self::NeedsAttention => "needs_attention",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum CaptureDeviceState {
    Active {
        device_name: String,
    },
    Switching {
        from: String,
        to: String,
    },
    Holding {
        last_good: String,
        reason: CaptureDeviceHoldingReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureDeviceHoldingReason {
    DeviceLost,
    OpenFailed,
}

impl CaptureDeviceState {
    pub(crate) fn active(device_name: impl Into<String>) -> Self {
        Self::Active {
            device_name: device_name.into(),
        }
    }

    pub(crate) fn switching(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::Switching {
            from: from.into(),
            to: to.into(),
        }
    }

    pub(crate) fn holding(
        last_good: impl Into<String>,
        reason: CaptureDeviceHoldingReason,
    ) -> Self {
        Self::Holding {
            last_good: last_good.into(),
            reason,
        }
    }

    pub(crate) fn last_good_device_name(&self) -> String {
        match self {
            Self::Active { device_name } => device_name.clone(),
            Self::Switching { from, .. } => from.clone(),
            Self::Holding { last_good, .. } => last_good.clone(),
        }
    }
}

/// Send+Sync recording metadata. The actual cpal::Stream (which is !Send)
/// lives on the dedicated recording thread — we only keep atomic handles here.
pub(crate) struct RecordingState {
    pub(crate) session_name: String,
    pub(crate) work_dir: PathBuf,
    pub(crate) start_time: chrono::DateTime<Local>,
    /// Monotonic start of the active capture segment. Only a real segment
    /// rotation (resume) resets this; mic/system lane swaps do not.
    pub(crate) segment_started_at: Instant,
    pub(crate) segment_index: i64,
    pub(crate) mic_peak: Arc<AtomicU32>,
    /// Frames from the currently attached microphone capture. This resets on
    /// a route switch so readiness reflects the new route, not an old one.
    pub(crate) mic_frames: Arc<AtomicU64>,
    pub(crate) spk_peak: Arc<AtomicU32>,
    pub(crate) mic_drops: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) spk_drops: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) spk_silence: Arc<std::sync::atomic::AtomicU64>,
    /// Session-wide system frames. System-capture replacement never resets it.
    pub(crate) spk_frames: Arc<std::sync::atomic::AtomicU64>,
    /// Current system-capture frames, used only for post-restart health/grace.
    pub(crate) spk_health_frames: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) spk_rate: Arc<AtomicU32>,
    /// Sticky session-wide signal observation used by final qualification.
    pub(crate) system_audio_seen: Arc<AtomicBool>,
    /// Signal observation for the current system-capture instance only.
    pub(crate) system_health_seen: Arc<AtomicBool>,
    pub(crate) live_transcription_mode: LiveTranscriptionMode,
    pub(crate) live_backchannel: Option<live_backchannel::LiveBackchannelHandle>,
    pub(crate) live_generation: u64,
    pub(crate) memo_lines: Vec<MemoLine>,
    pub(crate) phase: RecordingPhase,
    /// True while capture is paused: the current controller has sealed its
    /// writer, but the session remains active and resumable.
    pub(crate) paused: bool,
    pub(crate) capture_supervisor_stop: Arc<AtomicBool>,
    pub(crate) capture_supervisor_handle: Option<JoinHandle<()>>,
    /// Capture-time watermark of the last durable live transcript checkpoint.
    /// The backend supervisor uses this even when the renderer is backgrounded.
    pub(crate) last_live_checkpoint_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Capture-time deadline for retrying after a failed checkpoint. Kept
    /// separate from the last-success watermark so telemetry stays honest.
    pub(crate) next_live_checkpoint_attempt_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Coalesces periodic checkpoint requests so a slow decoder cannot build an
    /// unbounded queue of three-minute snapshots.
    pub(crate) live_checkpoint_in_flight: Arc<AtomicBool>,
    pub(crate) segment_controller: SegmentControllerClient,
    pub(crate) segment_telemetry: SegmentControllerTelemetry,
    /// Exact counters from every segment already sealed in this session.
    pub(crate) sealed_diagnostics: SegmentDiagnostics,
}

#[cfg(not(test))]
const CANDIDATE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);
// Keep the same actor-driven timeout path fast enough for deterministic unit tests.
#[cfg(test)]
const CANDIDATE_CONFIRM_TIMEOUT: Duration = Duration::from_millis(250);
const CONTROLLER_POLL_INTERVAL: Duration = Duration::from_millis(20);

fn mic_error_allows_backend_retry(reason: &str) -> bool {
    reason.contains(" microphone backend: Stalled")
        || reason.contains(" microphone backend: BackendSpecific")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegmentLifecycle {
    Running,
    Pausing,
    Paused,
    Finalizing,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MicRuntime {
    Active {
        token: u64,
        device: OpenTarget,
    },
    Switching {
        op_id: u64,
        active_token: Option<u64>,
        candidate_token: u64,
        deadline: Instant,
        from: String,
        to: String,
        manual: bool,
    },
    MicHolding {
        last_good: String,
        reason: CaptureDeviceHoldingReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SegmentControllerSnapshot {
    pub(crate) lifecycle: SegmentLifecycle,
    pub(crate) mic_runtime: MicRuntime,
    pub(crate) session_mic_target: SessionMicTarget,
    pub(crate) latest_registry_generation: u64,
    pub(crate) next_operation_id: u64,
    pub(crate) next_capture_token: u64,
    pub(crate) stale_event_count: u64,
    /// Mic/system operations never mutate this value; Slice C advances it only
    /// when installing a resumed segment.
    pub(crate) live_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MicSwapReport {
    pub(crate) op_id: u64,
    pub(crate) active_token: u64,
    pub(crate) device_uid: Option<String>,
    pub(crate) device_name: String,
    pub(crate) persisted_mode: Option<InputDeviceMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LaneDiagnostics {
    pub(crate) rejected_token_samples: u64,
    pub(crate) overlap_trimmed_frames: u64,
    pub(crate) synthesized_durable_frames: u64,
    pub(crate) synthesized_live_frames: u64,
    pub(crate) max_lane_skew_ms: u64,
    pub(crate) live_silence_debt_at_seal: u64,
    pub(crate) suspend_discontinuities: u64,
    pub(crate) timestamp_regressions: u64,
}

impl LaneDiagnostics {
    fn add_assign(&mut self, other: &Self) {
        self.rejected_token_samples = self
            .rejected_token_samples
            .saturating_add(other.rejected_token_samples);
        self.overlap_trimmed_frames = self
            .overlap_trimmed_frames
            .saturating_add(other.overlap_trimmed_frames);
        self.synthesized_durable_frames = self
            .synthesized_durable_frames
            .saturating_add(other.synthesized_durable_frames);
        self.synthesized_live_frames = self
            .synthesized_live_frames
            .saturating_add(other.synthesized_live_frames);
        self.max_lane_skew_ms = self.max_lane_skew_ms.max(other.max_lane_skew_ms);
        self.live_silence_debt_at_seal = self
            .live_silence_debt_at_seal
            .saturating_add(other.live_silence_debt_at_seal);
        self.suspend_discontinuities = self
            .suspend_discontinuities
            .saturating_add(other.suspend_discontinuities);
        self.timestamp_regressions = self
            .timestamp_regressions
            .saturating_add(other.timestamp_regressions);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SegmentDiagnostics {
    pub(crate) timeline_reusable: bool,
    pub(crate) mic: LaneDiagnostics,
    pub(crate) system: LaneDiagnostics,
    pub(crate) mic_gap_ms_total: u64,
    pub(crate) mic_switch_count: u64,
    pub(crate) stale_controller_events: u64,
}

impl Default for SegmentDiagnostics {
    fn default() -> Self {
        Self {
            timeline_reusable: true,
            mic: LaneDiagnostics::default(),
            system: LaneDiagnostics::default(),
            mic_gap_ms_total: 0,
            mic_switch_count: 0,
            stale_controller_events: 0,
        }
    }
}

impl SegmentDiagnostics {
    pub(crate) fn add_assign(&mut self, other: &Self) {
        self.timeline_reusable &= other.timeline_reusable;
        self.mic.add_assign(&other.mic);
        self.system.add_assign(&other.system);
        self.mic_gap_ms_total = self.mic_gap_ms_total.saturating_add(other.mic_gap_ms_total);
        self.mic_switch_count = self.mic_switch_count.saturating_add(other.mic_switch_count);
        self.stale_controller_events = self
            .stale_controller_events
            .saturating_add(other.stale_controller_events);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SealReport {
    pub(crate) duration_secs: f64,
    pub(crate) timeline_reusable: bool,
    pub(crate) diagnostics: SegmentDiagnostics,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResumePreconditions {
    pub(crate) target: SessionMicTarget,
    pub(crate) ladder: Vec<OpenTarget>,
    pub(crate) mic_holding: bool,
    pub(crate) next_capture_token: u64,
}

#[derive(Clone, Debug)]
pub(crate) enum SegmentControllerEvent {
    MicStreamError { token: u64, reason: String },
    RegistrySnapshot(Arc<DeviceSnapshot>),
    HoldingReprobe,
}

pub(crate) type CaptureEventSink = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SessionCaptureCounters {
    pub(crate) mic_drops: u64,
    pub(crate) spk_drops: u64,
    pub(crate) spk_frames: u64,
    pub(crate) system_audio_seen: bool,
}

pub(crate) struct SegmentControllerStart {
    pub(crate) registry: Arc<DeviceRegistry>,
    pub(crate) segment_id: u64,
    pub(crate) live_generation: u64,
    pub(crate) input_device_mode: InputDeviceMode,
    pub(crate) pinned_uid: Option<String>,
    /// Resume supplies the previous controller's session target so the sticky
    /// PinFallback latch survives segment rotation.
    pub(crate) session_mic_target: Option<SessionMicTarget>,
    /// Session-wide counters already accumulated by earlier paused segments.
    pub(crate) prior_session_counters: SessionCaptureCounters,
    pub(crate) live_audio: Option<recorder::LiveAudioSink>,
    pub(crate) wav_path: PathBuf,
    pub(crate) event_sink: Option<CaptureEventSink>,
    /// Initial capture startup is intentionally open-ended because macOS can
    /// take several seconds to settle a newly-routed device. The UI owns this
    /// token so the user can end that wait explicitly without a false timeout.
    pub(crate) startup_cancel: Option<Arc<AtomicBool>>,
}

pub(crate) struct SegmentControllerRuntime {
    pub(crate) client: SegmentControllerClient,
    pub(crate) snapshot: SegmentControllerSnapshot,
    pub(crate) telemetry: SegmentControllerTelemetry,
}

#[derive(Clone)]
pub(crate) struct SegmentControllerTelemetry {
    pub(crate) mic_peak: Arc<AtomicU32>,
    pub(crate) mic_frames: Arc<AtomicU64>,
    pub(crate) spk_peak: Arc<AtomicU32>,
    pub(crate) mic_drops: Arc<AtomicU64>,
    pub(crate) spk_drops: Arc<AtomicU64>,
    pub(crate) spk_silence: Arc<AtomicU64>,
    pub(crate) spk_frames: Arc<AtomicU64>,
    pub(crate) spk_health_frames: Arc<AtomicU64>,
    pub(crate) spk_rate: Arc<AtomicU32>,
    pub(crate) system_audio_seen: Arc<AtomicBool>,
    pub(crate) system_health_seen: Arc<AtomicBool>,
    pub(crate) timeline_reusable: Arc<AtomicBool>,
    pub(crate) mic_gap_ms_total: Arc<AtomicU64>,
    pub(crate) mic_switch_count: Arc<AtomicU64>,
    pub(crate) mic_rejected_token_samples: Arc<AtomicU64>,
    pub(crate) system_rejected_token_samples: Arc<AtomicU64>,
    pub(crate) mic_overlap_trimmed_frames: Arc<AtomicU64>,
    pub(crate) system_overlap_trimmed_frames: Arc<AtomicU64>,
    pub(crate) mic_synthesized_durable_frames: Arc<AtomicU64>,
    pub(crate) system_synthesized_durable_frames: Arc<AtomicU64>,
    pub(crate) mic_synthesized_live_frames: Arc<AtomicU64>,
    pub(crate) system_synthesized_live_frames: Arc<AtomicU64>,
    pub(crate) mic_max_lane_skew_ms: Arc<AtomicU64>,
    pub(crate) system_max_lane_skew_ms: Arc<AtomicU64>,
    pub(crate) mic_live_silence_debt_at_seal: Arc<AtomicU64>,
    pub(crate) system_live_silence_debt_at_seal: Arc<AtomicU64>,
    pub(crate) mic_suspend_discontinuities: Arc<AtomicU64>,
    pub(crate) system_suspend_discontinuities: Arc<AtomicU64>,
    pub(crate) mic_timestamp_regressions: Arc<AtomicU64>,
    pub(crate) system_timestamp_regressions: Arc<AtomicU64>,
}

impl Default for SegmentControllerTelemetry {
    fn default() -> Self {
        Self {
            mic_peak: Arc::new(AtomicU32::new(0)),
            mic_frames: Arc::new(AtomicU64::new(0)),
            spk_peak: Arc::new(AtomicU32::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            spk_drops: Arc::new(AtomicU64::new(0)),
            spk_silence: Arc::new(AtomicU64::new(0)),
            spk_frames: Arc::new(AtomicU64::new(0)),
            spk_health_frames: Arc::new(AtomicU64::new(0)),
            spk_rate: Arc::new(AtomicU32::new(48_000)),
            system_audio_seen: Arc::new(AtomicBool::new(false)),
            system_health_seen: Arc::new(AtomicBool::new(false)),
            timeline_reusable: Arc::new(AtomicBool::new(true)),
            mic_gap_ms_total: Arc::new(AtomicU64::new(0)),
            mic_switch_count: Arc::new(AtomicU64::new(0)),
            mic_rejected_token_samples: Arc::new(AtomicU64::new(0)),
            system_rejected_token_samples: Arc::new(AtomicU64::new(0)),
            mic_overlap_trimmed_frames: Arc::new(AtomicU64::new(0)),
            system_overlap_trimmed_frames: Arc::new(AtomicU64::new(0)),
            mic_synthesized_durable_frames: Arc::new(AtomicU64::new(0)),
            system_synthesized_durable_frames: Arc::new(AtomicU64::new(0)),
            mic_synthesized_live_frames: Arc::new(AtomicU64::new(0)),
            system_synthesized_live_frames: Arc::new(AtomicU64::new(0)),
            mic_max_lane_skew_ms: Arc::new(AtomicU64::new(0)),
            system_max_lane_skew_ms: Arc::new(AtomicU64::new(0)),
            mic_live_silence_debt_at_seal: Arc::new(AtomicU64::new(0)),
            system_live_silence_debt_at_seal: Arc::new(AtomicU64::new(0)),
            mic_suspend_discontinuities: Arc::new(AtomicU64::new(0)),
            system_suspend_discontinuities: Arc::new(AtomicU64::new(0)),
            mic_timestamp_regressions: Arc::new(AtomicU64::new(0)),
            system_timestamp_regressions: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl SegmentControllerTelemetry {
    pub(crate) fn diagnostics(&self, stale_controller_events: u64) -> SegmentDiagnostics {
        let lane = |prefix: bool| LaneDiagnostics {
            rejected_token_samples: if prefix {
                self.mic_rejected_token_samples.load(Ordering::Acquire)
            } else {
                self.system_rejected_token_samples.load(Ordering::Acquire)
            },
            overlap_trimmed_frames: if prefix {
                self.mic_overlap_trimmed_frames.load(Ordering::Acquire)
            } else {
                self.system_overlap_trimmed_frames.load(Ordering::Acquire)
            },
            synthesized_durable_frames: if prefix {
                self.mic_synthesized_durable_frames.load(Ordering::Acquire)
            } else {
                self.system_synthesized_durable_frames
                    .load(Ordering::Acquire)
            },
            synthesized_live_frames: if prefix {
                self.mic_synthesized_live_frames.load(Ordering::Acquire)
            } else {
                self.system_synthesized_live_frames.load(Ordering::Acquire)
            },
            max_lane_skew_ms: if prefix {
                self.mic_max_lane_skew_ms.load(Ordering::Acquire)
            } else {
                self.system_max_lane_skew_ms.load(Ordering::Acquire)
            },
            live_silence_debt_at_seal: if prefix {
                self.mic_live_silence_debt_at_seal.load(Ordering::Acquire)
            } else {
                self.system_live_silence_debt_at_seal
                    .load(Ordering::Acquire)
            },
            suspend_discontinuities: if prefix {
                self.mic_suspend_discontinuities.load(Ordering::Acquire)
            } else {
                self.system_suspend_discontinuities.load(Ordering::Acquire)
            },
            timestamp_regressions: if prefix {
                self.mic_timestamp_regressions.load(Ordering::Acquire)
            } else {
                self.system_timestamp_regressions.load(Ordering::Acquire)
            },
        };
        SegmentDiagnostics {
            timeline_reusable: self.timeline_reusable.load(Ordering::Acquire),
            mic: lane(true),
            system: lane(false),
            mic_gap_ms_total: self.mic_gap_ms_total.load(Ordering::Acquire),
            mic_switch_count: self.mic_switch_count.load(Ordering::Acquire),
            stale_controller_events,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SegmentControllerClient {
    tx: mpsc::Sender<ControllerMessage>,
    published: Arc<Mutex<SegmentControllerSnapshot>>,
}

impl SegmentControllerClient {
    pub(crate) fn snapshot(&self) -> SegmentControllerSnapshot {
        self.published.lock().unwrap().clone()
    }

    pub(crate) fn swap_mic(&self, requested_uid: Option<String>) -> Result<MicSwapReport, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::SwapMic {
                requested_uid,
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped mic-swap reply".to_string())?
    }

    pub(crate) fn drop_mic(&self, reason: CaptureDeviceHoldingReason) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::DropMic {
                reason,
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped mic-loss reply".to_string())?
    }

    pub(crate) fn swap_system(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::SwapSystem {
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped system-swap reply".to_string())?
    }

    pub(crate) fn pause_and_seal(&self) -> Result<SealReport, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::Pause {
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped pause reply".to_string())?
    }

    pub(crate) fn resume_preconditions(
        &self,
        snapshot: Arc<DeviceSnapshot>,
    ) -> Result<ResumePreconditions, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(
                ControllerCommand::ResumePreconditions {
                    snapshot,
                    reply: tx,
                },
            ))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped resume reply".to_string())?
    }

    pub(crate) fn finish(&self) -> Result<SealReport, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::Finish {
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped finish reply".to_string())?
    }

    pub(crate) fn discard(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ControllerMessage::Command(ControllerCommand::Discard {
                reply: tx,
            }))
            .map_err(|_| "segment controller stopped".to_string())?;
        rx.recv()
            .map_err(|_| "segment controller dropped discard reply".to_string())?
    }

    pub(crate) fn send_event(&self, event: SegmentControllerEvent) -> Result<(), String> {
        self.tx
            .send(ControllerMessage::Event(event))
            .map_err(|_| "segment controller stopped".to_string())
    }
}

pub(crate) struct SegmentController;

impl SegmentController {
    pub(crate) fn start(
        config: SegmentControllerStart,
    ) -> Result<SegmentControllerRuntime, String> {
        let snapshot = config.registry.snapshot();
        let mode = config.input_device_mode;
        let pinned_uid = config.pinned_uid.clone();
        let initial_target = config
            .session_mic_target
            .clone()
            .unwrap_or_else(|| mic_control::target_from_settings(mode, pinned_uid.as_deref()));
        let live_generation = config.live_generation;
        let prior_session_counters = config.prior_session_counters;
        let event_sink = config.event_sink.clone();
        let startup_cancel = config.startup_cancel.clone();
        Self::start_with_backend(
            initial_target,
            snapshot,
            live_generation,
            prior_session_counters,
            event_sink,
            startup_cancel,
            move || Ok(Box::new(NativeControllerBackend::new(config)) as Box<dyn ControllerBackend>),
        )
    }

    fn start_with_backend(
        initial_target: SessionMicTarget,
        snapshot: Arc<DeviceSnapshot>,
        live_generation: u64,
        prior_session_counters: SessionCaptureCounters,
        event_sink: Option<CaptureEventSink>,
        startup_cancel: Option<Arc<AtomicBool>>,
        factory: impl FnOnce() -> Result<Box<dyn ControllerBackend>, String> + Send + 'static,
    ) -> Result<SegmentControllerRuntime, String> {
        let (tx, rx) = mpsc::channel();
        let (startup_tx, startup_rx) = mpsc::channel();
        let initial_snapshot = SegmentControllerSnapshot {
            lifecycle: SegmentLifecycle::Running,
            mic_runtime: MicRuntime::MicHolding {
                last_good: "Unknown microphone".to_string(),
                reason: CaptureDeviceHoldingReason::OpenFailed,
            },
            session_mic_target: initial_target.clone(),
            latest_registry_generation: snapshot.generation,
            next_operation_id: 1,
            next_capture_token: 1,
            stale_event_count: 0,
            live_generation,
        };
        let published = Arc::new(Mutex::new(initial_snapshot));
        let published_for_actor = published.clone();
        let telemetry = SegmentControllerTelemetry::default();
        let telemetry_for_actor = telemetry.clone();
        if startup_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err("Recording startup cancelled.".to_string());
        }
        std::thread::Builder::new()
            .name("margins-segment-controller".into())
            .spawn(move || {
                let backend = match factory() {
                    Ok(backend) => backend,
                    Err(error) => {
                        let _ = startup_tx.send(Err(error));
                        return;
                    }
                };
                let mut actor = ControllerActor::new(
                    backend,
                    initial_target,
                    snapshot,
                    live_generation,
                    prior_session_counters,
                    event_sink,
                    published_for_actor,
                    telemetry_for_actor,
                );
                match actor.start() {
                    Ok(()) => {
                        let _ = startup_tx.send(Ok(actor.snapshot()));
                        actor.run(rx);
                    }
                    Err(error) => {
                        let _ = startup_tx.send(Err(error));
                    }
                }
            })
            .map_err(|error| format!("could not start segment controller: {error}"))?;
        // Initial capture has a UI cancellation token, so do not put a fixed
        // deadline on macOS device routing. AirPods and aggregate routes can
        // legitimately finish after the old five-second cutoff. Internal
        // callers without a cancel surface (currently resume) retain the old
        // bounded behavior instead of becoming an uninterruptible wait.
        let uncancellable_deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if startup_cancel
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
            {
                return Err("Recording startup cancelled.".to_string());
            }
            match startup_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if startup_cancel.is_none() && Instant::now() >= uncancellable_deadline {
                        return Err("timed out starting segment controller".to_string());
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("segment controller stopped during startup".to_string());
                }
            }
        };
        Ok(SegmentControllerRuntime {
            client: SegmentControllerClient { tx, published },
            snapshot,
            telemetry,
        })
    }
}

enum ControllerMessage {
    Command(ControllerCommand),
    Event(SegmentControllerEvent),
}

enum ControllerCommand {
    SwapMic {
        requested_uid: Option<String>,
        reply: mpsc::Sender<Result<MicSwapReport, String>>,
    },
    DropMic {
        reason: CaptureDeviceHoldingReason,
        reply: mpsc::Sender<Result<(), String>>,
    },
    SwapSystem {
        reply: mpsc::Sender<Result<(), String>>,
    },
    Pause {
        reply: mpsc::Sender<Result<SealReport, String>>,
    },
    ResumePreconditions {
        snapshot: Arc<DeviceSnapshot>,
        reply: mpsc::Sender<Result<ResumePreconditions, String>>,
    },
    Finish {
        reply: mpsc::Sender<Result<SealReport, String>>,
    },
    Discard {
        reply: mpsc::Sender<Result<(), String>>,
    },
}

trait ControllerBackend {
    fn start(
        &mut self,
        mic_ladder: &[OpenTarget],
        mic_token: u64,
        system_token: u64,
    ) -> Result<Option<OpenTarget>, String>;
    fn begin_mic_candidate(&mut self, target: &OpenTarget, token: u64) -> Result<(), String>;
    fn mic_candidate_ready(&self) -> bool;
    fn commit_mic_candidate(&mut self) -> Result<(), String>;
    fn cancel_mic_candidate(&mut self);
    fn retire_mic(&mut self) -> Result<(), String>;
    fn begin_system_candidate(&mut self, token: u64) -> Result<(), String>;
    fn system_candidate_ready(&self) -> bool;
    fn commit_system_candidate(&mut self) -> Result<(), String>;
    fn cancel_system_candidate(&mut self);
    fn seal(&mut self) -> Result<SealReport, String>;
    fn discard(&mut self) -> Result<(), String>;
    fn writer_failed(&self) -> bool;
    fn take_mic_error(&mut self) -> Option<(u64, String)>;
    fn telemetry(&mut self) -> BackendTelemetry;
    fn active_mic_backend_name(&self) -> Option<&'static str> {
        None
    }
}

#[derive(Default)]
struct BackendTelemetry {
    mic_peak: u32,
    mic_frames: u64,
    spk_peak: u32,
    mic_drops: u64,
    spk_drops: u64,
    spk_silence: u64,
    /// Cumulative across every system capture owned by this segment.
    spk_frames: u64,
    /// Frames from the currently attached system capture only.
    spk_health_frames: u64,
    spk_rate: u32,
    /// Sticky across every system capture owned by this segment.
    system_audio_seen: bool,
    /// Signal observation from the currently attached system capture only.
    system_health_seen: bool,
    timeline_reusable: bool,
    mic_rate: u32,
    mic: LaneDiagnostics,
    system: LaneDiagnostics,
}

fn load_lane_diagnostics(telemetry: &recorder::LaneTelemetry) -> LaneDiagnostics {
    LaneDiagnostics {
        rejected_token_samples: telemetry.rejected_token_samples.load(Ordering::Acquire),
        overlap_trimmed_frames: telemetry.overlap_trimmed_frames.load(Ordering::Acquire),
        synthesized_durable_frames: telemetry.synthesized_durable_frames.load(Ordering::Acquire),
        synthesized_live_frames: telemetry.synthesized_live_frames.load(Ordering::Acquire),
        max_lane_skew_ms: telemetry.max_lane_skew_ms.load(Ordering::Acquire),
        live_silence_debt_at_seal: telemetry.live_silence_debt_at_seal.load(Ordering::Acquire),
        suspend_discontinuities: telemetry.suspend_discontinuities.load(Ordering::Acquire),
        timestamp_regressions: telemetry.timestamp_regressions.load(Ordering::Acquire),
    }
}

struct NativeControllerBackend {
    registry: Arc<DeviceRegistry>,
    segment_id: u64,
    live_audio: Option<recorder::LiveAudioSink>,
    wav_path: PathBuf,
    writer: Option<recorder::SegmentWriter>,
    mic: Option<recorder::MicCapture>,
    mic_candidate: Option<recorder::MicCapture>,
    system: Option<recorder::SystemCapture>,
    system_candidate: Option<recorder::SystemCapture>,
    retired_mic_drops: u64,
    retired_system_drops: u64,
    retired_system_frames: u64,
    retired_system_audio_seen: bool,
    current_system_audio_seen: bool,
    mic_backend_preference: recorder::MicBackendKind,
}

impl NativeControllerBackend {
    fn new(config: SegmentControllerStart) -> Self {
        let mic_backend_preference = match std::env::var("MARGINS_MIC_BACKEND") {
            Ok(value) if value.eq_ignore_ascii_case("coreaudio") => {
                recorder::MicBackendKind::CoreAudio
            }
            _ => recorder::MicBackendKind::Cpal,
        };
        Self {
            registry: config.registry,
            segment_id: config.segment_id,
            live_audio: config.live_audio,
            wav_path: config.wav_path,
            writer: None,
            mic: None,
            mic_candidate: None,
            system: None,
            system_candidate: None,
            retired_mic_drops: 0,
            retired_system_drops: 0,
            retired_system_frames: 0,
            retired_system_audio_seen: false,
            current_system_audio_seen: false,
            mic_backend_preference,
        }
    }

    fn retire_active_mic(&mut self) -> Result<(), String> {
        let Some(mic) = self.mic.take() else {
            return Ok(());
        };
        let telemetry = mic.telemetry();
        let result = mic.retire().map_err(|error| error.to_string());
        self.retired_mic_drops = self.retired_mic_drops.saturating_add(
            telemetry
                .drops
                .load(Ordering::Acquire)
                .saturating_add(telemetry.packet_drops.load(Ordering::Acquire)),
        );
        result.map(|_| ())
    }

    fn retire_active_system(&mut self) -> Result<(), String> {
        let Some(system) = self.system.take() else {
            return Ok(());
        };
        let telemetry = system.telemetry();
        let result = system.retire().map_err(|error| error.to_string());
        let final_peak = f32::from_bits(telemetry.peak.load(Ordering::Acquire));
        self.retired_system_drops = self.retired_system_drops.saturating_add(
            telemetry
                .drops
                .load(Ordering::Acquire)
                .saturating_add(telemetry.packet_drops.load(Ordering::Acquire)),
        );
        self.retired_system_frames = self
            .retired_system_frames
            .saturating_add(telemetry.frames.load(Ordering::Acquire));
        self.retired_system_audio_seen |=
            self.current_system_audio_seen || final_peak >= SYSTEM_AUDIO_SEEN_THRESHOLD;
        self.current_system_audio_seen = false;
        result.map(|_| ())
    }

    fn start_mic(
        &mut self,
        target: &OpenTarget,
        sink: recorder::MicSink,
        standby: bool,
    ) -> Result<recorder::MicCapture, String> {
        let device = self.registry.resolve_live_device(target.uid.as_deref())?;
        let target = recorder::MicTarget::from_optional(device.as_ref())
            .with_stable_uid(target.resolved_uid.clone());
        let preferred = self.mic_backend_preference;
        let capture = if standby {
            recorder::MicCapture::start_standby_preferred(&target, sink, preferred)
        } else {
            recorder::MicCapture::start_preferred(&target, sink, preferred)
        }
        .map_err(|e| e.to_string())?;
        self.mic_backend_preference = capture.backend();
        Ok(capture)
    }
}

impl ControllerBackend for NativeControllerBackend {
    fn start(
        &mut self,
        mic_ladder: &[OpenTarget],
        mic_token: u64,
        system_token: u64,
    ) -> Result<Option<OpenTarget>, String> {
        let writer =
            recorder::SegmentWriter::start_deferred(self.segment_id, self.live_audio.take())
                .map_err(|error| error.to_string())?;
        let system = recorder::SystemCapture::start(writer.system_sink(system_token))
            .map_err(|error| format!("Could not start system audio capture: {error}"))?;
        let system_rate = system.native_rate();
        self.system = Some(system);
        self.writer = Some(writer);
        for target in mic_ladder {
            let sink = self.writer.as_ref().unwrap().mic_sink(mic_token);
            if let Ok(mic) = self.start_mic(target, sink, false) {
                self.mic = Some(mic);
                return Ok(Some(target.clone()));
            }
        }
        self.writer
            .as_ref()
            .unwrap()
            .set_unattached_lane_rate(recorder::CaptureLane::Mic, system_rate)
            .map_err(|error| error.to_string())?;
        Ok(None)
    }

    fn begin_mic_candidate(&mut self, target: &OpenTarget, token: u64) -> Result<(), String> {
        let sink = self
            .writer
            .as_ref()
            .ok_or("segment writer is not running")?
            .mic_sink(token);
        self.mic_candidate = Some(self.start_mic(target, sink, true)?);
        Ok(())
    }

    fn mic_candidate_ready(&self) -> bool {
        self.mic_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.first_packet_seen().load(Ordering::Acquire))
    }

    fn commit_mic_candidate(&mut self) -> Result<(), String> {
        if self.mic.is_some() {
            // retire() returns only after the lane actor's ACK. The standby
            // purge fence/Attach cannot happen before this line completes.
            self.retire_active_mic()?;
        }
        let candidate = self
            .mic_candidate
            .take()
            .ok_or("microphone candidate is missing")?;
        candidate
            .commit_standby()
            .map_err(|error| error.to_string())?;
        self.mic = Some(candidate);
        Ok(())
    }

    fn cancel_mic_candidate(&mut self) {
        self.mic_candidate.take();
    }

    fn retire_mic(&mut self) -> Result<(), String> {
        self.retire_active_mic()
    }

    fn begin_system_candidate(&mut self, token: u64) -> Result<(), String> {
        let writer = self
            .writer
            .as_ref()
            .ok_or("segment writer is not running")?;
        self.system_candidate = Some(
            recorder::SystemCapture::start_standby(writer.system_sink(token))
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    fn system_candidate_ready(&self) -> bool {
        self.system_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.first_packet_seen().load(Ordering::Acquire))
    }

    fn commit_system_candidate(&mut self) -> Result<(), String> {
        if self.system.is_some() {
            self.retire_active_system()?;
        }
        let candidate = self
            .system_candidate
            .take()
            .ok_or("system candidate is missing")?;
        candidate
            .commit_standby()
            .map_err(|error| error.to_string())?;
        self.system = Some(candidate);
        self.current_system_audio_seen = false;
        Ok(())
    }

    fn cancel_system_candidate(&mut self) {
        self.system_candidate.take();
    }

    fn seal(&mut self) -> Result<SealReport, String> {
        self.cancel_mic_candidate();
        self.cancel_system_candidate();
        self.retire_mic()?;
        self.retire_active_system()?;
        let writer = self.writer.take().ok_or("segment writer already sealed")?;
        let telemetry = writer.telemetry();
        let mic_rate = writer.mic_lane_rate().max(1);
        let sealed = writer.seal().map_err(|error| error.to_string())?;
        let timeline_reusable = sealed.timeline_reusable;
        let duration_secs = sealed
            .write_wav(&self.wav_path)
            .map_err(|error| error.to_string())?;
        let mic = load_lane_diagnostics(&telemetry.mic);
        let system = load_lane_diagnostics(&telemetry.system);
        Ok(SealReport {
            duration_secs,
            timeline_reusable,
            diagnostics: SegmentDiagnostics {
                timeline_reusable,
                mic_gap_ms_total: mic.synthesized_durable_frames.saturating_mul(1_000)
                    / u64::from(mic_rate),
                mic,
                system,
                ..SegmentDiagnostics::default()
            },
        })
    }

    fn discard(&mut self) -> Result<(), String> {
        self.cancel_mic_candidate();
        self.cancel_system_candidate();
        self.retire_mic()?;
        self.retire_active_system()?;
        self.writer.take();
        Ok(())
    }

    fn writer_failed(&self) -> bool {
        self.writer
            .as_ref()
            .is_some_and(|writer| writer.telemetry().writer_failed.load(Ordering::Acquire))
    }

    fn take_mic_error(&mut self) -> Option<(u64, String)> {
        if let Some(capture) = self.mic_candidate.as_mut() {
            if let Some(error) = capture.take_error(true) {
                let backend = capture.backend();
                if matches!(
                    error,
                    recorder::MicStreamErrorKind::BackendSpecific
                        | recorder::MicStreamErrorKind::Stalled
                ) {
                    self.mic_backend_preference = backend.alternate();
                }
                return Some((
                    capture.token(),
                    format!("{} microphone backend: {error:?}", backend.name()),
                ));
            }
        }
        if let Some(capture) = self.mic.as_mut() {
            if let Some(error) = capture.take_error(true) {
                let backend = capture.backend();
                if matches!(
                    error,
                    recorder::MicStreamErrorKind::BackendSpecific
                        | recorder::MicStreamErrorKind::Stalled
                ) {
                    self.mic_backend_preference = backend.alternate();
                }
                return Some((
                    capture.token(),
                    format!("{} microphone backend: {error:?}", backend.name()),
                ));
            }
        }
        None
    }

    fn active_mic_backend_name(&self) -> Option<&'static str> {
        self.mic.as_ref().map(|capture| capture.backend().name())
    }

    fn telemetry(&mut self) -> BackendTelemetry {
        let writer_telemetry = self.writer.as_ref().map(|writer| writer.telemetry());
        let mut values = BackendTelemetry {
            spk_rate: self
                .system
                .as_ref()
                .map(recorder::SystemCapture::native_rate)
                .unwrap_or(48_000),
            timeline_reusable: self
                .writer
                .as_ref()
                .is_none_or(|writer| writer.telemetry().timeline_reusable.load(Ordering::Acquire)),
            mic_rate: self
                .writer
                .as_ref()
                .map(recorder::SegmentWriter::mic_lane_rate)
                .unwrap_or(48_000),
            mic: writer_telemetry
                .as_ref()
                .map(|telemetry| load_lane_diagnostics(&telemetry.mic))
                .unwrap_or_default(),
            system: writer_telemetry
                .as_ref()
                .map(|telemetry| load_lane_diagnostics(&telemetry.system))
                .unwrap_or_default(),
            ..BackendTelemetry::default()
        };
        values.mic_drops = self.retired_mic_drops;
        if let Some(mic) = &self.mic {
            let telemetry = mic.telemetry();
            values.mic_peak = telemetry.peak.swap(0, Ordering::Relaxed);
            values.mic_frames = telemetry.frames.load(Ordering::Relaxed);
            values.mic_drops = values.mic_drops.saturating_add(
                telemetry
                    .drops
                    .load(Ordering::Relaxed)
                    .saturating_add(telemetry.packet_drops.load(Ordering::Relaxed)),
            );
        }
        values.spk_drops = self.retired_system_drops;
        values.spk_frames = self.retired_system_frames;
        values.system_audio_seen = self.retired_system_audio_seen;
        if let Some(system) = &self.system {
            let telemetry = system.telemetry();
            values.spk_peak = telemetry.peak.swap(0, Ordering::Relaxed);
            self.current_system_audio_seen |=
                f32::from_bits(values.spk_peak) >= SYSTEM_AUDIO_SEEN_THRESHOLD;
            values.spk_drops = values.spk_drops.saturating_add(
                telemetry
                    .drops
                    .load(Ordering::Relaxed)
                    .saturating_add(telemetry.packet_drops.load(Ordering::Relaxed)),
            );
            values.spk_silence = telemetry.silence.load(Ordering::Relaxed);
            values.spk_health_frames = telemetry.frames.load(Ordering::Relaxed);
            values.spk_frames = values.spk_frames.saturating_add(values.spk_health_frames);
            values.system_health_seen = self.current_system_audio_seen;
            values.system_audio_seen |= self.current_system_audio_seen;
        }
        values
    }
}

struct PendingMic {
    op_id: u64,
    token: u64,
    targets: Vec<OpenTarget>,
    target_index: usize,
    backend_retry_count: u8,
    deadline: Instant,
    started_at: Instant,
    manual: bool,
    old_usable: bool,
    cause: SwapCause,
    previous_target: Option<SessionMicTarget>,
    reply: Option<mpsc::Sender<Result<MicSwapReport, String>>>,
}

struct PendingSystem {
    token: u64,
    deadline: Instant,
    started_at: Instant,
    reply: mpsc::Sender<Result<(), String>>,
}

struct ControllerActor {
    backend: Box<dyn ControllerBackend>,
    policy: mic_control::PolicyState,
    latest_snapshot: Arc<DeviceSnapshot>,
    lifecycle: SegmentLifecycle,
    mic_runtime: MicRuntime,
    active_mic_token: Option<u64>,
    system_token: u64,
    next_operation_id: u64,
    next_capture_token: u64,
    stale_event_count: u64,
    mic_switch_count: u64,
    live_generation: u64,
    prior_session_counters: SessionCaptureCounters,
    pending_mic: Option<PendingMic>,
    pending_system: Option<PendingSystem>,
    event_sink: Option<CaptureEventSink>,
    published: Arc<Mutex<SegmentControllerSnapshot>>,
    telemetry: SegmentControllerTelemetry,
    should_exit: bool,
    sealed_report: Option<SealReport>,
    holding_reprobe_at: Instant,
    holding_started_at: Option<Instant>,
    holding_reason: Option<CaptureDeviceHoldingReason>,
    status_tick_at: Instant,
}

impl ControllerActor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        backend: Box<dyn ControllerBackend>,
        initial_target: SessionMicTarget,
        snapshot: Arc<DeviceSnapshot>,
        live_generation: u64,
        prior_session_counters: SessionCaptureCounters,
        event_sink: Option<CaptureEventSink>,
        published: Arc<Mutex<SegmentControllerSnapshot>>,
        telemetry: SegmentControllerTelemetry,
    ) -> Self {
        telemetry
            .mic_drops
            .store(prior_session_counters.mic_drops, Ordering::Release);
        telemetry
            .spk_drops
            .store(prior_session_counters.spk_drops, Ordering::Release);
        telemetry
            .spk_frames
            .store(prior_session_counters.spk_frames, Ordering::Release);
        telemetry
            .system_audio_seen
            .store(prior_session_counters.system_audio_seen, Ordering::Release);
        Self {
            backend,
            policy: mic_control::PolicyState::new(initial_target),
            latest_snapshot: snapshot,
            lifecycle: SegmentLifecycle::Running,
            mic_runtime: MicRuntime::MicHolding {
                last_good: "Unknown microphone".into(),
                reason: CaptureDeviceHoldingReason::OpenFailed,
            },
            active_mic_token: None,
            system_token: 0,
            next_operation_id: 1,
            next_capture_token: 1,
            stale_event_count: 0,
            mic_switch_count: 0,
            live_generation,
            prior_session_counters,
            pending_mic: None,
            pending_system: None,
            event_sink,
            published,
            telemetry,
            should_exit: false,
            sealed_report: None,
            holding_reprobe_at: Instant::now() + Duration::from_secs(5),
            holding_started_at: None,
            holding_reason: None,
            status_tick_at: Instant::now(),
        }
    }

    fn start(&mut self) -> Result<(), String> {
        let resume_pin_fallback = match &self.policy.target {
            SessionMicTarget::PinFallback { pinned_uid } => Some(pinned_uid.clone()),
            _ => None,
        };
        let mode = match self.policy.target {
            SessionMicTarget::Follow => InputDeviceMode::FollowDefault,
            SessionMicTarget::Pin { .. } | SessionMicTarget::PinFallback { .. } => {
                InputDeviceMode::Pinned
            }
        };
        let pinned_uid = match &self.policy.target {
            SessionMicTarget::Pin { uid } | SessionMicTarget::PinFallback { pinned_uid: uid } => {
                Some(uid.clone())
            }
            SessionMicTarget::Follow => None,
        };
        let mut transition = mic_control::reduce(
            &self.policy,
            PolicyEvent::Start {
                // A PinFallback resume deliberately resolves the default
                // ladder. Replaying it as Pinned would restore the vanished
                // pin in the same session, violating A11.
                mode: if resume_pin_fallback.is_some() {
                    InputDeviceMode::FollowDefault
                } else {
                    mode
                },
                pinned_uid: if resume_pin_fallback.is_some() {
                    None
                } else {
                    pinned_uid
                },
                snapshot: (*self.latest_snapshot).clone(),
            },
        );
        if let Some(pinned_uid) = resume_pin_fallback {
            transition.next.target = SessionMicTarget::PinFallback { pinned_uid };
        }
        self.policy = transition.next;
        let ladder = transition
            .actions
            .iter()
            .find_map(|action| match action {
                PolicyAction::BeginMicCandidate { ladder, .. } => Some(ladder.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let mic_token = self.take_capture_token();
        let system_token = self.take_capture_token();
        self.system_token = system_token;
        let initial_mic = self.backend.start(&ladder, mic_token, system_token)?;
        self.emit_trace(serde_json::json!({
            "kind": "mic_backend",
            "backend": self.backend.active_mic_backend_name().unwrap_or("unavailable"),
            "reason": "desktop_segment_controller",
            "device_uid": initial_mic.as_ref().and_then(|device| device.uid.clone()),
            "device_name": initial_mic.as_ref().map(|device| device.name.clone()),
            "selection_mode": match self.policy.target {
                SessionMicTarget::Follow => "follow_default",
                SessionMicTarget::Pin { .. } => "pinned",
                SessionMicTarget::PinFallback { .. } => "pinned_fallback",
            },
        }));
        match initial_mic {
            Some(device) => {
                self.active_mic_token = Some(mic_token);
                self.mic_runtime = MicRuntime::Active {
                    token: mic_token,
                    device: device.clone(),
                };
                self.policy = mic_control::reduce(
                    &self.policy,
                    PolicyEvent::CandidateCommitted(device.clone()),
                )
                .next;
                if matches!(self.policy.target, SessionMicTarget::PinFallback { .. }) {
                    self.emit_fallback(&device);
                }
                self.emit_state(CaptureDeviceState::active(device.name));
            }
            None => self.enter_holding(CaptureDeviceHoldingReason::OpenFailed),
        }
        self.publish();
        Ok(())
    }

    fn run(&mut self, rx: mpsc::Receiver<ControllerMessage>) {
        while !self.should_exit {
            match rx.recv_timeout(CONTROLLER_POLL_INTERVAL) {
                Ok(ControllerMessage::Command(command)) => self.handle_command(command),
                Ok(ControllerMessage::Event(event)) => self.handle_event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.backend.discard();
                    break;
                }
            }
            self.poll_candidates();
            if Instant::now() >= self.status_tick_at {
                self.copy_telemetry();
                self.status_tick_at = Instant::now() + Duration::from_millis(100);
            }
            if let Some((token, reason)) = self.backend.take_mic_error() {
                self.handle_event(SegmentControllerEvent::MicStreamError { token, reason });
            }
            if self.lifecycle == SegmentLifecycle::Running
                && matches!(self.mic_runtime, MicRuntime::MicHolding { .. })
                && self.pending_mic.is_none()
                && Instant::now() >= self.holding_reprobe_at
            {
                self.holding_reprobe_at = Instant::now() + Duration::from_secs(5);
                self.handle_event(SegmentControllerEvent::HoldingReprobe);
            }
            if self.backend.writer_failed() && self.lifecycle != SegmentLifecycle::Failed {
                self.fail("segment writer failed".to_string());
            }
        }
    }

    fn handle_command(&mut self, command: ControllerCommand) {
        // Commands are serialized here. Terminal and pause operations cancel
        // lower-precedence work before touching capture state.
        match command {
            ControllerCommand::Finish { reply } => {
                self.cancel_all_candidates("recording finished");
                let result = self.seal_terminal();
                let _ = reply.send(result);
                self.should_exit = true;
            }
            ControllerCommand::Discard { reply } => {
                self.cancel_all_candidates("recording discarded");
                self.lifecycle = SegmentLifecycle::Finalizing;
                let result = self.backend.discard();
                self.publish();
                let _ = reply.send(result);
                self.should_exit = true;
            }
            ControllerCommand::Pause { reply } => {
                if let Some(report) = self.sealed_report.clone() {
                    let _ = reply.send(Ok(report));
                    return;
                }
                self.cancel_all_candidates("recording paused");
                self.lifecycle = SegmentLifecycle::Pausing;
                self.publish();
                let result = self
                    .backend
                    .seal()
                    .map(|report| self.enrich_seal_report(report));
                let final_capture = self.backend.telemetry();
                self.publish_capture_session_counters(&final_capture);
                match &result {
                    Ok(report) => {
                        self.lifecycle = SegmentLifecycle::Paused;
                        self.sealed_report = Some(report.clone());
                        self.active_mic_token = None;
                    }
                    Err(error) => self.fail(error.clone()),
                }
                self.publish();
                let _ = reply.send(result);
            }
            ControllerCommand::SwapMic {
                requested_uid,
                reply,
            } => {
                if self.lifecycle != SegmentLifecycle::Running {
                    let _ = reply.send(Err("capture is not running".into()));
                    return;
                }
                self.cancel_pending_mic("superseded by manual selection");
                let previous_target = self.policy.target.clone();
                let transition = mic_control::reduce(
                    &self.policy,
                    PolicyEvent::ManualSelect {
                        uid: requested_uid,
                        snapshot: (*self.latest_snapshot).clone(),
                    },
                );
                self.policy = transition.next;
                let ladder = transition
                    .actions
                    .into_iter()
                    .find_map(|action| match action {
                        PolicyAction::BeginMicCandidate { ladder, .. } => Some(ladder),
                        _ => None,
                    });
                match ladder {
                    Some(ladder) if !ladder.is_empty() => {
                        self.begin_mic_operation(
                            ladder,
                            true,
                            self.active_mic_token.is_some(),
                            SwapCause::Manual,
                            Some(previous_target),
                            Some(reply),
                        );
                    }
                    _ => {
                        self.policy.target = previous_target;
                        let _ = reply.send(Err("requested microphone is unavailable".into()));
                    }
                }
            }
            ControllerCommand::DropMic { reason, reply } => {
                if self.lifecycle != SegmentLifecycle::Running {
                    let _ = reply.send(Err("capture is not running".into()));
                    return;
                }
                self.cancel_pending_mic("active microphone dropped");
                let result = self.backend.retire_mic();
                if result.is_ok() {
                    self.active_mic_token = None;
                    let transition = mic_control::reduce(
                        &self.policy,
                        PolicyEvent::ActiveCaptureFailed((*self.latest_snapshot).clone()),
                    );
                    self.policy = transition.next;
                    let ladder = transition
                        .actions
                        .into_iter()
                        .find_map(|action| match action {
                            PolicyAction::BeginMicCandidate { ladder, .. } => Some(ladder),
                            _ => None,
                        });
                    if let Some(ladder) = ladder.filter(|ladder| !ladder.is_empty()) {
                        self.begin_mic_operation(
                            ladder,
                            false,
                            false,
                            SwapCause::ActiveFailed,
                            None,
                            None,
                        );
                    } else {
                        self.enter_holding(reason);
                    }
                }
                let _ = reply.send(result);
            }
            ControllerCommand::SwapSystem { reply } => {
                if self.lifecycle != SegmentLifecycle::Running || self.pending_system.is_some() {
                    let _ = reply.send(Err("system capture is not available for restart".into()));
                    return;
                }
                self.take_operation_id();
                let token = self.take_capture_token();
                match self.backend.begin_system_candidate(token) {
                    Ok(()) => {
                        self.pending_system = Some(PendingSystem {
                            token,
                            deadline: Instant::now() + CANDIDATE_CONFIRM_TIMEOUT,
                            started_at: Instant::now(),
                            reply,
                        });
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
                self.publish();
            }
            ControllerCommand::ResumePreconditions { snapshot, reply } => {
                if self.lifecycle != SegmentLifecycle::Paused {
                    let _ = reply.send(Err("capture is not paused".into()));
                    return;
                }
                if snapshot.generation > self.latest_snapshot.generation {
                    self.latest_snapshot = snapshot;
                }
                let ladder = match &self.policy.target {
                    SessionMicTarget::Pin { uid }
                        if self.latest_snapshot.device_by_uid(uid).is_some() =>
                    {
                        mic_control::requested_then_default_then_remaining(
                            &self.latest_snapshot,
                            Some(uid),
                        )
                    }
                    // PinFallback intentionally follows default here (A11).
                    _ => mic_control::default_then_remaining(&self.latest_snapshot),
                };
                let _ = reply.send(Ok(ResumePreconditions {
                    target: self.policy.target.clone(),
                    mic_holding: ladder.is_empty(),
                    ladder,
                    next_capture_token: self.next_capture_token,
                }));
            }
        }
    }

    fn handle_event(&mut self, event: SegmentControllerEvent) {
        match event {
            SegmentControllerEvent::RegistrySnapshot(snapshot) => {
                if snapshot.generation <= self.latest_snapshot.generation {
                    self.stale_event_count = self.stale_event_count.saturating_add(1);
                    self.publish();
                    return;
                }
                self.latest_snapshot = snapshot.clone();
                let transition = mic_control::reduce(
                    &self.policy,
                    PolicyEvent::RegistrySnapshot((*snapshot).clone()),
                );
                self.policy = transition.next;
                if self.lifecycle == SegmentLifecycle::Running {
                    if let Some((ladder, cause, drop_first)) = transition
                        .actions
                        .into_iter()
                        .find_map(|action| match action {
                            PolicyAction::BeginMicCandidate {
                                ladder,
                                cause,
                                drop_active_first,
                                ..
                            } => Some((ladder, cause, drop_active_first)),
                            _ => None,
                        })
                    {
                        // Coalesce an automatic B candidate into the newest C
                        // registry intent. Reducer state already names C; keeping
                        // B alive here would let backend and policy diverge.
                        self.cancel_pending_mic("superseded by newer registry snapshot");
                        if drop_first {
                            let _ = self.backend.retire_mic();
                            self.active_mic_token = None;
                        }
                        self.begin_mic_operation(
                            ladder,
                            false,
                            self.active_mic_token.is_some(),
                            cause,
                            None,
                            None,
                        );
                    }
                }
            }
            SegmentControllerEvent::MicStreamError { token, reason } => {
                if self
                    .pending_mic
                    .as_ref()
                    .is_some_and(|pending| pending.token == token)
                {
                    self.fail_current_candidate(reason);
                } else if self.active_mic_token == Some(token) {
                    self.emit_trace(serde_json::json!({
                        "kind": "mic_capture_failed",
                        "token": token,
                        "backend": self.backend.active_mic_backend_name().unwrap_or("unavailable"),
                        "reason": reason.clone(),
                    }));
                    let _ = self.backend.retire_mic();
                    self.active_mic_token = None;
                    if self.backend.mic_candidate_ready()
                        && self.pending_mic.as_ref().is_some_and(|pending| {
                            self.latest_snapshot
                                .device_by_uid(&pending.targets[pending.target_index].resolved_uid)
                                .is_some()
                        })
                    {
                        if let Some(pending) = self.pending_mic.as_mut() {
                            pending.old_usable = false;
                        }
                        self.commit_pending_mic();
                        self.publish();
                        return;
                    }
                    let transition = mic_control::reduce(
                        &self.policy,
                        PolicyEvent::ActiveCaptureFailed((*self.latest_snapshot).clone()),
                    );
                    self.policy = transition.next;
                    if let Some(ladder) =
                        transition
                            .actions
                            .into_iter()
                            .find_map(|action| match action {
                                PolicyAction::BeginMicCandidate { ladder, .. } => Some(ladder),
                                _ => None,
                            })
                    {
                        self.cancel_pending_mic("active capture failed");
                        self.begin_mic_operation(
                            ladder,
                            false,
                            false,
                            SwapCause::ActiveFailed,
                            None,
                            None,
                        );
                    } else {
                        self.enter_holding(CaptureDeviceHoldingReason::DeviceLost);
                    }
                } else {
                    self.stale_event_count = self.stale_event_count.saturating_add(1);
                }
            }
            SegmentControllerEvent::HoldingReprobe => {
                if self.lifecycle == SegmentLifecycle::Running
                    && matches!(self.mic_runtime, MicRuntime::MicHolding { .. })
                    && self.pending_mic.is_none()
                {
                    let ladder = mic_control::holding_recovery_ladder(
                        &self.policy.target,
                        &self.latest_snapshot,
                    );
                    if !ladder.is_empty() {
                        self.begin_mic_operation(
                            ladder,
                            false,
                            false,
                            SwapCause::HoldingRecovery,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        self.publish();
    }

    fn begin_mic_operation(
        &mut self,
        targets: Vec<OpenTarget>,
        manual: bool,
        old_usable: bool,
        cause: SwapCause,
        previous_target: Option<SessionMicTarget>,
        reply: Option<mpsc::Sender<Result<MicSwapReport, String>>>,
    ) {
        if targets.is_empty() {
            self.enter_holding(CaptureDeviceHoldingReason::OpenFailed);
            return;
        }
        let op_id = self.take_operation_id();
        let token = self.take_capture_token();
        let from = self.current_device_name();
        let target = targets[0].clone();
        match self.backend.begin_mic_candidate(&target, token) {
            Ok(()) => {
                self.mic_runtime = MicRuntime::Switching {
                    op_id,
                    active_token: self.active_mic_token,
                    candidate_token: token,
                    deadline: Instant::now() + CANDIDATE_CONFIRM_TIMEOUT,
                    from: from.clone(),
                    to: target.name.clone(),
                    manual,
                };
                self.emit_state(CaptureDeviceState::switching(from, target.name.clone()));
                self.pending_mic = Some(PendingMic {
                    op_id,
                    token,
                    targets,
                    target_index: 0,
                    backend_retry_count: 0,
                    deadline: Instant::now() + CANDIDATE_CONFIRM_TIMEOUT,
                    started_at: Instant::now(),
                    manual,
                    old_usable,
                    cause,
                    previous_target,
                    reply,
                });
            }
            Err(error) => {
                let pending = PendingMic {
                    op_id,
                    token,
                    targets,
                    target_index: 0,
                    backend_retry_count: 0,
                    deadline: Instant::now(),
                    started_at: Instant::now(),
                    manual,
                    old_usable,
                    cause,
                    previous_target,
                    reply,
                };
                self.pending_mic = Some(pending);
                self.fail_current_candidate(error);
            }
        }
        self.publish();
    }

    fn fail_current_candidate(&mut self, error: String) {
        self.backend.cancel_mic_candidate();
        let Some(mut pending) = self.pending_mic.take() else {
            self.stale_event_count = self.stale_event_count.saturating_add(1);
            return;
        };
        // A native stream can open successfully yet never deliver a callback.
        // Retry the same physical device once after the backend flips (CPAL ↔
        // CoreAudio) before treating this as a device-selection failure. This
        // keeps manual switching make-before-break while covering the exact
        // opened-but-all-zero failure mode seen in the retained WAV.
        if pending.backend_retry_count == 0 && mic_error_allows_backend_retry(&error) {
            pending.backend_retry_count = 1;
            pending.token = self.take_capture_token();
            pending.deadline = Instant::now() + CANDIDATE_CONFIRM_TIMEOUT;
            let target = pending.targets[pending.target_index].clone();
            self.emit_trace(serde_json::json!({
                "kind": "mic_backend_retry",
                "device_name": target.name.clone(),
                "reason": error,
            }));
            match self.backend.begin_mic_candidate(&target, pending.token) {
                Ok(()) => {
                    self.mic_runtime = MicRuntime::Switching {
                        op_id: pending.op_id,
                        active_token: self.active_mic_token,
                        candidate_token: pending.token,
                        deadline: pending.deadline,
                        from: self.current_device_name(),
                        to: target.name,
                        manual: pending.manual,
                    };
                    self.pending_mic = Some(pending);
                    self.publish();
                    return;
                }
                Err(retry_error) => {
                    self.pending_mic = Some(pending);
                    self.fail_current_candidate(retry_error);
                    return;
                }
            }
        }
        // Manual selection is all-or-nothing. Automatic recovery walks the
        // finite device ladder once per signal after exhausting both backends.
        if !pending.manual && pending.target_index + 1 < pending.targets.len() {
            pending.target_index += 1;
            pending.backend_retry_count = 0;
            pending.token = self.take_capture_token();
            pending.deadline = Instant::now() + CANDIDATE_CONFIRM_TIMEOUT;
            let target = pending.targets[pending.target_index].clone();
            match self.backend.begin_mic_candidate(&target, pending.token) {
                Ok(()) => {
                    self.mic_runtime = MicRuntime::Switching {
                        op_id: pending.op_id,
                        active_token: self.active_mic_token,
                        candidate_token: pending.token,
                        deadline: pending.deadline,
                        from: self.current_device_name(),
                        to: target.name.clone(),
                        manual: false,
                    };
                    self.pending_mic = Some(pending);
                    self.publish();
                    return;
                }
                Err(next_error) => {
                    self.pending_mic = Some(pending);
                    self.fail_current_candidate(next_error);
                    return;
                }
            }
        }
        let transition = mic_control::reduce(
            &self.policy,
            PolicyEvent::CandidateFailed {
                manual: pending.manual,
                old_usable: pending.old_usable,
                reason: HoldingReason::OpenFailed,
                previous_target: pending.previous_target.clone(),
            },
        );
        self.policy = transition.next;
        if pending.old_usable {
            if let (Some(token), Some(device)) =
                (self.active_mic_token, self.policy_active_device())
            {
                self.mic_runtime = MicRuntime::Active {
                    token,
                    device: device.clone(),
                };
                self.emit_state(CaptureDeviceState::active(device.name));
            }
        } else {
            self.enter_holding(CaptureDeviceHoldingReason::OpenFailed);
        }
        if let Some(reply) = pending.reply.take() {
            let _ = reply.send(Err(error.clone()));
        }
        if pending.manual {
            self.emit_switch_failed(error);
        }
        self.publish();
    }

    fn commit_pending_mic(&mut self) {
        let Some(mut pending) = self.pending_mic.take() else {
            return;
        };
        let target = pending.targets[pending.target_index].clone();
        // A default candidate is represented by uid=None. Re-resolve it at the
        // commit boundary as well as on RegistrySnapshot so an automatic A→B
        // warming→C race can never attach obsolete B.
        let latest_default = self.latest_snapshot.default_device();
        if !pending.manual
            && target.uid.is_none()
            && latest_default.map(|device| device.uid.as_str())
                != Some(target.resolved_uid.as_str())
        {
            self.backend.cancel_mic_candidate();
            let old_usable = pending.old_usable;
            let cause = pending.cause;
            let ladder = mic_control::default_then_remaining(&self.latest_snapshot);
            self.begin_mic_operation(ladder, false, old_usable, cause, None, None);
            return;
        }
        // Re-resolve availability against the newest immutable snapshot.
        if self
            .latest_snapshot
            .device_by_uid(&target.resolved_uid)
            .is_none()
        {
            self.pending_mic = Some(pending);
            self.fail_current_candidate("candidate disappeared before commit".into());
            return;
        }
        match self.backend.commit_mic_candidate() {
            Ok(()) => {
                let gap_ms = pending.started_at.elapsed().as_millis() as u64;
                let from = self.current_device_name();
                let holding_reason = self.holding_reason.take();
                self.active_mic_token = Some(pending.token);
                let transition = mic_control::reduce(
                    &self.policy,
                    PolicyEvent::CandidateCommitted(target.clone()),
                );
                let persist = transition.actions.first().cloned();
                self.policy = transition.next;
                self.mic_runtime = MicRuntime::Active {
                    token: pending.token,
                    device: target.clone(),
                };
                self.mic_switch_count = self.mic_switch_count.saturating_add(1);
                // Publish the retired capture's final loss counters before a
                // synchronous swap caller can read RecordingStatus.
                self.copy_telemetry();
                self.emit_trace(serde_json::json!({
                    "kind": "mic_switch",
                    "from": from,
                    "to": target.name.clone(),
                    "mode": if pending.manual { "manual" } else { "automatic" },
                    "cause": format!("{:?}", pending.cause).to_ascii_lowercase(),
                    "gap_ms": gap_ms,
                    "make_before_break": pending.old_usable,
                    "backend": self.backend.active_mic_backend_name().unwrap_or("unavailable"),
                    "device_uid": target.uid.clone(),
                }));
                if let Some(started) = self.holding_started_at.take() {
                    self.emit_trace(serde_json::json!({
                        "kind": "mic_holding_exited",
                        "reason": match holding_reason {
                            Some(CaptureDeviceHoldingReason::DeviceLost) => "device_lost",
                            Some(CaptureDeviceHoldingReason::OpenFailed) | None => "open_failed",
                        },
                        "device_name": target.name.clone(),
                        "held_ms": started.elapsed().as_millis(),
                    }));
                }
                self.emit_state(CaptureDeviceState::active(target.name.clone()));
                if matches!(pending.cause, SwapCause::ActiveFailed) {
                    if let Some(sink) = &self.event_sink {
                        sink(serde_json::json!({
                            "state": "recovered",
                            "device_name": target.name.clone(),
                            "backend": self.backend.active_mic_backend_name().unwrap_or("unavailable"),
                        }));
                    }
                }
                if matches!(
                    pending.cause,
                    SwapCause::ActiveFailed | SwapCause::HoldingRecovery
                ) && matches!(self.policy.target, SessionMicTarget::PinFallback { .. })
                {
                    self.emit_fallback(&target);
                }
                if let Some(reply) = pending.reply.take() {
                    let persisted_mode = match persist {
                        Some(PolicyAction::PersistFollow) => Some(InputDeviceMode::FollowDefault),
                        Some(PolicyAction::PersistPin { .. }) => Some(InputDeviceMode::Pinned),
                        _ => None,
                    };
                    let _ = reply.send(Ok(MicSwapReport {
                        op_id: pending.op_id,
                        active_token: pending.token,
                        device_uid: target.uid.clone(),
                        device_name: target.name.clone(),
                        persisted_mode,
                    }));
                }
                // A registry snapshot may have arrived while a manual default
                // candidate was warming. Re-evaluate it after the manual
                // commit; a new pin intentionally ignores default changes.
                let latest_default = self
                    .latest_snapshot
                    .default_device()
                    .map(|device| device.uid.as_str());
                if pending.manual
                    && matches!(self.policy.target, SessionMicTarget::Follow)
                    && latest_default != Some(target.resolved_uid.as_str())
                {
                    let ladder = mic_control::default_then_remaining(&self.latest_snapshot);
                    if !ladder.is_empty() {
                        self.begin_mic_operation(
                            ladder,
                            false,
                            true,
                            SwapCause::DefaultChanged,
                            None,
                            None,
                        );
                    }
                }
            }
            Err(error) => {
                self.pending_mic = Some(pending);
                self.fail_current_candidate(error);
                return;
            }
        }
        self.publish();
    }

    fn commit_pending_system(&mut self) {
        let Some(pending) = self.pending_system.take() else {
            return;
        };
        let result = self.backend.commit_system_candidate();
        if result.is_ok() {
            self.system_token = pending.token;
            // Publish retired system counters before replying to the restart.
            self.copy_telemetry();
            // Reset only the replacement lane's health window. Session-wide
            // drops, frames, and seen-signal evidence remain cumulative.
            self.telemetry.spk_peak.store(0, Ordering::Release);
            self.telemetry.spk_silence.store(0, Ordering::Release);
            self.telemetry.spk_health_frames.store(0, Ordering::Release);
            self.telemetry
                .system_health_seen
                .store(false, Ordering::Release);
            self.emit_trace(serde_json::json!({
                "kind": "system_swap",
                "gap_ms": pending.started_at.elapsed().as_millis(),
            }));
        }
        let _ = pending.reply.send(result);
        self.publish();
    }

    fn fail_system_candidate(&mut self, error: String) {
        self.backend.cancel_system_candidate();
        if let Some(pending) = self.pending_system.take() {
            let _ = pending.reply.send(Err(error));
        }
    }

    fn poll_candidates(&mut self) {
        if let Some(pending) = self.pending_mic.as_ref() {
            if self.backend.mic_candidate_ready() {
                self.commit_pending_mic();
            } else if Instant::now() >= pending.deadline {
                self.fail_current_candidate("microphone confirmation timed out".into());
            }
        }
        if let Some(pending) = self.pending_system.as_ref() {
            if self.backend.system_candidate_ready() {
                self.commit_pending_system();
            } else if Instant::now() >= pending.deadline {
                self.fail_system_candidate("system confirmation timed out".into());
            }
        }
    }

    fn cancel_pending_mic(&mut self, reason: &str) {
        self.backend.cancel_mic_candidate();
        if let Some(mut pending) = self.pending_mic.take() {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(reason.to_string()));
            }
        }
    }

    fn cancel_all_candidates(&mut self, reason: &str) {
        self.cancel_pending_mic(reason);
        self.backend.cancel_system_candidate();
        if let Some(pending) = self.pending_system.take() {
            let _ = pending.reply.send(Err(reason.to_string()));
        }
    }

    fn seal_terminal(&mut self) -> Result<SealReport, String> {
        if let Some(report) = self.sealed_report.clone() {
            self.lifecycle = SegmentLifecycle::Finalizing;
            self.publish();
            return Ok(report);
        }
        self.lifecycle = SegmentLifecycle::Finalizing;
        self.publish();
        let result = self
            .backend
            .seal()
            .map(|report| self.enrich_seal_report(report));
        let final_capture = self.backend.telemetry();
        self.publish_capture_session_counters(&final_capture);
        if let Err(error) = &result {
            self.fail(error.clone());
        }
        result
    }

    fn enter_holding(&mut self, reason: CaptureDeviceHoldingReason) {
        let last_good = self.current_device_name();
        self.mic_runtime = MicRuntime::MicHolding {
            last_good: last_good.clone(),
            reason,
        };
        self.policy.runtime = PolicyMicRuntime::MicHolding {
            last_good: None,
            reason: match reason {
                CaptureDeviceHoldingReason::DeviceLost => HoldingReason::DeviceLost,
                CaptureDeviceHoldingReason::OpenFailed => HoldingReason::OpenFailed,
            },
        };
        self.holding_reprobe_at = Instant::now() + Duration::from_secs(5);
        if self.holding_started_at.is_none() {
            self.holding_started_at = Some(Instant::now());
            self.holding_reason = Some(reason);
            self.emit_trace(serde_json::json!({
                "kind": "mic_holding_entered",
                "reason": match reason {
                    CaptureDeviceHoldingReason::DeviceLost => "device_lost",
                    CaptureDeviceHoldingReason::OpenFailed => "open_failed",
                },
                "last_good": last_good.clone(),
                "held_ms": 0,
            }));
        }
        self.emit_state(CaptureDeviceState::holding(last_good, reason));
    }

    fn enrich_seal_report(&self, mut report: SealReport) -> SealReport {
        report.diagnostics.timeline_reusable &= report.timeline_reusable;
        report.diagnostics.mic_switch_count = self.mic_switch_count;
        report.diagnostics.stale_controller_events = self.stale_event_count;
        report
    }

    fn fail(&mut self, _reason: String) {
        self.cancel_all_candidates("segment controller failed");
        let _ = self.backend.discard();
        self.lifecycle = SegmentLifecycle::Failed;
        self.publish();
    }

    fn current_device_name(&self) -> String {
        match &self.mic_runtime {
            MicRuntime::Active { device, .. } => device.name.clone(),
            MicRuntime::Switching { from, .. } => from.clone(),
            MicRuntime::MicHolding { last_good, .. } => last_good.clone(),
        }
    }

    fn policy_active_device(&self) -> Option<OpenTarget> {
        match &self.policy.runtime {
            PolicyMicRuntime::Active(active) => Some(OpenTarget {
                uid: active.uid.clone(),
                resolved_uid: active.resolved_uid.clone(),
                name: active.name.clone(),
            }),
            PolicyMicRuntime::Switching {
                from: Some(active), ..
            } => Some(OpenTarget {
                uid: active.uid.clone(),
                resolved_uid: active.resolved_uid.clone(),
                name: active.name.clone(),
            }),
            _ => None,
        }
    }

    fn take_operation_id(&mut self) -> u64 {
        let id = self.next_operation_id;
        self.next_operation_id = self.next_operation_id.saturating_add(1);
        id
    }

    fn take_capture_token(&mut self) -> u64 {
        let token = self.next_capture_token;
        self.next_capture_token = self.next_capture_token.saturating_add(1);
        token
    }

    fn snapshot(&self) -> SegmentControllerSnapshot {
        SegmentControllerSnapshot {
            lifecycle: self.lifecycle,
            mic_runtime: self.mic_runtime.clone(),
            session_mic_target: self.policy.target.clone(),
            latest_registry_generation: self.latest_snapshot.generation,
            next_operation_id: self.next_operation_id,
            next_capture_token: self.next_capture_token,
            stale_event_count: self.stale_event_count,
            live_generation: self.live_generation,
        }
    }

    fn publish(&self) {
        *self.published.lock().unwrap() = self.snapshot();
    }

    fn copy_telemetry(&mut self) {
        let values = self.backend.telemetry();
        self.telemetry
            .mic_peak
            .store(values.mic_peak, Ordering::Relaxed);
        self.telemetry
            .mic_frames
            .store(values.mic_frames, Ordering::Relaxed);
        self.telemetry
            .spk_peak
            .store(values.spk_peak, Ordering::Relaxed);
        self.publish_capture_session_counters(&values);
        self.telemetry
            .spk_silence
            .store(values.spk_silence, Ordering::Relaxed);
        self.telemetry
            .spk_health_frames
            .store(values.spk_health_frames, Ordering::Relaxed);
        self.telemetry
            .spk_rate
            .store(values.spk_rate.max(1), Ordering::Relaxed);
        self.telemetry
            .timeline_reusable
            .store(values.timeline_reusable, Ordering::Relaxed);
        let mic_gap_ms = values.mic.synthesized_durable_frames.saturating_mul(1_000)
            / u64::from(values.mic_rate.max(1));
        self.telemetry
            .mic_gap_ms_total
            .store(mic_gap_ms, Ordering::Relaxed);
        self.telemetry
            .mic_switch_count
            .store(self.mic_switch_count, Ordering::Relaxed);
        self.telemetry
            .mic_rejected_token_samples
            .store(values.mic.rejected_token_samples, Ordering::Relaxed);
        self.telemetry
            .system_rejected_token_samples
            .store(values.system.rejected_token_samples, Ordering::Relaxed);
        self.telemetry
            .mic_overlap_trimmed_frames
            .store(values.mic.overlap_trimmed_frames, Ordering::Relaxed);
        self.telemetry
            .system_overlap_trimmed_frames
            .store(values.system.overlap_trimmed_frames, Ordering::Relaxed);
        self.telemetry
            .mic_synthesized_durable_frames
            .store(values.mic.synthesized_durable_frames, Ordering::Relaxed);
        self.telemetry
            .system_synthesized_durable_frames
            .store(values.system.synthesized_durable_frames, Ordering::Relaxed);
        self.telemetry
            .mic_synthesized_live_frames
            .store(values.mic.synthesized_live_frames, Ordering::Relaxed);
        self.telemetry
            .system_synthesized_live_frames
            .store(values.system.synthesized_live_frames, Ordering::Relaxed);
        self.telemetry
            .mic_max_lane_skew_ms
            .store(values.mic.max_lane_skew_ms, Ordering::Relaxed);
        self.telemetry
            .system_max_lane_skew_ms
            .store(values.system.max_lane_skew_ms, Ordering::Relaxed);
        self.telemetry
            .mic_live_silence_debt_at_seal
            .store(values.mic.live_silence_debt_at_seal, Ordering::Relaxed);
        self.telemetry
            .system_live_silence_debt_at_seal
            .store(values.system.live_silence_debt_at_seal, Ordering::Relaxed);
        self.telemetry
            .mic_suspend_discontinuities
            .store(values.mic.suspend_discontinuities, Ordering::Relaxed);
        self.telemetry
            .system_suspend_discontinuities
            .store(values.system.suspend_discontinuities, Ordering::Relaxed);
        self.telemetry
            .mic_timestamp_regressions
            .store(values.mic.timestamp_regressions, Ordering::Relaxed);
        self.telemetry
            .system_timestamp_regressions
            .store(values.system.timestamp_regressions, Ordering::Relaxed);
        self.telemetry
            .system_health_seen
            .store(values.system_health_seen, Ordering::Relaxed);
        if values.system_audio_seen
            || f32::from_bits(values.spk_peak) >= SYSTEM_AUDIO_SEEN_THRESHOLD
        {
            self.telemetry
                .system_audio_seen
                .store(true, Ordering::Relaxed);
        }
    }

    fn publish_capture_session_counters(&self, values: &BackendTelemetry) {
        self.telemetry.mic_drops.store(
            self.prior_session_counters
                .mic_drops
                .saturating_add(values.mic_drops),
            Ordering::Relaxed,
        );
        self.telemetry.spk_drops.store(
            self.prior_session_counters
                .spk_drops
                .saturating_add(values.spk_drops),
            Ordering::Relaxed,
        );
        self.telemetry.spk_frames.store(
            self.prior_session_counters
                .spk_frames
                .saturating_add(values.spk_frames),
            Ordering::Relaxed,
        );
        if self.prior_session_counters.system_audio_seen || values.system_audio_seen {
            self.telemetry
                .system_audio_seen
                .store(true, Ordering::Relaxed);
        }
    }

    fn emit_state(&self, state: CaptureDeviceState) {
        if let Some(sink) = &self.event_sink {
            if let Ok(value) = serde_json::to_value(state) {
                sink(value);
            }
        }
    }

    fn emit_fallback(&self, device: &OpenTarget) {
        let requested_uid = match &self.policy.target {
            SessionMicTarget::PinFallback { pinned_uid } => Some(pinned_uid.clone()),
            _ => None,
        };
        self.emit_trace(serde_json::json!({
            "kind": "mic_fallback",
            "pinned_uid": requested_uid.clone(),
            "to": device.name,
        }));
        if let Some(sink) = &self.event_sink {
            sink(serde_json::json!({
                "state": "fallback",
                "requested_uid": requested_uid,
                "device_name": device.name,
            }));
        }
    }

    fn emit_switch_failed(&self, error: String) {
        if let Some(sink) = &self.event_sink {
            sink(serde_json::json!({
                "state": "switch_failed",
                "device_name": self.current_device_name(),
                "error": error,
            }));
        }
    }

    fn emit_trace(&self, event: serde_json::Value) {
        if let Some(sink) = &self.event_sink {
            sink(serde_json::json!({ "trace": event }));
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct WebRecordingRecoveryStatus {
    pub(crate) recording_id: String,
    pub(crate) session_name: String,
    pub(crate) elapsed_secs: f64,
    pub(crate) finalization_error: Option<String>,
    pub(crate) owner_lease_active: bool,
    pub(crate) recovery_phase: crate::web_session::WebRecoveryPhase,
}

#[derive(Serialize)]
pub(crate) struct RecordingStatus {
    pub(crate) is_recording: bool,
    /// True when the active session is paused (segment finalized, resumable).
    pub(crate) paused: bool,
    pub(crate) session_name: Option<String>,
    /// Globally unambiguous, non-secret identity for hosted capture mutations.
    /// Owner capabilities remain separate and are never serialized here.
    pub(crate) web_recording_id: Option<String>,
    /// Stable, oldest-first retained hosted captures. Failed captures are never
    /// selected implicitly by display name.
    pub(crate) web_recoveries: Vec<WebRecordingRecoveryStatus>,
    pub(crate) elapsed_secs: f64,
    pub(crate) input_device_name: Option<String>,
    pub(crate) capture_device: CaptureDeviceState,
    /// Linear microphone peak, or None until the active capture has a real sample.
    pub(crate) mic_level: Option<f32>,
    /// Samples delivered by the currently attached microphone callback.
    pub(crate) mic_audio_frame_count: u64,
    pub(crate) spk_level: f32,
    pub(crate) mic_drop_count: u64,
    pub(crate) spk_drop_count: u64,
    pub(crate) mic_gap_ms_total: u64,
    pub(crate) mic_switch_count: u64,
    pub(crate) timeline_reusable: bool,
    pub(crate) speaker_silence_secs: f64,
    /// True when this session's live transcript mode expects a distinct
    /// system-audio channel. False after switching to mic diarization.
    pub(crate) system_audio_expected: bool,
    /// Total system-audio samples delivered by the native capture callback.
    /// Zero after the connection grace period indicates a tap that opened but
    /// is not delivering audio frames.
    pub(crate) system_audio_frame_count: u64,
    /// True once audible system signal crossed the observation threshold.
    pub(crate) system_audio_observed: bool,
    /// Backwards-compatible alias for system_audio_observed.
    pub(crate) system_audio_seen: bool,
    pub(crate) tap_status: String,
    pub(crate) tap_warning: Option<String>,
    pub(crate) live_transcription_mode: String,
    pub(crate) capture_phase: String,
    /// Hosted-browser transport telemetry. Native capture leaves these fields
    /// unset because its writer and live-ASR paths have different counters.
    pub(crate) webm_chunk_count: Option<u64>,
    pub(crate) webm_bytes: Option<u64>,
    pub(crate) webm_last_received_unix_ms: Option<u64>,
    /// Server-computed age, monotonic within a process and reconstructed from
    /// wall time after restart. Frontends should prefer this over timestamps.
    pub(crate) webm_last_received_age_ms: Option<u64>,
    pub(crate) live_pcm_batch_count: Option<u64>,
    pub(crate) live_pcm_sample_count: Option<u64>,
    pub(crate) live_pcm_last_received_unix_ms: Option<u64>,
    pub(crate) live_pcm_last_received_age_ms: Option<u64>,
    pub(crate) live_pcm_configured: Option<bool>,
    pub(crate) web_transport_server_unix_ms: Option<u64>,
    /// Hosted capture ownership lease. None on native capture and when talking
    /// to a server version that predates browser ownership.
    pub(crate) web_owner_lease_active: Option<bool>,
    pub(crate) web_owner_last_heartbeat_unix_ms: Option<u64>,
    pub(crate) web_owner_heartbeat_age_ms: Option<u64>,
    pub(crate) web_owner_lease_timeout_ms: Option<u64>,
    /// A failed durable finalization leaves the scoped session recoverable so
    /// its owner can retry or explicitly discard it.
    pub(crate) web_finalization_error: Option<String>,
    /// Freshness of the durable live transcript for the active session, if a
    /// checkpoint has been journaled. Lets agent-side hooks detect that their
    /// previously fetched transcript is stale without pulling the transcript.
    pub(crate) transcript_watermark: Option<crate::transcript_store::LiveTranscriptWatermark>,
}

pub(crate) fn capture_state_from_snapshot(
    snapshot: &SegmentControllerSnapshot,
) -> CaptureDeviceState {
    match &snapshot.mic_runtime {
        MicRuntime::Active { device, .. } => CaptureDeviceState::active(device.name.clone()),
        MicRuntime::Switching { from, to, .. } => {
            CaptureDeviceState::switching(from.clone(), to.clone())
        }
        MicRuntime::MicHolding { last_good, reason } => {
            CaptureDeviceState::holding(last_good.clone(), *reason)
        }
    }
}

pub(crate) fn recording_diagnostics(rec: &RecordingState) -> SegmentDiagnostics {
    let mut diagnostics = rec.sealed_diagnostics.clone();
    if !rec.paused {
        let snapshot = rec.segment_controller.snapshot();
        diagnostics.add_assign(
            &rec.segment_telemetry
                .diagnostics(snapshot.stale_event_count),
        );
    }
    diagnostics
}

pub(crate) fn recording_status_from_state(rec: &RecordingState) -> RecordingStatus {
    let elapsed = (Local::now() - rec.start_time).num_milliseconds() as f64 / 1000.0;
    let mic = f32::from_bits(rec.mic_peak.load(Ordering::Relaxed));
    let spk = f32::from_bits(rec.spk_peak.load(Ordering::Relaxed));
    let spk_rate = rec.spk_rate.load(Ordering::Relaxed).max(1) as f64;
    let speaker_silence_secs = rec.spk_silence.load(Ordering::Relaxed) as f64 / spk_rate;
    let system_audio_expected = rec.live_transcription_mode == LiveTranscriptionMode::StereoSplit;
    let system_audio_frame_count = rec.spk_frames.load(Ordering::Relaxed);
    let system_audio_seen = rec.system_audio_seen.load(Ordering::Relaxed);
    let system_health_frame_count = rec.spk_health_frames.load(Ordering::Relaxed);
    let system_health_seen = rec.system_health_seen.load(Ordering::Relaxed);
    let segment_elapsed_secs = rec.segment_started_at.elapsed().as_secs_f64();
    let (tap_status, tap_warning) = classify_tap_status(
        segment_elapsed_secs,
        speaker_silence_secs,
        system_audio_expected,
        system_health_frame_count,
        system_health_seen,
        system_audio_seen,
    );
    let snapshot = rec.segment_controller.snapshot();
    let capture_device = capture_state_from_snapshot(&snapshot);
    // Paused presentation takes precedence over the underlying mic runtime:
    // Holding describes a *running* session with no usable microphone. The
    // controller lifecycle otherwise supplies transient and failure phases so
    // writer failures cannot continue to look like healthy recording.
    let capture_phase = if rec.paused {
        RecordingPhase::Paused
    } else {
        match snapshot.lifecycle {
            SegmentLifecycle::Running => rec.phase,
            SegmentLifecycle::Pausing => RecordingPhase::Pausing,
            SegmentLifecycle::Paused => RecordingPhase::Paused,
            SegmentLifecycle::Finalizing => RecordingPhase::Finalizing,
            SegmentLifecycle::Failed => RecordingPhase::NeedsAttention,
        }
    };
    let diagnostics = recording_diagnostics(rec);
    let input_device_name = match &capture_device {
        CaptureDeviceState::Active { device_name } => Some(device_name.clone()),
        CaptureDeviceState::Switching { from, .. } => Some(from.clone()),
        CaptureDeviceState::Holding { last_good, .. } => Some(last_good.clone()),
    };
    RecordingStatus {
        is_recording: true,
        paused: rec.paused,
        session_name: Some(rec.session_name.clone()),
        web_recording_id: None,
        web_recoveries: Vec::new(),
        elapsed_secs: elapsed,
        input_device_name,
        capture_device,
        mic_level: Some(mic),
        mic_audio_frame_count: rec.mic_frames.load(Ordering::Relaxed),
        spk_level: spk,
        mic_drop_count: rec.mic_drops.load(Ordering::Relaxed),
        spk_drop_count: rec.spk_drops.load(Ordering::Relaxed),
        mic_gap_ms_total: diagnostics.mic_gap_ms_total,
        mic_switch_count: diagnostics.mic_switch_count,
        timeline_reusable: diagnostics.timeline_reusable,
        speaker_silence_secs,
        system_audio_expected,
        system_audio_frame_count,
        system_audio_observed: system_audio_seen,
        system_audio_seen,
        tap_status,
        tap_warning,
        live_transcription_mode: rec.live_transcription_mode.as_str().to_string(),
        capture_phase: capture_phase.as_str().to_string(),
        webm_chunk_count: None,
        webm_bytes: None,
        webm_last_received_unix_ms: None,
        webm_last_received_age_ms: None,
        live_pcm_batch_count: None,
        live_pcm_sample_count: None,
        live_pcm_last_received_unix_ms: None,
        live_pcm_last_received_age_ms: None,
        live_pcm_configured: None,
        web_transport_server_unix_ms: None,
        web_owner_lease_active: None,
        web_owner_last_heartbeat_unix_ms: None,
        web_owner_heartbeat_age_ms: None,
        web_owner_lease_timeout_ms: None,
        web_finalization_error: None,
        transcript_watermark: crate::transcript_store::read_live_transcript_watermark(
            &rec.work_dir.join(".margins"),
            &rec.session_name,
        ),
    }
}

pub(crate) fn classify_tap_status(
    segment_elapsed_secs: f64,
    speaker_silence_secs: f64,
    system_audio_expected: bool,
    system_audio_frame_count: u64,
    system_audio_seen: bool,
    system_audio_ever_seen: bool,
) -> (String, Option<String>) {
    if !system_audio_expected {
        return ("not_expected".to_string(), None);
    }

    if system_audio_frame_count == 0 {
        // Give a grace window before reporting blocked. Use the segment elapsed
        // clock for the initial start; after a restart the session-wide
        // ever-seen flag is true, so we stay "connecting" until frames arrive
        // rather than immediately jumping to "blocked" on a long-running segment.
        if segment_elapsed_secs <= 5.0 || system_audio_ever_seen {
            return ("connecting".to_string(), None);
        }
        return (
            "blocked".to_string(),
            Some(
                "Computer audio is not reaching Margins; open Audio setup to restore permission."
                    .to_string(),
            ),
        );
    }

    if !system_audio_seen {
        if segment_elapsed_secs <= 5.0 {
            return ("connecting".to_string(), None);
        }
        return (
            "quiet".to_string(),
            Some("Computer audio is quiet.".to_string()),
        );
    }

    if speaker_silence_secs <= 5.0 {
        return ("ok".to_string(), None);
    }

    // Silence alone cannot confirm the tap wedged: the tap may simply be
    // recording silence (verified: final artifacts show zero dropped samples
    // even when this path fired). Never escalate to "Interrupted" on silence;
    // stay "silent" (→ Quiet) until signal returns or the user restarts.
    (
        "silent".to_string(),
        Some("Computer audio is quiet.".to_string()),
    )
}

pub(crate) fn idle_recording_status() -> RecordingStatus {
    RecordingStatus {
        is_recording: false,
        paused: false,
        session_name: None,
        web_recording_id: None,
        web_recoveries: Vec::new(),
        elapsed_secs: 0.0,
        input_device_name: None,
        capture_device: CaptureDeviceState::active("System Default"),
        mic_level: None,
        mic_audio_frame_count: 0,
        spk_level: 0.0,
        mic_drop_count: 0,
        spk_drop_count: 0,
        mic_gap_ms_total: 0,
        mic_switch_count: 0,
        timeline_reusable: true,
        speaker_silence_secs: 0.0,
        system_audio_expected: false,
        system_audio_frame_count: 0,
        system_audio_observed: false,
        system_audio_seen: false,
        tap_status: "not_expected".to_string(),
        tap_warning: None,
        live_transcription_mode: LiveTranscriptionMode::StereoSplit.as_str().to_string(),
        capture_phase: "idle".to_string(),
        webm_chunk_count: None,
        webm_bytes: None,
        webm_last_received_unix_ms: None,
        webm_last_received_age_ms: None,
        live_pcm_batch_count: None,
        live_pcm_sample_count: None,
        live_pcm_last_received_unix_ms: None,
        live_pcm_last_received_age_ms: None,
        live_pcm_configured: None,
        web_transport_server_unix_ms: None,
        web_owner_lease_active: None,
        web_owner_last_heartbeat_unix_ms: None,
        web_owner_heartbeat_age_ms: None,
        web_owner_lease_timeout_ms: None,
        web_finalization_error: None,
        transcript_watermark: None,
    }
}

pub(crate) fn export_memo(lines: &[MemoLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if line.text.trim().is_empty() {
            continue;
        }
        if let Some(ordinal) = line.block_ordinal {
            // Block line (clock-stopped): use [block N] prefix instead of a timestamp.
            out.push_str(&format!("[block {}] {}\n", ordinal, line.text));
            continue;
        }
        let created = format_elapsed(line.created_secs);
        let grounding = if line.audio_pending_at_mark {
            " (audio not live yet)"
        } else {
            ""
        };
        if let Some(edited) = line.edited_secs {
            out.push_str(&format!(
                "[{} ~{}]{} {}\n",
                created,
                format_elapsed(edited),
                grounding,
                line.text
            ));
        } else {
            out.push_str(&format!("[{}]{} {}\n", created, grounding, line.text));
        }
    }
    out
}

fn format_elapsed(secs: f64) -> String {
    let total = secs as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_recording_state(phase: RecordingPhase) -> RecordingState {
        let runtime = start_fake_controller(Arc::new(FakeControllerShared::default()));
        let telemetry = runtime.telemetry;
        RecordingState {
            session_name: "test-session".to_string(),
            work_dir: PathBuf::from("/tmp"),
            start_time: Local::now(),
            segment_started_at: Instant::now(),
            segment_index: 0,
            mic_peak: Arc::clone(&telemetry.mic_peak),
            mic_frames: Arc::clone(&telemetry.mic_frames),
            spk_peak: Arc::clone(&telemetry.spk_peak),
            mic_drops: Arc::clone(&telemetry.mic_drops),
            spk_drops: Arc::clone(&telemetry.spk_drops),
            spk_silence: Arc::clone(&telemetry.spk_silence),
            spk_frames: Arc::clone(&telemetry.spk_frames),
            spk_health_frames: Arc::clone(&telemetry.spk_health_frames),
            spk_rate: Arc::clone(&telemetry.spk_rate),
            system_audio_seen: Arc::clone(&telemetry.system_audio_seen),
            system_health_seen: Arc::clone(&telemetry.system_health_seen),
            live_transcription_mode: LiveTranscriptionMode::StereoSplit,
            live_backchannel: None,
            live_generation: 0,
            memo_lines: Vec::new(),
            phase,
            paused: true,
            capture_supervisor_stop: Arc::new(AtomicBool::new(false)),
            capture_supervisor_handle: None,
            last_live_checkpoint_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            next_live_checkpoint_attempt_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
            segment_controller: runtime.client,
            segment_telemetry: telemetry,
            sealed_diagnostics: SegmentDiagnostics::default(),
        }
    }

    #[test]
    fn capture_device_state_serializes_frontend_contract() {
        assert_eq!(
            serde_json::to_value(CaptureDeviceState::active("MacBook Pro Microphone")).unwrap(),
            serde_json::json!({
                "state": "active",
                "device_name": "MacBook Pro Microphone"
            })
        );
        assert_eq!(
            serde_json::to_value(CaptureDeviceState::switching(
                "MacBook Pro Microphone",
                "AirPods Pro"
            ))
            .unwrap(),
            serde_json::json!({
                "state": "switching",
                "from": "MacBook Pro Microphone",
                "to": "AirPods Pro"
            })
        );
        assert_eq!(
            serde_json::to_value(CaptureDeviceState::holding(
                "AirPods Pro",
                CaptureDeviceHoldingReason::DeviceLost
            ))
            .unwrap(),
            serde_json::json!({
                "state": "holding",
                "last_good": "AirPods Pro",
                "reason": "device_lost"
            })
        );
        assert_eq!(
            serde_json::to_value(CaptureDeviceState::holding(
                "AirPods Pro",
                CaptureDeviceHoldingReason::OpenFailed
            ))
            .unwrap(),
            serde_json::json!({
                "state": "holding",
                "last_good": "AirPods Pro",
                "reason": "open_failed"
            })
        );
    }

    #[test]
    fn recording_status_serializes_expected_delivery_and_observation_separately() {
        let mut rec = test_recording_state(RecordingPhase::Recording);
        rec.start_time = Local::now() - chrono::Duration::seconds(6);
        rec.segment_started_at = Instant::now() - Duration::from_secs(6);
        rec.paused = false;

        let value = serde_json::to_value(recording_status_from_state(&rec)).unwrap();
        assert_eq!(value["system_audio_expected"], true);
        assert_eq!(value["system_audio_frame_count"], 0);
        assert_eq!(value["system_audio_observed"], false);
        assert_eq!(value["system_audio_seen"], false);
        assert_eq!(value["mic_gap_ms_total"], 0);
        assert_eq!(value["mic_switch_count"], 0);
        assert_eq!(value["timeline_reusable"], true);
        assert_eq!(value["tap_status"], "blocked");

        rec.live_transcription_mode = LiveTranscriptionMode::MicDiarized;
        let value = serde_json::to_value(recording_status_from_state(&rec)).unwrap();
        assert_eq!(value["system_audio_expected"], false);
        assert_eq!(value["tap_status"], "not_expected");
    }

    #[test]
    fn restarted_segment_gets_a_fresh_no_frames_grace_period() {
        let mut rec = test_recording_state(RecordingPhase::Recording);
        rec.start_time = Local::now() - chrono::Duration::hours(1);
        rec.segment_started_at = Instant::now();
        rec.paused = false;

        let value = serde_json::to_value(recording_status_from_state(&rec)).unwrap();
        assert!(value["elapsed_secs"].as_f64().unwrap() >= 3_599.0);
        assert_eq!(value["system_audio_frame_count"], 0);
        assert_eq!(value["tap_status"], "connecting");
    }

    #[test]
    fn classifies_tap_delivery_and_signal_separately() {
        // Within grace window: connecting
        assert_eq!(
            classify_tap_status(1.0, 0.0, true, 0, false, false).0,
            "connecting"
        );
        // No frames, old segment, no prior session audio: blocked (TCC issue)
        assert_eq!(classify_tap_status(6.0, 0.0, true, 0, false, false).0, "blocked");
        // No frames, old segment, BUT session previously had audio: connecting (restart grace)
        assert_eq!(classify_tap_status(6.0, 0.0, true, 0, false, true).0, "connecting");
        // Frames arriving, no signal seen yet: quiet
        assert_eq!(
            classify_tap_status(6.0, 6.0, true, 48_000, false, false).0,
            "quiet"
        );
        // Frames + recent signal: ok
        assert_eq!(classify_tap_status(6.0, 0.0, true, 48_000, true, false).0, "ok");
        // Frames + signal seen but now silent (5-15s): silent
        assert_eq!(
            classify_tap_status(6.0, 6.0, true, 48_000, true, false).0,
            "silent"
        );
        // Frames + signal seen + extended silence (>15s): still silent, not dead
        // The tap is alive (zero drops proven in artifacts); silence ≠ tap death.
        assert_eq!(
            classify_tap_status(16.0, 16.0, true, 48_000, true, false).0,
            "silent"
        );
        assert_eq!(
            classify_tap_status(16.0, 16.0, false, 0, false, false).0,
            "not_expected"
        );
    }

    #[test]
    fn extended_silence_does_not_show_interrupted() {
        // Tap delivered frames and had signal, then went quiet for >15s.
        // Must stay "silent" (→ Quiet in UI), never escalate to "dead" (→ Interrupted).
        let (status, warning) = classify_tap_status(60.0, 30.0, true, 48_000, true, true);
        assert_eq!(status, "silent");
        assert!(warning.is_some());

        // Same without prior session history — still "silent" not "dead"
        let (status, _) = classify_tap_status(60.0, 30.0, true, 48_000, true, false);
        assert_eq!(status, "silent");
    }

    #[test]
    fn restart_grace_prevents_blocked_flash() {
        // After a tap restart: spk_health_frames=0 but session had prior audio.
        // Must show "connecting" (restart grace), not "blocked" (which implies TCC block).
        let (status, _) = classify_tap_status(300.0, 0.0, true, 0, false, true);
        assert_eq!(status, "connecting");

        // Without prior session audio on a long-running segment: blocked (TCC issue).
        let (status, _) = classify_tap_status(300.0, 0.0, true, 0, false, false);
        assert_eq!(status, "blocked");
    }

    #[test]
    fn exports_non_empty_memo_lines_with_edit_times() {
        let lines = vec![
            MemoLine {
                text: "first".to_string(),
                created_secs: 3.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
            MemoLine {
                text: " ".to_string(),
                created_secs: 4.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
            MemoLine {
                text: "edited".to_string(),
                created_secs: 65.0,
                edited_secs: Some(70.0),
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
            MemoLine {
                text: "startup mark".to_string(),
                created_secs: 5.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: true,
                block_ordinal: None,
            },
        ];

        assert_eq!(
            export_memo(&lines),
            "[00:03] first\n[01:05 ~01:10] edited\n[00:05] (audio not live yet) startup mark\n"
        );
    }

    #[derive(Default)]
    struct FakeControllerShared {
        log: Mutex<Vec<String>>,
        candidate_ready: AtomicBool,
        candidate_open_error: Mutex<Option<String>>,
        mic_stream_error: Mutex<Option<(u64, String)>>,
        mic_candidate_cancel_count: AtomicU64,
        system_candidate_ready: AtomicBool,
        writer_failed: AtomicBool,
        system_start_count: std::sync::atomic::AtomicU64,
        system_identity: std::sync::atomic::AtomicU64,
        seal_count: std::sync::atomic::AtomicU64,
        mic_capture_drops: AtomicU64,
        system_capture_drops: AtomicU64,
        system_capture_frames: AtomicU64,
        system_capture_seen: AtomicBool,
    }

    struct FakeControllerBackend {
        shared: Arc<FakeControllerShared>,
        mic_candidate: bool,
        system_candidate: Option<u64>,
        retired_mic_drops: u64,
        retired_system_drops: u64,
        retired_system_frames: u64,
        retired_system_audio_seen: bool,
    }

    impl FakeControllerBackend {
        fn log(&self, value: &str) {
            self.shared.log.lock().unwrap().push(value.to_string());
        }
    }

    impl ControllerBackend for FakeControllerBackend {
        fn start(
            &mut self,
            mic_ladder: &[OpenTarget],
            _mic_token: u64,
            system_token: u64,
        ) -> Result<Option<OpenTarget>, String> {
            self.shared
                .system_start_count
                .fetch_add(1, Ordering::SeqCst);
            self.shared
                .system_identity
                .store(system_token, Ordering::SeqCst);
            self.log("system:start");
            Ok(mic_ladder.first().cloned())
        }

        fn begin_mic_candidate(&mut self, target: &OpenTarget, _token: u64) -> Result<(), String> {
            if let Some(error) = self.shared.candidate_open_error.lock().unwrap().take() {
                return Err(error);
            }
            self.mic_candidate = true;
            self.log(&format!("mic:candidate:{}", target.resolved_uid));
            Ok(())
        }

        fn mic_candidate_ready(&self) -> bool {
            self.mic_candidate && self.shared.candidate_ready.load(Ordering::SeqCst)
        }

        fn commit_mic_candidate(&mut self) -> Result<(), String> {
            assert!(self.mic_candidate);
            self.log("mic:retire_ack");
            self.retired_mic_drops = self
                .retired_mic_drops
                .saturating_add(self.shared.mic_capture_drops.swap(0, Ordering::SeqCst));
            self.log("mic:attach_after_ack");
            self.mic_candidate = false;
            Ok(())
        }

        fn cancel_mic_candidate(&mut self) {
            if self.mic_candidate {
                self.shared
                    .mic_candidate_cancel_count
                    .fetch_add(1, Ordering::SeqCst);
                self.log("mic:candidate_cancel");
            }
            self.mic_candidate = false;
        }

        fn retire_mic(&mut self) -> Result<(), String> {
            self.log("mic:retire_ack");
            self.retired_mic_drops = self
                .retired_mic_drops
                .saturating_add(self.shared.mic_capture_drops.swap(0, Ordering::SeqCst));
            Ok(())
        }

        fn begin_system_candidate(&mut self, token: u64) -> Result<(), String> {
            self.system_candidate = Some(token);
            self.shared
                .system_start_count
                .fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn system_candidate_ready(&self) -> bool {
            self.system_candidate.is_some()
                && self.shared.system_candidate_ready.load(Ordering::SeqCst)
        }

        fn commit_system_candidate(&mut self) -> Result<(), String> {
            let token = self.system_candidate.take().unwrap();
            self.retired_system_drops = self
                .retired_system_drops
                .saturating_add(self.shared.system_capture_drops.swap(0, Ordering::SeqCst));
            self.retired_system_frames = self
                .retired_system_frames
                .saturating_add(self.shared.system_capture_frames.swap(0, Ordering::SeqCst));
            self.retired_system_audio_seen |= self
                .shared
                .system_capture_seen
                .swap(false, Ordering::SeqCst);
            self.shared.system_identity.store(token, Ordering::SeqCst);
            Ok(())
        }

        fn cancel_system_candidate(&mut self) {
            self.system_candidate = None;
        }

        fn seal(&mut self) -> Result<SealReport, String> {
            self.shared.seal_count.fetch_add(1, Ordering::SeqCst);
            self.log("segment:seal");
            Ok(SealReport {
                duration_secs: 1.25,
                timeline_reusable: true,
                diagnostics: SegmentDiagnostics::default(),
            })
        }

        fn discard(&mut self) -> Result<(), String> {
            self.log("segment:discard");
            Ok(())
        }

        fn writer_failed(&self) -> bool {
            self.shared.writer_failed.load(Ordering::SeqCst)
        }

        fn take_mic_error(&mut self) -> Option<(u64, String)> {
            self.shared.mic_stream_error.lock().unwrap().take()
        }

        fn telemetry(&mut self) -> BackendTelemetry {
            let current_system_frames = self.shared.system_capture_frames.load(Ordering::SeqCst);
            let current_system_seen = self.shared.system_capture_seen.load(Ordering::SeqCst);
            BackendTelemetry {
                mic_drops: self
                    .retired_mic_drops
                    .saturating_add(self.shared.mic_capture_drops.load(Ordering::SeqCst)),
                spk_drops: self
                    .retired_system_drops
                    .saturating_add(self.shared.system_capture_drops.load(Ordering::SeqCst)),
                spk_frames: self
                    .retired_system_frames
                    .saturating_add(current_system_frames),
                spk_health_frames: current_system_frames,
                spk_rate: 48_000,
                system_audio_seen: self.retired_system_audio_seen || current_system_seen,
                system_health_seen: current_system_seen,
                timeline_reusable: true,
                ..BackendTelemetry::default()
            }
        }
    }

    fn controller_device(uid: &str, is_default: bool) -> crate::device_registry::DeviceInfo {
        crate::device_registry::DeviceInfo {
            uid: uid.into(),
            name: uid.to_uppercase(),
            is_default,
            sample_rate: Some(48_000),
        }
    }

    fn controller_snapshot() -> Arc<DeviceSnapshot> {
        Arc::new(DeviceSnapshot {
            generation: 7,
            devices: vec![
                controller_device("old", true),
                controller_device("new", false),
            ],
        })
    }

    fn start_fake_controller(shared: Arc<FakeControllerShared>) -> SegmentControllerRuntime {
        start_fake_controller_with(
            shared,
            SessionMicTarget::Follow,
            controller_snapshot(),
            None,
        )
    }

    fn start_fake_controller_with(
        shared: Arc<FakeControllerShared>,
        target: SessionMicTarget,
        snapshot: Arc<DeviceSnapshot>,
        event_sink: Option<CaptureEventSink>,
    ) -> SegmentControllerRuntime {
        let backend_shared = shared.clone();
        SegmentController::start_with_backend(
            target,
            snapshot,
            11,
            SessionCaptureCounters::default(),
            event_sink,
            None,
            move || {
                Ok(Box::new(FakeControllerBackend {
                    shared: backend_shared,
                    mic_candidate: false,
                    system_candidate: None,
                    retired_mic_drops: 0,
                    retired_system_drops: 0,
                    retired_system_frames: 0,
                    retired_system_audio_seen: false,
                }))
            },
        )
        .unwrap()
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..200 {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("condition did not become true");
    }

    #[test]
    fn controller_startup_wait_is_explicitly_cancellable() {
        let shared = Arc::new(FakeControllerShared::default());
        let backend_shared = Arc::clone(&shared);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_from_ui = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            cancel_from_ui.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let result = SegmentController::start_with_backend(
            SessionMicTarget::Follow,
            controller_snapshot(),
            11,
            SessionCaptureCounters::default(),
            None,
            Some(cancel),
            move || {
                // Simulate a macOS route open that outlives the UI's wait.
                std::thread::sleep(Duration::from_millis(500));
                Ok(Box::new(FakeControllerBackend {
                    shared: backend_shared,
                    mic_candidate: false,
                    system_candidate: None,
                    retired_mic_drops: 0,
                    retired_system_drops: 0,
                    retired_system_frames: 0,
                    retired_system_audio_seen: false,
                }))
            },
        );

        match result {
            Ok(_) => panic!("startup unexpectedly completed after cancellation"),
            Err(error) => assert_eq!(error, "Recording startup cancelled."),
        }
        assert!(started.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn controller_mic_swap_preserves_system_identity_generation_and_ack_order() {
        let shared = Arc::new(FakeControllerShared::default());
        shared.candidate_ready.store(true, Ordering::SeqCst);
        let runtime = start_fake_controller(shared.clone());
        let system_identity = shared.system_identity.load(Ordering::SeqCst);

        let report = runtime.client.swap_mic(Some("new".into())).unwrap();

        assert_eq!(report.device_uid.as_deref(), Some("new"));
        assert_eq!(shared.system_start_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            shared.system_identity.load(Ordering::SeqCst),
            system_identity
        );
        assert_eq!(runtime.client.snapshot().live_generation, 11);
        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 0);
        assert!(runtime.telemetry.timeline_reusable.load(Ordering::SeqCst));
        let log = shared.log.lock().unwrap().clone();
        let retired = log
            .iter()
            .position(|entry| entry == "mic:retire_ack")
            .unwrap();
        let attached = log
            .iter()
            .position(|entry| entry == "mic:attach_after_ack")
            .unwrap();
        assert!(retired < attached);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_follow_coalesces_warming_default_b_into_latest_default_c() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller_with(
            shared.clone(),
            SessionMicTarget::Follow,
            Arc::new(DeviceSnapshot {
                generation: 1,
                devices: vec![controller_device("a", true)],
            }),
            None,
        );

        runtime
            .client
            .send_event(SegmentControllerEvent::RegistrySnapshot(Arc::new(
                DeviceSnapshot {
                    generation: 2,
                    devices: vec![controller_device("a", false), controller_device("b", true)],
                },
            )))
            .unwrap();
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { ref to, .. } if to == "B"
            )
        });

        runtime
            .client
            .send_event(SegmentControllerEvent::RegistrySnapshot(Arc::new(
                DeviceSnapshot {
                    generation: 3,
                    devices: vec![
                        controller_device("a", false),
                        controller_device("b", false),
                        controller_device("c", true),
                    ],
                },
            )))
            .unwrap();
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { ref to, .. } if to == "C"
            )
        });
        shared.candidate_ready.store(true, Ordering::SeqCst);
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Active { ref device, .. } if device.resolved_uid == "c"
            )
        });

        let log = shared.log.lock().unwrap().clone();
        let b = log
            .iter()
            .position(|entry| entry == "mic:candidate:b")
            .unwrap();
        let cancelled = log
            .iter()
            .enumerate()
            .find(|(index, entry)| *index > b && entry.as_str() == "mic:candidate_cancel")
            .map(|(index, _)| index)
            .unwrap();
        let c = log
            .iter()
            .position(|entry| entry == "mic:candidate:c")
            .unwrap();
        assert!(b < cancelled && cancelled < c);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_lossy_mic_then_swap_keeps_session_drops_across_swap() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        shared.mic_capture_drops.store(7, Ordering::SeqCst);
        wait_until(|| runtime.telemetry.mic_drops.load(Ordering::SeqCst) == 7);

        shared.candidate_ready.store(true, Ordering::SeqCst);
        runtime.client.swap_mic(Some("new".into())).unwrap();
        assert_eq!(shared.mic_capture_drops.load(Ordering::SeqCst), 0);
        std::thread::sleep(Duration::from_millis(120));
        let retained_drops = runtime.telemetry.mic_drops.load(Ordering::SeqCst);
        assert_eq!(retained_drops, 7);

        // Capture-side drops are lost before the WAV is written, so the offline
        // pass cannot recover them either — re-transcribing buys nothing. With a
        // full decode and delivery above the catastrophic floor, reuse stands.
        let reasons =
            crate::live_transcript_reuse_rejections(&crate::LiveTranscriptQualification {
                transcript: "captured words",
                mic_audio_end_ms: 1_000,
                system_audio_end_ms: 1_000,
                mic_decoded_until_ms: 1_000,
                system_decoded_until_ms: 1_000,
                system_audio_expected: true,
                system_audio_seen: true,
                terminal_journal_durable: true,
                mic_frame_coverage: Some(1.0),
                system_frame_coverage: Some(1.0),
            });
        assert!(reasons.is_empty(), "unexpected reuse rejections: {reasons:?}");
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_system_restart_preserves_session_seen_drops_and_frames() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        shared.system_capture_drops.store(5, Ordering::SeqCst);
        shared.system_capture_frames.store(96_000, Ordering::SeqCst);
        shared.system_capture_seen.store(true, Ordering::SeqCst);
        wait_until(|| {
            runtime.telemetry.spk_drops.load(Ordering::SeqCst) == 5
                && runtime.telemetry.spk_frames.load(Ordering::SeqCst) == 96_000
                && runtime.telemetry.system_audio_seen.load(Ordering::SeqCst)
        });

        shared.system_candidate_ready.store(true, Ordering::SeqCst);
        runtime.client.swap_system().unwrap();
        std::thread::sleep(Duration::from_millis(120));

        assert_eq!(runtime.telemetry.spk_drops.load(Ordering::SeqCst), 5);
        assert_eq!(runtime.telemetry.spk_frames.load(Ordering::SeqCst), 96_000);
        assert!(runtime.telemetry.system_audio_seen.load(Ordering::SeqCst));
        assert_eq!(
            runtime.telemetry.spk_health_frames.load(Ordering::SeqCst),
            0
        );
        assert!(!runtime.telemetry.system_health_seen.load(Ordering::SeqCst));
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_resume_runtime_starts_with_prior_session_counters() {
        let shared = Arc::new(FakeControllerShared::default());
        let backend_shared = shared.clone();
        let runtime = SegmentController::start_with_backend(
            SessionMicTarget::Follow,
            controller_snapshot(),
            12,
            SessionCaptureCounters {
                mic_drops: 3,
                spk_drops: 4,
                spk_frames: 72_000,
                system_audio_seen: true,
            },
            None,
            None,
            move || {
                Ok(Box::new(FakeControllerBackend {
                    shared: backend_shared,
                    mic_candidate: false,
                    system_candidate: None,
                    retired_mic_drops: 0,
                    retired_system_drops: 0,
                    retired_system_frames: 0,
                    retired_system_audio_seen: false,
                }))
            },
        )
        .unwrap();

        assert_eq!(runtime.telemetry.mic_drops.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.telemetry.spk_drops.load(Ordering::SeqCst), 4);
        assert_eq!(runtime.telemetry.spk_frames.load(Ordering::SeqCst), 72_000);
        assert!(runtime.telemetry.system_audio_seen.load(Ordering::SeqCst));
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_mic_swap_does_not_seal_or_add_session_segment() {
        crate::initialize_test_sqlite_runtime();
        let root = std::env::temp_dir().join(format!(
            "margins-mic-swap-row-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        crate::session::create_session(&margins_dir, "demo", &Local::now(), ".margins/demo.md")
            .unwrap();
        crate::session::add_segment(
            &margins_dir,
            "demo",
            0,
            ".margins/recordings/demo_seg0.wav",
            0,
            None,
        )
        .unwrap();

        let shared = Arc::new(FakeControllerShared::default());
        shared.candidate_ready.store(true, Ordering::SeqCst);
        let runtime = start_fake_controller(shared.clone());
        runtime.client.swap_mic(Some("new".into())).unwrap();

        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 0);
        let meta = crate::session::get_session_meta(&margins_dir, "demo").unwrap();
        assert_eq!(meta.segments.len(), 1);
        assert_eq!(meta.segments[0].segment_index, 0);
        runtime.client.discard().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn controller_manual_candidate_open_error_clears_pending_and_restores_active_old_mic() {
        let shared = Arc::new(FakeControllerShared::default());
        *shared.candidate_open_error.lock().unwrap() = Some("candidate open failed".into());
        let runtime = start_fake_controller(shared.clone());

        let error = runtime.client.swap_mic(Some("new".into())).unwrap_err();

        assert_eq!(error, "candidate open failed");
        assert!(matches!(
            runtime.client.snapshot().mic_runtime,
            MicRuntime::Active { ref device, .. } if device.resolved_uid == "old"
        ));
        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 0);

        // The backend rejected the open before a standby capture existed. A
        // second operation can begin and commit immediately, proving the
        // failed operation left no pending candidate behind.
        shared.candidate_ready.store(true, Ordering::SeqCst);
        let report = runtime.client.swap_mic(Some("new".into())).unwrap();
        assert_eq!(report.device_uid.as_deref(), Some("new"));
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_manual_candidate_stream_error_cancels_and_restores_active_old_mic() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        let swap_client = runtime.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });
        let candidate_token = match runtime.client.snapshot().mic_runtime {
            MicRuntime::Switching {
                candidate_token, ..
            } => candidate_token,
            _ => unreachable!(),
        };
        *shared.mic_stream_error.lock().unwrap() =
            Some((candidate_token, "candidate stream failed".into()));

        let error = swap.join().unwrap().unwrap_err();

        assert_eq!(error, "candidate stream failed");
        assert!(matches!(
            runtime.client.snapshot().mic_runtime,
            MicRuntime::Active { ref device, .. } if device.resolved_uid == "old"
        ));
        assert_eq!(shared.mic_candidate_cancel_count.load(Ordering::SeqCst), 1);
        assert!(shared
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "mic:candidate_cancel"));
        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 0);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_manual_candidate_stall_retries_same_device_before_switching() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        let swap_client = runtime.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });
        let first_candidate_token = match runtime.client.snapshot().mic_runtime {
            MicRuntime::Switching {
                candidate_token, ..
            } => candidate_token,
            _ => unreachable!(),
        };
        *shared.mic_stream_error.lock().unwrap() = Some((
            first_candidate_token,
            "cpal microphone backend: Stalled".into(),
        ));

        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { candidate_token, .. }
                    if candidate_token != first_candidate_token
            )
        });
        shared.candidate_ready.store(true, Ordering::SeqCst);

        let report = swap.join().unwrap().unwrap();
        assert_eq!(report.device_uid.as_deref(), Some("new"));
        assert_eq!(
            shared
                .log
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.as_str() == "mic:candidate:new")
                .count(),
            2
        );
        assert!(mic_error_allows_backend_retry(
            "coreaudio microphone backend: BackendSpecific"
        ));
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_manual_candidate_timeout_cancels_and_restores_active_old_mic() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        let swap_client = runtime.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });

        let error = swap.join().unwrap().unwrap_err();

        assert_eq!(error, "microphone confirmation timed out");
        assert!(matches!(
            runtime.client.snapshot().mic_runtime,
            MicRuntime::Active { ref device, .. } if device.resolved_uid == "old"
        ));
        assert_eq!(shared.mic_candidate_cancel_count.load(Ordering::SeqCst), 1);
        assert!(shared
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "mic:candidate_cancel"));
        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 0);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_pause_cancels_candidate_without_waiting_for_deadline() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        let swap_client = runtime.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });

        let report = runtime.client.pause_and_seal().unwrap();

        assert_eq!(report.duration_secs, 1.25);
        assert_eq!(
            runtime.client.snapshot().lifecycle,
            SegmentLifecycle::Paused
        );
        assert!(swap.join().unwrap().is_err());
        assert!(shared
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "mic:candidate_cancel"));
        runtime.client.finish().unwrap();
    }

    #[test]
    fn controller_stop_cancels_candidate_and_stale_ready_cannot_resurrect() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());
        let swap_client = runtime.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });
        runtime.client.finish().unwrap();
        shared.candidate_ready.store(true, Ordering::SeqCst);

        assert!(swap.join().unwrap().is_err());
        assert_eq!(shared.seal_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.client.snapshot().lifecycle,
            SegmentLifecycle::Finalizing
        );
    }

    #[test]
    fn controller_ignores_stale_tokenized_stream_errors() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared);
        let before = runtime.client.snapshot();

        runtime
            .client
            .send_event(SegmentControllerEvent::MicStreamError {
                token: 99_999,
                reason: "retired callback".into(),
            })
            .unwrap();
        wait_until(|| runtime.client.snapshot().stale_event_count == 1);

        let after = runtime.client.snapshot();
        assert_eq!(after.lifecycle, SegmentLifecycle::Running);
        assert_eq!(after.mic_runtime, before.mic_runtime);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_writer_failure_enters_failed_and_rejects_swaps() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller(shared.clone());

        shared.writer_failed.store(true, Ordering::SeqCst);
        wait_until(|| runtime.client.snapshot().lifecycle == SegmentLifecycle::Failed);

        assert!(runtime.client.swap_mic(Some("new".into())).is_err());
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_pinned_target_ignores_default_only_registry_change() {
        let shared = Arc::new(FakeControllerShared::default());
        let runtime = start_fake_controller_with(
            shared.clone(),
            SessionMicTarget::Pin { uid: "old".into() },
            controller_snapshot(),
            None,
        );
        runtime
            .client
            .send_event(SegmentControllerEvent::RegistrySnapshot(Arc::new(
                DeviceSnapshot {
                    generation: 8,
                    devices: vec![
                        controller_device("old", false),
                        controller_device("new", true),
                    ],
                },
            )))
            .unwrap();
        wait_until(|| runtime.client.snapshot().latest_registry_generation == 8);

        assert!(matches!(
            runtime.client.snapshot().mic_runtime,
            MicRuntime::Active { ref device, .. } if device.resolved_uid == "old"
        ));
        assert_eq!(shared.system_start_count.load(Ordering::SeqCst), 1);
        assert!(!shared
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.starts_with("mic:candidate:")));
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_pin_fallback_latch_survives_pause_and_resume_resolution() {
        let shared = Arc::new(FakeControllerShared::default());
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let event_copy = Arc::clone(&events);
        let runtime = start_fake_controller_with(
            shared,
            SessionMicTarget::Pin { uid: "pin".into() },
            Arc::new(DeviceSnapshot {
                generation: 1,
                devices: vec![controller_device("fallback", true)],
            }),
            Some(Arc::new(move |event| {
                event_copy.lock().unwrap().push(event)
            })),
        );
        assert!(matches!(
            runtime.client.snapshot().session_mic_target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| { event["state"] == "fallback" && event["device_name"] == "FALLBACK" }));

        runtime.client.pause_and_seal().unwrap();
        let resume = runtime
            .client
            .resume_preconditions(Arc::new(DeviceSnapshot {
                generation: 2,
                devices: vec![
                    controller_device("pin", false),
                    controller_device("new-default", true),
                ],
            }))
            .unwrap();
        assert!(matches!(
            resume.target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert_eq!(resume.ladder[0].resolved_uid, "new-default");
        runtime.client.finish().unwrap();
    }

    #[test]
    fn controller_mic_loss_falls_back_notifies_and_keeps_latch_on_resume() {
        let shared = Arc::new(FakeControllerShared::default());
        shared.candidate_ready.store(true, Ordering::SeqCst);
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let event_copy = Arc::clone(&events);
        let runtime = start_fake_controller_with(
            shared,
            SessionMicTarget::Pin { uid: "old".into() },
            controller_snapshot(),
            Some(Arc::new(move |event| {
                event_copy.lock().unwrap().push(event)
            })),
        );
        let active_token = match runtime.client.snapshot().mic_runtime {
            MicRuntime::Active { token, .. } => token,
            _ => unreachable!(),
        };

        // A default-only topology change is remembered but does not move a
        // healthy pin. The active-token failure then resolves from that latest
        // snapshot and permanently latches PinFallback for this session.
        runtime
            .client
            .send_event(SegmentControllerEvent::RegistrySnapshot(Arc::new(
                DeviceSnapshot {
                    generation: 8,
                    devices: vec![
                        controller_device("old", false),
                        controller_device("new", true),
                    ],
                },
            )))
            .unwrap();
        wait_until(|| runtime.client.snapshot().latest_registry_generation == 8);
        runtime
            .client
            .send_event(SegmentControllerEvent::MicStreamError {
                token: active_token,
                reason: "device disconnected".into(),
            })
            .unwrap();
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Active { ref device, .. } if device.resolved_uid == "new"
            )
        });
        assert!(matches!(
            runtime.client.snapshot().session_mic_target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["state"] == "fallback" && event["device_name"] == "NEW"));

        runtime.client.pause_and_seal().unwrap();
        let resume = runtime
            .client
            .resume_preconditions(Arc::new(DeviceSnapshot {
                generation: 9,
                devices: vec![
                    controller_device("old", false),
                    controller_device("next-default", true),
                ],
            }))
            .unwrap();
        assert!(matches!(
            resume.target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert_eq!(resume.ladder[0].resolved_uid, "next-default");
        runtime.client.finish().unwrap();
    }

    #[test]
    fn controller_holding_entry_and_exit_publish_state_and_trace() {
        let shared = Arc::new(FakeControllerShared::default());
        shared.candidate_ready.store(true, Ordering::SeqCst);
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let event_copy = Arc::clone(&events);
        let runtime = start_fake_controller_with(
            shared,
            SessionMicTarget::Follow,
            Arc::new(DeviceSnapshot {
                generation: 1,
                devices: Vec::new(),
            }),
            Some(Arc::new(move |event| {
                event_copy.lock().unwrap().push(event)
            })),
        );
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["state"] == "holding"));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| { event["trace"]["kind"] == "mic_holding_entered" }));

        runtime
            .client
            .send_event(SegmentControllerEvent::RegistrySnapshot(Arc::new(
                DeviceSnapshot {
                    generation: 2,
                    devices: vec![controller_device("new", true)],
                },
            )))
            .unwrap();
        wait_until(|| {
            matches!(
                runtime.client.snapshot().mic_runtime,
                MicRuntime::Active { ref device, .. } if device.resolved_uid == "new"
            )
        });
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["state"] == "active" && event["device_name"] == "NEW"));
        assert!(events.lock().unwrap().iter().any(|event| {
            event["trace"]["kind"] == "mic_holding_exited"
                && event["trace"]["reason"] == "open_failed"
        }));
        runtime.client.finish().unwrap();
    }

    #[test]
    fn controller_system_swap_leaves_mic_and_generation_untouched() {
        let shared = Arc::new(FakeControllerShared::default());
        shared.system_candidate_ready.store(true, Ordering::SeqCst);
        let runtime = start_fake_controller(shared.clone());
        let mic_before = runtime.client.snapshot().mic_runtime;
        let generation_before = runtime.client.snapshot().live_generation;

        runtime.client.swap_system().unwrap();

        assert_eq!(runtime.client.snapshot().mic_runtime, mic_before);
        assert_eq!(runtime.client.snapshot().live_generation, generation_before);
        assert_eq!(shared.system_start_count.load(Ordering::SeqCst), 2);
        runtime.client.discard().unwrap();
    }

    #[test]
    fn controller_stop_from_active_switching_and_holding_seals_once() {
        let active_shared = Arc::new(FakeControllerShared::default());
        let active = start_fake_controller(active_shared.clone());
        active.client.finish().unwrap();
        assert_eq!(active_shared.seal_count.load(Ordering::SeqCst), 1);

        let switching_shared = Arc::new(FakeControllerShared::default());
        let switching = start_fake_controller(switching_shared.clone());
        let swap_client = switching.client.clone();
        let swap = std::thread::spawn(move || swap_client.swap_mic(Some("new".into())));
        wait_until(|| {
            matches!(
                switching.client.snapshot().mic_runtime,
                MicRuntime::Switching { .. }
            )
        });
        switching.client.finish().unwrap();
        assert!(swap.join().unwrap().is_err());
        assert_eq!(switching_shared.seal_count.load(Ordering::SeqCst), 1);

        let holding_shared = Arc::new(FakeControllerShared::default());
        let holding = start_fake_controller_with(
            holding_shared.clone(),
            SessionMicTarget::Follow,
            Arc::new(DeviceSnapshot {
                generation: 1,
                devices: Vec::new(),
            }),
            None,
        );
        assert!(matches!(
            holding.client.snapshot().mic_runtime,
            MicRuntime::MicHolding { .. }
        ));
        holding.client.finish().unwrap();
        assert_eq!(holding_shared.seal_count.load(Ordering::SeqCst), 1);
    }
}
