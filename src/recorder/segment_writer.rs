use super::{LiveAudioChannel, LiveAudioChunk, LiveAudioSink};
use anyhow::{bail, Context, Result};
use margins_media::timeline::scale_frames;
pub use margins_media::timeline::{target_frame, RationalResampler};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const LANE_QUEUE_CAPACITY: usize = 128;
pub const DRAIN_BATCH_MILLIS: u64 = 20;
const TICK_MILLIS: u64 = 100;
const ARCHIVE_SAMPLE_RATE: u32 = 16_000;
const LIVE_DEBT_CHUNK_MILLIS: u64 = 250;
const LATE_TICK_NANOS: u64 = 5_000_000_000;
const OVERSIZED_GAP_NANOS: u64 = 30_000_000_000;
const TIMESTAMP_JITTER_NANOS: u64 = 20_000_000;
static SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type SegmentId = u64;
pub type CaptureToken = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureLane {
    Mic,
    System,
}

impl CaptureLane {
    fn live_channel(self) -> LiveAudioChannel {
        match self {
            Self::Mic => LiveAudioChannel::Mic,
            Self::System => LiveAudioChannel::System,
        }
    }
}

/// POD timing metadata emitted once per native callback packet.
///
/// `capture_nanos` is relative to the capture's first callback clock reading.
/// It travels through a separate preallocated SPSC ring; callbacks never
/// allocate, lock, perform IO, or send on a channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PacketDesc {
    pub frame_count: u32,
    pub capture_nanos: u64,
}

#[derive(Default)]
pub struct LaneTelemetry {
    pub rejected_token_samples: AtomicU64,
    pub overlap_trimmed_frames: AtomicU64,
    pub synthesized_durable_frames: AtomicU64,
    pub synthesized_live_frames: AtomicU64,
    pub max_lane_skew_ms: AtomicU64,
    pub live_silence_debt_at_seal: AtomicU64,
    pub suspend_discontinuities: AtomicU64,
    pub timestamp_regressions: AtomicU64,
    pub durable_frame: AtomicU64,
    pub live_frame_enqueued: AtomicU64,
    pub timeline_frame: AtomicU64,
}

#[derive(Clone)]
pub struct SegmentWriterTelemetry {
    pub timeline_reusable: Arc<AtomicBool>,
    pub writer_failed: Arc<AtomicBool>,
    pub mic: Arc<LaneTelemetry>,
    pub system: Arc<LaneTelemetry>,
}

impl SegmentWriterTelemetry {
    fn lane(&self, lane: CaptureLane) -> Arc<LaneTelemetry> {
        match lane {
            CaptureLane::Mic => self.mic.clone(),
            CaptureLane::System => self.system.clone(),
        }
    }
}

trait ClockSource: Send + Sync {
    fn now_nanos(&self) -> u64;
}

struct InstantClock {
    epoch: Instant,
}

