# Aligned timeline

[00:00:05] ch0: Thanks for looking at the Atlas handoff with me. The capture itself is working. What I do not trust yet is what happens after the call.
[00:00:18] ch1: That is where I am stuck too. We can make a polished brief, but a polished brief is not automatically something the team will use.
[00:00:33] ch0: Right. The product claim cannot just be that it summarizes accurately.
[00:00:39] ch1: Especially because the team already has accurate information in project notes, decision records, and the issue tracker. Another summary can still be another place to check.

[00:02:41] memo: adoption problem is the handoff, not capture
[00:02:48] ch0: So the pilot should start at the return to work. The question is whether someone can reopen the project two days later and continue without reconstructing the call.
[00:03:02] ch1: Yes. I would rather see one paragraph land in the right project note than receive a beautiful six-section recap in email.
[00:03:17] ch0: What makes the paragraph usable rather than merely shorter?
[00:03:22] ch1: It needs the decision, the unresolved boundary, and the thread it changed. If it strips those apart, the reader still has to rebuild the reasoning.

[00:05:04] ch0: The current design writes a meeting note and links Atlas on first mention. We have not decided whether it should also update the project page.
[00:05:15] ch1: I would not automate the project-page edit in the pilot. A wrong write into the canonical page costs more trust than a correct meeting note earns.
[00:05:27] ch0: So meeting note first, explicit review before any roll-up.
[00:05:32] ch1: Exactly. Preserve the source note and make the proposed project update inspectable.

[00:08:18] memo: workspace has to remain canonical
[00:08:24] ch0: The workspace should stay the source of truth. We can suggest a roll-up, but we should not create a parallel product surface that quietly becomes canonical.
[00:08:39] ch1: And the link has to work in both directions. From the project page I need to reach the meeting evidence, and from the meeting note I need to see what project thread it belongs to.
[00:08:53] ch0: We can guarantee the meeting-to-project link now. The reverse link depends on the vault or on an approved update.
[00:09:05] ch1: Then call that boundary out. Do not imply the project page was updated when all we wrote was the meeting note.

[00:11:14] ch0: For the first pilot, I propose five design-partner calls, each ending with a connected meeting note in their existing workspace.
[00:11:25] ch1: Five is enough if we observe what happens later. Same-day reactions will overrate the quality because everyone still remembers the call.
[00:11:39] ch0: We should check after forty-eight hours whether they use the note to resume the work.
[00:11:46] ch1: And whether they follow the link into the project context instead of searching Slack or asking someone to retell the decision.

[00:14:09] ch1: My main concern is the digest you showed. If Atlas gets a fresh digest every Friday, people may start forwarding that instead of maintaining the project note.
[00:14:25] ch0: Even if every digest is correct when generated.
[00:14:29] ch1: Yes. It freezes a living thread into a snapshot, then the snapshot becomes a second source of truth. Two weeks later nobody knows which version carries the real decision.
[00:14:32] memo: same objection rui raised in march
[00:14:48] ch0: That is the strongest argument against shipping the digest as the primary artifact. The meeting note should join the workspace and point back to the project thread; a digest can only be a view over those sources.
[00:15:05] ch1: I agree. If the digest cannot show where each claim came from, leave it out of the pilot.

[00:17:46] memo: pilot should test return-to-work, not summaries sent
[00:17:52] ch0: Then our pilot measure is not summaries generated or sent. It is successful return-to-work after forty-eight hours, with the source trail intact.
[00:18:04] ch1: Define successful. I would count it when the person can name the decision and the open boundary, then reach the supporting meeting note without asking for a recap.
[00:18:20] ch0: I will rewrite the pilot scorecard around that and remove the weekly digest from the primary flow.
[00:18:28] ch1: I will recruit five design partners who already keep project notes, not teams looking for a new dashboard.
[00:18:41] ch0: We still need to decide whether proposed project-page updates are part of this pilot or a follow-on.
[00:18:48] ch1: Keep that open until we see whether the connected meeting note is enough.
