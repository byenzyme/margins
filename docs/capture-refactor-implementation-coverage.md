# Capture refactor foundation: implementation coverage

Date: 2026-09-15

Implementation base: `c954f879b70e3f2177387d88cc63786a4139db6f`

Scope: sections 1–9 of `capture-refactor-and-testing.md`

This report covers the local/direct and BB capture foundation only. It does not
implement remote Workspace authority, SSH, remote spooling, HTTPS exposure, or
phone/Shortcut intake from `remote-workspace-implementation.md`. Those changes
must be based on this committed tip in a separate worktree.

## Requirement coverage

| Design requirement | Implementation | Meaningful evidence |
| --- | --- | --- |
| 1. One session control seam | Reconciled `TimedMemoDocument` and `LiveRuntime` from `5a703dc5a`, `777e45922`, and `36b10450c`; folded the public `LiveRuntime` contract into `margins-meeting-runtime`; updated desktop and BB protocol callers. This consolidates the public trait and DTO vocabulary, but the production `NativeLiveRuntime` adapter still delegates to existing desktop handlers rather than the new `MeetingRuntime<SqliteMeetingRuntimeStorage>`. | Protocol/runtime suites exercise the typed command state machine. No `DesktopLive*` alias remains. The production-wiring limitation is called out below. |
| 2. Preserve proven capture behavior | Kept capture-first worker startup, bounded live-ASR queues, digest/discontinuity/finalization invariants, ID-scoped recovery, and durable hosted input behavior from the cited branches. | Existing recorder, recovery, runtime race, invalid-command, and finalization tests remain in the portable suites. |
| 3. Independent state ownership | Browser producer state is locally controlled; authority acknowledgements remain separate. Memo revisions, ASR health, note associations, processing attempts, and artifact retention remain independent facts. | Stalled memo/transport/ASR tests and application-record SQLite tests assert that one fact cannot block or overwrite another. |
| 4. Immediate control and isolated data paths | BB Pause stops production before awaiting authority. Stop now has explicit phases: update local UI and synchronously stop the recorder/release tracks; await only bounded final recorder data and durable upload drain; then invoke authority Stop. A drain failure or error envelope retains the Stop identity and projects `needs_attention`, never `saved`. Durable WebM and optional PCM retain different backpressure policies. Web chunk ACK follows `sync_data`. | `browser-capture.test.ts`, `app.test.tsx`, `web-durable-upload.test.ts`, and the real `web_session` stalled-worker test cover a held last upload, Pause, resolved Stop errors, same-ID retry, failed drain, lost ACK, missing stop event, bounded queues, and ASR isolation. |
| 5. Bounded persistence substrate | Replaced whole-`StoredSessionV1` load/replace in the meeting-runtime contract with compact session metadata, immutable receipts, paginated events, targeted chunk reads, `create_session(SessionDeltaV1)`, and transactional `apply_delta`. Added `SqliteMeetingRuntimeStorage` in `margins-store`; blob data is fsynced and renamed before metadata/receipt commit. This implementation is currently instantiated by the real SQLite tests only; it is not yet the production desktop/CLI session owner. | In-memory runtime invariants plus real SQLite restart, failpoint, missing-coverage, concurrent-CAS, exact-retry, and 256-chunk scaling tests. Compact runtime session JSON stays below 2 KiB and does not accumulate audio or event bodies. |
| 6. Independent application records | Added revisioned source-relative `session_note_associations` and named/attempted `session_processing_jobs`. `link_note` and `unlink_note` have no job or retention side effects. A coordinator transaction atomically publishes an association and completes the exact job attempt, rejecting cancellation, supersession, and tombstones. Missing note files are derived availability and no longer delete sessions. | Real SQLite tests reopen records, verify exact retries and revision conflicts, preserve failure/audio through link/unlink, and reject late publication. Workflow tests cover missing/renamed notes. |
| 6. Explicit retention | Removed note-completion-triggered audio deletion. Existing artifact retention remains independently classified and is applied only through explicit artifact prune; explicit session deletion retains its existing user-directed cleanup. | Application-record tests assert link/unlink and job completion retain audio. Artifact confinement/prune tests remain green. |
| 7. Single BB client owner | A route-independent `BrowserCaptureOwner` owns the producer, current snapshot, draft, heartbeat, refresh, and operation identity. Panels subscribe to it; per-panel polling and save-before-control coupling were removed. Server-side competing lease expiry and the persistent BB `saved:*` flag/schema were removed. The BB server retains only the capture ownership pointer and idempotent control receipt; a successful Stop response is projected directly from the Margins host result. | Panel remount/draft test; one-owner test; stalled memo Pause/Stop tests; project-routing and non-owner visibility tests; server test proves exact Stop receipt replay without a `saved:*` KV value and a fresh authoritative snapshot returns `ready`. |
| 7. Bounded network uncertainty | Added bounded fetch covering both headers and body, one heartbeat and one refresh in flight, last-ACK watchdog expiry, generation fencing for late callbacks, stable mutation IDs, and receipt reconciliation after an uncertain Stop. | Hung-header/body tests, hung-heartbeat local-expiry test, late callback tests, repeated Stop receipt test, and lost-ACK durable-upload test. |
| 8. Composed verification | Tests use the real controller, upload queue, runtime, SQLite adapter, desktop web-session functions, and BB adapter. Barriers/watchdogs hold the real ASR worker while audio/status/memo proceed. | See verification matrix below. The suite deliberately avoids a full adapter-by-fault cross product. |
| 9. Removal and partial cutover | Deleted the standalone `margins-live-runtime` crate, aggregate writes from the refactored runtime storage contract, `DesktopLive*` protocol names, panel polling, BB saved-state persistence/server lease timer, overloaded note setters/synchronizers, deleted-note session pruning, and automatic aligned-transcript audio cleanup. First-party consumers of those removed interfaces and generated BB bundles were updated together. Production desktop capture persistence has not been cut over to `SqliteMeetingRuntimeStorage`; that integration remains a Phase 1 gate for the next document. | Repository search finds no code callers for the removed names and no BB `saveSaved`/`savedKey`; isolated public-crate construction verifies the new dependency graph. Legacy database conversion is additive and idempotent. |

