# Skill Version

- Run ID: `2026-06-25-workspace-diagnostic-123440`
- Repo HEAD: `a396397`
- Evaluation surface: `skill-only`
- Dedicated workspace diagnostic skill found: no
- Candidate basis: existing Margins capture/distillation skills plus fixture manifests, expected diagnostics, read-only file evidence, and `enzyme scan`.
- Active stacked revision: `skill-revisions/v002-import-narrative-and-language.md`
- Enzyme evaluator context: `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`, ported from `../enzyme-rust/plugin/agent/SKILL.md`
- Original Enzyme skill source read this pass: yes, `../enzyme-rust/plugin/agent/SKILL.md`
- Enzyme CLI version used for read-only scans: `enzyme 0.5.15`

## Skill Files Read
- `desktop/src-tauri/resources/skills/margins-desktop/SKILL.md` sha256 `0074be93a8e46ab0`
- `skills/margins/SKILL.md` sha256 `eda10c82ccf65e44`

## Limitation
The current Margins skills are focused on capture-to-note distillation, bounded Enzyme search, and preserving capture when vault context is unavailable. They do not define a first-run workspace diagnostic contract, so these artifacts are best-candidate diagnostics rather than direct skill output.

## Active Patch
`v002-import-narrative-and-language` builds on v001. v001 tightens the
read-only Enzyme substrate for fixtures that already contain `.enzyme/` or
`.margins/`: scan a temporary copy, record pre/post mutation guards, and keep
private cache, import, and retrieval repair as separate optional actions.

v002 adds the judge-recommended quality layer: name import/cache/export state in
plain language, translate manifest convention tokens into outcome language,
report cache/import/export files regardless of dot prefix, and name competing
destinations with calibrated confidence.
