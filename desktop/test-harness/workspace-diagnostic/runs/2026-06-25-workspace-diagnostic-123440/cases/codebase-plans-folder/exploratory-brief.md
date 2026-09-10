# Exploratory Brief

## Workspace Hypothesis
This is best read as `codebase-with-docs` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `docs/notes/weekly-sync-2026-03-19.md`
- `docs/plans/meeting-2026-04-01.md`
- `docs/notes/incident-playbook.md`
- `docs/notes/meeting-2026-04-01.md`
- `docs/plans/retro-notes-2026-02.txt`
- `docs/plans/weekly-sync.md`
- `docs/notes/sprint-plan.md`
- `docs/plans/retro.md`

Directory signals considered:
- `  docs/`
- `    notes/`
- `    plans/`
- `  src/`
- `    sync/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes

## Meeting-Note Destination Hypotheses
- Primary: `docs/meetings/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes, transcripts, summaries. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- move existing files
- retag existing docs
- create people folders without evidence
