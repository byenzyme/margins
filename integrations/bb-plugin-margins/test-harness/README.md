# Meetings no-LLM browser lane

`no-llm.mjs` runs steps 1–6 of `../MEETINGS_SPEC.md` against a disposable
Margins home and an isolated bb server. It installs this plugin only into that
temporary bb instance. The two fixture threads are scheduled seven days out
and the temporary server and data are removed before the script exits. The
composer prompt is never submitted. Step 7 is intentionally absent.

Build from **this worktree** first. Keep Rust commands on the repo's guarded
lane and build the frontend before the server binary:

```bash
cd desktop && npm ci && npm run build && cd ..
scripts/cargo-lane shared -- cargo build -p margins-cli --no-default-features --features recall --bin margins-public
scripts/cargo-lane shared -- cargo build -p margins-desktop --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features 'recall,server' --bin margins-server
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

The runner creates a fixture vault copy with `inbox`, adds `note_folder` to a
desired Workspace TOML, preserves the exact reviewed plan JSON, applies that
plan, and sets the disposable home's global default to that Workspace. It
generates an amplitude-varying WAV and mounts it into headless Chrome as the
fake microphone. The browser journey checks recording, changing sidebar level,
memo persistence across thread switches, pause/resume, Stop, post-stop memo
save, and a seeded composer with no new thread. bb's SDK currently cannot pass
a project to `navigate.toCompose`, so the runner selects the fixture project
through the composer picker without sending.

Set `MARGINS_E2E_SPOKEN_WAV` to an existing
speech WAV, `MARGINS_E2E_ASR_MODEL_DIR` to the pinned Parakeet TDT v2 ONNX
directory, and `MARGINS_E2E_ORT_LIBRARY` to the ONNX Runtime library. Set all
three together, and build the server with
`scripts/cargo-lane shared -- cargo build -p margins-desktop --manifest-path
desktop/src-tauri/Cargo.toml --no-default-features --features hosted-web --bin
margins-server`. The runner repeats the WAV for Chrome's fake microphone and
creates `asr-runtime.json` under the disposable bb plugin host data directory,
selecting the `hosted-web` server built above. It requests transcription of
the same browser session after Finish and requires at least one spoken timeline
line; a memo-only live checkpoint is a failure. This lane makes no LLM call,
but it requires ASR assets to prove that the meeting has real speech for the
later distillation run.

Video, step screenshots, destination and plan evidence, bb logs, and assertion
results are written under gitignored `../e2e-artifacts/<timestamp>/`. Set
`MARGINS_E2E_ARTIFACTS` to an absolute path to choose another artifact folder.
The runner removes only its own temporary home, bb data, and Chrome container.
