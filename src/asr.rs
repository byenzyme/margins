//! Compatibility facade for model adapters now owned by `margins-media`.

pub use margins_media::providers::parakeet::{
    expected_model_files, missing_model_files, AsrModelKind,
};

pub trait AsrBackend {
    fn transcribe_words(&mut self, mono_16k: &[f32]) -> anyhow::Result<Vec<WordTiming>>;
}
pub use margins_media::transcript::{
    merge_and_dedupe_entries, merge_word_entries_to_phrases, transcript_json,
    words_to_transcript_entries, TranscriptWordEntry, WordTiming,
};

#[cfg(feature = "parakeet-asr")]
pub mod parakeet {
    pub use margins_media::providers::parakeet::ParakeetAsr;
}

#[cfg(feature = "parakeet-asr")]
impl AsrBackend for parakeet::ParakeetAsr {
    fn transcribe_words(&mut self, mono_16k: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
        self.transcribe_words(mono_16k)
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
impl AsrBackend for crate::coreml_asr::FluidCoreMlAsr {
    fn transcribe_words(&mut self, mono_16k: &[f32]) -> anyhow::Result<Vec<WordTiming>> {
        self.transcribe_words(mono_16k)
    }
}

#[cfg(not(feature = "parakeet-asr"))]
pub use margins_media::providers::parakeet::parakeet_feature_error;
