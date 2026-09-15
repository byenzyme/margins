# Remote Workspace implementation coverage

Implementation base: `4228ef5396074413cf4349365a5529132b4b956d`
Phase 1 checkpoint: `188d29b2c`
Native remote composition checkpoint: `46c8f6d4ed3185e6f32d9edee977f6b01212f539`

This report separates implemented portable behavior from platform and deployment
gates. “Supported” below means a production caller reaches the implementation;
it does not mean a real Mac, phone, installed application, or remote host was
tested.

## Acceptance plan and outcome

1. Map writers and cut over Workspace routing before adding transport.
2. Put local/service queries, receipts, memo, associations, jobs, and imports on
   the same canonical store.
3. Add scoped negotiation plus SSH/HTTPS adapters.
4. Add crash-recoverable client spooling and exact ACK/finalize behavior.
5. Adapt BB and Shortcut ingress to typed operations.
6. Run portable composed fixtures, real local HTTP/CLI processes, and record the
   remaining platform gates without inferring them.

Phases 0–1 have a portable production cutover and parity proof. Phase 2 is wired
for the supported read/query/import and application commands. Phase 3 composes the
native recorder with capture-time durable delivery and recovery. Phase 4 has the
typed BB capture adapter, exact-session agent handoff, and its 32-test suite. A real
Chromium/fake-device journey ran against separate Vite and production service
processes; the complete BB full-host UI journey remains environment-gated. Phase 5
has production multipart intake, receipts, pairing, and instructions;
the real-phone gate is unverified. Phase 6 has source-build and operator
packaging code plus operator documentation; native release archive, installation,
signing, and provisioned-host gates are unverified.

## Phase 0 — writer and operation map

### Persistence writers

| Caller | Production mutation path after cutover | Durable owner |
| --- | --- | --- |
| Public local CLI | `SessionStore` → `margins_store::canonical` | selected Captures binding’s `.margins/sessions.sqlite` |
| Native CLI and desktop recorder | direct device control remains local; session/segment/artifact calls use the same canonical module; TUI/native memo saves use `SqliteWorkspaceAuthorityStorage::replace_memo_lines` | same `sessions.sqlite`; no transport/spool on default local capture |
| Native desktop live memo | `SqliteWorkspaceAuthorityStorage::update_memo` / `replace_memo_lines` | `workspace_memos` + receipt rows in the same `sessions.sqlite`, with authority-owned Markdown projection |
| Hosted browser capture | existing bounded browser owner for device/WebM bytes; session/artifacts use canonical store and memo uses `SqliteWorkspaceAuthorityStorage` | same `sessions.sqlite` and confined artifact files |
| Remote capture service | `WorkspaceService` → `MeetingRuntime<SqliteMeetingRuntimeStorage>` | runtime state, receipts, events, canonical session/segment/artifact projection in one `sessions.sqlite` transaction |
| BB plugin | typed `/v1/.../browser/...` routes | hosted browser adapter above; no plugin call to generic `/api/invoke` or JSON audio route |
| Shortcut | multipart import → `WorkspaceService::import_finished_file` | immutable staged original + principal-scoped receipt + canonical artifact row |

The former public name `legacy` was removed; first-party code imports the selected
schema owner as `canonical`. `meeting-runtime.sqlite` is no longer a production or
test destination. At the implementation base it was instantiated only by tests,
so no production user-store move was needed. Canonical `sessions.sqlite` is opened
in place and extended transactionally. The operator guide explicitly refuses to
silently consume or delete any standalone experimental runtime database.

The superseded native/browser finalization writes that overwrote memo Markdown
directly were removed. Initial, periodic, final, imported, and whole-notepad memo
mutations now pass through the SQLite authority; Markdown remains a native readable
projection, not a competing revision owner. Native `Pause`/`Stop` still invoke the recorder implementation because local device
release must remain immediate and independent of memo, ASR, service, or network.
That is a capture-control adapter, not a second metadata store. The Stop behavior
from the first implementation remains: release synchronously, then bounded drain
and finalization with truthful incomplete state on failure.

