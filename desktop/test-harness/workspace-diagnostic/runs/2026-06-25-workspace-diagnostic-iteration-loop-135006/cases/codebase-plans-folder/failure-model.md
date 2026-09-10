# codebase-plans-folder failure model

## 2 Failure Model Pass
### Codebase corpus dominance
- Severity: medium
- Confidence: high
- Evidence: Source/package files and planning docs can make Margins look like it should diagnose a repo instead of choosing a meeting-note destination.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: A dedicated meeting folder or config separates docs from code/build surfaces.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Meeting convention split
- Severity: medium
- Confidence: high
- Evidence: Meeting-like content appears in docs/plans, docs/notes, README transcript, and raw-meeting.txt.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: Recurring notes consistently land in one folder.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Scan undercoverage
- Severity: medium
- Confidence: medium
- Evidence: The scan saw only README, so using scan alone would miss docs/plans and docs/notes evidence.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: A scan/config path includes the docs folders or manual audit confirms README is intentionally canonical.
- Do not infer: the workspace is unusable or must be reorganized before capture.
