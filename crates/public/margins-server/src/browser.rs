//! BB MediaRecorder adapter. All capture facts are reconstructed from the
//! Workspace runtime and its producer authority; there is no server registry.

use crate::{http, ServerState};
use anyhow::{bail, Context, Result};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use margins_meeting_protocol::{
    AudioChunkV1, AudioCodecV1, AudioContainerV1, AudioFormatV1, CaptureDiscontinuityV1,
    CaptureLaneV1, CaptureModeV1, CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1,
    CaptureSourceV1, ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1, ContentDigestV1,
    CreateSessionV1, DigestAlgorithmV1, DiscontinuityId, DiscontinuityReasonV1, DurationMillis,
    FinalizeSessionV1, LaneBoundaryV1, MessageId, ProtocolVersionV1, SegmentCloseReasonV1,
    SegmentCloseReferenceV1, SegmentId, SequenceRangeV1, ServerMessageBodyV1,
    SessionFinalizeReasonV1, SessionId, SessionMillis, UnixMillis,
};
use margins_workflows::workspace_service::{
    CaptureState, ServicePrincipal, OP_CAPTURE_WRITE, OP_SESSION_CREATE, OP_SESSION_READ,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

const LANE: &str = "mic";
const MAX_BROWSER_CHUNK_DURATION_MS: u64 = 5 * 60 * 1_000;
const MAX_BROWSER_FUTURE_SKEW_MS: u64 = 30_000;
/// A normal page heartbeats frequently. Thirty minutes permits long pauses
/// and short host outages while bounding orphaned capture recovery.
pub const BROWSER_OWNER_LEASE_MS: u64 = 30 * 60 * 1_000;
pub const BROWSER_OWNER_SWEEP_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNotepad {
    pub text: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub recording_id: String,
    pub session_id: String,
    pub next_sequence: u64,
    pub status: BrowserStatus,
    pub incomplete: bool,
    pub expired_lease: bool,
    pub notepad: BrowserNotepad,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserStatus {
    Recording,
    Paused,
    Saving,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartBody {
    owner_id: String,
    name: String,
    started_at_unix_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerBody {
    owner_id: String,
    segment_started_unix_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryBody {
    owner_id: String,
    expected_next_sequence: u64,
    segment_ended_unix_ms: Option<u64>,
    #[serde(default)]
    recovered_after_reload: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerQuery {
    owner_id: String,
}

fn recording_id(owner: &str) -> String {
    // The public session ID must not reveal the producer capability.
    format!("browser-{:x}", Sha256::digest(owner.as_bytes()))
}

fn authorized_recording(recording: &str, owner: &str) -> Result<SessionId> {
    if recording != recording_id(owner) {
        bail!("browser capture owner does not match recording");
    }
    Ok(SessionId(recording.to_string()))
}

fn now() -> UnixMillis {
    UnixMillis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    )
}

fn command(session: &SessionId, id: String, body: ClientMessageBodyV1) -> ClientMessageV1 {
    ClientMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: MessageId(id),
        session_id: session.clone(),
        // Stable across HTTP retries. Runtime receipts fingerprint the whole command.
        sent_at_unix_ms: UnixMillis(0),
        body,
    }
}

fn segment_id(index: usize) -> SegmentId {
    SegmentId(format!("browser-{index:06}"))
}

fn close_boundary(close: &margins_meeting_runtime::StoredSegmentCloseV1) -> Result<u64> {
    close
        .command
        .lane_boundaries
        .iter()
        .find(|boundary| boundary.lane_id.as_ref() == LANE)
        .map(|boundary| boundary.next_sequence)
        .context("browser segment close lacks microphone boundary")
}

struct Layout {
    closed_total: u64,
    closed_count: usize,
    current_watermark: u64,
    current_latest_end_ms: u64,
    latest_close_end_ms: u64,
}

fn layout(state: &CaptureState) -> Result<Layout> {
    let mut closed_total = 0_u64;
    let mut closed_count = 0_usize;
    let mut current_watermark = 0;
    let mut current_latest_end_ms = 0;
    let mut latest_close_end_ms = 0;
    for segment in &state.segments {
        if segment.segment_id != segment_id(closed_count) {
            bail!("browser runtime segment order is inconsistent");
        }
        if let Some(close) = &segment.close {
            if !close.finalized {
                bail!("browser segment close is incomplete");
            }
            closed_total = closed_total
                .checked_add(close_boundary(close)?)
                .context("browser chunk sequence overflow")?;
            latest_close_end_ms = latest_close_end_ms.max(close.command.ended_at_ms.0);
            closed_count += 1;
        } else {
            let lane = segment
                .lanes
                .iter()
                .find(|lane| lane.acknowledgement.lane_id.as_ref() == LANE)
                .context("browser runtime segment lacks microphone lane")?;
            current_watermark = lane.acknowledgement.durable_through_sequence;
            current_latest_end_ms = lane.latest_end_ms.0;
        }
    }
    Ok(Layout {
        closed_total,
        closed_count,
        current_watermark,
        current_latest_end_ms,
        latest_close_end_ms,
    })
}

fn session_millis(
    capture: &CaptureState,
    client_unix_ms: Option<u64>,
    received_unix_ms: u64,
    fallback_ms: u64,
) -> u64 {
    let Some(client_unix_ms) = client_unix_ms else {
        return fallback_ms;
    };
    let adjusted = i128::from(client_unix_ms) - i128::from(capture.client_started_at_unix_ms);
    let future_limit = received_unix_ms
        .saturating_sub(capture.started_at_unix_ms)
        .saturating_add(MAX_BROWSER_FUTURE_SKEW_MS);
    adjusted.clamp(0, i128::from(future_limit)) as u64
}

fn current_lane<'a>(
    capture: &'a CaptureState,
) -> Option<&'a margins_meeting_runtime::StoredLaneSummaryV1> {
    capture
        .segments
        .last()
        .filter(|segment| segment.close.is_none())
        .and_then(|segment| {
            segment
                .lanes
                .iter()
                .find(|lane| lane.acknowledgement.lane_id.as_ref() == LANE)
        })
}

fn durable_horizon(capture: &CaptureState, position: &Layout) -> u64 {
    let local = current_lane(capture)
        .map(|lane| {
            lane.acknowledgement
                .durable_out_of_order
                .iter()
                .map(|range| range.end_exclusive)
                .max()
                .unwrap_or(lane.acknowledgement.durable_through_sequence)
                .max(lane.acknowledgement.durable_through_sequence)
        })
        .unwrap_or(0);
    position.closed_total.saturating_add(local)
}

fn missing_ranges(capture: &CaptureState, local_horizon: u64) -> Vec<SequenceRangeV1> {
    let Some(lane) = current_lane(capture) else {
        return if local_horizon == 0 {
            vec![]
        } else {
            vec![SequenceRangeV1 {
                start: 0,
                end_exclusive: local_horizon,
            }]
        };
    };
    let mut ranges = lane.acknowledgement.durable_out_of_order.clone();
    ranges.sort_by_key(|range| range.start);
    let mut cursor = lane.acknowledgement.durable_through_sequence;
    let mut missing = Vec::new();
    for range in ranges {
        if range.start >= local_horizon {
            break;
        }
        if cursor < range.start {
            missing.push(SequenceRangeV1 {
                start: cursor,
                end_exclusive: range.start.min(local_horizon),
            });
        }
        cursor = cursor.max(range.end_exclusive);
    }
    if cursor < local_horizon {
        missing.push(SequenceRangeV1 {
            start: cursor,
            end_exclusive: local_horizon,
        });
    }
    missing
}

fn complete_through(state: &CaptureState, expected: u64) -> std::result::Result<Layout, Response> {
    let position = layout(state).map_err(browser_error)?;
    if expected < position.closed_total {
        return Err(gap_response(expected));
    }
    let local = expected - position.closed_total;
    if position.current_watermark < local {
        return Err(gap_response(
            position.closed_total + position.current_watermark,
        ));
    }
    Ok(position)
}

fn gap_response(sequence: u64) -> Response {
    http::workspace_error(
        StatusCode::CONFLICT,
        "browser_chunk_gap",
        true,
        format!("Missing browser audio sequence {sequence}; retry the chunk before Stop."),
    )
}

fn browser_error(error: anyhow::Error) -> Response {
    if error
        .to_string()
        .contains("browser capture owner lease expired")
    {
        return lease_expired_response();
    }
    http::service_error(error)
}

fn lease_expired_response() -> Response {
    http::workspace_error(
        StatusCode::CONFLICT,
        "browser_lease_expired",
        false,
        "Finished while you were away (incomplete). Audio already received was saved.",
    )
}

fn required_unix_ms(headers: &HeaderMap, name: &'static str) -> std::result::Result<u64, Response> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            http::workspace_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_chunk_time",
                false,
                format!("Browser audio chunk is missing valid {name}"),
            )
        })
}

