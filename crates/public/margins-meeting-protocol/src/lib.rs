//! Public, transport-neutral wire types for a Margins meeting runtime.
//!
//! The crate deliberately contains no networking, audio capture, async runtime,
//! or platform bindings. Applications choose their own framing and transport.

#![forbid(unsafe_code)]

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{
    de::{self, DeserializeOwned, Visitor},
    ser::SerializeStruct,
    Deserialize, Deserializer, Serialize, Serializer,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// The only protocol version understood by the `V1` DTOs.
pub const PROTOCOL_VERSION_V1: u16 = 1;

/// Largest integer that JSON/JavaScript clients can represent exactly.
///
/// V1 counters and timestamps must not exceed this value. This keeps the JSON
/// contract safe for browsers while retaining `u64` storage in Rust.
pub const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

/// A local, message-level V1 validation failure.
///
/// Validation that needs durable session state (idempotency conflicts, prior
/// sequences, lifecycle, and cross-message time ordering) remains the
/// runtime's responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrorV1 {
    pub field: &'static str,
    pub problem: &'static str,
}

impl fmt::Display for ValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.problem)
    }
}

impl std::error::Error for ValidationErrorV1 {}

fn invalid(field: &'static str, problem: &'static str) -> ValidationErrorV1 {
    ValidationErrorV1 { field, problem }
}

fn validate_id(field: &'static str, value: &str) -> Result<(), ValidationErrorV1> {
    if value.trim().is_empty() {
        Err(invalid(field, "must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_json_integer(field: &'static str, value: u64) -> Result<(), ValidationErrorV1> {
    if value > MAX_SAFE_JSON_INTEGER {
        Err(invalid(
            field,
            "must be exactly representable by JSON/JavaScript",
        ))
    } else {
        Ok(())
    }
}

/// A wire-level version marker that serializes as the integer `1`.
///
/// Deserialization rejects any other value before a message body is handled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct ProtocolVersionV1;

impl TryFrom<u16> for ProtocolVersionV1 {
    type Error = String;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value == PROTOCOL_VERSION_V1 {
            Ok(Self)
        } else {
            Err(format!(
                "unsupported protocol version {value}; expected {PROTOCOL_VERSION_V1}"
            ))
        }
    }
}

impl From<ProtocolVersionV1> for u16 {
    fn from(_: ProtocolVersionV1) -> Self {
        PROTOCOL_VERSION_V1
    }
}

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(/// Stable ID for a meeting session. Session IDs are chosen by the creator.
    SessionId);
string_id!(/// Globally unique ID for one command or event.
    MessageId);
string_id!(/// Stable ID for a capture source.
    SourceId);
string_id!(/// Stable ID for an ordered audio lane.
    LaneId);
string_id!(/// Stable ID for a recording segment.
    SegmentId);
string_id!(/// Stable ID for a reported capture discontinuity.
    DiscontinuityId);
string_id!(/// Stable ID for a partial transcript hypothesis.
    HypothesisId);
string_id!(/// Stable ID for a transcript span.
    TranscriptSpanId);
string_id!(/// Stable ID for a transcript commit.
    CommitId);
string_id!(/// Stable ID for a generated memo.
    MemoId);
string_id!(/// Stable ID for a generated artifact.
    ArtifactId);

/// Milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixMillis(pub u64);

/// Milliseconds from the session's monotonic time origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionMillis(pub u64);

/// An elapsed duration in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationMillis(pub u64);

/// A client-to-runtime message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMessageV1 {
    pub protocol_version: ProtocolVersionV1,
    pub message_id: MessageId,
    pub session_id: SessionId,
    pub sent_at_unix_ms: UnixMillis,
    pub body: ClientMessageBodyV1,
}

impl ClientMessageV1 {
    /// Validates invariants that can be checked without session state.
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("message_id", self.message_id.as_ref())?;
        validate_id("session_id", self.session_id.as_ref())?;
        validate_json_integer("sent_at_unix_ms", self.sent_at_unix_ms.0)?;
        self.body.validate()
    }
}

