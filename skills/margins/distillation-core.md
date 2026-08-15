# Margins Distillation Core

This is the shared source of truth for Margins note interpretation. Host-specific files may explain how inputs, tools, or save paths work, but they should not restate or fork these rules.

## Evidence Priority

Use the fullest transcript timeline as the factual source of truth. The memo is the user's attention signal: it decides what gets priority, but it is not the whole body of the note.

If both a full aligned transcript and a lighter capture-context sidecar are available, compare coverage. When the aligned transcript has more entries, both speakers, or earlier coverage, distill from it and use the capture context only to corroborate. A thin capture context can overrepresent the local channel and get the other person's contribution backwards.

Treat un-timestamped memo or reflection lines as part of the attention signal. They often carry the user's second-order interpretation after the timed memo stream.

## Memo Weighting

Classify memo lines before drafting:

- **Decision**: a choice was made.
- **Action item**: a commitment, owner, deadline, or next step.
- **Insight**: a realization, framing, or useful language.
- **Tension**: disagreement, unresolved pressure, risk, or open thread.
- **Question**: something to follow up on.
- **Observation**: neutral notation of what was being discussed.

Edited memos carry extra weight. A line followed by `*[edited at MM:SS]*` means the user came back to revise it later.

Order the note by importance, not memo order. Start from decisions and action items, then insights and tensions, then anomalous or deeper-arc memo signals, then significant un-noted transcript topics.

## Both-Speaker Capture

Capture both participants, not just the local user. Remote or other speakers often carry the useful content: constraints, specific people or resources, scope decisions, reframes, proposed solutions, reassurance, or offers to help. A reader should be able to tell what each person contributed, named specifically.

Separate what was said aloud from what the user privately felt or noticed. Attribute transcript claims to the speaker who said them. Attribute anxiety, hesitation, intent, or second-guessing to the memo when it was private processing.

Do not drop once-mentioned failure modes, risks, or negative-space concerns. For each participant, preserve every failure mode they named, even once. These often carry the scoping value of the call.

## Attribution Audit

Before drafting, audit people and speakers:

- Start with configured attendees and people from source frontmatter.
- Compare them with names mentioned in the transcript.
- Distinguish actual participants from absent third parties, future contacts, or action-item owners.
- Attribute channel turns only to configured or transcript-evidenced participants.
- Preserve configured/source people in frontmatter as `[[Name]]` wikilinks.
- Merge transcript-evidenced people only when useful; never drop supplied names.
- Mark uncertainty instead of guessing.

People-page enrichment is state-dependent. Never create a `people/` folder or any configured people folder. If the folder already exists, maintain missing person pages there using the configured person-note template. If the folder is absent, do nothing.

## Vault Evidence

Vault search and supplied vault context are supporting evidence, not setup. If the vault is empty, unavailable, or too thin, continue from the memo and transcript without visible mechanism language in the saved note.

Use vault context to calibrate vocabulary, confirm existing wikilinks and tags, and bridge this capture to prior thinking. Prefer a few high-signal bridges over many shallow citations.

Recall queries must use the vault's own vocabulary, not generic category labels. Build them from memo-marked or anomalous language first, then transcript nouns, people, projects, tools, and tensions. At least one query must come from the memo's most surprising, charged, or user-marked signal, even if the obvious transcript topic would be easier to search.

Only emit tags that already exist in evidence you read or confirmed. When a project, company, tool, person, or topic already has a confirmed note in the vault, wikilink the first in-body mention as `[[Existing Note Title]]`. Never invent wikilink targets.

When the host expects frontmatter, the note must start with a complete YAML frontmatter block. Model an existing same-session note first. If none exists, model the closest relevant vault notes you read: key set, order, and whether people use `[[wikilinks]]` or plain strings. Fall back to `created` / `tags` / `people` defaults only when there is no useful model. Do not omit the block, fence it, or place visible content before it.

## Template Catalog

Select one template explicitly before drafting:

- `1on1-idea-exchange.md`: default for most 1:1 conversations, catch-ups, brainstorms, and relationship-building conversations.
- `discovery-call.md`: client/prospect conversations focused on needs, fit, evidence, and next steps.
- `group-conversation.md`: three or more participants where who thought or committed what matters.
- `design-scoping-session.md`: architecture, design, or technical scoping captures, 1:1 or group, where the shape of the problem and concrete building blocks matter more than who-thought-what. Use when conclusions need to survive being disagreed with later.
- `talk-reflection.md`: lectures, talks, sermons, panels, or listening/reflection sessions where the memo captures the user's response.

