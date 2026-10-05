use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SizedSample;
#[cfg(target_os = "windows")]
use cpal::{FromSample, Sample};
use dasp::sample::ToSample;
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod coreaudio_mic;
mod segment_writer;
pub use segment_writer::{
    target_frame, CaptureLane, CaptureSink, CaptureToken, LaneTelemetry, MicSink,
    NativeSpoolSource, PacketDesc, RationalResampler, RetireAck, SealedSegment, SegmentId,
    SegmentWriter, SegmentWriterTelemetry, SystemSink, DRAIN_BATCH_MILLIS, LANE_QUEUE_CAPACITY,
};

pub type InputDevice = cpal::Device;

#[cfg(target_os = "macos")]
use cidre::{arc, av, cat, cf, core_audio as ca, mach, ns, os};
#[cfg(target_os = "windows")]
use windows::Win32::{
    Media::{
        Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
        },
        KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE},
        Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT},
    },
    System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    },
};

/// Name of the virtual audio tap device the app creates for system-audio capture.
/// Published so callers can filter it from microphone picker lists.
pub const TAP_NAME: &str = "margins-tap";

/// +20 dB gain applied to mic input. Raw hardware levels from built-in macs
/// and USB mics (e.g. Yeti at moderate gain) typically sit around -50 to -40
/// dBFS RMS; this brings speech into the -30 to -20 dBFS range.
const MIC_GAIN: f32 = 10.0;

/// Pre-allocated ring buffer capacity: 10 seconds of audio at 48kHz.
/// Gives the consumer thread plenty of slack to drain without drops,
/// even if macOS timer coalescing stretches the drain interval.
const RING_BUF_SECONDS: usize = 10;

/// Makes spool paths collision-free even when multiple capture/test threads
/// obtain the same coarse filesystem clock tick.
#[cfg(test)]
static SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// How often the consumer thread drains the ring buffer.
const DRAIN_INTERVAL: Duration = Duration::from_millis(5);

/// Threshold for detecting a dead Core Audio tap. A live tap with no audio still
/// produces noise-floor samples (~0.0002); a dead tap delivers exact 0.0 values.
/// Using a very small epsilon to account for floating-point artifacts.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const DEAD_TAP_THRESHOLD: f32 = 1e-7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveAudioChannel {
    Mic,
    System,
}

#[derive(Debug, Clone)]
pub struct LiveAudioChunk {
    pub channel: LiveAudioChannel,
    pub generation: u64,
    /// Wall-clock position of this generation on the capture session timeline.
    pub session_offset_ms: u64,
    pub sample_rate: u32,
    /// Position of the first sample in this generation's lane spool, in
    /// `sample_rate` frames. Durable runtime audio uses the same frame axis.
    pub start_frame: u64,
    /// Silence substituted for real audio that missed the bounded live queue.
    /// The durable runtime still holds the real audio for this span.
    pub synthesized: bool,
    pub samples: Vec<f32>,
}

