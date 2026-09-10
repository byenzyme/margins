---
name: watermark
description: Low-latency, transcript-grounded feedback during a live Margins session. Use when the user asks for the latest watermark, a sanity check, what to say next, how to steer or structure the conversation, what tactical point to surface, or any other in-the-moment question about the meeting currently being recorded. Infer the feedback intent from the free-form prompt; do not require mode flags or a fixed question format.
argument-hint: <free-form question or request>
user-invocable: true
allowed-tools: Bash
---

# Watermark — Live Meeting Feedback

Answer a time-sensitive question about the conversation currently being captured
by Margins. Treat `$ARGUMENTS` as a natural-language prompt, not as command-line
flags. The user may ask a direct question, offer a tentative interpretation, ask
for words to say, or simply request the newest read.

This is a live feedback loop, not meeting distillation. Optimize for a fresh,
speakable intervention while the conversation is still unfolding.

## Runtime recommendation

Prefer a low-latency model with low reasoning effort and a small output budget
for live invocations. Preserve the same behavioral contract when the host does
not expose model or reasoning controls. Escalate to a stronger or slower model
only when the prompt is unusually ambiguous or consequential; use the normal
Margins distillation workflow for comprehensive post-meeting synthesis.

This is an execution preference, not a model requirement. Keeping the skill
model-agnostic lets the command work across supported agent runtimes and avoids
coupling its behavior to a model name that may change.

## Core contract

1. Fetch the current session and transcript immediately.
2. Use the complete transcript view returned by the CLI, weighting the newest
   complete turn most heavily.
3. Infer what kind of help the prompt calls for.
4. Answer the question directly, grounded in the newest complete turn.
5. Keep the first useful response short enough to use in the room.

Aim to return useful guidance within 15 seconds. Do not run recall, search the
vault, inspect old notes, or delegate work before the first answer. Use the full
transcript view returned by the single CLI call; do not perform a separate,
slower reconstruction pass. Those extra operations improve retrospective
synthesis but make live feedback arrive a turn late.

## Fetch the freshest transcript

Run:

```bash
margins transcript --format json
```

The command resolves the active/recent session when no meeting id is supplied.
Parse its JSON fields:

- `decoded_until_ms`: the transcript watermark
- `live`: whether capture is active
- `terminal`: whether the transcript is final
- `body`: the preferred transcript view
- `meeting_id`: stable session identifier

If default resolution is ambiguous, run `margins recent`, choose the active or
newest live session, then run:

```bash
margins transcript "<meeting-id>" --format json
```

Do not use `--follow`: the command should take one fresh snapshot and return.
The user can invoke the skill again for the next watermark and transcript.

Run this command anew on every invocation. Transcription and transcript loading
are cheap enough that the CLI should own freshness and reconstruction; do not
maintain a hand-trimmed transcript window or treat the previous watermark as a
read boundary. The returned `body` is the context for the answer, while the
newest complete turn carries the most weight for what is timely now.

If the transcript is still live, reason from the newest complete turn and say
briefly when the final words appear partial. If no live transcript is available,
report that plainly and give the watermark or terminal state that is available.

## Infer the feedback intent

Infer intent from both `$ARGUMENTS` and the latest conversation. These are
reasoning lenses, not user-facing modes and not mutually exclusive:

- **Read** — What is the other person seeking, reacting to, or opening up now?
- **Sanity check** — Is the user's tentative interpretation supported by the
  latest exchange? Preserve useful nuance rather than returning a bare yes/no.
- **Steer** — Should the user redirect, narrow, challenge, affirm, or let the
  thread continue?
- **Structure** — What compact framework would organize the discussion now?
- **Tactical** — What decision, scope boundary, question, owner, or next step
  should be surfaced before the moment passes?
- **Speakable wording** — What sentence can the user say next?
- **Debrief** — Only when the capture is terminal or the user explicitly asks
  how the meeting went; this may be somewhat broader, but remains distinct from
  full Margins distillation.

