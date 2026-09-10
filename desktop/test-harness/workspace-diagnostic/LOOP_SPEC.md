# Workspace Diagnosis Self-Improvement Loop

This loop uses the synthetic workspaces in `test-cases/` to improve Margins's
first-project setup behavior and the Enzyme-backed diagnostic skill. It is not a
static fixture check. It is a maker/checker loop for answering:

> Can Margins look at a user's actual folder, explain how it can fit, and propose
> the smallest safe meeting-note default without making the user reorganize
> anything?

## Inputs

- Product context: `DESIGN_CONTEXT.md`
- Enzyme evaluator context: `ENZYME_EVALUATOR_CONTEXT.md`
- Fixture contract: `SPEC.md`
- Test cases: `test-cases/<case-id>/`
- Candidate diagnostic skill or prompt revision under test
- Optional candidate app UX copy or setup-flow implementation

The loop can be run manually by agents against the current mock workspaces. A
runnable script is not required. Agents should write the run artifacts described
below and use the fixture workspaces as the diagnostic substrate.

Each test case contributes:

```text
test-cases/<case-id>/
  manifest.json
  expected-diagnostic.md
  workspace/
```

## Outputs

Each loop run writes artifacts under:

```text
runs/<run-id>/
  run-manifest.json
  skill-version.md
  skill-revisions/
    v000-baseline.md
    v001-<short-reason>.md
  skill-promotion-plan.md
  cases/<case-id>/
    workspace-snapshot.json
    enzyme-scan.json
    substrate-map.md
    substrate-verdict.json
    failure-model.md
    failure-model-verdict.json
    repair-policy.md
    repair-policy-verdict.json
    product-translation.md
    product-translation-verdict.json
    exploratory-brief.md
    agent-diagnostic.md
    structured-diagnostic.json
    adversarial-user-log.md
    product-critic.md
    interaction-log.md
    judge-verdict.json
    diff-against-expected.md
    evaluator-trace.md
    enzyme-skill-diagnostic-probe.md
    trace-verdict.json
    verifier-verdict.json
  aggregate-verdict.json
  improvement-plan.md
  orchestration-trace.md
  memory-notes.md
```

If the loop patches the skill, also write:

```text
runs/<run-id>/
  skill-patch-summary.md
  rerun-comparison.md
```

Do not write diagnostic artifacts into the fixture workspaces.

## Agent Roles

Use separate threads for substantial runs.

The parent thread is the **Coordinator**. It should not silently collapse the
loop into one monolithic pass or into static artifact assembly. Before the four
pass artifacts are written, the Coordinator must run case-level Enzyme Skill
Diagnostic Probe threads. Each probe treats the upstream Enzyme skill as the
diagnostic procedure for one case or a tightly related case pair, then leaves a
trace that downstream artifact writers can cite.

A probe prompt must explicitly instruct the child thread to:

- read `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions, not
  merely as background or a citation;
- read `ENZYME_EVALUATOR_CONTEXT.md` and the active stacked revision;
- inspect the selected `test-cases/<case-id>/workspace/` through the Enzyme
  setup/indexability lens;
- run or review the read-only scan path, normally `enzyme scan -p <workspace>`,
  using a temporary copy when existing tool state could receive runtime logs;
- read representative workspace files after the scan;
- write a diagnostic trace before any final artifacts are assembled.

The diagnostic trace may be written as `enzyme-skill-diagnostic-probe.md` or as
a clearly marked section of `evaluator-trace.md`. It must include the skill
source loaded, case scope, scan command and mutation guard, representative files
read, Enzyme-lens hypotheses, evidence that confirmed or falsified them, and
the handoff claims that the Substrate, Failure Model, Repair Policy, and Product
Translation passes are allowed to use.

Static scans, directory snapshots, file-count summaries, manifest summaries, or
pre-filled artifact templates do not count as a full maker pass. They are
substrate evidence only. A maker pass becomes valid only when the case-level
probe has produced trace evidence that the Enzyme skill lens was actually used
to diagnose the selected workspace.

After the case-level probes, the loop is structured as four separate passes:

1. **Substrate Pass** — evidence map of what the workspace actually makes
   indexable or weak.
2. **Failure Model Pass** — explicit retrieval/setup failure modes, severity,
   confidence, and falsifiers.
3. **Repair Policy Pass** — tiered restructure options with approval and
   preservation boundaries.
4. **Product Translation Pass** — host-product copy/actions, such as Margins's
   "start capture now" promise.

For a full run, the Coordinator must spin off independent child threads for at
least:

1. case-level Enzyme Skill Diagnostic Probe work before artifact assembly;
2. maker work for the four passes above, grounded in the probe traces;
3. checker work for each pass artifact;
4. Skill Surgeon patch work routed to the pass that failed;
5. Regression Hunter / Verifier work after every patch.

The thread that patches the skill must not be the final judge of that patch.
Each child thread prompt must include:

- the run ID and artifact root;
- `ENZYME_EVALUATOR_CONTEXT.md`;
- the original Enzyme skill source path, `../enzyme-rust/plugin/agent/SKILL.md`,
  when that repo is available;
- the active stacked skill revision under `runs/<run-id>/skill-revisions/`;
- the exact cases it owns;
- the artifacts it must write;
- the read-only mutation boundary.

For Diagnostic Runner, Substrate Mapper, Failure Modeler, Repair Policy Agent,
Product Translation Agent, and Judge threads, the prompt must also say that
static scan/snapshot-only work is not sufficient coverage and that the child
must either consume a valid case-level Enzyme Skill Diagnostic Probe trace or
produce one before writing pass artifacts.

If a child thread cannot access `../enzyme-rust`, it must say so and continue
from `ENZYME_EVALUATOR_CONTEXT.md`, which is the ported copy for this loop.

- **Coordinator**
  - Owns run ID, case selection, artifact paths, stop conditions, and final
    synthesis.
  - Does not judge its own patches.
  - Writes `orchestration-trace.md` listing child thread IDs, role, provider,
    model, case scope, active skill revision, artifacts produced, and whether
    the thread used the Enzyme evaluator context.

- **Fixture Auditor**
  - Reviews selected test cases for realism and whether
    `expected-diagnostic.md` is evidence-backed.
  - May propose fixture patches, but should not patch the skill.

- **Diagnostic Runner**
  - Loads `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions when
    available, then `ENZYME_EVALUATOR_CONTEXT.md` and the active skill revision
    before producing pass artifacts.
  - Runs the current Enzyme-backed evaluation against each case through a
    case-level Enzyme Skill Diagnostic Probe before artifact assembly.
  - Produces `substrate-map.md`, `failure-model.md`, `repair-policy.md`, and
    `product-translation.md` before writing any final diagnostic.
  - Writes `evaluator-trace.md` per case with the exact Enzyme commands,
    representative files read, active skill revision, skill source loaded, probe
    hypotheses, and reasoning checkpoints.
  - A scan/snapshot-only runner output is incomplete and must be rerun or
    treated as provisional by downstream checkers.
  - Must not modify the fixture workspace.

