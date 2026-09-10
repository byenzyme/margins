//! Durable, transport-neutral state machine for `margins-meeting-protocol`.
//!
//! This crate stores encoded audio payloads as opaque bytes. It deliberately
//! has no HTTP, socket, async-runtime, audio-device, decoder, or OS bindings.

#![forbid(unsafe_code)]

use margins_meeting_protocol::{
    AppendProvenanceHopV1, AudioAcknowledgementV1, AudioChunkV1, CaptureDiscontinuityV1,
    CaptureHealthV1, CaptureProvenanceHopV1, ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1,
    CommandRejectedV1, CreateSessionV1, DigestAlgorithmV1, DurationMillis, FinalizeSessionV1,
    LaneId, MessageId, ProtocolVersionV1, ProvenanceHopRecordedV1, ReplayCompletedV1,
    SegmentFinalizedV1, SequenceRangeV1, ServerMessageBodyV1, ServerMessageV1, SessionCreatedV1,
    SessionFinalizedV1, SessionId, SessionMillis, UnixMillis, ValidationErrorV1,
    MAX_SAFE_JSON_INTEGER,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::Mutex,
};

const MAX_COMMIT_ATTEMPTS: usize = 64;

/// Result of an atomic storage write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCommit {
    Committed,
    Conflict,
}

/// Persistence seam used by [`MeetingRuntime`].
///
/// Implementations must make `create_session` and `replace_session` atomic.
/// A create must enforce uniqueness of both session ID and create idempotency
/// key. A replace commits only when the stored revision equals
/// `expected_revision`. Returning `Conflict` asks the runtime to reload and
/// deterministically retry the transition.
pub trait MeetingRuntimeStorage: Send + Sync {
    type Error;

    fn load_session(&self, session_id: &SessionId) -> Result<Option<StoredSessionV1>, Self::Error>;

    fn load_session_by_create_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<StoredSessionV1>, Self::Error>;

    fn create_session(&self, session: StoredSessionV1) -> Result<StorageCommit, Self::Error>;

    fn replace_session(
        &self,
        expected_revision: u64,
        session: StoredSessionV1,
    ) -> Result<StorageCommit, Self::Error>;
}

/// Error from the in-memory storage implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InMemoryStorageError;

impl fmt::Display for InMemoryStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("in-memory meeting runtime storage lock was poisoned")
    }
}

impl Error for InMemoryStorageError {}

#[derive(Debug, Default)]
struct InMemoryState {
    sessions: BTreeMap<SessionId, StoredSessionV1>,
    create_keys: BTreeMap<String, SessionId>,
}

/// Thread-safe, optimistic-concurrency in-memory persistence for tests and
/// single-process deployments.
#[derive(Debug, Default)]
pub struct InMemoryMeetingRuntimeStorage {
    inner: Mutex<InMemoryState>,
}

impl InMemoryMeetingRuntimeStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a durable snapshot without exposing the storage lock.
    pub fn snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<StoredSessionV1>, InMemoryStorageError> {
        self.load_session(session_id)
    }
}

impl MeetingRuntimeStorage for InMemoryMeetingRuntimeStorage {
    type Error = InMemoryStorageError;

    fn load_session(&self, session_id: &SessionId) -> Result<Option<StoredSessionV1>, Self::Error> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        Ok(state.sessions.get(session_id).cloned())
    }

    fn load_session_by_create_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<StoredSessionV1>, Self::Error> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        Ok(state
            .create_keys
            .get(idempotency_key)
            .and_then(|session_id| state.sessions.get(session_id))
            .cloned())
    }

    fn create_session(&self, session: StoredSessionV1) -> Result<StorageCommit, Self::Error> {
        let mut state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        if state.sessions.contains_key(&session.session_id)
            || state
                .create_keys
                .contains_key(&session.create.idempotency_key)
        {
            return Ok(StorageCommit::Conflict);
        }
        state.create_keys.insert(
            session.create.idempotency_key.clone(),
            session.session_id.clone(),
        );
        state.sessions.insert(session.session_id.clone(), session);
        Ok(StorageCommit::Committed)
    }

    fn replace_session(
        &self,
        expected_revision: u64,
        session: StoredSessionV1,
    ) -> Result<StorageCommit, Self::Error> {
        let mut state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        let Some(current) = state.sessions.get(&session.session_id) else {
            return Ok(StorageCommit::Conflict);
        };
        if current.revision != expected_revision
            || current.create.idempotency_key != session.create.idempotency_key
        {
            return Ok(StorageCommit::Conflict);
        }
        state.sessions.insert(session.session_id.clone(), session);
        Ok(StorageCommit::Committed)
    }
}

/// Runtime-level failure. Stateful command rejections for an existing session
/// are instead returned as durable `command_rejected` server events.
#[derive(Debug)]
pub enum RuntimeError<E> {
    InvalidMessage(ValidationErrorV1),
    UnknownSession(SessionId),
    CreateIdempotencyConflict {
        idempotency_key: String,
        existing_session_id: SessionId,
    },
    Storage(E),
    Contention,
}

