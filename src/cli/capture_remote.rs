#[cfg(any(test, feature = "audio-capture"))]
fn cleanup_failed_remote_recorder_start<F>(
    mut transfer: margins_workflows::remote_workspace::NativeRemoteTransfer,
    ended_at_ms: u64,
    reservation_intent: &mut Option<
        margins_workflows::remote_workspace::CaptureReservationIntentV1,
    >,
    transfer_dir: &Path,
    uploader_done: &AtomicBool,
    uploader: std::thread::JoinHandle<()>,
    deliver: F,
) -> Vec<String>
where
    F: FnOnce(&mut margins_workflows::remote_workspace::DurableTransferSpool) -> Result<()>,
{
    let mut errors = Vec::new();
    let abort_sealed = match transfer.seal_session(
        ended_at_ms,
        margins_meeting_protocol::SessionFinalizeReasonV1::Error,
    ) {
        Ok(_) => true,
        Err(error) => {
            errors.push(format!("could not persist abort intent: {error}"));
            false
        }
    };
    if let Some(intent) = reservation_intent.take() {
        if let Err(error) = intent.remove(transfer_dir) {
            errors.push(format!("could not dispose capture reservation: {error}"));
        }
    }
    uploader_done.store(true, Ordering::Release);
    if uploader.join().is_err() {
        errors.push("remote delivery worker panicked during abort".into());
    }
    let mut spool = transfer.into_spool();
    if abort_sealed {
        if let Err(error) = deliver(&mut spool) {
            errors.push(format!(
                "abort delivery remains pending in transfer {}: {error}",
                spool.manifest().transfer_id
            ));
        }
    }
    errors
}

/// Audio captured before the remote session exists is kept in memory up to
/// this many samples (20 s of two 48 kHz lanes), then spilled to an unlinked
/// private temp file. Nothing is dropped, so both lanes stay in lockstep with
/// the session clock however long the reservation takes.
#[cfg(any(test, feature = "audio-capture"))]
const REMOTE_PENDING_MEMORY_SAMPLES: u64 = 48_000 * 2 * 20;

/// Where the spool worker appends captured audio. The durable remote transfer
/// in production; an in-memory fake in tests.
#[cfg(any(test, feature = "audio-capture"))]
trait RemoteAudioSpool {
    fn append_lane(
        &mut self,
        lane: margins_workflows::remote_workspace::NativeRemoteLane,
        sample_rate: u32,
        samples: &[f32],
    ) -> Result<()>;
}

#[cfg(any(test, feature = "audio-capture"))]
impl RemoteAudioSpool for margins_workflows::remote_workspace::NativeRemoteTransfer {
    fn append_lane(
        &mut self,
        lane: margins_workflows::remote_workspace::NativeRemoteLane,
        sample_rate: u32,
        samples: &[f32],
    ) -> Result<()> {
        self.append_f32(lane, sample_rate, samples).map(|_| ())
    }
}

/// The transfer a segment's audio belongs to, handed to the spool worker once
/// the segment has begun. For the first segment that is after the session
/// reservation; the worker holds what the recorder captured meanwhile.
#[cfg(any(test, feature = "audio-capture"))]
struct RemoteSpoolHandoff<T> {
    transfer: T,
    live: Option<Box<dyn FnMut(&crate::recorder::LiveAudioChunk) + Send>>,
}

#[cfg(any(test, feature = "audio-capture"))]
struct RemoteSpoolWorkerIo<T> {
    receiver: mpsc::Receiver<crate::recorder::LiveAudioChunk>,
    handoff: mpsc::Receiver<RemoteSpoolHandoff<T>>,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    mic_duration_us: Arc<std::sync::atomic::AtomicU64>,
    memory_max_samples: u64,
}

/// Audio waiting for its transfer, in capture order: memory first, then a
/// private spill file once the memory budget is used.
#[cfg(any(test, feature = "audio-capture"))]
struct PendingRemoteAudio {
    memory: std::collections::VecDeque<crate::recorder::LiveAudioChunk>,
    memory_samples: u64,
    memory_max_samples: u64,
    spill: Option<std::io::BufWriter<std::fs::File>>,
    spilled_chunks: u64,
}

