# BB Live Margins Surface Audit

> Historical design audit. The implemented route names, JSON shapes, and final
> product decisions are documented in `BB_LIVE_MARGINS_PLUGIN_SPEC.md`. In
> particular, the shipped panel does not need or add a `margins live` CLI family.
>
> Implementation update, 2026-09-04: the local HTTP adapter now depends on the
> transport-neutral `margins-live-runtime::LiveRuntime` seam rather than being
> the seam itself. Tauri is the first adapter only. The target owner is a
> CLI-installed background runtime. That runtime is now composed as the
> windowless `margins-live` binary beside the normal `margins` command; the
> earlier menu-bar shell is not part of the product direction.
>
> Implementation update, 2026-09-04: the append-only memo recommendation below
> is also superseded. The panel now presents one editable notepad. It sends
> plain text with an opaque revision; the runtime, not bb, reconciles that text
> with the complete timestamped memo and rejects stale replacement attempts.

This audit treats `desktop/BB_LIVE_MARGINS_PLUGIN_SPEC.md` as the fixed product
boundary. It is report-only: no plugin/runtime implementation is proposed beyond
the smallest public surface needed to support that spec.

## Verdict

The bb live plugin should not bind to the existing Tauri invoke names, desktop
HTTP `/api/invoke/:command`, raw live snapshot files, or `RecordingStatus` shape.
Those are private desktop implementation details.

The smallest durable boundary is a tokened host-local Margins live API, backed by
the `margins-live-runtime` read/command port and versioned DTOs. The current public
CLI/shared crates already provide useful building blocks, especially transcript
JSON, session IDs, capture lifecycle vocabulary, event envelopes, and store
repositories, but they do not expose a stable live capture owner. CLI JSON
commands should eventually compose and own this runtime. One-shot commands may
wrap it for people and skills, but bb's rolling panel should talk to the
long-lived local process rather than repeatedly spawning commands.

## Stable Public Surface Today

The following can be used or reused today without depending on desktop-private
code:

- `crates/public/margins-cli/src/args.rs`
  - `Command::Current`
  - `Command::Recent { all }`
  - `Command::Transcript { meeting_id, format }`
  - `Command::Capabilities`
  - `Command::New { title }`
  - `Command::Attach { session }`
  - `TranscriptFormat::Json`
- `crates/public/margins-cli/src/commands/transcript.rs`
  - `margins transcript [meeting_id] --format json`
  - Emits `TranscriptJson` with `meeting_id`, `body`, `view`,
    `decoded_until_ms`, `committed_until_ms`, `updated_at_unix_ms`, `live`,
    `terminal`, `title`, `started_at`, `created_at`, `calendar_event`,
    `people`, `memo_path`, `saved_note_path`, `transcript_path`,
    `speaker_alias`.
- `crates/public/margins-cli/src/commands/sessions.rs`
  - `read_current_session`
  - `show_current`
  - `.margins/current`
  - This exposes the current session pointer and metadata, but only as a
    command/text behavior today, not a live-runtime resolver.
- `crates/public/margins-cli/src/commands/capabilities.rs`
  - `margins capabilities --json`
  - Useful for broad availability reporting, but it has no live-capture section.
- `crates/public/margins-cli/src/error.rs` and `src/output.rs`
  - `CliError`
  - JSON error envelope:
    `{"schema_version":"margins.error.v1","ok":false,"error":...}`
  - This only appears when the command path has opted into JSON output.
- `crates/public/margins-cli/src/services.rs`
  - `CliServices`
  - `SessionStore`
  - `standalone_services`
  - Good dependency-injection shape, but default public capture is unavailable.
- `crates/public/margins-core/src/capture.rs`
  - `CaptureProvider`, `CaptureHandle`, `CaptureObserver`
  - `CaptureRequest`, `CaptureCommand`, `CaptureAction`
  - `CaptureSnapshot`, `CaptureState`, `CaptureLaneSnapshot`,
    `CaptureLaneHealth`
  - `CaptureCapabilities`, `CaptureError`, `CaptureErrorCode`, `RetryAdvice`
  - Reusable vocabulary for extracted live host state.
- `crates/public/margins-core/src/session.rs`
  - `SessionLifecycle`, `SessionRecord`, `SessionSummary`, `SessionRepository`
  - Useful lifecycle model, but not wired to desktop live recorder state.
- `crates/public/margins-core/src/event.rs`
  - `EventEnvelope`, `EventKind`, `Event`, `EventSink`
  - Existing event kinds include `capture.started`, `capture.health`,
    `capture.paused`, `capture.resumed`, `capture.segment_sealed`,
    `transcript.entry`.
