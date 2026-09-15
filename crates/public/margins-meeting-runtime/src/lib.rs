//! Durable, transport-neutral state machine for `margins-meeting-protocol`.
//!
//! This crate stores encoded audio payloads as opaque bytes. It deliberately
//! has no HTTP, socket, async-runtime, audio-device, decoder, or OS bindings.

#![forbid(unsafe_code)]

use margins_meeting_protocol::{
    AppendProvenanceHopV1, AudioAcknowledgementV1, AudioChunkV1, CaptureDiscontinuityV1,
    CaptureHealthV1, CaptureProvenanceHopV1, ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1,
    CommandRejectedV1, CreateSessionV1, DigestAlgorithmV1, DurationMillis, FinalizeSessionV1,
    LaneId, LiveErrorV1, LiveMutationResponseV1, LiveOperationId, LiveSessionRequestV1,
    LiveSnapshotV1, LiveStartRequestV1, LiveUpdateNotepadRequestV1, MessageId, ProtocolVersionV1,
    ProvenanceHopRecordedV1, ReplayCompletedV1, SegmentFinalizedV1, SequenceRangeV1,
    ServerMessageBodyV1, ServerMessageV1, SessionCreatedV1, SessionFinalizedV1, SessionId,
    SessionMillis, UnixMillis, ValidationErrorV1, MAX_SAFE_JSON_INTEGER,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::Mutex,
};

/// One typed mutation for the process that owns the active local session.
/// Binary ingress deliberately remains on the streaming storage path below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRuntimeCommandV1 {
    Start(LiveStartRequestV1),
    Pause(LiveSessionRequestV1),
    Resume(LiveSessionRequestV1),
    Stop(LiveSessionRequestV1),
    UpdateNotepad(LiveUpdateNotepadRequestV1),
}