impl<E: fmt::Display> fmt::Display for RuntimeError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessage(error) => write!(formatter, "invalid client message: {error}"),
            Self::UnknownSession(session_id) => {
                write!(formatter, "unknown session {}", session_id.as_ref())
            }
            Self::CreateIdempotencyConflict {
                idempotency_key,
                existing_session_id,
            } => write!(
                formatter,
                "create idempotency key {idempotency_key:?} already belongs to session {}",
                existing_session_id.as_ref()
            ),
            Self::Storage(error) => write!(formatter, "meeting runtime storage error: {error}"),
            Self::Contention => formatter.write_str("meeting runtime storage remained contended"),
        }
    }
}

impl<E: Error + 'static> Error for RuntimeError<E> {}

/// Output for one accepted command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResponseV1 {
    /// Original envelopes to deliver. Exact command retries return the same
    /// envelopes with the same message IDs, sequences, and timestamps.
    pub messages: Vec<ServerMessageV1>,
    /// True only when the command message ID and full content were seen before.
    pub idempotent_replay: bool,
}

/// Stored session snapshot passed through the persistence trait.
///
/// Fields stay private so adapters cannot accidentally construct invalid
/// state. The type is serializable for database/blob adapters and exposes
/// read-only inspection methods for durable worker/outbox integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSessionV1 {
    revision: u64,
    session_id: SessionId,
    create_message: ClientMessageV1,
    create: CreateSessionV1,
    provenance: Vec<CaptureProvenanceHopV1>,
    commands: BTreeMap<MessageId, CommandRecord>,
    conflicting_commands: Vec<CommandRecord>,
    provenance_hops: BTreeMap<String, AppendProvenanceHopV1>,
    discontinuities: BTreeMap<String, CaptureDiscontinuityV1>,
    segments: BTreeMap<String, SegmentState>,
    last_health_at_ms: Option<SessionMillis>,
    finalize: Option<FinalizeRecord>,
    events: Vec<ServerMessageV1>,
}

impl StoredSessionV1 {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn create(&self) -> &CreateSessionV1 {
        &self.create
    }

    pub fn provenance(&self) -> &[CaptureProvenanceHopV1] {
        &self.provenance
    }

    pub fn events(&self) -> &[ServerMessageV1] {
        &self.events
    }

    /// Opaque encoded chunks, ordered by segment ID, lane ID, then sequence.
    pub fn audio_chunks(&self) -> impl Iterator<Item = &AudioChunkV1> {
        self.segments
            .values()
            .flat_map(|segment| segment.lanes.values())
            .flat_map(|lane| lane.chunks.values())
    }

