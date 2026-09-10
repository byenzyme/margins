# append-only-dated-log failure model

## 2 Failure Model Pass
### No material setup failure
- Severity: low
- Confidence: high
- Evidence: meeting-log is already a strong destination and scan agrees.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: Future captures fail to land there or user chooses a different folder.
- Do not infer: capture should be blocked.
### Duplicate/history drift
- Severity: low
- Confidence: medium
- Evidence: 2026-06-14 copy and transcript snippets can duplicate context.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: User wants dedupe or marks one canonical.
- Do not infer: capture should be blocked.
### Malformed metadata
- Severity: low
- Confidence: medium
- Evidence: One frontmatter block appears unclosed; retrieval still has filename/date/folder signal.
- Retrieval impact: may weaken or confuse setup/search unless handled in policy.
- Falsifier: Parser accepts it or note is repaired after approval.
- Do not infer: capture should be blocked.
