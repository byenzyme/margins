# UX loop harness

This folder contains the agent-readable substrate for Margins's hybrid UX E2E
loop. Use it when a user asks to improve or evaluate a full user job rather than
a static screen.

## Entry points

- Design doc: `desktop/UX_E2E_LOOP.md`
- Contract: `loop-contract.json`
- Baseline research brief: `research-brief.md`
- Jobs: `jobs/*.json`
- Fixtures: `fixtures/*`

## How an agent should invoke the loop

Normalize the user's request into:

```json
{
  "job": "customer-call-connected-note",
  "concern": "users cannot tell what is happening before the first note token",
  "fixture": "customer-call",
  "allow_llm": true,
  "mode": "report-only | patch-if-bounded",
  "changed_files": []
}
```

Then:

1. Read `CLAUDE.md`, `desktop/UX_E2E_LOOP.md`, `desktop/UX_REVIEW.md`,
   `desktop/TAURI_MULTIPASS.md`, this README, and the selected job/fixture.
2. Run the vault context pass when product/user framing, note quality,
   provenance, refinement, setup language, or trust is part of the task:
   search Enzyme/Obsidian for the user's own language, adjacent product
   memories, and fixture/domain anchors; keep 3-7 high-signal breadcrumbs.
3. Convert that context into a job-specific UX brief with falsifiable claims,
   including what terms to prefer/avoid and what trust signal should be visible.
4. Plan a journey dynamically from the brief and app state.
5. Run cheap CDP scenarios first when useful.
6. If `allow_llm` is true, run:
   ```bash
   cd desktop
   npm run ux:e2e:distill-fixture
   ```
7. Run the Playwright journey through the CLI/test runner with video enabled.
   The spec should create a run-scoped runtime vault before opening Vite and
   remove it afterwards:
   `test-harness/ux-loop/runtime/<run-id>.json` feeds the browser harness;
   `ux-e2e-runs/<run-id>/runtime-vault/` holds the temp Obsidian-shaped vault.
   Save raw video, trace, screenshots, timings, text,
   accessibility tree, generated note, and distill trace.
   - Preserve Playwright's raw video even if it is WebM.
   - Produce `journey.mp4` when ffmpeg or an equivalent conversion tool is
     available.
   - Store artifacts under `desktop/ux-e2e-runs/<run-id>/` with stable names.
8. Run the durable judge spine:
   ```bash
   cd desktop
   npm run ux:e2e:judge
   ```
   It reads the run artifacts and writes `ux-e2e-runs/<run-id>/judge-verdict.json`.
   Treat a green Playwright run without a verdict as artifact capture only, not
   a completed UX evaluation.
9. Judge evidence against both the UX claim and the vault-context claim before
   patching. Use `judge-verdict.json` as the auditable record.
10. Patch one bounded issue only.
11. Rerun the same journey and compare claim IDs before/after.

## Three-layer split

- Durable spine: artifact capture, deterministic checks, judge verdict schema,
  and before/after comparison. This should survive product redesigns.
- Per-job skin: fixture, runtime vault seed notes, journey intent, typed
  refinement, and falsifiable claims. This is allowed to change cheaply.
- Runner hooks: prefer roles, state, and stable test IDs over exact copy. The
  runner should keep collecting artifacts when copy changes; the judge should
  decide whether the new copy is better.

## bb thread split

Use separate bb child threads for substantial runs:

- research;
- journey planner;
- runner;
- judge;
- patcher;
- verifier;
- memory/synthesis.

The patcher should not be the final judge.

## Current fixture

`fixtures/customer-call` is a long transcript/memo substrate for a post-meeting
customer-call note-making job. The Playwright runner turns it into a temporary
vault with seeded related notes and people notes, then cleans that vault after
the run. The real LLM runner can use the same substrate to exercise distillation
latency, streaming, and generated-note quality when model calls are allowed.
