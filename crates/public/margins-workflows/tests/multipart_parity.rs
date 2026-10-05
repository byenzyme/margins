use chrono::{Duration, Local};
use margins_core::{AsrBackend, AsrRequest, AsrResult, TranscriptError, TranscriptWord};
use margins_media::audio::write_interleaved_wav;
use margins_store::canonical;
use margins_workflows::processing::{
    process_session, transcribe_audio, ProcessRequest, TranscribeRequest,
};
use margins_workflows::transcript_view::load_transcript_view;
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
            speakers: 0,
            align_only: false,
        },
        &asr,
        None,
    )
    .unwrap_err();
    assert!(process_error.to_string().contains("at least 1"));
    assert_eq!(asr.0.load(Ordering::SeqCst), 0);
    assert!(!dir.exists());

    let mono_path = work.join("mono.wav");
    write_interleaved_wav(&mono_path, &[0.0; 16_000], 16_000, 1).unwrap();
    let transcribe_error = transcribe_audio(
        TranscribeRequest {
            work_dir: work,
            margins_dir: &dir,
            audio_path: &mono_path,
            requested_name: None,
            memo_path: None,
            speakers: 2,
            started_at: Local::now(),
        },
        &asr,
        None,
    )
    .unwrap_err();
    assert!(transcribe_error.to_string().contains("diarization backend"));
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
    canonical::create_session(&dir, "meet", &started, ".margins/meet.md").unwrap();
    std::fs::write(dir.join("meet.md"), "[01:30] boundary memo\n").unwrap();
    for ordinal in [1, 0] {
        let rel = format!(".margins/meet_seg{ordinal}.wav");
        write_interleaved_wav(work.join(&rel), &[0.0; 16_000], 16_000, 1).unwrap();
        canonical::add_segment(
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
    std::fs::write(
        dir.join("meet_seg0.live-transcript.json"),
        serde_json::json!({
            "version": 2, "terminal": true,
            "decoded_until_ms": 11, "committed_until_ms": 11,
            "transcripts": [{"words": [{"channel": 0, "start_ms": 0, "end_ms": 11, "text": "Mm."}]}]
        })
        .to_string(),
    )
    .unwrap();
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
    assert_eq!(
        canonical::get_session_meta(&dir, "meet")
            .unwrap()
            .processing_state
            .as_deref(),
        Some("done")
    );
    let final_view = load_transcript_view(work, &dir, "meet").unwrap();
    assert_eq!(final_view.view, "aligned");
    assert!(final_view.terminal);
    assert!(final_view.body.contains("part-0"));
    assert!(!final_view.body.contains("Mm."));

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
    let artifacts = canonical::list_session_artifacts(&dir, "meet").unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        artifacts[0].kind,
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT
    );
}

