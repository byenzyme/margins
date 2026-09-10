# Coverage Matrix

| Case | Iteration 1 v000 artifacts | Iteration 2 v001 probe | Key coverage result | Status |
|---|---:|---:|---|---|
| `google-drive-export` | complete | complete | Drive/export material requires source-boundary and import-churn diagnosis beyond scan counts. | v001 probe passed; downstream artifacts provisional |
| `zoom-transcript-dump` | complete | complete | Scan saw one Markdown note; raw TXT/VTT/CSV/JSON transcript corpus is the real weak-indexability issue. | v001 probe passed; downstream artifacts provisional |
| `codebase-plans-folder` | complete | complete | Scan saw only README; representative reads found mixed docs/plans/notes/raw transcript surfaces. | v001 probe passed; downstream artifacts provisional |
| `partial-enzyme-or-margins` | complete | not rerun | Temp-copy scan path covered stale tool state in v000. | needs v001 probe in next run |
| `append-only-dated-log` | complete | not rerun | Mostly indexable control case; should guard against invented repairs. | needs v001 probe in next run |

## Stage-Gate Results

| Gate | Result | Notes |
|---|---|---|
| Setup | pass | Run ID, active revision, selected cases, paths, and mutation policy recorded. |
| v000 Substrate | pass with caveat | Artifacts exist for five cases; v001 later requires probe trace before counting as clean coverage. |
| v000 Failure Model | pass with caveat | Failure modes are evidence-aligned but inherited v000 trace weakness. |
| v000 Repair Policy | pass with caveat | Tiers generally preserve source material and approval boundaries. |
| v000 Product Translation | pass with caveat | Start-now promise preserved, but artifacts predate probe-first requirement. |
| v001 Probe Gate | pass for 3 cases | Probe threads loaded upstream skill, scanned/read workspace, wrote hypotheses/falsifiers/handoff claims. |
| v001 Downstream Rerun | incomplete | Four-pass artifacts were not fully regenerated from probe traces. |
