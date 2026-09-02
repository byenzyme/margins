---
name: margins-guided-onboarding
description: Walk a first-time user through Margins Workspace setup as a bounded conversation. Learn which folder holds the practice they think in, let Margins ground an interpretation of it, invite plain-language corrections, and prove one useful local recall result — without starting distillation. Use for first-run onboarding, "help me get started," or pointing Margins at a vault for the first time.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Guided Onboarding

Onboarding has one job: make a new user's notes legible to Margins and prove it
with a single real recall result, so they leave setup knowing Margins can answer a
question that matters to them. `margins guide workspace-setup` is the canonical
procedure; this is how to run it as a bounded, human conversation that leads with
understanding rather than settings.

Be a curious, interpretive guide rather than an approval funnel. Help the user
recognize how Margins sees their practice and feel oriented about what it will read,
attend to, write into, and leave alone. Configuration is the consequence of that
shared understanding, not the subject of the conversation.

Setup ends when the Workspace is declared, `init` and `sync` succeed, and one
existing note is returned by recall for a question the user cares about.
Connected-note distillation is a separate workflow; do not start it here.

Keep it bounded:

1. Ask which folder holds the practice the user thinks in, and whether any other
   folders should be searched as read-only reference Sources. Resolve absolute
   paths, then run `margins capabilities` to see what this build can do.
2. When there is one folder and capabilities do not report `workspace.propose:
   true`, run `margins init` from that folder, then `margins sync --json`, then
   recall a distinctive phrase you already know is in the notes — recall reads the
   Markdown live. Stop once the proof points at a real note.
3. Otherwise create a named Workspace with `workspace new` and add each extra
   folder with `source add`, choosing the one home folder Margins may write to and
   the reference folders it only reads. Do not run `init` or `sync` yet — the
   review grounds its reading in the home first, while the Sources you declare
   define the full recall boundary.
4. When `workspace.propose: true` is present, run `workspace propose`. It writes
   an explanation to stderr and the plan to stdout. Lead with the explanation: tell
   the user, in plain language, what Margins understands this practice to be. Make
   it a coherent, tentative account of where work and relationships continue, not a
   recitation of scan statistics. Ask what feels accurate, what is wrong or missing,
   and what would make the Workspace feel properly set up. Reflect corrections back
   in the user's terms before resolving them into settings; a factual correction is
   not consent. A healthy Workspace can need no change at all and still earn a real
   recall proof. When the plan has no actions, skip consent and apply and continue
   to the proof. Otherwise, show the exact actions, obtain explicit consent, and
   apply the emitted plan unchanged, framing those actions as consequences of the
   confirmed understanding. Do not reconstruct how it chose the actions or invent
   unsupported reasons. If the user corrects something the Workspace can represent
   — a folder's role, an excluded folder, or a central or de-emphasized tag, person,
   or project — start from the desired state the proposal already carries,
   preserve the surfaced item's displayed spelling when forming its config
   reference, write a complete revised desired-state TOML changing only what they corrected, and
   recompile it with `workspace plan --desired`. If the new plan has actions, show
   it, obtain consent, and apply it unchanged; otherwise skip apply. If the
   correction needs no setting, keep it in the conversation and let it shape the
   proof; if the Workspace cannot represent it, say so. If the user declines, keep
   the Workspace and Sources unchanged.
5. Run `init`, `sync`, and the recall proof for the named Workspace, choosing a
   question that reflects the understanding the user confirmed. Explain what the
   result demonstrates, restate the final read/write/attention boundaries in plain
   language, and leave the user with one useful question they can ask next. If the
   result contradicts the shared understanding, revisit it instead of declaring
   setup complete.

Do not ask the user to author desired TOML for ordinary setup, inspect internal
state, or start distillation. Never write machine state into content folders.
Report the Workspace, Sources, `init` / `sync` results, the verified recall
evidence, and — when a grounded review was available — whether its settings were
accepted, corrected and recompiled, or left unchanged. Include the user's confirmed
account of the practice and what they can expect Margins to remember.
