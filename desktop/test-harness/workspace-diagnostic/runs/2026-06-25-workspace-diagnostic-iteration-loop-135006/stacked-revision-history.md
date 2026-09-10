# Stacked Revision History

## v000 Baseline

Source: copied from `../enzyme-rust/plugin/agent/SKILL.md`.

Used for iteration 1 across five cases. The artifacts passed safety and product-fit checks for the three judged priority cases, but the checker found recurring depth issues and the coordinator/user identified a process flaw: artifact assembly could happen without a case-level Enzyme skill diagnostic probe.

## v001 Case-Level Enzyme Probes

Patch area: orchestration/trace discipline.

Files affected:

- `desktop/test-harness/workspace-diagnostic/LOOP_SPEC.md`
- `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`
- `skill-revisions/v001-case-level-enzyme-probes.md`

Verified by v001 probe threads for:

- `google-drive-export`
- `zoom-transcript-dump`
- `codebase-plans-folder`

Promotion recommendation: promote to loop-local evaluator context and loop spec. Do not promote upstream Enzyme skill yet.

## v002 Failure Taxonomy Specificity

Patch area: Failure Model pass.

Files affected:

- `desktop/test-harness/workspace-diagnostic/LOOP_SPEC.md`
- `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`
- `skill-revisions/v002-failure-taxonomy-specificity.md`

Reason: independent checker found missing `Non-Failures`, missing `Ambiguities`, and repeated generic retrieval-impact lines in all three judged priority cases.

Promotion recommendation: keep run-local until a follow-up verifier reruns the Failure Model pass and confirms the stricter structure improves the three priority cases without over-diagnosing `append-only-dated-log` or mishandling `partial-enzyme-or-margins`.
