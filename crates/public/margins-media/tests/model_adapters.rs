use margins_core::{
    AsrBackend, AsrRequest, DiarizationBackend, DiarizationErrorCode, DiarizationRequest,
    TranscriptErrorCode,
};
use margins_media::providers::{UnavailableAsr, UnavailableDiarization};

fn assert_send_sync<T: Send + Sync>() {}
fn assert_asr_backend<T: AsrBackend>() {}
fn assert_diarization_backend<T: DiarizationBackend>() {}

#[test]
fn unavailable_adapters_are_model_free_public_ports() {
    assert_send_sync::<UnavailableAsr>();
    assert_send_sync::<UnavailableDiarization>();
    assert_asr_backend::<UnavailableAsr>();
    assert_diarization_backend::<UnavailableDiarization>();

    let asr_error = UnavailableAsr
        .transcribe(AsrRequest {
            samples: vec![0.0],
            sample_rate_hz: 16_000,
            session_offset_ms: 0,
            language: None,
        })
        .unwrap_err();
    assert_eq!(asr_error.code, TranscriptErrorCode::Unavailable);
    assert_eq!(asr_error.message, "ASR is unavailable in this build");
    assert!(!asr_error.retryable);

    let diarization_error = UnavailableDiarization
        .diarize(DiarizationRequest {
            samples: vec![0.0],
            sample_rate_hz: 16_000,
            session_offset_ms: 0,
            max_speakers: None,
        })
        .unwrap_err();
    assert_eq!(diarization_error.code, DiarizationErrorCode::Unavailable);
    assert_eq!(
        diarization_error.message,
        "diarization is unavailable in this build"
    );
    assert!(!diarization_error.retryable);
}

#[cfg(feature = "parakeet-onnx")]
#[test]
fn parakeet_adapter_satisfies_the_public_port_without_loading_models() {
    assert_send_sync::<margins_media::providers::parakeet::ParakeetOnnxBackend>();
    assert_asr_backend::<margins_media::providers::parakeet::ParakeetOnnxBackend>();
}

#[cfg(feature = "polyvoice-diarization")]
#[test]
fn polyvoice_adapter_satisfies_the_public_port_without_loading_models() {
    assert_send_sync::<margins_media::providers::polyvoice::PolyvoiceDiarization>();
    assert_diarization_backend::<margins_media::providers::polyvoice::PolyvoiceDiarization>();
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
#[test]
fn coreml_adapter_satisfies_the_public_port_without_loading_models() {
    assert_send_sync::<margins_media::providers::coreml::CoreMlAsrBackend>();
    assert_asr_backend::<margins_media::providers::coreml::CoreMlAsrBackend>();
}

#[test]
fn parakeet_asset_validation_does_not_load_a_model() {
    use margins_media::providers::parakeet::{missing_model_files, AsrModelKind};

    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        missing_model_files(directory.path(), AsrModelKind::Tdt),
        vec![
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
            "vocab.txt",
        ]
    );
}
