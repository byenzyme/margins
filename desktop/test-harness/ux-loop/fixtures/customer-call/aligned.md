# Aligned timeline

[00:00:04] ch0: Thanks for making time. I wanted to understand whether Margins would fit the way your team already reviews calls, not pitch another meeting recorder.
[00:00:17] ch1: That is the right place to start. We already have recordings in a few places, and the failure is that nobody wants to turn those into the actual follow-up note.
[00:00:39] ch0: So the job is not recording. It is getting from conversation to a note that somebody trusts enough to send or file.
[00:00:51] ch1: Exactly. If your product starts by asking me to manage another dashboard, I am probably out.

[00:01:12] memo: customer worried onboarding will become another admin workflow
[00:01:18] ch0: What would make onboarding feel like setup rather than administration?
[00:01:28] ch1: I need to know where notes go, whether the audio is actually being captured, and whether I have to babysit the AI. I do not need to choose ten model settings.
[00:01:46] ch0: That suggests the first screen should answer readiness and destination, not expose the whole pipeline.
[00:01:57] ch1: Yes. If someone sees the word pipeline before the first useful note, that is already too much.

[00:03:12] ch0: When you finish a call, what do you expect to happen?
[00:03:18] ch1: There should be an immediate sense that the app has the recording and is doing something with it. Even if the AI takes a while, I need evidence that it is not stuck.
[00:03:36] ch0: What kind of evidence?
[00:03:40] ch1: Not logs. Something like reading memo marks, checking transcript, looking through related notes. It should map to my mental model of note-making.
[00:03:58] ch0: So progress should be semantic, not technical.
[00:04:02] ch1: Right. I do not care that a tool was called. I care that the tool found something relevant.

[00:06:48] memo: proof needs to be the note itself, not a dashboard
[00:06:55] ch0: Some products put the processing trace front and center to prove work happened. Would that help?
[00:07:05] ch1: Only after the note is readable. The proof is whether the note sounds like the meeting and remembers the marked moments. The trace is for when I am skeptical.
[00:07:24] ch0: So provenance should be available, but not the main artifact.
[00:07:30] ch1: Exactly. I want a note, not a cockpit.

[00:09:10] ch0: What would make the first generated note feel wrong?
[00:09:15] ch1: If it flattened the conversation into generic bullets. Or if it over-indexed on my quick notes and missed what the other person contributed.
[00:09:31] ch0: Your team wants both sides represented.
[00:09:35] ch1: Yes. If Marcus explains the constraint and I only see my own framing, I cannot trust the note.
[00:09:49] ch0: That is useful. The app should show that memo is an attention signal, not the whole source.
[00:09:58] ch1: Exactly.

[00:12:22] ch0: Let's talk pilot. What would the smallest useful pilot look like?
[00:12:30] ch1: Three people, two weeks, maybe eight to twelve calls. We would start with customer discovery and internal planning, not sales calls yet.
[00:12:48] ch0: Why not sales calls?
[00:12:51] ch1: Higher risk. We need to trust the note quality and privacy posture first.
[00:13:05] ch0: What would you need from us?
[00:13:08] ch1: A clear setup checklist, permission language we can forward internally, and two sample notes from realistic transcripts.

[00:14:05] memo: action items: pilot scope, permissions, sample recordings
[00:14:12] ch0: So action items are: I send the pilot scope, the permission language, and two sample recordings or notes.
[00:14:21] ch1: And I will find two people who actually feel the pain. Do not optimize for the teammate who already loves tooling.
[00:14:35] ch0: Good constraint. We need skeptical but motivated users.
[00:14:41] ch1: Right. The best user is someone who wants the note but does not want to configure the system.

[00:17:03] ch0: How should the app talk during processing?
[00:17:08] ch1: Calmly. The language should be human. "Finding related notes" is fine. "Running semantic search over indexed corpus" is not.
[00:17:22] ch0: What about "Distill"?
[00:17:26] ch1: Internally fine, but user-facing maybe "Make connected note" or "Create note." Distill sounds like a feature name.
[00:17:43] ch0: What about "Backchannel"?
[00:17:47] ch1: I like it if it is explained by the screen. It sounds like where the supporting evidence lives.

[00:20:18] ch0: When would there be too much information?
[00:20:22] ch1: During the meeting, almost anything beyond recording state and a place to mark is too much. After the meeting, more information is okay, but it should be layered.
[00:20:39] ch0: What does layered mean?
[00:20:42] ch1: First the note. Then a compact "how this was generated." Then details if I open them.
[00:20:55] ch0: So progressive disclosure.
[00:20:58] ch1: Yes, but without making the important status invisible.

[00:22:31] memo: open question: how much provenance is enough without clutter
[00:22:38] ch0: How much provenance would be enough for you?
[00:22:43] ch1: I want to know it used memo, transcript, and my notes. I do not need every retrieved note unless I ask.
[00:22:59] ch0: If it says "found five related notes," is that enough?
[00:23:05] ch1: Maybe if I can open the list. It should show titles or themes, not raw IDs.
[00:23:19] ch0: And if the search fails?
[00:23:22] ch1: Say that plainly and still make a transcript-derived note. Do not make it feel like the whole run failed.

[00:26:11] ch0: What would refinement look like after the note?
[00:26:16] ch1: I would ask for a shorter version, sharper actions, or more explicit open questions. It should revise the existing note, not start a new conversation.
[00:26:34] ch0: Should the old note remain visible?
[00:26:37] ch1: Yes, or at least the current note should not disappear. I need confidence I am editing the artifact I just reviewed.
[00:26:52] ch0: What if refinement takes another thirty seconds?
[00:26:57] ch1: Show that it is applying my instruction. The first feedback should be immediate.

[00:31:10] memo: they liked the idea of refinement after the first note
[00:31:16] ch0: It sounds like refinement is not a power-user feature. It is part of trusting the first draft.
[00:31:24] ch1: Yes. I do not expect the first note to be perfect. I expect the product to make the second pass easy.
[00:31:39] ch0: Would you use chat-style refinement?
[00:31:43] ch1: Maybe, but do not turn the whole app into a chat screen. The note should remain the object.

[00:34:08] ch0: What should the pilot measure?
[00:34:11] ch1: Whether people use the generated note without rewriting it from scratch. Also whether they can recover if something feels wrong.
[00:34:25] ch0: So adoption is less about recording count and more about note trust.
[00:34:31] ch1: Exactly. Did it save me the annoying synthesis work? Did I trust it enough to send or file?

[00:38:44] memo: follow up with concise two-minute review version
[00:38:50] ch0: I will send a concise pilot plan with the permission copy and two sample notes.
[00:38:57] ch1: Keep it short. Two-minute review. If it takes fifteen minutes to understand the pilot, that is the same problem the app is supposed to solve.
[00:39:12] ch0: That is a good forcing function.
[00:39:15] ch1: And include the failure states. I want to see how it handles missing system audio or a weak transcript, not just the happy path.
[00:39:33] ch0: That is fair. We will include recovery examples.

[00:40:05] ch1: One more thing. If the product can say "your mic audio is saved, but computer audio needs attention," that is much better than a generic error.
[00:40:17] ch0: Saved state first, technical problem second.
[00:40:21] ch1: Exactly.

