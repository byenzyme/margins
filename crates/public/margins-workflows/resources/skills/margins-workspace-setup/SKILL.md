---
name: margins-workspace-setup
description: Set up a Margins Workspace so the notes, thinking, and relationships of one practice become legible to recall. Declare the folders that hold that practice — for one person or a shared team vault — let Margins show back what it understands about them, invite plain-language corrections, persist only the settings needed to keep that understanding true, and prove it with one live local recall result. Use whenever someone wants to point Margins at their notes, add or change which folders it reads, or confirm that setup actually worked.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Workspace Setup

Help the user make one knowledge practice legible to Margins. Setup should leave
them with:

- a recognizable account of what their notes are about and where continuity lives;
- a clear boundary around what Margins may read, attend to, write into, and ignore;
- only the settings needed to keep that account true; and
- one useful question that recall can answer from a real note.

Be a reflective guide, not a configuration specialist. Talk to the user in the
language of their work. Configuration and command output are supporting evidence,
not the subject of the conversation.

Setup is not connected-note distillation. Do not transcribe a meeting, process a
session, or draft a note as part of this workflow.

## A small mental model

- A **Workspace** represents one practice. Its home is the only notes folder where
  Margins may write an approved note. Reference Sources may be searched but not
  modified.
- **Scan** is a read-only look at the Workspace home. The complete result gives you
  evidence for understanding the practice; it does not replace your judgment.
- **Plan** turns a complete desired Workspace configuration into an exact, reviewable
  list of changes. **Apply** commits only that plan.
- **Init** and **sync** prepare recall. **Recall** proves that the declared Sources
  are useful and reachable.

All Margins state belongs under its own `workspaces/<id>/` state directory. Never
create `.margins` inside a notes folder or rewrite the user's existing notes during
setup.

Never open, cat, print, or summarize credential bundles, auth files, Keychain
items, shell history, dotenv contents, token stores, cookie stores, or environment
variable values during setup. Use only redacted Margins product status and
capability reports when checking authorization or connection state.

## 1. Start with the practice

Ask which folder holds the notes the user thinks in. Ask whether other folders—such
as research, a shared vault, or an archive—should be searchable without becoming a
write destination.

Listen for the user's own names for the work, the projects and relationships that
continue over time, and what they hope Margins will help them remember. Keep your
initial reading tentative.

Then check the available capabilities:

```bash
margins capabilities
```

`margins capabilities` is supported and should stay in this workflow. Do not run
unsupported discovery commands; use only commands documented in this guide or
confirmed by `margins capabilities`, and treat redacted product status as the
boundary for auth-related checks.

Use the redacted `catalyst` object only as a readiness signal. Do not provision a
hosted lease at the start of a grounded review: the interpretation and consent
conversation may take time, and a short-lived lease should begin as late as
possible. If catalyst setup is needed, do it immediately before `init` as described
below.

If the report includes `recall.scan: true`, use the named Workspace and grounded
review path. Also use that path when the practice has more than one Source. For one
notes folder on a build without scan, use the direct path.

## 2. Direct setup when scan is unavailable

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
inside the recall boundary. It does not prove a broader interpretation of the
practice. Report that distinction plainly.

## 3. Declare a named Workspace and gather evidence

Choose the one writable home, add each read-only reference Source, and save the
complete scan result before initializing:

```bash
margins workspace new practice --home "/absolute/path/to/notes"
margins --workspace practice source add notes \
  --name research --role reference --path "/absolute/path/to/research"
margins --workspace practice source list --json
margins --workspace practice scan > /tmp/margins-workspace-scan.json
```

Source declarations define the full recall boundary. Scan examines the home so you
can understand it; it does not silently add Sources. Do not run `init` or `sync`
until the grounded review is settled, and do not run recall until after init and
sync.

## 4. Show the user an understanding, not scan output

Read the complete saved `scan.v2` result. Use the evidence reference below so that
you consider its coverage, candidates, representative files, structure, metadata,
existing settings, and catalyst profiles together. Do not reduce the scan to its
top-ranked folders or copy its candidates mechanically.

Form a short account of:

- what this practice appears to be and where its history accumulates;
- which projects, subjects, or relationships seem to continue across notes;
- which material looks incidental or structural; and
- one or two consequential questions Margins may be able to answer.