## Persistence and data preservation

- Existing session tables and user files are not reset or cleaned. Opening an
  older database additively projects valid note/failure state into the new
  association and job tables while retaining the old columns as a recoverable
  source copy.
- Existing JSON session import remains one-shot and leaves invalid input intact.
- Runtime blob staging uses immutable files. A crash after staging and before
  metadata commit produces no false receipt; the same command can recover it.
- Session deletion tombstones still fence late artifact, association, and job
  writes. No fixture or recording deletion was used for the refactor.

## Verification matrix

| Lane | Coverage | Result |
| --- | --- | --- |
| BB plugin | 28 Vitest cases across app, browser owner, and server adapter; TypeScript check; plugin production build | Pass. Stop ordering is composed through the real `BrowserCaptureOwner`, `MediaRecorder` binding, and `WebDurableUploadQueue`: a deferred last upload proves host Stop cannot overtake durable delivery. |
| Desktop browser | 155 Node tests and production Vite/TypeScript build | Pass |
| Desktop controller isolation | `web_session::tests::stalled_asr_does_not_serialize_durable_audio_status_or_memo` with a blocked production worker loader and one-second outer watchdog | Pass |
| Store/workflows | Complete `margins-store` and `margins-workflows` suites, including real SQLite, legacy conversion, artifact confinement, and isolated public graph | Pass (one credential-gated Granola probe ignored as designed) |
| Portable root | `cargo test --workspace --no-default-features --features recall` through `scripts/cargo-lane disposable` | Capture/runtime/store/workflow and 529 recall-engine tests passed. The sweep later failed in the unchanged `integrations_recall_sqlite_source` test: its expected post-refresh-error recall is rejected as `recall_unavailable_stale_materialization`; four following tests see its poisoned global lock. The same test fails when rerun alone with one test thread. Neither that test nor its recall/integration implementation differs from the base commit. |
| Portable desktop | `cargo test -p margins-desktop --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features recall -- --test-threads=1` through `scripts/cargo-lane disposable` | Pass: 367 passed, 3 capability-gated tests ignored. |

