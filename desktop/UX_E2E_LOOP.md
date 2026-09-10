# Margins hybrid UX E2E loop

This loop is for product-aware quality improvement, not static screenshot
checking. It uses Obsidian/product context to form a user-model, drives a real
journey through the app, runs the distillation path through an LLM, records
video/timing evidence, then patches the UX only when the evidence shows a
bounded improvement.

For remote/headless command routing, see `desktop/REMOTE_HEADLESS_DEV.md`.

## Goal

Improve Margins's end-to-end experience for one user job at a time:

1. finish or import a capture;
2. understand what is happening while Margins makes a connected note;
3. inspect how the note was generated;
4. refine the distillation without losing context;
5. leave with a trusted Obsidian note.

The loop should learn the app's product shape over time. Static checks are only
guardrails. The main evaluator is a research-informed agent reviewing videos,
traces, timing logs, screenshots, visible text, and generated notes.

## Durable Spine And Per-Job Skin

Keep the job-specific evaluation philosophy. A useful UX loop evaluates one real
job at a time, because generic screen checks collapse into "does it render?"
The reusable part is not the job; it is the evidence and judgment spine:

- durable spine: artifact capture, timing marks, generated-note capture, copy
  lint, vault-context checks, and `judge-verdict.json`;
- per-job skin: fixture, runtime vault seed notes, journey intent, typed
  refinement, and falsifiable claims;
- runner hooks: role/state/test-id hooks that keep the runner alive while the
  judge critiques mutable copy and layout.

A Playwright pass by itself is not a completed UX evaluation. It is artifact
capture. A real loop pass must also produce a verdict that scores the artifacts
against the job brief and claim IDs.

## When to invoke it

Use this loop when the request is about a user job or product-quality concern,
not just a static visual regression.

Good inputs:

```text
Evaluate whether the post-meeting note flow feels alive before the note appears.
```

```text
Improve refinement after distillation; users should feel like they are editing
the same note, not starting over.
```

```text
Run the hybrid UX loop for customer-call-connected-note.
Concern: provenance may be too much clutter.
Mode: report-only.
```

```text
Changed files: desktop/src/main.ts, desktop/src/styles.css.
Concern: Distill progress and how-generated boxes.
Allow real LLM: yes.
Patch if bounded.
```

The loop coordinator should normalize these into:

- `job` — the user job being evaluated;
- `concern` — the UX risk or product question;
- `fixture` — the capture fixture to use;
- `allow_llm` — whether real model calls are allowed;
- `mode` — report-only or patch-if-bounded;
- `changed_files` — optional diff context;
- `latency_budget` — optional target such as "first visible feedback < 500ms".

## User model

The research agent starts from:

- `desktop/UX_REVIEW.md`
- `desktop/TAURI_MULTIPASS.md`
- `desktop/UX_CDP_LOOP.md`
- `desktop/test-harness/ux-loop/research-brief.md`
- relevant Obsidian notes retrieved with Enzyme
- the current app code and previous run observations

It should produce or update a short working brief before planning a journey:

- user mindset;
- current job-to-be-done;
- screen-to-screen path;
- expected copy/information hierarchy;
- what counts as clutter;
- what latency must be explained;
- what would make the user lose trust.

The research output should not be a generic UX checklist. It should be a
job-specific brief with falsifiable claims. Example:

```md
Job: customer-call-connected-note
Mindset: tired post-meeting reviewer
Claim: The user can tell note-making has started before first note text appears.
Could be false if: the screen sits on generic loading for several seconds.
Evidence to collect: video, visible text snapshots, timing to first progress,
timing to first note token.
```

## Vault context pass

Run this pass before planning the journey whenever the concern depends on user
mental models, note quality, provenance, refinement, setup language, or how Margins
should explain vault work. Skip it only for narrow mechanical bugs, pure CSS
regressions, or harness failures where product context cannot change the right
fix.

In this repo, use the local Enzyme CLI against the user's Obsidian vault for
agent-run context retrieval:

```bash
enzyme status --vault ~/obsidian
enzyme petri --vault ~/obsidian --query "<job/concern>" --top 8
enzyme catalyze --vault ~/obsidian --limit 6 "<focused product/context query>"
```

If an MCP/hosted Enzyme endpoint is configured but returns an availability
error, do not stop the loop there. Fall back to the CLI route above and note the
fallback in the brief.

The pass should answer three concrete questions:

1. What does the user already call this job, artifact, or workflow?
2. What prior notes, projects, people, tags, or recurring themes should the app
   appear to understand?
3. What would make a user trust that Margins used the vault without showing a log?

Use Enzyme/Obsidian retrieval in this order:

1. Search for the user's own language around the job and concern, such as
   "meeting note", "customer call", "post-meeting review", "setup", "vault
   context", "provenance", or the fixture's domain terms.
2. Search for adjacent product memories: prior UX reviews, naming decisions,
   user complaints, sample notes, or notes about how Margins should feel.
3. Search for fixture/domain anchors from `memo.md`, `aligned.md`, and
   `capture-context.md`, then compare whether the generated note and UI surface
   the same ideas.

Keep retrieval bounded. Usually 3-7 high-signal notes or catalysts are enough.
Do not dump note contents into the brief. Record short breadcrumbs instead:

```md
Vault context:
- Source: <note title or catalyst> — <one sentence about why it matters>
- User language: <terms the product should prefer or avoid>
- Trust signal: <what the UI/generated note should make visible>
```

Convert vault context into falsifiable claims before running the journey:

```md
Claim: A post-meeting reviewer can tell Margins used related notes without reading
tool logs.
Evidence to collect: visible text, side-panel trace labels, generated note links.
Could be false if: the UI says only generic "processing", or the note lacks any
recognizable related-note language from the vault pass.
```

The judge should use vault findings as context, not as a script. A run fails only
when the UI or generated note contradicts the user's language, hides an important
trust signal, or makes vault work look like developer plumbing.

## Hybrid fixture strategy

The loop does not start with real microphone input. VPS runs should be stable and
headless. Instead, the loop fixtures capture the durable artifacts a real capture
would have produced:

```text
desktop/test-harness/ux-loop/fixtures/<fixture>/
  memo.md
  aligned.md
  capture-context.md
  settings.json
  expected-user-job.md
```

The fixture is not a mocked note. It is a realistic transcript/memo substrate.
Browser Playwright runs should create a run-scoped Obsidian-shaped vault from the
fixture before opening the app:

```text
desktop/test-harness/ux-loop/runtime/<run-id>.json  # served to Vite
desktop/ux-e2e-runs/<run-id>/runtime-vault/
  .obsidian/
  .margins/
  inbox/
  people/
```

The runtime vault should be deleted after the run. Evidence artifacts stay under
`ux-e2e-runs/<run-id>/` so reviewers can inspect the note, trace, video, and
timings without leaking temp vault state into the workspace. This is real
filesystem-backed vault context for the browser harness; it is not native Tauri
WebView automation.

Selected E2E runs can also pass the same substrate into the real `pi_distill`
path so the LLM, tool events, streaming note output, and refinement/responsiveness
can be assessed. That command may spend model/API resources and should only run
when explicitly allowed.

## Playwright video lane

The hybrid loop needs reviewable motion evidence, not only still screenshots.
The runner/verifier path should use the Playwright CLI or Playwright test runner
once it is wired, with video recording enabled for every job journey.

Required video artifacts per run:

```text
desktop/ux-e2e-runs/<run-id>/
  journey.webm          # raw browser recording when Playwright emits WebM
  journey.mp4           # review/share artifact when conversion is available
  trace.zip             # Playwright trace for interaction debugging
  screenshots/
  visible-text/
  timing.json
  runtime-vault-manifest.json
  generated-note.md
  distill-trace.json
  judge-verdict.json
```

Runner requirements:

1. Prefer a deterministic Playwright command that can be rerun from the repo,
   such as an npm script wrapping `playwright test` with a selected job/fixture.
