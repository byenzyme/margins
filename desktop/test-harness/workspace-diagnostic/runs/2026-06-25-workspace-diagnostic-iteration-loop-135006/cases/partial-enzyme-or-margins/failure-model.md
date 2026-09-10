# partial-enzyme-or-margins failure model

## 2 Failure Model Pass
### Stale tool-state confusion
- Severity: high
- Confidence: high
- Evidence: Multiple .enzyme/.margins files disagree about destination and cache status, so cache setup can mistake stale state for canonical structure.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: User confirms which tool state is authoritative or fresh setup ignores stale state.
- Do not infer: capture should be blocked.
### Destination casing/path drift
- Severity: medium
- Confidence: high
- Evidence: notes/captures, notes/Captures, and notes/meeting-notes all appear.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: A current app config or user choice selects one destination.
- Do not infer: capture should be blocked.
### Raw transcript/import leftovers
- Severity: medium
- Confidence: medium
- Evidence: Transcripts are present but scan excludes them from note entities.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: Reviewed import plan or summaries link transcripts to meeting notes.
- Do not infer: capture should be blocked.
