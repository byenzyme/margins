# Product Critic: zoom-transcript-dump

## Strongest thing the diagnostic did
The single most important behavior for a transcript dump is present: do-not-do
"import all transcripts as finished notes." That is the exact failure the
rubric and expected diagnostic warn against — treating raw transcripts as
completed meeting notes. Destination `notes/meetings/` matches expected.

## Most serious trust risk
Same shape as the Drive case: the import story is generic where it should be
specific.
1. "Hidden/cache state: no hidden app/cache folders visible" while
   `cache-state.json` and `import/zoom-manifest.csv` are present and listed as
   evidence; `zoom-export-log.json` reports a partial/incomplete import.
2. The VTT/TXT duplicate of the same meeting and the truncated check-in
   transcript (named in the expected diagnostic) are not surfaced.
3. detected_patterns contains literal junk tokens ("none", "mixed") echoed from
   the manifest — a tell that classification is manifest-derived rather than
   scan-derived.

## Would the user feel ready to start capture?
Yes, and crucially without first processing the transcript backlog.

## Is import/history understood as optional?
Yes, and the "transcripts are not finished notes" distinction is the right one.

## Surface / patch implicated
skill (primary): name the partial-import state and the duplicate transcript
formats in plain language; stop emitting raw manifest tokens as patterns; fix
visible-cache detection.
