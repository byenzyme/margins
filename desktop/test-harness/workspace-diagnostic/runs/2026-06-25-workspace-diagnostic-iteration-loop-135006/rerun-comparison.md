# Rerun Comparison

Run ID: `2026-06-25-workspace-diagnostic-iteration-loop-135006`

Scope: v001 regression verification for `google-drive-export`, `zoom-transcript-dump`, and `codebase-plans-folder`. `append-only-dated-log` and `partial-enzyme-or-margins` were not verified in this pass.

## Verdict

v001 fixed the targeted process failure for the three checked cases. Each case now has a case-level Enzyme Skill Diagnostic Probe that loads the upstream skill as operating instructions, applies the setup/indexability lens, runs or reviews the read-only scan path, reads representative files, records hypotheses/falsifiers, records four-pass handoff claims, and marks prior v000 artifacts provisional when trace coverage was insufficient.

## v000 vs v001

- v000 traces proved scan/snapshot-oriented artifact assembly, but used generic representative-file language and lacked case-level hypotheses, falsifiers, and handoff claims.
- v001 probes prove a diagnostic process before downstream artifact use.
- The old downstream pass artifacts are evidence-aligned in the checked cases, but remain provisional because they were not fully rebuilt under v001.

## Recurring Failure Modes

- Static scan output can underrepresent non-Markdown imports, transcript dumps, and nested docs.
- Older evaluator traces can pass on asserted read-only boundaries without strong pre/post mutation evidence.
- Adversarial/product-critic artifacts from v000 remain too templated for case-specific user pushback.

## Remaining Risks

- This verifier did not rerun or regenerate the four downstream pass artifacts.
- Two run cases are out of scope for this v001 regression check.
- Future loops should require recorded pre/post mutation proof, not only mutation assertions.

## Validation Commands Reviewed

- `enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/google-drive-export/workspace`
- `enzyme --version`
- `enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/zoom-transcript-dump/workspace`
- `enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace`
- `enzyme scan -p /Users/example/Hacks/margins/desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace`
- codebase pre/post hidden-folder and checksum guards recorded in the probe
