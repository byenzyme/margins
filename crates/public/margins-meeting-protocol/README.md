# Margins meeting protocol

`margins-meeting-protocol` contains the public V1 data-transfer objects for a
Margins meeting runtime. It has no socket, HTTP, async-runtime, audio-device,
Tauri, or OS bindings; callers choose JSON, CBOR, WebSocket, streams, queues,
or another framing layer.

## Typical flow

1. A mobile app or browser chooses stable `session_id` and `message_id` values,
   sends `create_session`, and declares capture sources, ordered lanes, formats,
   and origin provenance.
2. It sends `audio_chunk` messages. Sequence numbers start at zero and are
   independent for every `(segment_id, lane_id)`. Binary serializers use byte
   strings; JSON uses standard padded base64 instead of integer arrays.
3. A VPS relay forwards upstream messages unchanged and records itself with a
   separate, idempotent `append_provenance_hop` command. Provenance is
   self-asserted lineage, not authentication or cryptographic attestation.
4. The runtime emits cumulative `audio_acknowledged` events, replaceable
   transcript partials, immutable ordered transcript commits, memo revisions,
   and artifact-ready locators.
5. The producer sends `close_segment` with exclusive lane boundaries. The
   runtime emits `segment_finalized` only after every lower sequence is covered
   by a durable chunk or discontinuity. `finalize_session` names every exact
   close operation, atomically freezes that segment set, and waits for listed
   closes that raced behind it.

## Reconnect and idempotency

- Reuse the same `session_id`, create `idempotency_key`, and `message_id` when
  retrying the exact same operation. Reusing any for different content is a
  conflict.
- A chunk is additionally identified by `(session_id, segment_id, lane_id,
  sequence)`. A duplicate with the same digest is a no-op; a different digest
  is a conflict.
- Keep chunks until their sequence is below `durable_through_sequence` or is in
  `durable_out_of_order`. After reconnect, resend everything else in any order;
  processing remains lane-sequential.
- Report unrecoverable gaps with a stable discontinuity ID and exact half-open
  sequence range. Use an empty range at the next sequence when audio was never
  chunked. Close and finalize commands are retryable with their original ID.
- Server `sequence` is zero-based and strictly increasing per session. After a
  reconnect, send `resume_session` with the last applied sequence. The runtime
  replays original envelopes and ends with `replay_completed`; it rejects an
  unavailable cursor rather than silently skipping events. Detect a sequence
  gap before reducing later events.
- Deduplicate server events by `(session_id, sequence)` and require duplicates
  to have identical message IDs and content. Transcript commits additionally
  use `commit_sequence`; memo/artifact updates use `(ID, revision)`.

## Time, validation, and evolution

`SessionMillis(0)` is the creator-defined monotonic start of capture.
`started_at_unix_ms` is its approximate wall-clock mapping; relays preserve
session offsets and never rebase them. Unix timestamps are diagnostic and
never ordering authority. V1 integer values must not exceed
`MAX_SAFE_JSON_INTEGER`, so browser JSON round-trips exactly.

Receivers must call `ClientMessageV1::validate` or
`ServerMessageV1::validate` before stateful processing. Those methods check
local IDs, digests, safe integers, formats, spans, ranges, and canonical ACKs.
The runtime must additionally enforce same-ID/same-content retries, lane
sequence conflicts, declared lane/source membership, monotonic revisions and
times, lifecycle, close boundaries, and the exact finalize prerequisite set.

V1 command receivers ignore unknown object fields but reject an unknown
`protocol_version` or command `type`. V1 server-event receivers preserve an
unknown event type and payload as `Unknown`, keeping older clients and
transports live. Typed deserialization of a known payload does not retain its
unknown fields; transparent relays that must reproduce them byte-for-byte
should forward the original frame instead of deserializing and reserializing.