### BB operation mapping

| BB behavior | Typed operation / authority | Portable status |
| --- | --- | --- |
| Project start | stable `MARGINS_INSTANCE_ID`, `MARGINS_WORKSPACE`, `MARGINS_HOME`, explicit notes/Captures provisioning | implemented |
| Capabilities | `GET /v1/capabilities` | implemented; ASR requires configured model assets and a usable runtime, while recall reflects the available compiled lookup mode |
| Start | `POST /v1/workspaces/{w}/browser/sessions` | implemented |
| Browser audio | binary `PUT .../browser/sessions/{s}/chunks/{n}` | implemented; bounded browser owner retained |
| Pause/resume/heartbeat/stop | typed browser routes | implemented; owner/generation fencing retained |
| Notepad | typed `PUT .../notepad`, canonical timed memo revision | implemented |
| Snapshot/remount | typed snapshot plus existing single browser owner/event fan-out | implemented in adapter; real remount journey unverified |
| Transcript/artifacts | shared Workspace query routes | server implemented; plugin UI journey unverified |
| Note association | typed session note-association GET/PUT/DELETE | server implemented; ordinary note bytes remain native filesystem data |
| Processing outcome | typed latest-job query | server implemented; full plugin UI journey unverified |
| Agent context | exact last-session pin plus stable instance/Workspace/session values | implemented; metadata-only handoff inserts into the BB composer and instructs native note-file access |
| Cancel/discard/recover | browser owner’s truthful discard/finalize rules; remote transfer list/retry | substrate implemented; end-to-end plugin recovery unverified |

The BB saved-flag schema and dismiss RPC remain removed. No lifecycle competitor was
reintroduced.

## Phase 1 — local parity and routing

- Explicit local `--workspace` resolves its single absolute Captures binding for
  public and native `new`/`attach`; it does not discover from cwd. A conflicting
  `--project` is rejected before writes. No-selector local behavior is retained.
- `margins-server` requires `MARGINS_WORKSPACE`; optional provisioning creates only
  an exact explicit mapping and rejects later mismatch.
- `WorkspaceService` is called directly in-process by local composition. Merely
  using local Margins does not start a service, hash/encode chunks, create a transfer
  spool, or allocate a remote worker.
- Runtime session reservation and finalized PCM projection share the canonical
  SQLite transaction with receipts/events/revision checks. Direct native/browser
  completed segment durations and remote explicit finalization reduce to the same
  `input_finalized` query fact. Finished-file imports reduce through the durable
  `original_audio` artifact fact.
- Native-live and browser whole-notepad mutations now read/update the same timed
  memo document and durable request receipts as the service. Stale expected
  revisions are rejected; delayed edits carry observed capture time and paused
  state.
- A real CLI fixture creates `public-input` from an unrelated cwd, then an
  in-process production `WorkspaceService` reads that exact session and transcript
  artifact, updates the same memo revision, and the authority-owned Markdown
  projection contains the edit. The browser composed fixture independently creates
  a hosted session and proves the service sees the exact title, artifact size, and
  memo revision.
- Note associations, processing jobs, and retention tables remain independent.
  Association setters perform no note-file write, job completion, or audio deletion.
  Completion does not delete audio; retention deletion remains preview/apply only.

Composed counterevidence addressed: an early PCM fixture used two-byte payloads
described as 100 ms. It was replaced with two distinct mono signed-16 48 kHz lanes,
4,800 samples per 100 ms chunk. The final fixture reconstructs 9,600 samples per
lane, asserts both chunk-boundary sample values, mono channel identity through lane
artifacts, and exactly 200 ms duration after service restart.

## Phase 2 — read-only remote access

- Remote selection order supports explicit `--remote`/`--local`, then
  `MARGINS_REMOTE`; remote use requires Workspace and rejects `--project` before
  local work.