impl ClockSource for InstantClock {
    fn now_nanos(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

struct SegmentMediaClock {
    source: Arc<dyn ClockSource>,
    frozen_nanos: AtomicU64,
    last_freeze_wall: AtomicU64,
}

impl SegmentMediaClock {
    fn new() -> Self {
        Self {
            source: Arc::new(InstantClock {
                epoch: Instant::now(),
            }),
            frozen_nanos: AtomicU64::new(0),
            last_freeze_wall: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn with_source(source: Arc<dyn ClockSource>) -> Self {
        Self {
            source,
            frozen_nanos: AtomicU64::new(0),
            last_freeze_wall: AtomicU64::new(0),
        }
    }

    fn wall_nanos(&self) -> u64 {
        self.source.now_nanos()
    }

    fn media_nanos_at(&self, wall_nanos: u64) -> u64 {
        wall_nanos.saturating_sub(self.frozen_nanos.load(Ordering::Acquire))
    }

    fn media_nanos(&self) -> u64 {
        self.media_nanos_at(self.wall_nanos())
    }

    /// Freeze one observed discontinuity. The second lane observing the same
    /// wall-clock jump is coalesced by the one-second fence.
    fn freeze_gap(&self, wall_nanos: u64, gap_nanos: u64) -> bool {
        let previous = self.last_freeze_wall.load(Ordering::Acquire);
        if previous != 0 && wall_nanos.saturating_sub(previous) < 1_000_000_000 {
            return false;
        }
        if self
            .last_freeze_wall
            .compare_exchange(previous, wall_nanos, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.frozen_nanos.fetch_add(gap_nanos, Ordering::AcqRel);
        true
    }
}

struct SharedLaneActivity {
    mic_last_delivery_media_nanos: AtomicU64,
    system_last_delivery_media_nanos: AtomicU64,
}

impl SharedLaneActivity {
    fn new() -> Self {
        Self {
            mic_last_delivery_media_nanos: AtomicU64::new(0),
            system_last_delivery_media_nanos: AtomicU64::new(0),
        }
    }

    fn own(&self, lane: CaptureLane) -> &AtomicU64 {
        match lane {
            CaptureLane::Mic => &self.mic_last_delivery_media_nanos,
            CaptureLane::System => &self.system_last_delivery_media_nanos,
        }
    }

    fn other(&self, lane: CaptureLane) -> &AtomicU64 {
        match lane {
            CaptureLane::Mic => &self.system_last_delivery_media_nanos,
            CaptureLane::System => &self.mic_last_delivery_media_nanos,
        }
    }
}

enum LaneMsg {
    Attach {
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        source_rate: u32,
        fence_nanos: u64,
        ack: mpsc::Sender<Result<()>>,
    },
    Samples {
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        source_rate: u32,
        samples: Vec<f32>,
        packets: Vec<PacketDesc>,
    },
    Retire {
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        ack: mpsc::Sender<Result<RetireAck>>,
    },
    Tick {
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        wall_nanos: u64,
    },
    Seal {
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        target_frame: u64,
        ack: mpsc::Sender<Result<SealedLane>>,
    },
    SetRate {
        rate: u32,
        ack: mpsc::Sender<Result<()>>,
    },
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetireAck {
    pub token: CaptureToken,
    pub boundary_frame: u64,
}

/// Opaque, cloneable ingress for exactly one capture token.
///
/// The capture's drain thread is the only code that sends `Attach`, `Samples`,
/// and `Retire` through this ingress. That same-sender FIFO is part of the
/// public threading contract. `cpal::Stream` remains owned by the controller
/// thread because it is `!Send` on supported native backends.
#[derive(Clone)]
pub struct CaptureSink {
    segment_id: SegmentId,
    lane: CaptureLane,
    token: CaptureToken,
    sender: SyncSender<LaneMsg>,
    clock: Arc<SegmentMediaClock>,
}

pub type MicSink = CaptureSink;
pub type SystemSink = CaptureSink;

impl CaptureSink {
    pub fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    pub fn lane(&self) -> CaptureLane {
        self.lane
    }

    pub fn token(&self) -> CaptureToken {
        self.token
    }

    pub fn media_fence_nanos(&self) -> u64 {
        self.clock.media_nanos()
    }

    pub fn attach(&self, source_rate: u32, fence_nanos: u64) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(LaneMsg::Attach {
                segment_id: self.segment_id,
                lane: self.lane,
                token: self.token,
                source_rate,
                fence_nanos,
                ack: tx,
            })
            .context("lane actor stopped before attach")?;
        rx.recv().context("lane actor dropped attach ACK")??;
        Ok(())
    }

    pub fn write(
        &self,
        source_rate: u32,
        samples: Vec<f32>,
        packets: Vec<PacketDesc>,
    ) -> Result<()> {
        self.sender
            .send(LaneMsg::Samples {
                segment_id: self.segment_id,
                lane: self.lane,
                token: self.token,
                source_rate,
                samples,
                packets,
            })
            .context("lane actor stopped while capture was draining")
    }

    pub(crate) fn samples(
        &self,
        source_rate: u32,
        samples: Vec<f32>,
        packets: Vec<PacketDesc>,
    ) -> Result<()> {
        self.write(source_rate, samples, packets)
    }

    pub fn retire(&self) -> Result<RetireAck> {
        let (tx, rx) = mpsc::channel();
        self.sender
            .send(LaneMsg::Retire {
                segment_id: self.segment_id,
                lane: self.lane,
                token: self.token,
                ack: tx,
            })
            .context("lane actor stopped before retire")?;
        rx.recv().context("lane actor dropped retire ACK")?
    }
}

struct LaneActorHandle {
    sender: SyncSender<LaneMsg>,
    join: Option<JoinHandle<()>>,
    rate: Arc<AtomicU32>,
}

/// Segment-scoped coordinator. It owns no spool mutex: each lane actor has
/// exclusive ownership of its own spool, resampler, cursors, and telemetry.
pub struct SegmentWriter {
    segment_id: SegmentId,
    mic: LaneActorHandle,
    system: LaneActorHandle,
    clock: Arc<SegmentMediaClock>,
    telemetry: SegmentWriterTelemetry,
    ever_attached: Arc<AtomicBool>,
}

impl SegmentWriter {
    pub fn start(
        segment_id: SegmentId,
        mic_lane_rate: u32,
        system_lane_rate: u32,
        live_audio: Option<LiveAudioSink>,
    ) -> Result<Self> {
        if mic_lane_rate == 0 || system_lane_rate == 0 {
            bail!("lane sample rates must be non-zero");
        }
        Self::start_with_clock(
            segment_id,
            mic_lane_rate,
            system_lane_rate,
            live_audio,
            Arc::new(SegmentMediaClock::new()),
        )
    }

    /// Start with lane rates unresolved. A lane's first `Attach` fixes that
    /// lane's rate. If microphone startup fails, the controller must call
    /// `set_unattached_lane_rate(Mic, system.native_rate())` before sealing.
    /// No ticker output is emitted for an unresolved lane, so initial silence
    /// is materialized once, at the correct rate, by attach or seal.
    pub fn start_deferred(
        segment_id: SegmentId,
        live_audio: Option<LiveAudioSink>,
    ) -> Result<Self> {
        Self::start_with_clock(
            segment_id,
            0,
            0,
            live_audio,
            Arc::new(SegmentMediaClock::new()),
        )
    }

    fn start_with_clock(
        segment_id: SegmentId,
        mic_lane_rate: u32,
        system_lane_rate: u32,
        live_audio: Option<LiveAudioSink>,
        clock: Arc<SegmentMediaClock>,
    ) -> Result<Self> {
        let timeline_reusable = Arc::new(AtomicBool::new(true));
        let telemetry = SegmentWriterTelemetry {
            timeline_reusable,
            writer_failed: Arc::new(AtomicBool::new(false)),
            mic: Arc::new(LaneTelemetry::default()),
            system: Arc::new(LaneTelemetry::default()),
        };
        let activity = Arc::new(SharedLaneActivity::new());
        let ever_attached = Arc::new(AtomicBool::new(false));
        let mic = spawn_lane(
            segment_id,
            CaptureLane::Mic,
            mic_lane_rate,
            live_audio.clone(),
            clock.clone(),
            activity.clone(),
            telemetry.clone(),
            ever_attached.clone(),
        )?;
        let system = spawn_lane(
            segment_id,
            CaptureLane::System,
            system_lane_rate,
            live_audio,
            clock.clone(),
            activity,
            telemetry.clone(),
            ever_attached.clone(),
        )?;
        Ok(Self {
            segment_id,
            mic,
            system,
            clock,
            telemetry,
            ever_attached,
        })
    }

    pub fn telemetry(&self) -> SegmentWriterTelemetry {
        self.telemetry.clone()
    }

    pub fn media_frame(&self) -> Result<u64> {
        let rate = self.mic_lane_rate();
        if rate == 0 {
            bail!("microphone lane rate is unresolved");
        }
        target_frame(self.clock.media_nanos(), rate)
    }

    pub fn mic_lane_rate(&self) -> u32 {
        self.mic.rate.load(Ordering::Acquire)
    }

    pub fn system_lane_rate(&self) -> u32 {
        self.system.rate.load(Ordering::Acquire)
    }

    pub fn set_unattached_lane_rate(&self, lane: CaptureLane, rate: u32) -> Result<()> {
        if rate == 0 {
            bail!("lane sample rate must be non-zero");
        }
        let actor = self.actor(lane);
        let (tx, rx) = mpsc::channel();
        actor
            .sender
            .send(LaneMsg::SetRate { rate, ack: tx })
            .context("lane actor stopped before rate update")?;
        rx.recv().context("lane actor dropped rate-update ACK")??;
        Ok(())
    }

    pub fn capture_sink(&self, lane: CaptureLane, token: CaptureToken) -> CaptureSink {
        CaptureSink {
            segment_id: self.segment_id,
            lane,
            token,
            sender: self.actor(lane).sender.clone(),
            clock: self.clock.clone(),
        }
    }

    pub fn mic_sink(&self, token: CaptureToken) -> MicSink {
        self.capture_sink(CaptureLane::Mic, token)
    }

    pub fn system_sink(&self, token: CaptureToken) -> SystemSink {
        self.capture_sink(CaptureLane::System, token)
    }

    pub fn seal(self) -> Result<SealedSegment> {
        let frame = self.media_frame()?;
        self.seal_at(frame)
    }

    pub fn seal_at(mut self, requested_media_frame: u64) -> Result<SealedSegment> {
        let mic_rate = self.mic_lane_rate();
        let system_rate = self.system_lane_rate();
        let mic_durable = self.telemetry.mic.durable_frame.load(Ordering::Acquire);
        let system_durable = self.telemetry.system.durable_frame.load(Ordering::Acquire);
        let system_as_mic = scale_frames(system_durable, system_rate, mic_rate)?;
        let shared_frame = requested_media_frame.max(mic_durable).max(system_as_mic);
        let system_target = scale_frames(shared_frame, mic_rate, system_rate)?;

        let mic = request_seal(
            &self.mic.sender,
            self.segment_id,
            CaptureLane::Mic,
            shared_frame,
        )?;
        let system = request_seal(
            &self.system.sender,
            self.segment_id,
            CaptureLane::System,
            system_target,
        )?;
        join_lane(&mut self.mic)?;
        join_lane(&mut self.system)?;
        if self.telemetry.writer_failed.load(Ordering::Acquire) {
            bail!("segment lane writer failed");
        }
        Ok(SealedSegment {
            segment_id: self.segment_id,
            mic,
            system,
            output_rate: mic_rate,
            shared_frame,
            timeline_reusable: self.telemetry.timeline_reusable.load(Ordering::Acquire),
            ever_attached: self.ever_attached.load(Ordering::Acquire),
        })
    }

    fn actor(&self, lane: CaptureLane) -> &LaneActorHandle {
        match lane {
            CaptureLane::Mic => &self.mic,
            CaptureLane::System => &self.system,
        }
    }
}

impl Drop for SegmentWriter {
    fn drop(&mut self) {
        let _ = self.mic.sender.send(LaneMsg::Abort);
        let _ = self.system.sender.send(LaneMsg::Abort);
        if let Some(join) = self.mic.join.take() {
            let _ = join.join();
        }
        if let Some(join) = self.system.join.take() {
            let _ = join.join();
        }
    }
}

fn join_lane(actor: &mut LaneActorHandle) -> Result<()> {
    if let Some(join) = actor.join.take() {
        join.join()
            .map_err(|_| anyhow::anyhow!("lane actor panicked"))?;
    }
    Ok(())
}

fn request_seal(
    sender: &SyncSender<LaneMsg>,
    segment_id: SegmentId,
    lane: CaptureLane,
    target_frame: u64,
) -> Result<SealedLane> {
    let (tx, rx) = mpsc::channel();
    sender
        .send(LaneMsg::Seal {
            segment_id,
            lane,
            token: 0,
            target_frame,
            ack: tx,
        })
        .context("lane actor stopped before seal")?;
    rx.recv().context("lane actor dropped seal ACK")?
}

#[derive(Debug)]
struct SealedLane {
    path: PathBuf,
    samples_written: u64,
    rate: u32,
}

impl Drop for SealedLane {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct SealedSegment {
    pub segment_id: SegmentId,
    mic: SealedLane,
    system: SealedLane,
    pub output_rate: u32,
    pub shared_frame: u64,
    pub timeline_reusable: bool,
    ever_attached: bool,
}

impl SealedSegment {
    pub fn write_wav(self, path: impl AsRef<Path>) -> Result<f64> {
        if !self.ever_attached && self.shared_frame == 0 {
            return Ok(0.0);
        }
        write_sealed_stereo_wav(path.as_ref(), &self.mic, &self.system, self.output_rate)?;
        Ok(self.shared_frame as f64 / self.output_rate as f64)
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_lane(
    segment_id: SegmentId,
    lane: CaptureLane,
    rate: u32,
    live_audio: Option<LiveAudioSink>,
    clock: Arc<SegmentMediaClock>,
    activity: Arc<SharedLaneActivity>,
    telemetry: SegmentWriterTelemetry,
    ever_attached: Arc<AtomicBool>,
) -> Result<LaneActorHandle> {
    let (sender, receiver) = mpsc::sync_channel(LANE_QUEUE_CAPACITY);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let rate_atomic = Arc::new(AtomicU32::new(rate));
    let thread_rate = rate_atomic.clone();
    let join = std::thread::Builder::new()
        .name(format!("margins-{lane:?}-lane"))
        .spawn(move || {
            let lane_telemetry = telemetry.lane(lane);
            match LaneActor::new(
                segment_id,
                lane,
                rate,
                live_audio,
                clock,
                activity,
                telemetry.timeline_reusable.clone(),
                telemetry.writer_failed.clone(),
                lane_telemetry,
                thread_rate,
                ever_attached,
            ) {
                Ok(actor) => {
                    let _ = ready_tx.send(Ok(()));
                    actor.run(receiver)
                }
                Err(error) => {
                    let message = format!("{lane:?} lane failed to start: {error:#}");
                    let _ = ready_tx.send(Err(message.clone()));
                    eprintln!("{message}");
                }
            }
        })?;
    match ready_rx
        .recv()
        .context("lane actor did not report startup")?
    {
        Ok(()) => {}
        Err(message) => {
            let _ = join.join();
            bail!(message);
        }
    }
    Ok(LaneActorHandle {
        sender,
        join: Some(join),
        rate: rate_atomic,
    })
}

struct LaneActor {
    segment_id: SegmentId,
    lane: CaptureLane,
    rate: u32,
    active_token: CaptureToken,
    attached: bool,
    ever_attached: Arc<AtomicBool>,
    lane_ever_attached: bool,
    spool_path: PathBuf,
    writer: Option<BufWriter<File>>,
    durable_frame: u64,
    live_frame_enqueued: u64,
    timeline_frame: u64,
    resampler: Option<RationalResampler>,
    live_audio: Option<LiveAudioSink>,
    clock: Arc<SegmentMediaClock>,
    activity: Arc<SharedLaneActivity>,
    reusable: Arc<AtomicBool>,
    writer_failed: Arc<AtomicBool>,
    telemetry: Arc<LaneTelemetry>,
    rate_atomic: Arc<AtomicU32>,
    last_tick_wall_nanos: u64,
    delivery_started: bool,
    first_packet_capture_nanos: Option<u64>,
    attachment_source_frames: u64,
    last_packet_end_nanos: Option<u64>,
    pending_overlap_trim: u64,
    sealed: bool,
}

impl LaneActor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        segment_id: SegmentId,
        lane: CaptureLane,
        rate: u32,
        live_audio: Option<LiveAudioSink>,
        clock: Arc<SegmentMediaClock>,
        activity: Arc<SharedLaneActivity>,
        reusable: Arc<AtomicBool>,
        writer_failed: Arc<AtomicBool>,
        telemetry: Arc<LaneTelemetry>,
        rate_atomic: Arc<AtomicU32>,
        ever_attached: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (spool_path, writer) = new_spool(lane)?;
        Ok(Self {
            segment_id,
            lane,
            rate,
            active_token: 0,
            attached: false,
            ever_attached,
            lane_ever_attached: false,
            spool_path,
            writer: Some(writer),
            durable_frame: 0,
            live_frame_enqueued: 0,
            timeline_frame: 0,
            resampler: None,
            live_audio,
            clock,
            activity,
            reusable,
            writer_failed,
            telemetry,
            rate_atomic,
            last_tick_wall_nanos: 0,
            delivery_started: false,
            first_packet_capture_nanos: None,
            attachment_source_frames: 0,
            last_packet_end_nanos: None,
            pending_overlap_trim: 0,
            sealed: false,
        })
    }

    fn run(mut self, receiver: Receiver<LaneMsg>) {
        loop {
            let wall = self.clock.wall_nanos();
            if self.last_tick_wall_nanos == 0 {
                self.last_tick_wall_nanos = wall;
            }
            let elapsed = wall.saturating_sub(self.last_tick_wall_nanos);
            let timeout =
                Duration::from_nanos((TICK_MILLIS * 1_000_000).saturating_sub(elapsed).max(1));
            match receiver.recv_timeout(timeout) {
                Ok(LaneMsg::Abort) => break,
                Ok(message) => {
                    if self.handle(message) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let wall = self.clock.wall_nanos();
            if wall.saturating_sub(self.last_tick_wall_nanos) >= TICK_MILLIS * 1_000_000 {
                self.handle(LaneMsg::Tick {
                    segment_id: self.segment_id,
                    lane: self.lane,
                    token: self.active_token,
                    wall_nanos: wall,
                });
            }
        }
        if !self.sealed {
            let _ = std::fs::remove_file(&self.spool_path);
        }
    }

    fn handle(&mut self, message: LaneMsg) -> bool {
        match message {
            LaneMsg::Attach {
                segment_id,
                lane,
                token,
                source_rate,
                fence_nanos,
                ack,
            } => {
                let result = self.attach(segment_id, lane, token, source_rate, fence_nanos);
                let _ = ack.send(result);
            }
            LaneMsg::Samples {
                segment_id,
                lane,
                token,
                source_rate,
                samples,
                packets,
            } => {
                if let Err(error) =
                    self.samples(segment_id, lane, token, source_rate, samples, packets)
                {
                    self.fail_writer();
                    eprintln!("{:?} lane write failed: {error:#}", self.lane);
                }
            }
            LaneMsg::Retire {
                segment_id,
                lane,
                token,
                ack,
            } => {
                let result = self.retire(segment_id, lane, token);
                let _ = ack.send(result);
            }
            LaneMsg::Tick {
                segment_id,
                lane,
                token,
                wall_nanos,
            } => {
                if token != self.active_token {
                    self.fail_timeline();
                } else if let Err(error) = self
                    .validate(segment_id, lane)
                    .and_then(|()| self.tick(wall_nanos))
                {
                    self.fail_writer();
                    eprintln!("{:?} lane tick failed: {error:#}", self.lane);
                }
            }
            LaneMsg::Seal {
                segment_id,
                lane,
                token,
                target_frame,
                ack,
            } => {
                let result = self.seal(segment_id, lane, token, target_frame);
                self.sealed = result.is_ok();
                let _ = ack.send(result);
                return true;
            }
            LaneMsg::SetRate { rate, ack } => {
                let result = if self.lane_ever_attached
                    || self.durable_frame != 0
                    || self.live_frame_enqueued != 0
                    || self.timeline_frame != 0
                {
                    bail_result("cannot change a lane rate after attachment or materialization")
                } else {
                    self.rate = rate;
                    self.rate_atomic.store(rate, Ordering::Release);
                    Ok(())
                };
                let _ = ack.send(result);
            }
            LaneMsg::Abort => return true,
        }
        false
    }

    fn validate(&self, segment_id: SegmentId, lane: CaptureLane) -> Result<()> {
        if segment_id != self.segment_id || lane != self.lane {
            bail!("lane message routing invariant failed");
        }
        Ok(())
    }

    fn attach(
        &mut self,
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        source_rate: u32,
        fence_nanos: u64,
    ) -> Result<()> {
        self.validate(segment_id, lane)?;
        if token == 0 || self.active_token != 0 || self.attached {
            bail!("attach requires a non-zero token and a detached lane");
        }
        if self.rate == 0 {
            if source_rate == 0 {
                bail!("capture source rate must be non-zero");
            }
            self.rate = source_rate;
            self.rate_atomic.store(source_rate, Ordering::Release);
        }
        let mut target = target_frame(fence_nanos, self.rate)?;
        let fill = target.saturating_sub(self.durable_frame);
        if fill > target_frame(OVERSIZED_GAP_NANOS, self.rate)? {
            let other_last = self.activity.other(self.lane).load(Ordering::Acquire);
            let window_start = fence_nanos.saturating_sub(OVERSIZED_GAP_NANOS);
            if other_last <= window_start {
                let gap_nanos = frames_to_nanos(fill, self.rate)?;
                let wall = self.clock.wall_nanos();
                if self.clock.freeze_gap(wall, gap_nanos) {
                    self.telemetry
                        .suspend_discontinuities
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.fail_timeline();
                target = self.durable_frame;
            }
        }
        if target >= self.durable_frame {
            self.write_silence(target - self.durable_frame)?;
        } else {
            let overlap = self.durable_frame - target;
            self.pending_overlap_trim = overlap;
            self.telemetry
                .overlap_trimmed_frames
                .fetch_add(overlap, Ordering::Relaxed);
            if overlap > target_frame(TIMESTAMP_JITTER_NANOS, self.rate)? {
                self.fail_timeline();
            }
        }
        self.timeline_frame = target.max(self.durable_frame);
        self.try_live_silence_once(self.timeline_frame)?;
        self.active_token = token;
        self.attached = true;
        self.ever_attached.store(true, Ordering::Release);
        self.lane_ever_attached = true;
        self.resampler = Some(RationalResampler::new(source_rate, self.rate)?);
        self.delivery_started = false;
        self.first_packet_capture_nanos = None;
        self.last_packet_end_nanos = None;
        self.attachment_source_frames = 0;
        self.publish_cursors();
        Ok(())
    }

    fn samples(
        &mut self,
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
        source_rate: u32,
        samples: Vec<f32>,
        packets: Vec<PacketDesc>,
    ) -> Result<()> {
        self.validate(segment_id, lane)?;
        if token != self.active_token || !self.attached {
            self.telemetry
                .rejected_token_samples
                .fetch_add(samples.len() as u64, Ordering::Relaxed);
            self.fail_timeline();
            return Ok(());
        }
        if !self.delivery_started && !samples.is_empty() {
            // An attached native stream can start successfully without invoking
            // its callback (notably a macOS process tap before it recovers).
            // Keep the first real batch at its wall-clock position instead of
            // appending it at the beginning of the lane. The periodic ticker
            // normally materializes most of this gap incrementally; this final
            // fill accounts for the interval since the last tick.
            let batch_nanos = frames_to_nanos(samples.len() as u64, source_rate)?;
            let batch_start = self.clock.media_nanos().saturating_sub(batch_nanos);
            let target = target_frame(batch_start, self.rate)?;
            if target > self.durable_frame {
                self.write_silence(target - self.durable_frame)?;
            }
            self.delivery_started = true;
        }
        let durable_start = self.durable_frame;
        self.observe_packets(&packets, source_rate)?;
        let resampler = self
            .resampler
            .as_mut()
            .context("attached lane lacks resampler")?;
        if resampler.from_rate() != source_rate {
            bail!("capture source rate changed without attach");
        }
        let mut converted = resampler.process(&samples)?;
        if self.pending_overlap_trim != 0 {
            let trim = usize::try_from(self.pending_overlap_trim)
                .unwrap_or(usize::MAX)
                .min(converted.len());
            converted.drain(..trim);
            self.pending_overlap_trim -= trim as u64;
        }
        self.write_samples(&converted)?;
        let media_now = self.clock.media_nanos();
        self.activity
            .own(self.lane)
            .store(media_now, Ordering::Release);
        self.timeline_frame = self.timeline_frame.max(self.durable_frame);
        if self.live_frame_enqueued < durable_start {
            self.try_live_silence_once(durable_start)?;
        }
        if self.live_frame_enqueued == durable_start {
            self.enqueue_real(converted)?;
        } else if !converted.is_empty() {
            if let Some(sink) = &self.live_audio {
                add_live_count(sink, self.lane, false, converted.len() as u64);
            }
            self.fail_timeline();
        }
        self.publish_cursors();
        Ok(())
    }

    fn retire(
        &mut self,
        segment_id: SegmentId,
        lane: CaptureLane,
        token: CaptureToken,
    ) -> Result<RetireAck> {
        self.validate(segment_id, lane)?;
        if token != self.active_token || !self.attached {
            self.fail_timeline();
            bail!("retire token does not own the lane");
        }
        if let Some(resampler) = &mut self.resampler {
            let mut tail = resampler.finish()?;
            if self.pending_overlap_trim != 0 {
                let trim = usize::try_from(self.pending_overlap_trim)
                    .unwrap_or(usize::MAX)
                    .min(tail.len());
                tail.drain(..trim);
                self.pending_overlap_trim -= trim as u64;
            }
            self.write_samples(&tail)?;
            if !tail.is_empty()
                && self.live_frame_enqueued + tail.len() as u64 == self.durable_frame
            {
                self.enqueue_real(tail)?;
            }
        }
        self.attached = false;
        self.active_token = 0;
        self.resampler = None;
        self.timeline_frame = self.timeline_frame.max(self.durable_frame);
        self.publish_cursors();
        Ok(RetireAck {
            token,
            boundary_frame: self.durable_frame,
        })
    }

    fn tick(&mut self, wall_nanos: u64) -> Result<()> {
        if self.rate == 0 {
            self.last_tick_wall_nanos = wall_nanos;
            return Ok(());
        }
        let delta = wall_nanos.saturating_sub(self.last_tick_wall_nanos);
        if delta > LATE_TICK_NANOS {
            if self.clock.freeze_gap(wall_nanos, delta) {
                self.telemetry
                    .suspend_discontinuities
                    .fetch_add(1, Ordering::Relaxed);
            }
            self.fail_timeline();
        }
        self.last_tick_wall_nanos = wall_nanos;
        let media_target = target_frame(self.clock.media_nanos_at(wall_nanos), self.rate)?;
        if self.attached && !self.delivery_started {
            // Stay one drain batch behind the media clock so the first callback
            // can occupy its actual interval. Without this, a stream that emits
            // no frames for several seconds later shifts recovered audio to the
            // beginning of the system lane.
            let gap_nanos = self
                .clock
                .media_nanos_at(wall_nanos)
                .saturating_sub(DRAIN_BATCH_MILLIS * 1_000_000);
            let gap_target = target_frame(gap_nanos, self.rate)?;
            if gap_target > self.durable_frame {
                self.write_silence(gap_target - self.durable_frame)?;
            }
        }
        self.timeline_frame = if self.attached {
            media_target.max(self.durable_frame)
        } else {
            media_target
        };
        let live_target = if self.attached {
            self.durable_frame
        } else {
            self.timeline_frame
        };
        self.try_live_silence_once(live_target)?;
        self.publish_cursors();
        Ok(())
    }

    fn seal(
        &mut self,
        segment_id: SegmentId,
        lane: CaptureLane,
        _token: CaptureToken,
        target_frame: u64,
    ) -> Result<SealedLane> {
        self.validate(segment_id, lane)?;
        if self.attached || self.active_token != 0 {
            bail!("cannot seal a lane before its capture retires");
        }
        if target_frame > self.durable_frame {
            self.write_silence(target_frame - self.durable_frame)?;
        }
        self.timeline_frame = target_frame.max(self.durable_frame);
        // One bounded final attempt only. A long/full-queue debt is reported
        // and disqualifies reuse; seal must never create a catch-up burst.
        self.try_live_silence_once(self.timeline_frame)?;
        let debt = self.timeline_frame.saturating_sub(self.live_frame_enqueued);
        self.telemetry
            .live_silence_debt_at_seal
            .store(debt, Ordering::Relaxed);
        if self.live_audio.is_some() && debt != 0 {
            self.fail_timeline();
        }
        let mut writer = self.writer.take().context("lane spool already sealed")?;
        writer.flush()?;
        self.publish_cursors();
        Ok(SealedLane {
            path: self.spool_path.clone(),
            samples_written: self.durable_frame,
            rate: self.rate,
        })
    }

    fn observe_packets(&mut self, packets: &[PacketDesc], source_rate: u32) -> Result<()> {
        for packet in packets {
            if let Some(previous_end) = self.last_packet_end_nanos {
                if packet.capture_nanos.saturating_add(TIMESTAMP_JITTER_NANOS) < previous_end {
                    self.telemetry
                        .timestamp_regressions
                        .fetch_add(1, Ordering::Relaxed);
                    self.fail_timeline();
                }
            }
            let packet_duration = frames_to_nanos(u64::from(packet.frame_count), source_rate)?;
            self.last_packet_end_nanos = Some(packet.capture_nanos.saturating_add(packet_duration));
            let first = *self
                .first_packet_capture_nanos
                .get_or_insert(packet.capture_nanos);
            let capture_elapsed = packet.capture_nanos.saturating_sub(first);
            let durable_elapsed = frames_to_nanos(self.attachment_source_frames, source_rate)?;
            let skew = capture_elapsed.abs_diff(durable_elapsed) / 1_000_000;
            self.telemetry
                .max_lane_skew_ms
                .fetch_max(skew, Ordering::Relaxed);
            self.attachment_source_frames = self
                .attachment_source_frames
                .saturating_add(u64::from(packet.frame_count));
        }
        Ok(())
    }

    fn write_silence(&mut self, frames: u64) -> Result<()> {
        const ZERO_BLOCK: [u8; 16 * 1024] = [0; 16 * 1024];
        let writer = self.writer.as_mut().context("lane spool is sealed")?;
        let mut bytes = frames
            .checked_mul(4)
            .context("silence byte count overflow")?;
        while bytes > 0 {
            let n = usize::try_from(bytes.min(ZERO_BLOCK.len() as u64))?;
            writer.write_all(&ZERO_BLOCK[..n])?;
            bytes -= n as u64;
        }
        self.durable_frame = self.durable_frame.saturating_add(frames);
        self.telemetry
            .synthesized_durable_frames
            .fetch_add(frames, Ordering::Relaxed);
        Ok(())
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        let writer = self.writer.as_mut().context("lane spool is sealed")?;
        for sample in samples {
            writer.write_all(&sample.to_le_bytes())?;
        }
        self.durable_frame = self
            .durable_frame
            .checked_add(samples.len() as u64)
            .context("durable frame overflow")?;
        Ok(())
    }

    fn try_live_silence_once(&mut self, target: u64) -> Result<()> {
        if self.live_frame_enqueued >= target {
            return Ok(());
        }
        if self.live_audio.is_none() {
            self.live_frame_enqueued = target;
            return Ok(());
        }
        let cap = target_frame(LIVE_DEBT_CHUNK_MILLIS * 1_000_000, self.rate)?;
        let frames = target
            .saturating_sub(self.live_frame_enqueued)
            .min(cap.max(1));
        let samples = vec![0.0; usize::try_from(frames)?];
        if self.try_live_send(samples, false) {
            self.live_frame_enqueued += frames;
            self.telemetry
                .synthesized_live_frames
                .fetch_add(frames, Ordering::Relaxed);
        }
        Ok(())
    }

    fn enqueue_real(&mut self, samples: Vec<f32>) -> Result<()> {
        let frames = samples.len() as u64;
        if frames == 0 {
            return Ok(());
        }
        if self.live_audio.is_none() {
            self.live_frame_enqueued = self.live_frame_enqueued.saturating_add(frames);
            return Ok(());
        }
        if self.try_live_send(samples, true) {
            self.live_frame_enqueued = self.live_frame_enqueued.saturating_add(frames);
        } else {
            self.fail_timeline();
        }
        Ok(())
    }

    fn try_live_send(&self, samples: Vec<f32>, count_drop_on_failure: bool) -> bool {
        let Some(sink) = &self.live_audio else {
            return true;
        };
        let sample_count = samples.len() as u64;
        let generation_clock = *sink
            .generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if generation_clock.generation != sink.generation {
            if count_drop_on_failure {
                add_live_count(sink, self.lane, false, sample_count);
            }
            return false;
        }
        let chunk = LiveAudioChunk {
            channel: self.lane.live_channel(),
            generation: sink.generation,
            session_offset_ms: generation_clock.session_offset_ms,
            sample_rate: self.rate,
            samples,
        };
        let current_queued = sink.queued_samples.load(Ordering::Relaxed);
        if sink.queue_max_samples > 0
            && current_queued.saturating_add(sample_count) > sink.queue_max_samples
        {
            if count_drop_on_failure {
                add_live_count(sink, self.lane, false, sample_count);
            }
            return false;
        }
        match sink.sender.send(chunk) {
            Ok(()) => {
                sink.queued_samples
                    .fetch_add(sample_count, Ordering::Relaxed);
                add_live_count(sink, self.lane, true, sample_count);
                true
            }
            Err(_) => {
                if count_drop_on_failure {
                    add_live_count(sink, self.lane, false, sample_count);
                }
                false
            }
        }
    }

    fn fail_timeline(&self) {
        self.reusable.store(false, Ordering::Release);
    }

    fn fail_writer(&self) {
        self.writer_failed.store(true, Ordering::Release);
        self.fail_timeline();
    }

    fn publish_cursors(&self) {
        self.telemetry
            .durable_frame
            .store(self.durable_frame, Ordering::Release);
        self.telemetry
            .live_frame_enqueued
            .store(self.live_frame_enqueued, Ordering::Release);
        self.telemetry
            .timeline_frame
            .store(self.timeline_frame, Ordering::Release);
    }
}

fn bail_result<T>(message: &str) -> Result<T> {
    Err(anyhow::anyhow!(message.to_string()))
}

fn frames_to_nanos(frames: u64, rate: u32) -> Result<u64> {
    if rate == 0 {
        bail!("sample rate must be non-zero");
    }
    let nanos = u128::from(frames)
        .checked_mul(1_000_000_000u128)
        .context("frame-duration multiplication overflow")?
        / u128::from(rate);
    u64::try_from(nanos).context("frame duration exceeds u64 nanoseconds")
}

fn add_live_count(sink: &LiveAudioSink, lane: CaptureLane, accepted: bool, count: u64) {
    match (lane, accepted) {
        (CaptureLane::Mic, true) => sink
            .mic_accepted_samples
            .fetch_add(count, Ordering::Relaxed),
        (CaptureLane::System, true) => sink
            .system_accepted_samples
            .fetch_add(count, Ordering::Relaxed),
        (CaptureLane::Mic, false) => sink.mic_dropped_samples.fetch_add(count, Ordering::Relaxed),
        (CaptureLane::System, false) => sink
            .system_dropped_samples
            .fetch_add(count, Ordering::Relaxed),
    };
}

fn new_spool(lane: CaptureLane) -> Result<(PathBuf, BufWriter<File>)> {
    let unique = format!(
        "margins-lane-{:?}-{}-{}-{}.f32",
        lane,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SPOOL_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    );
    let path = std::env::temp_dir().join(unique);
    let writer = BufWriter::new(
        File::create(&path).with_context(|| format!("failed to create {}", path.display()))?,
    );
    Ok((path, writer))
}

fn resample_channel(samples: Vec<f32>, from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(samples);
    }
    let mut resampler = RationalResampler::new(from_rate, to_rate)?;
    let mut out = resampler.process(&samples)?;
    out.extend(resampler.finish()?);
    Ok(out)
}

fn write_sealed_stereo_wav(
    path: &Path,
    mic: &SealedLane,
    system: &SealedLane,
    output_rate: u32,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create WAV parent directory")?;
    }
    // Collect mic samples (already at output_rate from the spool).
    let mut mic_reader = RawF32Reader::open(&mic.path)?;
    let mut system_reader = StreamingResampledReader::open(&system.path, system.rate, output_rate)?;
    let system_output_len = scale_frames(system.samples_written, system.rate, output_rate)?;
    let output_len = mic.samples_written.max(system_output_len) as usize;

    let mut mic_samples = Vec::with_capacity(output_len);
    let mut system_samples = Vec::with_capacity(output_len);
    for _ in 0..output_len {
        mic_samples.push(mic_reader.next_sample()?.unwrap_or(0.0));
        system_samples.push(system_reader.next_sample()?.unwrap_or(0.0));
    }

    // Resample each channel from output_rate → ARCHIVE_SAMPLE_RATE (skip if equal).
    let mic_archive = resample_channel(mic_samples, output_rate, ARCHIVE_SAMPLE_RATE)?;
    let system_archive = resample_channel(system_samples, output_rate, ARCHIVE_SAMPLE_RATE)?;

    // Write as 16-bit PCM stereo at ARCHIVE_SAMPLE_RATE.
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: ARCHIVE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).context("failed to create WAV")?;
    let archive_len = mic_archive.len().max(system_archive.len());
    for i in 0..archive_len {
        let m = mic_archive.get(i).copied().unwrap_or(0.0);
        let s = system_archive.get(i).copied().unwrap_or(0.0);
        writer.write_sample((m.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)?;
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

struct RawF32Reader {
    reader: BufReader<File>,
}

impl RawF32Reader {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            reader: BufReader::new(File::open(path)?),
        })
    }

    fn next_sample(&mut self) -> Result<Option<f32>> {
        let mut bytes = [0u8; 4];
        match self.reader.read_exact(&mut bytes) {
            Ok(()) => Ok(Some(f32::from_le_bytes(bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

struct StreamingResampledReader {
    reader: RawF32Reader,
    resampler: RationalResampler,
    pending: VecDeque<f32>,
    finished: bool,
}

impl StreamingResampledReader {
    fn open(path: &Path, from_rate: u32, to_rate: u32) -> Result<Self> {
        Ok(Self {
            reader: RawF32Reader::open(path)?,
            resampler: RationalResampler::new(from_rate, to_rate)?,
            pending: VecDeque::new(),
            finished: false,
        })
    }

    fn next_sample(&mut self) -> Result<Option<f32>> {
        loop {
            if let Some(sample) = self.pending.pop_front() {
                return Ok(Some(sample));
            }
            if self.finished {
                return Ok(None);
            }
            let mut input = Vec::with_capacity(4_096);
            while input.len() < 4_096 {
                match self.reader.next_sample()? {
                    Some(sample) => input.push(sample),
                    None => break,
                }
            }
            if input.is_empty() {
                self.pending.extend(self.resampler.finish()?);
                self.finished = true;
            } else {
                self.pending.extend(self.resampler.process(&input)?);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_sink(_capacity: usize) -> (LiveAudioSink, Receiver<LiveAudioChunk>) {
        let (sender, receiver) = mpsc::channel();
        (
            LiveAudioSink {
                sender,
                generation: 0,
                generation_clock: Arc::new(std::sync::Mutex::new(
                    crate::recorder::LiveGenerationClock {
                        generation: 0,
                        session_offset_ms: 0,
                    },
                )),
                mic_accepted_samples: Arc::new(AtomicU64::new(0)),
                system_accepted_samples: Arc::new(AtomicU64::new(0)),
                mic_dropped_samples: Arc::new(AtomicU64::new(0)),
                system_dropped_samples: Arc::new(AtomicU64::new(0)),
                queued_samples: Arc::new(AtomicU64::new(0)),
                queue_max_samples: 0,
            },
            receiver,
        )
    }

    fn live_sink_with_budget(budget_samples: u64) -> (LiveAudioSink, Receiver<LiveAudioChunk>) {
        let (sender, receiver) = mpsc::channel();
        (
            LiveAudioSink {
                sender,
                generation: 0,
                generation_clock: Arc::new(std::sync::Mutex::new(
                    crate::recorder::LiveGenerationClock {
                        generation: 0,
                        session_offset_ms: 0,
                    },
                )),
                mic_accepted_samples: Arc::new(AtomicU64::new(0)),
                system_accepted_samples: Arc::new(AtomicU64::new(0)),
                mic_dropped_samples: Arc::new(AtomicU64::new(0)),
                system_dropped_samples: Arc::new(AtomicU64::new(0)),
                queued_samples: Arc::new(AtomicU64::new(0)),
                queue_max_samples: budget_samples,
            },
            receiver,
        )
    }

    struct TestClock(AtomicU64);

    impl TestClock {
        fn set(&self, nanos: u64) {
            self.0.store(nanos, Ordering::Release);
        }
    }

    impl ClockSource for TestClock {
        fn now_nanos(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }
    }

    fn test_writer(clock: Arc<TestClock>, live: Option<LiveAudioSink>) -> SegmentWriter {
        SegmentWriter::start_with_clock(
            7,
            48_000,
            48_000,
            live,
            Arc::new(SegmentMediaClock::with_source(clock)),
        )
        .unwrap()
    }

    fn send_attachment(
        writer: &SegmentWriter,
        lane: CaptureLane,
        token: u64,
        fence_nanos: u64,
        samples: &[f32],
    ) -> RetireAck {
        let sink = writer.capture_sink(lane, token);
        sink.attach(48_000, fence_nanos).unwrap();
        sink.samples(48_000, samples.to_vec(), Vec::new()).unwrap();
        sink.retire().unwrap()
    }

    #[test]
    fn deferred_writer_locks_first_attach_and_missing_mic_to_system_rate() {
        let writer = SegmentWriter::start_deferred(8, None).unwrap();
        let system = writer.system_sink(1);
        system.attach(44_100, 0).unwrap();
        system.retire().unwrap();
        assert_eq!(writer.system_lane_rate(), 44_100);
        writer
            .set_unattached_lane_rate(CaptureLane::Mic, 44_100)
            .unwrap();
        assert_eq!(writer.mic_lane_rate(), 44_100);
        writer.seal_at(0).unwrap();
    }

    #[test]
    fn gap_fill_attach_retire_reattach_is_exact_on_both_lanes() {
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let writer = test_writer(clock.clone(), None);
        for lane in [CaptureLane::Mic, CaptureLane::System] {
            send_attachment(&writer, lane, 1, 0, &[1.0; 480]);
            send_attachment(&writer, lane, 2, 20_000_000, &[0.5; 480]);
        }
        clock.set(30_000_000);
        let path = tempfile::NamedTempFile::new().unwrap();
        let sealed = writer.seal_at(1_440).unwrap();
        assert_eq!(sealed.mic.samples_written, 1_440);
        assert_eq!(sealed.system.samples_written, 1_440);
        sealed.write_wav(path.path()).unwrap();
        // After writing 1440 frames @ 48kHz, the archive WAV is 16kHz → ≈ 480 frames.
        let reader = hound::WavReader::open(path.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        let total: Vec<i16> = reader
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect();
        // 1440 frames @ 48k → ~480 frames @ 16k; 2 channels → ~960 i16 samples.
        let frame_count = total.len() / 2;
        assert!(
            frame_count >= 460 && frame_count <= 500,
            "expected ~480 archive frames, got {frame_count}"
        );
    }

    #[test]
    fn post_retire_token_samples_are_rejected_and_disqualify_timeline() {
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let writer = test_writer(clock, None);
        let sink = writer.mic_sink(1);
        sink.attach(48_000, 0).unwrap();
        sink.samples(48_000, vec![0.5; 20], Vec::new()).unwrap();
        sink.retire().unwrap();
        let replacement = writer.mic_sink(2);
        replacement.attach(48_000, 416_667).unwrap();
        sink.samples(48_000, vec![0.9; 7], Vec::new()).unwrap();
        replacement
            .samples(48_000, vec![0.5; 7], Vec::new())
            .unwrap();
        replacement.retire().unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let telemetry = writer.telemetry();
        assert_eq!(
            telemetry.mic.rejected_token_samples.load(Ordering::Acquire),
            7
        );
        assert!(!telemetry.timeline_reusable.load(Ordering::Acquire));
        let sealed = writer.seal_at(27).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        sealed.write_wav(file.path()).unwrap();
        // The WAV is now 16kHz/16-bit; verify spec and that the archive has content.
        let reader = hound::WavReader::open(file.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        // 27 frames @ 48kHz → ~9 frames @ 16kHz; 2 channels → ~18 samples.
        let total: Vec<i16> = reader
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert!(total.len() > 0, "archive must not be empty");
        assert_eq!(total.len() % 2, 0, "must be stereo (even sample count)");
    }

    #[test]
    fn detached_ticker_and_attach_keep_live_and_durable_cursors_separate() {
        let clock = Arc::new(TestClock(AtomicU64::new(1)));
        let (sink, rx) = live_sink(32);
        let writer = test_writer(clock.clone(), Some(sink));
        std::thread::sleep(Duration::from_millis(20));
        clock.set(100_000_001);
        std::thread::sleep(Duration::from_millis(120));

        let lane = writer.mic_sink(1);
        lane.attach(48_000, 100_000_000).unwrap();
        lane.samples(48_000, vec![0.75; 480], Vec::new()).unwrap();
        lane.retire().unwrap();
        std::thread::sleep(Duration::from_millis(20));

        let chunks: Vec<_> = rx
            .try_iter()
            .filter(|chunk| chunk.channel == LiveAudioChannel::Mic)
            .collect();
        let silence: usize = chunks
            .iter()
            .take_while(|chunk| chunk.samples.iter().all(|sample| *sample == 0.0))
            .map(|chunk| chunk.samples.len())
            .sum();
        let real: usize = chunks
            .iter()
            .filter(|chunk| chunk.samples.iter().any(|sample| *sample != 0.0))
            .map(|chunk| chunk.samples.len())
            .sum();
        assert_eq!(silence, 4_800, "attach must not duplicate ticker silence");
        assert_eq!(real, 480);
        let telemetry = writer.telemetry();
        assert_eq!(telemetry.mic.durable_frame.load(Ordering::Acquire), 5_280);
        assert_eq!(
            telemetry.mic.live_frame_enqueued.load(Ordering::Acquire),
            5_280
        );
        writer.seal_at(5_280).unwrap();
    }

    #[test]
    fn attached_ticker_preserves_gap_until_first_delivery() {
        let clock = Arc::new(TestClock(AtomicU64::new(1)));
        let writer = test_writer(clock.clone(), None);
        let lane = writer.system_sink(1);
        lane.attach(48_000, 0).unwrap();

        clock.set(300_000_001);
        std::thread::sleep(Duration::from_millis(120));
        lane.samples(48_000, vec![0.75; 480], Vec::new()).unwrap();
        lane.retire().unwrap();

        let sealed = writer.seal_at(14_400).unwrap();
        assert_eq!(sealed.system.samples_written, 14_400);
        let synthesized = sealed.system.samples_written - 480;
        let mut reader = RawF32Reader::open(&sealed.system.path).unwrap();
        for _ in 0..synthesized {
            assert_eq!(reader.next_sample().unwrap(), Some(0.0));
        }
        for _ in 0..480 {
            assert_eq!(reader.next_sample().unwrap(), Some(0.75));
        }
        assert_eq!(reader.next_sample().unwrap(), None);
    }

    #[test]
    fn full_live_queue_leaves_debt_and_real_samples_never_overtake_it() {
        // With unbounded channel + budget enforcement, "full queue" is simulated by
        // pre-setting queued_samples to the budget limit. The test verifies:
        //   - mic samples are dropped (debt) when budget is exhausted
        //   - after budget is freed (queued_samples decremented as a consumer would),
        //     the debt is flushed as a silence chunk before real samples.
        let clock = Arc::new(TestClock(AtomicU64::new(1)));
        // Budget = 480 samples; queued_samples pre-set to budget-full so the
        // 480-sample mic write is blocked (dropped → debt). The pre-sent system
        // chunk keeps rx non-empty so rx.recv() below has something to drain.
        let (sink, rx) = live_sink_with_budget(480);
        // Pre-send a system chunk to occupy rx (so the test can drain it below).
        sink.sender
            .send(LiveAudioChunk {
                channel: LiveAudioChannel::System,
                generation: 0,
                session_offset_ms: 0,
                sample_rate: 48_000,
                samples: vec![9.0],
            })
            .unwrap();
        // Pre-fill the budget counter to simulate the channel being "full".
        let queued_samples = Arc::clone(&sink.queued_samples);
        queued_samples.store(480, Ordering::SeqCst);
        let writer = test_writer(clock, Some(sink));
        let lane = writer.mic_sink(1);
        lane.attach(48_000, 0).unwrap();
        lane.samples(48_000, vec![0.5; 480], Vec::new()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let telemetry = writer.telemetry();
        assert_eq!(telemetry.mic.durable_frame.load(Ordering::Acquire), 480);
        assert_eq!(telemetry.mic.live_frame_enqueued.load(Ordering::Acquire), 0);

        let occupied = rx.recv().unwrap();
        assert_eq!(occupied.samples, vec![9.0]);
        // Simulate consumer draining: decrement queued_samples to 0.
        queued_samples.store(0, Ordering::SeqCst);
        lane.samples(48_000, vec![0.6; 10], Vec::new()).unwrap();
        let debt_chunk = rx.recv_timeout(Duration::from_millis(150)).unwrap();
        assert_eq!(debt_chunk.channel, LiveAudioChannel::Mic);
        assert!(debt_chunk.samples.iter().all(|sample| *sample == 0.0));
        assert_eq!(debt_chunk.samples.len(), 480);
        lane.retire().unwrap();
        writer.seal_at(490).unwrap();
    }

    #[test]
    fn reset_on_attach_prevents_interpolation_across_gap() {
        let mut first = RationalResampler::new(24_000, 48_000).unwrap();
        let mut before = first.process(&[1.0; 20]).unwrap();
        before.extend(first.finish().unwrap());
        let mut second = RationalResampler::new(24_000, 48_000).unwrap();
        let mut after = second.process(&[-1.0; 20]).unwrap();
        after.extend(second.finish().unwrap());
        assert!(before.iter().all(|v| *v == 1.0));
        assert!(after.iter().all(|v| *v == -1.0));
    }

    #[test]
    fn late_tick_freezes_media_time_without_silence_burst() {
        let clock = Arc::new(TestClock(AtomicU64::new(1)));
        let writer = test_writer(clock.clone(), None);
        std::thread::sleep(Duration::from_millis(20));
        clock.set(6_000_000_000);
        std::thread::sleep(Duration::from_millis(120));
        let telemetry = writer.telemetry();
        assert!(!telemetry.timeline_reusable.load(Ordering::Acquire));
        assert!(
            telemetry
                .mic
                .suspend_discontinuities
                .load(Ordering::Acquire)
                + telemetry
                    .system
                    .suspend_discontinuities
                    .load(Ordering::Acquire)
                >= 1
        );
        assert!(writer.media_frame().unwrap() < 48_000);
        writer.seal_at(0).unwrap();
    }

    #[test]
    fn seal_and_finalize_writes_16k_16bit_stereo() {
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let writer = test_writer(clock, None);
        // Write 64 mic samples and 32 system samples at 48kHz.
        send_attachment(&writer, CaptureLane::Mic, 1, 0, &[0.25; 64]);
        send_attachment(&writer, CaptureLane::System, 2, 0, &[-0.5; 32]);
        let sealed = writer.seal_at(64).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        sealed.write_wav(file.path()).unwrap();
        let reader = hound::WavReader::open(file.path()).unwrap();
        // WAV must be 16kHz, 16-bit signed int, 2-channel.
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        // Resampled from 64 frames @ 48kHz → ≈ 21 frames @ 16kHz (2:6 ratio).
        let total_samples: Vec<i16> = reader
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect();
        // 2 channels: total must be even, and > 0.
        assert!(total_samples.len() > 0);
        assert_eq!(total_samples.len() % 2, 0);
        // Channel order: mic (ch0) positive, system (ch1) negative (for non-silent portion).
        let mic_samples: Vec<i16> = total_samples.iter().copied().step_by(2).collect();
        let sys_samples: Vec<i16> = total_samples.iter().copied().skip(1).step_by(2).collect();
        assert!(
            mic_samples.iter().any(|&s| s > 0),
            "mic channel must contain positive samples"
        );
        assert!(
            sys_samples.iter().any(|&s| s < 0),
            "system channel must contain negative samples"
        );
    }
}
