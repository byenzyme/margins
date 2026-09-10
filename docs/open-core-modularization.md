# Margins open-core Rust architecture

Status: proposed architecture; no physical module move is authorized by this
document.

Baseline: `d60a91a2dfdbc7d03fcc2aefcf3fd063bb4277ab` (2026-08-10).

## Decision

Margins will become a Cargo workspace with six public crates and two private
crates. Public crates contain portable domain contracts, media transforms,
persistence, workflows, and the CLI. Native audio capture and every desktop
surface remain private. The boundary is enforced by dependency direction and
an allowlisted export, not by `pub` visibility or Cargo features alone.

The current root `margins` crate is a transitional umbrella. It is not the
future architectural boundary: at the baseline it combines a CLI, SQLite
storage, note workflows, model runtimes, terminal UI, platform capture, and
capture internals in one package. `margins-desktop` then imports those internals
directly. In particular, public types such as `cpal::Device`, atomics, channels,
`CaptureSink`, and `SegmentWriter` are implementation leaks, not stable APIs.

This design deliberately does **not** make native recording a plugin ABI in the
first migration. The public contract is a versioned Rust port and a versioned
JSON event protocol. A C ABI, dynamic loading, or third-party binary plugin
format would be a separate design with its own compatibility and security
requirements.

## Baseline dependency audit

There is no Cargo workspace at the baseline. The root package is both library
and binary:

```text
margins (root Cargo.toml; lib + CLI bin)
├── portable/domain: session, session_index, note_artifacts, project
├── portable/media: audio_info, audio_pipeline, alignment
├── model/runtime: asr, coreml_asr, diarization, offline_asr
├── application: granola_import, publish
├── terminal: app, parser, text_helpers, tui, main
└── native capture: recorder, recorder/coreaudio_mic,
                    recorder/segment_writer

margins-desktop (desktop/src-tauri/Cargo.toml; lib + Tauri/server bins)
└── margins (path = ../.., default-features = false)
```

The root default features are `coreml-asr` and `audio-capture`.
`audio-capture` enables `cpal` and `cidre`; `coreml-asr` enables the Objective-C
and Core ML bindings. The desktop defaults are `tauri-app`,
`rust-diarization-coreml`, and `coreml-asr`. Its `tauri-app` feature enables
`margins/audio-capture`, while its `server` feature intentionally does not.

The current module edges that determine the extraction order are:

```text
alignment ──> asr, parser
coreml_asr ──> asr, audio_pipeline
diarization ──> asr
offline_asr ──> asr, coreml_asr
granola_import ──> publish, session
session_index ──> session, note_artifacts
app ──> parser, text_helpers
tui ──> app, recorder
main ──> app, granola_import, parser, project, publish, recorder,
         session, session_index, tui
recorder ──> recorder/coreaudio_mic, recorder/segment_writer
```

Two baseline couplings must be broken rather than copied into the public
surface:

1. `src/project.rs` embeds skills from `desktop/src-tauri/resources`. Public
   code may not reach into the private desktop tree.
2. `src/lib.rs` duplicates hundreds of lines of recorder types as
   no-`audio-capture` stubs. Public contracts must be real, feature-independent
   definitions; unavailable providers report capabilities or a typed error.

Baseline validation also exposes two existing feature-isolation gaps that
Phase 0 must close: `examples/tap_probe.rs` imports optional `cidre` during an
all-targets no-default build, and a `#[tauri::command]` in desktop `lib.rs` is
not gated out of the `server`-only build. These are evidence for the proposed
boundaries, not changes made by this document.

The desktop also depends on concrete root modules throughout recording,
transcription, model management, session queries, note helpers, and imports.
Those consumers must switch to the new crates or to injected public ports
before the umbrella crate can disappear.

## Target packages and ownership

All public packages live under `crates/public/`. Private packages use an
explicit private path and `publish = false`. Repository placement is part of
the export control.

### `margins-core` (public)

Path: `crates/public/margins-core`

Purpose: dependency-light domain types and stable ports. Modules:

- `ids`: `SessionId`, `SegmentId`, `CaptureOperationId`, and event sequence
  newtypes.
- `audio`: `AudioLane`, `PcmChunk`, `AudioFormat`, and artifact descriptors;
  no device handles, threads, queues, or codecs.
- `memo`: parsed memo-line DTOs shared by the CLI and alignment workflow.
- `capture`: `CaptureProvider`, `CaptureHandle`, commands, capabilities,
  device descriptors, snapshots, outcomes, and errors.
- `session`: persisted session/segment/artifact records, lifecycle enums, and
  the `SessionRepository` port.
- `event`: the in-process event model, versioned wire envelope, and `EventSink`
  port.
- `transcript`: word/entry DTOs plus ASR and diarization ports.

First-party dependencies: none. External dependencies: `serde` and
`serde_json` only. Contract timestamps are Unix milliseconds or validated
RFC3339 strings so `chrono`, SQLite, Tauri, and Tokio do not enter the base
crate. Public ports return typed `margins_core` errors, never `anyhow::Error`.