    pub fn discontinuities(&self) -> impl Iterator<Item = &CaptureDiscontinuityV1> {
        self.discontinuities.values()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CommandRecord {
    message: ClientMessageV1,
    response_range: SequenceRangeV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SegmentState {
    lanes: BTreeMap<LaneId, LaneState>,
    close: Option<CloseRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LaneState {
    chunks: BTreeMap<u64, AudioChunkV1>,
    coverage: RangeSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CloseRecord {
    message_id: MessageId,
    command: CloseSegmentV1,
    finalized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FinalizeRecord {
    message_id: MessageId,
    command: FinalizeSessionV1,
    finalized: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RangeSet {
    ranges: Vec<SequenceRangeV1>,
}

impl RangeSet {
    fn add(&mut self, mut added: SequenceRangeV1) {
        if added.start == added.end_exclusive {
            return;
        }
        let mut merged = Vec::with_capacity(self.ranges.len() + 1);
        let mut inserted = false;
        for range in self.ranges.drain(..) {
            if range.end_exclusive < added.start {
                merged.push(range);
            } else if added.end_exclusive < range.start {
                if !inserted {
                    merged.push(added);
                    inserted = true;
                }
                merged.push(range);
            } else {
                added.start = added.start.min(range.start);
                added.end_exclusive = added.end_exclusive.max(range.end_exclusive);
            }
        }
        if !inserted {
            merged.push(added);
        }
        self.ranges = merged;
    }

    fn acknowledgement(&self, segment_id: &str, lane_id: &LaneId) -> AudioAcknowledgementV1 {
        let durable_through_sequence = self
            .ranges
            .first()
            .filter(|range| range.start == 0)
            .map_or(0, |range| range.end_exclusive);
        let durable_out_of_order = self
            .ranges
            .iter()
            .filter(|range| range.start > durable_through_sequence)
            .copied()
            .collect();
        AudioAcknowledgementV1 {
            segment_id: segment_id.into(),
            lane_id: lane_id.clone(),
            durable_through_sequence,
            durable_out_of_order,
        }
    }

    fn covers_below(&self, boundary: u64) -> bool {
        boundary == 0
            || self
                .ranges
                .first()
                .is_some_and(|range| range.start == 0 && range.end_exclusive >= boundary)
    }
}

/// Synchronous state machine. Transport code validates/framing/authenticates
/// around this type; the runtime itself re-validates local V1 invariants before
/// touching durable state.
#[derive(Debug)]
pub struct MeetingRuntime<S> {
    storage: S,
}

impl<S> MeetingRuntime<S>
where
    S: MeetingRuntimeStorage,
{
    pub fn new(storage: S) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    pub fn into_storage(self) -> S {
        self.storage
    }

    pub fn handle(
        &self,
        message: ClientMessageV1,
    ) -> Result<RuntimeResponseV1, RuntimeError<S::Error>> {
        message.validate().map_err(RuntimeError::InvalidMessage)?;
        if matches!(message.body, ClientMessageBodyV1::CreateSession(_)) {
            self.handle_create(message)
        } else {
            self.handle_existing(message)
        }
    }

    fn handle_create(
        &self,
        message: ClientMessageV1,
    ) -> Result<RuntimeResponseV1, RuntimeError<S::Error>> {
        let ClientMessageBodyV1::CreateSession(create) = message.body.clone() else {
            unreachable!();
        };
        for _ in 0..MAX_COMMIT_ATTEMPTS {
            if self
                .storage
                .load_session(&message.session_id)
                .map_err(RuntimeError::Storage)?
                .is_some()
            {
                return self.handle_existing(message);
            }
            if let Some(existing) = self
                .storage
                .load_session_by_create_idempotency_key(&create.idempotency_key)
                .map_err(RuntimeError::Storage)?
            {
                if existing.session_id != message.session_id {
                    return Err(RuntimeError::CreateIdempotencyConflict {
                        idempotency_key: create.idempotency_key,
                        existing_session_id: existing.session_id,
                    });
                }
                continue;
            }

            let mut session = StoredSessionV1 {
                revision: 0,
                session_id: message.session_id.clone(),
                create_message: message.clone(),
                create: create.clone(),
                provenance: create.provenance.hops.clone(),
                commands: BTreeMap::new(),
                conflicting_commands: Vec::new(),
                provenance_hops: BTreeMap::new(),
                discontinuities: BTreeMap::new(),
                segments: BTreeMap::new(),
                last_health_at_ms: None,
                finalize: None,
                events: Vec::new(),
            };
            let created = append_event(
                &mut session,
                message.sent_at_unix_ms,
                ServerMessageBodyV1::SessionCreated(SessionCreatedV1 {
                    create_message_id: message.message_id.clone(),
                    created_at_unix_ms: message.sent_at_unix_ms,
                }),
            );
            let response = vec![created];
            record_command(&mut session, message.clone(), response.clone());
            match self
                .storage
                .create_session(session)
                .map_err(RuntimeError::Storage)?
            {
                StorageCommit::Committed => {
                    return Ok(RuntimeResponseV1 {
                        messages: response,
                        idempotent_replay: false,
                    });
                }
                StorageCommit::Conflict => continue,
            }
        }
        Err(RuntimeError::Contention)
    }

    fn handle_existing(
        &self,
        message: ClientMessageV1,
    ) -> Result<RuntimeResponseV1, RuntimeError<S::Error>> {
        for _ in 0..MAX_COMMIT_ATTEMPTS {
            let session = self
                .storage
                .load_session(&message.session_id)
                .map_err(RuntimeError::Storage)?
                .ok_or_else(|| RuntimeError::UnknownSession(message.session_id.clone()))?;
            match self.try_apply_existing(session, message.clone())? {
                ApplyResult::Done(response) => return Ok(response),
                ApplyResult::Retry => continue,
            }
        }
        Err(RuntimeError::Contention)
    }

    fn try_apply_existing(
        &self,
        mut session: StoredSessionV1,
        message: ClientMessageV1,
    ) -> Result<ApplyResult, RuntimeError<S::Error>> {
        if let Some(record) = session.commands.get(&message.message_id) {
            if record.message == message {
                return Ok(ApplyResult::Done(RuntimeResponseV1 {
                    messages: command_response(&session, record),
                    idempotent_replay: true,
                }));
            }
            if let Some(record) = session
                .conflicting_commands
                .iter()
                .find(|record| record.message == message)
            {
                return Ok(ApplyResult::Done(RuntimeResponseV1 {
                    messages: command_response(&session, record),
                    idempotent_replay: true,
                }));
            }
            if let Some(record) = session
                .conflicting_commands
                .iter()
                .find(|record| record.message.message_id == message.message_id)
            {
                return Ok(ApplyResult::Done(RuntimeResponseV1 {
                    messages: command_response(&session, record),
                    idempotent_replay: false,
                }));
            }
            let response = reject(
                &mut session,
                &message,
                "conflict",
                "message_id was already used for different content",
            );
            let response_range = response_range(&response);
            session.conflicting_commands.push(CommandRecord {
                message,
                response_range,
            });
            return self.commit_existing(session, response, false);
        }

        let response = apply_command(&mut session, &message);
        record_command(&mut session, message, response.clone());
        self.commit_existing(session, response, false)
    }

    fn commit_existing(
        &self,
        mut session: StoredSessionV1,
        response: Vec<ServerMessageV1>,
        idempotent_replay: bool,
    ) -> Result<ApplyResult, RuntimeError<S::Error>> {
        let expected_revision = session.revision;
        session.revision = session
            .revision
            .checked_add(1)
            .ok_or(RuntimeError::Contention)?;
        match self
            .storage
            .replace_session(expected_revision, session)
            .map_err(RuntimeError::Storage)?
        {
            StorageCommit::Committed => Ok(ApplyResult::Done(RuntimeResponseV1 {
                messages: response,
                idempotent_replay,
            })),
            StorageCommit::Conflict => Ok(ApplyResult::Retry),
        }
    }
}

enum ApplyResult {
    Done(RuntimeResponseV1),
    Retry,
}

fn record_command(
    session: &mut StoredSessionV1,
    message: ClientMessageV1,
    response: Vec<ServerMessageV1>,
) {
    let response_range = response_range(&response);
    session.commands.insert(
        message.message_id.clone(),
        CommandRecord {
            message,
            response_range,
        },
    );
}

fn response_range(response: &[ServerMessageV1]) -> SequenceRangeV1 {
    let Some(first) = response.first() else {
        return SequenceRangeV1 {
            start: 0,
            end_exclusive: 0,
        };
    };
    for (offset, event) in response.iter().enumerate() {
        debug_assert_eq!(event.sequence, first.sequence + offset as u64);
    }
    SequenceRangeV1 {
        start: first.sequence,
        end_exclusive: first.sequence + response.len() as u64,
    }
}

fn command_response(session: &StoredSessionV1, record: &CommandRecord) -> Vec<ServerMessageV1> {
    if record.response_range.is_empty() {
        return Vec::new();
    }
    session.events
        [record.response_range.start as usize..record.response_range.end_exclusive as usize]
        .to_vec()
}

fn apply_command(session: &mut StoredSessionV1, message: &ClientMessageV1) -> Vec<ServerMessageV1> {
    match &message.body {
        ClientMessageBodyV1::CreateSession(_) => reject(
            session,
            message,
            "conflict",
            "session_id or create idempotency key was already used",
        ),
        ClientMessageBodyV1::ResumeSession(resume) => {
            let watermark = session.events.last().map(|event| event.sequence);
            if resume
                .after_server_sequence
                .is_some_and(|cursor| watermark.is_none_or(|last| cursor > last))
            {
                return reject(
                    session,
                    message,
                    "replay_unavailable",
                    "replay cursor is beyond the retained event log",
                );
            }
            let replayed: Vec<_> = session
                .events
                .iter()
                .filter(|event| {
                    resume
                        .after_server_sequence
                        .is_none_or(|cursor| event.sequence > cursor)
                })
                .cloned()
                .collect();
            let completed = append_event(
                session,
                message.sent_at_unix_ms,
                ServerMessageBodyV1::ReplayCompleted(ReplayCompletedV1 {
                    resume_message_id: message.message_id.clone(),
                    replayed_through_server_sequence: watermark,
                }),
            );
            let mut response = replayed;
            response.push(completed);
            response
        }
        ClientMessageBodyV1::AppendProvenanceHop(append) => {
            if let Some(existing) = session.provenance_hops.get(&append.provenance_hop_id) {
                if existing == append {
                    return Vec::new();
                }
                return reject(
                    session,
                    message,
                    "conflict",
                    "provenance_hop_id was already used for different content",
                );
            }
            if session
                .finalize
                .as_ref()
                .is_some_and(|value| value.finalized)
            {
                return reject(
                    session,
                    message,
                    "invalid_transition",
                    "session is finalized",
                );
            }
            session
                .provenance_hops
                .insert(append.provenance_hop_id.clone(), append.clone());
            session.provenance.push(append.hop.clone());
            vec![append_event(
                session,
                message.sent_at_unix_ms,
                ServerMessageBodyV1::ProvenanceHopRecorded(ProvenanceHopRecordedV1 {
                    append_message_id: message.message_id.clone(),
                    provenance_hop_id: append.provenance_hop_id.clone(),
                }),
            )]
        }
        ClientMessageBodyV1::AudioChunk(chunk) => apply_chunk(session, message, chunk),
        ClientMessageBodyV1::CaptureDiscontinuity(discontinuity) => {
            apply_discontinuity(session, message, discontinuity)
        }
        ClientMessageBodyV1::CaptureHealth(health) => apply_health(session, message, health),
        ClientMessageBodyV1::CloseSegment(close) => apply_close(session, message, close),
        ClientMessageBodyV1::FinalizeSession(finalize) => {
            apply_finalize(session, message, finalize)
        }
    }
}

fn apply_chunk(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    chunk: &AudioChunkV1,
) -> Vec<ServerMessageV1> {
    if !declares_lane(session, &chunk.lane_id) {
        return reject(
            session,
            message,
            "invalid_transition",
            "lane_id is not declared",
        );
    }
    if !segment_is_admissible(session, chunk.segment_id.as_ref()) {
        return reject(
            session,
            message,
            "invalid_transition",
            "segment is outside the finalized segment set",
        );
    }
    if !payload_digest_matches(chunk) {
        return reject(
            session,
            message,
            "digest_mismatch",
            "payload does not match payload_digest",
        );
    }
    let Some(chunk_end) = chunk.starts_at_ms.0.checked_add(chunk.duration_ms.0) else {
        return reject(
            session,
            message,
            "invalid_message",
            "chunk time range overflows V1",
        );
    };
    if chunk_end > MAX_SAFE_JSON_INTEGER {
        return reject(
            session,
            message,
            "invalid_message",
            "chunk time range overflows V1",
        );
    }
    let segment = session.segments.get(chunk.segment_id.as_ref());
    let lane = segment.and_then(|segment| segment.lanes.get(&chunk.lane_id));
    if let Some(existing) = lane.and_then(|lane| lane.chunks.get(&chunk.sequence)) {
        if existing == chunk {
            return Vec::new();
        }
        return reject(
            session,
            message,
            "sequence_conflict",
            "lane sequence was already stored with different content",
        );
    }
    if session.discontinuities.values().any(|gap| {
        gap.segment_id == chunk.segment_id
            && gap.lane_id == chunk.lane_id
            && gap.sequence_range.contains(chunk.sequence)
    }) {
        return reject(
            session,
            message,
            "sequence_conflict",
            "lane sequence is already declared discontinuous",
        );
    }
    if segment
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| {
            boundary_for(&close.command, &chunk.lane_id)
                .is_some_and(|value| chunk.sequence >= value)
        })
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "chunk sequence is at or beyond the close boundary",
        );
    }
    if segment
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| close.finalized)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "segment is finalized",
        );
    }
    if segment
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| chunk_end > close.command.ended_at_ms.0)
        || session
            .finalize
            .as_ref()
            .is_some_and(|finalize| chunk_end > finalize.command.ended_at_ms.0)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "chunk extends beyond a declared close or finalize boundary",
        );
    }
    if chunk.sequence == MAX_SAFE_JSON_INTEGER {
        return reject(
            session,
            message,
            "invalid_message",
            "chunk sequence cannot be acknowledged in V1",
        );
    }
    ensure_segment(session, chunk.segment_id.as_ref());
    let segment = session.segments.get_mut(chunk.segment_id.as_ref()).unwrap();
    let lane = segment.lanes.get_mut(&chunk.lane_id).unwrap();
    lane.chunks.insert(chunk.sequence, chunk.clone());
    lane.coverage.add(SequenceRangeV1 {
        start: chunk.sequence,
        end_exclusive: chunk.sequence + 1,
    });
    let ack = lane
        .coverage
        .acknowledgement(chunk.segment_id.as_ref(), &chunk.lane_id);
    let mut response = vec![append_event(
        session,
        message.sent_at_unix_ms,
        ServerMessageBodyV1::AudioAcknowledged(ack),
    )];
    finish_ready_operations(
        session,
        chunk.segment_id.as_ref(),
        message.sent_at_unix_ms,
        &mut response,
    );
    response
}

