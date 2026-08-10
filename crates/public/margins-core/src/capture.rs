//! In-process capture provider contract.

use crate::audio::{ArtifactDescriptor, ArtifactDestination, AudioLane, PcmChunk};
use crate::event::EventEnvelope;
use crate::ids::{CaptureDeviceId, CaptureOperationId};
use crate::{SegmentId, SessionId};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// Static capabilities reported before attempting capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureCapabilities {
    pub available: bool,
    pub supported_lanes: Vec<AudioLane>,
    pub supports_device_selection: bool,
    pub supports_live_pcm: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl CaptureCapabilities {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            supported_lanes: Vec::new(),
            supports_device_selection: false,
            supports_live_pcm: false,
            unavailable_reason: Some(reason.into()),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDeviceKind {
    Input,
    System,
    Virtual,
}

/// Provider-opaque device descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDevice {
    pub id: CaptureDeviceId,
    pub name: String,
    pub kind: CaptureDeviceKind,
    pub is_default: bool,
    pub is_available: bool,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Unknown,
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    Unavailable,
}

/// Inputs for starting one capture segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
    pub operation_id: CaptureOperationId,
    pub lanes: Vec<AudioLane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device_id: Option<CaptureDeviceId>,
    pub deliver_live_pcm: bool,
    pub destination: ArtifactDestination,
}

/// A command is paired with an idempotency key independent of its payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureCommand {
    pub operation_id: CaptureOperationId,
    /// Segment the caller observed when it issued this command.
    ///
    /// Providers reject a mismatch instead of allowing a delayed command to
    /// mutate a segment installed by a later resume operation.
    pub expected_segment_id: SegmentId,
    pub action: CaptureAction,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CaptureAction {
    Pause,
    Resume {
        segment_id: SegmentId,
        destination: ArtifactDestination,
    },
    SelectInput {
        device_id: CaptureDeviceId,
    },
    RestartLane {
        lane: AudioLane,
    },
    Finish,
    Cancel,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCommandStatus {
    Applied,
    AlreadyApplied,
}

/// Terminal value for a completed capture command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureCommandResult {
    pub operation_id: CaptureOperationId,
    pub status: CaptureCommandStatus,
    pub snapshot: CaptureSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_artifacts: Vec<ArtifactDescriptor>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Starting,
    Capturing,
    Paused,
    Finishing,
    Finished,
    Cancelled,
    Failed,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureLaneState {
    Starting,
    Active,
    Interrupted,
    Stopped,
    Failed,
}

/// Immutable telemetry for one logical lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureLaneSnapshot {
    pub lane: AudioLane,
    pub state: CaptureLaneState,
    pub generation: u64,
    pub delivered_frames: u64,
    pub durable_frames: u64,
    pub observed_signal: bool,
    pub dropped_live_frames: u64,
    pub dropped_durable_frames: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<CaptureErrorCode>,
}

/// Immutable state returned by a capture handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSnapshot {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
    pub state: CaptureState,
    pub lanes: Vec<CaptureLaneSnapshot>,
    pub timeline_reusable: bool,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureErrorCode {
    Unavailable,
    PermissionDenied,
    DeviceLost,
    OpenFailed,
    Stalled,
    WriterFailed,
    Cancelled,
    InvalidTransition,
    Internal,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAdvice {
    Retry,
    DoNotRetry,
    RequestPermission,
    SelectAnotherDevice,
}

/// Machine-stable capture failure plus presentation-neutral context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureError {
    pub code: CaptureErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_advice: Option<RetryAdvice>,
}

impl CaptureError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: CaptureErrorCode::Unavailable,
            message: message.into(),
            retry_advice: Some(RetryAdvice::DoNotRetry),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl Error for CaptureError {}

pub trait CaptureProvider: Send + Sync {
    fn capabilities(&self) -> CaptureCapabilities;
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError>;
    fn permission(&self, lane: AudioLane) -> Result<PermissionState, CaptureError>;
    fn request_permission(&self, lane: AudioLane) -> Result<PermissionState, CaptureError>;
    fn start(
        &self,
        request: CaptureRequest,
        observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}

pub trait CaptureObserver: Send + Sync {
    fn on_audio(&self, chunk: PcmChunk);
    fn on_event(&self, event: EventEnvelope);
}

pub trait CaptureHandle: Send + Sync {
    fn snapshot(&self) -> Result<CaptureSnapshot, CaptureError>;
    fn command(&self, command: CaptureCommand) -> Result<CaptureCommandResult, CaptureError>;
}

/// Provider installed by public compositions that have no native capture.
#[derive(Debug, Clone)]
pub struct UnavailableCaptureProvider {
    reason: String,
}

impl UnavailableCaptureProvider {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn error(&self) -> CaptureError {
        CaptureError::unavailable(self.reason.clone())
    }
}

impl Default for UnavailableCaptureProvider {
    fn default() -> Self {
        Self::new("capture is unavailable in this build")
    }
}

impl CaptureProvider for UnavailableCaptureProvider {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities::unavailable(self.reason.clone())
    }

    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Err(self.error())
    }

    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Err(self.error())
    }

    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Err(self.error())
    }

    fn start(
        &self,
        _request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Err(self.error())
    }
}
