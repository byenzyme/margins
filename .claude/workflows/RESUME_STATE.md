# Design roadmap — resume state

Updated 2026-06-10 ~07:20 (during usage block 06:00–11:00).

If a session restart or rate limit killed work, resume from here. Workflow runs are resumable:
`Workflow({scriptPath: ".claude/workflows/design-roadmap-implement.js", args: <args>, resumeFromRunId: "<id>"})` — agents with completed results in the run's journal.jsonl return cached; only unfinished agents re-run.

Transcript dirs live under:
`~/.claude/projects/-Users-example-Hacks-margins/d6a2737c-ebbc-4d4b-a4fa-b31f6e2d560a/subagents/workflows/<runId>/`

## Run ledger

| Run | Args | Status |
|---|---|---|
| wf_af0500c7-6db | demand research (fable) | killed deliberately — superseded |
| wf_5b98c739-132 | demand research (sonnet) | COMPLETE → .pi/reports/category-design-demand.md |
| wf_e3646ac5-cbd | implement {"tiers":[0]} | COMPLETE — items 1,2,3 done, typecheck pass, opus-approved, harness pass (1 CSS fix applied by judge) |
| wf_ea428799-8ba | implement {"tiers":[2]} | COMPLETE — items 7,8,9,10 done, typecheck pass, opus-approved, harness all-pass. "Enzyme connections" fixture heading fixed inline by orchestrator (mock-tauri.ts → "Related notes"). |

## Wave order (from .pi/reports/design-synthesis-roadmap.md)

ROADMAP COMPLETE 2026-06-10 ~09:30. All 20 items implemented, opus-approved, harness all-pass across all five waves (final wave wf_a3492aee-ece). Total cost ~$50 in subagent tokens across waves. Reset-timer cron deleted — nothing left to resume.
Working tree holds the combined diff (35 files, +2221/−606, includes pre-existing branch work) — awaiting user review; nothing committed.

## Open follow-ups

- distill-complete shows three concurrent green "NOTE SAVED"/"Saved" affirmations (header, panel header, status rail) — collapse to one or two when the Tier 5 wave (item 20) rebuilds that screen. Flagged by wave-3 harness judge.
- "REQUIRED" settings badge verified amber (`var(--yellow)`), not red — wave-12 contract holds; no action.

- Add a `recording-startup-failed` scenario to the ux:cdp harness (coverage gap flagged by wave-1 harness judge — item 2's banner has no screenshot evidence).
- Harness judge accepted "dismiss" only on the soft `waiting` audio banner, not the critical `dead` one (two recovery paths + setup link, no dismiss). Confirm this interpretation is acceptable.
- Pre-existing, not wave work: "Enzyme connections" heading inside generated note fixture (mock-tauri.ts:885) — fix in the note template, not UI chrome.

## Rules for any resumed implementation

- Models: opus = spec/review/harness judgment, sonnet = implement, haiku = mechanical. NEVER fable inside workflows — fable orchestrates only.
- Never git checkout/restore/stash; branch has in-flight changes. Do not commit.
- Typecheck: cd desktop && npx tsc --noEmit. Rust: export CARGO_TARGET_DIR=$HOME/.cache/margins-cargo-target.
