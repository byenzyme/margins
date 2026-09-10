# zoom-transcript-dump repair policy

## 3 Repair Policy Pass
- No-op: start capture now in notes/meetings and leave transcripts alone.
- Minimal reversible: keep transcripts as source boundary and create new meeting notes next to notes/; optionally add a manifest note that points to raw files after approval.
- Medium convention: convert only selected transcripts into dated meeting summaries, preserving raw files.
- Heavy migration: normalize the transcript archive, dedupe formats, and repair speaker labels only with preview/backups.

Approval boundary: any move, rename, dedupe, import materialization, or cache-state rewrite requires explicit approval. New capture destination creation is additive.
