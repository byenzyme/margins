# Design / Scoping Session

Use this template when a conversation produced real design or technical substance — an architecture discussion, a scoping session, a "what are we actually building and what's hard about it" working meeting. Works whether it was 1:1 or a group; the distinguishing feature is that the value is in the *shape of the problem and the concrete building blocks*, not in the texture of the exchange.

The trigger is not "it was technical." The trigger is: **the conversation produced conclusions that need to survive being disagreed with later.** That means the note has to separate strong ideas from soft ones, attribute where things came from, and make ownership legible — not narrate how nicely everyone's ideas combined.

## Output Structure

```markdown
### [Title — the problem being worked, not the meeting's category]

**[Date] · [Participants] · [Duration if available]**

**Context:** [What brought this group together and the nominal topic. Note who owns what coming in, and what was already decided before this session.]

---

### The spine

The single load-bearing claim underneath the surface topics — the thing that, if true, reorganizes everything else. State it in 2-3 sentences. Then show the evidence from the room that earns it (not projection). If several surface problems turn out to be the same problem, say so here; that collapse is usually the most valuable output of the session.

If you can only write a restatement of the meeting's category ("this was about the architecture"), you haven't found the spine yet. Look again at the anomalous or charged moments.

### Primitives / building blocks

The concrete concepts worth designing against, ordered by how load-bearing they are. For each:

- **[Primitive]** — *raised by [who]* · **[load-bearing / plausible / soft]**. What it is, in one or two sentences. Why it matters / what sits on top of it. Ground it in what was actually said.

Confidence is the point of this section. Be honest: a thing everyone nodded at is not the same as a thing that will hold. Mark as `soft` anything that's still a placeholder for a decision nobody actually made (a named-but-undefined concept, a metaphor that isn't yet a mapping, a piece of positioning doing no design work).

### Invariants and design forks

- **Invariants** — what the system must always / must never do, at whatever minimum bar was set. These are often the real deliverable.
- **Open forks** — decisions not yet made, each with *what tips it* one way or the other (e.g. "substrate: leaning X, pending Y; tip toward whatever lets [primitive] exist cleanly").

### Ownership

Who owns which piece. `owner: TBD` is a valid and useful state — surface it as an open question rather than papering over it. Where a boundary is contested or unclear (e.g. two systems being merged), name the boundary explicitly; that's where ownership gets decided.

### Reframes and decisions

The turns where someone changed what the work *means* ("these aren't bugs, they're missing architecture"; "define first, wire up later"). Attribute each. These usually carry more weight than the feature list.

### Action items

Consolidate every commitment, next step, and follow-up into this single section.

Format: `- [ ] **[Action]** — [owner, why, context]`. For a delegable deliverable, state whether it's *defining* or *doing* — the distinction protects scope.
```

## Rules

1. **Lead with the spine, not chronology or "ideas that combined."** Social convergence is not concept strength. The reader should get the shape of the problem before any list.
2. **Rate confidence explicitly.** Separate load-bearing primitives from soft ones. Do not let a tidy synthesis launder weak ideas into apparent decisions.
3. **Attribute provenance.** Every primitive, reframe, and decision names who raised it, so a later reader knows who to follow up with and nothing reads as a conclusion from nowhere.
4. **Make ownership legible**, including `TBD`. Name contested boundaries rather than smoothing them.
5. **Preserve every once-mentioned failure mode, risk, and negative-space concern** — in design sessions these often carry the real scoping value.
6. **Keep invariants and open forks distinct from settled decisions.** Don't present an open question as a conclusion, or bury a real decision in hedged language.
7. Use direct quotes for pivotal framings and distinctive phrasings. Reconstruct fragmented speech-to-text into intended meaning; flag uncertain reconstructions.
8. Cite genuine vault connections with `[[wikilinks]]`; don't force them.