#[test]
fn attached_audio_invalidates_processed_transcript_until_reprocessed() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    canonical::create_session(&dir, "meet", &Local::now(), ".margins/meet.md").unwrap();
    std::fs::write(dir.join("meet.md"), "").unwrap();
    for (ordinal, offset_ms) in [(0, 0), (1, 10_000)] {
        if ordinal == 1 {
            let old = load_transcript_view(work, &dir, "meet").unwrap();
            assert_eq!(old.view, "aligned");
            assert!(old.body.contains("part-0"));
        }
        let path = format!(".margins/meet_seg{ordinal}.wav");
        write_interleaved_wav(work.join(&path), &[0.0; 16_000], 16_000, 1).unwrap();
        canonical::add_segment(&dir, "meet", ordinal, &path, offset_ms, Some(1.0)).unwrap();
        if ordinal == 0 {
            process_session(
                ProcessRequest {
                    work_dir: work,
                    margins_dir: &dir,
                    session_name: "meet",
                    speakers: 1,
                    align_only: false,
                },
                &FakeAsr(AtomicUsize::new(0)),
                None,
            )
            .unwrap();
        }
    }
    let stale = load_transcript_view(work, &dir, "meet").unwrap();
    assert_eq!(stale.view, "incomplete");
    assert!(!stale.terminal);
    assert!(!stale.body.contains("part-0"));
    std::fs::write(
        dir.join("meet_seg1.live-transcript.json"),
        serde_json::json!({
            "version": 2, "terminal": true, "start_offset_ms": 10_000,
            "captured_until_ms": 11_000,
            "decoded_until_ms": 11_000, "committed_until_ms": 11_000,
            "transcripts": [{"words": [{"channel": 0, "start_ms": 10_100, "end_ms": 10_500, "text": "second"}]}]
        }).to_string(),
    ).unwrap();
    let live = load_transcript_view(work, &dir, "meet").unwrap();
    assert_eq!(
        margins_workflows::transcript_view::preferred_transcript_path(&dir, "meet"),
        Some(dir.join("meet_seg1.live-transcript.json"))
    );
    assert_eq!(live.view, "full");
    assert!(!live.terminal);
    assert!(live.body.contains("second"));
    assert!(!live.body.contains("part-0"));

    let stale_align = process_session(
        ProcessRequest {
            work_dir: work,
            margins_dir: &dir,
            session_name: "meet",
            speakers: 1,
            align_only: true,
        },
        &FakeAsr(AtomicUsize::new(0)),
        None,
    )
    .unwrap_err();
    assert!(stale_align.to_string().contains("predates newer audio"));

    process_session(
        ProcessRequest {
            work_dir: work,
            margins_dir: &dir,
            session_name: "meet",
            speakers: 1,
            align_only: false,
        },
        &FakeAsr(AtomicUsize::new(0)),
        None,
    )
    .unwrap();
    let final_view = load_transcript_view(work, &dir, "meet").unwrap();
    assert_eq!(final_view.view, "aligned");
    assert!(final_view.terminal);
    assert!(final_view.body.contains("part-0"));
    assert!(final_view.body.contains("part-1"));
}

#[test]
fn attached_terminal_checkpoint_cannot_cover_the_earlier_segment() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    canonical::create_session(&dir, "attached", &Local::now(), ".margins/attached.md").unwrap();
    std::fs::write(dir.join("attached.md"), "").unwrap();
    for (ordinal, offset) in [(0, 0), (1, 10_000)] {
        canonical::add_segment(
            &dir,
            "attached",
            ordinal,
            &format!("attached_seg{ordinal}.wav"),
            offset,
            Some(1.0),
        )
        .unwrap();
        let end = offset as u64 + 1_000;
        std::fs::write(
            dir.join(format!("attached_seg{ordinal}.live-transcript.json")),
            serde_json::json!({
                "version": 2, "terminal": true,
                "start_offset_ms": offset,
                "captured_until_ms": end,
                "decoded_until_ms": end,
                "committed_until_ms": end,
                "live_dropped_samples": 0,
                "transcripts": [{"words": [{"channel": 0, "start_ms": offset + 100,
                    "end_ms": offset + 500, "text": format!("part-{ordinal}")}]}]
            })
            .to_string(),
        )
        .unwrap();
    }
    let view = load_transcript_view(work, &dir, "attached").unwrap();
    assert_eq!(
        margins_workflows::transcript_view::preferred_transcript_path(&dir, "attached"),
        Some(dir.join("attached_seg1.live-transcript.json"))
    );
    assert_eq!(view.view, "full");
    assert!(!view.terminal);
    assert_eq!(view.captured_until_ms, 11_000);
    assert!(view.body.contains("part-1"));
    assert!(!view.body.contains("part-0"));
}

