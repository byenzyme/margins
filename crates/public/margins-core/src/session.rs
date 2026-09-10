//! Persisted session values and repository port.

use crate::audio::{ArtifactDescriptor, DurationMillis, UnixMillis};
use crate::{ArtifactId, SegmentId, SessionId};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    Active,
    Paused,
    Processing,
    Ready,
    NeedsAttention,
    Tombstoned,
}

impl SessionLifecycle {
    /// Returns whether the centralized lifecycle permits this transition.
    pub const fn can_transition_to(self, next: Self) -> bool {
        use SessionLifecycle::*;
        matches!(
            (self, next),
            (Active, Paused | Processing | NeedsAttention | Tombstoned)
                | (Paused, Active | Processing | NeedsAttention | Tombstoned)
                | (Processing, Ready | NeedsAttention | Tombstoned)
                | (Ready, Processing | NeedsAttention | Tombstoned)
                | (NeedsAttention, Active | Processing | Ready | Tombstoned)
        ) || self as u8 == next as u8
    }
}

/// Immutable, sealed segment metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRecord {
    pub id: SegmentId,
    pub ordinal: u64,
    pub start_offset_ms: u64,
    pub duration_ms: DurationMillis,
    pub started_at_ms: UnixMillis,
    pub audio: ArtifactDescriptor,
    pub dropped_live_frames: u64,
    pub dropped_durable_frames: u64,
    pub timeline_reusable: bool,
}

/// Values required to append a sealed segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSegment {
    pub id: SegmentId,
    pub ordinal: u64,
    pub start_offset_ms: u64,
    pub duration_ms: DurationMillis,
    pub started_at_ms: UnixMillis,
    pub audio: ArtifactDescriptor,
    pub dropped_live_frames: u64,
    pub dropped_durable_frames: u64,
    pub timeline_reusable: bool,
}

impl From<NewSegment> for SegmentRecord {
    fn from(value: NewSegment) -> Self {
        Self {
            id: value.id,
            ordinal: value.ordinal,
            start_offset_ms: value.start_offset_ms,
            duration_ms: value.duration_ms,
            started_at_ms: value.started_at_ms,
            audio: value.audio,
            dropped_live_frames: value.dropped_live_frames,
            dropped_durable_frames: value.dropped_durable_frames,
            timeline_reusable: value.timeline_reusable,
        }
    }
}

/// Registered artifact associated with a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionArtifact {
    pub id: ArtifactId,
    pub session_id: SessionId,
    pub kind: String,
    pub ordinal: u64,
    pub uri: String,
    pub retention_class: String,
    pub created_at_ms: UnixMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<UnixMillis>,
}

/// Inputs for creating a durable session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSession {
    pub id: SessionId,
    pub started_at_ms: UnixMillis,
    pub created_at_ms: UnixMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_uri: Option<String>,
}

/// Complete persisted session aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub revision: u64,
    pub lifecycle: SessionLifecycle,
    pub started_at_ms: UnixMillis,
    pub created_at_ms: UnixMillis,
    pub updated_at_ms: UnixMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_error: Option<String>,
    pub segments: Vec<SegmentRecord>,
    pub artifacts: Vec<SessionArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub revision: u64,
    pub lifecycle: SessionLifecycle,
    pub started_at_ms: UnixMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub segment_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<SessionLifecycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_after_ms: Option<UnixMillis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_before_ms: Option<UnixMillis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    NotFound,
    AlreadyExists,
    Conflict,
    InvalidTransition,
    InvalidSegment,
    Unavailable,
    CorruptData,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl SessionError {
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: SessionErrorCode::Conflict,
            message: message.into(),
            retryable: true,
        }
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl Error for SessionError {}

pub trait SessionRepository: Send + Sync {
    fn create(&self, new: NewSession) -> Result<SessionRecord, SessionError>;
    fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, SessionError>;
    fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError>;
    fn append_segment(
        &self,
        id: &SessionId,
        expected_revision: u64,
        segment: NewSegment,
    ) -> Result<SessionRecord, SessionError>;
    fn transition(
        &self,
        id: &SessionId,
        expected_revision: u64,
        next: SessionLifecycle,
    ) -> Result<SessionRecord, SessionError>;
    /// Inserts or replaces an artifact as one revision-guarded aggregate mutation.
    fn upsert_artifact(
        &self,
        expected_revision: u64,
        artifact: SessionArtifact,
    ) -> Result<SessionRecord, SessionError>;
    fn tombstone(&self, id: &SessionId, expected_revision: u64) -> Result<(), SessionError>;
}