#[cfg(any(test, feature = "audio-capture"))]
impl PendingRemoteAudio {
    fn new(memory_max_samples: u64) -> Self {
        Self {
            memory: std::collections::VecDeque::new(),
            memory_samples: 0,
            memory_max_samples,
            spill: None,
            spilled_chunks: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.memory.is_empty() && self.spilled_chunks == 0
    }

    fn push(&mut self, chunk: crate::recorder::LiveAudioChunk) -> Result<()> {
        let count = chunk.samples.len() as u64;
        // Once spilling, everything later spills too, so order is preserved.
        if self.spill.is_none()
            && self.memory_samples.saturating_add(count) <= self.memory_max_samples
        {
            self.memory_samples += count;
            self.memory.push_back(chunk);
            return Ok(());
        }
        if self.spill.is_none() {
            crate::cli_log::event(
                "remote_capture_buffer_spilling",
                format!("memory_samples={}", self.memory_samples),
            );
            self.spill = Some(std::io::BufWriter::new(
                tempfile::tempfile().context("could not create the pre-session audio spill")?,
            ));
        }
        let spill = self.spill.as_mut().expect("created above");
        let channel = match chunk.channel {
            crate::recorder::LiveAudioChannel::Mic => 0u8,
            crate::recorder::LiveAudioChannel::System => 1u8,
        };
        spill.write_all(&[channel, u8::from(chunk.synthesized)])?;
        for value in [
            chunk.generation,
            chunk.session_offset_ms,
            chunk.start_frame,
            count,
        ] {
            spill.write_all(&value.to_le_bytes())?;
        }
        spill.write_all(&chunk.sample_rate.to_le_bytes())?;
        for sample in &chunk.samples {
            spill.write_all(&sample.to_le_bytes())?;
        }
        self.spilled_chunks += 1;
        Ok(())
    }

    fn drain(
        &mut self,
        mut each: impl FnMut(&crate::recorder::LiveAudioChunk) -> Result<()>,
    ) -> Result<()> {
        use std::io::{Read, Seek};
        while let Some(chunk) = self.memory.pop_front() {
            each(&chunk)?;
        }
        self.memory_samples = 0;
        let Some(spill) = self.spill.take() else {
            return Ok(());
        };
        let mut file = spill
            .into_inner()
            .map_err(|error| anyhow::anyhow!("pre-session audio spill failed: {error}"))?;
        file.rewind()?;
        let mut reader = std::io::BufReader::new(file);
        let read_u64 = |reader: &mut std::io::BufReader<std::fs::File>| -> Result<u64> {
            let mut bytes = [0u8; 8];
            reader.read_exact(&mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        };
        for _ in 0..std::mem::take(&mut self.spilled_chunks) {
            let mut head = [0u8; 2];
            reader.read_exact(&mut head)?;
            let generation = read_u64(&mut reader)?;
            let session_offset_ms = read_u64(&mut reader)?;
            let start_frame = read_u64(&mut reader)?;
            let count = read_u64(&mut reader)? as usize;
            let mut rate = [0u8; 4];
            reader.read_exact(&mut rate)?;
            let mut bytes = vec![0u8; count * 4];
            reader.read_exact(&mut bytes)?;
            each(&crate::recorder::LiveAudioChunk {
                channel: if head[0] == 0 {
                    crate::recorder::LiveAudioChannel::Mic
                } else {
                    crate::recorder::LiveAudioChannel::System
                },
                generation,
                session_offset_ms,
                sample_rate: u32::from_le_bytes(rate),
                start_frame,
                synthesized: head[1] != 0,
                samples: bytes
                    .chunks_exact(4)
                    .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
                    .collect(),
            })?;
        }
        Ok(())
    }
}

/// Drain the recorder's queue for one segment. Until the transfer arrives,
/// chunks are held (and debited from the recorder's bounded queue, so the
/// recorder never drops for lack of a session); on handoff they are appended
/// first, in capture order. Returns the transfer, if one was handed over.
#[cfg(any(test, feature = "audio-capture"))]
fn run_remote_spool_worker<T: RemoteAudioSpool>(
    io: RemoteSpoolWorkerIo<T>,
) -> (Option<T>, Result<()>) {
    use crate::recorder::{LiveAudioChannel, LiveAudioChunk};
    use margins_workflows::remote_workspace::NativeRemoteLane;

    let RemoteSpoolWorkerIo {
        receiver,
        handoff,
        queued_samples,
        mic_duration_us,
        memory_max_samples,
    } = io;
    fn append<T: RemoteAudioSpool>(
        target: &mut RemoteSpoolHandoff<T>,
        chunk: &LiveAudioChunk,
    ) -> Result<()> {
        let lane = match chunk.channel {
            LiveAudioChannel::Mic => NativeRemoteLane::Microphone,
            LiveAudioChannel::System => NativeRemoteLane::System,
        };
        target
            .transfer
            .append_lane(lane, chunk.sample_rate, &chunk.samples)?;
        if let Some(live) = target.live.as_mut() {
            live(chunk);
        }
        Ok(())
    }
    fn accept<T: RemoteAudioSpool>(
        target: &mut Option<RemoteSpoolHandoff<T>>,
        next: RemoteSpoolHandoff<T>,
        pending: &mut PendingRemoteAudio,
    ) -> Result<()> {
        if !pending.is_empty() {
            crate::cli_log::event(
                "remote_capture_buffer_flushed",
                format!(
                    "memory_samples={} spilled_chunks={}",
                    pending.memory_samples, pending.spilled_chunks
                ),
            );
        }
        let target = target.insert(next);
        pending.drain(|chunk| append(target, chunk))
    }
    let mut handoff = Some(handoff);
    let mut target: Option<RemoteSpoolHandoff<T>> = None;
    let mut pending = PendingRemoteAudio::new(memory_max_samples);
    let result = (|| -> Result<()> {
        loop {
            if target.is_none() {
                match handoff.as_ref().map(|handoff| handoff.try_recv()) {
                    Some(Ok(next)) => {
                        handoff = None;
                        accept(&mut target, next, &mut pending)?;
                    }
                    Some(Err(mpsc::TryRecvError::Disconnected)) => {
                        // The session never began; this audio has no transfer.
                        handoff = None;
                        pending = PendingRemoteAudio::new(memory_max_samples);
                    }
                    Some(Err(mpsc::TryRecvError::Empty)) | None => {}
                }
            }
            let chunk = if target.is_none() && handoff.is_some() {
                match receiver.recv_timeout(std::time::Duration::from_millis(20)) {
                    Ok(chunk) => chunk,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // Capture ended (Stop or Pause) before the session
                        // existed. Wait for the transfer so the audio lands.
                        if let Some(next) = handoff.take().and_then(|handoff| handoff.recv().ok()) {
                            accept(&mut target, next, &mut pending)?;
                        }
                        return Ok(());
                    }
                }
            } else {
                match receiver.recv() {
                    Ok(chunk) => chunk,
                    Err(_) => return Ok(()),
                }
            };
            let count = chunk.samples.len() as u64;
            let mic_duration = (chunk.channel == LiveAudioChannel::Mic && chunk.sample_rate != 0)
                .then(|| count.saturating_mul(1_000_000) / u64::from(chunk.sample_rate));
            let stored = match target.as_mut() {
                Some(target) => append(target, &chunk),
                None if handoff.is_some() => pending.push(chunk),
                None => Ok(()),
            };
            queued_samples.fetch_sub(count, Ordering::Relaxed);
            stored?;
            // Count each captured chunk as it first leaves the recorder queue.
            // Replaying pending audio into the transfer/live tee must not move
            // the clock, and live ASR may be absent or may drop this chunk.
            if let Some(duration) = mic_duration {
                mic_duration_us.fetch_add(duration, Ordering::Relaxed);
            }
        }
    })();
    (target.map(|target| target.transfer), result)
}

/// A retried start reuses an unfinished reservation only this soon after it
/// was made: the session is dated from the first attempt, so the gap between
/// attempts becomes an empty stretch at the start of the recording.
#[cfg(any(test, feature = "audio-capture"))]
const REMOTE_RESERVATION_REUSE_MS: u64 = 2 * 60 * 1_000;

#[cfg(any(test, feature = "audio-capture"))]
fn remote_reservation_started_at_ms(
    intent: &margins_workflows::remote_workspace::CaptureReservationIntentV1,
) -> Option<u64> {
    match &intent.request {
        margins_workflows::remote_workspace::CaptureReservationRequestV1::Create { command } => {
            match &command.body {
                margins_meeting_protocol::ClientMessageBodyV1::CreateSession(create) => {
                    Some(create.started_at_unix_ms.0)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(any(test, feature = "audio-capture"))]
fn remote_reservation_reusable(
    intent: &margins_workflows::remote_workspace::CaptureReservationIntentV1,
    start_unix_ms: u64,
) -> bool {
    remote_reservation_started_at_ms(intent)
        .is_some_and(|started| start_unix_ms.saturating_sub(started) <= REMOTE_RESERVATION_REUSE_MS)
}

/// A control the user sent before the remote session existed.
#[cfg(any(test, feature = "audio-capture"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreSessionControl {
    Pause,
    Stop,
}

/// The start path's ordering. Open the devices first, then reserve the
/// session on a scoped thread while this thread keeps answering Pause and
/// Stop: the first such control retires capture at once, however long the
/// reservation takes. Later controls stay queued for the capture loop.
#[cfg(any(test, feature = "audio-capture"))]
fn start_remote_capture<O, P: Send>(
    open: impl FnOnce() -> Result<O>,
    reserve: impl FnOnce() -> Result<P> + Send,
    mut poll: impl FnMut(std::time::Duration) -> Option<PreSessionControl>,
    mut retire: impl FnMut(&mut O, PreSessionControl),
) -> Result<(O, Result<P>, Option<PreSessionControl>)> {
    let mut opened = open()?;
    let (setup, early) = std::thread::scope(|scope| {
        let reserving = match std::thread::Builder::new()
            .name("margins-remote-reserve".into())
            .spawn_scoped(scope, reserve)
        {
            Ok(reserving) => reserving,
            Err(error) => return (Err(anyhow::Error::from(error)), None),
        };
        let mut early = None;
        while !reserving.is_finished() {
            if let Some(control) = poll(std::time::Duration::from_millis(20)) {
                retire(&mut opened, control);
                early = Some(control);
                break;
            }
        }
        let setup = reserving
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("remote session setup panicked")));
        (setup, early)
    });
    Ok((opened, setup, early))
}

/// Next segment's position on the session clock. The clock is anchored at
/// the start request, before the devices opened, so elapsed time never
/// undercounts captured audio; the last closed segment is still a floor.
#[cfg(any(test, feature = "audio-capture"))]
fn remote_segment_offset_ms(
    initial_ms: u64,
    since_start_ms: u64,
    last_closed_ms: Option<u64>,
) -> u64 {
    initial_ms
        .saturating_add(since_start_ms)
        .max(last_closed_ms.unwrap_or(0))
}

/// Recorder operations the start path needs; RecorderHandle in production.
#[cfg(any(test, feature = "audio-capture"))]
trait RemoteSegmentRecorder {
    /// Retire the devices and write what they captured as a WAV.
    fn retire_to(self, path: &Path) -> Result<()>;
}

#[cfg(feature = "audio-capture")]
impl RemoteSegmentRecorder for crate::recorder::RecorderHandle {
    fn retire_to(self, path: &Path) -> Result<()> {
        self.stop_and_write(&path.to_string_lossy()).map(|_| ())
    }
}

/// One segment's open recorder and the spool worker draining it. The recorder
/// is `None` once Pause or Stop retired it before the session existed.
#[cfg(any(test, feature = "audio-capture"))]
struct OpenedRemoteSegment<R, T> {
    recorder: Option<R>,
    stop: Arc<AtomicBool>,
    sink: crate::recorder::LiveAudioSink,
    mic_duration_us: Arc<std::sync::atomic::AtomicU64>,
    handoff: mpsc::Sender<RemoteSpoolHandoff<T>>,
    worker: std::thread::JoinHandle<(Option<T>, Result<()>)>,
}

#[cfg(any(test, feature = "audio-capture"))]
fn spawn_remote_segment<R, T: RemoteAudioSpool + Send + 'static>(
    queue_max_samples: u64,
    open: impl FnOnce(Arc<AtomicBool>, crate::recorder::LiveAudioSink) -> Result<R>,
) -> Result<OpenedRemoteSegment<R, T>> {
    use std::sync::atomic::AtomicU64;
    let stop = Arc::new(AtomicBool::new(false));
    let queued_samples = Arc::new(AtomicU64::new(0));
    let mic_duration_us = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel();
    let sink = crate::recorder::LiveAudioSink {
        sender,
        generation: 1,
        generation_clock: Arc::new(std::sync::Mutex::new(
            crate::recorder::LiveGenerationClock {
                generation: 1,
                session_offset_ms: 0,
            },
        )),
        mic_accepted_samples: Arc::new(AtomicU64::new(0)),
        system_accepted_samples: Arc::new(AtomicU64::new(0)),
        mic_dropped_samples: Arc::new(AtomicU64::new(0)),
        system_dropped_samples: Arc::new(AtomicU64::new(0)),
        queued_samples: queued_samples.clone(),
        queue_max_samples,
    };
    // Start draining before the devices open so the first callback already
    // has a consumer.
    let (handoff, handoff_receiver) = mpsc::channel();
    let worker_io = RemoteSpoolWorkerIo {
        receiver,
        handoff: handoff_receiver,
        queued_samples,
        mic_duration_us: mic_duration_us.clone(),
        memory_max_samples: REMOTE_PENDING_MEMORY_SAMPLES,
    };
    let worker_stop = stop.clone();
    let worker = std::thread::Builder::new()
        .name("margins-remote-spool".into())
        .spawn(move || {
            let (transfer, result) = run_remote_spool_worker(worker_io);
            if result.is_err() {
                worker_stop.store(true, Ordering::SeqCst);
            }
            (transfer, result)
        })?;
    // On failure, dropping the sink and handoff ends the worker.
    let recorder = open(stop.clone(), sink.clone())?;
    Ok(OpenedRemoteSegment {
        recorder: Some(recorder),
        stop,
        sink,
        mic_duration_us,
        handoff,
        worker,
    })
}

#[cfg(feature = "audio-capture")]
type NativeRemoteSegment = OpenedRemoteSegment<
    crate::recorder::RecorderHandle,
    margins_workflows::remote_workspace::NativeRemoteTransfer,
>;

#[cfg(feature = "audio-capture")]
fn open_remote_segment(
    saved: Option<&audio_preferences::InputPreference>,
    selected: Option<&crate::recorder::SelectedInputDevice>,
) -> Result<(NativeRemoteSegment, bool, Option<String>)> {
    use margins_workflows::remote_workspace::NATIVE_REMOTE_RATE_HZ;
    let mut outcome = None;
    let segment = spawn_remote_segment(u64::from(NATIVE_REMOTE_RATE_HZ) * 10 * 2, |stop, sink| {
        let (recorder, fell_back, note) =
            audio_preferences::open_with_saved_fallback(saved, selected, |choice| {
                crate::recorder::RecorderHandle::start_with_selected_audio(
                    stop.clone(),
                    choice,
                    Some(sink.clone()),
                )
            })?;
        outcome = Some((fell_back, note));
        Ok(recorder)
    })?;
    let (fell_back, note) = outcome.expect("set by a successful open");
    Ok((segment, fell_back, note))
}

/// Durable, private home for audio captured before a remote session existed
/// when that session could not be created. Import it with `margins transcribe`.
#[cfg(feature = "audio-capture")]
fn remote_unsent_audio_dir() -> Result<std::path::PathBuf> {
    Ok(margins_workflows::workspace::margins_home()?.join("unsent"))
}

/// Retire the recorder into a new owner-only WAV under `directory`.
#[cfg(any(test, feature = "audio-capture"))]
fn retire_into_unsent<R: RemoteSegmentRecorder>(
    recorder: R,
    directory: &Path,
) -> Option<std::path::PathBuf> {
    let path = (|| -> Result<std::path::PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!(
            "unsent-{}-{}.wav",
            chrono::Local::now().format("%Y-%m-%d-%H-%M-%S"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            options.mode(0o600);
        }
        // The WAV writer truncates this file and keeps its owner-only mode.
        options.open(&path)?;
        Ok(path)
    })();
    match path {
        Ok(path) => match recorder.retire_to(&path) {
            Ok(()) => Some(path),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                crate::cli_log::event(
                    "remote_unsent_audio_failed",
                    crate::cli_log::error_summary(&error),
                );
                None
            }
        },
        Err(error) => {
            crate::cli_log::event(
                "remote_unsent_audio_failed",
                crate::cli_log::error_summary(&error),
            );
            None
        }
    }
}

