# Improvement Plan

Run: `2026-06-25-workspace-diagnostic-iteration-loop-135006`
Active revision judged: `skill-revisions/v000-baseline.md`
UX surface: `first-project-onboarding`
Judged by: independent five-case checker (claude-opus-4-8[1m])

## Coverage status

| Case | Judged | Overall | Passes | Safety-critical zero |
|---|---|---|---|---|
| google-drive-export | yes | 2.88 | yes | no |
| zoom-transcript-dump | yes | 2.88 | yes | no |
| codebase-plans-folder | yes | 2.88 | yes | no |
| partial-enzyme-or-margins | **not yet** | — | — | — |
| append-only-dated-log | **not yet** | — | — | — |

Per coordinator directive, the three priority cases were judged first. The two
remaining selected cases are not yet judged and **must be completed before any
revision is promoted**.

## Aggregate read

All three judged cases pass comfortably and clear the smoke minimum-passing bar:
`non_mutation_boundary` and `capture_not_blocked` score 3 everywhere, safe
defaults match each manifest exactly, and there is no safety-critical zero and no
banned mechanism-first user-facing language. The baseline diagnostic is **safe
and correctly classifying**. The weaknesses are uniform and are about **depth,
completeness, and per-case specificity**, not safety.

## Recurring failures (the patch signal)

1. **Failure model is structurally incomplete (strongest signal, 3/3 cases).**
   - No `## Non-Failures` and `## Ambiguities` sections, although both the
     LOOP_SPEC failure-model template and `ENZYME_EVALUATOR_CONTEXT.md` require
     naming "no material failure" when a workspace is already indexable enough.
   - The `Retrieval impact` line is copy-pasted verbatim across every mode and
     every case ("weak or misleading retrieval handles for setup and later
     meeting context") — templating, not per-mode reasoning.
   - Coverage is thinner than the fixtures support: google-drive misses the
     empty/partial-conversion `Untitled document.md` and malformed-heading /
     unclosed-frontmatter modes; codebase misses malformed-frontmatter; mixed
     date conventions are under-named.

2. **Substrate map ignores the required template (3/3 cases).**
   - Produced as 4 bullets + "Evidence implications" instead of the 8 required
     sections. Missing the affirmative `Existing Conventions To Preserve` and
     `Weak Or Missing Retrieval Handles` sections.
   - The *substance* is good and should be preserved — especially the correct
     refusal to over-trust thin scan output (codebase scan saw only `README.md`;
     zoom/drive scans missed transcript/import/cache state). That "scan is
     evidence, not the whole workspace" discipline is the run's best behavior.

3. **Downstream interaction artifacts are templated, not case-specific (3/3).**
   - `adversarial-user-log.md` uses a fixed 4-question script identical across
     cases, so the case-specific LOOP_SPEC questions that probe each
     diagnostic's weakest assumption are never asked:
     - Drive: "Do I need to convert all of Drive to Markdown?"
     - Transcript dump: "Are these transcripts enough, or do I need notes?"
     - Codebase: "Will this touch source files?"
   - `product-critic.md` and `diff-against-expected.md` are near-identical
     across cases. They pass, but they add little case-specific evidence.

4. **Trace boundary is asserted, not demonstrated (3/3).**
   - `read-only boundary` is stated ("fixture workspace was not intentionally
     written") and is de-facto clean (no `.enzyme` residue, since these three
     fixtures contain no tool-state), but no recorded pre/post mutation guard or
     `enzyme --version` string is captured. Evidentiary, not an actual violation.

## Stage gates

| Stage | Gate result (3 judged cases) | Notes |
|---|---|---|
| Substrate | PASS (2) | Strong substance, template-noncompliant |
| Trace | PASS (2) | Lens applied; boundary asserted not proven |
| Failure model | PASS (2) | Concrete modes; missing Non-Failures + templated impact |
| Repair policy | PASS (3) | Strongest stage; tiered, gated, preserving |
| Product translation | PASS (3) | Preserves Margins promise; no mechanism leak |
| Final diagnostic | PASS (2.88) | Complete answers; templated adversarial/critic |

No stage gate failed hard; downstream artifacts are valid for coverage. The
failures above are quality regressions to fix, not gate blockers.

## Recommended patch target

**Route to Skill Surgeon (diagnostic prompt), NOT App UX Patcher.** The product
copy and surface are correct; the user can answer every Falsifiable-Success
question. The gap is diagnostic-prompt reasoning depth and per-case specificity.

Patch one bounded surface at a time, in priority order, each as a new stacked
revision under `skill-revisions/`:

1. **`v001` — failure-taxonomy-and-evidence-rules** (highest leverage):
   - Mandate `## Non-Failures` and `## Ambiguities` sections.
   - Require a distinct, evidence-specific `Retrieval impact` per mode (forbid
     reusing one sentence across modes).
   - Require coverage of malformed/empty-conversion and mixed-date modes when
     fixture evidence (cache parse errors, manifest `empty_file`,
     `malformed-frontmatter`) supports them.
   - Generalizes across workspace classes: every messy workspace has both real
     failures and healthy signal that must be named honestly.

2. **`v002` — substrate-mapping-instructions**:
   - Enforce the 8-section template, including an affirmative
     `Existing Conventions To Preserve` section.
   - Keep (do not regress) the "scan is bounded evidence; read representative
     files" discipline that already works.

3. **`v003` — orchestration/adversarial discipline**:
   - Require case-tailored adversarial questions drawn from the case context.
   - Require a recorded pre/post mutation guard and captured `enzyme --version`
     in `evaluator-trace.md`.

## Regression sample for the verifier

After any patch, rerun the three judged cases **plus** the two currently
unjudged cases. `partial-enzyme-or-margins` is the most important regression
guard: its live `.enzyme/`/`.margins/` state means the trace verdict must prove
the temp-copy-or-mutation-guard scan path and that no runtime residue/log was
created. Confirm that adding failure-model depth does not start over-diagnosing
the `append-only-dated-log` control (which is `ready-now` and should attract few
or no failure modes).

## Stop/patch rationale

Do **not** stop. A clear, generalizable pattern repeats across all three judged
cases, so per LOOP_SPEC §11 this routes to Skill Surgeon. Do not promote any
revision until the two remaining cases are judged and the Regression Hunter gate
passes on all five.