If the evidence is sparse, contradictory, or too abstract, read a bounded set of
the files referenced by the scan. Use note content and patterns of use—not folder
names alone—to infer what something means.

Present this account in ordinary language. Cite recognizable folders, notes, and
connections, but do not narrate schema fields or ranking mechanics. Ask: “Does this
match how you work, and what did it miss?” Reflect corrections back in the user's
terms before deriving settings. A factual correction is not consent to change the
Workspace.

A healthy Workspace may need no setting change. It should still receive a rich
interpretation and a real recall proof.

## 5. Persist only consequences the user recognizes

After the user recognizes the account, decide whether any durable setting follows.
Ordinary setup changes should be narrow:

- add a structural folder that should be ignored;
- correct a Source boundary or role;
- choose an explicit, stable set of existing subjects for attention; or
- add an evidence-backed catalyst profile or folder expansion to such a subject.

Explicit entity curation is optional. Leave it automatic when the evidence is
uncertain or the user wants Margins to keep adapting. Never turn every scan
candidate into configuration, invent an entity, or configure a structure that does
not exist.

Before proposing any explicit entity, check and be ready to cite the exact scan
spec from `entity_curation_candidates[].spec`, the candidate's
`representative_samples`, and the structural expansion evidence in
`entity_curation_candidates[].expansion`. For older scans without the expansion
object, inspect `folder_page_entities` and `folder_stats` together, but do not
force the user or a later agent to make that join when the candidate already has
self-contained expansion evidence. If the user requested a specific curation and
the evidence does not support it, say so and keep the request unresolved rather
than declaring setup complete.

When a setting is needed, read and copy the complete config at
`current_config.config_path`. Do not reconstruct it from the scan summary. Change
only the confirmed setting, save a complete desired-state TOML file, and compile it:

```bash
margins --workspace practice workspace plan \
  --desired /tmp/margins-workspace-desired.toml --json \
  > /tmp/margins-workspace-plan.json
```

If `actions` is empty, explain that no settings need to change and skip both consent
and apply. Otherwise, translate each action into its user-visible consequence and
ask for explicit consent. If the user agrees, apply the saved plan unchanged:

```bash
margins --workspace practice workspace apply \
  --plan /tmp/margins-workspace-plan.json \
  --json
```

Apply derives its revision check and retry identity from the plan. It rejects a
stale or altered plan and commits atomically.

If the user corrects the understanding after seeing a plan, return to the complete
desired config, change only that correction, and run `workspace plan --desired`
again. Show the fresh plan before applying it. Never hand-edit plan JSON.

Only make settings the user consented to after seeing the current plan. Any
fallback policy change requires a fresh desired config, a fresh plan, and fresh
consent before apply.

If a correction changes only how the user describes the practice, keep it in the
conversation and let it shape the recall proof. Do not manufacture a setting for
something the Workspace cannot represent.

## 6. Initialize and prove the setup

Once the review and any consented plan are settled, check the redacted catalyst
status from `margins capabilities` again. If it reports `usable = false`, run this
once before `init`:

```bash
margins setup --only catalyst
```

This is the provisioning boundary: it may mint or refresh a short-lived hosted
lease, or prepare the configured local fallback. Never inspect its credential
files to diagnose it. If setup still reports no usable catalyst, stop and report
that readiness failure; do not run `init` repeatedly and do not install a different
fallback policy without the user's consent.

Then initialize and prove recall in this order:

```bash
margins --workspace practice init
margins --workspace practice sync --json
margins --workspace practice recall "<question from the confirmed understanding>"
margins --workspace practice recall "<distinctive phrase from research>" --source research
```

When possible, test both claims:

- A distinctive phrase proves that a declared Source is reachable.
- A question from the confirmed account tests whether Margins can support the
  understanding you presented.

Verify that each result cites a real supporting note. If recall contradicts the
account, revisit the interpretation rather than declaring success.

Report the Workspace id and state path, its home and reference Sources, any approved
attention or exclusion settings, the `init` and `sync` results, and at least one
source-backed recall result. Restate what the user can expect Margins to remember
and confirm that their notes were not modified. Never declare setup complete while
a requested, supported curation decision remains unreviewed, unsupported by scan
evidence, or unconsented.

Do not begin connected-note distillation as part of setup.

## Evidence reference: reading the complete scan

The field names below are for your analysis. The user normally needs the meaning you
derive from them, not the names themselves.

