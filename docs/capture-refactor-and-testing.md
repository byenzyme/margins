# Capture refactor and testing

Status: proposed implementation design; no runtime changes made by these documents.
Date: 2026-09-15.

Companion to [Remote workspace authority and local capture](remote-workspace-implementation.md).
That document owns product scope, addressing, authorization, and remote delivery.
This document explains how to reduce implementation complexity and prove that
slow audio delivery, transcription, or note persistence cannot block unrelated work.
Backward compatibility is not required. Existing user data must be preserved.

## 1. Design decision

Consolidate the places that decide what happened to a session. Use one session
control interface, one persistence owner, independent application records, and
adapters for native devices, browsers, and remote delivery. Start with modules
inside existing crates. Introduce a trait only at an actual replaceable boundary.

The primary test is a recording that can be paused and stopped while note saving
and transcription are deliberately stalled. Passing a queue unit test alone does
not establish that the application can do this.

## 2. Commit evidence and limits

Reviewed local main `f589b295d` and BB branch tip `36b10450c`. They are different
trees; references below identify changes to reconcile, not a claim that all have
landed in main. The Mac reported `9546e17cabfa27baa8bc51d5349e1b61a20edb1d`, whose
commit object was unavailable here. Native parity needs review against that tree.

| Commit | Reuse or correction |
| --- | --- |
| `5a703dc5a` | Extracted `TimedMemoDocument` and shared timed editing across TUI/desktop. Reuse this model. |
| `777e45922` | Added project-scoped browser capture, reused timed memo in hosted notepad updates, and removed the older plugin live transport. Avoid rebuilding the removed stack. |
| `36b10450c` | Pruned unused plugin operations, duplicate fields, and the old live-runtime installer path. Preserve this scope reduction. |
| `7332bd58f`, `16d388034` | Decoupled native/web capture from model loading; added warming/degraded handling and queue budgets. Preserve these behaviors and test their composition. |
| `634da6e39` | Hardened digest validation, discontinuity conflicts, invalid-command handling, and finalization races. These are correctness requirements. |
| `ff76286d1` | Mixed compatibility plumbing with useful validation and tombstone guards. Remove compatibility machinery without deleting those protections. |
| `0f636700d`, `ecdf7e2cb` | Hardened hosted finalization and recovery. Replacement code must preserve ID-scoped recovery and durable input handling. |

Relevant implementation sites:

- Main: `crates/public/margins-meeting-runtime/src/lib.rs`,
  `crates/public/margins-store/src/{legacy,sqlite}.rs`,
  `desktop/src-tauri/src/{web_session,live_backchannel,web_live_asr,lib}.rs`.
- BB branch: `crates/public/margins-live-runtime/src/lib.rs`,
  `crates/public/margins-core/src/memo.rs`,
  `integrations/bb-plugin-margins/src/{app,browser-capture,server,project-server,contracts}.*`.

## 3. State ownership

| Fact | Owner | May coexist with |
| --- | --- | --- |
| Producer recording/paused/stopped | Capture controller on the device | Delivery pending; ASR warming or failed |
| Accepted producer and takeover rights | Session authority | Disconnected producer with recoverable input |
| Input received and durably finalized | Session persistence | Processing queued or failed; explicit audio gaps |
| Local unacknowledged input | Delivery adapter | Producer stopped |
| Memo document and revision | Shared timed-memo model and authority | Client draft based on an older revision |

| Transcription health and progress | ASR worker/job | Healthy durable capture |
| Note association | Application metadata | Missing synced file; failed later job |
| Job outcome | Named processing job | Earlier note remains available |
| Audio removal eligibility and application | Retention module | Any independent note-link status |

Keep current capture health observable without persisting every meter update.
Use existing durable receipts and job records for facts that must survive restart.
The UI projects these facts into a concise display; its labels are not a second
authoritative lifecycle. Remove `Processing`/`Ready` and note errors from the
authoritative capture lifecycle when their consumers move to explicit job facts.

### Native TUI authority (2026-10-02)

