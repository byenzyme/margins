---
name: margins-distill
description: Turn a finished Margins meeting into a grounded draft note in Codex using its final transcript, original timed memo, and relevant declared Workspace Sources.
---

# Distill a finished meeting

Call the Margins plugin's `get_distillation_context` with the user's session ID, or omit it for the latest visible meeting in the configured Workspace. Pin the returned ID for the rest of this request. If `ready` is false, explain whether capture or final transcription is pending; do not present provisional words as a final note. A user who explicitly wants an interim read can use `margins-watermark`.

Use the final transcript and the original memo together. The memo records what the user chose to notice; preserve unusual or untimed memo lines rather than treating them as noise. Distinguish direct quotes and decisions from interpretations. Identify unresolved questions, owners, and follow-ups only when supported by the meeting.

If connections to earlier thinking would improve the note, form one or two focused queries from distinctive memo or transcript language and call `recall_workspace`. Recall searches only declared Sources. Treat returned excerpts as leads; cite their Source reference, and do not claim to have read a whole note when only an excerpt was returned. If recall is unavailable, produce the grounded meeting draft and state that cross-note connections were not checked.

Draft the note in the Codex chat first, with a title, the meeting's main movement, decisions or open questions, follow-ups, and concise source pointers. Use the transcript's channel distinction when it matters. Do not overwrite the original memo. Save to a notes folder only when the user has named an accessible destination and asked for the write; if this Codex host has no file-writing tool for that destination, return the complete draft for review instead of claiming it was saved.