fn assert_single_worker_checkpoint_spans_segments(name: &str, offsets_ms: &[i64]) {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    let started = Local::now() - Duration::minutes(1);
    canonical::create_session(&dir, name, &started, &format!(".margins/{name}.md")).unwrap();
    std::fs::write(dir.join(format!("{name}.md")), "").unwrap();
    for (ordinal, offset_ms) in offsets_ms.iter().enumerate() {
        canonical::add_segment(
            &dir,
            name,
            ordinal as i64,
            &format!("{name}_seg{ordinal}.wav"),
            *offset_ms,
            Some(1.0),
        )
        .unwrap();
    }
    canonical::mark_session_ended(&dir, name).unwrap();
    let captured_until_ms = offsets_ms.last().copied().unwrap().max(0) + 1_000;
    let checkpoint_name = format!("{name}_seg0.live-transcript.json");
    std::fs::write(
        dir.join(&checkpoint_name),
        serde_json::json!({
            "version": 2, "terminal": true, "start_offset_ms": 0,
            "captured_until_ms": captured_until_ms,
            "decoded_until_ms": captured_until_ms,
            "committed_until_ms": captured_until_ms,
            "live_dropped_samples": 0,
            "transcripts": [{"words": [{"channel": 0, "start_ms": 100,
                "end_ms": 500, "text": "first"},
                {"channel": 0, "start_ms": captured_until_ms - 900,
                    "end_ms": captured_until_ms - 500, "text": "last"}]}]
        })
        .to_string(),
    )
    .unwrap();
    canonical::upsert_session_artifact(
        &dir,
        name,
        canonical::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &format!(".margins/{checkpoint_name}"),
        "durable",
        None,
    )
    .unwrap();
    let view = load_transcript_view(work, &dir, name).unwrap();
    assert_eq!(view.view, "full");
    assert!(view.terminal, "same worker covered every segment: {name}");
    assert_eq!(view.captured_until_ms, captured_until_ms as u64);
    assert!(view.body.contains("first"));
    assert!(view.body.contains("last"));
    assert_eq!(
        margins_workflows::transcript_view::preferred_transcript_path(&dir, name),
        Some(dir.join(checkpoint_name))
    );
}

#[test]
fn paused_and_resumed_session_keeps_its_seg0_checkpoint_terminal() {
    assert_single_worker_checkpoint_spans_segments("paused", &[0, 10_000]);
}

#[test]
fn mic_switched_session_keeps_its_seg0_checkpoint_terminal() {
    assert_single_worker_checkpoint_spans_segments("switched", &[0, 1_000, 2_000]);
}

#[test]
fn crashed_unfinished_segment_is_recorded_as_an_incomplete_processing_gap() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    canonical::create_session(&dir, "crashed", &Local::now(), ".margins/crashed.md").unwrap();
    std::fs::write(dir.join("crashed.md"), "").unwrap();
    let path = ".margins/crashed_seg0.wav";
    write_interleaved_wav(work.join(path), &[0.0; 16_000], 16_000, 1).unwrap();
    canonical::add_segment(&dir, "crashed", 0, path, 0, Some(1.0)).unwrap();
    canonical::add_segment(&dir, "crashed", 1, "crashed_seg1.wav", 10_000, None).unwrap();
    let result = process_session(
        ProcessRequest {
            work_dir: work,
            margins_dir: &dir,
            session_name: "crashed",
            speakers: 1,
            align_only: false,
        },
        &FakeAsr(AtomicUsize::new(0)),
        None,
    )
    .unwrap();
    assert_eq!(result.segment_count, 2);
    let view = load_transcript_view(work, &dir, "crashed").unwrap();
    assert_eq!(view.view, "incomplete");
    assert!(!view.terminal);
    assert!(view.body.contains("part-0"));
    assert!(view
        .body
        .contains("Segment 1 began at 10000 ms but did not finish"));
    assert_eq!(
        canonical::list_processing_gaps(&dir, "crashed")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        canonical::get_session_meta(&dir, "crashed")
            .unwrap()
            .processing_state
            .as_deref(),
        Some("none")
    );
}

#[test]
fn crashed_first_segment_can_be_reported_without_claiming_audio_was_transcribed() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path();
    let dir = work.join(".margins");
    canonical::create_session(
        &dir,
        "first-crash",
        &Local::now(),
        ".margins/first-crash.md",
    )
    .unwrap();
    std::fs::write(dir.join("first-crash.md"), "").unwrap();
    canonical::add_segment(&dir, "first-crash", 0, "first-crash_seg0.wav", 0, None).unwrap();
    let calls = AtomicUsize::new(0);
    let result = process_session(
        ProcessRequest {
            work_dir: work,
            margins_dir: &dir,
            session_name: "first-crash",
            speakers: 1,
            align_only: false,
        },
        &FakeAsr(calls),
        None,
    )
    .unwrap();
    assert_eq!(result.transcript_entries, 0);
    let view = load_transcript_view(work, &dir, "first-crash").unwrap();
    assert_eq!(view.view, "incomplete");
    assert!(!view.terminal);
    assert!(view
        .body
        .contains("Segment 0 began at 0 ms but did not finish"));
    assert_eq!(
        canonical::list_processing_gaps(&dir, "first-crash")
            .unwrap()
            .len(),
        1
    );
}
