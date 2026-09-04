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
- one source-backed insight and a useful next question grounded in their notes.

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

Resolve the practice boundary from the user's request and any existing named
Workspace first. Ask which folder holds the notes only when the writable home is
genuinely ambiguous. Ask about other searchable folders—such as research, a shared
vault, or an archive—only when the user mentions them, the existing Source list
contains an unresolved boundary, or the requested change requires that decision.
Do not ask the user to reconfirm a home and lack of reference Sources that the
request plus current Workspace already establish.

Listen for the user's own names for the work, the projects and relationships that
continue over time, and what they hope Margins will help them remember. Keep your
initial reading tentative. “Listen for” is not an instruction to administer a
continuity or attention questionnaire before scanning. Use what the user already
said and let the grounded review surface what still needs correction.

Then check the available capabilities:

```bash
margins capabilities
```

`margins capabilities` is supported and should stay in this workflow. Do not run
unsupported discovery commands; use only commands documented in this guide or
confirmed by `margins capabilities`, and treat redacted product status as the
boundary for auth-related checks.

Use the redacted `catalyst` object only as a readiness signal. Do not provision a
hosted lease at the start of a grounded review: the interpretation and recognition
conversation may take time, and a short-lived lease should begin as late as
possible. If catalyst setup is needed, do it immediately before `init` as described
below.

Treat memories or transcripts from earlier setup attempts as hypotheses, never as
evidence that the installed build still has the same behavior. Establish current
behavior from this Workspace's scan, plan, init, sync, and recall results.

If the report includes `recall.scan: true`, use the named Workspace and grounded
review path. Also use that path when the practice has more than one Source. For one
notes folder on a build without scan, use the direct path.

On the grounded path, scan as soon as the explicit named Workspace and its Source
boundary are resolved. Do not inspect the config separately, sample notes, or do
other practice interpretation first: `scan.v2` already carries current config,
representative-note, coverage, and structural evidence. `scan` is a top-level
command selected by the global Workspace flag: `margins --workspace <id> scan`.
There is no `margins workspace scan` subcommand. Do not probe that shape or infer
the command hierarchy from the word “Workspace” when this guide and
`recall.scan: true` have already established the supported invocation.

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
complete scan result immediately after the boundary is settled and before
initializing:

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
terms before deriving settings.

This recognition pause is the one universal discovery question in grounded setup.
Do not append a separate menu about continuity, attention, profiles, expansion, or
what Margins should focus on. If the user already named what matters, show how the
scan supports or complicates it instead of asking them to choose it again.

The user's opening request to “set up Margins,” “follow this guide end to end,” or
otherwise complete setup authorizes the ordinary setup workflow, including applying
the minimum supported settings that follow from the account they recognize. Pause
here for their answer so corrections can shape those settings. Do not prepare, plan,
or apply exclusions, attention, profiles, expansion, or Source changes before this
recognition pause is settled.

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

The agent owns this derivation. Do not ask the user to design `[policy].entities`,
choose between automatic and explicit attention in the abstract, or repeat a
priority they already expressed. Propose the minimum concrete configuration that
follows from the recognized account, translate its consequences—including when an
explicit set replaces automatic attention—and then carry it through. Ask one
additional plain-language question only when a real unresolved ambiguity would
produce materially different configurations and neither the scan nor the user's
prior words resolve it.

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

If `actions` is empty, explain that no settings need to change and skip apply.
Otherwise, show the user-visible consequences and apply the saved plan unchanged in
the same turn. Do not ask for a second “apply this plan” confirmation: the opening
setup request plus recognition of the grounded account already authorize these
ordinary, minimum setup consequences.

```bash
margins --workspace practice workspace apply \
  --plan /tmp/margins-workspace-plan.json \
  --json
```

Apply derives its revision check and retry identity from the plan. It rejects a
stale or altered plan and commits atomically.

If the user corrects the understanding before apply, return to the complete desired
config, change only that correction, and run `workspace plan --desired` again. Show
the fresh consequences and apply that fresh plan. Never hand-edit plan JSON.

Only apply settings that are narrow consequences of the recognized account. A
fallback policy change after failure is not part of that derivation: stop and report
the failure rather than silently switching attention, removing a profile, or
installing a different fallback.

In the ordinary grounded path, recognition of the scan-grounded account is the only
universal pause. Boundary clarification and an additional missing decision are
conditional exceptions, not a questionnaire or mandatory setup stages.

