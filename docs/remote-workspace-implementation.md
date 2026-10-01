# Remote workspace authority and local capture

Status: implementation specification; proposed interfaces below are not shipped commands.
Date: 2026-09-14; revised 2026-09-15.

Copied from the Mac primary checkout's `docs/remote-workspace-implementation.md`
(source SHA-256 `61bb067087bdc48e48f759e5a9d6e3267473a5e591284856fd44dc55a765885f`).
This local revision incorporates the commit review and the decision that backward
compatibility is not required. See [Capture refactor and testing](capture-refactor-and-testing.md)
for the implementation boundaries, removal plan, and executable acceptance criteria.

Local operation remains the default. Shared service semantics must support the
plugin and optional remote access without imposing transport costs on local CLI,
TUI, or native desktop operation. This is an acceptance requirement.

## 1. Product decision

A user can select a remote Margins instance and a Workspace, record with the
microphone and system audio on the machine in front of them, and continue the
session through the CLI, TUI, BB plugin, or an agent. The selected instance owns
durable sessions, processing, artifacts, and retrieval. A phone can upload a
finished recording into that same Workspace.

Example, after configuring the remote service:

```sh
margins --remote ssh://beelink --workspace obsidian new
margins --remote ssh://beelink --workspace obsidian recent
margins --remote ssh://beelink --workspace obsidian transcript latest
margins --remote ssh://beelink --workspace obsidian recall "What did we decide?"
```

The first command captures on the invoking Mac. It saves to the selected
Workspace on Beelink. Running it from `~/obsidian/inbox`, `~/Downloads`, or a
machine without an Obsidian folder produces the same destination.

This is one authoritative workspace accessed by clients. It does not replicate
two independently writable workspace databases. Local capture buffering is a
delivery mechanism, not a second workspace.

### The simple boundary

Margins owns sessions and their artifacts. The agent reads and writes ordinary
notes using its native filesystem tools; the user's existing file sync carries
those notes between machines.

Within Margins, keep four responsibilities separate:

| Component | Owns |
| --- | --- |
| Session core | Recorded segments, timed memo, transcript and artifact identities |
| Note associations | Which notes were derived from a session |
| Processing jobs | Requested work, input revision, progress, result, and failure |
| Retention | Eligibility and explicit removal through the existing preview/apply workflow |

The last three belong to the application layer above the session core. They can
use the same process and database; this separation introduces no additional
service or network hop. A note association does not complete a processing job or
authorize audio deletion. An application coordinator explicitly composes these
operations when a workflow requires them.

The capture API serves the session core. Application operations expose note
associations and processing jobs only where callers need them. Retention remains
an explicit administrative workflow. Desktop, CLI, and BB reuse these operations
instead of implementing their own completion rules.

### Success criteria

- The user can name where capture happens and where the session is saved.
- Changing the client working directory cannot change the explicitly selected
  remote Workspace, session store, or recall corpus. Note output follows the
  agent's local distillation workflow and its selected local notes folder.
- Mic and system audio retain separate identities and timing through delivery.
- A disconnect cannot turn unreceived audio into a successful saved session.
- CLI, BB, and phone imports expose the same sessions to `recent`, `transcript`,
  `artifacts`, and recall workflows.
- An agent can retrieve Margins artifacts, read synced notes locally, and write
  the resulting note with native tools. It can optionally associate that note
  with the session without triggering processing or retention side effects.
- Each workspace has one canonical session representation. Existing SQLite
  files, indexes, and audio files may remain separate physical stores.
- Local capture and retrieval retain their direct execution path, with no required
  server startup, handshake, serialization, upload spool, or network round trip.
- The BB plugin's complete operational API is implemented and exercised against
  this service in both local-host and explicitly remote configurations.

## 2. Scope and limits

Deliver:

1. Explicit instance/workspace resolution for commands and agent handoffs.
2. A shared Rust application service and versioned remote API.
3. SSH access for native clients and HTTPS access for browser/phone clients.
4. Native local capture with durable resumable delivery.
5. Remote session queries, memo editing, recall, and application note associations.
6. BB transport integration and a finished-file ingestion endpoint for Shortcuts.
7. Migration, capability negotiation, failure reporting, and recovery tools.

Defer:

- Multi-master workspace synchronization, conflict-free replicated text editing,
  automatic failover to another authority, and simultaneous recording writers
  for a single session.
- Synchronizing the user's entire notes folder, models, keychains, or indexes.
- Automatic iOS Voice Memos mirroring and a native iOS companion.
- Installing or reconfiguring Tailscale, SSH, system services, or model caches
  implicitly when the user runs `new`.
- A hosted billing/account platform. The service can later be hosted, but this
  implementation targets a user-controlled instance.
- A redesign of setup, distillation prompts, or source declarations.
- Retaining old APIs, wire aliases, or storage interfaces solely for backward
  compatibility. Existing recordings and notes must still be preserved.

SSH/TLS protects transport. The processing host receives plaintext audio and
context and must be trusted. This design does not provide end-to-end encryption
against that host; such a promise would conflict with processing there.

## 3. Existing seams and evidence

These observations describe the checkout inspected for this specification;
related BB work may be on another branch. Reconcile branch state before coding.

| Existing component | Reuse | Missing or incompatible behavior |
| --- | --- | --- |
| [Native CLI dispatch](../src/cli.rs) | Interactive capture and native composition | `new`/`attach` resolve through project/cwd after parsing workspace; explicit workspace must control capture routing |
| [Public CLI dispatch](../crates/public/margins-cli/src/lib.rs) and [services](../crates/public/margins-cli/src/services.rs) | Injectable services, command parsing, session compatibility store | Path-scoped store calls and local filesystem operations cannot serve as a remote API |
| [Meeting protocol](../crates/public/margins-meeting-protocol/README.md) | Session/source/lane IDs, chunks, digests, acknowledgements, close/finalize, replay | Does not by itself provide workspace queries, authorization, application operations, or production storage |
| [Meeting runtime](../crates/public/margins-meeting-runtime/README.md) | Durable-command semantics and optimistic storage contract | Production persistence, worker integration, and canonical-store adaptation are application concerns; in-memory storage is insufficient |
| [Session repository](../crates/public/margins-core/src/session.rs) | Revisions, immutable sealed segments, artifacts, tombstones | Must reconcile with the legacy session store; two writable authorities are unacceptable |
| [Workspace resolution](../crates/public/margins-workflows/src/workspace.rs) | Declared Workspace, captures directory, ledger/index paths, reviewed policy | Instance selection must happen before local resolution; capture-binding behavior needs a common resolver |
| [Current HTTP adapter](../desktop/src-tauri/src/server/http.rs) | Existing server hosting and web capture routes | Generic invoke dispatch, token-injected assets, and query-token events are not the public remote service contract |
| [Archive workflow](../crates/public/margins-workflows/src/archive.rs) | Visible aligned Markdown for existing folder sync | Does not restore sessions on a second machine or constitute a sync service |

