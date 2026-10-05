//! Backend-independent live decode loop.
//!
//! The recorder's live queue is bounded. While the speech model warms up (or
//! decodes slower than real time) the segment writer substitutes silence for
//! audio that missed the queue, and the queue may stop short of a segment's
//! end. The meeting runtime still holds every captured frame as durable 16 kHz
//! PCM, so this loop replaces each missed span with that durable audio before
//! it reaches the decoder. Each frame of a segment lane is handed to the
//! decoder exactly once, in order: real queued frames, durable replacements
//! for synthesized or missing frames, then the durable tail once the segment
//! closes. Any span the durable store cannot supply stays silent and keeps the
//! checkpoint non-terminal.

use super::{live_checkpoint_complete, write_checkpoint, DurableLiveAudio, LiveDurableSource};
use crate::recorder::{LiveAudioChannel, LiveAudioChunk};
use anyhow::{bail, Context, Result};
use margins_core::AsrStreamDecoder;
use margins_media::timeline::RationalResampler;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const RATE_16K: u64 = 16_000;
/// Largest block moved to the decoder at once: one second for synthesized
/// spans and gaps, five seconds (one runtime batch) for durable tails.
const SYNTH_BLOCK_SECONDS: u64 = 1;
const TAIL_BLOCK_FRAMES: usize = 5 * RATE_16K as usize;
/// Queued native samples (both lanes) above which the worker reports that it
/// is still catching up rather than live.
pub(super) const LIVE_LAG_STATUS_SAMPLES: u64 = 48_000 * 2 * 3;
/// How long to wait for the runtime to commit a missed span. The runtime
/// commits five-second batches, so the newest part of a span is briefly not
/// durable yet. The budget is spent once: after one expiry the runtime is
/// treated as stalled and later spans are read without waiting.
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
pub(super) const DURABLE_WAIT: Duration = Duration::from_secs(30);
const DURABLE_POLL: Duration = Duration::from_millis(200);

pub(super) struct LiveWorkerIo {
    pub rx: mpsc::Receiver<LiveAudioChunk>,
    pub finish_rx: mpsc::Receiver<u64>,
    pub checkpoint: PathBuf,
    pub offset_ms: u64,
    pub queued_samples: Arc<AtomicU64>,
    pub mic_dropped_samples: Arc<AtomicU64>,
    pub system_dropped_samples: Arc<AtomicU64>,
    pub status: Arc<AtomicU8>,
    /// 16 kHz frames that reached the decoder as silence because neither the
    /// queue nor the durable runtime supplied them.
    pub unrecovered_frames: Arc<AtomicU64>,
    /// Recorder generation -> durable native segment ordinal.
    pub segments: Arc<Mutex<BTreeMap<u64, i64>>>,
    pub durable: Option<LiveDurableSource>,
    pub durable_wait: Duration,
}

struct Lane {
    channel: LiveAudioChannel,
    resampler: Option<RationalResampler>,
    /// Next native frame of the active segment not yet handed to the decoder.
    native_next: u64,
    /// 16 kHz frames of the active segment handed to the decoder.
    segment_frames: u64,
    /// 16 kHz frames on the live timeline, including inter-segment silence.
    timeline_frames: u64,
}

impl Lane {
    fn new(channel: LiveAudioChannel) -> Self {
        Self {
            channel,
            resampler: None,
            native_next: 0,
            segment_frames: 0,
            timeline_frames: 0,
        }
    }
}

struct Active {
    generation: Option<u64>,
    ordinal: Option<i64>,
}

struct Decode<'a, D: AsrStreamDecoder> {
    decoders: [&'a mut D; 2],
    lanes: [Lane; 2],
    checkpoint: PathBuf,
    offset_ms: u64,
    queued_samples: Arc<AtomicU64>,
    status: Arc<AtomicU8>,
    unrecovered_frames: Arc<AtomicU64>,
    segments: Arc<Mutex<BTreeMap<u64, i64>>>,
    durable: Option<Box<dyn DurableLiveAudio>>,
    durable_wait: Duration,
    active: Option<Active>,
    /// Next durable segment this transcript has not started yet.
    next_ordinal: Option<i64>,
    last_update_local_ms: u64,
    recovered_frames: u64,
    durable_error_logged: bool,
    /// A wait for durable audio expired; never wait again.
    durable_stalled: bool,
}

