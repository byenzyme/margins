---
name: margins
description: End-to-end margins session processing — transcribe, align memo + transcript, distill into a structured vault note connected to existing thinking.
argument-hint: <session-name> [--align-only] [--audio <audio-file> [--speakers N]]
user-invocable: true
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion
---

# Margins — Capture to Vault Note

Take an margins session end-to-end: transcribe audio, align the transcript with the user's real-time memo, distill into a structured vault note connected to existing thinking.

**Environment**: `$OBSIDIAN_VAULT` refers to the Obsidian vault root (the additional working directory configured for this project).

## Prerequisites

- Standalone public Rust CLI from `crates/public/margins-cli` in `$PATH`, built
  with the ASR feature for the platform (for example,
  `cargo install --path crates/public/margins-cli --features coreml-asr`)
- For multi-speaker audio, build Margins with `polyvoice-diarization` (the default release build includes it)

## What this produces

1. **Aligned timeline** (`.margins/<session>_aligned.md`) — interleaved transcript + memo on a shared timeline
2. **Vault note draft or approved final note** — structured note written to the obsidian vault using a template, with recall-sourced connections

If `--align-only` is passed, only the aligned timeline (step 1) is produced.

## Agent delegation policy

For sessions that require transcription or long audio processing, prefer splitting the work into two stages instead of asking one background agent to transcribe, align, synthesize, and overwrite the final note in a single pass.

### Stage A — mechanical processing agent

A background agent may handle the mechanical work:

- resolve the correct memo, metadata, audio, transcript, and existing aligned timeline
- transcribe audio when needed
- create or correct metadata when the input is an external audio file or imported note
- preserve the original memo as `.margins/<session>_memo.md` before any note rewrite or rename
- align against the preserved raw memo, not against a distilled final note
- report transcript quality, alignment counts, files written, and risks

The mechanical agent should **not overwrite the vault note with final synthesis** unless the user explicitly asks for unattended finalization. Its default output after alignment is a report plus the aligned timeline.

### Stage B — synthesis pass

Dense interpretive sessions need a slower synthesis pass in the parent session or with a stronger model. This pass should:

- read the preserved raw memo, aligned timeline, transcript, and existing note
- treat un-timestamped memo/reflection lines as part of the user's attention signal, not leftovers
- run recall searches from the most distinctive or charged memo language, not only from obvious transcript topics
- identify the deeper arc underneath the surface topic before drafting
- produce a draft for review unless the user has already approved writing final notes unattended

The common failure mode is optimizing for the apparent template and missing anomalous memo lines that reveal what the user was actually working out.

## Arguments

`$ARGUMENTS` format: `<session-name> [--align-only] [--audio <file>] [--speakers N]`

- **session-name** (required): The margins session name (e.g., `my-call`). Used to find/create:
  - Memo, audio segments, and transcript artifacts registered for the stable session id
  - External audio supplied through `--audio`
- **--align-only** (optional): Stop after producing the aligned timeline. Skip distillation.
- **--audio \<file\>** (optional): Explicit audio file path (for non-margins recordings).
- **--speakers N** (optional): Override speaker count for mono diarization. Accept the legacy skill spelling `--num-speakers N`, but translate it to `margins ... --speakers N`.

### Artifact resolver / path handling

Resolve through the standalone Rust CLI before transcribing. This preserves the
selected project, stable session ids, current pointer, multipart offsets, and
registered artifact precedence without teaching agents storage internals.

1. When the user means the newest meeting in the active vault, skip discovery:
   `margins transcript latest` (and `margins artifacts latest`) resolve it in a
   single call. Prefer this shorthand over a `margins recent` round-trip. Run
   `margins recent` only to browse or disambiguate among meetings by id/title.
   Reach for `margins recent --all` solely as a last resort — when you have no id
   and the meeting is in another vault. It is a cross-vault discovery aid, not
   part of the normal resolve path.
2. Run `margins artifacts "<session-id>"` to inspect registered files and
   existence. Do not query SQLite or broadly glob transcript/audio files.
3. Run `margins transcript "<session-id>"` to retrieve the preferred full
   transcript or aligned fallback plus memo/saved-note paths.