- `crates/public/margins-core/src/memo.rs`
  - `MemoLine { at_ms, text, tags }`
  - Safe as a parsed/shared memo DTO candidate, but it is not the desktop live
    memo model.
- `crates/public/margins-workflows/src/transcript_view.rs`
  - `TranscriptView`
  - `load_transcript_view`
  - `render_live_capture_context`
  - This is the best existing read model for rolling transcript plus memo text.
    It already selects live snapshots, terminal checkpoints, and aligned
    fallbacks.
- `crates/public/margins-workflows/src/alignment.rs`
  - `render_aligned_markdown`
  - Timestamp parsing/rendering for `[MM:SS]` and `[MM:SS ~MM:SS]`.
- `crates/public/margins-store/src/lib.rs`
  - `SqliteSessionRepository`
  - `legacy::*` helpers for current sessions, segments, artifacts, and metadata.
- `crates/public/margins-meeting-protocol/src/lib.rs`
  - `ClientMessageV1`, `ServerMessageV1`
  - `CreateSessionV1`, `ResumeSessionV1`, `AudioChunkV1`,
    `CaptureDiscontinuityV1`, `CaptureHealthV1`, `CloseSegmentV1`,
    `FinalizeSessionV1`
  - This is durable meeting ingestion/replay protocol, not a local recorder
    control API.
- `crates/public/margins-meeting-runtime/src/lib.rs`
  - `MeetingRuntime`, `MeetingRuntimeStorage`
  - Provides idempotent event-runtime behavior for protocol messages, but does
    not own OS capture, Tauri app state, or live recorder controls.

Important negative finding: `margins new` and `margins attach` are not stable live
recording controls for the bb plugin. `crates/public/margins-cli/src/commands/capture.rs`
starts through an injected `CaptureProvider`, but the default public composition
has no available native provider, and the command path immediately finishes a
successful injected capture deterministically. It is useful structure, not a
live runtime contract.

## Desktop-Private Surface To Avoid

These symbols and paths describe current behavior and should be mined for
extraction, but must not become bb plugin dependencies:

- `desktop/src-tauri/src/lib.rs`
  - Tauri commands: `start_recording`, `stop_recording`, `pause_recording`,
    `resume_recording`, `get_recording_status`, `sync_memo`,
    `checkpoint_memo_line`, `append_active_pad_line`,
    `request_backchannel_for_memo`, `steer_backchannel_for_memo`.
  - Private impls: `start_recording_impl`, `stop_recording_impl`,
    `pause_recording_impl`, `resume_recording_impl`,
    `get_recording_status_impl`, `sync_memo_impl`.
  - Private desktop memo type:
    `MemoLine { text, created_secs, edited_secs, draft_started_secs,
    audio_pending_at_mark, block_ordinal }`.
  - Private live context functions:
    `build_live_capture_context`, `render_capture_context_markdown`,
    `format_live_transcript_prompt_context`.
- `desktop/src-tauri/src/recording.rs`
  - `RecordingState`, `RecordingStatus`, `RecordingPhase`,
    `recording_status_from_state`, `idle_recording_status`, `export_memo`.
  - These include useful fields, but the shape is Tauri/frontend-facing and can
    change with desktop UI needs.
- `desktop/src-tauri/src/transcript_store.rs`
  - Files and helpers around `*_live_transcript_snapshot.json`,
    `*_live_transcript_segments.jsonl`, and `*_live_watermark.json`.
  - `LiveTranscriptWatermark` is a good DTO candidate, but the file layout is
    private persistence.
- `desktop/src-tauri/src/server/http.rs`
  - `/api/invoke/:command`
  - `/api/audio/chunk`
  - `/api/live-audio/pcm`
  - `/ws/events`
  - Bearer-token auth is useful, but `/api/invoke` exposes raw Tauri command
    names with `{ok,result}` / `{ok:false,error:string}` instead of a stable
    product contract.
- `desktop/src-tauri/src/dispatch.rs`
  - Internal command router mirrors frontend invoke names. It is explicitly the
    wrong contract for a plugin.
- `desktop/src-tauri/src/web_session.rs`
  - Hosted recording state and web pause/resume behavior are useful
    counterevidence: hosted pause currently toggles pause state without sealing a
    segment the same way native pause does.
