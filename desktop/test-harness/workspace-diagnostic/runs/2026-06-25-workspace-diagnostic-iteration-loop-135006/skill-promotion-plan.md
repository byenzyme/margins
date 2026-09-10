# Skill Promotion Plan

Run ID: `2026-06-25-workspace-diagnostic-iteration-loop-135006`

## Classification

- Upstream Enzyme skill: do not promote now. The failure was loop orchestration and verifier coverage, not the upstream diagnostic skill behavior.
- Loop-local evaluator context: promote. Keep the v001 requirement that every maker/checker path must consume or produce a case-level Enzyme Skill Diagnostic Probe before pass artifacts count.
- Margins prompt/app: do not promote now. No app UX or product-copy patch is justified by this verifier pass.
- Run-local: keep as revision evidence. `v001-case-level-enzyme-probes.md` should remain the run-local record and active comparison point.

## Revision Status

- `v001-case-level-enzyme-probes.md`: verified for the three priority probe cases. Promote its process rule to the loop-local evaluator context and loop spec.
- `v002-failure-taxonomy-specificity.md`: stacked after the checker recommendation, but not independently verified in this run. Keep it run-local until the Failure Model pass is rerun and checked across the three priority cases plus `partial-enzyme-or-margins` and `append-only-dated-log`.

## Required Promotion Content

- Case probes must load upstream `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions when available, otherwise explicitly use the ported evaluator context.
- Probes must run or review `enzyme scan -p <workspace>` under read-only rules and treat scan as evidence, not diagnosis.
- Probes must read representative files after scan.
- Probes must record hypotheses, confirmations/falsifiers, and handoff claims for Substrate, Failure Model, Repair Policy, and Product Translation.
- Old artifacts must be marked provisional when they lack v001 trace coverage.

## Remaining Risks

- Require stronger mutation guards in the next revision: captured `enzyme --version`, pre/post hidden-folder check, and checksum or file-list comparison.
- Require downstream pass artifacts to cite v001 probe claims when they are reused instead of regenerated.
- Keep unverified cases out of aggregate claims until their probes are checked.
- Do not promote `v002` until a checker confirms that failure models include `Non-Failures`, `Ambiguities`, and evidence-specific retrieval impacts.