If a correction changes only how the user describes the practice, keep it in the
conversation and let it shape the recall proof. Do not manufacture a setting for
something the Workspace cannot represent.

### When useful structure is missing

Do not confuse “nothing stable to configure yet” with “nothing useful to offer.”
When the notes contain continuing work or relationships but few durable handles,
keep attention adaptive and finish setup normally. At the close, offer at most two
small future capture habits supported by the evidence. For each habit:

- name the real thread or material in the user's language;
- explain briefly what Margins cannot follow reliably yet;
- suggest the smallest future-only habit that would create continuity; and
- name a question that habit would make answerable.

Useful handles can be a date on new notes, one stable name or wikilink for a
recurring person or subject, one home or dated running log for a continuing thread,
or a short Markdown note that preserves the decision trapped in another format.
Do not prescribe a generic folder taxonomy or imply that tidiness improves recall.

This coaching is optional advice, not another question, setup gate, or permission
to restructure the vault. Do not create folders, move or rewrite existing notes,
or invent tags and links during setup. If the user explicitly wants help
reorganizing their practice, offer that as a separate, scoped follow-up.

## 6. Initialize and prove the setup

Once the review and any plan are settled, check the redacted catalyst
status from `margins capabilities` again. If it reports `usable = false`, run this
once before `init`:

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

If `init` fails after an applied plan, preserve that policy while you
report and diagnose the failure. Do not apply automatic attention, remove a
profile, or try another policy as a diagnostic shortcut. Any such fallback is a
new material setting: stop and leave it for a later explicit request. A
`live_lexical` status only confirms that an index exists; it does not establish
that the recall command is usable when catalyst readiness is still pending.

Treat `workspace status`'s `live_lexical` document count as portable live Markdown
coverage, not as the official semantic index size. Never relabel that number as
“indexed documents.” In an official runtime, `mode = "indexed"` reports the
persisted engine index count and may be described that way. A recall response's
`note_count` is the exact number of documents searched by that recall call.

Then initialize and prove recall in this order:

```bash
margins --workspace practice init
margins --workspace practice sync --json
margins --workspace practice recall "<question from the confirmed understanding>"
margins --workspace practice recall "<distinctive phrase from research>" --source research
```

When possible, test both claims:

- A distinctive phrase proves that a declared Source is reachable. Copy one
  contiguous, verbatim phrase of roughly 5–10 words from the body of a scan-cited
  note; do not substitute a title, filename, or bag of related keywords.
- A question from the confirmed account tests whether Margins can support the
  understanding you presented.

Verify that each result cites a real supporting note. If recall contradicts the
account, revisit the interpretation rather than declaring success. A nearby semantic
result is not an exact-phrase boundary proof: if the distinctive phrase does not
retrieve the note containing it, report that proof as incomplete instead of
silently substituting a different query or describing semantic recall as equivalent.

Lead the final handoff with what the proof revealed, not the setup machinery. Answer
the confirmed account question in two to four plain-language sentences grounded in
the returned excerpts, cite the supporting notes, and name the useful pattern,
change, tension, or connection. Do not merely report that recall returned real
notes. If the evidence does not support an honest insight, say so and offer a
narrower next question rather than fabricating one.

Then state what this demonstrates Margins can now help with and give one natural
next question. Follow with a compact, secondary setup receipt: the Workspace and
its read/write boundary, settings translated into the user's language, whether
machine-level catalyst setup changed, whether `init` and `sync` succeeded, and
confirmation that notes were not modified. Exact state paths, revision hashes,
similarity scores, profile names, and other implementation details are optional;
include them only when the user asks or they explain a consequential limitation.
Never declare setup complete while a requested, supported curation decision remains
unreviewed, unsupported by scan evidence, unresolved, or outside the recognized
account.

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
the evidence and user's account support it, use that as evidence for a proposed
stable attention surface, usually with `relational`; do not turn it into a separate
preference question before plan review. If recurring people appear but no durable
structure exists, you may explain that one note per person is an optional practice.
Do not create, reorganize, or configure those notes during setup.

When automatic attention is left in place, note-only Workspaces draw from the scan's
folder, tag, link, and log coverage. Workspaces with connected mail or calendar
Sources use bounded, noise-filtered people signals instead. If those signals are
not safe enough, Margins chooses none rather than guessing.