fn snapshot_value(
    state: &ServerState,
    principal: &ServicePrincipal,
    owner: &str,
    session: &SessionId,
) -> Result<BrowserSnapshot> {
    let capture = state
        .workspace_service
        .capture_state_for_producer(principal, owner, session)?;
    let memo = state.workspace_service.memo(principal, session)?;
    let position = layout(&capture)?;
    let paused = capture
        .segments
        .last()
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| close.command.reason == SegmentCloseReasonV1::Pause)
        && !state.workspace_service.capture_command_recorded(
            principal,
            owner,
            session,
            &MessageId(format!("browser-resume-{}", position.closed_count)),
        )?;
    Ok(BrowserSnapshot {
        recording_id: session.as_ref().to_string(),
        session_id: session.as_ref().to_string(),
        next_sequence: position
            .closed_total
            .saturating_add(position.current_watermark),
        status: if capture.input_finalized {
            BrowserStatus::Saving
        } else if paused {
            BrowserStatus::Paused
        } else {
            BrowserStatus::Recording
        },
        incomplete: capture.input_finalized
            && state
                .workspace_service
                .session(principal, session)?
                .capture_incomplete,
        expired_lease: capture.expired_lease_finish,
        notepad: BrowserNotepad {
            text: memo
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            revision: memo.revision,
        },
    })
}