#[derive(Clone)]
pub struct LiveAudioSink {
    pub sender: Sender<LiveAudioChunk>,
    pub generation: u64,
    pub generation_clock: Arc<Mutex<LiveGenerationClock>>,
    pub mic_accepted_samples: Arc<AtomicU64>,
    pub system_accepted_samples: Arc<AtomicU64>,
    pub mic_dropped_samples: Arc<AtomicU64>,
    pub system_dropped_samples: Arc<AtomicU64>,
    pub queued_samples: Arc<AtomicU64>,
    /// Maximum number of samples that may be queued before drops are counted.
    /// Zero means unbounded (no budget enforcement).
    pub queue_max_samples: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveGenerationClock {
    pub generation: u64,
    pub session_offset_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MicStreamErrorKind {
    DeviceNotAvailable = 1,
    BackendSpecific = 2,
    Stalled = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneAuthorization {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

#[cfg(target_os = "macos")]
pub fn microphone_authorization() -> Result<MicrophoneAuthorization> {
    use av::AuthorizationStatus;

    let status = av::CaptureDevice::authorization_status_for_media_type(av::MediaType::audio())
        .map_err(|error| anyhow::anyhow!("could not query microphone permission: {error}"))?;
    Ok(match status {
        AuthorizationStatus::NotDetermined => MicrophoneAuthorization::NotDetermined,
        AuthorizationStatus::Restricted => MicrophoneAuthorization::Restricted,
        AuthorizationStatus::Denied => MicrophoneAuthorization::Denied,
        AuthorizationStatus::Authorized => MicrophoneAuthorization::Authorized,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn microphone_authorization() -> Result<MicrophoneAuthorization> {
    Ok(MicrophoneAuthorization::Authorized)
}

/// Show the macOS microphone permission prompt and block until the user
/// answers, returning whether access was granted. TCC microphone permission
/// is per-app, so the grant covers every input device no matter which one is
/// selected when the prompt appears. Only meaningful when authorization is
/// NotDetermined. Pump the caller's run loop while waiting because macOS can
/// deliver the completion there.
#[cfg(target_os = "macos")]
pub fn request_microphone_access() -> Result<bool> {
    use cidre::blocks;
    use cidre::cf;
    use std::sync::mpsc::TryRecvError;

    let (tx, rx) = std::sync::mpsc::channel();
    let mut block = blocks::SendBlock::new1(move |granted: bool| {
        let _ = tx.send(granted);
    });
    av::CaptureDevice::request_access_for_media_type_ch(av::MediaType::audio(), &mut block)
        .map_err(|error| anyhow::anyhow!("could not request microphone permission: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match rx.try_recv() {
            Ok(granted) => return Ok(granted),
            Err(TryRecvError::Disconnected) => {
                bail!("microphone permission callback was disconnected")
            }
            Err(TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            bail!("microphone permission prompt did not complete within 90 seconds")
        }
        // The completion can be delivered on this thread's run loop. A
        // blocking channel receive would prevent that callback from running.
        let outcome = cf::RunLoop::run_in_mode(cf::RunLoopMode::default(), 0.1, true);
        if outcome == cf::RunLoopRunResult::Finished {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request_microphone_access() -> Result<bool> {
    Ok(true)
}

impl MicStreamErrorKind {
    fn from_stream_error(err: &cpal::StreamError) -> Self {
        match err {
            cpal::StreamError::DeviceNotAvailable => Self::DeviceNotAvailable,
            cpal::StreamError::BackendSpecific { .. } => Self::BackendSpecific,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::DeviceNotAvailable),
            2 => Some(Self::BackendSpecific),
            3 => Some(Self::Stalled),
            _ => None,
        }
    }
}

pub fn take_mic_stream_error(flag: &AtomicU8) -> Option<MicStreamErrorKind> {
    MicStreamErrorKind::from_code(flag.swap(0, Ordering::SeqCst))
}

#[derive(Clone)]
pub struct MicCaptureTelemetry {
    pub peak: Arc<AtomicU32>,
    pub drops: Arc<AtomicU64>,
    pub packet_drops: Arc<AtomicU64>,
    pub frames: Arc<AtomicU64>,
    /// Consecutive callback frames containing exact digital zero.
    pub silence: Arc<AtomicU64>,
    pub error: Arc<AtomicU8>,
}

fn next_exact_zero_run(run: u64, sample: f32) -> u64 {
    if sample == 0.0 {
        run.saturating_add(1)
    } else {
        0
    }
}

#[derive(Clone)]
pub struct SystemCaptureTelemetry {
    pub peak: Arc<AtomicU32>,
    pub drops: Arc<AtomicU64>,
    pub packet_drops: Arc<AtomicU64>,
    pub silence: Arc<AtomicU64>,
    pub frames: Arc<AtomicU64>,
}

/// Microphone selection passed to the backend seam. Slice E can add a native
/// CoreAudio implementation behind `MicStream` without changing capture,
/// drain, writer, or controller contracts.
pub struct MicTarget<'a> {
    device: Option<&'a InputDevice>,
    stable_uid: Option<String>,
}

impl<'a> MicTarget<'a> {
    pub fn system_default() -> Self {
        Self {
            device: None,
            stable_uid: None,
        }
    }

    pub fn device(device: &'a InputDevice) -> Self {
        Self {
            device: Some(device),
            stable_uid: None,
        }
    }

    pub fn from_optional(device: Option<&'a InputDevice>) -> Self {
        Self {
            device,
            stable_uid: None,
        }
    }

    /// Attach the stable platform UID resolved by the device registry.
    /// Existing callers may omit it; the cpal backend ignores this field.
    pub fn with_stable_uid(mut self, uid: impl Into<String>) -> Self {
        self.stable_uid = Some(uid.into());
        self
    }

    pub fn stable_uid(&self) -> Option<&str> {
        self.stable_uid.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicBackendKind {
    Cpal,
    CoreAudio,
}

impl MicBackendKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpal => "cpal",
            Self::CoreAudio => "coreaudio",
        }
    }

    pub fn alternate(self) -> Self {
        match self {
            Self::Cpal => Self::CoreAudio,
            Self::CoreAudio => Self::Cpal,
        }
    }
}

enum MicStream {
    Cpal(cpal::Stream),
    #[cfg(target_os = "macos")]
    CoreAudio(coreaudio_mic::CoreAudioMicStream),
}

impl MicStream {
    fn backend(&self) -> MicBackendKind {
        match self {
            Self::Cpal(_) => MicBackendKind::Cpal,
            #[cfg(target_os = "macos")]
            Self::CoreAudio(_) => MicBackendKind::CoreAudio,
        }
    }

    fn stop(self) -> Result<()> {
        match self {
            Self::Cpal(stream) => {
                drop(stream);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            Self::CoreAudio(stream) => stream.stop(),
        }
    }
}

enum DrainCommand {
    CommitStandby(std::sync::mpsc::Sender<Result<()>>),
}

#[derive(Debug, Clone, Copy)]
struct DrainOutcome {
    retire: Option<RetireAck>,
}

struct RawMicCapture {
    stream: MicStream,
    native_rate: u32,
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<PacketDesc>,
}

struct RawSystemCapture {
    capture: SpeakerCapture,
    native_rate: u32,
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<PacketDesc>,
}

/// Independently owned microphone capture.
///
/// This value, including its `cpal::Stream`, must stay on the controller
/// thread that created it (`cpal::Stream` is `!Send` on macOS). The native
/// callback only converts/downmixs, pushes into the two SPSC rings, and updates
/// atomics. The drain thread owns all allocation and bounded lane sends.
pub struct MicCapture {
    stop_flag: Arc<AtomicBool>,
    quiescing: bool,
    stream: Option<MicStream>,
    consumer_handle: Option<std::thread::JoinHandle<Result<DrainOutcome>>>,
    standby_command: SyncSender<DrainCommand>,
    first_packet_seen: Arc<AtomicBool>,
    native_rate: u32,
    token: CaptureToken,
    telemetry: MicCaptureTelemetry,
    backend: MicBackendKind,
    health: MicProgressHealth,
}

const MIC_STALL_TIMEOUT: Duration = Duration::from_secs(2);

struct MicProgressHealth {
    last_frames: u64,
    last_progress_at: Instant,
}

impl MicProgressHealth {
    fn new(now: Instant) -> Self {
        Self {
            last_frames: 0,
            last_progress_at: now,
        }
    }

    fn stalled(&mut self, now: Instant, frames: u64, enabled: bool) -> bool {
        if frames != self.last_frames {
            self.last_frames = frames;
            self.last_progress_at = now;
            return false;
        }
        enabled && now.duration_since(self.last_progress_at) >= MIC_STALL_TIMEOUT
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetiredMic {
    pub token: CaptureToken,
    pub boundary_frame: Option<u64>,
    pub native_rate: u32,
}

impl MicCapture {
    pub fn start(target: &MicTarget<'_>, sink: MicSink) -> Result<Self> {
        Self::start_with_error_flag(target, sink, Arc::new(AtomicU8::new(0)))
    }

    pub fn start_with_error_flag(
        target: &MicTarget<'_>,
        sink: MicSink,
        error: Arc<AtomicU8>,
    ) -> Result<Self> {
        Self::start_mode(target, sink, error, false)
    }

    pub fn start_standby(target: &MicTarget<'_>, sink: MicSink) -> Result<Self> {
        Self::start_mode(target, sink, Arc::new(AtomicU8::new(0)), true)
    }

    pub fn start_preferred(
        target: &MicTarget<'_>,
        sink: MicSink,
        preferred: MicBackendKind,
    ) -> Result<Self> {
        Self::start_mode_preferred(target, sink, Arc::new(AtomicU8::new(0)), false, preferred)
    }

    pub fn start_standby_preferred(
        target: &MicTarget<'_>,
        sink: MicSink,
        preferred: MicBackendKind,
    ) -> Result<Self> {
        Self::start_mode_preferred(target, sink, Arc::new(AtomicU8::new(0)), true, preferred)
    }

    fn start_mode(
        target: &MicTarget<'_>,
        sink: MicSink,
        error: Arc<AtomicU8>,
        standby: bool,
    ) -> Result<Self> {
        Self::start_mode_preferred(target, sink, error, standby, MicBackendKind::Cpal)
    }

    fn start_mode_preferred(
        target: &MicTarget<'_>,
        sink: MicSink,
        error: Arc<AtomicU8>,
        standby: bool,
        preferred: MicBackendKind,
    ) -> Result<Self> {
        if sink.lane() != CaptureLane::Mic {
            bail!("MicCapture requires a mic lane sink");
        }
        let telemetry = MicCaptureTelemetry {
            peak: Arc::new(AtomicU32::new(0)),
            drops: Arc::new(AtomicU64::new(0)),
            packet_drops: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
            silence: Arc::new(AtomicU64::new(0)),
            error,
        };
        let raw = start_mic_raw_for_backend(target, &telemetry, preferred).or_else(|primary| {
            start_mic_raw_for_backend(target, &telemetry, preferred.alternate()).map_err(
                |fallback| {
                    anyhow::anyhow!(
                        "{} microphone backend failed: {primary:#}; {} fallback failed: {fallback:#}",
                        preferred.name(),
                        preferred.alternate().name(),
                    )
                },
            )
        })?;
        Self::from_raw(raw, sink, telemetry, standby)
    }

    fn from_raw(
        raw: RawMicCapture,
        sink: MicSink,
        telemetry: MicCaptureTelemetry,
        standby: bool,
    ) -> Result<Self> {
        let native_rate = raw.native_rate;
        let backend = raw.stream.backend();
        let token = sink.token();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let first_packet_seen = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = std::sync::mpsc::sync_channel(2);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let drain_stop = stop_flag.clone();
        let drain_first = first_packet_seen.clone();
        let consumer_handle = std::thread::Builder::new()
            .name("margins-mic-drain".into())
            .spawn(move || {
                drain_capture_to_lane(
                    raw.samples,
                    raw.packets,
                    native_rate,
                    sink,
                    drain_stop,
                    command_rx,
                    drain_first,
                    standby,
                    ready_tx,
                )
            })?;
        match ready_rx.recv().context("mic drain did not initialize")? {
            Ok(()) => Ok(Self {
                stop_flag,
                quiescing: false,
                stream: Some(raw.stream),
                consumer_handle: Some(consumer_handle),
                standby_command: command_tx,
                first_packet_seen,
                native_rate,
                token,
                telemetry,
                backend,
                health: MicProgressHealth::new(Instant::now()),
            }),
            Err(error) => {
                let _ = raw.stream.stop();
                stop_flag.store(true, Ordering::Release);
                let _ = consumer_handle.join();
                Err(error)
            }
        }
    }

    pub fn native_rate(&self) -> u32 {
        self.native_rate
    }

    pub fn token(&self) -> CaptureToken {
        self.token
    }

    pub fn telemetry(&self) -> MicCaptureTelemetry {
        self.telemetry.clone()
    }

    pub fn backend(&self) -> MicBackendKind {
        self.backend
    }

    /// Poll native stream errors and sample delivery. Error callbacks are not
    /// guaranteed when a device silently stops invoking its input callback, so
    /// active captures also use forward progress as a backend-independent
    /// liveness signal.
    pub fn take_error(&mut self, detect_stall: bool) -> Option<MicStreamErrorKind> {
        if let Some(error) = take_mic_stream_error(&self.telemetry.error) {
            return Some(error);
        }
        let frames = self.telemetry.frames.load(Ordering::Acquire);
        if self.health.stalled(Instant::now(), frames, detect_stall) {
            return Some(MicStreamErrorKind::Stalled);
        }
        None
    }

    pub fn first_packet_seen(&self) -> Arc<AtomicBool> {
        self.first_packet_seen.clone()
    }

    /// Commit a confirmed standby capture. The drain purges both rings,
    /// records the writer media-clock fence, sends `Attach` next, and only then
    /// forwards post-fence pops.
    pub fn commit_standby(&self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.standby_command
            .send(DrainCommand::CommitStandby(tx))
            .context("standby drain stopped before commit")?;
        rx.recv().context("standby drain dropped commit ACK")??;
        Ok(())
    }

    /// Producer quiesce -> drain finish -> final Samples -> Retire -> join ->
    /// lane ACK. No system-capture state is touched.
    pub fn retire(mut self) -> Result<RetiredMic> {
        self.quiescing = true;
        // Quiesce the native producer before allowing the drain to finish.
        if let Some(stream) = self.stream.take() {
            stream.stop()?;
        }
        self.stop_flag.store(true, Ordering::Release);
        let outcome = self
            .consumer_handle
            .take()
            .context("mic drain already retired")?
            .join()
            .map_err(|_| anyhow::anyhow!("mic drain panicked"))??;
        Ok(RetiredMic {
            token: self.token,
            boundary_frame: outcome.retire.map(|ack| ack.boundary_frame),
            native_rate: self.native_rate,
        })
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.stop();
        }
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.consumer_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Independently owned system-audio capture. On Windows the platform producer
/// thread is joined by `SpeakerCapture::drop` before the drain is allowed to
/// finish, matching the same retirement protocol as microphone capture.
pub struct SystemCapture {
    stop_flag: Arc<AtomicBool>,
    quiescing: bool,
    capture: Option<SpeakerCapture>,
    consumer_handle: Option<std::thread::JoinHandle<Result<DrainOutcome>>>,
    standby_command: SyncSender<DrainCommand>,
    first_packet_seen: Arc<AtomicBool>,
    native_rate: u32,
    token: CaptureToken,
    telemetry: SystemCaptureTelemetry,
}

#[derive(Debug, Clone, Copy)]
pub struct RetiredSystem {
    pub token: CaptureToken,
    pub boundary_frame: Option<u64>,
    pub native_rate: u32,
}

impl SystemCapture {
    pub fn start(sink: SystemSink) -> Result<Self> {
        Self::start_mode(sink, false)
    }

    pub fn start_standby(sink: SystemSink) -> Result<Self> {
        Self::start_mode(sink, true)
    }

    fn start_mode(sink: SystemSink, standby: bool) -> Result<Self> {
        if sink.lane() != CaptureLane::System {
            bail!("SystemCapture requires a system lane sink");
        }
        let telemetry = SystemCaptureTelemetry {
            peak: Arc::new(AtomicU32::new(0)),
            drops: Arc::new(AtomicU64::new(0)),
            packet_drops: Arc::new(AtomicU64::new(0)),
            silence: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
        };
        let raw = start_speaker_raw(&telemetry)?;
        Self::from_raw(raw, sink, telemetry, standby)
    }

    fn from_raw(
        raw: RawSystemCapture,
        sink: SystemSink,
        telemetry: SystemCaptureTelemetry,
        standby: bool,
    ) -> Result<Self> {
        let native_rate = raw.native_rate;
        let token = sink.token();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let first_packet_seen = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = std::sync::mpsc::sync_channel(2);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let drain_stop = stop_flag.clone();
        let drain_first = first_packet_seen.clone();
        let consumer_handle = std::thread::Builder::new()
            .name("margins-system-drain".into())
            .spawn(move || {
                drain_capture_to_lane(
                    raw.samples,
                    raw.packets,
                    native_rate,
                    sink,
                    drain_stop,
                    command_rx,
                    drain_first,
                    standby,
                    ready_tx,
                )
            })?;
        match ready_rx.recv().context("system drain did not initialize")? {
            Ok(()) => Ok(Self {
                stop_flag,
                quiescing: false,
                capture: Some(raw.capture),
                consumer_handle: Some(consumer_handle),
                standby_command: command_tx,
                first_packet_seen,
                native_rate,
                token,
                telemetry,
            }),
            Err(error) => {
                drop(raw.capture);
                stop_flag.store(true, Ordering::Release);
                let _ = consumer_handle.join();
                Err(error)
            }
        }
    }

    pub fn native_rate(&self) -> u32 {
        self.native_rate
    }

    pub fn token(&self) -> CaptureToken {
        self.token
    }

    pub fn telemetry(&self) -> SystemCaptureTelemetry {
        self.telemetry.clone()
    }

    pub fn first_packet_seen(&self) -> Arc<AtomicBool> {
        self.first_packet_seen.clone()
    }

    pub fn commit_standby(&self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.standby_command
            .send(DrainCommand::CommitStandby(tx))
            .context("standby drain stopped before commit")?;
        rx.recv().context("standby drain dropped commit ACK")??;
        Ok(())
    }

    pub fn retire(mut self) -> Result<RetiredSystem> {
        self.quiescing = true;
        drop(self.capture.take());
        self.stop_flag.store(true, Ordering::Release);
        let outcome = self
            .consumer_handle
            .take()
            .context("system drain already retired")?
            .join()
            .map_err(|_| anyhow::anyhow!("system drain panicked"))??;
        Ok(RetiredSystem {
            token: self.token,
            boundary_frame: outcome.retire.map(|ack| ack.boundary_frame),
            native_rate: self.native_rate,
        })
    }
}

impl Drop for SystemCapture {
    fn drop(&mut self) {
        drop(self.capture.take());
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.consumer_handle.take() {
            let _ = handle.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_capture_to_lane(
    mut samples: rtrb::Consumer<f32>,
    mut packets: rtrb::Consumer<PacketDesc>,
    native_rate: u32,
    sink: CaptureSink,
    stop: Arc<AtomicBool>,
    commands: std::sync::mpsc::Receiver<DrainCommand>,
    first_packet_seen: Arc<AtomicBool>,
    mut standby: bool,
    ready: SyncSender<Result<()>>,
) -> Result<DrainOutcome> {
    set_drain_thread_qos();
    let mut attached = false;
    if !standby {
        // Stream construction may begin callback delivery before the drain
        // thread is scheduled. Purge that pre-attach backlog, then take the
        // writer-owned fence, exactly like a standby commit.
        purge_capture_rings(&mut samples, &mut packets);
        let result = sink.attach(native_rate, sink.media_fence_nanos());
        match result {
            Ok(()) => {
                attached = true;
                let _ = ready.send(Ok(()));
            }
            Err(error) => {
                let message = format!("lane attach failed: {error:#}");
                let _ = ready.send(Err(anyhow::anyhow!(message.clone())));
                bail!(message);
            }
        }
    } else {
        let _ = ready.send(Ok(()));
    }

    let batch_frames = ((u64::from(native_rate) * DRAIN_BATCH_MILLIS) / 1_000).max(1) as usize;
    loop {
        if let Ok(DrainCommand::CommitStandby(reply)) = commands.try_recv() {
            if !standby || attached {
                let _ = reply.send(Err(anyhow::anyhow!("capture is not in standby")));
            } else {
                purge_capture_rings(&mut samples, &mut packets);
                let fence = sink.media_fence_nanos();
                let result = sink.attach(native_rate, fence);
                if result.is_ok() {
                    standby = false;
                    attached = true;
                }
                let _ = reply.send(result);
            }
        }

        if standby {
            let saw = purge_capture_rings(&mut samples, &mut packets);
            if saw {
                first_packet_seen.store(true, Ordering::Release);
            }
        } else {
            drain_one_batch(
                &mut samples,
                &mut packets,
                native_rate,
                batch_frames,
                &sink,
                &first_packet_seen,
            )?;
        }

        if stop.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(DRAIN_INTERVAL);
    }

    if standby {
        purge_capture_rings(&mut samples, &mut packets);
        return Ok(DrainOutcome { retire: None });
    }
    while !samples.is_empty() || !packets.is_empty() {
        drain_one_batch(
            &mut samples,
            &mut packets,
            native_rate,
            batch_frames,
            &sink,
            &first_packet_seen,
        )?;
    }
    let retire = if attached { Some(sink.retire()?) } else { None };
    Ok(DrainOutcome { retire })
}

fn drain_one_batch(
    samples: &mut rtrb::Consumer<f32>,
    packets: &mut rtrb::Consumer<PacketDesc>,
    native_rate: u32,
    batch_frames: usize,
    sink: &CaptureSink,
    first_packet_seen: &AtomicBool,
) -> Result<()> {
    let mut batch = Vec::with_capacity(batch_frames);
    while batch.len() < batch_frames {
        match samples.pop() {
            Ok(sample) => batch.push(sample),
            Err(_) => break,
        }
    }
    let mut packet_batch = Vec::new();
    let mut described = 0usize;
    while described < batch.len() || (batch.is_empty() && packet_batch.is_empty()) {
        match packets.pop() {
            Ok(packet) => {
                described = described.saturating_add(packet.frame_count as usize);
                packet_batch.push(packet);
            }
            Err(_) => break,
        }
    }
    if !batch.is_empty() {
        first_packet_seen.store(true, Ordering::Release);
        sink.samples(native_rate, batch, packet_batch)?;
    } else if !packet_batch.is_empty() {
        first_packet_seen.store(true, Ordering::Release);
    }
    Ok(())
}

fn purge_capture_rings(
    samples: &mut rtrb::Consumer<f32>,
    packets: &mut rtrb::Consumer<PacketDesc>,
) -> bool {
    let mut saw = false;
    while samples.pop().is_ok() {
        saw = true;
    }
    while packets.pop().is_ok() {
        saw = true;
    }
    saw
}

fn set_drain_thread_qos() {
    #[cfg(target_os = "macos")]
    {
        use std::os::raw::c_int;
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: c_int, relative_priority: c_int) -> c_int;
        }
        const QOS_CLASS_USER_INTERACTIVE: c_int = 0x21;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
        }
    }
}

/// Backwards-compatible combined recorder facade. Desktop Slice B consumes the
/// independent capture/writer primitives directly; this facade intentionally
/// remains fail-fast if either native input cannot start.
pub struct RecorderHandle {
    external_stop_flag: Arc<AtomicBool>,
    mic: MicCapture,
    system: SystemCapture,
    writer: SegmentWriter,
    spk_rate: u32,
    mic_telemetry: MicCaptureTelemetry,
    system_telemetry: SystemCaptureTelemetry,
    opened_mic_name: String,
    opened_mic_uid: Option<String>,
}

impl RecorderHandle {
    pub fn start(stop_flag: Arc<AtomicBool>, device: Option<&InputDevice>) -> Result<Self> {
        Self::start_with_live_audio(stop_flag, device, None)
    }

    pub fn start_with_live_audio(
        stop_flag: Arc<AtomicBool>,
        device: Option<&InputDevice>,
        live_audio: Option<LiveAudioSink>,
    ) -> Result<Self> {
        Self::start_with_identity(
            stop_flag,
            device,
            None,
            live_audio,
            Arc::new(AtomicU8::new(0)),
        )
    }

    pub fn start_with_selected_audio(
        stop_flag: Arc<AtomicBool>,
        selected: Option<&SelectedInputDevice>,
        live_audio: Option<LiveAudioSink>,
    ) -> Result<Self> {
        Self::start_with_identity(
            stop_flag,
            selected.map(|selection| &selection.device),
            selected,
            live_audio,
            Arc::new(AtomicU8::new(0)),
        )
    }

    pub fn start_with_live_audio_and_mic_error(
        stop_flag: Arc<AtomicBool>,
        device: Option<&InputDevice>,
        live_audio: Option<LiveAudioSink>,
        mic_error: Arc<AtomicU8>,
    ) -> Result<Self> {
        Self::start_with_identity(stop_flag, device, None, live_audio, mic_error)
    }

    fn start_with_identity(
        stop_flag: Arc<AtomicBool>,
        device: Option<&InputDevice>,
        selected: Option<&SelectedInputDevice>,
        live_audio: Option<LiveAudioSink>,
        mic_error: Arc<AtomicU8>,
    ) -> Result<Self> {
        let mic_telemetry = MicCaptureTelemetry {
            peak: Arc::new(AtomicU32::new(0)),
            drops: Arc::new(AtomicU64::new(0)),
            packet_drops: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
            silence: Arc::new(AtomicU64::new(0)),
            error: mic_error,
        };
        let system_telemetry = SystemCaptureTelemetry {
            peak: Arc::new(AtomicU32::new(0)),
            drops: Arc::new(AtomicU64::new(0)),
            packet_drops: Arc::new(AtomicU64::new(0)),
            silence: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
        };
        let default_uid = if selected.is_none() && device.is_none() {
            default_input_device_uid()
        } else {
            None
        };
        let raw_mic = start_mic_raw(device, &mic_telemetry)?;
        let raw_system = start_speaker_raw(&system_telemetry)?;
        let sample_rate = raw_mic.native_rate;
        let spk_rate = raw_system.native_rate;
        let opened_name = device
            .and_then(|device| device.name().ok())
            .or_else(default_input_device_name)
            .unwrap_or_else(|| "unknown".into());
        let occurrence = selected.map_or(0, |selection| selection.occurrence);
        let opened_uid = if selected.is_none() && device.is_none() {
            default_uid
        } else {
            selected
                .and_then(|selection| selection.uid.clone())
                .or_else(|| input_device_uid_at(&opened_name, occurrence))
        };
        let opened_uid_label = opened_uid.as_deref().unwrap_or("unknown");
        let requested_name = selected.map_or("system default", |selection| selection.name.as_str());
        let requested_uid = selected
            .and_then(|selection| selection.uid.as_deref())
            .unwrap_or("default");
        let writer = SegmentWriter::start(0, sample_rate, spk_rate, live_audio)?;
        let mic = MicCapture::from_raw(raw_mic, writer.mic_sink(1), mic_telemetry.clone(), false)?;
        let system = match SystemCapture::from_raw(
            raw_system,
            writer.system_sink(2),
            system_telemetry.clone(),
            false,
        ) {
            Ok(system) => system,
            Err(error) => {
                let _ = mic.retire();
                return Err(error);
            }
        };
        crate::cli_log::event(
            "capture_rates",
            format!("mic={sample_rate} speaker={spk_rate}"),
        );
        crate::cli_log::event(
            "capture_device_opened",
            format!(
                "requested_name={requested_name:?} requested_uid={requested_uid:?} requested_occurrence={occurrence} opened_name={opened_name:?} opened_uid={opened_uid_label:?} mic_rate={sample_rate} system_rate={spk_rate}"
            ),
        );
        if sample_rate != spk_rate {
            // Each lane retains its own native rate. Seal maps the shared
            // timeline boundary into both rates; the runtime then resamples
            // each lane to 16 kHz independently.
            crate::cli_log::event(
                "capture_rate_mismatch",
                format!("mic={sample_rate} speaker={spk_rate} using={sample_rate}"),
            );
        }
        Ok(Self {
            external_stop_flag: stop_flag,
            mic,
            system,
            writer,
            spk_rate,
            mic_telemetry,
            system_telemetry,
            opened_mic_name: opened_name,
            opened_mic_uid: opened_uid,
        })
    }

    pub fn mic_name(&self) -> &str {
        &self.opened_mic_name
    }

    pub fn mic_uid(&self) -> Option<&str> {
        self.opened_mic_uid.as_deref()
    }

    pub fn mic_peak(&self) -> Arc<AtomicU32> {
        self.mic_telemetry.peak.clone()
    }

    pub fn spk_peak(&self) -> Arc<AtomicU32> {
        self.system_telemetry.peak.clone()
    }

    pub fn mic_drops(&self) -> Arc<AtomicU64> {
        self.mic_telemetry.drops.clone()
    }

    pub fn mic_frames(&self) -> Arc<AtomicU64> {
        self.mic_telemetry.frames.clone()
    }

    pub fn mic_silence(&self) -> Arc<AtomicU64> {
        self.mic_telemetry.silence.clone()
    }

    pub fn mic_rate(&self) -> u32 {
        self.mic.native_rate()
    }

    pub fn real_spool_frames(&self, lane: CaptureLane) -> Arc<AtomicU64> {
        let telemetry = self.writer.telemetry();
        match lane {
            CaptureLane::Mic => telemetry.mic.real_spool_frames.clone(),
            CaptureLane::System => telemetry.system.real_spool_frames.clone(),
        }
    }

    pub fn spk_drops(&self) -> Arc<AtomicU64> {
        self.system_telemetry.drops.clone()
    }

    pub fn spk_silence(&self) -> Arc<AtomicU64> {
        self.system_telemetry.silence.clone()
    }

    pub fn spk_frames(&self) -> Arc<AtomicU64> {
        self.system_telemetry.frames.clone()
    }

    pub fn spk_rate(&self) -> u32 {
        self.spk_rate
    }

    pub fn native_spool_sources(&self) -> [NativeSpoolSource; 2] {
        self.writer.native_spool_sources()
    }

    pub fn bound_native_spool(&self) -> Result<Arc<AtomicBool>> {
        let overflow = Arc::new(AtomicBool::new(false));
        self.writer.bound_spool(60, overflow.clone())?;
        Ok(overflow)
    }

    pub fn stop_and_write(self, path: &str) -> Result<f64> {
        let sealed = self.stop_capture_and_seal()?;
        let duration = sealed.write_wav(path)?;
        if duration == 0.0 {
            eprintln!("No audio captured.");
        } else {
            crate::cli_log::event(
                "capture_wav_written",
                format!("duration_s={duration:.1} path={path}"),
            );
        }
        Ok(duration)
    }

    /// Seal the native spools and drain their remaining samples into the
    /// canonical runtime chunks. No full-length WAV is rendered on this path.
    pub fn stop_and_flush(self, flush: impl FnOnce() -> Result<()>) -> Result<f64> {
        self.stop_capture_and_seal()?.finish_without_wav(flush)
    }

    fn stop_capture_and_seal(self) -> Result<segment_writer::SealedSegment> {
        self.external_stop_flag.store(true, Ordering::SeqCst);
        self.mic.retire().context("mic retirement failed")?;
        self.system.retire().context("system retirement failed")?;
        let mic_dropped = self.mic_telemetry.drops.load(Ordering::Relaxed);
        let spk_dropped = self.system_telemetry.drops.load(Ordering::Relaxed);
        if mic_dropped > 0 || spk_dropped > 0 {
            // A genuine capture-quality problem: keep this on stderr.
            eprintln!("Warning: dropped samples — mic: {mic_dropped}, speaker: {spk_dropped}");
            crate::cli_log::event(
                "capture_dropped_samples",
                format!("mic={mic_dropped} speaker={spk_dropped}"),
            );
        }
        let telemetry = self.writer.telemetry();
        let sealed = self.writer.seal()?;
        crate::cli_log::event(
            "capture_lane_summary",
            format!(
                "mic_callback={} mic_real_spool={} mic_nonzero_spool={} mic_padded={} system_callback={} system_real_spool={} system_nonzero_spool={} system_padded={}",
                self.mic_telemetry.frames.load(Ordering::Relaxed),
                telemetry.mic.real_spool_frames.load(Ordering::Relaxed),
                telemetry.mic.nonzero_spool_frames.load(Ordering::Relaxed),
                telemetry.mic.synthesized_durable_frames.load(Ordering::Relaxed),
                self.system_telemetry.frames.load(Ordering::Relaxed),
                telemetry.system.real_spool_frames.load(Ordering::Relaxed),
                telemetry.system.nonzero_spool_frames.load(Ordering::Relaxed),
                telemetry.system.synthesized_durable_frames.load(Ordering::Relaxed),
            ),
        );
        Ok(sealed)
    }
}

pub struct SelectedInputDevice {
    pub name: String,
    pub occurrence: usize,
    pub uid: Option<String>,
    pub device: InputDevice,
}

/// Enumerate all available input devices. Returns (name, device) pairs.
pub fn list_input_devices() -> Vec<(String, cpal::Device)> {
    let host = cpal::default_host();
    let devices = match host.input_devices() {
        Ok(devs) => devs,
        Err(_) => return Vec::new(),
    };
    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            Some((name, d))
        })
        .collect()
}

/// Resolve the user's picker choice against its displayed name snapshot.
/// Re-enumeration may reorder devices between opening the picker and selecting
/// one, so the old numeric index must never be applied to the new list.
pub fn selected_input_device(
    displayed_names: &[String],
    displayed_uids: &[Option<String>],
    selected_index: usize,
) -> Result<SelectedInputDevice> {
    let (name, occurrence) = selected_name_occurrence(displayed_names, selected_index)
        .context("selected audio input is no longer available")?;
    let requested_uid = displayed_uids.get(selected_index).cloned().flatten();
    let devices = list_input_devices();
    let available_names = devices
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let available_uids = input_device_uid_snapshot(&available_names);
    let available_index = selected_device_position(
        displayed_names,
        displayed_uids,
        selected_index,
        &available_names,
        &available_uids,
    )
    .with_context(|| format!("selected audio input is no longer available: {name}"))?;
    let (_, device) = devices
        .into_iter()
        .nth(available_index)
        .context("selected audio input disappeared during enumeration")?;
    let uid = requested_uid.or_else(|| available_uids[available_index].clone());
    Ok(SelectedInputDevice {
        name,
        occurrence,
        uid,
        device,
    })
}

pub fn input_device_uid_snapshot(names: &[String]) -> Vec<Option<String>> {
    #[cfg(not(target_os = "macos"))]
    return vec![None; names.len()];

    #[cfg(target_os = "macos")]
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let occurrence = names[..index]
                .iter()
                .filter(|candidate| *candidate == name)
                .count();
            input_device_uid_at(name, occurrence)
        })
        .collect()
}

fn selected_device_position(
    displayed_names: &[String],
    displayed_uids: &[Option<String>],
    selected_index: usize,
    available_names: &[String],
    available_uids: &[Option<String>],
) -> Option<usize> {
    let (name, occurrence) = selected_name_occurrence(displayed_names, selected_index)?;
    if let Some(uid) = displayed_uids.get(selected_index).and_then(Option::as_ref) {
        // A UID is authoritative. Refuse to open a different device if the
        // selected one disappeared or changed its display name.
        return available_uids
            .iter()
            .position(|candidate| candidate.as_ref() == Some(uid));
    }
    available_names
        .iter()
        .enumerate()
        .filter(|(_, candidate)| **candidate == name)
        .nth(occurrence)
        .map(|(index, _)| index)
}

fn selected_name_occurrence(names: &[String], index: usize) -> Option<(String, usize)> {
    let name = names.get(index)?.clone();
    let occurrence = names[..index]
        .iter()
        .filter(|candidate| **candidate == name)
        .count();
    Some((name, occurrence))
}

/// Get the name of the default input device, if any.
pub fn default_input_device_name() -> Option<String> {
    let host = cpal::default_host();
    host.default_input_device().and_then(|d| d.name().ok())
}

#[cfg(target_os = "macos")]
fn default_input_device_uid() -> Option<String> {
    ca::System::default_input_device()
        .ok()?
        .uid()
        .ok()
        .map(|uid| uid.to_string())
}

#[cfg(not(target_os = "macos"))]
fn default_input_device_uid() -> Option<String> {
    None
}

/// Resolve the stable Core Audio UID corresponding to a cpal device name.
/// cpal does not currently expose the UID, so keep this platform bridge next
/// to the rest of the Core Audio integration.
#[cfg(target_os = "macos")]
pub fn input_device_uid(device_name: &str) -> Option<String> {
    input_device_uid_at(device_name, 0)
}

#[cfg(target_os = "macos")]
pub fn input_device_uid_at(device_name: &str, occurrence: usize) -> Option<String> {
    ca::System::devices()
        .ok()?
        .into_iter()
        .filter(|device| {
            device
                .input_stream_cfg()
                .is_ok_and(|config| config.number_buffers() > 0)
        })
        .filter(|device| {
            device
                .name()
                .is_ok_and(|name| name.to_string() == device_name)
        })
        .nth(occurrence)?
        .uid()
        .ok()
        .map(|uid| uid.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn input_device_uid(device_name: &str) -> Option<String> {
    Some(device_name.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn input_device_uid_at(device_name: &str, _occurrence: usize) -> Option<String> {
    input_device_uid(device_name)
}

/// Briefly open an input device and report the peak level observed by the same
/// mic path used for recordings. This is intended for setup/preflight UI, not
/// for recording audio: callers should ask the user to speak during the probe.
pub fn test_input_device_level(
    device: Option<&cpal::Device>,
    duration: Duration,
) -> Result<(f32, u64)> {
    let target = MicTarget::from_optional(device);
    test_input_target_level(&target, duration, MicBackendKind::Cpal)
}

/// Exercise the same backend selection used by production capture without
/// creating a session. This keeps Audio Setup useful for validating the native
/// fallback under the installed app's microphone permission.
pub fn test_input_target_level(
    target: &MicTarget<'_>,
    duration: Duration,
    preferred: MicBackendKind,
) -> Result<(f32, u64)> {
    let peak = Arc::new(AtomicU32::new(0));
    let drops = Arc::new(AtomicU64::new(0));
    let telemetry = MicCaptureTelemetry {
        peak: peak.clone(),
        drops: drops.clone(),
        packet_drops: Arc::new(AtomicU64::new(0)),
        frames: Arc::new(AtomicU64::new(0)),
        silence: Arc::new(AtomicU64::new(0)),
        error: Arc::new(AtomicU8::new(0)),
    };
    let raw = start_mic_raw_for_backend(target, &telemetry, preferred).or_else(|primary| {
        start_mic_raw_for_backend(target, &telemetry, preferred.alternate()).map_err(|fallback| {
            anyhow::anyhow!(
                "{} microphone test failed: {primary:#}; {} fallback failed: {fallback:#}",
                preferred.name(),
                preferred.alternate().name(),
            )
        })
    })?;
    let backend = raw.stream.backend();
    std::thread::sleep(duration);
    raw.stream.stop()?;
    eprintln!("microphone test completed with {} backend", backend.name());
    Ok((
        f32::from_bits(peak.load(Ordering::Relaxed)),
        drops
            .load(Ordering::Relaxed)
            .saturating_add(telemetry.packet_drops.load(Ordering::Relaxed)),
    ))
}

/// Briefly open the platform system-audio capture and report whether it is
/// receiving samples. On macOS this probes the process tap; on Windows this
/// probes WASAPI loopback on the default render endpoint.
#[derive(Debug, Clone, Copy)]
pub struct SystemAudioTapProbe {
    pub peak: f32,
    pub drop_count: u64,
    pub silent_secs: f64,
    pub frame_count: u64,
    pub test_tone_played: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn test_system_audio_tap_level(duration: Duration) -> Result<SystemAudioTapProbe> {
    let peak = Arc::new(AtomicU32::new(0));
    let drops = Arc::new(AtomicU64::new(0));
    let silence = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let (capture, rate, _consumer) =
        start_speaker(peak.clone(), drops.clone(), silence.clone(), frames.clone())?;
    // On macOS the tone deliberately comes from afplay, a separate process.
    // A same-process tone can be captured even when TCC prevents a global tap
    // from receiving other applications, producing a false Ready result.
    let tone = start_system_audio_test_tone(duration);
    if let Err(error) = &tone {
        eprintln!("Computer-audio test tone did not start: {error:#}");
    }
    std::thread::sleep(duration);

    #[cfg(target_os = "macos")]
    let test_tone_played = tone
        .as_ref()
        .is_ok_and(SystemAudioTestTone::playback_succeeded);
    #[cfg(target_os = "windows")]
    let test_tone_played = tone.is_ok();

    drop(tone);
    drop(capture);
    let silent_secs = if rate == 0 {
        0.0
    } else {
        silence.load(Ordering::Relaxed) as f64 / rate as f64
    };
    Ok(SystemAudioTapProbe {
        peak: f32::from_bits(peak.load(Ordering::Relaxed)),
        drop_count: drops.load(Ordering::Relaxed),
        silent_secs,
        frame_count: frames.load(Ordering::Relaxed),
        test_tone_played,
    })
}

#[cfg(target_os = "macos")]
struct SystemAudioTestTone {
    launchd_label: String,
    path: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
impl SystemAudioTestTone {
    fn playback_succeeded(&self) -> bool {
        let uid = match std::process::Command::new("/usr/bin/id").arg("-u").output() {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            _ => return false,
        };
        let service = format!("gui/{uid}/{}", self.launchd_label);
        let output = match std::process::Command::new("/bin/launchctl")
            .args(["print", &service])
            .output()
        {
            Ok(output) if output.status.success() => output,
            _ => return false,
        };
        let status = String::from_utf8_lossy(&output.stdout);
        launchd_job_succeeded(&status)
    }
}

#[cfg(target_os = "macos")]
fn launchd_job_succeeded(status: &str) -> bool {
    if let Some(exit_code) = status.lines().find_map(|line| {
        line.trim()
            .strip_prefix("last exit code = ")
            .and_then(|code| code.parse::<i32>().ok())
    }) {
        return exit_code == 0;
    }
    status.contains("state = running") || status.contains("active count = 1")
}

#[cfg(target_os = "macos")]
impl Drop for SystemAudioTestTone {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/bin/launchctl")
            .args(["remove", &self.launchd_label])
            .status();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(target_os = "macos")]
fn start_system_audio_test_tone(duration: Duration) -> Result<SystemAudioTestTone> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "margins-system-audio-test-{}-{}.wav",
        std::process::id(),
        nonce
    ));
    let launchd_label = format!(
        "com.margins.system-audio-test.{}.{}",
        std::process::id(),
        nonce
    );
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec)
        .context("Could not prepare the computer-audio test tone")?;
    let sample_count = (duration.as_secs_f64() * spec.sample_rate as f64).ceil() as usize;
    let step = 880.0 * 2.0 * std::f32::consts::PI / spec.sample_rate as f32;
    let amplitude = i16::MAX as f32 * 0.08;
    for index in 0..sample_count {
        writer.write_sample(((index as f32 * step).sin() * amplitude) as i16)?;
    }
    writer.finalize()?;

    let status = std::process::Command::new("/bin/launchctl")
        .args([
            "submit",
            "-l",
            &launchd_label,
            "-o",
            "/dev/null",
            "-e",
            "/dev/null",
            "--",
            "/usr/bin/afplay",
        ])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Could not play the computer-audio test tone")?;
    if !status.success() {
        let _ = std::fs::remove_file(&path);
        bail!("Could not launch the computer-audio test tone");
    }
    Ok(SystemAudioTestTone {
        launchd_label,
        path,
    })
}

#[cfg(target_os = "windows")]
fn start_system_audio_test_tone(_duration: Duration) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("No default output device is available")?;
    let config = device
        .default_output_config()
        .context("Could not read default output config")?;
    match config.sample_format() {
        cpal::SampleFormat::F32 => build_test_tone_stream::<f32>(&device, &config.into()),
        cpal::SampleFormat::F64 => build_test_tone_stream::<f64>(&device, &config.into()),
        cpal::SampleFormat::I8 => build_test_tone_stream::<i8>(&device, &config.into()),
        cpal::SampleFormat::I16 => build_test_tone_stream::<i16>(&device, &config.into()),
        cpal::SampleFormat::I32 => build_test_tone_stream::<i32>(&device, &config.into()),
        cpal::SampleFormat::I64 => build_test_tone_stream::<i64>(&device, &config.into()),
        cpal::SampleFormat::U8 => build_test_tone_stream::<u8>(&device, &config.into()),
        cpal::SampleFormat::U16 => build_test_tone_stream::<u16>(&device, &config.into()),
        cpal::SampleFormat::U32 => build_test_tone_stream::<u32>(&device, &config.into()),
        cpal::SampleFormat::U64 => build_test_tone_stream::<u64>(&device, &config.into()),
        format => bail!("unsupported output format: {:?}", format),
    }
}

#[cfg(target_os = "windows")]
fn build_test_tone_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate.0 as f32;
    let mut phase = 0.0f32;
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            write_test_tone(data, channels, sample_rate, &mut phase);
        },
        |err| eprintln!("system-audio test tone error: {err}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

#[cfg(target_os = "windows")]
fn write_test_tone<T>(output: &mut [T], channels: usize, sample_rate: f32, phase: &mut f32)
where
    T: Sample + FromSample<f32>,
{
    let step = 880.0 * 2.0 * std::f32::consts::PI / sample_rate;
    for frame in output.chunks_mut(channels) {
        let sample = (*phase).sin() * 0.08;
        *phase = (*phase + step) % (2.0 * std::f32::consts::PI);
        let value = T::from_sample(sample);
        for channel in frame {
            *channel = value;
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn test_system_audio_tap_level(_duration: Duration) -> Result<SystemAudioTapProbe> {
    bail!("System audio capture is only supported on macOS and Windows")
}

// --- Shared consumer drain ---

#[cfg(test)]
#[derive(Debug)]
struct SpoolCapture {
    path: std::path::PathBuf,
    samples_written: u64,
}

#[cfg(test)]
impl Drop for SpoolCapture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
fn new_spool_capture(channel: LiveAudioChannel) -> Result<(SpoolCapture, BufWriter<File>)> {
    let mut path = std::env::temp_dir();
    let channel = match channel {
        LiveAudioChannel::Mic => "mic",
        LiveAudioChannel::System => "system",
    };
    let unique = format!(
        "margins-recording-{channel}-{}-{:?}-{}.f32",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SPOOL_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    );
    path.push(unique);
    let file =
        File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok((
        SpoolCapture {
            path,
            samples_written: 0,
        },
        BufWriter::new(file),
    ))
}

#[cfg(test)]
fn drain_ring_buffer_to_writer(
    consumer: &mut rtrb::Consumer<f32>,
    stop: &AtomicBool,
    sample_rate: u32,
    channel: LiveAudioChannel,
    live_audio: Option<&LiveAudioSink>,
    writer: &mut impl Write,
    samples_written: &mut u64,
) -> Result<()> {
    // Prevent macOS from deprioritizing this thread — timer coalescing
    // can stretch a 5ms sleep to 50ms+ under power saving, which risks
    // overflowing the ring buffer.
    #[cfg(target_os = "macos")]
    {
        use std::os::raw::c_int;
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: c_int, relative_priority: c_int) -> c_int;
        }
        const QOS_CLASS_USER_INTERACTIVE: c_int = 0x21;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
        }
    }

    while !stop.load(Ordering::Relaxed) {
        drain_available_samples(
            consumer,
            writer,
            samples_written,
            sample_rate,
            channel,
            live_audio,
        )?;
        std::thread::sleep(DRAIN_INTERVAL);
    }
    // Final drain after stop — the audio callback may have pushed more samples
    // between our last pop and the stream being dropped.
    drain_available_samples(
        consumer,
        writer,
        samples_written,
        sample_rate,
        channel,
        live_audio,
    )?;
    Ok(())
}

#[cfg(test)]
fn drain_available_samples(
    consumer: &mut rtrb::Consumer<f32>,
    writer: &mut impl Write,
    samples_written: &mut u64,
    sample_rate: u32,
    channel: LiveAudioChannel,
    live_audio: Option<&LiveAudioSink>,
) -> Result<()> {
    let mut chunk = Vec::new();
    while let Ok(sample) = consumer.pop() {
        writer.write_all(&sample.to_le_bytes())?;
        chunk.push(sample);
    }
    *samples_written += chunk.len() as u64;
    if let Some(tx) = live_audio {
        if !chunk.is_empty() {
            let generation_clock = *tx
                .generation_clock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if generation_clock.generation != tx.generation {
                // Generation fencing deliberately discards standby/stale
                // recorder output during a cutover. It is not queue loss: the
                // active generation owns that part of the session timeline.
                return Ok(());
            }
            let sample_count = chunk.len() as u64;
            let current_queued = tx.queued_samples.load(Ordering::Relaxed);
            if tx.queue_max_samples > 0
                && current_queued.saturating_add(sample_count) > tx.queue_max_samples
            {
                match channel {
                    LiveAudioChannel::Mic => tx
                        .mic_dropped_samples
                        .fetch_add(sample_count, Ordering::Relaxed),
                    LiveAudioChannel::System => tx
                        .system_dropped_samples
                        .fetch_add(sample_count, Ordering::Relaxed),
                };
            } else {
                let chunk = LiveAudioChunk {
                    channel,
                    generation: tx.generation,
                    session_offset_ms: generation_clock.session_offset_ms,
                    sample_rate,
                    start_frame: *samples_written - sample_count,
                    synthesized: false,
                    samples: chunk,
                };
                match tx.sender.send(chunk) {
                    Ok(()) => {
                        tx.queued_samples.fetch_add(sample_count, Ordering::Relaxed);
                        match channel {
                            LiveAudioChannel::Mic => tx
                                .mic_accepted_samples
                                .fetch_add(sample_count, Ordering::Relaxed),
                            LiveAudioChannel::System => tx
                                .system_accepted_samples
                                .fetch_add(sample_count, Ordering::Relaxed),
                        }
                    }
                    Err(_) => match channel {
                        LiveAudioChannel::Mic => tx
                            .mic_dropped_samples
                            .fetch_add(sample_count, Ordering::Relaxed),
                        LiveAudioChannel::System => tx
                            .system_dropped_samples
                            .fetch_add(sample_count, Ordering::Relaxed),
                    },
                };
            }
        }
    }
    Ok(())
}

// --- Microphone capture backends ---

fn start_mic_raw_for_backend(
    target: &MicTarget<'_>,
    telemetry: &MicCaptureTelemetry,
    backend: MicBackendKind,
) -> Result<RawMicCapture> {
    match backend {
        MicBackendKind::Cpal => start_mic_raw(target.device, telemetry),
        MicBackendKind::CoreAudio => {
            #[cfg(target_os = "macos")]
            {
                let raw = coreaudio_mic::start_input(target.stable_uid(), telemetry.clone())?;
                Ok(RawMicCapture {
                    stream: MicStream::CoreAudio(raw.stream),
                    native_rate: raw.native_rate,
                    samples: raw.samples,
                    packets: raw.packets,
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = target;
                let _ = telemetry;
                bail!("direct CoreAudio capture is only available on macOS")
            }
        }
    }
}

fn start_mic_raw(
    device: Option<&cpal::Device>,
    telemetry: &MicCaptureTelemetry,
) -> Result<RawMicCapture> {
    let host = cpal::default_host();
    let default_device;
    let device = match device {
        Some(d) => d,
        None => {
            default_device = host.default_input_device().context("no mic found")?;
            &default_device
        }
    };
    let config = device.default_input_config()?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let format = config.sample_format();

    let capacity = rate as usize * RING_BUF_SECONDS;
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let (packet_producer, packet_consumer) = rtrb::RingBuffer::new(32_768);

    let stream = match format {
        cpal::SampleFormat::F32 => build_mic_stream::<f32>(
            device,
            &config,
            producer,
            packet_producer,
            channels,
            telemetry.clone(),
        )?,
        cpal::SampleFormat::I16 => build_mic_stream::<i16>(
            device,
            &config,
            producer,
            packet_producer,
            channels,
            telemetry.clone(),
        )?,
        cpal::SampleFormat::I32 => build_mic_stream::<i32>(
            device,
            &config,
            producer,
            packet_producer,
            channels,
            telemetry.clone(),
        )?,
        _ => bail!("unsupported mic format: {:?}", format),
    };
    stream.play()?;

    Ok(RawMicCapture {
        stream: MicStream::Cpal(stream),
        native_rate: rate,
        samples: consumer,
        packets: packet_consumer,
    })
}

fn build_mic_stream<S: SizedSample + ToSample<f32> + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut producer: rtrb::Producer<f32>,
    mut packet_producer: rtrb::Producer<PacketDesc>,
    channels: usize,
    telemetry: MicCaptureTelemetry,
) -> Result<cpal::Stream> {
    let error_flag = telemetry.error.clone();
    let mut first_capture = None;
    Ok(device.build_input_stream(
        &config.config(),
        move |data: &[S], info: &cpal::InputCallbackInfo| {
            let mut local_peak = 0.0f32;
            let mut local_drops = 0u64;
            let mut silent_run = telemetry.silence.load(Ordering::Relaxed);
            for sample in data.iter().step_by(channels) {
                let s = (sample.to_sample::<f32>() * MIC_GAIN).clamp(-1.0, 1.0);
                silent_run = next_exact_zero_run(silent_run, s);
                local_peak = local_peak.max(s.abs());
                if producer.push(s).is_err() {
                    local_drops += 1;
                }
            }
            telemetry
                .peak
                .fetch_max(local_peak.to_bits(), Ordering::Relaxed);
            telemetry.silence.store(silent_run, Ordering::Relaxed);
            if local_drops > 0 {
                telemetry.drops.fetch_add(local_drops, Ordering::Relaxed);
            }
            let capture = info.timestamp().capture;
            let base = *first_capture.get_or_insert(capture);
            let capture_nanos = capture
                .duration_since(&base)
                .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
                .unwrap_or(0);
            let frame_count = u32::try_from(data.len() / channels).unwrap_or(u32::MAX);
            telemetry
                .frames
                .fetch_add(u64::from(frame_count), Ordering::Relaxed);
            if packet_producer
                .push(PacketDesc {
                    frame_count,
                    capture_nanos,
                })
                .is_err()
            {
                telemetry
                    .packet_drops
                    .fetch_add(u64::from(frame_count), Ordering::Relaxed);
            }
        },
        move |err| {
            let kind = MicStreamErrorKind::from_stream_error(&err);
            error_flag.store(kind as u8, Ordering::SeqCst);
            eprintln!("mic error: {err}");
        },
        None,
    )?)
}

#[cfg(target_os = "macos")]
pub struct InputDeviceWatcher {
    ctx: Box<InputDeviceListenerCtx>,
    addrs: Vec<ca::PropAddr>,
    registered: usize,
}

#[cfg(target_os = "macos")]
struct InputDeviceListenerCtx {
    sender: SyncSender<()>,
}

#[cfg(target_os = "macos")]
extern "C-unwind" fn input_device_listener(
    _obj_id: ca::Obj,
    number_addresses: u32,
    addresses: *const ca::PropAddr,
    client_data: *mut InputDeviceListenerCtx,
) -> os::Status {
    let addresses = unsafe { std::slice::from_raw_parts(addresses, number_addresses as usize) };
    let changed = addresses.iter().any(|addr| {
        addr.selector == ca::PropSelector::HW_DEFAULT_INPUT_DEVICE
            || addr.selector == ca::PropSelector::HW_DEVICES
    });
    if changed && !client_data.is_null() {
        let ctx = unsafe { &*client_data };
        let _ = ctx.sender.try_send(());
    }
    os::Status::NO_ERR
}

#[cfg(target_os = "macos")]
impl InputDeviceWatcher {
    pub fn start(sender: SyncSender<()>) -> Result<Self> {
        let mut watcher = Self {
            ctx: Box::new(InputDeviceListenerCtx { sender }),
            addrs: vec![
                ca::PropSelector::HW_DEVICES.global_addr(),
                ca::PropSelector::HW_DEFAULT_INPUT_DEVICE.global_addr(),
            ],
            registered: 0,
        };
        for addr in &watcher.addrs {
            if let Err(error) = ca::System::OBJ.add_prop_listener(
                addr,
                input_device_listener,
                watcher.ctx.as_mut() as *mut InputDeviceListenerCtx,
            ) {
                return Err(anyhow::anyhow!(
                    "AudioObjectAddPropertyListener failed: {error}"
                ));
            }
            watcher.registered += 1;
        }
        Ok(watcher)
    }
}

#[cfg(target_os = "macos")]
impl Drop for InputDeviceWatcher {
    fn drop(&mut self) {
        for addr in self.addrs.iter().take(self.registered) {
            let _ = ca::System::OBJ.remove_prop_listener(
                addr,
                input_device_listener,
                self.ctx.as_mut() as *mut InputDeviceListenerCtx,
            );
        }
        self.registered = 0;
    }
}

// --- System audio capture (platform loopback) ---

#[cfg(target_os = "macos")]
struct SpeakerCapture {
    _device: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _ctx: Box<SpeakerCtx>,
    _tap: ca::TapGuard,
}

#[cfg(target_os = "macos")]
struct SpeakerCtx {
    producer: rtrb::Producer<f32>,
    packet_producer: rtrb::Producer<PacketDesc>,
    format: arc::R<av::AudioFormat>,
    peak: Arc<AtomicU32>,
    drops: Arc<AtomicU64>,
    packet_drops: Arc<AtomicU64>,
    silence: Arc<AtomicU64>,
    frames: Arc<AtomicU64>,
    timebase_numer: u32,
    timebase_denom: u32,
    first_host_time: u64,
}

#[cfg(target_os = "macos")]
fn start_speaker_platform(telemetry: &SystemCaptureTelemetry) -> Result<RawSystemCapture> {
    use ca::aggregate_device_keys as agg_keys;
    use ca::sub_device_keys as sub_keys;

    let mut tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new());
    // Be explicit about the intended non-invasive behavior. The default is
    // expected to be unmuted, but setting it removes ambiguity if macOS or
    // cidre defaults change.
    tap_desc.set_private(true);
    tap_desc.set_mute_behavior(ca::TapMuteBehavior::Unmuted);
    let tap = tap_desc
        .create_process_tap()
        .context(margins_cli::error::MACOS_SYSTEM_AUDIO_TAP_FAILURE_CONTEXT)?;
    let asbd = tap.asbd()?;
    let tap_rate = asbd.sample_rate;
    let format = av::AudioFormat::with_asbd(&asbd).context("bad audio format from tap")?;

    let output_device = ca::System::default_output_device().context("no default output device")?;
    // The tap ASBD can remain at 48 kHz when opening a Bluetooth microphone
    // switches the default output into its 24 kHz headset profile. The aggregate
    // device below is clocked by that output device, so its callback delivers
    // output-clock frames even though the tap advertises its stale ASBD rate.
    // Labeling those frames with the tap rate makes computer audio run at 2x
    // speed in both the rolling ASR path and the final stereo WAV.
    let output_nominal_rate = output_device.nominal_sample_rate().ok();
    let output_actual_rate = output_device.actual_sample_rate().ok();
    let rate = effective_speaker_sample_rate(tap_rate, output_nominal_rate, output_actual_rate);
    if valid_sample_rate(tap_rate) != Some(rate) {
        // The aggregate device is clocked by the output, so we trust the output
        // rate over the tap's advertised ASBD. This is normal (e.g. a 48 kHz tap
        // over 44.1 kHz output) and self-correcting, so it goes to diagnostics.
        crate::cli_log::event(
            "system_tap_rate_realigned",
            format!(
                "tap={tap_rate:.0} output_nominal={} output_actual={} using={rate}",
                output_nominal_rate
                    .map(|value| format!("{value:.0}"))
                    .unwrap_or_else(|| "unknown".to_string()),
                output_actual_rate
                    .map(|value| format!("{value:.0}"))
                    .unwrap_or_else(|| "unknown".to_string()),
            ),
        );
    }

    let capacity = rate as usize * RING_BUF_SECONDS;
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let (packet_producer, packet_consumer) = rtrb::RingBuffer::new(32_768);

    let output_uid = output_device.uid().context("no output device uid")?;
    let sub_device =
        cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[output_uid.as_type_ref()]);
    let sub_tap = cf::DictionaryOf::with_keys_values(
        &[sub_keys::uid()],
        &[tap.uid().context("no tap uid")?.as_type_ref()],
    );
    let agg_desc = cf::DictionaryOf::with_keys_values(
        &[
            agg_keys::is_private(),
            agg_keys::is_stacked(),
            agg_keys::tap_auto_start(),
            agg_keys::name(),
            agg_keys::main_sub_device(),
            agg_keys::uid(),
            agg_keys::sub_device_list(),
            agg_keys::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::String::from_str(TAP_NAME).as_ref(),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );

    let timebase = mach::TimeBaseInfo::new();
    let mut ctx = Box::new(SpeakerCtx {
        producer,
        packet_producer,
        format,
        peak: telemetry.peak.clone(),
        drops: telemetry.drops.clone(),
        packet_drops: telemetry.packet_drops.clone(),
        silence: telemetry.silence.clone(),
        frames: telemetry.frames.clone(),
        timebase_numer: timebase.numer,
        timebase_denom: timebase.denom,
        first_host_time: 0,
    });

    let agg_device = ca::AggregateDevice::with_desc(&agg_desc)
        .map_err(|e| anyhow::anyhow!("AggregateDevice::with_desc failed: {}", e))?;
    let proc_id = agg_device
        .create_io_proc_id(speaker_io_proc, Some(&mut ctx))
        .map_err(|e| anyhow::anyhow!("create_io_proc_id failed: {}", e))?;
    let device = ca::device_start(agg_device, Some(proc_id))
        .map_err(|e| anyhow::anyhow!("device_start failed: {}", e))
        .with_context(|| {
            margins_cli::error::macos_system_audio_permission_likely_message(
                "could not start macOS system-audio IO",
            )
        })?;

    Ok(RawSystemCapture {
        capture: SpeakerCapture {
            _device: device,
            _ctx: ctx,
            _tap: tap,
        },
        native_rate: rate,
        samples: consumer,
        packets: packet_consumer,
    })
}

#[cfg(any(target_os = "macos", test))]
fn valid_sample_rate(rate: f64) -> Option<u32> {
    if rate.is_finite() && (8_000.0..=384_000.0).contains(&rate) {
        Some(rate.round() as u32)
    } else {
        None
    }
}

#[cfg(any(target_os = "macos", test))]
fn effective_speaker_sample_rate(
    tap_rate: f64,
    output_nominal_rate: Option<f64>,
    output_actual_rate: Option<f64>,
) -> u32 {
    output_actual_rate
        .and_then(valid_sample_rate)
        .or_else(|| output_nominal_rate.and_then(valid_sample_rate))
        .or_else(|| valid_sample_rate(tap_rate))
        .unwrap_or(48_000)
}

#[cfg(target_os = "macos")]
extern "C" fn speaker_io_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input_data: &cat::AudioBufList<1>,
    input_time: &cat::AudioTimeStamp,
    _output_data: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut SpeakerCtx>,
) -> os::Status {
    let ctx = ctx.unwrap();
    let capture_nanos = if input_time.host_time == 0 || ctx.timebase_denom == 0 {
        0
    } else {
        if ctx.first_host_time == 0 {
            ctx.first_host_time = input_time.host_time;
        }
        let ticks = input_time.host_time.saturating_sub(ctx.first_host_time);
        u64::try_from(
            u128::from(ticks) * u128::from(ctx.timebase_numer) / u128::from(ctx.timebase_denom),
        )
        .unwrap_or(u64::MAX)
    };

    // Try typed PCM buffer first
    if let Some(view) = av::AudioPcmBuf::with_buf_list_no_copy(&ctx.format, input_data, None) {
        if let Some(data) = view.data_f32_at(0) {
            push_speaker_samples(ctx, data, capture_nanos);
            return os::Status::NO_ERR;
        }
    }

    // Fallback: read raw f32 samples from buffer
    let buf = &input_data.buffers[0];
    if buf.data_bytes_size > 0 && !buf.data.is_null() {
        let count = buf.data_bytes_size as usize / std::mem::size_of::<f32>();
        if count > 0 {
            let data = unsafe { std::slice::from_raw_parts(buf.data as *const f32, count) };
            push_speaker_samples(ctx, data, capture_nanos);
        }
    }

    os::Status::NO_ERR
}

/// Push speaker samples into the ring buffer, tracking drops and silence.
#[cfg(target_os = "macos")]
fn push_speaker_samples(ctx: &mut SpeakerCtx, data: &[f32], capture_nanos: u64) {
    ctx.frames.fetch_add(data.len() as u64, Ordering::Relaxed);
    push_samples_tracked(
        &mut ctx.producer,
        &ctx.peak,
        &ctx.drops,
        Some(&ctx.silence),
        data,
    );
    let frame_count = u32::try_from(data.len()).unwrap_or(u32::MAX);
    if ctx
        .packet_producer
        .push(PacketDesc {
            frame_count,
            capture_nanos,
        })
        .is_err()
    {
        ctx.packet_drops
            .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    }
}

#[cfg(target_os = "windows")]
struct SpeakerCapture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
impl Drop for SpeakerCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "windows")]
fn start_speaker_platform(telemetry: &SystemCaptureTelemetry) -> Result<RawSystemCapture> {
    let capacity = 48_000 * RING_BUF_SECONDS;
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let (packet_producer, packet_consumer) = rtrb::RingBuffer::new(32_768);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);

    let thread = std::thread::spawn(move || {
        let result = run_wasapi_loopback_capture(
            producer,
            packet_producer,
            telemetry.peak.clone(),
            telemetry.drops.clone(),
            telemetry.packet_drops.clone(),
            telemetry.silence.clone(),
            telemetry.frames.clone(),
            thread_stop,
            ready_tx,
        );
        if let Err(e) = result {
            eprintln!("Windows system audio error: {e}");
        }
    });

    let rate = match ready_rx
        .recv()
        .context("Windows system-audio capture thread did not start")?
    {
        Ok(rate) => rate,
        Err(e) => {
            stop.store(true, Ordering::SeqCst);
            let _ = thread.join();
            return Err(e);
        }
    };

    Ok(RawSystemCapture {
        capture: SpeakerCapture {
            stop,
            thread: Some(thread),
        },
        native_rate: rate,
        samples: consumer,
        packets: packet_consumer,
    })
}

#[cfg(target_os = "windows")]
fn run_wasapi_loopback_capture(
    mut producer: rtrb::Producer<f32>,
    mut packet_producer: rtrb::Producer<PacketDesc>,
    peak: Arc<AtomicU32>,
    drops: Arc<AtomicU64>,
    packet_drops: Arc<AtomicU64>,
    silence: Arc<AtomicU64>,
    frames_seen: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::SyncSender<Result<u32>>,
) -> Result<()> {
    let capture_epoch = std::time::Instant::now();
    let _com = ComApartment::init()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .context("failed to create Windows audio device enumerator")?;
    let endpoint = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .context("no default Windows output device")?;
    let audio_client: IAudioClient = unsafe { endpoint.Activate(CLSCTX_ALL, None) }
        .context("failed to activate Windows audio client")?;
    let mix_format = WasapiMixFormat::from_audio_client(&audio_client)?;

    unsafe {
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            mix_format.wave_format_ptr,
            None,
        )
    }
    .context("failed to initialize Windows WASAPI loopback")?;
    let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
        .context("failed to get Windows loopback capture client")?;
    unsafe { audio_client.Start() }.context("failed to start Windows loopback capture")?;

    let _started = WasapiStartedClient {
        client: audio_client,
    };
    let _ = ready.send(Ok(mix_format.sample_rate));

    while !stop.load(Ordering::Relaxed) {
        let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }
            .context("failed to query Windows loopback packet size")?;
        if packet_frames == 0 {
            std::thread::sleep(DRAIN_INTERVAL);
            continue;
        }

        while packet_frames > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe { capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                .context("failed to read Windows loopback packet")?;

            let samples = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                vec![0.0; frames as usize]
            } else {
                unsafe { mix_format.samples_to_mono(data as *const u8, frames as usize) }
            };
            frames_seen.fetch_add(samples.len() as u64, Ordering::Relaxed);
            push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &samples);
            let frame_count = u32::try_from(samples.len()).unwrap_or(u32::MAX);
            let capture_nanos =
                u64::try_from(capture_epoch.elapsed().as_nanos()).unwrap_or(u64::MAX);
            if packet_producer
                .push(PacketDesc {
                    frame_count,
                    capture_nanos,
                })
                .is_err()
            {
                packet_drops.fetch_add(u64::from(frame_count), Ordering::Relaxed);
            }

            unsafe { capture_client.ReleaseBuffer(frames) }
                .context("failed to release Windows loopback packet")?;
            packet_frames = unsafe { capture_client.GetNextPacketSize() }
                .context("failed to query Windows loopback packet size")?;
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
struct ComApartment;

#[cfg(target_os = "windows")]
impl ComApartment {
    fn init() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .context("failed to initialize COM for Windows audio capture")?;
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[cfg(target_os = "windows")]
struct WasapiStartedClient {
    client: IAudioClient,
}

#[cfg(target_os = "windows")]
impl Drop for WasapiStartedClient {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

#[cfg(target_os = "windows")]
struct WasapiMixFormat {
    wave_format_ptr: *mut WAVEFORMATEX,
    sample_rate: u32,
    channels: usize,
    bytes_per_frame: usize,
    sample_format: WasapiSampleFormat,
}

#[cfg(target_os = "windows")]
impl WasapiMixFormat {
    fn from_audio_client(audio_client: &IAudioClient) -> Result<Self> {
        let wave_format_ptr = unsafe { audio_client.GetMixFormat() }
            .context("failed to read Windows loopback mix format")?;
        let format = unsafe { *wave_format_ptr };
        let sample_format = unsafe { wasapi_sample_format(wave_format_ptr)? };
        Ok(Self {
            wave_format_ptr,
            sample_rate: format.nSamplesPerSec,
            channels: format.nChannels as usize,
            bytes_per_frame: format.nBlockAlign as usize,
            sample_format,
        })
    }

    unsafe fn samples_to_mono(&self, data: *const u8, frames: usize) -> Vec<f32> {
        wasapi_packet_to_mono(
            data,
            frames,
            self.channels,
            self.bytes_per_frame,
            self.sample_format,
        )
    }
}

#[cfg(target_os = "windows")]
impl Drop for WasapiMixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.wave_format_ptr as *const std::ffi::c_void)) };
    }
}

#[cfg(target_os = "windows")]
unsafe fn wasapi_sample_format(format: *const WAVEFORMATEX) -> Result<WasapiSampleFormat> {
    let tag = (*format).wFormatTag as u32;
    let bits = (*format).wBitsPerSample;
    let subformat = if tag == WAVE_FORMAT_EXTENSIBLE {
        let extensible = format as *const WAVEFORMATEXTENSIBLE;
        Some((*extensible).SubFormat)
    } else {
        None
    };

    if tag == WAVE_FORMAT_IEEE_FLOAT || subformat == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) {
        if bits == 32 {
            return Ok(WasapiSampleFormat::Float32);
        }
    }
    if tag == WAVE_FORMAT_PCM || subformat == Some(KSDATAFORMAT_SUBTYPE_PCM) {
        return match bits {
            16 => Ok(WasapiSampleFormat::Pcm16),
            24 => Ok(WasapiSampleFormat::Pcm24),
            32 => Ok(WasapiSampleFormat::Pcm32),
            _ => bail!("unsupported Windows loopback PCM bit depth: {bits}"),
        };
    }
    bail!("unsupported Windows loopback mix format: tag={tag} bits={bits}")
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WasapiSampleFormat {
    Float32,
    Pcm16,
    Pcm24,
    Pcm32,
}

#[cfg(any(target_os = "windows", test))]
unsafe fn wasapi_packet_to_mono(
    data: *const u8,
    frames: usize,
    channels: usize,
    bytes_per_frame: usize,
    format: WasapiSampleFormat,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    if frames == 0 || channels == 0 || bytes_per_frame == 0 {
        return out;
    }

    for frame_idx in 0..frames {
        let frame = data.add(frame_idx * bytes_per_frame);
        let mut sum = 0.0f32;
        for channel_idx in 0..channels {
            sum += match format {
                WasapiSampleFormat::Float32 => {
                    let offset = channel_idx * std::mem::size_of::<f32>();
                    std::ptr::read_unaligned(frame.add(offset) as *const f32)
                }
                WasapiSampleFormat::Pcm16 => {
                    let offset = channel_idx * std::mem::size_of::<i16>();
                    let sample = std::ptr::read_unaligned(frame.add(offset) as *const i16);
                    sample as f32 / i16::MAX as f32
                }
                WasapiSampleFormat::Pcm24 => {
                    let offset = channel_idx * 3;
                    pcm24_to_f32(frame.add(offset))
                }
                WasapiSampleFormat::Pcm32 => {
                    let offset = channel_idx * std::mem::size_of::<i32>();
                    let sample = std::ptr::read_unaligned(frame.add(offset) as *const i32);
                    sample as f32 / i32::MAX as f32
                }
            };
        }
        out.push((sum / channels as f32).clamp(-1.0, 1.0));
    }

    out
}

#[cfg(any(target_os = "windows", test))]
unsafe fn pcm24_to_f32(data: *const u8) -> f32 {
    let b0 = *data as i32;
    let b1 = *data.add(1) as i32;
    let b2 = *data.add(2) as i32;
    let raw = b0 | (b1 << 8) | (b2 << 16);
    let signed = if raw & 0x80_0000 != 0 {
        raw | !0x00ff_ffff
    } else {
        raw
    };
    signed as f32 / 8_388_607.0
}

/// Core sample-push logic, platform-independent. Tracks peak level, dropped
/// samples (ring buffer full), and optionally consecutive dead-tap duration.
///
/// The silence counter detects a dead Core Audio tap by checking for exact-zero
/// samples (below `DEAD_TAP_THRESHOLD`). A live tap with nobody talking still
/// produces noise-floor samples that exceed this threshold.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn push_samples_tracked(
    producer: &mut rtrb::Producer<f32>,
    peak: &AtomicU32,
    drops: &AtomicU64,
    silence: Option<&AtomicU64>,
    data: &[f32],
) {
    let mut all_dead = true;
    let mut local_peak = 0.0f32;
    let mut local_drops = 0u64;
    for &s in data {
        let abs = s.abs();
        local_peak = local_peak.max(abs);
        if producer.push(s).is_err() {
            local_drops += 1;
        }
        if abs > DEAD_TAP_THRESHOLD {
            all_dead = false;
        }
    }
    peak.fetch_max(local_peak.to_bits(), Ordering::Relaxed);
    if local_drops > 0 {
        drops.fetch_add(local_drops, Ordering::Relaxed);
    }
    if let Some(silence) = silence {
        if all_dead {
            silence.fetch_add(data.len() as u64, Ordering::Relaxed);
        } else {
            silence.store(0, Ordering::Relaxed);
        }
    }
}

