---
name: aside
description: End-to-end aside session processing — transcribe, align memo + transcript, distill into a structured vault note via Enzyme.
argument-hint: <session-name> [--align-only] [--diarize <audio-file> [--num-speakers N]]
user-invocable: true
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion, mcp__enzyme__semantic_search, mcp__enzyme__start_exploring_vault
---

# Aside — Capture to Vault Note

Take an aside session end-to-end: transcribe audio, align the transcript with the user's real-time memo, distill into a structured vault note connected to existing thinking via Enzyme.

**Environment**: `$OBSIDIAN_VAULT` refers to the Obsidian vault root (the additional working directory configured for this project).

## Prerequisites

- `fluidaudiocli` binary in `$PATH` — built from [FluidInference/FluidAudio](https://github.com/FluidInference/FluidAudio) (Parakeet TDT v3 via CoreML)
- `pip install diarize` — for mono/pre-recorded audio diarization (WeSpeaker + Silero VAD, no HF gating)

## What this produces

1. **Aligned timeline** (`.aside/<session>_aligned.md`) — interleaved transcript + memo on a shared timeline
2. **Vault note** — structured note written to the obsidian vault using a template, with Enzyme-sourced connections

If `--align-only` is passed, only the aligned timeline (step 1) is produced.

## Arguments

`$ARGUMENTS` format: `<session-name> [--align-only] [--audio <file>] [--num-speakers N]`

- **session-name** (required): The aside session name (e.g., `my-call`). Used to find/create:
  - Memo: `<session-name>.md` (in the aside working directory)
  - Audio: `.aside/<session-name>_seg*.wav` (aside recorder) or `--audio` file
  - Metadata: `.aside/<session-name>.meta.json` (for segment offsets and durations)
- **--align-only** (optional): Stop after producing the aligned timeline. Skip distillation.
- **--audio \<file\>** (optional): Explicit audio file path (for non-aside recordings).
- **--num-speakers N** (optional): Override speaker count for diarization. If not provided, the skill infers the mode.

### Mode inference

Before transcribing, the skill should **infer the transcription mode** and confirm with the user:

1. **Read the memo** — look for cues about the recording type:
   - Names/speakers mentioned → likely multi-speaker
   - "call", "interview", "chat" in the session name → likely multi-speaker
   - "lecture", "sermon", "talk", "notes" → likely single-speaker
   - Aside recorder segments (`.aside/<name>_seg*.wav`) → stereo (mode is automatic)

2. **Check the audio** — stereo vs mono:
   - Stereo → aside recorder output, automatic channel separation
   - Mono → present options to user

3. **Ask the user** (for mono audio only):
   > How should I transcribe this?
   > 1. **Single speaker** — lecture, voice memo, sermon (fastest)
   > 2. **Two speakers** — conversation, interview, phone call
   > 3. **Multiple speakers** — meeting, group discussion (specify count)

   Skip asking if `--num-speakers` was provided or if cues are unambiguous.

### Parsing $ARGUMENTS

```
$ARGUMENTS = "standup"
-> session: "standup"
-> memo: "standup.md"
-> audio: ".aside/standup_seg*.wav" (stereo, auto)
-> output: ".aside/standup_aligned.md"

$ARGUMENTS = "standup --align-only"
-> session: "standup"
-> output: ".aside/standup_aligned.md"
-> STOP after Phase I

$ARGUMENTS = "coffee-chat --audio ~/Downloads/recording.m4a"
-> session: "coffee-chat"
-> audio: ~/Downloads/recording.m4a
-> infer mode from memo + ask user

$ARGUMENTS = "coffee-chat --audio ~/Downloads/recording.m4a --num-speakers 3"
-> session: "coffee-chat"
-> audio: ~/Downloads/recording.m4a, 3 speakers (no need to ask)
```

---

## Phase I — Alignment

### Step 1: Locate session artifacts

1. Read the memo file `<session-name>.md` — contains lines like:
   ```
   [00:05] discussing API redesign
   [01:30 ~02:15] revisited auth approach — decided on JWT
   [05:00] action item: draft RFC by Friday
   ```
2. Read `.aside/<session-name>.meta.json` for segment info (`segment_index`, `wav_path`, `offset_ms`, `duration_secs`) and `vault_note_path` (if the session was published to the vault on quit).
3. List WAV segments: `.aside/<session-name>_seg*.wav`

If the memo file doesn't exist, ask the user for the correct session name.

### Step 2: Transcribe audio

Run transcription using the unified CLI:

```bash
python3 ${CLAUDE_PLUGIN_ROOT}/skills/aside/scripts/aside.py transcribe "<audio-file>" --output "<output-path>" [--num-speakers N]
```

**Modes** (auto-detected from input format + `--num-speakers`):

| Input | --num-speakers | Behavior |
|---|---|---|
| Stereo WAV | (ignored) | Splits channels. Ch0 = mic, Ch1 = system audio. |
| Mono/any format | 1 (default) | Plain transcription, single channel. |
| Mono/any format | 2+ | Diarize (WeSpeaker + spectral clustering), then transcribe. |

**For aside recorder sessions** (stereo segments):
```bash
python3 ${CLAUDE_PLUGIN_ROOT}/skills/aside/scripts/aside.py transcribe ".aside/<session-name>_seg<N>.wav" --output "/tmp/<session-name>_seg<N>_transcript.json"
```

When there are multiple segments, adjust transcript timestamps by each segment's `offset_ms` from meta so they align to the session's global timeline.

**For mono recordings** (voice memos, external audio):
```bash
python3 ${CLAUDE_PLUGIN_ROOT}/skills/aside/scripts/aside.py transcribe "<audio-file>" --output ".aside/<session-name>_transcript.json" --num-speakers 2
```

Accepts any ffmpeg-readable format (m4a, wav, mp3, etc.). With `--num-speakers > 1`:
- Speaker separation uses diarization instead of stereo channels
- Channels map to `SPEAKER_00`, `SPEAKER_01`, etc.
- Long files are automatically chunked (30-min default) with cross-chunk speaker consistency
- VAD filtering drops tokens outside speech regions

**Speaker identification**: When diarization is used, the skill should ask the user to identify which speaker is which during distillation (Step 4). Present a short sample from each channel and ask the user to label them.

### Step 3: Align memo + transcript

Run:
```bash
python3 ${CLAUDE_PLUGIN_ROOT}/skills/aside/scripts/aside.py align \
  --memo "<session-name>.md" \
  --transcripts /tmp/<session-name>_seg0_transcript.json [seg1...] \
  --meta ".aside/<session-name>.meta.json" \
  --output ".aside/<session-name>_aligned.md"
```

This produces the aligned timeline. In the output, `ch0` = mic (the local user), `ch1` = system audio (the remote participant). Report to the user:
> Aligned <N> memo lines with <M> transcript entries. Output: `.aside/<session-name>_aligned.md`

**If `--align-only` was passed, stop here.**

---

## Phase II — Distillation

### Step 4: Memo-guided analysis

The memo is the user's real-time attention signal — what they found important enough to write down during the conversation, not in hindsight. Use it to drive analysis priority.

1. **Extract topics from memo lines first.** Each memo line marks a moment the user chose to note. Classify each line:
   - **Decision** — a choice was made ("decided on JWT")
   - **Action item** — a commitment or next step ("draft RFC by Friday")
   - **Insight** — a realization or interesting framing ("the real bottleneck is onboarding, not retention")
   - **Tension** — a disagreement or unresolved question ("revisited auth approach")
   - **Question** — something to follow up on
   - **Observation** — neutral notation of what's being discussed

2. **Edited memos signal reconsideration.** A memo line followed by `*[edited at MM:SS]*` means the user came back to revise it at that later timestamp. This indicates the topic was important enough to revisit. Weight these higher.

3. **Extract topics from un-noted transcript.** Scan the transcript for significant topics that the user did *not* memo. These are secondary — the user may have chosen not to note them for a reason, or they may have been too absorbed to write.

4. **Build a prioritized topic list.** Memo-marked topics first (ordered by classification weight: decisions > action items > insights > tensions > questions > observations), then significant un-noted topics.

### Step 5: Enzyme vault search

Connect the conversation to existing vault thinking.

#### Phase A: Explore the vault

Run `mcp__enzyme__start_exploring_vault` first. This returns the **slate** — trending entities with catalysts that represent where the vault has already found language for things. Use the slate to calibrate search queries: if the transcript discusses "knowledge management tools" but the vault uses "pkm" or "tool-thinking", reach for the vault's language.

#### Phase B: Search for connections

Use **two search strategies**:

**Structured search (Grep)** — for concrete anchors that exist verbatim in the vault:
- People mentioned: `[[Person Name]]`
- Tags from the slate that match transcript topics: `#pkm`, `#ai-ux`
- Companies or proper nouns mentioned in the conversation
- Wikilinks or note titles

Run Grep for each concrete anchor. Prioritize anchors that appear near memo-marked topics.

**Semantic search** — for themes and concepts without a concrete anchor:
- Formulate 2-3 queries from the prioritized topic list (Step 4), using the vault's vocabulary where possible
- Focus on memo-marked topics first, then significant un-noted topics
- Queries should be substantive and specific, drawn from actual conversation content

**Good queries** (drawn from specific themes, calibrated to vault language):
- "happenstance interfaces and serendipity in knowledge tools"
- "creative tool vs consumer tool positioning"
- "behavioral graph as enabler business"

**Bad queries** (generic):
- "meeting notes"
- "conversation summary"
- "knowledge management"

For each query, run `mcp__enzyme__semantic_search` with `result_limit: 5`.

#### Phase C: Read and collect

After both structured and semantic results come back:
- Read the top 3-5 most relevant notes
- Note **existing tags** that appear in those notes (for use in the output — never invent tags)
- Note **people links** (`[[Person Name]]`) that appear
- Note **connections** between the transcript content and vault content — these become citations in the draft

### Step 6: Template + draft

1. **Select and load template** from `$CLAUDE_PLUGIN_ROOT/skills/aside/templates/`:
   - `1on1-idea-exchange.md` — default for most 1:1 conversations (idea exchange, catch-ups, brainstorms)
   - `discovery-call.md` — client/prospect conversations focused on needs, fit, and next steps
   - `group-conversation.md` — 3+ participants where tracking who thinks what matters
   - `talk-reflection.md` — sermons, lectures, talks, or any session where the user is listening and reflecting, not conversing. The memo captures their thinking in response to a speaker.

   Choose based on the memo and transcript content. If unclear, default to `1on1-idea-exchange`.

2. **Generate draft** following these principles:

   **Topic ordering**: Use the memo-weighted priority from Step 4. Decisions and action items surface first, then insights and tensions, then un-noted topics. This reflects what the user actually cared about during the conversation.

   **Content principles:**
   - Preserve specific language and direct quotes — use their actual words
   - Reconstruct fragmented speech-to-text into intended meaning; flag uncertain reconstructions with [reconstructed]
   - Weave in vault context as natural `[[wikilink]]` citations where connections exist
   - Populate frontmatter with tags extracted from Enzyme results only (never invent tags)
   - Add people to the `people:` field using `[[Name]]` format
   - Skip pleasantries, logistics, and small talk unless they contained real content
   - Prioritize specificity over comprehensiveness — five vivid points beat fifteen generic bullets

   **Writing style:**
   - Direct statements over contrast constructions (no "doesn't X, but Y" patterns)
   - Use em dashes sparingly
   - No rhetorical questions as transitions
   - Avoid AI-typical phrases: "disappears into the background", "perhaps the better question is", "conceived as"
   - Active voice, concrete language, varied sentence construction

   **Citation integration:**
   - Reference vault notes naturally: `as explored in [[note title]]` or `connects to [[note title]]`
   - Use block embeds `![[file#^block-id]]` only when the source has explicit block IDs and the quote is concise and directly relevant
   - Don't force connections — only cite where the link genuinely enriches the note

### Step 7: Review

Present the complete draft to the user. Ask:
- Does the structure capture what mattered in this conversation?
- Any sections to expand, trim, or restructure?
- Any quotes or moments missing that should be included?

Apply revisions if requested. Iterate until the user is satisfied.

### Step 8: Write to vault

Once approved:

1. **If `vault_note_path` exists in session metadata** (from Step 1): the note was already created on session quit. Read the existing vault note, replace everything after frontmatter with the approved distilled content, and update frontmatter `tags`/`people` fields from Enzyme results.
2. **If `vault_note_path` is absent**: fall back to creating the note via `./scripts/new-note.sh` (run from `$OBSIDIAN_VAULT/`), then populate with the approved content using the Edit tool.
3. Rename with a descriptive suffix following vault naming conventions:
   - Keep timestamp prefix
   - Add 3-7 word descriptive name, lowercase
   - Pattern for conversations: `[timestamp] chat with [person] about [topic].md`

```bash
mv "$OBSIDIAN_VAULT/inbox/[old-filename].md" "$OBSIDIAN_VAULT/inbox/[old-filename-prefix] [descriptive name].md"
```

4. After any rename, update `vault_note_path` in the session metadata:
   - Read `.aside/<session-name>.meta.json`, set `vault_note_path` to the new path, write back.

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
/aside standup
# Full pipeline: transcribe stereo audio, align, distill into vault note

/aside standup --align-only
# Only produce the aligned timeline, skip distillation

/aside coffee-chat --diarize ~/Downloads/recording.m4a
# Diarize mono audio (2 speakers default), then distill

/aside group-call --diarize ~/Downloads/meeting.wav --num-speakers 4
# Diarize with 4 expected speakers
```