Existing note completion needs separation: `set_vault_note_path` in
`margins-store/src/legacy.rs` registers the note, clears errors, and marks processing
done. Desktop completion also handles drafts, refinement state, refresh, and
conditional audio cleanup. Do not expose that bundled setter as a note-association
operation. Move supported workflows to explicit application orchestration. Remove the bundled
setter and update callers together; do not add a legacy-completion wrapper.

Relevant historical threads:

- `thr_8bep7y23tb`, **Explore margins plugin integration**: later work implements
  browser capture to the BB project host and shared notepad handling. Its reported
  native Mac bridge remained incomplete; verify current branch before reusing it.
- `thr_qivcpcuu6m`, **voice memo upload scoping**: Shortcut-first upload, durable
  receipt, asynchronous transcription, and a later native background-upload path.
  It created `docs/ios-voice-memo-ingestion.md` in the Linux checkout; that file
  was absent from this checkout when this specification was written.
- `thr_3pb5pjwxme`, **Investigate path configuration issues**: `_margins` archive.
- `thr_3v2sayig2x`, **Understand margins CLI state sync**: earlier replication
  exploration, distinct from the single-authority design selected here.

## 4. Addressing and path ownership

The canonical address is `(instance_id, workspace_id, session_id)`. Human SSH
aliases and workspace names are selectors; the handshake returns stable identity.
A Workspace selection is resolved on the selected instance.

### Resolution order

1. Select instance: explicit `--remote` or `--local`; otherwise `MARGINS_REMOTE`;
   otherwise local. `--local` suppresses the environment default. Reject using
   `--local` and `--remote` together.
2. Select Workspace: explicit `--workspace`, then `MARGINS_WORKSPACE`.
3. For remote operation, require a Workspace selector. Do not infer a remote
   Workspace from the client's cwd or create one as a fallback.
4. Resolve and authorize the Workspace on the instance. Unknown Workspace fails
   before starting capture. An instance switch never triggers local fallback.
5. For local operation without an explicit selector, retain existing discovery
   compatibility. For local operation with an explicit Workspace, that Workspace
   owns capture too; fix the current interactive dispatch discrepancy.

`--project` combined with remote operation is an error. With an explicit local
Workspace it is also ambiguous and must be rejected. Preserve legacy project
behavior only when no Workspace was explicitly selected.

| Input or artifact | Resolution rule |
| --- | --- |
| Remote session storage | Server's resolved Workspace and capture binding |
| Recall source paths | Server's declared Sources |
| Distilled note output | Agent's local notes folder and existing distillation instructions |
| Note association | Logical Source ID plus Source-relative path; resolved per machine |
| Local import file or accompanying memo | Client cwd, opened and uploaded by client |
| Download/export destination | Explicit client path, resolved on client |
| Pending capture data | Private client application storage |
| Template selected by ID | Server Workspace template; explicit local template requires upload |
| Cwd metadata | Optional diagnostic context, never routing or authorization input |

Do not translate `/Users/person/obsidian/inbox` into `/home/example/obsidian/inbox`.
The remote Workspace already knows its paths. It may use existing Obsidian or
other folder synchronization to obtain its Sources and distribute generated notes.
Source identity derived from server absolute paths can remain unchanged in this
phase: there is only one authority. Relocation-stable source identity is a separate
requirement for moving or replicating the authority, not a prerequisite for access.

Create one shared capture-store resolver. It must honor declared capture Sources,
use the Workspace-managed captures directory for new capture when unambiguous,
and reject multiple possible writable destinations. It must not silently create a
new empty store while an existing bound capture store contains the user's sessions.

## 5. CLI and TUI contract

All commands below are proposed additions or extensions.

```sh
# Save repeated connection selection in the shell if desired.
export MARGINS_REMOTE=ssh://beelink
export MARGINS_WORKSPACE=obsidian

margins new --title "Customer conversation"
margins attach <session-id>
margins current
margins recent
margins transcript latest --format json
margins artifacts latest
margins recall "What remains open?"
margins transcribe ./voice-memo.m4a --name "Walking reflection"

# Proposed recovery surface.
margins transfers list --json
margins transfers retry <transfer-id>
```

Remote `note` handoffs preserve instance and Workspace selection for Margins calls,
resolve latest session before recall, and use existing skill behavior. Ordinary
file reads/writes use the agent's local synced notes. There is no `note publish`
command in this feature. An unavailable Source or unfinished transcript is reported
explicitly. An optional application operation links the written note to its session.

Supported first remote command set: `new`, bare attach behavior, `attach`,
`current`, `ls`, `recent`, `rename`, `transcript`, `artifacts`, `recall`, file import,
processing status, memo read/update, note handoff, and note association. `recent --all`
means authorized Workspaces on the selected instance only. Mutations always name
one Workspace. Unsupported commands fail with `remote_command_unsupported` before
performing any local work. Setup, connection credential management, model install,
workspace plan/apply, source mutation, archive toggles, and retention management
are initially administered on the server using existing commands.

### User-visible capture states

Before recording, show: `Recording on this Mac · Saving to beelink / obsidian`.
Show actual negotiated sources: `Microphone + system audio` or `Microphone only`.

During capture distinguish:

- Recording, delivery current.
- Recording, connection lost, saved locally pending upload.
- Paused, with pending upload count/bytes when applicable.
- Recording stopped, upload pending.
- Saved to remote Workspace, processing pending/running/failed/complete.

`Saved` requires server durable acknowledgement of all finalized input. Processing
completion is a separate fact. Ctrl-C stops local capture and seals the local
manifest; a second interrupt may exit while preserving queued delivery. Signal
and crash recovery must never delete pending files. Normal successful command
completion returns zero only after remote input finalization; exiting with pending
delivery returns a documented distinct nonzero code and transfer ID.

Initial connection and authorization are required to start a new remote session.
Continuing an already reserved session through network loss is supported. Starting
a brand-new session completely offline is deferred.

