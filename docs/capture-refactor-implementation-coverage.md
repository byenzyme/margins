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
| 1. One session control seam | Reconciled `TimedMemoDocument` and `LiveRuntime` from `5a703dc5a`, `777e45922`, and `36b10450c`; folded `LiveRuntime` into `margins-meeting-runtime`; updated desktop and BB callers. | Protocol/runtime suites exercise the production state machine and typed commands. No `DesktopLive*` alias remains. |
| 2. Preserve proven capture behavior | Kept capture-first worker startup, bounded live-ASR queues, digest/discontinuity/finalization invariants, ID-scoped recovery, and durable hosted input behavior from the cited branches. | Existing recorder, recovery, runtime race, invalid-command, and finalization tests remain in the portable suites. |
| 3. Independent state ownership | Browser producer state is locally controlled; authority acknowledgements remain separate. Memo revisions, ASR health, note associations, processing attempts, and artifact retention remain independent facts. | Stalled memo/transport/ASR tests and application-record SQLite tests assert that one fact cannot block or overwrite another. |
| 4. Immediate control and isolated data paths | BB Pause stops production before awaiting authority. Stop releases tracks immediately, bounds the missing `MediaRecorder.stop` event and upload drain, and continues authority reconciliation separately. Durable WebM and optional PCM retain different backpressure policies. Web chunk ACK follows `sync_data`. | `browser-capture.test.ts`, `app.test.tsx`, `web-durable-upload.test.ts`, and the real `web_session` stalled-worker test cover Pause, Stop, lost ACK, missing stop event, bounded queues, and ASR isolation. |
| 5. Bounded canonical persistence | Replaced whole-`StoredSessionV1` load/replace with compact session metadata, immutable receipts, paginated events, targeted chunk reads, `create_session(SessionDeltaV1)`, and transactional `apply_delta`. Added one SQLite adapter in `margins-store`; blob data is fsynced and renamed before metadata/receipt commit. | In-memory runtime invariants plus real SQLite restart, failpoint, missing-coverage, concurrent-CAS, exact-retry, and 256-chunk scaling tests. Compact session JSON stays below 2 KiB and does not accumulate audio or event bodies. |
| 6. Independent application records | Added revisioned source-relative `session_note_associations` and named/attempted `session_processing_jobs`. `link_note` and `unlink_note` have no job or retention side effects. A coordinator transaction atomically publishes an association and completes the exact job attempt, rejecting cancellation, supersession, and tombstones. Missing note files are derived availability and no longer delete sessions. | Real SQLite tests reopen records, verify exact retries and revision conflicts, preserve failure/audio through link/unlink, and reject late publication. Workflow tests cover missing/renamed notes. |
| 6. Explicit retention | Removed note-completion-triggered audio deletion. Existing artifact retention remains independently classified and is applied only through explicit artifact prune; explicit session deletion retains its existing user-directed cleanup. | Application-record tests assert link/unlink and job completion retain audio. Artifact confinement/prune tests remain green. |
| 7. Single BB client owner | A route-independent `BrowserCaptureOwner` owns the producer, current snapshot, draft, heartbeat, refresh, and operation identity. Panels subscribe to it; per-panel polling and save-before-control coupling were removed. Server-side competing lease expiry was removed. | Panel remount/draft test; one-owner test; stalled memo Pause/Stop tests; project-routing and non-owner visibility tests. |
| 7. Bounded network uncertainty | Added bounded fetch covering both headers and body, one heartbeat and one refresh in flight, last-ACK watchdog expiry, generation fencing for late callbacks, stable mutation IDs, and receipt reconciliation after an uncertain Stop. | Hung-header/body tests, hung-heartbeat local-expiry test, late callback tests, repeated Stop receipt test, and lost-ACK durable-upload test. |
| 8. Composed verification | Tests use the real controller, upload queue, runtime, SQLite adapter, desktop web-session functions, and BB adapter. Barriers/watchdogs hold the real ASR worker while audio/status/memo proceed. | See verification matrix below. The suite deliberately avoids a full adapter-by-fault cross product. |
| 9. Removal/cutover | Deleted the standalone `margins-live-runtime` crate, aggregate storage writes, `DesktopLive*` protocol names, panel polling, server lease timer, overloaded note setters/synchronizers, deleted-note session pruning, and automatic aligned-transcript audio cleanup. First-party callers and generated BB bundles were updated together. | Repository search finds no code callers for the removed names; isolated public-crate construction verifies the new dependency graph. Legacy database conversion is additive and idempotent. |

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
| BB plugin | 24 Vitest cases across app, browser owner, and server adapter; TypeScript check; plugin production build | Pass |
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

## Deletions and intentionally retained seams

Deleted:

- `crates/public/margins-live-runtime/`
- whole-session `MeetingRuntimeStorage` load/replace operations
- `DesktopLive*` compatibility protocol types
- BB per-panel refresh intervals and server-owned disconnect expiry loop
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