If unclear, default to `1on1-idea-exchange.md`. `design-scoping-session` is orthogonal to headcount; prefer it over `group-conversation` when design substance matters more than tracking who-thinks-what.

Templates provide a structure, not a cage. Preserve the chosen template's intent, but adapt headings when evidence calls for it.

## Drafting Rules

Before drafting, write a one-sentence private answer to: "Beyond the surface topic, this is really about..." If that only restates the transcript category, look again at anomalous or charged memo lines.

Choose a natural note title before drafting. Use 3-7 words, lowercase sentence style, and name the central tension or specific object of the session. Never use a generic recap label such as "meeting notes", "customer call recap", "discussion summary", or the calendar title alone.

Identify the strongest reframe turn before drafting. If the call moved from "finish the thing" to "make a handoffable version", from "debug the feature" to "define the failure mode", or from "extend scope" to "clarify maturity/runbook gaps", make that turn visible.

Preserve specifics over generic summary: concrete examples, screens, flows, data sources, tools, repos, integrations, architecture details, experiments and their results, asks made by the other person, and direct-ish quotes for pivotal moments.

Distinguish settled decisions from open questions. State decisions as settled only when they were actually made. Keep unresolved questions or deferred choices in `Tensions and open threads` or an equivalent section.

Consolidate action items. Every commitment, next step, owner, and deadline mentioned anywhere in the conversation goes into one final `### Action items` section as a checkbox list. Do not scatter action items across thematic sections.

Skip pleasantries, logistics, and small talk unless they contained real content. Prioritize specificity over comprehensiveness.

For poor transcripts, reconstruct intended meaning from context and memo lines. Preserve distinctive phrasing. Flag uncertain reconstructions with `[reconstructed]` or `[unclear]`. Do not reproduce speech-to-text artifacts.

## Missed Openings

If the memo suggests the user was internally processing instead of asking live questions, add a compact section for questions the user seemed to hold internally, moments where a sharper external question could have changed the conversation, or follow-up questions to ask next time. Include this only when supported by the memo/transcript.

## Confidence And Provenance

These apply to any substantive session and are core to `design-scoping-session`.

- Separate social convergence from concept strength. A thing everyone nodded at is not the same as a thing that will hold.
- Mark building blocks as `load-bearing`, `plausible`, or `soft` when the session produced conclusions. `soft` means a placeholder for a decision nobody made: a named-but-undefined concept, a metaphor that is not yet a mapping, or positioning doing no design work.
- Attribute provenance for substantive points. Name who raised each primitive, reframe, decision, or material concern.
- Find the spine before drafting: the one load-bearing claim that reorganizes everything else.
- Make ownership legible, including `TBD`. Name contested boundaries rather than smoothing them over.
- For scoping or work-call notes, preserve enough concrete detail to scope follow-up without rereading the transcript: product surfaces, implementation tasks, examples, tools, repos, integrations, risks, asks, and next steps.

## Grounding Markers

When the renderer supports grounding markers, stream only user-facing final-note Markdown plus hidden grounding comments. Never stream private reasoning.

Assign stable memo IDs by memo line order: `m001`, `m002`, `m003`, including un-timestamped memo/reflection lines.

As a first pass, attach every grounded claim to a short exact `note_quote` anchor: a phrase that appears in the visible note and identifies the claim the marker supports. The `note_quote` is required for each `MARGINS:USE` marker, not an optional example field.

Place a marker immediately before the paragraph or section that accounts for those memo IDs:

```md
<!--MARGINS:USE {"section_id":"weekly-status","memo_ids":["m002"],"note_quote":"weekly status ritual","mode":"absorbed","disposition":"folded_into_section","transcript_refs":[{"start_secs":64,"end_secs":79,"quote":"weekly status ritual"}],"vault_refs":["Operating cadence.md"]}-->
```

Use `MARGINS:USE` for v1. Use dispositions such as `absorbed`, `accounted_for`, `folded_into_section`, `folded_into_action_item`, or `not_included`.

Memo lines are attention signals. Do not force verbatim memo text into the note unless the phrasing itself matters.

The visible note must remain clean Markdown if all `<!--MARGINS:...-->` comments are stripped. Do not add a visible `Grounding`, `Sources`, or memo-accounting section.

Output the note as raw Markdown, never inside a code fence. If frontmatter is present, the first bytes of the output must be `---`, not ```yaml.

## Writing Style

Richness comes from specific, transcript-grounded detail, not from more words.

- Direct statements over contrast constructions.
- Use em dashes sparingly.
- No rhetorical questions as transitions.
- Avoid AI-typical phrases: "disappears into the background", "perhaps the better question is", "conceived as".
- Active voice, concrete language, varied sentence construction.