### Session ownership and current selection

`new` reserves a stable session ID remotely before activating native devices.
Device failure marks the reservation aborted/failed with no fabricated successful
capture; empty reservations are excluded from ordinary `recent` results.

One capture producer owns a session at a time. Other clients may observe or edit
the memo with revision checks. Bare attach uses a client-instance-workspace current
pointer; it must not accidentally attach to a different client's latest recording.
The server stores an authorized client-scoped current pointer. `latest` is a query
over the Workspace's sessions, distinct from `current`. IDs are returned by create
and pinned for the full capture, processing, and note workflow.

## 6. Rust architecture

```text
Local CLI/TUI/desktop          Remote CLI / BB bridge / iOS
  | typed in-process calls      | authenticated wire client
  | direct native audio sink    | SSH tunnel / local IPC / HTTPS
  |                             | server adapter
  +-----------------------------+
                           |
                shared WorkspaceService semantics
           / session commands / queries
           / application: note associations, processing jobs
                           |
              canonical repository adapters
               + durable input/event/outbox log
                           |
               media and processing workers
                           |
             existing session store + artifacts
             Workspace ledger + recall index
```

Use one session control interface, a concrete application module, and transport
adapters. Do not add a trait or crate for every operation family. The BB branch
already contains `LiveRuntime::execute/snapshot`; evolve or fold that seam into the
session module instead of introducing a parallel `CaptureSessionService`.

- Session control owns producer lifecycle, timed memo mutations, input closure,
  bounded snapshots, receipts, and event replay.
- Application modules own note associations, processing jobs, and retention.
  A concrete coordinator composes them; repositories need traits only for a real
  alternative implementation or a meaningful test seam.
- Native recorder, browser media, binary ingress, and SSH/HTTP adapters supply
  platform and delivery capabilities. Audio bytes bypass ordinary control RPC.

Retain the meeting runtime's tested ordering, digest, discontinuity, and finalize
rules. Its current whole-session load/replace storage interface is not the production
persistence design: it includes accumulated audio and command/event history. Refactor
it to bounded reads and transactional changes before adopting it as authority.
Remove superseded control interfaces and `DesktopLive*` compatibility aliases.

Capture, delivery, transcription, note association, and processing are independent
facts. The UI derives a display from them. A stopped producer may still have pending
delivery and unsaved memo edits; durable audio may coexist with failed transcription.
Do not replace these facts with another universal session `done` flag.

Requests carry resolved Workspace context and authenticated principal information.
Wire requests contain selectors and IDs, never a caller-supplied server disk path.
Local in-process and remote adapters implement the same application semantics.
CaptureProvider stays client-side. Its audio callback enqueues bounded work to the
selected audio sink and never waits for SSH, HTTP, a database transaction, or ASR.
Local execution selects the existing durable recorder sink. Remote execution
selects the resumable spool/upload sink.

### Local fast path and adapter selection

Select execution mode once during command/session initialization. Keep that choice
outside sample loops. Share validation, lifecycle, memo reconciliation, and query
semantics as typed Rust functions; serialization belongs only at process/network
boundaries. A service is an application boundary, not necessarily a daemon.

| Client and destination | Execution path |
| --- | --- |
| Local CLI/TUI/native desktop, no service owning that store | In-process service/repository; existing recorder sink |
| Local client, store already owned by a running service | Reuse local IPC/control connection; native capture uses a bounded local streaming adapter |
| BB plugin and Margins on the same host | Existing authenticated BB bridge plus local IPC/loopback service |
| BB plugin targeting another host | Same plugin operations through configured remote service adapter |
| Native CLI targeting another host | SSH/HTTPS adapter plus resumable delivery |
| Phone | HTTPS intake/capture adapter |

Store ownership discovery must be local and lightweight. It cannot probe remote
hosts or launch a daemon during ordinary local invocation. If an existing service
owns the store, reuse its coordination rather than bypassing it. Local IPC has a
real cost; measure it separately and never claim it is identical to in-process
execution. The default standalone path remains in-process.

Local capture must not create a second upload copy of audio, calculate transport
digests per chunk, encode/decode JSON or base64, wait for remote acknowledgements,
or persist remote replay envelopes merely to satisfy a shared abstraction. Keep
existing durability guarantees; reuse canonical recorder files directly. Remote
receipt/retry state is allocated only for clients that require delivery recovery.
When a local IPC client needs crash-safe delivery, reuse its recorder artifact as
the recovery source where possible rather than maintaining duplicate full audio.

Local queries go straight to the application/repository implementation and return
typed results. Do not wrap local recall in an HTTP request or JSON round trip.
The shared facade must not serialize independent reads behind slow ASR, uploads,
or a global transport mutex. Use existing bounded audio buffers and ownership
transfer; avoid additional per-sample allocation or copying.

Dependencies for SSH/HTTP, reconnect timers, and transfer workers must be lazy or
feature-gated so normal local startup does not initialize them. A typed adapter
enum or narrow trait is sufficient; an RPC framework is not required internally.
Use contract parity tests to keep adapters equivalent instead of making local
calls traverse the wire implementation.

Reuse public protocol DTOs and state-machine invariants. Extend them only for
missing operations, with explicit version identification and clear rejection of unsupported clients. Keep workspace
queries and application operations outside the audio command enum unless the existing branch
has already established a shared envelope. Rust-generated fixtures must validate
the BB client's schemas. Do not maintain hand-written competing session models.

Likely implementation locations:

- `src/cli.rs`, `crates/public/margins-cli/src/{args,lib,services}.rs`: selection,
  dispatch, client orchestration, and command handoffs.
- `crates/public/margins-workflows`: shared Workspace service orchestration and
  capture destination resolution.
- `crates/public/margins-meeting-protocol`: reusable wire types and negotiation.
- `crates/public/margins-meeting-runtime`: existing command-state invariants.
- `crates/public/margins-store`: transactional canonical persistence adapters.
- Existing native capture/media adapters: spool producer integration.
- `desktop/src-tauri/src/server`: initial network adapter and headless composition.
- BB integration worktree: client transport and authenticated Workspace routing.

Extract a new crate only when dependency direction requires it. Public protocol,
service contracts, and portable persistence must not acquire Tauri/CoreAudio/ASR
dependencies. Native capture and platform processing remain capability-gated.

## 7. Transport and service lifecycle

### SSH

