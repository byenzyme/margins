# Remote Workspace implementation coverage

Implementation base: `4228ef5396074413cf4349365a5529132b4b956d`
Phase 1 checkpoint: `188d29b2c`
Native remote composition checkpoint: `46c8f6d4ed3185e6f32d9edee977f6b01212f539`
16 kHz remote ASR candidate: `68e6a07741fc23172436d3901fc9ae296ed35611`

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
packaging code plus operator documentation. Native remote finalization now admits
a durable, restart-recoverable ASR-only job and a bounded service worker publishes
the transcript without delaying capture finalization. Native release archive,
installation, signing, and provisioned-host gates are unverified.

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
bytes under the same identity, producer/generation fencing, direct sample-exact
16 kHz mono s16 lanes, preserved replay of former 48 kHz manifests, out-of-order
arrival, normal attach after finalization, and service reopen. New device-native
44.1/48 kHz f32 input passes a persistent per-lane, anti-aliasing resampler in the
bounded spool worker; callback partitioning does not change bytes, tails use an
exact ceiling frame count, and rate changes inside a segment are rejected. The
audio callback remains free of resampling, disk, hashes, network, and ASR. The
deterministic transport example exposes durable stops after spool, chunk, close,
or memo for cross-process fault injection. ASR remains outside capture delivery:
successful finalization admits the job durably before producer release, then
returns without waiting for the bounded service worker.

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
- The Linux service uses the same logical Parakeet TDT 0.6b v2 model as the Mac
  default, with platform-specific packaging. The verified pinned Linux export is
  `smcleod/parakeet-tdt-0.6b-v2-int8` revision
  `d64884b484b919e9656d0b70cb95dfdc98852bef`: 631 MiB of model assets (652,282,300
  byte int8 encoder, 8,998,557 byte int8 decoder/joint, 9,384 byte vocabulary)
  plus a 23 MiB extracted official ONNX Runtime 1.24.4. This is not the v3 model
  and the runtime is not responsible for the model-size difference versus the
  Mac's roughly 464 MB 6-bit-palettized/mixed-precision CoreML v2 bundle.
- Remote PCM finalization now admits one stable `transcribe_session` attempt for
  its exact meeting revision before producer release. A dedicated bounded worker
  reads canonical audio artifacts, honors each lane's declared rate, retains the
  originals, publishes/registers a transcript, and durably records complete or
  failed state. Startup scans queued/running jobs. It never publishes note bytes or
  converts a note association into job success.
- [Remote operations](remote-workspace-operations.md) documents explicit service
  provisioning, SSH/HTTPS, backup/recovery, capability semantics, and revocation.
  [Shortcut instructions](../integrations/shortcuts/README.md) contain no credential.

Actual iPhone background behavior, app switch/lock, cellular retry, native Mac
microphone/system capture, installed application, signing/package, and published
installation were not run and remain release gates. An isolated server on an
existing authorized Tailnet host was exercised without replacing a production
service.

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

scripts/cargo-lane disposable -- cargo test -p margins-workflows \
  --test remote_workspace --test workspace_service -p margins-store \
  --test meeting_runtime_sqlite --no-default-features
  19 passed; direct16 exact lanes, 44.1/48 partition-invariant resampling,
  anti-alias rejection, tail/frame timing, unsupported rate changes, legacy48
  recovery labeling, ASR admission/replay/restart, spool races, and SQLite CAS

scripts/cargo-lane disposable -- cargo check -p margins-workflows \
  --example remote_pcm_transport --no-default-features
  passed; the production verification entrypoint now consumes direct16 s16 fixtures

ORT_DYLIB_PATH=/tmp/.../libonnxruntime.so.1.24.4 \
MARGINS_PARAKEET_MODEL_DIR=/tmp/.../parakeet-tdt-0.6b-v2-int8 \
MARGINS_PARAKEET_MODEL_KIND=tdt-v2 \
MARGINS_PARAKEET_SMOKE_WAV=/tmp/.../spoken-fixture-16k-mono-s16.wav \
MARGINS_PARAKEET_EXPECTED_PHRASES='margins verification|alpha lane 437|system lane 913|stop now' \
scripts/cargo-lane disposable -- cargo test -p margins-media \
  --no-default-features --features parakeet-onnx-dynamic \
  smoke_transcribes_wav_with_parakeet_onnx -- --ignored --nocapture
  1 passed; real pinned v2 model returned all expected words

