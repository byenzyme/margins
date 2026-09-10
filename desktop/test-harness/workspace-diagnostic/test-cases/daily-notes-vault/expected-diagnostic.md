# Expected Diagnostic

## What Margins should notice
- The workspace is dominated by `Daily/` notes with varying formats: `YYYY-MM-DD.md`, `Daily Note - 6-12-2026.md`, and occasional missing headings.
- A few meeting summaries exist under `meetings/`, copied snippets, and one embedded meeting section in daily notes, but they don’t imply a separate meeting-system folder.
- The vault has a lot of day-specific work context (standups, reminders, logs), so note capture can piggyback on that behavior.

## Uncertainty to call out
- Date conventions are inconsistent, with shorthand filenames and body-only headings.
- Some files reference meeting participants without a consistent schema (`from:`, `attendees:`, ad-hoc tags).
- Some notes are not meetings, so destination should avoid a strict “meeting-only” interpretation of the whole vault.

## Safest default recommendation
- Capture new notes in `Daily/` and suggest filename `YYYY-MM-DD` with a short meeting title in the header/body as needed.
- Keep it reversible: this is a non-breaking addition to an existing daily workflow.

## What to avoid
- Avoid asking user to create a brand-new meeting folder.
- Avoid suggesting cleanup of legacy daily notes before capture.
- Avoid treating every `Daily` file as a meeting artifact.
- Avoid inferring a strict people/project taxonomy from one-off links in `Daily/2026-06-14.md`.

## Tone for user
- “You’re fine to start now. I can keep new meeting notes under `Daily/` using the same date-based rhythm.”
