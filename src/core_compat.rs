//! Lossless value conversions for the transitional umbrella crate.

impl From<crate::recorder::LiveAudioChannel> for margins_core::AudioLane {
    fn from(value: crate::recorder::LiveAudioChannel) -> Self {
        match value {
            crate::recorder::LiveAudioChannel::Mic => Self::Microphone,
            crate::recorder::LiveAudioChannel::System => Self::System,
        }
    }
}

impl From<crate::recorder::CaptureLane> for margins_core::AudioLane {
    fn from(value: crate::recorder::CaptureLane) -> Self {
        match value {
            crate::recorder::CaptureLane::Mic => Self::Microphone,
            crate::recorder::CaptureLane::System => Self::System,
        }
    }
}

impl From<crate::recorder::LiveAudioChunk> for margins_core::PcmChunk {
    fn from(value: crate::recorder::LiveAudioChunk) -> Self {
        Self {
            lane: value.channel.into(),
            generation: value.generation,
            session_offset_ms: margins_core::SessionMillis(value.session_offset_ms),
            sample_rate_hz: value.sample_rate,
            samples: value.samples,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lane_conversions_cover_every_legacy_variant() {
        assert_eq!(
            margins_core::AudioLane::from(crate::recorder::LiveAudioChannel::Mic),
            margins_core::AudioLane::Microphone
        );
        assert_eq!(
            margins_core::AudioLane::from(crate::recorder::LiveAudioChannel::System),
            margins_core::AudioLane::System
        );
        assert_eq!(
            margins_core::AudioLane::from(crate::recorder::CaptureLane::Mic),
            margins_core::AudioLane::Microphone
        );
        assert_eq!(
            margins_core::AudioLane::from(crate::recorder::CaptureLane::System),
            margins_core::AudioLane::System
        );
    }

    #[test]
    fn live_audio_chunk_conversion_preserves_pcm_and_timeline() {
        let chunk = crate::recorder::LiveAudioChunk {
            channel: crate::recorder::LiveAudioChannel::System,
            generation: 3,
            session_offset_ms: 42,
            sample_rate: 48_000,
            samples: vec![0.25, -0.5],
        };

        let core: margins_core::PcmChunk = chunk.into();
        assert_eq!(core.lane, margins_core::AudioLane::System);
        assert_eq!(core.generation, 3);
        assert_eq!(core.session_offset_ms.0, 42);
        assert_eq!(core.sample_rate_hz, 48_000);
        assert_eq!(core.samples, vec![0.25, -0.5]);
    }

    #[test]
    fn transcript_conversions_preserve_every_legacy_field() {
        let word = crate::asr::WordTiming {
            start_ms: 10,
            end_ms: 20,
            text: "hello".to_owned(),
        };
        let core_word: margins_core::TranscriptWord = word.into();
        assert_eq!(core_word.start_ms, 10);
        assert_eq!(core_word.end_ms, 20);
        assert_eq!(core_word.text, "hello");
        assert_eq!(core_word.speaker, None);
        assert_eq!(core_word.confidence_per_mille, None);

        let segment = crate::diarization::SpeakerSegment {
            start_ms: 30,
            end_ms: 40,
            speaker: "SPEAKER_01".to_owned(),
        };
        let core_segment: margins_core::SpeakerSegment = segment.into();
        assert_eq!(core_segment.start_ms, 30);
        assert_eq!(core_segment.end_ms, 40);
        assert_eq!(core_segment.speaker.as_str(), "SPEAKER_01");
    }
}
