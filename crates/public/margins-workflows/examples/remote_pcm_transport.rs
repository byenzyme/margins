//! Bounded deterministic transport verifier for a real Workspace service.
//!
//! Required environment: MARGINS_REMOTE, MARGINS_WORKSPACE,
//! MARGINS_FIXTURE_MIC_S16LE, and
//! MARGINS_FIXTURE_SYSTEM_S16LE. Inputs are mono 16 kHz signed little-endian
//! PCM and remain subject to the production chunk/spool/service limits. The
//! direct 16 kHz fixture seam is byte-preserving; recorder-native 44.1/48 kHz
//! f32 input is resampled by the production spool worker before this boundary.
//! MARGINS_REMOTE_TOKEN is required only for HTTPS/loopback. The optional
//! MARGINS_FIXTURE_STOP_AFTER (`spool`, `chunks`, `close`, or `memo`) exposes
//! durable barriers for restart/lost-response verification.

use anyhow::{Context, Result};
use margins_meeting_protocol::{
    SegmentCloseReasonV1, SessionFinalizeReasonV1, WorkspaceMemoLineV1, WorkspaceMemoReplaceV1,
};
use margins_workflows::remote_workspace::{
    deliver_chunks, deliver_closes, deliver_finalize, deliver_memo, native_create_session_command,
    transfer_root, DurableTransferSpool, NativePcmLane, NativePcmTransfer, RemoteConnection,
    NATIVE_PCM_RATE_HZ,
};

fn main() -> Result<()> {
    let remote = required("MARGINS_REMOTE")?;
    let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
    let workspace = required("MARGINS_WORKSPACE")?;
    let mic = std::fs::read(required("MARGINS_FIXTURE_MIC_S16LE")?)?;
    let system = std::fs::read(required("MARGINS_FIXTURE_SYSTEM_S16LE")?)?;
    let suffix = chrono::Utc::now().timestamp_millis();
    let session_id = format!("pcm-fixture-{suffix}");
    let transfer_id = format!("pcm-fixture-{suffix}");
    let connection = RemoteConnection::connect(&remote, &workspace, token.as_deref())?;
    let capabilities = connection.client.capabilities()?;
    let create = native_create_session_command(
        &session_id,
        &format!("reserve-{suffix}"),
        Some("Deterministic PCM transport fixture".into()),
        "remote-pcm-transport-example",
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
    let initial_memo = connection.client.memo(&session_id)?;
    let mut transfer = NativePcmTransfer::new(spool);
    transfer.begin_segment("fixture-segment".into(), 0)?;
    let mic_frames = transfer.append_s16le(NativePcmLane::Microphone, NATIVE_PCM_RATE_HZ, &mic)?;
    let system_frames =
        transfer.append_s16le(NativePcmLane::System, NATIVE_PCM_RATE_HZ, &system)?;
    let close = transfer.close_segment(SegmentCloseReasonV1::Stop)?;
    let ended_at_ms = match &close.body {
        margins_meeting_protocol::ClientMessageBodyV1::CloseSegment(close) => close.ended_at_ms.0,
        _ => unreachable!("native close returned the wrong command"),
    };
    transfer
        .spool_mut()
        .set_memo_intent(WorkspaceMemoReplaceV1 {
            request_id: format!("fixture-memo-{suffix}"),
            expected_revision: initial_memo.revision,
            lines: vec![WorkspaceMemoLineV1 {
                text: "sample-exact remote transport fixture".into(),
                created_secs: 0.25,
                edited_secs: Some(0.25),
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            }],
        })?;
    transfer.seal_session(ended_at_ms, SessionFinalizeReasonV1::Completed)?;
    let mut spool = transfer.into_spool();
    let stop_after = std::env::var("MARGINS_FIXTURE_STOP_AFTER").ok();
    if stop_after
        .as_deref()
        .is_some_and(|value| !matches!(value, "spool" | "chunks" | "close" | "memo"))
    {
        anyhow::bail!("MARGINS_FIXTURE_STOP_AFTER must be spool, chunks, close, or memo");
    }
    if stop_after.as_deref() != Some("spool") {
        deliver_chunks(&mut spool, &connection.client)?;
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
    println!(
        "{}",
        serde_json::json!({
            "schema":"margins.remote-pcm-fixture.v1",
            "instance_id":capabilities.instance_id,
            "workspace_id":workspace,
            "session_id":session_id,
            "mic_frames":mic_frames,
            "system_frames":system_frames,
            "duration_ms":ended_at_ms,
            "completed":completed,
            "stopped_after":stop_after,
            "transfer_id":transfer_id,
        })
    );
    Ok(())
}

fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is required"))
}