- **Substrate Mapper**
  - Creates `substrate-map.md`.
  - Maps corpus shape, source boundaries, generated/noise folders, existing
    conventions, date/entity handles, raw/import material, and missing handles.
  - Must not propose copy or repairs except as evidence implications.

- **Failure Modeler**
  - Creates `failure-model.md`.
  - Converts substrate evidence into explicit Enzyme retrieval/setup failure
    modes.
  - Each failure mode must include evidence, severity, confidence, retrieval
    impact, and what would falsify it.
  - Must name "no material failure" when a workspace is already indexable
    enough, rather than inventing issues.

- **Repair Policy Agent**
  - Creates `repair-policy.md`.
  - Turns failure modes into tiered options: no-op/as-is, minimal reversible
    repair, medium convention repair, and heavy migration.
  - Must preserve raw/source material and state which actions require explicit
    approval.
  - Must not translate the policy into product copy.

- **Product Translation Agent**
  - Creates `product-translation.md`.
  - Translates the repair policy into the host product's setup promise. For
    Margins, first capture remains available even when Enzyme would benefit from
    later repair.

- **Final Diagnostic Writer**
  - Sole owner of `agent-diagnostic.md` and `structured-diagnostic.json`.
  - Reads `product-translation.md` and emits the final user-facing and machine
    readable diagnostic artifacts. No other role should overwrite them.

- **Exploratory Diagnostic Agent**
  - Uses the Enzyme setup/indexability lens from
    `ENZYME_EVALUATOR_CONTEXT.md`.
  - Reads the workspace like a human onboarding reviewer before producing any
    final diagnostic.
  - Forms hypotheses about workspace type, meeting-note conventions, import
    history, and uncertainty.
  - Decides which files or patterns matter instead of relying only on file
    counts or fixed heuristics.
  - Writes `exploratory-brief.md`.

- **Adversarial User Agent**
  - Interacts with the diagnostic as a first-time Margins user.
  - Asks realistic follow-up questions:
    - "Can I start without importing?"
    - "Will this change my files?"
    - "Where will the note go?"
    - "Can it use my old Drive notes/transcripts?"
    - "Why are you suggesting dated meeting notes?"
    - "This is a code repo, why are you talking about a vault?"
  - Records whether the diagnostic answers without drifting into vault
    philosophy or mechanism language.
  - Writes `adversarial-user-log.md`.

- **Product Critic**
  - Reviews whether the guidance fits Margins's first-run meeting-notes UX.
  - May disagree with `expected-diagnostic.md` when the fixture expectation is
    too prescriptive, under-evidenced, or less product-correct than the
    diagnostic.
  - Evaluates tone, trust, setup burden, import framing, and whether the user
    would feel judged.
  - Writes `product-critic.md`.

- **Judge**
  - Loads `ENZYME_EVALUATOR_CONTEXT.md` before scoring.
  - Scores each pass artifact before scoring final product copy.
  - Scores outputs against `DESIGN_CONTEXT.md`, the case evidence, the
    adversarial interaction, product-critic notes, and the rubrics below.
  - Treats `expected-diagnostic.md` as evidence, not as the oracle.
  - Reviews artifacts before reading implementation diffs.
  - Writes `trace-verdict.json` per case evaluating whether the runner's trace
    proves that the Enzyme skill lens was actually used and that the read-only
    boundary held.
  - Writes `substrate-verdict.json`, `failure-model-verdict.json`,
    `repair-policy-verdict.json`, and `product-translation-verdict.json`.

