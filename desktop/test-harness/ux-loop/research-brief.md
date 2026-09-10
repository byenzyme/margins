# Margins UX loop research brief

This is the baseline brief for the hybrid UX E2E loop. The research agent should
refresh it from Obsidian/product notes before a serious run, but this file gives
the loop a stable starting point.

## Product posture

Margins should feel like a calm Obsidian-native meeting notebook. It is used while
the user is cognitively occupied, so the app should create confidence without
demanding attention.

The product should not feel like:

- a developer console;
- a SaaS admin dashboard;
- an excited AI copilot;
- a transcript processing tool;
- a generic chat app.

## User mindsets

### About to join a meeting

The user has seconds. They need to know:

- capture is ready;
- microphone and computer audio are ready or clearly blocked;
- the primary action is obvious;
- setup work is one-time and not scary.

Too much information:

- model names;
- backend/provider terms;
- long explanations;
- optional advanced settings styled like required setup.

### In the meeting

The user is trying to stay present. Margins should stay quiet.

They need:

- unmistakable recording state;
- glanceable audio health;
- a low-friction memo/mark path;
- warnings that are calm, actionable, and rare.

Too much information:

- paragraphs during recording;
- provenance/debug details;
- assistant nudges that compete with the person speaking;
- modal interruptions unless capture is truly compromised.

### Recovering from trouble

The user is anxious about losing the recording.

They need:

- what is still saved;
- what is broken;
- what to do next;
- whether they can continue with degraded capture.

Too much information:

- raw permission names without user outcome;
- backend errors before the saved-state reassurance;
- ambiguous retry buttons.

### Post-meeting reviewer

The user is tired and wants a useful note.

They need:

- a clear next action;
- visible progress quickly;
- evidence that memo, transcript, and vault context are being used;
- a final note that reads like an Obsidian note, not a chat transcript;
- a refinement path that does not reset context.

Too much information:

- the trace dominating the note;
- every transcript detail presented as equally important;
- implementation language like `process`, `synthesize`, `ch0`, `ENTRY`, `cleanup`.

### Skeptical/power user

The user wants proof without clutter.

They need:

- inspectable provenance;
- trace steps grouped into human-readable phases;
- source evidence available on demand;
- generated note and saved path as the primary artifact.

Too much information:

- always-open logs;
- raw tool names as primary labels;
- provenance cards that compete with the note.

## Evaluation questions

- Does the screen make the current app state obvious?
- Is the next action obvious?
- Does the UI explain waiting before it feels broken?
- Does the note remain the primary artifact?
- Can provenance be inspected without cluttering the main path?
- Does the user know what was saved if something fails?
- Does refinement feel like continuing the same note-making conversation?

