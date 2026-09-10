# Exploratory Brief

## Workspace Hypothesis
This is best read as `partial-enzyme-or-margins` with Margins fit `needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `notes/Captures/2026-06-08-customer-checkin.md`
- `notes/legacy/2026-05-20-kickoff.md`
- `notes/meeting-notes/2026-06-01 meeting sync.md`
- `.margins/cache-state.json`
- `.margins/import-state.json`
- `.enzyme/cache-state.json`
- `transcripts/2026-06-07-enzyme-migration.txt`
- `cache-state.json`

Directory signals considered:
- `  .margins/`
- `  .enzyme/`
- `  notes/`
- `    Captures/`
- `    legacy/`
- `    meeting-notes/`
- `  transcripts/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes
- hidden app/cache folders must be read as state, not instructions to repair

## Meeting-Note Destination Hypotheses
- Primary: `notes/meeting-notes/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes, transcripts, import-state. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- repair or rewrite existing tool state files
- move notes into `.enzyme` or `.margins` directories
- delete stale cache artifacts