### `margins-meeting-protocol` (public)

Path: `crates/public/margins-meeting-protocol`

Purpose: the transport-neutral V1 wire contract for mobile/browser capture,
optional VPS relay, and a meeting runtime. It owns reconnect replay cursors,
idempotent command/event correlation, per-lane audio ACKs and discontinuities,
close/finalize prerequisites, capture provenance, transcripts, memos, and
artifact notices. It contains no runtime, persistence, device, Tauri, or
network implementation. External dependencies are `serde`, `serde_json` for
preservable unknown event payloads, and `base64` for browser-efficient JSON
audio while binary serializers retain byte strings.

### `margins-media` (public)

Path: `crates/public/margins-media`

Purpose: reusable, non-capture audio/transcript implementation.

- Decode/probe/write audio, channel extraction, downmixing, chunking, and
  segment combination.
- Pure rational/linear resampling and media-frame/time conversion.
- Transcript merge/dedup/phrase transforms.
- Optional statically linked model adapters behind features:
  `parakeet-onnx`, `coreml-asr`, and `polyvoice-diarization`.

First-party dependency: `margins-core`. Main external dependencies:
`anyhow` internally, `hound`, and `symphonia`; optional model features own
`libloading`, `ndarray`, `ort`, `realfft`, `polyvoice`, `block2`, `objc2`,
`objc2-core-ml`, and `objc2-foundation`.

Core ML ASR is public because it is a model adapter over caller-supplied PCM,
not a device-capture implementation. CoreAudio capture is private; the shared
word “Core” is not a reason to couple the two.

### `margins-store` (public)

Path: `crates/public/margins-store`

Purpose: local persistence and query implementations.

- SQLite implementation of `SessionRepository`, including schema migration,
  tombstones, segments, artifacts, grounding, and vault-note registry.
- Session list/index queries.

First-party dependency: `margins-core`. Main external dependencies:
`anyhow`, `chrono`, `rusqlite` with `bundled`, `serde`, and `serde_json`.
Neither capture nor desktop may be referenced here.

### `margins-workflows` (public)

Path: `crates/public/margins-workflows`

Purpose: application services that compose public ports.

- Offline transcription orchestration through `AsrBackend`.
- Granola import, transcript/memo alignment, note artifact manipulation,
  vault publishing, and project resolution.
- Processing lifecycle events emitted through `EventSink`.

First-party dependencies: `margins-core`, `margins-media`, and
`margins-store`. Main external dependencies: `anyhow`, `chrono`, `dirs`,
`regex`, `serde`, `serde_json`, and `toml`.

The two workspace-setup skills currently embedded from the desktop resource
tree move to `crates/public/margins-workflows/resources/skills/`. The workflow
crate embeds its own public resources. The desktop-only `margins-desktop` skill
does not move.

### `margins-cli` (public)

Path: `crates/public/margins-cli`

Purpose: command parsing, terminal editor, and public command application.

It exposes `run(CliServices, args)` from a library target and a normal binary
that constructs only public services. Commands that require capture inspect
`CaptureCapabilities`; an export without a native provider returns the stable
`capture_unavailable` error while import, process, project, transcript,
artifact, and note commands remain usable. A private distribution may inject
the native provider without forking command parsing.

First-party dependencies: `margins-core`, `margins-media`, `margins-store`, and
`margins-workflows`. Main external dependencies: `anyhow`, `chrono`, `clap`,
`crossterm`, and `ratatui`.

### `margins-capture-native` (private)

Path: `crates/private/margins-capture-native`

Purpose: the complete native capture implementation and capture-specific
durability/timing policy.

- CPAL and direct CoreAudio microphone implementations.
- macOS process tap and Windows WASAPI loopback capture.
- Callback rings, drain threads, standby/commit/retire protocol, capture
  tokens, segment actors, spool files, silence synthesis, lane sealing, device
  watching, permission probes, test tone, health telemetry, and the combined
  legacy recorder facade while it is still needed.
- An implementation of `margins_core::capture::CaptureProvider` whose returned
  handle is a sendable actor client. The underlying `cpal::Stream` remains on
  the private controller thread because it is not `Send` on supported native
  backends.

First-party dependencies: `margins-core` and `margins-media`. Main external
dependencies: `anyhow`, `cpal`, `dasp`, `hound`, and `rtrb`; target-specific
`cidre` on macOS and `windows` on Windows. `publish = false` is mandatory.

### `margins-desktop` (private)

Path remains `desktop/src-tauri`.

Purpose: Tauri application, local web server, native capture controller,
product policy, AI/backchannel orchestration, settings/auth/calendar/Granola
integrations, transport adapters, and the private native-CLI composition
binary. The TypeScript frontend and all desktop resources remain under
`desktop/` and are outside the public export.

It depends on all five public crates and optionally on
`margins-capture-native`. `tauri-app` and `native-cli` enable the native
provider; `server` does not. A thin private `margins` bin calls the public
`margins_cli::run` with that provider, preserving native CLI recording without
adding a public-to-private dependency. The server installs an
unavailable/browser capture adapter through the same public port. Tauri
emission and WebSocket broadcast remain private adapters implementing
`EventSink`.

