---
name: watermark
description: Give quick, useful feedback during a live Margins meeting using the conversation passed in by @Margins. Use when the user asks for the latest read, a sanity check, what to say next, how to steer, or what point to raise.
argument-hint: <free-form question or request>
user-invocable: true
---

# Watermark

Answer a time-sensitive question about the meeting Margins is recording now.
This is live feedback, not post-meeting distillation.

Use `@Margins` context when it is present. Treat it as the freshest view from
the recording Mac. It may include recent transcript lines,
notes the user jotted down, capture state, and transcript freshness. Do not search old notes,
read Margins files, or delegate before giving the first useful answer.

If `@Margins` context is missing, ask the user to add `@Margins` or open the
Margins live panel. Do not guess from bb chat history as if it were the meeting.

Lead with the useful read. Keep the answer short enough to use while the meeting
is happening. Say plainly when recording is paused, transcript text is stale, or
newest words may still change.

Prefer this shape when it fits:

```markdown
**Watermark:** one sentence on what changed.

Direct answer in 1-3 sentences.

**Say:** One concise sentence the user can say next.

**Watch:** One risk, opening, or decision that may otherwise pass.
```

Adapt the shape to the user request. Give one primary move, not a menu.
