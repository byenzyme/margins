use matroska_demuxer::{Frame, MatroskaFile, TrackType};
use ropus::{Channels, DecodeMode, Decoder};
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const OPUS_SAMPLE_RATE: u32 = 48_000;
const TARGET_SAMPLE_RATE: u32 = 16_000;
const TARGET_CHANNELS: u16 = 1;

// matroska-demuxer fills Frame::data before this module can inspect a packet,
// so the whole-file cap is also the practical pre-packet allocation ceiling.
const MAX_WEBM_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PACKET_BYTES: usize = 256 * 1024;
const MAX_CHANNELS: u64 = 2;
const MAX_DURATION_SECS: u64 = 8 * 60 * 60;
const MAX_OUTPUT_FRAMES: usize = TARGET_SAMPLE_RATE as usize * MAX_DURATION_SECS as usize;
const MAX_TIMESTAMP_GAP_SECS: u64 = 5 * 60;
const MAX_TRACKS: usize = 16;
const MAX_CODEC_PRIVATE_BYTES: usize = 1024;
const STREAM_CHUNK_FRAMES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostedWebmFinalizer {
    Native,
    FfmpegCompatibility,
}

impl HostedWebmFinalizer {
    pub(crate) fn from_env() -> Self {
        match std::env::var("MARGINS_HOSTED_WEBM_FINALIZER") {
            Ok(value) if value.eq_ignore_ascii_case("ffmpeg") => Self::FfmpegCompatibility,
            _ => Self::Native,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::FfmpegCompatibility => "ffmpeg",
        }
    }
}

#[derive(Debug)]
struct OpusHead {
    channels: u8,
    pre_skip: usize,
    output_gain_q8: i16,
}

#[derive(Debug, Default, Clone, Copy)]
struct FinalizeStats {
    written_frames: usize,
    max_decode_frames: usize,
    max_write_chunk_frames: usize,
    packet_count: usize,
}

pub(crate) fn finalize_webm_opus_to_wav(webm: &Path, wav: &Path) -> Result<(), String> {
    finalize_webm_opus_to_wav_with_stats(webm, wav).map(|_| ())
}

