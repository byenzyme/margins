use margins_media::audio::{
    extract_channel, load_wav, resample_mono_linear, write_interleaved_wav,
};
use margins_media::info::probe;
use margins_media::transcript::{
    merge_and_dedupe_entries, transcript_json, words_to_transcript_entries, TranscriptWordEntry,
    WordTiming,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct AudioGolden {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    expected_frames: usize,
    expected_duration_secs: f64,
    resample_rate: u32,
    expected_channel_0: Vec<f32>,
    expected_channel_1: Vec<f32>,
}

#[derive(Deserialize)]
struct TranscriptGolden {
    words: Vec<WordTiming>,
    channel: u32,
    offset_ms: u64,
    max_gap_ms: u64,
    expected_entries: Vec<TranscriptWordEntry>,
    expected_json: serde_json::Value,
}

#[test]
fn wav_channel_order_frame_count_and_resampling_match_golden_fixture() {
    let fixture: AudioGolden =
        serde_json::from_str(include_str!("fixtures/audio_golden.json")).unwrap();
    let wav = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();

    let duration = write_interleaved_wav(
        wav.path(),
        &fixture.samples,
        fixture.sample_rate,
        fixture.channels,
    )
    .unwrap();
    let info = probe(wav.path()).unwrap();
    let loaded = load_wav(wav.path()).unwrap();

    assert_eq!(loaded.samples, fixture.samples);
    assert_eq!(loaded.frame_count(), fixture.expected_frames);
    assert_eq!(info.channels, fixture.channels);
    assert_eq!(info.sample_rate, fixture.sample_rate);
    assert_eq!(duration, fixture.expected_duration_secs);
    assert_eq!(info.duration_secs, fixture.expected_duration_secs);

    let channel_0 = extract_channel(&loaded, 0).unwrap();
    let channel_1 = extract_channel(&loaded, 1).unwrap();
    assert_eq!(
        resample_mono_linear(&channel_0, fixture.sample_rate, fixture.resample_rate),
        fixture.expected_channel_0
    );
    assert_eq!(
        resample_mono_linear(&channel_1, fixture.sample_rate, fixture.resample_rate),
        fixture.expected_channel_1
    );
}

#[test]
fn transcript_timing_phrase_and_json_output_match_golden_fixture() {
    let fixture: TranscriptGolden =
        serde_json::from_str(include_str!("fixtures/transcript_golden.json")).unwrap();
    let entries = words_to_transcript_entries(&fixture.words, fixture.channel, fixture.offset_ms);
    let entries = merge_and_dedupe_entries(entries, fixture.max_gap_ms);

    assert_eq!(entries, fixture.expected_entries);
    assert_eq!(transcript_json(&entries), fixture.expected_json);
}