/// Commands and observations accepted by a meeting runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ClientMessageBodyV1 {
    CreateSession(CreateSessionV1),
    ResumeSession(ResumeSessionV1),
    AppendProvenanceHop(AppendProvenanceHopV1),
    AudioChunk(AudioChunkV1),
    CaptureDiscontinuity(CaptureDiscontinuityV1),
    CaptureHealth(CaptureHealthV1),
    CloseSegment(CloseSegmentV1),
    FinalizeSession(FinalizeSessionV1),
}

impl ClientMessageBodyV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        match self {
            Self::CreateSession(value) => value.validate(),
            Self::ResumeSession(value) => value.validate(),
            Self::AppendProvenanceHop(value) => value.validate(),
            Self::AudioChunk(value) => value.validate(),
            Self::CaptureDiscontinuity(value) => value.validate(),
            Self::CaptureHealth(value) => value.validate(),
            Self::CloseSegment(value) => value.validate(),
            Self::FinalizeSession(value) => value.validate(),
        }
    }
}

/// A runtime-to-client message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMessageV1 {
    pub protocol_version: ProtocolVersionV1,
    pub message_id: MessageId,
    pub session_id: SessionId,
    /// Zero-based, strictly increasing within the session's server event
    /// stream. Replayed events retain their original sequence and message ID.
    pub sequence: u64,
    pub sent_at_unix_ms: UnixMillis,
    pub body: ServerMessageBodyV1,
}

impl ServerMessageV1 {
    /// Validates invariants that can be checked without session state.
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("message_id", self.message_id.as_ref())?;
        validate_id("session_id", self.session_id.as_ref())?;
        validate_json_integer("sequence", self.sequence)?;
        validate_json_integer("sent_at_unix_ms", self.sent_at_unix_ms.0)?;
        self.body.validate()
    }
}

/// Events emitted by a meeting runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessageBodyV1 {
    SessionCreated(SessionCreatedV1),
    ReplayCompleted(ReplayCompletedV1),
    ProvenanceHopRecorded(ProvenanceHopRecordedV1),
    CommandRejected(CommandRejectedV1),
    AudioAcknowledged(AudioAcknowledgementV1),
    SegmentFinalized(SegmentFinalizedV1),
    SessionFinalized(SessionFinalizedV1),
    TranscriptPartial(TranscriptPartialV1),
    TranscriptCommitted(TranscriptCommittedV1),
    Memo(MemoEventV1),
    ArtifactReady(ArtifactReadyV1),
    /// A future event kind retained by V1 transports and ignored by reducers
    /// that do not understand it.
    Unknown(UnknownServerMessageV1),
}

/// A server event introduced after this V1 library was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownServerMessageV1 {
    pub message_type: String,
    pub payload: Value,
}

impl ServerMessageBodyV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        match self {
            Self::AudioAcknowledged(value) => value.validate(),
            Self::SegmentFinalized(value) => value.validate(),
            Self::SessionFinalized(value) => value.validate(),
            Self::TranscriptPartial(value) => value.validate(),
            Self::TranscriptCommitted(value) => value.validate(),
            Self::Memo(value) => value.validate(),
            Self::ArtifactReady(value) => value.validate(),
            Self::Unknown(value) => validate_id("body.type", &value.message_type),
            Self::SessionCreated(value) => {
                validate_id("create_message_id", value.create_message_id.as_ref())?;
                validate_json_integer("created_at_unix_ms", value.created_at_unix_ms.0)
            }
            Self::ReplayCompleted(value) => value.validate(),
            Self::ProvenanceHopRecorded(value) => value.validate(),
            Self::CommandRejected(value) => value.validate(),
        }
    }
}

