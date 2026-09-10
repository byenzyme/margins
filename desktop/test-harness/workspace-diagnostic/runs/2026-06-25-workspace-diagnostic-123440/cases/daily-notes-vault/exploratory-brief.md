# Exploratory Brief

## Workspace Hypothesis
This is best read as `obsidian-vault` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `Daily/2026-06-09.md`
- `Daily/2026-06-10.md`
- `Daily/2026-06-13.md`
- `Daily/2026-06-14.md`
- `Daily/Daily Note - 6-11-2026.md`
- `meetings/2026-06-11 product sync.md`
- `.obsidian/daily-notes.json`
- `Templates/Daily Template.md`

Directory signals considered:
- `  .obsidian/`
- `  Daily/`
- `  People/`
- `  Projects/`
- `    Atlas/`
- `  Templates/`
- `  meetings/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- hidden app/cache folders must be read as state, not instructions to repair

## Meeting-Note Destination Hypotheses
- Primary: `Daily/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes, summaries. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- move existing files
- force a meeting folder that doesn't exist
- rebuild all daily notes format first