Representative HTTP behavior is covered by the desktop browser suite's actual
async HTTP preflight and durable WebM response-envelope cases. The BB adapter is
covered through its production RPC server/controller tests. Direct session
semantics are covered by the shared Rust runtime suite; the existing desktop
loopback routes retain exact-token, route-prefix, memo-revision, and replay
contract tests. The remote service contract and transport matrix belong to the
second implementation document.

## Production cutover boundary for the second implementation

The bounded runtime/store work in this branch is a foundation, not the single
production persistence owner yet:

- `SqliteMeetingRuntimeStorage` is exported from `crates/public/margins-store`,
  but repository search finds constructions only in
  `crates/public/margins-store/tests/meeting_runtime_sqlite.rs`.
- `desktop/src-tauri/src/desktop_live.rs::NativeLiveRuntime` is the production
  `LiveRuntime` implementation. Its commands still call
  `start_recording_impl`, `pause_recording_impl`, `resume_recording_impl`,
  `stop_recording_impl`, and the existing notepad/session functions over
  `AppState`.
- Those handlers continue to create and update the established `.margins`
  session files and legacy store records. They do not route through
  `MeetingRuntime<SqliteMeetingRuntimeStorage>` today. Accordingly, section 5's
  goal of one persistence owner is proven for the new runtime substrate but not
  completed across production CLI/desktop/web capture writers.

`remote-workspace-implementation.md` Phase 1 explicitly owns this remaining
cutover: make local CLI and service observe the same session IDs/artifacts and
one writer, route production clients through the bounded repository, convert
existing data additively, and then delete the superseded desktop/path-scoped
writes. The second child must treat this as required local service parity before
adding SSH, HTTPS, phone, or remote delivery paths. No claim in this report uses
the test-only SQLite construction as evidence of production consolidation.

## Deletions and intentionally retained seams

Deleted:

- `crates/public/margins-live-runtime/`
- whole-session `MeetingRuntimeStorage` load/replace operations
- `DesktopLive*` compatibility protocol types
- BB per-panel refresh intervals and server-owned disconnect expiry loop
- BB `saved:*` KV state, its schema, and the saved-dismiss RPC
- `set_vault_note_path`, note error/processing bundled setters, metadata sync,
  deleted-note session pruning, and completion-triggered audio cleanup

Retained deliberately:

- the compact `StoredSessionV1` name as the one session metadata record, rather
  than adding another session abstraction
- old SQLite columns as non-authoritative migration source data, because SQLite
  destructive table reconstruction would add risk without product value
- explicit user-requested whole-session deletion and independent artifact
  retention/prune commands
- direct local recorder files with no mandatory spool, wire encoding, or service
  startup
- existing production desktop session/file writers until the Phase 1
  single-owner cutover above is implemented and verified

## Verification not available on this host

The following evidence remains explicitly outstanding and is not inferred from
portable tests:

- macOS microphone/system-audio stop latency and callback/drop measurements
- real browser `MediaRecorder` device/codec behavior outside the deterministic
  controller harness
- native CoreML rolling readiness/shutdown and Mac installed-app behavior
- before/after native CPU, RSS, disk-write, and latency measurements
- Mac-to-service recovery, phone capture, and Shortcut intake

No native app was installed or reinstalled, no paid model was invoked, and no
remote/SSH/phone implementation was started in this foundation worktree.