- **Skill Surgeon**
  - Reads the active skill revision and writes a new stacked revision under
    `runs/<run-id>/skill-revisions/`.
  - Patches the specific instruction area associated with the failing pass:
    substrate mapping, failure modeling, repair policy, product translation, or
    orchestration/trace discipline.
  - Does not patch fixtures unless the Judge explicitly says the fixture is
    wrong or under-evidenced.
  - Must explain why each change generalizes across workspace classes instead
    of merely fitting one fixture.
  - Must never overwrite an earlier revision. It appends a new revision and
    names the previous revision it builds on.
  - When a revision is verified, writes or updates `skill-promotion-plan.md`
    explaining whether the improvement should be promoted to the upstream
    Enzyme skill, the loop-local evaluator context, the host-product diagnostic
    prompt, or only the run-local revision.

- **App UX Patcher**
  - Patches app surfaces when aggregate findings implicate product UI rather
    than Enzyme skill or diagnostic prompt behavior.
  - Owns bounded changes to onboarding, settings project picker, readiness
    lines, cache-building copy, import entry points, and "What Margins noticed"
    surfaces.
  - Must provide before/after copy or screenshots and a verifier path proving
    the user misunderstanding was resolved without changing unrelated UX.

- **Regression Hunter**
  - Verifies the active stacked skill revision, not an implicit one-off prompt.
  - Reruns the same cases after patching.
  - Searches for new failure modes across unrelated fixtures.
  - In particular, checks that improving import/transcript behavior did not
    degrade codebase, empty-folder, daily-note, or messy-vault behavior.
  - Writes `verifier-verdict.json` for every rerun case.

- **Memory/Synthesis Agent**
  - Updates `memory-notes.md` with durable lessons:
    - phrases that earned trust;
    - phrases that made setup feel heavy;
    - recurring workspace shapes;
    - diagnostic blind spots;
    - skill changes that generalized well.
  - Does not patch the skill.

## Run Structure

The four-pass structure is the authoritative loop. Existing artifacts such as
`exploratory-brief.md`, `agent-diagnostic.md`, `product-critic.md`, and
`judge-verdict.json` remain useful, but they are downstream of the four pass
artifacts. A run should not patch the skill from final prose alone when an
earlier pass artifact reveals the real failure.

The case-level Enzyme Skill Diagnostic Probe is the entry condition for those
four passes. It is not an extra final artifact writer. Its job is to make the
diagnostic process real: load the upstream Enzyme skill as the operating lens,
read the selected workspace through that lens, record scan and file evidence,
and hand off grounded claims. The four pass artifacts then organize that
diagnosis. If the maker starts by filling artifact templates from a directory
snapshot, the run has skipped the diagnostic procedure and must be treated as
not covered.

## Stage Gates And Artifact Ownership

Every stage has a maker artifact, a checker artifact, and an explicit gate.
Downstream stages may draft provisional artifacts before a gate passes, but
those artifacts do not count for coverage and must not drive patches until the
upstream gate is either passed or explicitly waived in `aggregate-verdict.json`.

| Stage | Maker artifact | Producer | Checker artifact | Gate to continue | If gate fails |
|---|---|---|---|---|---|
| Setup | `run-manifest.json`, `skill-version.md` | Coordinator | coordinator self-check in `orchestration-trace.md` | selected cases, active revision, Enzyme source/context recorded | fix manifest/version before any case work counts |
| Substrate | `workspace-snapshot.json`, `enzyme-scan.json`, `enzyme-skill-diagnostic-probe.md` or probe section in `evaluator-trace.md`, `substrate-map.md`, `evaluator-trace.md` | Case-Level Enzyme Skill Diagnostic Probe / Diagnostic Runner / Substrate Mapper | `substrate-verdict.json`, `trace-verdict.json` | upstream Enzyme skill lens was loaded, selected workspace was diagnosed through it, evidence map is grounded, read-only boundary proven | route patch to orchestration/trace discipline or scan/audit/file-reading instructions; downstream artifacts become provisional |
| Failure model | `failure-model.md` | Failure Modeler | `failure-model-verdict.json` | failure modes have evidence, severity, confidence, retrieval impact, falsifiers | patch failure taxonomy/evidence rules; do not patch product copy from this case yet |
| Repair policy | `repair-policy.md` | Repair Policy Agent | `repair-policy-verdict.json` | repair tiers are concrete, reversible/approval-scoped, source-preserving | patch repair tiering/approval/preservation rules |
| Product translation | `product-translation.md` | Product Translation Agent | `product-translation-verdict.json` | translation preserves host-product promise while carrying real Enzyme failure modes | patch host-product translation or app UX surface |
| Final diagnostic | `agent-diagnostic.md`, `structured-diagnostic.json`, `adversarial-user-log.md`, `product-critic.md`, `diff-against-expected.md`, `interaction-log.md` | Final Diagnostic Writer / Adversarial User / Product Critic | `judge-verdict.json` | case score passes and no safety-critical zero | route patch by failed score category and upstream pass evidence |
| Regression | rerun updates to affected maker artifacts | Regression Hunter | `verifier-verdict.json`, updated pass verdicts, `rerun-comparison.md` | all affected pass verdicts and final verifier verdict pass | keep revision unpromoted; patch again or revert active revision |

`affected pass` means the pass whose instructions changed in the active
revision, plus every downstream pass that consumes it. For example, a
failure-model patch requires rerunning failure model, repair policy, product
translation, final diagnostic, and verifier artifacts for the target cases and
regression sample.

## Artifact Schemas

Artifacts may be prose or JSON, but they must expose enough structure for a
checker to decide pass/fail.