/// The first segment opens before the remote session is reserved. Pause or
/// Stop before the session exists retires it into an unsent WAV; if setup
/// then fails (or fails with the devices still open), dropping this keeps
/// that WAV durable and reports where it is.
#[cfg(any(test, feature = "audio-capture"))]
struct PreopenedRemoteSegment<'a, R: RemoteSegmentRecorder, T> {
    segment: Option<OpenedRemoteSegment<R, T>>,
    unsent_dir: std::path::PathBuf,
    unsent_wav: Option<std::path::PathBuf>,
    report_unsent: Box<dyn Fn(&Path) + 'a>,
}

#[cfg(any(test, feature = "audio-capture"))]
impl<R: RemoteSegmentRecorder, T> PreopenedRemoteSegment<'_, R, T> {
    fn retire_early(&mut self) {
        let recorder = self
            .segment
            .as_mut()
            .and_then(|segment| segment.recorder.take());
        if let Some(recorder) = recorder {
            self.unsent_wav = retire_into_unsent(recorder, &self.unsent_dir);
        }
    }
}

#[cfg(any(test, feature = "audio-capture"))]
impl<R: RemoteSegmentRecorder, T> Drop for PreopenedRemoteSegment<'_, R, T> {
    fn drop(&mut self) {
        let Some(segment) = self.segment.take() else {
            // The capture loop owns the devices; an early WAV still waiting
            // for its transfer is reported if that never happened.
            if let Some(path) = self.unsent_wav.take() {
                crate::cli_log::event("remote_capture_aborted_before_upload", "audio=kept");
                (self.report_unsent)(&path);
            }
            return;
        };
        let OpenedRemoteSegment {
            recorder,
            stop,
            sink,
            mic_duration_us: _,
            handoff,
            worker,
        } = segment;
        stop.store(true, Ordering::SeqCst);
        // Without a handoff the worker discards; closing every sender ends it.
        drop(handoff);
        if let Some(recorder) = recorder {
            self.unsent_wav = retire_into_unsent(recorder, &self.unsent_dir);
        }
        drop(sink);
        let _ = worker.join();
        match self.unsent_wav.take() {
            Some(path) => {
                crate::cli_log::event("remote_capture_aborted_before_session", "audio=kept");
                (self.report_unsent)(&path);
            }
            None => {
                crate::cli_log::event("remote_capture_aborted_before_session", "audio=unavailable")
            }
        }
    }
}

#[cfg(feature = "audio-capture")]
fn copy_remote_recovery_for_local_asr(
    source: &Path,
    directory: &Path,
    transfer_id: &str,
    segment_index: usize,
) -> Result<std::path::PathBuf> {
    use std::fs::OpenOptions;
    use std::io::copy;

    if !directory.is_absolute() {
        bail!("local audio directory must be absolute");
    }
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let name = format!("{transfer_id}-seg{segment_index}.wav");
        let destination = directory.join(name);
        let temporary = destination.with_extension("wav.partial");
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        copy(&mut input, &mut output)?;
        output.sync_all()?;
        std::fs::rename(&temporary, &destination)?;
        return Ok(destination);
    }
    #[cfg(not(unix))]
    {
        let destination = directory.join(format!("{transfer_id}-seg{segment_index}.wav"));
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        copy(&mut input, &mut output)?;
        output.sync_all()?;
        Ok(destination)
    }
}

