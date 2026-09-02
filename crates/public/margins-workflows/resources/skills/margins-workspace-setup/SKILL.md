---
name: margins-workspace-setup
description: Set up a Margins Workspace so the notes, thinking, and relationships of one practice become legible to recall. Declare the folders that hold that practice — for one person or a shared team vault — let Margins show back what it understands about them, invite plain-language corrections, persist only the settings needed to keep that understanding true, and prove it with one live local recall result. Use whenever someone wants to point Margins at their notes, add or change which folders it reads, or confirm that setup actually worked.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Workspace Setup

Setup exists to make one knowledge practice legible to Margins: to show where the
work, thinking, and relationships accumulate, to surface the consequential
questions Margins can already answer from that material, and to persist only the
minimum settings needed to keep that understanding true. The practice may belong to
one person or a shared team vault. It ends by proving one of those questions with a
live local recall result over the practice's own notes.

Act as a reflective setup guide, not a configuration closer. The user should leave
with a recognizable account of their practice and confidence about what Margins will
read, attend to, write into, and leave alone. A mechanically valid config is not, by
itself, a successful setup.

Setup is not distillation. Distillation turns a transcript or memo into a
connected note once a Workspace exists; that is a separate workflow.

Hold this mental model as you work, because every command below serves it:

- A **Workspace** is the durable read / write / attention boundary for a single
  practice: the one home folder Margins may write approved notes into, the
  reference folders it may read, and what it should ignore.
- **propose** is a read-only grounded review — Margins grounds an initial reading
  in the Workspace home and tells you what it understands the practice to be, with
  a draft of the settings that reading implies. Your Source declarations still
  define the full recall boundary.
- **plan** and **apply** are the exact consent seam: a plan is the precise list of
  setting changes, and apply commits only the plan the user actually saw.
- **init** and **sync** materialize the Workspace so recall can run over it.
- **recall** proves a question the interpretation promised, using the user's real
  notes.

All machine state lives under the Margins home in `workspaces/<id>/`. Never create
`.margins` inside a notes folder, and never rewrite existing notes during setup —
the practice on disk belongs to the user, not to us.

## Start from the practice, not the settings

Before touching config, learn what the user is actually keeping and why. Which
folder holds the notes they think in? Are there other folders — research, a shared
vault, an archive — they want searched but never written to? One folder becomes
the Workspace home; any others are read-only reference Sources.

Listen for the user's own names for the work, the relationships that matter, and
what they hope Margins will help them remember. Form a small, tentative reading of
the practice rather than a taxonomy: where work accumulates, how it changes over
time, which projects or relationships continue across notes, and what looks
incidental. The goal is recognition, not an impressive-sounding analysis.

## Check what this build can do

Run:

```bash
margins capabilities
```

If the report includes `workspace.propose: true`, this build can ground an
interpretation in the Workspace home before initialization. That grounded review is
the centerpiece of setup, so use the named-Workspace path below. Otherwise, declare
the folder directly.

## Declare one folder directly

When there is a single notes folder and no `workspace.propose`, the Workspace is
that folder. Run these commands from the notes folder:

```bash
cd "/absolute/path/to/notes"
margins init
margins sync --json
margins recall "an exact phrase from these notes"
margins workspace status --json
```

`init` adopts the current directory as the Workspace home. `sync` refreshes the
declared Sources so recall is current; recall itself reads the Markdown live at
query time. Be precise about this proof: an exact-phrase result pointing to an
existing note proves that the Source is inside the recall boundary and setup can
retrieve it. It does not by itself prove a broader interpretation of the
practice. `status --json` reports the Workspace id and its state path.

## Declare a named Workspace and its Sources

Use this path when the user has more than one folder, or when `workspace.propose:
true` is present. Name the practice, set its home, and add each reference folder.
Do not run `init` or `sync` yet — the grounded review comes next, against the
Workspace you just declared:

```bash
margins workspace new practice --home "/absolute/path/to/notes"
margins --workspace practice source add notes \
  --name research --role reference --path "/absolute/path/to/research"
margins --workspace practice source list --json
```

The home is the one folder where approved notes may be written; reference Sources
are searched but never modified. That read / write boundary is the durable part of
the Workspace.

## Lead with understanding, then review settings

Only when `margins capabilities` reports `workspace.propose: true`:

```bash
margins --workspace practice workspace propose \
  --json > /tmp/margins-workspace-plan.json
```

The command writes its explanation to stderr and the `margins.workspace.plan.v1`
plan to stdout. The explanation — what Margins understands this practice to be, and
the consequential questions it can answer — is the centerpiece; the plan is a draft
of the settings that understanding implies.