fn finalize_webm_opus_to_wav_with_stats(webm: &Path, wav: &Path) -> Result<FinalizeStats, String> {
    let metadata = std::fs::metadata(webm)
        .map_err(|error| format!("failed to stat WebM {}: {error}", webm.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "WebM input is not a regular file: {}",
            webm.display()
        ));
    }
    if metadata.len() == 0 {
        return Err("WebM input is empty; no durable browser audio was received".to_string());
    }
    if metadata.len() > MAX_WEBM_BYTES {
        return Err(format!(
            "WebM input is too large for the native finalizer: {} bytes exceeds {} byte limit",
            metadata.len(),
            MAX_WEBM_BYTES
        ));
    }

    let file = File::open(webm)
        .map_err(|error| format!("failed to open WebM {}: {error}", webm.display()))?;
    let mut webm =
        MatroskaFile::open(file).map_err(|error| format!("failed to parse WebM: {error}"))?;
    let timestamp_scale_ns = webm.info().timestamp_scale().get();
    if webm.tracks().len() > MAX_TRACKS {
        return Err(format!(
            "WebM has too many tracks: {} exceeds {} track limit",
            webm.tracks().len(),
            MAX_TRACKS
        ));
    }

    let (track_number, head, codec_delay_ns) = {
        let track = webm
            .tracks()
            .iter()
            .find(|track| track.track_type() == TrackType::Audio && track.codec_id() == "A_OPUS")
            .ok_or_else(|| "WebM does not contain an Opus audio track".to_string())?;
        if !track.flag_enabled() {
            return Err("WebM Opus audio track is disabled".to_string());
        }
        if track.content_encodings().is_some() {
            return Err("WebM Opus audio track uses unsupported content encodings".to_string());
        }
        let audio = track
            .audio()
            .ok_or_else(|| "WebM Opus track is missing audio metadata".to_string())?;
        let declared_channels = audio.channels().get();
        if !(1..=MAX_CHANNELS).contains(&declared_channels) {
            return Err(format!(
                "WebM Opus channel count {declared_channels} is unsupported"
            ));
        }
        let private = track
            .codec_private()
            .ok_or_else(|| "WebM Opus track is missing OpusHead codec private data".to_string())?;
        if private.len() > MAX_CODEC_PRIVATE_BYTES {
            return Err(format!(
                "WebM Opus codec private data is too large: {} bytes",
                private.len()
            ));
        }
        let head = parse_opus_head(private)?;
        if u64::from(head.channels) != declared_channels {
            return Err(format!(
                "WebM Opus channel mismatch: OpusHead has {}, track metadata has {declared_channels}",
                head.channels
            ));
        }
        (track.track_number().get(), head, track.codec_delay())
    };

    let channels = match head.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        _ => {
            return Err(format!(
                "WebM Opus channel count {} is unsupported",
                head.channels
            ))
        }
    };
    let channel_count = channels.count();
    let mut decoder = Decoder::new(TARGET_SAMPLE_RATE, channels)
        .map_err(|error| format!("failed to initialize Opus decoder: {error}"))?;
    if head.output_gain_q8 != 0 {
        decoder
            .set_gain(i32::from(head.output_gain_q8))
            .map_err(|error| format!("failed to apply Opus output gain: {error}"))?;
    }

    let mut frame = Frame::default();
    let mut packet_count = 0usize;
    let mut last_end_frame = 0usize;
    let mut remaining_pre_skip = pre_skip_target_frames(&head, codec_delay_ns)?;
    let mut decode_buf = vec![0.0f32; decoder.max_frame_samples_per_channel() * channel_count];
    let mut mono_buf = Vec::with_capacity(decoder.max_frame_samples_per_channel());
    let mut sink = StreamingWavSink::create(wav)?;
    let mut stats = FinalizeStats::default();

    while webm
        .next_frame(&mut frame)
        .map_err(|error| format!("failed to read WebM audio packet: {error}"))?
    {
        if frame.track != track_number {
            continue;
        }
        packet_count += 1;
        if frame.data.is_empty() {
            continue;
        }
        if frame.data.len() > MAX_PACKET_BYTES {
            return Err(format!(
                "WebM Opus packet is too large: {} bytes exceeds {} byte limit",
                frame.data.len(),
                MAX_PACKET_BYTES
            ));
        }

        let timestamp_frame = timestamp_to_target_frames(frame.timestamp, timestamp_scale_ns)?;
        if timestamp_frame > last_end_frame {
            let gap_frames = timestamp_frame - last_end_frame;
            if gap_frames > TARGET_SAMPLE_RATE as usize * MAX_TIMESTAMP_GAP_SECS as usize {
                return Err(format!(
                    "WebM timestamp gap is too large: {} frames at 16 kHz",
                    gap_frames
                ));
            }
            check_output_frame_limit(sink.written_frames(), gap_frames)?;
            sink.write_silence(gap_frames)?;
            last_end_frame = timestamp_frame;
        }

        let frames = decoder
            .decode_float(&frame.data, &mut decode_buf, DecodeMode::Normal)
            .map_err(|error| format!("failed to decode Opus packet {packet_count}: {error}"))?;
        stats.max_decode_frames = stats.max_decode_frames.max(frames);
        let produced = frames
            .checked_mul(channel_count)
            .ok_or_else(|| "decoded Opus sample count overflowed".to_string())?;
        let mut start_frame = 0usize;
        if remaining_pre_skip > 0 {
            start_frame = remaining_pre_skip.min(frames);
            remaining_pre_skip -= start_frame;
        }
        if start_frame < frames {
            write_packet_mono(
                &mut sink,
                &decode_buf[start_frame * channel_count..produced],
                frames - start_frame,
                channel_count,
                &mut mono_buf,
            )?;
        }
        last_end_frame = last_end_frame
            .checked_add(frames)
            .ok_or_else(|| "decoded Opus duration overflowed".to_string())?;
        if last_end_frame > MAX_OUTPUT_FRAMES {
            return Err(format!(
                "decoded WebM duration exceeds {} seconds",
                MAX_DURATION_SECS
            ));
        }
    }

    if packet_count == 0 {
        return Err("WebM Opus track contains no audio packets".to_string());
    }
    if sink.written_frames() == 0 {
        return Err("WebM Opus decoded no audible samples after pre-skip".to_string());
    }

    stats.written_frames = sink.written_frames();
    stats.max_write_chunk_frames = sink.max_write_chunk_frames();
    stats.packet_count = packet_count;
    sink.finish()?;
    Ok(stats)
}

