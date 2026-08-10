use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use symphonia::core::audio::conv::IntoSample;
use symphonia::core::audio::{Audio, GenericAudioBufferRef};

/// Interleaved floating-point audio loaded from a WAV file.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioBuffer {
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / self.sample_rate as f64
        }
    }
}

/// Load PCM/float WAV into normalized interleaved f32 samples.
pub fn load_wav(path: impl AsRef<Path>) -> Result<AudioBuffer> {
    let path = path.as_ref();
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open WAV {}", path.display()))?;
    let spec = reader.spec();

    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read float WAV samples")?,
        hound::SampleFormat::Int => {
            let magnitude_bits = spec
                .bits_per_sample
                .checked_sub(1)
                .context("integer WAV bits per sample must be non-zero")?;
            let denom = 1_u64
                .checked_shl(u32::from(magnitude_bits))
                .context("integer WAV bit depth exceeds 64 bits")?
                .saturating_sub(1) as f32;
            if denom == 0.0 {
                bail!(
                    "unsupported integer WAV bit depth: {}",
                    spec.bits_per_sample
                );
            }
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|s| (s as f32 / denom).clamp(-1.0, 1.0)))
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to read integer WAV samples")?
        }
    };

    Ok(AudioBuffer {
        samples,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    })
}

/// Load common compressed/uncompressed audio containers through Symphonia.
pub fn load_audio_any(path: impl AsRef<Path>) -> Result<AudioBuffer> {
    let path = path.as_ref();
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    {
        return load_wav(path);
    }

    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open audio file {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| format!("failed to parse audio container {}", path.display()))?;
    let mut format = probed;
    let track = format
        .default_track(TrackType::Audio)
        .context("audio file does not contain a default audio track")?;
    let track_id = track.id;
    let codec_params = match &track.codec_params {
        Some(CodecParameters::Audio(params)) => params.clone(),
        _ => bail!("default track is not decodable audio in {}", path.display()),
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .context("failed to initialize audio decoder")?;

    let mut samples = Vec::new();
    let mut sample_rate = codec_params.sample_rate.unwrap_or(0);
    let mut channels = codec_params
        .channels
        .map(|channels| channels.count() as u16)
        .unwrap_or(0);

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::ResetRequired) => {
                bail!(
                    "audio decoder reset is not supported for {}",
                    path.display()
                );
            }
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(err) => return Err(err).context("failed to read audio packet"),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err).context("failed to decode audio packet"),
        };
        if sample_rate == 0 {
            sample_rate = decoded.spec().rate();
        }
        if channels == 0 {
            channels = decoded.spec().channels().count() as u16;
        }
        append_audio_buffer_ref(decoded, &mut samples);
    }

    if sample_rate == 0 || channels == 0 {
        bail!(
            "decoded audio had no sample rate or channels: {}",
            path.display()
        );
    }
    Ok(AudioBuffer {
        samples,
        sample_rate,
        channels,
    })
}

