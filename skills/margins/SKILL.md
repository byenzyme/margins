---
name: margins
description: End-to-end margins session processing — transcribe, align memo + transcript, distill into a structured vault note via Enzyme.
argument-hint: <session-name> [--align-only] [--audio <audio-file> [--speakers N]]
user-invocable: true
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, AskUserQuestion, mcp__enzyme__semantic_search, mcp__enzyme__start_exploring_vault
---

# Margins — Capture to Vault Note

Take an margins session end-to-end: transcribe audio, align the transcript with the user's real-time memo, distill into a structured vault note connected to existing thinking via Enzyme.

**Environment**: `$OBSIDIAN_VAULT` refers to the Obsidian vault root (the additional working directory configured for this project).

## Prerequisites

- Standalone public Rust CLI from `crates/public/margins-cli` in `$PATH`, built
  with the ASR feature for the platform (for example,
  `cargo install --path crates/public/margins-cli --features coreml-asr`)
- For multi-speaker audio, build Margins with `polyvoice-diarization` (the default release build includes it)

## What this produces

1. **Aligned timeline** (`.margins/<session>_aligned.md`) — interleaved transcript + memo on a shared timeline
2. **Vault note draft or approved final note** — structured note written to the obsidian vault using a template, with Enzyme-sourced connections

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
- run Enzyme searches from the most distinctive or charged memo language, not only from obvious transcript topics
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

1. Run `margins recent` and match the requested id/title. Use `latest` only
   when the user explicitly means the newest meeting.
2. Run `margins artifacts "<session-id>"` to inspect registered files and
   existence. Do not query SQLite or broadly glob transcript/audio files.
3. Run `margins transcript "<session-id>"` to retrieve the preferred full
   transcript or aligned fallback plus memo/saved-note paths.
4. For a project other than the configured active project, pass the historical
   global selector as `margins --project "<id-or-path>" ...`; relative audio,
   memo, and Granola paths remain relative to the invocation directory.
5. If an aligned transcript is already registered and the user did not ask to
   rerun, use it. If only alignment must be rebuilt, use `--align-only`.
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
-> process: `margins process standup` (channel mode and segment offsets are automatic)

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
5. Use `margins artifacts` and `margins transcript` to check existing aligned
   and full transcript outputs before retranscribing.

If deterministic resolution fails, present the candidate files found and ask the user for the correct session name/path.

### Step 2: Transcribe audio

Run processing through the standalone public Rust CLI. For an existing Margins
session, it resolves every registered segment, applies global offsets,
preserves stereo mic/system channels, writes transcript JSON, and produces the
aligned timeline:

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

### Step 4: Memo-guided analysis

**Source of truth: the full aligned transcript.** Draw the substance of the note from `<margins-dir>/<session-name>_aligned.md` — the complete both-channel timeline. The memo is the *attention signal* that sets priority, not the body of the note. Build the note from the aligned transcript and let the memo decide what surfaces first; do not invert this and reconstruct the conversation from memo lines alone.

If a lighter `_capture_context.md` sidecar is also present (produced by the live worker during desktop capture), treat it as secondary. The live capture context can be lopsided: the live worker sometimes commits only the local mic channel or just the tail of the conversation, so it drops the remote speaker and most of the discussion. Relying on it produces a thin, one-sided note that can even get key points backwards. **If the aligned transcript is materially more complete than the capture context — more entries, both channels present, earlier coverage — distill from the aligned transcript and use the capture context only to corroborate.**

1. **Extract topics from memo lines first.** Each memo line marks a moment the user chose to note. Classify each line:
   - **Decision** — a choice was made ("decided on JWT")
   - **Action item** — a commitment or next step ("draft RFC by Friday")
   - **Insight** — a realization or interesting framing ("the real bottleneck is onboarding, not retention")
   - **Tension** — a disagreement or unresolved question ("revisited auth approach")
   - **Question** — something to follow up on
   - **Observation** — neutral notation of what's being discussed

2. **Edited memos signal reconsideration.** A memo line followed by `*[edited at MM:SS]*` means the user came back to revise it at that later timestamp. This indicates the topic was important enough to revisit. Weight these higher.

