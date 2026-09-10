# Expected diagnostic

## What Margins should notice
- Most content is mixed admin/personal material: contact data, invoices, receipts, travel, and health notes.
- The only potentially meeting-like artifacts are ad-hoc snippets in `misc/pasted-from-calendar.txt` and `travel/board-meeting-notes.md`.
- There is no stable `meetings/`, `notes/`, or other recurring meeting-notes destination in this workspace.
- Several names and IDs appear (`INV-...`, dates, attendee-style notes), but they are sparse and not backed by a recurring structure.

## What uncertainty to name
- The workspace is not a meeting-note vault and likely mixes multiple unrelated domains.
- Meeting-looking snippets are exceptions, not a workflow signal.

## Safest default to recommend
Treat the workspace as start-fresh for captures and keep it additive:

- `notes/meetings/YYYY-MM-DD call-notes.md`

This creates a clean layer without touching existing mixed-domain artifacts.

## What to avoid
- do not move/rename historical files
- do not infer team or people folders
- do not assume one convention based on filename words like “board” or “meeting”
- do not impose a meeting taxonomy before first capture

## Suggested language
- “You can start now. I’ll add new notes to `notes/meetings` by default and leave everything else untouched.”
- “I can import the old pasted snippets later, but they’re optional because this workspace isn’t currently meeting-centric.”