fn principal(
    state: &ServerState,
    headers: &HeaderMap,
    workspace: &str,
    operation: &str,
) -> std::result::Result<ServicePrincipal, Response> {
    http::workspace_auth_operation(state, headers, workspace, operation)
}

pub async fn start(
    State(state): State<ServerState>,
    Path(workspace): Path<String>,
    headers: HeaderMap,
    Json(body): Json<StartBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_SESSION_CREATE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = principal.require(&workspace, OP_CAPTURE_WRITE) {
        return browser_error(response);
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 160 {
        return http::workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            false,
            "Meeting name must have 1–160 characters",
        );
    }
    let session = SessionId(recording_id(&body.owner_id));
    // A lost start response can be retried with the same owner capability.
    if state
        .workspace_service
        .capture_state_for_producer(&principal, &body.owner_id, &session)
        .is_ok()
    {
        return snapshot_value(&state, &principal, &body.owner_id, &session)
            .map(http::workspace_ok)
            .unwrap_or_else(browser_error);
    }
    let started = now();
    let client_started = body.started_at_unix_ms.unwrap_or(started.0);
    let create = command(
        &session,
        format!("browser-create-{}", session.as_ref()),
        ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: format!("browser-{}", session.as_ref()),
            started_at_unix_ms: started,
            title: Some(name.to_string()),
            sources: vec![CaptureSourceV1 {
                source_id: LANE.into(),
                kind: CaptureSourceKindV1::Microphone,
                label: None,
                external_id: None,
            }],
            lanes: vec![CaptureLaneV1 {
                lane_id: LANE.into(),
                source_ids: vec![LANE.into()],
                label: None,
                format: AudioFormatV1 {
                    codec: AudioCodecV1::Opus,
                    container: AudioContainerV1::Webm,
                    sample_rate_hz: 48_000,
                    channel_count: 1,
                },
            }],
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: "bb-browser".into(),
                    producer_version: Some(env!("CARGO_PKG_VERSION").into()),
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: started,
                    attributes: BTreeMap::from([(
                        "client_started_at_unix_ms".into(),
                        client_started.to_string(),
                    )]),
                }],
            },
        }),
    );
    state
        .workspace_service
        .reserve_session_with_producer_token(&principal, create, &body.owner_id)
        .and_then(|_| snapshot_value(&state, &principal, &body.owner_id, &session))
        .map(http::workspace_ok)
        .unwrap_or_else(browser_error)
}

pub async fn snapshot(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    Query(query): Query<OwnerQuery>,
    headers: HeaderMap,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_SESSION_READ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &query.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    snapshot_value(&state, &principal, &query.owner_id, &session)
        .map(http::workspace_ok)
        .unwrap_or_else(browser_error)
}