impl Serialize for ServerMessageBodyV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ServerMessageBodyV1", 2)?;
        macro_rules! known {
            ($kind:literal, $payload:expr) => {{
                state.serialize_field("type", $kind)?;
                state.serialize_field("payload", $payload)?;
            }};
        }
        match self {
            Self::SessionCreated(value) => known!("session_created", value),
            Self::ReplayCompleted(value) => known!("replay_completed", value),
            Self::ProvenanceHopRecorded(value) => known!("provenance_hop_recorded", value),
            Self::CommandRejected(value) => known!("command_rejected", value),
            Self::AudioAcknowledged(value) => known!("audio_acknowledged", value),
            Self::SegmentFinalized(value) => known!("segment_finalized", value),
            Self::SessionFinalized(value) => known!("session_finalized", value),
            Self::TranscriptPartial(value) => known!("transcript_partial", value),
            Self::TranscriptCommitted(value) => known!("transcript_committed", value),
            Self::Memo(value) => known!("memo", value),
            Self::ArtifactReady(value) => known!("artifact_ready", value),
            Self::Unknown(value) => {
                state.serialize_field("type", &value.message_type)?;
                state.serialize_field("payload", &value.payload)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for ServerMessageBodyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireBody {
            #[serde(rename = "type")]
            message_type: String,
            #[serde(default)]
            payload: Value,
        }

        fn payload<T, E>(value: Value) -> Result<T, E>
        where
            T: DeserializeOwned,
            E: de::Error,
        {
            serde_json::from_value(value).map_err(E::custom)
        }

        let wire = WireBody::deserialize(deserializer)?;
        Ok(match wire.message_type.as_str() {
            "session_created" => Self::SessionCreated(payload(wire.payload)?),
            "replay_completed" => Self::ReplayCompleted(payload(wire.payload)?),
            "provenance_hop_recorded" => Self::ProvenanceHopRecorded(payload(wire.payload)?),
            "command_rejected" => Self::CommandRejected(payload(wire.payload)?),
            "audio_acknowledged" => Self::AudioAcknowledged(payload(wire.payload)?),
            "segment_finalized" => Self::SegmentFinalized(payload(wire.payload)?),
            "session_finalized" => Self::SessionFinalized(payload(wire.payload)?),
            "transcript_partial" => Self::TranscriptPartial(payload(wire.payload)?),
            "transcript_committed" => Self::TranscriptCommitted(payload(wire.payload)?),
            "memo" => Self::Memo(payload(wire.payload)?),
            "artifact_ready" => Self::ArtifactReady(payload(wire.payload)?),
            _ => Self::Unknown(UnknownServerMessageV1 {
                message_type: wire.message_type,
                payload: wire.payload,
            }),
        })
    }
}

/// Creates a session and declares its initial capture graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSessionV1 {
    /// Stable across retries of the create operation.
    pub idempotency_key: String,
    pub started_at_unix_ms: UnixMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub sources: Vec<CaptureSourceV1>,
    pub lanes: Vec<CaptureLaneV1>,
    pub provenance: CaptureProvenanceV1,
}

impl CreateSessionV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("idempotency_key", &self.idempotency_key)?;
        validate_json_integer("started_at_unix_ms", self.started_at_unix_ms.0)?;
        if self.sources.is_empty() {
            return Err(invalid("sources", "must contain at least one source"));
        }
        if self.lanes.is_empty() {
            return Err(invalid("lanes", "must contain at least one lane"));
        }
        self.provenance.validate()?;

        let mut source_ids = BTreeSet::new();
        for source in &self.sources {
            validate_id("sources[].source_id", source.source_id.as_ref())?;
            if !source_ids.insert(source.source_id.as_ref()) {
                return Err(invalid("sources[].source_id", "must be unique"));
            }
        }
        let mut lane_ids = BTreeSet::new();
        for lane in &self.lanes {
            validate_id("lanes[].lane_id", lane.lane_id.as_ref())?;
            if !lane_ids.insert(lane.lane_id.as_ref()) {
                return Err(invalid("lanes[].lane_id", "must be unique"));
            }
            if lane.source_ids.is_empty() {
                return Err(invalid(
                    "lanes[].source_ids",
                    "must contain at least one source",
                ));
            }
            let mut lane_sources = BTreeSet::new();
            for source_id in &lane.source_ids {
                if !source_ids.contains(source_id.as_ref()) {
                    return Err(invalid(
                        "lanes[].source_ids",
                        "must reference a declared source",
                    ));
                }
                if !lane_sources.insert(source_id.as_ref()) {
                    return Err(invalid("lanes[].source_ids", "must be unique"));
                }
            }
            lane.format.validate()?;
        }
        Ok(())
    }
}

