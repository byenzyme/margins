//! Transcript DTOs and synchronous ASR/diarization ports.

use crate::{AudioLane, SpeakerId, TranscriptEntryId};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// Word-level ASR timing in session-relative milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptWord {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<SpeakerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_per_mille: Option<u16>,
}

/// One display/reduction unit in a transcript stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub id: TranscriptEntryId,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<SpeakerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<AudioLane>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptWord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrRequest {
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
    pub session_offset_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrResult {
    pub words: Vec<TranscriptWord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_language: Option<String>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptErrorCode {
    Unavailable,
    InvalidAudio,
    ModelLoadFailed,
    InferenceFailed,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptError {
    pub code: TranscriptErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl Error for TranscriptError {}

pub trait AsrBackend: Send + Sync {
    fn is_available(&self) -> bool {
        true
    }

    fn backend_name(&self) -> &'static str {
        "injected"
    }

    fn transcribe(&self, request: AsrRequest) -> Result<AsrResult, TranscriptError>;
}

/// A decoder fed with bounded, mono PCM windows by a live capture adapter.
///
/// The caller owns overlap, cadence and checkpoint durability. Each request is
/// independent and its word times are relative to `session_offset_ms`. A batch
/// backend can therefore serve a live stream without a second provider API.
pub trait AsrChunkDecoder: Send {
    fn decode_chunk(&mut self, request: AsrRequest) -> Result<AsrResult, TranscriptError>;
}

impl<T: AsrBackend> AsrChunkDecoder for T {
    fn decode_chunk(&mut self, request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        self.transcribe(request)
    }
}

/// Cumulative update from a provider with native incremental decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrStreamUpdate {
    pub committed: Vec<TranscriptWord>,
    pub hypothesis: Vec<TranscriptWord>,
    pub decoded_until_ms: u64,
    pub committed_until_ms: u64,
}

/// Native stateful decoder over mono 16 kHz PCM. The live adapter owns capture
/// and calls `finish_until` once for a terminal checkpoint.
pub trait AsrStreamDecoder {
    fn append_audio(&mut self, mono_16k: &[f32]);
    fn update_until(&mut self, end_ms: u64) -> Result<AsrStreamUpdate, TranscriptError>;
    fn finish_until(&mut self, end_ms: u64) -> Result<AsrStreamUpdate, TranscriptError>;
}

/// One speaker turn emitted by a diarization backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: SpeakerId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationRequest {
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
    pub session_offset_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_speakers: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiarizationResult {
    pub segments: Vec<SpeakerSegment>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationErrorCode {
    Unavailable,
    InvalidAudio,
    ModelLoadFailed,
    InferenceFailed,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiarizationError {
    pub code: DiarizationErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl fmt::Display for DiarizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl Error for DiarizationError {}

pub trait DiarizationBackend: Send + Sync {
    fn is_available(&self) -> bool {
        true
    }

    fn diarize(&self, request: DiarizationRequest) -> Result<DiarizationResult, DiarizationError>;
}
