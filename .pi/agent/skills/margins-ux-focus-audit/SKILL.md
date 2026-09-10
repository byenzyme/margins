# Margins UX focus audit

Use when auditing input focus states, keyboard focus order, pane spacing, control borders, memo editor focus, or settings form density in the Margins desktop/Vite harness.

## Approach

Do not run a monolithic all-controls visual test by default. Work skillfully:

1. Read the relevant UI/CSS first (`desktop/src/main.ts`, `desktop/src/styles.css`, and scenario fixtures in `desktop/test-harness/mock-tauri.ts`).
2. Identify shared focus-state families by selector and pane metaphor.
3. Pick one or two representative controls per family and scenario.
4. Use CDP ad hoc to focus/crop only those controls, with enough surrounding context to judge rubbing/crowding.
5. Batch independent scenarios quickly, but inspect the selected crops/images manually before concluding.

## Representative focus families

Usually inspect:

- Session title: `.session-title-input`
- People entry: `.people-pill-input` plus remove pill button
- Memo editor: committed memo textarea and pending `#memo-input-new`
- Settings text fields: advanced path/template inputs
- Settings long textareas: person template, distill instructions
- Settings selects: editor, microphone
- Required/warning fields: `.required-empty`, `.required-group.missing`
- Radio/checkbox rows: AI mode, cleanup, model toggles
- Follow-up chat textarea/actions when on distilled-note scenarios
- Buttons near panel edges: delete, stop recording, choose folder, audio test

## Fast batches

Start Vite visibly:

```bash
cd desktop && npm run dev
```

Open or capture selected scenarios with the normal harness:

```bash
cd desktop && npm run ux:cdp -- --scenarios recording-healthy,recording-with-memo,settings-audio,backchannel-built
```

For focused crops, write a tiny one-off CDP/Runtime snippet from the current task context rather than adding a permanent harness command. The snippet should:

- query the specific selector(s)
- call `el.focus()` and `el.scrollIntoView({ block: 'center' })`
- get `getBoundingClientRect()`
- screenshot a clip expanded by ~16–28px around the rect
- save files named by scenario + focus family

## Judgement criteria

A good focus state:

- makes keyboard location obvious
- does not apply a generic heavy input outline to prose-like memo rows
- keeps halos/rings away from adjacent borders, pills, and panel edges
- fits the pane metaphor: memo row/rail, title subtle edit box, settings form box
- works in dark and light mode when theme changes are relevant

## Report format

End with:

- Source/CSS families inspected
- Scenarios opened/captured
- Representative crops reviewed, with paths
- Findings by focus family
- CSS/layout changes made
- Remaining cases intentionally not sampled and why
