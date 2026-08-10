use crate::transcript::WordTiming;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: String,
}

impl From<SpeakerSegment> for margins_core::SpeakerSegment {
    fn from(value: SpeakerSegment) -> Self {
        Self {
            start_ms: value.start_ms,
            end_ms: value.end_ms,
            speaker: margins_core::SpeakerId::new(value.speaker),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerWordTiming {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub speaker: String,
    pub channel: u32,
}

#[cfg(not(feature = "polyvoice-diarization"))]
pub fn polyvoice_feature_error() -> anyhow::Error {
    anyhow::anyhow!("Rust diarization is not enabled; build with feature `polyvoice-diarization`")
}

/// Assign each ASR word to the diarization speaker segment covering its midpoint.
/// Speaker labels are also mapped to stable numeric channels for the existing
/// transcript JSON shape.
pub fn assign_speakers_to_words(
    words: &[WordTiming],
    segments: &[SpeakerSegment],
) -> Vec<SpeakerWordTiming> {
    let mut speaker_to_channel = BTreeMap::<String, u32>::new();
    let mut next_channel = 0u32;

    words
        .iter()
        .map(|word| {
            let midpoint = (word.start_ms + word.end_ms) / 2;
            let speaker = best_segment_for_midpoint(midpoint, segments)
                .map(|seg| seg.speaker.clone())
                .unwrap_or_else(|| "SPEAKER_00".to_string());
            let channel = *speaker_to_channel
                .entry(speaker.clone())
                .or_insert_with(|| {
                    let channel = next_channel;
                    next_channel += 1;
                    channel
                });

            SpeakerWordTiming {
                start_ms: word.start_ms,
                end_ms: word.end_ms,
                text: word.text.clone(),
                speaker,
                channel,
            }
        })
        .collect()
}

fn best_segment_for_midpoint<'a>(
    midpoint_ms: u64,
    segments: &'a [SpeakerSegment],
) -> Option<&'a SpeakerSegment> {
    segments
        .iter()
        .find(|seg| seg.start_ms <= midpoint_ms && midpoint_ms <= seg.end_ms)
        .or_else(|| {
            segments.iter().min_by_key(|seg| {
                let seg_mid = (seg.start_ms + seg.end_ms) / 2;
                seg_mid.abs_diff(midpoint_ms)
            })
        })
}

pub fn expected_polyvoice_model_note() -> &'static str {
    "polyvoice downloads/validates its balanced WeSpeaker + powerset models in the user cache on first use"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_speakers_by_word_midpoint() {
        let words = vec![
            WordTiming {
                start_ms: 100,
                end_ms: 200,
                text: "hello".into(),
            },
            WordTiming {
                start_ms: 900,
                end_ms: 1000,
                text: "there".into(),
            },
        ];
        let segments = vec![
            SpeakerSegment {
                start_ms: 0,
                end_ms: 500,
                speaker: "SPEAKER_00".into(),
            },
            SpeakerSegment {
                start_ms: 800,
                end_ms: 1200,
                speaker: "SPEAKER_01".into(),
            },
        ];
        let assigned = assign_speakers_to_words(&words, &segments);
        assert_eq!(assigned[0].speaker, "SPEAKER_00");
        assert_eq!(assigned[0].channel, 0);
        assert_eq!(assigned[1].speaker, "SPEAKER_01");
        assert_eq!(assigned[1].channel, 1);
    }
}