3. **Extract topics from un-noted transcript.** Scan the transcript for significant topics that the user did _not_ memo. These are secondary — the user may have chosen not to note them for a reason, or they may have been too absorbed to write.

   **Capture both participants, not just the local user.** The remote speaker (ch1) often carries the most useful content: pointers to specific people or resources, proposed solutions, scope decisions, reframes of the problem, and emotional or relational moves like reassurance or an offer to help. Pull these from the aligned transcript by channel. A good test: a reader should be able to tell from the note what each person actually contributed, named specifically — not a blurred summary of "the conversation."

   **Separate what was said from what the user privately felt.** The memo can record the user's in-the-moment anxiety, intent, or second-guessing, which may diverge from what was actually voiced in the room. Attribute things to the transcript when they were said aloud and to the memo when they were the user's private processing. Do not conflate the two.

   **Audit speaker attribution before drafting.** List the people from session settings/frontmatter, the names mentioned in the transcript, and any absent third parties. Attribute channel turns only to configured or transcript-evidenced participants. If a mentioned person is only someone to coordinate with later, keep them out of speaker attribution and mark uncertainty instead of guessing.

   **Do not drop once-mentioned failure modes.** For each participant, list every failure mode, risk, or negative-space concern they named, including ones mentioned only once or as an margins. These often carry the scoping value of the call and should appear in `Tensions`, `What emerged`, or action items when relevant.

4. **Include un-timestamped memo/reflection lines.** Lines without timestamps, especially lines written after the timed memo stream, are still part of the user's attention signal. They often contain the second-order interpretation the user was working out while listening. Classify them too, and weight charged language highly.

5. **Look for anomalous memo signals.** Identify memo lines that do not fit the obvious transcript template. These may reveal a deeper arc than the transcript's surface category suggests.

6. **Detect missed openings.** If memo lines or later reflections suggest uncertainty, floundering, hesitation, “what should I ask,” or internal analysis that did not become a live question, capture that as a first-class signal. The note may need a section such as `Missed openings`, `Questions I held internally`, or `Follow-up questions to ask next time`.

7. **Detect pivot/reframe turns.** Surface any moment where a participant changes the frame of the work: "not ready", "not complete", "no runbook", "maturity gap", "this is the real problem", "we should not optimize that yet", or similar. Treat these as candidate section anchors, not incidental details.

8. **Build a prioritized topic list.** Memo-marked topics first (ordered by classification weight: decisions > action items > insights > tensions > questions > observations), then anomalous/deeper-arc topics, then significant un-noted transcript topics.

### Step 5: Enzyme vault search

Connect the conversation to existing vault thinking.

Treat vault search as supporting evidence, not as a prerequisite for writing the
note. If Enzyme is unavailable, uninitialized, or too thin to return useful
connections, continue from the memo and transcript and say nothing visible about
the failed mechanism in the saved note.

#### Phase A: Explore the vault

Run a bounded Petri pass first, using a conversation-specific query from the
memo and transcript:

```bash
enzyme petri --query "specific query"
```

This returns the **slate** — trending entities with catalysts that represent
where the vault has already found language for related things. Use the slate to
calibrate search queries: if the transcript discusses "knowledge management
tools" but the vault uses "pkm" or "tool-thinking", reach for the vault's
language. Keep this calibration narrow; do not repaint the whole vault just to
draft one note. (If an Enzyme MCP server is available instead of the CLI,
`mcp__enzyme__start_exploring_vault` is the equivalent fallback.)

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
- Focus on memo-marked topics first, especially anomalous or emotionally charged memo language, then significant un-noted topics
- At least one query should come from the surprising/deeper memo signal rather than the obvious surface topic
- Queries should be substantive and specific, drawn from actual conversation content

**Good queries** (drawn from specific themes, calibrated to vault language):

- "happenstance interfaces and serendipity in knowledge tools"
- "creative tool vs consumer tool positioning"
- "behavioral graph as enabler business"

**Bad queries** (generic):

