# google-drive-export failure model

## 2 Failure Model Pass
### Import-state invisibility
- Severity: high
- Confidence: high
- Evidence: Scan excludes the manifest/log/cache files that explain duplicate export churn, so retrieval can over-trust the converted Markdown.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: A completed import manifest with no errors and no duplicate/copy files.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Duplicate meeting evidence
- Severity: medium
- Confidence: high
- Evidence: Two client kickoff notes plus an older loose text file can split actions across near-duplicates.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: User confirms one copy is canonical or duplicate metadata links them.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Raw transcript weak evidence
- Severity: medium
- Confidence: medium
- Evidence: Transcript TXT has timestamps and inaudible fragments but no durable decision/action note.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: A linked summary note with decisions and action owners.
- Do not infer: the workspace is unusable or must be reorganized before capture.
