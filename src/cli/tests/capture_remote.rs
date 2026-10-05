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

    /// One 10 ms chunk whose samples carry its sequence number.
    fn fake_chunk(
        channel: crate::recorder::LiveAudioChannel,
        sequence: u64,
    ) -> crate::recorder::LiveAudioChunk {
        crate::recorder::LiveAudioChunk {
            channel,
            generation: 1,
            session_offset_ms: 0,
            sample_rate: 48_000,
            start_frame: sequence * 480,
            synthesized: false,
            samples: vec![sequence as f32; 480],
        }
    }

    /// Fake devices: from open, a mic and a system chunk every 10 ms through
    /// the real sink, until retired.
    struct FakeRecorder {
        retired: Arc<AtomicBool>,
        emitted: Arc<std::sync::atomic::AtomicU64>,
        thread: std::thread::JoinHandle<()>,
    }

    struct FakeDevices {
        emitted: Arc<std::sync::atomic::AtomicU64>,
        first_sample: Arc<Mutex<Option<std::time::Instant>>>,
        retired_at: Arc<Mutex<Option<std::time::Instant>>>,
    }

    impl FakeDevices {
        fn new() -> Self {
            Self {
                emitted: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                first_sample: Arc::new(Mutex::new(None)),
                retired_at: Arc::new(Mutex::new(None)),
            }
        }

        fn open(
            &self,
            sink: crate::recorder::LiveAudioSink,
        ) -> FakeRecorder {
            use crate::recorder::LiveAudioChannel;
            let retired = Arc::new(AtomicBool::new(false));
            let emitted = self.emitted.clone();
            let first_sample = self.first_sample.clone();
            let thread_retired = retired.clone();
            let thread_emitted = emitted.clone();
            let thread = std::thread::spawn(move || {
                let mut sequence = 0;
                while !thread_retired.load(Ordering::SeqCst) {
                    for channel in [LiveAudioChannel::Mic, LiveAudioChannel::System] {
                        sink.queued_samples.fetch_add(480, Ordering::Relaxed);
                        sink.sender.send(fake_chunk(channel, sequence)).unwrap();
                    }
                    sink.mic_accepted_samples.fetch_add(480, Ordering::Relaxed);
                    first_sample
                        .lock()
                        .unwrap()
                        .get_or_insert_with(std::time::Instant::now);
                    sequence += 1;
                    thread_emitted.store(sequence, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            });
            FakeRecorder {
                retired,
                emitted,
                thread,
            }
        }
    }

    impl RemoteSegmentRecorder for FakeRecorder {
        fn retire_to(self, path: &Path) -> Result<()> {
            self.retired.store(true, Ordering::SeqCst);
            self.thread.join().unwrap();
            std::fs::write(path, format!("{} chunks", self.emitted.load(Ordering::SeqCst)))?;
            Ok(())
        }
    }

    type FakeSegment = OpenedRemoteSegment<FakeRecorder, FakeRemoteSpool>;

    fn open_fake_segment(devices: &FakeDevices) -> Result<FakeSegment> {
        spawn_remote_segment(480 * 2 * 4, |_stop, sink| Ok(devices.open(sink)))
    }

    fn fake_preopened<'a>(
        segment: FakeSegment,
        unsent_dir: &Path,
        reported: &'a Mutex<Vec<std::path::PathBuf>>,
    ) -> PreopenedRemoteSegment<'a, FakeRecorder, FakeRemoteSpool> {
        PreopenedRemoteSegment {
            segment: Some(segment),
            unsent_dir: unsent_dir.to_path_buf(),
            unsent_wav: None,
            report_unsent: Box::new(move |path| reported.lock().unwrap().push(path.to_path_buf())),
        }
    }

    /// Every chunk the fake devices emitted, in capture order, both lanes.
    fn expected_interleaved(
        emitted: u64,
    ) -> Vec<(margins_workflows::remote_workspace::NativeRemoteLane, f32, usize)> {
        use margins_workflows::remote_workspace::NativeRemoteLane;
        (0..emitted)
            .flat_map(|sequence| {
                [
                    (NativeRemoteLane::Microphone, sequence as f32, 480),
                    (NativeRemoteLane::System, sequence as f32, 480),
                ]
            })
            .collect()
    }

    /// Hand the transfer over, retire the devices normally, and join.
    fn finish_fake_segment(
        segment: FakeSegment,
        unsent_dir: &Path,
    ) -> (FakeRemoteSpool, (Option<FakeRemoteSpool>, Result<()>)) {
        let spool = FakeRemoteSpool::default();
        let OpenedRemoteSegment {
            recorder,
            sink,
            handoff,
            worker,
            ..
        } = segment;
        handoff
            .send(RemoteSpoolHandoff {
                transfer: spool.clone(),
                live: None,
            })
            .unwrap();
        drop(handoff);
        if let Some(recorder) = recorder {
            std::thread::sleep(std::time::Duration::from_millis(50));
            recorder.retire_to(&unsent_dir.join("segment.wav")).unwrap();
        }
        drop(sink);
        (spool, worker.join().unwrap())
    }

    /// Wait for a handshake from another thread. The deadline only bounds a
    /// broken implementation; passing never depends on how fast a host is.
    fn wait_for(what: &str, mut ready: impl FnMut() -> Option<String>) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match ready() {
                None => return Ok(()),
                Some(state) if std::time::Instant::now() >= deadline => {
                    bail!("timed out waiting for {what} ({state})")
                }
                Some(_) => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
    }

    #[test]
    fn remote_start_opens_devices_before_reserving_and_uploads_audio_captured_meanwhile() {
        let unsent = tempfile::tempdir().unwrap();
        let reported = Mutex::new(Vec::new());
        let devices = FakeDevices::new();
        let emitted_when_reserving = Mutex::new(None);
        let (mut preopened, setup, early) = start_remote_capture(
            || Ok(fake_preopened(open_fake_segment(&devices)?, unsent.path(), &reported)),
            || {
                // The relay replies only once audio has been captured while
                // the reservation is in flight. Reserving before opening the
                // devices would never see a sample and time out.
                *emitted_when_reserving.lock().unwrap() =
                    Some(devices.emitted.load(Ordering::SeqCst));
                wait_for("audio captured during the reservation", || {
                    let emitted = devices.emitted.load(Ordering::SeqCst);
                    (emitted < 5).then(|| format!("{emitted} chunks captured"))
                })?;
                Ok("session")
            },
            |timeout| {
                std::thread::sleep(timeout);
                None
            },
            |_, _| panic!("no control was sent"),
        )
        .unwrap();
        assert_eq!(setup.unwrap(), "session");
        assert_eq!(early, None);
        assert!(
            devices.first_sample.lock().unwrap().is_some(),
            "devices captured before the session existed"
        );
        let emitted_when_reserving = emitted_when_reserving.lock().unwrap().unwrap();

        let segment = preopened.segment.take().unwrap();
        let queued = segment.sink.queued_samples.clone();
        let (spool, (returned, result)) = finish_fake_segment(segment, unsent.path());
        result.unwrap();
        assert!(returned.is_some());
        let emitted = devices.emitted.load(Ordering::SeqCst);
        assert!(
            emitted >= 5 && emitted >= emitted_when_reserving,
            "captured {emitted} chunks ({emitted_when_reserving} when reserving began)"
        );
        assert_eq!(
            *spool.appended.lock().unwrap(),
            expected_interleaved(emitted),
            "both lanes from Start onward, interleaved in capture order ({emitted} chunks)"
        );
        assert_eq!(queued.load(Ordering::Relaxed), 0);
        drop(preopened);
        assert!(reported.lock().unwrap().is_empty());
    }

    fn control_before_session(control: PreSessionControl) {
        let unsent = tempfile::tempdir().unwrap();
        let reported = Mutex::new(Vec::new());
        let devices = FakeDevices::new();
        let reserve_saw_retirement = AtomicBool::new(false);
        let mut polls_after_control = 0;
        let mut sent = false;
        let (mut preopened, setup, early) = start_remote_capture(
            || Ok(fake_preopened(open_fake_segment(&devices)?, unsent.path(), &reported)),
            || {
                // A stalled relay: the reservation cannot finish until the
                // control has already retired the devices. Queuing the control
                // until after the reservation would time out here.
                wait_for("the control to retire the devices", || {
                    devices
                        .retired_at
                        .lock()
                        .unwrap()
                        .is_none()
                        .then(|| format!("{} chunks captured", devices.emitted.load(Ordering::SeqCst)))
                })?;
                reserve_saw_retirement.store(true, Ordering::SeqCst);
                Ok(())
            },
            |timeout| {
                std::thread::sleep(timeout);
                if sent {
                    polls_after_control += 1;
                    return None;
                }
                // Send the control once some audio has been captured.
                (devices.emitted.load(Ordering::SeqCst) >= 3).then(|| {
                    sent = true;
                    control
                })
            },
            |preopened, _| {
                preopened.retire_early();
                *devices.retired_at.lock().unwrap() = Some(std::time::Instant::now());
            },
        )
        .unwrap();
        setup.unwrap();
        assert!(reserve_saw_retirement.load(Ordering::SeqCst));
        assert_eq!(early, Some(control));
        assert_eq!(polls_after_control, 0, "later controls stay queued for the capture loop");
        let emitted_at_retire = devices.emitted.load(Ordering::SeqCst);
        assert!(emitted_at_retire >= 3, "captured {emitted_at_retire} chunks");

        let wav = preopened.unsent_wav.take().expect("early audio kept as a WAV");
        assert_eq!(
            std::fs::read_to_string(&wav).unwrap(),
            format!("{emitted_at_retire} chunks")
        );
        let segment = preopened.segment.take().unwrap();
        assert!(segment.recorder.is_none());
        let (spool, (_, result)) = finish_fake_segment(segment, unsent.path());
        result.unwrap();
        assert_eq!(
            devices.emitted.load(Ordering::SeqCst),
            emitted_at_retire,
            "devices stopped at {control:?}"
        );
        assert_eq!(
            *spool.appended.lock().unwrap(),
            expected_interleaved(emitted_at_retire),
            "what was captured before {control:?} reaches the session ({emitted_at_retire} chunks)"
        );
    }

    #[test]
    fn remote_stop_before_the_session_exists_retires_devices_at_once() {
        control_before_session(PreSessionControl::Stop);
    }

    #[test]
    fn remote_pause_before_the_session_exists_stops_capturing_at_once() {
        control_before_session(PreSessionControl::Pause);
    }

    #[test]
    fn failed_remote_setup_keeps_captured_audio_as_a_private_unsent_wav() {
        let root = tempfile::tempdir().unwrap();
        let unsent = root.path().join("unsent");
        let reported = Mutex::new(Vec::new());
        let devices = FakeDevices::new();
        let (preopened, setup, _) = start_remote_capture(
            || Ok(fake_preopened(open_fake_segment(&devices)?, &unsent, &reported)),
            || -> Result<()> {
                // Fail only once some audio exists to keep.
                wait_for("audio captured before the failure", || {
                    (devices.emitted.load(Ordering::SeqCst) == 0).then(|| "0 chunks".into())
                })?;
                bail!("capture relay rejected request (502 Bad Gateway)")
            },
            |timeout| {
                std::thread::sleep(timeout);
                None
            },
            |_, _| {},
        )
        .unwrap();
        let error = setup.unwrap_err();
        assert!(error.to_string().contains("502"), "{error:#}");
        drop(preopened);

        let reported = reported.lock().unwrap().clone();
        assert_eq!(reported.len(), 1);
        let wav = &reported[0];
        assert!(wav.starts_with(&unsent));
        let emitted = devices.emitted.load(Ordering::SeqCst);
        assert!(emitted > 0);
        assert_eq!(std::fs::read_to_string(wav).unwrap(), format!("{emitted} chunks"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(wav), 0o600);
            assert_eq!(mode(&unsent), 0o700);
        }
    }

    #[test]
    fn remote_pending_audio_spills_to_disk_without_dropping_or_reordering() {
        use crate::recorder::LiveAudioChannel;
        let (sender, receiver) = mpsc::channel();
        let (handoff, handoff_receiver) = mpsc::channel();
        let queued = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let io = RemoteSpoolWorkerIo {
            receiver,
            handoff: handoff_receiver,
            queued_samples: queued.clone(),
            // Two chunks fit in memory; the rest spill.
            memory_max_samples: 480 * 2,
        };
        let worker = std::thread::spawn(move || run_remote_spool_worker(io));
        for sequence in 0..6 {
            for channel in [LiveAudioChannel::Mic, LiveAudioChannel::System] {
                queued.fetch_add(480, Ordering::Relaxed);
                sender.send(fake_chunk(channel, sequence)).unwrap();
            }
        }
        // The recorder's bounded queue is debited while the session is pending.
        wait_for("pending audio to leave the recorder queue", || {
            let queued = queued.load(Ordering::Relaxed);
            (queued != 0).then(|| format!("{queued} samples queued"))
        })
        .unwrap();
        let spool = FakeRemoteSpool::default();
        handoff
            .send(RemoteSpoolHandoff {
                transfer: spool.clone(),
                live: None,
            })
            .unwrap();
        drop(sender);
        worker.join().unwrap().1.unwrap();
        assert_eq!(*spool.appended.lock().unwrap(), expected_interleaved(6));
    }

    #[test]
    fn remote_capture_without_a_session_ends_when_the_recorder_stops() {
        let (sender, receiver) = mpsc::channel();
        let (handoff, handoff_receiver) = mpsc::channel::<RemoteSpoolHandoff<FakeRemoteSpool>>();
        let queued = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker = std::thread::spawn({
            let queued = queued.clone();
            move || {
                run_remote_spool_worker(RemoteSpoolWorkerIo {
                    receiver,
                    handoff: handoff_receiver,
                    queued_samples: queued,
                    memory_max_samples: REMOTE_PENDING_MEMORY_SAMPLES,
                })
            }
        });
        queued.fetch_add(480, Ordering::Relaxed);
        sender
            .send(fake_chunk(crate::recorder::LiveAudioChannel::Mic, 0))
            .unwrap();
        // Reservation failed: no transfer will ever arrive.
        drop(handoff);
        queued.fetch_add(480, Ordering::Relaxed);
        sender
            .send(fake_chunk(crate::recorder::LiveAudioChannel::Mic, 1))
            .unwrap();
        drop(sender);
        let (returned, result) = worker.join().unwrap();
        result.unwrap();
        assert!(returned.is_none());
        assert_eq!(queued.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn remote_segment_offsets_never_overlap_after_a_slow_device_open() {
        // Start at 0; a fallback open took 400 ms, then the segment captured
        // 1 000 ms of audio and closed at 1 000 ms. The next segment is placed
        // by time since the start request (1 400 ms), never before that close.
        assert_eq!(remote_segment_offset_ms(0, 1_400, Some(1_000)), 1_400);
        // Even a clock that undercounts cannot move a segment backwards.
        assert_eq!(remote_segment_offset_ms(0, 900, Some(1_000)), 1_000);
        assert_eq!(remote_segment_offset_ms(5_000, 200, None), 5_200);
    }

    #[test]
    fn a_retried_start_reuses_only_a_recent_unfinished_reservation() {
        use margins_workflows::remote_workspace::{
            native_create_session_command_at, CaptureReservationIntentV1,
            CaptureReservationRequestV1,
        };
        let start = 1_700_000_600_000u64;
        let intent = |started_at: u64| CaptureReservationIntentV1 {
            schema: "margins.capture-reservation.v1".into(),
            transfer_id: "transfer".into(),
            instance_id: "instance".into(),
            remote_url: "https://example.test".into(),
            workspace_id: "workspace".into(),
            session_id: "session".into(),
            request: CaptureReservationRequestV1::Create {
                command: native_create_session_command_at(
                    "session",
                    "key",
                    Some("Meeting".into()),
                    "test",
                    started_at,
                ),
            },
        };
        // A retry seconds later continues the same session.
        assert!(remote_reservation_reusable(&intent(start - 30_000), start));
        assert_eq!(remote_reservation_started_at_ms(&intent(start - 30_000)), Some(start - 30_000));
        // Ten minutes later it would open with a ten-minute empty stretch.
        assert!(!remote_reservation_reusable(&intent(start - 10 * 60_000), start));
        assert!(remote_reservation_reusable(
            &intent(start - REMOTE_RESERVATION_REUSE_MS),
            start
        ));
    }

    #[test]
    fn remote_session_is_dated_from_the_capture_start() {
        use margins_workflows::remote_workspace::native_create_session_command_at;
        let command = native_create_session_command_at(
            "session",
            "key",
            None,
            "test",
            1_700_000_000_000,
        );
        let margins_meeting_protocol::ClientMessageBodyV1::CreateSession(create) = &command.body
        else {
            panic!("expected a create command");
        };
        assert_eq!(create.started_at_unix_ms.0, 1_700_000_000_000);
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