pub(super) fn run_live_worker<D: AsrStreamDecoder>(
    mic: &mut D,
    system: &mut D,
    io: LiveWorkerIo,
) -> Result<()> {
    let LiveWorkerIo {
        rx,
        finish_rx,
        checkpoint,
        offset_ms,
        queued_samples,
        mic_dropped_samples,
        system_dropped_samples,
        status,
        unrecovered_frames,
        segments,
        durable,
        durable_wait,
    } = io;
    let (durable, next_ordinal) = match durable {
        Some(source) => (Some(source.audio), Some(source.first_ordinal)),
        None => (None, None),
    };
    let mut decode = Decode {
        decoders: [mic, system],
        lanes: [
            Lane::new(LiveAudioChannel::Mic),
            Lane::new(LiveAudioChannel::System),
        ],
        checkpoint,
        offset_ms,
        queued_samples,
        status,
        unrecovered_frames,
        segments,
        durable,
        durable_wait,
        active: None,
        next_ordinal,
        last_update_local_ms: 0,
        recovered_frames: 0,
        durable_error_logged: false,
        durable_stalled: false,
    };
    let dropped = || {
        mic_dropped_samples.load(Ordering::Acquire) + system_dropped_samples.load(Ordering::Acquire)
    };
    decode.set_lagging(
        decode.queued_samples.load(Ordering::Acquire) > LIVE_LAG_STATUS_SAMPLES || dropped() > 0,
    );
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => decode.on_chunk(chunk)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let duration_ms = finish_rx
                    .recv()
                    .context("live transcription ended without a final duration")?;
                return decode.finish(duration_ms, dropped());
            }
        }
    }
}