Main external dependencies stay here: Tauri and its plugins, `tokio`, `reqwest`,
`asupersync`, the `pi_agent_rust` Git dependency, `notify`, `axum`,
`tower-http`, `rust-embed`, and macOS WebKit/window dependencies. No public
package may gain an edge to any of them.

## Exact first-party dependency DAG

An arrow means “depends on.” This is the complete allowed first-party graph;
transitive edges are shown only through their direct owner.

```text
margins-core

margins-meeting-protocol

margins-media ───────────────> margins-core
margins-store ───────────────> margins-core

margins-workflows ───────────> margins-core
                  ├──────────> margins-media
                  └──────────> margins-store

margins-cli ─────────────────> margins-core
            ├────────────────> margins-media
            ├────────────────> margins-store
            └────────────────> margins-workflows

margins-capture-native ──────> margins-core
                       └─────> margins-media

margins-desktop ─────────────> margins-core
                ├────────────> margins-media
                ├────────────> margins-store
                ├────────────> margins-workflows
                ├────────────> margins-cli             [native-cli bin]
                └────────────> margins-capture-native  [optional; app/CLI]
```

Forbidden edges are as important as allowed ones:

- Public crates never depend on `margins-capture-native` or
  `margins-desktop`, including in dev-dependencies, build-dependencies,
  examples, fixtures, or embedded paths.
- `margins-core` never depends on another first-party crate.
- `margins-store` never depends on media, workflow, CLI, capture, or desktop.
- `margins-media` never depends on storage, workflow, CLI, capture, or desktop.
- `margins-capture-native` never persists sessions or emits Tauri/WebSocket
  messages directly.
- `margins-workflows` never selects a native device or imports a capture
  implementation.
- `margins-desktop` is the only composition root allowed to combine private
  capture with desktop policy.

Cargo features may remove an edge but may not reverse one. The workspace root
uses public crates as `default-members`; private packages are explicit CI jobs.

### Existing external dependency disposition

This table accounts for every dependency declared by the two baseline Cargo
manifests. An external crate may legitimately have more than one owner, but it
must not be pulled across a boundary merely for convenience.

| Baseline dependencies | Target owner(s) |
| --- | --- |
| `serde`, `serde_json` | `margins-core` and `margins-meeting-protocol` for contracts/wire values; also direct where implementations serialize private or persisted data. |
| `base64` | `margins-meeting-protocol` for JSON audio payloads; binary serializers use native byte strings. |
| `anyhow` | Internal implementation in media, store, workflows, CLI, capture, and desktop; never the public core port error type. |
| `chrono` | `margins-store`, `margins-workflows`, `margins-cli`, and desktop presentation/integrations. |
| `clap`, `crossterm`, `ratatui` | `margins-cli` only. |
| `hound`, `symphonia` | `margins-media`; `hound` may also remain private in capture for spool/finalization until it delegates to media. |
| `rusqlite` (`bundled`) | `margins-store` only. |
| `dirs`, `regex`, `toml` | `margins-workflows`; desktop may retain direct settings/config use. |
| `libloading`, `ndarray`, `ort`, `realfft` | Optional `margins-media/parakeet-onnx` provider. |
| `polyvoice` | Optional `margins-media/polyvoice-diarization`; desktop keeps a direct edge only for product-specific live VAD not moved behind the public provider. |
| `block2`, `objc2`, `objc2-core-ml`, `objc2-foundation` from the root manifest | Optional macOS `margins-media/coreml-asr` provider. |
| `cpal`, `dasp`, `rtrb`, `cidre`, and `windows` | `margins-capture-native` only. The duplicate desktop `cpal` edge is removed. |
| `tempfile` | Dev-dependency of whichever extracted crates own the migrated tests; never a runtime edge. |
| `tauri`, `tauri-build`, `tauri-plugin-shell`, `tauri-plugin-dialog`, `tauri-plugin-fs`, `tauri-plugin-global-shortcut`, `tauri-plugin-updater` | `margins-desktop` only. |
| `window-vibrancy`, desktop `objc2`, desktop `objc2-foundation`, `objc2-web-kit` | `margins-desktop/tauri-app` target-specific UI integration only. |
| `reqwest`, `tokio`, `async-trait`, `base64`, `rand`, `sha2`, `asupersync`, `pi_agent_rust`, `notify` | Private desktop services/integrations only. |
| `axum`, `tower-http`, `rust-embed` | Private `margins-desktop/server` only. |

The target feature map is equally strict:

```text
margins-media:
  default = []
  parakeet-onnx = [libloading, ndarray, ort, realfft]
  parakeet-onnx-dynamic = [parakeet-onnx, ort/load-dynamic]
  coreml-asr = [block2, objc2, objc2-core-ml, objc2-foundation]  # macOS
  polyvoice-diarization = [polyvoice]
  polyvoice-coreml = [polyvoice-diarization, polyvoice/coreml]   # macOS

margins-capture-native:
  default = []
  macOS target = [cpal, cidre]
  Windows target = [cpal, windows]

margins-desktop:
  default = [tauri-app, rust-diarization-coreml, coreml-asr]
  tauri-app = [margins-capture-native, Tauri/plugin/UI dependencies]
  native-cli = [margins-cli, margins-capture-native]
  server = [axum, tower-http, rust-embed]
  coreml-asr/parakeet-asr/rust-diarization* forward only to margins-media
```

The public CLI’s default binary has no capture feature. Native capture is
selected only by the private composition binaries, never by a public Cargo
feature that references a private path.

## Stable contracts

The contracts below define the boundary. Exact Rust spelling can be refined in
an RFC patch, but semantics, ownership, and wire requirements are normative.

### Capture provider

```rust
pub trait CaptureProvider: Send + Sync {
    fn capabilities(&self) -> CaptureCapabilities;
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError>;
    fn permission(&self, lane: AudioLane) -> Result<PermissionState, CaptureError>;
    fn request_permission(&self, lane: AudioLane)
        -> Result<PermissionState, CaptureError>;
    fn start(
        &self,
        request: CaptureRequest,
        observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}

pub trait CaptureObserver: Send + Sync {
    fn on_audio(&self, chunk: PcmChunk);
    fn on_event(&self, event: EventEnvelope);
}

pub trait CaptureHandle: Send + Sync {
    fn snapshot(&self) -> Result<CaptureSnapshot, CaptureError>;
    fn command(&self, command: CaptureCommand)
        -> Result<CaptureCommandResult, CaptureError>;
}
```

Normative rules:

- Public device identity is `CaptureDevice { id, name, kind, is_default,
  is_available }`. `id` is provider-opaque and stable only for that provider;
  a `cpal::Device`, CoreAudio object ID, UID lookup strategy, or tap name never
  crosses the boundary.
- `CaptureRequest` includes session/segment IDs, desired lanes, optional input
  device ID, live-PCM preference, an application-supplied durable destination,
  and an operation ID. The provider never chooses an application-visible path;
  completed artifacts are returned as descriptors.
- Commands are `Pause`, `Resume`, `SelectInput`, `RestartLane`, `Finish`, and
  `Cancel`. Every command carries an operation ID. Repeating a completed
  operation ID returns the same terminal result; stale operations cannot
  mutate a newer segment.
- `Finish` and `Cancel` are terminal and idempotent. Exactly one terminal event
  is emitted. `Pause` seals a segment; `Resume` starts a new segment and never
  silently rewrites the prior artifact.
- `PcmChunk` contains `lane`, `generation`, `session_offset_ms`,
  `sample_rate_hz`, and owned mono `f32` samples. Chunks for a lane/generation
  are ordered. A generation change is explicit and its session offset never
  decreases.
- Backpressure is observable. Dropped live chunks increment snapshot/event
  counters, but live-delivery loss does not imply durable-WAV loss. The
  contract reports these separately.
- Provider snapshots are immutable values. They include lane state, delivered
  frames, signal observation, drop counters, current generation, and last
  error code. They contain no `Arc<Atomic*>`, sender, mutex, or implementation
  handle.
- `CaptureError` has stable machine codes (`unavailable`, `permission_denied`,
  `device_lost`, `open_failed`, `stalled`, `writer_failed`, `cancelled`,
  `invalid_transition`, `internal`) plus a human message and optional retry
  advice. Desktop copy is not produced by the provider.

### Persisted session

```rust
pub trait SessionRepository: Send + Sync {
    fn create(&self, new: NewSession) -> Result<SessionRecord, SessionError>;
    fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, SessionError>;
    fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError>;
    fn append_segment(
        &self,
        id: &SessionId,
        expected_revision: u64,
        segment: NewSegment,
    ) -> Result<SessionRecord, SessionError>;
    fn transition(
        &self,
        id: &SessionId,
        expected_revision: u64,
        next: SessionLifecycle,
    ) -> Result<SessionRecord, SessionError>;
    fn upsert_artifact(&self, artifact: SessionArtifact)
        -> Result<(), SessionError>;
    fn tombstone(&self, id: &SessionId, expected_revision: u64)
        -> Result<(), SessionError>;
}
```

Normative rules:

- `SessionId` is the stable identity; title and note path are mutable labels.
- `SessionRecord` has a monotonically increasing `revision`. Mutations use
  optimistic concurrency and fail with `conflict` rather than overwrite a
  concurrent desktop/CLI update.
- Segment ordinals are unique and append-only within a session. Offsets are
  non-negative and nondecreasing. A sealed segment’s audio path, start offset,
  duration, format, and capture diagnostics are immutable.
- Lifecycle is `Active`, `Paused`, `Processing`, `Ready`, `NeedsAttention`, or
  `Tombstoned`. Allowed transitions are centralized in `margins-core`; SQLite
  strings and UI labels are adapters.
