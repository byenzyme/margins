# Workspace configuration in the Enzyme workspace language

Status: in progress (2026-10-05); public side and recall engine path implemented (see below). Decided by Joshua: adopt the `.enzyme`
workspace language as the single Workspace configuration format; make
`enzyme-spec` a public crate (published: github.com/byenzyme/enzyme-spec,
tag `v0.1.0`, Apache-2.0); deliver as draft PRs.

## Why

Before this change one Workspace was described four ways: Margins'
`workspaces/<id>/config.toml`, Margins' private mirror of Enzyme's TOML engine
config (`src/workspace_recall.rs`), Enzyme's own TOML, and Enzyme's `.enzyme`
language. Margins translated between them through a temporary engine home on
every index run, and pinned a side-branch engine 124 commits behind master.

After it, a Workspace is one program in one grammar, parsed by one parser.

## Layout

```
~/.margins/                          MARGINS_HOME, machine-local, never synced; an Enzyme home
  margins.toml                       host preferences: [workspace] default, [workspace.names], [retention]
  configs/                           one .enzyme namespace, owned by Margins
    <id>.enzyme                      workspace "<id>" { … } — the whole Workspace config
    settings.enzyme                  engine settings { generation …; model …; updates disabled }
    profiles.enzyme                  optional shared custom profiles
  workspaces/<id>/                   state only, no configuration
    enzyme.db  ledger.db  receipts/  …
  models/                            local model files
  google/<account>/  granola/<account>/   connection state and tokens (unchanged)
```

Nothing is written into a notes folder except notes Margins creates in its
declared write folder. Nothing is shared with `~/.enzyme`: Margins parses its own
`configs/` with `enzyme-spec` and hands the engine an in-memory configuration; the
engine never reads Margins files or Enzyme's home.