2. Record video from journey start through completion or failure. Include the
   slow middle states before first note text, not only the final note.
3. Save the raw video even when conversion fails.
4. Convert to MP4 when the local toolchain supports it, and record the conversion
   command in the run manifest. Playwright commonly records WebM; MP4 may require
   an explicit ffmpeg step.
5. Keep the video path stable enough for judge/verifier threads to reference it
   without hunting through Playwright's default output folders.

The judge should review the MP4/WebM before reading implementation diffs. Video
is the source of truth for latency, confusing transitions, loading states,
scroll jumps, and whether provenance competes with the note.

## Judge Verdict

Every run that claims to evaluate UX quality should write:

```text
desktop/ux-e2e-runs/<run-id>/judge-verdict.json
```

Minimum schema:

```json
{
  "run_id": "customer-call-real-llm",
  "job": "customer-call-connected-note",
  "note_source": "real | mock-or-unknown",
  "overall": "pass | risk | fail",
  "checks": [
    {
      "id": "copy-no-private-language",
      "kind": "copy_lint",
      "severity": "high",
      "verdict": "pass | risk | fail",
      "evidence": ["visible-text/02-writing-note.txt"]
    }
  ],
  "claims": [
    {
      "id": "started-before-first-note-token",
      "verdict": "pass | risk | fail",
      "evidence": ["timing.json#click_to_first_note_token_ms"]
    }
  ]
}
```

The runner should not treat the verdict as a replacement for human/product
judgment. It gives reviewer threads a stable packet: what was checked
deterministically, what needs judgment, and what claim IDs changed before/after.

## Enforced Vs Judged

Separate hard checks from taste checks:

- enforced by scripts/spec: artifact presence, note source, timing marks, no
  forbidden private language in visible surfaces, vault-relative saved path;
- judged by reviewer thread: brand fit, visual hierarchy, whether provenance
  competes with the note, generated-note usefulness, whether the UI matches the
  user's mental model from Obsidian/Enzyme context.

The runner should avoid pinning volatile copy unless that copy is the claim
being evaluated. Prefer stable roles, app state, and test IDs so the copy can
change without breaking artifact capture.

Implementation target:

```bash
cd desktop
npm install -D @playwright/test
npx playwright install chromium
npm run ux:e2e -- --project=chromium
```

Expected scripts once wired:

```json
{
  "ux:e2e": "playwright test -c test-harness/ux-loop/playwright.config.ts",
  "ux:e2e:headed": "playwright test -c test-harness/ux-loop/playwright.config.ts --headed --workers=1",
  "ux:e2e:mp4": "node scripts/ux-e2e-mp4.mjs",
  "ux:e2e:report": "node scripts/ux-e2e-report.mjs"
}
```

The Playwright config should set `outputDir` under `desktop/ux-e2e-runs/`, use a
single Chromium worker, start/reuse Vite through `webServer`, enable `trace:
"on"`, and enable full-journey video with an explicit size matching the desktop
window. The first pass should use the browser harness at `127.0.0.1:5173`, not
native Tauri WebView automation.

The spec should copy video to stable names after the page/context closes:

```ts
const video = page.video();
await page.close();
if (video) await video.saveAs(testInfo.outputPath("journey.webm"));
```

Then convert for review when `ffmpeg` is available:

```bash
ffmpeg -y -i journey.webm \
  -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart -crf 23 \
  journey.mp4
```

## Loop contract

The machine-readable contract lives at:

```text
desktop/test-harness/ux-loop/loop-contract.json
```

It follows the loop-engineering shape:

- `goal`
- `trigger`
- `intake`
- `research`
- `journey_plan`
- `act`
- `verify`
- `memory`
- `limits`
- `hand_off`
- `done`

## First fixture

`customer-call` models a dense post-meeting note-making job:

- a long customer-discovery transcript;
- several memo marks;
- ambiguous user intent;
- action items and open questions;
- content that should be summarized, not dumped;
- enough length to expose first-token latency and streaming UX.

