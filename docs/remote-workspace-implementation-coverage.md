# Remote Workspace implementation coverage

Implementation base: `4228ef5396074413cf4349365a5529132b4b956d`
Phase 1 checkpoint: `188d29b2c`
Native remote composition checkpoint: `46c8f6d4ed3185e6f32d9edee977f6b01212f539`
16 kHz remote ASR candidate: `68e6a07741fc23172436d3901fc9ae296ed35611`
Native Opus candidate: `9353e00fa83e9fae54f03d3fb70da5643ebbda3a`
SSH catch-up checkpoint: `f3aff7f12078d7f3ca41e00245ad603057e982b1`
Sustained-transport candidate: `49a6d9a225d65a1ad4188db63017157f49fd2821`

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

scripts/cargo-lane shared -- cargo build -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features \
  --features server,parakeet-asr-dynamic --bin margins-server
scripts/cargo-lane shared -- cargo build -p margins --no-default-features \
  --bin margins-private
  exact clean candidate 49a6d9a225d65a1ad4188db63017157f49fd2821;
  server SHA-256 0962624c328108c6819840253ffb4ca92ba6ecabae015d628fb295f8d08c0c4e;
  portable discovery CLI SHA-256
  533087dbd42a37661bebe8a8867d147e074879fcaf2989e76fdce89a937eb427

ORT_DYLIB_PATH=/tmp/.../libonnxruntime.so.1.24.4 \
MARGINS_PARAKEET_MODEL_DIR=/tmp/.../parakeet-tdt-0.6b-v2-int8 \
MARGINS_PARAKEET_MODEL_KIND=tdt-v2 \
MARGINS_OPUS_EVAL_FIXTURES=/tmp/margins-opus-eval-fixtures-thr_n2qdmhncai \
MARGINS_OPUS_EVAL_RESULTS=/tmp/margins-opus-quality-matrix-49a6d9a225d6.json \
scripts/cargo-lane disposable -- cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml --no-default-features \
  --features server,parakeet-asr-dynamic \
  server::remote_asr::tests::pinned_v2_opus_quality_matrix \
  -- --exact --ignored --nocapture
  1 passed; 28 sanitized PCM/16/24/32 rows, no fixture or transcript bodies committed
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
slower drain. This was diagnosed read-only in that candidate; the Opus change below
fixes it by pinning finalization to the durable close boundary before uploader join.
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

## Native Opus transport and bounded Stop drain

The native remote path now uses a remote-only 16 kHz mono Opus packet stream at a
24 kbps target for each separately identified microphone and system lane. Device
input remains at its actual rate. The existing persistent anti-aliasing resampler
runs in the bounded capture worker, not the device callback; exact 16 kHz signed-16
input bypasses float conversion. The local recorder/archive path is unchanged and
does not construct the resampler, encoder, remote spool, or uploader.

Each Opus packet represents exactly 320 decoded source samples (20 ms). Twenty-five
packets form a normal 500 ms immutable network command. A terminal command keeps
source-bearing audio with the encoder lookahead/tail so exact 500 ms multiples,
partial final frames, pre-skip, and source trimming remain unambiguous. The real ropus encoder
reports 104 lookahead samples at 16 kHz; framing converts that to 312 samples in the
48 kHz Opus clock, while decode drops 104 samples and trims to the declared source
count. The receiver rejects non-20-ms packets, packet-count/length bombs, truncation,
missing or repeated START/END, gaps, overlaps, source-count overflow, timing mismatch,
and malformed PCM before artifact projection or finalization. Retransmission hashes
the exact encoded packet-stream bytes and does not advance decoder state.

Crash persistence and network cadence are separate. While a segment is open, a temporary
full 16 kHz PCM journal is disk-reserve-accounted and fsynced at intervals no greater
than 100 ms, including when one public adapter call contains a much larger slice.
That deliberate ~64 KB/s two-lane journal cost makes a process-loss prefix
re-encodable without pretending the lost Opus encoder state can be resumed. Clean
close durably stores all compressed chunks and the close before deleting journals.
Recovery validates existing chunk or ACK identity, re-encodes the durable prefix as
a complete segment, and starts later capture at the recovered close boundary. A
crash after one lane journal is deleted reuses the already-durable two-lane close;
the valid ACK-fsynced/frame-not-yet-unlinked state is accepted only when digests
match. Low space stops the producer truthfully before breaching the configured
reserve. Existing raw 16/48 kHz PCM spools remain retryable, but attaching a native
Opus recorder to a PCM-declared session is rejected rather than mixing or relabeling
formats. An interrupted first-generation PacketStream spool whose open marker lacks
the network aggregation field is recovered with its original five-packet command
identity, so existing unacknowledged data is neither relabeled nor discarded.