- "meeting notes"
- "conversation summary"
- "knowledge management"

For each query, run `enzyme catalyze "specific query" --limit 5`. Use only supported Enzyme CLI arguments:

```bash
enzyme petri --query "specific query"
enzyme catalyze "specific query" --limit 5
```

Do **not** run unqueried whole-vault `enzyme petri` during Margins note
distillation. Do **not** pass `--limit`, `--top`, or `--catalyst-budget` to
`enzyme petri`. Do not use unsupported flags such as `--no-guide`,
`--catalysts-per-entity`, or `--threshold`. (If an Enzyme MCP server is
available instead of the CLI, `mcp__enzyme__semantic_search` with `result_limit:
5` is the equivalent fallback.)

#### Phase C: Read and collect

After both structured and semantic results come back:

- Read the top 3-5 most relevant notes
- Note the **frontmatter schema** of those notes: which keys appear (e.g. `created`, `tags`, `people`, `type`, `aliases`), their order, and whether values use `[[wikilinks]]` or plain strings. Match this exact key set and ordering in the output note's frontmatter rather than imposing Margins's default keys.
- If an existing note or reference note for the same session is supplied, its frontmatter schema wins over broader related-note schemas.
- Note **existing tags** that appear in those notes (for use in the output — never invent tags)
- Note **people links** (`[[Person Name]]`) that appear
- Note **connections** between the transcript content and vault content — these become citations in the draft

### Step 6: Template + draft

1. **Select and load template** from the bundle (`$CLAUDE_PLUGIN_ROOT/skills/margins/templates/`), which is the source of truth. For each template, if `<margins-dir>/templates/<name>.md` exists it is an explicit user override and takes precedence; otherwise load the bundled file. Never copy the bundle into `<margins-dir>/templates/` — read the bundle directly so updates always apply.
   - `1on1-idea-exchange.md` — default for most 1:1 conversations (idea exchange, catch-ups, brainstorms)
   - `discovery-call.md` — client/prospect conversations focused on needs, fit, and next steps
   - `group-conversation.md` — 3+ participants where tracking who thinks what matters
   - `design-scoping-session.md` — architecture, design, or technical scoping sessions (1:1 or group) where the value is the *shape of the problem and the concrete building blocks*, not the texture of the exchange. Use when the conversation produced conclusions that need to survive being disagreed with later — lead with the spine, extract primitives with confidence levels, attribute provenance, make ownership legible.
   - `talk-reflection.md` — sermons, lectures, talks, or any session where the user is listening and reflecting, not conversing. The memo captures their thinking in response to a speaker.

   Choose based on the memo and transcript content. If unclear, default to `1on1-idea-exchange`. Note that `design-scoping-session` is orthogonal to headcount — a design session can be 1:1 or a group; pick it over `group-conversation` when the design substance matters more than who-thinks-what.

