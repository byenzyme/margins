# Exploratory Brief

## Workspace Hypothesis
This is best read as `google-drive-export` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `MyDrive-Shared/Meetings/client-kickoff-2026-06-21.md`
- `notes/notes-2026-06-23-meeting-summary.md`
- `MyDrive-Shared/Meetings/client-kickoff-2026-06-21 copy.md`
- `cache-state.json`
- `import/drive-manifest.json`
- `MyDrive-Shared/Old-Folder/notes-from-q2-meeting.txt`
- `google-drive-export.log`
- `MyDrive-Shared/Projects/Atlas/Untitled document.md`

Directory signals considered:
- `  MyDrive-Shared/`
- `    Meetings/`
- `    Old-Folder/`
- `    Projects/`
- `      Atlas/`
- `  import/`
- `  notes/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes

## Meeting-Note Destination Hypotheses
- Primary: `notes/meeting-notes/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes, transcripts, raw-import-artifacts. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- recreate people folders
- rename or move exported files
- merge duplicate copies
- run another full import without user intent