ORT_DYLIB_PATH=/tmp/.../libonnxruntime.so.1.24.4 \
MARGINS_PARAKEET_MODEL_DIR=/tmp/.../parakeet-tdt-0.6b-v2-int8 \
MARGINS_PARAKEET_MODEL_KIND=tdt-v2 \
MARGINS_REMOTE_ASR_SPOKEN_PCM=/tmp/.../spoken-fixture-16k-mono-s16.pcm \
MARGINS_REMOTE_ASR_SILENCE_PCM=/tmp/.../system-silence-16k-mono-s16.pcm \
scripts/cargo-lane disposable -- cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features \
  --features hosted-web finalized_remote_pcm_runs_durable_asr_job_and_retains_exact_audio \
  -- --ignored --nocapture
  1 passed in 7.24 s after compile; real finalize, durable job, v2 inference,
  transcript registration, expected words, and exact retained mic bytes

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

That 48 kHz run is retained as pre-conversion recovery evidence, not the new wire
format. Candidate `68e6a07741fc23172436d3901fc9ae296ed35611` was built on the
shared lane and started as an isolated service on loopback port 38871 with the
pinned v2 model. The server SHA-256 was
`38bf3ce7709ebd48c9dfd5211a560b0c4a8a2c0ac50b2b24c3d8258435e36fd3`;
the exact discovery CLI was
`d81331bb781bb36088f16eafd922dc37eb8db4cee8dc900eb548b78cd545e12a`.
Authenticated capabilities reported `instance=candidate-68e6a077`,
`workspace=mac-e2e`, `asr=true`, and exactly mono/raw/s16/16 kHz capture.

A production HTTP client/spool/service run then sent the 6.12275-second spoken mic
fixture and duration-aligned system silence. Each lane contained 97,964 frames and
195,928 payload bytes; this is one third of 48 kHz s16 payload for the same frame
timeline (request count/headers are separate overhead). Server artifacts remained
byte-identical to the direct16 inputs: mic SHA-256
`a88e718bd931fc87d6015e5b846082595e1bdb3c4f14ca3cc13ec9f94ec1dc68`,
system SHA-256
`bf78d8f24d4537b85911acdd2755fcffc48fb22b176289f86ba17cf0bf3fdc5b`.
The canonical segment contract recorded 16,000 Hz, mono signed16, 97,964 frames,
195,928 bytes, and 6,122 ms. Finalize returned while processing was independent;
job `transcribe:<session>` attempt 1 reached complete, and CLI transcript/artifact
queries returned one durable transcript containing “Margins Verification”, “Alpha
Lane 437”, “System Lane 913”, and “Stop Now”, alongside both retained audio lanes.

A second direct16 transfer stopped after its durable memo: 124 audio chunks, close,
and memo were acknowledged while finalize and ASR had not started. After stopping
only the scoped server and reopening the same binary/state, `transfers retry`
finalized that identity; the job was first observable running and then complete,
the same expected transcript appeared, chunk files were reclaimed only after
durability, and both server audio hashes still matched. In a separate disposable
Workspace, a durable attempt was staged as `running` before process start; startup
rediscovered and completed attempt 1 and retained exactly one transcript artifact.
This latter test directly exercises restart scanning, while the former exercises
the real transfer/finalize ordering boundary.

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

The authorized Mac verifier built the exact committed `audio-capture` composition
through the managed shared lane, confirmed its embedded full Git identity and
`dirty=false`, and passed the canonical native composition test 1/1. Across the
real `ssh://bs-server` discovery and Tailscale boundary, distinct two-second,
48 kHz mono s16 fixtures arrived as 20 bounded chunks and 96,000 samples per lane.
The canonical server artifacts matched the source bytes exactly (mic SHA-256
`b9ea16f54ebde363da595804b0d486a5951826894f029cc87f36f5a5e36c2491`, system
SHA-256 `fdb819f615731eb33c423ab0ada1467770c1b4ce7c96f3d3991c08de4c503428`),
the timed memo remained readable, close preceded the single finalize at 2,000 ms,
and successful completion removed the local producer credential.

