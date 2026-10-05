    // Fake-source / fake-ASR coverage for durable catch-up after a long model
    // warmup. The fake recorder follows the segment writer's live-queue rules:
    // a bounded queue, silence substituted for missed spans in 250 ms debt
    // chunks, and real audio only once the debt is paid. Its durable journal
    // resamples the same native stream to 16 kHz, like the meeting runtime.

    use crate::recorder::{LiveAudioChannel, LiveAudioChunk};
    use margins_core::{AsrStreamDecoder, AsrStreamUpdate, TranscriptError};
    use margins_media::timeline::RationalResampler;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingDecoder {
        audio: Vec<f32>,
    }

    impl RecordingDecoder {
        fn update(&self, end_ms: u64) -> AsrStreamUpdate {
            let decoded = end_ms.min(self.audio.len() as u64 * 1_000 / 16_000);
            AsrStreamUpdate {
                committed: Vec::new(),
                hypothesis: Vec::new(),
                decoded_until_ms: decoded,
                committed_until_ms: decoded,
            }
        }
    }

    impl AsrStreamDecoder for RecordingDecoder {
        fn append_audio(&mut self, mono_16k: &[f32]) {
            self.audio.extend_from_slice(mono_16k);
        }

        fn update_until(&mut self, end_ms: u64) -> Result<AsrStreamUpdate, TranscriptError> {
            Ok(self.update(end_ms))
        }

        fn finish_until(&mut self, end_ms: u64) -> Result<AsrStreamUpdate, TranscriptError> {
            Ok(self.update(end_ms))
        }
    }

    #[derive(Default)]
    struct FakeDurableState {
        lanes: BTreeMap<(i64, usize), Vec<f32>>,
        starts: BTreeMap<i64, u64>,
        /// Newest frames per lane not committed yet (runtime batch lag).
        uncommitted_tail: usize,
        /// Frames per lane beyond which audio is permanently unavailable.
        available_limit: Option<usize>,
    }

    #[derive(Clone, Default)]
    struct FakeDurable(Arc<Mutex<FakeDurableState>>);

    impl FakeDurable {
        fn lane(&self, ordinal: i64, index: usize) -> Vec<f32> {
            self.0.lock().unwrap().lanes[&(ordinal, index)].clone()
        }
    }

    impl DurableLiveAudio for FakeDurable {
        fn segments(&mut self) -> Result<Vec<(i64, u64)>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .starts
                .iter()
                .map(|(ordinal, start)| (*ordinal, *start))
                .collect())
        }

        fn read(
            &mut self,
            ordinal: i64,
            channel: LiveAudioChannel,
            from_frame: u64,
            max_frames: usize,
        ) -> Result<Vec<f32>> {
            let state = self.0.lock().unwrap();
            let index = match channel {
                LiveAudioChannel::Mic => 0,
                LiveAudioChannel::System => 1,
            };
            let Some(lane) = state.lanes.get(&(ordinal, index)) else {
                return Ok(Vec::new());
            };
            let visible = lane
                .len()
                .saturating_sub(state.uncommitted_tail)
                .min(state.available_limit.unwrap_or(usize::MAX));
            let from = (from_frame as usize).min(visible);
            let to = from.saturating_add(max_frames).min(visible);
            Ok(lane[from..to].to_vec())
        }
    }

    struct FakeLane {
        channel: LiveAudioChannel,
        rate: u32,
        captured: u64,
        enqueued: u64,
        journal: RationalResampler,
    }

    struct FakeRecorder {
        lanes: [FakeLane; 2],
        generation: u64,
        ordinal: i64,
        session_offset_ms: u64,
        tx: mpsc::Sender<LiveAudioChunk>,
        queued: Arc<AtomicU64>,
        budget: u64,
        dropped: Arc<AtomicU64>,
        durable: FakeDurable,
    }

    impl FakeRecorder {
        fn new(
            tx: mpsc::Sender<LiveAudioChunk>,
            queued: Arc<AtomicU64>,
            budget: u64,
            dropped: Arc<AtomicU64>,
            durable: FakeDurable,
        ) -> Self {
            durable.0.lock().unwrap().starts.insert(0, 0);
            let lane = |channel, rate| FakeLane {
                channel,
                rate,
                captured: 0,
                enqueued: 0,
                journal: RationalResampler::new(rate, 16_000).unwrap(),
            };
            Self {
                lanes: [
                    lane(LiveAudioChannel::Mic, 16_000),
                    lane(LiveAudioChannel::System, 48_000),
                ],
                generation: 1,
                ordinal: 0,
                session_offset_ms: 0,
                tx,
                queued,
                budget,
                dropped,
                durable,
            }
        }

        fn try_send(&self, index: usize, start_frame: u64, synthesized: bool, samples: Vec<f32>) -> bool {
            let count = samples.len() as u64;
            if self.queued.load(Ordering::Acquire) + count > self.budget {
                return false;
            }
            self.queued.fetch_add(count, Ordering::AcqRel);
            self.tx
                .send(LiveAudioChunk {
                    channel: self.lanes[index].channel,
                    generation: self.generation,
                    session_offset_ms: self.session_offset_ms,
                    sample_rate: self.lanes[index].rate,
                    start_frame,
                    synthesized,
                    samples,
                })
                .is_ok()
        }

        /// One 10 ms capture packet on both lanes.
        fn packet(&mut self, packet_index: u64) {
            for index in 0..2 {
                let rate = u64::from(self.lanes[index].rate);
                let frames = rate / 100;
                let base = packet_index * frames;
                let samples = (0..frames)
                    .map(|frame| {
                        let value = (base + frame).wrapping_mul(2_654_435_761) % 1_000;
                        value as f32 / 1_000.0 - 0.5 + index as f32 * 0.01
                    })
                    .collect::<Vec<_>>();
                let durable_start = self.lanes[index].captured;
                self.lanes[index].captured += frames;
                let journaled = self.lanes[index].journal.process(&samples).unwrap();
                self.durable
                    .0
                    .lock()
                    .unwrap()
                    .lanes
                    .entry((self.ordinal, index))
                    .or_default()
                    .extend(journaled);
                let enqueued = self.lanes[index].enqueued;
                if enqueued < durable_start {
                    let debt = (durable_start - enqueued).min(rate / 4);
                    if self.try_send(index, enqueued, true, vec![0.0; debt as usize]) {
                        self.lanes[index].enqueued += debt;
                    }
                }
                if self.lanes[index].enqueued == durable_start
                    && self.try_send(index, durable_start, false, samples)
                {
                    self.lanes[index].enqueued += frames;
                } else {
                    self.dropped.fetch_add(frames, Ordering::AcqRel);
                }
            }
        }
    }

    struct DecodeRun {
        result: Result<()>,
        mic: RecordingDecoder,
        system: RecordingDecoder,
    }

    struct DecodeHarness {
        tx: Option<mpsc::Sender<LiveAudioChunk>>,
        finish_tx: mpsc::Sender<u64>,
        rx: Option<mpsc::Receiver<LiveAudioChunk>>,
        finish_rx: Option<mpsc::Receiver<u64>>,
        queued: Arc<AtomicU64>,
        dropped: Arc<AtomicU64>,
        status: Arc<AtomicU8>,
        unrecovered: Arc<AtomicU64>,
        segments: Arc<Mutex<BTreeMap<u64, i64>>>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        root: tempfile::TempDir,
    }

    impl DecodeHarness {
        fn new() -> Self {
            let (tx, rx) = mpsc::channel();
            let (finish_tx, finish_rx) = mpsc::channel();
            let segments = Arc::new(Mutex::new(BTreeMap::from([(1, 0)])));
            Self {
                tx: Some(tx),
                finish_tx,
                rx: Some(rx),
                finish_rx: Some(finish_rx),
                queued: Arc::new(AtomicU64::new(0)),
                dropped: Arc::new(AtomicU64::new(0)),
                status: Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING)),
                unrecovered: Arc::new(AtomicU64::new(0)),
                segments,
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                root: tempfile::tempdir().unwrap(),
            }
        }

        fn checkpoint(&self) -> std::path::PathBuf {
            self.root.path().join("live.json")
        }

        fn checkpoint_value(&self) -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(self.checkpoint()).unwrap()).unwrap()
        }

        /// The model has finished warming up: start draining the queue.
        fn start_worker(
            &mut self,
            durable: Option<FakeDurable>,
            durable_wait: Duration,
        ) -> std::thread::JoinHandle<DecodeRun> {
            let io = decode::LiveWorkerIo {
                rx: self.rx.take().unwrap(),
                finish_rx: self.finish_rx.take().unwrap(),
                checkpoint: self.checkpoint(),
                offset_ms: 0,
                queued_samples: self.queued.clone(),
                mic_dropped_samples: self.dropped.clone(),
                system_dropped_samples: Arc::new(AtomicU64::new(0)),
                status: self.status.clone(),
                unrecovered_frames: self.unrecovered.clone(),
                segments: self.segments.clone(),
                durable: durable.map(|audio| LiveDurableSource {
                    audio: Box::new(audio),
                    first_ordinal: 0,
                }),
                durable_wait,
                cancel: self.cancel.clone(),
            };
            std::thread::spawn(move || {
                let mut mic = RecordingDecoder::default();
                let mut system = RecordingDecoder::default();
                let result = decode::run_live_worker(&mut mic, &mut system, io);
                DecodeRun {
                    result,
                    mic,
                    system,
                }
            })
        }

        fn finish(&mut self, duration_ms: u64) {
            drop(self.tx.take());
            self.finish_tx.send(duration_ms).unwrap();
        }
    }

    use std::time::Duration;

    /// 20 s captured before the model is ready (queue holds 2 s), 10 s more
    /// while the worker catches up, the runtime's newest batch committed only
    /// at stop.
    fn long_warmup_capture(
        harness: &mut DecodeHarness,
        durable: &FakeDurable,
        worker_durable: Option<FakeDurable>,
        durable_wait: Duration,
    ) -> DecodeRun {
        let mut recorder = FakeRecorder::new(
            harness.tx.clone().unwrap(),
            harness.queued.clone(),
            48_000 * 2 * 2,
            harness.dropped.clone(),
            durable.clone(),
        );
        durable.0.lock().unwrap().uncommitted_tail = 5 * 16_000;
        for packet in 0..2_000 {
            recorder.packet(packet);
        }
        assert!(
            harness.dropped.load(Ordering::Acquire) > 0,
            "warmup must overflow the bounded live queue"
        );
        let worker = harness.start_worker(worker_durable, durable_wait);
        for packet in 2_000..3_000 {
            recorder.packet(packet);
            if packet % 100 == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        // Stop: the runtime flushes its final batch.
        durable.0.lock().unwrap().uncommitted_tail = 0;
        drop(recorder);
        harness.finish(30_000);
        worker.join().unwrap()
    }

    #[test]
    fn long_warmup_catches_up_from_durable_audio_to_a_terminal_transcript() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        let run = long_warmup_capture(
            &mut harness,
            &durable,
            Some(durable.clone()),
            Duration::from_secs(10),
        );

        run.result.unwrap();
        // Every captured frame was decoded once, in order: no drop, no overlap.
        assert_eq!(run.mic.audio.len(), 30 * 16_000);
        assert_eq!(run.system.audio.len(), 30 * 16_000);
        // The live resampler interpolates from silence at the first real
        // sample after a recovered span, so allow float rounding there.
        for (index, audio) in [(0, &run.mic.audio), (1, &run.system.audio)] {
            let captured = durable.lane(0, index);
            let worst = audio
                .iter()
                .zip(&captured)
                .map(|(decoded, captured)| (decoded - captured).abs())
                .fold(0.0f32, f32::max);
            assert!(worst <= 1e-6, "lane {index} decode differs from capture by {worst}");
        }
        let value = harness.checkpoint_value();
        assert_eq!(value["terminal"], true);
        assert_eq!(value["live_dropped_samples"], 0);
        assert_eq!(value["decoded_until_ms"], 30_000);
        assert_eq!(value["captured_until_ms"], 30_000);
        assert!(value["live_recovered_frames"].as_u64().unwrap() > 16_000 * 2 * 10);
        assert_eq!(harness.unrecovered.load(Ordering::Acquire), 0);
        assert_eq!(
            harness.status.load(Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_READY
        );
    }

    #[test]
    fn warmup_beyond_durable_availability_stays_incomplete() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        // Only the first three seconds can ever be read back.
        durable.0.lock().unwrap().available_limit = Some(3 * 16_000);
        let run = long_warmup_capture(
            &mut harness,
            &durable,
            Some(durable.clone()),
            Duration::from_millis(2),
        );

        assert!(run.result.unwrap_err().to_string().contains("incomplete"));
        assert!(harness.unrecovered.load(Ordering::Acquire) > 0);
        let value = harness.checkpoint_value();
        assert_eq!(value["terminal"], false);
        assert!(value["live_dropped_samples"].as_u64().unwrap() > 0);
    }

    #[test]
    fn stopped_runtime_writes_spend_one_wait_budget_then_finish_incomplete() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        // The runtime stopped committing after three seconds. Dozens of
        // synthesized spans per lane must not each wait the full budget.
        durable.0.lock().unwrap().available_limit = Some(3 * 16_000);
        let wait = Duration::from_millis(500);
        let started = std::time::Instant::now();
        let run = long_warmup_capture(&mut harness, &durable, Some(durable.clone()), wait);
        let elapsed = started.elapsed();

        assert!(run.result.unwrap_err().to_string().contains("incomplete"));
        assert!(
            elapsed < Duration::from_secs(5),
            "worker took {elapsed:?} with a {wait:?} durable wait"
        );
        assert!(harness.unrecovered.load(Ordering::Acquire) > 0);
        assert_eq!(harness.checkpoint_value()["terminal"], false);
    }

    #[test]
    fn resumed_commits_after_an_early_stall_wait_again_at_the_live_edge() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        let audio = |index: usize| {
            (0..6 * 16_000)
                .map(|frame| ((frame * 7 + index * 3) % 101) as f32 / 101.0 - 0.5)
                .collect::<Vec<f32>>()
        };
        {
            let mut state = durable.0.lock().unwrap();
            state.starts.insert(0, 0);
            state.lanes.insert((0, 0), audio(0));
            state.lanes.insert((0, 1), audio(1));
            // A commit hiccup: only the first second is durable.
            state.available_limit = Some(16_000);
        }
        let tx = harness.tx.clone().unwrap();
        let send_synthesized = |from_s: usize, to_s: usize| {
            for channel in [LiveAudioChannel::Mic, LiveAudioChannel::System] {
                tx.send(LiveAudioChunk {
                    channel,
                    generation: 1,
                    session_offset_ms: 0,
                    sample_rate: 16_000,
                    start_frame: (from_s * 16_000) as u64,
                    synthesized: true,
                    samples: vec![0.0; (to_s - from_s) * 16_000],
                })
                .unwrap();
            }
        };
        let wait = Duration::from_millis(1_000);
        send_synthesized(0, 2);
        let worker = harness.start_worker(Some(durable.clone()), wait);
        // The mic wait expires; the stalled system lane does not wait.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while harness.unrecovered.load(Ordering::Acquire) < 2 * 16_000 {
            assert!(std::time::Instant::now() < deadline, "stall was not recorded");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Commits resume.
        durable.0.lock().unwrap().available_limit = Some(4 * 16_000);
        send_synthesized(2, 4);
        // The newest batch commits shortly after the worker reaches it. A
        // still-stalled worker would not wait and would lose these frames.
        send_synthesized(4, 6);
        std::thread::sleep(Duration::from_millis(300));
        durable.0.lock().unwrap().available_limit = None;
        drop(tx);
        harness.finish(6_000);
        let run = worker.join().unwrap();

        // Only the hiccup itself (one second per lane) is missing.
        assert!(run.result.unwrap_err().to_string().contains("incomplete"));
        assert_eq!(harness.unrecovered.load(Ordering::Acquire), 2 * 16_000);
        for (index, decoded) in [(0, &run.mic.audio), (1, &run.system.audio)] {
            let captured = audio(index);
            assert_eq!(decoded.len(), captured.len());
            assert!(decoded[..16_000] == captured[..16_000]);
            assert!(decoded[16_000..32_000].iter().all(|sample| *sample == 0.0));
            assert!(decoded[32_000..] == captured[32_000..], "lane {index}");
        }
        let value = harness.checkpoint_value();
        assert_eq!(value["live_dropped_samples"], 2 * 16_000);
        assert_eq!(value["live_recovered_frames"], 2 * 5 * 16_000);
    }

    #[test]
    fn queue_drops_without_durable_audio_stay_incomplete() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        let run = long_warmup_capture(&mut harness, &durable, None, Duration::from_millis(2));

        assert!(run.result.unwrap_err().to_string().contains("incomplete"));
        let value = harness.checkpoint_value();
        assert_eq!(value["terminal"], false);
        assert!(
            value["live_dropped_samples"].as_u64().unwrap()
                >= harness.dropped.load(Ordering::Acquire)
        );
    }

    #[test]
    fn cancelled_worker_abandons_its_backlog_without_a_terminal_checkpoint() {
        let mut harness = DecodeHarness::new();
        let tx = harness.tx.clone().unwrap();
        // A long warmup leaves a backlog queued for the decoder.
        for second in 0..20u64 {
            for channel in [LiveAudioChannel::Mic, LiveAudioChannel::System] {
                tx.send(LiveAudioChunk {
                    channel,
                    generation: 1,
                    session_offset_ms: 0,
                    sample_rate: 16_000,
                    start_frame: second * 16_000,
                    synthesized: false,
                    samples: vec![0.1; 16_000],
                })
                .unwrap();
            }
        }
        drop(tx);
        harness.cancel.store(true, Ordering::Release);
        let worker = harness.start_worker(None, Duration::from_millis(2));
        drop(harness.tx.take());
        let run = worker.join().unwrap();

        run.result.unwrap();
        assert!(run.mic.audio.is_empty() && run.system.audio.is_empty());
        assert!(
            !harness.checkpoint().exists(),
            "a cancelled transcript must not publish a checkpoint"
        );
    }

    #[test]
    fn idle_worker_cancelled_after_warmup_exits_cleanly() {
        let mut harness = DecodeHarness::new();
        let worker = harness.start_worker(None, Duration::from_millis(2));
        std::thread::sleep(Duration::from_millis(50));
        // LiveTranscriptWorker::cancel: flag first, then both channels close.
        harness.cancel.store(true, Ordering::Release);
        drop(harness.tx.take());
        harness.finish_tx = mpsc::channel().0;
        let run = worker.join().unwrap();

        run.result
            .expect("a closed queue after cancel is not a missing final duration");
        assert!(!harness.checkpoint().exists());
    }

    #[test]
    fn unseen_segments_and_tails_are_decoded_from_durable_audio_on_the_session_timeline() {
        let mut harness = DecodeHarness::new();
        let durable = FakeDurable::default();
        let tone = |ordinal: i64, index: usize, seconds: usize| {
            vec![0.1 * (ordinal + 1) as f32 + index as f32 * 0.01; seconds * 16_000]
        };
        {
            let mut state = durable.0.lock().unwrap();
            // Ordinal 3 has no durable audio; ordinal 4 follows that hole.
            for (ordinal, start_ms, seconds) in
                [(0, 0, 2), (1, 5_000, 1), (2, 8_000, 1), (4, 10_000, 1)]
            {
                state.starts.insert(ordinal, start_ms);
                for index in 0..2 {
                    state
                        .lanes
                        .insert((ordinal, index), tone(ordinal, index, seconds));
                }
            }
        }
        // Generation 2 (segment 1) and segment 4 never delivered a chunk;
        // segment 0 delivered only its first second.
        harness.segments.lock().unwrap().insert(3, 2);
        let tx = harness.tx.clone().unwrap();
        for (generation, ordinal, offset_ms) in [(1, 0, 0), (3, 2, 8_000)] {
            for (index, channel) in [LiveAudioChannel::Mic, LiveAudioChannel::System]
                .into_iter()
                .enumerate()
            {
                tx.send(LiveAudioChunk {
                    channel,
                    generation,
                    session_offset_ms: offset_ms,
                    sample_rate: 16_000,
                    start_frame: 0,
                    synthesized: false,
                    samples: tone(ordinal, index, 1),
                })
                .unwrap();
            }
        }
        drop(tx);
        let worker = harness.start_worker(Some(durable.clone()), Duration::from_millis(2));
        harness.finish(11_000);
        let run = worker.join().unwrap();

        run.result.unwrap();
        for (index, audio) in [(0, &run.mic.audio), (1, &run.system.audio)] {
            let mut expected = durable.lane(0, index);
            expected.extend(vec![0.0; 3 * 16_000]);
            expected.extend(durable.lane(1, index));
            expected.extend(vec![0.0; 2 * 16_000]);
            expected.extend(durable.lane(2, index));
            expected.extend(vec![0.0; 16_000]);
            expected.extend(durable.lane(4, index));
            assert!(*audio == expected, "lane {index} timeline differs");
        }
        let value = harness.checkpoint_value();
        assert_eq!(value["terminal"], true);
        assert_eq!(value["decoded_until_ms"], 11_000);
    }

    #[test]
    fn sqlite_runtime_reader_walks_variable_length_chunks() {
        use crate::live_asr::DurableLiveAudio as _;
        let root = tempfile::tempdir().unwrap();
        let mut producer = crate::local_runtime::LocalMeetingProducer::reserve(
            root.path(),
            "meeting",
            None,
            chrono::Local::now(),
        )
        .unwrap();
        let wav = root.path().join("meeting_seg0.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        let frames = 12 * 16_000 + 123;
        for frame in 0..frames {
            writer.write_sample((frame % 2_000) as i16).unwrap();
            writer.write_sample(-((frame % 3_000) as i16)).unwrap();
        }
        writer.finalize().unwrap();
        producer.open(0, 1_500).unwrap();
        producer
            .ingest_wav_and_close(
                0,
                &wav,
                1_500,
                margins_meeting_protocol::SegmentCloseReasonV1::Stop,
            )
            .unwrap();

        let mut reader = crate::local_runtime::RuntimeLiveAudioReader::new(root.path(), "meeting");
        assert_eq!(reader.segments().unwrap(), vec![(0, 1_500)]);
        let mut mic = Vec::new();
        loop {
            let block = reader
                .read(0, LiveAudioChannel::Mic, mic.len() as u64, 7_919)
                .unwrap();
            if block.is_empty() {
                break;
            }
            mic.extend(block);
        }
        assert_eq!(mic.len(), frames);
        assert_eq!((mic[1_999] * 32_767.0).round() as i16, 1_999);
        assert_eq!((mic[2_000] * 32_767.0).round() as i16, 0);
        // Seeking backwards restarts the walk instead of failing.
        let system = reader.read(0, LiveAudioChannel::System, 2_999, 2).unwrap();
        assert_eq!((system[0] * 32_767.0).round() as i16, -2_999);
        assert_eq!((system[1] * 32_767.0).round() as i16, 0);
    }