- `run-manifest.json`: run id, selected cases, UX surface, active skill revision,
  Enzyme source/context paths, child thread ids, and mutation policy.
- `skill-version.md`: active revision, prior revision lineage, source files
  read, Enzyme CLI version when used, and known limitations.
- `workspace-snapshot.json`: file/dir counts, tree, extensions, hidden/tool
  state, likely relevant files, large files, and mutation guard baseline.
- `enzyme-skill-diagnostic-probe.md`: case-level proof that an agent loaded the
  upstream Enzyme skill or ported evaluator context as operating instructions,
  inspected the selected workspace through the setup/indexability lens, ran or
  reviewed read-only scan evidence, read representative files, and produced
  diagnostic handoff claims before pass artifacts were assembled.
- `interaction-log.md`: adversarial questions, diagnostic answers, and whether
  answers preserved agency/capture/read-write boundaries.
- `diff-against-expected.md`: alignment and disagreement with
  `expected-diagnostic.md`, treating expected output as evidence rather than
  oracle.
- `improvement-plan.md`: aggregate failure pattern, pass that failed, proposed
  next agent, target cases, regression sample, and stop/patch rationale.
- `verifier-verdict.json`: case id, active revision, rerun scope, affected
  passes checked, pass/fail, failures, regressions, and validation commands.
- `skill-promotion-plan.md`: accepted revisions, promotion target, files to
  patch, evidence required before promotion, and reasons to keep a revision
  run-local.

## Promotion Path For Skill Improvements

Run-local revisions are not durable skill improvements by themselves. After a
revision passes the Regression Hunter gate, the Coordinator must write
`skill-promotion-plan.md` and classify the revision:

- **Promote to upstream Enzyme skill** (`../enzyme-rust/plugin/agent/SKILL.md`)
  when the change improves general Enzyme behavior: setup/indexability
  diagnosis, failure-mode taxonomy, repair tiers, scan/apply/petri/catalyze
  discipline, read/write safety, or source-structure preservation.
- **Promote to `ENZYME_EVALUATOR_CONTEXT.md`** when the change is loop-specific
  evaluator procedure, such as artifact gates, trace expectations, or fixture
  mutation guards.
- **Promote to host-product diagnostic prompt/app** when the change is specific
  to Margins copy, onboarding, note destinations, or UI behavior.
- **Keep run-local** when the revision only explains a one-off fixture result or
  experimental prompt variant.

Promotion requires a separate patch after verification. Do not mark a skill
improvement durable if it only exists under `runs/<run-id>/skill-revisions/`.

### 0. Port Enzyme Skill Context And Choose Active Revision

Before selecting cases, the Coordinator must read the Enzyme skill from
`../enzyme-rust/plugin/agent/SKILL.md` when available. Update
`ENZYME_EVALUATOR_CONTEXT.md` if the source skill has changed in ways relevant
to setup/indexability, read/write safety, existing-structure preservation, or
`enzyme scan` / `petri` / `catalyze` / `apply` usage.

Create or choose the active stacked revision under:

```text
runs/<run-id>/skill-revisions/
```

The baseline revision is `v000-baseline.md`. Every later patch must append
`vNNN-<short-reason>.md`; runner, judge, patcher, and verifier artifacts must
name the revision they evaluated.

### 1. Select Cases

Choose one of:

- **Smoke:** `fresh-empty`, `append-only-dated-log`,
  `codebase-plans-folder`, `google-drive-export`,
  `partial-enzyme-or-margins`
- **Full:** all cases under `test-cases/`
- **Targeted:** cases named by the current failure

Record selected cases in `run-manifest.json`.

Also record the UX surface being evaluated:

- `skill-only`
- `first-project-onboarding`
- `settings-project-picker`
- `readiness-line`
- `what-margins-noticed-panel`
- `build-private-search-cache`
- `import-history-entrypoint`

For skill-only runs, app screenshots are optional. For app UX runs, include the
surface copy or screenshots as additional artifacts under the run directory.

### 2. Substrate Pass: Evidence Before Interpretation

Before a Substrate Mapper writes `substrate-map.md`, a case-level Enzyme Skill
Diagnostic Probe must run for that case. The probe is allowed to create the
snapshot and scan evidence, but its required output is the diagnostic trace:
what the Enzyme skill instructed the agent to look for, what the workspace made
visible, what representative files confirmed, and which claims should flow into
the four passes.

For each case, record:

- file count;
- directory tree;
- extensions;
- hidden folders such as `.obsidian`, `.margins`, `.enzyme`;
- largest files;
- files likely to contain meetings, transcripts, imports, dates, people, or
  project names.

This is an artifact for the Judge. It is not a substitute for running the
diagnostic.

This phase is a guardrail, not the evaluator. Do not let file-count heuristics
replace agent judgment about the workspace.

The scan is substrate evidence, not the diagnostic itself. The diagnostic is
the agent process that applies the Enzyme skill's setup/indexability procedure
to the selected workspace, reads files after the scan, and records a trace of
the resulting hypotheses and falsifiers.

Prefer read-only inspection. A diagnostic run must not create or mutate:

- `.enzyme/`;
- `.margins/`;
- global `~/.enzyme/config.toml`;
- any user note or imported file.

When available, use a read-only Enzyme scan over the fixture workspace. Store
raw output as `enzyme-scan.json`. Canonical invocation should match the Enzyme
skill's vault-selection model:

```bash
enzyme scan -p <workspace>
```

