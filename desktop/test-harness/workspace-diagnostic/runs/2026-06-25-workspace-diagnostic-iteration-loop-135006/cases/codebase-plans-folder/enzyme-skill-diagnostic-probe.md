# Enzyme Skill Diagnostic Probe: codebase-plans-folder

## Skill Source Loaded
- Loaded `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions.
- Loaded loop context: `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`.
- Loaded loop spec: `desktop/test-harness/workspace-diagnostic/LOOP_SPEC.md`.
- Loaded active revision: `desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/skill-revisions/v001-case-level-enzyme-probes.md`.

## Case Scope
- Case: `codebase-plans-folder`.
- Workspace: `desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace`.
- Case evidence loaded: `manifest.json` and `expected-diagnostic.md`.

## Exact Commands
- `enzyme --version`
  - Result: `enzyme 0.5.15`.
- Mutation guard before scan:
  - `find desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace -name .enzyme -o -name .margins -o -name .obsidian -o -name .git -o -name node_modules -o -name target -o -name dist -o -name build`
  - Result: no matching hidden/tool/generated folders.
  - `find desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace -type f | sort | shasum -a 256`
  - Result: `b09f8746079f8e0c6ad24a26db744f2644c90916df62a1361ed314ec32578bde  -`.
- Read-only scans:
  - `enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace`
  - `enzyme scan -p /Users/example/Hacks/margins/desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace`
  - Both scans reported `files: 1`, `entities: ["folder:."]`, and sampled only `README.md`.
- Workspace structure check:
  - `find desktop/test-harness/workspace-diagnostic/test-cases/codebase-plans-folder/workspace -maxdepth 3 -type d -o -type f | sort`
- Mutation guard after scan:
  - Same hidden/tool/generated-folder check: no matches.
  - Same file checksum: `b09f8746079f8e0c6ad24a26db744f2644c90916df62a1361ed314ec32578bde  -`.
- Disallowed commands not run: no `--write-config`, `init`, `refresh`, `apply`, `petri`, or `catalyze`.

## Mutation Guard
- Scan ran directly against the fixture, not a temporary copy, because no `.enzyme/`, `.margins/`, `.obsidian/`, `.git/`, `node_modules/`, `target/`, `dist/`, or `build/` existed in the workspace.
- No fixture files changed across pre/post checks.
- Writes from this probe were limited to this run case directory.

## Representative Files Read
- `README.md`: states this is an Atlas Sync partial monorepo; code lives in `src/`, planning partly in `docs/plans/`, call notes mixed into `docs/notes/`, and raw transcript notes are ad hoc.
- `docs/plans/meeting-2026-04-01.md`: dated meeting note with `owner`, `status`, attendees, and action-owner/status body markers.
- `docs/plans/weekly-sync.md`: undated filename with owner/status markers and body note that no reliable path exists for recurring check-ins.
- `docs/plans/sprint-plan.md`: undated planning note with owner/status blocks.
- `docs/plans/retro.md` and `docs/plans/retro-notes-2026-02.txt`: retro material with inconsistent date handling and explicit "create meeting folder?" prompt.
- `docs/plans/incident-playbook.md`: incident/project doc with dates and owner/status material, not cleanly a meeting note.
- `docs/plans/roadmap.md`: malformed frontmatter-like block with planning/product tags and dated roadmap bullets.
- `docs/notes/weekly-sync-2026-03-19.md`: stronger meeting note convention: frontmatter date/tags, participants, decisions, TODO to add notes to a meeting folder.
- `docs/notes/meeting-2026-04-01.md`: meeting note with draft frontmatter and date only in body.
- `docs/notes/incident-playbook.md` and `docs/notes/sprint-plan.md`: docs/notes also mixes incident/playbook and planning material.
- `raw-meeting.txt`: two transcript excerpts; explicitly suggests `docs/meetings maybe, but don't move anything`.
- `package.json` and `src/sync/conflicts.ts`: confirm this is a codebase, with source/build concerns separate from meeting-note capture.

## Enzyme-Lens Hypotheses
- H1: The workspace has enough markdown/text source for an Margins first capture, but not enough stable convention to treat any existing folder as the canonical meeting-note destination.
- H2: `docs/plans` and `docs/notes` are useful retrieval signal but are mixed surfaces; placing new notes into either would increase ambiguity.
- H3: `docs/meetings` is the safest additive next-note destination because it is adjacent to existing docs and is explicitly suggested by source text.
- H4: Existing code/source files should be treated as repo substrate and mostly excluded from meeting-memory repair decisions.
- H5: The `enzyme scan` result undercovers the workspace and must not be treated as the complete substrate map.

