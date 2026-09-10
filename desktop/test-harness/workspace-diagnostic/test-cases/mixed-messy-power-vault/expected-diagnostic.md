# Expected Diagnostic

## What Margins should notice
- Workspace mixes strategy docs, meeting notes, pasted chat logs, and raw transcripts in separate roots (`notes/`, `meetings/`, `transcripts/`, `exports/`).
- Naming is inconsistent: some files are dated, some are prefixed by project name, and several have duplicated headings or stale “DONE” checkboxes.
- Metadata is inconsistent: there are some YAML frontmatter headers, some body labels, and a few malformed snippets.
- Enough historical capture exists to infer meeting context, but not enough structure for strict automation.
- There is also a stale import manifest (`imports/legacy-capture-index.md`) that references legacy capture folders, confirming mixed source history.

## Uncertainty to call out
- There is no single canonical meeting destination and multiple folders could be “closest match”.
- Some files are historical exports and should be treated as historical references, not current canonical sources.

## Safest default recommendation
- Place new notes in `notes/captures/` with `YYYY-MM-DD meeting-title.md`.
- Preserve existing folder behaviors and treat legacy import manifests as history; prioritize current activity folders when suggesting a capture destination.

## What to avoid
- Avoid creating deep people/project hierarchies from weak signals.
- Avoid treating `imports/legacy-capture-index.md` as a current capture rule.
- Avoid deleting/renaming legacy exports.
- Avoid declaring the vault unusable due to mixed quality.

## Tone for user
- “You’re good to start now. I’ll keep new notes in `notes/captures/` to avoid stepping on your existing archive folders.”
