#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
use crate::asr;
use crate::asr::TranscriptWordEntry;
#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
use margins_core::{AsrBackend, AsrRequest};

#[derive(Debug, Clone)]
pub struct OfflineTranscript {
    pub backend: &'static str,
    pub entries: Vec<TranscriptWordEntry>,
}

pub fn transcribe_mono_16k(mono_16k: &[f32]) -> Result<OfflineTranscript> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        return transcribe_mono_16k_coreml(mono_16k);
    }

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    {
        return transcribe_mono_16k_parakeet(mono_16k);
    }

    #[allow(unreachable_code)]
    {
        let _ = mono_16k;
        bail!(
            "No native ASR backend is enabled for this margins binary. Build macOS with `coreml-asr` or Linux/Windows with `parakeet-asr`."
        )
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn transcribe_mono_16k_coreml(mono_16k: &[f32]) -> Result<OfflineTranscript> {
    let model_dir = margins_media::model_registry::resolve_coreml_dir().context(
        "FluidAudio CoreML model assets were not found. Install FluidAudio models or set MARGINS_FLUID_COREML_MODEL_DIR.",
    )?;
    let backend = margins_media::providers::coreml::CoreMlAsrBackend::from_dir_auto(&model_dir)
        .with_context(|| {
            format!(
                "failed to load CoreML ASR models from {}",
                model_dir.display()
            )
        })?;
    let words = transcribe_public(&backend, mono_16k)?;
    Ok(OfflineTranscript {
        backend: "coreml",
        entries: asr::merge_and_dedupe_entries(
            asr::words_to_transcript_entries(&words, 0, 0),
            2_000,
        ),
    })
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn transcribe_mono_16k_parakeet(mono_16k: &[f32]) -> Result<OfflineTranscript> {
    let (model_dir, kind) = margins_media::model_registry::resolve_parakeet_model()?
        .context("Set MARGINS_PARAKEET_MODEL_DIR to a Parakeet TDT ONNX model folder.")?;
    let backend =
        margins_media::providers::parakeet::ParakeetOnnxBackend::from_dir(&model_dir, kind)
            .with_context(|| {
                format!(
                    "failed to load Parakeet ONNX models from {}",
                    model_dir.display()
                )
            })?;
    let words = transcribe_public(&backend, mono_16k)?;
    Ok(OfflineTranscript {
        backend: "parakeet-onnx",
        entries: asr::merge_and_dedupe_entries(
            asr::words_to_transcript_entries(&words, 0, 0),
            2_000,
        ),
    })
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
fn transcribe_public(backend: &dyn AsrBackend, mono_16k: &[f32]) -> Result<Vec<asr::WordTiming>> {
    Ok(backend
        .transcribe(AsrRequest {
            samples: mono_16k.to_vec(),
            sample_rate_hz: 16_000,
            session_offset_ms: 0,
            language: None,
        })?
        .words
        .iter()
        .map(asr::WordTiming::from)
        .collect())
}