- `desktop/src-tauri/resources/hooks/margins-agent-hook.mjs`
  - Current agent hook polls `/api/invoke/get_recording_status` and injects a
    stale-transcript hint. This proves the need for a content-free freshness
    read, but it should be moved off raw invoke names.

## Required Operation Matrix

| Operation | Classification | Evidence and required boundary |
| --- | --- | --- |
| Discover current live meeting | EXTRACT | Reuse `.margins/current`, `read_current_session`, `load_transcript_view`, and desktop active `RecordingState`; add one resolver that distinguishes active runtime, current pointer, terminal transcript, and ambiguity. |
| Attach current live meeting | REUSE | Attachment is a bb/plugin concern once a `session_id` is resolved. Margins only needs to return a stable `LiveSessionRefV1`. |
| Start | EXTRACT | Desktop `start_recording_impl` already owns permissions, session creation, segment controller, live backchannel, and current pointer updates. Public CLI capture is not sufficient. |
| Pause | EXTRACT | Desktop `pause_recording_impl` seals the active segment and persists memo. Expose narrow semantics, not the Tauri command. |
| Resume | EXTRACT | Desktop `resume_recording_impl` creates the next segment with a fresh generation and offset. Expose this as the public behavior. |
| Stop | EXTRACT | Desktop `stop_recording_impl` finalizes live ASR, writes terminal live transcript data, persists memo/context, and schedules downstream work. |
| Append memo with Margins-owned timestamp | EXTRACT | `append_active_pad_line` already assigns elapsed time from Margins state and handles paused block ordinals. Extract a stable append operation; do not expose `sync_memo` full-vector replacement. |
| Read rolling transcript plus health | EXTRACT | Transcript is REUSE via `TranscriptJson`/`TranscriptView`; health is private in `RecordingStatus`. Combine them into one bounded snapshot DTO. |
| Resolve bounded watermark | REUSE for MVP | The bundled `watermark` skill already resolves live tactical guidance from `margins transcript --format json`. A plugin tool named `margins_watermark` is a bb/provider routing convenience, not a product requirement. Add a service verb only if bb cannot reliably route the skill/mention context to a local CLI turn. |

## Boundary Choice

### Option A: CLI JSON commands

Pros:

- Already available to local agents.
- `margins transcript --format json` is a good stable read path.
- Easy for skills such as `watermark` and `margins` to consume.

Cons:

- Current public CLI has no live runtime owner.
- `margins new`/`attach` are not long-lived recording commands in the public
  composition.
- Pause/resume/stop need to address the already-running desktop recorder.
- Process startup and repeated command execution are acceptable for a skill, but
  not ideal for a thread panel or rolling UI.

### Option B: tokened host-local loopback/service API

Pros:

- Matches existing process ownership: the desktop app already owns capture,
  permissions, segment controllers, live ASR, memo persistence, and current
  session state.
- Supports low-cost polling for rolling status/transcript freshness.
- Reuses the existing local bearer-token pattern without exposing raw Tauri
  invoke names.
- Works with bb host routing: plugin backend can register tools and mentions,
  and a plugin host worker can call the service on the machine where the thread
  environment runs. bb's host RPC is explicitly `hostId` targeted.
- Avoids a second store and avoids direct reads of private snapshot files.

Cons:

- Requires adding a small stable API surface to the desktop server.
- Requires bb plugin code to resolve thread/environment host identity before
  making host RPC calls. The bb mention search context exposes `projectId` and
  `threadId`, while host RPC calls require `hostId`, so that resolution must be
  handled server-side.

### Option C: direct Rust embedding

Pros:

- Type-safe in theory.
- Could share `margins-core` types directly.

Cons:

- Wrong process owner for OS capture and Tauri state.
- Risks duplicate state machines and store access from the plugin host worker.
- Couples bb plugin runtime to desktop internals and native dependencies.

Chosen boundary: Option B, a tokened host-local Margins live API, with shared DTOs
in public Rust crates and optional CLI wrappers later. This is the smallest
elegant boundary because it keeps one capture owner, one store, one clock, and
one transcript read model.

Long-lived service justification: the plugin needs rolling UI/status and
mutations against a live recorder. A CLI-only contract would either require a
separate daemon or repeated process startup, and it still would not know the
private in-memory recorder state. The desktop service is already alive when
capture is alive, can poll at UI cadence, and can remain host-local behind the
existing token.

## Minimal Verbs

Use versioned routes and DTOs. Keep the verb set small:

- `GET /api/live/v1/current?mode=active_or_current`
- `POST /api/live/v1/start`
- `POST /api/live/v1/sessions/{session_id}/pause`
- `POST /api/live/v1/sessions/{session_id}/resume`
- `POST /api/live/v1/sessions/{session_id}/stop`
- `POST /api/live/v1/sessions/{session_id}/memo/append`
- `GET /api/live/v1/sessions/{session_id}/snapshot?max_chars=16000`

Optional CLI wrappers can mirror the same contract:

- `margins live current --json`
- `margins live start --json [--title ...]`
- `margins live pause --json [session_id]`
- `margins live resume --json [session_id]`
- `margins live stop --json [session_id]`
- `margins live memo append --json [session_id] --text ...`
- `margins live snapshot --json [session_id] --max-chars ...`

## Minimal DTOs

All responses should use:

```json
{
  "schema_version": "margins.live.response.v1",
  "ok": true,
  "result": {}
}
```

Errors should use:

```json
{
  "schema_version": "margins.live.error.v1",
  "ok": false,
  "error": {
    "code": "not_running",
    "message": "No live Margins recording is active.",
    "retryable": false,
    "details": {}
  }
}
```

Error codes:

- `not_running`
- `not_found`
- `ambiguous_current`
- `already_running`
- `invalid_state`
- `capability_unavailable`
- `permission_denied`
- `busy`
- `stale_generation`
- `payload_too_large`
- `invalid_request`
- `internal`

`LiveSessionRefV1`:

```json
{
  "schema_version": "margins.live.session_ref.v1",
  "session_id": "20260904-121314",
  "title": "Customer call",
  "state": "recording",
  "live": true,
  "terminal": false,
  "source": "active_runtime",
  "started_at_unix_ms": 1788523994000,
  "current_segment_id": "seg-0",
  "generation": 3
}
```

`LiveSnapshotV1`:

```json
{
  "schema_version": "margins.live.snapshot.v1",
  "session": {},
  "health": {},
  "transcript": {},
  "transcript_watermark": {},
  "sequence": 42,
  "observed_at_unix_ms": 1788524010000
}
```

`LiveHealthV1`:

```json
{
  "schema_version": "margins.live.health.v1",
  "state": "recording",
  "paused": false,
  "elapsed_ms": 15600,
  "timeline_reusable": true,
  "capture_phase": "recording",
  "lanes": [
    {
      "lane": "microphone",
      "state": "healthy",
      "level": 0.42,
      "frame_count": 48000,
      "drop_count": 0,
      "silence_secs": 0.8
    }
  ],
  "freshness": {
    "status_age_ms": 0,
    "transcript_age_ms": 1200,
    "transcript_committed_until_ms": 14300
  }
}
```

`LiveTranscriptSliceV1`:

```json
{
  "schema_version": "margins.live.transcript_slice.v1",
  "body": "bounded markdown or text",
  "view": "live",
  "decoded_until_ms": 15100,
  "committed_until_ms": 14300,
  "updated_at_unix_ms": 1788524008800,
  "live": true,
  "terminal": false,
  "truncated": false,
  "max_chars": 16000
}
```

`LiveTranscriptWatermarkV1`:

```json
{
  "schema_version": "margins.live.watermark.v1",
  "decoded_until_ms": 15100,
  "committed_until_ms": 14300,
  "updated_at_unix_ms": 1788524008800,
  "stale_ms": 1200,
  "live": true,
  "terminal": false
}
```

`LiveMutationRequestV1`:

```json
{
  "schema_version": "margins.live.mutation.v1",
  "request_id": "uuid-or-bb-idempotency-key",
  "expected_generation": 3
}
```

`MemoAppendRequestV1`:

```json
{
  "schema_version": "margins.live.memo_append.v1",
  "request_id": "uuid-or-bb-idempotency-key",
  "expected_generation": 3,
  "text": "Follow up on pricing.",
  "client_observed_unix_ms": 1788524009000
}
```

`MemoAppendResultV1`:

```json
{
  "schema_version": "margins.live.memo_append_result.v1",
  "line_id": "session:3:12",
  "at_ms": 15200,
  "block_ordinal": null,
  "created_unix_ms": 1788524009200,
  "text": "Follow up on pricing."
}
```

## Semantics That Must Be Explicit

Authoritative clock:

- Margins assigns all recording-relative timestamps.
- Memo append must not accept `at_ms` from bb or the provider.
- `client_observed_unix_ms` is diagnostic only.
- Existing desktop code derives elapsed time from `RecordingState.start_time`;
  the public facade should preserve that single clock owner.

Freshness:

- `updated_at_unix_ms`, `decoded_until_ms`, and `committed_until_ms` come from
  the transcript store.