Use the system SSH client and its configuration, agent, ProxyJump, and known-host
handling. Parse `ssh://[user@]host[:port]`; reject passwords, URL query strings,
fragments, shell syntax, and option-like host values. Support SSH config aliases.

Proposed bootstrap: invoke the fixed command `margins serve discover --json` on the
host. It returns protocol versions, stable instance identity, loopback endpoint,
and a short-lived scoped credential over SSH. No user data is interpolated into a
remote shell command. Workspace selection travels through the API afterward.

Create a loopback-only SSH forward with exit-on-forward-failure and server-alive
checks. Do not request a PTY. Use the ordinary HTTP/event client through that
forward. Credentials stay in memory or private IPC, never URL parameters, argv,
logs, or rendered output. Clean up the forwarding child on normal exit; restarting
the client reconstructs the connection from its pending transfer manifest.

The service is long-lived so processing survives CLI exit. `new` does not install
or start a persistent service implicitly. Discovery fails clearly if it is absent,
with the explicit provisioning command. Provisioning must reuse the instance's
service and store lock; it must not start one independent database owner per SSH
connection. Start with explicit server launch and add supported OS service setup
as a separately documented operation.

### HTTPS and browser access

The same API is available behind trusted HTTPS for BB and iOS. Plain HTTP is
allowed only on loopback for SSH/local use. Remote exposure requires explicit
configuration and scoped credentials. Tailnet reachability does not replace
application authorization.

Build a service router separate from the current desktop SPA router. Do not expose
token-injected HTML, generic arbitrary invoke commands, or bearer tokens in event
URLs. Browser integration can proxy through the authenticated BB host service;
direct browser access requires an origin allowlist and authenticated streaming.

Discovery negotiates supported protocol versions and capabilities, including
capture formats/lanes, maximum chunk size, event replay limits, import formats,
ASR availability, recall mode, and supported application operations. Unsupported required
capabilities fail before device start; optional missing processing capability may
allow capture with an explicit `saved; transcription unavailable` outcome.

## 8. API surface

The table describes required capabilities and illustrative route mappings, not a
requirement for one endpoint per operation. Reconcile with the BB branch and reuse
typed command/query envelopes where practical. Separate binary audio/file transfer
and events where their transport needs differ. Application operations can share
the same transport while delegating to their own modules. Use one contract and update first-party callers together. Remove superseded routes
and generic-invoke mutation implementations at cutover; backward compatibility is
not an acceptance requirement.

| Operation | Proposed endpoint | Contract |
| --- | --- | --- |
| Negotiate | `GET /v1/capabilities` | Identity, versions, limits, capabilities |
| Workspace read | `GET /v1/workspaces/{w}` | Authorized summary and source freshness |
| Create session | `POST /v1/workspaces/{w}/sessions` | Idempotent reservation; session + producer token |
| Read/list | `GET /v1/workspaces/{w}/sessions[/{s}]` | Stable IDs, state, revisions, pagination |
| Mutate session | `POST /v1/workspaces/{w}/sessions/{s}/commands` | Typed versioned commands and receipts |
| Audio ingress | `PUT /v1/workspaces/{w}/sessions/{s}/segments/{g}/lanes/{l}/chunks/{n}` | Binary bytes + validated metadata/digest; durable ACK |
| Events | `GET /v1/workspaces/{w}/sessions/{s}/events?after={cursor}` | Authenticated SSE/replay, ordered event envelopes |
| Memo read/update | `GET/PUT /v1/workspaces/{w}/sessions/{s}/memo` | Canonical document and expected revision |
| Transcript | `GET /v1/workspaces/{w}/sessions/{s}/transcript` | Existing transcript semantics, explicit readiness |
| Artifact bytes | `GET /v1/workspaces/{w}/artifacts/{id}/content` | Authorized stream/range, digest and media type |
| Recall | `POST /v1/workspaces/{w}/recall` | Existing evidence contract with Source-relative references for local reads |
| Note association | Application command/query envelope | Link/unlink/list session note references; no file writes or completion side effects |
| Processing jobs | Application command/query envelope | Request/inspect/retry/cancel supported processing independently of note links |
| File intake | `POST /v1/workspaces/{w}/imports` | Multipart original audio, metadata, stable upload key |
| Intake status | `GET /v1/workspaces/{w}/imports/{id}` | Durable receipt and processing state |

There is no source-file `evidence/read` or note-content publication endpoint in
this scope. Margins artifact content access remains necessary because transcripts
and recordings may exist only on the selected instance. Ordinary notes are assumed
to be synced and are read/written by the agent locally.

Mutations use immutable request IDs; same ID plus same content returns the
original receipt, and changed content returns a conflict. Revision-dependent
operations include expected revision. Request identity is scoped by instance,
Workspace, principal, and operation where appropriate. Client-generated IDs are
identifiers, never authentication.

Map errors consistently: unauthenticated 401, forbidden 403 (or non-disclosing
404 for out-of-scope objects), missing 404, conflicting revision/identity 409,
expired replay cursor 410, oversized body 413, unsupported media 415, invalid
command 422, throttled 429, unavailable capability/service 503. Return a structured
code, retryability, request ID, and safe explanation. Rate-limited responses include
retry guidance. Clients must inspect the body; HTTP success is not an audio ACK.

Queries use paginated stable cursors. A request receipt includes the committed
revision so a subsequent query can request read-after-write consistency. Event
cursor expiry requires an authoritative snapshot and explicit reconciliation;
clients must not silently skip missing events.

## 9. Local capture, timing, and delivery

### Audio and time

Preserve native mic/system separation and existing recorder quality. A lane is an
audio source, not necessarily a person. System audio may contain several speakers;
existing diarization remains a processing option.

Negotiate the format before capture. Preserve existing sample rate/channel
information, declare any conversion, and avoid introducing lossy encoding as a
default merely to simplify transport. Server workers reconstruct the existing
canonical audio/session format without collapsing source lanes. Start with bounded
binary PCM chunks for native capture; finished compressed files use import.

Use the creator's monotonic timeline with sample offsets. Wall-clock time is
metadata; network arrival time never assigns audio or memo offsets. Each segment
declares lane formats and timing; sequence numbers are per segment/lane. Pause and
resume follow existing session timeline semantics and explicitly close/open
segments where required. Device clock drift/resampling must be measured and gaps
represented explicitly rather than patched with invented timestamps.

### Durable local spool

This subsection applies to capture delivered to another authority. Ordinary
in-process local capture retains its canonical recorder files and existing crash
recovery, without a separate transport spool.