fn payload_digest_matches(chunk: &AudioChunkV1) -> bool {
    let actual = match chunk.payload_digest.algorithm {
        DigestAlgorithmV1::Sha256 => format!("{:x}", Sha256::digest(&chunk.payload)),
        DigestAlgorithmV1::Blake3 => blake3::hash(&chunk.payload).to_hex().to_string(),
    };
    actual == chunk.payload_digest.hex
}

fn apply_discontinuity(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    discontinuity: &CaptureDiscontinuityV1,
) -> Vec<ServerMessageV1> {
    if let Some(existing) = session
        .discontinuities
        .get(discontinuity.discontinuity_id.as_ref())
    {
        if existing == discontinuity {
            return Vec::new();
        }
        return reject(
            session,
            message,
            "conflict",
            "discontinuity_id was already used for different content",
        );
    }
    if !declares_lane(session, &discontinuity.lane_id) {
        return reject(
            session,
            message,
            "invalid_transition",
            "lane_id is not declared",
        );
    }
    if !segment_is_admissible(session, discontinuity.segment_id.as_ref()) {
        return reject(
            session,
            message,
            "invalid_transition",
            "segment is outside the finalized segment set",
        );
    }
    let Some(gap_end) = discontinuity
        .starts_at_ms
        .0
        .checked_add(discontinuity.duration_ms.0)
    else {
        return reject(
            session,
            message,
            "invalid_message",
            "discontinuity time range overflows V1",
        );
    };
    if gap_end > MAX_SAFE_JSON_INTEGER {
        return reject(
            session,
            message,
            "invalid_message",
            "discontinuity time range overflows V1",
        );
    }
    let segment = session.segments.get(discontinuity.segment_id.as_ref());
    if segment
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| close.finalized)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "segment is finalized",
        );
    }
    if let Some(close) = segment.and_then(|segment| segment.close.as_ref()) {
        let boundary = boundary_for(&close.command, &discontinuity.lane_id).unwrap();
        if discontinuity.sequence_range.end_exclusive > boundary
            || discontinuity.sequence_range.start > boundary
        {
            return reject(
                session,
                message,
                "invalid_transition",
                "discontinuity extends beyond the close boundary",
            );
        }
    }
    if segment
        .and_then(|segment| segment.close.as_ref())
        .is_some_and(|close| gap_end > close.command.ended_at_ms.0)
        || session
            .finalize
            .as_ref()
            .is_some_and(|finalize| gap_end > finalize.command.ended_at_ms.0)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "discontinuity extends beyond a declared close or finalize boundary",
        );
    }
    if !discontinuity.sequence_range.is_empty()
        && (segment
            .and_then(|segment| segment.lanes.get(&discontinuity.lane_id))
            .is_some_and(|lane| {
                lane.chunks
                    .keys()
                    .any(|sequence| discontinuity.sequence_range.contains(*sequence))
            })
            || session.discontinuities.values().any(|existing| {
                existing.segment_id == discontinuity.segment_id
                    && existing.lane_id == discontinuity.lane_id
                    && ranges_overlap(existing.sequence_range, discontinuity.sequence_range)
            }))
    {
        return reject(
            session,
            message,
            "sequence_conflict",
            "discontinuity overlaps durable audio or another discontinuity",
        );
    }
    ensure_segment(session, discontinuity.segment_id.as_ref());
    session.discontinuities.insert(
        discontinuity.discontinuity_id.0.clone(),
        discontinuity.clone(),
    );
    let segment = session
        .segments
        .get_mut(discontinuity.segment_id.as_ref())
        .unwrap();
    let lane = segment.lanes.get_mut(&discontinuity.lane_id).unwrap();
    lane.coverage.add(discontinuity.sequence_range);
    let ack = lane
        .coverage
        .acknowledgement(discontinuity.segment_id.as_ref(), &discontinuity.lane_id);
    let mut response = vec![append_event(
        session,
        message.sent_at_unix_ms,
        ServerMessageBodyV1::AudioAcknowledged(ack),
    )];
    finish_ready_operations(
        session,
        discontinuity.segment_id.as_ref(),
        message.sent_at_unix_ms,
        &mut response,
    );
    response
}

