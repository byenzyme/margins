# Margins local e2e harness (headless Linux VPS)

**Parked desktop harness.** `run.sh` still builds the retired
`desktop/src-tauri` server, and `distill-fixture.mjs` calls the retired
`/api/invoke` routes. Neither script tests the shipped standalone
`margins-server`; use `integrations/bb-plugin-margins/test-harness/no-llm.mjs`
for the current BB plugin and server E2E lane.

Drive the **real** Margins app on a headless Linux box — real Rust backend, real
on-disk workspace, real distillation — with no macOS and no native Tauri GUI.

## How it works

The native Tauri app can't run here (no macOS WebView; Tauri won't even compile
on Linux without GTK/WebKit). Instead we run the **`margins-server`** HTTP binary
(`cargo ... --no-default-features --features hosted-web`) which exposes the same ~72
commands over `POST /api/invoke/<command>`. The Vite frontend detects the server
token and routes every `invoke()` through HTTP instead of Tauri IPC, so it's the
real product UI talking to the real backend. `hosted-web` includes portable
recall plus dynamically loaded Parakeet ASR; live ASR activates when the runtime
and model environment variables documented in `../../REMOTE_HEADLESS_DEV.md`
are present.

A browser is driven by [`agent-browser`](https://www.npmjs.com/package/agent-browser).
Its Chrome runs network-isolated (under `bb-app.service`), so `127.0.0.1` from the
browser is NOT the host — reach the host via the docker-bridge gateway IP
(`172.18.0.1` by default; override with `MARGINS_E2E_GATEWAY_IP`).

## Run it

Build the frontend once first (`cd desktop && npm run build`) so `dist/` exists —
`margins-server` embeds it via RustEmbed and the build otherwise fails. Then:

```bash
desktop/test-harness/local-e2e/run.sh            # build + boot + seed, stays up
desktop/test-harness/local-e2e/run.sh --keep-state --rebuild
desktop/test-harness/local-e2e/run.sh --hermetic-bin   # CLI-install E2E: installs into temp HOME, self-cleans
```

Booting a second worktree/run? Override ports + state so you don't collide with
or drive someone else's backend: `MARGINS_E2E_PORT`, `MARGINS_E2E_VITE_PORT`,
`MARGINS_E2E_STATE`. See `../REMOTE_HEADLESS_DEV.md` → "Port hygiene" and
"Repeatable setup / CLI-install E2E" for the full hermetic loop.

Then, from another shell:

```bash
agent-browser open "http://172.18.0.1:5173/"
agent-browser snapshot -i -c
agent-browser screenshot /tmp/margins.png
```

## Two lanes

| Lane | Backend | When |
| ---- | ------- | ---- |
| **Real** (`run.sh`) | `margins-server` + isolated seeded vault | durable interaction, real distillation |
| **Mock** | Vite only, `?scenario=<name>` forces `mock-tauri.ts` | quick UI/state screenshots, fault injection |

Mock lane (no Rust build):

```bash
cd desktop && npm run dev -- --host 0.0.0.0 --port 5173 --strictPort
agent-browser open "http://172.18.0.1:5173/?scenario=home-capture-notes"
```

## What's seeded

`seed-vault/` — an Obsidian-style vault (Acme pilot theme) with `meetings/`,
`people/`, and interlinked notes. Copied into an isolated `$HOME/Documents/margins`
per run so real state never touches your machine.

`fixtures/acme-pilot-review/` — a transcript + memo + expected-job fixture that
ties into the seed vault, for exercising distillation (note generation).

## Real distillation

With the harness up (`run.sh`), generate a real note from the fixture:

```bash
node distill-fixture.mjs
```

It stages the session via the `margins-e2e-stage` bin (real `margins::session`
helpers), then runs `process_session` with `forceTranscribe:false` (uses the
transcript checkpoint, skips ASR). The included broker fetches a free key from
`api.enzyme.garden` at runtime — needs outbound network, no manual secret. The
note lands in the vault's `meetings/` folder.

## Setup / CLI-install flow (hermetic)

`run.sh --hermetic-bin` installs `enzyme`/`margins` into the temp HOME instead of
your real `~/.local/bin`, and leak-checks + wipes on teardown. **Drive the actual
flow agentically** (agent-browser: open → snapshot → click "Agent setup" → wait
for "Copy setup prompt" → click again → assert the copied prompt carries a real
temp `ENZYME_BIN`) rather than via a committed script — selectors/labels rot and
failures want judgment. The step-by-step and the eval/clipboard gotchas live in
`../REMOTE_HEADLESS_DEV.md` → "Repeatable setup / CLI-install E2E".

## Caveats

- Parakeet is included in the hosted binary but needs an external ONNX Runtime
  and model bundle. Without those runtime assets, feed transcript fixtures to
  the distillation flow; CoreML remains macOS-only.
- `update_settings` still panics headless (nested-runtime bug); `distill-fixture.mjs`
  seeds title/people/transcript directly and uses the running server's settings.
- agent-browser video (`record start/stop`) needs ffmpeg on PATH (`~/.local/bin`).