4. Concrete meeting IDs are resolved automatically across registered vaults by
   `artifacts` and `transcript`. If the CLI reports a duplicate-id ambiguity,
   pass the `vault` id from `recent --all` as the historical global selector:
   `margins --project "<id-or-path>" ...`. Relative audio, memo, and Granola
   paths remain relative to the invocation directory.
5. Treat a successful `margins transcript` body as the usable transcript. In
   particular, `view="full"` may be rendered directly from a terminal
   `*.live-transcript.json` checkpoint; consume that body and do **not** run
   `margins process` merely because no `_aligned.md` file exists. Process only
   when the transcript command has no usable body, the user explicitly asks to
   reprocess, or alignment genuinely must be rebuilt.
6. For audio-only input, call `margins transcribe`; a memo is optional. Do not
   fail solely because timed memo lines are absent.
7. Never delete artifacts unless the user explicitly asks. Use
   `margins artifacts-prune` only for registered expired temporary artifacts.

### Mode inference

Before transcribing, the skill should **infer the transcription mode** and confirm with the user:

1. **Read the memo** — look for cues about the recording type:
   - Names/speakers mentioned → likely multi-speaker
   - "call", "interview", "chat" in the session name → likely multi-speaker
   - "lecture", "sermon", "talk", "notes" → likely single-speaker
   - Existing Margins session returned by `margins recent` → channel mode is automatic

2. **Check the audio** — stereo vs mono:
   - Stereo → margins recorder output, automatic channel separation
   - Mono → present options to user

3. **Ask the user** (for mono audio only):

   > How should I transcribe this?
   >
   > 1. **Single speaker** — lecture, voice memo, sermon (fastest)
   > 2. **Two speakers** — conversation, interview, phone call
   > 3. **Multiple speakers** — meeting, group discussion (specify count)

   Skip asking if `--speakers` was provided or if cues are unambiguous.

### Parsing $ARGUMENTS

```
$ARGUMENTS = "standup"
-> session: "standup"
-> resolve: `margins recent`, then `margins artifacts standup`
-> inspect: `margins transcript standup`
-> process only if no usable transcript is returned

$ARGUMENTS = "standup --align-only"
-> session: "standup"
-> output: ".margins/standup_aligned.md"
-> STOP after Phase I

$ARGUMENTS = "coffee-chat --audio ~/Downloads/recording.m4a"
-> session: "coffee-chat"
-> audio: ~/Downloads/recording.m4a
-> infer mode from memo + ask user

$ARGUMENTS = "coffee-chat --audio ~/Downloads/recording.m4a --speakers 3"
-> session: "coffee-chat"
-> audio: ~/Downloads/recording.m4a, 3 speakers (no need to ask)
```

---

## Phase I — Alignment

### Step 1: Locate session artifacts

Use the CLI artifact resolver above to identify the stable session id and
registered files.

1. Run `margins recent`, `margins artifacts "<session-id>"`, and
   `margins transcript "<session-id>"`. Read the reported memo, which contains
   lines like:
   ```
   [00:05] discussing API redesign
   [01:30 ~02:15] revisited auth approach — decided on JWT
   [05:00] action item: draft RFC by Friday
   ```
2. Preserve the raw memo before any rewrite or distillation. Alignment uses the
   original memo, not a distilled final note.
3. Treat the paths and `segments` metadata returned by the CLI as authoritative;
   do not inspect or modify `.margins/sessions.sqlite`.
4. Templates are read from the **bundle** shipped with the skill (`$CLAUDE_PLUGIN_ROOT/skills/margins/templates/`), which is the source of truth so bundle updates always take effect. A file in `<margins-dir>/templates/<name>.md` is honored only as an explicit user override for that one template. Do **not** auto-seed or copy the bundle into `<margins-dir>/templates/` — auto-seeding turns that directory into a stale cache that shadows later bundle updates.
5. Use `margins artifacts` and `margins transcript` to check existing aligned,
   terminal-checkpoint, and full transcript outputs before retranscribing. A
   successful `view="full"` response is sufficient even when its source path
   ends in `.live-transcript.json`.

If deterministic resolution fails, present the candidate files found and ask the user for the correct session name/path.

### Step 2: Transcribe audio

Skip this step when `margins transcript` already returned a usable body. Do not
re-run ASR for a terminal live checkpoint that the CLI rendered as
`view="full"`.