/// Confirms that a create command resolved to a durable session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCreatedV1 {
    pub create_message_id: MessageId,
    pub created_at_unix_ms: UnixMillis,
}

/// Requests replay of server events after a reconnect.
///
/// `None` requests the complete retained event stream. The runtime replays
/// original envelopes (including their message IDs and sequences), then emits
/// `ReplayCompleted` with the replay watermark. A runtime that cannot satisfy
/// the cursor must reject this command rather than silently skip events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSessionV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_server_sequence: Option<u64>,
}

impl ResumeSessionV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        if let Some(sequence) = self.after_server_sequence {
            validate_json_integer("after_server_sequence", sequence)?;
        }
        Ok(())
    }
}

/// Marks the durable watermark covered by one replay request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCompletedV1 {
    pub resume_message_id: MessageId,
    /// Highest server sequence included in the replay snapshot, or `None` if
    /// the session had no earlier events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed_through_server_sequence: Option<u64>,
}

impl ReplayCompletedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("resume_message_id", self.resume_message_id.as_ref())?;
        if let Some(sequence) = self.replayed_through_server_sequence {
            validate_json_integer("replayed_through_server_sequence", sequence)?;
        }
        Ok(())
    }
}

/// A stable, machine-readable rejection of a client command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRejectedV1 {
    pub rejected_message_id: MessageId,
    /// Open string code so V1 clients can safely display/record new codes.
    /// Initial codes are `invalid_message`, `conflict`, `unknown_session`,
    /// `invalid_transition`, `sequence_conflict`, `replay_unavailable`, and
    /// `internal`.
    pub code: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

impl CommandRejectedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("rejected_message_id", self.rejected_message_id.as_ref())?;
        validate_id("code", &self.code)?;
        if self.details.keys().any(|key| key.trim().is_empty()) {
            return Err(invalid("details", "keys must not be empty"));
        }
        Ok(())
    }
}

/// A logical source from which one or more capture lanes are derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSourceV1 {
    pub source_id: SourceId,
    pub kind: CaptureSourceKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Opaque source identity; never interpreted as a platform device handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceKindV1 {
    Microphone,
    SystemAudio,
    RemoteParticipant,
    Media,
    Mixed,
}

/// An independently ordered stream of encoded audio chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureLaneV1 {
    pub lane_id: LaneId,
    pub source_ids: Vec<SourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub format: AudioFormatV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormatV1 {
    pub codec: AudioCodecV1,
    pub container: AudioContainerV1,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
}

