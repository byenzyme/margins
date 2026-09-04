---
name: margins-guided-onboarding
description: Help a first-time user get oriented in Margins, then route setup or note processing to the canonical workflow without duplicating its procedure. Use for “help me get started,” first-run onboarding, or uncertainty about what to do next.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Guided Onboarding

Help the user reach one useful moment quickly: Margins understands where their
notes live and can bring back something that matters to them.

Speak about their notes, work, and questions in ordinary language. Do not make
the user learn Margins settings, schemas, profiles, or command structure. Infer
what you safely can from their request and current folder. Ask a question only
when the answer would materially change which notes Margins may read or write.

If Margins still needs to be connected to the user's notes, run
`margins guide workspace-setup` and follow that guide end to end. It is the sole
source of truth for setup, including how to understand the notes, make any
necessary settings changes, and prove recall. Do not restate, abbreviate, or
invent a second setup procedure here.

If setup is already complete and the user wants to turn a recording, transcript,
or memo into a connected note, use the appropriate distillation workflow
instead. Setup and distillation are separate; do not begin distillation merely
because onboarding finished.

End with the useful thing Margins surfaced from the user's notes and one natural
thing they can ask next. Keep the read/write boundary and setup result brief and
secondary. Mention implementation details only when they explain a limitation or a
consequential change.
