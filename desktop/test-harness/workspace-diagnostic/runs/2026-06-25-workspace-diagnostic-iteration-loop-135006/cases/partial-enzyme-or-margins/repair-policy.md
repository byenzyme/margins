# partial-enzyme-or-margins repair policy

## 3 Repair Policy Pass
- No-op: start new captures in notes/meeting-notes and leave stale tool state untouched.
- Minimal reversible: scan via temp copy or guard, then ignore .enzyme/.margins as runtime state unless user approves repair.
- Medium convention: after a successful new capture, reconcile Margins destination config with notes/meeting-notes.
- Heavy migration: repair or delete stale cache/import state only with backup, preview, and explicit approval.

Approval boundary: moving, renaming, deleting, deduping, rewriting cache state, or materializing imports requires explicit approval.