fn parse_opus_head(data: &[u8]) -> Result<OpusHead, String> {
    if data.len() < 19 {
        return Err(format!(
            "OpusHead is too short: {} bytes; expected at least 19",
            data.len()
        ));
    }
    if &data[..8] != b"OpusHead" {
        return Err("Opus codec private data does not start with OpusHead".to_string());
    }
    if data[8] != 1 {
        return Err(format!("unsupported OpusHead version {}", data[8]));
    }
    let channels = data[9];
    if !(1..=2).contains(&channels) {
        return Err(format!("unsupported OpusHead channel count {channels}"));
    }
    let pre_skip = u16::from_le_bytes([data[10], data[11]]) as usize;
    let output_gain_q8 = i16::from_le_bytes([data[16], data[17]]);
    Ok(OpusHead {
        channels,
        pre_skip,
        output_gain_q8,
    })
}

fn pre_skip_target_frames(head: &OpusHead, codec_delay_ns: Option<u64>) -> Result<usize, String> {
    let pre_skip = frames_48k_to_target(head.pre_skip)?;
    if let Some(delay_ns) = codec_delay_ns {
        let delay_samples = ns_to_target_frames(delay_ns)?;
        return Ok(pre_skip.max(delay_samples));
    }
    Ok(pre_skip)
}

fn timestamp_to_target_frames(timestamp: u64, timestamp_scale_ns: u64) -> Result<usize, String> {
    let samples =
        (u128::from(timestamp) * u128::from(timestamp_scale_ns) * u128::from(TARGET_SAMPLE_RATE)
            + 500_000_000)
            / 1_000_000_000;
    usize::try_from(samples)
        .map_err(|_| "WebM timestamp does not fit in duration limits".to_string())
}

fn ns_to_target_frames(ns: u64) -> Result<usize, String> {
    let samples = (u128::from(ns) * u128::from(TARGET_SAMPLE_RATE) + 500_000_000) / 1_000_000_000;
    usize::try_from(samples)
        .map_err(|_| "WebM codec delay does not fit in duration limits".to_string())
}

fn frames_48k_to_target(frames: usize) -> Result<usize, String> {
    let target = (u128::try_from(frames)
        .map_err(|_| "Opus pre-skip does not fit in duration limits".to_string())?
        * u128::from(TARGET_SAMPLE_RATE)
        + u128::from(OPUS_SAMPLE_RATE / 2))
        / u128::from(OPUS_SAMPLE_RATE);
    usize::try_from(target).map_err(|_| "Opus pre-skip does not fit in duration limits".to_string())
}

fn write_packet_mono(
    sink: &mut StreamingWavSink,
    samples: &[f32],
    frames: usize,
    channels: usize,
    mono_buf: &mut Vec<f32>,
) -> Result<(), String> {
    check_output_frame_limit(sink.written_frames(), frames)?;
    match channels {
        1 => sink.write_samples(samples),
        2 => {
            mono_buf.clear();
            mono_buf.reserve(frames);
            for frame in samples.chunks_exact(2) {
                mono_buf.push((frame[0] + frame[1]) * 0.5);
            }
            sink.write_samples(mono_buf)
        }
        _ => Err(format!("WebM Opus channel count {channels} is unsupported")),
    }
}

fn check_output_frame_limit(current_frames: usize, additional_frames: usize) -> Result<(), String> {
    let total = current_frames
        .checked_add(additional_frames)
        .ok_or_else(|| "decoded WebM duration overflowed".to_string())?;
    if total > MAX_OUTPUT_FRAMES {
        return Err(format!(
            "decoded WebM duration exceeds {} seconds",
            MAX_DURATION_SECS
        ));
    }
    Ok(())
}

struct StreamingWavSink {
    temp_path: PathBuf,
    final_path: PathBuf,
    sync_file: File,
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    written_frames: usize,
    max_write_chunk_frames: usize,
    renamed: bool,
}

