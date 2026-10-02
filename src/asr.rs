//! Compatibility facade for model adapters now owned by `margins-media`.

pub use margins_media::providers::parakeet::{
    expected_model_files, missing_model_files, AsrModelKind,
};

pub use margins_media::transcript::{
    merge_and_dedupe_entries, merge_word_entries_to_phrases, transcript_json,
    words_to_transcript_entries, TranscriptWordEntry, WordTiming,
};

#[cfg(feature = "parakeet-asr")]
pub mod parakeet {
    pub use margins_media::providers::parakeet::ParakeetAsr;
}

#[cfg(not(feature = "parakeet-asr"))]
pub use margins_media::providers::parakeet::parakeet_feature_error;