- `observed_at_unix_ms` comes from the live status read.
- `stale_ms` is computed by Margins or by the plugin from those fields, but the
  source timestamp must be included.
- Stale transcript is not the same as stopped capture.

Idempotency:

- Every mutation takes `request_id`.
- Exact replay returns the same result when feasible.
- Reuse of a `request_id` with a different body returns `invalid_request` or
  `conflict`.
- `expected_generation` prevents a pause/resume/append from applying to a stale
  segment after a resume.

Current-session ambiguity:

- `active_runtime` wins over `.margins/current`.
- If no active runtime exists and the current pointer points to a terminal
  transcript, return `not_running` unless the caller explicitly asks for
  historical current.
- If more than one live runtime can be observed, return `ambiguous_current` with
  bounded session refs.

Pause/resume:

- Native pause seals the current segment.
- Native resume creates a new segment and increments generation.
- Calling pause when already paused and resume when already recording should be
  idempotent success if `request_id`/generation match.
- Hosted web pause currently does not seal a segment. Either the live contract
  is native-only for MVP, or hosted behavior must be changed before claiming the
  same semantics.

Terminal/live states:

- `recording`, `paused`, `finalizing`, and `needs_attention` are live states.
- `ready`, `stopped`, `cancelled`, and `failed` are terminal for mutation
  purposes unless a specific recovery verb is later added.
- Transcript `terminal: true` means transcript finalization, not necessarily
  successful note distillation.

Content bounds:

- Memo append text should have a small hard limit, for example 4 KiB per line.
- Snapshot transcript body should default to a bounded tail, for example 16 KiB,
  with a server-side maximum, for example 64 KiB.
- Watermark guidance, if ever service-generated, should be bounded separately
  from transcript transport.

## Watermark Tool Necessity

`margins_watermark` is not necessary as a Margins product operation for MVP.

The existing `watermark` skill already defines the right low-latency behavior:
call `margins transcript --format json` immediately, use `decoded_until_ms`,
`live`, `terminal`, `body`, and `meeting_id`, and answer briefly. That remains a
tool-free skill path for local CLI-capable agents.

What bb may need is routing, not a new Margins cognitive API:

- `@Margins` can resolve to bounded current-session context using the live
  snapshot DTO.
- If a provider cannot reliably use send-time mention context or reach the
  recording host, the plugin may expose exactly one bb-native routing adapter,
  `margins_watermark`. It reads the same bounded snapshot as `@Margins`; it does
  not add broader retrieval or memo mutation powers.
- A provider-specific limitation may justify that one tool, but the product need
  is the composable read model plus UI mutations, not an LLM watermark endpoint.

Keep `request_backchannel_for_memo` and `steer_backchannel_for_memo` private.
They are memo cue/backchannel features with recall and prompt behavior; they are
not the same operation as a bounded watermark.

## bb Routing Evidence

The bb plugin SDK supports the needed plugin shape without requiring Margins to
expose Tauri internals:

- Backend tools: `bb.agents.registerTool(...)`.
- Dynamic agent setup: `bb.agents.configure(...)` and
  `bb.agents.contributeInstructions(...)`.
- Mentions: `bb.ui.registerMentionProvider(...)` with
  `PluginMentionSearchContext { trigger, query, projectId, threadId }` and
  `resolve(itemId) -> { context }`.
- Composer insertion from app UI: `PluginComposerApi.insertMention(...)`.
- Thread UI surfaces: `threadPanelAction(...)` and
  `experimental_threadHeaderAction(...)`.
- Host execution: `bb.hosts.experimental_client(...).call(method, input,
  { hostId })`.
- Host worker definition: `experimental_defineHostEntry(...)`.

Open bb-side question: the mention/search UI context gives `projectId` and
`threadId`, not `hostId`. The plugin backend must resolve thread to environment
and host before calling the host worker. Do not make the browser/app component
invent or persist host IDs.

## Implementation Sequence

Mandatory MVP:

1. Add public DTOs and errors.
   - Likely file: `crates/public/margins-core/src/live.rs`
   - Export from `crates/public/margins-core/src/lib.rs`
   - Symbols: `LiveSessionRefV1`, `LiveSnapshotV1`, `LiveHealthV1`,
     `LiveTranscriptSliceV1`, `LiveTranscriptWatermarkV1`, `LiveMutationRequestV1`,
     `MemoAppendRequestV1`, `MemoAppendResultV1`, `LiveErrorV1`,
     `LiveErrorCode`.
