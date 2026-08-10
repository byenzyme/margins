//! Optional speech-model adapters over caller-supplied PCM.
//!
//! This module owns model loading and inference only. It does not enumerate,
//! open, or monitor audio devices and does not own capture callbacks.

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[allow(unsafe_code)]
pub mod coreml;
#[allow(unsafe_code)]
pub mod parakeet;
#[cfg(feature = "polyvoice-diarization")]
pub mod polyvoice;

use margins_core::{
    AsrBackend, AsrRequest, AsrResult, DiarizationBackend, DiarizationError, DiarizationErrorCode,
    DiarizationRequest, DiarizationResult, TranscriptError, TranscriptErrorCode,
};

/// Stable no-model ASR adapter for no-default-feature composition.
#[derive(Debug, Clone, Default)]
pub struct UnavailableAsr;

impl AsrBackend for UnavailableAsr {
    fn is_available(&self) -> bool {
        false
    }

    fn backend_name(&self) -> &'static str {
        "unavailable"
    }

    fn transcribe(&self, _request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        Err(TranscriptError {
            code: TranscriptErrorCode::Unavailable,
            message: "ASR is unavailable in this build".into(),
            retryable: false,
        })
    }
}

/// Stable no-model diarization adapter for no-default-feature composition.
#[derive(Debug, Clone, Default)]
pub struct UnavailableDiarization;

impl DiarizationBackend for UnavailableDiarization {
    fn is_available(&self) -> bool {
        false
    }

    fn diarize(&self, _request: DiarizationRequest) -> Result<DiarizationResult, DiarizationError> {
        Err(DiarizationError {
            code: DiarizationErrorCode::Unavailable,
            message: "diarization is unavailable in this build".into(),
            retryable: false,
        })
    }
}

/// Feature-selected compatibility adapter for compositions that want the
/// Polyvoice provider when compiled and a typed unavailable result otherwise.
pub struct PublicDiarizationBackend {
    pub max_speakers: Option<usize>,
}

impl DiarizationBackend for PublicDiarizationBackend {
    fn is_available(&self) -> bool {
        cfg!(feature = "polyvoice-diarization")
    }

    fn diarize(&self, request: DiarizationRequest) -> Result<DiarizationResult, DiarizationError> {
        #[cfg(feature = "polyvoice-diarization")]
        {
            let backend = polyvoice::PolyvoiceDiarization::with_speaker_count(
                self.max_speakers.or(request.max_speakers.map(usize::from)),
            )
            .map_err(|error| DiarizationError {
                code: DiarizationErrorCode::ModelLoadFailed,
                message: error.to_string(),
                retryable: false,
            })?;
            return DiarizationBackend::diarize(&backend, request);
        }
        #[cfg(not(feature = "polyvoice-diarization"))]
        {
            let _ = request;
            Err(DiarizationError {
                code: DiarizationErrorCode::Unavailable,
                message:
                    "Rust diarization is not enabled; build with feature `polyvoice-diarization`"
                        .into(),
                retryable: false,
            })
        }
    }
}
