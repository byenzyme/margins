---
name: connected-note
description: Turn a BB @Meeting from Margins into a connected note in its selected Workspace Home Source.
---

# BB connected note

When a BB message contains `@Meeting` and a `margins-context-v1` block, use the
exact `workspaceId`, `sessionId`, and `memoRevision` from that block. Call
`margins_bb_meeting_read` with `part: context`, then `memo` and `transcript`.
For long content, continue from `nextOffset` until `totalChars` is read. If the
transcript is incomplete, wait for it rather than inventing spoken details.

The BB tool reads the project's Margins service on the correct host. The local
`margins` CLI and the Codex Margins MCP may point at a different store, so do
not use them to fetch the pinned BB meeting.

Use the returned Home Source and destination for note placement. Read nearby
notes through the ordinary project files to find useful links. Write a
grounded note with the meeting's decisions, actions, open questions, and
connections that the evidence supports. Follow any user or project instruction
about whether the note should be saved or drafted in chat.

After writing a file, call `margins_bb_note_link` with the same meeting IDs,
the returned `homeSourceId`, the path relative to `homeRoot`, and
`expectedRevision` from `context.noteAssociation.revision` (or `0` when null).
The tool checks that the file exists within the Home Source. If the user asked
for chat only, give the note in chat and do not link a file that was not written.
