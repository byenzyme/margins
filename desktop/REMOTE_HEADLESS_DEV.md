# Remote / headless desktop development

**Parked desktop workflow.** This document describes the former Tauri hosted-web
server and `/api/invoke` frontend. The shipped BB plugin and standalone server
use `integrations/bb-plugin-margins/test-harness/no-llm.mjs` for headless E2E.

**Source of truth for running and driving Margins on a non-macOS / headless
remote (VPS).** The native Tauri app can't run here (no macOS WebView; Tauri
won't compile on Linux without GTK/WebKit), so we run the **`margins-server`**
HTTP backend and point the real Vite frontend at it — same product code, real
backend — and drive it with a browser. This does **not** replace native Tauri
verification (see "What still needs a Mac").

AGENTS.md keeps only a one-line breadcrumb to this file; everything about the
headless lanes lives here.

## Which lane do I want?

| Goal | Lane | Browser | Backend |
| --- | --- | --- | --- |
| Fault/edge UI states, fast fixtures | `npm run headless:cdp` (mock) | ux-cdp Chromium | mocked (`mock-tauri.ts`) |
| Screenshot the real backend by view | `npm run headless:cdp:real` | ux-cdp Chromium | real `margins-server` |
| **Interactively drive real state** (click flows, real `ensure_cli_tools`, distillation, video) | `test-harness/local-e2e/run.sh` + **agent-browser** | agent-browser's own Chrome | real `margins-server` |

For anything touching storage, settings, `ensure_cli_tools`, or real
distillation, use the **real** backend — the mock stubs those commands (e.g.
`ensure_cli_tools` returns a fake `/mock/enzyme` path), so a passing mock run
proves nothing about the real command. The mock/CDP screenshot commands remain
the fast path for pure UI fault states.

---

# The interactive real lane (agent-browser + margins-server)

## What it can and cannot do

`margins-server` is built `--no-default-features --features hosted-web`
(server + portable recall + dynamically loaded Parakeet, while still excluding
tauri/GTK/cpal/ALSA/CoreML) and exposes the same ~72 commands over
`POST /api/invoke/<command>`. The frontend detects the server token and routes
every `invoke()` through HTTP instead of Tauri IPC.

- **Works headless:** real settings/projects/notes on disk, vault grounding, the
  command surface over HTTP, **included-broker distillation from a transcript**
  (`update_settings` and the AI config path are async-native — no nested-runtime
  panics), browser screenshots and video.
- **Provisioned headless capability:** ASR/transcription via
  `parakeet-asr-dynamic` (loads `libonnxruntime.so` at runtime — see below).
  The normal hosted build contains the capability; without provisioned runtime
  assets, sessions still accept durable audio and transcript fixtures.
- **Browser capture:** `getUserMedia` microphone audio flows through the Web
  Audio PCM bridge for live Parakeet while MediaRecorder keeps the durable WebM.
  Native cpal capture and system-audio capture remain unavailable headless.

## Prerequisites (one-time, no sudo)

- **Build the frontend once so `desktop/dist/` exists** (`cd desktop && npm run
  build`). `margins-server` embeds `dist/` via `#[derive(RustEmbed)]`; if the
  folder is missing the server build fails confusingly with `RustEmbed folder
  '.../dist' does not exist` → cascading `E0599: no associated function 'get'
  found for struct 'Assets'`. `run.sh` does **not** build the frontend. (The dev
  harness serves the UI from Vite, not the embedded copy, so a one-time `dist/`
  is enough.)
- `agent-browser install` — downloads its own Chrome for Testing under
  `~/.agent-browser/browsers/`; runs network-isolated. This is the browser to
  drive — **not** the ux-cdp Chromium and **not** a system Chrome (there is none
  on PATH). Idempotent; on "shared library" errors use `--with-deps`.