## Confirmations / Falsifiers
- Confirmed H1: multiple files contain meeting-like content, dates, owner/status markers, attendees, and decisions; no single recurring note convention dominates.
- Confirmed H2: `docs/plans` contains meetings, roadmap, sprint plan, incident playbook, and retro material; `docs/notes` contains weekly sync, incident playbook, sprint plan, and meeting files.
- Confirmed H3: `raw-meeting.txt` says "docs/meetings maybe, but don't move anything"; `docs/notes/weekly-sync-2026-03-19.md` says meeting notes still land in multiple places and TODO is to add notes to a meeting folder.
- Confirmed H4: `package.json` and `src/sync/conflicts.ts` are ordinary codebase files, useful for workspace type but not primary meeting-memory evidence.
- Confirmed H5: both relative and absolute `enzyme scan -p` returned only `README.md`, while direct file reads found many markdown/text docs under `docs/`.
- Falsifiers not found: no existing `docs/meetings` folder; no stable people/company folder convention; no strong evidence that old files should be moved, renamed, retagged, or normalized before capture.

## Substrate Handoff Claims
- Workspace class: codebase with mixed docs and partial planning/meeting notes.
- Already indexable signal: README overview, dated/undated meeting notes, owner/status/action markers, participants, decisions, roadmap dates, raw transcript excerpts.
- Weak signal: split between `docs/plans`, `docs/notes`, and `raw-meeting.txt`; inconsistent filename/body date conventions; malformed frontmatter in `docs/plans/roadmap.md`; raw transcript lacks durable summary structure.
- Scan caveat: current Enzyme scan sees only `README.md`; downstream substrate maps must combine scan evidence with direct representative file reads.
- Existing conventions to preserve: project docs under `docs/`, owner/status/action body markers, occasional frontmatter in `docs/notes`, and date-bearing filenames where already present.

## Failure-Model Handoff Claims
- Failure mode: scan undercoverage. Evidence: `enzyme scan -p` reports one file despite many docs/text files. Severity medium, confidence high for this run. Retrieval impact: scan-only setup would miss the actual meeting-note surfaces.
- Failure mode: meeting destination ambiguity. Evidence: meeting material appears in `docs/plans`, `docs/notes`, README transcript, and `raw-meeting.txt`. Severity medium, confidence high. Retrieval impact: future notes could scatter and weaken continuity.
- Failure mode: codebase/docs mixing. Evidence: source files and package metadata share the root with notes. Severity low-to-medium, confidence high. Retrieval impact: repo artifacts could dominate setup language if not separated from meeting-memory surfaces.
- Non-failure: the workspace is not unusable and does not require migration before first capture.

## Repair-Policy Handoff Claims
- No-op / initialize as-is: acceptable for reading existing material, but scan undercoverage means a scan-only cache/setup claim should be treated cautiously.
- Minimal reversible repair: create only new captures under `docs/meetings/YYYY-MM-DD meeting-title.md`; this is additive and source-backed.
- Medium convention repair: if approved later, add a lightweight meeting-note template using date, participants, decisions, owners/actions, following existing owner/status language.
- Heavy migration: moving, renaming, deduplicating, retagging, or normalizing `docs/plans`, `docs/notes`, or raw transcript files requires backup, preview, and explicit approval.
- Do-not-touch: source code, package metadata, existing plan docs, existing notes, raw transcript exports, and any people/team taxonomy.

## Product-Translation Handoff Claims
- Margins should say the user can start now.
- Recommended first capture path: `docs/meetings/YYYY-MM-DD planning-checkin.md` or equivalent meeting title.
- Product copy should separate: choosing a folder, reading workspace shape, building a private search cache, importing history, and repairing structure.
- Product copy should not imply old files will be moved, renamed, retagged, or normalized.
- Product copy should avoid Enzyme internals and avoid "vault health" language.

## Prior Baseline Artifact Status
- Status: provisional.
- Reason: the existing `substrate-map.md`, `failure-model.md`, `repair-policy.md`, and `product-translation.md` mostly align with the evidence found here, especially scan undercoverage, mixed meeting destinations, and the additive `docs/meetings` recommendation.
- Coverage gap: the prior `evaluator-trace.md` names v000 baseline and lists generic "selected note/import/cache files" rather than v001-required case-level hypotheses, exact representative files, confirmations/falsifiers, and handoff claims.
- Recommendation: do not treat the prior baseline artifacts as fully valid v001 coverage unless a checker explicitly accepts this probe as supplemental trace. Otherwise rerun or revalidate the four pass artifacts against this probe before using them for promotion or regression judgment.