| Evidence | What it helps you judge |
| --- | --- |
| `schema_version`, `scan_id`, `vault_path`, `generated_at`, `status`, `files` | Which scan you are reading and the home it describes. |
| `instructions`, `summary` | The scan's own cautions and the overall size, age, and shape of the material. |
| `coverage_entities` | A small set chosen to cover distinct parts of the notes, so a large folder does not crowd out a smaller but meaningful area. Start here for breadth. |
| `entity_curation_candidates` | Existing folders that may deserve stable attention, with `representative_samples`, exact `spec` values, and self-contained `expansion` evidence when available. Use these to investigate, not to auto-configure. |
| `top_entities`, `top_folders`, `top_tags`, `top_links` | Frequent or prominent threads. Compare them with coverage evidence; frequency alone does not establish importance. |
| `entity_samples`, `sample_files` | Concrete excerpts and files that reveal what a candidate means in practice. |
| `folder_stats`, `folder_page_entities`, `folder_children`, `tag_children` | How the material is organized, whether a folder contains durable linked pages, and whether parent/child choices would duplicate attention. |
| `frontmatter_samples` | Metadata conventions already used by the notes. Treat them as evidence of an existing practice, not a schema to impose. |
| `current_config` | What Margins already attends to or excludes. `current_config.config_path` locates the complete config to copy before planning a change. |
| `available_profiles` | The catalyst profiles supported by this build. Never configure a profile that the scan does not list. |
| `entities`, `excluded_folders` | Compatibility summaries. Use their richer supporting fields above before making a judgment. |

Keep the whole scan available while reasoning. Absence and disagreement between
sections are useful evidence; do not force a recommendation merely because a field
is present.

## Evidence reference: entities and catalyst profiles

An entity is an existing thread—such as a folder, tag, linked note, or running
log—that Margins can build recall catalysts around. A non-empty `[policy].entities`
list is the exact stable attention surface: Margins selects those entities instead
of appending automatic choices. An empty list leaves attention adaptive.

Use exactly the spellings surfaced by scan:

```toml
[policy]
entities = [
  "#enzyme",
  { "folder:people" = { profile = "relational", expandable = true } },
]
```

Valid simple forms are `folder:<path>`, `#<tag>`, `[[<linked name>]]`, and
`log:<name>`. An entity may also have a `profile` or, for a folder,
`expandable = true`.

A catalyst profile changes the kinds of questions Margins develops for an entity;
it is not a weight or an importance score. Use `available_profiles` as the
authoritative catalog for the installed build. Offer a profile only when
representative notes make the posture clear:

| Profile | Use when the notes primarily preserve… |
| --- | --- |
| `relational` | a continuing relationship and its history. |
| `operational` | unresolved execution, ownership, and coordination. |
| `decision_trace` | decisions, bets, and their rationale. |
| `resonance_trace` | material repeatedly connected to active thinking. |
| `reflective` | patterns emerging through reflection over time. |
| `tension_trace` | unresolved assumptions, competing forces, and tradeoffs. |
| `preference_evidence` | stable preferences demonstrated by actions. |

Leave an ambiguous entity without a profile.

Use `expandable = true` only for a folder whose `folder_page_entities` and
`folder_stats` show real child pages that the user wants treated as separate
threads. Prefer `entity_curation_candidates[].expansion` when present:
`mode = "automatic"` or `expands_automatically = true` means `expandable = true`
is redundant in config; `mode = "explicit_available"` means real child pages
exist below the automatic threshold and `expandable = true` is available only if
the user wants those child pages treated as separate threads.
`mode = "not_applicable"` means there is no supported folder expansion to
configure. Never persist the child links themselves merely because they live in
an expandable folder.

For a meeting-oriented practice, durable person or relationship notes can make
preparation and follow-up more continuous. If that structure already exists and
the evidence supports it, explain the benefit and ask whether the user wants it in
the stable attention surface, usually with `relational`. If recurring people appear
but no durable structure exists, you may explain that one note per person is an
optional practice. Do not create, reorganize, or configure those notes during
setup.

When automatic attention is left in place, note-only Workspaces draw from the scan's
folder, tag, link, and log coverage. Workspaces with connected mail or calendar
Sources use bounded, noise-filtered people signals instead. If those signals are
not safe enough, Margins chooses none rather than guessing.