impl AudioFormatV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        if self.sample_rate_hz == 0 {
            return Err(invalid("format.sample_rate_hz", "must be nonzero"));
        }
        if self.channel_count == 0 {
            return Err(invalid("format.channel_count", "must be nonzero"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodecV1 {
    PcmS16Le,
    PcmF32Le,
    Opus,
    AacLc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioContainerV1 {
    Raw,
    Webm,
    Ogg,
    Mp4,
}

/// Dependency-free representation of a content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDigestV1 {
    pub algorithm: DigestAlgorithmV1,
    /// Lowercase hexadecimal digest bytes.
    pub hex: String,
}

impl ContentDigestV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        let expected_length = match self.algorithm {
            DigestAlgorithmV1::Sha256 | DigestAlgorithmV1::Blake3 => 64,
        };
        if self.hex.len() != expected_length
            || !self
                .hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(
                "digest.hex",
                "must be a lowercase hexadecimal digest of the declared algorithm",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithmV1 {
    Sha256,
    Blake3,
}

/// One audio payload in the lane-local sequence namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioChunkV1 {
    pub segment_id: SegmentId,
    pub lane_id: LaneId,
    /// Zero-based and strictly increasing within `(segment_id, lane_id)`.
    pub sequence: u64,
    pub starts_at_ms: SessionMillis,
    pub duration_ms: DurationMillis,
    pub payload_digest: ContentDigestV1,
    /// Encoded according to the lane format declared at session creation.
    /// Human-readable serializers use standard padded base64; binary
    /// serializers use their native byte-string representation.
    #[serde(with = "audio_payload")]
    pub payload: Vec<u8>,
}

impl AudioChunkV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("lane_id", self.lane_id.as_ref())?;
        validate_json_integer("sequence", self.sequence)?;
        validate_json_integer("starts_at_ms", self.starts_at_ms.0)?;
        validate_json_integer("duration_ms", self.duration_ms.0)?;
        if self.duration_ms.0 == 0 {
            return Err(invalid("duration_ms", "must be nonzero"));
        }
        if self.payload.is_empty() {
            return Err(invalid("payload", "must not be empty"));
        }
        self.payload_digest.validate()
    }
}

mod audio_payload {
    use super::*;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    struct AudioPayloadVisitor;

    impl<'de> Visitor<'de> for AudioPayloadVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("standard padded base64 or a binary byte string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            BASE64_STANDARD.decode(value).map_err(E::custom)
        }

        fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value.to_vec())
        }

        fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value)
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(AudioPayloadVisitor)
        } else {
            deserializer.deserialize_byte_buf(AudioPayloadVisitor)
        }
    }
}

/// A half-open lane sequence range: `[start, end_exclusive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceRangeV1 {
    pub start: u64,
    pub end_exclusive: u64,
}

impl SequenceRangeV1 {
    pub fn is_empty(self) -> bool {
        self.start == self.end_exclusive
    }

    pub fn is_valid(self) -> bool {
        self.start <= self.end_exclusive
    }

    pub fn contains(self, sequence: u64) -> bool {
        self.start <= sequence && sequence < self.end_exclusive
    }

    pub fn validate(self) -> Result<(), ValidationErrorV1> {
        validate_json_integer("sequence_range.start", self.start)?;
        validate_json_integer("sequence_range.end_exclusive", self.end_exclusive)?;
        if !self.is_valid() {
            return Err(invalid(
                "sequence_range",
                "start must not exceed end_exclusive",
            ));
        }
        Ok(())
    }
}

/// Cumulative durable receipt state for one ordered lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioAcknowledgementV1 {
    pub segment_id: SegmentId,
    pub lane_id: LaneId,
    /// Every sequence below this exclusive boundary is durably covered by an
    /// audio chunk or discontinuity. Zero means sequence zero is still needed.
    pub durable_through_sequence: u64,
    /// Durable ranges at or above `durable_through_sequence`; sorted,
    /// non-adjacent, disjoint, and half-open.
    pub durable_out_of_order: Vec<SequenceRangeV1>,
}

impl AudioAcknowledgementV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("lane_id", self.lane_id.as_ref())?;
        validate_json_integer("durable_through_sequence", self.durable_through_sequence)?;
        let mut previous_end = self.durable_through_sequence;
        for range in &self.durable_out_of_order {
            range.validate()?;
            if range.is_empty() {
                return Err(invalid(
                    "durable_out_of_order",
                    "must not contain empty ranges",
                ));
            }
            if range.start <= previous_end {
                return Err(invalid(
                    "durable_out_of_order",
                    "must be sorted, non-adjacent, disjoint, and above the cumulative boundary",
                ));
            }
            previous_end = range.end_exclusive;
        }
        Ok(())
    }

    /// Returns whether a sequence is known durable by this acknowledgement.
    pub fn covers(&self, sequence: u64) -> bool {
        sequence < self.durable_through_sequence
            || self
                .durable_out_of_order
                .iter()
                .any(|range| range.contains(sequence))
    }
}

