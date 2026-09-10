# Exploratory Brief

## Workspace Hypothesis
This is best read as `personal-vault` with Margins fit `ready-now`. The user should be able to start capture now.

## Evidence That Supports It
- `meeting-log/2026-06-14.md`
- `meeting-log/2026-06-16.md`
- `meeting-log/2026-06-17.md`
- `meeting-log/append-only.md`
- `meeting-log/old-log.txt`
- `meeting-log/transcript-2026-06-16-zoom.txt`
- `meeting-log/2026-06-14 copy.md`
- `meeting-log/2026-06-15.md`

Directory signals considered:
- `  meeting-log/`
- `  plans/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes

## Meeting-Note Destination Hypotheses
- Primary: `meeting-log/YYYY-MM-DD topic.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes, transcripts. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- restructure append-only logs
- rename legacy log files
- delete duplicates before asking user
- create a different top-level scheme
