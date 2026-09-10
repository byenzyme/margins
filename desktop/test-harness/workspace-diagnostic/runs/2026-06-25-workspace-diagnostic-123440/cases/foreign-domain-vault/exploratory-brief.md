# Exploratory Brief

## Workspace Hypothesis
This is best read as `foreign-domain-vault` with Margins fit `start-fresh`. The user should be able to start capture now.

## Evidence That Supports It
- `travel/board-meeting-notes.md`
- `travel/itinerary-2026-08-dublin.md`
- `finance/tax/receipts-notes.md`
- `health/personal-docs.txt`
- `misc/pasted-from-calendar.txt`
- `README.txt`
- `finance/invoices/INV-2026-05-29.csv`
- `contacts/people.json`

Directory signals considered:
- `  contacts/`
- `  finance/`
- `    invoices/`
- `    tax/`
- `  health/`
- `  misc/`
- `  travel/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed

## Meeting-Note Destination Hypotheses
- Primary: `notes/meetings/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: notes. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- move existing files
- create people folders
- impose people-based taxonomy
- assume this is a meeting notes vault