/// Write interleaved stereo WAV from two mono raw-f32 spool files.
/// Resamples `spk` to `mic_rate` if rates differ. Returns duration in seconds.
#[cfg(test)]
fn write_stereo_wav_from_spools(
    path: &str,
    mic_capture: &SpoolCapture,
    spk_capture: &SpoolCapture,
    mic_rate: u32,
    spk_rate: u32,
) -> Result<f64> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: mic_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(path, spec).context("failed to create WAV")?;
    let mut mic = RawF32Reader::open(&mic_capture.path)?;
    let mut spk = ResampledRawF32Reader::open(
        &spk_capture.path,
        spk_capture.samples_written,
        spk_rate,
        mic_rate,
    )?;

    let max_len = mic_capture.samples_written.max(spk.output_len());
    for i in 0..max_len {
        let m = if i < mic_capture.samples_written {
            mic.next_sample()?.unwrap_or(0.0)
        } else {
            0.0
        };
        let s = if i < spk.output_len() {
            spk.next_sample()?
        } else {
            0.0
        };
        writer.write_sample(m)?;
        writer.write_sample(s)?;
    }
    writer.finalize()?;

    Ok(max_len as f64 / mic_rate as f64)
}

#[cfg(test)]
fn write_spool(samples: &[f32]) -> Result<SpoolCapture> {
    let (mut capture, mut writer) = new_spool_capture(LiveAudioChannel::Mic)?;
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    capture.samples_written = samples.len() as u64;
    Ok(capture)
}