/// Declares audio that cannot be retransmitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDiscontinuityV1 {
    pub discontinuity_id: DiscontinuityId,
    pub segment_id: SegmentId,
    pub lane_id: LaneId,
    /// Chunk sequences that cannot be retransmitted. An empty range at the
    /// next sequence represents capture time for which no chunk was assigned.
    pub sequence_range: SequenceRangeV1,
    pub starts_at_ms: SessionMillis,
    pub duration_ms: DurationMillis,
    pub reason: DiscontinuityReasonV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CaptureDiscontinuityV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("discontinuity_id", self.discontinuity_id.as_ref())?;
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("lane_id", self.lane_id.as_ref())?;
        self.sequence_range.validate()?;
        validate_json_integer("starts_at_ms", self.starts_at_ms.0)?;
        validate_json_integer("duration_ms", self.duration_ms.0)?;
        if self.duration_ms.0 == 0 {
            return Err(invalid("duration_ms", "must be nonzero"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscontinuityReasonV1 {
    QueueOverflow,
    DeviceUnavailable,
    PermissionRevoked,
    ClockReset,
    SuspendResume,
    NetworkLoss,
    SourceEnded,
    Unknown,
}

/// A capture-health observation, suitable for live telemetry and final audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureHealthV1 {
    pub observed_at_ms: SessionMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_id: Option<LaneId>,
    pub state: CaptureHealthStateV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_audio_ms: Option<DurationMillis>,
    pub dropped_chunk_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_skew_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CaptureHealthV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_json_integer("observed_at_ms", self.observed_at_ms.0)?;
        validate_json_integer("dropped_chunk_count", self.dropped_chunk_count)?;
        if let Some(lane_id) = &self.lane_id {
            validate_id("lane_id", lane_id.as_ref())?;
        }
        if let Some(queued) = self.queued_audio_ms {
            validate_json_integer("queued_audio_ms", queued.0)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureHealthStateV1 {
    Healthy,
    Degraded,
    Stalled,
    Recovered,
    Ended,
}

/// Ordered capture lineage, from the original producer through any relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureProvenanceV1 {
    pub hops: Vec<CaptureProvenanceHopV1>,
}

impl CaptureProvenanceV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        if self.hops.is_empty() {
            return Err(invalid("provenance.hops", "must not be empty"));
        }
        for hop in &self.hops {
            hop.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureProvenanceHopV1 {
    pub producer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_version: Option<String>,
    pub mode: CaptureModeV1,
    pub observed_at_unix_ms: UnixMillis,
    /// Small, non-secret, implementation-defined provenance labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

impl CaptureProvenanceHopV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("provenance.hops[].producer", &self.producer)?;
        validate_json_integer(
            "provenance.hops[].observed_at_unix_ms",
            self.observed_at_unix_ms.0,
        )?;
        if self.attributes.iter().any(|(key, _)| key.trim().is_empty()) {
            return Err(invalid(
                "provenance.hops[].attributes",
                "keys must not be empty",
            ));
        }
        Ok(())
    }
}

/// Appends relay lineage without rewriting an idempotent original command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendProvenanceHopV1 {
    /// Stable across retries by this relay for this session.
    pub provenance_hop_id: String,
    pub hop: CaptureProvenanceHopV1,
}

impl AppendProvenanceHopV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("provenance_hop_id", &self.provenance_hop_id)?;
        self.hop.validate()
    }
}

/// Confirms that a relay provenance hop was durably recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceHopRecordedV1 {
    pub append_message_id: MessageId,
    pub provenance_hop_id: String,
}

impl ProvenanceHopRecordedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("append_message_id", self.append_message_id.as_ref())?;
        validate_id("provenance_hop_id", &self.provenance_hop_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureModeV1 {
    Live,
    Imported,
    Relayed,
}

/// Exclusive next sequence expected for a lane when a segment closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneBoundaryV1 {
    pub lane_id: LaneId,
    pub next_sequence: u64,
}

/// Announces an immutable end boundary for a segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseSegmentV1 {
    pub segment_id: SegmentId,
    pub ended_at_ms: SessionMillis,
    pub lane_boundaries: Vec<LaneBoundaryV1>,
    pub reason: SegmentCloseReasonV1,
}

