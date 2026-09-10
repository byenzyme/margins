# Aside Opportunity Memo

## Working Frame

Most meeting tools solve the archive problem: capture the meeting, summarize it, extract action items, and make the record searchable or exportable. That is useful, but it is increasingly commoditized. Users already tolerate messy transcript silos because the workaround is good enough: store the raw transcript somewhere, ask an agent to read it later, and move on.

The opportunity for Aside is not "better meeting notes" or even "local-first meeting notes." Those are not sharp enough. The stronger opportunity is helping conversations change the next action, next question, next decision, or next relationship move.

The core product question is:

> What should this conversation change?

That is the distinction. A meeting bot produces an archive. Aside should produce continuity that acts.

## The Problem With The Current Category

The current meeting-notes category is organized around relief:

- I do not need to take notes.
- I can remember what happened.
- I can send a follow-up.
- I can search the transcript later.
- I can push the note into CRM, Notion, Slack, or email.

Granola is strong here because the job is convenience. It joins the call, writes the recap, drafts follow-ups, and integrates with workflows that already exist. For many users, that is enough.

This means Aside should not try to win by saying:

- summaries are better;
- transcripts are private;
- notes live in Markdown;
- Obsidian is a better destination;
- local-first is morally or architecturally superior.

Those may matter, but they are not the product wedge by themselves. The user's revealed behavior is that siloed transcripts are acceptable if the workflow is convenient enough.

The sharper critique is:

> Existing meeting tools help users preserve what happened, but not enough of them help users understand what the conversation now requires.

That requirement might be a question to ask next time, an unresolved objection, a proof artifact to build, a relationship obligation, a decision that needs to be forced, or a pattern across conversations that should change strategy.

## The Opportunity

Aside can become the continuity layer for high-context work.

Not a transcript repository. Not a meeting bot. Not a generic memory layer.

A tool that helps a user carry the right thing forward from one conversation into the next action.

The value is strongest where conversations are part of an ongoing project, relationship, or decision process:

- founders doing customer discovery;
- consultants and coaches with long-running client relationships;
- product leaders synthesizing repeated user/research signals;
- investors or operators who need proof, follow-up, and relationship continuity;
- researchers/writers/interviewers who need exact moments to feed future work.

The common thread is not "they need notes." The common thread is:

> The next move depends on interpreting this conversation in the context of prior work.

That is where a transcript archive is insufficient. The user does not merely need recall. They need orientation.

## The Core Job

The core job is not catch-up, though catch-up is required.

Catch-up answers:

- Who is this?
- What happened last time?
- What did I promise?
- What are the action items?

Significance answers:

- Why does this conversation matter now?
- What project, relationship, or open question does it belong to?
- What am I tempted to miss?
- What should I be listening for?
- What would make this conversation change my mind?
- What is the next action this conversation should produce?

Aside should earn trust with accurate catch-up, then deliver value through significance.

The product should help the user enter and leave conversations with a sharper sense of purpose.

## The Key Product Primitive

The primitive should not be "related notes."

It should be **carry-forward obligations**.

A carry-forward obligation is something from the user's archive, meeting history, marks, or project context that should affect the next action.

Examples:

- **Unresolved question:** "You never resolved whether this is a workflow-delta product or a memory product."
- **Proof debt:** "You keep saying this needs evidence. The next call should produce a proof artifact."
- **Relationship obligation:** "They gave you a sharp lens. You owe them a visible update, not a generic follow-up."
- **Decision pressure:** "This meeting should decide whether founder discovery or consultant continuity is the wedge."
- **Risk:** "This framing may sound like surveillance if shared externally."
- **Pattern to test:** "You believe action items are commoditized. Ask whether next-call prep is actually valuable."

This is much sharper than surfacing themes. Themes are often impressive but inert. Carry-forward obligations imply a next move.

The hard product rule should be:

> If a surfaced insight does not change a question, follow-up, decision, prep step, or project state, it is probably decoration.

## Design Against Recognition Theater

The major failure mode is recognition theater.

Recognition theater is when the AI notices a recurring concept in the user's notes and presents it as value:

> "This connects to your recurring theme of memory as judgment."