impl StreamingWavSink {
    fn create(final_path: &Path) -> Result<Self, String> {
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create native finalized WAV directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let temp_path = temp_wav_path(final_path);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| {
                format!(
                    "failed to create temporary native finalized WAV {}: {error}",
                    temp_path.display()
                )
            })?;
        let sync_file = file
            .try_clone()
            .map_err(|error| format!("failed to clone temporary WAV handle: {error}"))?;
        let writer = hound::WavWriter::new(
            BufWriter::new(file),
            hound::WavSpec {
                channels: TARGET_CHANNELS,
                sample_rate: TARGET_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|error| format!("failed to initialize native finalized WAV: {error}"))?;

        Ok(Self {
            temp_path,
            final_path: final_path.to_path_buf(),
            sync_file,
            writer: Some(writer),
            written_frames: 0,
            max_write_chunk_frames: 0,
            renamed: false,
        })
    }

    fn written_frames(&self) -> usize {
        self.written_frames
    }

    fn max_write_chunk_frames(&self) -> usize {
        self.max_write_chunk_frames
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<(), String> {
        check_output_frame_limit(self.written_frames, samples.len())?;
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| "native finalized WAV writer is already closed".to_string())?;
        for sample in samples {
            writer
                .write_sample(sample.clamp(-1.0, 1.0))
                .map_err(|error| format!("failed to write native finalized WAV sample: {error}"))?;
        }
        self.written_frames += samples.len();
        self.max_write_chunk_frames = self.max_write_chunk_frames.max(samples.len());
        Ok(())
    }

    fn write_silence(&mut self, frames: usize) -> Result<(), String> {
        static SILENCE: [f32; STREAM_CHUNK_FRAMES] = [0.0; STREAM_CHUNK_FRAMES];
        let mut remaining = frames;
        while remaining > 0 {
            let chunk = remaining.min(STREAM_CHUNK_FRAMES);
            self.write_samples(&SILENCE[..chunk])?;
            remaining -= chunk;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| "native finalized WAV writer is already closed".to_string())?;
        writer
            .finalize()
            .map_err(|error| format!("failed to finalize native WAV header: {error}"))?;
        self.sync_file.sync_all().map_err(|error| {
            format!(
                "failed to sync temporary native finalized WAV {}: {error}",
                self.temp_path.display()
            )
        })?;
        std::fs::rename(&self.temp_path, &self.final_path).map_err(|error| {
            format!(
                "failed to atomically publish native finalized WAV {}: {error}",
                self.final_path.display()
            )
        })?;
        self.renamed = true;
        let _ = sync_parent_directory(&self.final_path);
        Ok(())
    }
}

impl Drop for StreamingWavSink {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

fn temp_wav_path(final_path: &Path) -> PathBuf {
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output.wav");
    final_path.with_file_name(format!(
        ".{file_name}.native-webm-opus.{}.{}.tmp.wav",
        std::process::id(),
        counter
    ))
}

fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropus::{Application, Encoder};

    const AUDIO_TRACK: u64 = 1;
    const TIMESTAMP_SCALE_NS: u64 = 1_000_000;

    #[test]
    fn parses_opus_head_delay_metadata() {
        let head = parse_opus_head(&opus_head(2, 312)).unwrap();
        assert_eq!(head.channels, 2);
        assert_eq!(head.pre_skip, 312);
        assert_eq!(pre_skip_target_frames(&head, Some(6_500_000)).unwrap(), 104);
    }

    #[test]
    fn rejects_malformed_opus_head_without_panicking() {
        let err = parse_opus_head(b"Opus").unwrap_err();
        assert!(err.contains("too short"));

        let mut wrong = opus_head(1, 312);
        wrong[9] = 9;
        let err = parse_opus_head(&wrong).unwrap_err();
        assert!(err.contains("channel"));
    }

    #[test]
    fn finalizer_env_is_native_unless_ffmpeg_is_explicitly_opted_in() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::restore_after();

        std::env::remove_var("MARGINS_HOSTED_WEBM_FINALIZER");
        assert_eq!(HostedWebmFinalizer::from_env(), HostedWebmFinalizer::Native);