fn ranges_overlap(left: SequenceRangeV1, right: SequenceRangeV1) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && left.start < right.end_exclusive
        && right.start < left.end_exclusive
}

fn apply_health(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    health: &CaptureHealthV1,
) -> Vec<ServerMessageV1> {
    if health
        .lane_id
        .as_ref()
        .is_some_and(|lane_id| !declares_lane(session, lane_id))
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "lane_id is not declared",
        );
    }
    if session
        .finalize
        .as_ref()
        .is_some_and(|value| value.finalized)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "session is finalized",
        );
    }
    if session
        .last_health_at_ms
        .is_some_and(|last| health.observed_at_ms < last)
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "capture health time moved backwards",
        );
    }
    session.last_health_at_ms = Some(health.observed_at_ms);
    Vec::new()
}

fn apply_close(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    close: &CloseSegmentV1,
) -> Vec<ServerMessageV1> {
    if !segment_is_admissible(session, close.segment_id.as_ref()) {
        return reject(
            session,
            message,
            "invalid_transition",
            "close is not one of the exact operations declared by finalize_session",
        );
    }
    let declared: BTreeSet<_> = session
        .create
        .lanes
        .iter()
        .map(|lane| lane.lane_id.clone())
        .collect();
    let supplied: BTreeSet<_> = close
        .lane_boundaries
        .iter()
        .map(|boundary| boundary.lane_id.clone())
        .collect();
    if declared != supplied {
        return reject(
            session,
            message,
            "invalid_transition",
            "close must declare exactly every session lane",
        );
    }
    if let Some(finalize) = &session.finalize {
        let expected = finalize
            .command
            .segment_closes
            .iter()
            .find(|reference| reference.segment_id == close.segment_id);
        if expected.is_none_or(|reference| reference.close_message_id != message.message_id) {
            return reject(
                session,
                message,
                "invalid_transition",
                "close message ID does not match finalize_session",
            );
        }
        if close.ended_at_ms > finalize.command.ended_at_ms {
            return reject(
                session,
                message,
                "invalid_transition",
                "segment ended_at_ms exceeds the session finalize boundary",
            );
        }
    }
    let segment = session.segments.get(close.segment_id.as_ref());
    if segment.is_some_and(|segment| segment.close.is_some()) {
        return reject(
            session,
            message,
            "conflict",
            "segment already has a different close operation",
        );
    }
    for boundary in &close.lane_boundaries {
        if segment
            .and_then(|segment| segment.lanes.get(&boundary.lane_id))
            .is_some_and(|lane| {
                lane.chunks
                    .keys()
                    .any(|sequence| *sequence >= boundary.next_sequence)
            })
        {
            return reject(
                session,
                message,
                "invalid_transition",
                "close boundary excludes an already durable chunk",
            );
        }
        if session.discontinuities.values().any(|discontinuity| {
            discontinuity.segment_id == close.segment_id
                && discontinuity.lane_id == boundary.lane_id
                && (discontinuity.sequence_range.end_exclusive > boundary.next_sequence
                    || discontinuity.sequence_range.start > boundary.next_sequence)
        }) {
            return reject(
                session,
                message,
                "invalid_transition",
                "close boundary excludes an already durable discontinuity",
            );
        }
    }
    let latest_chunk_end = segment
        .into_iter()
        .flat_map(|segment| segment.lanes.values())
        .flat_map(|lane| lane.chunks.values())
        .filter_map(|chunk| chunk.starts_at_ms.0.checked_add(chunk.duration_ms.0))
        .max()
        .unwrap_or(0);
    let latest_discontinuity_end = session
        .discontinuities
        .values()
        .filter(|discontinuity| discontinuity.segment_id == close.segment_id)
        .filter_map(|discontinuity| {
            discontinuity
                .starts_at_ms
                .0
                .checked_add(discontinuity.duration_ms.0)
        })
        .max()
        .unwrap_or(0);
    let latest_end = latest_chunk_end.max(latest_discontinuity_end);
    if close.ended_at_ms.0 < latest_end {
        return reject(
            session,
            message,
            "invalid_transition",
            "segment ended_at_ms precedes durable audio",
        );
    }
    ensure_segment(session, close.segment_id.as_ref());
    session
        .segments
        .get_mut(close.segment_id.as_ref())
        .unwrap()
        .close = Some(CloseRecord {
        message_id: message.message_id.clone(),
        command: close.clone(),
        finalized: false,
    });
    let mut response = Vec::new();
    finish_ready_operations(
        session,
        close.segment_id.as_ref(),
        message.sent_at_unix_ms,
        &mut response,
    );
    response
}