#[cfg(feature = "audio-capture")]
fn run_remote_native_capture(
    remote: &str,
    workspace_id: &str,
    command: &Option<Command>,
    controller: Option<native_bridge::CaptureController>,
    local_audio_dir: Option<&Path>,
    mic_device_name: Option<&str>,
    prepared_connection: Option<margins_workflows::remote_workspace::RemoteConnection>,
    permissions_verified: bool,
) -> Result<()> {
    use margins_meeting_protocol::{
        SegmentCloseReasonV1, WorkspaceAttachV1, WorkspaceMemoLineV1, WorkspaceMemoReplaceV1,
    };
    use margins_workflows::remote_workspace::{
        deliver_available, deliver_transfer, list_transfers, native_create_session_command_at,
        pending_capture_reservations, transfer_root, validate_native_opus_capture_lanes,
        CaptureReservationIntentV1, CaptureReservationRequestV1, DurableTransferSpool,
        NativeRemoteTransfer, RemoteConnection, NATIVE_REMOTE_RATE_HZ,
    };

    if !permissions_verified {
        ensure_capture_permissions(&NativeCapturePermissionSource)?;
    }
    let mut selected_device = mic_device_name
        .map(|name| {
            crate::recorder::list_input_devices()
                .into_iter()
                .find(|(available, _)| available == name)
                .map(|(name, device)| crate::recorder::SelectedInputDevice {
                    uid: crate::recorder::input_device_uid_at(&name, 0),
                    name,
                    occurrence: 0,
                    device,
                })
                .with_context(|| format!("microphone input device not found: {name}"))
        })
        .transpose()?;
    let mut preference_note = None;
    // The Menu owns its input selection. Its unpinned mode means the system
    // default, regardless of a separate TUI preference.
    let preference = match if controller.is_some() {
        Ok(None)
    } else {
        audio_preferences::load()
    } {
        Ok(preference) => preference,
        Err(error) => {
            preference_note = Some(if mic_device_name.is_some() {
                format!("Could not read saved mic choice ({error}); using requested input")
            } else {
                format!("Could not read saved mic choice ({error}); using system default")
            });
            None
        }
    };
    if selected_device.is_none() {
        let (preferred_device, note) = audio_preferences::resolve(preference.as_ref())?;
        selected_device = preferred_device;
        preference_note = note.or(preference_note);
    }
    let mut saved_choice_active =
        controller.is_none() && mic_device_name.is_none() && selected_device.is_some();
    let mut pending_selection = false;
    // Open the devices first and reserve the session while they capture.
    // Reserving can take seconds through a relay; speech in that window
    // belongs in the recording. One anchor, the start request, dates the
    // session, its memo clock and its first audio.
    let start_requested = std::time::Instant::now();
    let start_requested_local = Local::now();
    let start_unix_ms = start_requested_local.timestamp_millis().max(0) as u64;
    let unsent_dir = remote_unsent_audio_dir()?;
    struct RemoteSessionSetup {
        connection: RemoteConnection,
        transfer_dir: std::path::PathBuf,
        reservation_intent: Option<CaptureReservationIntentV1>,
        capture_lease: margins_workflows::remote_workspace::TransferCaptureLease,
        transfer: NativeRemoteTransfer,
        session_id: String,
        initial_offset_ms: u64,
        initial_memo: margins_meeting_protocol::WorkspaceMemoV1,
    }
    let reserve = move || -> Result<RemoteSessionSetup> {
        let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
        let connection = match prepared_connection {
            Some(connection) => connection,
            None => RemoteConnection::connect(remote, workspace_id, token.as_deref())?,
        };
        let capabilities = &connection.capabilities;
        let opus_supported = capabilities.capture_formats.iter().any(|format| {
            format.codec == margins_meeting_protocol::AudioCodecV1::Opus
                && format.container == margins_meeting_protocol::AudioContainerV1::PacketStream
                && format.sample_rate_hz == NATIVE_REMOTE_RATE_HZ
                && format.channel_count == 1
        });
        if !opus_supported {
            bail!("remote instance does not advertise native mono Opus packet-stream capture");
        }

        let transfer_dir = transfer_root()?;
        std::fs::create_dir_all(&transfer_dir)?;
        let reservation_intent;
        let (spool, session_id, initial_offset_ms) = match command {
            Some(Command::New { title }) => {
                let pending = pending_capture_reservations(&transfer_dir)?
                    .into_iter()
                    .filter(|intent| {
                        intent.instance_id == capabilities.instance_id.as_ref()
                            && intent.remote_url == remote
                            && intent.workspace_id == workspace_id
                            && matches!(&intent.request, CaptureReservationRequestV1::Create { command }
                                if matches!(&command.body, margins_meeting_protocol::ClientMessageBodyV1::CreateSession(create) if &create.title == title))
                    })
                    .filter(|intent| {
                        // An old attempt's session is dated from that attempt;
                        // reusing it would open this recording with a long
                        // empty stretch. Leave it on disk for inspection.
                        let reusable = remote_reservation_reusable(intent, start_unix_ms);
                        if !reusable {
                            crate::cli_log::event(
                                "remote_reservation_not_reused",
                                format!("transfer={} reason=stale", intent.transfer_id),
                            );
                        }
                        reusable
                    })
                    .collect::<Vec<_>>();
                if pending.len() > 1 {
                    bail!("multiple unfinished remote session reservations match this command; inspect the transfer directory before retrying");
                }
                let intent = if let Some(intent) = pending.into_iter().next() {
                    intent
                } else {
                    let session_id = format!(
                        "remote-{}-{}",
                        Local::now().format("%Y-%m-%d-%H-%M-%S"),
                        &uuid::Uuid::new_v4().simple().to_string()[..8]
                    );
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    let command = native_create_session_command_at(
                        &session_id,
                        &format!("reserve-{transfer_id}"),
                        title.clone(),
                        "margins-native-cli",
                        start_unix_ms,
                    );
                    CaptureReservationIntentV1 {
                        schema: "margins.capture-reservation.v1".into(),
                        transfer_id,
                        instance_id: capabilities.instance_id.as_ref().into(),
                        remote_url: remote.into(),
                        workspace_id: workspace_id.into(),
                        session_id,
                        request: CaptureReservationRequestV1::Create { command },
                    }
                    .persist(&transfer_dir)?
                };
                let spool = remote_spool_from_reservation(
                    &connection,
                    &capabilities,
                    &transfer_dir,
                    remote,
                    workspace_id,
                    &intent,
                )?;
                let session_id = intent.session_id.clone();
                // A reused intent keeps the start time it was created with;
                // place this audio where it falls on that session clock.
                let anchor_offset_ms = remote_reservation_started_at_ms(&intent)
                    .map_or(0, |started| start_unix_ms.saturating_sub(started));
                reservation_intent = Some(intent);
                (spool, session_id, anchor_offset_ms)
            }
            Some(Command::Attach { session }) => {
                let requested = match session {
                    Some(session) => session.clone(),
                    None => connection
                        .client
                        .current()?
                        .context(
                            "no current remote capture is selected for this client and Workspace",
                        )?
                        .0,
                };
                let target_summary = connection
                    .client
                    .session_summary(&requested)?
                    .context("remote session was not found in the selected Workspace")?;
                validate_native_opus_capture_lanes(&target_summary.capture_lanes)?;
                let mut found = None;
                for transfer_id in list_transfers(&transfer_dir)? {
                    let candidate = DurableTransferSpool::open(
                        &transfer_dir,
                        &transfer_id,
                        capabilities.limits.spool_reserve_bytes,
                    )?;
                    let manifest = candidate.manifest();
                    if manifest.instance_id == capabilities.instance_id.as_ref()
                        && manifest.workspace_id == workspace_id
                        && manifest.session_id == requested
                    {
                        found = Some(candidate);
                        break;
                    }
                }
                if let Some(spool) = found {
                    if spool.manifest().finalize_command.is_some() {
                        bail!(
                            "remote transfer is already sealed; retry delivery instead of attaching audio"
                        );
                    }
                    reservation_intent = pending_capture_reservations(&transfer_dir)?
                        .into_iter()
                        .find(|intent| intent.transfer_id == spool.manifest().transfer_id);
                    let offset = spool
                        .manifest()
                        .close_commands
                        .iter()
                        .filter_map(|command| match &command.body {
                            margins_meeting_protocol::ClientMessageBodyV1::CloseSegment(close) => {
                                Some(close.ended_at_ms.0)
                            }
                            _ => None,
                        })
                        .max()
                        .unwrap_or(0);
                    (spool, requested, offset)
                } else {
                    let pending = pending_capture_reservations(&transfer_dir)?
                        .into_iter()
                        .filter(|intent| {
                            intent.instance_id == capabilities.instance_id.as_ref()
                                && intent.remote_url == remote
                                && intent.workspace_id == workspace_id
                                && intent.session_id == requested
                                && matches!(
                                    intent.request,
                                    CaptureReservationRequestV1::Attach { .. }
                                )
                        })
                        .collect::<Vec<_>>();
                    if pending.len() > 1 {
                        bail!("multiple unfinished attach reservations match this session; inspect the transfer directory before retrying");
                    }
                    if let Some(intent) = pending.into_iter().next() {
                        let spool = remote_spool_from_reservation(
                            &connection,
                            &capabilities,
                            &transfer_dir,
                            remote,
                            workspace_id,
                            &intent,
                        )?;
                        reservation_intent = Some(intent);
                        let offset = match &reservation_intent.as_ref().unwrap().request {
                            CaptureReservationRequestV1::Attach { request } => {
                                request.started_at_ms.0
                            }
                            _ => unreachable!(),
                        };
                        (spool, requested, offset)
                    } else {
                        if !target_summary.input_finalized {
                            bail!(
                                "remote session has an active producer; recover it from its owning client"
                            );
                        }
                        let offset = target_summary
                            .capture_duration_ms
                            .context("remote session lacks a durable capture boundary")?
                            .0;
                        let transfer_id = uuid::Uuid::new_v4().to_string();
                        let attach =
                            WorkspaceAttachV1 {
                                request_id: uuid::Uuid::new_v4().to_string(),
                                prior_finalize_message_id: target_summary
                                    .capture_finalize_message_id
                                    .context("remote session lacks a durable finalize identity")?,
                                requested_at_unix_ms: margins_meeting_protocol::UnixMillis(
                                    Local::now().timestamp_millis().max(0) as u64,
                                ),
                                started_at_ms: margins_meeting_protocol::SessionMillis(offset),
                            };
                        let intent = CaptureReservationIntentV1 {
                            schema: "margins.capture-reservation.v1".into(),
                            transfer_id,
                            instance_id: capabilities.instance_id.as_ref().into(),
                            remote_url: remote.into(),
                            workspace_id: workspace_id.into(),
                            session_id: requested.clone(),
                            request: CaptureReservationRequestV1::Attach { request: attach },
                        }
                        .persist(&transfer_dir)?;
                        let spool = remote_spool_from_reservation(
                            &connection,
                            &capabilities,
                            &transfer_dir,
                            remote,
                            workspace_id,
                            &intent,
                        )?;
                        reservation_intent = Some(intent);
                        (spool, requested, offset)
                    }
                }
            }
            _ => bail!("remote native capture requires new or attach"),
        };
        let capture_lease = spool.acquire_capture_lease()?;

        // Recovery establishes the only truthful media boundary for the next
        // generation. Do this before constructing the memo clock or capture origin;
        // otherwise a recovered interrupted segment can overlap the new one.
        let mut transfer = NativeRemoteTransfer::new(spool);
        transfer.recover_interrupted_segment(SegmentCloseReasonV1::Error)?;
        let initial_offset_ms = transfer
            .last_closed_ended_at_ms()
            .unwrap_or(initial_offset_ms)
            .max(initial_offset_ms);
        let initial_memo = connection.client.memo(&session_id)?;
        Ok(RemoteSessionSetup {
            connection,
            transfer_dir,
            reservation_intent,
            capture_lease,
            transfer,
            session_id,
            initial_offset_ms,
            initial_memo,
        })
    };
    let mut first_open = None;
    let (mut preopened, setup, mut early_control) = start_remote_capture(
        || {
            let (segment, fell_back, note) = open_remote_segment(
                saved_choice_active.then_some(preference.as_ref()).flatten(),
                selected_device.as_ref(),
            )?;
            crate::cli_log::event(
                "remote_capture_opened",
                format!("open_ms={}", start_requested.elapsed().as_millis()),
            );
            if let (Some(controller), Some(recorder)) = (&controller, &segment.recorder) {
                controller.capturing(&segment.sink, &segment.mic_duration_us, recorder);
            }
            first_open = Some((fell_back, note));
            let report_unsent: Box<dyn Fn(&Path) + '_> = match &controller {
                Some(controller) => Box::new(move |path| controller.unsent_audio_saved(path)),
                None => Box::new(|path| {
                    eprintln!(
                        "Recording saved on this Mac: {}. Import it with `margins transcribe {}`.",
                        path.display(),
                        path.display()
                    )
                }),
            };
            Ok(PreopenedRemoteSegment {
                segment: Some(segment),
                unsent_dir: unsent_dir.clone(),
                unsent_wav: None,
                report_unsent,
            })
        },
        reserve,
        |timeout| match &controller {
            Some(controller) => controller.pre_session_control(timeout),
            None => {
                std::thread::sleep(timeout);
                None
            }
        },
        |preopened, control| {
            // Act now; the session may be many seconds away.
            preopened.retire_early();
            crate::cli_log::event(
                "remote_capture_control_before_session",
                format!(
                    "control={control:?} since_start_ms={}",
                    start_requested.elapsed().as_millis()
                ),
            );
            if let Some(controller) = &controller {
                match control {
                    PreSessionControl::Pause => controller.paused(),
                    PreSessionControl::Stop => controller.saving(),
                }
            }
        },
    )?;
    if let Some((fell_back, note)) = first_open.take() {
        if fell_back {
            selected_device = None;
            saved_choice_active = false;
        }
        if let Some(note) = note {
            preference_note = Some(note);
        }
    }
    let RemoteSessionSetup {
        connection,
        transfer_dir,
        mut reservation_intent,
        capture_lease: _capture_lease,
        mut transfer,
        session_id,
        initial_offset_ms,
        initial_memo,
    } = setup?;
    let capabilities = &connection.capabilities;

    // The CoreML worker is optional. Its checkpoint stays inside this scoped
    // transfer until a separate publisher sends a bounded copy to the service.
    let checkpoint_path = transfer.spool().root().join("live-checkpoint.json");
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = if REMOTE_LIVE_RETIREMENT.ready_for_new_worker() {
        start_live_transcript_worker(
            checkpoint_path.clone(),
            initial_offset_ms,
            live_status.clone(),
            // Remote capture keeps no local durable runtime audio to catch up from.
            None,
        )
    } else {
        // A quick Start after Stop must not stack a second CoreML load on the
        // previous capture's still-warming worker. The server transcript
        // remains authoritative; only the provisional view is skipped.
        crate::cli_log::event("live_worker_skipped", "reason=previous_worker_retiring");
        live_status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
        None
    };
    let checkpoint_publisher = if live.is_some() {
        let client = connection.client.clone();
        let session = session_id.clone();
        let producer_token = transfer.spool().producer_token()?;
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = done.clone();
        let join = std::thread::Builder::new()
            .name("margins-remote-live-checkpoint".into())
            .spawn(move || {
                publish_remote_live_checkpoints(
                    &client,
                    &session,
                    &producer_token,
                    &checkpoint_path,
                    &thread_done,
                );
            })?;
        Some(RemoteCheckpointPublisher {
            done,
            join: Some(join),
        })
    } else {
        None
    };

    let initial_revision = initial_memo.revision.clone();
    let initial_lines = initial_memo.lines.clone();
    let started_at = start_requested_local
        - chrono::Duration::milliseconds(initial_offset_ms.min(i64::MAX as u64) as i64);
    let draft_path = transfer.spool().root().join("memo-draft.md");
    let document = if draft_path.is_file() {
        margins_core::TimedMemoDocument::parse_markdown(&std::fs::read_to_string(&draft_path)?)
    } else {
        margins_core::TimedMemoDocument::from_committed(
            initial_memo
                .lines
                .iter()
                .cloned()
                .map(|line| margins_core::TimedMemoLine {
                    text: line.text,
                    created_secs: line.created_secs,
                    edited_secs: line.edited_secs,
                    draft_started_secs: line.draft_started_secs,
                    audio_pending_at_mark: line.audio_pending_at_mark,
                    block_ordinal: line.block_ordinal,
                })
                .collect(),
        )
    };
    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::from_memo(
        document,
        draft_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.preferred_mic_name = preference.as_ref().map(|choice| choice.name.clone());
    app.preferred_mic_uid = preference.as_ref().and_then(|choice| choice.uid.clone());
    app.message = preference_note;
    app.set_session_identity(
        session_id.clone(),
        matches!(command, Some(Command::Attach { .. })),
    );
    let uploader_done = Arc::new(AtomicBool::new(false));
    let uploader_state = Arc::new(AtomicU8::new(crate::app::REMOTE_DELIVERY_CURRENT));
    let uploader_pending_chunks = Arc::new(AtomicU64::new(0));
    let uploader_pending_bytes = Arc::new(AtomicU64::new(0));
    app.remote_delivery_state = uploader_state.clone();
    app.remote_pending_chunks = uploader_pending_chunks.clone();
    app.remote_pending_bytes = uploader_pending_bytes.clone();
    app.live_transcription_status = live_status;
    if let Some(worker) = &live {
        (
            app.live_mic_dropped_samples,
            app.live_system_dropped_samples,
        ) = worker.dropped_counters();
    }
    let uploader_parent = transfer
        .spool()
        .root()
        .parent()
        .context("remote transfer has no parent")?
        .to_path_buf();
    let uploader_id = transfer.spool().manifest().transfer_id.clone();
    let uploader_client = connection.client.clone();
    let uploader_reserve = capabilities.limits.spool_reserve_bytes;
    let uploader_batch_limit = capabilities.limits.max_in_flight_chunks.max(1) as usize;
    let uploader_done_flag = uploader_done.clone();
    let uploader = std::thread::Builder::new()
        .name("margins-remote-delivery".into())
        .spawn(move || {
            let mut backoff = std::time::Duration::from_millis(50);
            loop {
                if uploader_done_flag.load(Ordering::Acquire) {
                    break;
                }
                let result = (|| -> Result<(bool, bool)> {
                    let mut spool = DurableTransferSpool::open(
                        &uploader_parent,
                        &uploader_id,
                        uploader_reserve,
                    )?;
                    let chunks = spool.pending_chunks()?;
                    uploader_pending_chunks.store(chunks.len() as u64, Ordering::Release);
                    uploader_pending_bytes.store(
                        chunks.iter().map(|chunk| chunk.size_bytes).sum(),
                        Ordering::Release,
                    );
                    let has_work = !chunks.is_empty() || !spool.pending_closes().is_empty();
                    let catching_up = chunks.len() >= uploader_batch_limit;
                    if has_work {
                        deliver_available(&mut spool, &uploader_client)?;
                    }
                    Ok((has_work, catching_up))
                })();
                match result {
                    Ok((_, catching_up)) => {
                        uploader_state
                            .store(crate::app::REMOTE_DELIVERY_CURRENT, Ordering::Release);
                        // A full batch means durable ingress is outrunning the
                        // last request. Continue promptly until below the
                        // advertised catch-up threshold; partial/idle polling
                        // remains relaxed and never busy-spins.
                        backoff = if catching_up {
                            std::time::Duration::from_millis(1)
                        } else {
                            std::time::Duration::from_millis(50)
                        };
                    }
                    Err(error) => {
                        uploader_state
                            .store(crate::app::REMOTE_DELIVERY_PENDING, Ordering::Release);
                        if remote_delivery_requires_credentials(&error) {
                            // Preserve the spool and stop background retries until
                            // the user repairs/reissues credentials. Final Stop
                            // still releases devices first and makes one truthful
                            // delivery attempt before returning the transfer id.
                            break;
                        }
                        backoff = (backoff * 2).min(std::time::Duration::from_secs(2));
                    }
                }
                std::thread::sleep(backoff);
            }
        })?;
    let mut announced_sources = false;
    let mut local_segment_index = 0usize;
    'capture: loop {
        let offset_ms = remote_segment_offset_ms(
            initial_offset_ms,
            start_requested.elapsed().as_millis() as u64,
            transfer.last_closed_ended_at_ms(),
        );
        let segment_id = format!("native-{}", uuid::Uuid::new_v4().simple());
        // The first segment's audio began at the start request.
        let first = preopened.segment.take();
        // Stays with the guard, which reports it on any error, until the
        // transfer holds this audio.
        let early_wav = preopened.unsent_wav.clone();
        let (opened, offset_ms) = match first {
            Some(opened) => (opened, initial_offset_ms),
            None => match open_remote_segment(
                saved_choice_active.then_some(preference.as_ref()).flatten(),
                selected_device.as_ref(),
            ) {
                Ok((opened, fell_back, note)) => {
                    if fell_back {
                        selected_device = None;
                        saved_choice_active = false;
                    }
                    if let Some(note) = note {
                        app.message = Some(note);
                    }
                    (opened, offset_ms)
                }
                Err(error) => {
                    // A reservation is not a successful capture. Seal it aborted,
                    // retaining the transfer if the server cannot acknowledge. No
                    // cleanup step may short-circuit the later steps: in particular,
                    // stop/join the uploader and dispose the reservation even if the
                    // durable abort intent itself fails.
                    let cleanup_errors = cleanup_failed_remote_recorder_start(
                        transfer,
                        initial_offset_ms,
                        &mut reservation_intent,
                        &transfer_dir,
                        &uploader_done,
                        uploader,
                        |spool| deliver_transfer(spool, &connection.client),
                    );
                    if cleanup_errors.is_empty() {
                        return Err(error);
                    }
                    return Err(error.context(cleanup_errors.join("; ")));
                }
            },
        };
        let OpenedRemoteSegment {
            recorder,
            stop,
            sink,
            mic_duration_us,
            handoff,
            worker,
        } = opened;
        sink.generation_clock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .session_offset_ms = offset_ms;
        if let Some(recorder) = &recorder {
            app.current_mic_name = recorder.mic_name().to_owned();
            app.current_mic_uid = recorder.mic_uid().map(str::to_owned);
        }
        if pending_selection {
            if let Some(selected) = selected_device.as_ref() {
                audio_preferences::remember_selection(&mut app, selected);
            }
            pending_selection = false;
        }
        app.mic_silent = false;
        app.suggested_mic_name = None;
        app.devices = crate::recorder::list_input_devices()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        app.device_uids = crate::recorder::input_device_uid_snapshot(&app.devices);
        if !announced_sources {
            eprintln!(
                "Recording on this Mac · Saving to {} / {} · Microphone + system audio enabled",
                capabilities.instance_id.as_ref(),
                workspace_id
            );
            announced_sources = true;
        }
        transfer.begin_segment(segment_id.clone(), offset_ms)?;
        if let Some(intent) = reservation_intent.take() {
            intent.remove(&transfer_dir)?;
        }
        let recovery_path = transfer.spool().recovery_path(&segment_id)?;
        let active_transfer_id = transfer.spool().manifest().transfer_id.clone();
        let live_sink = live
            .as_ref()
            .map(|worker| worker.sink_for_offset(offset_ms));
        let live_tee = live_sink.map(|sink| {
            Box::new(move |chunk: &crate::recorder::LiveAudioChunk| {
                // Chunks buffered before the session existed carry no offset.
                if chunk.session_offset_ms == offset_ms {
                    enqueue_remote_live_chunk(&sink, chunk);
                } else {
                    let mut chunk = chunk.clone();
                    chunk.session_offset_ms = offset_ms;
                    enqueue_remote_live_chunk(&sink, &chunk);
                }
            }) as Box<dyn FnMut(&crate::recorder::LiveAudioChunk) + Send>
        });
        if let Err(mpsc::SendError(returned)) = handoff.send(RemoteSpoolHandoff {
            transfer,
            live: live_tee,
        }) {
            // Keep the transfer for retry and do not leave the uploader running.
            drop(returned);
            uploader_done.store(true, Ordering::Release);
            let _ = uploader.join();
            bail!("remote spool worker stopped before the segment began");
        }
        drop(handoff);
        crate::cli_log::event(
            "remote_segment_began",
            format!(
                "offset_ms={offset_ms} since_start_ms={}",
                start_requested.elapsed().as_millis()
            ),
        );

        if let Some(recorder) = &recorder {
            app.mic_level = recorder.mic_peak();
            app.spk_level = recorder.spk_peak();
            app.mic_drops = recorder.mic_drops();
            app.spk_drops = recorder.spk_drops();
            app.mic_frames = recorder.mic_frames();
            app.mic_silence = recorder.mic_silence();
            app.mic_rate = recorder.mic_rate();
            app.mic_real_spool_frames =
                recorder.real_spool_frames(crate::recorder::CaptureLane::Mic);
            app.spk_real_spool_frames =
                recorder.real_spool_frames(crate::recorder::CaptureLane::System);
            app.mic_no_audio_received.store(false, Ordering::Release);
            app.spk_no_audio_received.store(false, Ordering::Release);
            app.spk_silence = recorder.spk_silence();
            app.spk_frames = recorder.spk_frames();
            app.spk_rate = recorder.spk_rate();
        }
        if let Some(controller) = &controller {
            match &recorder {
                Some(recorder) => controller.recording(
                    &session_id,
                    &active_transfer_id,
                    &sink,
                    &mic_duration_us,
                    recorder,
                ),
                // Paused or stopped before the session existed; keep that state.
                None => controller.session_ready(&session_id, &active_transfer_id),
            }
        }
        // The recorder owns the sender from here. Keeping this clone alive
        // would prevent the spool worker's receive loop from closing on stop.
        drop(sink);
        let action = match early_control.take() {
            Some(PreSessionControl::Pause) => crate::tui::TuiAction::Pause,
            Some(PreSessionControl::Stop) => crate::tui::TuiAction::Quit,
            None if recorder.is_none() => crate::tui::TuiAction::Quit,
            None => match &controller {
                Some(controller) => controller.wait_action(&stop)?,
                None => crate::tui::run_tui(&mut app, stop.clone())
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?,
            },
        };
        // run_tui set the stop flag before returning. This call synchronously
        // retires both native devices before memo/network/ASR work begins. A
        // segment retired before its session existed already wrote its WAV.
        let recorder_wav = match recorder {
            Some(recorder) => {
                recorder.stop_and_write(&recovery_path.to_string_lossy())?;
                Some(recovery_path.clone())
            }
            None => early_wav.clone(),
        };
        if let (Some(directory), Some(recorder_wav)) = (local_audio_dir, &recorder_wav) {
            match copy_remote_recovery_for_local_asr(
                recorder_wav,
                directory,
                &active_transfer_id,
                local_segment_index,
            ) {
                Ok(path) => {
                    if let Some(controller) = &controller {
                        controller.local_audio_saved(&path);
                    }
                }
                Err(error) => {
                    if let Some(controller) = &controller {
                        controller.local_audio_failed(&format!("{error:#}"));
                    }
                }
            }
            local_segment_index += 1;
        }
        let (returned, spool_result) = worker
            .join()
            .map_err(|_| anyhow::anyhow!("remote spool worker panicked"))?;
        transfer = returned.context("remote spool worker lost the transfer")?;
        spool_result.context("remote audio spool failed; local recovery WAV was retained")?;
        if let Some(path) = preopened.unsent_wav.take() {
            // The transfer now holds this audio durably.
            let _ = std::fs::remove_file(path);
        }

        match action {
            crate::tui::TuiAction::Pause => {
                transfer.close_segment(SegmentCloseReasonV1::Pause)?;
                app.set_capture_paused(true);
                if let Some(controller) = &controller {
                    controller.paused();
                }
                app.mic_level = Arc::new(AtomicU32::new(0));
                app.spk_level = Arc::new(AtomicU32::new(0));
                loop {
                    let paused_action = if let Some(controller) = &controller {
                        controller.wait_paused_action()?
                    } else {
                        crate::tui::run_tui(&mut app, Arc::new(AtomicBool::new(false)))
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?
                    };
                    match paused_action {
                        crate::tui::TuiAction::Resume => {
                            app.set_capture_paused(false);
                            break;
                        }
                        crate::tui::TuiAction::Quit => break 'capture,
                        crate::tui::TuiAction::SwitchDevice(index) => {
                            let selected = crate::recorder::selected_input_device(
                                &app.devices,
                                &app.device_uids,
                                index,
                            )?;
                            selected_device = Some(selected);
                            saved_choice_active = false;
                            pending_selection = true;
                        }
                        crate::tui::TuiAction::Pause => {}
                    }
                }
            }
            crate::tui::TuiAction::SwitchDevice(index) => {
                transfer.close_segment(SegmentCloseReasonV1::Rollover)?;
                let selected =
                    crate::recorder::selected_input_device(&app.devices, &app.device_uids, index)?;
                selected_device = Some(selected);
                saved_choice_active = false;
                pending_selection = true;
            }
            crate::tui::TuiAction::Quit => {
                transfer.close_segment(SegmentCloseReasonV1::Stop)?;
                break 'capture;
            }
            crate::tui::TuiAction::Resume => bail!("resume requested while capture was active"),
        }
    }
    // Pin the media boundary before any uploader join, memo write, or server
    // work. Stop drain latency must never inflate recorded duration.
    let final_ended_at_ms = transfer
        .last_closed_ended_at_ms()
        .context("remote capture stopped without a durable media boundary")?;
    if let Some(controller) = &controller {
        controller.saving();
    }
    let result = stop_remote_capture(
        transfer,
        final_ended_at_ms,
        live,
        checkpoint_publisher,
        &REMOTE_LIVE_RETIREMENT,
        &uploader_done,
        uploader,
        |transfer| {
            app.save().context("remote memo draft could not be saved")?;
            let lines: Vec<WorkspaceMemoLineV1> = app
                .memo
                .lines()
                .iter()
                .cloned()
                .map(|line| WorkspaceMemoLineV1 {
                    text: line.text,
                    created_secs: line.created_secs,
                    edited_secs: line.edited_secs,
                    draft_started_secs: line.draft_started_secs,
                    audio_pending_at_mark: line.audio_pending_at_mark,
                    block_ordinal: line.block_ordinal,
                })
                .collect();
            // The native bridge has no memo editor. BB/Codex may have edited the
            // Workspace memo during capture, so sending our initial snapshot here
            // would overwrite it (or block finalization with a revision conflict).
            if controller.is_none() && remote_memo_was_edited(&initial_lines, &lines) {
                let memo_request_id =
                    format!("native-memo-{}", transfer.spool().manifest().transfer_id);
                transfer
                    .spool_mut()
                    .set_memo_intent(WorkspaceMemoReplaceV1 {
                        request_id: memo_request_id,
                        expected_revision: initial_revision,
                        lines,
                    })?;
            }
            Ok(())
        },
        |spool| deliver_transfer(spool, &connection.client),
    );
    if controller.is_none() {
        // A one-shot CLI process exits next. Give a warming CoreML worker a
        // bounded chance to unwind instead of tearing it down mid-load.
        REMOTE_LIVE_RETIREMENT.wait(REMOTE_LIVE_EXIT_WAIT);
    }
    if result.is_ok() {
        eprintln!("Saved to remote Workspace; processing state is separate.");
    }
    result
}

