# v002 Import Narrative And Language

Builds on: `skill-revisions/v001-temp-copy-scan-guard.md`.

## Failure Pattern

The Enzyme-aware judge found that v000/v001 was safe but too generic in the
parts that should help a first-time user understand how their existing history
fits:

- import/cache/export prose was near-identical across empty, clean, and
  import-heavy workspaces;
- raw fixture convention tokens leaked into user-facing copy and structured
  patterns;
- visible cache/import/export files were underreported when they were not in
  dot-prefixed tool folders;
- competing destinations and confidence tradeoffs were sometimes collapsed into
  one answer.

## Changed Instruction Area

After the Enzyme scan and representative file reads, diagnostics must translate
indexability evidence into plain setup language:

1. **Name import/cache/export state specifically.** If the workspace contains
   Drive exports, Zoom/VTT/TXT transcript dumps, cache-state files, import
   manifests, interrupted migration markers, duplicate copies, or half-converted
   notes, name that state in user-facing prose. Keep the action optional:
   history can help later, but it does not block the next capture.
2. **Translate convention tags.** Do not surface raw tokens such as
   `malformed-frontmatter`, `wikilinks`, `csv-headers`, `none`, or `mixed`.
   Translate them into outcome language, for example: "some notes have
   inconsistent metadata, which is fine and will not be changed."
3. **Report cache/import/export evidence regardless of dot prefix.** A visible
   `cache-state.json`, `import/drive-manifest.json`, export log, or transcript
   manifest is setup evidence even when no hidden `.enzyme/` or `.margins/`
   directory exists.
4. **Name competing destinations.** When two plausible destinations exist, name
   the alternative and the tradeoff. Use lower confidence when the structure is
   strong but not clearly a meeting-note workflow.

## Why It Generalizes

The change improves trust without making setup heavier. Users with exports,
transcripts, partial caches, or messy notes need to hear what Margins noticed in
their terms, while users with empty or clean folders still need the short
capture-now promise. Translating scan/manifest vocabulary into outcome language
preserves the Enzyme setup lens without exposing mechanism or making cleanup a
prerequisite.

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

- `fresh-empty`: richer import prose must not add irrelevant setup anxiety.
- `append-only-dated-log`: do not imply its existing dated log must become a new
  structure.
- `codebase-plans-folder`: do not make source files or project docs sound like
  they will be rewritten.

## Old vs New Language

Old:

> History can help later, but it is optional.

New:

> I see a Drive export log and a few half-converted meeting notes. Margins can use
> those later as optional history after preview; the next capture can still
> start now and the exported files stay where they are.

Old:

> Metadata conventions: malformed-frontmatter, mixed, wikilinks.

New:

> Some notes have inconsistent metadata and some use links. That is useful
> signal, not something Margins needs to normalize before capture.

Old:

> No hidden app/cache folders visible.

New:

> I see cache/import state files, so cache-building and import recovery should
> stay separate from choosing the next-note destination.

## Regression Sample To Run

Rerun affected import/language cases plus low-import regressions:

- `google-drive-export`
- `zoom-transcript-dump`
- `partial-enzyme-or-margins`
- `daily-notes-vault`
- `rfc-decision-log`
- `fresh-empty`
- `append-only-dated-log`
- `codebase-plans-folder`