2. **Generate draft** following these principles:

   **Topic ordering**: Use the memo-weighted priority from Step 4. Decisions and action items surface first, then insights and tensions, then anomalous/deeper-arc memo topics, then un-noted topics. This reflects what the user actually cared about during the conversation.

   **Second-pass frame:** Before drafting, write a one-sentence answer for yourself: “Beyond the surface topic, this is really about…” If that sentence only restates the transcript category, think again using the anomalous memo lines and Enzyme results.

   **Reframe check:** Before drafting, identify the strongest turn where someone changed the frame of the work. If the call moved from "finish the thing" to "make a handoffable version", from "debug the feature" to "define the failure mode", or from "extend scope" to "clarify maturity/runbook gaps", give that turn visible weight.

   **Content principles:**
   - Preserve specifics over generic summary: concrete experiments and their results, named architecture or implementation details, and direct-ish quotes for pivotal moments. Use the participants' actual words.
   - **Distinguish what was decided from what stayed open.** State decisions that were actually made as settled, and keep unresolved questions or deferred choices in their own thread (`Tensions and open threads` or similar). Do not present an open question as a conclusion, or bury a real decision in hedged language.
   - **Account for both speakers.** When the remote participant proposed a solution, named a person or resource, made a scope call, reframed the problem, or offered reassurance, name that contribution specifically rather than folding it into a generic recap.
   - Reconstruct fragmented speech-to-text into intended meaning; flag uncertain reconstructions with [reconstructed]
   - Weave in vault context as natural `[[wikilink]]` citations where connections exist
   - Populate frontmatter with tags extracted from Enzyme results only, and grep-confirm each emitted tag before writing (never invent tags)
   - Frontmatter keys and ordering must match the existing same-session note if one was supplied; otherwise match the vault notes read in Step 5 Phase C. Only fall back to `created` / `tags` / `people` defaults when no existing vault note was available to model from.
   - Preserve people/attendees from session metadata and source memo/note frontmatter; add them to the `people:` field using `[[Name]]` format, and merge any transcript-evidenced people without dropping supplied names
   - When the conversation names a project, company, tool, or topic that already has a note in the vault (confirmed via grep/Enzyme in Step 5), wikilink the first in-body mention as `[[Existing Note Title]]`. Only link to notes you confirmed exist — never invent a wikilink target.
   - Consolidate action items. Every commitment, next step, owner, and deadline mentioned anywhere in the conversation goes into one final `### Action items` section as a checkbox list (`- [ ] ...`). Do not scatter action items across thematic sections.
   - Skip pleasantries, logistics, and small talk unless they contained real content
   - Prioritize specificity over comprehensiveness — five vivid points beat fifteen generic bullets

   **Writing style:**
   Richness comes from specific, transcript-grounded detail, not from more words. Keep the existing style rules below — they make the specifics land:
   - Direct statements over contrast constructions (no "doesn't X, but Y" patterns)
   - Use em dashes sparingly
   - No rhetorical questions as transitions
   - Avoid AI-typical phrases: "disappears into the background", "perhaps the better question is", "conceived as"
   - Active voice, concrete language, varied sentence construction

   **Citation integration:**
   - Reference vault notes naturally: `as explored in [[note title]]` or `connects to [[note title]]`
   - Use block embeds `![[file#^block-id]]` only when the source has explicit block IDs and the quote is concise and directly relevant
   - Don't force connections — only cite where the link genuinely enriches the note. For genuinely novel concepts with no vault note, do not invent a wikilink target.

   **Work/scoping-call evidence:**
   For technical, product, client, sales, or collaboration calls, make the note useful as a future scoping artifact. Before finalizing, check whether the draft includes:
   - concrete work items, product surfaces, features, or implementation tasks mentioned
   - examples, screens, flows, data sources, tools, repos, or integrations named in the transcript
   - open implementation questions and risks
   - asks made by the other person
   - next steps proposed by the user
   - enough transcript-grounded detail to scope follow-up work without re-reading the full transcript

   **Missed-opening / question-capture section:**
   If the memo suggests the user was internally processing instead of asking live questions, add a compact section capturing:
   - questions the user seemed to hold internally
   - moments where a sharper external question could have changed the conversation
   - follow-up questions to ask in the next call
     Do this without turning every note into a coaching note; include it only when the memo/transcript supports it.

### Desktop streaming / grounding marker contract

When running inside Margins Desktop or any renderer that supports grounding markers:

- Stream only user-facing final-note Markdown plus hidden grounding comments. Never stream private reasoning.
- Use memo IDs as attention and grounding signals, not as the final note order. Assign IDs by memo line order: `m001`, `m002`, `m003`, ... including un-timestamped memo/reflection lines when present.
- Place a marker immediately before the section or paragraph that accounts for those memo IDs:

  ```md
  <!--MARGINS:USE {"section_id":"weekly-status","memo_ids":["m002"],"mode":"absorbed","disposition":"folded_into_section","transcript_refs":[{"start_secs":64,"end_secs":79,"quote":"weekly status ritual"}],"vault_refs":["Operating cadence.md"]}-->

  ## Weekly status ritual
  ```

