# Skill Patch Summary

Run ID: `2026-06-25-workspace-diagnostic-iteration-loop-135006`

New revision: `skill-revisions/v001-case-level-enzyme-probes.md`

Prior revision: `skill-revisions/v000-baseline.md`

## Failure Pattern

The loop failure was orchestration discipline. The maker became
artifact-oriented and static: it could write scans, snapshots, pass artifacts,
and final diagnostics without first spawning case-level agents that actually
used the upstream Enzyme skill as the diagnostic procedure.

User feedback clarified the intended skill development process: for each case,
spawn a thread that automatically loads the Enzyme skill, reads the selected
test workspace, applies the setup/indexability lens, and produces diagnostic
trace evidence. Artifact writing should follow from that diagnostic process,
not replace it.

## Changed Instruction Or Prompt Area

Patched loop-local process instructions in:

- `desktop/test-harness/workspace-diagnostic/LOOP_SPEC.md`
- `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`

Added run-local stacked revision:

- `desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/skill-revisions/v001-case-level-enzyme-probes.md`

The patch requires case-level Enzyme Skill Diagnostic Probe threads before
Substrate, Failure Model, Repair Policy, Product Translation, or final
diagnostic artifacts count as maker coverage. Probe prompts must explicitly load
`../enzyme-rust/plugin/agent/SKILL.md` as operating instructions when available,
fall back to `ENZYME_EVALUATOR_CONTEXT.md` only when needed, run or review the
read-only scan path, read representative workspace files, and write trace
evidence.

## Why The Change Generalizes

The change does not encode a fixture-specific answer. It makes the loop enforce
the diagnostic procedure that should apply to every workspace class: use the
Enzyme skill to assess indexability, preserve existing structure, treat scan
output as substrate evidence, inspect real files, separate failure modes from
repair policy, and translate the result into the host-product surface.

This should improve raw imports, transcript dumps, code repos, partial tool
state, empty folders, append-only logs, and structured Obsidian vaults because
all of them need a real Enzyme-lens diagnosis before artifact assembly.

## Cases Expected To Improve

- `google-drive-export`
- `zoom-transcript-dump`
- `codebase-plans-folder`
- `partial-enzyme-or-margins`
- `append-only-dated-log`

The expected improvement is trace validity and diagnostic grounding, not a
specific user-facing sentence.

## Cases At Risk Of Regression

- `fresh-empty`, because a probe could make a simple first-capture setup feel
  heavier than needed.
- Small plain folders, because trace work could become verbose without changing
  the product recommendation.
- Highly structured Obsidian vaults, because an overzealous probe could invent
  repairs despite adequate existing conventions.
- Environments without access to `../enzyme-rust`, because the fallback must be
  explicit and must still use the loop-local Enzyme evaluator context as the
  operating lens.

## Old vs New Language

Old:

```text
Read ENZYME_EVALUATOR_CONTEXT.md and the active skill revision. Run the current
Enzyme-backed evaluation against each case and produce the required artifacts.
```

New:

```text
For each assigned case, first run a case-level Enzyme Skill Diagnostic Probe.
Read ../enzyme-rust/plugin/agent/SKILL.md as operating instructions; if it is
unavailable, say so and use ENZYME_EVALUATOR_CONTEXT.md as the ported lens.
Inspect the selected workspace through the Enzyme setup/indexability procedure,
run or review enzyme scan -p <workspace> under the read-only policy, read
representative files, and write a diagnostic trace before assembling pass or
final diagnostic artifacts. Static scan/snapshot-only work is provisional and
does not count as maker coverage.
```

## Verification Needed

The next maker/verifier pass should set the active revision to
`skill-revisions/v001-case-level-enzyme-probes.md`, rerun the same selected
cases, and fail any case whose trace does not prove that the case-level probe
preceded artifact assembly.
