    #[test]
    fn checkpoint_replace_is_atomic_and_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let checkpoint = root.path().join("live.json");
        let outside = root.path().join("outside.json");
        std::fs::write(&outside, b"private").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &checkpoint).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&checkpoint, b"old").unwrap();

        let value = serde_json::json!({"version": 1, "terminal": false});
        write_checkpoint_value(&checkpoint, &value).unwrap();

        assert_eq!(std::fs::read(&outside).unwrap(), b"private");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(checkpoint).unwrap())
                .unwrap(),
            value
        );
    }

    #[test]
    fn recorder_generation_gaps_preserve_the_session_timeline() {
        assert_eq!(timeline_gap_samples(12_000, 10_000, 1_500), 8_000);
        assert_eq!(timeline_gap_samples(12_000, 10_000, 2_000), 0);
        assert_eq!(timeline_gap_samples(9_000, 10_000, 0), 0);
    }

    #[test]
    fn queue_accounting_handles_consumer_winning_the_send_increment_race() {
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let consumer_counter = queued.clone();
        let consumer = std::thread::spawn(move || debit_queued_samples(&consumer_counter, 512));
        queued.fetch_add(512, std::sync::atomic::Ordering::Release);
        consumer.join().unwrap();
        assert_eq!(queued.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn live_queue_budget_uses_native_rate_for_both_lanes() {
        assert!(LIVE_QUEUE_MAX_SAMPLES >= 96_000 * 2 * 30);
    }

    #[test]
    fn dropped_or_short_live_audio_cannot_be_terminal() {
        assert!(live_checkpoint_complete(60_000, 60_000, 0));
        assert!(!live_checkpoint_complete(60_000, 11, 0));
        assert!(!live_checkpoint_complete(60_000, 60_000, 1));
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    #[test]
    #[ignore = "requires local FluidAudio CoreML assets"]
    fn native_worker_warms_on_its_thread_and_writes_terminal_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let checkpoint = root.path().join("segment.live-transcript.json");
        let status = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            crate::app::LIVE_TRANSCRIPTION_WARMING,
        ));
        let started = std::time::Instant::now();
        let worker = super::LiveTranscriptWorker::start(checkpoint.clone(), 4_250, status.clone())
            .unwrap()
            .expect("local CoreML models are required for this smoke test");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "worker startup must not block capture on model warmup"
        );
        assert_eq!(
            status.load(std::sync::atomic::Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_WARMING
        );
        let sink = worker.sink_for_offset(4_250);
        let samples = vec![0.0; 16_000 * 3];
        sink.queued_samples
            .fetch_add(samples.len() as u64, std::sync::atomic::Ordering::Release);
        sink.sender
            .send(crate::recorder::LiveAudioChunk {
                channel: crate::recorder::LiveAudioChannel::Mic,
                generation: sink.generation,
                session_offset_ms: 4_250,
                sample_rate: 16_000,
                samples,
            })
            .unwrap();

        let mut live_value = None;
        for _ in 0..1_200 {
            if let Ok(bytes) = std::fs::read(&checkpoint) {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if value["terminal"] == false {
                    live_value = Some(value);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let live_value = live_value.expect("incremental checkpoint was not written within 120s");
        assert!(live_value["decoded_until_ms"].as_u64().unwrap() > 4_250);
        assert_eq!(
            status.load(std::sync::atomic::Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_READY
        );

        drop(sink);
        assert!(worker.begin_finish(3_000).complete().unwrap());

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
        assert_eq!(value["terminal"], true);
        assert_eq!(value["decoded_until_ms"], 7_250);
    }
