# Margins UX CDP loop

This desktop app has a browser harness for screenshot-driven UX iteration. The
preferred lane is now **real-state CDP**: Chrome renders the Vite frontend while
frontend Tauri calls are proxied to the Rust HTTP backend in an isolated temp
profile. The older `?scenario=` mock lane remains available for fault injection
and hard-to-produce edge states.

## What it is

- Real-state CDP starts `margins-server` with a temporary `HOME`, starts Vite
  with an HTTP proxy and injected auth token, then captures real backend-backed
  app views without `?scenario=`.
- The frontend can still run in Chrome/Vite with mocked Tauri commands when
  `?scenario=...` is present.
- Mock Tauri commands and scenario fixtures live in `desktop/test-harness/mock-tauri.ts` and are loaded only by the dev browser harness. Production builds should not include a mock backend chunk.
- Every captured preview is pinned to the same viewport as the real desktop window. The harness reads `app.windows[0].width/height` from `desktop/src-tauri/tauri.conf.json` and applies `Emulation.setDeviceMetricsOverride` (Retina `deviceScaleFactor` 2, override with `MARGINS_UX_DPR`). Screenshots are clipped to that viewport (`captureBeyondViewport:false`) so sizes are consistent run-to-run and match the shipped window by construction. Bump the window size in one place — `tauri.conf.json` — and both the app and the previews follow.
- `desktop/scripts/ux-cdp.mjs` is a tiny CLI wrapper around the modular raw-CDP harness in `desktop/test-harness/ux-cdp/`.
- The script captures:
  - screenshot PNG
  - visible text dump
  - accessibility tree JSON
  - console log JSON
  - markdown report
- The Pi project extension `.pi/extensions/margins-ux-cdp.ts` exposes the same loop as the `margins_ux_cdp` tool after `/reload`.

## Run manually

Run the preferred real-state lane:

```bash
cd desktop
npm run ux:cdp:real
```

This writes artifacts to `desktop/ux-shots-real/`. By default it captures:

- `real-home`
- `real-settings-audio`

It uses isolated temporary settings, vault, server token, and session storage by
setting `HOME` only for the Rust backend process. It also runs the backend with
`MARGINS_DISABLE_KEYCHAIN=1`, so screenshot capture never reads or writes macOS
Keychain items. Your normal Margins desktop profile is not touched. Re-run with
`-- --keep-state` to inspect the temp state after capture:

```bash
npm run ux:cdp:real -- --keep-state
npm run ux:cdp:real -- --views home,settings-audio,settings-ai
```

Use the mock-scenario lane only when you need crafted or fault states. Start the
Vite frontend:

```bash
cd desktop
npm run dev
```

Run the default scenario set:

```bash
npm run ux:cdp
```

Run selected scenarios:

```bash
npm run ux:cdp -- --scenarios settings-audio,recording-healthy,recording-dead-tap,distill-complete
```

Artifacts are written to `desktop/ux-shots/`:

```text
<scenario>.png
<scenario>.txt
<scenario>.ax.json
<scenario>.console.json
report.md
```

`desktop/ux-shots/` is gitignored.

The CDP script opens a separate Chrome instance/profile if needed:

```text
--remote-debugging-port=9222
--user-data-dir=/tmp/margins-ux-chrome
```

For non-macOS or headless remotes, prefer the consolidated lane in
`desktop/REMOTE_HEADLESS_DEV.md`:

```bash
npm run headless:cdp -- --scenarios settings-audio,recording-healthy
```

## Run from Pi

After `/reload`, use the `margins_ux_cdp` tool:

```json
{
  "action": "run",
  "scenarios": ["settings-audio", "recording-dead-tap"],
  "out": "ux-shots"
}
```

Make sure `cd desktop && npm run dev` is running first. The tool intentionally does not start Vite for you, so the agent can keep the dev server visible/killable.

## Setup review states

Setup now lives in Settings and the project controls in the sidebar. Use the Settings audio scenarios when permissions/setup transitions are under review:

```bash
cd desktop
npm run dev
# or open an existing dev server with ?scenario=settings-audio
```

The `settings-audio` harness scenario opens the Audio settings pane, `settings-audio-blocked` opens it with a blocked computer-audio preflight, and `settings-audio-ready` shows the green post-check state. For the native Tauri dev app, run `npm run tauri:dev` and grant dev audio permissions separately with `npm run dev:audio-perms`. After granting macOS Microphone or Screen & System Audio Recording permissions, use the in-app restart action to reopen Margins on the Audio settings pane and rerun the preflight.

For selective screenshot review, use the Pi skill `.pi/agent/skills/margins-ux-cdp/SKILL.md`. It maps change categories to the CDP scenarios/screenshots that should be inspected, so small changes do not require a monolithic run while meaningful UX changes still get visual evidence.

For focus-state audits, prefer the Pi skill `.pi/agent/skills/margins-ux-focus-audit/SKILL.md`: read the relevant selectors first, pick representative controls, and use small ad-hoc CDP crops rather than adding broad permanent harness commands.

## Selective screenshot review

Choose scenarios based on the changed UI family, then inspect the generated `.png`, `.txt`, and `.console.json` artifacts. Use `.ax.json` when labels, focus, forms, buttons, or semantic structure changed. Re-run the same scenarios before/after and compare.

| Change made | Scenarios to inspect |
|---|---|
| First-run/empty setup | `first-run`, `home-empty`, `settings-audio` |
| Settings/audio setup/advanced fields | `settings-audio`; add `recording-healthy` if readiness copy is shared |
| Note-making AI / live cues | settings-ai, settings-ai-api |
| Home/session list/navigation | `home-empty`, `home-capture-notes`, `backchannel-built`, `distill-complete` |
| Recording layout/status/timer/start-stop | `recording-healthy`, `recording-muted-mic`, `recording-dead-tap`, `recording-warming`, `recording-transcript-degraded` |
| Audio health warnings/recoverability | `settings-audio`, `recording-healthy`, `recording-muted-mic`, `recording-dead-tap` |
| Memo editor/in-meeting notes | `recording-with-memo`, `recording-healthy` |
| Backchannel/transcript/speaker evidence | `backchannel-built`, `recording-with-memo` |
| Distill/progress/vault activity | `distill-running`, `distill-complete` |
| Pi login/auth/dependency errors | `distill-error-pi-login`, `distill-running` |
| Final note/review/open/copy/reprocess | `distill-complete`, `backchannel-built` |
| Global CSS/theme/typography/shared layout | `settings-audio`, `recording-healthy`, `backchannel-built`, `distill-error-pi-login`, `distill-complete` |
| Focus rings/keyboard/form density | Use `.pi/agent/skills/margins-ux-focus-audit/SKILL.md`; usually `settings-audio`, `recording-with-memo`, `backchannel-built` |

If a change introduces a new durable state, add or update a named scenario instead of relying on ad-hoc Runtime state mutation.

## Current scenarios

These are mock/fault-injection states. Do not treat them as the canonical app
state when the same state can be produced by the real backend lane.

- `first-run`
- `settings-audio-blocked`
- `settings-audio-ready`
- `settings-audio`
- `settings-ai`
- `settings-ai-api`
- `home-empty`
- `home-real` (real local data — see "Real-data snapshot" below)
- `home-capture-notes`
- `recording-healthy`
- `recording-muted-mic`
- `recording-dead-tap`
- `recording-with-memo`
- `recording-warming` — recording active, transcript warming up (status line: "Recording · elapsed · transcript warming up")
- `recording-transcript-degraded` — recording active, live transcript failed to start (status line: degraded one-liner; stop/pause/discard still usable)
- `recording-warming-then-ready` — warming for ~3 s then ready event clears the warming line to normal recording status
- `backchannel-built`
- `distill-running`
- `distill-error-pi-login`
- `distill-complete`

