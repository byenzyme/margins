---
name: margins-workspace-setup
description: Set up a Margins Workspace for one practice's notes. Point Margins at the notes folder, start it from the Margins meetings preset (meeting notes, people, projects), review and apply the resulting program, index it, and prove it with one live exact-phrase recall result. Use whenever someone wants to point Margins at their notes, add or change which folders it reads, or confirm that setup actually worked.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Workspace Setup

Help the user point Margins at one practice's notes. Setup should leave them with:

- a clear boundary around what Margins may read, write into, and ignore;
- a Workspace program, started from the Margins meetings preset, that they have
  seen and know how to edit; and
- one real example showing that Margins can find an existing note.

Talk to the user in the language of their work. Configuration and command output
are supporting evidence, not the subject of the conversation.

Setup is not connected-note distillation. Do not transcribe a meeting, process a
session, or draft a note as part of this workflow.

## A small mental model

- A **Workspace** represents one practice. Its home is the only notes folder where
  Margins may write an approved note. Reference Sources may be searched but not
  modified.
- Each Workspace is one **program**, `$MARGINS_HOME/configs/<id>.enzyme`
  (`margins --workspace <id> workspace show` prints its path and
  `workspace show --text` its text). It says which folders Margins reads, where
  it writes notes, and what it leaves out. `margins guide glossary` explains
  these words for the user.
- Setup starts every program from the **Margins meetings preset**: notes go to
  `Meetings`, `Templates` and `Attachments` are left out, and Margins learns
  questions from `Meetings` (open work and follow-ups), `People` (relationships,
  including linked pages), and `Projects` (decisions). Readings for folders the
  notes folder does not have are dropped; folder names match case-insensitively.
  The preset also says `learn questions automatically`: besides its readings,
  Margins picks what they miss, chosen again every time it indexes. A program
  without that statement attends to exactly its readings; one with no readings
  leaves attention fully automatic. Running setup again adds the statement to
  a preset-made program that lacks it and changes nothing else.
- **Plan** turns the preset into an exact, reviewable list of changes. **Apply**
  commits only that plan.
- **Init** and **sync** prepare recall. **Recall** proves that the notes folder is
  reachable.
- Connected services, including Granola, are two-step: first authorize the
  machine connection, then bind the account as a typed Workspace Source before
  sync. A successful connection alone does not expand the Workspace boundary.

All Margins state belongs under its own `workspaces/<id>/` state directory. Never
create `.margins` inside a notes folder or rewrite the user's existing notes during
setup.

Never open, cat, print, or summarize credential bundles, auth files, Keychain
items, shell history, dotenv contents, token stores, cookie stores, or environment
variable values during setup. Use only redacted Margins product status and
capability reports when checking authorization or connection state.

## 1. Choose the notes folder

Resolve the notes folder from the user's request and any existing named Workspace
first. Ask which folder holds the notes only when the writable home is genuinely
ambiguous. Ask about other searchable folders—such as research, a shared vault, or
an archive—only when the user mentions them or the requested change requires that
decision.

Then check the available capabilities:

```bash
margins capabilities
```

`margins capabilities` is supported and should stay in this workflow. Do not run
unsupported discovery commands; use only commands documented in this guide or
confirmed by `margins capabilities`, and treat redacted product status as the
boundary for auth-related checks. Use the redacted `catalyst` object only as a
readiness signal; if catalyst setup is needed, do it immediately before `init` as
described below.

Treat memories or transcripts from earlier setup attempts as hypotheses, never as
evidence that the installed build still has the same behavior. Establish current
behavior from this Workspace's plan, init, sync, and recall results.

If the report includes `workspace.preset: true`, use the named Workspace path
below. For one notes folder on a build without it, use the direct path.

## 2. Direct setup when the preset is unavailable

Run these commands from the notes folder:

```bash
cd "/absolute/path/to/notes"
margins init
margins sync --json
margins recall "an exact phrase from these notes"
margins workspace status --json
```

Do not run recall before `margins init` has completed for this Workspace.
An exact-phrase result pointing to an existing note proves that the folder is
inside the recall boundary. Report that plainly.