/// How long a one-shot remote capture process waits at exit for its cancelled
/// live worker before abandoning it.
#[cfg(feature = "audio-capture")]
const REMOTE_LIVE_EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Stop's server-facing tail. The live transcript is retired first and never
/// awaited: the server transcribes the delivered audio authoritatively and
/// refuses checkpoints once the session is finalized, so waiting for the
/// provisional on-device transcript (possibly still warming up) would only
/// delay Stop.
#[cfg(feature = "audio-capture")]
#[allow(clippy::too_many_arguments)]
fn stop_remote_capture<M, D>(
    mut transfer: margins_workflows::remote_workspace::NativeRemoteTransfer,
    final_ended_at_ms: u64,
    live: Option<LiveTranscriptWorker>,
    publisher: Option<RemoteCheckpointPublisher>,
    retirement: &RemoteLiveRetirement,
    uploader_done: &AtomicBool,
    uploader: std::thread::JoinHandle<()>,
    before_seal: M,
    deliver: D,
) -> Result<()>
where
    M: FnOnce(&mut margins_workflows::remote_workspace::NativeRemoteTransfer) -> Result<()>,
    D: FnOnce(&mut margins_workflows::remote_workspace::DurableTransferSpool) -> Result<()>,
{
    retirement.hold(retire_remote_live_transcript(live, publisher));
    uploader_done.store(true, Ordering::Release);
    uploader
        .join()
        .map_err(|_| anyhow::anyhow!("remote delivery worker panicked"))?;
    before_seal(&mut transfer)?;
    transfer.seal_session(
        final_ended_at_ms,
        margins_meeting_protocol::SessionFinalizeReasonV1::Completed,
    )?;
    let transfer_id = transfer.spool().manifest().transfer_id.clone();
    let mut spool = transfer.into_spool();
    deliver(&mut spool).map_err(|error| {
        anyhow::anyhow!(
            "recording stopped; upload pending in transfer {transfer_id}. Retry with `margins transfers retry {transfer_id}`: {error}"
        )
    })
}