impl CloseSegmentV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_json_integer("ended_at_ms", self.ended_at_ms.0)?;
        if self.lane_boundaries.is_empty() {
            return Err(invalid(
                "lane_boundaries",
                "must contain every lane declared for the segment",
            ));
        }
        validate_lane_boundaries(&self.lane_boundaries)
    }
}

fn validate_lane_boundaries(boundaries: &[LaneBoundaryV1]) -> Result<(), ValidationErrorV1> {
    let mut lane_ids = BTreeSet::new();
    for boundary in boundaries {
        validate_id("lane_boundaries[].lane_id", boundary.lane_id.as_ref())?;
        validate_json_integer("lane_boundaries[].next_sequence", boundary.next_sequence)?;
        if !lane_ids.insert(boundary.lane_id.as_ref()) {
            return Err(invalid("lane_boundaries[].lane_id", "must be unique"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentCloseReasonV1 {
    Pause,
    Rollover,
    SourceEnded,
    Stop,
    Error,
}

/// Confirms all chunks or discontinuities below the close boundary are durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentFinalizedV1 {
    pub segment_id: SegmentId,
    pub close_message_id: MessageId,
    pub finalized_at_unix_ms: UnixMillis,
    pub duration_ms: DurationMillis,
    pub lane_boundaries: Vec<LaneBoundaryV1>,
}

impl SegmentFinalizedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("close_message_id", self.close_message_id.as_ref())?;
        validate_json_integer("finalized_at_unix_ms", self.finalized_at_unix_ms.0)?;
        validate_json_integer("duration_ms", self.duration_ms.0)?;
        validate_lane_boundaries(&self.lane_boundaries)
    }
}

/// Identifies the exact close operation that must finalize a segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentCloseReferenceV1 {
    pub segment_id: SegmentId,
    pub close_message_id: MessageId,
}

impl SegmentCloseReferenceV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_closes[].segment_id", self.segment_id.as_ref())?;
        validate_id(
            "segment_closes[].close_message_id",
            self.close_message_id.as_ref(),
        )
    }
}

/// Requests that no unlisted segments be accepted for the session.
///
/// The runtime atomically freezes the segment set, waits for every referenced
/// close operation (even if a close arrives after this command), and only then
/// emits `SessionFinalized`. A listed close remains admissible after this
/// command; any unlisted segment or close is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizeSessionV1 {
    pub ended_at_ms: SessionMillis,
    pub segment_closes: Vec<SegmentCloseReferenceV1>,
    pub reason: SessionFinalizeReasonV1,
}

impl FinalizeSessionV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_json_integer("ended_at_ms", self.ended_at_ms.0)?;
        let mut segment_ids = BTreeSet::new();
        let mut close_ids = BTreeSet::new();
        for reference in &self.segment_closes {
            reference.validate()?;
            if !segment_ids.insert(reference.segment_id.as_ref()) {
                return Err(invalid("segment_closes[].segment_id", "must be unique"));
            }
            if !close_ids.insert(reference.close_message_id.as_ref()) {
                return Err(invalid(
                    "segment_closes[].close_message_id",
                    "must be unique",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFinalizeReasonV1 {
    Completed,
    Cancelled,
    SourceEnded,
    Error,
}

/// Confirms the session boundary and exact finalized segment set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFinalizedV1 {
    pub finalize_message_id: MessageId,
    pub finalized_at_unix_ms: UnixMillis,
    pub duration_ms: DurationMillis,
    pub segment_closes: Vec<SegmentCloseReferenceV1>,
}

impl SessionFinalizedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("finalize_message_id", self.finalize_message_id.as_ref())?;
        validate_json_integer("finalized_at_unix_ms", self.finalized_at_unix_ms.0)?;
        validate_json_integer("duration_ms", self.duration_ms.0)?;
        let command = FinalizeSessionV1 {
            ended_at_ms: SessionMillis(self.duration_ms.0),
            segment_closes: self.segment_closes.clone(),
            reason: SessionFinalizeReasonV1::Completed,
        };
        command.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSpanV1 {
    pub span_id: TranscriptSpanId,
    pub starts_at_ms: SessionMillis,
    pub ends_at_ms: SessionMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
    /// Integer confidence in the inclusive range 0..=1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_per_mille: Option<u16>,
}

impl TranscriptSpanV1 {
    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("spans[].span_id", self.span_id.as_ref())?;
        validate_json_integer("spans[].starts_at_ms", self.starts_at_ms.0)?;
        validate_json_integer("spans[].ends_at_ms", self.ends_at_ms.0)?;
        if self.starts_at_ms > self.ends_at_ms {
            return Err(invalid(
                "spans[]",
                "starts_at_ms must not exceed ends_at_ms",
            ));
        }
        if self.text.trim().is_empty() {
            return Err(invalid("spans[].text", "must not be empty"));
        }
        if self.confidence_per_mille.is_some_and(|value| value > 1_000) {
            return Err(invalid(
                "spans[].confidence_per_mille",
                "must be in the inclusive range 0..=1000",
            ));
        }
        Ok(())
    }
}

/// Replaceable transcript hypothesis. Revisions are monotonic per hypothesis ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPartialV1 {
    pub segment_id: SegmentId,
    pub hypothesis_id: HypothesisId,
    pub revision: u64,
    pub spans: Vec<TranscriptSpanV1>,
}

impl TranscriptPartialV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("hypothesis_id", self.hypothesis_id.as_ref())?;
        validate_json_integer("revision", self.revision)?;
        for span in &self.spans {
            span.validate()?;
        }
        Ok(())
    }
}

