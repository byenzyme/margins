use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Lightweight audio metadata used by the desktop app before outsourcing any
/// transcription work. This intentionally avoids ffprobe for Margins's own WAV
/// recorder output. For non-WAV imports we can add Symphonia/AVFoundation later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub duration_secs: f64,
    pub container: String,
}

pub fn probe_wav(path: impl AsRef<Path>) -> Result<AudioInfo> {
    let path = path.as_ref();
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open WAV {}", path.display()))?;
    let spec = reader.spec();
    // hound reports duration in sample frames (samples per channel), not raw
    // interleaved sample count.
    let sample_frames = reader.duration() as f64;
    let channels = spec.channels.max(1);
    let sample_rate = spec.sample_rate.max(1);
    let duration_secs = sample_frames / sample_rate as f64;

    Ok(AudioInfo {
        channels,
        sample_rate,
        bits_per_sample: spec.bits_per_sample,
        duration_secs,
        container: "wav".to_string(),
    })
}

pub fn probe(path: impl AsRef<Path>) -> Result<AudioInfo> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "wav" => probe_wav(path),
        _ => anyhow::bail!(
            "unsupported audio container for bundled Rust probe: {}",
            path.display(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_wav_metadata() {
        let path = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(path.path(), spec).unwrap();
            for _ in 0..96_000 {
                writer.write_sample::<i16>(0).unwrap();
            }
            writer.finalize().unwrap();
        }

        let info = probe(path.path()).unwrap();
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert!((info.duration_secs - 1.0).abs() < 0.001);
    }
}