impl LiveRuntimeCommandV1 {
    pub fn operation_id(&self) -> &LiveOperationId {
        match self {
            Self::Start(request) => &request.operation_id,
            Self::Pause(request) | Self::Resume(request) | Self::Stop(request) => {
                &request.operation_id
            }
            Self::UpdateNotepad(request) => &request.operation_id,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationErrorV1> {
        match self {
            Self::Start(request) => request.validate(),
            Self::Pause(request) | Self::Resume(request) | Self::Stop(request) => {
                request.validate()
            }
            Self::UpdateNotepad(request) => request.validate(),
        }
    }
}

pub type LiveRuntimeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LiveMutationResponseV1, LiveErrorV1>> + Send + 'a>>;

/// The single command/snapshot control port for the process owning capture.
pub trait LiveRuntime: Send + Sync {
    fn snapshot(&self, session_id: Option<&str>) -> Result<LiveSnapshotV1, LiveErrorV1>;
    fn execute(&self, command: LiveRuntimeCommandV1) -> LiveRuntimeFuture<'_>;
}

const MAX_COMMIT_ATTEMPTS: usize = 64;
const MAX_REPLAY_EVENTS: usize = 512;

/// Result of an atomic storage write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCommit {
    Committed,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCommandReceiptV1 {
    pub message_id: MessageId,
    pub fingerprint: String,
    pub response_range: SequenceRangeV1,
}

#[derive(Debug, Clone)]
pub struct SessionDeltaV1 {
    pub expected_revision: u64,
    pub session: StoredSessionV1,
    pub receipt: StoredCommandReceiptV1,
    pub events: Vec<ServerMessageV1>,
    pub audio_chunk: Option<AudioChunkV1>,
}

/// Bounded persistence seam used by [`MeetingRuntime`]. Payloads, receipts,
/// and event pages are separate rows. Session loads therefore never clone
/// accumulated audio or command/event history.
pub trait MeetingRuntimeStorage: Send + Sync {
    type Error;

    fn load_session(&self, session_id: &SessionId) -> Result<Option<StoredSessionV1>, Self::Error>;

    fn load_session_by_create_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<StoredSessionV1>, Self::Error>;

    fn load_command_receipts(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<Vec<StoredCommandReceiptV1>, Self::Error>;
    fn load_events(
        &self,
        session_id: &SessionId,
        range: SequenceRangeV1,
        limit: usize,
    ) -> Result<Vec<ServerMessageV1>, Self::Error>;
    fn load_audio_chunk(
        &self,
        session_id: &SessionId,
        segment_id: &str,
        lane_id: &LaneId,
        sequence: u64,
    ) -> Result<Option<AudioChunkV1>, Self::Error>;
    fn create_session(&self, delta: SessionDeltaV1) -> Result<StorageCommit, Self::Error>;
    fn apply_delta(&self, delta: SessionDeltaV1) -> Result<StorageCommit, Self::Error>;
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
    receipts: BTreeMap<(SessionId, MessageId), Vec<StoredCommandReceiptV1>>,
    events: BTreeMap<SessionId, Vec<ServerMessageV1>>,
    chunks: BTreeMap<(SessionId, String, LaneId, u64), AudioChunkV1>,
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

    pub fn snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<InMemoryRuntimeSnapshot>, InMemoryStorageError> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        let Some(session) = state.sessions.get(session_id).cloned() else {
            return Ok(None);
        };
        Ok(Some(InMemoryRuntimeSnapshot {
            session,
            events: state.events.get(session_id).cloned().unwrap_or_default(),
            audio_chunk_count: state
                .chunks
                .keys()
                .filter(|(id, _, _, _)| id == session_id)
                .count(),
        }))
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryRuntimeSnapshot {
    session: StoredSessionV1,
    events: Vec<ServerMessageV1>,
    audio_chunk_count: usize,
}

impl InMemoryRuntimeSnapshot {
    pub fn session(&self) -> &StoredSessionV1 {
        &self.session
    }
    pub fn events(&self) -> &[ServerMessageV1] {
        &self.events
    }
    pub fn audio_chunk_count(&self) -> usize {
        self.audio_chunk_count
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

    fn load_command_receipts(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<Vec<StoredCommandReceiptV1>, Self::Error> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        Ok(state
            .receipts
            .get(&(session_id.clone(), message_id.clone()))
            .cloned()
            .unwrap_or_default())
    }

    fn load_events(
        &self,
        session_id: &SessionId,
        range: SequenceRangeV1,
        limit: usize,
    ) -> Result<Vec<ServerMessageV1>, Self::Error> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        let events = state
            .events
            .get(session_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let start = usize::try_from(range.start)
            .unwrap_or(usize::MAX)
            .min(events.len());
        let end = usize::try_from(range.end_exclusive)
            .unwrap_or(usize::MAX)
            .min(events.len())
            .min(start.saturating_add(limit));
        Ok(events[start..end].to_vec())
    }

    fn load_audio_chunk(
        &self,
        session_id: &SessionId,
        segment_id: &str,
        lane_id: &LaneId,
        sequence: u64,
    ) -> Result<Option<AudioChunkV1>, Self::Error> {
        let state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        Ok(state
            .chunks
            .get(&(
                session_id.clone(),
                segment_id.to_string(),
                lane_id.clone(),
                sequence,
            ))
            .cloned())
    }

    fn create_session(&self, delta: SessionDeltaV1) -> Result<StorageCommit, Self::Error> {
        let mut state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        let SessionDeltaV1 {
            session,
            receipt,
            events,
            ..
        } = delta;
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
        let session_id = session.session_id.clone();
        state.sessions.insert(session_id.clone(), session);
        state
            .receipts
            .entry((session_id.clone(), receipt.message_id.clone()))
            .or_default()
            .push(receipt);
        state.events.insert(session_id, events);
        Ok(StorageCommit::Committed)
    }

    fn apply_delta(&self, delta: SessionDeltaV1) -> Result<StorageCommit, Self::Error> {
        let mut state = self.inner.lock().map_err(|_| InMemoryStorageError)?;
        let session = delta.session;
        let Some(current) = state.sessions.get(&session.session_id) else {
            return Ok(StorageCommit::Conflict);
        };
        if current.revision != delta.expected_revision
            || current.create.idempotency_key != session.create.idempotency_key
        {
            return Ok(StorageCommit::Conflict);
        }
        let session_id = session.session_id.clone();
        if let Some(chunk) = delta.audio_chunk {
            state.chunks.insert(
                (
                    session_id.clone(),
                    chunk.segment_id.as_ref().to_string(),
                    chunk.lane_id.clone(),
                    chunk.sequence,
                ),
                chunk,
            );
        }
        state
            .receipts
            .entry((session_id.clone(), delta.receipt.message_id.clone()))
            .or_default()
            .push(delta.receipt);
        state
            .events
            .entry(session_id.clone())
            .or_default()
            .extend(delta.events);
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
    provenance_hops: BTreeMap<String, AppendProvenanceHopV1>,
    discontinuities: BTreeMap<String, CaptureDiscontinuityV1>,
    segments: BTreeMap<String, SegmentState>,
    last_health_at_ms: Option<SessionMillis>,
    finalize: Option<FinalizeRecord>,
    next_event_sequence: u64,
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

    pub fn discontinuities(&self) -> impl Iterator<Item = &CaptureDiscontinuityV1> {
        self.discontinuities.values()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SegmentState {
    lanes: BTreeMap<LaneId, LaneState>,
    close: Option<CloseRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LaneState {
    coverage: RangeSet,
    latest_end_ms: u64,
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

    fn overlaps(&self, candidate: SequenceRangeV1) -> bool {
        self.ranges
            .iter()
            .any(|range| ranges_overlap(*range, candidate))
    }

    fn has_at_or_after(&self, sequence: u64) -> bool {
        self.ranges
            .iter()
            .any(|range| range.end_exclusive > sequence)
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
                provenance_hops: BTreeMap::new(),
                discontinuities: BTreeMap::new(),
                segments: BTreeMap::new(),
                last_health_at_ms: None,
                finalize: None,
                next_event_sequence: 0,
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
            let receipt = command_receipt(&message, &response);
            match self
                .storage
                .create_session(SessionDeltaV1 {
                    expected_revision: 0,
                    session,
                    receipt,
                    events: response.clone(),
                    audio_chunk: None,
                })
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
        let fingerprint = command_fingerprint(&message);
        let receipts = self
            .storage
            .load_command_receipts(&message.session_id, &message.message_id)
            .map_err(RuntimeError::Storage)?;
        if let Some(record) = receipts
            .iter()
            .find(|record| record.fingerprint == fingerprint)
        {
            let messages = self
                .storage
                .load_events(
                    &message.session_id,
                    record.response_range,
                    MAX_REPLAY_EVENTS,
                )
                .map_err(RuntimeError::Storage)?;
            return Ok(ApplyResult::Done(RuntimeResponseV1 {
                messages,
                idempotent_replay: true,
            }));
        }
        if !receipts.is_empty() {
            let event_start = session.next_event_sequence;
            let response = reject(
                &mut session,
                &message,
                "conflict",
                "message_id was already used for different content",
            );
            return self.commit_existing(session, message, response, None, event_start, false);
        }

        let existing_chunk = match &message.body {
            ClientMessageBodyV1::AudioChunk(chunk) => self
                .storage
                .load_audio_chunk(
                    &message.session_id,
                    chunk.segment_id.as_ref(),
                    &chunk.lane_id,
                    chunk.sequence,
                )
                .map_err(RuntimeError::Storage)?,
            _ => None,
        };
        let replay_events = match &message.body {
            ClientMessageBodyV1::ResumeSession(resume) => {
                let start = resume
                    .after_server_sequence
                    .map_or(0, |value| value.saturating_add(1));
                self.storage
                    .load_events(
                        &message.session_id,
                        SequenceRangeV1 {
                            start,
                            end_exclusive: session.next_event_sequence,
                        },
                        MAX_REPLAY_EVENTS,
                    )
                    .map_err(RuntimeError::Storage)?
            }
            _ => Vec::new(),
        };
        let known_close_commands = match &message.body {
            ClientMessageBodyV1::FinalizeSession(finalize) => {
                let mut known = BTreeSet::new();
                for reference in &finalize.segment_closes {
                    if !self
                        .storage
                        .load_command_receipts(&message.session_id, &reference.close_message_id)
                        .map_err(RuntimeError::Storage)?
                        .is_empty()
                    {
                        known.insert(reference.close_message_id.clone());
                    }
                }
                known
            }
            _ => BTreeSet::new(),
        };
        let event_start = session.next_event_sequence;
        let response = apply_command(
            &mut session,
            &message,
            existing_chunk.as_ref(),
            replay_events,
            &known_close_commands,
        );
        let audio_chunk = match &message.body {
            ClientMessageBodyV1::AudioChunk(chunk)
                if existing_chunk.is_none()
                    && response.iter().any(|event| {
                        matches!(event.body, ServerMessageBodyV1::AudioAcknowledged(_))
                    }) =>
            {
                Some(chunk.clone())
            }
            _ => None,
        };
        self.commit_existing(session, message, response, audio_chunk, event_start, false)
    }

    fn commit_existing(
        &self,
        mut session: StoredSessionV1,
        message: ClientMessageV1,
        response: Vec<ServerMessageV1>,
        audio_chunk: Option<AudioChunkV1>,
        event_start: u64,
        idempotent_replay: bool,
    ) -> Result<ApplyResult, RuntimeError<S::Error>> {
        let expected_revision = session.revision;
        session.revision = session
            .revision
            .checked_add(1)
            .ok_or(RuntimeError::Contention)?;
        match self
            .storage
            .apply_delta(SessionDeltaV1 {
                expected_revision,
                receipt: command_receipt(&message, &response),
                session,
                events: response
                    .iter()
                    .filter(|event| event.sequence >= event_start)
                    .cloned()
                    .collect(),
                audio_chunk,
            })
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

fn command_receipt(
    message: &ClientMessageV1,
    response: &[ServerMessageV1],
) -> StoredCommandReceiptV1 {
    StoredCommandReceiptV1 {
        message_id: message.message_id.clone(),
        fingerprint: command_fingerprint(message),
        response_range: response_range(response),
    }
}

fn command_fingerprint(message: &ClientMessageV1) -> String {
    struct HashWriter(blake3::Hasher);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(blake3::Hasher::new());
    serde_json::to_writer(&mut writer, message).expect("validated protocol messages serialize");
    writer.0.finalize().to_hex().to_string()
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

fn apply_command(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    existing_chunk: Option<&AudioChunkV1>,
    replay_events: Vec<ServerMessageV1>,
    known_close_commands: &BTreeSet<MessageId>,
) -> Vec<ServerMessageV1> {
    match &message.body {
        ClientMessageBodyV1::CreateSession(_) => reject(
            session,
            message,
            "conflict",
            "session_id or create idempotency key was already used",
        ),
        ClientMessageBodyV1::ResumeSession(resume) => {
            let watermark = session.next_event_sequence.checked_sub(1);
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
            let replayed = replay_events;
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
        ClientMessageBodyV1::AudioChunk(chunk) => {
            apply_chunk(session, message, chunk, existing_chunk)
        }
        ClientMessageBodyV1::CaptureDiscontinuity(discontinuity) => {
            apply_discontinuity(session, message, discontinuity)
        }
        ClientMessageBodyV1::CaptureHealth(health) => apply_health(session, message, health),
        ClientMessageBodyV1::CloseSegment(close) => apply_close(session, message, close),
        ClientMessageBodyV1::FinalizeSession(finalize) => {
            apply_finalize(session, message, finalize, known_close_commands)
        }
    }
}

fn apply_chunk(
    session: &mut StoredSessionV1,
    message: &ClientMessageV1,
    chunk: &AudioChunkV1,
    existing_chunk: Option<&AudioChunkV1>,
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
    if let Some(existing) = existing_chunk {
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
    lane.coverage.add(SequenceRangeV1 {
        start: chunk.sequence,
        end_exclusive: chunk.sequence + 1,
    });
    lane.latest_end_ms = lane.latest_end_ms.max(chunk_end);
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
            .is_some_and(|lane| lane.coverage.overlaps(discontinuity.sequence_range))
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
            .is_some_and(|lane| lane.coverage.has_at_or_after(boundary.next_sequence))
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
        .map(|lane| lane.latest_end_ms)
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
    known_close_commands: &BTreeSet<MessageId>,
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
        if known_close_commands.contains(&reference.close_message_id) {
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
        .map(|lane| lane.latest_end_ms)
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
                    coverage: RangeSet::default(),
                    latest_end_ms: 0,
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
    let sequence = session.next_event_sequence;
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
    session.next_event_sequence += 1;
    event
}