Store private pending data under the platform Margins application data directory,
outside the user's note folder. Key it by stable instance, Workspace, session,
segment, lane, and sequence. Persist a manifest with negotiated format, digests,
pending commands, memo revisions/operations, server identity, and acknowledgement
watermarks. Credentials are referenced securely, not embedded in manifests.

Write chunk -> flush durable local bytes -> record durable manifest entry -> send.
On recovery, reconcile files and journal entries before resend. Reuse exact IDs
and bytes. Memory queues are bounded; callbacks report overflow explicitly. If
spool writes fail or disk reaches the configured reserve, stop capture safely and
surface the exact lost span. Never silently discard pending audio to make space.

Server ACK permits local reclamation only when original input can be recovered
from durable server storage after a restart. Pending data has no automatic expiry.
Provide list/retry/export and explicit confirmed discard operations. No background
delivery daemon is required for phase one: after CLI exit, recovery commands or
the next client launch resume delivery visibly.

### Delivery and finalization

Reuse the protocol's cumulative/out-of-order acknowledgement semantics. Chunk
retries with the same identity/digest are no-ops; different bytes conflict.
Close names exclusive lane boundaries. Finalize names the exact segment closes
and succeeds only after their complete durable coverage. Any discontinuity is
retained as a quality warning; finalization does not imply gap-free capture.

On reconnect, validate the stable instance identity before replay. An endpoint
now serving a different instance cannot receive pending data without explicit
rebind. Retry with bounded exponential backoff and jitter. Authentication failures
retain pending data and stop retries until credentials are repaired.

## 10. Persistence and workers

The service owns all remote mutations. Local administrative commands on the host
must either use that service or acquire the same operation/store coordination.
Avoid a background service and legacy CLI independently rewriting memo or session
files without revision checks.

Replace the current `MeetingRuntimeStorage` whole-session load/replace contract
with transactional changes and uniqueness constraints. Store immutable payloads
separately; chunk metadata records identity, digest, location, and receipt. Command
receipts and event history must also be read incrementally rather than loaded with
every chunk. A blob reference alone does not solve growing command/event snapshots.

Each commit touches the affected session/lane state, chunk or command receipt, and
new events/outbox records. Concurrent commits retain revision/conflict checks. Keep
atomic receipt publication: a timeout is not evidence that a mutation failed.
Bound current snapshots and paginate history. Verify allocations, bytes read/written,
and transaction work across increasing session sizes; per-chunk work must not scan,
clone, or rewrite accumulated audio. Local capture reuses canonical recorder files
without transport hashing or a duplicate spool.

Crash-consistent audio ingestion:

1. Validate authentication, Workspace, producer, lane, bounds, format, digest.
2. Write an immutable blob to a confined staging path, flush it, atomically rename
   into final storage, and flush the containing directory where supported.
3. Commit chunk metadata, deduplication identity, event, and worker outbox entry
   transactionally.
4. Return the durable acknowledgement only after the commit.

A crash before step 3 may leave an orphan blob, cleaned after a grace period. A
committed blob reference must never point to missing unflushed content. Recovery
checks this invariant. Final canonical audio assembly is similarly atomic; retain
acknowledged chunks until assembly and registry publication are durable.

Choose one canonical persistence owner during phase one. CLI, desktop, and the API
read and mutate the same session IDs, memo, artifacts, and revisions. Consolidate
legacy path-scoped store calls, public aggregate repository writes, and web capture
persistence; remove duplicate representations and synchronization triggers maintained
only to support competing writers. Receipt, event, and job tables may remain in the
same database. Preserve existing data through an explicit conversion if needed.
Document and test recovery across database/filesystem boundaries.

Processing workers claim durable jobs with leases, deduplicate by input revision
and processing options, and publish results through existing workflows. Capture
does not await ASR or note generation. A Linux receiver may save audio without a
native ASR capability; expose that state and allow explicit supported processing
later. Do not promise Mac CoreML on the server unless it actually has it. Optional
processing on another trusted machine is deferred to a separate worker contract.

Reuse existing retention rules and explicit preview/apply. A server receipt means
durable storage on that instance, not an independent backup. Instance backup must
include canonical metadata, acknowledged blobs, and pending jobs consistently.

## 11. Memo editing and distillation

Reuse `TimedMemoDocument` from commit `5a703dc5a` on the BB branch; commit
`777e45922` also uses it for hosted notepad reconciliation. Confirm branch integration
before editing so this existing extraction is not reimplemented. Canonical line IDs, timing, split/join behavior,
and revision checks belong to Margins.

The local TUI records timing at user action time on the capture timeline. During
network loss, persist edits with stable operation IDs and their observed base
revision. The server validates and reconciles them using the same timed document
rules. It must not timestamp a delayed upload as though the user just typed it.
The wire seam may need a trusted capture-timeline edit form in addition to the BB
whole-notepad form; both must reduce into one canonical document.

Concurrent stale edits return the current revision/document and preserve the local
draft. Never silently overwrite another client's edits. Phase one can require the
user to reconcile a stale whole-document edit; automatic collaborative text merging
is outside scope.

For distillation on a client-side agent:

1. Resolve latest session on the selected instance/Workspace, then pin its ID.
2. Fetch transcript, memo, and artifact metadata through the API.
3. Query remote recall if useful. Resolve returned Source-relative references
   against the locally synced Source and read notes with native agent tools.
4. Generate and write the note locally using the existing skill and chosen provider.
5. Optionally associate its Source-relative reference with the Margins session.

Recall continues to enforce the server's declared Sources and exclusions. Return
Source identity and relative path alongside existing evidence fields; update the
first-party callers together. A server absolute path alone cannot identify the client's
copy. Local Source-root mapping belongs to the agent/client environment and must
not become another Workspace setup protocol. If a Source is not available locally,
report that limitation rather than falling back to arbitrary remote file reads.
File sync may lag: expose an indexed content hash/revision when available, and
have the agent read the actual local contents before citing them. Do not silently
assume the server's indexed excerpt matches the local file.

Handoff text retains remote selection for Margins commands even in a fresh shell,
and identifies the local notes root for filesystem work. Local temporary downloads
of Margins artifacts never become the canonical artifact by accident.

### Application responsibilities

**Note associations** store a session ID, logical Source ID, and relative note path,
with an optional observed content hash. Link/unlink operations are idempotent and
revision-checked. They perform no file writes, clear no processing errors, finish
no jobs, remove no drafts, and delete no audio. Validate Source membership and path
syntax, rejecting absolute paths and traversal. Resolve paths with confinement
checks whenever the application later opens them.

