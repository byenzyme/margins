# Workspace Diagnosis Design Context

This document captures the product intent behind the workspace diagnostic
fixtures. The fixtures in `test-cases/` are not just synthetic folders; they are
evidence for a loop that should help Margins learn how to meet a user at the
moment they first point the app at a folder.

## Original UX Problem

Margins needs a clean bridge between two very different first-run situations:

- the user has no meaningful vault structure yet;
- the user points Margins at an existing vault, codebase, Drive export, transcript
  dump, or messy work folder with conventions already in motion.

The early product idea was: the Initialize button could run Enzyme and then show
an audit or diagnostic explaining what the workspace looks like and how Margins
can fit into it. That raised a tension. A useful meeting-notes app does need a
minimal structure for new notes, but asking a user to buy into append-only logs,
atomic notes, people folders, evergreen notes, tags, links, and frontmatter
during setup is too much. The first-run job is not to teach vault philosophy.
It is to make the next meeting note land somewhere understandable.

The better product question became:

> Will Margins connect to my existing work, or just dump text somewhere?

## Product Stance

Margins should have an opinionated default, but it should not require the user to
adopt the philosophy before the first capture.

The receptive language is outcome language:

- where the next meeting note will go;
- whether existing files will be changed;
- whether past notes/transcripts can be used as context;
- what is known, what is uncertain, and what can be skipped.

Avoid structure-first language:

- "you need an append-only log";
- "normalize your schema";
- "vault health";
- "audit score";
- "restructure";
- "optimize tags";
- mechanism labels such as petri, catalysts, embeddings, or index internals as
  primary user-facing copy.

The durable user-facing promise is:

> You can start now. Here is the least surprising place for new meeting notes.
> Existing files will not be changed.

## Read Before Write

The thread exploration found an important implementation boundary: current setup
paths can conflate choosing a folder, validating it, and indexing it. The future
UX should separate these operations.

1. **Choose folder**
   - The user selects a project/workspace.
   - This should not create `.enzyme`, run refresh, write global Enzyme config,
     or imply that structure has been created.

2. **Read workspace**
   - A read-only scan/diagnosis looks at shape: folders, filenames, dates,
     Markdown quality, imports, transcripts, note conventions, and existing
     cache state.
   - The result should be stored in Margins app data or test artifacts, not
     written into the user's vault.

3. **Explain fit**
   - The UI says what Margins can safely infer and where uncertainty remains.
   - It proposes the smallest useful destination for future meeting notes.
   - It must make capture available without requiring cleanup or import.

4. **Initialize context**
   - Building a private search cache is a separate explicit action.
   - Copy should say what is written, such as `.enzyme/`, and that user notes
     are not edited.

5. **Import history**
   - Importing past Google Docs, Drive exports, transcripts, PDFs, or meeting
     notes is an optional context booster.
   - It is not a prerequisite for starting capture.

## Margins UX Surfaces Under Test

This loop is meant to improve specific setup moments in Margins, not only the
diagnostic skill in isolation.

Primary surfaces:

- first-project onboarding, especially the project/folder step;
- settings project picker and project readiness line;
- any "What Margins noticed" panel, drawer, or disclosure;
- the explicit action that builds a private search cache;
- the optional import-history entry point for old notes, Drive exports, and
  transcripts;
- the ready-to-capture state after setup is skipped, accepted, or partially
  completed.

The current product risk is that setup can collapse several different actions
into one ambiguous "initialize" moment:

- selecting a folder;
- validating whether it exists;
- reading workspace shape;
- creating missing destination folders;
- building or refreshing `.enzyme`;
- explaining where future notes will go.

The loop should help keep those moments legible. A product change is better
when the user can tell which thing just happened and which things have not
happened yet.

Surface-level questions the loop should answer:

- What should the project readiness line say for each workspace type?
- When should Margins say "fresh folder" versus "existing notes found"?
- What copy explains "Build private search cache" without sounding scary or
  hiding the `.enzyme/` write?
- When, if ever, should a "What Margins noticed" panel appear?
- How should import history be offered without making setup feel blocked?
- Does the user still know they can start capture now?

The loop may recommend skill changes, app-copy changes, or surface changes. It
should not assume every failure belongs to the skill.

## Meeting Notes Shape

Margins only needs a minimal default for new notes:

- one conversation becomes one note;
- the note has a date-bearing filename or metadata;
- the destination is visible before or at save time;
- existing conventions are reused when strong enough;
- weak or conflicting conventions are named as uncertain rather than "bad."

The app may say:

> Margins works best when each conversation lands as its own dated note. That
> keeps the original meeting intact and makes decisions easier to trace later.

It should not say:

> You need to restructure your vault into an append-only log.

## Import And Existing History

Many users will not think in terms of Markdown conversion. They will think:

- "I have old meeting notes in Google Drive";
- "I have transcripts somewhere";
- "Our project docs live in a repo";
- "I have notes, but they are not organized";
- "Can Margins use this history without making me migrate everything?"

The right distinction is:

- **start fresh:** save new meeting notes from now on;
- **point at existing notes:** read/index local Markdown/text without copying;
- **bring in past meeting material:** copy/convert supported material into a
  chosen import folder after preview.

Import guidance should preserve originals, show where converted notes will go,
and report what worked or failed. It should not imply that a user must migrate a
whole Drive before Margins becomes useful.

## Self-Improvement Loop

The executable loop contract lives in `LOOP_SPEC.md`. This section records the
intent behind that contract.

The loop this fixture suite supports:

1. Generate realistic spotty workspaces across many shapes.
2. Run the diagnostic or skill against each workspace.
3. Compare the output with each case's `expected-diagnostic.md`.
4. Grade realism and diagnostic behavior with a checker thread.
5. Patch either the diagnostic/skill or the fixtures.
6. Rerun the same cases and compare claims before/after.

The loop should use maker/checker separation:

- one thread generates or patches fixtures;
- a separate review thread grades realism, overclaims, and UX fit;
- patch threads improve specific batches;
- the parent thread validates structure and synthesizes next steps.

The loop should prefer agentic evaluation over heuristic smoke checks. Static
checks such as JSON validity, file counts, and read-only mutation guards are
necessary, but they are not the product evaluator. The primary evaluator is a
set of agents that read the workspace, simulate user pushback, critique the
setup experience, patch the skill, hunt regressions, and preserve what was
learned.

## Falsifiable Success

The diagnostic is working if a first-time user can answer:

- Where will my next meeting note be saved?
- Will Margins change existing files?
- Can I start recording without importing or cleaning up?
- If I have old notes/transcripts, can I bring them in later?
- What did Margins infer, and where was it unsure?

It fails if the user concludes:

- "I need to organize my vault before I can use Margins";
- "Margins is going to restructure my files";
- "I need to convert my whole Drive to Markdown first";
- "I do not know where the note will land";
- "Margins is judging my messy folder as broken."

## How The Fixtures Should Be Used

The workspaces are intentionally imperfect. They should catch diagnostics that
overfit to ideal Obsidian vaults or polished meeting-note folders. A good
diagnostic should respond differently to:

- a codebase with plan docs but no dates;
- a daily-notes vault with meetings embedded in body text;
- a transcript dump with almost no summaries;
- a Drive export with half-converted documents;
- a foreign-domain folder where meeting notes should be isolated;
- a partial `.enzyme` / `.margins` state where cache setup and note structure must
  be explained separately.

Across all cases, the product behavior should stay stable: start capture is
available, the safest default is explicit, and no existing content is changed.