#[cfg(test)]
struct RawF32Reader {
    reader: BufReader<File>,
}

#[cfg(test)]
impl RawF32Reader {
    fn open(path: &std::path::Path) -> Result<Self> {
        Ok(Self {
            reader: BufReader::new(
                File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
            ),
        })
    }

    fn next_sample(&mut self) -> Result<Option<f32>> {
        let mut bytes = [0u8; 4];
        match self.reader.read_exact(&mut bytes) {
            Ok(()) => Ok(Some(f32::from_le_bytes(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
struct ResampledRawF32Reader {
    reader: RawF32Reader,
    from_rate: u32,
    to_rate: u32,
    source_len: u64,
    out_len: u64,
    out_index: u64,
    source_index: u64,
    current: Option<f32>,
    next: Option<f32>,
}

#[cfg(test)]
impl ResampledRawF32Reader {
    fn open(path: &std::path::Path, source_len: u64, from_rate: u32, to_rate: u32) -> Result<Self> {
        let mut reader = RawF32Reader::open(path)?;
        let current = reader.next_sample()?;
        let next = reader.next_sample()?;
        let out_len = if from_rate == to_rate {
            source_len
        } else {
            ((source_len as f64) / (from_rate as f64 / to_rate as f64)).ceil() as u64
        };
        Ok(Self {
            reader,
            from_rate,
            to_rate,
            source_len,
            out_len,
            out_index: 0,
            source_index: 0,
            current,
            next,
        })
    }

    fn output_len(&self) -> u64 {
        self.out_len
    }

    fn next_sample(&mut self) -> Result<f32> {
        if self.out_index >= self.out_len || self.source_len == 0 {
            return Ok(0.0);
        }
        if self.from_rate == self.to_rate {
            let sample = self.current.unwrap_or(0.0);
            self.out_index += 1;
            self.current = self.next.take();
            self.next = self.reader.next_sample()?;
            self.source_index += 1;
            return Ok(sample);
        }

        let ratio = self.from_rate as f64 / self.to_rate as f64;
        let src_pos = self.out_index as f64 * ratio;
        let idx = src_pos.floor() as u64;
        let frac = (src_pos - idx as f64) as f32;
        while self.source_index < idx {
            self.current = self.next.take();
            self.next = self.reader.next_sample()?;
            self.source_index += 1;
        }
        let current = self.current.unwrap_or(0.0);
        let next = self.next.unwrap_or(current);
        self.out_index += 1;
        Ok(current * (1.0 - frac) + next * frac)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct SpeakerCapture;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn start_speaker_platform(_telemetry: &SystemCaptureTelemetry) -> Result<RawSystemCapture> {
    bail!("System audio capture is only supported on macOS and Windows")
}

fn start_speaker_raw(telemetry: &SystemCaptureTelemetry) -> Result<RawSystemCapture> {
    start_speaker_platform(telemetry)
}

fn start_speaker(
    peak: Arc<AtomicU32>,
    drops: Arc<AtomicU64>,
    silence: Arc<AtomicU64>,
    frames: Arc<AtomicU64>,
) -> Result<(SpeakerCapture, u32, rtrb::Consumer<f32>)> {
    let telemetry = SystemCaptureTelemetry {
        peak,
        drops,
        packet_drops: Arc::new(AtomicU64::new(0)),
        silence,
        frames,
    };
    let raw = start_speaker_raw(&telemetry)?;
    Ok((raw.capture, raw.native_rate, raw.samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_zero_mic_run_resets_for_a_real_noise_floor() {
        let mut run = 0;
        for sample in vec![0.0; 3 * 48_000] {
            run = next_exact_zero_run(run, sample);
        }
        assert_eq!(run, 144_000);
        assert_eq!(next_exact_zero_run(run, -0.0), 144_001);
        assert_eq!(next_exact_zero_run(run, 0.000_000_1), 0);
    }
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn picker_choice_keeps_uid_when_enumeration_reorders() {
        let displayed = ["Built-in".into(), "USB".into(), "USB".into()];
        let reordered = ["USB".into(), "Built-in".into(), "USB".into()];
        let displayed_uids = [
            Some("builtin".into()),
            Some("usb-1".into()),
            Some("usb-2".into()),
        ];
        let reordered_uids = [
            Some("usb-2".into()),
            Some("builtin".into()),
            Some("usb-1".into()),
        ];
        let (name, occurrence) = selected_name_occurrence(&displayed, 2).unwrap();
        assert_eq!((name.as_str(), occurrence), ("USB", 1));
        let actual =
            selected_device_position(&displayed, &displayed_uids, 2, &reordered, &reordered_uids);
        assert_eq!(actual, Some(0));
        assert_eq!(
            selected_device_position(
                &displayed,
                &[None, None, None],
                2,
                &reordered,
                &[None, None, None],
            ),
            Some(2),
        );
        assert_eq!(
            selected_device_position(
                &displayed,
                &displayed_uids,
                2,
                &reordered,
                &[
                    Some("other".into()),
                    Some("builtin".into()),
                    Some("usb-1".into())
                ],
            ),
            None,
        );
        assert_ne!(displayed[0], reordered[0]);
    }

    #[test]
    fn microphone_progress_health_detects_and_resets_stalls() {
        let started = Instant::now();
        let mut health = MicProgressHealth::new(started);

        assert!(!health.stalled(started + Duration::from_secs(1), 0, true));
        assert!(health.stalled(started + MIC_STALL_TIMEOUT, 0, true));
        assert!(!health.stalled(started + Duration::from_secs(3), 480, true));
        assert!(!health.stalled(started + Duration::from_secs(4), 480, false));
        assert!(health.stalled(started + Duration::from_secs(5), 480, true));
    }

    #[test]
    fn mic_target_carries_optional_stable_uid() {
        let target = MicTarget::system_default().with_stable_uid("BuiltInMicrophoneDevice");
        assert_eq!(target.stable_uid(), Some("BuiltInMicrophoneDevice"));
        assert!(target.device.is_none());
        assert!(MicTarget::system_default().stable_uid().is_none());
    }

    #[test]
    fn speaker_rate_follows_airpods_output_clock_instead_of_stale_tap_asbd() {
        assert_eq!(
            effective_speaker_sample_rate(48_000.0, Some(24_000.0), Some(24_000.0)),
            24_000
        );
    }

    #[test]
    fn speaker_rate_prefers_measured_output_then_nominal_then_tap() {
        assert_eq!(
            effective_speaker_sample_rate(48_000.0, Some(44_100.0), Some(44_099.8)),
            44_100
        );
        assert_eq!(
            effective_speaker_sample_rate(48_000.0, Some(44_100.0), Some(0.0)),
            44_100
        );
        assert_eq!(
            effective_speaker_sample_rate(48_000.0, Some(f64::NAN), None),
            48_000
        );
    }

    // --- push_samples_tracked ---

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_test_tone_status_detects_child_failure() {
        assert!(launchd_job_succeeded(
            "state = exited\nlast exit code = 0\n"
        ));
        assert!(launchd_job_succeeded("state = running\nactive count = 1\n"));
        assert!(!launchd_job_succeeded(
            "state = exited\nlast exit code = 1\n"
        ));
        assert!(!launchd_job_succeeded(
            "state = spawn scheduled\nactive count = 0\n"
        ));
    }

    #[test]
    fn silence_detection_all_silent() {
        let (mut producer, _consumer) = rtrb::RingBuffer::new(1024);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);
        let silence = AtomicU64::new(0);

        let silent_data = vec![0.0f32; 480];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &silent_data);

        assert_eq!(silence.load(Ordering::Relaxed), 480);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn silence_detection_resets_on_signal() {
        let (mut producer, _consumer) = rtrb::RingBuffer::new(4096);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);
        let silence = AtomicU64::new(0);

        // First: accumulate silence
        let silent = vec![0.0f32; 480];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &silent);
        assert_eq!(silence.load(Ordering::Relaxed), 480);

        // More silence accumulates
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &silent);
        assert_eq!(silence.load(Ordering::Relaxed), 960);

        // Signal arrives — resets to 0
        let loud = vec![0.5f32; 100];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &loud);
        assert_eq!(silence.load(Ordering::Relaxed), 0);

        // Silence starts counting from zero again
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &silent);
        assert_eq!(silence.load(Ordering::Relaxed), 480);
    }

    #[test]
    fn dead_tap_threshold_boundary() {
        let (mut producer, _consumer) = rtrb::RingBuffer::new(1024);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);
        let silence = AtomicU64::new(0);

        // Exactly at dead-tap threshold — should count as dead
        let at_threshold = vec![DEAD_TAP_THRESHOLD; 100];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &at_threshold);
        assert_eq!(silence.load(Ordering::Relaxed), 100);

        // Just above dead-tap threshold — should reset (live noise floor)
        let above = vec![DEAD_TAP_THRESHOLD + 1e-6; 100];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &above);
        assert_eq!(silence.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn noise_floor_does_not_trigger_dead_tap() {
        let (mut producer, _consumer) = rtrb::RingBuffer::new(4096);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);
        let silence = AtomicU64::new(0);

        // Typical Google Meet noise floor (~0.0002) — live tap, nobody talking
        let noise_floor = vec![0.0002f32; 480];
        push_samples_tracked(&mut producer, &peak, &drops, Some(&silence), &noise_floor);
        assert_eq!(
            silence.load(Ordering::Relaxed),
            0,
            "noise floor should not count as dead tap"
        );
    }

    #[test]
    fn drop_counting_when_buffer_full() {
        // Tiny buffer — will overflow immediately
        let (mut producer, _consumer) = rtrb::RingBuffer::new(10);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);

        let data = vec![0.5f32; 50];
        push_samples_tracked(&mut producer, &peak, &drops, None, &data);

        // 10 fit in the buffer, 40 dropped
        assert_eq!(drops.load(Ordering::Relaxed), 40);
    }

    #[test]
    fn peak_tracking() {
        let (mut producer, _consumer) = rtrb::RingBuffer::new(1024);
        let peak = AtomicU32::new(0);
        let drops = AtomicU64::new(0);

        let data = vec![0.1, 0.3, 0.7, 0.2, 0.05];
        push_samples_tracked(&mut producer, &peak, &drops, None, &data);

        let recorded_peak = f32::from_bits(peak.load(Ordering::Relaxed));
        assert!((recorded_peak - 0.7).abs() < 1e-6);
    }

    #[test]
    fn wasapi_f32_stereo_downmixes_to_mono() {
        let interleaved = [0.2f32, 0.6, -0.4, 0.2];
        let mono = unsafe {
            wasapi_packet_to_mono(
                interleaved.as_ptr() as *const u8,
                2,
                2,
                2 * std::mem::size_of::<f32>(),
                WasapiSampleFormat::Float32,
            )
        };
        assert!((mono[0] - 0.4).abs() < 0.0001);
        assert!((mono[1] + 0.1).abs() < 0.0001);
    }

    #[test]
    fn wasapi_i16_stereo_downmixes_to_mono() {
        let interleaved = [16_384i16, 16_384, -16_384, 0];
        let mono = unsafe {
            wasapi_packet_to_mono(
                interleaved.as_ptr() as *const u8,
                2,
                2,
                2 * std::mem::size_of::<i16>(),
                WasapiSampleFormat::Pcm16,
            )
        };
        assert!((mono[0] - 0.5).abs() < 0.001);
        assert!((mono[1] + 0.25).abs() < 0.001);
    }

    #[test]
    fn wasapi_i24_sign_extends_samples() {
        let positive = [0xff, 0xff, 0x7f];
        let negative = [0x00, 0x00, 0x80];
        let pos = unsafe { pcm24_to_f32(positive.as_ptr()) };
        let neg = unsafe { pcm24_to_f32(negative.as_ptr()) };
        assert!(pos > 0.99);
        assert!(neg < -0.99);
    }

    #[test]
    fn wasapi_i24_packet_downmixes_to_mono() {
        let interleaved = [0x00, 0x00, 0x40, 0x00, 0x00, 0x40];
        let mono = unsafe {
            wasapi_packet_to_mono(interleaved.as_ptr(), 1, 2, 6, WasapiSampleFormat::Pcm24)
        };
        assert!((mono[0] - 0.5).abs() < 0.001);
    }

    #[test]
    fn wasapi_i32_stereo_downmixes_to_mono() {
        let interleaved = [i32::MAX / 2, i32::MAX / 2, -i32::MAX / 2, 0];
        let mono = unsafe {
            wasapi_packet_to_mono(
                interleaved.as_ptr() as *const u8,
                2,
                2,
                2 * std::mem::size_of::<i32>(),
                WasapiSampleFormat::Pcm32,
            )
        };
        assert!((mono[0] - 0.5).abs() < 0.001);
        assert!((mono[1] + 0.25).abs() < 0.001);
    }

    // --- drain_ring_buffer ---

    #[test]
    fn drain_collects_all_samples() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(1024);
        let stop = Arc::new(AtomicBool::new(false));

        // Push some samples before starting drain
        for i in 0..100 {
            producer.push(i as f32 * 0.01).unwrap();
        }

        // Signal stop immediately — drain should still collect what's there
        stop.store(true, Ordering::SeqCst);
        let mut bytes = Vec::new();
        let mut samples_written = 0;
        let mut consumer = consumer;
        drain_ring_buffer_to_writer(
            &mut consumer,
            &stop,
            48000,
            LiveAudioChannel::Mic,
            None,
            &mut bytes,
            &mut samples_written,
        )
        .unwrap();

        assert_eq!(samples_written, 100);
        assert_eq!(bytes.len(), 100 * std::mem::size_of::<f32>());
        assert!((f32::from_le_bytes(bytes[0..4].try_into().unwrap()) - 0.0).abs() < 1e-6);
        assert!((f32::from_le_bytes(bytes[396..400].try_into().unwrap()) - 0.99).abs() < 1e-6);
    }