fn apply_finalize(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    finalize: &FinalizeSessionV1,
) -> Vec<ServerMessageV1> {
    if session.finalize.is_some() {
        return reject(
            session,
            message,
            "conflict",
            "session already has a different finalize operation",
        );
    }
    let declared: BTreeMap<_, _> = finalize
        .segment_closes
        .iter()
        .map(|reference| {
            (
                reference.segment_id.as_ref(),
                reference.close_message_id.as_ref(),
            )
        })
        .collect();
    if session
        .segments
        .keys()
        .any(|segment_id| !declared.contains_key(segment_id.as_str()))
    {
        return reject(
            session,
            message,
            "invalid_transition",
            "finalize_session omitted an existing segment",
        );
    }
    for reference in &finalize.segment_closes {
        if reference.close_message_id == message.message_id {
            return reject(
                session,
                message,
                "invalid_transition",
                "finalize_session cannot reserve its own message ID for a close",
            );
        }
        if session.commands.contains_key(&reference.close_message_id) {
            let exact_close_exists = session
                .segments
                .get(reference.segment_id.as_ref())
                .and_then(|segment| segment.close.as_ref())
                .is_some_and(|close| close.message_id == reference.close_message_id);
            if !exact_close_exists {
                return reject(
                    session,
                    message,
                    "invalid_transition",
                    "a referenced close message ID was already used by another command",
                );
            }
        }
    }
    for (segment_id, segment) in &session.segments {
        if let Some(close) = &segment.close {
            if declared.get(segment_id.as_str()).copied() != Some(close.message_id.as_ref()) {
                return reject(
                    session,
                    message,
                    "invalid_transition",
                    "finalize_session does not name the exact close operation",
                );
            }
            if close.command.ended_at_ms > finalize.ended_at_ms {
                return reject(
                    session,
                    message,
                    "invalid_transition",
                    "session ended_at_ms precedes a segment close",
                );
            }
        }
    }
    let latest_chunk_end = session
        .segments
        .values()
        .flat_map(|segment| segment.lanes.values())
        .flat_map(|lane| lane.chunks.values())
        .filter_map(|chunk| chunk.starts_at_ms.0.checked_add(chunk.duration_ms.0))
        .max()
        .unwrap_or(0);
    let latest_discontinuity_end = session
        .discontinuities
        .values()
        .filter_map(|gap| gap.starts_at_ms.0.checked_add(gap.duration_ms.0))
        .max()
        .unwrap_or(0);
    if finalize.ended_at_ms.0 < latest_chunk_end.max(latest_discontinuity_end) {
        return reject(
            session,
            message,
            "invalid_transition",
            "session ended_at_ms precedes durable audio",
        );
    }
    session.finalize = Some(FinalizeRecord {
        message_id: message.message_id.clone(),
        command: finalize.clone(),
        finalized: false,
    });
    let mut response = Vec::new();
    maybe_finalize_session(session, message.sent_at_unix_ms, &mut response);
    response
}