## 3. Declare the Workspace and its Sources

Create the Workspace with its one writable home (skip this when a Workspace for
that folder already exists), and add each read-only reference Source:

```bash
margins workspace new practice --home "/absolute/path/to/notes"
margins --workspace practice source add notes \
  --name research --role reference --path "/absolute/path/to/research"
margins --workspace practice source list --json
```

Source declarations define the full recall boundary.

If the user wants Granola in this Workspace, keep it on the same Source path
rather than switching to file import:

```bash
margins connect status --service granola --json
margins connect granola --account <account-email>
margins --workspace practice source add granola \
  --name granola --account <account-email> --time-range last_30_days
margins --workspace practice source list --json
```

The status output should show the account as authorized before binding or sync.
After binding, `sync`/reconcile fetches the connected account through the Granola
connector. `margins import granola <export.json-or-csv>` is only for an offline
export file the user already has; do not present it as the continuation for a
successful OAuth/MCP connection.

## 4. Start from the preset and review the plan

```bash
margins --workspace practice workspace plan --preset margins-meetings --json \
  > /tmp/margins-workspace-plan.json
```

The plan's `preset` object lists the `readings` it kept, the
`skipped_readings` whose folders the notes folder does not have, and the
`note_folder` new notes go to; `program_path` is the program file. `actions`
has one plain-language `summary` per change and `diff` is the exact program
change. The preset only adds to the program, so running setup again on a set-up
Workspace changes nothing; its note folder, including a chosen Home root, is
kept.

**Show the plan before you apply it.** Before running `workspace apply`, write
the user a message, in their terms, with the plan's plain-language consequences:

- the folders Margins will learn from (the `preset.readings` it kept);
- the preset folders it skipped because the notes folder does not have them
  (`preset.skipped_readings`);
- what it leaves out; and
- where new notes will go (`preset.note_folder`) and where the program lives
  (`program_path`).

The consequences must reach the user before the apply command runs; reporting
them only in the final summary, after apply, is not showing the plan.
If `actions` is empty, explain that nothing needs to change and skip apply.
Otherwise, once that message is out, apply the saved plan unchanged; you need
not wait for a reply. The opening setup request authorizes this preset, so after the user has
seen its consequences do not ask for a second “apply this plan” confirmation.

```bash
margins --workspace practice workspace apply \
  --plan /tmp/margins-workspace-plan.json \
  --json
```

Apply derives its revision check and retry identity from the plan. It rejects a
stale or altered plan and commits atomically. Never hand-edit plan JSON.
Without `--json`, `workspace plan` prints the same consequences in plain
language and saves the plan for `workspace apply --plan`, and `workspace apply`
prints what changed; that is the form a person at a terminal uses.

If the user wants something different before apply—notes in another folder, a
folder left out, a reading removed—apply the preset first, then change the program
as described in "Refining the program". Do not create folders, move or rewrite
existing notes, or invent tags and links during setup.

## 5. Initialize and prove the setup

Check the redacted catalyst status from `margins capabilities` again. If it reports
`usable = false`, run this once before `init`:

```bash
margins setup --only catalyst
```

This is the provisioning boundary: it may mint or refresh a short-lived hosted
lease, or prepare the configured local fallback. Never inspect its credential
files to diagnose it. If setup still reports no usable catalyst, stop and report
that readiness failure; do not run `init` repeatedly and do not install a different
fallback policy without a later explicit request.

Provisioning can also persist a machine-level catalyst mode outside the Workspace.
If you run `margins setup`, say so in the final state-change summary. Do not claim
that only the Workspace state changed when setup created or changed machine-level
Margins configuration.

If `init` fails after an applied plan, preserve that program while you report and
diagnose the failure. Do not remove readings or try another policy as a diagnostic
shortcut. An `indexed` or `live_lexical` status only confirms that an index
exists; it does not establish that the recall command is usable when catalyst
readiness is still pending.

Treat `workspace status`'s `live_lexical` document count as portable live Markdown
coverage, not as the official semantic index size. Never relabel that number as
“indexed documents.” In an official runtime, `mode = "indexed"` reports the
persisted engine index count and may be described that way. A recall response's
`note_count` is the exact number of documents searched by that recall call.

