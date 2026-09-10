# Margins UX CDP screenshot review

Use when implementing or reviewing Margins desktop/Vite UX changes with the Chrome/CDP harness. This skill is for choosing which screenshots/artifacts to review, not for native macOS/Tauri validation.

## Core rule

Review screenshots selectively, based on what changed. Do not blindly run every scenario for a small localized change, but do not ship a meaningful UX change without visual evidence.

For every reviewed scenario, inspect at least:

- `<scenario>.png`
- `<scenario>.txt`
- `<scenario>.console.json`

Use `<scenario>.ax.json` when accessibility, labels, focus, buttons, forms, or semantic structure changed.

Always compare the same scenarios before/after when judging a visual change.

## Scenario selection matrix

Pick the narrowest set that covers the changed UI family plus one adjacent regression scenario when the change touches shared layout/CSS.

| Change made | Screenshot scenarios to inspect |
|---|---|
| First-run, onboarding, empty-state, setup guidance | `first-run`, `home-empty`, `settings-audio` |
| Settings layout, audio device fields, permissions/setup copy, advanced fields | `settings-audio`; add `recording-healthy` if readiness/status copy is shared |
| Home/session list, session cards, navigation, empty library | `home-empty`, `backchannel-built`, `distill-complete` |
| Recording screen layout, timer, start/stop affordance, capture status | `recording-healthy`, `recording-muted-mic`, `recording-dead-tap` |
| Audio health warnings/errors, mic/tap readiness, recoverability copy | `settings-audio`, `recording-healthy`, `recording-muted-mic`, `recording-dead-tap` |
| Memo editor, timestamped notes, in-meeting input ergonomics | `recording-with-memo`, `recording-healthy` |
| Backchannel/raw evidence, transcript preview, speakers, memo-to-evidence review | `backchannel-built`, `recording-with-memo` |
| Distill/progress/job state, Enzyme/vault activity indicators | `distill-running`, `distill-complete` |
| Pi login/auth/dependency errors, retry/error recovery | `distill-error-pi-login`, `distill-running` |
| Final note/review screen, vault/open/copy/reprocess actions | `distill-complete`, `backchannel-built` |
| Follow-up chat/revision actions on distilled notes | `distill-complete` plus any scenario fixture that exposes the changed follow-up state |
| Global CSS, typography, colors, pane spacing, shared buttons/cards | Baseline smoke: `settings-audio`, `recording-healthy`, `backchannel-built`, `distill-error-pi-login`, `distill-complete` |
| Focus rings, keyboard focus order, form density, control borders | Use `.pi/agent/skills/margins-ux-focus-audit/SKILL.md`; usually include `settings-audio`, `recording-with-memo`, `backchannel-built` |
| Mock Tauri/scenario fixture behavior only | The changed scenario(s) plus one unrelated smoke scenario, usually `recording-healthy` |
| Tauri command facade or native plumbing with no visible UI change | Screenshots are optional; run build/native checks. If the visible state can change, use the row for that state. |

## Baseline set for broad UX changes

For a broad redesign, theme change, shared layout refactor, or any change that affects multiple durable states, inspect the baseline desktop UX set:

```bash
cd desktop && npm run ux:cdp -- --scenarios settings-audio,recording-healthy,recording-dead-tap,backchannel-built,distill-running,distill-error-pi-login,distill-complete
```

## Workflow

1. Read `desktop/UX_CDP_LOOP.md`, `desktop/UX_REVIEW.md`, `desktop/TAURI_MULTIPASS.md`, and the relevant source/CSS/fixture files.
2. Choose scenarios from the matrix above. If unsure, include the adjacent warning/error state.
3. Start Vite visibly if needed:
   ```bash
   cd desktop && npm run dev
   ```
4. Capture selected scenarios:
   ```bash
   cd desktop && npm run ux:cdp -- --scenarios <comma-separated-scenarios>
   ```
   Or use the Pi `margins_ux_cdp` tool with the same scenario list.
5. Inspect screenshots/text/logs manually before concluding.
6. Patch code.
7. Re-run the same selected scenarios and compare before/after artifacts.
8. In the final report, cite the scenario names and artifact paths reviewed.

## Report format

End with:

- Change category and selected scenario rows
- Screenshots/artifacts reviewed, with paths
- Before/after UX findings using visible evidence
- Scenarios intentionally skipped and why
- Validation/tests run