- Delete remains two-phase: tombstone first, external artifact cleanup second,
  final row removal last. A failed cleanup is retryable and discoverable.
- Storage migrations accept the baseline SQLite/legacy JSON data and preserve
  stable session IDs, segment offsets, artifact registry rows, grounding,
  note errors, and lifecycle fields.

### Events

In-process events are typed and `#[non_exhaustive]`. Transport uses a forward-
compatible envelope:

```json
{
  "schema": "margins.event",
  "version": 1,
  "sequence": 42,
  "emitted_at_ms": 1786377600000,
  "session_id": "01...",
  "segment_id": 3,
  "operation_id": 17,
  "kind": "capture.lane_state_changed",
  "payload": {}
}
```

Normative rules:

- `schema`, `version`, `sequence`, `emitted_at_ms`, `session_id`, `kind`, and
  `payload` are always present. Segment and operation IDs are nullable only
  when the event is genuinely session-wide.
- Sequence is monotonically increasing within a capture/session event stream.
  Consumers reduce by sequence and ignore a duplicate sequence. Wall clock is
  diagnostic, never ordering authority.
- Version 1 kinds are grouped as `capture.*`, `session.*`, `transcript.*`, and
  `note.*`. The initial set covers started, lane/device state changed, health,
  paused/resumed, segment sealed, progress, transcript entry, note delta,
  failed, cancelled, and completed.
- Unknown kinds and unknown payload fields must round-trip through transports
  and be ignored by reducers that do not understand them. Removing/renaming a
  field or changing its meaning requires a new envelope version; adding an
  optional payload field does not.
- Tauri and WebSocket use the identical envelope. Transport event names become
  a single `margins-event`; current names can be emitted in parallel during
  migration.
- The current `ProcessingEvent.stage`, `.track`, and `.phase` strings become an
  adapter into typed kinds. Human `message` remains optional presentation
  metadata and never drives state.
- `EventSink::publish(&EventEnvelope)` is the only application event port.
  Capture code, workflows, Tauri, and Axum do not know one another’s senders.

## File migration map

The table is exhaustive for baseline root Rust files. “Split” identifies the
specific ownership change; it does not authorize the move in this patch.

| Baseline file | Target | Action |
| --- | --- | --- |
| `src/lib.rs` | transitional root facade | Re-export new public crates during migration; delete recorder stubs and retire the facade in the final phase. |
| `src/main.rs` | `margins-cli/src/lib.rs`, `margins-cli/src/main.rs`, `desktop/src-tauri/src/native_cli_main.rs` | Split command/orchestration from binaries; the public bin supplies an unavailable provider and the private native bin injects capture. |
| `src/app.rs` | `margins-cli/src/editor.rs` | Replace direct recorder knowledge with public capture status/actions. |
| `src/tui.rs` | `margins-cli/src/tui.rs` | Replace `crate::recorder` device calls with `CaptureProvider`. |
| `src/parser.rs` | `margins-cli/src/memo_parser.rs` | Move with tests; alignment receives parsed public memo DTOs. |
| `src/text_helpers.rs` | `margins-cli/src/text_helpers.rs` | Move unchanged with tests. |
| `src/alignment.rs` | `margins-workflows/src/alignment.rs` | Depend on core transcript/memo DTOs, not CLI parser internals. |
| `src/granola_import.rs` | `margins-workflows/src/import/granola.rs` | Replace concrete session calls with `SessionRepository`. |
| `src/note_artifacts.rs` | `margins-workflows/src/note_artifacts.rs` | Move pure frontmatter/note logic and tests. |
| `src/offline_asr.rs` | `margins-workflows/src/transcription.rs` | Inject `AsrBackend`; model directory selection stays in composition code. |
| `src/project.rs` | `margins-workflows/src/project.rs` | Move public project logic; relocate its two embedded public skills out of desktop. |
| `src/publish.rs` | `margins-workflows/src/publish.rs` | Move vault rendering/publish service. |
| `src/session.rs` | `margins-core/src/session.rs`, `margins-store/src/sqlite.rs` | Split DTOs/invariants/port from SQLite schema, legacy migration, and queries. |
| `src/session_index.rs` | `margins-store/src/index.rs` | Move queries; return core DTOs. |
| `src/audio_info.rs` | `margins-media/src/info.rs` | Move with tests. |
| `src/audio_pipeline.rs` | `margins-media/src/audio.rs` | Move decode/write/transform functions and tests. |
| `src/asr.rs` | `margins-core/src/transcript.rs`, `margins-media/src/transcript.rs`, `margins-media/src/providers/parakeet.rs` | Split traits/DTOs, pure transforms, and optional ONNX implementation. |
| `src/coreml_asr.rs` | `margins-media/src/providers/coreml.rs` | Move model adapter and rolling inference tests behind `coreml-asr`. |
| `src/diarization.rs` | `margins-core/src/transcript.rs`, `margins-media/src/diarization.rs`, `margins-media/src/providers/polyvoice.rs` | Split port/DTOs, pure speaker assignment, and optional provider. |
| `src/recorder.rs` | `margins-capture-native/src/lib.rs` plus small extractions to core/media | Move all native capture, rings, device/permission/probe/watch logic, and legacy facade privately. Replace public recorder exports with contracts. |
| `src/recorder/coreaudio_mic.rs` | `margins-capture-native/src/platform/macos/microphone.rs` | Move intact with tests; keep private. |
| `src/recorder/segment_writer.rs` | `margins-capture-native/src/segment_writer.rs` plus pure math extraction | Keep actor/spool/timeline durability machinery private; extract only `RationalResampler`, checked frame scaling, and their platform-free tests to media. |