The engine's generator selection reads the Margins home as an Enzyme home
(`configs/settings.enzyme`, `models/`); it never reads `~/.enzyme`.

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
  learn questions from folder "notes/people" including linked pages about relationships
  learn questions from source "mail"
}
```

With more than one Markdown source, folder names start with the source name
(`"notes/people"`); with one, they are relative to its root (`"people"`).

Mapping from the retired `config.toml`:

| `config.toml` | program |
|---|---|
| `[bindings.x] kind = "notes"` | `source markdown "x" { path … }` |
| `role = "home"`, `note_folder` | `remember in folder "<note_folder>" in source "x" create note` (exactly one per Margins Workspace; `"."` = the source root) |
| `kind = "captures"` | `source margins-captures "x" { path … }` |
| `kind = "google-mail" / "google-calendar" / "google-meet" / "granola"` | host sources of the same kind name, fields as below |
| `policy.excluded_folders / excluded_tags / excluded_entities` | `leave out folders / tags / links` |
| `policy.entities` (`ref`, `profile`, `expandable`) | `learn questions from <kind> "<name>" [including linked pages] [about <profile>]` |
| `[retention]` | machine `margins.toml` `[retention]` (global) with optional `[retention.<id>]` override |

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
- `workspace show [--text] [--json]` prints the program path (or its text);
  `workspace edit` opens a copy in `$VISUAL`/`$EDITOR`, shows the diff, and
  applies it through this same plan/apply path after confirmation. An invalid
  or declined edit is never applied; the text stays in a kept file. Without an
  interactive terminal, `edit` refuses and points to `show`/`plan`/`apply`.
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
excluded (including folders at or under an excluded folder), repeated entities, and `expandable` on anything but a folder were
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

### Machine config and index name

A root `config.toml` (Enzyme's legacy machine file, which Margins used for its
machine config) is migrated the first time machine configuration is read or
written, under the machine lock: it is validated first; `[llm] mode` and
`local_model` become `settings { generation …; model "…" }` in
`configs/settings.enzyme`, which also gets `updates disabled` unless it already
says otherwise; every other key moves to `margins.toml`; the original is kept as
`config.toml.migrated[.<n>]`. An invalid file is left untouched and reported.
A Workspace's `index.db` (with its SQLite sidecars) is renamed to `enzyme.db` on
first resolution, without reindexing; `index.identity` is unchanged. When both
names exist, `enzyme.db` is the index and `index.db` is left alone.

`settings`, `profiles`, and `margins-sources` are reserved program names in
`configs/`. A Workspace that already uses one of them as its id (a legacy
`workspaces/<id>/config.toml`, a program in the reserved slot, or the machine
default) is never skipped or migrated into that slot: every command that meets
it fails with `margins workspace rename <id> <new-id>`. The rename validates
first, moves the state directory (leaving a `workspaces/<id>` symlink so
recorded paths keep resolving), writes the program under the new id, keeps the
old declaration (`configs/<id>.enzyme.renamed-to-<new-id>` or
`config.toml.migrated`), moves the default, display name, and retention
override, and resumes if interrupted. Only reserved ids can be renamed; other
ids are referenced from sessions and server or plugin selections that the
rename cannot update.

The index rename never replaces an existing file (a hard link refuses an
existing destination). An older CLI or `margins-server` that still has
`index.db` open would keep writing to the renamed files, so upgrade the CLI and
the server together.

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
- `enzyme-spec` is published at github.com/byenzyme/enzyme-spec (`v0.1.0`);
  enzyme-rust consumes that tag and keeps no in-repo copy.

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
- Language validation runs `enzyme_spec::resolve_in` with an
  `enzyme_spec::Environment` holding Margins' source kinds
  (`margins_workflows::source_kinds`, the same `margins-sources.enzyme` text the
  engine reads) and the optional `configs/profiles.enzyme`, so readings,
  profiles, folder qualification and exclusions are checked by the engine's own
  resolver against the sources it will index.
- `settings.enzyme` is rewritten only when it lacks `updates disabled`; that
  rewrite renders the program and drops its comments.
- Plan/apply is `enzyme_spec::plan::ConfigStore` over `configs/` (lock,
  journal and receipts in `configs/.enzyme-apply/`, shared with `enzyme
  workspace plan|apply`). Margins adds only its view-level `actions` and
  receipt. Plan JSON is `margins.workspace.plan.v2`: `workspace_id`,
  `base_revision`, `plan_id`, `actions` (each with a `summary`; `update_program`
  when the change is outside the typed view), `desired_program`,
  `desired_sha256`, `diff`, and `program_plan`, the language plan
  (`enzyme.plan.v1`) that apply hands to the store unchanged. Re-applying an
  applied plan returns its receipt again (same `request_id`, `replayed:
  true`) while the program still holds that plan's result; after a later
  change it is stale. A plan made before `program_plan` existed is refused
  as invalid ("plan again"). A legacy
  `.toml` desired file is converted with the migration rules onto the current
  program. `workspace plan --preset <name|path>` fills a preset through
  `enzyme compile --preset --dry-run`, drops folder readings whose folders do
  not exist, adds the rest to the current program, and adds `program_path` and
  `preset` (`readings`, `skipped_readings`, `note_folder`) to the plan JSON.
  (`workspace compile` and `margins.workspace.compile.v2` were removed with the
  scan, 2026-10-06.)
- Migration also runs explicitly: `margins workspace migrate [--dry-run] [--json]`.
  Enzyme's implicit folder exclusions (`.git`, `node_modules`, …) are not written.
  With several Markdown sources, unqualified legacy folder references are
  qualified with the Home source name. `workspace plan --preset` qualifies
  preset folder readings the same way and plans them as a view change.
- `update_program` is emitted only when applying the view actions to the
  current program does not yield the desired program's statements (layout-only
  differences are visible in the diff).

## Margins implementation (recall side)

- Margins runs the shipped `enzyme` CLI (`src/enzyme_cli.rs`) with
  `ENZYME_HOME=$MARGINS_HOME`, `--workspace <id>`, and an allowlisted
  environment (plus proxy and CA variables: `HTTP(S)_PROXY`, `NO_PROXY`,
  `ALL_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`). `MARGINS_ENZYME_BIN`, when
  set, is used or the call fails. Otherwise it uses the first of
  `<exe dir>/../libexec/margins/enzyme` (when that `libexec/margins` exists;
  otherwise the archive's `<exe dir>/enzyme`) and
  `$MARGINS_HOME/bin/enzyme` whose `enzyme --version` equals the `version` in
  `scripts/enzyme-cli.pin`, skipping missing, non-executable, and
  other-version candidates, and fails listing each rejection; it never
  searches `PATH`, where a user's own `enzyme` may be another release. The
  choice is made once per process. The generator is
  always explicit: `--llm env` with Margins' hosted bundle as `OPENAI_*`,
  `--llm local` when the selected model is installed, otherwise `--llm none`
  (the index is built, and init and recall fail closed until setup chooses a
  generator). Exit codes 2–5 become typed errors; 4 is "workspace busy"
  (`MARGINS_ENZYME_LOCK_TIMEOUT` passes `--lock-timeout`). Exit 5 (index
  built, catalysts failed) is reported as `catalysts_pending`.
- `margins init` runs `enzyme init --json-progress`. A sync, connector
  reconcile, Granola import, or retention change runs `enzyme refresh`, which
  builds a due catalyst epoch in a detached worker that inherits the call's
  environment. These sync-triggered updates wait at most 10 s for another
  build of the Workspace and then report it busy; the next sync or init
  catches up. Recall runs `enzyme search --json` (catalyst, direct and
  exact-phrase hits); readiness, counts and source freshness come from
  `enzyme status --json`; the model registry from `enzyme model list --json`.
- Before any call Margins writes `configs/settings.enzyme` with `updates
  disabled` and the managed `configs/margins-sources.enzyme`, whose `source
  kind` definitions expand `google-mail`, `google-calendar`, `google-meet` and
  `granola` to SQLite sources over `workspaces/<id>/ledger.db` (`accepts` lists
  the fields only sync reads). Calendar reads the window the last sync
  materialized. `margins-captures` stays in the program as the capture store's
  declaration; its kind indexes nothing, so captures never enter recall and a
  one-Markdown Workspace keeps root-relative refs. Margins creates `ledger.db`
  before indexing because every program declares a captures source.
- The engine reads the program as written. Margins no longer adds hidden
  rules: entity selection is the engine's automatic selection on every
  init/refresh (never written back), the managed-projection tag exclusion and
  Margins' correspondent link readings are gone.
- Document identity: one Markdown source gives root-relative refs
  (`people/ada.md`); several give `<source name>/<relative>`; ledger records
  give `sqlite:<source name>/<lowercase hex of the record id>`, which each kind
  in `margins-sources.enzyme` emits as its `document ref` through the
  `{source}` placeholder and Margins decodes back to the record
  (`margins_workflows::source_kinds::ledger_record_id`). A source rename is a
  new identity. The in-process Margins used the same refs, so an index it built
  keeps its ledger documents without re-embedding.
- An index the in-process Margins built at `workspaces/<id>/enzyme.db` is
  reused when `enzyme status` reports a compatible schema (its marker
  `index.identity` is removed after the first CLI run); an index the engine
  cannot read is rebuilt once with `init --force`. `MARGINS_RECALL_DEBUG=1`
  prints which (`engine index reuse …` or `engine index rebuild …`).
- enzyme-spec unification: the engine and `margins-workflows` both depend on
  `enzyme-spec = { git = "https://github.com/byenzyme/enzyme-spec", tag =
  "v0.4.0" }`. `scripts/enzyme-cli.pin` names the enzyme-rust `rev` that
  tests and gates build (`scripts/enzyme-bin`) and the `version` Margins
  requires at runtime.

### Which Workspace a command uses, and its exit codes

`recall`, `workspace status`, `source list`, `sync`, `integrations`, and
`import granola` never create a Workspace. Each uses the `--workspace` (or
`MARGINS_WORKSPACE`) selection, else the one Workspace that declares the
current directory, else the machine default, and then prints
`Using Workspace <id> (default)` on stderr. Commands that write (`sync`,
imports, `integrations reconcile`) migrate a retired `config.toml` and rename
`index.db` as any writing command does; read-only ones inspect without
writing. Only `margins init` establishes a new Workspace for the current
folder.

For `recall`, `workspace status`, and `sync` the exit code is:

| Exit | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | The command failed. stderr names the error code: `<margins_error code="…">` in readable mode, or a `margins.error.v1` object with `--json` (failures inside `sync` itself use a `margins.sync.v1` envelope with `ok: false`). `workspace_required` means no Workspace applied; `command_failed` covers engine and index failures. `sync` also exits 1, after printing its full `margins.sync.v1` envelope, when any source or the recall refresh is not ok. |
| 2 | The arguments were not valid. |

`recall` prints readable results; pass `--json` for the `margins.recall.v1`
envelope.

### Inspecting a Workspace with `margins enzyme`

Margins ships its own `enzyme`, separate from any `enzyme` you install, and a
Margins home is an Enzyme home. `margins enzyme <args…>` runs that bundled
engine with the same lookup, version check, and scrubbed environment Margins
uses, `ENZYME_HOME=$MARGINS_HOME` (never `~/.enzyme`), and the selected
Workspace (`--workspace`, `MARGINS_WORKSPACE`, the one whose notes folder holds
the current directory, or the default):

```bash
margins enzyme --workspace <id> status
margins enzyme --workspace <id> status --json
margins enzyme --workspace <id> search "a phrase" --json
margins enzyme scan --workspace <id> --json
```

`init` and `refresh` get Margins' catalyst generator unless you pass `--llm`.
`update`, `login`, and `logout` are refused: Margins pins the engine it ships
and never uses an Enzyme account.

## Open items

- `desktop/INTEGRATIONS_CONNECTOR_CONTRACT.md` (parked) still names plan.v1.
