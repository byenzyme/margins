# v001 Case-Level Enzyme Probes

Prior revision: `v000-baseline.md`

## Failure Pattern

The failing behavior was orchestration and loop discipline, not a single
workspace-class diagnostic rule. The maker pass became artifact-oriented: it
produced snapshots, scans, pass files, and final diagnostic artifacts, but the
loop did not force a case-level agent to use the upstream Enzyme skill as the
diagnostic procedure before artifact assembly.

The result is a false sense of coverage. A run can contain `substrate-map.md`,
`failure-model.md`, `repair-policy.md`, and `product-translation.md` while the
trace only proves static evidence collection. That misses the intended skill
development process: spawn case-level agents that load the Enzyme skill, read
the test workspace through its setup/indexability lens, and produce diagnostic
trace evidence before writing or checking artifacts.

## Changed Instruction Area

Patch target: loop-local orchestration, runner prompts, evaluator trace
requirements, and judge coverage gates.

New instruction:

- The Coordinator must spawn case-level Enzyme Skill Diagnostic Probe threads
  before pass artifact assembly.
- Each probe prompt must explicitly load
  `../enzyme-rust/plugin/agent/SKILL.md` as operating instructions when
  available, then load `ENZYME_EVALUATOR_CONTEXT.md` and the active stacked
  revision.
- Each probe must inspect the selected `test-cases/<case-id>/workspace/`
  through the Enzyme setup/indexability lens, run or review
  `enzyme scan -p <workspace>` under the read-only mutation policy, read
  representative files, and write a diagnostic trace.
- The trace must name the skill source loaded, case scope, scan command,
  mutation guard, representative files read, hypotheses, confirmed/falsified
  claims, and handoff claims for the four passes.
- Static scan, directory snapshot, file-count, manifest-summary, or template
  filling work does not count as a full maker pass.
- Judges must fail trace coverage when artifacts exist but the trace does not
  prove the Enzyme skill lens was used as an agent process before artifact
  assembly.

## Why This Generalizes

This change is process-level. It does not tune the answer for a specific
fixture. It forces every workspace class to pass through the same diagnostic
shape that the upstream Enzyme skill expects:

- read existing structure before suggesting structure;
- treat `enzyme scan` as evidence, not the whole diagnosis;
- identify what is already indexable, weak, raw, noisy, or risky;
- preserve source material and read/write boundaries;
- separate Enzyme setup quality from Margins's first-capture promise.

That discipline applies to raw imports, transcript dumps, code repos,
partially initialized tool-state folders, append-only logs, empty folders, and
messy Obsidian vaults. The case-specific conclusions can vary, but the maker
must show the same skill-driven reasoning path.

## Expected Cases To Improve

- `google-drive-export`: should move from "Drive export has files" toward a
  probe-backed distinction between raw source material, materialized markdown,
  import boundaries, and optional later conversion.
- `zoom-transcript-dump`: should force the agent to inspect transcript content
  and decide whether it is searchable source text, weak decision evidence, or
  enough for a minimal next-note default.
- `codebase-plans-folder`: should reduce the risk that generated/source folders
  dominate the diagnosis and should force the agent to preserve code boundaries
  while identifying docs/plans as usable signal.
- `partial-enzyme-or-margins`: should make the mutation guard and stale/runtime
  tool-state interpretation explicit instead of treating hidden folders as a
  generic scan fact.
- `append-only-dated-log`: should protect the "already indexable enough" case by
  requiring the agent to name non-failures rather than inventing repairs from a
  template.

## Cases At Regression Risk

- `fresh-empty`: the extra probe step could overstate Enzyme setup concerns when
  there is little to diagnose. The expected result should remain a lightweight
  start-now default.
- Highly structured Obsidian vaults: probes could become too Enzyme-centric and
  recommend unnecessary repair despite strong existing conventions.
- Very small plain folders: agents may add process verbosity without improving
  the user-facing diagnostic. The trace should stay internal; product copy
  should remain concise.
- Any environment without access to `../enzyme-rust`: the fallback must be
  explicit and use `ENZYME_EVALUATOR_CONTEXT.md`, not silently skip the skill
  lens.

## Old vs New Example Language

Old coordinator/maker prompt language:

```text
Read ENZYME_EVALUATOR_CONTEXT.md and the active skill revision. Run the current
Enzyme-backed evaluation against each case. Produce substrate-map.md,
failure-model.md, repair-policy.md, product-translation.md, evaluator-trace.md,
agent-diagnostic.md, and structured-diagnostic.json.
```

Problem: this allows the child to treat scan output and fixture files as inputs
to artifact writing without first performing a case-level Enzyme diagnostic.

New coordinator/maker prompt language:

```text
For each assigned case, first run a case-level Enzyme Skill Diagnostic Probe.
Read ../enzyme-rust/plugin/agent/SKILL.md as operating instructions; if it is
not accessible, say so and use ENZYME_EVALUATOR_CONTEXT.md as the ported lens.
Then read the active revision, inspect test-cases/<case-id>/workspace/ through
the Enzyme setup/indexability procedure, run or review enzyme scan -p
<workspace> under the read-only policy, read representative files, and write a
diagnostic trace with hypotheses, confirmed/falsified evidence, mutation guard,
and handoff claims. Only after that trace exists may you assemble
substrate-map.md, failure-model.md, repair-policy.md, product-translation.md,
agent-diagnostic.md, or structured-diagnostic.json. A scan/snapshot-only pass is
provisional and does not count as maker coverage.
```

Old judge language:

```text
If the trace proves the Enzyme evaluator context was loaded and the read-only
boundary held, the trace can pass.
```

New judge language:

```text
Trace coverage requires proof that a case-level Enzyme Skill Diagnostic Probe
occurred before artifact assembly: the upstream skill or ported context was used
as operating instructions, the selected workspace was read through that lens,
scan was treated as substrate evidence, representative files were inspected,
and diagnostic handoff claims were recorded. Static scan/snapshot-only traces
fail even when downstream artifacts exist.
```