The active uploader opens the same locked spool on a separate thread. Candidate
`9353e00fa` first used 100 ms network commands; even a 16-command catch-up batch had
no sustained headroom across the measured tunnel. Candidate `ddeb6f2d7` then made
that failure explicit: a 61.228-second run accumulated 195 commands / 38,059 bytes /
17.428 seconds at high water and needed 16.243 seconds to drain after Stop. Its
1,226 commands in 83 sequential requests consumed 62.861 seconds of request time.
Normal append also performed a full pending-frame scan and a growing ACK-directory
scan for every new command, making producer work quadratic; encode-plus-spool wall
grew to 50.355 seconds and the paced producer fell 14.37 seconds behind its media
deadlines. This negative run is retained as the reason for the final design, not
treated as success.

Candidate `49a6d9a225d65a1ad4188db63017157f49fd2821` fixes both limits. Recovery PCM and
its length marker are still fsynced every 100 ms, but one immutable audio command now
carries 500 ms, reducing healthy two-lane command ingress from 20/s to 4/s. The
server continues to accept at most 16 commands per HTTP batch. A full-batch result
causes a prompt catch-up iteration; partial and idle polling remain relaxed. Normal
monotonic capture appends directly under its capture lease and never enumerates old
frames or ACKs. Crash recovery performs one bounded directory pass, builds a
sequence-indexed identity view, then validates re-encoded blocks with logarithmic
lookups. ACKs remain per immutable 500 ms command, so HTTP batch count is not
confused with receipt cost. Capability/limit/instance negotiation occurs once at
connection readiness and is cached; the capture-time uploader does not poll it.
Every mutation carries the negotiated instance fence. Stop first retires and writes
the local recorder, closes/tail-encodes the media, and pins final duration to that
last durable close boundary. Only then does it join the bounded uploader, persist
memo/finalize intents, and drain receipts; delayed transport therefore affects drain
time but not media duration.

The exact `49a6d9a2` portable suite passed workflow library 192/192 with one
ignored, protocol 8/8, remote transport 19/19, and service 7/7:

```sh
scripts/cargo-lane disposable -- cargo test \
  -p margins-meeting-protocol -p margins-workflows --lib \
  --test roundtrip --test remote_workspace --test workspace_service \
  -- --nocapture
```

It covers stream framing and hostile inputs, exact 16 kHz input, persistent 44.1 and
48 kHz resampling across arbitrary partitions, anti-alias rejection, terminal tails,
silence with DTX disabled, reserve failure, interrupted-process recovery, partial
close cleanup, ACK/frame crash state, attach format fencing, manifest lost-update and
reservation-promotion races, delayed Stop duration, lost ACKs, memo-before-finalize,
and legacy spool replay. A separate HTTP regression passed 1/1 and proves a
capture-only principal can finalize while only the service principal reads/schedules
the admitted processing job.

A two-second delayed-HTTP paced fixture used 160 ms per batch while the producer
continued independently at a monotonic 20 ms source cadence. Under the final 500 ms
network framing it carried exactly eight durable audio commands in three HTTP
batches: 2,278 encoded bytes and 5,474 attempted full batch-body bytes. Capture wall
was 2,000 ms; maximum observed backlog was four commands / 1,130 bytes / 979 ms;
five commands were present immediately after terminal blocks were published and the
final drain took 353 ms. Aggregate request time was 486 ms. The invariant is based
on delivery making progress during a finite paced source and terminal commands being
at most two beyond the in-loop high water; generous five-second capture and
capture-relative age/drain ceilings are fixture liveness bounds, not product latency
claims. Ownership/ordering are covered by deterministic barrier tests rather than
host-speed thresholds. These are Linux debug fixture measurements, not SSH or
native-device performance.