fn ensure_segment(session: &mut StoredSessionV1, segment_id: &str) {
    if session.segments.contains_key(segment_id) {
        return;
    }
    let lanes = session
        .create
        .lanes
        .iter()
        .map(|lane| {
            (
                lane.lane_id.clone(),
                LaneState {
                    chunks: BTreeMap::new(),
                    coverage: RangeSet::default(),
                },
            )
        })
        .collect();
    session
        .segments
        .insert(segment_id.to_owned(), SegmentState { lanes, close: None });
}

fn declares_lane(session: &StoredSessionV1, lane_id: &LaneId) -> bool {
    session
        .create
        .lanes
        .iter()
        .any(|lane| lane.lane_id == *lane_id)
}

fn segment_is_admissible(session: &StoredSessionV1, segment_id: &str) -> bool {
    session.finalize.as_ref().is_none_or(|finalize| {
        !finalize.finalized
            && finalize
                .command
                .segment_closes
                .iter()
                .any(|reference| reference.segment_id.as_ref() == segment_id)
    })
}

fn boundary_for(close: &CloseSegmentV1, lane_id: &LaneId) -> Option<u64> {
    close
        .lane_boundaries
        .iter()
        .find(|boundary| boundary.lane_id == *lane_id)
        .map(|boundary| boundary.next_sequence)
}

