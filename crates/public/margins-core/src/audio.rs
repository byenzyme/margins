//! Platform-neutral audio and artifact values.

use crate::{ArtifactId, SegmentId};
use serde::{Deserialize, Serialize};

pub use margins_meeting_protocol::{DurationMillis, SessionMillis, UnixMillis};

/// A logical mono lane in a capture session.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioLane {
    Microphone,
    System,
}

/// Sample encoding for a durable artifact.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    Float32,
    Signed16,
}

/// Describes decoded PCM without exposing a codec or device handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub sample_format: SampleFormat,
}

/// Owned mono PCM delivered to an in-process observer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcmChunk {
    pub lane: AudioLane,
    pub generation: u64,
    pub session_offset_ms: SessionMillis,
    pub sample_rate_hz: u32,
    pub samples: Vec<f32>,
}

/// Application-selected durable destination. The URI is opaque to core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDestination {
    pub uri: String,
}

/// A completed durable artifact returned by a provider or repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDescriptor {
    pub id: ArtifactId,
    pub segment_id: SegmentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<AudioLane>,
    pub uri: String,
    pub format: AudioFormat,
    pub duration_ms: DurationMillis,
    pub frame_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_length: Option<u64>,
}
