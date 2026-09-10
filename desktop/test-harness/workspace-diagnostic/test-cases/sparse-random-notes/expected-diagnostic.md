# Expected diagnostic for sparse-random-notes

## What Margins should notice
- Existing notes are real but inconsistent: mixed formats, partial headings, and mixed date conventions.
- Meeting-style material appears across `notes/`, `projects/Orbit/`, and `clients/Acme/`, with transcript text in plain-text form and lightweight inbox drops.
- There are project signals (`Orbit`, `Acme`) and attendee names, but no single, trusted destination.

## Uncertainty to surface
- Conventions look personal rather than canonical.
- Mention that meeting notes are currently “mixed by habit,” so guidance should stay conservative.

## Safe default recommendation
- Recommend a minimal `notes/meetings` destination to reduce risk.
- Use `YYYY-MM-DD meeting-title.md` names for new notes.
- Keep additions append-only there until the user explicitly picks another structure.

## What to avoid
- Avoid asking the user to migrate or import everything first.
- Avoid creating people/project directories due to sparse evidence.
- Avoid suggesting renames for existing legacy files.

## Suggested tone
- Receptive and light: “There are enough clues to start here now, but we should keep this non-destructive and reversible.”
