# Workspace Diagnostic Fixture Spec

This fixture suite exists to stress-test Margins's first-project workspace
diagnosis before it becomes product UX. The goal is to generate realistic,
spotty folders a user might point Margins at, then run the diagnostic/skill
against them and judge whether Margins gives useful, non-prescriptive guidance.

For the product reasoning behind this suite, including how it fits Margins's
first-run onboarding, import questions, and read-before-write setup boundary,
see `DESIGN_CONTEXT.md`. For the multi-agent run protocol, artifacts, scoring
rubric, and patch/rerun loop, see `LOOP_SPEC.md`.

## Product Question

When a user chooses a folder for Margins, can Margins answer:

> You can start now. Here is the least surprising place for new meeting notes.
> Existing files will not be changed.

The diagnostic should not ask the user to adopt a vault philosophy before their
first capture. It may suggest a safe default, such as one dated note per
conversation, but only as a reversible meeting-note destination.

## Workspace Axes

Each test case should vary along several dimensions:

- **Workspace type:** empty folder, Obsidian vault, codebase with docs, Google
  Drive export, transcript dump, project planning folder, personal vault, team
  knowledge base.
- **Structure level:** flat, light folders, strong conventions, foreign
  non-notes structure.
- **Date convention:** none, filename dates, body dates, frontmatter dates,
  transcript timestamps, mixed/conflicting.
- **Metadata convention:** no frontmatter, YAML frontmatter, inline tags,
  body labels, Google Docs-style headings, codebase status/owner blocks.
- **Meeting material:** none, meeting notes, raw transcripts, AI summaries,
  action-item docs, absent recording references.
- **Entity signals:** no people, names in filenames, people folders,
  wikilinks, email-style participants, project/team names.
- **Markdown quality:** clean Markdown, converted Google Docs Markdown,
  plaintext transcripts, copied Slack/Zoom artifacts, malformed frontmatter.
- **Scale:** tiny, small, medium, large simulated by sample files plus manifest.
- **Margins fit:** ready now, usable but sparse, needs destination choice, import
  would help, start fresh, too ambiguous to infer conventions.

## Canonical Test Cases

The initial suite should contain these cases under `test-cases/`:

1. `fresh-empty`
2. `sparse-random-notes`
3. `append-only-dated-log`
4. `frontmatter-obsidian`
5. `daily-notes-vault`
6. `codebase-plans-folder`
7. `rfc-decision-log`
8. `google-drive-export`
9. `zoom-transcript-dump`
10. `mixed-messy-power-vault`
11. `foreign-domain-vault`
12. `partial-enzyme-or-margins`

Each case should be realistic and imperfect. Avoid polished toy examples.
Include typos, partial conventions, missing dates, duplicated headings, stale
action items, copied transcript artifacts, or contradictory folder practices
where appropriate.

## Test Case Layout

Each test case uses this structure:

```text
test-cases/<case-id>/
  manifest.json
  expected-diagnostic.md
  workspace/
    ... synthetic files and folders ...
```

`manifest.json` should include:

```json
{
  "id": "case-id",
  "title": "Human-readable title",
  "workspace_type": "codebase-with-docs",
  "scale": "small",
  "simulated_total_files": 24,
  "date_conventions": ["filename", "body"],
  "metadata_conventions": ["body-labels"],
  "meeting_material": ["notes", "transcripts"],
  "entity_signals": ["names-in-body"],
  "margins_fit": "usable-but-needs-destination-choice",
  "expected_safe_default": {
    "folder": "docs/meetings",
    "filename": "YYYY-MM-DD meeting-title.md"
  },
  "must_not_suggest": [
    "move existing files",
    "create people folders without evidence"
  ]
}
```

`expected-diagnostic.md` should describe the desired skill output:

- what Margins should notice;
- where uncertainty should be named;
- the safest default destination for new meeting notes;
- what Margins should explicitly avoid doing;
- what language would be receptive to this user.

## Diagnostic Output Contract

The skill or diagnostic should eventually produce structured output equivalent
to:

```json
{
  "workspace_type": "codebase-with-docs",
  "confidence": "medium",
  "detected_patterns": [
    "plans folder",
    "markdown docs",
    "owner/status sections",
    "no consistent meeting-note destination"
  ],
  "safe_default": {
    "folder": "docs/meetings",
    "filename": "YYYY-MM-DD meeting-title.md",
    "reason": "keeps meeting notes near project docs without changing existing plans"
  },
  "do_not_do": [
    "do not create people folders",
    "do not retag existing docs",
    "do not move plan files"
  ],
  "next_actions": [
    "start_capture",
    "choose_destination",
    "build_private_search_cache",
    "import_past_notes"
  ]
}
```

## Judging Rules

A run passes if:

- it correctly identifies the workspace class;
- it names uncertainty when conventions are weak;
- it proposes the smallest useful meeting-note destination;
- it treats messy but active workspaces as usable;
- it distinguishes importing history from starting new captures;
- it avoids moving, renaming, retagging, or restructuring existing files;
- it explains dates/frontmatter as useful defaults, not requirements.

A run fails if:

- it says the user needs an append-only log structure;
- it treats a messy vault as broken;
- it invents people/project folders without evidence;
- it blocks capture until import, cleanup, or indexing;
- it uses mechanism-first language such as petri, catalyst, embedding, schema
  normalization, or vault health;
- it cannot explain where the next meeting note should land.

## Generation Guidance

Synthetic workspaces should be spotty but plausible. Prefer:

- small samples that imply larger scale through `manifest.json`;
- files with realistic partial content instead of lorem ipsum;
- mixed date practices and naming styles;
- actual code/docs context for codebase fixtures;
- prior meeting notes and transcripts that are useful but inconsistent;
- clear source boundaries for imported material.

Do not include secrets, real personal data, API keys, or proprietary source.
