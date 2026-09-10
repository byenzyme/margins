# Skill Patch Summary

Run ID: `2026-06-25-workspace-diagnostic-123440`

Active revision: `skill-revisions/v001-temp-copy-scan-guard.md`

## Failure Pattern

`enzyme scan` without `--write-config` created
`test-cases/partial-enzyme-or-margins/workspace/.enzyme/enzyme.log` because the
fixture already contained `.enzyme` state. The generated file was removed. The
failure is a read-only boundary gap: avoiding `--write-config` is not sufficient
when runtime state already exists in the workspace.

## Changed Skill Instruction Area

The new revision builds on v000 and changes the evaluator procedure for the
Enzyme scan substrate:

- detect `.enzyme/` or `.margins/` before scanning;
- scan a temporary copy for those cases;
- record pre/post mutation guards in `evaluator-trace.md`;
- treat private cache build, import/materialization, and retrieval repair as
  separate optional later actions.

## Why It Generalizes

Partial cache/app state is common in real user folders and in failed or resumed
setup attempts. A temporary-copy scan preserves the product promise that reading
workspace shape is distinct from writing cache state. Separating cache, import,
and repair applies across empty folders, codebases, Drive exports, transcript
dumps, daily notes, and messy vaults because none of those actions should block
the next capture.

## Cases Expected To Improve

- `partial-enzyme-or-margins`
- `mixed-messy-power-vault`
- `google-drive-export`
- `zoom-transcript-dump`
- future fixtures with stale `.enzyme/`, `.margins/`, or failed initialization
  state

## Cases At Risk Of Regression

- `fresh-empty`: do not make setup sound heavier than one clear note
  destination.
- `append-only-dated-log`: do not dilute the existing dated-log convention with
  unnecessary cache/repair caveats.
- `codebase-plans-folder`: keep the docs meeting-note destination explicit while
  preserving the source-file non-mutation boundary.

## Old vs New Language

Old:

> I ran a read-only Enzyme scan without `--write-config`, so the fixture was not
> changed.

New:

> This workspace already has tool state, so I scanned a temporary copy and
> compared the original before and after. The original files were unchanged.

Old:

> Initialize Enzyme later if you want better retrieval.

New:

> Building the private search cache is separate from import and repair. It
> writes cache files, not note edits, and none of those actions block the next
> capture.

## Regression Sample To Run

Run a targeted verifier over:

- `partial-enzyme-or-margins`
- `fresh-empty`
- `append-only-dated-log`
- `codebase-plans-folder`
- `google-drive-export`

The verifier should confirm that fixture files remain unchanged, v001 is named
in every trace, and cache/import/repair are described as optional separate
actions.

---

# v002 Patch Summary

Active revision: `skill-revisions/v002-import-narrative-and-language.md`

## Failure Pattern

The judge found v000 safe but shallow: import/cache/export prose was generic,
manifest convention tags leaked into user-facing copy, visible cache/import
files were underreported, and ambiguous destinations were not always named.

## Changed Skill Instruction Area

v002 builds on v001 and changes the diagnostic language layer:

- name Drive/Zoom/import/cache/export state in plain language;
- keep history/import/cache optional and separate from first capture;
- translate raw convention tags into outcome language;
- report cache/import/export evidence regardless of dot-prefix;
- name competing note destinations and calibrate confidence.

## Why It Generalizes

This improves evidence accuracy without weakening the safety promise. Users with
messy history get clearer recognition of what Margins noticed, while empty or
clean workspaces keep the short "start now" path.

## Cases Expected To Improve

- `google-drive-export`
- `zoom-transcript-dump`
- `partial-enzyme-or-margins`
- `mixed-messy-power-vault`
- `frontmatter-obsidian`
- `sparse-random-notes`
- `daily-notes-vault`
- `rfc-decision-log`

## Cases At Risk Of Regression

- `fresh-empty`
- `append-only-dated-log`
- `codebase-plans-folder`

## Old vs New Language

Old:

> History can help later, but it is optional.

New:

> I see an export log and half-converted notes. Margins can use them later after
> preview; the next capture can still start now, and those files stay where they
> are.

## Regression Sample To Run

- `google-drive-export`
- `zoom-transcript-dump`
- `partial-enzyme-or-margins`
- `daily-notes-vault`
- `rfc-decision-log`
- `fresh-empty`
- `append-only-dated-log`
- `codebase-plans-folder`