A linked note may not yet exist on the server because file sync is delayed. The
application derives availability when resolving it and can display "Note not
available on this device yet." Do not introduce a persisted sync/completion state
machine for this. An observed hash describes what the client linked; it does not
freeze future edits or make an absent file a successful processing result.

**Processing jobs** describe work performed by Margins: operation, input revision,
progress, result references, and failures. Their coordinator owns transitions,
retry/cancellation, and job-specific draft/refinement handling. An external agent
writing a note need not create a synthetic Margins job. A session may have saved
audio, an available previous note, and a failed later distillation job at once.
Present those facts without forcing them into one session `done` flag.

**Retention** independently determines eligibility and executes explicit removal
through the existing preview/apply workflow. A note link or job-completion event
alone never invokes deletion. Carry forward the applicable eligibility rules, but remove automatic audio deletion
from note-link and processing-completion paths. This intentional behavior change
must be documented in release notes; deletion runs only through retention apply.

For app-managed distillation, the application coordinator runs a job, saves through
the existing local writer, links the resulting note, and completes that specific
job. It may expose newly eligible retention work. This orchestration stays shared
across desktop, CLI, and BB and is not built into capture commands or database
setters. Modules and tables can remain in one process/database; do not create
additional deployed services merely to separate these responsibilities.

## 12. BB integration

The plugin is a first-class consumer of the service implemented here. Its transport
must be functional as part of delivery, not deferred as a hypothetical future API.
Local-host operation is its default; selecting a remote instance changes the host
adapter, not the UI's session/memo model. In BB, "local host" means the selected
project's host, which may differ from the device displaying the browser. Display
capture origin and save destination explicitly when they differ.

### Plugin contract coverage

During phase 0, inventory every plugin RPC, event, schema, and lifecycle action in
the current integration branch. Map it to the shared service and corresponding
wire operation. Check this mapping into the implementation change. Required coverage:

| Plugin behavior | Shared service requirement |
| --- | --- |
| Resolve project and initialize | Explicit instance/Workspace mapping, discovery, capabilities, compatible startup |
| Start recording | Session reservation, producer authorization, capture-source negotiation |
| Deliver browser/native audio | Format-specific ingress adapter into canonical capture workflow |
| Pause/resume/stop | Typed retry-safe commands, durable input finalization, processing status |
| Edit notepad | Shared timed memo, expected revision, conflict and receipt handling |
| Reconnect or remount panel | Snapshot plus ordered event replay; active producer retained |
| Window heartbeat/disconnect | Producer policy/lease enforcement independent of panel mount |
| Show transcript/note/artifacts | Shared query APIs and authorized content retrieval |
| Link an agent-written note | Application note association; ordinary note contents remain in the synced filesystem |
| Show processing outcomes | Application job queries/events; note association is not job success |
| Add session context to an agent | Stable instance/Workspace/session reference and remote-aware handoff |
| Cancel/discard/recover, where currently supported | Existing retention and recovery semantics; distinguish abandoning an empty reservation from deleting saved data |

The endpoint table in section 8 is a base inventory. Add missing typed operations
identified by this audit to the same contract before calling plugin integration
complete. Do not retain a separate generic-invoke mutation path as the plugin's
permanent implementation. Update current clients to the same service and delete replaced routes; do not
retain a compatibility dispatcher.

Keep audio streaming off ordinary JSON plugin RPC when the bridge supports a
binary stream. If the current BB bridge imposes framing/size constraints, document
the supported route, limits, and measured overhead, and implement a bounded adapter.
Never route audio through an agent conversation or encode each sample as JSON.
Control RPC, audio delivery, and event streaming may use different channels while
sharing authorization, IDs, and application semantics. Subscribe once per client
owner and fan out snapshots to panels; remove per-panel polling and connections.
The owner retains the latest revision/cursor and local drafts. Reject stale responses
and fence callbacks by session/operation generation. Use at most one heartbeat and
one refresh in flight per owner, with deadlines covering response bodies as well as
headers. Cancelling a request does not imply rollback; reconcile its durable receipt.

Pause and Stop act on the local audio producer immediately. Neither waits for memo
persistence, network acknowledgement, upload drain, or ASR. Queue the corresponding
retry-safe command and report local versus acknowledged state truthfully. Keep an
unsaved draft recoverable when saving fails. Stop acknowledgement, input delivery
completion, and processing completion remain separately observable.

The service is the authority for producer lease expiry and takeover. The browser
owner independently enforces its local microphone-stop policy; BB routing/storage
must not implement a competing expiry loop or independent durable saved flag.
A server timer cannot release a disconnected browser's microphone. Conversely,
a browser timeout cannot prove the server failed to commit.

Same-host native capture should write through the local recorder/service path;
the plugin consumes control/status events. It should not send audio to the browser
and back to the same host. Browser microphone capture necessarily crosses the
browser boundary; preserve its existing streaming path without adding an extra
remote relay when the project service is local. Capture on the user's Mac with a
different project host still uses the remote delivery adapter.

Keep the plugin's current project-selection experience. The BB host adapter maps
the selected project to an explicit Margins instance and Workspace, then calls the
same API. Different projects must not all collapse into the same accidental cwd
store. Explicit user mapping may deliberately share one Workspace.

Browser capture uploads microphone audio. Native Mac mic/system capture requires
a bridge on the user's Mac; reaching a remote BB host does not grant access to
the Mac's devices. Reuse the local capture producer and delivery contract when that
bridge exists. Never enable system-audio UI based solely on the server's OS.

Preserve capture ownership above panel lifetime. Switching threads or closing a
panel leaves the active client owner intact. Existing browser policy stops capture
after its window disconnects beyond a negotiated grace period; a native TUI may
continue spooling offline. Model these as producer policies, not as a universal
server timeout that discards input. After grace expiry the server marks the producer
interrupted and blocks takeover until explicit recovery; it cannot prove missing
audio was saved. Native late recovery must remain possible with the producer token.

The server authenticates the principal and issues an unguessable producer token
scoped to the session. A browser-generated window ID is useful coordination
metadata but does not authorize capture ownership. Stop, pause, resume, and memo
changes must be retry-safe. Stop receipt and pending-processing status remain
visible after panel remount.

## 13. iOS Shortcut intake

Ship a small client of the same service:

