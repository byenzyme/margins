//! In-process meeting producer for the native TUI. The native device adapter
//! still creates the familiar stereo WAV; this adapter journals its PCM lanes
//! through the same Recorder facade used by transport producers.

use anyhow::{bail, Context, Result};
use margins_meeting_protocol::{
    AudioCodecV1, AudioContainerV1, AudioFormatV1, BeginCaptureGenerationV1, CaptureLaneV1,
    CaptureModeV1, CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1,
    CaptureSourceV1, CloseSegmentV1, CreateSessionV1, DurationMillis, FinalizeSessionV1,
    LaneBoundaryV1, SegmentCloseReasonV1, SegmentCloseReferenceV1, SessionFinalizeReasonV1,
    SessionId, SessionMillis, UnixMillis,
};
use margins_meeting_runtime::{MeetingRuntime, MeetingRuntimeStorage};
use margins_store::SqliteMeetingRuntimeStorage;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) struct LocalMeetingProducer {
    runtime: MeetingRuntime<SqliteMeetingRuntimeStorage>,
    session_id: SessionId,
    closes: Vec<SegmentCloseReferenceV1>,
    last_end_ms: u64,
    origin_ms: u64,
}

fn now_ms() -> UnixMillis {
    UnixMillis(chrono::Utc::now().timestamp_millis().max(0) as u64)
}

fn segment_id(name: &str, ordinal: i64) -> String {
    format!("{name}-seg-{ordinal}")
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
            storage.allow_finalized_legacy_adoption_once();
        }
        let runtime = MeetingRuntime::new(storage);
        let origin_ms = started_at.timestamp_millis().max(0) as u64;
        let session_id = SessionId::from(name.to_owned());
        let format = AudioFormatV1 {
            codec: AudioCodecV1::PcmS16Le,
            container: AudioContainerV1::Raw,
            sample_rate_hz: 16_000,
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
        })
    }

    pub(super) fn open(&self, ordinal: i64) -> Result<()> {
        let id = segment_id(self.session_id.as_ref(), ordinal);
        for lane in ["mic", "system"] {
            self.runtime
                .recorder()
                .open_lane(&self.session_id, id.clone().into(), lane.into())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        Ok(())
    }

    pub(super) fn existing_segment_start(&self, ordinal: i64) -> Result<Option<u64>> {
        let id = segment_id(self.session_id.as_ref(), ordinal);
        Ok(self
            .runtime
            .storage()
            .load_audio_chunk(&self.session_id, &id, &"mic".into(), 0)?
            .map(|chunk| chunk.starts_at_ms.0))
    }

    /// The native recorder's sealed WAV is the lossless source for both lanes.
    /// Chunk IDs and sequences are deterministic so a retry after a process
    /// interruption can replay the same file without duplicating audio.
    pub(super) fn ingest_wav_and_close(
        &mut self,
        ordinal: i64,
        wav_path: &Path,
        offset_ms: u64,
        reason: SegmentCloseReasonV1,
    ) -> Result<u64> {
        let samples = if wav_path.exists() {
            let mut reader = hound::WavReader::open(wav_path)
                .with_context(|| format!("could not read capture {}", wav_path.display()))?;
            let spec = reader.spec();
            if spec.channels != 2
                || spec.sample_rate != 16_000
                || spec.bits_per_sample != 16
                || spec.sample_format != hound::SampleFormat::Int
            {
                bail!("native capture WAV must be 16 kHz stereo s16 PCM");
            }
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        if samples.len() % 2 != 0 {
            bail!("native capture WAV has a partial stereo frame");
        }
        let frames = samples.len() / 2;
        let duration_ms = ((frames as u64) * 1000).div_ceil(16_000);
        let segment = segment_id(self.session_id.as_ref(), ordinal);
        let mut boundaries = Vec::new();
        for (lane, channel) in [("mic", 0usize), ("system", 1usize)] {
            let handle = self
                .runtime
                .recorder()
                .open_lane(&self.session_id, segment.clone().into(), lane.into())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let mut sequence = 0u64;
            for chunk in samples.chunks(1600 * 2) {
                let payload: Vec<u8> = chunk
                    .chunks_exact(2)
                    .flat_map(|pair| pair[channel].to_le_bytes())
                    .collect();
                let chunk_frames = (chunk.len() / 2) as u64;
                let starts_at = offset_ms + sequence * 100;
                self.runtime
                    .recorder()
                    .append_chunk(
                        &handle,
                        format!("{segment}-{lane}-{sequence}").into(),
                        UnixMillis(self.origin_ms.saturating_add(starts_at)),
                        sequence,
                        SessionMillis(starts_at),
                        DurationMillis((chunk_frames * 1000).div_ceil(16_000)),
                        payload,
                    )
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                sequence += 1;
            }
            boundaries.push(LaneBoundaryV1 {
                lane_id: lane.into(),
                next_sequence: sequence,
            });
        }
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
        if frames > 0 {
            self.runtime
                .storage()
                .link_native_wav(self.session_id.as_ref(), &segment, ordinal)?;
        }
        self.closes.push(SegmentCloseReferenceV1 {
            segment_id: segment.into(),
            close_message_id: close_id.into(),
        });
        self.last_end_ms = self.last_end_ms.max(offset_ms + duration_ms);
        Ok(duration_ms)
    }

    pub(super) fn finish(&self, ended_at_ms: u64) -> Result<()> {
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
                    reason: SessionFinalizeReasonV1::Completed,
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
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..1600 {
            writer.write_sample(100i16).unwrap();
            writer.write_sample(-100i16).unwrap();
        }
        writer.finalize().unwrap();
        let mut producer =
            LocalMeetingProducer::reserve(dir.path(), "fake", None, chrono::Local::now()).unwrap();
        producer.open(0).unwrap();
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
        reopened.open(1).unwrap();
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
        producer.open(1).unwrap();
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            1
        );
    }

    #[test]
    fn restart_replays_durable_pcm_before_closing_a_sealed_wav() {
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
        resumed.open(0).unwrap();
        resumed
            .ingest_wav_and_close(0, &wav, 0, SegmentCloseReasonV1::Error)
            .unwrap();
        resumed.finish(200).unwrap();
        assert_eq!(
            margins_store::canonical::list_sessions(dir.path()).unwrap()[0].segment_count,
            1
        );
    }
}