Scenarios are implemented in `desktop/test-harness/mock-tauri.ts`. Keep scenario fixtures and mock behavior there; `desktop/src/lib/tauri.ts` should remain a small facade over real Tauri commands plus the dev-only harness boundary.

## Real-State Views

Real-state views are captured without `?scenario=`:

- `home`
- `settings-audio`
- `settings-ai`

The command composes the required pieces automatically:

```bash
cd desktop
npm run ux:cdp:real -- --views home,settings-audio
```

Internally it:

1. starts `cargo run --no-default-features --features hosted-web --bin margins-server` with isolated temp
   state;
2. starts Vite with `MARGINS_DEV_HTTP_TARGET` and `MARGINS_DEV_HTTP_TOKEN`;
3. runs `node scripts/ux-cdp.mjs real` against the Vite URL;
4. removes the temp state unless `--keep-state` is passed.

Use this lane for first-run layout, settings layout, real command routing, real
session list density once seeded, and anything where mock state could diverge
from the app.

## Real-data snapshot

Most scenarios use crafted synthetic fixtures. The `home-real` scenario instead renders the **actual local desktop app state** so you can evaluate the real session-list layout and density.

Capture (or refresh) the snapshot:

```bash
cd desktop
npm run snapshot:state
```

This runs the real Rust indexing path (`session_index::list_sessions_with_notes`) against your configured vault — no reimplementation, so it cannot drift from what the installed app shows — and writes `desktop/test-harness/local-snapshot.json` (gitignored). Settings are included with API keys/tokens masked; a few real note/aligned bodies are embedded so opening a real session shows real content.

The harness fetches the snapshot at load; if it is absent (CI / fresh clone), `home-real` falls back to the synthetic dense list. Re-run `npm run snapshot:state` whenever your local sessions change.

```bash
npm run ux:cdp -- --scenarios home-real
```

## Agent workflow

For screenshot-driven UX work:

1. Read `CLAUDE.md`, this file, `desktop/UX_REVIEW.md`, `desktop/TAURI_MULTIPASS.md`, and the relevant source files.
2. Start with real-state CDP:
   ```bash
   cd desktop && npm run ux:cdp:real
   ```
3. Inspect the generated `.png`, `.txt`, `.ax.json`, and `.console.json` files.
   Always open/read the PNG screenshots, not just the text dump.
4. If the state is hard to produce through real behavior, use the mock scenario
   lane:
   ```bash
   cd desktop && npm run dev
   cd desktop && npm run ux:cdp -- --scenarios recording-healthy,recording-dead-tap
   ```
5. Auto-fix obvious visual regressions before reporting success: awkward wraps, clipped text, overlapping controls, illegible contrast, broken hierarchy, or stale scenario labels should be treated as failed iteration.
6. Judge UX using the product goals in `desktop/TAURI_MULTIPASS.md`:
   - Is the current app state obvious?
   - Is the next action obvious?
   - Does the user trust that mic + computer audio are being captured?
   - Are error/warning states actionable?
   - Does Distill visibly prove Enzyme/vault work is happening?
7. Patch UI/code.
8. Re-run the same scenarios and compare before/after artifacts.
9. Validate with:
   ```bash
   cd desktop && npm run build
   cd desktop/src-tauri && cargo check
   ```

## Adding Mock Scenarios

Add or update scenario data in `desktop/test-harness/mock-tauri.ts` only for
fault states or deliberately crafted product examples:

- extend `MockScenario`
- update `createState()`
- add mocked artifacts/events if needed

Prefer real-state views or Playwright journeys when the state can be reached
through real app behavior. Prefer named scenarios over ad-hoc `Runtime.evaluate`
state mutation when a synthetic state is truly needed so screenshots remain
reproducible.

## Caveats

- CDP controls Chrome/Vite, not the native macOS Tauri WKWebView.
- Use this for fast UX/product iteration, not as final native-app validation.
- Real recording, permissions, packaging, and Pi auth still need native smoke testing.