Then initialize and prove recall in this order:

```bash
margins --workspace practice init
margins --workspace practice sync --json
margins --workspace practice recall "<distinctive phrase from a note>"
margins --workspace practice recall "<distinctive phrase from research>" --source research
```

Copy one contiguous, verbatim phrase of roughly 5–10 words from the body of a real
note in each Source you prove; do not substitute a title, filename, or bag of
related keywords. Verify that the result cites the note containing it. A nearby
semantic result is not an exact-phrase boundary proof: if the phrase does not
retrieve its note, report that proof as incomplete instead of silently
substituting a different query or describing semantic recall as equivalent.

## 6. Hand off

Lead with what Margins can now do for the user, then give the operational receipt:
the Workspace and its read/write boundary, the folders it learns from and the
preset folders it skipped, where new notes go, whether machine-level catalyst
setup changed, whether `init` and `sync` succeeded, the recall proof, and
confirmation that notes were not modified. Always tell the user where the program
lives (`program_path`, or `margins --workspace <id> workspace show`) and that they
can change it with `margins --workspace <id> workspace edit`. Exact state paths
other than the program, revision hashes, similarity scores, and profile names are
optional; include them only when the user asks or they explain a consequential
limitation.

Do not begin connected-note distillation as part of setup.

## Refining the program

Refinement is optional and never a setup step. In a terminal,
`margins --workspace practice workspace edit` opens the program in `$EDITOR`, then
shows the plan and applies it after confirmation. An agent changes the program by
copying the complete program from `program_path`, changing only the requested
statement, keeping every other statement exactly as written, and planning it:

```bash
margins --workspace practice workspace plan \
  --desired /tmp/margins-workspace-desired.enzyme --json \
  > /tmp/margins-workspace-plan.json
```

Show the user the plan's user-visible consequences before applying it, then
apply that plan unchanged. An agent that
wants evidence about the notes before proposing a change may read the engine's own
read-only inventory, `margins enzyme scan --workspace <id> --json`. Margins ships
its own `enzyme`, separate from any `enzyme` the user installed, and
`margins enzyme` runs that bundled engine on the Margins home (never
`~/.enzyme`). The Knowledge Practice Review Contract below applies to that
optional review only.

An entity is an existing thread—such as a folder, tag, linked note, or running
log—that Margins can build recall catalysts around. In the program each one is a
`learn questions from …` reading. The readings form the exact attention surface;
with none, attention is automatic. A reading may add `about <profile>` or, for a folder,
`including linked pages`:

```enzyme
workspace "practice" {
  source markdown "home" { path "/Users/me/notes" }

  learn questions from tag "enzyme"
  learn questions from folder "People"
    including linked pages
    about relationships
  leave out folders ["Templates", "Attachments"]

  remember in folder "Meetings" create note
}
```

When the Workspace declares more than one Markdown source, folder readings start
with the source name (`folder "home/People"`) and the one
`remember in folder … create note` names its source (`in source "home"`).
`remember in folder` is where Margins writes notes (`"."` is the source root);
a Workspace has exactly one. Exclusions are `leave out folders|tags|links [...]`.
A folder reading must name a folder that exists under the notes folder.

A catalyst profile changes the kinds of questions Margins develops for an entity;
it is not a weight or an importance score. Offer a profile only when the notes make
the posture clear:

| Profile | Use when the notes primarily preserve… |
| --- | --- |
| `relational` | a continuing relationship and its history. |
| `operational` | unresolved execution, ownership, and coordination. |
| `decision_trace` | decisions, bets, and their rationale. |
| `resonance_trace` | material repeatedly connected to active thinking. |
| `reflective` | patterns emerging through reflection over time. |
| `tension_trace` | unresolved assumptions, competing forces, and tradeoffs. |
| `preference_evidence` | stable preferences demonstrated by actions. |

The program may also name a profile by its alias: `relationships` is `relational`
and `decisions` is `decision_trace` (the preset uses the aliases). Leave an
ambiguous entity without a profile.