pub async fn heartbeat(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<OwnerBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &body.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let capture = match state.workspace_service.capture_state_for_producer(
        &principal,
        &body.owner_id,
        &session,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if capture.input_finalized {
        return snapshot_value(&state, &principal, &body.owner_id, &session)
            .map(http::workspace_ok)
            .unwrap_or_else(browser_error);
    }
    if let Err(error) =
        state
            .workspace_service
            .touch_capture_producer(&principal, &body.owner_id, &session)
    {
        // Stop may release the producer between the read and lease refresh.
        if state
            .workspace_service
            .capture_state_for_producer(&principal, &body.owner_id, &session)
            .is_ok_and(|capture| capture.input_finalized)
        {
            return snapshot_value(&state, &principal, &body.owner_id, &session)
                .map(http::workspace_ok)
                .unwrap_or_else(browser_error);
        }
        return browser_error(error);
    }
    snapshot_value(&state, &principal, &body.owner_id, &session)
        .map(http::workspace_ok)
        .unwrap_or_else(browser_error)
}

pub async fn pause(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<BoundaryBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &body.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let capture = match state.workspace_service.capture_state_for_producer(
        &principal,
        &body.owner_id,
        &session,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if capture.input_finalized {
        return if capture.expired_lease_finish {
            lease_expired_response()
        } else {
            http::workspace_error(
                StatusCode::CONFLICT,
                "browser_already_saved",
                false,
                "This recording was already saved",
            )
        };
    }
    if let Err(error) =
        state
            .workspace_service
            .touch_capture_producer(&principal, &body.owner_id, &session)
    {
        return browser_error(error);
    }
    let position = match layout(&capture) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if body.expected_next_sequence < position.closed_total {
        return gap_response(body.expected_next_sequence);
    }
    if body.expected_next_sequence == position.closed_total
        && capture
            .segments
            .last()
            .and_then(|segment| segment.close.as_ref())
            .is_some_and(|close| close.command.reason == SegmentCloseReasonV1::Pause)
        && !state
            .workspace_service
            .capture_command_recorded(
                &principal,
                &body.owner_id,
                &session,
                &MessageId(format!("browser-resume-{}", position.closed_count)),
            )
            .unwrap_or(false)
    {
        return snapshot_value(&state, &principal, &body.owner_id, &session)
            .map(http::workspace_ok)
            .unwrap_or_else(browser_error);
    }
    let local = body.expected_next_sequence - position.closed_total;
    let (missing, latest_gap_end_ms) = if body.recovered_after_reload {
        match declare_missing_chunks(
            &state,
            &principal,
            Some(&body.owner_id),
            &session,
            &capture,
            &position,
            local,
        ) {
            Ok(value) => value,
            Err(error) => return browser_error(error),
        }
    } else {
        (Vec::new(), 0)
    };
    let covered = if missing.is_empty() {
        capture.clone()
    } else {
        match state.workspace_service.capture_state_for_producer(
            &principal,
            &body.owner_id,
            &session,
        ) {
            Ok(value) => value,
            Err(error) => return browser_error(error),
        }
    };
    let position = match complete_through(&covered, body.expected_next_sequence) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ended_at_ms = session_millis(
        &capture,
        body.segment_ended_unix_ms,
        now().0,
        position
            .current_latest_end_ms
            .max(position.latest_close_end_ms),
    )
    .max(position.current_latest_end_ms)
    .max(position.latest_close_end_ms);
    // A fresh MediaRecorder stream cannot be appended to the prior WebM
    // segment. The browser may also have lost its final unflushed timeslice
    // during reload, even when every assigned sequence was acknowledged.
    if body.recovered_after_reload && missing.is_empty() {
        let discontinuity = CaptureDiscontinuityV1 {
            discontinuity_id: DiscontinuityId(format!(
                "browser-reload-tail-{}",
                position.closed_count
            )),
            segment_id: segment_id(position.closed_count),
            lane_id: LANE.into(),
            sequence_range: SequenceRangeV1 {
                start: local,
                end_exclusive: local,
            },
            starts_at_ms: SessionMillis(position.current_latest_end_ms),
            duration_ms: DurationMillis(1),
            reason: DiscontinuityReasonV1::SourceEnded,
            detail: Some(
                "Browser page reloaded before MediaRecorder flushed its final timeslice".into(),
            ),
        };
        if let Err(error) = state.workspace_service.execute_capture(
            &principal,
            &body.owner_id,
            command(
                &session,
                format!("browser-reload-tail-command-{}", position.closed_count),
                ClientMessageBodyV1::CaptureDiscontinuity(discontinuity),
            ),
        ) {
            return browser_error(error);
        }
    }
    let close = CloseSegmentV1 {
        segment_id: segment_id(position.closed_count),
        ended_at_ms: SessionMillis(
            ended_at_ms.max(latest_gap_end_ms).max(
                position
                    .current_latest_end_ms
                    .saturating_add(u64::from(body.recovered_after_reload)),
            ),
        ),
        lane_boundaries: vec![LaneBoundaryV1 {
            lane_id: LANE.into(),
            next_sequence: local,
        }],
        reason: SegmentCloseReasonV1::Pause,
    };
    state
        .workspace_service
        .execute_capture(
            &principal,
            &body.owner_id,
            command(
                &session,
                format!("browser-pause-{}", position.closed_count),
                ClientMessageBodyV1::CloseSegment(close),
            ),
        )
        .and_then(|_| snapshot_value(&state, &principal, &body.owner_id, &session))
        .map(http::workspace_ok)
        .unwrap_or_else(browser_error)
}

pub async fn resume(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<OwnerBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &body.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let capture = match state.workspace_service.capture_state_for_producer(
        &principal,
        &body.owner_id,
        &session,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if capture.input_finalized {
        return if capture.expired_lease_finish {
            lease_expired_response()
        } else {
            http::workspace_error(
                StatusCode::CONFLICT,
                "browser_already_saved",
                false,
                "This recording was already saved",
            )
        };
    }
    if let Err(error) =
        state
            .workspace_service
            .touch_capture_producer(&principal, &body.owner_id, &session)
    {
        return browser_error(error);
    }
    let position = match layout(&capture) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    // A browser wall clock can step backwards while paused. Resume still
    // opens a new WebM segment; its first chunk is positioned on the server
    // receive clock when its client time precedes the durable close.
    let resume_id = MessageId(format!("browser-resume-{}", position.closed_count));
    if state
        .workspace_service
        .capture_command_recorded(&principal, &body.owner_id, &session, &resume_id)
        .unwrap_or(false)
    {
        return snapshot_value(&state, &principal, &body.owner_id, &session)
            .map(http::workspace_ok)
            .unwrap_or_else(browser_error);
    }
    if !capture
        .segments
        .last()
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| close.command.reason == SegmentCloseReasonV1::Pause)
    {
        return http::workspace_error(
            StatusCode::CONFLICT,
            "invalid_transition",
            false,
            "Browser recording is not paused",
        );
    }
    state
        .workspace_service
        .recover_capture_for_producer(&principal, &body.owner_id, &session, resume_id)
        .and_then(|_| snapshot_value(&state, &principal, &body.owner_id, &session))
        .map(http::workspace_ok)
        .unwrap_or_else(browser_error)
}

pub async fn chunk(
    State(state): State<ServerState>,
    Path((workspace, recording, sequence)): Path<(String, String, u64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let owner = match headers
        .get("X-Margins-Capture-Owner")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => value,
        None => {
            return http::workspace_error(
                StatusCode::UNAUTHORIZED,
                "owner_required",
                false,
                "Missing capture owner",
            )
        }
    };
    let session = match authorized_recording(&recording, owner) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let captured_start_unix_ms =
        match required_unix_ms(&headers, "X-Margins-Captured-Start-Unix-Ms") {
            Ok(value) => value,
            Err(response) => return response,
        };
    let captured_end_unix_ms = match required_unix_ms(&headers, "X-Margins-Captured-End-Unix-Ms") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let duration_ms = captured_end_unix_ms.saturating_sub(captured_start_unix_ms);
    if duration_ms == 0 || duration_ms > MAX_BROWSER_CHUNK_DURATION_MS {
        return http::workspace_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_chunk_time",
            false,
            "Browser audio chunk duration is outside the supported range",
        );
    }
    if body.is_empty()
        || body.len() as u64 > margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES
    {
        return http::workspace_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_chunk",
            false,
            "Browser audio chunk is empty or too large",
        );
    }
    let capture = match state
        .workspace_service
        .capture_state_for_producer(&principal, owner, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if let Err(error) = state
        .workspace_service
        .touch_capture_producer(&principal, owner, &session)
    {
        return browser_error(error);
    }
    let mut base = 0_u64;
    let mut selected = None;
    for (index, segment) in capture.segments.iter().enumerate() {
        if let Some(close) = &segment.close {
            let end = match close_boundary(close).and_then(|length| {
                base.checked_add(length)
                    .context("browser sequence overflow")
            }) {
                Ok(value) => value,
                Err(error) => return browser_error(error),
            };
            if sequence < end {
                selected = Some((segment_id(index), sequence - base, index));
                break;
            }
            base = end;
        }
    }
    let replaying_closed = selected.is_some();
    let (segment, local_sequence, segment_index) = selected.unwrap_or_else(|| {
        (
            segment_id(
                capture
                    .segments
                    .iter()
                    .filter(|segment| segment.close.is_some())
                    .count(),
            ),
            sequence.saturating_sub(base),
            capture
                .segments
                .iter()
                .filter(|segment| segment.close.is_some())
                .count(),
        )
    });
    if !replaying_closed && sequence < base {
        return gap_response(sequence);
    }
    let payload = body.to_vec();
    let digest = format!("{:x}", Sha256::digest(&payload));
    // A lost ACK can cause a retry after the server's receive-time clamp has
    // moved. Compare the durable digest before normalizing the client clock.
    match state.workspace_service.capture_chunk_metadata(
        &principal,
        &session,
        segment.as_ref(),
        &LANE.into(),
        local_sequence,
    ) {
        Ok(Some(existing)) if existing.payload_digest.hex == digest => {
            return http::workspace_ok(json!({"sequence": sequence, "durable": true}));
        }
        Ok(Some(_)) => {
            return http::workspace_error(
                StatusCode::CONFLICT,
                "browser_chunk_conflict",
                false,
                "Browser audio sequence is already durable with different content",
            );
        }
        Ok(None) => {}
        Err(error) => return browser_error(error),
    }
    let prior_chunk_end = match state.workspace_service.capture_chunk_end_before(
        &principal,
        &session,
        segment.as_ref(),
        &LANE.into(),
        local_sequence,
    ) {
        Ok(value) => value.unwrap_or(0),
        Err(error) => return browser_error(error),
    };
    let previous_close_end = if replaying_closed {
        0
    } else {
        capture
            .segments
            .iter()
            .filter_map(|segment| segment.close.as_ref())
            .map(|close| close.command.ended_at_ms.0)
            .max()
            .unwrap_or(0)
    };
    let floor_ms = prior_chunk_end.max(previous_close_end);
    let received_unix_ms = now().0;
    let server_received_at_ms = received_unix_ms.saturating_sub(capture.started_at_unix_ms);
    let backwards_after_pause = !replaying_closed
        && segment_index > 0
        && captured_start_unix_ms
            < capture
                .client_started_at_unix_ms
                .saturating_add(previous_close_end);
    let starts_at_ms = if backwards_after_pause {
        server_received_at_ms
            .saturating_sub(duration_ms)
            .max(floor_ms)
    } else {
        session_millis(
            &capture,
            Some(captured_start_unix_ms),
            received_unix_ms,
            floor_ms,
        )
        .max(floor_ms)
    };
    let ends_at_ms = if backwards_after_pause {
        server_received_at_ms.max(starts_at_ms.saturating_add(1))
    } else {
        session_millis(
            &capture,
            Some(captured_end_unix_ms),
            received_unix_ms,
            starts_at_ms,
        )
        .max(starts_at_ms.saturating_add(1))
    };
    if let Err(error) = state.workspace_service.recover_capture_for_producer(
        &principal,
        owner,
        &session,
        MessageId(format!("browser-recover-{segment_index}")),
    ) {
        return browser_error(error);
    }
    let result = state.workspace_service.execute_capture(
        &principal,
        owner,
        command(
            &session,
            format!("browser-chunk-{sequence}"),
            ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                segment_id: segment,
                lane_id: LANE.into(),
                sequence: local_sequence,
                starts_at_ms: SessionMillis(starts_at_ms),
                duration_ms: DurationMillis(ends_at_ms - starts_at_ms),
                payload_digest: ContentDigestV1 {
                    algorithm: DigestAlgorithmV1::Sha256,
                    hex: digest,
                },
                payload,
            }),
        ),
    );
    match result {
        Ok(response)
            if response
                .messages
                .iter()
                .any(|event| matches!(event.body, ServerMessageBodyV1::AudioAcknowledged(_))) =>
        {
            http::workspace_ok(json!({"sequence": sequence, "durable": true}))
        }
        Ok(_) => http::workspace_error(
            StatusCode::CONFLICT,
            "chunk_not_acknowledged",
            true,
            "Browser audio chunk was not durably acknowledged",
        ),
        Err(error) => browser_error(error),
    }
}