- Browser WebM/Opus finalization is native in `margins-server`: the server
  demuxes WebM, decodes Opus directly to 16 kHz, and streams mono WAV output
  through a synced temporary file. Hosted capture never resolves or spawns
  `ffmpeg`; the compatibility finalizer and its environment switches were
  retired. `ffmpeg` remains optional for agent-browser video conversion. A
  browser client never needs a local ffmpeg installation.

## Spin up

```bash
desktop/test-harness/local-e2e/run.sh --keep-state
```

Builds `margins-server` (cached), seeds an **isolated** `$HOME/Documents/margins`
vault (`seed-vault/`: an Obsidian-style Acme-pilot vault, real state AND
distillation grounding), boots the server on `:8787` with token + the embedded
distillation skill materialized under `MARGINS_DATA_DIR`, starts Vite on
`0.0.0.0:5173` with the proxy, prints the URL. `--keep-state`
reuses the workspace; omit for a fresh vault. State lives under `/tmp/margins-e2e`.

The seed vault ships `meetings/` + `people/` but **no `.margins/` enzyme index**,
so its project boots in `needs_setup` — the correct starting state for exercising
the project-setup / `ensure_cli_tools` flow.

## Port hygiene & multiple worktrees (READ THIS FIRST)

The defaults (`8787` server, `5173` Vite) collide the moment a second worktree or
a stale run is already up. A colliding server silently binds to *someone else's*
backend, or Vite serves *another worktree's* frontend — so you "test" code that
isn't yours. Before booting:

```bash
ss -ltnp | grep -E ':8787|:5173'          # what's listening, and whose pid
ls -l /proc/<pid>/cwd                       # which worktree that pid belongs to
# Confirm the Vite you reach serves YOUR edits (grep a marker you just added):
curl -s http://127.0.0.1:<vite-port>/src/main.ts | grep -c '<your-new-symbol>'
```

If the defaults are taken, run on your own ports + isolated state — do **not**
kill a neighbor's harness:

```bash
MARGINS_E2E_PORT=8899 MARGINS_E2E_VITE_PORT=5201 MARGINS_E2E_STATE=/tmp/margins-e2e-<slug> \
  desktop/test-harness/local-e2e/run.sh --keep-state
```

Clean up **your own** stale runs (scope kills — `grep -v` the neighbor's path):

```bash
pgrep -af 'run.sh|cargo build .*margins-server|margins-server' | grep -v grep
kill <your-run.sh-pid> <your-cargo-pid>              # SIGTERM; trap tears down children
pkill -9 -f 'features server --bin margins-server'   # only if a server is wedged
```

`run.sh`'s EXIT trap removes `$STATE_ROOT` unless `--keep-state`. `CARGO_TARGET_DIR`
defaults to the repository-wide shared cache derived from Git's common directory,
so sibling worktrees reuse artifacts and Cargo's target lock serializes builds.
The trap doesn't always reap the Vite grandchild — if a port lingers,
`kill $(ss -ltnp | grep :<vite> | grep -o 'pid=[0-9]*' | cut -d= -f2)`.

## Drive it with agent-browser

Two frontend modes, both via query param (DEV builds default to mock):

- **Real backend:** `http://<gateway-ip>:<vite>/?backend=http`
- **Mock (fault states, no Rust build):** `.../?scenario=<name>` (forces
  `mock-tauri.ts`; scenarios listed in that file, e.g. `home-capture-notes`).

Gateway gotcha: agent-browser's Chrome is network-isolated (under
`bb-app.service`), so its `127.0.0.1` is NOT the host. Vite binds `0.0.0.0`
(run.sh does); reach the host via the **docker-bridge gateway IP** (`172.18.0.1`
by default; confirm with `ip -o -4 addr` — the `br-*`/`docker0` address). Use
*your* Vite port, not a hardcoded `5173`.