- HTTPS and loopback-test clients negotiate protocol/instance/Workspace,
  capabilities and limits on connect. Plain remote HTTP is rejected.
- `ssh://` accepts an OpenSSH config alias only. Discovery uses a fixed command and
  the tunnel uses fixed forward/keepalive arguments; URL usernames, paths, shell
  commands, and arbitrary options are rejected.
- Remote CLI production support is wired for capabilities, `current`, `ls`,
  single-Workspace `recent`, rename, memo, note association, processing status,
  transcript, artifacts, recall, note handoff, and finished-file intake. `recent
  --all` is rejected before transport because no instance-wide authorization endpoint
  exists. Recall evidence exposes
  Source identity and Source-relative paths, never a source-file proxy.
- Unsupported commands return `remote_command_unsupported` before local mutation.
  The adapter now rejects them before opening HTTPS/SSH or reading a local input.

Private native composition routes remote `new` and `attach` through reservation,
the durable spool, capture-time delivery, timed memo, explicit segment close, and
finalization. A normal attach starts a new generation only from the exact prior
finalize identity; recovery attach retains the existing producer. The actual Mac
device/terminal journey remains a platform-verification gate, not an implementation
placeholder.

## Phase 3 — durable delivery

- A transfer manifest pins remote URL, instance, Workspace, session, producer token,
  and close/finalize intents. Owner-only directories/files contain the credential.
  Retry re-negotiates and refuses to send if the live instance differs from the
  preserved manifest.
- Each self-describing frame is atomically published and fsynced before scheduling.
  Payload digests and immutable command identity are checked. Conflicting bytes fail.
- Disk reserve, advertised chunk and in-flight limits, producer ownership, and
  immutable receipt replay are enforced. ACK evidence is fsynced before a frame is
  unlinked. Restart scans complete remaining frames; exact retries keep identity.
- One owner-only transfer lock serializes manifest merge, frame publication, ACK
  publication, and completion across capture and retry processes. Capture-time
  delivery writes only per-frame/per-control ACK deltas; close/memo/finalize setters
  reload and merge under lock. A deterministic stale-snapshot barrier test appends
  audio, a second close, memo, and finalize before a delayed control ACK, then reopens
  and proves no intent loss, ACK regression, frame deletion, or false completion.
- Reservation intent is persisted before reserve/attach. Promotion is serialized by
  a per-reservation lock, validates every immutable routing identity, and lets a
  concurrent loser reopen the completed spool rather than recreate it. A separate
  capture-lifetime file lease fences the loser across real processes. The regression
  first exposed and then corrected a `try_lock_exclusive` false-result handling bug.
- Memo receipts contain the request fingerprint. Finalization reopens the latest
  manifest under the transfer lock and holds that ownership across readiness,
  server finalization, and local completion, so a stale retry cannot overtake a
  concurrently appended memo. The deterministic regression retains a pre-memo
  snapshot and proves no finalize request is sent before the memo ACK.
- Close is accepted only after contiguous lane boundaries; finalize follows close.
  Producer release occurs only after successful finalization. Failed delivery keeps
  bytes and identity for `transfers list/retry`.
- Native remote capture uploads current chunks while recording with bounded retry,
  reports delivery-current versus locally pending bytes, and keeps Pause/Stop device
  release independent of network. Recorder recovery WAVs live inside the transfer
  directory and survive failed ACK/finalization; successful durable completion
  removes that duplicate and the producer credential. Default local capture never
  constructs this spool.

Portable tests cover restart before finalize, duplicate/lost-ACK replay, different
bytes under the same identity, producer/generation fencing, two sample-exact 48 kHz
mono s16 lanes, out-of-order arrival, normal attach after finalization, and service
reopen. The deterministic transport example exposes durable stops after spool,
chunk, close, or memo for cross-process fault injection. ASR is not part of capture
delivery and remains capability-truthful.

## Phases 4–6 — adapters and operations