When processing is actually required, use the standalone public Rust CLI. For
an existing Margins session, it resolves every registered segment, applies
global offsets, preserves stereo mic/system channels, writes transcript JSON,
and produces the aligned timeline:

```bash
margins process "<session-name>" [--speakers N]
```

**Modes** (auto-detected from input format + `--speakers`):

| Input           | --speakers | Behavior                                                    |
| --------------- | -------------- | ----------------------------------------------------------- |
| Stereo WAV      | (ignored)      | Splits channels. Ch0 = mic, Ch1 = system audio.             |
| Mono/any format | 1 (default)    | Plain transcription, single channel.                        |
| Mono/any format | 2+             | Diarize (WeSpeaker + spectral clustering), then transcribe. |

**For margins recorder sessions** (stereo segments):

```bash
margins process "<session-name>"
```

Do not concatenate or offset segments by hand. `margins process` reads every segment and its `offset_ms` from session metadata.

**For mono recordings** (voice memos, external audio):

```bash
margins transcribe "<audio-file>" --name "<session-name>" --speakers 2 [--memo "<memo-file>"]
```

Accepts WAV, M4A, MP3, FLAC, AAC, and other formats supported by the Rust decoder. With `--speakers > 1`:

- Speaker separation uses diarization instead of stereo channels
- Channels map to `SPEAKER_00`, `SPEAKER_01`, etc.
- Long files are automatically chunked (30-min default) with cross-chunk speaker consistency
- VAD filtering drops tokens outside speech regions

**Speaker identification**: When diarization is used, the skill should ask the user to identify which speaker is which during distillation (Step 4). Present a short sample from each channel and ask the user to label them.

### Step 3: Align memo + transcript

Normal processing already aligns the memo. If transcript JSON exists and only alignment needs rebuilding, run:

```bash
margins process "<session-name>" --align-only
```

This produces the aligned timeline. In the output, `ch0` = mic (the local user), `ch1` = system audio (the remote participant). Report to the user:

> Aligned <N> memo lines with <M> transcript entries. Output: `.margins/<session-name>_aligned.md`

**If `--align-only` was passed, stop here.**

---

## Phase II — Distillation

Before drafting, read and apply the shared interpretation rules in `skills/margins/distillation-core.md`. That file is the single source of truth for memo weighting, both-speaker capture, attribution audit, vault evidence, frontmatter matching, people enrichment, template selection, grounding markers, confidence/provenance, and writing style. Do not restate or fork those rules here.

### Step 4: Host-specific evidence setup

Use the body returned by `margins transcript "<session-id>"` as the complete
transcript source. This includes a terminal live checkpoint rendered with
`view="full"`; it does not need a redundant `margins process` pass. If the CLI
falls back to an `_aligned.md` file, that body remains usable but can omit
stretches where no memo was taken. If a desktop `_capture_context.md` sidecar is
present, pass it through the core's evidence-priority rule rather than treating
it as authoritative.

If diarization was used and speaker labels are still generic, present a short sample from each channel and ask the user to label them before drafting.

### Step 5: Vault search

Run vault search using the CLI and apply the core's vault-evidence rules. Treat search as supporting evidence, not a prerequisite.

For 2-3 conversation-specific queries, run:

```bash
margins recall "specific query"
```

Interactive terminals show the `enzyme catalyze`-style tree. Agent shell capture
is non-interactive and returns the compatible JSON envelope; use its `results`
entries (`file_path`, `content`, `similarity`, and optional `via_catalyst_id`) as
recall evidence. If `results` is empty, fall back to the memo/transcript and
targeted Grep for concrete anchors. Recovery and degraded-mode details are
diagnostic stderr; do not copy them into the saved note.

Use Grep only for concrete anchors such as existing people links, tags, companies, proper nouns, wikilinks, or note titles.

Read the top 3-5 most relevant notes, collect existing tags and confirmed wikilinks, and model frontmatter from an existing same-session note first or the closest relevant notes otherwise.

### Step 6: Template + draft

Load templates from `$CLAUDE_PLUGIN_ROOT/skills/margins/templates/`, with `<margins-dir>/templates/<name>.md` honored only as an explicit per-template user override. Never copy or auto-seed bundled templates into `<margins-dir>/templates/`.

