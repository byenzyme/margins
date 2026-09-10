# Expected Diagnostic

## What Margins should notice
- A mixed Obsidian vault with both strict and broken YAML; some meeting notes have `tags`, while others have partial frontmatter or malformed keys.
- Existing meeting material is split between `notes/meetings/`, `notes/archive/`, and transcript imports in `imports/`.
- `notes/inbox.md` and `notes/projects/` contain ad hoc links and one-off naming like `Client Sync - 2026-06-12.md`, so conventions remain unstable.

## Uncertainty to call out
- Date consistency is weak: filenames, frontmatter dates, and sentence timestamps do not always match.
- There is enough evidence of existing meeting capture, but no single canonical folder or template is used.

## Safest default recommendation
- Use `notes/meetings/` for new meeting notes with filename format `YYYY-MM-DD meeting-title.md`.
- Do not try to migrate or rewrite old files first; just place one new note and make it easy to find later.

## What to avoid
- Avoid asking for cleanup before capture can start.
- Avoid creating new people/project buckets without evidence from multiple notes.
- Avoid “fixing” malformed frontmatter in existing files.

## Tone for user
- “You can start now. I’ll save new notes in `notes/meetings/` and only read that one spot unless you tell me otherwise.”