A real local HTTP/SQLite/pinned-v2 run from exact clean candidate
`9353e00fa83e9fae54f03d3fb70da5643ebbda3a` used equal 6.122-second microphone
speech and silent-system lanes. It produced 124 durable commands in 16 HTTP batches,
24,982 unique encoded bytes, and 76,338 attempted full batch-body bytes. Maximum
backlog was 14 commands / 2,999 bytes / 676 ms; 12 commands / 2,186 bytes were
pending at Stop, the last input ACK landed 355 ms later, and complete drain took
930 ms. Aggregate HTTP request time was 4,055 ms. The prior 80 ms prototype needed
154 commands, 22 batches, and 1,519 ms to drain the same fixture. Session
`opus-fixture-1789501613252` retained distinct microphone/system Opus artifacts:
19,927 bytes / SHA-256
`1b7c114d69750e38f4f514e45028392ccd554c27abcaa9cd3279229f23cb4886`, and
5,055 bytes / SHA-256
`1adc54c1e3e20755d3b672b61d904761d96e930ec534cf152782cf2cd376198e`.
Its processing job completed at progress 1.0 and registered a durable nonempty
622-byte transcript; transcript contents were not included in evidence.

The independent verifier then ran the same paced fixture from the exact candidate
Mac client through `ssh://bs-server` into that exact Linux service. Client metrics
for session `opus-fixture-1789501628472` report 17 HTTP batches, 124 durable audio
commands/ACKs, 24,982 canonical encoded bytes, and 76,346 attempted full batch-body
bytes. Capture wall was 6,241 ms. SSH latency produced a materially larger high
water of 51 commands / 9,241 bytes / 3,208 ms; 49 commands / 9,241 bytes remained
at Stop, the last input ACK landed 3,164 ms later, and final drain took 3,976 ms.
The final close and session duration nevertheless remained the actual 6,123 ms
media boundary, proving the former uploader-wait duration inflation is fixed in
this composed run.

Server state independently contains exactly 62 chunks per lane, sequences 0-61,
and 127 durable meeting receipts without duplicate events. It records first audio
ACK at 19:47:11.124 UTC, last audio ACK at 19:47:17.216, segment finalization at
19:47:17.227, and session finalization at 19:47:17.579. The canonical artifacts
have the same sizes and hashes as the same-host run above; the transcript is 622
bytes and the ASR job completed at progress 1.0 at 19:47:24.328. The server stores
per-command events/receipts rather than HTTP request counters, so 17 batches is a
client-side measurement while the 124 unique mutations are independently verified
server-side. No transcript content was read for this audit.

Those exact `9353e00fa` client metrics are also counterevidence for the original
eight-command/320-ms tuning. Its 6.816 seconds of aggregate request time means about
401 ms/request and 7.29 commands/request, or roughly 19.95 commands/s against a
two-lane production rate of 20 commands/s. The 3.208-second maximum age therefore
does not establish a stable steady state even though encoded bytes remain small.
Checkpoint `f3aff7f1` increases catch-up batches to 16 and partial-batch age to 500
ms without weakening the 100-ms recovery or receipt boundary. A matching Mac/SSH
rerun is required to measure whether that supplies adequate sustained headroom; the
change is not declared sufficient from the Linux fixture alone.

The later `ddeb6f2d7` 61.2275-second Mac/SSH run disproved that sufficiency rather
than merely missing a test threshold. It produced 1,226 unique 100 ms commands in 83
HTTP batches, accumulated 195 commands / 38,059 bytes / 17.428 seconds, left 187
commands pending at Stop, and drained for 16.243 seconds. Sequential request time was
62.861 seconds. In addition to the command-rate deficit, per-append full directory
scans made encode-plus-spool wall grow to 50.355 seconds and capture wall to 75.600
seconds for 61.228 seconds of media. Duration remained correctly pinned, but this is
explicit failure evidence for that candidate.

Exact candidate `49a6d9a225d65a1ad4188db63017157f49fd2821` replaces those commands
with 500 ms containers and removes historical scans from normal append. A real
same-host paced client/service/SQLite/pinned-v2 run over the 6.12275-second fixture
used 26 audio commands in seven HTTP batches. Capture wall was 6,123 ms; backlog
peaked at six commands / 3,885 bytes / 681 ms; the same six commands were pending at
Stop, the last input ACK followed 188 ms later, and complete drain took 361 ms.
Canonical encoded audio was 21,846 bytes versus 32,625 attempted full batch-body
bytes; aggregate request time was 851 ms. The server retained an 18,359-byte
microphone artifact (SHA-256
`15d2b5b3eeb586031905c5b756a76cd746642cebd647d44554b5158a40cbac45`), a
3,487-byte silent-system artifact (SHA-256
`864035e0df1b04d6d485c4e9808f5c1d84cbd29e61b0c111a664debbb3c4d019`),
and a 622-byte transcript from completed pinned-v2 attempt 1. Its transcript body
was not read in this audit.