    #[test]
    fn live_audio_sink_respects_active_generation() {
        let generation_clock = Arc::new(Mutex::new(LiveGenerationClock {
            generation: 0,
            session_offset_ms: 12_375,
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = LiveAudioSink {
            sender: tx,
            generation: 1,
            generation_clock: generation_clock.clone(),
            mic_accepted_samples: Arc::new(AtomicU64::new(0)),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: Arc::new(AtomicU64::new(0)),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            queued_samples: Arc::new(AtomicU64::new(0)),
            queue_max_samples: 0,
        };

        let (mut producer, consumer) = rtrb::RingBuffer::new(1024);
        for i in 0..16 {
            producer.push(i as f32).unwrap();
        }
        let stop = Arc::new(AtomicBool::new(true));
        let mut bytes = Vec::new();
        let mut samples_written = 0;
        let mut consumer = consumer;
        drain_ring_buffer_to_writer(
            &mut consumer,
            &stop,
            48000,
            LiveAudioChannel::Mic,
            Some(&sink),
            &mut bytes,
            &mut samples_written,
        )
        .unwrap();
        assert_eq!(sink.mic_dropped_samples.load(Ordering::Relaxed), 0);
        assert_eq!(samples_written, 16);
        assert!(rx.try_recv().is_err());

        generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation = 1;
        let (mut producer, consumer) = rtrb::RingBuffer::new(1024);
        for i in 0..8 {
            producer.push(i as f32).unwrap();
        }
        let mut bytes = Vec::new();
        let mut samples_written = 0;
        let mut consumer = consumer;
        drain_ring_buffer_to_writer(
            &mut consumer,
            &stop,
            48000,
            LiveAudioChannel::System,
            Some(&sink),
            &mut bytes,
            &mut samples_written,
        )
        .unwrap();
        assert_eq!(samples_written, 8);
        let chunk = rx
            .try_recv()
            .expect("active generation should tee a live chunk");
        assert_eq!(chunk.channel, LiveAudioChannel::System);
        assert_eq!(chunk.generation, 1);
        assert_eq!(chunk.session_offset_ms, 12_375);
        assert_eq!(chunk.samples.len(), 8);
        assert_eq!(sink.system_accepted_samples.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn drain_streams_without_retaining_samples() {
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        let stop = Arc::new(AtomicBool::new(true)); // stop immediately

        let mut bytes = Vec::new();
        let mut samples_written = 0;
        let mut consumer = consumer;
        drain_ring_buffer_to_writer(
            &mut consumer,
            &stop,
            48000,
            LiveAudioChannel::Mic,
            None,
            &mut bytes,
            &mut samples_written,
        )
        .unwrap();

        assert_eq!(samples_written, 0);
        assert!(bytes.is_empty());
    }

    // --- write_stereo_wav ---

    #[test]
    fn write_stereo_wav_basic() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_stereo.wav");
        let path_str = path.to_str().unwrap();

        let mic = vec![0.1f32; 480];
        let spk = vec![0.2f32; 480];

        let mic = write_spool(&mic).unwrap();
        let spk = write_spool(&spk).unwrap();
        let duration = write_stereo_wav_from_spools(path_str, &mic, &spk, 48000, 48000).unwrap();
        assert!((duration - 0.01).abs() < 0.001); // 480/48000 = 0.01s

        // Read back and verify
        let reader = hound::WavReader::open(path_str).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48000);

        let samples: Vec<f32> = reader.into_samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 960); // 480 * 2 channels
                                        // Interleaved: mic, spk, mic, spk...
        assert!((samples[0] - 0.1).abs() < 1e-6); // ch0
        assert!((samples[1] - 0.2).abs() < 1e-6); // ch1

        std::fs::remove_file(path_str).ok();
    }

    #[test]
    fn write_stereo_wav_with_resample() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_stereo_resample.wav");
        let path_str = path.to_str().unwrap();

        let mic = vec![0.1f32; 240]; // 240 samples at 24kHz = 0.01s
        let spk = vec![0.2f32; 480]; // 480 samples at 48kHz = 0.01s

        let mic = write_spool(&mic).unwrap();
        let spk = write_spool(&spk).unwrap();
        let duration = write_stereo_wav_from_spools(path_str, &mic, &spk, 24000, 48000).unwrap();
        assert!((duration - 0.01).abs() < 0.001);

        let reader = hound::WavReader::open(path_str).unwrap();
        assert_eq!(reader.spec().sample_rate, 24000);
        assert_eq!(reader.spec().channels, 2);

        std::fs::remove_file(path_str).ok();
    }

    #[test]
    fn write_stereo_wav_uses_airpods_output_clock_for_full_duration() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_stereo_airpods_clock.wav");
        let path_str = path.to_str().unwrap();

        let mic = write_spool(&vec![0.1f32; 24_000]).unwrap();
        let spk = write_spool(&vec![0.2f32; 24_000]).unwrap();
        let spk_rate = effective_speaker_sample_rate(48_000.0, Some(24_000.0), Some(24_000.0));
        let duration =
            write_stereo_wav_from_spools(path_str, &mic, &spk, 24_000, spk_rate).unwrap();

        assert!((duration - 1.0).abs() < 0.001);
        let reader = hound::WavReader::open(path_str).unwrap();
        let samples: Vec<f32> = reader.into_samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 48_000);
        assert!((samples[samples.len() - 1] - 0.2).abs() < 1e-6);

