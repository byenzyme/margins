# Relationship re-entry

## The product in one sentence

Margins helps someone re-enter an important relationship without rereading its
history.

The acute moment is not generic meeting preparation. It is the few minutes before a
recurring, consequential conversation when the user remembers the people but has lost
the shape of what has been unfolding: what changed, what remains open, which concerns
keep returning, and what this meeting may now require.

For the first users described in the GTM work, this is most legible for founders moving
among recurring customer, advisor, investor, and close-collaborator conversations. Their
history is rich enough to matter, scattered enough to be difficult to recover, and
sensitive enough that an assistant must earn trust before intervening live.

## The day-to-day experience

Ten minutes before a meeting, Margins offers to prepare the relationship—not merely
summarize the last call.

The first view is brief enough to scan. It might show a trajectory, several adjacent
threads, a tension, an open fork, or only a few facts. Its shape follows the evidence;
the interface does not force every relationship into a single story.

For example:

> Three threads appear active.
>
> You have moved from explaining personalization to confronting whether the underlying
> power math is visible. Andrew has repeatedly asked for direction rather than another
> map. Separately, both of you remain uneasy about when inferred signals become
> surveillance.
>
> The open fork: is this conversation about choosing a direction, or agreeing on the
> evidence that would justify one?

The user can then interrogate it by voice or text:

- What changed since we last spoke?
- Show me the evidence for the first thread.
- What am I most likely to avoid?
- Give me one question to carry into the room.

Voice is an interaction layer. Relationship preparation is the job. The useful loop is:

1. A calendar or attendee context opens a preparation opportunity.
2. Margins assembles the bounded history of those people and this work.
3. A preparation partner shows the shortest honest shape the evidence earns.
4. The user questions, narrows, or corrects it.
5. During the meeting, Margins uses only a deliberately selected historical frame when
   it materially improves a live cue.
6. Afterward, the note updates what the next preparation should know.

## What the research established

The preparation experiment compared the existing fixed-section packet with an adaptive
construction while holding the model lane, cutoff, relationship material, source
catalog, token setting, and source validation constant. This was a comparison of two
complete constructions, not a pure persona ablation.

The adaptive construction won blind in two deliberately different Andrew cases:

- a rich multi-session relationship arc;
- a sparse case containing three related experiences that should not automatically
  become one narrative.

Its first version was meaningfully better but 2.2 times too long. A plain-language
revision required the shortest form that preserved what the founder should notice or
do, put fresh concrete actions before broad interpretation, prohibited an arc without
recurrence, and ended with one or two lines usable in the room. The revised construction
won both cases again. It was 808 words versus an 886-word control in the rich case, and
724 versus 572 in the fragment case; the judge found the additional fragment-case
material earned its attention cost through concrete meeting action.

The research supports these product judgments:

- Fragments are a legitimate result. Narrative must be earned by recurrence, change,
  or a supported turning point.
- The generator should decide privately how much coherence the evidence supports; it
  should not emit a classification or fill a fixed visible outline.
- Fresh source-prescribed actions belong before broader interpretation.
- Every factual claim needs an eligible source. Interpretation should remain visibly
  tentative and preserve counterevidence or open forks.
- Identity integrity is load-bearing. A broad first-name match previously conflated two
  different Kevins, so relationship neighborhoods must use full-name, alias, and
  first-class entity evidence rather than filename resemblance alone.
- A full preparation packet should not be injected into live generation by default.
  Earlier blind live-cue studies found history inert or harmful to the speak/quiet
  decision. Material history should be selected before live generation, and no history
  is a valid result.

This evidence is directional, not a reliability benchmark: six successful preparation
generations, one relationship, two cases, one model lane, and one generation per cell.
The revision was informed by the first reveal.

## What exists in the desktop today

The desktop already has two different assistance paths:

- During recording, a submitted memo is an attention trigger for a short live cue.
  See `request_backchannel_for_memo` in `desktop/src-tauri/src/lib.rs`.
- Before recording, and during clock-stopped pause blocks, a prep sketch is annotated
  with source-backed marginalia that can be steered. See `hydrate_prep_sketch` in
  `desktop/src-tauri/src/lib.rs` and `desktop/src-tauri/src/prep_ai.rs`.

The adaptive relationship packet and trajectory generator are not production desktop
features. They were implemented only in the portable replay/evaluation lane on branch
`bb/extract-portable-live-cue-core-and-linux-replay-thr_myqgtcw66y`, most directly in:

- `51694f1` — longitudinal packet and trajectory replay lanes;
- `185aa10` — adaptive relationship-preparation construction and focused evaluation.

That branch is research evidence, not a merge unit. It contains a long dependency chain
and a temporary vendored Pi fork. Do not merge or cherry-pick the branch wholesale.
Inspect the two commits above, then implement the feature against current `main`.

## The next feature

Add a first-class **Prepare relationship** surface before the meeting.

Its input should be the selected meeting, resolved attendees, a temporal cutoff, and the
user's optional preparation sketch. Product retrieval must use the selected Margins
Workspace store at `$MARGINS_HOME/workspaces/<id>/index.db`, not the standalone
Enzyme research database.

The implementation should:

1. Resolve each attendee to a full relationship identity and aliases. If identity is
   ambiguous, ask or omit rather than merge.
2. Assemble a bounded relationship neighborhood: person records, eligible prior
   sessions, first-class relational catalysts, relevant work artifacts, and an optional
   prior trajectory. Preserve a source manifest.
3. Generate an adaptive preparation artifact whose form follows the evidence. Keep
   audit provenance separate from its visible organization.
4. Show a compact top layer with expandable evidence and support conversational follow-up
   by voice or text.
5. Let the existing Prep notes rail engage with the user's sketch and the prepared
   relationship context rather than replacing the sketch.
6. Keep the full packet out of the live-cue prompt. If a live moment benefits from
   history, select one material frame upstream; otherwise use transcript and memo alone.
7. After the meeting note is accepted, update a relationship trajectory in the Margins
   product store for the next preparation cycle.

Do not begin by changing the live-cue persona. The most powerful product proof is that a
user can open an upcoming meeting and recover what the conversation is part of in under
a minute.

## Acceptance criteria

A useful first implementation should demonstrate that:

- preparation is available before recording from real attendee or meeting context;
- the first useful view arrives quickly enough for a five-minute preparation window;
- the top layer can be understood in roughly 60–90 seconds;
- every recalled fact opens its source and every interpretation is distinguishable from
  fact;
- separate people are never joined without explicit identity evidence;
- rich history may yield a trajectory, while sparse history may yield fragments or no
  synthesis;
- the freshest concrete request or promised action is not displaced by a broader story;
- voice/text follow-up can narrow, challenge, or request evidence without regenerating
  the whole artifact blindly;
- the existing clock-stopped prep flow remains useful and live cues do not inherit the
  full packet automatically;
- an accepted post-meeting note can improve the next packet without back-projecting new
  knowledge into old meetings.

Evaluate with contemporaneous controls and blind judgment. Read
`desktop/PROMPT_BEHAVIOR_EVALUATION.md` before designing the comparison.

## Research record

The full local evaluation bundle is intentionally outside Git because it contains
private, source-derived artifacts:

- `/workspace/projects/margins-constellation-eval/RESULT.md`
- `/workspace/projects/margins-constellation-eval/RUNBOOK.md`
- `/workspace/projects/margins-constellation-eval/judgment/`

The result document contains the complete generated examples and source-type manifests.
Use it when available, but this brief is the self-contained product and implementation
handoff.