`margins new` and local attach reserve or recover an in-process meeting runtime
session. After the native recorder seals each stereo WAV, the TUI sends its two
16 kHz PCM lanes through the runtime Recorder facade, closes the segment, and
finalizes the session. The store projects runtime segment metadata and links the
same native WAV for the existing audio processing path. The public CLI's
unavailable capture command no longer writes a second set of session rows.
Attach replays an interrupted runtime ingest of a sealed WAV and repairs a
missing WAV link after segment finalization. Native samples still held by the
recorder before WAV seal are outside this recovery path.

The TUI remembers the memo revision and lines it actually read. On save, it
compares and replaces against that revision. Independent line edits are merged
and retried; conflicting edits to one line leave the remote version intact and
write the local draft to a `*.memo-conflict-*.md` file for review. The memo's
SQLite record is authoritative. `mirror_stale` now means its repairable Markdown
projection differs from that record or could not be refreshed; it no longer
needs to account for TUI writes made directly to Markdown. A stale projection
can be refreshed with `SqliteWorkspaceAuthorityStorage::refresh_memo_mirror`.

Older sessions can retain `lifecycle_state = active` in SQLite despite finalized
audio. Readers report them as ended when they have a finalized segment and no
meeting-runtime row. This is a read-time interpretation and leaves historical
rows unchanged.

## 4. Control, data, and processing boundaries

```text
Native / browser producer
  | bounded enqueue
  +--> canonical local recorder OR remote delivery spool --> durable authority
  +--> optional bounded live ASR feed                     --> transcript results

CLI / TUI / desktop / BB controls --> one session control implementation
                                        | short persistence transactions
                                        +--> snapshot / receipts / events

Application coordinator --> processing jobs / note associations / retention
```

Evolve or fold the BB branch's `LiveRuntime::execute/snapshot` into the session
module. Do not introduce another parallel control trait. Remove `DesktopLive*`
compatibility aliases. The control interface owns typed mutations, receipts,
bounded snapshots, and replay; binary ingress has its own streaming adapter.

Select local/direct versus remote execution once at initialization. Local capture
keeps the existing recorder files, with no duplicate spool, wire serialization,
transport hashing, or mandatory server startup. A service already owning the store
uses local IPC and is measured separately from standalone in-process execution.

The capture callback cannot wait for network, database, ASR, or spool disk writes.
Use a bounded enqueue into the writer. If durable capacity is exhausted, stop
safely and report loss; silently dropping indispensable audio is not recovery.
The optional live ASR queue may drop with explicit accounting and invalidate live
transcript reuse. Share deadline mechanics where useful, not these distinct policies.

Pause and Stop stop local production immediately. They do not await a note save,
remote command, MediaRecorder stop event, upload drain, or ASR finish. Release
microphone tracks when stopping; drain available final data separately. Display
local state and pending authority acknowledgement truthfully. Preserve failed drafts.

The current BB panel awaits `saveNotepad()` before invoking pause/stop. Remove that
dependency. Immediate local control must also work when the panel displays recovery.

## 5. Persistence: reuse rules, replace whole snapshots

The current `MeetingRuntimeStorage` loads/replaces `StoredSessionV1`, containing
chunks, command bodies, and events. In-memory loads clone the aggregate. Adopting
it unchanged with serialized SQLite snapshots would make per-chunk work grow with
the recording. Moving payload bytes alone is insufficient if command history still
retains those bodies or every command reloads the complete event history.

Refactor toward a short transaction reading affected state and applying a delta:

1. Validate scope, producer, command/chunk identity, and format.
2. For remote input, durably stage immutable payload bytes outside the transaction.
3. Read affected session/lane metadata and the relevant receipt.
4. Enforce revision, digest, coverage, close, and finalize invariants.
5. Commit changed metadata, receipt, new events, and required work notification.
6. Acknowledge only after the referenced bytes and transaction are durable.

Use uniqueness constraints and transactions to resolve concurrent attempts.
Receipts identify immutable requests without retaining duplicate audio bodies.
Events/history are paginated; replay does not reload audio. Preserve exact retry
and conflict behavior from the existing runtime tests. Expected revision for a
memo edit should not change merely because an audio chunk arrived.

