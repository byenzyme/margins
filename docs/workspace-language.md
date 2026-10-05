# Workspace configuration in the Enzyme workspace language

Status: in progress (2026-10-05); public side implemented (see below). Decided by Joshua: adopt the `.enzyme`
workspace language as the single Workspace configuration format; make
`enzyme-spec` a public crate (prepared, not published); deliver as draft PRs.

## Why

Before this change one Workspace was described four ways: Margins'
`workspaces/<id>/config.toml`, Margins' private mirror of Enzyme's TOML engine
config (`src/workspace_recall.rs`), Enzyme's own TOML, and Enzyme's `.enzyme`
language. Margins translated between them through a temporary engine home on
every index run, and pinned a side-branch engine 124 commits behind master.

After it, a Workspace is one program in one grammar, parsed by one parser.

## Layout

```
~/.margins/                          MARGINS_HOME, machine-local, never synced
  config.toml                        host preferences: [workspace] default, [llm], [retention]
  configs/                           one .enzyme namespace, owned by Margins
    <id>.enzyme                      workspace "<id>" { … } — the whole Workspace config
    profiles.enzyme                  optional shared custom profiles
  workspaces/<id>/                   state only, no configuration
    index.db  ledger.db  receipts/  …
  google/<account>/  granola/<account>/   connection state and tokens (unchanged)
```

Nothing is written into a notes folder except notes Margins creates in its
declared write folder. Nothing is shared with `~/.enzyme`: Margins parses its own
`configs/` with `enzyme-spec` and hands the engine an in-memory configuration; the
engine never reads Margins files or Enzyme's home.

## The program

```enzyme
workspace "practice" {
  source markdown "notes" { path "~/notes" }
  source markdown "library" { path "~/library" }
  source google-mail "mail" {
    account "me@example.com"
    query "-in:spam -in:trash"
    backfill days 365
  }
  source google-calendar "calendar" { account "me@example.com" }

  remember in folder "inbox" in source "notes" create note

  leave out folders { "archive" }
  learn questions from folder "people" including linked pages about relationships
  learn questions from source "mail"
}
```

Mapping from the retired `config.toml`:

| `config.toml` | program |
|---|---|
| `[bindings.x] kind = "notes"` | `source markdown "x" { path … }` |
| `role = "home"`, `note_folder` | `remember in folder "<note_folder>" in source "x" create note` (exactly one per Margins Workspace; `"."` = the source root) |
| `kind = "captures"` | `source margins-captures "x" { path … }` |
| `kind = "google-mail" / "google-calendar" / "google-meet" / "granola"` | host sources of the same kind name, fields as below |
| `policy.excluded_folders / excluded_tags / excluded_entities` | `leave out folders / tags / links` |
| `policy.entities` (`ref`, `profile`, `expandable`) | `learn questions from <kind> "<name>" [including linked pages] [about <profile>]` |
| `[retention]` | machine `config.toml` `[retention]` (global) with optional `[retention.<id>]` override |

Host source fields (all optional unless noted):

- `google-mail`: `account` (required), `query`, `backfill days N`
- `google-calendar`: `account` (required), `lookback days N`, `lookahead days N`
- `google-meet`: `account` (required)
- `granola`: `account` (required), `time range "last_30_days"`, `workspace only true|false`
- `margins-captures`: `path` (required)

Defaults equal the current `default_declaration()` values and are omitted when
rendering.

Margins lowers host sources to the native SQLite sources it already builds over
`ledger.db` (and the capture registry) before handing the program to the engine,
so readings such as `learn questions from source "mail"` keep working. Users
never see ledger SQL. Margins also adds its own projection-tag exclusion during
lowering; it is not written into user files.

Write enforcement stays in Margins: the program states intent, Margins refuses a
note destination outside the declaring Markdown source.

## Plan and apply

- `workspace plan --desired <file.enzyme>` parses, host-lowers and validates the
  complete desired program; the plan records `base_revision` (SHA-256 of the
  current program bytes, or `absent`), the desired program text and its digest, a
  unified diff, and a human-readable `actions` summary.
- `workspace apply --plan <plan.json>` refuses a plan whose digest does not match
  its text, or whose base no longer matches the file; otherwise atomically writes
  the program and a receipt. Re-applying an applied plan is a no-op.