A prompt can imply several lenses. For example, a concern that the conversation
is drifting may need a sanity check, a tactical scope judgment, and one sentence
that redirects without shutting down the useful thread. Do not ask the user to
choose a mode when their prompt and the transcript make the need legible.

## Use prior interaction as coaching memory

Within the same agent conversation, retain:

- the user's stated goal for the meeting;
- decisions and frames already landed;
- advice already given and whether the transcript shows it landed;
- unresolved loops and promised follow-ups;
- the previous `decoded_until_ms`.

On later invocations, call `margins transcript --format json` again and use the
complete view it returns. The previous watermark helps explain what changed
since the last request, but it does not constrain transcript retrieval or
analysis. This keeps the answer grounded when a recent turn depends on an older
decision, definition, or conversational thread.

Update an interpretation when new evidence warrants it. Do not preserve an old
coaching frame merely for consistency.

## Response shape

Lead with the outcome, not process narration. Normally use:

```markdown
**Watermark:** ~MM:SS — one sentence on what changed.

Direct answer to the user's question in 1–3 sentences.

**Say:** “One concise sentence the user could actually say next.”

**Watch:** One risk, opening, or decision that may otherwise pass.
```

Adapt rather than mechanically filling every field:

- Omit `Say` when the user asked only for a factual read.
- Omit `Watch` when there is no meaningful secondary point.
- For a requested structure, a small list or table can replace `Say`.
- For a terminal debrief, use `Landed`, `Tighten`, and `Carry forward` if useful.

Keep the initial live answer around 80–180 words. A structure may run to roughly
250 words when the user explicitly needs something to walk through. Give one
primary move rather than a menu of equally weighted possibilities.

## Judgment principles

- Distinguish what is timely from what is merely interesting. A divergent thread
  can be valuable and still be wrong for the present scope.
- Answer the user's actual uncertainty. If they ask whether to steer away, make
  the call and explain the governing constraint.
- Prefer a reusable rule plus one application over a long bespoke analysis.
- Preserve the other person's agency. A good intervention can narrow scope
  without dismissing what they are exploring.
- Convert vision into a decision, experiment, owner, or parked thread when the
  moment calls for tactical closure.
- Label uncertainty caused by partial transcription, unclear speakers, or a
  stale watermark. Do not manufacture confidence from noisy live text.
- Do not expose private reasoning or summarize unrelated parts of the meeting.

## Pseudonymized examples

### Tentative interpretation

Input:

```text
/watermark the external-events idea feels hard to automate because quality is
the bottleneck—is that right? get the latest watermark and sanity-check me
```

Good behavior: inspect the latest turn, decide whether the interpretation is
supported, distinguish signal detection from response quality if relevant, and
offer one timely sentence. Do not launch a general research pass on automation.

### Request for a structure

Input:

```text
/watermark I think they want a high-level structure to walk through now—what
would organize this without dragging us into implementation?
```

Good behavior: infer the structure lens and return one compact framework tied to
the themes now on the floor. Do not ask the user to rerun with a `structure`
flag.

### Scope and quantitative design

Input:

```text
/watermark they are moving into quantitative design. should I steer away because
the cells may be too small, or surface something specific for scope?
```

Good behavior: make the steer-versus-surface judgment, identify the governing
quantity constraint, and give one speakable intervention. Avoid solving the
entire experiment design during the live response.

### Tactical close

Input:

```text
/watermark newest one—what tactical things should I make sure we land?
```

Good behavior: identify the few decisions, owners, or open loops most at risk of
being lost before the meeting ends. Rank them; do not recap the full meeting.

### Post-meeting reflection

Input:

```text
/watermark we just finished. how did I do? I think the last section wandered and
wasn't useful for today's scope, though it may matter elsewhere
```

Good behavior: confirm the terminal watermark, assess what landed and what did
not, distinguish divergence from waste, and recommend where to preserve any
valuable off-scope insight. Keep full note creation in the separate Margins
distillation workflow.