fn execute_completion_command(
    state: &ServerState,
    principal: &ServicePrincipal,
    owner: Option<&str>,
    command: ClientMessageV1,
) -> Result<margins_meeting_runtime::RuntimeResponseV1> {
    match owner {
        Some(owner) => state
            .workspace_service
            .execute_capture(principal, owner, command),
        None => state
            .workspace_service
            .execute_expired_capture(principal, command),
    }
}

fn completion_capture(
    state: &ServerState,
    principal: &ServicePrincipal,
    owner: Option<&str>,
    session: &SessionId,
) -> Result<CaptureState> {
    match owner {
        Some(owner) => state
            .workspace_service
            .capture_state_for_producer(principal, owner, session),
        None => state.workspace_service.capture_state(principal, session),
    }
}

fn declare_missing_chunks(
    state: &ServerState,
    principal: &ServicePrincipal,
    owner: Option<&str>,
    session: &SessionId,
    capture: &CaptureState,
    position: &Layout,
    local_horizon: u64,
) -> Result<(Vec<SequenceRangeV1>, u64)> {
    let missing = missing_ranges(capture, local_horizon);
    let mut latest_gap_end_ms = 0;
    let segment = segment_id(position.closed_count);
    for range in &missing {
        let anchor_ms = state
            .workspace_service
            .capture_chunk_end_before(
                principal,
                session,
                segment.as_ref(),
                &LANE.into(),
                range.start,
            )?
            .unwrap_or(position.latest_close_end_ms);
        latest_gap_end_ms = latest_gap_end_ms.max(anchor_ms.saturating_add(1));
        let discontinuity = CaptureDiscontinuityV1 {
            discontinuity_id: DiscontinuityId(format!(
                "browser-missing-{}-{}-{}",
                position.closed_count, range.start, range.end_exclusive
            )),
            segment_id: segment.clone(),
            lane_id: LANE.into(),
            sequence_range: *range,
            starts_at_ms: SessionMillis(anchor_ms),
            duration_ms: DurationMillis(1),
            reason: DiscontinuityReasonV1::NetworkLoss,
            detail: Some("Browser audio upload was not durably received".into()),
        };
        execute_completion_command(
            state,
            principal,
            owner,
            command(
                session,
                format!(
                    "browser-missing-command-{}-{}-{}",
                    position.closed_count, range.start, range.end_exclusive
                ),
                ClientMessageBodyV1::CaptureDiscontinuity(discontinuity),
            ),
        )?;
    }
    Ok((missing, latest_gap_end_ms))
}