```bash
agent-browser open "http://172.18.0.1:5201/?backend=http"   # your gateway IP + Vite port
agent-browser snapshot -i -c          # interactive elements with @refs
agent-browser click @e3               # act on a ref; re-snapshot after any change
agent-browser screenshot /tmp/x.png
agent-browser record start /tmp/recap.webm   # video (needs ffmpeg); ...drive...; record stop
```

Read `agent-browser skills get core --full` before a session.

**eval for reads and dispatched clicks.** Read module-local state via the DOM:

```bash
agent-browser eval 'document.querySelector(".project-setup-hint-action.primary")?.textContent'
```

Clicking gotcha (learned the hard way): the app re-renders by replacing
`innerHTML`, so `agent-browser click` (a physical CDP mouse event at fixed
coords) frequently lands on a **detached node right after load** and silently
no-ops (it still reports `✓ Done`). On freshly loaded / just-rendered views,
prefer an **eval-dispatched** click — it fires the inline `onclick`
deterministically:

```bash
agent-browser eval 'document.querySelector(".project-setup-hint-action.primary")?.click(); "clicked"'
```

Clipboard gotcha: agent-browser serves over `http://` (an **insecure context**),
where Chromium leaves `navigator.clipboard` **undefined** — real `writeText`/
`readText` throw. To assert copied content, shim it before driving:

```bash
agent-browser eval "Object.defineProperty(navigator,'clipboard',{configurable:true,\
value:{writeText:t=>{window.__copied=t;return Promise.resolve()},\
readText:()=>Promise.resolve(window.__copied||'')}});1"
# ...drive the copy...
agent-browser eval 'window.__copied'   # the text the app tried to copy
```

Fidelity note: `eval` is for **reads and environment shims**. Driving the
behavior under test with eval-`.click()` steps off the real user path (bypasses
visibility/overlay checks and user-activation), so use a real click when the
harness allows and drop to eval only when it forces you. This lane drives
**Chromium, not macOS WKWebView** — WKWebView-specific behavior (transient
user-activation for clipboard/focus, native drag, some permission prompts) will
not reproduce here; verify those on a Mac.

## Repeatable setup / CLI-install E2E (hermetic, self-cleaning)

`ensure_cli_tools` ("Agent setup") installs `enzyme`/`margins` into
`$HOME/.local/bin`. To test it for real **without** reusing or polluting your
real bin dir, run hermetic and let it self-clean:

```bash
cd desktop && npm run build            # dist/ must exist (RustEmbed)
MARGINS_E2E_PORT=8899 MARGINS_E2E_VITE_PORT=5201 MARGINS_E2E_STATE=/tmp/margins-e2e-setup \
  test-harness/local-e2e/run.sh --hermetic-bin   # NO --keep-state -> temp bins wiped on exit
```

`--hermetic-bin` strips the real `~/.local/bin` from the *server's* PATH, so the
install path actually downloads + installs into the isolated temp HOME
(`$MARGINS_E2E_STATE/home/.local/bin`). On teardown the trap removes the temp
state (binaries and all) and diffs the real `~/.local/bin`, warning loudly if
anything leaked.

**Drive the flow agentically, not with a committed script.** The isolation +
leak-check is durable plumbing; the *interaction* is deliberately left to an
agent driving agent-browser live (snapshot → observe → act → assert), because
selectors/labels rot and failures need judgment, not a brittle golden-path `.sh`.
For this flow, roughly:

1. Open `?backend=http`; wait for the enabled "Agent setup" button (real-backend
   hydration is slower than a fixed sleep).
2. Click → label goes `Installing CLI…` → `Copy setup prompt` (real install into
   the temp HOME), no error banner.
3. Click again → the copied prompt carries a real temp `ENZYME_BIN` (shim the
   clipboard first) and no error.
4. Optionally verify `$MARGINS_E2E_STATE/home/.local/bin/{enzyme,margins}` exist.

This loop (boot hermetic → install into temp → drive → teardown → leak check) is
the reusable *template* for any install/setup-touching E2E, and leaves the box as
it found it. The real *clipboard write* is WKWebView-specific — this lane proves
the state machine + prompt contents, not the native copy.

