---
name: margins
description: Turn the latest Margins session, a selected session, or supplied evidence into a reviewed Markdown note connected to the user's existing notes.
argument-hint: "[latest|<session-id>|<transcript-or-memo-path>] [--audio <audio-file>] [--memo <memo-file>]"
user-invocable: true
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion, margins_bb_meeting_read, margins_bb_note_link
---

# Margins — Connected Note

Start with the latest Margins session unless the user selects another session or
supplies evidence directly. Turn that evidence into a useful note, consult
existing Workspace notes when they can improve it, and save the approved result
in the Workspace Home destination. A pinned BB `@Meeting` is an explicit
selection; use the BB tools described below for that session. The job is
complete when the user approves the draft and the note is saved.

## Inputs and output

`latest` is the normal starting point. A stable session id selects a different
Margins session. A transcript or memo path, text supplied in the conversation,
or an audio file is an explicit override when the user wants to work from
something else. Audio is available when the capability report in step 1 lists
`audio` under `distillation.inputs`.

The output is a reviewed Markdown note in the Workspace Home Source, under its
reviewed `note_folder` when one is configured. Machine state and imported-audio
artifacts stay under the Workspace `state_dir`; never create `.margins` inside a
notes Source.

## BB `@Meeting` handoff

When a BB message carries `<margins-context-v1>`, use its exact `workspaceId`,
`sessionId`, and `memoRevision`. The meeting mention pins that session; do not
resolve `latest` or query a local CLI, which may point at another store. Read
`context`, `memo`, and `transcript` with `margins_bb_meeting_read`, continuing
from `nextOffset` until each part is complete. Use the returned Home Source and
destination for the note. If the transcript is pending, wait for the server's
transcript. A memo-only checkpoint has no usable spoken timeline. Use a full
transcript only when it is terminal; do not write a final note from an active
live meeting.

After saving an approved note under the returned Home Source, call
`margins_bb_note_link` with the same pinned IDs and memo revision, the returned
`homeSourceId`, the path relative to `homeRoot`, and the association's current
revision (or `0` when absent). This records the reference, not the note body.
If the user requested a draft in chat only, do not link a file. Follow the BB
`connected-note` skill when it is available for the full host procedure. For
this BB route, use the pinned tool evidence and ordinary Home Source files for
context; skip the local CLI resolution, transcript, recall, and destination
commands below.

## 1. Find Margins and check the Workspace

Resolve whichever Margins command is available, then use that command
throughout the workflow:

```bash
MARGINS_CLI="${MARGINS_CLI_BIN:-$(command -v margins || command -v margins-public)}"
"$MARGINS_CLI" capabilities
"$MARGINS_CLI" workspace status --json
"$MARGINS_CLI" sync --json
```

If no command is available, stop and ask the user to install Margins. A supplied
`MARGINS_CLI_BIN` is the absolute executable for this workflow; keep using it.
Read the Workspace id, `home`, and `state_dir` from status. If no Workspace is
ready, follow `"$MARGINS_CLI" guide workspace-setup`, then return here after a
recall query succeeds. If `sync` reports an unavailable Source, tell the user
which Source failed and continue only when the remaining evidence is enough to
ground the note. Do not modify Margins state directly.

## 2. Resolve the session evidence

Resolve `latest` directly; do not list every session first. Replace `latest`
with a stable session id only when the user selects another session:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" transcript latest --format json
```

Use the returned `body` as the factual record. Read `memo_path` when it is
present; the memo records what the user noticed or cared about. Preserve speaker
names, timestamps, decisions, risks, and action items. If `saved_note_path`
already names a note, show it to the user before replacing or duplicating it.

For a TUI session started with `margins new`, the memo contains timestamped
lines captured while the user listened. Preserve the raw memo, including later
untimed reflections; do not rewrite it as a finished note. Capture may still be
finishing transcription after the TUI closes. For `view="pending"`, wait for
the server transcript. Use a `view="full"` body only when `terminal="true"`;
a terminal `*.live-transcript.json` checkpoint is usable without re-running ASR.
For other non-live results with `incomplete="true"` or `terminal="false"`,
run `process` once, then read the transcript again. If it remains incomplete,
disclose the gap and do not present its partial body as complete. Do not process
an active live meeting for a final note. Memo-only bodies without spoken timeline
lines are not usable transcripts. If speaker labels are generic, ask for
identities before attributing claims to named people.

If the transcript command has no usable body, ask Margins which files belong to
the same session. Use the same `latest` or stable session id as above:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" artifacts latest
```

Run `recent` only when the user needs to browse or disambiguate sessions:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" recent
```

Use these commands rather than searching hidden Margins files for artifacts.
If a registered recording has no usable transcript after capture finishes and
`capabilities` reports audio processing, run `"$MARGINS_CLI" --workspace
"<workspace-id>" process "<session-id>"` once, then read the transcript again.
Use `--align-only` only when the user asks to rebuild alignment from an existing
transcript. If no session exists and the user has not supplied other evidence,
ask for a transcript, memo, text, or supported audio file.

### Explicitly supplied evidence

For a transcript or memo path, read the file directly. If both are present, the
transcript remains the primary factual record and the memo is the user's
attention signal. Text supplied in the conversation can be used directly.

For audio, first confirm that `distillation.inputs` includes `audio`. Keep
generated artifacts in Workspace state by using the explicit Workspace id:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" transcribe \
  "/absolute/path/to/audio" --name "<stable-name>" \
  [--memo "/absolute/path/to/memo.md"] [--speakers N]
```

Use the returned meeting id instead of `latest` in the transcript command above
so imported audio follows the same evidence path without selecting a different
session. If audio is not a reported input, ask for a transcript instead.

## 3. Retrieve useful prior context

Form focused queries from consequential, distinctive language in the input.
Usually one to four searches are enough:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" recall --json "<specific query>"
```

`recall` returns matching excerpts with their note paths. Read the best three to
five referenced notes when they could change the draft. Treat those matches as
supporting evidence; the transcript or memo remains the primary factual record.

Use prior notes to confirm vocabulary, tags, people, and wikilinks. Do not
invent a wikilink target or copy unrelated recall results into the note.

## 4. Draft the note

Read `distillation-core.md` next to this skill and apply its evidence priority,
attribution, action-item, provenance, and writing rules. Choose the closest
file from the adjacent `templates/` directory; adapt its headings when the
evidence requires it. The CLI installs both alongside this `SKILL.md`.

Produce a complete Markdown draft. Keep factual claims grounded in the input,
separate settled decisions from open questions, and include only connections
supported by recall evidence.

## 5. Review and write

Show the draft and ask for approval or changes. Once approved, read
`"$MARGINS_CLI" --workspace "<workspace-id>" workspace destination --json`.
Write under its `destination`, which is the Home Source plus any reviewed
`note_folder`. Use existing naming conventions there. If none is evident, use
a timestamp plus a specific 3–7 word title. If `saved_note_path` already names
a note, read it first and preserve the user's edits; revise the reviewed note
there rather than creating a duplicate.

Never write the finished note under `state_dir`, `captures`, or `.margins`.
Report the saved path, the input evidence used, and the prior notes that
materially informed the result.
