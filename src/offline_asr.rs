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
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OfflineTranscript {
    pub backend: &'static str,
    pub entries: Vec<TranscriptWordEntry>,
}

/// Root composition adapter for the public workflow port. Model selection and
/// feature policy stay private to this facade; workflows receive only PCM and
/// public transcript values.
pub struct PublicAsrBackend;

impl margins_core::AsrBackend for PublicAsrBackend {
    fn backend_name(&self) -> &'static str {
        #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
        {
            return "coreml";
        }
        #[cfg(all(
            not(all(feature = "coreml-asr", target_os = "macos")),
            feature = "parakeet-asr"
        ))]
        {
            return "parakeet-onnx";
        }
        #[allow(unreachable_code)]
        "unavailable"
    }

    fn transcribe(
        &self,
        request: margins_core::AsrRequest,
    ) -> std::result::Result<margins_core::AsrResult, margins_core::TranscriptError> {
        transcribe_mono_16k(&request.samples)
            .map(|transcript| margins_core::AsrResult {
                words: transcript
                    .entries
                    .into_iter()
                    .map(|entry| margins_core::TranscriptWord {
                        start_ms: entry.start_ms.saturating_add(request.session_offset_ms),
                        end_ms: entry.end_ms.saturating_add(request.session_offset_ms),
                        text: entry.text,
                        speaker: None,
                        confidence_per_mille: None,
                    })
                    .collect(),
                detected_language: None,
            })
            .map_err(|error| margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::InferenceFailed,
                message: error.to_string(),
                retryable: false,
            })
    }
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
    let model_dir = resolve_coreml_model_dir().context(
        "FluidAudio CoreML model assets were not found. Install FluidAudio models or set MARGINS_FLUID_COREML_MODEL_DIR.",
    )?;
    let mut backend =
        crate::coreml_asr::FluidCoreMlAsr::from_dir_auto(&model_dir).with_context(|| {
            format!(
                "failed to load CoreML ASR models from {}",
                model_dir.display()
            )
        })?;
    let words = backend.transcribe_words(mono_16k)?;
    Ok(OfflineTranscript {
        backend: "coreml",
        entries: asr::merge_and_dedupe_entries(
            asr::words_to_transcript_entries(&words, 0, 0),
            2_000,
        ),
    })
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn resolve_coreml_model_dir() -> Option<PathBuf> {
    let override_dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    resolve_coreml_model_dir_from(override_dir, default_coreml_model_dir())
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn resolve_coreml_model_dir_from(
    override_dir: Option<PathBuf>,
    default_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    match override_dir {
        Some(dir) => has_coreml_assets(&dir).then_some(dir),
        None => default_dir,
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn default_coreml_model_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let root = home.join("Library/Application Support/FluidAudio/Models");
    let preferred = match std::env::var("MARGINS_FLUID_COREML_VERSION")
        .ok()
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("v3") | Some("3") => "parakeet-tdt-0.6b-v3",
        _ => "parakeet-tdt-0.6b-v2",
    };
    [
        root.join(preferred),
        root.join("parakeet-tdt-0.6b-v2"),
        root.join("parakeet-tdt-0.6b-v3"),
    ]
    .into_iter()
    .find(|dir| has_coreml_assets(dir))
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn has_coreml_assets(dir: &std::path::Path) -> bool {
    dir.join("Preprocessor.mlmodelc").exists()
        && dir.join("Encoder.mlmodelc").exists()
        && dir.join("Decoder.mlmodelc").exists()
        && dir.join("JointDecision.mlmodelc").exists()
        && dir.join("parakeet_vocab.json").exists()
}

#[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
mod coreml_model_dir_tests {
    use super::*;

    #[test]
    fn missing_override_does_not_fall_back_to_shared_cache() {
        let root =
            std::env::temp_dir().join(format!("offline_asr_override_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let missing_override = root.join("missing");
        let shared = root.join("shared");
        for name in [
            "Preprocessor.mlmodelc",
            "Encoder.mlmodelc",
            "Decoder.mlmodelc",
            "JointDecision.mlmodelc",
        ] {
            std::fs::create_dir_all(shared.join(name)).unwrap();
        }
        std::fs::write(shared.join("parakeet_vocab.json"), b"{}").unwrap();
        assert_eq!(
            resolve_coreml_model_dir_from(Some(missing_override), Some(shared)),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn transcribe_mono_16k_parakeet(mono_16k: &[f32]) -> Result<OfflineTranscript> {
    let (model_dir, kind) = resolve_parakeet_model_dir()?;
    let mut backend =
        asr::parakeet::ParakeetAsr::from_dir(&model_dir, kind).with_context(|| {
            format!(
                "failed to load Parakeet ONNX models from {}",
                model_dir.display()
            )
        })?;
    let words = backend.transcribe_words(mono_16k)?;
    Ok(OfflineTranscript {
        backend: "parakeet-onnx",
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
pub fn resolve_parakeet_model_dir() -> Result<(PathBuf, asr::AsrModelKind)> {
    let dir = std::env::var("MARGINS_PARAKEET_MODEL_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(expand_tilde_path)
        .ok_or_else(|| {
            anyhow::anyhow!("Set MARGINS_PARAKEET_MODEL_DIR to a Parakeet TDT ONNX model folder.")
        })?;

    let kind = match std::env::var("MARGINS_PARAKEET_MODEL_KIND")
        .unwrap_or_else(|_| "tdt".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "tdt" | "tdt-v2" | "v2" | "tdt-v3" | "v3" => asr::AsrModelKind::Tdt,
        "ctc" => asr::AsrModelKind::Ctc,
        other => bail!(
            "Unknown MARGINS_PARAKEET_MODEL_KIND `{other}`; use `tdt-v2`, `tdt-v3`, or `ctc`."
        ),
    };

    let missing = asr::missing_model_files(&dir, kind);
    if !missing.is_empty() {
        bail!(
            "Parakeet ONNX model folder {} is missing: {}",
            dir.display(),
            missing.join(", ")
        );
    }

    Ok((dir, kind))
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn expand_tilde_path(value: String) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}
