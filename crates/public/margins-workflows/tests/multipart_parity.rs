use chrono::{Duration, Local};
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptWord};
use margins_media::audio::write_interleaved_wav;
use margins_store::legacy;
use margins_workflows::processing::{
    process_session, transcribe_audio, ProcessRequest, TranscribeRequest,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeAsr(AtomicUsize);

impl AsrBackend for FakeAsr {
    fn backend_name(&self) -> &'static str {
        "fake-asr"
    }

    fn transcribe(&self, request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        let call = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(AsrResult {
            words: vec![TranscriptWord {
                start_ms: request.session_offset_ms + 1_000,
                end_ms: request.session_offset_ms + 2_000,
                text: format!("part-{call}"),
                speaker: None,
                confidence_per_mille: None,
            }],
            detected_language: None,
        })
    }
}

#[test]
fn invalid_speaker_configuration_fails_before_provider_or_filesystem_writes() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join("not-created").join(".margins");
    let asr = FakeAsr(AtomicUsize::new(0));

    let process_error = process_session(
        ProcessRequest {
            work_dir: work,
            margins_dir: &dir,
            session_name: "meet",
            speakers: 2,
            align_only: false,
        },
        &asr,
        None,
    )
    .unwrap_err();
    assert!(process_error.to_string().contains("diarization backend"));
    assert_eq!(asr.0.load(Ordering::SeqCst), 0);
    assert!(!dir.exists());

    let transcribe_error = transcribe_audio(
        TranscribeRequest {
            work_dir: work,
            margins_dir: &dir,
            audio_path: &work.join("missing.wav"),
            requested_name: None,
            memo_path: None,
            speakers: 0,
            started_at: Local::now(),
        },
        &asr,
        None,
    )
    .unwrap_err();
    assert!(transcribe_error.to_string().contains("at least 1"));
    assert_eq!(asr.0.load(Ordering::SeqCst), 0);
    assert!(!dir.exists());
}

#[test]
fn path_like_session_name_fails_before_provider_or_filesystem_writes() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("not-created").join(".margins");
    let asr = FakeAsr(AtomicUsize::new(0));

    let error = process_session(
        ProcessRequest {
            work_dir: temp.path(),
            margins_dir: &dir,
            session_name: "../escaped",
            speakers: 1,
            align_only: false,
        },
        &asr,
        None,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("session name must be a single path component"));
    assert_eq!(asr.0.load(Ordering::SeqCst), 0);
    assert!(!dir.exists());
    assert!(!temp.path().join("escaped_transcript.json").exists());
    assert!(!temp.path().join("escaped_aligned.md").exists());
}

#[test]
fn multipart_offsets_apply_once_and_align_only_makes_no_asr_calls() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    let started = Local::now() - Duration::minutes(5);
    legacy::create_session(&dir, "meet", &started, ".margins/meet.md").unwrap();
    std::fs::write(dir.join("meet.md"), "[01:30] boundary memo\n").unwrap();
    for ordinal in [1, 0] {
        let rel = format!(".margins/meet_seg{ordinal}.wav");
        write_interleaved_wav(work.join(&rel), &[0.0; 16_000], 16_000, 1).unwrap();
        legacy::add_segment(
            &dir,
            "meet",
            ordinal,
            &rel,
            if ordinal == 0 { 0 } else { 90_000 },
            Some(1.0),
        )
        .unwrap();
    }
    let asr = FakeAsr(AtomicUsize::new(0));
    let request = || ProcessRequest {
        work_dir: work,
        margins_dir: &dir,
        session_name: "meet",
        speakers: 1,
        align_only: false,
    };
    let first = process_session(request(), &asr, None).unwrap();
    assert_eq!(asr.0.load(Ordering::SeqCst), 2);
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&first.transcript_json).unwrap()).unwrap();
    let words = json["transcripts"][0]["words"].as_array().unwrap();
    assert_eq!(words[0]["start_ms"], 1_000);
    assert_eq!(words[1]["start_ms"], 91_000);
    assert_eq!(first.asr_backend, "fake-asr");

    let second = process_session(
        ProcessRequest {
            align_only: true,
            ..request()
        },
        &asr,
        None,
    )
    .unwrap();
    assert_eq!(asr.0.load(Ordering::SeqCst), 2);
    assert_eq!(second.transcript_entries, 2);
    let aligned = std::fs::read_to_string(second.aligned_path).unwrap();
    assert!(aligned.find("part-0").unwrap() < aligned.find("boundary memo").unwrap());
    assert!(aligned.find("boundary memo").unwrap() < aligned.find("part-1").unwrap());
    let artifacts = legacy::list_session_artifacts(&dir, "meet").unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT);
}
