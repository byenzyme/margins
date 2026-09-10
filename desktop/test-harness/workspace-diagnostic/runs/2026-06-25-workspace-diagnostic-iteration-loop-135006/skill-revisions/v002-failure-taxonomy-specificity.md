# v002 Failure Taxonomy Specificity

Prior revision: `v001-case-level-enzyme-probes.md`

## Failure Pattern

The independent checker passed the three priority cases but found a recurring
Failure Model weakness in all three: the artifacts omitted required
`## Non-Failures` and `## Ambiguities` sections, and reused a generic
`Retrieval impact` sentence across modes and cases.

That is the failed pass area. Product translation and repair policy were strong;
the next improvement belongs in failure modeling, not app copy.

## Changed Instruction Area

Patch target: Failure Model Pass and Enzyme evaluator failure-mode discipline.

New instruction:

- Every `failure-model.md` must include `## Non-Failures` and `## Ambiguities`.
- `Non-Failures` must name healthy retrieval signal or "already indexable
  enough" conditions.
- `Ambiguities` must name what the workspace evidence cannot decide without
  user confirmation.
- Every failure mode needs a distinct, evidence-specific `Retrieval impact`.
- Repeated generic impact lines are a failure-model defect.

## Why This Generalizes

Poorly structured workspaces often contain both real retrieval hazards and
usable signal. Drive exports can contain one good note beside malformed imports;
transcript dumps can be searchable source but weak decision evidence; code repos
can contain useful docs while source/build files are mostly noise; append-only
logs can be good enough despite minor drift.

Naming non-failures and ambiguities prevents the skill from moralizing messy
workspaces while still diagnosing concrete indexability risks.

## Cases Expected To Improve

- `google-drive-export`: should name usable materialized notes as non-failures
  while distinguishing malformed/partial exports as failures.
- `zoom-transcript-dump`: should distinguish searchable transcript source from
  weak decision-bearing evidence.
- `codebase-plans-folder`: should distinguish useful docs/plans from codebase
  files that should not drive meeting-memory repair.
- `append-only-dated-log`: should protect the ready-now control case by naming
  "no material failure" where appropriate.
- `partial-enzyme-or-margins`: should separate stale tool state risk from healthy
  existing notes.

## Cases At Risk Of Regression

- Very small or empty folders, where forced sections could become verbose.
- Already structured vaults, where agents may invent ambiguities to satisfy the
  template.

Mitigation: allow concise sections and explicitly permit "No material failure"
when evidence supports it.

## Old vs New Example Language

Old:

```text
- Retrieval impact: weak or misleading retrieval handles for setup and later
  meeting context.
```

New:

```text
- Retrieval impact: the scan may omit the docs that actually contain meeting
  decisions, so a setup preview based only on scan output could choose the wrong
  destination or understate available context.
```

Old:

```text
# Failure Model
## Failure Modes
...
```

New:

```text
# Failure Model
## Failure Modes
...
## Non-Failures
- The workspace already has enough dated meeting notes to start capture now.
## Ambiguities
- It is unclear whether old transcript exports should be imported or merely
  preserved as source material; ask before materializing them.
```

## Verification Needed

Rerun the Failure Model pass for the three judged priority cases plus
`partial-enzyme-or-margins` and `append-only-dated-log`. The checker should fail
any failure model missing `Non-Failures`, missing `Ambiguities`, or using
copy-pasted retrieval-impact text.