- BB launcher selects one stable instance/Workspace and uses typed control/binary
  browser routes. Stop durably records an independent exact last-session pointer
  before clearing active capture. Remount restores that pin; connected-note context
  queries that exact transcript/memo/artifact/association metadata and inserts a
  native-filesystem-only instruction into the composer. No note, transcript, memo,
  or artifact bytes cross the BB host handoff. All 32 plugin tests pass.
- Scoped credential storage hashes tokens, uses an owner-only atomic file, checks
  Workspace/operation/expiry, and supports revocation. Browser routes enforce their
  specific operation scope. Shortcut credentials contain import/receipt only.
- `POST /v1/workspaces/{w}/imports` accepts multipart metadata plus the original
  file. Stable upload IDs return the original durable receipt for identical bytes
  and conflict for changed bytes. The older id-addressed binary PUT is retained as
  the resumable CLI transport, not a note/source proxy.
- The CLI release and validation workflows build and package both `margins-private`
  and `margins-server` for each supported source-build target. The first-party
  headless launcher now provisions an explicit test Workspace, so the mandatory
  routing variable does not break development startup.
- [Remote operations](remote-workspace-operations.md) documents explicit service
  provisioning, SSH/HTTPS, backup/recovery, capability semantics, and revocation.
  [Shortcut instructions](../integrations/shortcuts/README.md) contain no credential.

Actual iPhone background behavior, app switch/lock, cellular retry, Mac permissions,
native system audio, installed application, signing/package, published installation,
and provisioned remote-host behavior were not run and remain release gates.

## Verification evidence

Available baseline before changes:

- `scripts/cargo-lane disposable -- cargo test -p margins-store -p margins-workflows`
  passed: store unit 13, application 5, index 3, compatibility 3, runtime SQLite 5,
  public 2, repository 6; workflows unit 189 with 1 ignored; all integration suites.
- Prior evidence from the companion coverage: desktop Rust 367 passed with 3
  capability tests ignored; desktop frontend 155; BB 28.
- Known portable root counterevidence remains
  `integrations_recall_sqlite_source recall_unavailable_stale_materialization`
  after 529 recall-engine tests; it reproduced alone on the base, with four sibling
  failures from poisoned-lock fallout. The final broad lane re-evaluated it after
  the Source-relative recall serialization change and reproduced the same first
  failure and four poisoned-lock follow-ons.

Implementation checks run from the canonical worktree/origin:

```text
scripts/cargo-lane disposable -- cargo test -p margins-workflows --test workspace_service --test remote_workspace
  focused suites passed, including service/auth, attach generation, six
  transport/spool tests, stale-manifest and concurrent-promotion barriers

temporary ALSA pkg-config metadata + scripts/cargo-lane disposable -- cargo check
  -p margins --no-default-features --features audio-capture
  passed; this is only a Linux type/ownership check, not an audio or linker claim;
  temporary metadata was removed immediately afterward

scripts/cargo-lane disposable -- cargo test -p margins-store -p margins-workflows
  passed after cutover: store suites unchanged at 13/5/3/3/5/2/6;
  workflows 190 passed, 1 ignored, plus every integration suite

scripts/cargo-lane disposable -- cargo test -p margins-cli --test command_contract \
  workspace_transcribe_keeps_generated_artifacts_out_of_notes_sources -- --exact
  1 passed; real SQLite CLI-create/service-read/service-mutate parity

scripts/cargo-lane disposable -- cargo test -p margins-cli --test command_contract \
  unsupported_remote_command_fails_before_transport_or_local_mutation -- --exact
  1 passed; 68 filtered

scripts/cargo-lane disposable -- cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features server \
  web_session::tests::hosted_writer_and_workspace_service_share_session_title_artifact_and_memo_facts \
  -- --exact --test-threads=1
  1 passed; 380 filtered; 95 s cold disposable compile, test 0.17 s

scripts/cargo-lane shared -- cargo check -p margins-cli -p margins-workflows --tests
  passed

scripts/cargo-lane shared -- cargo check -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features server
  passed

scripts/cargo-lane disposable -- cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features recall \
  -- --test-threads=1
  368 passed, 3 ignored after the memo-authority cutover

  Counterevidence from the first broad run: 367 passed, 1 failed, 3 ignored;
  `whole_notepad_updates_reuse_the_shared_timed_memo_model` hit a foreign-key
  failure because its old fixture bypassed canonical session creation. The fix
  made the fixture production-shaped and removed the remaining direct final-memo
  writers; the focused test and the complete 371-test lane then passed.

scripts/cargo-lane disposable -- cargo test --workspace --no-default-features --features recall
  529 recall-engine tests passed, then the known
  integrations_recall_sqlite_source stale-materialization failure reproduced;
  four sibling tests failed only after the shared test lock was poisoned

cd integrations/bb-plugin-margins && npm run typecheck && npm test
  7 files, 32 tests passed

cd desktop && npm ci && npm run build
  passed

scripts/cargo-lane shared -- cargo check -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features \
  --features hosted-web --bin margins-server
  passed; capability probing validates configured assets and dynamic ONNX runtime

temporary ALSA pkg-config metadata + scripts/cargo-lane disposable -- cargo check \
  -p margins --no-default-features --features audio-capture --bin margins-private
  passed; Linux type/ownership evidence only, temporary metadata removed
```

