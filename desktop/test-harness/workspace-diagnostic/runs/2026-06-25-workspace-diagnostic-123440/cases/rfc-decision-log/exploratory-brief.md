# Exploratory Brief

## Workspace Hypothesis
This is best read as `codebase-with-docs` with Margins fit `ready-now`. The user should be able to start capture now.

## Evidence That Supports It
- `notes/2026-03-11-architecture-review.md`
- `decisions/DEC-026-security-thread.md`
- `rfc/0001-api-contract.md`
- `rfc/0002-retry-policy.md`
- `rfc/rfc-appendix.txt`
- `notes/meeting-sync-jan.md`
- `notes/raw-thread.txt`
- `rfc/ADR-0002-api-limits.md`

Directory signals considered:
- `  decisions/`
- `  notes/`
- `  rfc/`
- `  src/`

## Evidence That Complicates It
- multiple date signals are present
- metadata is mixed
- raw transcripts are not the same thing as finished notes

## Meeting-Note Destination Hypotheses
- Primary: `docs/meeting-notes/YYYY-MM-DD meeting-title.md` because it is the manifest safe default and matches the strongest low-risk path.
- Alternative: keep using an existing obvious meeting folder only if the user explicitly chooses it.
- Rejected: reorganizing old material to make a cleaner taxonomy before first capture.

## Import/Transcript History
Meeting material signals: summaries, notes, transcripts. Treat old notes, transcripts, cache files, and import manifests as optional history rather than setup blockers.

## Questions A First-Time User Might Ask
- Can I start without importing or indexing everything?
- Will Margins change or move anything already here?
- Where exactly will the next meeting note go?
- Can old notes or transcripts help later?

## What The Diagnostic Must Not Infer
- move existing files
- rename RFC names
- create people folders without evidence
- merge decision log and meeting notes automatically