`--vault <workspace>` is accepted by current CLI help, but `-p` is preferred in
loop prompts because the source skill describes vault path precedence as `-p`
flag, `ENZYME_VAULT_ROOT`, then current directory.

Do not run Enzyme commands that initialize, refresh, write config, generate
embeddings, or create cache state unless the run is explicitly testing the
separate "Initialize context" action.

Evaluators must use the ported Enzyme skill context in
`ENZYME_EVALUATOR_CONTEXT.md`. For fixture diagnosis, that means:

- run or review `enzyme scan -p <workspace>` without `--write-config`;
- when the fixture contains `.enzyme/` or `.margins/`, prefer scanning a temporary
  copy because the CLI may create runtime logs in existing tool-state folders;
- use scan output as indexability evidence, not as a command to restructure;
- keep `enzyme init`, `enzyme refresh`, `enzyme apply`, `petri`, and `catalyze`
  out of the read-only path when they would require initialization or writes;
- frame cache-building, import materialization, and structural repair as
  optional later actions, separate from first capture.

Record this in `evaluator-trace.md`, including:

- whether `../enzyme-rust/plugin/agent/SKILL.md` was read as operating
  instructions or the loop-local port was used as fallback;
- `enzyme --version`;
- exact Enzyme command(s), normally `enzyme scan -p <workspace>`;
- whether the scan ran against the fixture or a temporary copy;
- confirmation that `--write-config`, `init`, `refresh`, `apply`, `petri`, and
  `catalyze` were not used unless the case explicitly allowed writes;
- pre/post mutation guard results, including cleanup of generated runtime logs;
- scan output path;
- representative fixture files read after the scan.
- Enzyme-lens hypotheses formed before final artifact assembly, evidence that
  confirmed or falsified them, and handoff claims consumed by later passes.

The Substrate Mapper then writes `substrate-map.md`:

```md
# Substrate Map

## Corpus Shape
## Source Boundaries
## Already Indexable Signal
## Weak Or Missing Retrieval Handles
## Noise / Generated / Runtime Material
## Existing Conventions To Preserve
## Import / Raw Material
## Evidence Gaps
```

The Substrate Judge writes `substrate-verdict.json`. This verdict fails if the
map is only file counts, misses obvious raw/generated/import material, misses
existing conventions, or treats scan output as the whole workspace.

The Trace Judge writes `trace-verdict.json`. This verdict fails if the trace
only proves a static scan/snapshot was collected but does not prove a
case-level Enzyme Skill Diagnostic Probe occurred before artifact assembly.

### 3. Failure Model Pass: Explicit Retrieval Failure Modes

The Failure Modeler reads `substrate-map.md`, scan output, representative files,
and fixture expectations. It writes `failure-model.md`:

```md
# Failure Model

## Failure Modes

### <failure-mode-name>
- Evidence:
- Severity:
- Confidence:
- Retrieval impact:
- What would falsify this:
- What not to infer:

## Non-Failures
## Ambiguities
```

`## Non-Failures` and `## Ambiguities` are required sections, not optional
polish. `Non-Failures` should name healthy or sufficient retrieval signal so
the diagnostic does not over-pathologize a usable workspace. `Ambiguities`
should name what evidence cannot decide without user confirmation.

Each failure mode's `Retrieval impact` must be case-specific. Do not paste the
same generic impact sentence across modes or cases. The line should connect the
observed evidence to the concrete retrieval/setup risk, such as transcript
chatter outranking decisions, malformed frontmatter weakening date/entity
filters, scan undercoverage hiding useful docs, or mixed destinations
scattering future meeting continuity.

Good failure modes are concrete and retrieval-oriented:

- transcripts are present but not decision-bearing notes;
- people recur but have no stable handles;
- dates exist but are split across filenames, bodies, and metadata;
- source and generated artifacts are mixed;
- a folder scope is too broad for useful retrieval;
- prior tool state may pollute or confuse cache setup;
- a code repo has useful docs but source/build folders should be excluded.

Bad failure modes are vague or moralizing:

- "vault unhealthy";
- "needs cleanup";
- "schema inconsistent";
- "not Enzyme-ready" without evidence and retrieval impact.

The Failure Model Judge writes `failure-model-verdict.json`. This verdict fails
if the model misses the actual retrieval problem, invents fake problems, omits
severity/confidence/falsifiers, omits required `Non-Failures` or `Ambiguities`,
uses repeated generic retrieval-impact language, or collapses Margins onboarding
needs into Enzyme retrieval quality.

### 4. Repair Policy Pass: Restructure Options Without Commands

The Repair Policy Agent reads `failure-model.md` and writes `repair-policy.md`:

```md
# Repair Policy

## No-Op / Initialize As-Is
## Minimal Reversible Repair
## Medium Convention Repair
## Heavy Migration
## Approval Boundary
## Raw Source Preservation
## Do-Not-Touch List
## Recommended Path For This Product Surface
```

Repair policy is where restructuring is allowed. It should not be limited to
"do nothing"; it should rank concrete options by impact, effort, reversibility,
and risk. Examples:

- exclude generated/runtime folders;
- materialize raw exports into an import folder after preview;
- add date metadata to date-named notes only when that is deterministic;
- add stable wikilinks or entity fields only when existing conventions support
  them;
- split unrelated domains only as a heavier migration with backup/source
  preservation.