`Voice Memos -> Share -> Send to Margins -> upload -> Received by workspace`.

The Shortcut sends the original compressed recording as multipart data, optional
title/memo, an upload identity, and its paired Workspace. It waits for a durable
receipt, not transcription. Server intake stages and verifies the full file,
registers a canonical imported session, and queues existing processing workflows.
Processing failure never requires a second upload.

Issue a revocable upload-only credential bound to one Workspace. It can inspect
receipts for its own uploads and cannot list sessions, read recall, mutate policy,
or fetch unrelated audio. Shared Shortcut templates contain no credential;
pairing fills installation-specific values privately.

Retried requests reuse an upload ID. Independently repeating Share may generate
a new ID; content hashes can flag a likely duplicate but must not globally suppress
intentional repeated imports. Record origin and original filename as metadata;
never use the supplied filename directly as a server storage path.

Incomplete uploads do not create successful sessions. Apply streaming body limits,
quotas, bounded staging cleanup, and media validation before invoking decoders.
The receipt says which Workspace received the file and whether processing is
queued or unavailable. The Shortcut cannot promise reliable continued upload after
lock/app switching without device testing. A later native share extension can
use resumable intake with the same session/receipt model.

## 14. Authorization, limits, and operations

- Scope every read/write by authenticated principal and Workspace, including
  artifact ranges, recall, note associations, jobs, event replay, and receipt lookup.
- SSH discovery grants only what that OS user is authorized to access. HTTPS
  credentials are separately revocable and scoped.
- Verify instance identity on every reconnect and retain SSH host-key checking.
- Initial defaults should be explicit and capability-advertised: maximum in-flight
  chunks, bytes per request, concurrent uploads, import size, and spool reserve.
  Measure representative recordings before finalizing numeric defaults.
- Reserve bounded capture ingestion capacity so large imports or slow recall/ASR
  cannot starve audio delivery. Backpressure retains client data.
- Logs contain request/session IDs, byte counts, lag, retries, and error codes;
  exclude credentials, audio, memo contents, and retrieved note text by default.
- Expose health for storage availability, upload backlog, processing backlog,
  last successful receipt, and source freshness. Separate service reachability
  from readiness for native transcription or hosted recall.
- Service shutdown stops new reservations, drains committed transactions, preserves
  pending jobs, and closes transports so clients retain unacknowledged input.

## 15. Failure contract

| Failure | Required behavior |
| --- | --- |
| Wrong/unknown Workspace | Fail before capture; no local fallback or auto-create |
| Server unavailable at start | No capture starts; actionable error |
| Local permission denied | Reservation marked failed; no successful empty session |
| Network lost mid-capture | Native spool continues within disk bounds; visible pending state |
| ACK lost after commit | Exact retry returns durable receipt; one chunk/session |
| Client crash | Local queue recovers; no automatic device restart |
| Server crash | Committed audio/events/receipts survive; jobs resume |
| Disk full locally | Safe capture stop, retain pending bytes, explicit gap if any |
| Disk full remotely | No false ACK; client retains pending bytes |
| Another producer attaches | Conflict until current producer explicitly releases/recovery resolves |
| Memo revision conflict | Preserve draft; surface current document for reconciliation |
| ASR unavailable/fails | Recording stays saved; processing state remains explicit |
| Linked note missing on server | Association remains valid; show derived unavailability while file sync catches up |
| Note linked while a processing job has failed | Keep the association and job failure independently; no audio deletion |
| Replay cursor expired | Snapshot/reconcile required; no silent skipped events |
| Host alias points to new instance | Refuse pending replay until explicit rebind |
| User closes BB panel | Capture continues under window owner |
| User closes BB window | Client policy stops capture; server reports incomplete delivery honestly |
| Shortcut upload interrupted | No receipt of success; staged partial expires; retry supported |

## 16. Delivery phases and acceptance gates

### Phase 0: integration map

Inspect the live BB branch, timed-memo extraction, protocol/runtime changes, and
canonical session adapters. Produce a short mapping of each existing writer to
its authoritative store. Resolve duplicate authority before adding transport.
Gate: exact files/branches and reuse decisions are recorded; no invented second
session model or unnecessary protocol fork.

### Phase 1: local service parity and Workspace routing

Extract shared resolution/service calls; fix explicit Workspace behavior for
native and public `new`/`attach`. Refactor runtime persistence to bounded transactional changes; implement
snapshot/receipt/event storage and local query/application operations with one writer. Separate
note association from the bundled legacy completion setter; identify existing
consumers of `processing_state = done` and move them to explicit job/session facts.
Gate: local CLI and service see identical sessions and artifacts; explicit
Workspace wins from every cwd; existing local no-selector workflows still work.
Record local performance baselines before refactoring and pass the local fast-path
checks below. Define the complete plugin operation mapping in this phase. Update first-party
callers together at cutover and remove superseded implementations.

### Phase 2: read-only remote access

Add instance negotiation, SSH discovery/tunnel, scoped HTTPS authentication,
recent/transcript/artifact/recall clients, Source-relative references, and preserved
agent handoffs using local filesystem tools for ordinary notes.
Gate: read-only remote CLI works from a machine with no notes folder; authorization
and endpoint-version failures have no local side effects.

### Phase 3: native capture and durable delivery

Add local spool, producer ownership, lane upload, memo timing, pause/resume,
close/finalize, recovery commands, processing jobs, and optional note associations.
Gate: a deterministic two-lane recording survives client/server restart and network
loss and yields exactly one canonical session, correctly timed memo, and note.

### Phase 4: BB client adaptation

Route the plugin through the shared service, retain project mapping and window
ownership, and run its existing real browser journey. Integrate native capture
only when the local bridge and permission flow are verified.
Gate: record/edit/pause/reconnect/stop/panel-remount produces a session visible to
the remote CLI. Test browser mic and native system audio as separate capabilities.
Run the complete available plugin journey against both same-host and remote service
adapters, including agent handoff, artifact access, recovery, and revision conflicts.
Plugin integration is a release gate for this design. Native bridge capability may
remain unavailable only when explicitly reported; no unimplemented transport or
unmapped required plugin operation may be described as supported.

### Phase 5: Shortcut intake

Add file intake/receipts, scoped pairing, template instructions, and original-file
processing. Gate: a real phone can share a recording, receive a truthful receipt,
and find it through the same CLI/BB workspace. Interruption behavior is documented
from real-device evidence, not assumed from the HTTP API.