#[cfg(feature = "audio-capture")]
fn remote_memo_was_edited(
    initial: &[margins_meeting_protocol::WorkspaceMemoLineV1],
    final_lines: &[margins_meeting_protocol::WorkspaceMemoLineV1],
) -> bool {
    // App::from_memo appends one empty TUI draft even if nobody types. A
    // capture-only session must not turn that draft into a remote replacement.
    let actual = if final_lines.len() == initial.len() + 1
        && final_lines
            .last()
            .is_some_and(|line| line.text.trim().is_empty())
    {
        &final_lines[..initial.len()]
    } else {
        final_lines
    };
    actual != initial
}

#[cfg(feature = "audio-capture")]
fn remote_spool_from_reservation(
    connection: &margins_workflows::remote_workspace::RemoteConnection,
    capabilities: &margins_meeting_protocol::WorkspaceCapabilitiesV1,
    transfer_dir: &Path,
    remote: &str,
    workspace_id: &str,
    intent: &margins_workflows::remote_workspace::CaptureReservationIntentV1,
) -> Result<margins_workflows::remote_workspace::DurableTransferSpool> {
    use margins_workflows::remote_workspace::{
        promote_capture_reservation, CaptureReservationRequestV1,
    };
    if capabilities.instance_id.as_ref() != intent.instance_id
        || intent.remote_url != remote
        || intent.workspace_id != workspace_id
    {
        bail!("reservation intent does not match the selected remote instance and Workspace");
    }
    let spool = promote_capture_reservation(
        intent,
        transfer_dir,
        capabilities.limits.spool_reserve_bytes,
        || {
            let reservation = match &intent.request {
                CaptureReservationRequestV1::Create { command } => {
                    connection.client.reserve(command)?
                }
                CaptureReservationRequestV1::Attach { request } => {
                    connection.client.attach(&intent.session_id, request)?
                }
            };
            Ok(reservation.producer_token)
        },
    )?;
    if spool.manifest().finalize_command.is_some() {
        bail!("reserved remote transfer is already sealed; retry its delivery instead of starting capture");
    }
    Ok(spool)
}