The Repair Policy Judge writes `repair-policy-verdict.json`. This verdict fails
if the policy is vague, jumps straight to migration, lacks approval boundaries,
would overwrite raw/source material, or refuses to propose any restructure when
the failure model shows one would materially improve retrieval.

### 5. Product Translation Pass: Host-Product Behavior

The Product Translation Agent turns the repair policy into the product surface
being evaluated. For Margins, it writes:

- `product-translation.md`;

The translation must preserve the host product promise. For Margins:

- first capture can start now;
- next-note destination is explicit;
- existing files are unchanged;
- repair/import/cache work is optional and separate;
- Enzyme retrieval improvements are explained as later quality improvements, not
  prerequisites.

For pure Enzyme setup, the same repair policy may translate differently:
"initialize as-is will be weak; do these two repairs first" can be valid when
the user asked for Enzyme setup quality rather than a host product's first
capture flow.

The Product Translation Judge writes `product-translation-verdict.json`. This
verdict fails if product copy hides the repair policy, blocks first capture for
Margins, or waters down a real Enzyme failure into vague reassurance.

### 6. Exploratory Brief

For compatibility with earlier runs, the Diagnostic Runner may also write
`exploratory-brief.md`. It should summarize the four pass artifacts rather than
replace them. Pass it:

- `DESIGN_CONTEXT.md`;
- `SPEC.md`;
- the case `manifest.json`;
- the read-only workspace snapshot;
- the read-only Enzyme scan output, if available.

The agent writes `exploratory-brief.md`:

```md
# Exploratory Brief

## Workspace Hypothesis

## Evidence That Supports It

## Evidence That Complicates It

## Meeting-Note Destination Hypotheses

## Import/Transcript History

## Questions A First-Time User Might Ask

## What The Diagnostic Must Not Infer
```

This brief should include competing hypotheses when the workspace is ambiguous.
For example, a daily-notes vault with a `meetings/` folder should not collapse
immediately into one convention.

### 7. Final Diagnostic Artifacts

The final diagnostic artifacts are produced by the Final Diagnostic Writer from
`product-translation.md`, not directly from file counts or manifest fields. Pass
the writer:

- `DESIGN_CONTEXT.md`;
- `SPEC.md`;
- the case `manifest.json`;
- the case `expected-diagnostic.md`;
- `workspace-snapshot.json`;
- `enzyme-scan.json`, if available;
- `substrate-map.md`;
- `failure-model.md`;
- `repair-policy.md`;
- `product-translation.md`;
- `exploratory-brief.md`.

The agent produces:

- a user-facing diagnostic in `agent-diagnostic.md`;
- a machine-readable summary in `structured-diagnostic.json`.

The final diagnostic must answer:

- what Margins noticed;
- what it is uncertain about;
- where the next meeting note should go;
- whether existing files will change;
- whether capture can start now;
- whether import/history could help later.

### 8. Adversarial User Interaction

Run a short interaction against the diagnostic. The Adversarial User Agent
should ask 3-5 questions chosen from the case context. It should push on the
diagnostic's weakest assumptions, not merely confirm it.

Examples:

- Empty folder: "Do I need to create folders first?"
- Codebase: "Will this touch source files?"
- Drive export: "Do I need to convert all of Drive to Markdown?"
- Transcript dump: "Are these transcripts enough, or do I need notes?"
- Messy vault: "Are you saying my existing structure is wrong?"
- Partial cache: "What is `.enzyme`, and is it safe to rebuild?"
- Append-only log: "Are you telling me I need one note per meeting forever?"
- Foreign-domain folder: "Why are you creating meeting folders in my recipe or
  finance files?"
- Sparse folder: "Do I need to import more history before this becomes useful?"

Record the transcript in `adversarial-user-log.md`.

The interaction should grade the answer qualitatively:

- did it reduce setup anxiety;
- did it preserve agency;
- did it make capture feel available;
- did it explain import/history without making it a prerequisite;
- did it avoid mechanism and vault-philosophy language?

### 9. Product Critic Review

Run a Product Critic after the diagnostic and adversarial interaction. The
critic reviews the experience as an Margins onboarding moment, not as a schema
matching exercise.

The critic may say:

- the skill is right and `expected-diagnostic.md` is wrong;
- the fixture is unrealistic or under-evidenced;
- the diagnostic is safe but not compelling;
- the guidance is accurate but too heavy for first run;
- the suggested default is sensible but the copy makes it sound like a mandate.

The critic writes `product-critic.md` with:

- strongest thing the diagnostic did;
- most serious trust risk;
- whether the user would feel ready to start capture;
- whether the user would understand import/history as optional;
- whether a skill, fixture, judge, or app-UX patch is needed.

The critic should explicitly answer which Margins UX surface is implicated:

- the skill diagnosed the workspace poorly;
- the skill was good, but the app surface would make the action feel too heavy;
- the readiness copy hides a write or overstates readiness;
- the import entry point is too early, too late, or too mandatory;
- the "What Margins noticed" surface is warranted or should stay collapsed;
- setup blocks capture when it should not.

### 10. Integrated Judge

The Judge writes `judge-verdict.json` per case:

```json
{
  "case_id": "codebase-plans-folder",
  "overall_score": 0,
  "scores": {
    "workspace_classification": 0,
    "safe_default": 0,
    "non_mutation_boundary": 0,
    "capture_not_blocked": 0,
    "import_history_handling": 0,
    "language_fit": 0,
    "uncertainty_honesty": 0,
    "evidence_grounding": 0
  },
  "passes": false,
  "findings": [],
  "regressions": [],
  "expected_diagnostic_issue": false,
  "fixture_issue": false,
  "skill_issue": false,
  "app_ux_issue": false,
  "recommended_next_agent": "skill_surgeon"
}
```