### Phase 6: release and operations

Package a compatible server/client pair, document explicit service provisioning,
backup/recovery, credential revocation, and unsupported remote commands. Test the
published installation path and source availability on the actual server platform.
Gate: a clean client with no preexisting Margins state completes the workflow.

## 17. Verification plan

Run portable tests through `scripts/cargo-lane disposable` following AGENTS.md.
Native audio tests require the explicit platform lane. Real model calls and native
installation/device tests follow the repository's existing authorization and
coordination rules. A documentation-only change does not require Cargo builds.

Required behavioral tests:

1. Resolution matrix: explicit local/remote Workspace, cwd in home/inbox/unrelated
   folder/no vault, environment overrides, conflicting project, missing Workspace.
   Assert exact store ownership and absence of unexpected `.margins` creation.
2. Lane fidelity: distinct deterministic signals on mic/system lanes; sequence,
   sample count, timeline, channel identity, and final artifact reconstruction.
3. Delivery faults: duplicate/out-of-order chunks, ACK loss, conflicting digests,
   lost close/finalize requests, reconnect, and retry with unchanged IDs.
4. Crash injection at every spool, blob, transaction, receipt, assembly, and note
   registration boundary. Assert no acknowledged data loss or duplicate session,
   note association, or job result.
5. Large-session scaling: memory and per-chunk transaction work do not grow with
   accumulated audio size; slow processing does not block capture acknowledgements.
6. Timing: delayed memo delivery and network jitter do not shift memo timestamps;
   pause/resume and device discontinuities retain explicit timeline semantics.
7. Ownership: two clients, simultaneous attach, BB panel remount, expired producer,
   revoked credentials, interrupted native recovery, and takeover attempts.
8. Authorization: cross-workspace/session artifact reads, event replay, forged
   producer IDs, upload-only tokens, source exclusions, traversal/symlink escape.
9. Read consistency: imported/native/BB sessions produce equivalent query results;
   stale evidence revisions and unavailable source freshness remain visible.
10. Distillation: remote latest pinned before recall, Source-relative references
    resolve on the client, agent reads/writes local notes, and optional note links
    survive server-side sync delay without a synthetic completion lifecycle.
11. Protocol contract: the current client/server pair, unknown versions, oversized
    payloads, expired event cursors, and Rust-generated BB fixtures.
12. Real Mac-to-Linux run: capture lanes local, audio durable remote, capabilities
    truthfully report available processing, and local cwd has no routing effect.
13. Real Shortcut run: successful share, repeated request, app switch/phone lock,
    broken connection, invalid credential, and processing failure after receipt.
14. Application separation: link/unlink does not write note bytes, clear errors,
    finish jobs, remove drafts, or delete audio. A saved session, linked note, and
    failed processing job coexist correctly. Retention runs only through its
    explicit workflow. Shared application orchestration preserves app-managed
    distillation and refinement behavior without exposing bundled legacy setters.

### Local performance regression gate

The target is no measurable regression in default local operation attributable to
optional remote support. Do not promise literal zero CPU overhead without evidence.
Compare before/after release builds on the same host, with the same fixtures and
model/cache state; report cold startup separately from warm operation.

Measure CLI startup, time to recorder readiness, audio callback/buffer pressure and
dropped frames, capture CPU/RSS, disk bytes written, memo edit latency, finalization
latency, and warm/cold recall latency. Use repeated runs and report distributions
and measurement noise. Set the comparison method and permitted noise envelope from
the baseline before examining candidate results; a repeatable regression outside
that envelope blocks rollout until explained and fixed or explicitly reviewed.

Structural assertions complement timing measurements:

- Default local command makes no network request, starts no server or SSH process,
  and initializes no remote reconnect/transfer worker.
- Local capture adds no duplicate audio spool, transport hashing, serialization,
  per-sample allocation, or transport acknowledgement wait.
- Local recall performs no RPC round trip or wire serialization.
- Service-owned same-host IPC is measured as a separate deployment mode against
  its pre-change plugin/service baseline, with no added relay or redundant polling.
- Both adapters pass shared behavior tests; local speed is not achieved by omitting
  revision checks, Workspace boundaries, or existing durability.

Preserve benchmark commands, fixture descriptions, build identities, and results
with the implementation evidence. Broad native build and installed-app measurements
use the repository's coordinated lanes; do not add benchmark work to this doc edit.

Before release, record measured upload lag, finalization latency, local spool growth,
recovery duration, and memory use for a long two-lane session under constrained
bandwidth. Report actual measurements; do not infer production readiness solely
from small synthetic fixtures.

## 18. Cutover and data preservation

Remote mode is opt-in. Preserve local no-selector behavior initially. The explicit
Workspace capture fix is intentional and needs release notes plus regression tests.
Do not move user data automatically as a side effect of `new` or remote discovery.

Inventory legacy `.margins` stores and capture bindings. If a migration is required,
produce a separate inspect/plan/apply path with conflict checks, backups, and stable
session IDs. An explicit conversion tool may read old data; the application does
not need a permanent legacy reader or writer after conversion.
The `_margins` archive remains compatible with external folder synchronization;
it neither supplies remote session authority nor becomes the upload queue.

Backward compatibility is not required. Update CLI, desktop, and BB consumers to
one current contract, and remove old routes, wire aliases, bundled completion
setters, and duplicate persistence interfaces in the corresponding cutover. Do not
build compatibility wrappers or maintain an old/new protocol test matrix.

Capability/version checks still reject incompatible clients clearly. Existing user
data and unacknowledged captures must be preserved: inspect and explicitly convert
stores when necessary, with recovery/export available. Lack of API compatibility
is not authorization to discard data. Old binaries must refuse unsupported schemas
instead of attempting a downgrade.

## 19. Decisions to settle during implementation

These are bounded engineering decisions, not reasons to change the product contract:

- Exact canonical persistence adapter after inspecting the BB branch and legacy
  registry; choose and document one session authority.
- Reuse or extension of its existing timed-memo wire operations for offline edits.
- Server packaging and explicit service installation on each supported OS.
- Initial chunk sizes, queue limits, import quota, replay retention, and disk reserve,
  based on measurement and advertised through capabilities.
- Explicit data conversion for existing note pointers and processing flags into
  independent application records, without keeping competing runtime models.

The architectural invariant is fixed: instance and Workspace select authority;
capture stays on the user's device; durable acknowledgements govern delivery;
all clients read and mutate the same canonical Margins session workflows.
