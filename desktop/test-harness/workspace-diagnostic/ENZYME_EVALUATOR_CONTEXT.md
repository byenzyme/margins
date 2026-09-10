# Enzyme Evaluator Context

This file ports the Enzyme agent skill from `../enzyme-rust/plugin/agent/SKILL.md`
into the workspace-diagnostic loop. Every runner, judge, patcher, and verifier
pass should load this context before evaluating a fixture, and should treat it
as operating instructions for diagnosis rather than background reference text.

When a child evaluator runs in a workspace that can access `../enzyme-rust`, it
should read `../enzyme-rust/plugin/agent/SKILL.md` directly before using this
ported context. This file is the loop-local copy for agents that cannot read the
source repo, and for preserving the exact Enzyme stance used by a run.

## What Enzyme Adds To This Loop

Enzyme is not a generic vault-health checker. It is a local semantic retrieval
layer that learns from the markdown structure the user already has: folders,
tags, wikilinks, dates, inboxes, daily notes, people pages, frontmatter, and
source text.

The `enzyme scan` output is substrate evidence. It can show corpus shape,
candidate folders, markdown surfaces, hidden tool state, and obvious weak
spots, but it is not the diagnosis. The diagnosis is the agent process that
loads the Enzyme setup/indexability lens, reads the selected workspace,
inspects representative files after the scan, forms and falsifies hypotheses,
and records what should flow into substrate mapping, failure modeling, repair
policy, and product translation.

For workspace diagnosis, use Enzyme as an indexability and repair-policy lens:

- what is already usable as retrieval signal;
- what is weakly indexable or raw, such as transcript dumps or Drive exports;
- what retrieval/setup failure modes are likely;
- what would materially improve future retrieval;
- what restructuring is safe, reversible, or too risky without approval;
- what can safely wait until after first capture when the host product is Margins.

Do not turn that lens into a generic demand that the user reorganize. The repair
policy should be concrete and tiered. In an Margins onboarding surface, Margins's
setup promise still wins:

> You can start now. Here is the least surprising place for new meeting notes.
> Existing files will not be changed.

In a pure Enzyme setup surface, it is acceptable to say that initializing as-is
will be weak and recommend a small repair first, as long as the action is
evidence-backed, reversible, and approval-gated.

## Required Evaluator Discipline

1. Load `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions when it
   is available. If it is not available, say so and use this ported context as
   the operating lens.
2. Run or review a case-level Enzyme Skill Diagnostic Probe before assembling or
   checking final artifacts. The probe must apply the setup/indexability lens to
   the selected workspace, not merely collect a directory snapshot.
3. Run or review `enzyme scan -p <workspace>` before judging or patching a
   case. Do not pass `--write-config` during fixture evaluation. If the fixture
   already contains `.enzyme/` or `.margins/`, scan a temporary copy or run a
   pre/post mutation guard because the CLI may write runtime logs inside an
   existing tool-state directory.
4. Treat scan output as evidence, not as the whole diagnosis. Read representative
   files and the fixture manifest/expected diagnostic, then record the
   hypotheses, confirmations, falsifiers, and handoff claims that come from
   applying the Enzyme lens.
5. Do not run `enzyme init`, `enzyme refresh`, `enzyme apply`, or any command
   that creates `.enzyme/`, writes global config, mutates notes, or materializes
   imports during the read-only loop.
6. If `petri` or `catalyze` would require initialization, record that they are
   unavailable for the read-only fixture and judge whether the diagnostic gives
   a useful setup preview without them.
7. Preserve existing structure. Follow Obsidian/markdown conventions already in
   the workspace before suggesting any new shape.
8. Create people folders, company folders, frontmatter fields, tags, or wikilinks
   only when the workspace already has that convention or the user explicitly
   approves a setup/repair step.
9. Separate these operations in the user-facing diagnosis:
   - choosing a folder;
   - reading workspace shape;
   - choosing the next-note destination;
   - building a private search cache;
   - importing/materializing history;
   - repairing structure for stronger retrieval.
10. Explain setup in outcome language. Avoid exposing embedding internals,
   catalysts, schema normalization, vault health, or index mechanisms unless the
   user asks.
11. Write an evaluator trace for each case. The trace must name the active skill
   revision, whether the original Enzyme skill source was read as operating
   instructions or this ported context was used, exact Enzyme commands,
   representative files read, Enzyme-lens hypotheses, confirmed/falsified
   claims, and the read-only boundary evidence.
12. If a scan creates runtime residue such as `.enzyme/enzyme.log`, record the
    mutation, remove only the generated residue after confirming it is generated
    runtime residue, and treat the case as needing a verifier rerun through a
    temporary-copy scan path. Do not clean or modify user notes, imports, config,
    or cache state.
13. Separate substrate evidence, failure modes, repair policy, and product
    translation. Do not let reassuring product copy hide a real Enzyme
    indexability failure.
14. Static scan/snapshot-only maker work is incomplete. Evaluators and judges
    should fail coverage when artifacts exist but the trace does not show a
    case-level Enzyme skill diagnostic process.

## Enzyme Setup Lens For Each Case

For each fixture, the evaluator should answer:

- What is already indexable as-is?
- What is not yet indexable or would be weak evidence?
- What concrete failure modes would make retrieval poor or misleading?
- Which existing folders or metadata should Margins respect?
- What repair tiers are available: no-op, minimal reversible, medium convention,
  or heavy migration?
- Which optional repair/import/cache action would improve retrieval materially?
- What should Margins explicitly avoid writing or changing now?

## Failure Modes To Look For

Name failure modes only when evidence supports them. Good failure modes include:

- raw dumps with little durable markdown source text;
- transcript piles with timestamps but no decision/action-bearing notes;
- imports without source boundaries or previewable materialization plans;
- duplicate, half-converted, or interrupted export/import files;
- generated/runtime/build folders mixed into the corpus;
- recurring people, projects, or companies with no stable handles when the
  workspace otherwise uses entity conventions;
- dates split across filenames, bodies, and metadata in ways that weaken
  temporal retrieval;
- frontmatter fields that conflict enough to make retrieval or filtering
  misleading;
- multiple unrelated domains in one folder with no scope boundary;
- code repos where docs are useful but source/build artifacts dominate scan
  evidence;
- stale `.enzyme` or `.margins` state that could confuse cache setup.

For each failure mode, record:

- evidence;
- severity;
- confidence;
- retrieval impact, written specifically for that failure mode and case;
- what would falsify it;
- what not to infer.

Also record `## Non-Failures` and `## Ambiguities` for every case. A
well-structured or partly structured workspace still has healthy retrieval
signal; name that signal explicitly instead of implying that every observed
imperfection is a failure. Ambiguities should separate what the workspace
evidence supports from what would require user confirmation.

