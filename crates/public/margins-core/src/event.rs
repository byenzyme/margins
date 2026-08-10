//! Typed in-process events and the versioned transport envelope.

use crate::{CaptureOperationId, EventSequence, SegmentId, SessionId, TranscriptEntry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub const EVENT_SCHEMA: &str = "margins.event";
pub const EVENT_VERSION_V1: u16 = 1;

/// Forward-compatible event kind. Unknown strings are retained verbatim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventKind(pub String);

impl EventKind {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl From<&str> for EventKind {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Versioned event representation shared by Tauri, WebSocket, and other transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema: String,
    pub version: u16,
    pub sequence: EventSequence,
    pub emitted_at_ms: u64,
    pub session_id: SessionId,
    pub segment_id: Option<SegmentId>,
    pub operation_id: Option<CaptureOperationId>,
    pub kind: EventKind,
    pub payload: Value,
    /// Unknown top-level fields are retained across decode/encode cycles.
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl EventEnvelope {
    pub fn v1(
        sequence: EventSequence,
        emitted_at_ms: u64,
        session_id: SessionId,
        kind: impl Into<EventKind>,
        payload: Value,
    ) -> Self {
        Self {
            schema: EVENT_SCHEMA.to_owned(),
            version: EVENT_VERSION_V1,
            sequence,
            emitted_at_ms,
            session_id,
            segment_id: None,
            operation_id: None,
            kind: kind.into(),
            payload,
            extensions: BTreeMap::new(),
        }
    }

    /// Wraps a typed in-process event in a V1 transport envelope.
    pub fn from_event_v1(
        sequence: EventSequence,
        emitted_at_ms: u64,
        session_id: SessionId,
        segment_id: Option<SegmentId>,
        operation_id: Option<CaptureOperationId>,
        event: &Event,
    ) -> Result<Self, EventError> {
        let mut envelope = Self::v1(
            sequence,
            emitted_at_ms,
            session_id,
            event.kind(),
            event.payload()?,
        );
        envelope.segment_id = segment_id;
        envelope.operation_id = operation_id;
        envelope.validate_v1()?;
        Ok(envelope)
    }

    /// Validates V1 invariants that do not require stream history.
    ///
    /// Stateful sequence monotonicity and duplicate reduction remain the
    /// responsibility of the event-stream owner.
    pub fn validate_v1(&self) -> Result<(), EventError> {
        if self.schema != EVENT_SCHEMA {
            return Err(EventError::invalid_envelope("schema must be margins.event"));
        }
        if self.version != EVENT_VERSION_V1 {
            return Err(EventError::invalid_envelope("version must be 1"));
        }
        if self.sequence.0 > margins_meeting_protocol::MAX_SAFE_JSON_INTEGER {
            return Err(EventError::invalid_envelope(
                "sequence must be exactly representable by JSON/JavaScript",
            ));
        }
        if self.emitted_at_ms > margins_meeting_protocol::MAX_SAFE_JSON_INTEGER {
            return Err(EventError::invalid_envelope(
                "emitted_at_ms must be exactly representable by JSON/JavaScript",
            ));
        }
        if self.session_id.as_ref().trim().is_empty() {
            return Err(EventError::invalid_envelope("session_id must not be empty"));
        }
        if self
            .segment_id
            .as_ref()
            .is_some_and(|value| value.as_ref().trim().is_empty())
        {
            return Err(EventError::invalid_envelope("segment_id must not be empty"));
        }
        if self
            .operation_id
            .as_ref()
            .is_some_and(|value| value.as_ref().trim().is_empty())
        {
            return Err(EventError::invalid_envelope(
                "operation_id must not be empty",
            ));
        }
        if self.kind.0.trim().is_empty() {
            return Err(EventError::invalid_envelope("kind must not be empty"));
        }
        if !self.payload.is_object() {
            return Err(EventError::invalid_envelope(
                "payload must be a JSON object",
            ));
        }
        if is_known_segment_event(&self.kind.0) && self.segment_id.is_none() {
            return Err(EventError::invalid_envelope(
                "segment_id is required for segment-scoped events",
            ));
        }
        if self.extensions.keys().any(|key| is_envelope_field(key)) {
            return Err(EventError::invalid_envelope(
                "extensions must not shadow envelope fields",
            ));
        }
        Ok(())
    }
}

fn is_known_segment_event(kind: &str) -> bool {
    matches!(
        kind,
        "capture.started"
            | "capture.lane_state_changed"
            | "capture.health"
            | "capture.paused"
            | "capture.resumed"
            | "capture.segment_sealed"
            | "transcript.entry"
    )
}

fn is_envelope_field(key: &str) -> bool {
    matches!(
        key,
        "schema"
            | "version"
            | "sequence"
            | "emitted_at_ms"
            | "session_id"
            | "segment_id"
            | "operation_id"
            | "kind"
            | "payload"
    )
}

impl From<String> for EventKind {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Typed events used inside a process before envelope serialization.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Event {
    CaptureStarted,
    CaptureLaneStateChanged {
        lane: crate::AudioLane,
        state: crate::CaptureLaneState,
    },
    CaptureHealth {
        dropped_live_frames: u64,
        dropped_durable_frames: u64,
    },
    CapturePaused,
    CaptureResumed,
    SegmentSealed {
        artifact: crate::ArtifactDescriptor,
    },
    Progress {
        stage: String,
        progress: Option<f32>,
        message: Option<String>,
    },
    TranscriptEntry {
        entry: TranscriptEntry,
    },
    NoteDelta {
        text: String,
    },
    Failed {
        code: String,
        message: String,
    },
    Cancelled,
    Completed,
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CaptureStarted => "capture.started",
            Self::CaptureLaneStateChanged { .. } => "capture.lane_state_changed",
            Self::CaptureHealth { .. } => "capture.health",
            Self::CapturePaused => "capture.paused",
            Self::CaptureResumed => "capture.resumed",
            Self::SegmentSealed { .. } => "capture.segment_sealed",
            Self::Progress { .. } => "session.progress",
            Self::TranscriptEntry { .. } => "transcript.entry",
            Self::NoteDelta { .. } => "note.delta",
            Self::Failed { .. } => "session.failed",
            Self::Cancelled => "session.cancelled",
            Self::Completed => "session.completed",
        }
    }

    pub fn payload(&self) -> Result<Value, EventError> {
        let encoded = serde_json::to_value(self).map_err(|error| EventError {
            code: EventErrorCode::Serialization,
            message: error.to_string(),
            retryable: false,
        })?;
        let Value::Object(mut object) = encoded else {
            return Ok(Value::Object(Default::default()));
        };
        Ok(object
            .remove("payload")
            .unwrap_or_else(|| Value::Object(Default::default())))
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventErrorCode {
    Unavailable,
    Rejected,
    InvalidEnvelope,
    Serialization,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventError {
    pub code: EventErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl EventError {
    fn invalid_envelope(message: impl Into<String>) -> Self {
        Self {
            code: EventErrorCode::InvalidEnvelope,
            message: message.into(),
            retryable: false,
        }
    }
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl Error for EventError {}

pub trait EventSink: Send + Sync {
    fn publish(&self, event: &EventEnvelope) -> Result<(), EventError>;
}

/// Sink for compositions that intentionally discard events.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn publish(&self, _event: &EventEnvelope) -> Result<(), EventError> {
        Ok(())
    }
}
