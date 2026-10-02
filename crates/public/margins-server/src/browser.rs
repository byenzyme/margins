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
    AudioChunkV1, AudioCodecV1, AudioContainerV1, AudioFormatV1, CaptureLaneV1, CaptureModeV1,
    CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1, CaptureSourceV1,
    ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1, ContentDigestV1, CreateSessionV1,
    DigestAlgorithmV1, DurationMillis, FinalizeSessionV1, LaneBoundaryV1, MessageId,
    ProtocolVersionV1, SegmentCloseReasonV1, SegmentCloseReferenceV1, SegmentId,
    ServerMessageBodyV1, SessionFinalizeReasonV1, SessionId, SessionMillis, UnixMillis,
};
use margins_workflows::workspace_service::{
    CaptureState, ServicePrincipal, OP_CAPTURE_WRITE, OP_SESSION_CREATE, OP_SESSION_READ,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

const CHUNK_TIMESLICE_MS: u64 = 3_000;
const LANE: &str = "mic";

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
    pub status: BrowserStatus,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerBody {
    owner_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryBody {
    owner_id: String,
    expected_next_sequence: u64,
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
}

fn layout(state: &CaptureState) -> Result<Layout> {
    let mut closed_total = 0_u64;
    let mut closed_count = 0_usize;
    let mut current_watermark = 0;
    let mut current_latest_end_ms = 0;
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
    })
}

fn complete_through(state: &CaptureState, expected: u64) -> std::result::Result<Layout, u64> {
    let position = layout(state).map_err(|_| 0_u64)?;
    if expected < position.closed_total {
        return Err(expected);
    }
    let local = expected - position.closed_total;
    if position.current_watermark < local {
        return Err(position.closed_total + position.current_watermark);
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
    http::service_error(error)
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
        status: if capture.input_finalized {
            BrowserStatus::Saving
        } else if paused {
            BrowserStatus::Paused
        } else {
            BrowserStatus::Recording
        },
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
                    attributes: BTreeMap::new(),
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
    let capture = match state
        .workspace_service
        .capture_state_for_producer(&principal, &body.owner_id, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let position = match complete_through(&capture, body.expected_next_sequence) {
        Ok(value) => value,
        Err(sequence) => return gap_response(sequence),
    };
    if body.expected_next_sequence == position.closed_total
        && capture
            .segments
            .last()
            .and_then(|segment| segment.close.as_ref())
            .is_some_and(|close| close.command.reason == SegmentCloseReasonV1::Pause)
    {
        return snapshot_value(&state, &principal, &body.owner_id, &session)
            .map(http::workspace_ok)
            .unwrap_or_else(browser_error);
    }
    let local = body.expected_next_sequence - position.closed_total;
    let close = CloseSegmentV1 {
        segment_id: segment_id(position.closed_count),
        ended_at_ms: SessionMillis(
            body.expected_next_sequence
                .saturating_mul(CHUNK_TIMESLICE_MS)
                .max(position.current_latest_end_ms),
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
    let capture = match state
        .workspace_service
        .capture_state_for_producer(&principal, &body.owner_id, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let position = match layout(&capture) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
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
        .recover_capture_for_producer(
            &principal,
            &body.owner_id,
            &session,
            MessageId(format!("browser-resume-{}", position.closed_count)),
        )
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
        None => return http::workspace_error(StatusCode::UNAUTHORIZED, "owner_required", false, "Missing capture owner"),
    };
    let session = match authorized_recording(&recording, owner) {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    if body.is_empty() || body.len() as u64 > margins_workflows::workspace_service::DEFAULT_MAX_CHUNK_BYTES {
        return http::workspace_error(StatusCode::PAYLOAD_TOO_LARGE, "invalid_chunk", false, "Browser audio chunk is empty or too large");
    }
    let capture = match state
        .workspace_service
        .capture_state_for_producer(&principal, owner, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let mut base = 0_u64;
    let mut selected = None;
    for (index, segment) in capture.segments.iter().enumerate() {
        if let Some(close) = &segment.close {
            let end = match close_boundary(close).and_then(|length| base.checked_add(length).context("browser sequence overflow")) {
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
    let (segment, local_sequence, segment_index) =
        selected.unwrap_or_else(|| (segment_id(capture.segments.iter().filter(|segment| segment.close.is_some()).count()), sequence.saturating_sub(base), capture.segments.iter().filter(|segment| segment.close.is_some()).count()));
    if !replaying_closed && sequence < base {
        return gap_response(sequence);
    }
    if let Err(error) = state.workspace_service.recover_capture_for_producer(
        &principal,
        owner,
        &session,
        MessageId(format!("browser-recover-{segment_index}")),
    ) {
        return browser_error(error);
    }
    let payload = body.to_vec();
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
                starts_at_ms: SessionMillis(sequence.saturating_mul(CHUNK_TIMESLICE_MS)),
                duration_ms: DurationMillis(CHUNK_TIMESLICE_MS),
                payload_digest: ContentDigestV1 {
                    algorithm: DigestAlgorithmV1::Sha256,
                    hex: format!("{:x}", Sha256::digest(&payload)),
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
        Ok(_) => http::workspace_error(StatusCode::CONFLICT, "chunk_not_acknowledged", true, "Browser audio chunk was not durably acknowledged"),
        Err(error) => browser_error(error),
    }
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
        return http::workspace_ok(json!({"sessionId": recording, "inputFinalized": true}));
    }
    let capture = match state
        .workspace_service
        .capture_state_for_producer(&principal, &body.owner_id, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let position = match complete_through(&capture, body.expected_next_sequence) {
        Ok(value) => value,
        Err(sequence) => return gap_response(sequence),
    };
    if body.expected_next_sequence == 0 {
        return http::workspace_error(StatusCode::CONFLICT, "browser_audio_empty", true, "Recording has no durable audio chunk");
    }
    if capture.segments.iter().all(|segment| segment.close.is_some()) && position.closed_total == body.expected_next_sequence {
        // Stop after Pause, with no new MediaRecorder segment.
    } else {
        let close = CloseSegmentV1 {
            segment_id: segment_id(position.closed_count),
            ended_at_ms: SessionMillis(
                body.expected_next_sequence
                    .saturating_mul(CHUNK_TIMESLICE_MS)
                    .max(position.current_latest_end_ms),
            ),
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
    let closed = match state
        .workspace_service
        .capture_state_for_producer(&principal, &body.owner_id, &session)
    {
        Ok(value) => value,
        Err(error) => return browser_error(error),
    };
    let references = closed
        .segments
        .iter()
        .filter_map(|segment| segment.close.as_ref().map(|close| SegmentCloseReferenceV1 {
            segment_id: segment.segment_id.clone(),
            close_message_id: close.message_id.clone(),
        }))
        .collect();
    let result = state.workspace_service.execute_capture(
        &principal,
        &body.owner_id,
        command(
            &session,
            format!("browser-finish-{}", body.expected_next_sequence),
            ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(body.expected_next_sequence.saturating_mul(CHUNK_TIMESLICE_MS)),
                segment_closes: references,
                reason: SessionFinalizeReasonV1::Completed,
            }),
        ),
    );
    match result {
        Ok(_) => http::workspace_ok(json!({"sessionId": recording, "inputFinalized": true})),
        Err(error) => browser_error(error),
    }
}