impl<D: AsrStreamDecoder> Decode<'_, D> {
    fn on_chunk(&mut self, chunk: LiveAudioChunk) -> Result<()> {
        super::debit_queued_samples(&self.queued_samples, chunk.samples.len() as u64);
        let active_generation = self.active.as_ref().and_then(|active| active.generation);
        if active_generation != Some(chunk.generation) {
            if active_generation.is_some_and(|generation| chunk.generation < generation) {
                // A retired recorder's late output. Its segment's tail was
                // already taken from the durable runtime.
                return Ok(());
            }
            let ordinal = self
                .segments
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&chunk.generation)
                .copied();
            self.end_segment()?;
            if ordinal.is_some() {
                self.catch_up_unseen_segments(ordinal)?;
            }
            self.begin_segment(Some(chunk.generation), ordinal, chunk.session_offset_ms)?;
        }

        let index = lane_index(chunk.channel);
        let rate = chunk.sample_rate;
        if self.lanes[index]
            .resampler
            .as_ref()
            .is_none_or(|resampler| resampler.from_rate() != rate)
        {
            self.lanes[index].resampler = Some(RationalResampler::new(rate, RATE_16K as u32)?);
        }
        let mut samples = chunk.samples;
        let mut start = chunk.start_frame;
        let native_next = self.lanes[index].native_next;
        if start < native_next {
            // Never decode a frame twice.
            let overlap = usize::try_from(native_next - start).unwrap_or(usize::MAX);
            if overlap >= samples.len() {
                return Ok(());
            }
            samples.drain(..overlap);
            start = native_next;
        }
        let hole = start - native_next;
        if hole > 0 {
            // Frames that never reached the queue at all.
            self.recover_native(index, hole)?;
        }
        if chunk.synthesized {
            self.recover_native(index, samples.len() as u64)?;
        } else {
            let converted = self.lanes[index]
                .resampler
                .as_mut()
                .expect("resampler initialized")
                .process(&samples)?;
            self.lanes[index].native_next += samples.len() as u64;
            self.append(index, &converted);
            self.maybe_update()?;
        }
        self.set_lagging(
            chunk.synthesized
                || hole > 0
                || self.queued_samples.load(Ordering::Acquire) > LIVE_LAG_STATUS_SAMPLES,
        );
        Ok(())
    }

    /// Advance a lane over `count` native frames whose real audio is only in
    /// the durable runtime. The resampler sees silence so its frame accounting
    /// stays exact; its output is then replaced with the durable frames.
    fn recover_native(&mut self, index: usize, count: u64) -> Result<()> {
        let rate = u64::from(
            self.lanes[index]
                .resampler
                .as_ref()
                .expect("resampler initialized")
                .from_rate(),
        );
        let block = rate * SYNTH_BLOCK_SECONDS;
        let mut remaining = count;
        while remaining > 0 {
            let frames = remaining.min(block);
            let silence = vec![0.0; frames as usize];
            let converted = self.lanes[index]
                .resampler
                .as_mut()
                .expect("resampler initialized")
                .process(&silence)?;
            self.lanes[index].native_next += frames;
            remaining -= frames;
            let from = self.lanes[index].segment_frames;
            let replacement = self.read_durable(index, from, converted.len(), true)?;
            self.append(index, &replacement);
            self.maybe_update()?;
        }
        Ok(())
    }

    /// Exactly `count` frames starting at `from` in the active segment lane:
    /// durable audio where available, silence (counted as unrecovered) where
    /// not. `wait` allows the runtime time to commit its newest batch.
    fn read_durable(
        &mut self,
        index: usize,
        from: u64,
        count: usize,
        wait: bool,
    ) -> Result<Vec<f32>> {
        let channel = self.lanes[index].channel;
        let ordinal = self.active.as_ref().and_then(|active| active.ordinal);
        let mut out = Vec::with_capacity(count);
        let wait = wait && !self.durable_stalled;
        let mut source = self.durable.take();
        if let (Some(durable), Some(ordinal)) = (source.as_mut(), ordinal) {
            let deadline = Instant::now() + self.durable_wait;
            while out.len() < count {
                match durable.read(ordinal, channel, from + out.len() as u64, count - out.len()) {
                    Ok(frames) if !frames.is_empty() => out.extend(frames),
                    Ok(_) if wait && Instant::now() < deadline => {
                        self.set_lagging(true);
                        std::thread::sleep(DURABLE_POLL.min(self.durable_wait));
                    }
                    Ok(_) => {
                        if wait && !self.durable_stalled {
                            self.durable_stalled = true;
                            crate::cli_log::event(
                                "live_catch_up_stalled",
                                format!(
                                    "lane={channel:?} ordinal={ordinal} from_frame={} wait_ms={}",
                                    from + out.len() as u64,
                                    self.durable_wait.as_millis()
                                ),
                            );
                        }
                        break;
                    }
                    Err(error) => {
                        if !self.durable_error_logged {
                            self.durable_error_logged = true;
                            crate::cli_log::event(
                                "live_catch_up_read_failed",
                                crate::cli_log::error_summary(&error),
                            );
                        }
                        break;
                    }
                }
            }
        }
        self.durable = source;
        out.truncate(count);
        self.recovered_frames += out.len() as u64;
        let missing = count - out.len();
        if missing > 0 {
            self.unrecovered_frames
                .fetch_add(missing as u64, Ordering::AcqRel);
            crate::cli_log::event(
                "live_catch_up_unavailable",
                format!(
                    "lane={channel:?} ordinal={ordinal:?} from_frame={} missing_frames={missing}",
                    from + out.len() as u64
                ),
            );
            out.resize(count, 0.0);
        }
        Ok(out)
    }

    fn begin_segment(
        &mut self,
        generation: Option<u64>,
        ordinal: Option<i64>,
        session_offset_ms: u64,
    ) -> Result<()> {
        let start_frame = session_offset_ms
            .saturating_sub(self.offset_ms)
            .saturating_mul(RATE_16K)
            / 1_000;
        for index in 0..2 {
            let gap = start_frame.saturating_sub(self.lanes[index].timeline_frames);
            let mut remaining = gap;
            while remaining > 0 {
                let frames = remaining.min(RATE_16K * SYNTH_BLOCK_SECONDS);
                self.decoders[index].append_audio(&vec![0.0; frames as usize]);
                self.lanes[index].timeline_frames += frames;
                remaining -= frames;
            }
            let lane = &mut self.lanes[index];
            lane.resampler = None;
            lane.native_next = 0;
            lane.segment_frames = 0;
        }
        if let Some(ordinal) = ordinal {
            self.next_ordinal = Some(ordinal + 1);
        }
        self.active = Some(Active {
            generation,
            ordinal,
        });
        self.maybe_update()
    }

    /// Hand the decoder whatever the closed active segment holds beyond the
    /// frames the queue delivered.
    fn end_segment(&mut self) -> Result<()> {
        let Some(ordinal) = self.active.as_ref().and_then(|active| active.ordinal) else {
            return Ok(());
        };
        if self.durable.is_none() {
            return Ok(());
        }
        for index in 0..2 {
            loop {
                let from = self.lanes[index].segment_frames;
                let channel = self.lanes[index].channel;
                let durable = self.durable.as_mut().expect("durable source present");
                let frames = match durable.read(ordinal, channel, from, TAIL_BLOCK_FRAMES) {
                    Ok(frames) => frames,
                    Err(error) => {
                        // The tail length is unknown, so completeness cannot
                        // be shown; record at least one unrecovered frame.
                        self.unrecovered_frames.fetch_add(1, Ordering::AcqRel);
                        crate::cli_log::event(
                            "live_catch_up_tail_failed",
                            crate::cli_log::error_summary(&error),
                        );
                        break;
                    }
                };
                if frames.is_empty() {
                    break;
                }
                self.recovered_frames += frames.len() as u64;
                self.append(index, &frames);
                self.maybe_update()?;
            }
        }
        Ok(())
    }

    /// Decode durable segments the queue never delivered a chunk for, in
    /// ordinal order, up to (not including) `until`; `None` means through the
    /// last durable segment. Ordinals without durable audio are skipped.
    fn catch_up_unseen_segments(&mut self, until: Option<i64>) -> Result<()> {
        let (Some(durable), Some(next)) = (self.durable.as_mut(), self.next_ordinal) else {
            return Ok(());
        };
        let segments = match durable.segments() {
            Ok(segments) => segments,
            Err(error) => {
                self.unrecovered_frames.fetch_add(1, Ordering::AcqRel);
                crate::cli_log::event(
                    "live_catch_up_segment_failed",
                    crate::cli_log::error_summary(&error),
                );
                return Ok(());
            }
        };
        for (ordinal, start_ms) in segments {
            if ordinal < next {
                continue;
            }
            if until.is_some_and(|until| ordinal >= until) {
                break;
            }
            crate::cli_log::event(
                "live_catch_up_segment",
                format!("ordinal={ordinal} start_ms={start_ms}"),
            );
            self.begin_segment(None, Some(ordinal), start_ms)?;
            self.end_segment()?;
        }
        Ok(())
    }

    fn append(&mut self, index: usize, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.decoders[index].append_audio(samples);
        let lane = &mut self.lanes[index];
        lane.segment_frames += samples.len() as u64;
        lane.timeline_frames += samples.len() as u64;
    }

    fn local_end_ms(&self) -> u64 {
        self.lanes[0]
            .timeline_frames
            .max(self.lanes[1].timeline_frames)
            .saturating_mul(1_000)
            / RATE_16K
    }

    fn maybe_update(&mut self) -> Result<()> {
        let local_end_ms = self.local_end_ms();
        if local_end_ms.saturating_sub(self.last_update_local_ms) < 3_000 {
            return Ok(());
        }
        let [mic, system] = &mut self.decoders;
        let mic = mic.update_until(local_end_ms)?;
        let system = system.update_until(local_end_ms)?;
        write_checkpoint(
            &self.checkpoint,
            &mic,
            &system,
            self.offset_ms,
            false,
            None,
            0,
            self.recovered_frames,
        )?;
        self.last_update_local_ms = local_end_ms;
        Ok(())
    }

    fn set_lagging(&self, lagging: bool) {
        let next = if lagging {
            crate::app::LIVE_TRANSCRIPTION_CATCHING_UP
        } else {
            crate::app::LIVE_TRANSCRIPTION_READY
        };
        let previous = self.status.load(Ordering::Acquire);
        if previous == next || previous == crate::app::LIVE_TRANSCRIPTION_DEGRADED {
            return;
        }
        self.status.store(next, Ordering::Release);
        crate::cli_log::event(
            if lagging {
                "live_worker_catching_up"
            } else {
                "live_worker_caught_up"
            },
            format!(
                "queued={} recovered_frames={} unrecovered_frames={}",
                self.queued_samples.load(Ordering::Acquire),
                self.recovered_frames,
                self.unrecovered_frames.load(Ordering::Acquire),
            ),
        );
    }

    fn finish(&mut self, duration_ms: u64, recorder_dropped: u64) -> Result<()> {
        self.end_segment()?;
        self.catch_up_unseen_segments(None)?;
        let local_duration_ms = duration_ms.min(self.local_end_ms());
        let [mic, system] = &mut self.decoders;
        let mic = mic.finish_until(local_duration_ms)?;
        let system = system.finish_until(local_duration_ms)?;
        let unrecovered = self.unrecovered_frames.load(Ordering::Acquire);
        // Without a durable source nothing can replace dropped queue audio,
        // so the recorder's drop count stays authoritative.
        let missing = if self.durable.is_some() {
            unrecovered
        } else {
            recorder_dropped.saturating_add(unrecovered)
        };
        let captured_until_ms = self.offset_ms.saturating_add(duration_ms);
        let decoded_until_ms = self
            .offset_ms
            .saturating_add(mic.decoded_until_ms.max(system.decoded_until_ms));
        let terminal = live_checkpoint_complete(captured_until_ms, decoded_until_ms, missing);
        write_checkpoint(
            &self.checkpoint,
            &mic,
            &system,
            self.offset_ms,
            terminal,
            Some(captured_until_ms),
            missing,
            self.recovered_frames,
        )?;
        if terminal {
            self.set_lagging(false);
        } else {
            bail!(
                "live transcript incomplete: captured_until_ms={captured_until_ms} decoded_until_ms={decoded_until_ms} missing_samples={missing}; run margins process for an offline transcript"
            );
        }
        Ok(())
    }
}

fn lane_index(channel: LiveAudioChannel) -> usize {
    match channel {
        LiveAudioChannel::Mic => 0,
        LiveAudioChannel::System => 1,
    }
}
