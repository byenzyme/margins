//! Paced deterministic Opus transport verifier for a real Workspace service.
//!
//! Required environment: MARGINS_REMOTE, MARGINS_WORKSPACE,
//! MARGINS_FIXTURE_MIC_S16LE, and
//! MARGINS_FIXTURE_SYSTEM_S16LE. Inputs are mono 16 kHz signed little-endian
//! PCM and remain subject to the production resample/Opus/spool/service limits.
//! Direct 16 kHz input is sample-exact before lossy Opus encoding; decoded
//! source-frame count and timeline remain exact.
//! By default the two lanes are interleaved at a real-time 20 ms cadence and
//! capture-time delivery uses the same production `deliver_available`
//! scheduler as the native recorder. Set MARGINS_FIXTURE_PACED=0 only for
//! protocol/recovery setup. MARGINS_REMOTE_TOKEN is required only for
//! HTTPS/loopback. The optional
//! MARGINS_FIXTURE_STOP_AFTER (`spool`, `chunks`, `close`, or `memo`) exposes
//! durable barriers for restart/lost-response verification.

use anyhow::{Context, Result};
use margins_meeting_protocol::{
    SegmentCloseReasonV1, SessionFinalizeReasonV1, WorkspaceMemoLineV1, WorkspaceMemoReplaceV1,
};
use margins_workflows::remote_workspace::{
    deliver_available, deliver_chunks, deliver_closes, deliver_finalize, deliver_memo,
    native_create_session_command, transfer_root, DurableTransferSpool, NativeRemoteLane,
    NativeRemoteTransfer, RemoteConnection, NATIVE_OPUS_BITRATE_BPS, NATIVE_REMOTE_RATE_HZ,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Default)]
struct PendingHighWater {
    chunks: usize,
    bytes: u64,
    oldest_ms: u64,
}