That may feel meaningful, but it can still leave the user asking: "So what?"

Actionable continuity is different:

> "This call should not become another abstract memory conversation. Ask what concrete workflow would prove better judgment."

Or:

> "Do not send a broad follow-up. Send the artifact that shows how their lens changed the work."

Aside should be disciplined about not showing conceptual continuity unless it is attached to an action, concern, or decision.

The product should quietly suppress many "interesting" connections. It should only foreground the ones that matter to the next move.

## The Before-Meeting Opportunity

Before a meeting, the user does not need a research dossier. They need a concise orientation.

The ideal pre-meeting artifact is not a briefing document. It is a posture-setting card.

A useful structure:

```text
This meeting is for:
[one job]

Do not drift into:
[one tempting but wrong mode]

Carry forward:
[one unresolved thread]

Ask:
[one question]

Evidence:
[1-2 source links]
```

Example:

```text
This meeting is for:
Testing whether workflow-delta proof is commercially legible.

Do not drift into:
Explaining the whole memory-layer thesis.

Carry forward:
Liam pushed on commercial proof, not product taste.

Ask:
"What would make this look durable rather than bespoke?"

Evidence:
June 9 investor note; proof-artifact thread.
```

This is not just catch-up. It changes how the user enters the room.

The killer experience is when the user glances at Aside and thinks:

> "Right. That is the point of this call."

## The After-Meeting Opportunity

After a meeting, the note should not lead with summary. Summary is useful, but it should be subordinate.

The more valuable post-meeting structure is:

```text
What changed
What remains unresolved
What to carry forward
Next action
Evidence
Summary
Transcript
```

The user should leave the meeting not just remembering what happened, but knowing what the conversation now demands.

Examples:

- "This strengthened the case for founder discovery, but weakened the VC wedge."
- "The open question is no longer local-first; it is whether next-call prep changes behavior."
- "The next action is to build a with/without artifact, not write positioning copy."
- "This should be followed up as a narrow ask, not a broad update."

This makes the meeting note an operational object, not just a record.

## The First 3-5 Meetings

The compounding value should become visible quickly.

After one meeting, Aside should prove that it can carry forward a question.

After three meetings, it should show a pattern.

After five meetings, it should help the user make a better decision.

Possible product-level promise:

> By the fifth conversation, Aside should show what is repeating, what is unresolved, and what next move the user keeps avoiding.

That is the retention hook. Not "your archive is growing," but "your conversations are starting to compound."

## Why Local/Vault-Native Still Matters

Local-first and Obsidian-native matter only insofar as they enable this continuity.

The product should not lead with:

> "Your data is local."

It should lead with:

> "Your meetings can use the strategy docs, relationship notes, past calls, and open questions you already have."

Locality is the mechanism that makes the context broader and safer. It allows Aside to work from material the user would not upload to a generic meeting bot. But the felt value is not storage. The felt value is that the next conversation starts with the right context.

A better formulation:

> Because Aside works from your own files, it can help each meeting inherit the thinking around it.

## What Needs To Be True

For this strategy to work, several things need to be true.

1. **The user must have high-context work.**
   This is not for every meeting. It is for conversations where history, relationship, judgment, or strategy matter.

2. **The user must care about the next move.**
   If the job is only "send me a recap," Granola wins.

3. **The product must be selective.**
   Surfacing too many connections will make Aside feel like an overthinking machine.

4. **The output must be action-shaped.**
   Every major insight should attach to a question, follow-up, decision, risk, or project thread.

5. **The system must show evidence.**
   Because significance can feel presumptuous, Aside needs source trails: marks, transcript moments, prior notes, prior meetings.

## Strategic Position

Aside should not be framed as a better archive.

It should be framed as a tool for people whose conversations are part of an ongoing body of work.

Possible positioning:

> Aside turns past conversations into the next right move.

Or:

> Aside helps every meeting remember what it is supposed to advance.

Or:

> Meeting notes for work that has a memory.

The internal product test should be stricter:

> Did Aside change what the user asked, sent, decided, prepared, or carried forward?

If not, the experience may be impressive, but it is not yet valuable enough.

The opportunity is to make meeting memory operational. Not memory as a feature. Memory as better next action.