        std::env::set_var("MARGINS_HOSTED_WEBM_FINALIZER", "native");
        assert_eq!(HostedWebmFinalizer::from_env(), HostedWebmFinalizer::Native);

        std::env::set_var("MARGINS_HOSTED_WEBM_FINALIZER", "ffmpeg");
        assert_eq!(
            HostedWebmFinalizer::from_env(),
            HostedWebmFinalizer::FfmpegCompatibility
        );
    }

    #[test]
    fn finalizes_chrome_style_webm_without_ffmpeg() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::set_invalid_ffmpeg();

        let fixture = make_chrome_unknown_size_webm_fixture(20, 20, 1, true);
        assert_contains_one_unknown_size_segment(&fixture.webm);
        assert_contains_at_least_unknown_size_clusters(&fixture.webm, 2);
        let paths = write_fixture("chrome-style", &fixture.webm);
        let stats = finalize_webm_opus_to_wav_with_stats(&paths.webm, &paths.wav).unwrap();

        let wav = margins::audio_pipeline::load_wav(&paths.wav).unwrap();
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.channels, 1);
        assert!((wav.frame_count() as i64 - fixture.expected_16k_frames as i64).abs() <= 1);
        assert!(wav.samples.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(stats.packet_count, 20);
        assert_eq!(stats.written_frames, wav.frame_count());
        assert!(stats.max_decode_frames <= 320);
        assert!(stats.max_write_chunk_frames <= STREAM_CHUNK_FRAMES);
        assert_no_native_temp_wavs(paths.wav.parent().unwrap());
    }

    #[test]
    fn finalizes_timesliced_server_storage_after_chunked_writes() {
        let fixture = make_webm_fixture(12, 20, 1, true);
        let dir = tempfile::Builder::new()
            .prefix("margins-webm-opus-timesliced-")
            .tempdir()
            .unwrap();
        let webm = dir.path().join("input.webm");
        let wav = dir.path().join("output.wav");
        let mut file = std::fs::File::create(&webm).unwrap();
        for chunk in fixture.webm.chunks(173) {
            use std::io::Write as _;
            file.write_all(chunk).unwrap();
            file.sync_data().unwrap();
        }
        drop(file);

        finalize_webm_opus_to_wav(&webm, &wav).unwrap();
        let wav = margins::audio_pipeline::load_wav(&wav).unwrap();
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.channels, 1);
        assert!((wav.frame_count() as i64 - fixture.expected_16k_frames as i64).abs() <= 1);
    }

    #[test]
    fn finalizes_short_webm_capture() {
        let fixture = make_webm_fixture(2, 20, 1, true);
        let paths = write_fixture("short", &fixture.webm);
        finalize_webm_opus_to_wav(&paths.webm, &paths.wav).unwrap();

        let wav = margins::audio_pipeline::load_wav(&paths.wav).unwrap();
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.channels, 1);
        assert!(wav.frame_count() > 0);
        assert!(wav.frame_count() < 1_000);
    }

    #[test]
    fn preserves_timestamp_gap_as_silence() {
        let fixture = make_webm_with_timestamps(&[0, 1_000], 1, true);
        let paths = write_fixture("gap", &fixture.webm);
        finalize_webm_opus_to_wav(&paths.webm, &paths.wav).unwrap();

        let wav = margins::audio_pipeline::load_wav(&paths.wav).unwrap();
        assert!((wav.frame_count() as i64 - fixture.expected_16k_frames as i64).abs() <= 1);
        let silent_start = 400usize;
        let silent_end = 15_000usize.min(wav.samples.len());
        assert!(wav.samples[silent_start..silent_end]
            .iter()
            .all(|sample| sample.abs() < 0.01));
    }

    #[test]
    fn long_gapped_webm_streams_bounded_chunks() {
        let fixture = make_webm_with_timestamps(&[0, 61_000], 1, true);
        let paths = write_fixture("long-gap", &fixture.webm);
        let stats = finalize_webm_opus_to_wav_with_stats(&paths.webm, &paths.wav).unwrap();

        assert_eq!(stats.packet_count, 2);
        assert!(stats.written_frames > 900_000);
        assert!(stats.max_decode_frames <= 320);
        assert!(stats.max_write_chunk_frames <= STREAM_CHUNK_FRAMES);
        assert!(
            (wav_duration_frames(&paths.wav) as i64 - fixture.expected_16k_frames as i64).abs()
                <= 1
        );
    }

    #[test]
    fn oversized_packet_removes_temp_and_preserves_existing_wav() {
        let packet = vec![0u8; MAX_PACKET_BYTES + 1];
        let fixture = write_webm(1, 0, &[0], &[packet], WebmFixtureLayout::KnownSizes);
        let paths = write_fixture("oversized-packet", &fixture);
        let previous = b"previous finalized wav";
        std::fs::write(&paths.wav, previous).unwrap();

        let error = finalize_webm_opus_to_wav(&paths.webm, &paths.wav).unwrap_err();
        assert!(error.contains("packet is too large"), "{error}");
        assert_eq!(std::fs::read(&paths.wav).unwrap(), previous);
        assert_no_native_temp_wavs(paths.wav.parent().unwrap());
    }

    #[test]
    fn malformed_and_empty_inputs_return_actionable_errors() {
        let truncated = write_fixture("truncated", b"\x1a\x45\xdf\xa3\x84webm");
        let error = finalize_webm_opus_to_wav(&truncated.webm, &truncated.wav).unwrap_err();
        assert!(
            error.contains("parse WebM") || error.contains("read WebM"),
            "{error}"
        );

        let empty = write_fixture("empty", b"");
        let error = finalize_webm_opus_to_wav(&empty.webm, &empty.wav).unwrap_err();
        assert!(error.contains("empty"), "{error}");
    }

    #[test]
    fn unsupported_codec_private_is_bounded_error() {
        let mut fixture = make_webm_fixture(4, 20, 1, true);
        let needle = b"OpusHead";
        let pos = fixture
            .webm
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();
        fixture.webm[pos..pos + needle.len()].copy_from_slice(b"NotOpus!");
        let paths = write_fixture("bad-opus-head", &fixture.webm);
        let error = finalize_webm_opus_to_wav(&paths.webm, &paths.wav).unwrap_err();
        assert!(error.contains("OpusHead"), "{error}");
    }

    fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"OpusHead");
        head.push(1);
        head.push(channels);
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        head
    }

    struct Fixture {
        webm: Vec<u8>,
        expected_16k_frames: usize,
    }

    struct FixturePaths {
        _dir: tempfile::TempDir,
        webm: std::path::PathBuf,
        wav: std::path::PathBuf,
    }

    fn make_webm_fixture(
        packets: usize,
        packet_ms: u64,
        channels: u8,
        include_pre_skip: bool,
    ) -> Fixture {
        let timestamps: Vec<u64> = (0..packets)
            .map(|packet| packet as u64 * packet_ms)
            .collect();
        make_webm_with_timestamps(&timestamps, channels, include_pre_skip)
    }

    fn make_chrome_unknown_size_webm_fixture(
        packets: usize,
        packet_ms: u64,
        channels: u8,
        include_pre_skip: bool,
    ) -> Fixture {
        let timestamps: Vec<u64> = (0..packets)
            .map(|packet| packet as u64 * packet_ms)
            .collect();
        make_webm_with_layout(
            &timestamps,
            channels,
            include_pre_skip,
            WebmFixtureLayout::ChromeUnknownSizes,
        )
    }

    fn make_webm_with_timestamps(
        timestamps_ms: &[u64],
        channels: u8,
        include_pre_skip: bool,
    ) -> Fixture {
        make_webm_with_layout(
            timestamps_ms,
            channels,
            include_pre_skip,
            WebmFixtureLayout::KnownSizes,
        )
    }

    fn make_webm_with_layout(
        timestamps_ms: &[u64],
        channels: u8,
        include_pre_skip: bool,
        layout: WebmFixtureLayout,
    ) -> Fixture {
        let mut encoder = Encoder::builder(
            OPUS_SAMPLE_RATE,
            if channels == 1 {
                Channels::Mono
            } else {
                Channels::Stereo
            },
            Application::Audio,
        )
        .build()
        .unwrap();
        let pre_skip = if include_pre_skip {
            encoder.lookahead() as u16
        } else {
            0
        };
        let channels_usize = channels as usize;
        let frame_samples = 960usize;
        let mut encoded = Vec::with_capacity(timestamps_ms.len());
        for packet_idx in 0..timestamps_ms.len() {
            let mut pcm = Vec::with_capacity(frame_samples * channels_usize);
            for frame in 0..frame_samples {
                let t = (packet_idx * frame_samples + frame) as f32 / OPUS_SAMPLE_RATE as f32;
                let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.25;
                for _ in 0..channels_usize {
                    pcm.push(sample);
                }
            }
            let mut packet = [0u8; 4000];
            let len = encoder.encode_float(&pcm, &mut packet).unwrap();
            encoded.push(packet[..len].to_vec());
        }
        let last_timestamp = timestamps_ms.last().copied().unwrap_or(0);
        let total_48k = if timestamps_ms.is_empty() {
            0
        } else {
            (last_timestamp as usize * 48) + frame_samples - pre_skip as usize
        };
        let expected_16k_frames = ((total_48k as f64) / 3.0).round() as usize;

        Fixture {
            webm: write_webm(channels, pre_skip, timestamps_ms, &encoded, layout),
            expected_16k_frames,
        }
    }

    fn write_fixture(name: &str, bytes: &[u8]) -> FixturePaths {
        let dir = tempfile::Builder::new()
            .prefix(&format!("margins-webm-opus-{name}-"))
            .tempdir()
            .unwrap();
        let webm = dir.path().join("input.webm");
        let wav = dir.path().join("output.wav");
        std::fs::write(&webm, bytes).unwrap();
        FixturePaths {
            _dir: dir,
            webm,
            wav,
        }
    }

    fn wav_duration_frames(path: &Path) -> usize {
        hound::WavReader::open(path).unwrap().duration() as usize
    }

    fn assert_no_native_temp_wavs(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(
                !name.contains(".native-webm-opus.") && !name.ends_with(".tmp.wav"),
                "unexpected temporary WAV remains at {}",
                path.display()
            );
        }
    }

    #[derive(Clone, Copy)]
    enum WebmFixtureLayout {
        KnownSizes,
        ChromeUnknownSizes,
    }

    fn write_webm(
        channels: u8,
        pre_skip: u16,
        timestamps_ms: &[u64],
        packets: &[Vec<u8>],
        layout: WebmFixtureLayout,
    ) -> Vec<u8> {
        let ebml = element(
            0x1A45DFA3,
            [
                element_uint(0x4286, 1),
                element_uint(0x42F7, 1),
                element_uint(0x42F2, 4),
                element_uint(0x42F3, 8),
                element_string(0x4282, "webm"),
                element_uint(0x4287, 4),
                element_uint(0x4285, 2),
            ]
            .concat(),
        );
        let info = element(
            0x1549A966,
            [
                element_uint(0x2AD7B1, TIMESTAMP_SCALE_NS),
                element_string(0x4D80, "margins-test"),
                element_string(0x5741, "margins-test"),
            ]
            .concat(),
        );
        let track = element(
            0xAE,
            [
                element_uint(0xD7, AUDIO_TRACK),
                element_uint(0x73C5, 1),
                element_uint(0x83, 2),
                element_string(0x86, "A_OPUS"),
                element_binary(0x63A2, &opus_head(channels, pre_skip)),
                element_uint(0x56AA, u64::from(pre_skip) * 1_000_000_000 / 48_000),
                element_uint(0x56BB, 80_000_000),
                element(
                    0xE1,
                    [
                        element_float(0xB5, 48_000.0),
                        element_uint(0x9F, u64::from(channels)),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
        );
        let tracks = element(0x1654AE6B, track);
        let mut clusters = Vec::new();
        for (timestamp, packet) in timestamps_ms.iter().zip(packets) {
            let mut block = Vec::new();
            block.push(0x80 | AUDIO_TRACK as u8);
            block.extend_from_slice(&0i16.to_be_bytes());
            block.push(0x80);
            block.extend_from_slice(packet);
            let cluster_body =
                [element_uint(0xE7, *timestamp), element_binary(0xA3, &block)].concat();
            clusters.extend(match layout {
                WebmFixtureLayout::KnownSizes => element(0x1F43B675, cluster_body),
                WebmFixtureLayout::ChromeUnknownSizes => {
                    element_unknown_size_8(0x1F43B675, cluster_body)
                }
            });
        }
        let segment_body = [info, tracks, clusters].concat();
        let segment = match layout {
            WebmFixtureLayout::KnownSizes => element(0x18538067, segment_body),
            WebmFixtureLayout::ChromeUnknownSizes => {
                element_unknown_size_8(0x18538067, segment_body)
            }
        };
        [ebml, segment].concat()
    }

    fn assert_contains_one_unknown_size_segment(webm: &[u8]) {
        assert_eq!(count_unknown_size_elements(webm, 0x18538067), 1);
    }

    fn assert_contains_at_least_unknown_size_clusters(webm: &[u8], expected_minimum: usize) {
        assert!(count_unknown_size_elements(webm, 0x1F43B675) >= expected_minimum);
    }

    fn count_unknown_size_elements(webm: &[u8], id: u64) -> usize {
        let pattern = [id_bytes(id), unknown_size_8()].concat();
        webm.windows(pattern.len())
            .filter(|window| *window == pattern.as_slice())
            .count()
    }

    fn element_unknown_size_8(id: u64, body: Vec<u8>) -> Vec<u8> {
        [id_bytes(id), unknown_size_8(), body].concat()
    }

    fn unknown_size_8() -> Vec<u8> {
        vec![0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
    }

    fn element(id: u64, body: Vec<u8>) -> Vec<u8> {
        [id_bytes(id), size_bytes(body.len() as u64), body].concat()
    }

    fn element_uint(id: u64, value: u64) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len() - 1);
        element(id, bytes[first..].to_vec())
    }

    fn element_float(id: u64, value: f64) -> Vec<u8> {
        element(id, value.to_be_bytes().to_vec())
    }

    fn element_string(id: u64, value: &str) -> Vec<u8> {
        element(id, value.as_bytes().to_vec())
    }

    fn element_binary(id: u64, value: &[u8]) -> Vec<u8> {
        element(id, value.to_vec())
    }

    fn id_bytes(id: u64) -> Vec<u8> {
        let bytes = id.to_be_bytes();
        bytes
            .iter()
            .skip_while(|byte| **byte == 0)
            .copied()
            .collect()
    }

    fn size_bytes(size: u64) -> Vec<u8> {
        if size < 0x7f {
            vec![0x80 | size as u8]
        } else if size < 0x3fff {
            vec![0x40 | ((size >> 8) as u8), size as u8]
        } else if size < 0x1f_ffff {
            vec![0x20 | ((size >> 16) as u8), (size >> 8) as u8, size as u8]
        } else if size < 0x0fff_ffff {
            vec![
                0x10 | ((size >> 24) as u8),
                (size >> 16) as u8,
                (size >> 8) as u8,
                size as u8,
            ]
        } else {
            panic!("test EBML element too large");
        }
    }

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    struct EnvGuard {
        ffmpeg_bin: Option<std::ffi::OsString>,
        finalizer: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn restore_after() -> Self {
            Self {
                ffmpeg_bin: std::env::var_os("FFMPEG_BIN"),
                finalizer: std::env::var_os("MARGINS_HOSTED_WEBM_FINALIZER"),
            }
        }

        fn set_invalid_ffmpeg() -> Self {
            let guard = Self::restore_after();
            std::env::set_var("FFMPEG_BIN", "/definitely/not/ffmpeg");
            std::env::remove_var("MARGINS_HOSTED_WEBM_FINALIZER");
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.ffmpeg_bin {
                Some(value) => std::env::set_var("FFMPEG_BIN", value),
                None => std::env::remove_var("FFMPEG_BIN"),
            }
            match &self.finalizer {
                Some(value) => std::env::set_var("MARGINS_HOSTED_WEBM_FINALIZER", value),
                None => std::env::remove_var("MARGINS_HOSTED_WEBM_FINALIZER"),
            }
        }
    }
}
