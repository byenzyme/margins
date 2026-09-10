# Exploratory Brief

## Workspace Hypothesis
This is best read as `zoom-transcript-dump` with Margins fit `usable-but-needs-destination-choice`. The user should be able to start capture now.

## Evidence That Supports It
- `notes/2026-06-17-product-review.md`
- `cache-state.json`
- `import/zoom-manifest.csv`
- `zoom-export-log.json`
- `transcripts/06-20 product review (copy).txt`
- `transcripts/06-20-product-review.vtt`
- `transcripts/2026-06-17_q2-planning.txt`
- `transcripts/2026-06-22_customer_checkin.txt`

Directory signals considered:
- `  import/`
- `  notes/`
- `  transcripts/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes

## Meeting-Note Destination Hypotheses
- Primary: `notes/meetings/YYYY-MM-DD topic.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: transcripts, notes, raw-import-artifacts. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- import all transcripts as finished notes
- rename old transcript files
- move existing files before first capture
