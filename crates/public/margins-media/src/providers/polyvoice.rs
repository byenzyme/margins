//! Polyvoice diarization over caller-supplied mono PCM.

use crate::diarization::SpeakerSegment;
use anyhow::{anyhow, Result};
use polyvoice::pipeline_v2::Pipeline;
use polyvoice::{ModelRegistry, Profile, SampleRate};
use std::sync::Mutex;

/// Balanced Polyvoice pipeline with powerset segmentation and WeSpeaker
/// embeddings. Construction may resolve model assets, but inference never
/// opens or owns an audio device.
pub struct PolyvoiceDiarization {
    pipeline: Mutex<Pipeline>,
}

impl PolyvoiceDiarization {
    /// Construct a diarizer with an optional speaker-count cap.
    pub fn with_speaker_count(max_speakers: Option<usize>) -> Result<Self> {
        let registry = ModelRegistry::default()?;
        let mut builder = Pipeline::builder()
            .profile(Profile::Balanced)
            .with_models_from(registry);
        if let Some(n) = max_speakers {
            builder = builder.max_speakers(n.clamp(1, 255) as u8);
        }
        let pipeline = builder.build()?;
        Ok(Self {
            pipeline: Mutex::new(pipeline),
        })
    }

    pub fn from_default_registry() -> Result<Self> {
        Self::with_speaker_count(None)
    }

    pub fn diarize_pcm(&self, mono_16k: &[f32]) -> Result<Vec<SpeakerSegment>> {
        let sample_rate = SampleRate::new(16_000).ok_or_else(|| anyhow!("invalid sample rate"))?;
        let pipeline = self
            .pipeline
            .lock()
            .map_err(|_| anyhow!("Polyvoice pipeline lock was poisoned"))?;
        let result = pipeline.run(mono_16k, sample_rate)?;
        Ok(result
            .turns
            .into_iter()
            .map(|turn| SpeakerSegment {
                start_ms: seconds_to_ms(turn.time.start),
                end_ms: seconds_to_ms(turn.time.end),
                speaker: format!("SPEAKER_{:02}", turn.speaker.0),
            })
            .collect())
    }
}

impl margins_core::DiarizationBackend for PolyvoiceDiarization {
    fn diarize(
        &self,
        request: margins_core::DiarizationRequest,
    ) -> Result<margins_core::DiarizationResult, margins_core::DiarizationError> {
        if request.sample_rate_hz != 16_000 || request.samples.is_empty() {
            return Err(margins_core::DiarizationError {
                code: margins_core::DiarizationErrorCode::InvalidAudio,
                message: "Polyvoice requires non-empty mono 16 kHz f32 PCM".into(),
                retryable: false,
            });
        }
        self.diarize_pcm(&request.samples)
            .map(|segments| margins_core::DiarizationResult {
                segments: segments
                    .into_iter()
                    .map(|segment| margins_core::SpeakerSegment {
                        start_ms: segment.start_ms.saturating_add(request.session_offset_ms),
                        end_ms: segment.end_ms.saturating_add(request.session_offset_ms),
                        speaker: margins_core::SpeakerId::new(segment.speaker),
                    })
                    .collect(),
            })
            .map_err(|error| margins_core::DiarizationError {
                code: margins_core::DiarizationErrorCode::InferenceFailed,
                message: error.to_string(),
                retryable: false,
            })
    }
}

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1000.0).round() as u64
}