## Real distillation (transcript → note, included broker)

```bash
node desktop/test-harness/local-e2e/distill-fixture.mjs
```

Stages the `fixtures/acme-pilot-review` transcript through the **`import_transcript`**
server command (args: `title`, `people`, aligned `transcript`, optional
`memo`/`name`/`projectId`; `ch0/ch1` map to `people[0]/[1]`), then calls
`process_session` with `forceTranscribe:false` (uses the checkpoint, skips ASR).
The included broker (`ai_mode: "included"`) fetches a free OpenRouter key from
`https://api.enzyme.garden/margins/free-config` at runtime — needs outbound
network, no manual secret. The note lands in the vault's `meetings/`. To distill
your own material, add a fixture dir (`aligned.md` timeline + `memo.md` +
`settings.json`, mirroring `acme-pilot-review`). Do not run repeatedly — the key
is usage-limited.

## Optional: real headless ASR (audio → transcript)

```bash
cargo build --no-default-features --features hosted-web --bin margins-server
# runtime needs:
#   ORT_DYLIB_PATH=/path/to/libonnxruntime.so.<ver>   (onnxruntime-linux-x64 release)
#   MARGINS_PARAKEET_MODEL_DIR=/path/to/parakeet-tdt-0.6b-v2-int8
#   MARGINS_PARAKEET_MODEL_KIND=tdt-v2
```

The build above is intentionally headless: do not add `tauri-app`. For a fast
regression check of this documented feature combination, run
`npm run headless:server:check` from `desktop/`.

`parakeet-asr-dynamic` requires an explicit, loadable ONNX Runtime library.
Set `ORT_DYLIB_PATH` to the official ONNX Runtime **1.24.x** library before
starting ASR; Margins validates the file and its `OrtGetApiBase` export before
initializing ORT and fails with that instruction rather than waiting in ORT's
lazy loader.

Keep `MARGINS_PARAKEET_MODEL_DIR` outside disposable profile/HOME directories
on a persistent host cache or mounted model volume. Every hosted profile can
reuse the same read-only ONNX assets; they contain no user data. Hosted Linux
builds never download the macOS-only FluidAudio CoreML bundle during model
preparation. A cold host still needs the ONNX bundle provisioned once, but a
fresh profile on a warm host should perform no model transfer.

The interoperable speech-model contract for native Mac and hosted Linux is
**Parakeet TDT 0.6b v2**. Packaging differs by platform: the default Mac CoreML
bundle is about 464 MB and uses a 6-bit-palettized/mixed-precision encoder; the
verified Linux ONNX int8 export is `smcleod/parakeet-tdt-0.6b-v2-int8` at pinned
revision `d64884b484b919e9656d0b70cb95dfdc98852bef`. Its runtime-required files
are `encoder-model.int8.onnx` (652,282,300 bytes),
`decoder_joint-model.int8.onnx` (8,998,557 bytes), and `vocab.txt` (9,384
bytes), about 631 MiB installed (the export also includes a 140,152-byte
`nemo128.onnx` preprocessor). The official ONNX Runtime 1.24.x Linux
x64 archive is about 8 MB; it is not the source of the model-size difference.
Do not silently substitute a v3 model directory while onboarding a machine that
is expected to match the Mac v2 language/model contract.

Native remote capture declares and durably uploads separate microphone and
system mono/raw/s16 lanes at 16 kHz. The recorder may receive device-native
44.1 or 48 kHz f32 callbacks, but its bounded spool worker performs one
persistent anti-aliasing conversion per lane before disk/network delivery; the
audio callback never resamples, hashes, writes, or waits on the network. Direct
16 kHz s16 fixture input is byte-preserving. Older unacknowledged 48 kHz spools
remain valid and are replayed with their declared format; the server converts
those once at the ASR boundary without rewriting or deleting the source audio.

