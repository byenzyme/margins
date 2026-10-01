pub mod alignment;
pub mod app;
pub mod asr;
pub mod audio_info;
pub mod audio_pipeline;
#[cfg(feature = "recall-local-model")]
pub mod catalyst_model_setup;
pub mod cli;
mod cli_log;
mod core_compat;
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub mod coreml_asr;
pub mod diarization;
pub mod granola_import;
pub mod hosted_credentials;
pub mod included_lease;
mod note;
pub mod note_artifacts;
pub mod offline_asr;
pub mod parser;
pub mod project;
pub mod publish;
#[cfg(feature = "recall")]
pub mod recall;
#[cfg(feature = "recall")]
pub mod scan;
#[cfg(feature = "recall")]
mod setup_compile;
mod speech_model_setup;
#[cfg(feature = "recall")]
mod workspace_recall;
/// Portable public contracts. The root crate re-exports these and provides
/// only lossless legacy-value conversions; it does not implement the ports.
pub use margins_core as core;

/// Initialize the process-wide SQLite runtime before recall and Margins storage
/// can open their independently wrapped connections.
pub fn initialize_sqlite_runtime() -> anyhow::Result<()> {
    #[cfg(feature = "recall")]
    recall_engine::initialize_sqlite_runtime()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_process_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
#[cfg(feature = "audio-capture")]
pub mod recorder;
#[cfg(not(feature = "audio-capture"))]
pub mod recorder {
    use anyhow::{bail, Result};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
    use std::sync::mpsc::{Sender, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Name of the virtual audio tap device the app creates for system-audio
    /// capture. Mirrored from the real module: it is a plain constant with no
    /// platform dependency, and callers filter it out of microphone picker
    /// lists. Defining it here lets tests that only need the name compile in
    /// the portable lane instead of dragging in `audio-capture` (and with it
    /// cidre/Xcode).
    pub const TAP_NAME: &str = "margins-tap";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LiveAudioChannel {
        Mic,
        System,
    }

    #[derive(Debug, Clone)]
    pub struct LiveAudioChunk {
        pub channel: LiveAudioChannel,
        pub generation: u64,
        pub session_offset_ms: u64,
        pub sample_rate: u32,
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
        pub queue_max_samples: u64,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct LiveGenerationClock {
        pub generation: u64,
        pub session_offset_ms: u64,
    }

    pub struct InputDevice;

    pub type SegmentId = u64;
    pub type CaptureToken = u64;
    pub const LANE_QUEUE_CAPACITY: usize = 128;
    pub const DRAIN_BATCH_MILLIS: u64 = 20;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CaptureLane {
        Mic,
        System,
    }

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

    #[derive(Clone)]
    pub struct CaptureSink;
    pub type MicSink = CaptureSink;
    pub type SystemSink = CaptureSink;

    impl CaptureSink {
        pub fn segment_id(&self) -> SegmentId {
            0
        }
        pub fn lane(&self) -> CaptureLane {
            CaptureLane::Mic
        }
        pub fn token(&self) -> CaptureToken {
            0
        }
        pub fn media_fence_nanos(&self) -> u64 {
            0
        }
        pub fn attach(&self, _source_rate: u32, _fence_nanos: u64) -> Result<()> {
            bail!("audio capture is not available in this build")
        }
        pub fn write(
            &self,
            _source_rate: u32,
            _samples: Vec<f32>,
            _packets: Vec<PacketDesc>,
        ) -> Result<()> {
            bail!("audio capture is not available in this build")
        }
        pub fn retire(&self) -> Result<RetireAck> {
            bail!("audio capture is not available in this build")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RetireAck {
        pub token: CaptureToken,
        pub boundary_frame: u64,
    }

    pub struct SegmentWriter;

    impl SegmentWriter {
        pub fn start(
            _segment_id: SegmentId,
            _mic_lane_rate: u32,
            _system_lane_rate: u32,
            _live_audio: Option<LiveAudioSink>,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }

        pub fn start_deferred(
            _segment_id: SegmentId,
            _live_audio: Option<LiveAudioSink>,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }

        pub fn telemetry(&self) -> SegmentWriterTelemetry {
            SegmentWriterTelemetry {
                timeline_reusable: Arc::new(AtomicBool::new(false)),
                writer_failed: Arc::new(AtomicBool::new(true)),
                mic: Arc::new(LaneTelemetry::default()),
                system: Arc::new(LaneTelemetry::default()),
            }
        }

        pub fn media_frame(&self) -> Result<u64> {
            Ok(0)
        }
        pub fn mic_lane_rate(&self) -> u32 {
            48_000
        }
        pub fn system_lane_rate(&self) -> u32 {
            48_000
        }
        pub fn set_unattached_lane_rate(&self, _lane: CaptureLane, _rate: u32) -> Result<()> {
            bail!("audio capture is not available in this build")
        }
        pub fn capture_sink(&self, _lane: CaptureLane, _token: CaptureToken) -> CaptureSink {
            CaptureSink
        }
        pub fn mic_sink(&self, _token: CaptureToken) -> MicSink {
            CaptureSink
        }
        pub fn system_sink(&self, _token: CaptureToken) -> SystemSink {
            CaptureSink
        }
        pub fn seal(self) -> Result<SealedSegment> {
            bail!("audio capture is not available in this build")
        }
        pub fn seal_at(self, _requested_media_frame: u64) -> Result<SealedSegment> {
            bail!("audio capture is not available in this build")
        }
    }

    pub struct SealedSegment {
        pub segment_id: SegmentId,
        pub output_rate: u32,
        pub shared_frame: u64,
        pub timeline_reusable: bool,
    }

    impl SealedSegment {
        pub fn write_wav(self, _path: impl AsRef<std::path::Path>) -> Result<f64> {
            bail!("audio capture is not available in this build")
        }
    }

    pub use margins_media::timeline::{target_frame, RationalResampler};

    #[derive(Clone)]
    pub struct MicCaptureTelemetry {
        pub peak: Arc<AtomicU32>,
        pub drops: Arc<AtomicU64>,
        pub packet_drops: Arc<AtomicU64>,
        pub frames: Arc<AtomicU64>,
        pub error: Arc<AtomicU8>,
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

    #[derive(Clone)]
    pub struct SystemCaptureTelemetry {
        pub peak: Arc<AtomicU32>,
        pub drops: Arc<AtomicU64>,
        pub packet_drops: Arc<AtomicU64>,
        pub silence: Arc<AtomicU64>,
        pub frames: Arc<AtomicU64>,
    }

    pub struct MicTarget<'a> {
        _device: Option<&'a InputDevice>,
        stable_uid: Option<String>,
    }
    impl<'a> MicTarget<'a> {
        pub fn system_default() -> Self {
            Self {
                _device: None,
                stable_uid: None,
            }
        }
        pub fn device(device: &'a InputDevice) -> Self {
            Self {
                _device: Some(device),
                stable_uid: None,
            }
        }
        pub fn from_optional(device: Option<&'a InputDevice>) -> Self {
            Self {
                _device: device,
                stable_uid: None,
            }
        }
        pub fn with_stable_uid(mut self, uid: impl Into<String>) -> Self {
            self.stable_uid = Some(uid.into());
            self
        }
        pub fn stable_uid(&self) -> Option<&str> {
            self.stable_uid.as_deref()
        }
    }

    pub struct MicCapture;
    #[derive(Debug, Clone, Copy)]
    pub struct RetiredMic {
        pub token: CaptureToken,
        pub boundary_frame: Option<u64>,
        pub native_rate: u32,
    }
    impl MicCapture {
        pub fn start(_target: &MicTarget<'_>, _sink: MicSink) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn start_with_error_flag(
            _target: &MicTarget<'_>,
            _sink: MicSink,
            _error: Arc<AtomicU8>,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn start_standby(_target: &MicTarget<'_>, _sink: MicSink) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn start_preferred(
            _target: &MicTarget<'_>,
            _sink: MicSink,
            _preferred: MicBackendKind,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn start_standby_preferred(
            _target: &MicTarget<'_>,
            _sink: MicSink,
            _preferred: MicBackendKind,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn native_rate(&self) -> u32 {
            0
        }
        pub fn token(&self) -> CaptureToken {
            0
        }
        pub fn telemetry(&self) -> MicCaptureTelemetry {
            MicCaptureTelemetry {
                peak: Arc::new(AtomicU32::new(0)),
                drops: Arc::new(AtomicU64::new(0)),
                packet_drops: Arc::new(AtomicU64::new(0)),
                frames: Arc::new(AtomicU64::new(0)),
                error: Arc::new(AtomicU8::new(0)),
            }
        }
        pub fn backend(&self) -> MicBackendKind {
            MicBackendKind::Cpal
        }
        pub fn take_error(&mut self, _detect_stall: bool) -> Option<MicStreamErrorKind> {
            None
        }
        pub fn first_packet_seen(&self) -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }
        pub fn commit_standby(&self) -> Result<()> {
            bail!("audio capture is not available in this build")
        }
        pub fn retire(self) -> Result<RetiredMic> {
            bail!("audio capture is not available in this build")
        }
    }

    pub struct SystemCapture;
    #[derive(Debug, Clone, Copy)]
    pub struct RetiredSystem {
        pub token: CaptureToken,
        pub boundary_frame: Option<u64>,
        pub native_rate: u32,
    }
    impl SystemCapture {
        pub fn start(_sink: SystemSink) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn start_standby(_sink: SystemSink) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
        pub fn native_rate(&self) -> u32 {
            0
        }
        pub fn token(&self) -> CaptureToken {
            0
        }
        pub fn telemetry(&self) -> SystemCaptureTelemetry {
            SystemCaptureTelemetry {
                peak: Arc::new(AtomicU32::new(0)),
                drops: Arc::new(AtomicU64::new(0)),
                packet_drops: Arc::new(AtomicU64::new(0)),
                silence: Arc::new(AtomicU64::new(0)),
                frames: Arc::new(AtomicU64::new(0)),
            }
        }
        pub fn first_packet_seen(&self) -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }
        pub fn commit_standby(&self) -> Result<()> {
            bail!("audio capture is not available in this build")
        }
        pub fn retire(self) -> Result<RetiredSystem> {
            bail!("audio capture is not available in this build")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MicStreamErrorKind {
        DeviceNotAvailable,
        BackendSpecific,
        Stalled,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MicrophoneAuthorization {
        NotDetermined,
        Restricted,
        Denied,
        Authorized,
    }

    pub fn microphone_authorization() -> Result<MicrophoneAuthorization> {
        Ok(MicrophoneAuthorization::Authorized)
    }

    pub fn input_device_uid_at(device_name: &str, _occurrence: usize) -> Option<String> {
        Some(device_name.to_string())
    }

    pub fn take_mic_stream_error(_flag: &AtomicU8) -> Option<MicStreamErrorKind> {
        None
    }

    #[cfg(target_os = "macos")]
    pub struct InputDeviceWatcher;

    #[cfg(target_os = "macos")]
    impl InputDeviceWatcher {
        pub fn start(_sender: SyncSender<()>) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }
    }

    pub struct RecorderHandle {
        mic_peak: Arc<AtomicU32>,
        spk_peak: Arc<AtomicU32>,
        mic_drops: Arc<AtomicU64>,
        spk_drops: Arc<AtomicU64>,
        spk_silence: Arc<AtomicU64>,
        spk_frames: Arc<AtomicU64>,
    }

    impl RecorderHandle {
        pub fn start(_stop_flag: Arc<AtomicBool>, _device: Option<&InputDevice>) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }

        pub fn start_with_live_audio(
            _stop_flag: Arc<AtomicBool>,
            _device: Option<&InputDevice>,
            _live_audio: Option<LiveAudioSink>,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }

        pub fn start_with_live_audio_and_mic_error(
            _stop_flag: Arc<AtomicBool>,
            _device: Option<&InputDevice>,
            _live_audio: Option<LiveAudioSink>,
            _mic_error: Arc<AtomicU8>,
        ) -> Result<Self> {
            bail!("audio capture is not available in this build")
        }

        pub fn mic_peak(&self) -> Arc<AtomicU32> {
            self.mic_peak.clone()
        }

        pub fn spk_peak(&self) -> Arc<AtomicU32> {
            self.spk_peak.clone()
        }

        pub fn mic_drops(&self) -> Arc<AtomicU64> {
            self.mic_drops.clone()
        }

        pub fn spk_drops(&self) -> Arc<AtomicU64> {
            self.spk_drops.clone()
        }

        pub fn spk_silence(&self) -> Arc<AtomicU64> {
            self.spk_silence.clone()
        }

        pub fn spk_frames(&self) -> Arc<AtomicU64> {
            self.spk_frames.clone()
        }

        pub fn spk_rate(&self) -> u32 {
            48_000
        }

        pub fn stop_and_write(self, _path: &str) -> Result<f64> {
            bail!("audio capture is not available in this build")
        }
    }

    pub fn list_input_devices() -> Vec<(String, InputDevice)> {
        Vec::new()
    }

    pub fn default_input_device_name() -> Option<String> {
        None
    }

    pub fn test_input_device_level(
        _device: Option<&InputDevice>,
        _duration: Duration,
    ) -> Result<(f32, u64)> {
        bail!("audio capture is not available in this build")
    }

    #[derive(Debug, Clone, Copy)]
    pub struct SystemAudioTapProbe {
        pub peak: f32,
        pub drop_count: u64,
        pub silent_secs: f64,
        pub frame_count: u64,
        pub test_tone_played: bool,
    }

    pub fn test_system_audio_tap_level(_duration: Duration) -> Result<SystemAudioTapProbe> {
        bail!("system audio capture is not available in this build")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn mic_target_carries_optional_stable_uid() {
            let target = MicTarget::system_default().with_stable_uid("BuiltInMicrophoneDevice");
            assert_eq!(target.stable_uid(), Some("BuiltInMicrophoneDevice"));
            assert!(target._device.is_none());
            assert!(MicTarget::system_default().stable_uid().is_none());
        }
    }
}
pub mod session;
pub mod session_index;
pub mod text_helpers;
pub mod tui;

// The segment writer is platform-independent even though production reaches it
// through the native recorder module. Compile its real implementation in the
// portable test lane so timeline regressions do not require ALSA/CoreAudio.
#[cfg(all(test, not(feature = "audio-capture")))]
mod portable_segment_writer_tests {
    use crate::recorder::{LiveAudioChannel, LiveAudioChunk, LiveAudioSink};

    mod implementation {
        include!("recorder/segment_writer.rs");
    }
}

#[cfg(test)]
mod media_compatibility_tests {
    #[test]
    fn root_recorder_path_reexports_portable_timeline_helpers() {
        assert_eq!(
            crate::recorder::target_frame(1_000_000_000, 48_000).unwrap(),
            48_000
        );

        let mut resampler = crate::recorder::RationalResampler::new(2, 1).unwrap();
        let mut output = resampler.process(&[0.0, 1.0]).unwrap();
        output.extend(resampler.finish().unwrap());
        assert_eq!(output, vec![1.0]);
    }

    #[test]
    fn root_media_facades_preserve_legacy_type_and_function_paths() {
        let words = vec![crate::asr::WordTiming {
            start_ms: 10,
            end_ms: 20,
            text: "hello".into(),
        }];
        let entries = crate::asr::words_to_transcript_entries(&words, 1, 5);
        assert_eq!(entries[0].start_ms, 15);

        let audio = crate::audio_pipeline::AudioBuffer {
            samples: vec![0.25, 0.75],
            sample_rate: 16_000,
            channels: 2,
        };
        assert_eq!(
            crate::audio_pipeline::downmix_to_mono(&audio).unwrap(),
            vec![0.5]
        );
    }
}