fn main() -> Result<()> {
    let remote = required("MARGINS_REMOTE")?;
    let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
    let workspace = required("MARGINS_WORKSPACE")?;
    let mic = std::fs::read(required("MARGINS_FIXTURE_MIC_S16LE")?)?;
    let system = std::fs::read(required("MARGINS_FIXTURE_SYSTEM_S16LE")?)?;
    if mic.len() % 2 != 0 || system.len() % 2 != 0 || mic.len() != system.len() {
        anyhow::bail!("fixture lanes must be equal-duration mono s16le");
    }
    let paced = std::env::var("MARGINS_FIXTURE_PACED")
        .map(|value| value != "0")
        .unwrap_or(true);
    let stop_after = std::env::var("MARGINS_FIXTURE_STOP_AFTER").ok();
    if stop_after
        .as_deref()
        .is_some_and(|value| !matches!(value, "spool" | "chunks" | "close" | "memo"))
    {
        anyhow::bail!("MARGINS_FIXTURE_STOP_AFTER must be spool, chunks, close, or memo");
    }
    let suffix = chrono::Utc::now().timestamp_millis();
    let session_id = format!("opus-fixture-{suffix}");
    let transfer_id = format!("opus-fixture-{suffix}");
    let connection = RemoteConnection::connect(&remote, &workspace, token.as_deref())?;
    let capabilities = &connection.capabilities;
    let create = native_create_session_command(
        &session_id,
        &format!("reserve-{suffix}"),
        Some("Deterministic Opus transport fixture".into()),
        "remote-opus-transport-example",
    );
    let reservation = connection.client.reserve(&create)?;
    let spool = DurableTransferSpool::create(
        &transfer_root()?,
        &transfer_id,
        capabilities.instance_id.as_ref(),
        &remote,
        &workspace,
        &session_id,
        &reservation.producer_token,
        capabilities.limits.spool_reserve_bytes,
    )?;
    let uploader_done = Arc::new(AtomicBool::new(false));
    let uploader = if stop_after.as_deref() == Some("spool") {
        None
    } else {
        let parent = spool
            .root()
            .parent()
            .context("fixture transfer has no parent")?
            .to_path_buf();
        let transfer_id = spool.manifest().transfer_id.clone();
        let reserve_bytes = capabilities.limits.spool_reserve_bytes;
        let client = connection.client.clone();
        let done = uploader_done.clone();
        Some(std::thread::spawn(move || {
            let mut backoff = Duration::from_millis(50);
            while !done.load(Ordering::Acquire) {
                let attempt = (|| -> Result<()> {
                    let mut delivery =
                        DurableTransferSpool::open(&parent, &transfer_id, reserve_bytes)?;
                    deliver_available(&mut delivery, &client)
                })();
                backoff = if attempt.is_ok() {
                    Duration::from_millis(50)
                } else {
                    (backoff * 2).min(Duration::from_secs(2))
                };
                std::thread::sleep(backoff);
            }
        }))
    };
    let initial_memo = connection.client.memo(&session_id)?;
    let bitrate = std::env::var("MARGINS_FIXTURE_OPUS_BITRATE_BPS")
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(NATIVE_OPUS_BITRATE_BPS);
    let mut transfer = NativeRemoteTransfer::new_with_bitrate(spool, bitrate);
    transfer.begin_segment("fixture-segment".into(), 0)?;
    let capture_started = Instant::now();
    let mut encode_spool_wall = Duration::ZERO;
    let mut encode_spool_cpu_micros = 0u64;
    let mut high_water = PendingHighWater::default();
    let frame_bytes = usize::from(margins_meeting_protocol::OPUS_PACKET_FRAME_SAMPLES_V1) * 2;
    let mut mic_frames = 0usize;
    let mut system_frames = 0usize;
    for (index, (mic_frame, system_frame)) in mic
        .chunks(frame_bytes)
        .zip(system.chunks(frame_bytes))
        .enumerate()
    {
        let encode_started = Instant::now();
        let encode_cpu_started = process_cpu_micros();
        mic_frames += transfer.append_s16le(
            NativeRemoteLane::Microphone,
            NATIVE_REMOTE_RATE_HZ,
            mic_frame,
        )?;
        system_frames += transfer.append_s16le(
            NativeRemoteLane::System,
            NATIVE_REMOTE_RATE_HZ,
            system_frame,
        )?;
        encode_spool_wall += encode_started.elapsed();
        encode_spool_cpu_micros = encode_spool_cpu_micros
            .saturating_add(process_cpu_micros().saturating_sub(encode_cpu_started));
        observe_pending(transfer.spool(), &mut high_water)?;
        if paced {
            let delivered_frames = ((index + 1) * frame_bytes / 2).min(mic.len() / 2);
            let deadline = capture_started
                + Duration::from_secs_f64(delivered_frames as f64 / NATIVE_REMOTE_RATE_HZ as f64);
            if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(remaining);
            }
        }
    }
    let capture_wall = capture_started.elapsed();
    let stop_started = Instant::now();
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop)?;
    let ended_at_ms = match &close.body {
        margins_meeting_protocol::ClientMessageBodyV1::CloseSegment(close) => close.ended_at_ms.0,
        _ => unreachable!("native close returned the wrong command"),
    };
    observe_pending(transfer.spool(), &mut high_water)?;
    let pending_at_stop = transfer.spool().pending_chunks()?;
    let pending_chunks_at_stop = pending_at_stop.len();
    let pending_encoded_bytes_at_stop = pending_at_stop
        .iter()
        .map(|chunk| chunk.size_bytes)
        .sum::<u64>();
    uploader_done.store(true, Ordering::Release);
    if let Some(uploader) = uploader {
        uploader
            .join()
            .map_err(|_| anyhow::anyhow!("fixture uploader thread panicked"))?;
    }
    transfer
        .spool_mut()
        .set_memo_intent(WorkspaceMemoReplaceV1 {
            request_id: format!("fixture-memo-{suffix}"),
            expected_revision: initial_memo.revision,
            lines: vec![WorkspaceMemoLineV1 {
                text: "deterministic remote Opus transport fixture".into(),
                created_secs: 0.25,
                edited_secs: Some(0.25),
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            }],
        })?;
    transfer.seal_session(ended_at_ms, SessionFinalizeReasonV1::Completed)?;
    let mut spool = transfer.into_spool();
    let mut last_input_ack_after_stop_ms = None;
    if stop_after.as_deref() != Some("spool") {
        deliver_chunks(&mut spool, &connection.client)?;
        last_input_ack_after_stop_ms = Some(if pending_chunks_at_stop == 0 {
            0
        } else {
            stop_started.elapsed().as_millis() as u64
        });
    }
    if !matches!(stop_after.as_deref(), Some("spool" | "chunks")) {
        deliver_closes(&mut spool, &connection.client)?;
    }
    if !matches!(stop_after.as_deref(), Some("spool" | "chunks" | "close")) {
        deliver_memo(&mut spool, &connection.client)?;
    }
    let completed = if stop_after.is_none() {
        deliver_finalize(&mut spool, &connection.client)?;
        true
    } else {
        false
    };
    let final_drain_ms = stop_started.elapsed().as_millis() as u64;
    let delivery = connection.client.delivery_metrics();
    let canonical_encoded_bytes = if completed {
        Some(
            connection
                .client
                .artifacts(&session_id)?
                .into_iter()
                .filter(|artifact| artifact.kind.ends_with("_opus"))
                .filter_map(|artifact| artifact.size_bytes)
                .sum::<u64>(),
        )
    } else {
        None
    };
    println!(
        "{}",
        serde_json::json!({
            "schema":"margins.remote-opus-fixture.v1",
            "instance_id":capabilities.instance_id,
            "workspace_id":workspace,
            "session_id":session_id,
            "mic_frames":mic_frames,
            "system_frames":system_frames,
            "bitrate_per_lane_bps":bitrate,
            "duration_ms":ended_at_ms,
            "paced":paced,
            "capture_wall_ms":capture_wall.as_millis() as u64,
            "encode_spool_wall_micros":encode_spool_wall.as_micros() as u64,
            "encode_spool_cpu_micros":encode_spool_cpu_micros,
            "max_pending_chunks":high_water.chunks,
            "max_pending_encoded_bytes":high_water.bytes,
            "max_pending_oldest_ms":high_water.oldest_ms,
            "pending_chunks_at_stop":pending_chunks_at_stop,
            "pending_encoded_bytes_at_stop":pending_encoded_bytes_at_stop,
            "http_batch_requests":delivery.http_batch_requests,
            "durable_audio_commands":delivery.durable_audio_commands,
            "durable_audio_receipts":delivery.durable_audio_receipts,
            "encoded_payload_bytes_attempted":delivery.encoded_payload_bytes,
            "canonical_encoded_bytes":canonical_encoded_bytes,
            "batch_body_bytes_attempted":delivery.batch_body_bytes,
            "batch_request_micros":delivery.batch_request_micros,
            "last_batch_ack_unix_ms":delivery.last_batch_ack_unix_ms,
            "last_input_ack_after_stop_ms":last_input_ack_after_stop_ms,
            "final_drain_ms":final_drain_ms,
            "completed":completed,
            "stopped_after":stop_after,
            "transfer_id":transfer_id,
        })
    );
    Ok(())
}

fn observe_pending(spool: &DurableTransferSpool, high_water: &mut PendingHighWater) -> Result<()> {
    let pending = spool.pending_chunks()?;
    high_water.chunks = high_water.chunks.max(pending.len());
    high_water.bytes = high_water
        .bytes
        .max(pending.iter().map(|chunk| chunk.size_bytes).sum());
    let oldest_ms = pending
        .iter()
        .filter_map(|chunk| chunk.path.metadata().ok()?.modified().ok()?.elapsed().ok())
        .map(|age| age.as_millis() as u64)
        .max()
        .unwrap_or_default();
    high_water.oldest_ms = high_water.oldest_ms.max(oldest_ms);
    Ok(())
}

#[cfg(unix)]
fn process_cpu_micros() -> u64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the pointer is valid and CLOCK_THREAD_CPUTIME_ID writes one timespec.
    // The producer thread metric excludes the concurrent uploader's CPU time.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) } != 0 {
        return 0;
    }
    (value.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add(value.tv_nsec as u64 / 1_000)
}

#[cfg(not(unix))]
fn process_cpu_micros() -> u64 {
    0
}

fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is required"))
}