Run only the real-LLM distill substrate:

```bash
cd desktop
npm run ux:e2e:distill-fixture
```

This calls:

```bash
cd desktop/src-tauri
cargo run --example pi_distill_fixture -- ../test-harness/ux-loop/fixtures/customer-call
```

The example writes into a temporary workspace and prints emitted stages plus the
saved note path. It requires the same model/Pi/API configuration as the normal
desktop distillation path.

## Video loop shape

Once the Playwright driver is added, the job should be:

1. load the app in a browser harness scenario backed by the same fixture;
2. open the finished capture;
3. click the note-making action;
4. capture video, trace, screenshots, text, AX tree, console, and timing marks;
5. wait for real streamed note output from the LLM bridge;
6. inspect generation/provenance boxes;
7. issue one refinement request;
8. capture before/after note state;
9. judge the video against the research brief;
10. patch one bounded UX issue and rerun the same job.

## bb thread orchestration

For serious runs, use bb child threads to isolate judgments:

| Thread | Inputs | Output |
|---|---|---|
| Research | user request, product docs, Obsidian/Enzyme results | job-specific UX brief |
| Planner | UX brief, fixture inventory, app source | Playwright/CDP journey plan |
| Runner | journey plan, fixture, app URL | video, trace, screenshots, timing log, generated note |
| Judge | UX brief + artifacts | pass/risk/fail claims with evidence |
| Patch | judge findings + relevant files | one bounded implementation change |
| Verifier | original claims + patched app | before/after comparison |
| Memory | all results | updated observations for future runs |

The parent thread coordinates artifacts and stop conditions. It should avoid
letting the patch thread grade its own work.

## Invocation outcomes

Given a product/UX goal, the loop should:

1. retrieve product context;
2. produce a job-specific brief;
3. choose or create a fixture;
4. run CDP/Playwright and, when allowed, the real LLM distill fixture;
5. judge the evidence;
6. patch one bounded issue or escalate;
7. rerun the same journey;
8. record what changed and what the loop learned.

Given a specific concern, the loop should narrow to one falsifiable claim and
collect evidence against that claim before patching.

Given a changed area or diff, the loop should infer the affected user job and
select the smallest journey that exercises it.

## Timing metrics

Every real-LLM run should record:

- click to first visible progress;
- click to model/session start;
- click to first tool/provenance event;
- click to first note token;
- first note token to complete;
- refine click to first note token;
- UI render lag after streamed events;
- total wall-clock and model/tool error state.

Latency is not automatically failure. Unexplained latency is. A slow step passes
only when the UI makes useful work visible and preserves user trust.

## Stop conditions

Pass when the selected user job improves with evidence:

- clearer path;
- less clutter;
- better copy placement;
- better responsiveness or better explanation of waiting;
- stronger trust in the final note.

Escalate when:

- the next change is product/taste judgment rather than mechanical UX repair;
- the generated note quality is the bottleneck, not the interface;
- the model/tool provider fails in a way the app cannot fix;
- two patch attempts fail to improve the same judged claim.

## Current implementation status

Implemented now:

- loop contract: `desktop/test-harness/ux-loop/loop-contract.json`;
- baseline research brief: `desktop/test-harness/ux-loop/research-brief.md`;
- first job: `desktop/test-harness/ux-loop/jobs/customer-call-connected-note.json`;
- first long fixture: `desktop/test-harness/ux-loop/fixtures/customer-call/`;
- Playwright Test dependency, config, scripts, and first browser journey smoke;
- WebM-to-MP4 conversion helper;
- real-LLM fixture runner: `desktop/src-tauri/examples/pi_distill_fixture.rs`;
- npm entry point: `npm run ux:e2e:distill-fixture`.

Still to wire:

- additional Playwright journeys beyond the first customer-call smoke;
- richer run manifest aggregation across traces/screenshots/video;
- bridge from browser harness to streamed real-LLM fixture output;
- run memory under `desktop/ux-e2e-runs/`.
