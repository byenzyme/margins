# Expected diagnostic

## What Margins should notice
- This is a codebase with mixed planning docs.
- `docs/plans/` exists, but conventions are irregular:
  - `docs/plans/sprint-plan.md` (no date in filename)
  - `docs/plans/weekly-sync.md` (no date, owner/status markers)
  - `docs/plans/retro.md` (no date, minimal notes)
  - `docs/plans/meeting-2026-04-01.md` (dated filename, meeting-ish body notes)
- `docs/notes/incident-playbook.md` has owner/status snippets in body and YAML code block.
- `raw-meeting.txt` contains two-part ad-hoc transcript text with owner/status language but no formal capture structure.

## What uncertainty to name
- Date style is mixed and not enforced.
- There is no single, evidence-backed destination for recurring meeting notes.
- Meeting-like notes are present, but only as raw exports or inline body notes in plan docs.

## Safest default to recommend
Use a new folder that is adjacent to docs but separate from the existing planning files: `docs/meetings`.

- Suggested first capture path: `docs/meetings/YYYY-MM-DD planning-checkin.md`
- Keep it additive and avoid moving/reformatting existing plan files.

## What to avoid
- do not move existing plan docs into a new meeting location
- do not rename old notes to force one format
- do not create people/team folders without clear evidence

## Suggested language
- “You can start now. I’ll add new meeting notes to `docs/meetings` to stay near project docs without changing what’s already here.”
- “I’ll keep `YYYY-MM-DD` dates when I create notes, but I won’t force old ones into that pattern.”