Composed recovery checks then covered four distinct boundaries. Restarting the
client from a fully spooled transfer delivered the same 40 chunks and hashes. A
proxy discarded one chunk response only after server commit; retry did not create
a duplicate chunk. A separate disconnect occurred before the server saw chunk 0;
the complete local spool remained recoverable. A final proxy discarded the finalize
response after server commit; retry preserved exactly one finalize. Finally,
`STOP_AFTER=memo` left 40 chunks, one close, a durable memo, and zero finalizes;
after restarting only the scoped server against the same state, retry produced one
finalize after close, 40 unique chunks, and 43 unique receipts without changing
the artifacts. Three compile-free complete transfers took 14.20 s, 13.39 s, and
13.09 s (median 13.39 s); these measure fresh SSH setup plus delivery of both
two-second lanes and control receipts, not recorder callback or Stop latency.

Actual SSH CLI readback found one additional client-contract defect: a successful
`result: null` for absent processing job or note association was mistaken for a
missing result field. The envelope now distinguishes a present JSON null from an
omitted field. A real loopback HTTP regression covers both optional routes, and the
full seven-test spool/transport suite passes with the shared decoder change.
Native local and remote `new` reached the normal macOS permission boundary but the
terminal host lacked Microphone permission; no lane opened. With no remotely
controllable System Settings window, the verifier did not bypass/reset TCC, so real
device Pause/Stop timing and native `attach` remained unclaimed in that automated
lane.

After the user enabled permission and launched the normal private CLI from their
own Terminal, the exact title
`terminal-remote-native-68e6-20260915T172103Z` resolved to two server sessions,
not one. The earlier incomplete attempt
`remote-2026-09-15-10-22-29-a23bba2f` retained 140 acknowledged chunks / 448,000
bytes (microphone 7.200 s, system 6.800 s) with its producer and identity still
active; it has no close, finalize, canonical artifact, memo, or job. No cleanup was
performed. The completed attempt
`remote-2026-09-15-10-22-56-817dcd90` retained 75 chunks and 237,830 bytes per
lane, both declared mono/raw/s16/16 kHz with a 7.432 s boundary. The microphone
artifact SHA-256 is
`5918a14f92d7f02fc38bc0ab7018cc9028eb6c16f61eb1bb15cbc932d6f24780`;
the system artifact SHA-256 is
`9c39f7a486ba0f59fa8b082621af3d26bdac3c938309d0957e11325e15e29c6e`.
The verifier measured a non-silent microphone signal and equal-duration silent
system lane, as expected because no system tone was played. Its memo is durable,
the producer is released, and ASR attempt 1 completed with a nonempty 405-byte
registered transcript (content was not reported in this audit).

That user-operated run also supplied important Stop-latency counterevidence. The
two 100 ms lanes generated about 20 requests/s, while the single sequential HTTP
delivery loop landed 150 chunk requests over 23.756 s, about 6.31 requests/s and
20 KiB/s of PCM payload across this SSH path. The close intent was created at
17:23:05.069 UTC and the finalize intent at 17:23:10.601. Server file/row times
showed the last microphone chunk at 17:23:15.281, the last system chunk at
17:23:21.644, canonical artifacts by 17:23:22.427, memo by 17:23:22.538, and
producer release/finalize commit by 17:23:22.686. The transcript appeared at
17:23:25.379 and the job completed at 17:23:25.435. The scoped server does not log
per-request arrival, so close/finalize arrival cannot be narrowed further than
those durable bounds. Source inspection proves ASR is scheduled with bounded
`try_send` after durable job admission and does not hold the finalize response;
the observed wait is the chunk/control drain. The Mac-side recorder trace placed
both native-device retirement plus durable recovery-WAV completion at
17:23:05.042, 27 ms before close-intent creation. It then observed roughly 17 s
of post-release drain before CLI exit. This establishes immediate local release
for this Stop, but is one run rather than a readiness/callback latency distribution.

