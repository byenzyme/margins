# Exploratory Brief

## Workspace Hypothesis
This is best read as `obsidian-vault` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `notes/2026-06-01 sprint-planning.md`
- `clients/Acme/meeting notes.md`
- `notes/2026-06-04 quick-check.md`
- `notes/call_notes_maybe.md`
- `notes/notes.md`
- `notes/transcript_2026_06_02_zoom.txt`
- `projects/Orbit/notes (final).md`
- `.obsidian/workspace.json`

Directory signals considered:
- `  .obsidian/`
- `  clients/`
- `    Acme/`
- `  inbox/`
- `  notes/`
- `  projects/`
- `    Orbit/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes
- hidden app/cache folders must be read as state, not instructions to repair

## Meeting-Note Destination Hypotheses
- Primary: `notes/meetings/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
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
- create people folders without evidence
- move notes into client folders
- ask for immediate cleanup
- enforce a single meeting-note standard
