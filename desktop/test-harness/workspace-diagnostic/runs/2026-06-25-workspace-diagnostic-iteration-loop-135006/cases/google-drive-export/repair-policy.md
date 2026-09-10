# google-drive-export repair policy

## 3 Repair Policy Pass
- No-op: start new captures without changing exported files; existing Markdown is enough for a first note destination.
- Minimal reversible: keep old export under MyDrive-Shared as source material and create only new notes under notes/meeting-notes; optionally exclude cache/log/import artifacts from private search cache.
- Medium convention: after approval, materialize selected Drive history into reviewed Markdown summaries while preserving originals.
- Heavy migration: deduplicate Drive copies or convert the export tree only with preview, backup, and explicit approval.

Approval boundary: any move, rename, dedupe, import materialization, or cache-state rewrite requires explicit approval. New capture destination creation is additive.
