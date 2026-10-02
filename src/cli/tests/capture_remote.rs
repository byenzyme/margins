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