Choose one implementation in `margins-store`. Remove parallel path-scoped and
aggregate write APIs, compatibility synchronization triggers, and redundant
serialized segment representations after supported callers use the same owner.
Tables for chunks, receipts, memo revisions, and jobs represent different facts;
they are not competing session registries. Blob/filesystem recovery remains explicit.

No global mutex or task queue may serialize status, memo edits, and control behind
ASR or network waits. Short database serialization is acceptable; decoding,
transcoding, uploads, and model loading cannot run inside those transactions.

## 6. Application simplification

- `link_note`/`unlink_note`: update only the association, with source-relative
  references and revision checks. Missing synced files are derived availability.
- Processing jobs: own input identity/revision, attempt, progress, output, and
  failure. Reject late results from cancelled or superseded attempts.
- Distillation coordinator: write through the existing local writer, associate
  the result, and complete the specific job explicitly. Preserve draft/refinement
  behavior through that coordinator.
- Retention: evaluate eligibility and apply explicit removal. Completion and
  linking never call audio deletion.

Delete overloaded `set_vault_note_path` and equivalent metadata-sync side effects;
update callers together. Do not retain `complete_legacy_*` wrappers. Removing
automatic cleanup is a documented behavior change. No remote note writer,
source-file proxy, file-sync lifecycle, or generic workflow framework is needed.

## 7. BB simplification

The browser owner retains the media producer, connection state, current snapshot,
event cursor, and local draft. Panels subscribe to it. Keep BB project-to-workspace
mapping and routing; derive session status from Margins rather than a second BB
saved flag. Remove per-panel polling and redundant refresh-after-mutation chains.

The authority owns lease expiry and takeover; the browser independently enforces
its negotiated local stop deadline. Remove the competing BB server expiry loop.
A never-settling heartbeat must still cause local deadline expiry: base the watchdog
on last acknowledgement, not only on a promise rejection. Limit heartbeat and
refresh concurrency, fence old callbacks, and reconcile receipts after uncertainty.

Propagate request cancellation through hops where supported and bound waits at
each adapter, including response-body reads. A timeout can coexist with a committed
operation; retry with the same identity or query its receipt. Do not assume abort
rolls back writes. Avoid making best-effort notification delivery delay a receipt.

Place shared browser transport utilities in a small neutral module consumed by
desktop and BB. Preserve separate policies for durable WebM and optional PCM.
Only introduce a distributable package if the build boundary needs it.

## 8. Test design

### Deterministic policy and orchestration

Inject the monotonic clock, device interface, transport, storage failpoints, and
processing worker at their boundaries. Run the production controller/reducer and
queue code. Do not mock the exact method whose non-blocking behavior is under test.
Use a manually controlled scheduler or fake timers for policy tests, and barriers
for real concurrency. Avoid assertions based on arbitrary short sleeps.

Faults: delayed success, rejection before commit, commit with lost ACK, hung
headers/body, ignored cancellation with late completion, reordered events, and
client/server restart. Separate control, audio, memo, event, and ASR fault controls.

| Scenario | Required observation |
| --- | --- |
| Memo save never resolves; Stop clicked | Producer stop invoked and tracks released before memo completion; draft retained; stop command can proceed |
| Pause while control transport hangs | Local samples stop; acknowledgement remains pending; no false fully-acknowledged paused state |
| Heartbeat never resolves | At most one in flight; local deadline still expires; late response cannot revive capture |
| ASR never becomes ready or stops consuming | Durable writer/ACKs and memo/control progress; ASR budget bounded; degradation visible |
| Audio upload commits but ACK is lost | Exact retry yields one stored chunk and valid receipt |
| A durable chunk is missing | Later receipt/closure cannot falsely claim full coverage; retry or explicit discontinuity required |
| Stop with missing recorder stop event | Tracks released immediately; final-data wait bounded; pending/subset result truthful |
| Old refresh returns after Stop or session switch | No state regression, focus change, or replacement of newer draft |
| Panel closes/remounts | Same owner and producer; no duplicate stream, heartbeat, or subscription |
| Linked note during failed processing | Association changes; failure, draft, and retained audio remain |
| Late job success after cancel/delete | No resurrection or unauthorized result publication |

