# zoom-transcript-dump failure model

## 2 Failure Model Pass
### Transcript pile invisible to setup scan
- Severity: high
- Confidence: high
- Evidence: The main corpus is transcript/source files, but scan exposes only one summary note, so initialization as-is would underrepresent history.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: Transcripts are converted or configured as supported source targets with reviewed summaries.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Raw speech is weak decision evidence
- Severity: medium
- Confidence: high
- Evidence: Timestamps, missing speakers, VTT blocks, and truncation make retrieval noisy for decisions/actions.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: A per-meeting summary exists for each transcript with date/topic/action owners.
- Do not infer: the workspace is unusable or must be reorganized before capture.
### Duplicate format collision
- Severity: medium
- Confidence: medium
- Evidence: 06-20 appears as TXT, copy TXT, and VTT, so retrieval can surface redundant or conflicting snippets.
- Retrieval impact: weak or misleading retrieval handles for setup and later meeting context.
- Falsifier: Manifest marks canonical file or duplicates are explicitly linked.
- Do not infer: the workspace is unusable or must be reorganized before capture.