Desktop Rust remains in the private `margins-desktop` package. Its internal
refactor is nevertheless explicit:

- Capture/product cluster stays private:
  `recording.rs`, `mic_control.rs`, `device_registry.rs`, `audio_devices.rs`,
  `live_backchannel.rs`, `live_asr_worker.rs`, `web_live_asr.rs`,
  `transcript_store.rs`, `transcription.rs`, and `speech_models.rs`. These
  switch from `margins::recorder` and model globals to injected ports/new public
  crates. `recording.rs` remains the product-level controller; it does not move
  into the capture crate.
- Event/transport adapters stay private: `ctx.rs`, `processing_events.rs`, and
  `server/events.rs`. They implement/translate `EventSink` and emit the common
  envelope. `server/assets.rs`, `server/auth.rs`, `server/http.rs`, and
  `server/mod.rs` remain private server adapters.
- Thin duplicate shims `granola_import.rs`, `note_artifacts.rs`, and
  `session_index.rs` are deleted after callers import the owning public crates
  directly.
- Desktop composition and integrations stay private: `lib.rs`, `main.rs`, new
  `native_cli_main.rs`, `server_main.rs`, `dispatch.rs`, `web_session.rs`, `settings.rs`,
  `calendar.rs`, `editor.rs`, `agent_hooks.rs`, `ai_auth.rs`, `ai_config.rs`,
  `backchannel_ai.rs`, `granola_mcp.rs`, `pi_distill.rs`, `pi_events.rs`,
  `prep_ai.rs`, and `reprocess.rs`.
- All `desktop/src`, `desktop/public`, `desktop/scripts`, desktop test harnesses,
  Tauri configuration/resources, and package metadata remain private.

## Recorder/CoreAudio/segment-writer privacy audit

This audit covers the recent capture line present at the baseline, especially
`a1e974b`, `1a78837`, `cbdca4b`, `b079132`, `3694d24`, `a45870b`, `66a1e8f`,
`1fe8df1`, `a2838de`, `7332bd5`, `16d3880`, and `f458b84`.

| Work | Classification | Boundary decision |
| --- | --- | --- |
| Independent mic/system ownership, controller actor, standby candidate, commit/retire ACK ordering, capture tokens, stale-event rejection (`a1e974b`) | Private capture implementation | These rules solve native callback and non-`Send` stream ownership. Expose only idempotent command/results and ordered state events. |
| Two lane actors, bounded lane queues, per-lane spool files, tick/silence synthesis, overlap trimming, media clock freezing, seal/drain protocol, timeline-reuse qualification (`a1e974b`, `66a1e8f`, `7332bd5`) | Private segment writer | This is differentiated durability and recovery policy. Do not publish `CaptureSink`, `LaneMsg`, `SegmentWriter`, atomics, queue constants, spool format, or ACK protocol. |
| Direct CoreAudio IOProc registration/destruction, ASBD mapping, UID lookup, callback flight guard, buffer layout inspection, host-time conversion, CPAL fallback (`a1e974b`, `3694d24`) | Private platform implementation | Public contract exposes opaque device IDs, permission, normalized errors, PCM, and value telemetry only. No `cidre`, AudioObject IDs, callbacks, or backend names are contractual. |
| Generation/session-offset mapping across restarts (`1a78837`, `f458b84`) | Split | Pure monotonic epoch/frame conversion and tests are reusable in `margins-media::timeline`. Transition creation, verification policy, live CoreML queues, pause semantics, and product recovery stay private. |
| System-tap delivery versus quiet signal, grace windows, “silence is not interruption,” sticky session signal (`cbdca4b`, `b079132`, `1fe8df1`) | Split | Public snapshots distinguish delivered frames, observed signal, dropped frames, and lane state. Thresholds, grace durations, TCC diagnosis, warning severity, and copy remain private desktop policy. |
| Startup cancellation, stall detection, backend fallback ladder, pending selection by UID, virtual tap filtering (`3694d24`, `a2838de`) | Private policy/implementation with public capability | Public commands include cancellation and opaque device selection, and errors include `stalled`/`device_lost`. Retry count, CPAL↔CoreAudio order, two-second stall threshold, UID mechanics, and tap name remain private. |
| Capture-loss accounting and live transcript reuse qualification (`a45870b`) | Split | Public outcomes report durable/live drop counters and `timeline_reusable`; whether to reuse live text or run offline ASR is workflow/product policy, not provider API. |
| Capture-first ASR decoupling, unbounded channel plus explicit sample budget, re-prewarm/web follow-ups (`7332bd5`, `16d3880`) | Private orchestration with reusable backpressure semantics | Public events report queue acceptance/drop values. Channel type, queue size, worker lifecycle, CoreML prewarm strategy, and web/Tauri behavior remain private. |
| Rational resampling, checked nanoseconds-to-frame conversion, WAV PCM encoding | Reusable public media code | Extract implementation and tests only after removing dependencies on lane actors, live sinks, and private telemetry. Generic WAV decode/write already belongs in `margins-media`. |
| `PacketDesc { frame_count, capture_nanos }` | Private | It is callback-to-drain transport metadata and its clock origin is provider-specific. Public consumers receive normalized `PcmChunk` offsets/events instead. |
| Atomics-based `MicCaptureTelemetry`, `SystemCaptureTelemetry`, `LaneTelemetry`, `LiveAudioSink` | Private | Convert to immutable core snapshots/events at the adapter. Memory ordering and queue ownership are not API. |
| Desktop `SegmentController`, mic policy state machine, `CaptureDeviceState`, diagnostics reducer, resume preconditions | Private desktop product logic | It composes the provider with settings, UX, ASR, memo, and session persistence. Only core command/event/session types cross boundaries. |

