# Workspace configuration in the Enzyme workspace language

Status: in progress (2026-10-05); public side and recall engine path implemented (see below). Decided by Joshua: adopt the `.enzyme`
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

Migration keeps exactly what the previous engine honored. A learning entity is
read only as `#tag`, `[[link]]`, `log:<name>`, or `folder:<path>`; an excluded
entity only as `folder:<path>`, `#tag`, or `[[link]]`. Other forms (`Project:
Atlas`, `person:ada`, `tag:x`, bare names), learning entities that are also
excluded, repeated entities, and `expandable` on anything but a folder were
ignored then and are dropped now, each reported in the migration `warnings`
(`workspace migrate --json`) and on stderr. An unqualified legacy `folder:<path>`
is Home-relative; with several Markdown sources it is qualified with the Home
source name even when its first segment names another source.
`folder:markdown_<hash>/<path>` (the old internal identity of a Markdown root)
maps back to that source's name.

A Workspace that cannot migrate keeps its `config.toml` untouched (conversion
and validation run before anything is written) and is skipped with a warning by
Workspace listing; `workspace list` shows it with its error. A `config.toml`
found beside an existing program (a crash between the program write and the
rename) is retired under the Workspace lock to the first free
`config.toml.migrated[.<n>]`.

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
- Language validation runs the same host lowering the engine path uses
  (`margins_workflows::workspace_lowering::lower_for_engine`) and then
  `enzyme_spec::resolve` with the optional `configs/profiles.enzyme`, so readings,
  profiles, folder qualification and exclusions are checked by the engine's own
  resolver against the sources it will index.
- Plan JSON is `margins.workspace.plan.v2`: `workspace_id`, `base_revision`,
  `plan_id`, `actions` (each with a `summary`; `update_program` when the change is
  outside the typed view), `desired_program`, `desired_sha256`, `diff`. A legacy
  `.toml` desired file is converted with the migration rules onto the current
  program. `workspace compile` emits `margins.workspace.compile.v2` with
  `desired_program` (no `desired_toml`).
- Migration also runs explicitly: `margins workspace migrate [--dry-run] [--json]`.
  Enzyme's implicit folder exclusions (`.git`, `node_modules`, …) are not written.
  With several Markdown sources, unqualified legacy folder references are
  qualified with the Home source name. `workspace compile` qualifies its
  Home-relative scan specs the same way and plans them as a view change.
- `update_program` is emitted only when applying the view actions to the
  current program does not yield the desired program's statements (layout-only
  differences are visible in the diff).

## Margins implementation (recall side)

- Lowering (`workspace_lowering`): `google-mail`, `google-calendar`,
  `google-meet` and `granola` become read-only `sqlite` sources over the
  Workspace `ledger.db`, keeping the source name; `margins-captures` is dropped
  (the capture registry has never been indexed, and keeping it out preserves a
  one-Markdown Workspace's identity); the `margins-managed-projection` tag
  exclusion is added. Margins also adds, only in the lowered program, its
  catalyst budget (`total_limit`) and, when the program has no readings, its
  automatic correspondent/people link readings and correspondence noise as
  `leave out links`.
- One engine seam (`src/recall_engine_seam.rs`): inputs are the rendered lowered
  program text, workspace name, state directory and generator home; inside it
  parses and resolves with `enzyme-spec`, builds
  `EnzymeConfig::from_program_workspace`, resolves the generator, reconciles
  catalysts, and calls `ensure_searchable_sync` with `workspace_config` (never
  `config_path`). A Workspace whose only lowered source is one Markdown source
  indexes from that path (the engine's path-addressed identity); every other
  Workspace indexes from its empty state directory. No other Margins module
  builds engine configuration. Search over an opened index and read-only scan
  helpers still call the engine directly.
- Document identity follows the language: one Markdown source gives root-relative
  refs (`people/ada.md`); several give `<source name>/<relative>`; ledger sources
  give `sqlite:<source name>/<hex id>`. Folder readings use the same identity.
  A source rename is therefore a new identity. `workspaces/<id>/index.identity`
  records the identity version; an index built by an earlier release (hashed
  `markdown_…`/`gmail_…` namespaces) is fully reindexed exactly once, and its
  folder/collection catalysts are dropped.
- enzyme-spec unification: `scripts/private_recall_manifest.py` adds
  `[patch."https://github.com/byenzyme/enzyme-rust.git"] enzyme-spec = <the
  declaration margins-workflows uses>`, so the engine and Margins link one
  `enzyme-spec` (check: `scripts/with-private-recall cargo tree -i enzyme-spec
  --features recall`).

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
- The private composition's enzyme-spec patch points at the same sibling path
  until publication; release CI cannot build the private composition until
  enzyme-spec has a fetchable source.
- `desktop/INTEGRATIONS_CONNECTOR_CONTRACT.md` (parked) still names plan.v1.