For native remote sessions, a successful durable `FinalizeSession` admits a
revision-stable `transcribe_session` job before releasing producer authority.
The HTTP response does not wait for inference. A bounded server worker reads the
canonical retained lane artifacts, writes/registers the transcript, and updates
the durable job independently; queued/running work is discovered again after a
server restart. Capability negotiation reports ASR only when both the configured
model files and a loadable ONNX Runtime are present.

## FluidAudio model-cache contract

FluidAudio CoreML assets live by default in the shared
`~/Library/Application Support/FluidAudio/Models` cache. This is deliberate:
the assets are roughly 464 MB, additive model files verified for expected size
and required-file presence, and do
not contain a profile's settings, recordings, credentials, or generated notes.
Profile settings may select an existing model location but never redirect
downloads into the shared cache.

First-run installs therefore keep their settings/profile isolated while sharing
verified model assets. A test that requires full storage isolation can set
`MARGINS_FIRST_RUN_MODEL_DIR=/tmp/margins-first-run-models/parakeet-tdt-0.6b-v2`
before `npm run app:reinstall:first-run`; the script passes it through as
`MARGINS_FLUID_COREML_MODEL_DIR`, and every CoreML lookup/download path uses it.
Use a disposable empty directory and expect a fresh ~464 MB download.

Then a session from a real 16 kHz mono WAV can be distilled with `process_session`
`forceTranscribe:true`. Static `parakeet-asr` does NOT link on this box
(unresolved C++ stdlib symbols from `ort-sys`; no `g++`, `cc` is a Zig clang
wrapper) — use the `-dynamic` feature. ASR accuracy is unbenchmarked here.

For live backchannel latency, build the harness with those same assets:

```bash
MARGINS_E2E_SERVER_FEATURES=hosted-web \
ORT_DYLIB_PATH=/path/to/libonnxruntime.so \
MARGINS_PARAKEET_MODEL_DIR=/path/to/parakeet-tdt-model \
desktop/test-harness/local-e2e/run.sh --rebuild --keep-state
```

The harness can then POST little-endian f32 PCM chunks to
`/api/live-audio/pcm?session=<name>&channel=mic&sample_rate=16000` with the
opaque capture capability in `X-Margins-Capture-Owner` while
agent-browser drives marks through the real frontend. WebM upload remains the
durable recording path; PCM injection exists to make ongoing ASR deterministic.

## Hosted browser capture verification still required

Unit coverage can prove bounded batching, ordered uploads, owner/session
routing, memo hydration guards, and the reload/lease state transitions, but it
cannot prove that a particular
Chrome build schedules `ScriptProcessorNode` callbacks for a physical
microphone. `ScriptProcessorNode` is deprecated, and autoplay/user-activation,
device permission, audio-service, and background-tab behavior remain browser
runtime variables. The August hosted evidence did not include the PCM bridge's
console warning text, so there is no evidence-backed AudioWorklet migration or
audio-graph rewrite in the current fix.

Run this exact live check on the hosted Chrome/HTTPS deployment before rollout:

1. Build/start the `hosted-web` server with a working ONNX Runtime and Parakeet
   model, open a fresh Chrome tab, grant a physical microphone, and speak for at
   least 20 seconds. Do not inject PCM through the harness endpoint.
2. Poll `get_recording_status`. Confirm `webm_chunk_count` and `webm_bytes` grow
   about every three seconds, while `live_pcm_batch_count` and
   `live_pcm_sample_count` grow about every half second. Compare last-received
   timestamps to `web_transport_server_unix_ms`, not the browser clock.
   Confirm `web_owner_lease_active` remains true during slow permission/setup
   and normal capture; the opaque owner capability must never appear in status.
3. Confirm a Parakeet snapshot reports non-zero `mic_available_ms` and
   `mic_decoded_samples`, then create a memo mark and confirm its backchannel
   request contains transcript context.