fn finish_ready_operations(
    session: &mut StoredSessionV1,
    segment_id: &str,
    sent_at: UnixMillis,
    response: &mut Vec<ServerMessageV1>,
) {
    let ready = session.segments.get(segment_id).is_some_and(|segment| {
        segment.close.as_ref().is_some_and(|close| {
            !close.finalized
                && close.command.lane_boundaries.iter().all(|boundary| {
                    segment
                        .lanes
                        .get(&boundary.lane_id)
                        .is_some_and(|lane| lane.coverage.covers_below(boundary.next_sequence))
                })
        })
    });
    if ready {
        let (close_message_id, ended_at_ms, lane_boundaries) = {
            let segment = session.segments.get_mut(segment_id).unwrap();
            let close = segment.close.as_mut().unwrap();
            close.finalized = true;
            (
                close.message_id.clone(),
                close.command.ended_at_ms,
                close.command.lane_boundaries.clone(),
            )
        };
        response.push(append_event(
            session,
            sent_at,
            ServerMessageBodyV1::SegmentFinalized(SegmentFinalizedV1 {
                segment_id: segment_id.into(),
                close_message_id,
                finalized_at_unix_ms: sent_at,
                duration_ms: DurationMillis(ended_at_ms.0),
                lane_boundaries,
            }),
        ));
    }
    maybe_finalize_session(session, sent_at, response);
}

fn maybe_finalize_session(
    session: &mut StoredSessionV1,
    sent_at: UnixMillis,
    response: &mut Vec<ServerMessageV1>,
) {
    let ready = session.finalize.as_ref().is_some_and(|finalize| {
        !finalize.finalized
            && finalize.command.segment_closes.iter().all(|reference| {
                session
                    .segments
                    .get(reference.segment_id.as_ref())
                    .and_then(|segment| segment.close.as_ref())
                    .is_some_and(|close| {
                        close.finalized && close.message_id == reference.close_message_id
                    })
            })
    });
    if !ready {
        return;
    }
    let (message_id, command) = {
        let finalize = session.finalize.as_mut().unwrap();
        finalize.finalized = true;
        (finalize.message_id.clone(), finalize.command.clone())
    };
    response.push(append_event(
        session,
        sent_at,
        ServerMessageBodyV1::SessionFinalized(SessionFinalizedV1 {
            finalize_message_id: message_id,
            finalized_at_unix_ms: sent_at,
            duration_ms: DurationMillis(command.ended_at_ms.0),
            segment_closes: command.segment_closes,
        }),
    ));
}

fn reject(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    code: &str,
    explanation: &str,
) -> Vec<ServerMessageV1> {
    vec![append_event(
        session,
        message.sent_at_unix_ms,
        ServerMessageBodyV1::CommandRejected(CommandRejectedV1 {
            rejected_message_id: message.message_id.clone(),
            code: code.to_owned(),
            retryable: false,
            message: Some(explanation.to_owned()),
            details: BTreeMap::new(),
        }),
    )]
}

fn append_event(
    session: &mut StoredSessionV1,
    sent_at_unix_ms: UnixMillis,
    body: ServerMessageBodyV1,
) -> ServerMessageV1 {
    let sequence = session.events.len() as u64;
    assert!(
        sequence <= MAX_SAFE_JSON_INTEGER,
        "V1 server sequence exhausted"
    );
    let event = ServerMessageV1 {
        protocol_version: ProtocolVersionV1,
        message_id: format!(
            "margins-runtime-v1:{}:{sequence}",
            session.session_id.as_ref()
        )
        .into(),
        session_id: session.session_id.clone(),
        sequence,
        sent_at_unix_ms,
        body,
    };
    debug_assert!(event.validate().is_ok());
    session.events.push(event.clone());
    event
}