Do not reuse one generic retrieval-impact sentence across every mode. The
impact should explain the concrete retrieval/setup risk: for example, raw
transcripts may retrieve speaker chatter without decisions, malformed
frontmatter may break date/entity filtering, scan undercoverage may hide useful
docs from setup, and mixed destination folders may scatter future meeting
continuity. If two modes have the same impact, consider whether they are really
one mode or whether one should become an ambiguity/non-failure.

## Repair Policy Tiers

Repair policy is not the same as executing repairs. Propose options in tiers:

- **No-op / initialize as-is:** useful when the workspace is already indexable
  enough or when the host product must keep setup lightweight.
- **Minimal reversible repair:** exclude generated folders, scan a temp copy,
  preserve raw exports, add deterministic date metadata to date-named notes, or
  choose an import/source-boundary folder.
- **Medium convention repair:** adopt stable wikilinks, tags, entity fields, or
  templates when the workspace already gestures at those conventions.
- **Heavy migration:** split unrelated domains, convert raw exports, deduplicate
  transcript piles, or restructure folders. This always requires backup,
  preview, and explicit approval.

Every repair recommendation must state why it improves retrieval, what it risks,
and whether it is required before the host product's next action.

## Good Diagnostic Language

- "Margins can start saving new captures here now."
- "The existing files stay as they are."
- "This folder already has useful signals: dated notes, transcripts, project
  docs, and a few people/project references."
- "These transcripts are searchable source material, but they are weak decision
  evidence until a few summaries or meeting notes exist."
- "A small reversible repair would be to exclude generated folders and keep
  imported transcripts under one source-boundary folder."
- "Building the private search cache is a separate action. It reads these notes
  and writes cache files; it does not rewrite your notes."
- "Importing old Drive notes or transcripts can help later, but it is not
  required for the next capture."

## Bad Diagnostic Language

- "Your vault is unhealthy."
- "Normalize the schema before capture."
- "Move these notes into a new taxonomy."
- "Create people pages for every name found."
- "Initialize Enzyme before starting."
- "Run a full import before this workspace is useful."
- "Everything is fine" when the substrate has obvious retrieval failure modes.
- "Clean up your notes" without concrete evidence, tiers, and approval
  boundaries.

## Stacked Skill Revisions

Loop patches must stack. A patcher must not silently replace the baseline
diagnostic behavior. Instead, every bounded improvement creates a new revision
artifact:

```text
runs/<run-id>/skill-revisions/
  v000-baseline.md
  v001-<short-reason>.md
  v002-<short-reason>.md
```

Each revision must include:

- the prior revision it builds on;
- the failure pattern from judge evidence;
- the changed instruction area;
- why the change generalizes across workspace classes;
- cases expected to improve;
- cases at risk of regression;
- old vs. new example language.

Runner and verifier passes must state which revision they evaluated.
