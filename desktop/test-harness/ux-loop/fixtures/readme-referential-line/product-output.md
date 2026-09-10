---
created: '[[2026-08-13]]'
tags:
  - atlas
  - product-design
people:
  - '[[Leah Morgan]]'
---

# keep the atlas handoff in the workspace

**2026-08-13 · Leah Morgan and me · ~19 min**

**Context:** A working review of the Atlas handoff design — what happens *after* a capture, not the capture itself, which already works. Coming in, the project shape was set by [[projects/Atlas rollout|Atlas rollout]]: source notes land beside existing project material, claims stay traceable, and canonical project-page updates require review during the pilot. Two prior conversations sit underneath this one: review-before-roll-up was first agreed with [[Rui Tan]] in March, and the pilot scorecard was sketched with Leah in June.

---

### The spine

Adoption fails at the handoff, not at capture. The durable artifact has to live inside the workspace the team already returns to; the moment it becomes a *separate destination* — a polished brief, a Friday digest — it freezes a living thread into a snapshot that competes with the real decision. So the pilot has to measure return-to-work, not summaries produced.

Three surface problems collapse into that one. I opened distrusting "what happens after the call" (memo: *adoption problem is the handoff, not capture*). Leah pointed out the team already holds accurate information in project notes, decision records, and the issue tracker, so "another summary can still be another place to check." And her main worry — a weekly digest that people forward and maintain instead of the project note — is the same failure in a third costume. Accuracy, digest, and adoption are one problem: a second source of truth.

That collapse is what earns the decision to keep the workspace canonical. It also explains why same-day satisfaction is a trap: "same-day reactions will overrate the quality because everyone still remembers the call" (Leah).

### The March echo

Leah's digest objection landed as recognition, not novelty. She described the digest freezing "a living thread into a snapshot," which then "becomes a second source of truth" until "two weeks later nobody knows which version carries the real decision." That is the same objection Rui raised in March — see [[meetings/2026-03-12 Atlas review with Rui|Atlas review with Rui]], where the concern was never day-one accuracy but a brief that ages into a competing authority while the project note changes underneath it.

Two independent reviewers converging on the same failure mode is why it is load-bearing rather than one person's worry. It also settled the digest's fate in the pilot: I called it "the strongest argument against shipping the digest as the primary artifact," and Leah's condition was flat — if the digest cannot show where each claim came from, leave it out.

### Primitives / building blocks

- **Return-to-work at 48 hours** — *raised by me, defined by Leah* · **load-bearing**. The pilot measure is not summaries generated or sent; it is whether a person can reopen the project two days later, name the decision and the open boundary, and reach the source note without asking for a recap. Leah's definition is the sharp one: count it only when they can name *both* the decision and the unresolved boundary and follow the link, rather than searching Slack or asking someone to retell it. Extends the continuity test in [[principles/Living artifacts|Living artifacts]] and the scorecard already started in [[meetings/2026-06-04 Atlas pilot scorecard|Atlas pilot scorecard]].
- **The usable paragraph = decision + unresolved boundary + the thread it changed** — *raised by Leah* · **load-bearing**. What separates a usable handoff from a merely shorter one. Strip those three apart and "the reader still has to rebuild the reasoning." Leah would rather one paragraph land in the right project note than a beautiful six-section recap arrive by email.
- **Meeting-note-first, review before any roll-up** — *raised by Leah, endorsed by me* · **load-bearing**. Leah's trust argument: "a wrong write into the canonical page costs more trust than a correct meeting note earns." So write the meeting note, preserve the source, and make any proposed project-page update inspectable before it is applied.
- **Digest as a view, never a source** — *the March/Leah constraint* · **load-bearing**. A digest is allowed only as a provenance-showing view over the source notes. Without that, it is excluded from the pilot.
- **Bidirectional link** — *raised by Leah* · **plausible**. From the project page she needs to reach the meeting evidence, and from the meeting note she needs to see which project thread it belongs to. Only half is guaranteed: I can promise the meeting-to-project link now; the reverse link depends on the vault or on an approved update. Plausible, not settled, because the return direction is conditional.

### Invariants

- The workspace stays the source of truth. Do not create a parallel product surface that quietly becomes canonical.
- Never imply the project page was updated when all that was written is the meeting note. Call that boundary out explicitly.
- A digest must show where each claim came from, or it is left out.
- Preserve the source meeting note and keep any proposed project update inspectable before writing.

### Open forks

- **Are proposed project-page updates part of this pilot or a follow-on?** Leah's call: keep it open until we see whether the connected meeting note alone is enough. This is the same boundary left open with Rui in March and named as the pilot question in [[projects/Atlas rollout|Atlas rollout]] — it has not moved, and we chose not to force it.
- **The reverse (project → meeting) link.** Guaranteed only where the vault or an approved update supports it; until then it stays a conditional, not a promise.

### Ownership

- **Pilot scorecard + primary flow** — me. Rewrite the scorecard around return-to-work and remove the weekly digest from the primary flow.
- **Design-partner recruiting** — Leah. Specifically five partners who already keep project notes, "not teams looking for a new dashboard."
- **Project-page roll-up** — owner: TBD, deliberately deferred behind proving the connected meeting note.

### Reframes and decisions

- **From "does it summarize accurately" to "can someone resume the work."** The product claim cannot just be that it summarizes accurately; accuracy is table stakes because the team already has accurate sources. (me → confirmed by Leah)
- **From summaries-generated to return-to-work as the pilot metric.** "Our pilot measure is not summaries generated or sent. It is successful return-to-work after forty-eight hours, with the source trail intact." (me, memo: *pilot should test return-to-work, not summaries sent*)
- **From "maybe ship the digest" to "digest only as a sourced view."** Settled by the March echo above.
- **Pilot scoped to five design-partner calls**, each ending with a connected meeting note in the partner's existing workspace, observed at 48 hours rather than same-day. (me + Leah)

### Action items

- [ ] **Rewrite the pilot scorecard around return-to-work** — me. *Doing.* Score on the 48-hour test (name the decision and open boundary, reach the source note without a recap); remove the weekly digest from the primary flow.
- [ ] **Recruit five design partners who already keep project notes** — Leah. *Doing.* Existing note-keepers, not teams shopping for a new dashboard.
- [ ] **Decide whether proposed project-page updates belong in this pilot or a follow-on** — owner TBD. *Defining.* Hold open until the connected meeting note is shown to be sufficient on its own.
