//! Optional direct CoreAudio microphone capture.
//!
//! This module is clean-room work derived only from Apple's CoreAudio SDK
//! headers/documentation, cidre 0.14.2's public API, and this repository's
//! existing process-tap IOProc. No external GPL implementation was consulted.

use super::{
    next_exact_zero_run, MicCaptureTelemetry, MicStreamErrorKind, PacketDesc, MIC_GAIN,
    RING_BUF_SECONDS,
};
use anyhow::{bail, Context, Result};
use cidre::{cat, core_audio as ca, mach, os};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    /// cidre 0.14.2 binds creation, start, and stop, but not the matching
    /// destruction routine. The signature is from AudioHardware.h.
    fn AudioDeviceDestroyIOProcID(device: ca::Device, proc_id: ca::DeviceIoProcId) -> os::Status;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PcmEncoding {
    F32,
    I16,
    I32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InputFormat {
    sample_rate: u32,
    channels: u32,
    encoding: PcmEncoding,
    non_interleaved: bool,
    bytes_per_sample: usize,
}

fn map_input_format(asbd: &cat::AudioStreamBasicDesc) -> Result<InputFormat> {
    if asbd.format != cat::AudioFormat::LINEAR_PCM {
        bail!("CoreAudio input is not linear PCM: {:?}", asbd.format);
    }
    if !asbd.is_native_endian() {
        bail!("CoreAudio input is not native endian");
    }
    if !asbd.format_flags.contains(cat::AudioFormatFlags::IS_PACKED) {
        bail!("CoreAudio input is not packed PCM");
    }
    let encoding = if asbd.format_flags.contains(cat::AudioFormatFlags::IS_FLOAT)
        && asbd.bits_per_channel == 32
    {
        PcmEncoding::F32
    } else if asbd
        .format_flags
        .contains(cat::AudioFormatFlags::IS_SIGNED_INTEGER)
        && asbd.bits_per_channel == 16
    {
        PcmEncoding::I16
    } else if asbd
        .format_flags
        .contains(cat::AudioFormatFlags::IS_SIGNED_INTEGER)
        && asbd.bits_per_channel == 32
    {
        PcmEncoding::I32
    } else {
        bail!(
            "unsupported CoreAudio PCM flags/bits: flags={} bits={}",
            asbd.format_flags,
            asbd.bits_per_channel
        );
    };
    let sample_rate = f64_to_sample_rate(asbd.sample_rate)?;
    if asbd.channels_per_frame == 0 {
        bail!("CoreAudio input reports zero channels");
    }
    Ok(InputFormat {
        sample_rate,
        channels: asbd.channels_per_frame,
        encoding,
        non_interleaved: asbd
            .format_flags
            .contains(cat::AudioFormatFlags::IS_NON_INTERLEAVED),
        bytes_per_sample: usize::try_from(asbd.bits_per_channel / 8)
            .context("CoreAudio bits-per-channel does not fit usize")?,
    })
}

fn f64_to_sample_rate(rate: f64) -> Result<u32> {
    if !rate.is_finite() || rate <= 0.0 || rate.fract() != 0.0 || rate > f64::from(u32::MAX) {
        bail!("unsupported CoreAudio sample rate: {rate}");
    }
    Ok(rate as u32)
}

fn resolve_device_by_uid(uid: &str) -> Result<ca::Device> {
    ca::System::devices()
        .context("could not enumerate CoreAudio devices")?
        .into_iter()
        .find(|device| {
            device
                .uid()
                .is_ok_and(|candidate| candidate.to_string() == uid)
        })
        .with_context(|| format!("CoreAudio input UID is unavailable: {uid}"))
}

#[cfg(test)]
fn input_devices() -> Result<Vec<ca::Device>> {
    Ok(ca::System::devices()
        .context("could not enumerate CoreAudio devices")?
        .into_iter()
        .filter(|device| {
            device.input_stream_cfg().is_ok_and(|config| {
                config.number_buffers() > 0
                    && config
                        .buffers()
                        .iter()
                        .take(config.number_buffers())
                        .any(|buffer| buffer.number_channels > 0)
            })
        })
        .collect())
}

struct IoProcRegistration {
    device: ca::Device,
    proc_id: Option<ca::DeviceIoProcId>,
}

impl IoProcRegistration {
    fn new(device: ca::Device, proc_id: ca::DeviceIoProcId) -> Self {
        Self {
            device,
            proc_id: Some(proc_id),
        }
    }

    fn proc_id(&self) -> ca::DeviceIoProcId {
        self.proc_id.expect("IOProc registration already destroyed")
    }

    fn destroy(&mut self) -> Result<()> {
        let Some(proc_id) = self.proc_id.take() else {
            return Ok(());
        };
        // SAFETY: this ID was returned by AudioDeviceCreateIOProcID for this
        // exact device, and the Option enforces exactly one destroy attempt.
        unsafe { AudioDeviceDestroyIOProcID(self.device, proc_id) }
            .result()
            .map_err(|error| anyhow::anyhow!("AudioDeviceDestroyIOProcID failed: {error}"))
    }
}

impl Drop for IoProcRegistration {
    fn drop(&mut self) {
        let result = self.destroy();
        debug_assert!(result.is_ok(), "{result:?}");
    }
}

const OBSERVED_BUFFERS: usize = 8;

struct InputCtx {
    sample_producer: rtrb::Producer<f32>,
    packet_producer: rtrb::Producer<PacketDesc>,
    telemetry: MicCaptureTelemetry,
    format: InputFormat,
    timebase_numer: u32,
    timebase_denom: u32,
    first_host_time: u64,
    callbacks: AtomicU64,
    callbacks_in_flight: AtomicU32,
    pushed_samples: AtomicU64,
    last_pushed_bits: AtomicU32,
    observed_number_buffers: AtomicU32,
    observed_buffer_channels: [AtomicU32; OBSERVED_BUFFERS],
    observed_buffer_bytes: [AtomicU32; OBSERVED_BUFFERS],
}

impl InputCtx {
    fn new(
        sample_producer: rtrb::Producer<f32>,
        packet_producer: rtrb::Producer<PacketDesc>,
        telemetry: MicCaptureTelemetry,
        format: InputFormat,
    ) -> Self {
        let timebase = mach::TimeBaseInfo::new();
        Self {
            sample_producer,
            packet_producer,
            telemetry,
            format,
            timebase_numer: timebase.numer,
            timebase_denom: timebase.denom,
            first_host_time: 0,
            callbacks: AtomicU64::new(0),
            callbacks_in_flight: AtomicU32::new(0),
            pushed_samples: AtomicU64::new(0),
            last_pushed_bits: AtomicU32::new(0),
            observed_number_buffers: AtomicU32::new(0),
            observed_buffer_channels: std::array::from_fn(|_| AtomicU32::new(0)),
            observed_buffer_bytes: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

struct CallbackFlight(*const AtomicU32);

impl Drop for CallbackFlight {
    fn drop(&mut self) {
        // SAFETY: the guard is created from the callback context and is
        // dropped before that context can be released.
        unsafe { &*self.0 }.fetch_sub(1, Ordering::Release);
    }
}

extern "C" fn input_io_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input_data: &cat::AudioBufList<1>,
    input_time: &cat::AudioTimeStamp,
    _output_data: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut InputCtx>,
) -> os::Status {
    let Some(ctx) = ctx else {
        return os::Status::NO_ERR;
    };
    ctx.callbacks_in_flight.fetch_add(1, Ordering::Acquire);
    let _flight = CallbackFlight(&ctx.callbacks_in_flight);
    ctx.callbacks.fetch_add(1, Ordering::Relaxed);

    let number_buffers = usize::try_from(input_data.number_buffers).unwrap_or(0);
    ctx.observed_number_buffers
        .store(input_data.number_buffers, Ordering::Relaxed);
    // AudioBufferList ends in a variable-length AudioBuffer array. cidre's
    // `AudioBufList<1>` represents the ABI prefix; the HAL-owned allocation is
    // sized according to `number_buffers`.
    let buffers =
        unsafe { std::slice::from_raw_parts(input_data.buffers.as_ptr(), number_buffers) };
    for (index, buffer) in buffers.iter().take(OBSERVED_BUFFERS).enumerate() {
        ctx.observed_buffer_channels[index].store(buffer.number_channels, Ordering::Relaxed);
        ctx.observed_buffer_bytes[index].store(buffer.data_bytes_size, Ordering::Relaxed);
    }

    let capture_nanos = capture_nanos(ctx, input_time);
    let Some(buffer) = buffers
        .iter()
        .find(|buffer| buffer.number_channels > 0 && !buffer.data.is_null())
    else {
        return os::Status::NO_ERR;
    };
    let channels = usize::try_from(buffer.number_channels).unwrap_or(0);
    let frame_bytes = match ctx.format.bytes_per_sample.checked_mul(channels) {
        Some(value) if value > 0 => value,
        _ => return os::Status::NO_ERR,
    };
    let frames = usize::try_from(buffer.data_bytes_size).unwrap_or(0) / frame_bytes;
    let mut local_peak = 0.0f32;
    let mut local_drops = 0u64;
    let mut pushed = 0u64;
    let mut last_bits = 0u32;
    let mut silent_run = ctx.telemetry.silence.load(Ordering::Relaxed);

    for frame in 0..frames {
        let sample =
            unsafe { read_channel_zero(buffer.data, frame, channels, ctx.format.encoding) };
        let sample = (sample * MIC_GAIN).clamp(-1.0, 1.0);
        silent_run = next_exact_zero_run(silent_run, sample);
        local_peak = local_peak.max(sample.abs());
        if ctx.sample_producer.push(sample).is_ok() {
            pushed += 1;
            last_bits = sample.to_bits();
        } else {
            local_drops += 1;
        }
    }
    ctx.telemetry
        .peak
        .fetch_max(local_peak.to_bits(), Ordering::Relaxed);
    ctx.telemetry.silence.store(silent_run, Ordering::Relaxed);
    if local_drops > 0 {
        ctx.telemetry
            .drops
            .fetch_add(local_drops, Ordering::Relaxed);
    }
    if pushed > 0 {
        ctx.pushed_samples.fetch_add(pushed, Ordering::Relaxed);
        ctx.last_pushed_bits.store(last_bits, Ordering::Relaxed);
    }
    let frame_count = u32::try_from(frames).unwrap_or(u32::MAX);
    ctx.telemetry
        .frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    if ctx
        .packet_producer
        .push(PacketDesc {
            frame_count,
            capture_nanos,
        })
        .is_err()
    {
        ctx.telemetry
            .packet_drops
            .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    }
    os::Status::NO_ERR
}

unsafe fn read_channel_zero(
    data: *mut u8,
    frame: usize,
    channels: usize,
    encoding: PcmEncoding,
) -> f32 {
    let index = frame.saturating_mul(channels);
    match encoding {
        PcmEncoding::F32 => unsafe { *data.cast::<f32>().add(index) },
        PcmEncoding::I16 => f32::from(unsafe { *data.cast::<i16>().add(index) }) / 32768.0,
        PcmEncoding::I32 => (unsafe { *data.cast::<i32>().add(index) }) as f32 / 2_147_483_648.0,
    }
}

fn capture_nanos(ctx: &mut InputCtx, input_time: &cat::AudioTimeStamp) -> u64 {
    if input_time.host_time == 0 || ctx.timebase_denom == 0 {
        return 0;
    }
    if ctx.first_host_time == 0 {
        ctx.first_host_time = input_time.host_time;
    }
    let ticks = input_time.host_time.saturating_sub(ctx.first_host_time);
    u64::try_from(
        u128::from(ticks) * u128::from(ctx.timebase_numer) / u128::from(ctx.timebase_denom),
    )
    .unwrap_or(u64::MAX)
}

struct PreparedInput {
    registration: IoProcRegistration,
    ctx: Box<InputCtx>,
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<PacketDesc>,
    asbd: cat::AudioStreamBasicDesc,
}

impl PreparedInput {
    fn prepare(device: ca::Device, telemetry: MicCaptureTelemetry) -> Result<Self> {
        let asbd = device
            .input_asbd()
            .context("could not read CoreAudio input ASBD")?;
        let format = map_input_format(&asbd)?;
        let capacity = usize::try_from(format.sample_rate)
            .context("sample rate does not fit usize")?
            .checked_mul(RING_BUF_SECONDS)
            .context("CoreAudio sample ring capacity overflow")?;
        let (sample_producer, samples) = rtrb::RingBuffer::new(capacity);
        let (packet_producer, packets) = rtrb::RingBuffer::new(32_768);
        let mut ctx = Box::new(InputCtx::new(
            sample_producer,
            packet_producer,
            telemetry,
            format,
        ));
        let proc_id = device
            .create_io_proc_id(input_io_proc, Some(ctx.as_mut()))
            .map_err(|error| anyhow::anyhow!("AudioDeviceCreateIOProcID failed: {error}"))?;
        Ok(Self {
            registration: IoProcRegistration::new(device, proc_id),
            ctx,
            samples,
            packets,
            asbd,
        })
    }

    #[cfg(test)]
    fn cancel(mut self) -> Result<()> {
        self.registration.destroy()
    }

    fn start(self) -> Result<RunningInput> {
        let started_at = Instant::now();
        let started = ca::device_start(self.registration.device, Some(self.registration.proc_id()))
            .map_err(|error| anyhow::anyhow!("AudioDeviceStart failed: {error}"))?;
        let start_returned_after = started_at.elapsed();
        Ok(RunningInput {
            started: Some(started),
            registration: self.registration,
            ctx: self.ctx,
            samples: self.samples,
            packets: self.packets,
            asbd: self.asbd,
            started_at,
            start_returned_after,
        })
    }
}

#[cfg_attr(not(test), allow(dead_code))]
struct RunningInput {
    // Field order is intentional: stop before destroy before freeing context.
    started: Option<ca::hardware::StartedDevice<ca::Device>>,
    registration: IoProcRegistration,
    ctx: Box<InputCtx>,
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<PacketDesc>,
    asbd: cat::AudioStreamBasicDesc,
    started_at: Instant,
    start_returned_after: Duration,
}

/// Production owner for a started HAL input IOProc. The sample and packet
/// consumers are moved to Margins' existing drain thread; this value retains
/// only the callback registration and context that must outlive every callback.
pub(super) struct CoreAudioMicStream {
    started: Option<ca::hardware::StartedDevice<ca::Device>>,
    listeners: Option<DeviceListeners>,
    registration: IoProcRegistration,
    ctx: Box<InputCtx>,
}

pub(super) struct CoreAudioRawCapture {
    pub stream: CoreAudioMicStream,
    pub native_rate: u32,
    pub samples: rtrb::Consumer<f32>,
    pub packets: rtrb::Consumer<PacketDesc>,
}

impl CoreAudioMicStream {
    pub(super) fn stop(mut self) -> Result<()> {
        if let Some(started) = self.started.take() {
            started
                .stop()
                .map_err(|error| anyhow::anyhow!("AudioDeviceStop failed: {error}"))?;
        }
        self.listeners.take();

        // AudioDeviceStop is synchronous. Also observe the explicit in-flight
        // guard before destroying the callback registration and context.
        while self.ctx.callbacks_in_flight.load(Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
        self.registration.destroy()
    }
}

impl Drop for CoreAudioMicStream {
    fn drop(&mut self) {
        if let Some(started) = self.started.take() {
            let _ = started.stop();
        }
        self.listeners.take();
        while self.ctx.callbacks_in_flight.load(Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
        let _ = self.registration.destroy();
    }
}

/// Start the native macOS microphone backend by stable CoreAudio UID. A
/// missing UID intentionally means the live system default.
pub(super) fn start_input(
    stable_uid: Option<&str>,
    telemetry: MicCaptureTelemetry,
) -> Result<CoreAudioRawCapture> {
    let device = match stable_uid {
        Some(uid) => resolve_device_by_uid(uid)?,
        None => ca::System::default_input_device()
            .context("no default CoreAudio input device is available")?,
    };
    let error = telemetry.error.clone();
    let running = PreparedInput::prepare(device, telemetry)?.start()?;
    let listeners = DeviceListeners::start(device, error)?;
    let native_rate = f64_to_sample_rate(running.asbd.sample_rate)?;
    let RunningInput {
        started,
        registration,
        ctx,
        samples,
        packets,
        ..
    } = running;
    Ok(CoreAudioRawCapture {
        stream: CoreAudioMicStream {
            started,
            listeners: Some(listeners),
            registration,
            ctx,
        },
        native_rate,
        samples,
        packets,
    })
}

#[cfg(test)]
struct StoppedInput {
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<PacketDesc>,
    callbacks_at_stop: u64,
    callbacks_after_quiescence_wait: u64,
    callbacks_in_flight: u32,
    pushed_samples: u64,
    last_pushed_bits: u32,
    stop_duration: Duration,
    observed_number_buffers: u32,
    observed_buffer_channels: [u32; OBSERVED_BUFFERS],
    observed_buffer_bytes: [u32; OBSERVED_BUFFERS],
}

#[cfg(test)]
impl RunningInput {
    fn callback_count(&self) -> u64 {
        self.ctx.callbacks.load(Ordering::Acquire)
    }

    fn wait_for_first_packet(&self, timeout: Duration) -> Result<Duration> {
        while self.callback_count() == 0 {
            if self.started_at.elapsed() >= timeout {
                bail!("timed out waiting for the first CoreAudio input callback");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(self.started_at.elapsed())
    }

    fn stop(mut self) -> Result<StoppedInput> {
        let stop_started = Instant::now();
        let started = self
            .started
            .take()
            .context("CoreAudio input already stopped")?;
        started
            .stop()
            .map_err(|error| anyhow::anyhow!("AudioDeviceStop failed: {error}"))?;
        let stop_duration = stop_started.elapsed();
        let callbacks_at_stop = self.ctx.callbacks.load(Ordering::Acquire);
        let callbacks_in_flight = self.ctx.callbacks_in_flight.load(Ordering::Acquire);
        // Keep both the registered IOProc and its context alive after stop so
        // any late callback is observable instead of becoming a lifetime bug.
        std::thread::sleep(Duration::from_millis(100));
        let callbacks_after_quiescence_wait = self.ctx.callbacks.load(Ordering::Acquire);
        let pushed_samples = self.ctx.pushed_samples.load(Ordering::Acquire);
        let last_pushed_bits = self.ctx.last_pushed_bits.load(Ordering::Acquire);
        let observed_number_buffers = self.ctx.observed_number_buffers.load(Ordering::Acquire);
        let observed_buffer_channels = std::array::from_fn(|index| {
            self.ctx.observed_buffer_channels[index].load(Ordering::Acquire)
        });
        let observed_buffer_bytes = std::array::from_fn(|index| {
            self.ctx.observed_buffer_bytes[index].load(Ordering::Acquire)
        });
        self.registration.destroy()?;
        Ok(StoppedInput {
            samples: self.samples,
            packets: self.packets,
            callbacks_at_stop,
            callbacks_after_quiescence_wait,
            callbacks_in_flight,
            pushed_samples,
            last_pushed_bits,
            stop_duration,
            observed_number_buffers,
            observed_buffer_channels,
            observed_buffer_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceEvent {
    IsAlive,
    NominalRate,
}

struct DeviceListenerCtx {
    sender: Option<SyncSender<DeviceEvent>>,
    error: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

struct DeviceListeners {
    device: ca::Device,
    ctx: Box<DeviceListenerCtx>,
    addrs: [(ca::PropAddr, DeviceEvent); 2],
    registered: usize,
}

extern "C-unwind" fn device_listener(
    _object: ca::Obj,
    number_addresses: u32,
    addresses: *const ca::PropAddr,
    client_data: *mut DeviceListenerCtx,
) -> os::Status {
    if client_data.is_null() || addresses.is_null() {
        return os::Status::NO_ERR;
    }
    let ctx = unsafe { &*client_data };
    let addresses = unsafe {
        std::slice::from_raw_parts(addresses, usize::try_from(number_addresses).unwrap_or(0))
    };
    for address in addresses {
        let event = if address.selector == ca::PropSelector::DEVICE_IS_ALIVE {
            Some(DeviceEvent::IsAlive)
        } else if address.selector == ca::PropSelector::DEVICE_NOMINAL_SAMPLE_RATE {
            Some(DeviceEvent::NominalRate)
        } else {
            None
        };
        if let Some(event) = event {
            let kind = match event {
                DeviceEvent::IsAlive => MicStreamErrorKind::DeviceNotAvailable,
                DeviceEvent::NominalRate => MicStreamErrorKind::BackendSpecific,
            };
            ctx.error.store(kind as u8, Ordering::Release);
            if let Some(sender) = &ctx.sender {
                match sender.try_send(event) {
                    Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
                }
            }
        }
    }
    os::Status::NO_ERR
}

impl DeviceListeners {
    fn start(
        device: ca::Device,
        error: std::sync::Arc<std::sync::atomic::AtomicU8>,
    ) -> Result<Self> {
        Self::start_inner(device, error, None)
    }

    fn start_inner(
        device: ca::Device,
        error: std::sync::Arc<std::sync::atomic::AtomicU8>,
        sender: Option<SyncSender<DeviceEvent>>,
    ) -> Result<Self> {
        let mut listeners = Self {
            device,
            ctx: Box::new(DeviceListenerCtx { sender, error }),
            addrs: [
                (
                    ca::PropSelector::DEVICE_IS_ALIVE.global_addr(),
                    DeviceEvent::IsAlive,
                ),
                (
                    ca::PropSelector::DEVICE_NOMINAL_SAMPLE_RATE.global_addr(),
                    DeviceEvent::NominalRate,
                ),
            ],
            registered: 0,
        };
        for (address, _) in &listeners.addrs {
            listeners
                .device
                .add_prop_listener(
                    address,
                    device_listener,
                    listeners.ctx.as_mut() as *mut DeviceListenerCtx,
                )
                .map_err(|error| {
                    anyhow::anyhow!("AudioObjectAddPropertyListener failed: {error}")
                })?;
            listeners.registered += 1;
        }
        Ok(listeners)
    }

    #[cfg(test)]
    fn start_with_sender(device: ca::Device, sender: SyncSender<DeviceEvent>) -> Result<Self> {
        use std::sync::atomic::AtomicU8;
        use std::sync::Arc;

        Self::start_inner(device, Arc::new(AtomicU8::new(0)), Some(sender))
    }
}

impl Drop for DeviceListeners {
    fn drop(&mut self) {
        for (address, _) in self.addrs.iter().take(self.registered) {
            let result = self.device.remove_prop_listener(
                address,
                device_listener,
                self.ctx.as_mut() as *mut DeviceListenerCtx,
            );
            debug_assert!(result.is_ok(), "{result:?}");
        }
        self.registered = 0;
    }
}

#[cfg(test)]
fn spike_telemetry() -> MicCaptureTelemetry {
    use std::sync::atomic::AtomicU8;
    use std::sync::Arc;

    MicCaptureTelemetry {
        peak: Arc::new(AtomicU32::new(0)),
        drops: Arc::new(AtomicU64::new(0)),
        packet_drops: Arc::new(AtomicU64::new(0)),
        frames: Arc::new(AtomicU64::new(0)),
        silence: Arc::new(AtomicU64::new(0)),
        error: Arc::new(AtomicU8::new(0)),
    }
}

#[cfg(test)]
fn device_label(device: ca::Device) -> String {
    let name = device
        .name()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "<unknown name>".to_owned());
    let uid = device
        .uid()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "<unknown UID>".to_owned());
    format!("{name} [{uid}]")
}

#[cfg(test)]
fn select_spike_device(devices: &[ca::Device]) -> Result<ca::Device> {
    if let Ok(uid) = std::env::var("MARGINS_COREAUDIO_SPIKE_UID") {
        return resolve_device_by_uid(&uid);
    }
    devices
        .iter()
        .copied()
        .find(|device| {
            device
                .transport_type()
                .is_ok_and(|transport| transport == ca::DeviceTransportType::BUILT_IN)
        })
        .or_else(|| ca::System::default_input_device().ok())
        .context("no CoreAudio input device is available")
}

/// Native hardware spike for 02R section 9 / amendment A9.
///
/// Run serially with output visible:
/// `cargo test -p margins native_coreaudio_input_spike -- --ignored --nocapture --test-threads=1`
///
/// By default the built-in microphone is used, falling back to the system
/// default. Set `MARGINS_COREAUDIO_SPIKE_UID` to exercise a particular UID.
/// If authorization is `NotDetermined`, rerun interactively with
/// `MARGINS_COREAUDIO_SPIKE_REQUEST_PERMISSION=1` and answer the macOS prompt.
/// Set `MARGINS_COREAUDIO_SPIKE_PRIVACY_WAIT_SECS=5` to leave a longer window
/// for observing the privacy indicator before the IOProc is started.
/// The test prints a manual privacy-indicator observation checkpoint after
/// IOProc creation but before start; it deliberately does not assert UI state.
#[cfg(test)]
#[test]
#[ignore = "requires macOS microphone permission and real input hardware"]
fn native_coreaudio_input_spike() {
    let devices = match input_devices() {
        Ok(devices) if !devices.is_empty() => devices,
        Ok(_) => {
            eprintln!("SKIP: no CoreAudio input device is present");
            return;
        }
        Err(error) => panic!("CoreAudio enumeration failed: {error:#}"),
    };
    eprintln!("CoreAudio input devices (UID-resolved, no name round-trip):");
    for device in &devices {
        let asbd = device.input_asbd();
        let layout = device.input_stream_cfg();
        eprintln!(
            "  {} transport={:?} nominal={:?} actual={:?} ASBD={asbd:?} layout={layout:?}",
            device_label(*device),
            device.transport_type(),
            device.nominal_sample_rate(),
            device.actual_sample_rate(),
        );
    }
    let device = select_spike_device(&devices).expect("spike device selection failed");
    let is_builtin = device
        .transport_type()
        .is_ok_and(|transport| transport == ca::DeviceTransportType::BUILT_IN);
    eprintln!("selected={} built_in={is_builtin}", device_label(device));
    if !is_builtin {
        eprintln!(
            "SKIP-NOTE: no built-in input was available; layout proof used the selected device"
        );
    }

    let prepared = PreparedInput::prepare(device, spike_telemetry())
        .expect("creating a prepared IOProc must succeed");
    eprintln!("prepared ASBD={:?}", prepared.asbd);
    let privacy_wait = std::env::var("MARGINS_COREAUDIO_SPIKE_PRIVACY_WAIT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_millis(750));
    std::thread::sleep(privacy_wait);
    assert_eq!(
        prepared.ctx.callbacks.load(Ordering::Acquire),
        0,
        "a created-but-not-started IOProc must not receive callbacks"
    );
    eprintln!(
        "MANUAL OBSERVATION: during the preceding {privacy_wait:?} prepared-only window, verify macOS showed no microphone privacy indicator"
    );
    prepared
        .cancel()
        .expect("prepared IOProc cancellation failed");
    for _ in 0..16 {
        PreparedInput::prepare(device, spike_telemetry())
            .expect("repeat IOProc creation failed")
            .cancel()
            .expect("repeat AudioDeviceDestroyIOProcID failed");
    }
    eprintln!("prepare/cancel cleanup: 17/17 destroy calls succeeded");

    let mut authorization = super::microphone_authorization();
    if matches!(
        authorization,
        Ok(super::MicrophoneAuthorization::NotDetermined)
    ) && std::env::var_os("MARGINS_COREAUDIO_SPIKE_REQUEST_PERMISSION").is_some()
    {
        eprintln!("REQUESTING: answer the macOS microphone-access prompt for the test process");
        authorization = match super::request_microphone_access() {
            Ok(true) => Ok(super::MicrophoneAuthorization::Authorized),
            Ok(false) => Ok(super::MicrophoneAuthorization::Denied),
            Err(error) => panic!("microphone permission request failed: {error:#}"),
        };
    }
    if !matches!(
        authorization,
        Ok(super::MicrophoneAuthorization::Authorized)
    ) {
        eprintln!(
            "SKIP: start/delivery proofs require microphone authorization; current state: {authorization:?}"
        );
        return;
    }

    let running = PreparedInput::prepare(device, spike_telemetry())
        .and_then(PreparedInput::start)
        .expect("CoreAudio input start failed");
    let start_returned = running.start_returned_after;
    let first_packet = running
        .wait_for_first_packet(Duration::from_secs(3))
        .expect("first CoreAudio packet not delivered");
    std::thread::sleep(Duration::from_millis(500));
    let asbd = running.asbd;
    let stopped = running.stop().expect("CoreAudio input stop failed");
    assert_eq!(stopped.callbacks_in_flight, 0);
    assert!(stopped.callbacks_at_stop > 0);
    assert_eq!(
        stopped.callbacks_at_stop, stopped.callbacks_after_quiescence_wait,
        "CoreAudio invoked the IOProc after AudioDeviceStop returned"
    );

    let mut samples = stopped.samples;
    let mut drained = 0u64;
    let mut last = None;
    while let Ok(sample) = samples.pop() {
        drained += 1;
        last = Some(sample);
    }
    let mut packets = stopped.packets;
    let mut packet_count = 0u64;
    let mut packet_frames = 0u64;
    while let Ok(packet) = packets.pop() {
        packet_count += 1;
        packet_frames += u64::from(packet.frame_count);
    }
    assert_eq!(
        drained, stopped.pushed_samples,
        "tail sample count mismatch"
    );
    assert_eq!(
        last.map(f32::to_bits),
        Some(stopped.last_pushed_bits),
        "the final callback sample was not retained after stop"
    );
    assert!(packet_count > 0, "no PacketDesc was delivered");
    assert_eq!(packet_frames, stopped.pushed_samples);
    assert!(stopped.observed_number_buffers > 0);
    eprintln!(
        "single-client proof: ASBD={asbd:?} start_return={:?} first_packet={:?} stop={:?} callbacks={} buffers={} buffer_channels={:?} buffer_bytes={:?} samples={} packets={}",
        start_returned,
        first_packet,
        stopped.stop_duration,
        stopped.callbacks_at_stop,
        stopped.observed_number_buffers,
        &stopped.observed_buffer_channels[..usize::try_from(stopped.observed_number_buffers).unwrap_or(0).min(OBSERVED_BUFFERS)],
        &stopped.observed_buffer_bytes[..usize::try_from(stopped.observed_number_buffers).unwrap_or(0).min(OBSERVED_BUFFERS)],
        drained,
        packet_count,
    );

    if let Some(second) = devices
        .iter()
        .copied()
        .find(|candidate| *candidate != device)
    {
        let first = PreparedInput::prepare(device, spike_telemetry())
            .and_then(PreparedInput::start)
            .expect("first concurrent CoreAudio client failed to start");
        let second_running = PreparedInput::prepare(second, spike_telemetry())
            .and_then(PreparedInput::start)
            .expect("second concurrent CoreAudio client failed to start");
        let first_latency = first
            .wait_for_first_packet(Duration::from_secs(3))
            .expect("first concurrent client delivered no packet");
        let second_latency = second_running
            .wait_for_first_packet(Duration::from_secs(3))
            .expect("second concurrent client delivered no packet");
        let first_stopped = first.stop().expect("first concurrent stop failed");
        let second_stopped = second_running
            .stop()
            .expect("second concurrent stop failed");
        eprintln!(
            "dual-device proof: first={} callbacks={} first_packet={:?}; second={} callbacks={} first_packet={:?}",
            device_label(device),
            first_stopped.callbacks_at_stop,
            first_latency,
            device_label(second),
            second_stopped.callbacks_at_stop,
            second_latency,
        );
    } else {
        eprintln!("SKIP: dual-device proof requires a second CoreAudio input device");
    }
}

/// Manual device-loss proof. Set the UID of a removable input, start this test,
/// and physically unplug/disable it while the test waits:
/// `MARGINS_COREAUDIO_SPIKE_DEVICE_LOSS_UID=... cargo test -p margins native_coreaudio_device_loss_listener -- --ignored --nocapture --test-threads=1`
#[cfg(test)]
#[test]
#[ignore = "requires manually disconnecting the selected CoreAudio device"]
fn native_coreaudio_device_loss_listener() {
    let Ok(uid) = std::env::var("MARGINS_COREAUDIO_SPIKE_DEVICE_LOSS_UID") else {
        eprintln!("SKIP: set MARGINS_COREAUDIO_SPIKE_DEVICE_LOSS_UID to a removable input UID");
        return;
    };
    let device = resolve_device_by_uid(&uid).expect("device-loss UID did not resolve");
    let (sender, receiver) = std::sync::mpsc::sync_channel(8);
    let _listeners =
        DeviceListeners::start_with_sender(device, sender).expect("listener registration failed");
    let running = PreparedInput::prepare(device, spike_telemetry())
        .and_then(PreparedInput::start)
        .expect("device-loss capture start failed");
    running
        .wait_for_first_packet(Duration::from_secs(3))
        .expect("device-loss capture delivered no first packet");
    eprintln!(
        "UNPLUG NOW: waiting 20 seconds for DeviceIsAlive on {}",
        device_label(device)
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw_alive = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(DeviceEvent::IsAlive) => {
                saw_alive = true;
                break;
            }
            Ok(DeviceEvent::NominalRate) => {
                eprintln!("listener also observed nominal-rate change");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("device listener disconnected: {error}"),
        }
    }
    assert!(
        saw_alive,
        "no DeviceIsAlive callback arrived after the manual disconnect window"
    );
    assert!(!device.is_alive().unwrap_or(false));
    let _ = running.stop();
    eprintln!(
        "device-loss proof: DeviceIsAlive callback received; listener callback used try_send only"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asbd(bits: u32, flags: cat::AudioFormatFlags, channels: u32) -> cat::AudioStreamBasicDesc {
        let bytes = bits / 8;
        cat::AudioStreamBasicDesc {
            sample_rate: 48_000.0,
            format: cat::AudioFormat::LINEAR_PCM,
            format_flags: flags,
            bytes_per_packet: bytes * channels,
            frames_per_packet: 1,
            bytes_per_frame: bytes * channels,
            channels_per_frame: channels,
            bits_per_channel: bits,
            reserved: 0,
        }
    }

    #[test]
    fn maps_supported_coreaudio_input_formats() {
        let f32_format =
            map_input_format(&asbd(32, cat::AudioFormatFlags::NATIVE_FLOAT_PACKED, 2)).unwrap();
        assert_eq!(f32_format.encoding, PcmEncoding::F32);
        assert_eq!(f32_format.channels, 2);

        let i16_format = map_input_format(&asbd(
            16,
            cat::AudioFormatFlags(
                cat::AudioFormatFlags::IS_SIGNED_INTEGER.0 | cat::AudioFormatFlags::IS_PACKED.0,
            ),
            1,
        ))
        .unwrap();
        assert_eq!(i16_format.encoding, PcmEncoding::I16);

        let i32_non_interleaved = map_input_format(&asbd(
            32,
            cat::AudioFormatFlags(
                cat::AudioFormatFlags::IS_SIGNED_INTEGER.0
                    | cat::AudioFormatFlags::IS_PACKED.0
                    | cat::AudioFormatFlags::IS_NON_INTERLEAVED.0,
            ),
            4,
        ))
        .unwrap();
        assert_eq!(i32_non_interleaved.encoding, PcmEncoding::I32);
        assert!(i32_non_interleaved.non_interleaved);
    }

    #[test]
    fn rejects_unsupported_coreaudio_input_formats() {
        let mut compressed = asbd(32, cat::AudioFormatFlags::NATIVE_FLOAT_PACKED, 1);
        compressed.format = cat::AudioFormat::MPEG4_AAC;
        assert!(map_input_format(&compressed).is_err());

        let unpacked = asbd(32, cat::AudioFormatFlags::IS_FLOAT, 1);
        assert!(map_input_format(&unpacked).is_err());

        let unsigned = asbd(16, cat::AudioFormatFlags::IS_PACKED, 1);
        assert!(map_input_format(&unsigned).is_err());
    }

    #[test]
    fn sample_rate_mapping_is_exact_and_bounded() {
        assert_eq!(f64_to_sample_rate(44_100.0).unwrap(), 44_100);
        assert!(f64_to_sample_rate(0.0).is_err());
        assert!(f64_to_sample_rate(48_000.5).is_err());
        assert!(f64_to_sample_rate(f64::NAN).is_err());
    }
}