The rule for later review is simple: reusable math and value semantics may move
public; native resource ownership, recovery strategy, durability mechanics,
platform bindings, product thresholds, and orchestration remain private.

## Build-green migration phases

Each phase is independently mergeable. Compatibility re-exports are removed
only after every consumer has migrated. No phase combines a crate move with a
behavioral rewrite.

### Phase 0 — freeze the observable baseline

- Add contract characterization tests for session JSON/SQLite migration,
  transcript JSON, processing event JSON, recorder WAV channel layout, segment
  offsets, pause/resume, device swap, and terminal capture outcomes.
- Record `cargo metadata` feature graphs for root and desktop.
- Add CI feature lanes matching current supported builds, including the native
  rolling harness as an opt-in local job.

Green gate: all existing root, desktop unit, native capture, and headless server
checks behave exactly as at `d60a91a`.

### Phase 1 — introduce `margins-core` without moving implementation

- Add IDs, capture/session/event/transcript DTOs and ports.
- Implement adapters from current recorder/session/processing types.
- Re-export core types from `margins` where source compatibility is useful.
- Add JSON golden tests for version 1 events and compile tests proving public
  DTOs do not expose Tauri/CPAL/SQLite types.

Green gate: root and desktop callers still compile through the umbrella crate;
new contract tests pass on macOS and Linux no-default builds.

### Phase 2 — extract portable media

- Move audio info/pipeline, transcript transforms, pure timeline math, and
  model adapters in small commits.
- Leave forwarding modules in `margins` for each moved module.
- First extract `RationalResampler`/frame math with their tests; do not move the
  segment actor.

Green gate: byte/float fixture outputs, word timing, alignment, resampling, and
Core ML/Parakeet feature tests match baseline tolerances. Root API re-exports
keep both binaries green.

### Phase 3 — extract store and workflows

- Split session DTOs from SQLite implementation; make the existing functions a
  compatibility facade over `SqliteSessionRepository`.
- Move session index, note artifacts, import, publish, project, alignment, and
  offline transcription in dependency order.
- Relocate public embedded skills before changing `project.rs` include paths.

Green gate: migrate copies of baseline databases and legacy JSON fixtures, then
compare records and generated note/alignment output. CLI and desktop operate on
the same on-disk schema without a one-way migration.

### Phase 4 — make native capture private behind the port

- Create `margins-capture-native` with `publish = false` and move the three
  recorder files without algorithmic edits.
- Implement `CaptureProvider` as a thin adapter over the existing controller
  thread. Convert internal telemetry to value snapshots and event envelopes.
- Move CLI capture startup into injected services. Keep a temporary private
  compatibility facade for packaged native CLI builds; the public CLI uses an
  unavailable provider.
- Delete the feature-off recorder duplicate definitions once all consumers use
  core contracts.

Green gate: recorder unit tests and native rolling harness pass; before/after
WAVs preserve channel order, frame counts, offsets, retirement behavior,
timeline qualification, and drop diagnostics. `margins-cli` builds from the
public graph alone.

### Phase 5 — migrate desktop composition and event transport

- Inject provider/repository/event ports in `AppState`/`Ctx`.
- Move `recording.rs` from concrete recorder types to provider commands and
  snapshots. Keep its policy and UI-facing state private.
- Emit versioned envelopes through Tauri and WebSocket; temporarily dual-emit
  legacy events until frontend reducers and tests consume the envelope.
- Replace root `margins::*` imports with owning crates; delete three desktop
  re-export shims.
- Add the private `native-cli` bin/feature in the desktop composition package,
  then remove the temporary root native-CLI facade.

Green gate: Tauri default build, server-only build, desktop tests, TypeScript
tests/build, real-state CDP smoke, and first-run/native verification pass. The
server build does not resolve `margins-capture-native`, CPAL, CIDRE, Tauri, or
CoreAudio.