Choose the template using the catalog in `skills/margins/distillation-core.md`, draft according to the shared rules, and preserve clean Markdown if grounding comments are stripped.

### Step 7: Review

Present the complete draft to the user. Ask:

- Does the structure capture what mattered in this conversation?
- Any sections to expand, trim, or restructure?
- Any quotes or moments missing that should be included?

Apply revisions if requested. Iterate until the user is satisfied.

### Step 8: Write to vault

Once approved:

**Where the note lands.** Margins uses a git-style folder model: the vault root
is the folder that *contains* the `.margins/` directory for this session (walk
up from the session's `.margins/` path) — this is the folder where the user ran
`margins new`. The distilled note lands in that vault root, next to the session.
Never leave a note stranded inside `.margins/` — that directory is Margins'
internal store, not a note destination. Never create `meetings/`, `people/`, or
any other folder; if a `people/` (or similar) folder already exists, read it for
context only.

1. Read `saved_note_path` from `margins transcript "<session-id>"`. If it exists,
   read that note first. Prefer targeted edits or replacing the reviewed
   distillation section; do not discard user edits unless the user explicitly
   approved full replacement. Update frontmatter `tags`/`people` fields from
   recall results.
2. If `saved_note_path` is absent, create the note in the vault root
   (`<vault>/[timestamp] [descriptive name].md`) with the Edit tool. If — and
   only if — you cannot resolve a vault root at all (no `.margins/` or
   `.obsidian/` parent folder is discoverable), ask the user once for the
   destination instead of writing into `.margins/`.
3. For a newly created, unregistered note, rename with a descriptive suffix
   following vault naming conventions:
   - Keep timestamp prefix
   - Add 3-7 word descriptive name, lowercase
   - Pattern for conversations: `[timestamp] chat with [person] about [topic].md`

```bash
mv "<vault>/[old-filename].md" "<vault>/[old-filename-prefix] [descriptive name].md"
```

4. Do not rename an already registered saved note or update Margins storage by
   hand. Preserve its path so the stable session pointer remains valid.

---

## Handling Poor Transcript Quality

Many transcripts come from speech-to-text and contain fragmented, garbled text. When you encounter this:

- Reconstruct the most likely intended meaning from context
- Use memo lines to disambiguate unclear passages
- Preserve distinctive phrasing even when surrounding text is garbled
- If a passage is genuinely unrecoverable, note it as `[unclear]` rather than guessing
- Don't reproduce speech-to-text artifacts ("I. Mean. That. The.") — clean them up

## Example Invocations

```
/margins standup
# Full pipeline: transcribe stereo audio, align, distill into vault note

/margins standup --align-only
# Only produce the aligned timeline, skip distillation

/margins coffee-chat --audio ~/Downloads/recording.m4a --speakers 2
# Diarize mono audio with 2 speakers, then distill

/margins group-call --audio ~/Downloads/meeting.wav --speakers 4
# Diarize with 4 expected speakers
```

## Confidence calibration and provenance

Use the shared rules in `skills/margins/distillation-core.md`. Keep this section out of sync by design: the core file owns the policy.

## Optional: shareable derivative (private + shared split)

Some sessions warrant two artifacts from one conversation. Offer this when the user wants to circulate a recap, or when the private note contains material that shouldn't leave their hands.

- **Private note** — the full strategic record: room dynamics (who to weight on which decision), the user's own ownership/lane read, missed openings and self-critique, confidence calibration, comp/relationship context, and links to the user's private vault notes. Written to be useful to the user and to a later agent making decisions.
- **Shared note** — a clean, self-contained recap for the other participants: enough grounding context to stand alone, the problem shape and building blocks as the group's output, attributed provenance, decisions, open questions, and consolidated action items. Strip everything private: comp, dynamics, self-coaching, strategic self-positioning, and private-note wikilinks (convert to plain text so it renders outside the vault).

Keep them consistent on facts but different in register — the shared version states the spine and primitives as the group's shared output (owners can be left as open questions rather than asserted), which is both more accurate and more useful to the other participants than a "look how well our ideas combined" narrative. Confirm sensitive framings with the user before including them in the shared version. This split is a parent-session workflow; the desktop distiller produces a single note.
