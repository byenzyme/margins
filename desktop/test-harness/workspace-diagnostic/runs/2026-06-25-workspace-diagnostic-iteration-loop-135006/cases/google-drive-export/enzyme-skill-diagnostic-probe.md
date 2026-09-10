# google-drive-export Enzyme skill diagnostic probe

## Skill source loaded

- Upstream Enzyme skill: `../enzyme-rust/plugin/agent/SKILL.md`
- Loop context: `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`
- Active revision: `desktop/test-harness/workspace-diagnostic/runs/2026-06-25-workspace-diagnostic-iteration-loop-135006/skill-revisions/v001-case-level-enzyme-probes.md`

## Command

```bash
enzyme scan -p desktop/test-harness/workspace-diagnostic/test-cases/google-drive-export/workspace
```

Mutation guard: pre/post checks found no `.enzyme` or `.margins` residue. No `--write-config`, `init`, `refresh`, `apply`, `petri`, or `catalyze` ran.

## Representative files read

- `manifest.json`
- `expected-diagnostic.md`
- `google-drive-export.log`
- `import/drive-manifest.json`
- `cache-state.json`
- `MyDrive-Shared/Meetings/client-kickoff-2026-06-21.md`
- `MyDrive-Shared/Meetings/client-kickoff-2026-06-21 copy.md`
- `MyDrive-Shared/Projects/Atlas/roadmap-2026-06-20.md`
- `MyDrive-Shared/Projects/Atlas/Untitled document.md`
- `MyDrive-Shared/Projects/Atlas/zoom-transcript-client-call.txt`
- `notes/notes-2026-06-23-meeting-summary.md`

## Hypotheses

- This is a Google Drive export with a small local `notes/` area, not a curated vault.
- Enzyme can index some converted Markdown, but scan alone misses important setup evidence in `.txt`, `.json`, and `.log` files.
- `notes/meeting-notes/YYYY-MM-DD meeting-title.md` is the safest additive default for new Margins captures.
- `MyDrive-Shared/` should be treated as preserved import/source material, not the canonical active meeting-note home.
- Cleanup should be optional and approval-gated; first capture should not require dedupe, moves, renames, people folders, or a new import.

## Confirmations / falsifiers

- Confirmed: scan reported 5 Markdown files and entities `folder:mydrive-shared`, `folder:notes`, `#atlas`, `#roadmap`.
- Confirmed: export log and manifest record duplicate title/filename handling, malformed heading conversion, and an empty file.
- Confirmed: `cache-state.json` is partial and reports parse errors.
- Confirmed: meeting material is split across `MyDrive-Shared/Meetings`, `MyDrive-Shared/Projects/Atlas`, and `notes/`.
- Confirmed: raw transcript text is useful source material but weak decision evidence without a summary.
- Falsifier: a clean completed import manifest, no duplicate/copy files, and reviewed Markdown summaries would reduce the import-risk claim.
- Falsifier: an explicit user preference or existing config naming another active capture folder would weaken the `notes/meeting-notes/` default.

## Handoff claims

- Substrate: include both indexed Markdown and non-indexed import evidence; otherwise the workspace looks cleaner than it is.
- Failure model: primary risks are import-state invisibility, duplicate meeting evidence, malformed conversion/frontmatter, and raw transcript weak evidence.
- Repair policy: no-op is acceptable for first capture; minimal repair is additive capture under `notes/meeting-notes/`; medium/heavy cleanup requires preview, backup, and explicit approval.
- Product translation: lead with "You can start now"; say existing Drive export files stay untouched; frame old Drive notes/transcripts as optional later context.
- Baseline status: prior v000 artifacts are aligned but provisional under v001 because the old trace was scan/snapshot-oriented rather than a case-level Enzyme probe.