A final rebuilt `margins-server` was started on a disposable loopback port with an
explicit provisioned Workspace. Authenticated HTTP import and identical retry were
then queried from both remote-mode and direct local `margins-private` processes;
both returned session `http-cli-parity` from the same `sessions.sqlite`. Capabilities
reported `instance=e2e-final`, `workspace=parity`, `asr=false`, `recall=false`.
The remote artifact was `original_audio`; the second multipart upload returned the
same receipt with `replayed=true`; service discovery succeeded without a Workspace
flag because the instance serves exactly one Workspace. An upload-only credential
received 403 for session listing, an invalid credential received 401, and the
revoked credential received 401. Only `sessions.sqlite` existed; no experimental
runtime database was created.

The production headless server initially exposed a composed startup failure that
unit construction had missed: `WorkspaceService` opened libsql before the process
initialized the shared native SQLite runtime, and libsql rejected the later
`SQLITE_CONFIG_SERIALIZED` call. Moving runtime initialization before the canonical
Workspace open fixed the real process; the rebuilt server then started normally.

A Docker-isolated Chromium with a 30-second 437 Hz, 48 kHz mono fake microphone
drove the real frontend through Vite to a separate production `margins-server`.
During capture, durable WebM counters reached 5 chunks / 243,674 bytes and later
15 / 731,664 bytes. Across a five-second pause they stayed exactly fixed; five
seconds after resume they grew from 16 / 767,406 to 18 / 865,004. Finish preserved
session `2026-09-15-08-41`, a 3,790,148-byte recovery WAV, canonical SQLite, and
the capture-context artifact. With no model/runtime configured, processing failed
truthfully while capture remained saved. Counterevidence found during this run was
that capabilities originally advertised ASR from the compile feature alone; the
rebuilt endpoint now reports `asr_available=false` after checking both model assets
and the dynamic runtime (`recall_available=true` for compiled local lookup). The
unauthenticated `/health` response now reports only liveness, version, and protocol;
it no longer presents compile features as supported runtime capabilities.

The production `remote_pcm_transport` example then sent distinct 437 Hz and 913 Hz
two-second lanes through the real HTTP client, durable spool, loopback server, and
canonical artifact projection. The receipt reported 96,000 frames per lane and
2,000 ms. Both 192,000-byte server artifacts compared byte-for-byte equal to their
input raw s16 files; mic SHA-256 was
`97e50c76d56fe80e588d3eff764c4dffa6b0695dc7123f65ab12485c92e37fbf`
and system SHA-256 was
`c7963073a991ed19c81f06c4174c51062250f5c02a076793bba5c40a0637c1c9`.