fn schedule_finalized_audio(state: &ServerState) {
    if state.workspace_service.asr_available() {
        if let Err(error) = state.remote_asr_jobs.schedule_pending(
            state.workspace_service.clone(),
            state.service_principal.clone(),
        ) {
            eprintln!("[margins-server] deferred ASR schedule after browser finalize: {error:#}");
        }
    }
}

/// This path is deliberately distinct from strict Stop. Every absent chunk is
/// declared as a durable runtime discontinuity before an Error finalization.
/// The same function serves a user decision and an expired owner lease.
fn finalize_incomplete(
    state: &ServerState,
    principal: &ServicePrincipal,
    owner: Option<&str>,
    session: &SessionId,
    expected_next_sequence: u64,
    ended_unix_ms: Option<u64>,
) -> Response {
    let capture = match completion_capture(state, principal, owner, session) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if capture.input_finalized {
        if owner.is_none() {
            if let Err(error) = state
                .workspace_service
                .release_expired_finalized_capture(principal, session)
            {
                return browser_error(error);
            }
        }
        schedule_finalized_audio(state);
        let incomplete = state
            .workspace_service
            .session(principal, session)
            .map(|summary| summary.capture_incomplete)
            .unwrap_or(true);
        return http::workspace_ok(json!({
            "sessionId": session.as_ref(),
            "inputFinalized": true,
            "incomplete": incomplete
        }));
    }
    if let Some(owner) = owner {
        if let Err(error) = state
            .workspace_service
            .touch_capture_producer(principal, owner, session)
        {
            return browser_error(error);
        }
    }
    let position = match layout(&capture) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let horizon = expected_next_sequence
        .max(durable_horizon(&capture, &position))
        .max(position.closed_total);
    let local_horizon = horizon - position.closed_total;
    let ended_at_ms = session_millis(
        &capture,
        ended_unix_ms,
        now().0,
        position
            .current_latest_end_ms
            .max(position.latest_close_end_ms),
    )
    .max(position.current_latest_end_ms)
    .max(position.latest_close_end_ms)
    .max(1);
    let (missing, latest_gap_end_ms) = match declare_missing_chunks(
        state,
        principal,
        owner,
        session,
        &capture,
        &position,
        local_horizon,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let ended_at_ms = ended_at_ms.max(latest_gap_end_ms);
    let covered = match completion_capture(state, principal, owner, session) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let position = match complete_through(&covered, horizon) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let all_closed = covered
        .segments
        .iter()
        .all(|segment| segment.close.is_some());
    if !all_closed || horizon > position.closed_total {
        let close = CloseSegmentV1 {
            segment_id: segment_id(position.closed_count),
            ended_at_ms: SessionMillis(ended_at_ms.max(position.current_latest_end_ms)),
            lane_boundaries: vec![LaneBoundaryV1 {
                lane_id: LANE.into(),
                next_sequence: local_horizon,
            }],
            reason: SegmentCloseReasonV1::Error,
        };
        if let Err(error) = execute_completion_command(
            state,
            principal,
            owner,
            command(
                session,
                format!("browser-incomplete-close-{}", position.closed_count),
                ClientMessageBodyV1::CloseSegment(close),
            ),
        ) {
            return browser_error(error);
        }
    }
    let closed = match completion_capture(state, principal, owner, session) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let references = closed
        .segments
        .iter()
        .filter_map(|segment| {
            segment.close.as_ref().map(|close| SegmentCloseReferenceV1 {
                segment_id: segment.segment_id.clone(),
                close_message_id: close.message_id.clone(),
            })
        })
        .collect();
    let result = execute_completion_command(
        state,
        principal,
        owner,
        command(
            session,
            format!(
                "browser-{}-incomplete-finish-{horizon}",
                if owner.is_none() { "lease" } else { "user" }
            ),
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(
                    ended_at_ms
                        .max(position.current_latest_end_ms)
                        .max(position.latest_close_end_ms),
                ),
                segment_closes: references,
                reason: SessionFinalizeReasonV1::Error,
            }),
        ),
    );
    match result {
        Ok(_) => {
            schedule_finalized_audio(state);
            http::workspace_ok(json!({
                "sessionId": session.as_ref(),
                "inputFinalized": true,
                "incomplete": true,
                "missingRanges": missing
            }))
        }
        Err(error) => browser_error(error),
    }
}