- Supported marker for v1: `MARGINS:USE`. `MARGINS:SOURCES` may be used only if it follows the same line-oriented HTML-comment shape and is safe to strip.
- Use `mode`/`disposition` language such as `absorbed`, `accounted_for`, `folded_into_section`, `folded_into_action_item`, or `not_included`.
- Memo lines are attention signals. Do not force verbatim memo text into the note unless the phrasing itself matters.
- The visible note must remain clean Markdown if all `<!--MARGINS:...-->` comments are stripped.
- Do **not** add a visible `Grounding`, `Sources`, or memo-accounting section to the note. The renderer uses hidden markers to account for memo lines; visible source UI is handled by the desktop app.
- Final save should pass clean Markdown to `margins_save_note` and include optional `grounding` metadata when the tool/backend supports it.

### Step 7: Review

Present the complete draft to the user. Ask:

- Does the structure capture what mattered in this conversation?
- Any sections to expand, trim, or restructure?
- Any quotes or moments missing that should be included?

Apply revisions if requested. Iterate until the user is satisfied.

### Step 8: Write to vault

Once approved:

1. Read `saved_note_path` from `margins transcript "<session-id>"`. If it exists,
   read that note first. Prefer targeted edits or replacing the reviewed
   distillation section; do not discard user edits unless the user explicitly
   approved full replacement. Update frontmatter `tags`/`people` fields from
   Enzyme results.
2. If `saved_note_path` is absent, fall back to creating the note via
   `./scripts/new-note.sh` (run from `$OBSIDIAN_VAULT/`), then populate it with
   the approved content using the Edit tool.
3. For a newly created, unregistered note, rename with a descriptive suffix
   following vault naming conventions:
   - Keep timestamp prefix
   - Add 3-7 word descriptive name, lowercase
   - Pattern for conversations: `[timestamp] chat with [person] about [topic].md`

```bash
mv "$OBSIDIAN_VAULT/inbox/[old-filename].md" "$OBSIDIAN_VAULT/inbox/[old-filename-prefix] [descriptive name].md"
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

These apply to any substantive session, and are the core discipline of `design-scoping-session`. They can be layered onto any template.

- **Separate social convergence from concept strength.** A thing everyone nodded at is not the same as a thing that will hold. When a session produced conclusions, mark which you actually trust — `load-bearing` (build on it), `plausible` (real shape, not yet a decision), `soft` (a placeholder for a decision nobody made: a named-but-undefined concept, a metaphor that isn't yet a mapping, positioning doing no design work). Do not let a tidy synthesis launder weak ideas into apparent decisions. A note that says "here's how our ideas combined" is usually hiding this failure.
- **Attribute provenance.** For substantive points, name who raised each one. In a private note this records where thinking came from; in anything shared it tells a reader who to follow up with and keeps conclusions from reading as if they appeared from nowhere.
- **Find the spine before drafting.** Beyond the surface topics, what is the one load-bearing claim that reorganizes everything else? If several surface problems turn out to be the same problem, that collapse is usually the most valuable output — lead with it.
- **Make ownership legible, including `TBD`.** Name contested boundaries rather than smoothing them over.

## Optional: shareable derivative (private + shared split)

Some sessions warrant two artifacts from one conversation. Offer this when the user wants to circulate a recap, or when the private note contains material that shouldn't leave their hands.

- **Private note** — the full strategic record: room dynamics (who to weight on which decision), the user's own ownership/lane read, missed openings and self-critique, confidence calibration, comp/relationship context, and links to the user's private vault notes. Written to be useful to the user and to a later agent making decisions.
- **Shared note** — a clean, self-contained recap for the other participants: enough grounding context to stand alone, the problem shape and building blocks as the group's output, attributed provenance, decisions, open questions, and consolidated action items. Strip everything private: comp, dynamics, self-coaching, strategic self-positioning, and private-note wikilinks (convert to plain text so it renders outside the vault).

Keep them consistent on facts but different in register — the shared version states the spine and primitives as the group's shared output (owners can be left as open questions rather than asserted), which is both more accurate and more useful to the other participants than a "look how well our ideas combined" narrative. Confirm sensitive framings with the user before including them in the shared version. This split is a parent-session workflow; the desktop distiller produces a single note.
