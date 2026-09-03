# Margins open-source boundary

Margins publishes a portable Rust workspace for local Markdown workflows. A
checkout can:

- declare and inspect Workspaces and local Markdown Sources;
- initialize and sync those declarations;
- search them at query time with source-backed lexical recall;
- use reviewable plan/apply automation when direct declaration is not enough;
- turn the latest or a selected Margins session into a connected note with the
  included skill, with supplied transcripts, memos, and supported audio as
  explicit alternatives.

These are the standard workflows in the published CLI. Its capability report
lists the workflows included in the current binary; other Margins builds may
add capabilities to the report.

## Setup

Setup starts by naming the practice the user wants Margins to remember and
choosing its home and read-only reference folders. `margins guide
workspace-setup` gives an agent the complete understanding, correction, consent,
and proof loop. For a single notes folder, its direct command path is:

```bash
cd /absolute/path/to/notes
margins init
margins sync --json
margins recall "a distinctive phrase"
```

For multiple local folders, create a named Workspace and add Sources directly
with `source add`. Users do not need to write a desired TOML document.
`workspace plan` and `workspace apply` remain an optional deterministic
automation path over the same public Workspace protocol.

Some Margins builds may report `recall.scan: true`. The setup agent consumes the
complete read-only `scan.v2` result as grounded evidence about the Workspace home,
then explains what Margins understands the practice to be and asks in plain
language what is wrong or missing. Your Source declarations still define the full
recall boundary. If settings follow from the confirmed understanding, the agent
compiles a fresh plan with `workspace plan --desired`; the plan finally applied is
always the exact one last reviewed, unchanged, before `init` and `sync`.

## Distillation

Setup ends with a declared, synced Workspace and a live recall proof.
Distillation is a separate skill workflow. It normally resolves the selected
Workspace's latest session through `transcript latest`, reads its transcript and
memo evidence, and produces a reviewed connected note. Users can instead select
a session or supply a transcript, memo, or supported audio file. The published
CLI searches declared Markdown at query time and returns paths to the supporting
notes. Other builds can provide additional recall modes through the same
`margins recall` result contract.

This boundary reuses the existing Workspace plan and recall evidence contracts
rather than introducing a second setup format.

## Export discipline

`open-source-boundary.json` is a fail-closed literal allowlist. The exporter
rejects globs, missing required files, overlapping ownership, untracked files,
denied paths, and sensitive personal data. New files are not exported unless
the manifest selects them explicitly.

The manifest and policy do not themselves grant a license; the licenses in the
exported files do. Export does not imply that every crate name is published or
reserved on crates.io, and it does not grant rights to Margins trademarks.
