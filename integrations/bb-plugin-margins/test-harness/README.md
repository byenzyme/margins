# Meetings browser E2E

`no-llm.mjs` runs steps 1–6 of `../MEETINGS_SPEC.md` against a disposable
Margins home and an isolated bb server. It installs this plugin only into that
temporary bb instance. The two fixture threads are scheduled seven days out
and the temporary server and data are removed before the script exits. The
default lane stops before Make note. Both lanes suppress automatic note-thread
creation in the isolated server so the real-LLM lane sends only its explicit
Make note request. Step 7 requires the explicit
`MARGINS_E2E_REAL_LLM=1` opt-in described below.

Build from **this worktree** first. Keep Rust commands on the repo's guarded
lane:

```bash
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public
scripts/cargo-lane shared -- cargo build -p margins-server --no-default-features --features parakeet-asr --bin margins-server
cd integrations/bb-plugin-margins && npm run typecheck && npm test && npm run build && cd ../..
agent-browser install
docker build -f integrations/bb-plugin-margins/test-harness/chrome.Dockerfile -t margins-bb-e2e-chrome:local .
```

Set `MARGINS_E2E_CHROME_BIN` to the `chrome` file installed by agent-browser.
Run with absolute paths; use the shared target reported by `scripts/cargo-lane
status` for the two Rust binaries:

```bash
MARGINS_E2E_BIN=/absolute/shared-target/debug/margins-public \
MARGINS_E2E_SERVER_BIN=/absolute/shared-target/debug/margins-server \
MARGINS_E2E_BB_APP=/absolute/bb/packages/bb-app/dist/bb-app.js \
MARGINS_E2E_CHROME_BIN=/absolute/.agent-browser/browsers/chrome-version/chrome \
node integrations/bb-plugin-margins/test-harness/no-llm.mjs
```

The runner creates a fixture vault copy with `inbox`, initializes the disposable
bb project as a Git checkout, adds `note_folder` to a
desired Workspace TOML, preserves the exact reviewed plan JSON, applies that
plan, and sets the disposable home's global default to that Workspace. It
prepares a repeated speech WAV and mounts it into headless Chrome as the
fake microphone. The browser journey checks recording, changing sidebar level,
memo persistence across thread switches, pause/resume, Stop, post-stop memo
save, and an exact spoken transcript before Make note creates a thread.

The bundled `fixtures/launch-accessibility.wav` is the default speech sample.
Set `MARGINS_E2E_SPOKEN_WAV` to an existing speech WAV to override it.
Set `MARGINS_E2E_ASR_MODEL_DIR` to the pinned Parakeet TDT v2 ONNX
directory, and `MARGINS_E2E_ORT_LIBRARY` to the ONNX Runtime library. Set both
ASR paths, and build the server with
`scripts/cargo-lane shared -- cargo build -p margins-server --no-default-features
--features parakeet-asr --bin margins-server`. The runner repeats the WAV for 120 seconds so speech remains
available after a cold install, and creates `asr-runtime.json` under the disposable
bb plugin host data directory,
selecting the server built above. It requests transcription of
the same browser session after Finish and requires at least one spoken timeline
line; a memo-only live checkpoint is a failure. This lane makes no LLM call,
but it requires ASR assets to prove that the meeting has real speech for the
later distillation run.

Video, step screenshots, a four-second clip with another thread open while the
recording overlay persists, destination and plan evidence, bb logs, and assertion
results are written under gitignored `../e2e-artifacts/<timestamp>/`. Set
`MARGINS_E2E_ARTIFACTS` to an absolute path to choose another artifact folder.
The runner closes its named agent-browser session, verifies that its daemon
exited, and records the result in `browser-cleanup.json` and `assertions.json`.
It removes only its own temporary home, bb data, and Chrome container.
The isolated bb server and CLI calls do not inherit the parent bb thread's
`BB_THREAD_ID`, `BB_PROJECT_ID`, `BB_ENVIRONMENT_ID`, `BB_HOST_DAEMON_PORT`, or
`BB_THREAD_STORAGE`.

## macOS lane (no Docker)

On macOS the runner selects a local lane automatically. It runs the same
journey and assertions with these differences:

- Chrome runs as a local headless process instead of in Docker, with the same
  fake-media flags, so the prepared WAV is the microphone. Set
  `MARGINS_E2E_CHROME_BIN` to the inner executable of a Chrome or Chrome for
  Testing app. If it is unset, the runner downloads the pinned Chrome for Testing
  version in `no-llm.mjs` into `MARGINS_E2E_CHROME_CACHE` (default
  `~/Library/Caches/margins-bb-e2e/chrome-for-testing`) and reuses it on later
  runs. The browser profile is always a temporary directory, so the runner never
  opens your normal Chrome profile. Chrome runs with
  `--disable-features=AudioServiceSandbox`, because the macOS audio-service
  sandbox cannot read the WAV and would leave the microphone silent. Every other
  part of Chrome stays sandboxed.
- On a macOS client, Start meeting records through Margins Menu. The runner
  instead clicks **Browser mic only**, which is the explicit browser-microphone
  path. It also aborts all page requests to the Menu bridge on loopback ports
  18764 and 18765. Any such requests are recorded in
  `menu-bridge-requests.json`, and the run fails if one of them gets a response.
  As a result, the isolated page never reaches a MarginsMenu that is running on
  your Mac.