Mac candidate preparation found that Cargo could reuse the CLI build script's output
after a linked-worktree branch switch, leaving a new executable labeled with an old
branch, commit, and build time. The build script now watches the absolute per-worktree
`HEAD` and index plus the symbolic ref target and common `packed-refs`. A command
contract regression compares the embedded commit with the checkout's real Git HEAD
when Git is available. On the shared target, an initial build embedded clean commit
`50b61ab8e37d1e35d9f56992de1bef45a4b6a4e8` at `09:12:51Z`; advancing the ref to
`b7c7d08da0ed18cb007f9b06cc4877cb850e7df1` and rebuilding without cleaning caused
Cargo to recompile `margins-cli` and embed that exact new commit at `09:13:12Z`.
The prior Mac binary whose metadata still named `46c8f6d4e` is rejected as evidence.
The first corrected-candidate Mac run also found that the documented
`audio-capture`-only native composition test hard-coded all optional recall
features as present. Its assertions now follow the actual `recall` and
`recall-local-model` feature matrix, so the canonical native lane verifies capture
without falsely requiring or advertising unrelated optional capabilities.

For backup/restore, the stopped isolated state tree was copied byte-for-byte,
restored to its exact absolute Workspace paths, and restarted. The service returned
the same finalized session, and the restored WAV SHA-256 matched
`b59ebd32565b0e997d801910107e7652e1631c324a5103fa39ef95678685efe6`.

A portable debug archive check installed `margins` and `margins-server` under the
same two-file topology used by CI, produced and verified a checksum, extracted both
as executables, and then ran the official smoke script. The smoke correctly rejected
this local no-audio binary as lacking a usable capture composition. This host has no
linkable ALSA development library, so the native Linux release-feature archive could
not be produced without installing system packages; that rejection is counterevidence,
not a release result. The committed CI source builds the audio composition and server
before packaging, while its actual release run remains unverified here.

## Robustness, latency, and burden

Measured: the composed service/spool tests complete in well under one second after
compile (service fixture 0.34 s; spool 0.01 s). The desktop parity test itself took
0.17 s. Cold disposable compile time is not runtime latency and is reported
separately above. BB increased from the prior 28-test baseline to 32/32 and the
frontend production build passed. The browser counter measurements above are
protocol/storage evidence, not native audio-fidelity or native latency evidence.

Inferred, not measured on native hardware: the direct local path adds only canonical
SQLite memo/session work already required for durability; it does not add network,
wire encoding, duplicate audio, spool hashing, or remote threads. Therefore the
design avoids an architectural local callback-pressure regression, but recorder
readiness, callback pressure, native Stop latency, and warm CLI startup still need
comparable Mac measurements. No native-performance claim is made from Linux builds.

The principal implementation burden versus the old unit-only substrate is the
production adapters, owner-scoped credentials, explicit Workspace mapping, immutable
spool/receipt bookkeeping, and crash-boundary tests. Simplicity is recovered by one
canonical database, one shared service, typed first-party routes, no source-file or
note-publishing API, no background delivery daemon, and no remote work in default
local capture.

## Remaining gates

1. Run the exact committed private CLI on Mac for local and SSH remote `new`/`attach`,
   pause/resume/Stop, capture-time network loss, memo, recovery, and measured latency.
2. Run the complete BB full-host UI journey on an environment that can install/load
   this plugin, including remote remount, stale callbacks, conflicts, recovery,
   artifacts, jobs, and the now-wired agent handoff. Typed production host/client,
   remount, and exact-session contract tests are complete; the full BB host UI is not.
3. Run real Mac microphone/system lanes and measure readiness/callback/Stop latency
   (delegated to the authorized Mac verifier; no result claimed here yet).
4. Run the real Shortcut share/retry/app-switch/lock/cellular matrix.
5. Verify signed/published clean-client installation and supported OS service
   installation. Portable archive topology/checksum/extraction and stopped-state
   backup/restore are verified; the no-audio package failed the native capture smoke
   as expected. No push, merge, production deployment, credential change, or release
   was performed here.