### Phase 6 — cut the public export and retire compatibility

- Remove the root umbrella/facades after downstream import search reaches zero.
- Add a deterministic allowlist export script and CI job.
- Generate lockfile/SBOM/license inventory from the exported tree, then build
  and test inside a clean directory with no access to the private checkout.
- Resolve the baseline license inconsistency: `Cargo.toml` declares Apache-2.0
  while `README.md`/`LICENSE` declare GPLv3. Every public package manifest,
  source distribution, and README must state the approved license consistently
  before export.

Green gate: both full monorepo and clean exported tree meet the acceptance
criteria below.

## Required validation matrix

Use the shared target directory required by repository policy:

```bash
export CARGO_TARGET_DIR=/Users/example/Hacks/margins-cargo-target
```

The eventual CI matrix must include:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo check -p margins-cli --no-default-features
cargo check -p margins-media --no-default-features
cargo check -p margins-media --features parakeet-onnx
cargo check -p margins-media --features coreml-asr              # macOS
cargo check -p margins-capture-native --all-targets             # macOS/Windows
cargo check -p margins-desktop --no-default-features --features server
cargo check -p margins-desktop --no-default-features --features native-cli --bin margins
cargo check -p margins-desktop                                  # macOS
cd desktop && npm test && npm run build
```

The native rolling Core ML harness remains the high-confidence local gate for
capture queue, checkpoint, drain, qualification, reuse, and injected-drop
behavior. Frontend-only tests are not a substitute for it.

## Measurable open-source export acceptance criteria

An export is acceptable only when all of the following are mechanically true:

1. **Allowlist:** the export contains only the six public crate directories,
   approved public resources, workspace manifests/lockfile, public docs/tests,
   and license files. It contains zero files under `crates/private/` or
   `desktop/`.
2. **Independent build:** after copying the export to an empty temporary
   directory with the original repository unavailable, `cargo metadata
   --locked`, `cargo test --workspace --all-targets --locked`, and the public
   feature matrix exit 0 on every supported public platform.
3. **No private edges:** machine inspection of all normal, target, build, and
   dev dependency tables finds zero references to `margins-capture-native`,
   `margins-desktop`, `tauri`, `cpal`, `cidre`, `windows`, `pi_agent_rust`, or
   any path outside the exported root. The public lockfile has zero unapproved
   Git dependencies.
4. **No private source leakage:** a denylist scan finds zero baseline-private
   symbols/strings (`SegmentWriter`, `LaneMsg`, `CaptureSink`,
   `AudioDeviceCreateIOProcID`, `margins-tap`, `CAPTURE_DEVICE_CHANGED_EVENT`),
   except their names in this architecture/audit document when that document
   is intentionally exported. A secret scanner reports zero findings.
5. **Contract compatibility:** version 1 event golden fixtures decode and
   re-encode byte-equivalent canonical JSON; unknown event kinds and payload
   fields survive round-trip; public API compatibility checking reports no
   unintended semver break after the first public tag.
6. **Session compatibility:** 100% of the checked-in baseline SQLite/legacy JSON
   fixtures migrate without loss of session IDs, segment count/order/offsets,
   artifact rows, grounding, note status, or tombstones. Reopening is
   idempotent and does not change the database a second time.
7. **Media compatibility:** golden WAV/transcript fixtures preserve channel
   order and frame counts exactly, resampling duration within one destination
   frame, word timestamps within the existing test tolerance, and deterministic
   alignment/note output.
8. **Private regression:** the full monorepo Tauri build, server-only build,
   desktop tests, native recorder tests, and native rolling harness remain
   green against the exact public crates exported at that revision.
9. **Feature isolation:** `cargo tree` for public no-default builds contains no
   platform capture or desktop stack. The server-only desktop tree contains no
   native capture crate. CI fails on a forbidden edge.
10. **Licensing/SBOM:** every exported file is covered by the approved license;
    crate manifest license fields and top-level license text agree; an SBOM and
    third-party notice inventory are generated; automated policy reports zero
    denied or unknown licenses.
11. **Documentation:** public rustdoc builds with warnings denied, every public
    port has at least one runnable example or contract test, and the README
    states precisely which capture/desktop capabilities are absent from the
    open export.
12. **Reproducibility:** running the export command twice from the same commit
    produces identical file lists and content hashes, and writes a manifest
    containing the source commit and SHA-256 for every exported file.

## Non-goals and review triggers

- This plan does not move files, rename commands, change storage, or change
  capture behavior.
- It does not promise that private capture will be published later.
- It does not expose desktop AI/auth/calendar/Granola integrations as public
  service APIs.
- It does not make JSON events a remote trust boundary; authentication and
  authorization remain transport responsibilities.
- Any proposal that adds a public-to-private edge, exposes a platform handle,
  moves the segment writer public, changes event version 1 semantics, or makes
  capture dynamically loadable requires architecture-owner review and an
  explicit amendment to this document.