/// Immutable, ordered transcript material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCommittedV1 {
    pub segment_id: SegmentId,
    pub commit_id: CommitId,
    /// Zero-based and strictly increasing within the session.
    pub commit_sequence: u64,
    pub committed_through_ms: SessionMillis,
    pub spans: Vec<TranscriptSpanV1>,
}

impl TranscriptCommittedV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("segment_id", self.segment_id.as_ref())?;
        validate_id("commit_id", self.commit_id.as_ref())?;
        validate_json_integer("commit_sequence", self.commit_sequence)?;
        validate_json_integer("committed_through_ms", self.committed_through_ms.0)?;
        for span in &self.spans {
            span.validate()?;
            if span.ends_at_ms > self.committed_through_ms {
                return Err(invalid(
                    "spans[].ends_at_ms",
                    "must not exceed committed_through_ms",
                ));
            }
        }
        Ok(())
    }
}

/// A replaceable generated memo. A final memo may still gain a higher revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoEventV1 {
    pub memo_id: MemoId,
    pub revision: u64,
    pub status: MemoStatusV1,
    pub markdown: String,
    pub based_on_commit_sequence: u64,
    pub generated_at_unix_ms: UnixMillis,
}

impl MemoEventV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("memo_id", self.memo_id.as_ref())?;
        validate_json_integer("revision", self.revision)?;
        validate_json_integer("based_on_commit_sequence", self.based_on_commit_sequence)?;
        validate_json_integer("generated_at_unix_ms", self.generated_at_unix_ms.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoStatusV1 {
    Partial,
    Final,
}

/// Announces a durable artifact without prescribing how its locator is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReadyV1 {
    pub artifact_id: ArtifactId,
    pub revision: u64,
    pub kind: ArtifactKindV1,
    pub media_type: String,
    pub byte_length: u64,
    pub digest: ContentDigestV1,
    /// Opaque to this protocol. A host may use a URL, object key, or local token.
    pub locator: String,
    pub generated_at_unix_ms: UnixMillis,
}

impl ArtifactReadyV1 {
    fn validate(&self) -> Result<(), ValidationErrorV1> {
        validate_id("artifact_id", self.artifact_id.as_ref())?;
        validate_json_integer("revision", self.revision)?;
        validate_json_integer("byte_length", self.byte_length)?;
        validate_json_integer("generated_at_unix_ms", self.generated_at_unix_ms.0)?;
        validate_id("media_type", &self.media_type)?;
        validate_id("locator", &self.locator)?;
        self.digest.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKindV1 {
    Transcript,
    Memo,
    Audio,
    Captions,
    Other,
}