Present the understanding first. Keep the explanation's evidence faithful — the
folders, notes, and connections it actually cites — before restating it in the
user's own vocabulary. Then ask, in ordinary language, what is wrong or missing —
not "approve these settings," but "does this match how you work, and what did it
miss?" A healthy Workspace often needs no setting change at all and still deserves
the richest interpretation and a real recall proof.

Do not merely recite scan findings or turn the first user response into a config
decision. Help the user see a coherent picture: what this practice appears to be,
where its continuity lives, what Margins could help them follow, and where the
reading is uncertain. Use tentative language when the evidence is ambiguous. When
the user corrects that picture, reflect the revised understanding back in their
terms and make sure it now feels accurate before deriving settings from it.

Only after the user has reacted to the understanding do you turn to the concrete
plan. A factual correction is not automatically consent. First confirm that the
user recognizes the revised account and understands the resulting attention and
read/write boundaries. If `actions` is empty, tell the user that no settings need
to change, skip consent and `workspace apply`, and continue to the recall proof.
Otherwise, show the exact plan actions and ask for explicit consent, framing those
actions as the settings consequence of the confirmed understanding. Do not
reconstruct unsupported reasons for how the command chose an action. If the user
consents to the plan as shown, apply the saved plan unchanged:

```bash
margins --workspace practice workspace apply \
  --plan /tmp/margins-workspace-plan.json \
  --if-revision "<base_revision from the plan>" \
  --request-id "<unique setup request id>" --json
```

`--if-revision` refuses to apply if the Workspace changed underneath you;
`--request-id` makes the apply idempotent. The plan you apply is always the exact
plan the user saw.

### When the user corrects a supported Source or policy

Supported corrections are anything the Workspace config can represent: a folder in
the wrong role, a folder that should be excluded, a reference that should be the
home, or policy — an excluded folder or tag, or a surfaced folder, tag, person,
project, or running log made central (pinned) or kept out of attention (excluded).
Use only an item the review surfaced and preserve its displayed spelling when
forming the config reference: `folder:<displayed path>`, `#<displayed tag>`,
`[[<displayed linked name>]]`, or `log:<displayed log name>`. Never infer a new
entity. Do not hand-edit the plan JSON. The plan carries the full desired state it
would produce; start from that desired state, write a complete desired-state TOML,
and change only what the user corrected.

```bash
margins --workspace practice workspace plan \
  --desired /tmp/margins-workspace-desired.toml --json \
  > /tmp/margins-workspace-plan.json
```

This compiles a fresh, exact plan from the user's correction. If the new plan has no
actions, report that no settings need to change and skip apply. Otherwise, present
that new plan, get explicit consent, and apply it unchanged with the same
`--if-revision` / `--request-id` guards. Apply is refused unless the plan still
matches the current Workspace, so the plan you apply is always the plan the user
last saw — never JSON you edited by hand.

If the correction is descriptive but implies no runtime setting — how the user
thinks about the material, what a folder is really for — keep it in the interaction
rather than inventing config, and let it shape the recall phrase you choose to prove
setup. If the correction asks for something the Workspace cannot represent — a
folder that is neither home nor reference, a rule the policy fields don't cover —
say so plainly instead of manufacturing a setting to stand in for it.

## Initialize and prove

Once any review is settled, materialize the named Workspace and prove a real
question:

```bash
margins --workspace practice init
margins --workspace practice sync --json
margins --workspace practice recall "<question from the confirmed understanding>"
margins --workspace practice recall "<distinctive phrase from research>" --source research
```

When a grounded review supplied a question, use that question first and verify that
the result cites a real note supporting the answer. Use an exact phrase as the
source-boundary check, especially for each reference Source. Keep the claims
separate: the phrase proves reachability; the grounded question tests the
understanding the user confirmed.

Close the loop in plain language. Explain what the result demonstrates about the
practice, remind the user what Margins will attend to and leave alone, and name one
useful question they can now return with. If the result does not support the shared
understanding, say so and revisit the interpretation instead of declaring setup
complete.

## Report

Report the Workspace id and state path, the declared local Sources, the `init` /
`sync` results, at least one source-backed recall result, and — when a grounded
review was available — whether its suggested settings were accepted, corrected and
recompiled, or left unchanged. Also summarize the user's confirmed understanding of
the practice and the resulting read/write/attention boundaries. Confirm the notes
folders were untouched and hold no `.margins` state.
Do not begin connected-note distillation as part of setup.

## Automation with an existing desired config

When automation already holds a complete desired Workspace config, `workspace plan
--desired ... --json` compiles the reviewable `margins.workspace.plan.v1`, and
`workspace apply` commits it with its base revision and a unique request id. Never
make a person author desired-state TOML just to complete ordinary setup — that is
the machine's job, reserved here for recompiling a plan from a correction.