Use a 0-3 scale for each category:

- **0:** harmful or wrong;
- **1:** partially useful but confusing, overconfident, or too generic;
- **2:** acceptable and safe;
- **3:** strong, specific, and product-ready.

The Judge should cite:

- concrete fixture evidence;
- `exploratory-brief.md`;
- `agent-diagnostic.md`;
- `adversarial-user-log.md`;
- `product-critic.md`;
- `expected-diagnostic.md` only where it remains product-correct.

The Judge is allowed to mark `expected_diagnostic_issue: true` when the
expected output is too prescriptive, not evidence-backed, or contradicts
`DESIGN_CONTEXT.md`.

The Judge also writes `trace-verdict.json` per case:

```json
{
  "case_id": "codebase-plans-folder",
  "active_skill_revision": "skill-revisions/v000-baseline.md",
  "enzyme_context_loaded": true,
  "original_enzyme_skill_read_or_ported_context_used": true,
  "case_level_enzyme_skill_probe_present": true,
  "probe_preceded_artifact_assembly": true,
  "scan_treated_as_substrate_evidence_not_diagnostic": true,
  "read_only_boundary_proven": true,
  "trace_complete": true,
  "missing_trace_evidence": [],
  "trace_findings": []
}
```

If the trace does not prove that the Enzyme evaluator context was loaded, the
case is not covered by a valid judge pass even if `judge-verdict.json` exists.
If the trace does not prove that a case-level Enzyme Skill Diagnostic Probe
loaded the upstream skill or ported context as operating instructions, read the
workspace through that lens, and preceded artifact assembly, the maker pass is
not covered. A scan-only or snapshot-only trace is a trace failure even when all
downstream artifacts exist.

### 11. Aggregate And Patch Routing

`aggregate-verdict.json` should include:

- per-case score;
- pass/fail count;
- top recurring pass failures;
- top recurring skill failures;
- top recurring fixture failures;
- cases that regressed;
- whether the loop should patch the skill, patch fixtures, patch app UX, or
  stop.

The aggregate step should not merely average scores. It should assign the next
agent based on the pattern:

- use **Skill Surgeon** for repeated pass failures and route the patch to the
  failed instruction area:
  - substrate failures -> scan/audit/file-reading instructions;
  - failure-model failures -> failure taxonomy and evidence/falsifier rules;
  - repair-policy failures -> restructure tiers, approval, and preservation
    rules;
  - product-translation failures -> host-product copy/action separation;
  - trace failures -> orchestration, case-level probe prompts, Enzyme skill
    loading, diagnostic trace requirements, and mutation-guard discipline;
- use **Fixture Auditor** for under-evidenced or contradictory fixtures;
- use **Product Critic** when the expected behavior itself is questionable;
- use **App UX Patcher** when the skill is sound but setup copy/surface would
  still make the user distrust the flow;
- stop when failures are isolated, low-risk, or not actionable.

For app UX failures, the aggregate should name the target surface and the
specific user misunderstanding to resolve, such as:

- user thinks existing notes will be changed;
- user thinks capture is blocked until import;
- user cannot tell where the next note will go;
- user cannot distinguish read-only diagnosis from cache-building;
- user feels judged for having a messy folder.

If the target is an app surface, route to **App UX Patcher**, not Skill Surgeon,
unless the same finding also requires diagnostic prompt changes.

### 12. Skill Surgery And Patch

Patch one bounded surface at a time:

- diagnostic skill/prompt;
- Enzyme setup/indexability instructions;
- failure-mode taxonomy;
- repair-policy tiering;
- structured output schema;
- app setup copy;
- fixture evidence;
- judge criteria.

The default is to patch the skill/prompt when:

- multiple cases fail for the same language or reasoning problem;
- multiple cases miss the same indexability failure mode;
- repair advice is too vague, too timid, or too aggressive for the evidence;
- the diagnostic invents structure;
- the diagnostic blocks capture;
- the diagnostic cannot distinguish import history from starting fresh;
- the diagnostic fails to name read/write boundaries.

The Skill Surgeon must write `skill-patch-summary.md` with:

- failure pattern;
- changed skill instruction or prompt area;
- why the change generalizes;
- cases expected to improve;
- cases at risk of regression;
- examples of old vs new language.

Skill/prompt changes must stack. Each bounded patch creates a new revision under
`runs/<run-id>/skill-revisions/`, with the prior revision named explicitly. The
next runner/verifier pass must evaluate that active revision and record it in
`skill-version.md`, `run-manifest.json`, and `rerun-comparison.md`.

Patch fixtures only when:

- `expected-diagnostic.md` asks for behavior not evidenced by the workspace;
- the case is too thin to exercise its intended scenario;
- the manifest contradicts actual files.

### 13. Regression Hunt

Rerun the same cases after patching. Do not add easier cases to claim success.

The Regression Hunter should also run at least two unrelated cases that were not
the patch target when doing targeted runs. For example:

- after patching Google Drive import handling, rerun a codebase and empty
  folder;
- after patching daily-note handling, rerun transcript dump and messy vault;
- after patching non-mutation copy, rerun partial cache and append-only log.

`rerun-comparison.md` should include:

