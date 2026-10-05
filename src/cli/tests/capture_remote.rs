    /// In-memory stand-in for the durable remote transfer.
    #[derive(Clone, Default)]
    struct FakeRemoteSpool {
        appended: Arc<Mutex<Vec<(margins_workflows::remote_workspace::NativeRemoteLane, f32, usize)>>>,
    }

    impl RemoteAudioSpool for FakeRemoteSpool {
        fn append_lane(
            &mut self,
            lane: margins_workflows::remote_workspace::NativeRemoteLane,
            _sample_rate: u32,
            samples: &[f32],
        ) -> Result<()> {
            self.appended
                .lock()
                .unwrap()
                .push((lane, samples[0], samples.len()));
            Ok(())
        }
    }

    struct FakeCapture {
        sender: mpsc::Sender<crate::recorder::LiveAudioChunk>,
        queued: Arc<std::sync::atomic::AtomicU64>,
        mic_dropped: Arc<std::sync::atomic::AtomicU64>,
        system_dropped: Arc<std::sync::atomic::AtomicU64>,
        handoff: mpsc::Sender<RemoteSpoolHandoff<FakeRemoteSpool>>,
        worker: std::thread::JoinHandle<(Option<FakeRemoteSpool>, Result<()>)>,
    }

    fn fake_capture(pending_max_samples: u64) -> FakeCapture {
        let (sender, receiver) = mpsc::channel();
        let (handoff, handoff_receiver) = mpsc::channel();
        let queued = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mic_dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let system_dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let io = RemoteSpoolWorkerIo {
            receiver,
            handoff: handoff_receiver,
            queued_samples: queued.clone(),
            mic_dropped_samples: mic_dropped.clone(),
            system_dropped_samples: system_dropped.clone(),
            pending_max_samples,
        };
        FakeCapture {
            sender,
            queued,
            mic_dropped,
            system_dropped,
            handoff,
            worker: std::thread::spawn(move || run_remote_spool_worker(io)),
        }
    }

    /// One 10 ms mic chunk whose samples carry its sequence number.
    fn fake_chunk(sequence: u64) -> crate::recorder::LiveAudioChunk {
        crate::recorder::LiveAudioChunk {
            channel: crate::recorder::LiveAudioChannel::Mic,
            generation: 1,
            session_offset_ms: 0,
            sample_rate: 48_000,
            start_frame: sequence * 480,
            synthesized: false,
            samples: vec![sequence as f32; 480],
        }
    }

    fn capture_chunk(capture: &FakeCapture, sequence: u64) {
        capture.queued.fetch_add(480, Ordering::Relaxed);
        capture.sender.send(fake_chunk(sequence)).unwrap();
    }

    #[test]
    fn remote_start_captures_immediately_and_uploads_audio_buffered_during_reservation() {
        use margins_workflows::remote_workspace::NativeRemoteLane;

        let start = std::time::Instant::now();
        let capture = fake_capture(REMOTE_PENDING_MAX_SAMPLES);
        // Fake recorder: devices open on Start and deliver a chunk every 10 ms.
        let recorder = {
            let sender = capture.sender.clone();
            let queued = capture.queued.clone();
            std::thread::spawn(move || {
                let mut first_sample_at = None;
                for sequence in 0..60u64 {
                    queued.fetch_add(480, Ordering::Relaxed);
                    sender.send(fake_chunk(sequence)).unwrap();
                    first_sample_at.get_or_insert_with(|| start.elapsed());
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                first_sample_at.unwrap()
            })
        };
        // Fake relay: the session reservation takes 300 ms.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let spool = FakeRemoteSpool::default();
        let live = Arc::new(Mutex::new(Vec::new()));
        let live_seen = live.clone();
        capture
            .handoff
            .send(RemoteSpoolHandoff {
                transfer: spool.clone(),
                live: Some(Box::new(move |chunk: &crate::recorder::LiveAudioChunk| {
                    live_seen.lock().unwrap().push(chunk.samples[0]);
                })),
            })
            .unwrap();
        let first_sample_at = recorder.join().unwrap();
        drop(capture.sender);
        let (returned, result) = capture.worker.join().unwrap();
        result.unwrap();
        assert!(returned.is_some());

        assert!(
            first_sample_at < std::time::Duration::from_millis(100),
            "first captured sample arrived {first_sample_at:?} after Start"
        );
        let appended = spool.appended.lock().unwrap().clone();
        let expected: Vec<_> = (0..60u64)
            .map(|sequence| (NativeRemoteLane::Microphone, sequence as f32, 480))
            .collect();
        assert_eq!(appended, expected, "every chunk from Start onward, in order");
        assert_eq!(
            *live.lock().unwrap(),
            (0..60).map(|sequence| sequence as f32).collect::<Vec<_>>()
        );
        assert_eq!(capture.queued.load(Ordering::Relaxed), 0);
        assert_eq!(capture.mic_dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn remote_pending_audio_drains_the_recorder_queue_and_is_bounded() {
        let capture = fake_capture(480 * 3);
        for sequence in 0..5 {
            capture_chunk(&capture, sequence);
        }
        // The recorder's bounded queue is debited while the session is still
        // pending, so the recorder itself never drops for want of a session.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while capture.queued.load(Ordering::Relaxed) != 0 {
            assert!(std::time::Instant::now() < deadline, "pending audio was not drained");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let spool = FakeRemoteSpool::default();
        capture
            .handoff
            .send(RemoteSpoolHandoff {
                transfer: spool.clone(),
                live: None,
            })
            .unwrap();
        drop(capture.sender);
        capture.worker.join().unwrap().1.unwrap();
        let firsts: Vec<f32> = spool.appended.lock().unwrap().iter().map(|entry| entry.1).collect();
        assert_eq!(firsts, vec![0.0, 1.0, 2.0]);
        assert_eq!(capture.mic_dropped.load(Ordering::Relaxed), 480 * 2);
        assert_eq!(capture.system_dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn remote_capture_stopped_before_the_session_exists_still_delivers_its_audio() {
        let capture = fake_capture(REMOTE_PENDING_MAX_SAMPLES);
        capture_chunk(&capture, 0);
        capture_chunk(&capture, 1);
        drop(capture.sender);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let spool = FakeRemoteSpool::default();
        capture
            .handoff
            .send(RemoteSpoolHandoff {
                transfer: spool.clone(),
                live: None,
            })
            .unwrap();
        let (returned, result) = capture.worker.join().unwrap();
        result.unwrap();
        assert!(returned.is_some());
        assert_eq!(spool.appended.lock().unwrap().len(), 2);
    }

    #[test]
    fn remote_capture_without_a_session_ends_when_the_recorder_stops() {
        let capture = fake_capture(REMOTE_PENDING_MAX_SAMPLES);
        capture_chunk(&capture, 0);
        // Reservation failed: no transfer will ever arrive.
        drop(capture.handoff);
        capture.queued.fetch_add(480, Ordering::Relaxed);
        capture.sender.send(fake_chunk(1)).unwrap();
        drop(capture.sender);
        let (returned, result) = capture.worker.join().unwrap();
        result.unwrap();
        assert!(returned.is_none());
        assert_eq!(capture.queued.load(Ordering::Relaxed), 0);
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn untouched_remote_memo_does_not_replace_concurrent_workspace_notes() {
        use margins_meeting_protocol::WorkspaceMemoLineV1;

        let line = |text: &str| WorkspaceMemoLineV1 {
            text: text.into(),
            created_secs: 0.0,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: None,
        };
        let initial = vec![line("Existing note")];
        assert!(!remote_memo_was_edited(&initial, &initial));
        assert!(!remote_memo_was_edited(
            &initial,
            &[line("Existing note"), line("")]
        ));
        assert!(!remote_memo_was_edited(&[], &[line("")]));
        assert!(remote_memo_was_edited(
            &initial,
            &[line("Edited note"), line("")]
        ));
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn remote_live_tee_preserves_generation_and_drops_when_bounded() {
        use crate::recorder::{
            LiveAudioChannel, LiveAudioChunk, LiveAudioSink, LiveGenerationClock,
        };
        let (sender, receiver) = mpsc::channel();
        let queued = Arc::new(AtomicU64::new(0));
        let accepted = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = LiveAudioSink {
            sender,
            generation: 7,
            generation_clock: Arc::new(Mutex::new(LiveGenerationClock {
                generation: 7,
                session_offset_ms: 4_250,
            })),
            mic_accepted_samples: accepted.clone(),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: dropped.clone(),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            queued_samples: queued.clone(),
            queue_max_samples: 3,
        };
        let chunk = LiveAudioChunk {
            channel: LiveAudioChannel::Mic,
            generation: 1,
            session_offset_ms: 4_250,
            sample_rate: 48_000,
            start_frame: 0,
            synthesized: false,
            samples: vec![0.2, 0.3],
        };
        enqueue_remote_live_chunk(&sink, &chunk);
        enqueue_remote_live_chunk(&sink, &chunk);
        let delivered = receiver.try_recv().unwrap();
        assert_eq!(delivered.generation, 7);
        assert_eq!(delivered.session_offset_ms, 4_250);
        assert_eq!(delivered.samples, chunk.samples);
        assert!(receiver.try_recv().is_err());
        assert_eq!(queued.load(Ordering::Acquire), 2);
        assert_eq!(accepted.load(Ordering::Acquire), 2);
        assert_eq!(dropped.load(Ordering::Acquire), 2);
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn remote_recovery_copy_is_private_and_keeps_source() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("recovery.wav");
        std::fs::write(&source, b"RIFF-test-audio").unwrap();
        let directory = temp.path().join("local-audio");
        let copied =
            copy_remote_recovery_for_local_asr(&source, &directory, "transfer-a", 0).unwrap();
        assert_eq!(std::fs::read(&copied).unwrap(), b"RIFF-test-audio");
        assert_eq!(std::fs::read(&source).unwrap(), b"RIFF-test-audio");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn remote_recorder_start_failure_seals_abort_removes_reservation_and_joins_uploader() {
        use margins_meeting_protocol::{ClientMessageBodyV1, SessionFinalizeReasonV1};
        use margins_workflows::remote_workspace::{
            native_create_session_command, CaptureReservationIntentV1, CaptureReservationRequestV1,
            DurableTransferSpool, NativeRemoteTransfer,
        };

        let temp = tempfile::tempdir().unwrap();
        let intent = CaptureReservationIntentV1 {
            schema: "margins.capture-reservation.v1".into(),
            transfer_id: "start-failure".into(),
            instance_id: "instance-a".into(),
            remote_url: "https://example.test".into(),
            workspace_id: "workspace-a".into(),
            session_id: "session-a".into(),
            request: CaptureReservationRequestV1::Create {
                command: native_create_session_command(
                    "session-a",
                    "reserve-start-failure",
                    Some("start failure".into()),
                    "test",
                ),
            },
        }
        .persist(temp.path())
        .unwrap();
        let spool = DurableTransferSpool::create(
            temp.path(),
            "start-failure",
            "instance-a",
            "https://example.test",
            "workspace-a",
            "session-a",
            "producer-secret",
            0,
        )
        .unwrap();
        let transfer = NativeRemoteTransfer::new(spool);
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let (joined_tx, joined_rx) = mpsc::channel();
        let uploader = std::thread::spawn(move || {
            while !worker_done.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            joined_tx.send(()).unwrap();
        });
        let mut reservation = Some(intent);
        let delivered = Arc::new(AtomicBool::new(false));
        let delivered_flag = delivered.clone();
        let errors = cleanup_failed_remote_recorder_start(
            transfer,
            0,
            &mut reservation,
            temp.path(),
            done.as_ref(),
            uploader,
            move |spool| {
                let finalize = spool
                    .manifest()
                    .finalize_command
                    .as_ref()
                    .expect("no-media abort must be durable before delivery");
                let ClientMessageBodyV1::FinalizeSession(finalize) = &finalize.body else {
                    panic!("expected finalize");
                };
                assert_eq!(finalize.reason, SessionFinalizeReasonV1::Error);
                assert_eq!(finalize.ended_at_ms.0, 0);
                delivered_flag.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(reservation.is_none());
        assert!(done.load(Ordering::Acquire));
        joined_rx.try_recv().unwrap();
        assert!(delivered.load(Ordering::Acquire));
        assert!(!temp.path().join("reservations/start-failure.json").exists());
        let reopened = DurableTransferSpool::open(temp.path(), "start-failure", 0).unwrap();
        assert!(reopened.manifest().finalize_command.is_some());
        assert!(reopened.pending_chunks().unwrap().is_empty());
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn remote_stop_finalizes_without_waiting_for_a_warming_live_worker() {
        use margins_workflows::remote_workspace::{
            DurableTransferSpool, NativeRemoteLane, NativeRemoteTransfer,
        };

        let temp = tempfile::tempdir().unwrap();
        let spool = DurableTransferSpool::create(
            temp.path(),
            "stop-latency",
            "instance-a",
            "https://example.test",
            "workspace-a",
            "session-a",
            "producer-secret",
            0,
        )
        .unwrap();
        let mut transfer = NativeRemoteTransfer::new(spool);
        transfer.begin_segment("native-seg".into(), 0).unwrap();
        for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
            transfer.append_f32(lane, 16_000, &vec![0.0; 16_000]).unwrap();
        }
        transfer
            .close_segment(margins_meeting_protocol::SegmentCloseReasonV1::Stop)
            .unwrap();
        let ended = transfer.last_closed_ended_at_ms().unwrap();

        // The reported CoreML warmup outlasted the whole capture; audio sits
        // queued for a decoder that is not ready yet.
        let warmup = std::time::Duration::from_secs(4);
        let worker = LiveTranscriptWorker::start_simulated_warmup(
            Arc::new(std::sync::atomic::AtomicU8::new(
                crate::app::LIVE_TRANSCRIPTION_WARMING,
            )),
            warmup,
        );
        let sink = worker.sink_for_offset(0);
        sink.sender
            .send(crate::recorder::LiveAudioChunk {
                channel: crate::recorder::LiveAudioChannel::Mic,
                generation: sink.generation,
                session_offset_ms: 0,
                sample_rate: 48_000,
                start_frame: 0,
                synthesized: false,
                samples: vec![0.0; 48_000],
            })
            .unwrap();
        drop(sink);
        let publisher_done = Arc::new(AtomicBool::new(false));
        let publisher_flag = publisher_done.clone();
        let publisher = RemoteCheckpointPublisher {
            done: publisher_done,
            join: Some(std::thread::spawn(move || {
                while !publisher_flag.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })),
        };
        let uploader_done = Arc::new(AtomicBool::new(false));
        let uploader_flag = uploader_done.clone();
        let uploader = std::thread::spawn(move || {
            while !uploader_flag.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let retirement = RemoteLiveRetirement::new();

        let stop = std::time::Instant::now();
        let mut delivered = None;
        stop_remote_capture(
            transfer,
            ended,
            Some(worker),
            Some(publisher),
            &retirement,
            &uploader_done,
            uploader,
            |_| Ok(()),
            |spool| {
                assert!(
                    spool.manifest().finalize_command.is_some(),
                    "delivery must carry the sealed finalize"
                );
                delivered = Some(stop.elapsed());
                Ok(())
            },
        )
        .unwrap();
        let delivered = delivered.expect("Stop delivered the sealed session");
        assert!(
            delivered < std::time::Duration::from_secs(1),
            "Stop waited {delivered:?} for the live worker before finalizing"
        );

        // A new capture may not stack a second model load on this one.
        assert!(!retirement.ready_for_new_worker());
        // A one-shot process gives up after its bounded exit wait.
        assert!(!retirement.wait(std::time::Duration::from_millis(100)));
        assert!(retirement.wait(warmup + std::time::Duration::from_secs(2)));
        assert!(retirement.ready_for_new_worker());
        assert!(stop.elapsed() >= warmup - std::time::Duration::from_millis(100));
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn remote_stop_without_a_live_worker_has_nothing_to_retire() {
        assert!(retire_remote_live_transcript(None, None).is_none());
    }

    #[test]
    fn remote_live_checkpoint_failures_name_their_cause() {
        let classify = |message: &str| remote_live_checkpoint_failure(&anyhow::anyhow!("{message}"));
        assert_eq!(
            classify("invalid_request: capture producer is no longer active"),
            (
                "remote_live_checkpoint_skipped",
                "reason=session_finalized".to_string()
            )
        );
        assert_eq!(
            classify("invalid_request: invalid live checkpoint word timeline"),
            (
                "remote_live_checkpoint_upload_failed",
                "category=server code=invalid_request reason=word_timeline".to_string()
            )
        );
        assert_eq!(
            classify("capture relay rejected request (401 Unauthorized)").1,
            "category=relay status=401 Unauthorized"
        );
        assert_eq!(
            classify("Some free-form failure with /private/path").1,
            "category=application details=stderr"
        );
    }