- Commands that change sources (`connect`, `integrations`, `add/remove source`)
  edit the program AST and re-render it through `enzyme_spec::render_program`
  (comments are not preserved, matching Enzyme's own readable-file writes).

## Migration

On first resolution of a Workspace that has `workspaces/<id>/config.toml` and no
`configs/<id>.enzyme`, Margins converts it deterministically, validates the
result, writes the program atomically, and renames the old file to
`config.toml.migrated`. The conversion is idempotent and covered by fixtures for
every binding kind. `margins workspace migrate --dry-run` prints the program
without writing.

## Engine changes (enzyme-rust)

- Any number and mix of sources per workspace; vault-body statements
  (`leave out`, `references in fields`, `remember in`, `when asked`, …) allowed in
  every workspace. A workspace with exactly one Markdown source and nothing else
  keeps its legacy lowering and index identity.
- Host source kinds: `source <kind> "name" { <words> <value> … }` parses and
  renders for any unknown kind; a host lowers them before resolution.
- `remember in folder "<dir>" [in source "<name>"] [when { … }] create note [{ … }]`
  compiles to agent instructions and marks the declaring Markdown source writable
  (other Markdown sources of a multi-source workspace become read-only).
- An in-memory runtime API builds the engine configuration for one workspace from
  a resolved program, with no config file or `ENZYME_HOME` lookup, and the
  indexer takes its sources, roots, exclusions, logs and frontmatter fields from
  it.
- `enzyme-spec` is self-contained and ready to publish as a public crate.

## Margins implementation (public side)

- `margins_workflows::workspace_program` owns the program: `WorkspaceProgram`
  (text + AST), `derive_view` (program → the read-only `WorkspaceConfig` view the
  rest of Margins reads), and `reconcile` (edit the AST toward a desired view,
  keeping every statement the view cannot express: learning settings, inline and
  shared profiles, name patterns, `when asked`, create-note conditions and
  guidance). Only Markdown and the five Margins host kinds are accepted; unknown
  host fields are errors.
- `ResolvedWorkspace { program, config, … }`: `config_path` is the program file;
  `workspace_revision` hashes its bytes. Display names live in machine config
  `[workspace.names]`.
- Language validation lowers host sources to placeholder SQLite sources and runs
  `enzyme_spec::resolve` with the optional `configs/profiles.enzyme`, so readings,
  profiles, folder qualification and exclusions are checked by the engine's own
  resolver. The real ledger lowering belongs to the engine path.
- Plan JSON is `margins.workspace.plan.v2`: `workspace_id`, `base_revision`,
  `plan_id`, `actions` (each with a `summary`; `update_program` when the change is
  outside the typed view), `desired_program`, `desired_sha256`, `diff`. A legacy
  `.toml` desired file is converted with the migration rules onto the current
  program. `workspace compile` emits `margins.workspace.compile.v2` with
  `desired_program` (no `desired_toml`).
- Migration also runs explicitly: `margins workspace migrate [--dry-run] [--json]`.
  Enzyme's implicit folder exclusions (`.git`, `node_modules`, …) are not written.
  With several Markdown sources, unqualified legacy folder references are
  qualified with the Home source name.

## Open items

- Publish `enzyme-spec` publicly (repository, license choice). Until then
  `margins-workflows` depends on it by path
  (`../../../../enzyme-rust-worktrees/margins-workspace-language/crates/enzyme-spec`),
  because Cargo must fetch a git source before a `[patch]` can replace it. On
  publication, switch to
  `enzyme-spec = { git = "https://github.com/byenzyme/enzyme-spec", tag = "v0.1.0" }`,
  run `cargo update -p enzyme-spec` for `Cargo.lock` and
  `Cargo.private-recall.lock`, and drop the path rewrite in
  `crates/public/margins-workflows/tests/standalone.rs`.
- Shell E2E harnesses that read `workspaces/<id>/config.toml` or expect
  `plan.v1` (`scripts/e2e-fresh-onboarding.sh`, `scripts/e2e-fresh-workspace-setup.sh`,
  `scripts/e2e-official-hosted-workspace-setup.sh`, `scripts/core-product-smoke.sh`,
  `scripts/workspace-setup-rollout-review.py`, `tests/test_setup_e2e_lanes.sh`,
  `tests/test_e2e_fresh_onboarding.sh`, `tests/test_workspace_setup_rollout_review.sh`)
  still need to move to `configs/<id>.enzyme` and `plan.v2`.