4. Pause for five seconds, resume, and speak again. Both transports must stop
   advancing while paused and resume afterward; server batch/sample totals must
   remain monotonic and the transcript must preserve sample order.
5. Block `/api/live-audio/pcm` only, including one request that never settles.
   Durable WebM counters must keep advancing,
   the UI must report a live-transcript microphone failure/stale state, and
   finishing must still produce a valid WebM/WAV. Restore the route and verify
   subsequent ordered batches are admitted. Observe that the queue is bounded
   and Finish does not wait indefinitely for the supplementary PCM request.
6. Open a second tab during a healthy recording. It must say **capturing in
   another tab**, expose no pause/Finish/discard control, and a direct stop or
   discard invocation without the owner capability must fail without affecting
   the first tab. A memo checkpoint rejected in this observer tab must suppress
   its follow-on backchannel request; direct backchannel requests with the
   wrong owner or before owner memo hydration must not replace memo state or
   reach model work. Also force `/api/audio/chunk` to return HTTP 200 with
   `{ "ok": false }` and confirm the owner UI reports a durable upload failure.
7. During another recording, reload the owner tab. While the old lease or fresh
   transport evidence remains, it must still say **capturing in another tab**.
   Leave the owner tab backgrounded for more than one minute and confirm normal
   Chrome timer clamping does not enable takeover; the server uses a 75-second
   lease and monotonic transport freshness for this decision.
   Only after both time out may it say **interrupted**. Confirm committed memo
   marks hydrate before any control is enabled, then use the explicit **Take
   control** action. Drop the first claim response and separately fail its first
   hydration request; retry must keep the same accepted capability. If WebM
   bytes exist, Finish must preserve the partial audio;
   if no first WebM chunk arrived, the copy must require explicit discard. Force
   a transcode failure once and confirm retry/discard remains available.
8. Pause for longer than both stale thresholds, then resume. The UI must not
   flash missing/stale during the new local grace window; subsequent WebM and
   PCM counters must resume monotonically. With the local amplitude meter
   unavailable, healthy WebM transport may say **Live**, but `mic_level` must
   remain null.
9. Hold session A in Finish long enough for another tab to start session B.
   Force A to fail once in transcoding and once after transcoding (for example,
   index/readiness publication). B must remain the only active singleton while
   A remains owner-scoped and independently retryable/discardable.

If step 2 still shows zero PCM, preserve the first PCM startup/upload warning,
Chrome version, `AudioContext.state`, page visibility, permission state, and the
Network entry (or absence) for `/api/live-audio/pcm`. Those facts distinguish a
callback-generation failure from an HTTP/admission failure and are required
before changing the audio graph.

---

# The mock / CDP screenshot lanes

From `desktop/`:

```bash
npm run headless:check
npm run headless:cdp:real                                   # real backend, screenshots by view
npm run headless:cdp -- --scenarios settings-audio,recording-healthy
npm run headless:e2e -- --grep customer-call-connected-note
```

- `headless:cdp:real` starts the Rust HTTP backend with isolated temp state,
  starts Vite with a proxy/token, and captures real backend-backed CDP views
  without `?scenario=`.
- `headless:cdp` starts Vite if needed, sets `MARGINS_UX_HEADLESS=1`, delegates
  to `npm run ux:cdp` — the mock-scenario lane.
- `headless:e2e` delegates to `npm run ux:e2e`; Playwright runs Chromium headless
  and starts/reuses Vite.
- `npm run headless -- real-distill-help` for the opt-in real-LLM distill
  commands. Don't run the real distill lane unless model spend is allowed.