- before/after scores;
- fixed findings;
- new regressions;
- examples of improved user-facing language;
- any cases that still fail and why.

The Regression Hunter must also write or update `trace-verdict.json` for every
rerun case. A rerun only counts as verification when both the diagnostic behavior
and every affected pass verdict pass.

`verifier-verdict.json` must include:

```json
{
  "case_id": "google-drive-export",
  "active_skill_revision": "skill-revisions/v002-example.md",
  "rerun_scope": ["failure-model", "repair-policy", "product-translation", "final-diagnostic"],
  "affected_passes_checked": {
    "failure_model": true,
    "repair_policy": true,
    "product_translation": true,
    "final_diagnostic": true,
    "trace": true
  },
  "passes": true,
  "fixed_findings": [],
  "regressions": [],
  "remaining_risks": [],
  "validation_commands": []
}
```

### 14. Memory Synthesis

At the end of the run, write or update `memory-notes.md`:

```md
# Workspace Diagnosis Memory

## Language That Worked

## Language That Failed

## Reusable Workspace Patterns

## Import/Transcript Lessons

## Read/Write Boundary Lessons

## Skill Blind Spots

## Fixture Gaps To Add Later
```

This is the loop's durable learning layer. It should summarize what future
agents should remember without requiring them to reread all artifacts.

## Scoring Rubric

### Workspace Classification

Does the diagnostic correctly identify the workspace shape?

Good:

- "This looks like a project repo with docs and planning notes."
- "This looks like a transcript dump, not a finished notes folder."
- "This vault has daily notes and a few standalone meeting notes."

Bad:

- "This is an unhealthy vault."
- "This is an Obsidian vault" when the folder is a codebase or Drive export.

### Safe Default

Does it propose the least surprising place for future meeting notes?

Good:

- `docs/meetings/` for a codebase with `docs/`;
- `meetings/` for a plain folder;
- `Daily/` or `meetings/` only when evidence supports the tradeoff;
- isolated `Margins meetings/` for foreign-domain folders.

Bad:

- creating people folders without evidence;
- moving existing notes;
- inventing a full taxonomy.

### Non-Mutation Boundary

Does it clearly say existing files will not be changed?

Good:

- "This only chooses where new notes go."
- "Building context creates a private cache; your notes are not edited."
- "Import can copy converted files into a chosen folder after preview."

Bad:

- silent indexing;
- "clean up";
- "normalize";
- "restructure."

### Capture Not Blocked

Can the user start recording without importing, cleaning, or indexing?

Good:

- "You can start now."
- "Importing old notes can help later."

Bad:

- "Before your first capture, organize/import/index everything."

### Import History Handling

Does it handle Drive exports, transcripts, and previous notes as optional
context?

Good:

- distinguishes source files from converted copies;
- preserves originals;
- names unsupported or messy files without judgment.

Bad:

- requires Markdown conversion up front;
- treats transcripts as finished notes;
- hides what will be copied.

### Language Fit

Is the language receptive to a meeting-notes user?

Good:

- "meeting notes";
- "past notes";
- "transcripts";
- "where new notes go";
- "your files will not be changed."

Bad:

- "append-only log";
- "schema";
- "vault health";
- "catalyst";
- "embedding";
- "knowledge base optimization."

### Uncertainty Honesty

Does it name weak evidence?

Good:

- "I see a few dated files, but not a consistent convention yet."
- "There are transcripts here, but not many finished notes."

Bad:

- overconfidently inferring conventions from one file.

### Evidence Grounding

Can each claim point to concrete files or patterns?

Good:

- cites folder names, filename patterns, frontmatter/body labels, import logs,
  or transcript artifacts.

Bad:

- broad claims that do not appear in the fixture.

## Stop Conditions

Stop and report instead of patching when:

- the diagnostic substrate would mutate the fixture;
- the skill cannot be run read-only;
- artifacts are missing or invalid;
- the Judge marks the fixture, not the skill, as the blocker for more than two
  cases in the selected run;
- the same regression appears after two patch attempts.

## Minimum Passing Bar

For a smoke run:

- all selected cases have `overall_score >= 2`;
- no case scores `0` on non-mutation boundary or capture-not-blocked;
- no case suggests restructuring existing files;
- at least one import/transcript case distinguishes import history from
  starting capture.

For a full run:

- at least 10 of 12 cases pass;
- all failures have concrete skill or fixture findings;
- no high-risk regression in language fit, non-mutation boundary, or capture
  availability.

## Suggested Thread Split

Use cheap coding agents for mechanical fixture and artifact work. Use stronger
review agents for product judgment.

- Coordinator: parent thread
- Fixture Auditor: `gpt-5.5`, read-only
- Diagnostic Runner: `gpt-5.3-codex-spark` or equivalent, read-only
- Exploratory Diagnostic Agent: `gpt-5.5` for broad runs, Spark for cheap
  targeted cases
- Adversarial User Agent: `gpt-5.5` or Claude Opus when UX-sensitive
- Product Critic: Claude Opus or `gpt-5.5`, read-only
- Judge: `gpt-5.5` or Claude Opus, read-only
- Skill Surgeon: Codex, workspace-write
- Regression Hunter: `gpt-5.3-codex-spark`, read-only
- Memory/Synthesis Agent: `gpt-5.5`, read-only unless updating run artifacts

The Skill Surgeon should not be the final judge. The Product Critic should see
diagnostic artifacts before reading skill diffs.