The same trace found a separate duration-accounting defect in candidate `68e6`.
The close correctly records the last audio boundary at 7,432 ms, corroborated by
118,915 samples / 16,000 Hz = 7.4321875 s in each artifact. Native CLI
finalization currently samples `capture_started.elapsed()` only after joining the
uploader, so this run wrote 13,181 ms as the session-finalize boundary: 5,749 ms
of post-release drain was incorrectly added to summary duration. Audio bytes,
per-chunk timing, close boundaries, device release, and ASR input/output are not
altered, but session metadata/UI duration is overstated and the error grows with a
slower drain. This was diagnosed read-only and remains a production fix gate.
The complete Mac-side verifier report is committed independently as
`8018a9cc3a9d1ce044ff8ed3ba8a6d6da5b01d6a`. After inspection, the exact scoped
server PID was stopped, loopback port 38871 was confirmed closed, and the isolated
state/evidence tree was preserved.

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
The Mac SSH median is a complete fresh-tunnel transfer measurement; it must not be
read as local-capture or instantaneous-control latency. The later user-operated
native run measured a concrete throughput limit: 150 100-ms chunk requests took
23.756 s to land while the capture timeline was only 7.432 s. This is a healthy
durability result but negative Stop-latency evidence: reducing sample payload to
16 kHz did not reduce the fixed per-request SSH/HTTP cost enough for two lanes at
the current cadence. The real composed v2 ASR
unit completed in 7.24 s for 6.12275 s of two-lane input after compilation; the
standalone v2 model smoke spent 3.08 s in its test. Those are debug-build,
same-host fixture measurements, not a production throughput benchmark. New direct16
PCM uses 64,000 bytes per two-second mono lane instead of 192,000 at 48 kHz, and
195,928 instead of about 587,784 bytes for the 6.12275-second fixture. This measured
threefold payload reduction does not reduce fixed HTTP/SSH request overhead because
the 100 ms chunk cadence is unchanged.

Inferred, not measured on native hardware: the direct local path adds only canonical
SQLite memo/session work already required for durability; it does not add network,
wire encoding, duplicate audio, spool hashing, or remote threads. Therefore the
design avoids an architectural local callback-pressure regression, but recorder
remote capture does add a persistent anti-aliasing resampler in the already-bounded
spool worker. Its remote-only CPU cost is inferred; callback cost is not, because
the work remains downstream of the bounded queue. Recorder readiness, callback
pressure, native Stop latency, and warm CLI startup still need comparable Mac
measurements. No native-performance claim is made from Linux builds.

The principal implementation burden versus the old unit-only substrate is the
production adapters, owner-scoped credentials, explicit Workspace mapping, immutable
spool/receipt bookkeeping, and crash-boundary tests. Simplicity is recovered by one
canonical database, one shared service, typed first-party routes, no source-file or
note-publishing API, no background delivery daemon, and no remote work in default
local capture.

## Remaining gates

1. The user-operated native SSH `new` path now has real two-lane server evidence,
   but its sequential 100-ms request delivery fell behind capture and made Stop
   wait for the durable backlog. Decide and implement a bounded batching or
   multiplexing policy before treating remote Stop latency as acceptable; then
   measure local release separately from drain. Also pin finalize duration to the
   last closed audio boundary instead of post-uploader wall time; regression-test a
   blocked drain and multi-segment pause/attach. Native `attach` and pause/resume
   remain unverified. Sample-exact SSH delivery, disconnect, lost ACK, client/server
   restart, memo, device retirement, and finalize recovery are complete.
2. Run the complete BB full-host UI journey on an environment that can install/load
   this plugin, including remote remount, stale callbacks, conflicts, recovery,
   artifacts, jobs, and the now-wired agent handoff. Typed production host/client,
   remount, and exact-session contract tests are complete; the full BB host UI is not.
3. Measure readiness, callback pressure, local Stop/device-release timing, and
   system-lane source fidelity across representative Mac runs. One user-operated
   remote Stop now has a device-retirement trace, and its deliberately quiet system
   lane is duration-correct, but no system source was played and one trace is not a
   performance distribution.
4. Run the real Shortcut share/retry/app-switch/lock/cellular matrix.
5. Verify signed/published clean-client installation and supported OS service
   installation. Portable archive topology/checksum/extraction and stopped-state
   backup/restore are verified; the no-audio package failed the native capture smoke
   as expected. No push, merge, production deployment, credential change, or release
   was performed here.
