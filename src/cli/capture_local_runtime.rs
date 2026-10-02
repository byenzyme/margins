//! In-process meeting producer for the native TUI. It streams bounded PCM
//! batches from native device spools through the shared Recorder facade.

use anyhow::{bail, Context, Result};
use margins_meeting_protocol::{
    AudioCodecV1, AudioContainerV1, AudioFormatV1, BeginCaptureGenerationV1, CaptureLaneV1,
    CaptureModeV1, CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1,
    CaptureSourceV1, CloseSegmentV1, CreateSessionV1, DurationMillis, FinalizeSessionV1,
    LaneBoundaryV1, SegmentCloseReasonV1, SegmentCloseReferenceV1, SessionFinalizeReasonV1,
    SessionId, SessionMillis, UnixMillis,
};
use margins_meeting_runtime::{MeetingRuntime, MeetingRuntimeStorage, RecorderLaneV1};
use margins_store::SqliteMeetingRuntimeStorage;
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub(super) struct LocalMeetingProducer {
    runtime: MeetingRuntime<SqliteMeetingRuntimeStorage>,
    session_id: SessionId,
    closes: Vec<SegmentCloseReferenceV1>,
    last_end_ms: u64,
    origin_ms: u64,
    stream: Option<RuntimeStreamWorker>,
}

fn now_ms() -> UnixMillis {
    UnixMillis(chrono::Utc::now().timestamp_millis().max(0) as u64)
}

fn segment_id(name: &str, ordinal: i64) -> String {
    format!("{name}-seg-{ordinal}")
}

const PCM_SAMPLE_RATE: u32 = 16_000;
const PCM_BATCH_FRAMES: usize = 5 * PCM_SAMPLE_RATE as usize;

struct PcmLaneSender {
    lane: RecorderLaneV1,
    resampler: margins_media::timeline::RationalResampler,
    sequence: u64,
    sent_frames: u64,
    pending: Vec<u8>,
    offset_ms: u64,
    origin_ms: u64,
}

impl PcmLaneSender {
    fn new(
        runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>,
        session: &SessionId,
        ordinal: i64,
        lane: &str,
        source_rate: u32,
        offset_ms: u64,
        origin_ms: u64,
    ) -> Result<Self> {
        let segment = segment_id(session.as_ref(), ordinal);
        let handle = runtime
            .recorder()
            .open_lane(session, segment.into(), lane.into())
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(Self {
            lane: handle,
            resampler: margins_media::timeline::RationalResampler::new(
                source_rate,
                PCM_SAMPLE_RATE,
            )?,
            sequence: 0,
            sent_frames: 0,
            pending: Vec::with_capacity(PCM_BATCH_FRAMES * 2),
            offset_ms,
            origin_ms,
        })
    }

    fn push_samples(
        &mut self,
        runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>,
        samples: &[f32],
    ) -> Result<()> {
        let converted = self.resampler.process(samples)?;
        self.push_converted(runtime, &converted)
    }

    fn push_converted(
        &mut self,
        runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>,
        samples: &[f32],
    ) -> Result<()> {
        for sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
            self.pending.extend_from_slice(&value.to_le_bytes());
        }
        if self.pending.len() >= PCM_BATCH_FRAMES * 2 {
            self.flush(runtime)?;
        }
        Ok(())
    }

    fn flush(&mut self, runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let frames = (self.pending.len() / 2) as u64;
        let start_ms = self.offset_ms + self.sent_frames.div_ceil(16);
        let next_frames = self.sent_frames + frames;
        let end_ms = self.offset_ms + next_frames.div_ceil(16);
        runtime
            .recorder()
            .append_chunk(
                &self.lane,
                format!(
                    "{}-{}-{}",
                    self.lane.segment_id.as_ref(),
                    self.lane.lane_id.as_ref(),
                    self.sequence
                )
                .into(),
                UnixMillis(self.origin_ms.saturating_add(start_ms)),
                self.sequence,
                SessionMillis(start_ms),
                DurationMillis(end_ms.saturating_sub(start_ms)),
                std::mem::take(&mut self.pending),
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.sequence += 1;
        self.sent_frames = next_frames;
        Ok(())
    }

    fn finish(
        &mut self,
        runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>,
    ) -> Result<(u64, u64)> {
        let final_samples = self.resampler.finish()?;
        self.push_converted(runtime, &final_samples)?;
        self.flush(runtime)?;
        Ok((self.sequence, self.sent_frames))
    }
}

struct SpoolReader {
    source: crate::recorder::NativeSpoolSource,
    file: std::fs::File,
    tail: Vec<u8>,
    sender: PcmLaneSender,
}