The candidate's real 61.2275-second Mac producer through `ssh://bs-server`
completed with capture wall
61.247 seconds and duration 61.228 seconds. It emitted 246 audio commands/receipts in
64 HTTP batches; aggregate request time was 20.504 seconds. Backlog stayed at six
commands / 5,321 bytes / 1.468 seconds; six commands / 4,221 bytes were pending at
Stop, the last input ACK followed 602 ms later, and complete drain took 2.022 seconds.
Canonical encoded audio was 218,065 bytes and attempted full batch bodies were
320,522 bytes. Producer encode-plus-spool wall was 37.913 seconds and its isolated
thread CPU metric was 22.549 seconds. Unlike the ddeb run, the paced producer did not
fall behind its media deadline and backlog age remained bounded across the full
minute.

The server independently records 123 contiguous sequences per lane, 246 unique
audio acknowledgements, one segment finalize, one session finalize, 249 unique
meeting receipts/events, and a released producer. Both close and finalize are pinned
to 61,228 ms. The microphone artifact is 183,508 bytes (SHA-256
`805800e20a023b86a9aa5625e3604614392132f64cc49149b112a6b904ac6d0e`), the
system artifact is 34,557 bytes (SHA-256
`6e48a47483bcb427746a54064bea69956a41dd82e8bc44ed27db475fe5d1c00a`),
and the 3,708-byte transcript was registered by pinned-v2 ASR attempt 1 with no
failure. Transcript contents were not read for this audit.

The independent Mac verifier also passed the exact native composition assertion,
the complete 19-test remote transport suite, legacy-spool recovery, sustained SSH
transport, and ASR checks against this immutable source. Its tracked report is
commit `8c7a5cd962a02bf4b480d1a58d12977dd0a1d081` on the verifier-only branch
`bb/mac-e2e-verify-opus-49a6d9a2`; it includes the ddeb failure, root cause, and
matching 49a6 retest rather than replacing the counterevidence.
The exact Mac audio-capture binary SHA-256 is
`39693825adbf6c5042b9c90370addce1bf2be1df45dcb52af2859d77e48be8ae`;
its release smoke embeds full commit `49a6d9a225d65a1ad4188db63017157f49fd2821`,
`dirty=false`, native-recorder capture, and TUI support. The packaged composition
test passed 1/1 and the complete remote suite passed 19/19. Its local delayed-HTTP
pace measurement was 2,560 ms capture wall, four commands / 1,130 bytes / 735 ms at
high water, four pending at Stop, 477 ms drain, and four batches for eight commands.
The audio-only feature slice truthfully reports recall unavailable.

A second exact SSH transfer stopped with the client still reporting
`completed=false` and retaining its finalization intent. Before the scoped server
restart, authority state had in fact committed that exact finalize while its client
ACK was unobserved: this is a lost-finalize-response case, not a first finalize after
restart. After stopping only the verified server PID and restarting the same binary,
model, instance, port, and state, `transfers retry` completed in 2.41 seconds and the
client transfer list became empty. Canonical session
`opus-fixture-1789501752481` has exactly 124 audio ACK events, one segment-finalized
event, one session-finalized event, and 127 receipts with 127 distinct message IDs
and fingerprints. It retained the same 6,123 ms boundary and exact audio hashes as
both prior runs; no audio or finalize was duplicated. The restart-recovered durable
ASR job completed at attempt 1/progress 1.0 and registered another 622-byte
transcript. This proves lost-finalize-response replay plus processing recovery across
a server restart; it does not claim the first finalize commit happened after restart.

