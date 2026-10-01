# Meetings browser E2E

`no-llm.mjs` runs steps 1–6 of `../MEETINGS_SPEC.md` against a disposable
Margins home and an isolated bb server. It installs this plugin only into that
temporary bb instance. The two fixture threads are scheduled seven days out
and the temporary server and data are removed before the script exits. The
default lane stops before Make note and suppresses automatic note-thread creation
in the isolated server. Step 7 requires the explicit
`MARGINS_E2E_REAL_LLM=1` opt-in described below.

Build from **this worktree** first. Keep Rust commands on the repo's guarded
lane and build the frontend before the server binary:

```bash
cd desktop && npm ci && npm run build && cd ..
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public
scripts/cargo-lane shared -- cargo build -p margins-desktop --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features hosted-web --bin margins-server
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
prepares a single-utterance WAV and mounts it into headless Chrome as the
fake microphone. The browser journey checks recording, changing sidebar level,
memo persistence across thread switches, pause/resume, Stop, post-stop memo
save, and an exact spoken transcript before Make note creates a thread.

The bundled `fixtures/launch-accessibility.wav` is the default speech sample.
Set `MARGINS_E2E_SPOKEN_WAV` to an existing speech WAV to override it.
Set `MARGINS_E2E_ASR_MODEL_DIR` to the pinned Parakeet TDT v2 ONNX
directory, and `MARGINS_E2E_ORT_LIBRARY` to the ONNX Runtime library. Set both
ASR paths, and build the server with
`scripts/cargo-lane shared -- cargo build -p margins-desktop --manifest-path
desktop/src-tauri/Cargo.toml --no-default-features --features hosted-web --bin
margins-server`. The runner plays the WAV once, pads the rest with silence for
Chrome's fake microphone, and creates `asr-runtime.json` under the disposable
bb plugin host data directory,
selecting the `hosted-web` server built above. It requests transcription of
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