        std::fs::remove_file(path_str).ok();
    }

    #[test]
    fn write_stereo_wav_unequal_lengths_pads_zeros() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_stereo_unequal.wav");
        let path_str = path.to_str().unwrap();

        let mic = vec![0.5f32; 100];
        let spk = vec![0.3f32; 50]; // shorter

        let mic = write_spool(&mic).unwrap();
        let spk = write_spool(&spk).unwrap();
        write_stereo_wav_from_spools(path_str, &mic, &spk, 48000, 48000).unwrap();

        let reader = hound::WavReader::open(path_str).unwrap();
        let samples: Vec<f32> = reader.into_samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 200); // 100 * 2 channels (padded to longer)

        // Past spk's length, ch1 should be 0.0
        assert!((samples[198] - 0.5).abs() < 1e-6); // mic at index 99
        assert!((samples[199] - 0.0).abs() < 1e-6); // spk padded with 0

        std::fs::remove_file(path_str).ok();
    }

    // --- Integrated: ring buffer pressure simulation ---

    #[test]
    fn simulated_buffer_pressure() {
        // Simulates a producer pushing faster than consumer drains,
        // verifying drop counting works under contention.
        let (mut producer, consumer) = rtrb::RingBuffer::new(100);
        let stop = Arc::new(AtomicBool::new(false));
        let peak = AtomicU32::new(0);

        // Fill buffer to 80%
        for _ in 0..80 {
            producer.push(0.1).unwrap();
        }

        // Now push 50 more with drop tracking — 30 fit, 20 drop
        let data = vec![0.1f32; 50];
        let drops_local = AtomicU64::new(0);
        push_samples_tracked(&mut producer, &peak, &drops_local, None, &data);
        // Ring buffer has capacity 100, 80 already in, ~20 fit (rtrb uses
        // one slot as sentinel, so actual usable capacity is 99)
        let dropped = drops_local.load(Ordering::Relaxed);
        assert!(dropped > 0, "expected some drops, got 0");
        assert!(dropped <= 50, "can't drop more than we pushed");

        // Drain should recover all non-dropped samples
        stop.store(true, Ordering::SeqCst);
        let mut bytes = Vec::new();
        let mut samples_written = 0;
        let mut consumer = consumer;
        drain_ring_buffer_to_writer(
            &mut consumer,
            &stop,
            48000,
            LiveAudioChannel::Mic,
            None,
            &mut bytes,
            &mut samples_written,
        )
        .unwrap();
        assert_eq!(samples_written + dropped, 130); // 80 + 50 = 130 total
    }

    // --- Tap health timing ---

    #[test]
    fn tap_silence_seconds_calculation() {
        // Verify the silence-to-seconds conversion used by health warnings.
        let spk_rate: u32 = 48000;
        let threshold_secs: u64 = 10;
        let threshold_samples = threshold_secs * spk_rate as u64;

        let silence = AtomicU64::new(threshold_samples - 1);
        let silent_secs = silence.load(Ordering::Relaxed) / spk_rate as u64;
        assert_eq!(silent_secs, 9); // not yet at threshold

        silence.store(threshold_samples, Ordering::Relaxed);
        let silent_secs = silence.load(Ordering::Relaxed) / spk_rate as u64;
        assert_eq!(silent_secs, 10); // at threshold — should warn, not restart capture
    }

    #[test]
    fn purge_fence_discards_prefence_standby_and_materializes_gap() {
        let writer = SegmentWriter::start(41, 1_000, 1_000, None).unwrap();
        let sink = writer.mic_sink(9);
        let (mut sample_producer, sample_consumer) = rtrb::RingBuffer::new(256);
        let (mut packet_producer, packet_consumer) = rtrb::RingBuffer::new(64);
        for _ in 0..12 {
            sample_producer.push(9.0).unwrap();
        }
        packet_producer
            .push(PacketDesc {
                frame_count: 12,
                capture_nanos: 0,
            })
            .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let first = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = std::sync::mpsc::sync_channel(2);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = stop.clone();
        let thread_first = first.clone();
        let drain = std::thread::spawn(move || {
            drain_capture_to_lane(
                sample_consumer,
                packet_consumer,
                1_000,
                sink,
                thread_stop,
                command_rx,
                thread_first,
                true,
                ready_tx,
            )
        });
        ready_rx.recv().unwrap().unwrap();
        for _ in 0..50 {
            if first.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(first.load(Ordering::Acquire));

        let (commit_tx, commit_rx) = std::sync::mpsc::channel();
        command_tx
            .send(DrainCommand::CommitStandby(commit_tx))
            .unwrap();
        commit_rx.recv().unwrap().unwrap();
        for _ in 0..8 {
            sample_producer.push(0.25).unwrap();
        }
        packet_producer
            .push(PacketDesc {
                frame_count: 8,
                capture_nanos: 20_000_000,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(15));
        stop.store(true, Ordering::Release);
        let outcome = drain.join().unwrap().unwrap();
        assert!(outcome.retire.is_some());

        let sealed = writer.seal().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        sealed.write_wav(file.path()).unwrap();
        // Archive WAV is now 16kHz/16-bit PCM; read as i16.
        let reader = hound::WavReader::open(file.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        let all_samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        let mic: Vec<i16> = all_samples.into_iter().step_by(2).collect();
        // 9.0 clamped to 1.0 would be i16::MAX (32767). The purge fence must
        // have removed those samples — no frame should saturate at i16::MAX.
        assert!(
            !mic.contains(&i16::MAX),
            "purge-fence samples must be removed"
        );
        // After purging, there must be leading silence followed by non-silent samples.
        // 0.25 → i16 ≈ 8192, resampled; check at least one sample is clearly non-zero.
        assert!(mic.iter().any(|&s| s != 0), "must have non-silent samples");
        // The first non-zero sample must be preceded by silence (gap materialised).
        let first_real = mic.iter().position(|&s| s != 0).unwrap();
        assert!(first_real > 0, "purge-fence gap must be explicit silence");
    }

    #[test]
    fn retirement_final_drain_preserves_last_quiesced_ring_sample() {
        let writer = SegmentWriter::start(42, 1_000, 1_000, None).unwrap();
        let sink = writer.mic_sink(1);
        let (mut sample_producer, sample_consumer) = rtrb::RingBuffer::new(256);
        let (mut packet_producer, packet_consumer) = rtrb::RingBuffer::new(64);
        let stop = Arc::new(AtomicBool::new(false));
        let first = Arc::new(AtomicBool::new(false));
        let (_command_tx, command_rx) = std::sync::mpsc::sync_channel(2);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread_stop = stop.clone();
        let drain = std::thread::spawn(move || {
            drain_capture_to_lane(
                sample_consumer,
                packet_consumer,
                1_000,
                sink,
                thread_stop,
                command_rx,
                first,
                false,
                ready_tx,
            )
        });
        ready_rx.recv().unwrap().unwrap();
        for index in 0..50 {
            sample_producer.push(index as f32 / 100.0).unwrap();
        }
        packet_producer
            .push(PacketDesc {
                frame_count: 50,
                capture_nanos: 0,
            })
            .unwrap();
        // Simulated producer quiescence: no producer writes occur after this
        // point, and only then is the drain permitted to finish.
        stop.store(true, Ordering::Release);
        let outcome = drain.join().unwrap().unwrap();
        let boundary = outcome.retire.unwrap().boundary_frame;
        assert!(boundary >= 50);

        let sealed = writer.seal_at(boundary).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        sealed.write_wav(file.path()).unwrap();
        // Archive WAV is now 16kHz/16-bit PCM; read as i16.
        let reader = hound::WavReader::open(file.path()).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        let all_samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        let mic: Vec<i16> = all_samples.into_iter().step_by(2).collect();
        // The last input sample was index 49 → 0.49 → i16 ≈ 16055.
        // After 16x upsampling (1kHz→16kHz) the max mic value in the archive
        // should reflect that the 0.49 sample was preserved (not silenced).
        let max_mic = mic.iter().copied().max().unwrap_or(0);
        assert!(
            max_mic > 14_000,
            "last ring sample (0.49 → ~16055) must be present; max={max_mic}"
        );
    }
}