Browser/CDP behavior: the CDP harness reuses an existing endpoint at
`http://$MARGINS_CDP_HOST:$MARGINS_CDP_PORT` (defaults `127.0.0.1:9222`); else it
launches a local Chrome/Chromium with `--remote-debugging-port` + a temp
`--user-data-dir`. On Linux/CI (or `MARGINS_UX_HEADLESS=1`) it launches
`--headless=new`. These lanes use **Chromium** — Playwright's at
`~/.cache/ms-playwright/...`, or set `MARGINS_CHROME_BIN`. Point at an external
endpoint with `MARGINS_CDP_HOST` / `MARGINS_CDP_PORT`; the CDP scripts also honor
`MARGINS_UX_BASE` to target your Vite URL.

---

# Reasoning about an E2E test plan for a feature

1. **Lane:** real backend (`run.sh` + `?backend=http`) for anything touching
   storage/distillation/settings-read; mock (`?scenario=`) only for fault/edge UI
   states hard to produce for real.
2. **Command surface:** grep the feature's `invoke(...)` calls in `desktop/src/`,
   find the matching `#[tauri::command]` fn — hit it directly via
   `POST /api/invoke/<command>` with the bearer token to probe shapes before
   driving the UI.
3. **Seed** the minimum real state (vault notes, a session) so the feature has
   something to act on; use the seed vault as grounding.
4. **Drive** with agent-browser (snapshot → act → re-snapshot), capturing
   screenshots per state and a `record`ed video.
5. For note-generation, drive `distill-fixture.mjs` and judge the markdown against
   an `expected-user-job.md`.
6. Watch the `run.sh` output / server log for panics — a good bug-finder for
   server-mode-only defects.

# Config / env reference

| Var | Purpose |
| --- | --- |
| `MARGINS_HOST` / `MARGINS_PORT` | server bind (default `127.0.0.1:8787`) |
| `MARGINS_DATA_DIR` | token dir (`<dir>/token`, Bearer for `/api/invoke`) |
| `MARGINS_PROFILE` | settings profile (`~/.config/margins-<profile>/`) |
| `MARGINS_DISABLE_KEYCHAIN=1` | required headless |
| `MARGINS_DISTILL_SKILL_PATH` | optional override for `process_session` / `refine_session`; when unset, the hosted server uses its embedded Margins distillation bundle materialized under `MARGINS_DATA_DIR`; when set, it must point at a valid host preamble such as `skills/margins/hosts/desktop.md` |
| `MARGINS_DEV_HTTP_TARGET` / `_TOKEN` | Vite proxy target + injected token |
| `MARGINS_E2E_PORT` / `MARGINS_E2E_VITE_PORT` | run.sh server/Vite ports — override to avoid worktree collisions (default `8787`/`5173`) |
| `MARGINS_E2E_STATE` | run.sh isolated state root (default `/tmp/margins-e2e`); unique dir per parallel harness |
| `MARGINS_E2E_GATEWAY_IP` | host IP the agent-browser sandbox reaches (default `172.18.0.1`) |
| `CARGO_TARGET_DIR` | build dir (default per-worktree `<repo>/target-linux-server`) |
| `MARGINS_UX_BASE` | CDP-lane Vite base URL |
| `MARGINS_CDP_HOST` / `MARGINS_CDP_PORT` | external CDP endpoint (default `127.0.0.1:9222`) |
| `MARGINS_CHROME_BIN` | Chromium path for the CDP lanes |
| `MARGINS_UX_HEADLESS=1` | force `--headless=new` for the CDP lanes |

# What still needs a Mac / native pass

- Tauri WebView behavior (incl. WKWebView user-activation for clipboard/focus).
- Real microphone and system-audio permissions.
- macOS packaging, install/reinstall flow, app entitlements.
- Real native Pi auth and local app integration.
- **macOS/Tauri GUI build is unverified from Linux** — the `server`/`tauri-app`
  feature split is designed not to touch the default build, but verify GUI/CoreML
  builds on a Mac before shipping backend changes made in this lane.

Use the headless lane for fast evidence and bounded harness work, then the native
Tauri loops in `AGENTS.md` / `desktop/UX_CDP_LOOP.md` when the change depends on
macOS behavior.