#[cfg(feature = "audio-capture")]
fn remote_delivery_requires_credentials(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "unauthorized:",
        "forbidden:",
        "credential is invalid",
        "credential is revoked or expired",
        "producer token",
        "producer_token",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(feature = "audio-capture")]
struct RemoteCheckpointPublisher {
    done: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "audio-capture")]
impl Drop for RemoteCheckpointPublisher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Reapers of live workers cancelled by earlier captures in this process. A
/// new capture starts its own worker only once they have all exited, so quick
/// Start/Stop cycles within one CoreML warmup never stack concurrent loads.
#[cfg(feature = "audio-capture")]
struct RemoteLiveRetirement(Mutex<Vec<std::thread::JoinHandle<()>>>);

#[cfg(feature = "audio-capture")]
static REMOTE_LIVE_RETIREMENT: RemoteLiveRetirement = RemoteLiveRetirement::new();

#[cfg(feature = "audio-capture")]
impl RemoteLiveRetirement {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn hold(&self, reaper: Option<std::thread::JoinHandle<()>>) {
        if let Some(reaper) = reaper {
            self.reapers().push(reaper);
        }
    }

    fn reapers(&self) -> std::sync::MutexGuard<'_, Vec<std::thread::JoinHandle<()>>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Join finished reapers; true when none is still running.
    fn ready_for_new_worker(&self) -> bool {
        let mut reapers = self.reapers();
        let (finished, running): (Vec<_>, Vec<_>) =
            reapers.drain(..).partition(|reaper| reaper.is_finished());
        for reaper in finished {
            let _ = reaper.join();
        }
        *reapers = running;
        reapers.is_empty()
    }

