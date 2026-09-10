# Enzyme Skill Diagnostic Probe: zoom-transcript-dump

## Skill Source Loaded
- Read `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions.
- Read `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`.
- Active revision: `skill-revisions/v001-case-level-enzyme-probes.md`.

## Command
- `enzyme --version` -> `enzyme 0.5.15`
- `enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/zoom-transcript-dump/workspace`
- Not run: `--write-config`, `init`, `refresh`, `apply`, `petri`, `catalyze`.
- Mutation guard: scanned fixture directly because no `.enzyme` or `.margins` existed; post-scan check found no generated residue. This probe wrote only under the run case artifact directory.

## Representative Files
- `manifest.json`: transcript dump with expected safe default `notes/meetings/YYYY-MM-DD topic.md`.
- `expected-diagnostic.md`: start capture now; do not convert, rename, or move old transcripts.
- `zoom-export-log.json`: `status: partial`; missing speakers and truncated customer check-in.
- `cache-state.json`: `note_imported_count: 1`, `transcript_imported_count: 4`, `incomplete: true`.
- `import/zoom-manifest.csv`: mixed OK, missing-label, VTT, and partial/truncated entries.
- `notes/2026-06-17-product-review.md`: manual note says transcript summary got messy and asks to keep manual notes next to raw dumps.
- `transcripts/*`: timestamped TXT, duplicate TXT, VTT, and truncated customer-checkin source files.

## Hypotheses
- H1: This is a partial Zoom transcript archive, not a curated meeting-note vault.
- H2: Enzyme scan will underrepresent the workspace because most source material is TXT/VTT/CSV/JSON, not Markdown.
- H3: Margins should use an additive new-note destination under `notes/meetings/`.
- H4: Raw transcripts are useful source material but weak decision/action evidence without selected summaries.
- H5: Cache/import state is incomplete and should not be treated as a reliable ready cache.

## Confirmations / Falsifiers
- Confirmed H1: raw transcript/import files dominate; the only Markdown note describes a messy transcript-summary attempt.
- Confirmed H2: scan returned `files: 1`, `entities: ["folder:notes"]`, sampling only `notes/2026-06-17-product-review.md`.
- Confirmed H3: manifest names `notes/meetings/YYYY-MM-DD topic.md`; no stronger destination convention exists.
- Confirmed H4: transcripts include missing speakers, duplicates, VTT/TXT mismatch, uncertain segments, and truncation.
- Confirmed H5: `cache-state.json` says `incomplete: true`; export log says `status: partial`.
- Falsifiers not found: complete per-meeting Markdown summaries, canonical duplicate map, complete import/cache state, or approved transcript materialization plan.

## Handoff Claims
- Substrate: Enzyme-indexable Markdown is one note in `folder:notes`; broader transcript/import history exists outside scan-visible Markdown and must be preserved as raw source material.
- Failure model: Markdown-only initialization would underrepresent history; transcript speech is noisy decision evidence; duplicate product-review files can create redundant/conflicting retrieval snippets.
- Repair policy: first capture can proceed as-is; minimal repair is additive new notes under `notes/meetings/`; any conversion, dedupe, rename, cache rewrite, or import materialization requires preview, backup, and explicit approval.
- Product translation: Margins should say the user can start now, existing files will not change, new notes go to `notes/meetings/YYYY-MM-DD topic.md`, and transcript import/cache building is a separate later action.
- Baseline status: prior artifacts are evidence-aligned but provisional under v001 because the previous trace lacked explicit case-level hypotheses, confirmations/falsifiers, and handoff claims.
