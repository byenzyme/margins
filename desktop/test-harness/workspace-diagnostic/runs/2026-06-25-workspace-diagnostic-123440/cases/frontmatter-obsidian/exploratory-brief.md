# Exploratory Brief

## Workspace Hypothesis
This is best read as `obsidian-vault` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `notes/archive/Client Sync - 2026-06-12 archive copy.md`
- `notes/meetings/2026-06-10-standup.md`
- `notes/meetings/2026-06-15-random-meeting-note.md`
- `notes/meetings/Client Sync - 2026-06-12.md`
- `notes/meetings/Client_sync_2026-06-13_followup.md`
- `imports/zoom/2026-06-15_zoom-transcript.txt`
- `notes/meetings/client-kickoff.md`
- `notes/projects/phoenix/notes-archive/meeting-notes.md`

Directory signals considered:
- `  .obsidian/`
- `  imports/`
- `    zoom/`
- `  notes/`
- `    archive/`
- `    meetings/`
- `    projects/`
- `      phoenix/`

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
- move existing files
- retag existing notes
- create people folders without evidence
- normalize all frontmatter before first capture