### Shared service contract suite

Run the same observable command, receipt, revision, and query cases against an
in-memory reference and real temporary SQLite storage. Run wire contract cases
through loopback and the BB adapter using the real service implementation. Use
Rust-generated fixtures for the TypeScript contract. Remove tests whose sole
purpose is accepting obsolete versions; keep malformed-input and unknown-version
rejection tests. Local and remote execution share semantics, not identical costs.

### Durability and isolation suite

Restart at blob flush, rename, metadata commit, ACK send, close/finalize commit,
job claim, result publication, and retention apply. Reopen real temporary storage
and assert no acknowledged input loss, duplicate session, receipt, or job outcome.
Verify finalization with conflicting/missing chunks, explicit gaps, concurrent
close/finalize, and exact retries. Preserve authorization and tombstone tests.

Hold ASR/finalizer work on a barrier while independently requesting status, editing
memo, and delivering a chunk. Assert those complete before releasing the worker.
Add outer watchdogs so a regression fails instead of hanging CI indefinitely.
Database contention and disk-full tests must prove no false durable ACK.

### Scaling and native evidence

Grow fixed-size recordings across several session sizes. Count allocations,
payload bytes copied/read/written, rows touched, and active tasks. Metadata index
lookups may grow logarithmically; per-chunk work must not traverse all prior chunks
or events. Capture queues and in-flight request counts must remain bounded.

For actual latency, compare repeated before/after runs with fixed fixtures and
build identities; define the noise allowance before seeing candidate results.
Measure producer stop, input readiness, callbacks/drops, CPU/RSS, disk writes,
memo latency, finalization, and recall. Verify zero added transport/spool work on
default local execution. Fake-time tests cannot prove native responsiveness.

Keep narrow real browser MediaRecorder, native mic/system, CoreML rolling, and
Mac-to-service recovery passes. They validate device/codec/platform integration;
deterministic tests must run without those capabilities. Use repository cargo lanes
and native verification instructions when implementing. No builds are needed for
this documentation change.

## 9. Delivery and removal gates

1. Pin the relevant trees and enumerate active writers, routes, and completion
   callers. Reconcile the existing timed-memo extraction and live control seam.
2. Build the record/edit/pause/stop regression harness against current production
   orchestration. Demonstrate failures for stalled memo/transport cases before fixes.
3. Consolidate client control ownership and independent states. Remove panel polling,
   save-before-stop coupling, competing lease bookkeeping, and obsolete contracts.
4. Replace whole-session persistence with transactional changes and one store owner.
   Port runtime invariants and run real storage crash/scaling tests before cutover.
5. Separate associations/jobs/retention and update completion callers together.
   Remove bundled setters and direct completion-triggered audio cleanup.
6. Run the same supported journey through direct, loopback, and BB adapters. Add
   remote spooling and SSH once the service semantics are proven. Remove replaced
   generic-invoke routes and unused abstractions in the corresponding slice.

Every slice records: retained behavior, removed files/interfaces, remaining callers,
meaningful tests, and measured limitations. Temporary duplication during development
must have an explicit deletion gate; it is not the shipped architecture. Keep data
conversion explicit and recoverable. Do not delete fixtures or user recordings to
make a breaking API change easier.

## 10. Questions to resolve in code

- Which existing SQLite rows can be canonical without duplicate segment JSON?
- Which runtime transitions need bounded lane coverage reads versus session reads?
- How does offline memo editing preserve action time using the shared document?
- What browser storage/recovery guarantees are actually supported across refresh?
- Which BB hop supports binary streaming and cancellation, and what are its limits?
- Which CoreML worker ownership constraints require a dedicated thread? Keep those
  constraints while testing readiness and shutdown through portable controller seams.

These decisions require implementation evidence. None requires preserving an old
API, introducing another session model, or coupling capture to processing success.
