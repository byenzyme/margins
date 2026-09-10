# append-only-dated-log repair policy

## 3 Repair Policy Pass
- No-op: keep using meeting-log as-is; this is ready now.
- Minimal reversible: create each new note as meeting-log/YYYY-MM-DD topic.md.
- Medium convention: optionally fix malformed frontmatter or duplicate notes after user asks.
- Heavy migration: not recommended; it would add risk without material retrieval benefit.

Approval boundary: moving, renaming, deleting, deduping, rewriting cache state, or materializing imports requires explicit approval.
