# Skill Version

Active revision: `skill-revisions/v000-baseline.md`

Prior revision lineage: upstream Enzyme skill baseline copied from `../enzyme-rust/plugin/agent/SKILL.md`.

Source files read by the coordinator:

- `../enzyme-rust/plugin/agent/SKILL.md`
- `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`
- `desktop/test-harness/workspace-diagnostic/LOOP_SPEC.md`
- `desktop/test-harness/workspace-diagnostic/DESIGN_CONTEXT.md`
- `desktop/test-harness/workspace-diagnostic/SPEC.md`
- selected case manifests under `desktop/test-harness/workspace-diagnostic/test-cases/`

Enzyme CLI version observed: `enzyme 0.5.15`

Known limitations:

- This is a targeted run rather than the full fixture suite.
- The run is read-only against fixture workspaces and does not initialize Enzyme, refresh indexes, materialize imports, or build search caches.
- Expected diagnostics are treated as evidence, not an oracle.