The nonprivate, ephemeral macOS `say` evaluation corpus is not committed. It contains
clean US names/numbers, the same speech at -21.94 dB and with seeded pink noise,
Indian and British names/numbers, 450 ms two-voice overlap, and a long all-zero lane.
The ignored, environment-gated test runs the same pinned Parakeet TDT v2 engine over
PCM and production PacketStream Opus at 16/24/32 kbps. Acceptance requires 24 kbps
to match or improve its per-case PCM WER and critical-token hits, remain within
160 ms at first/last word boundaries, remain below 15% of PCM transport bytes, and
emit zero words for silence at every format. Exact sanitized 28-row results and hash
are preserved outside git for the immutable committed candidate. The owner-only JSON
is `/tmp/margins-opus-quality-matrix-49a6d9a225d6.json`, mode 0600, 10,354
bytes, SHA-256
`b90fcb5d19adec7c03f8454b1f4a431d70d2840e17ac37736a06d00209bc00f8`.
It was rebuilt and rerun from exact clean commit `49a6d9a225d65a1ad4188db63017157f49fd2821`
after the 500 ms grouping change; earlier grouping-specific artifact hashes are not
reused as final evidence. Audio and transcript bodies are not retained in the
report.

At the selected 24 kbps/lane target, every speech case exactly matched its PCM
baseline WER and critical-token count. The compact comparison below reports
transport bytes, WER, critical hits, first/last word boundaries in milliseconds,
ASR milliseconds, and encoder-plus-durable-spool wall microseconds. It does not call
the final column pure codec CPU because it includes fsync.

| Case | PCM bytes / WER / critical / bounds / ASR ms | Opus 24 bytes / WER / critical / bounds / ASR ms / encode+spool us |
| --- | --- | --- |
| US names/numbers | 409924 / .53125 / 4/7 / 0-12720 / 809 | 39021 / .53125 / 4/7 / 0-12720 / 759 / 3163565 |
| Quiet US | 409924 / .53125 / 4/7 / 80-12720 / 756 | 34763 / .53125 / 4/7 / 80-12640 / 753 / 2122808 |
| Noisy US | 409924 / .53125 / 4/7 / 80-12720 / 754 | 40266 / .53125 / 4/7 / 80-12720 / 759 / 2167170 |
| Indian names/numbers | 302102 / .47826 / 0/5 / 0-9360 / 570 | 27562 / .47826 / 0/5 / 0-9360 / 558 / 1627818 |
| British names/numbers | 322952 / .30435 / 3/6 / 0-10000 / 596 | 30247 / .30435 / 3/6 / 0-10000 / 595 / 1715470 |
| Two-voice overlap | 180102 / .76923 / 1/6 / 0-5440 / 348 | 17883 / .76923 / 1/6 / 0-5440 / 347 / 947751 |
| System silence | 409924 / 0 / 0/0 / none / 746 | 7243 / 0 / 0/0 / none / 757 / 1809290 |

The full 28 rows retain all 16/24/32 kbps variants. Important counterevidence is
not averaged away: pinned v2 already performs weakly on the Indian and overlap PCM
baselines; 32 kbps improves the Indian row to .43478 and 1/5 critical hits, while
16 and 32 kbps regress the quiet/noisy US rows to .59375 and 3/7. The approved
24 kbps candidate matches rather than improves those PCM baselines. Silence emits
zero words in PCM and every Opus variant.

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

The final same-host Opus composition used 21,846 unique encoded bytes instead of
391,856 PCM bytes (94.4% less audio payload), while 32,625 attempted full request
bodies expose the still-material command metadata. It needed seven HTTP batches and
361 ms of final drain. The exact 61-second SSH run used 218,065 encoded bytes and
320,522 full-body bytes; its six-command high water and 2.022-second Stop drain are
the measured evidence that 500 ms commands have steady headroom on this path. This
does not promise the same latency on every tunnel. The quality matrix
`encode_micros` field measures encoder plus durable-spool wall time, including fsync;
it is not pure codec CPU. Only the paced harness's thread-clock measurement isolates
producer-thread CPU. All are debug measurements rather than release-performance
claims.

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

1. Opus packet transport, 500 ms network framing with independent 100 ms crash
   checkpoints, binary command batching, the actual-media duration fix, portable
   paced/recovery regressions, exact pinned-v2 quality matrix, and a 61-second
   matching Mac/SSH sustained run are complete. The earlier 100 ms-command SSH runs
   remain counterevidence and recovery baselines, not evidence for the final framing.
   A real device-driven native `attach` plus pause/resume under delayed delivery is
   still needed to corroborate the portable duration/continuity tests; automation
   could not obtain the app-server process's macOS microphone permission even though
   the user's own Terminal run proved native capture works.
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
