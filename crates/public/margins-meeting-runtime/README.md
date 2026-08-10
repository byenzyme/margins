# Margins meeting runtime

`margins-meeting-runtime` is the public, server-side V1 state machine for
[`margins-meeting-protocol`](../margins-meeting-protocol/README.md). It accepts
validated protocol commands, atomically advances durable session state, and
returns ordered protocol events. It is a library, not an HTTP server, relay,
capture stack, audio decoder, or transcription engine.

The crate treats chunk payloads as opaque encoded bytes. It has no dependency
on desktop code, recorder code, CPAL, Tauri, an async runtime, or an audio
codec. Its runtime dependencies are the public protocol crate, Serde, and the
SHA-256/BLAKE3 implementations used to verify declared chunk digests.

## Integration seams

```text
mobile app / browser
  |  ClientMessageV1 over an application-selected transport
  v
VPS API / queue adapter
  |  authenticate, deserialize, validate, then MeetingRuntime::handle
  v
MeetingRuntime<MeetingRuntimeStorage>
  |-- atomic command state + append-only ServerMessageV1 event log
  |-- opaque durable chunks/discontinuities via StoredSessionV1 snapshots
  v
transcript worker / memo worker
  |  consume ordered opaque chunks and publish results through an application
  |  outbox; transport and worker-result ingestion remain adapter concerns
  v
client reconnect
     ResumeSessionV1 -> original retained envelopes + ReplayCompletedV1
```

A mobile/browser client owns stable session, command, segment, lane, and close
IDs. A VPS transport adapter performs authentication and framing, calls
`ClientMessageV1::validate`, then calls `MeetingRuntime::handle`. The runtime
validates again before state changes. It never interprets a payload as audio.

`MeetingRuntimeStorage` is an optimistic compare-and-swap persistence contract.
Production adapters should implement its create and replace operations as
database transactions, including unique indexes for `session_id` and the create
`idempotency_key`. `InMemoryMeetingRuntimeStorage` uses one mutex-protected map,
implements the same conflicts atomically, is thread-safe, and is intended for
tests and bounded single-process use.

Workers can read `StoredSessionV1::audio_chunks` in deterministic
segment/lane/sequence order and `discontinuities` as explicit gap coverage.
Worker leasing, queue offsets, transcript inference, and publishing transcript,
memo, or artifact events are intentionally outside this command state machine;
an application should make those an outbox/worker adapter so inference retries
cannot mutate command idempotency.

## Guarantees

- A message ID is immutable. An exact retry returns its original envelopes;
  reuse with different content produces one durable, repeatable conflict.
- A create idempotency key belongs to exactly one session.
- Chunk identity is `(session, segment, lane, sequence)`: declared digests are
  verified, exact content is a no-op, and different content is a sequence
  conflict.
- ACKs are canonical cumulative coverage plus sorted, merged out-of-order
  ranges. Chunks and non-empty discontinuities contribute equal durable gap
  coverage.
- Close commands declare immutable, exclusive boundaries and remain pending
  until every lower sequence on every lane is covered.
- Finalize atomically freezes the exact segment/close-message set, admits a
  named close that races behind it, and emits session finalization only after
  those exact closes finalize.
- The event log is append-only and zero-based. Resume replays stored envelopes
  without changing IDs, timestamps, content, or sequences, and rejects a
  cursor beyond the retained watermark.

## Minimal use

```rust
use margins_meeting_runtime::{InMemoryMeetingRuntimeStorage, MeetingRuntime};

let runtime = MeetingRuntime::new(InMemoryMeetingRuntimeStorage::new());
// let response = runtime.handle(validated_client_message)?;
// send response.messages using your own HTTP/WebSocket/queue adapter.
```

For a durable service, replace only the storage type and put authentication,
rate limits, payload-size limits, transport acknowledgements, worker queues,
and observability around the runtime. The storage trait does not prescribe SQL,
object storage, a cloud vendor, or a network framework.