impl SpoolReader {
    fn read_available(
        &mut self,
        runtime: &MeetingRuntime<SqliteMeetingRuntimeStorage>,
    ) -> Result<bool> {
        let mut buffer = [0u8; 64 * 1024];
        let count = self.file.read(&mut buffer)?;
        if count == 0 {
            return Ok(false);
        }
        self.tail.extend_from_slice(&buffer[..count]);
        let complete = self.tail.len() - self.tail.len() % 4;
        if complete > 0 {
            let prior_sequence = self.sender.sequence;
            let samples = self.tail[..complete]
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>();
            self.tail.drain(..complete);
            self.sender.push_samples(runtime, &samples)?;
            if self.sender.sequence > prior_sequence {
                let consumed = self
                    .file
                    .stream_position()?
                    .saturating_sub(self.tail.len() as u64);
                if self.source.compact_through(consumed)? {
                    self.file = std::fs::File::open(&self.source.path)?;
                    // Any partial f32 sample was kept by the actor's copy.
                    self.tail.clear();
                }
            }
        }
        Ok(true)
    }
}

struct RuntimeStreamWorker {
    stop: mpsc::Sender<()>,
    join: JoinHandle<Result<[(u64, u64); 2]>>,
}

impl RuntimeStreamWorker {
    fn start(
        dir: PathBuf,
        session: SessionId,
        ordinal: i64,
        sources: [crate::recorder::NativeSpoolSource; 2],
        offset_ms: u64,
        origin_ms: u64,
    ) -> Result<Self> {
        let (stop, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("margins-runtime-audio".into())
            .spawn(move || {
                let setup = || -> Result<(MeetingRuntime<SqliteMeetingRuntimeStorage>, [SpoolReader; 2])> {
                    let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(&dir)?);
                    let readers = ["mic", "system"].map(|lane| lane.to_owned());
                    let mut sources = sources.into_iter();
                    let mut make = |lane: &str| -> Result<SpoolReader> {
                        let source = sources.next().context("missing native spool")?;
                        Ok(SpoolReader {
                            file: std::fs::File::open(&source.path)?,
                            tail: Vec::new(),
                            sender: PcmLaneSender::new(
                                &runtime,
                                &session,
                                ordinal,
                                lane,
                                source.rate,
                                offset_ms,
                                origin_ms,
                            )?,
                            source,
                        })
                    };
                    let mic = make(&readers[0])?;
                    let system = make(&readers[1])?;
                    drop(make);
                    Ok((runtime, [mic, system]))
                };
                let (runtime, mut readers) = match setup() {
                    Ok(value) => {
                        let _ = ready_tx.send(Ok(()));
                        value
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return Err(error);
                    }
                };
                let mut stopping = false;
                loop {
                    let mut advanced = false;
                    for reader in &mut readers {
                        advanced |= reader.read_available(&runtime)?;
                    }
                    stopping |= !matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
                    if stopping && !advanced {
                        break;
                    }
                    if !advanced {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                if readers.iter().any(|reader| !reader.tail.is_empty()) {
                    bail!("native spool ended with a partial f32 sample");
                }
                Ok([
                    readers[0].sender.finish(&runtime)?,
                    readers[1].sender.finish(&runtime)?,
                ])
            })?;
        match ready_rx
            .recv()
            .context("native runtime worker stopped before readiness")?
        {
            Ok(()) => Ok(Self { stop, join }),
            Err(message) => {
                let _ = join.join();
                bail!("native runtime worker could not start: {message}")
            }
        }
    }

    fn flush(self) -> Result<[(u64, u64); 2]> {
        let _ = self.stop.send(());
        self.join
            .join()
            .map_err(|_| anyhow::anyhow!("native runtime worker panicked"))?
    }
}

impl LocalMeetingProducer {
    pub(super) fn reserve(
        margins_dir: &Path,
        name: &str,
        title: Option<&str>,
        started_at: chrono::DateTime<chrono::Local>,
    ) -> Result<Self> {
        Self::reserve_with_adoption(margins_dir, name, title, started_at, false)
    }

    fn reserve_with_adoption(
        margins_dir: &Path,
        name: &str,
        title: Option<&str>,
        started_at: chrono::DateTime<chrono::Local>,
        adopt_legacy: bool,
    ) -> Result<Self> {
        let storage = SqliteMeetingRuntimeStorage::open(margins_dir)?;
        if adopt_legacy {
            storage.allow_legacy_adoption_once();
        }
        let runtime = MeetingRuntime::new(storage);
        let origin_ms = started_at.timestamp_millis().max(0) as u64;
        let session_id = SessionId::from(name.to_owned());
        let format = AudioFormatV1 {
            codec: AudioCodecV1::PcmS16Le,
            container: AudioContainerV1::Raw,
            sample_rate_hz: PCM_SAMPLE_RATE,
            channel_count: 1,
        };
        let sources = [
            ("mic", CaptureSourceKindV1::Microphone),
            ("system", CaptureSourceKindV1::SystemAudio),
        ];
        runtime
            .recorder()
            .reserve(
                &session_id,
                format!("{name}-reserve").into(),
                now_ms(),
                CreateSessionV1 {
                    idempotency_key: format!("native-tui-{name}"),
                    started_at_unix_ms: UnixMillis(origin_ms),
                    title: title.map(str::to_owned),
                    sources: sources
                        .iter()
                        .map(|(id, kind)| CaptureSourceV1 {
                            source_id: (*id).into(),
                            kind: *kind,
                            label: None,
                            external_id: None,
                        })
                        .collect(),
                    lanes: sources
                        .iter()
                        .map(|(id, _)| CaptureLaneV1 {
                            lane_id: (*id).into(),
                            source_ids: vec![(*id).into()],
                            label: None,
                            format: format.clone(),
                        })
                        .collect(),
                    provenance: CaptureProvenanceV1 {
                        hops: vec![CaptureProvenanceHopV1 {
                            producer: "margins-tui".into(),
                            producer_version: None,
                            mode: CaptureModeV1::Live,
                            observed_at_unix_ms: now_ms(),
                            attributes: BTreeMap::new(),
                        }],
                    },
                },
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(Self {
            runtime,
            session_id,
            closes: Vec::new(),
            last_end_ms: 0,
            origin_ms,
            stream: None,
        })
    }

    /// A reopened producer must replay durable state before it opens any lane.
    pub(super) fn recover(
        margins_dir: &Path,
        name: &str,
        offset_ms: u64,
        title: Option<&str>,
        started_at: chrono::DateTime<chrono::Local>,
    ) -> Result<Self> {
        let runtime = MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(margins_dir)?);
        let session_id = SessionId::from(name.to_owned());
        let recovered = runtime.recorder().recover(
            &session_id,
            format!("{name}-recover-{}", uuid::Uuid::new_v4()).into(),
            now_ms(),
            None,
        );
        let (state, _) = match recovered {
            Ok(recovered) => recovered,
            Err(margins_meeting_runtime::RecorderError::Runtime(
                margins_meeting_runtime::RuntimeError::UnknownSession(_),
            )) => return Self::reserve_with_adoption(margins_dir, name, title, started_at, true),
            Err(error) => return Err(anyhow::anyhow!("{error}")),
        };
        runtime.storage().reconcile_native_wavs(name)?;
        let origin_ms = state.create().started_at_unix_ms.0;
        if let Some((prior_id, ended_at)) = state.finalized_input() {
            runtime
                .recorder()
                .start_generation(
                    &session_id,
                    format!("{name}-generation-{}", uuid::Uuid::new_v4()).into(),
                    now_ms(),
                    BeginCaptureGenerationV1 {
                        prior_finalize_message_id: prior_id.clone(),
                        started_at_ms: SessionMillis(offset_ms.max(ended_at.0)),
                    },
                )
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        Ok(Self {
            runtime,
            session_id,
            closes: Vec::new(),
            last_end_ms: 0,
            origin_ms,
            stream: None,
        })
    }

    pub(super) fn next_ordinal(&self) -> Result<i64> {
        self.runtime
            .storage()
            .next_native_ordinal(self.session_id.as_ref())
    }

    pub(super) fn pending_segment(&self) -> Result<Option<(i64, u64)>> {
        self.runtime
            .storage()
            .pending_native_segment(self.session_id.as_ref())
    }

    pub(super) fn open(&self, ordinal: i64, offset_ms: u64) -> Result<()> {
        let id = segment_id(self.session_id.as_ref(), ordinal);
        for lane in ["mic", "system"] {
            self.runtime
                .recorder()
                .open_lane(&self.session_id, id.clone().into(), lane.into())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        self.runtime
            .storage()
            .open_native_segment(self.session_id.as_ref(), ordinal, offset_ms)?;
        Ok(())
    }

    pub(super) fn existing_segment_start(&self, ordinal: i64) -> Result<Option<u64>> {
        let id = segment_id(self.session_id.as_ref(), ordinal);
        let chunk_start = self
            .runtime
            .storage()
            .load_audio_chunk(&self.session_id, &id, &"mic".into(), 0)?
            .map(|chunk| chunk.starts_at_ms.0);
        Ok(chunk_start.or(self
            .pending_segment()?
            .filter(|(pending, _)| *pending == ordinal)
            .map(|(_, start)| start)))
    }

    /// Recovery has already replayed the runtime log before this is called.
    /// Finish an interrupted open segment from its durable chunk boundaries;
    /// the in-memory tail was smaller than one five-second batch.
    pub(super) fn recover_pending_segment(&mut self, ordinal: i64, offset_ms: u64) -> Result<bool> {
        let [(mic_count, mic_end), (system_count, system_end)] = self
            .runtime
            .storage()
            .native_chunk_boundaries(self.session_id.as_ref(), ordinal)?;
        if mic_count == 0 && system_count == 0 {
            let wav = self
                .runtime
                .storage()
                .directory()
                .join(format!("{}_seg{ordinal}.wav", self.session_id.as_ref()));
            if wav.is_file() {
                self.open(ordinal, offset_ms)?;
                self.ingest_wav_and_close(ordinal, &wav, offset_ms, SegmentCloseReasonV1::Error)?;
                return Ok(true);
            }
            self.runtime
                .storage()
                .discard_empty_native_segment(self.session_id.as_ref(), ordinal)?;
            return Ok(false);
        }
        let segment = segment_id(self.session_id.as_ref(), ordinal);
        let close_id = format!("{segment}-close");
        let end = mic_end.max(system_end);
        self.runtime
            .recorder()
            .close_segment(
                &self.session_id,
                close_id.clone().into(),
                UnixMillis(self.origin_ms.saturating_add(end)),
                CloseSegmentV1 {
                    segment_id: segment.clone().into(),
                    ended_at_ms: SessionMillis(end),
                    lane_boundaries: vec![
                        LaneBoundaryV1 {
                            lane_id: "mic".into(),
                            next_sequence: mic_count,
                        },
                        LaneBoundaryV1 {
                            lane_id: "system".into(),
                            next_sequence: system_count,
                        },
                    ],
                    reason: SegmentCloseReasonV1::Error,
                },
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.closes.push(SegmentCloseReferenceV1 {
            segment_id: segment.into(),
            close_message_id: close_id.into(),
        });
        self.last_end_ms = self.last_end_ms.max(end);
        Ok(true)
    }

    pub(super) fn start_stream(
        &mut self,
        ordinal: i64,
        offset_ms: u64,
        sources: [crate::recorder::NativeSpoolSource; 2],
    ) -> Result<()> {
        if self.stream.is_some() {
            bail!("a native runtime segment is already streaming");
        }
        self.open(ordinal, offset_ms)?;
        self.stream = Some(RuntimeStreamWorker::start(
            self.runtime.storage().directory().to_path_buf(),
            self.session_id.clone(),
            ordinal,
            sources,
            offset_ms,
            self.origin_ms,
        )?);
        Ok(())
    }

    pub(super) fn flush_stream_and_close(
        &mut self,
        ordinal: i64,
        offset_ms: u64,
        reason: SegmentCloseReasonV1,
    ) -> Result<u64> {
        let worker = self
            .stream
            .take()
            .context("no native runtime stream is active")?;
        let [(mic_count, mic_frames), (system_count, system_frames)] = worker.flush()?;
        if mic_count == 0 && system_count == 0 {
            self.runtime
                .storage()
                .discard_empty_native_segment(self.session_id.as_ref(), ordinal)?;
            return Ok(0);
        }
        let duration_ms = mic_frames.max(system_frames).div_ceil(16);
        let segment = segment_id(self.session_id.as_ref(), ordinal);
        let close_id = format!("{segment}-close");
        self.runtime
            .recorder()
            .close_segment(
                &self.session_id,
                close_id.clone().into(),
                UnixMillis(self.origin_ms.saturating_add(offset_ms + duration_ms)),
                CloseSegmentV1 {
                    segment_id: segment.clone().into(),
                    ended_at_ms: SessionMillis(offset_ms + duration_ms),
                    lane_boundaries: vec![
                        LaneBoundaryV1 {
                            lane_id: "mic".into(),
                            next_sequence: mic_count,
                        },
                        LaneBoundaryV1 {
                            lane_id: "system".into(),
                            next_sequence: system_count,
                        },
                    ],
                    reason,
                },
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.closes.push(SegmentCloseReferenceV1 {
            segment_id: segment.into(),
            close_message_id: close_id.into(),
        });
        self.last_end_ms = self.last_end_ms.max(offset_ms + duration_ms);
        Ok(duration_ms)
    }

    /// Adopt an older native WAV left by a capture that stopped before the
    /// streaming runtime path was introduced. New capture writes chunks live.
    pub(super) fn ingest_wav_and_close(
        &mut self,
        ordinal: i64,
        wav_path: &Path,
        offset_ms: u64,
        reason: SegmentCloseReasonV1,
    ) -> Result<u64> {
        if !wav_path.exists() {
            self.runtime
                .storage()
                .discard_empty_native_segment(self.session_id.as_ref(), ordinal)?;
            return Ok(0);
        }
        let mut reader = hound::WavReader::open(wav_path)
            .with_context(|| format!("could not read capture {}", wav_path.display()))?;
        let spec = reader.spec();
        if spec.channels != 2
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
        {
            bail!("native capture WAV must be stereo s16 PCM");
        }
        let segment = segment_id(self.session_id.as_ref(), ordinal);
        let mut mic = PcmLaneSender::new(
            &self.runtime,
            &self.session_id,
            ordinal,
            "mic",
            spec.sample_rate,
            offset_ms,
            self.origin_ms,
        )?;
        let mut system = PcmLaneSender::new(
            &self.runtime,
            &self.session_id,
            ordinal,
            "system",
            spec.sample_rate,
            offset_ms,
            self.origin_ms,
        )?;
        let mut samples = reader.samples::<i16>();
        let mut mic_batch = Vec::with_capacity(4096);
        let mut system_batch = Vec::with_capacity(4096);
        loop {
            let Some(mic_sample) = samples.next() else {
                break;
            };
            let system_sample = samples
                .next()
                .context("native capture WAV has a partial stereo frame")??;
            mic_batch.push(mic_sample? as f32 / 32767.0);
            system_batch.push(system_sample as f32 / 32767.0);
            if mic_batch.len() == 4096 {
                mic.push_samples(&self.runtime, &mic_batch)?;
                system.push_samples(&self.runtime, &system_batch)?;
                mic_batch.clear();
                system_batch.clear();
            }
        }
        if !mic_batch.is_empty() {
            mic.push_samples(&self.runtime, &mic_batch)?;
            system.push_samples(&self.runtime, &system_batch)?;
        }
        let (mic_count, mic_frames) = mic.finish(&self.runtime)?;
        let (system_count, system_frames) = system.finish(&self.runtime)?;
        if mic_count == 0 && system_count == 0 {
            self.runtime
                .storage()
                .discard_empty_native_segment(self.session_id.as_ref(), ordinal)?;
            return Ok(0);
        }
        let duration_ms = mic_frames.max(system_frames).div_ceil(16);
        let boundaries = [("mic", mic_count), ("system", system_count)]
            .into_iter()
            .map(|(lane, count)| LaneBoundaryV1 {
                lane_id: lane.into(),
                next_sequence: count,
            })
            .collect();
        let close_id = format!("{segment}-close");
        let close = CloseSegmentV1 {
            segment_id: segment.clone().into(),
            ended_at_ms: SessionMillis(offset_ms + duration_ms),
            lane_boundaries: boundaries,
            reason,
        };
        self.runtime
            .recorder()
            .close_segment(
                &self.session_id,
                close_id.clone().into(),
                UnixMillis(self.origin_ms.saturating_add(offset_ms + duration_ms)),
                close,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.runtime
            .storage()
            .link_native_wav(self.session_id.as_ref(), &segment, ordinal)?;
        self.closes.push(SegmentCloseReferenceV1 {
            segment_id: segment.into(),
            close_message_id: close_id.into(),
        });
        self.last_end_ms = self.last_end_ms.max(offset_ms + duration_ms);
        Ok(duration_ms)
    }

    pub(super) fn finish(&self, ended_at_ms: u64) -> Result<()> {
        self.finish_with_reason(ended_at_ms, SessionFinalizeReasonV1::Completed)
    }

    pub(super) fn finish_with_reason(
        &self,
        ended_at_ms: u64,
        reason: SessionFinalizeReasonV1,
    ) -> Result<()> {
        self.runtime
            .recorder()
            .finish(
                &self.session_id,
                format!(
                    "{}-finish-{}",
                    self.session_id.as_ref(),
                    uuid::Uuid::new_v4()
                )
                .into(),
                UnixMillis(
                    self.origin_ms
                        .saturating_add(ended_at_ms.max(self.last_end_ms)),
                ),
                FinalizeSessionV1 {
                    ended_at_ms: SessionMillis(ended_at_ms.max(self.last_end_ms)),
                    segment_closes: self.closes.clone(),
                    reason,
                },
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok(())
    }

    pub(super) fn register_transcript(&self, ordinal: i64) -> Result<()> {
        self.runtime
            .storage()
            .register_native_transcript(self.session_id.as_ref(), ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_meeting_protocol::{ClientMessageBodyV1, ClientMessageV1, ServerMessageBodyV1};
    use margins_meeting_runtime::test_support::{
        assert_recorder_conformance, RecorderConformanceAdapter,
    };
    use std::io::Write;

    struct NativeSourceFake {
        directory: tempfile::TempDir,
        runtime: MeetingRuntime<SqliteMeetingRuntimeStorage>,
    }

    impl NativeSourceFake {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let runtime =
                MeetingRuntime::new(SqliteMeetingRuntimeStorage::open(directory.path()).unwrap());
            Self { directory, runtime }
        }
    }

    impl RecorderConformanceAdapter for NativeSourceFake {
        type Error = margins_meeting_runtime::RecorderError<anyhow::Error>;
        fn send(
            &mut self,
            message: ClientMessageV1,
        ) -> std::result::Result<margins_meeting_runtime::RuntimeResponseV1, Self::Error> {
            let recorder = self.runtime.recorder();
            let session = &message.session_id;
            let id = message.message_id;
            let sent_at = message.sent_at_unix_ms;
            match message.body {
                ClientMessageBodyV1::CreateSession(create) => {
                    recorder.reserve(session, id, sent_at, create)
                }
                ClientMessageBodyV1::AudioChunk(chunk) => {
                    let lane = recorder.open_lane(session, chunk.segment_id, chunk.lane_id)?;
                    recorder.append_chunk(
                        &lane,
                        id,
                        sent_at,
                        chunk.sequence,
                        chunk.starts_at_ms,
                        chunk.duration_ms,
                        chunk.payload,
                    )
                }
                ClientMessageBodyV1::CloseSegment(close) => {
                    recorder.close_segment(session, id, sent_at, close)
                }
                ClientMessageBodyV1::FinalizeSession(finalize) => {
                    recorder.finish(session, id, sent_at, finalize)
                }
                ClientMessageBodyV1::ResumeSession(resume) => recorder
                    .recover(session, id, sent_at, resume.after_server_sequence)
                    .map(|(_, response)| response),
                ClientMessageBodyV1::BeginCaptureGeneration(begin) => {
                    recorder.start_generation(session, id, sent_at, begin)
                }
                _ => Err(margins_meeting_runtime::RecorderError::InvalidTransition(
                    "native TUI does not emit this command",
                )),
            }
        }
        fn restart(&mut self) {
            self.runtime = MeetingRuntime::new(
                SqliteMeetingRuntimeStorage::open(self.directory.path()).unwrap(),
            );
        }
    }

    #[test]
    fn tui_runtime_storage_obeys_recorder_conformance() {
        let mut adapter = NativeSourceFake::new();
        assert_recorder_conformance(&mut adapter, "fake-native-source");
    }

    #[test]
    fn fake_stereo_source_is_journaled_and_finalized() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("fake_seg0.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..4800 {
            writer.write_sample(100i16).unwrap();
            writer.write_sample(-100i16).unwrap();
        }
        writer.finalize().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "fake", None, chrono::Local::now()).unwrap();
        producer.open(0, 0).unwrap();
        assert_eq!(
            producer
                .ingest_wav_and_close(0, &wav, 0, SegmentCloseReasonV1::Stop)
                .unwrap(),
            100
        );
        let db = rusqlite::Connection::open(margins_store::canonical::database_path(dir.path()))
            .unwrap();
        db.execute(
            "UPDATE session_segments SET wav_path = '.margins/unlinked.pcm' WHERE session_name = 'fake'",
            [],
        )
        .unwrap();
        drop(db);
        producer.finish(100).unwrap();
        let (_, replay) = producer
            .runtime
            .recorder()
            .recover(&producer.session_id, "verify".into(), now_ms(), None)
            .unwrap();
        assert!(replay
            .messages
            .iter()
            .any(|message| matches!(message.body, ServerMessageBodyV1::SessionFinalized(_))));
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            1
        );
        let reopened =
            LocalMeetingProducer::recover(dir.path(), "fake", 200, None, chrono::Local::now())
                .unwrap();
        let db = rusqlite::Connection::open(margins_store::canonical::database_path(dir.path()))
            .unwrap();
        let linked: String = db
            .query_row(
                "SELECT wav_path FROM session_segments WHERE session_name = 'fake'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked, ".margins/fake_seg0.wav");
        reopened.open(1, 200).unwrap();
    }

    #[test]
    fn attach_lazily_adopts_finalized_pre_runtime_session() {
        let dir = tempfile::tempdir().unwrap();
        let started_at = chrono::Local::now();
        margins_store::canonical::create_session(dir.path(), "old", &started_at, ".margins/old.md")
            .unwrap();
        margins_store::canonical::add_segment(
            dir.path(),
            "old",
            0,
            ".margins/old_seg0.wav",
            0,
            Some(1.0),
        )
        .unwrap();
        let producer =
            LocalMeetingProducer::recover(dir.path(), "old", 2_000, None, started_at).unwrap();
        producer.open(1, 2_000).unwrap();
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            2
        );
    }

    #[test]
    fn attach_adopts_pre_runtime_session_without_finalized_segments() {
        let dir = tempfile::tempdir().unwrap();
        let started_at = chrono::Local::now();
        margins_store::canonical::create_session(
            dir.path(),
            "crashed",
            &started_at,
            ".margins/crashed.md",
        )
        .unwrap();
        let producer =
            LocalMeetingProducer::recover(dir.path(), "crashed", 2_000, None, started_at).unwrap();
        producer.open(0, 0).unwrap();
    }

    #[test]
    fn open_segment_is_visible_and_empty_segment_does_not_consume_an_ordinal() {
        let dir = tempfile::tempdir().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "visible", None, chrono::Local::now())
                .unwrap();
        producer.open(0, 0).unwrap();
        let sessions = margins_store::canonical::list_sessions(dir.path()).unwrap();
        assert_eq!(sessions[0].segment_count, 1);
        assert_eq!(sessions[0].lifecycle_state, "active");
        assert_eq!(producer.pending_segment().unwrap(), Some((0, 0)));
        assert_eq!(
            producer
                .ingest_wav_and_close(
                    0,
                    &dir.path().join("absent.wav"),
                    0,
                    SegmentCloseReasonV1::Pause
                )
                .unwrap(),
            0
        );
        assert_eq!(producer.next_ordinal().unwrap(), 0);
        producer.open(0, 100).unwrap();
    }

    #[test]
    fn fake_native_spools_stream_during_capture_and_export_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "streamed", None, chrono::Local::now())
                .unwrap();
        let mic_path = dir.path().join("mic.f32");
        let system_path = dir.path().join("system.f32");
        let mut mic = std::fs::File::create(&mic_path).unwrap();
        let mut system = std::fs::File::create(&system_path).unwrap();
        producer
            .start_stream(
                0,
                0,
                [
                    crate::recorder::NativeSpoolSource::fake(mic_path, 16_000),
                    crate::recorder::NativeSpoolSource::fake(system_path, 16_000),
                ],
            )
            .unwrap();
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            1
        );
        let five_seconds = vec![0.125f32.to_le_bytes(); 5 * 16_000].concat();
        mic.write_all(&five_seconds).unwrap();
        system.write_all(&five_seconds).unwrap();
        mic.sync_all().unwrap();
        system.sync_all().unwrap();
        let started = std::time::Instant::now();
        while producer
            .runtime
            .storage()
            .native_chunk_boundaries("streamed", 0)
            .unwrap()[0]
            .0
            == 0
        {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        let one_second = vec![0.25f32.to_le_bytes(); 16_000].concat();
        mic.write_all(&one_second).unwrap();
        system.write_all(&one_second).unwrap();
        mic.sync_all().unwrap();
        system.sync_all().unwrap();
        assert_eq!(
            producer
                .flush_stream_and_close(0, 0, SegmentCloseReasonV1::Stop)
                .unwrap(),
            6_000
        );
        producer.finish(6_000).unwrap();
        let stats = producer.runtime.storage().stats().unwrap();
        assert_eq!(stats.blob_bytes, 6 * 16_000 * 2 * 2);
        assert!(!dir.path().join("artifacts/streamed").exists());
        let wav = producer
            .runtime
            .storage()
            .export_native_wav("streamed", 0)
            .unwrap();
        let reader = hound::WavReader::open(&wav).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.duration(), 6 * 16_000);
        drop(reader);
        std::fs::write(&wav, b"user kept export").unwrap();
        producer
            .runtime
            .storage()
            .export_native_wav("streamed", 0)
            .unwrap();
        assert_eq!(std::fs::read(wav).unwrap(), b"user kept export");
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn native_lane_actor_compacts_after_runtime_commit() {
        let dir = tempfile::tempdir().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "compact", None, chrono::Local::now())
                .unwrap();
        let writer = crate::recorder::SegmentWriter::start(0, 16_000, 16_000, None).unwrap();
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        writer.bound_spool(60, overflow.clone()).unwrap();
        let sources = writer.native_spool_sources();
        let paths = [sources[0].path.clone(), sources[1].path.clone()];
        producer.start_stream(0, 0, sources).unwrap();
        let mic = writer.mic_sink(1);
        let system = writer.system_sink(2);
        mic.attach(16_000, 0).unwrap();
        system.attach(16_000, 0).unwrap();
        mic.samples(16_000, vec![0.1; 80_000], Vec::new()).unwrap();
        system
            .samples(16_000, vec![0.2; 80_000], Vec::new())
            .unwrap();
        mic.samples(16_000, vec![0.1; 16_000], Vec::new()).unwrap();
        system
            .samples(16_000, vec![0.2; 16_000], Vec::new())
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            let boundaries = producer
                .runtime
                .storage()
                .native_chunk_boundaries("compact", 0)
                .unwrap();
            let size = paths
                .iter()
                .map(|path| std::fs::metadata(path).unwrap().len())
                .sum::<u64>();
            if boundaries[0].0 > 0 && boundaries[1].0 > 0 && size < 200_000 {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        mic.retire().unwrap();
        system.retire().unwrap();
        let sealed = writer.seal_at(96_000).unwrap();
        assert_eq!(
            producer
                .flush_stream_and_close(0, 0, SegmentCloseReasonV1::Stop)
                .unwrap(),
            6_000
        );
        sealed.finish_without_wav(|| Ok(())).unwrap();
        assert!(!overflow.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(feature = "audio-capture")]
    #[test]
    fn storage_backlog_exit_saves_memo_and_can_export_then_attach() {
        let dir = tempfile::tempdir().unwrap();
        let started_at = chrono::Local::now();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "backlog", None, started_at).unwrap();
        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(dir.path()).unwrap();
        let observed = authority.memo("backlog").unwrap();
        let mut app = crate::app::App::new(
            dir.path().join("backlog.md").to_string_lossy().into_owned(),
            started_at,
            "fake mic".into(),
        );
        app.bind_workspace_authority(dir.path().to_path_buf(), "backlog".into());
        app.observe_memo(observed.revision, observed.lines);
        for letter in "keep this memo".chars() {
            app.insert_char(letter);
        }
        app.enter();
        // Hold off runtime ingestion until the actor hits its real 60-second
        // cap, then follow the TUI's memo -> drain -> error-finalize path.
        let writer = crate::recorder::SegmentWriter::start(0, 100, 100, None).unwrap();
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        writer.bound_spool(60, overflow.clone()).unwrap();
        let sources = writer.native_spool_sources();
        let mic = writer.mic_sink(1);
        let system = writer.system_sink(2);
        mic.attach(100, 0).unwrap();
        system.attach(100, 0).unwrap();
        mic.samples(100, vec![0.1; 6_000], Vec::new()).unwrap();
        system.samples(100, vec![0.1; 6_000], Vec::new()).unwrap();
        mic.samples(100, vec![0.1; 100], Vec::new()).unwrap();
        mic.retire().unwrap();
        system.retire().unwrap();
        assert!(overflow.load(std::sync::atomic::Ordering::Acquire));

        app.save().unwrap();
        producer.start_stream(0, 0, sources).unwrap();
        let sealed = writer.seal_at(6_000).unwrap();
        assert_eq!(
            producer
                .flush_stream_and_close(0, 0, SegmentCloseReasonV1::Error)
                .unwrap(),
            60_000
        );
        sealed.finish_without_wav(|| Ok(())).unwrap();
        producer
            .finish_with_reason(60_000, SessionFinalizeReasonV1::Error)
            .unwrap();
        assert_eq!(
            authority.memo("backlog").unwrap().lines[0].text,
            "keep this memo"
        );
        let wav = producer
            .runtime
            .storage()
            .export_native_wav("backlog", 0)
            .unwrap();
        assert_eq!(hound::WavReader::open(wav).unwrap().duration(), 960_000);
        let resumed =
            LocalMeetingProducer::recover(dir.path(), "backlog", 61_000, None, started_at).unwrap();
        assert_eq!(resumed.next_ordinal().unwrap(), 1);
        resumed.open(1, 61_000).unwrap();
    }

    /// Performance fixture for the stop path: an hour reaches the runtime
    /// while capture is live, so closing only drains the last partial batch.
    #[test]
    #[ignore = "writes one hour of canonical PCM to measure storage and stop latency"]
    fn fake_hour_stream_has_bounded_stop_latency() {
        let dir = tempfile::tempdir().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "hour", None, chrono::Local::now()).unwrap();
        let mic_path = dir.path().join("mic.f32");
        let system_path = dir.path().join("system.f32");
        let mut mic = std::fs::File::create(&mic_path).unwrap();
        let mut system = std::fs::File::create(&system_path).unwrap();
        producer
            .start_stream(
                0,
                0,
                [
                    crate::recorder::NativeSpoolSource::fake(mic_path.clone(), 16_000),
                    crate::recorder::NativeSpoolSource::fake(system_path.clone(), 16_000),
                ],
            )
            .unwrap();
        let five_seconds = vec![0.125f32.to_le_bytes(); 5 * 16_000].concat();
        for batch in 0..720 {
            mic.write_all(&five_seconds).unwrap();
            system.write_all(&five_seconds).unwrap();
            let wait_started = std::time::Instant::now();
            loop {
                let boundary = producer
                    .runtime
                    .storage()
                    .native_chunk_boundaries("hour", 0)
                    .unwrap();
                if boundary[0].0 > batch && boundary[1].0 > batch {
                    break;
                }
                assert!(wait_started.elapsed() < Duration::from_secs(30));
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let tail = vec![0.25f32.to_le_bytes(); 16_000].concat();
        mic.write_all(&tail).unwrap();
        system.write_all(&tail).unwrap();
        let stopped = std::time::Instant::now();
        let duration = producer
            .flush_stream_and_close(0, 0, SegmentCloseReasonV1::Stop)
            .unwrap();
        let stop_latency = stopped.elapsed();
        assert_eq!(duration, 3_601_000);
        producer.finish(duration).unwrap();
        let stats = producer.runtime.storage().stats().unwrap();
        assert_eq!(stats.blob_bytes, 3_601 * 16_000 * 2 * 2);
        std::fs::remove_file(mic_path).unwrap();
        std::fs::remove_file(system_path).unwrap();
        fn total_files(path: &Path) -> u64 {
            std::fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        total_files(&path)
                    } else {
                        std::fs::metadata(path).unwrap().len()
                    }
                })
                .sum()
        }
        eprintln!(
            "FAKE_HOUR stop_ms={} blob_bytes={} total_files_bytes={} crash_loss_window_seconds<=5",
            stop_latency.as_millis(),
            stats.blob_bytes,
            total_files(dir.path())
        );
        assert!(stop_latency < Duration::from_secs(30));
    }

    #[test]
    fn restart_finalizes_durable_pcm_without_replaying_a_sealed_wav() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("replay_seg0.wav");
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
        for _ in 0..3200 {
            writer.write_sample(7i16).unwrap();
            writer.write_sample(8i16).unwrap();
        }
        writer.finalize().unwrap();
        let first = LocalMeetingProducer::reserve(dir.path(), "replay", None, chrono::Local::now())
            .unwrap();
        first.open(0, 0).unwrap();
        let mic = first
            .runtime
            .recorder()
            .open_lane(&first.session_id, "replay-seg-0".into(), "mic".into())
            .unwrap();
        first
            .runtime
            .recorder()
            .append_chunk(
                &mic,
                "replay-seg-0-mic-0".into(),
                UnixMillis(first.origin_ms),
                0,
                SessionMillis(0),
                DurationMillis(100),
                (0..1600).flat_map(|_| 7i16.to_le_bytes()).collect(),
            )
            .unwrap();
        drop(first);
        let mut resumed =
            LocalMeetingProducer::recover(dir.path(), "replay", 200, None, chrono::Local::now())
                .unwrap();
        assert_eq!(resumed.existing_segment_start(0).unwrap(), Some(0));
        assert_eq!(resumed.pending_segment().unwrap(), Some((0, 0)));
        resumed.recover_pending_segment(0, 0).unwrap();
        resumed.finish(200).unwrap();
        assert_eq!(std::fs::metadata(&wav).unwrap().len(), 44 + 3200 * 2 * 2);
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            1
        );
    }
}