fn append_audio_buffer_ref(decoded: GenericAudioBufferRef<'_>, out: &mut Vec<f32>) {
    match decoded {
        GenericAudioBufferRef::F32(buf) => append_planar(buf, out, |v| v),
        GenericAudioBufferRef::F64(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::U8(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::U16(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::U24(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::U32(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::S8(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::S16(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::S24(buf) => append_planar(buf, out, |v| v.into_sample()),
        GenericAudioBufferRef::S32(buf) => append_planar(buf, out, |v| v.into_sample()),
    }
}

fn append_planar<T>(
    buf: &symphonia::core::audio::AudioBuffer<T>,
    out: &mut Vec<f32>,
    convert: impl Fn(T) -> f32,
) where
    T: Copy + symphonia::core::audio::sample::Sample,
{
    let channels = buf.spec().channels().count();
    for frame in 0..buf.frames() {
        for channel in 0..channels {
            out.push(convert(buf[channel][frame]));
        }
    }
}

/// Extract one channel from interleaved audio.
pub fn extract_channel(audio: &AudioBuffer, channel: usize) -> Result<Vec<f32>> {
    let channels = audio.channels as usize;
    if channels == 0 {
        bail!("audio has zero channels");
    }
    if channel >= channels {
        bail!("requested channel {channel}, but audio has {channels} channels");
    }
    Ok(audio
        .samples
        .chunks(channels)
        .filter_map(|frame| frame.get(channel).copied())
        .collect())
}

/// Average all channels into mono.
pub fn downmix_to_mono(audio: &AudioBuffer) -> Result<Vec<f32>> {
    let channels = audio.channels as usize;
    if channels == 0 {
        bail!("audio has zero channels");
    }
    if channels == 1 {
        return Ok(audio.samples.clone());
    }
    Ok(audio
        .samples
        .chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect())
}

/// Resample mono audio with deterministic linear interpolation.
///
/// This is intentionally dependency-light and good enough for wiring the Rust
/// ASR/diarization adapters. A higher-quality sinc/FFT resampler can replace
/// this behind the same function later.
pub fn resample_mono_linear(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples.to_vec();
    }

    let ratio = source_rate as f64 / target_rate as f64;
    let out_len = ((samples.len() as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = samples.get(idx).copied().unwrap_or(0.0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }

    out
}

pub fn mono_16k_from_wav(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let audio = load_wav(path)?;
    let mono = downmix_to_mono(&audio)?;
    Ok(resample_mono_linear(&mono, audio.sample_rate, 16_000))
}

pub fn wav_channel_16k(path: impl AsRef<Path>, channel: usize) -> Result<Vec<f32>> {
    let audio = load_wav(path)?;
    let mono = extract_channel(&audio, channel)?;
    Ok(resample_mono_linear(&mono, audio.sample_rate, 16_000))
}

pub fn chunk_mono(samples: &[f32], sample_rate: u32, chunk_secs: f64) -> Vec<(usize, Vec<f32>)> {
    let chunk_len = (sample_rate as f64 * chunk_secs).round().max(1.0) as usize;
    samples
        .chunks(chunk_len)
        .enumerate()
        .map(|(idx, chunk)| (idx * chunk_len, chunk.to_vec()))
        .collect()
}

pub fn write_interleaved_wav(
    path: impl AsRef<Path>,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<f64> {
    if channels == 0 {
        bail!("cannot write WAV with zero channels");
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let path = path.as_ref();
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create WAV {}", path.display()))?;
    for sample in samples {
        writer.write_sample(sample.clamp(-1.0, 1.0))?;
    }
    writer.finalize()?;
    Ok((samples.len() / channels as usize) as f64 / sample_rate as f64)
}

fn convert_audio_buffer(
    audio: &AudioBuffer,
    target_rate: u32,
    target_channels: u16,
) -> Result<Vec<f32>> {
    if target_channels == 0 {
        bail!("target channel count cannot be zero");
    }
    let target_channels_usize = target_channels as usize;
    let mut channels = Vec::with_capacity(target_channels_usize);
    let mut frame_count = 0usize;
    for channel in 0..target_channels_usize {
        let source = if channel < audio.channels as usize {
            extract_channel(audio, channel)?
        } else {
            vec![0.0; audio.frame_count()]
        };
        let resampled = resample_mono_linear(&source, audio.sample_rate, target_rate);
        frame_count = frame_count.max(resampled.len());
        channels.push(resampled);
    }

    let mut out = Vec::with_capacity(frame_count * target_channels_usize);
    for frame in 0..frame_count {
        for channel in &channels {
            out.push(channel.get(frame).copied().unwrap_or(0.0));
        }
    }
    Ok(out)
}

/// Combine WAV segments into one interleaved WAV, preserving each segment's
/// wall-clock offset by inserting silence between segments when needed.
///
/// Segment offsets are milliseconds from the session start. If a segment offset
/// overlaps the already-written audio, the segment is appended rather than
/// mixed, avoiding destructive overlap during device-switch edge cases.
pub fn combine_wav_segments_with_offsets(
    segments: &[(PathBuf, i64)],
    output: impl AsRef<Path>,
) -> Result<f64> {
    if segments.is_empty() {
        bail!("no WAV segments to combine");
    }

    let mut loaded = Vec::with_capacity(segments.len());
    for (path, offset_ms) in segments {
        loaded.push((load_wav(path)?, (*offset_ms).max(0)));
    }

    let target_rate = loaded[0].0.sample_rate;
    if target_rate == 0 {
        bail!("first WAV segment has zero sample rate");
    }
    let target_channels = loaded
        .iter()
        .map(|(audio, _)| audio.channels)
        .max()
        .unwrap_or(1)
        .max(1);
    let target_channels_usize = target_channels as usize;

    let mut combined = Vec::<f32>::new();
    for (audio, offset_ms) in &loaded {
        let start_frame = ((*offset_ms as f64 / 1000.0) * target_rate as f64).round() as usize;
        let current_frames = combined.len() / target_channels_usize;
        let write_frame = start_frame.max(current_frames);
        if write_frame > current_frames {
            combined.resize(write_frame * target_channels_usize, 0.0);
        }

        let converted = convert_audio_buffer(audio, target_rate, target_channels)?;
        combined.extend(converted);
    }

    write_interleaved_wav(output, &combined, target_rate, target_channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_channels_and_resamples() {
        let path = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        write_test_wav(path.path(), 48_000, 2, 48_000, &[1.0, 0.0]);

        let audio = load_wav(path.path()).unwrap();
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.frame_count(), 48_000);

        let ch0 = extract_channel(&audio, 0).unwrap();
        let ch1 = extract_channel(&audio, 1).unwrap();
        assert!(ch0.iter().all(|s| *s > 0.99));
        assert!(ch1.iter().all(|s| s.abs() < 0.001));

        let resampled = resample_mono_linear(&ch0, 48_000, 16_000);
        assert!((resampled.len() as i64 - 16_000).abs() <= 1);
    }

    #[test]
    fn normalizes_integer_pcm_using_its_declared_bit_depth() {
        let eight_bit = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(eight_bit.path(), spec).unwrap();
        for sample in [i8::MIN, 0, i8::MAX] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let loaded = load_wav(eight_bit.path()).unwrap();
        assert_eq!(loaded.samples, vec![-1.0, 0.0, 1.0]);

        let sixteen_bit = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(sixteen_bit.path(), spec).unwrap();
        for sample in [i16::MIN, 0, i16::MAX] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let loaded = load_wav(sixteen_bit.path()).unwrap();
        assert_eq!(loaded.samples, vec![-1.0, 0.0, 1.0]);
    }

    #[test]
    fn combines_segments_with_silence_gap() {
        let seg0 = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let seg1 = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let out = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        write_test_wav(seg0.path(), 1_000, 2, 1_000, &[0.5, 0.0]);
        write_test_wav(seg1.path(), 1_000, 2, 1_000, &[0.0, 0.5]);

        let duration = combine_wav_segments_with_offsets(
            &[
                (seg0.path().to_path_buf(), 0),
                (seg1.path().to_path_buf(), 1_500),
            ],
            out.path(),
        )
        .unwrap();
        assert!((duration - 2.5).abs() < 0.001);

        let audio = load_wav(out.path()).unwrap();
        assert_eq!(audio.sample_rate, 1_000);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.frame_count(), 2_500);
        let gap_start = 1_000 * 2;
        let gap_end = 1_500 * 2;
        assert!(audio.samples[gap_start..gap_end]
            .iter()
            .all(|s| s.abs() < 0.001));
        assert!(audio.samples[1_500 * 2 + 1] > 0.49);
    }

    fn write_test_wav(path: &Path, sample_rate: u32, channels: u16, frames: usize, frame: &[f32]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..frames {
            for channel in 0..channels as usize {
                writer
                    .write_sample::<f32>(frame.get(channel).copied().unwrap_or(0.0))
                    .unwrap();
            }
        }
        writer.finalize().unwrap();
    }
}
