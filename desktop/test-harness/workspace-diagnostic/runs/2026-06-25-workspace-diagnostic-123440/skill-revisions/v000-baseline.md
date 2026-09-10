# v000 Baseline Workspace Diagnostic Skill

Builds on: none.

Source context:

- `desktop/test-harness/workspace-diagnostic/DESIGN_CONTEXT.md`
- `desktop/test-harness/workspace-diagnostic/SPEC.md`
- `desktop/test-harness/workspace-diagnostic/ENZYME_EVALUATOR_CONTEXT.md`
- current Margins distillation skills, which do not include a dedicated workspace
  diagnostic skill

## Diagnostic Contract

For each workspace, produce a first-project Margins diagnosis that answers:

- what Margins noticed;
- what is uncertain;
- where the next meeting note should go;
- whether existing files will change;
- whether capture can start now;
- whether import/history/cache setup could help later.

## Enzyme Lens

Use `enzyme scan --vault <workspace>` without `--write-config` as the primary
read-only Enzyme substrate. Treat it as indexability evidence:

- already useful signal: folders, dates, Markdown files, transcripts, project
  docs, tags, wikilinks, frontmatter, and source text;
- weak signal: raw exports, duplicate transcripts, missing dates, ambiguous
  folders, generated/cache directories;
- optional later work: private search cache, import/materialization, or small
  retrieval repairs after preview and approval.

Do not run `enzyme init`, `enzyme refresh`, `enzyme apply`, `petri`, or
`catalyze` against the fixture during the read-only loop when doing so would
write files, require initialization, or spend model resources.

## User-Facing Stance

Use this promise as the center:

> You can start now. Here is the least surprising place for new meeting notes.
> Existing files will not be changed.

Prefer:

- "Margins can start saving new captures here now."
- "Existing files stay as they are."
- "The private search cache is a separate action; it does not rewrite notes."
- "Importing old notes or transcripts can help later, but it is not required."

Avoid:

- vault-health scoring;
- schema normalization language;
- moving, renaming, retagging, or rewriting old material;
- creating people/project folders without strong existing convention;
- implying Enzyme initialization, import, or cleanup blocks first capture.

## Safe Default Rule

Choose the least surprising next-note destination from existing evidence:

- existing meeting folder when clearly present;
- `docs/meetings` or `docs/meeting-notes` for code/docs repos;
- `notes/meetings` or `notes/meeting-notes` for loose note folders;
- `meeting-notes` for empty folders;
- an isolated meeting folder for foreign-domain folders;
- preserve daily-note destinations only when the case evidence supports using
  daily notes for captures.

The safe default is a future capture destination, not a migration instruction.