- The temporary root is under `/private/tmp`, because macOS limits Unix socket
  paths to 104 bytes. The agent-browser socket and pid files are written there
  (`AGENT_BROWSER_SOCKET_DIR`), and that pid file is how the runner checks that
  the daemon exited.
- The isolated bb server keeps your real `HOME`. bb's built-in plugins query
  the login keychain, and an empty `HOME` makes macOS show "Keychain Not Found".
  Instead, each write location is redirected into the temporary root:
  `BB_DATA_DIR`, `MARGINS_HOME`, `MARGINS_CLI_BIN_DIR` (fresh release), and
  `MARGINS_FLUID_COREML_MODEL_DIR`.
- Transcription uses the CoreML recorder. For a local build, use
  `scripts/cargo-lane shared -- cargo build -p margins-server --no-default-features --features coreml-asr --bin margins-server`
  and set `MARGINS_E2E_COREML_MODEL_DIR` to an installed
  `parakeet-tdt-0.6b-v2` CoreML directory. The runner opens it read-only through
  `MARGINS_FLUID_COREML_MODEL_DIR`. The ONNX
  `MARGINS_E2E_ASR_MODEL_DIR` and `MARGINS_E2E_ORT_LIBRARY` variables are not
  used on macOS.
- With `MARGINS_E2E_COLD_ASR=1`, the runner points
  `MARGINS_FLUID_COREML_MODEL_DIR` at an empty directory under the temporary
  root. It then waits up to ten minutes for the server to download the
  four `.mlmodelc` bundles and `parakeet_vocab.json` before it clicks Transcribe.
  `cold-asr-install.json` records whether the model and the release runtime were
  installed.
- At exit, the runner stops Chrome and bb. It then terminates every process
  that descended from either one, and every process whose command line contains
  the temporary root's name. This includes recorders that bb started. The result
  is recorded in `process-cleanup.json`, and the run fails if any process is
  still running. The cleanup does not touch any other `margins-server` or
  MarginsMenu.

To run the published release with an empty model cache, use these commands.
`margins-public` only prepares the disposable Workspace:

```bash
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public
MARGINS_E2E_FRESH_RELEASE=1 MARGINS_E2E_COLD_ASR=1 \
MARGINS_E2E_BIN=/absolute/shared-target/debug/margins-public \
MARGINS_E2E_BB_APP=/absolute/bb-app/dist/bb-app.js \
node integrations/bb-plugin-margins/test-harness/no-llm.mjs
```

The local-build override is the same command without
`MARGINS_E2E_FRESH_RELEASE`, with `MARGINS_E2E_SERVER_BIN` set to the CoreML
server built above. Add `MARGINS_E2E_COREML_MODEL_DIR` unless `MARGINS_E2E_COLD_ASR=1`.
The macOS lane needs `ffmpeg`, `curl`, `agent-browser`, and `bb` on `PATH`.
If the `ffmpeg` on `PATH` is broken, set `MARGINS_E2E_FFMPEG` to a working
binary that has libx264. The runner uses it for the WAV and the clip, and puts
its directory first on agent-browser's `PATH` for the journey video.
`MARGINS_E2E_BB_APP` must be a full `bb-app` package that includes
`server/dist`. An enrolled machine's host-only runtime cannot serve the
isolated bb. For example, use `npm install --prefix /tmp/bb-app-e2e bb-app@<host version>`
and then
`/tmp/bb-app-e2e/node_modules/bb-app/dist/bb-app.js`. Run `npm ci` in
`integrations/bb-plugin-margins` first, because bb bundles the plugin frontend
from its `node_modules`.

## Approved real-LLM step 7

Set `MARGINS_E2E_REAL_LLM=1` with the same required absolute paths and hosted
ASR assets. This disables bb's automatic provider retry in the isolated server
and clicks Make note once, which creates and starts the connected-note thread.
The runner waits for a note in the fixture
`inbox/`, checks that it reflects both the revised memo and spoken transcript,
then checks the Distilled list, note and thread links, and the thread Margins
tab. It copies the throwaway vault into the run's artifact directory before
cleanup. A failed run must be inspected through its `llm-send.marker` before
any retry: a marker means Make note was clicked and another run may send
another LLM request.

## Published-release fresh install

After the pinned release is published, set `MARGINS_E2E_FRESH_RELEASE=1` and
omit `MARGINS_E2E_SERVER_BIN`. The runner installs the plugin into its isolated
BB server without a CLI or server override. The plugin downloads the verified
release into temporary plugin storage and installs its CLI into a temporary bin
directory. The runner requires both installed executables before recording.
`MARGINS_E2E_BIN` is still used only to prepare and inspect the disposable
Workspace outside the plugin. The ASR model and ONNX Runtime paths keep the
test focused on release installation rather than a separate model download.

## Empty model cache development lane

Set `MARGINS_E2E_COLD_ASR=1` and omit `MARGINS_E2E_ASR_MODEL_DIR` and
`MARGINS_E2E_ORT_LIBRARY`. Use a locally built `MARGINS_E2E_SERVER_BIN` to
test development changes, or add `MARGINS_E2E_FRESH_RELEASE=1` to test the
published release. The runner gives
the isolated BB host an empty `XDG_CACHE_HOME`, records while the server
downloads its pinned model and ONNX Runtime, waits for both assets before
clicking Transcribe, and requires a spoken transcript. This downloads about
654 MB and removes the temporary cache on exit. `cold-asr-install.json` and
the BB logs in the artifact folder record what was installed. This lane does
not by itself prove the published release includes automatic setup; combine it
with the published-release lane above for that claim.