pub async fn finish_incomplete(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<BoundaryBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &body.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    finalize_incomplete(
        &state,
        &principal,
        Some(&body.owner_id),
        &session,
        body.expected_next_sequence,
        body.segment_ended_unix_ms,
    )
}

/// Reconstruct and finish expired captures using only durable runtime and
/// producer rows. Claimed rows remain retryable after a server crash.
pub fn sweep_expired_browser_captures(state: &ServerState, observed_unix_ms: u64) -> Result<usize> {
    let cutoff = i64::try_from(observed_unix_ms.saturating_sub(BROWSER_OWNER_LEASE_MS))
        .context("browser owner lease cutoff exceeds SQLite range")?;
    let sessions = state
        .workspace_service
        .claim_expired_browser_producers(&state.service_principal, cutoff)?;
    let mut finished = 0;
    for session in sessions {
        let response = finalize_incomplete(
            state,
            &state.service_principal,
            None,
            &session,
            0,
            Some(observed_unix_ms),
        );
        if response.status().is_success() {
            finished += 1;
        } else {
            eprintln!(
                "[margins-server] expired browser capture {} remains pending: {}",
                session.as_ref(),
                response.status()
            );
        }
    }
    Ok(finished)
}

pub async fn stop(
    State(state): State<ServerState>,
    Path((workspace, recording)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<BoundaryBody>,
) -> Response {
    let principal = match principal(&state, &headers, &workspace, OP_CAPTURE_WRITE) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match authorized_recording(&recording, &body.owner_id) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    // The producer reservation is released on finalization. The hashed
    // recording identity still proves a lost Stop response belongs to owner.
    if state
        .workspace_service
        .capture_state(&principal, &session)
        .is_ok_and(|capture| capture.input_finalized)
    {
        if state.workspace_service.asr_available() {
            if let Err(error) = state.remote_asr_jobs.schedule_pending(
                state.workspace_service.clone(),
                state.service_principal.clone(),
            ) {
                eprintln!("[margins-server] deferred ASR schedule after Stop replay: {error:#}");
            }
        }
        let incomplete = state
            .workspace_service
            .session(&principal, &session)
            .map(|summary| summary.capture_incomplete)
            .unwrap_or(false);
        return http::workspace_ok(
            json!({"sessionId": recording, "inputFinalized": true, "incomplete": incomplete}),
        );
    }
    let capture = match state.workspace_service.capture_state_for_producer(
        &principal,
        &body.owner_id,
        &session,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if let Err(error) =
        state
            .workspace_service
            .touch_capture_producer(&principal, &body.owner_id, &session)
    {
        return browser_error(error);
    }
    let position = match complete_through(&capture, body.expected_next_sequence) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if body.expected_next_sequence == 0 {
        return http::workspace_error(
            StatusCode::CONFLICT,
            "browser_audio_empty",
            false,
            "Recording has no durable audio chunk; choose Finish with what was saved",
        );
    }
    if durable_horizon(&capture, &position) > body.expected_next_sequence {
        return http::workspace_error(
            StatusCode::CONFLICT,
            "browser_sequence_stale",
            true,
            "Server has newer durable audio; refresh the capture snapshot before Stop",
        );
    }
    let ended_at_ms = session_millis(
        &capture,
        body.segment_ended_unix_ms,
        now().0,
        position
            .current_latest_end_ms
            .max(position.latest_close_end_ms),
    )
    .max(position.current_latest_end_ms)
    .max(position.latest_close_end_ms);
    if capture
        .segments
        .iter()
        .all(|segment| segment.close.is_some())
        && position.closed_total == body.expected_next_sequence
    {
        // Stop after Pause, with no new MediaRecorder segment.
    } else {
        let close = CloseSegmentV1 {
            segment_id: segment_id(position.closed_count),
            ended_at_ms: SessionMillis(ended_at_ms),
            lane_boundaries: vec![LaneBoundaryV1 {
                lane_id: LANE.into(),
                next_sequence: body.expected_next_sequence - position.closed_total,
            }],
            reason: SegmentCloseReasonV1::Stop,
        };
        if let Err(error) = state.workspace_service.execute_capture(
            &principal,
            &body.owner_id,
            command(
                &session,
                format!("browser-stop-close-{}", position.closed_count),
                ClientMessageBodyV1::CloseSegment(close),
            ),
        ) {
            return browser_error(error);
        }
    }
    let closed = match state.workspace_service.capture_state_for_producer(
        &principal,
        &body.owner_id,
        &session,
    ) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let references = closed
        .segments
        .iter()
        .filter_map(|segment| {
            segment.close.as_ref().map(|close| SegmentCloseReferenceV1 {
                segment_id: segment.segment_id.clone(),
                close_message_id: close.message_id.clone(),
            })
        })
        .collect();
    let result = state.workspace_service.execute_capture(
        &principal,
        &body.owner_id,
        command(
            &session,
            format!("browser-finish-{}", body.expected_next_sequence),
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(ended_at_ms),
                segment_closes: references,
                reason: SessionFinalizeReasonV1::Completed,
            }),
        ),
    );
    match result {
        Ok(_) => {
            if state.workspace_service.asr_available() {
                if let Err(error) = state.remote_asr_jobs.schedule_pending(
                    state.workspace_service.clone(),
                    state.service_principal.clone(),
                ) {
                    eprintln!("[margins-server] deferred ASR schedule after Stop: {error:#}");
                }
            }
            let incomplete = state
                .workspace_service
                .session(&principal, &session)
                .map(|summary| summary.capture_incomplete)
                .unwrap_or(false);
            http::workspace_ok(
                json!({"sessionId": recording, "inputFinalized": true, "incomplete": incomplete}),
            )
        }
        Err(error) => browser_error(error),
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn browser_snapshot_matches_shared_plugin_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../contracts/browser-snapshot.v3.json")).unwrap();
        let rust = BrowserSnapshot {
            recording_id: "browser-contract-session".into(),
            session_id: "browser-contract-session".into(),
            next_sequence: 3,
            status: BrowserStatus::Paused,
            incomplete: false,
            expired_lease: false,
            notepad: BrowserNotepad {
                text: "Review launch plan".into(),
                revision: "memo-revision".into(),
            },
        };
        assert_eq!(serde_json::to_value(rust).unwrap(), fixture);
    }
}