2. Extract desktop live facade.
   - Likely file: `desktop/src-tauri/src/live_api.rs`
   - Wrap existing `start_recording_impl`, `pause_recording_impl`,
     `resume_recording_impl`, `stop_recording_impl`,
     `get_recording_status_impl`, and `append_active_pad_line`.
   - Return public DTOs and stable error codes, not strings.
3. Add stable live HTTP routes.
   - File: `desktop/src-tauri/src/server/http.rs`
   - Add `/api/live/v1/...` routes beside `/api/invoke`, behind the same local
     bearer-token mechanism.
   - Keep `/api/invoke` for existing desktop/web clients, but do not document it
     as plugin API.
4. Build one snapshot read model.
   - Reuse `margins_workflows::transcript_view::load_transcript_view`.
   - Reuse `desktop/src-tauri/src/transcript_store.rs` watermark reads.
   - Reuse `recording_status_from_state` as an implementation source, but map it
     into `LiveHealthV1`.
5. Add memo append idempotency and timestamp result.
   - Refactor `append_active_pad_line` so it returns `MemoAppendResultV1`.
   - Keep Margins as timestamp owner.
   - Avoid exposing `sync_memo` full replacement as the plugin write contract.
6. Add focused tests.
   - DTO serde/golden tests in public crate.
   - Live facade unit tests around idle/current/active/paused/terminal mapping.
   - Memo append tests for recording vs paused block timestamp assignment.
   - HTTP handler tests for auth, idempotency, stale generation, and bounded
     transcript output.
   - Transcript snapshot tests that assert `live`, `terminal`, freshness, and
     truncation behavior.

Optional follow-ups:

- Add `margins live ... --json` CLI wrappers over the same facade where the
  desktop service is reachable.
- Add server-sent events or websocket updates only if polling proves too slow or
  wasteful for the panel. Start with polling because the existing watermark
  sidecar already records freshness.
- Add the single `margins_watermark` bb routing adapter only if provider tests
  show that `@Margins` plus the bundled `watermark` skill cannot cover refresh
  or cross-host execution. Do not add a Margins generated-answer endpoint.
- Harmonize hosted web pause/resume with native segment-sealing semantics before
  exposing hosted live sessions through the same contract.
- Add a `capabilities --json` live section once the live API exists.

## Spec Challenges And Counterevidence

- If the spec assumes `margins new` or `margins attach` can drive the live
  desktop recording, current CLI code contradicts that. Public capture uses an
  injected provider and then immediately finishes the segment; default public
  composition reports capture unavailable.
- If the spec assumes `/api/invoke/get_recording_status` is an acceptable plugin
  dependency, current code shows it is a raw desktop dispatch bridge with
  frontend command names and string errors. It is useful precedent, not a
  contract.
- If the spec assumes pause/resume semantics are already uniform across native
  and hosted capture, current `web_session` behavior contradicts that. Native
  pause seals a segment; hosted pause toggles paused state.
- If the spec assumes the plugin should read live transcript files directly,
  current code contradicts that boundary. `transcript_store` filenames and
  snapshot layouts are persistence details, while `TranscriptJson` and
  `TranscriptView` are the composable read surfaces.
- If the spec requires a `margins_watermark` product tool, current skills
  evidence weakens that requirement. The `watermark` skill is already designed
  around `margins transcript --format json`; the missing piece is bb routing and
  context attachment, not a new Margins reasoning endpoint.
- If the spec assumes mention search can directly know the machine host, bb SDK
  evidence is weaker. Mention context includes project/thread identity; host RPC
  calls require a `hostId`. The plugin must resolve that mapping through bb
  server-side state.

## Open Questions

- Should the MVP live contract support hosted web recordings, or only native
  desktop recordings? The current pause/resume mismatch argues for native-only
  first.
- Where should short-lived mutation idempotency live: in desktop `AppState`, in
  the session store, or in a small sidecar under `.margins`? For crash-safe
  retries, store-backed is cleaner; for MVP local UI retries, bounded in-memory
  may be enough.
- Should the public live DTOs live in `margins-core::live`, or in a tiny
  dedicated contract crate if the same schema will be consumed by TypeScript
  generation? `margins-core::live` is the smallest first move.
- Should `TranscriptJson` be reused verbatim inside `LiveSnapshotV1`, or should
  `LiveTranscriptSliceV1` be a smaller bounded projection? Prefer the bounded
  projection for plugin UI/tool safety, with `margins transcript --format json`
  remaining the full local CLI path.
