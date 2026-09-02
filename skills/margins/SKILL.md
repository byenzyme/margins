---
name: margins
description: Turn the latest Margins session, a selected session, or supplied evidence into a reviewed Markdown note connected to the user's existing notes.
argument-hint: "[latest|<session-id>|<transcript-or-memo-path>] [--audio <audio-file>] [--memo <memo-file>]"
user-invocable: true
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion
---

# Margins — Connected Note

Start with the latest Margins session unless the user selects another session or
supplies evidence directly. Turn that evidence into a useful note, consult
existing Workspace notes when they can improve it, and save the approved result
in the Workspace home. The job is complete when the user approves the draft and
the note is saved.

## Inputs and output

`latest` is the normal starting point. A stable session id selects a different
Margins session. A transcript or memo path, text supplied in the conversation,
or an audio file is an explicit override when the user wants to work from
something else. Audio is available when the capability report in step 1 lists
`audio` under `distillation.inputs`.

The output is a reviewed Markdown note in the Workspace `home` folder. Machine
state and imported-audio artifacts stay under the Workspace `state_dir`; never
create `.margins` inside a notes Source.

## 1. Find Margins and check the Workspace

Resolve whichever Margins command is available, then use that command
throughout the workflow:

```bash
MARGINS_CLI="$(command -v margins || command -v margins-public)"
"$MARGINS_CLI" capabilities
"$MARGINS_CLI" workspace status --json
"$MARGINS_CLI" sync --json
```

If neither command is found, stop and ask the user to install Margins.
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
If no session exists and the user has not supplied other evidence, ask for a
transcript, memo, text, or supported audio file.

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

Use the returned meeting id with the `transcript ... --format json` command
above so imported audio follows the same evidence path as any other session. If
audio is not a reported input, ask for a transcript instead.

## 3. Retrieve useful prior context

Form focused queries from consequential, distinctive language in the input.
Usually one to four searches are enough:

```bash
"$MARGINS_CLI" --workspace "<workspace-id>" recall "<specific query>"
```

`recall` returns matching excerpts with their note paths. Read the best three to
five referenced notes when they could change the draft. Treat those matches as
supporting evidence; the transcript or memo remains the primary factual record.

Use prior notes to confirm vocabulary, tags, people, and wikilinks. Do not
invent a wikilink target or copy unrelated recall results into the note.

## 4. Draft the note

Read `skills/margins/distillation-core.md` and apply its evidence priority,
attribution, action-item, provenance, and writing rules. Choose the closest
template from `skills/margins/templates/`; adapt its headings when the evidence
requires it.

Produce a complete Markdown draft. Keep factual claims grounded in the input,
separate settled decisions from open questions, and include only connections
supported by recall evidence.

## 5. Review and write

Show the draft and ask for approval or changes. Once approved, write it to the
Workspace `home` folder using the existing naming conventions there. If no
convention is evident, use a timestamp plus a specific 3–7 word title.

Never write the finished note under `state_dir`, `captures`, or `.margins`.
Report the saved path, the input evidence used, and the prior notes that
materially informed the result.