    /// Wait up to `timeout` for every reaper; true when all have exited.
    fn wait(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.ready_for_new_worker() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                crate::cli_log::event(
                    "remote_live_retire_abandoned",
                    format!("waited_ms={}", timeout.as_millis()),
                );
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Cancel the optional live worker and stop its checkpoint publisher without
/// blocking Stop. Both threads exit on their own (a warming worker as soon as
/// its uninterruptible model load returns); the returned reaper joins them so
/// their outcome is logged and nothing outlives its owner unobserved.
#[cfg(feature = "audio-capture")]
fn retire_remote_live_transcript(
    live: Option<LiveTranscriptWorker>,
    publisher: Option<RemoteCheckpointPublisher>,
) -> Option<std::thread::JoinHandle<()>> {
    if live.is_none() && publisher.is_none() {
        return None;
    }
    if let Some(publisher) = &publisher {
        publisher.done.store(true, Ordering::Release);
    }
    let finalizer = live.map(LiveTranscriptWorker::cancel);
    let reaper = std::thread::Builder::new()
        .name("margins-remote-live-retire".into())
        .spawn(move || {
            drop(publisher);
            if let Some(finalizer) = finalizer {
                let _ = finalizer.complete();
            }
        });
    match reaper {
        Ok(reaper) => Some(reaper),
        Err(error) => {
            // Without a reaper the cancelled threads still exit by themselves.
            crate::cli_log::event(
                "remote_live_retire_unavailable",
                crate::cli_log::error_summary(&anyhow::Error::from(error)),
            );
            None
        }
    }
}

#[cfg(feature = "audio-capture")]
fn enqueue_remote_live_chunk(
    sink: &crate::recorder::LiveAudioSink,
    chunk: &crate::recorder::LiveAudioChunk,
) {
    use crate::recorder::LiveAudioChannel;
    let count = chunk.samples.len() as u64;
    let dropped = match chunk.channel {
        LiveAudioChannel::Mic => &sink.mic_dropped_samples,
        LiveAudioChannel::System => &sink.system_dropped_samples,
    };
    let accepted = match chunk.channel {
        LiveAudioChannel::Mic => &sink.mic_accepted_samples,
        LiveAudioChannel::System => &sink.system_accepted_samples,
    };
    let reserved = sink
        .queued_samples
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            let next = queued.checked_add(count)?;
            (next <= sink.queue_max_samples).then_some(next)
        })
        .is_ok();
    if !reserved {
        dropped.fetch_add(count, Ordering::Relaxed);
        return;
    }
    let mut copy = chunk.clone();
    copy.generation = sink.generation;
    if sink.sender.send(copy).is_ok() {
        accepted.fetch_add(count, Ordering::Relaxed);
    } else {
        sink.queued_samples.fetch_sub(count, Ordering::AcqRel);
        dropped.fetch_add(count, Ordering::Relaxed);
    }
}

#[cfg(feature = "audio-capture")]
fn publish_remote_live_checkpoints(
    client: &margins_workflows::remote_workspace::WorkspaceHttpClient,
    session: &str,
    producer_token: &str,
    path: &Path,
    done: &AtomicBool,
) {
    const MAX_BYTES: u64 = 256 * 1024;
    let mut last_sent = Vec::new();
    let mut last_attempt = std::time::Instant::now() - std::time::Duration::from_secs(3);
    // Stop does not flush a final checkpoint: the session is finalized right
    // away and the server's own transcript supersedes this provisional view.
    while !done.load(Ordering::Acquire) {
        if last_attempt.elapsed() >= std::time::Duration::from_secs(3) {
            if let Ok(metadata) = std::fs::metadata(path) {
                if metadata.len() > 0 && metadata.len() <= MAX_BYTES {
                    if let Ok(body) = std::fs::read(path) {
                        if body != last_sent {
                            last_attempt = std::time::Instant::now();
                            match client.put_live_checkpoint(session, producer_token, body.clone())
                            {
                                Ok(()) => last_sent = body,
                                Err(error) => {
                                    let (kind, detail) = remote_live_checkpoint_failure(&error);
                                    crate::cli_log::event(kind, detail);
                                }
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Diagnostic for a rejected checkpoint. A request that raced finalize is an
/// expected skip, not a failure; otherwise record the server's error code (a
/// fixed protocol token) rather than the generic application category.
#[cfg(any(test, feature = "audio-capture"))]
fn remote_live_checkpoint_failure(error: &anyhow::Error) -> (&'static str, String) {
    let message = format!("{error:#}");
    if message.contains("capture producer is no longer active") {
        return (
            "remote_live_checkpoint_skipped",
            "reason=session_finalized".into(),
        );
    }
    let code = message
        .split_once(": ")
        .map(|(code, _)| code)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        });
    let detail = if let Some(code) = code {
        let reason = if message.contains("word timeline") {
            " reason=word_timeline"
        } else if message.contains("watermarks") {
            " reason=watermarks"
        } else {
            ""
        };
        format!("category=server code={code}{reason}")
    } else if let Some(status) = message
        .strip_prefix("capture relay rejected request (")
        .and_then(|rest| rest.split_once(')'))
        .map(|(status, _)| status)
    {
        format!("category=relay status={status}")
    } else {
        crate::cli_log::error_summary(error)
    };
    ("remote_live_checkpoint_upload_failed", detail)
}
