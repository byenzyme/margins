# Margins integrations: connector contract (v1)

This is the shared contract for the batteries-included integration layer. Read
`desktop/RELATIONSHIP_REENTRY_PRODUCT_BRIEF.md` first; it defines the product
surface every connector ultimately feeds.

Design rationale, trade-offs, and simplification candidates live in
`desktop/INTEGRATIONS_DESIGN_RATIONALE.md`; read it before revising this
contract.

## Workspace and Source boundary

A connector always runs inside a selected id-named Workspace. Persistent
machine writes are confined to `$MARGINS_HOME/workspaces/<id>/`; projections
are confined to the writable home notes Source. For the ordinary single-notes-
folder case, a workspace-scoped command in an unbound cwd implicitly creates a
Workspace whose home is cwd, but only after the strict isolation deny-list
passes. Resolution otherwise uses `--workspace <id>`, `MARGINS_WORKSPACE`, or a
persisted binding for cwd inside a declared Source. `$HOME`, `/`, Margins or
Enzyme config/state paths, temporary paths, folders without Markdown evidence,
and ancestors of declared homes refuse before mutation and print the explicit
`margins workspace new <id> --home <path>` alternative. Every Source beyond
the one implicit home is declared, never inferred from cwd.

Successful implicit creation prints exactly `Created workspace <id> with home
<path>` once on stderr. Stdout remains exclusively the requested command
payload, including valid JSON on a first implicit `--json` invocation.

## Workspace desired state (`config.toml`)

Workspace config is the only desired-state authority. Bindings live under
`[bindings.<name>]`; the legacy `[sources.<name>]` table is not read, aliased,
or migrated.

The map key is a human/CLI binding name. For the native Markdown and Gmail
consumers migrated in this phase, it never enters engine document refs. CLI
commands remain `source add`, `source list`, and `source remove`, and serialized
kind spellings remain `notes`, `google-mail`, etc.

`WorkspaceBinding` is internally tagged by `kind` with concrete variants:

| kind | required fields | notes |
| --- | --- | --- |
| `notes` | absolute `path`, `role` (`home` or `reference`) | Enzyme ingests directly; no FolderConnector authority |
| `captures` | absolute `path` | session registry |
| `google-mail` | normalized `account`, `gmail.query`, `gmail.backfill_days` | one binding per account |
| `google-calendar` | normalized `account`, `calendar.lookback_days`, `calendar.lookahead_days` | defaults 365/180; all accessible calendars for that account |
| `google-meet` | normalized `account` | one binding per account; Meet REST/Drive transport |

`source list --json` serializes typed bindings: each row flattens the variant
fields plus `cache_raw_payload`, `project_to_home`, and `indexed_how`.
Irrelevant fields are omitted—never `null`. A `notes` row has `path` and `role`
but no `account`, `gmail`, or `calendar`; a `google-mail` row has `account` and
`gmail` but no `path`, `role`, or `calendar`; a `google-calendar` row has
`account` and `calendar` but no `path`, `role`, or `gmail`.

### Retention policy (`RetentionPolicy`)

Workspace config may declare an optional `[retention]` table with
`raw_cache_max_age_days` and `tombstone_max_age_days`. Absent fields mean retain
until explicit purge; there is no automatic age-based deletion. Age eligibility
is computed only for the `expired` retention scope and never applied implicitly
— every destructive change requires an explicit `retention preview` followed by
`retention apply`.

### Stable document identity

For native Markdown and Gmail, binding display names are labels only. Engine
collection keys derive deterministically and cannot collide with valid CLI
binding names:

- Native Markdown: `markdown_<sha256>/…` from the declared absolute root path
  (including a single implicit home). Changing the root is a new collection;
  relocation-preserving identity is out of scope.
- Gmail: `sqlite:gmail_<sha256>/…` from the normalized account only. Display
  rename and query/backfill changes preserve thread refs. Margins catalog keys
  must exactly match Enzyme refs.
- Calendar: `sqlite:calendar_<sha256>/…` from the normalized account only.
  Display rename and selector changes preserve event refs. Margins catalog keys
  must exactly match Enzyme refs.
- Google Meet: `sqlite:meet_<sha256>/…` from the normalized account only.
  Display rename preserves transcript refs. Margins catalog keys must exactly
  match Enzyme refs.

Removed bindings exclude product evidence from configured retrieval without
purging ledger, raw cache, or `index.db` by default. Removed Gmail bindings
exclude stale materialization receipts from reconcile and status; removed
Calendar bindings make old status, refresh, and retrieval inert immediately
while authoritative rows remain in the ledger. Removed Meet bindings make old
status, refresh, and retrieval inert immediately while authoritative
`external_document_evidence` rows remain in the ledger.

The Source kinds are `notes | captures | google-mail | google-calendar |
google-meet`. Exactly one notes Source has `role = home`; every other
Source is read-only. Remote lifecycle separates declaration, revisioned apply,
and materialization reconcile:

Implicit creation is shared by commands that can establish or read a Workspace:
`workspace status`; `source add` and `source list`; `init`,
`scan` and `recall`; and every `integrations` operation. Machine-only
commands such as `setup`, `connect google`, `disconnect google`, `connect status`,
connection export/import, `workspace new`, guides, and capabilities do not create
a Workspace. `disconnect google --account X` is machine scope only; it never
resolves or mutates Workspace declarations. Workspace binding changes use
`source add ... --name NAME` and `source remove NAME` only. Removal requires an
existing Workspace and never creates one implicitly.

```text
single folder: cd notes → init → recall
multiple Sources: workspace new → source add → connect google → workspace plan/apply → integrations reconcile → recall
```

For remote kinds, `source add` declares desired state. Agents may also mutate
desired state through revisioned plan/apply. Gmail's corpus boundary is
declared with `--query` (non-temporal; labels belong here) and
`--backfill-days` (sole temporal boundary; defaults `-in:spam -in:trash` and
365 days). Calendar's rolling boundary is declared with `--lookback-days` and
`--lookahead-days` (defaults 365 and 180); it means all accessible calendars
for that account. `integrations reconcile` materializes every active binding
from workspace desired state: initial reconcile is a complete bounded snapshot
and later reconciles may use the provider updated-event cursor. Gmail, Calendar,
and Meet reconcile automatically.
Successful reconcile refreshes recall.

## Doctrine

### Human-facing Google connection

The native Google driver is internal transport. Users and user-facing agents
only ever see **connect Google**: one
`margins connect google [--account <email>]` command, one Google consent screen
for read-only mail, calendar, files, Docs, and Meet, and
`margins connect status --json` for verification. OAuth-client,
client-project, and transport details must not leak into onboarding copy or
prompts. Desktop uses browser plus loopback. Headless SSH uses
`margins connect google --headless [--account <email>] --json`; the consent URL
goes to stderr before consent, the browser intentionally finishes on a localhost
cannot-connect page, and the final address-bar URL is accepted only through a
hidden local terminal prompt. Callback URLs, codes, and tokens must never be
pasted into chat, logs, Workspace config, fixtures, or ledgers.

1. **Hosted sources stay authoritative.** Margins maintains a private local
   context plane. Connectors mirror selected history into Workspace state and
   project only human-readable artifacts into home; they never become a system
   of record and never write back in v1.
2. **Every connector feeds exactly two consumers:**
   - the **relationship re-entry packet** (bounded attendee/account
     neighborhood with a source manifest and temporal cutoff);
   - the **held-set transition surface** (post-import proactive utterances:
     named person + concrete action + why-now + receipt).
   Anything ingested that cannot surface as a receipt behind an utterance or a
   packet claim is wasted scope. No browsing UI, no unified inbox, no
   dashboards.
   **Superseded 2026-08-25 by automatic correspondent entities; retained as the
   original approval decision.**
3. **Setup proposes desired state; the user applies it.** Gmail membership is
   declared in workspace desired state (`query` + `backfill_days`); Calendar
   membership is declared in workspace desired state (`lookback_days` +
   `lookahead_days`). Agents mutate desired state with revisioned
   `workspace plan` / `workspace apply`; materialization runs through
   `integrations reconcile`. Observable curation signals persist only as
   KnowledgePolicy inputs and never confer or deny recall entity status:
   retained real correspondents are automatic, noise-filtered entities.
   Nothing requires hand-editing a config file.
4. **Normalized output is inspectable evidence.** Google Meet is materialized
   in dedicated `external_document_evidence`; observable people
   are stored as role-blind `external_document_participants` associations, and
   an external document may truthfully have none. Raw cache is optional and
   quota-bound. Calendar is materialized in `calendar_event_evidence` and role-blind
   `calendar_event_attendees` with no home projection. Gmail is materialized
   in `thread_evidence` with no home projection. Native Markdown (home and
   reference roots) is ingested directly by Margins recall. There is no generic
   Episode register, `recall_items` view, or
   FolderConnector production authority for Markdown/reference folders.
5. **Identity integrity is load-bearing.** Resolve people by email, full name,
   and alias evidence. Never merge on bare first-name resemblance (the
   two-Kevins rule). When identity is ambiguous: ask or omit, never guess.

## Normalized record: external documents and dedicated evidence tables

Recall authority is native Markdown plus dedicated evidence tables
for the recurring external connector kinds. There is no generic Episode register,
`recall_items` view, or Episode-framework projection path.

Calendar authority is `calendar_event_evidence` plus role-blind
`calendar_event_attendees`. Meet authority is `external_document_evidence` plus role-blind
`external_document_participants`. Gmail authority is `thread_evidence` plus
rebuildable `participant_threads`. Optional connector-specific raw cache may
retain transport payloads for reconciliation; raw cache is never evidence.

Meet reconcile is a complete provider snapshot with explicit tombstones for
records absent from the latest snapshot.

Every active Calendar event in the configured rolling boundary is materialized.
Meet materializes every authoritative document returned by its refresh
transport. Participants/attendees are document associations
for retrieval; source-native observable metadata may be retained, but
participant sampling/ranking cannot change corpus membership. Product evidence
for Calendar and Meet is `EvidenceHandle::ExternalRecord { connector_id,
source_account, source_id, href? }` with connector ids `gcal`, `google_meet`,
and `google_meet`; native filesystem paths remain native Markdown only. Explicit
cancellation or snapshot tombstones mark authoritative rows inactive. Failed
refresh preserves the last coherent materialization and reports stale/error
machine-readably.

| Source kind | raw cache | projection | recall |
|---|---|---|---|
| notes | forbidden | none | native Enzyme ingestion (`markdown_<sha256>/…`) |
| captures | forbidden | none | native/session registry |
| google-mail | quota-bound | forbidden | `thread_evidence` |
| google-calendar | quota-bound | forbidden | `calendar_event_evidence` + `calendar_event_attendees` |
| google-meet | quota-bound (optional) | none currently | `external_document_evidence` |

## Importer contract (one-shot migrations)

An `Importer` is distinct from a recurring `Connector`. It surveys a bounded
input in memory, validates the user-approved materialization in an ephemeral
staging directory, atomically commits native files into the vault, and returns
an in-memory result. It has no Workspace Source binding, SQLite schema, cursor,
health, tombstone, run id, receipt, finalization state, or ongoing freshness.
Successful imported notes enter the ordinary Margins recall index through the
same refresh used for other native Markdown.

Granola implements `Importer`. Input may be selected export files or a bounded
MCP batch fetched with machine-level authorization; authorization metadata is a
transport capability, not migration provenance. Raw payloads, provider ids,
accounts, export paths, hashes, resolution decisions, and import history are
not persisted. The tradeoff is deliberate: a later run is another import, not
a resume or reconciliation. Safe deterministic collision allocation preserves
existing user files and handles same-batch filename collisions without
overwriting.

Future Granola notes contain only:

- `occurred_at`;
- confidently resolved people wikilinks;
- optional organizations backed by explicit export/person organization fields;
- a human-readable filename and H1;
- source-native `Notes` and `Transcript` sections, without import-time summary.

Ambiguous people are omitted from the note; email-domain organization guesses
are forbidden. Imported meetings are not recording sessions. Markdown must not
contain Granola/source ids, source account, hashes, raw provenance,
`margins_session`, duplicated title/content fields, import timestamps, or an
unresolved-participants field. Staging is cleaned on both success and failure.

```rust
trait Importer {
    type Survey;
    type Options;
    type Output;

    fn survey(&self) -> Result<Self::Survey>;
    fn import(self, options: &Self::Options) -> Result<Self::Output>;
}
```

## Connector trait (Rust, in `crates/public/margins-workflows`)

### Google connection command

Google's user-facing lifecycle separates machine connection from Workspace
declaration. Machine scope: `margins connect google [--account EMAIL] [--headless]`,
`margins connect status --json`, and `margins disconnect google --account X`.
Workspace scope: `source add ... --name NAME` and `source remove NAME`. Connect
authorizes one machine capability and never creates Workspace source identity.
Disconnect forgets only that machine credential; every Workspace source
declaration, authoritative evidence row, raw cache, materialization receipt,
connector cursor, and recall index stays intact. Existing connector materializations report
`needs_auth` and retain their last successful refresh; declarations which have
never refreshed have no connector ledger row. Machine connection status still
reports `connected: false` and retains every declaring Workspace association. Connection requests
view-only mail, calendar, and file access in one consent screen. The 2026-08-24
Workspace-owned location is **superseded by the 2026-08-25 machine-account
resolution below**; remediation must never ask a principal to install tools,
create cloud projects, manage credential files, or choose a credential-storage
backend.

Keep connectors and importers in the portable public workflows crate so they
build and test on Linux and are reachable from both the desktop app and the
`margins` CLI. Granola must not implement the recurring `Connector` trait.

```rust
trait Connector {
    /// Discover the configured collection and report observable curation
    /// evidence. Margins orchestration may persist raw cache and authoritative
    /// materialization during discovery when truthful observation requires the
    /// complete corpus; it must not project human-facing artifacts as a side
    /// effect.
    fn observe(&self, ctx: &ConnectorCtx) -> Result<ObservationReport>;
    /// Refresh or reconcile the configured collection. Repeating the same
    /// coherent snapshot must be idempotent.
    fn reconcile(&self, ctx: &ConnectorCtx, binding: &WorkspaceBinding) -> Result<ReconcileResult>;
    /// Cheap freshness/health check for status surfaces.
    fn health(&self, ctx: &ConnectorCtx) -> Result<HealthReport>;
}
```

- `ObservationReport`: item counts by kind, date range, detected accounts,
  inferred people/orgs with evidence, and additive proposal flags such as
  `likely-automated`. Rendered for human review and KnowledgePolicy only.
- Gmail observation resolves the Workspace Gmail selector (`query` +
  `backfill_days`), pages through every thread matching that selector, and may
  write quota-bound raw cache plus authoritative `thread_evidence` and
  rebuildable `participant_threads` during reconcile. Transport `--max 100` is
  per-page sizing only; there is no total result cap. Ranked participant
  proposals and activity/recency/noise evidence are KnowledgePolicy inputs
  only; they do not choose corpus membership.
- The authoritative snapshot transaction stores a separate materialization
  receipt for the exact selector. Changing `query` or `backfill_days` preserves
  the last coherent evidence but every machine-readable freshness surface
  reports `stale` / `refresh_required` until reconcile successfully receipts
  the new selector. Renaming the binding alone remains fresh.
- Calendar reconcile derives the request boundary from the active
  `CalendarCollectionSelector` on the binding. Initial reconcile is a complete
  bounded snapshot; later reconciles may use the provider updated-event cursor.
  Attendee proposals and activity signals are KnowledgePolicy inputs only; they
  do not choose corpus membership.
- `ReconcileResult`: authoritative records written, updated, unchanged,
  tombstones, next cursor, and a per-run manifest. JSON receipt counters are
  `records_written`, `records_updated`, and `records_unchanged`.
- `HealthReport`: last successful sync, cursor age, auth state, and a single
  legible status (`fresh | stale | needs-auth | error`). Failures never
  invalidate the existing local index; they make staleness visible.

## Agent-side contract: workspace plan/apply and integrations reconcile

All mutation commands require literal `--workspace <id>`. Integrations reconcile
and retention apply also require explicit `--if-revision` and `--request-id` as
shown. Workspace apply reads its revision and deterministic retry identity from
the reviewed plan. JSON failures emit `margins.error.v1` with typed `code`,
`retryable`, and `details`; there is no XML companion.

Config revision is a semantic canonical SHA-256 over workspace desired state.
It excludes credentials, executable paths, discovery metadata, and timestamps.
Read it from `margins --workspace <id> workspace status --json`.

### Desired-state mutation

```bash
margins --workspace <id> workspace plan --desired FILE --json
margins --workspace <id> workspace apply --plan FILE --json
```

`workspace plan` emits `margins.workspace.plan.v1` with explicit `workspace_id`,
`base_revision`, deterministic `plan_id`, ordered `actions`, and the desired
config payload. `workspace apply` emits `margins.workspace.apply.v1` with
`before_revision`, `after_revision`, `request_id`, `plan_id`, `replayed`, and
ordered action receipts.

Applying the same exact plan derives the same request id from its request hash and
replays the exact receipt without transport or writes (`replayed: true`). A plan
whose `base_revision` no longer matches the Workspace returns
`workspace_revision_conflict`.

### Materialization reconcile

```bash
margins --workspace <id> integrations reconcile \
  --if-revision REV --request-id ID [--connector ID] [--account ACCOUNT] --json
margins --workspace <id> integrations status --json
```

`integrations status` is read-only. `integrations reconcile` emits
`margins.integrations.reconcile.v1` with `workspace_id`, `revision`,
`request_id`, `replayed`, and ordered connector results. Each result carries a
typed `status`, optional typed `error`, and counters
`records_written`, `records_updated`, `records_unchanged`, and `tombstones`.
Gmail, Calendar, and Meet reconcile automatically from declared bindings. The
account-derived binding and collection identity does not change
when the binding is renamed; selector changes affect membership and require a
fresh materialization/index before semantic retrieval resumes.

Failed refresh preserves the last coherent evidence and marks stale/error.
Binding removal excludes evidence without purge. Explicit retention preview/apply
is the only supported purge path; see the retention contract below.
The official runtime refreshes the Margins recall index after successful or partially successful
reconcile commits and again after `retention apply` scopes `materialization` or
`all`; there is no Enzyme retention policy or upstream seam.

Email proposal evidence observed during reconcile uses this shape:

```json
{
  "threads": 3,
  "thread_count": 3,
  "user_replied": 4,
  "user_sent": 6,
  "last_interaction": "2026-08-20T11:00:00Z",
  "automated_signals": ["list-unsubscribe"],
  "samples": ["sent", "recent"]
}
```

`threads` is the observed-thread count and `thread_count` is its explicit alias.
`user_replied` and `user_sent` count matching messages across the observed
window. `last_interaction` is RFC3339 or `null`. Proposal rank is descending
`user_replied`, then descending `user_sent`, descending human-likelihood score,
descending `thread_count`, descending `last_interaction`, and finally lexical
proposal value. Native Markdown bindings are not observed, reconciled, or
indexed by FolderConnector.

### Retention preview and apply

Retention is explicit, revisioned, and destructive. There is no automatic purge,
no provider deletion, no native Markdown deletion, no message-level Gmail
target, no receipt deletion, and no migration
or compatibility path. Source removal removes only the selected binding while
preserving authoritative evidence, raw cache, and indexes; credential loss
preserves bindings and all retained data. Neither action purges.

Closed connector ids are exactly `email`, `gcal`, and `google_meet`.
Accounts are normalized. Retention does not require an active binding, so removed
collections remain purgeable.

```bash
margins --workspace <id> retention preview \
  --connector ID --account ACCOUNT \
  --scope expired|raw-cache|tombstones|materialization|all --json
margins --workspace <id> retention apply \
  --plan FILE --if-revision REV --request-id ID --json
```

`retention preview` is read-only and deterministic for the exact Workspace
revision. It emits `margins.retention.preview.v1` with `workspace_id`,
`revision`, `plan_id`, `target` (`connector_id`, normalized `source_account`),
`scope`, optional `cutoffs` when `scope = expired`, counts by
raw/active/tombstoned/associations/connector-state/curation, a ledger
fingerprint, `destructive: true`, and `index_refresh_required`. Preview never
writes. The JSON enum spelling for CLI `raw-cache` is `raw_cache`.

`retention apply` requires literal `--workspace`, the exact preview plan file,
matching `--if-revision`, and `--request-id`. It uses the same reconcile lock as
materialization. Reusing a `--request-id` with the same request hash replays the
exact receipt; a different hash under the same request id returns
`idempotency_conflict`; a changed ledger or config revision returns a typed stale
or conflict envelope in `margins.error.v1`. Ledger deletion and the apply receipt
are atomic.

Scope semantics:

| scope | deletes | preserves |
| --- | --- | --- |
| `raw-cache` | `raw_items` rows for the target only | connector cursors/evidence/index, connector state, historical runs/reconcile/purge receipts |
| `tombstones` | physically tombstoned authoritative rows for the target | active evidence, raw cache, cursors, receipts |
| `materialization` | active and tombstoned authoritative evidence and associations for the target; connector state, cursor, and health; curation observations | raw cache, historical run/reconcile/purge receipts; next reconcile requires a new request id |
| `all` | `materialization` plus `raw-cache` for the target | historical run/reconcile/purge receipts |
| `expired` | rows eligible under `[retention]` age cutoffs for the target | active authoritative evidence; never deletes live corpus members |

Connector-specific tombstone behavior during reconcile (unchanged by retention
semantics):

- Gmail absent upstream threads become ledger tombstones; one role-blind thread
  document remains (PR #74 semantics).
- Calendar complete snapshot tombstones only inside the observed rolling scope.
- Meet tombstones remain for records absent from the latest snapshot.

After `retention apply` with scope `materialization` or `all`, the official
binary refreshes generic Margins recall sources so derived docs/chunks/occurrences/
entities/catalysts are pruned from `$MARGINS_HOME/workspaces/<id>/index.db`.
Raw cache remains separately purgeable under `raw-cache` or `all`; native
Markdown roots are out of scope for retention and remain a separate deletion
surface.

`retention apply` emits `margins.retention.apply.v1` with `workspace_id`,
the unchanged Workspace `revision`, `request_id`, `request_hash`, `plan_id`,
`target`, `scope`, `deleted` counters, `replayed`, and
`index_refresh_required`.

## Integrations datastore (SQLite)

Connector state and the permitted raw fetched corpus live in SQLite at
`$MARGINS_HOME/workspaces/<id>/ledger.db`:

- `connectors`: connector id, account, materialization receipt (JSON), cursor/checkpoint,
  last sync, scope boundary, health state.
- `external_document_evidence`: authoritative Meet document rows —
  connector id, source_account, source_id, occurred_at, title, body_text,
  optional href, provider `attributes_json`, imported_at, and tombstoned_at.
  Enzyme, not this provider table, owns document/content hashes.
- `external_document_participants`: rebuildable, role-blind participant
  associations keyed to `external_document_evidence`.
- `raw_items` (allowed only by the code policy): quota-bound fetched source
  records retained independently from authoritative materialization for
  transport debugging and incremental reconciliation.
- `runs`: per-reconcile manifests (counts and errors).
- `reconcile_receipts`: request id, exact request hash, deterministic result
  envelope JSON, and creation timestamp.
- `purge_receipts`: request id, exact request hash, deterministic retention
  apply envelope JSON, and creation timestamp.

For email, Gmail membership is declared only by `source add google-mail --query
... --backfill-days ...` or by revisioned workspace plan/apply. Entity
entity curation/exclusion belongs to Workspace `[policy].entities` and
`[policy].excluded_entities`; both default to empty. `entities` uses Enzyme's
simple-or-options shape, including optional catalyst profiles and explicit
folder expansion.

Home Markdown exists only for policy-permitted human-readable artifacts. The
SQLite side is machine state; deleting a projected note must not corrupt it,
and re-projection can rebuild allowed notes from retained raw rows. Native
Markdown bodies remain in their declared roots and are never copied into
Workspace state. No documented flow requires hand-editing any state file.

Active Gmail materialization receipts refresh and report only while they still
match the account in a current Gmail binding. Removing that binding leaves
ledger evidence intact but excludes stale receipts from reconcile, status, and
product context until the binding is re-declared. Removed Calendar bindings make
old status, refresh, and retrieval inert immediately while authoritative rows
remain in the ledger. Removed Meet bindings make old status, refresh, and
retrieval inert immediately while authoritative
`external_document_evidence` rows remain in the ledger.

## Post-refresh indexing

A successful reconcile refreshes Workspace recall before returning. Gmail Sources
bind `thread_evidence`; Calendar Sources bind
`calendar_event_evidence` and `calendar_event_attendees`; Meet Sources bind
`external_document_evidence` and
`external_document_participants` automatically.

## Fresh-onboarding e2e harness

Use `scripts/e2e-fresh-onboarding.sh` to evaluate brand-new Margins onboarding
without touching the tester's real connector state, Margins configuration, or
notes. Start with `init` (optionally `--sandbox DIR`), source the printed env
file, create the named Workspace and declare Sources, then run `phase1`,
`auth-instructions`, `phase2`, `verify-isolation`, and
`report`. Every Workspace-scoped command passes `--workspace` or uses the
explicit sandbox `MARGINS_WORKSPACE`; it deliberately does not rely on cwd
selection or implicit creation. The auth step
remains human-driven: the script prints `margins connect google --account
<email>` for deterministic desktop tests and
`margins connect google --headless --account <email> --json` for headless SSH.
It never automates the consent screen or handles credentials.

For a credential-free Linux/CI run, stop at the native consent boundary and
validate the hermetic phases: setup, desired-state declarations, auth
instructions, contract greps, and report generation. Recorded HTTP fixtures
cover transport pagination/materialization behavior without live Google calls.
Every artifact stays under the sandbox, isolation verification compares
checksums and mtimes for the real config/auth locations, and the temporary
sandbox cleans itself unless the operator explicitly requests preservation.

Build provenance is a validation gate, not report metadata. Set
`MARGINS_E2E_EXPECTED_COMMIT` to the full commit from the validation brief
before `init`; init records the capabilities build object and binary SHA-256,
and every later phase refuses a commit mismatch or changed binary before its
product assertions. Mac validation briefs must carry that expected full SHA,
and the checklist is incomplete until the report's single `binary provenance:`
line is pasted into the result. Findings from any other binary are not evidence.

The harness uses user-chosen binding names (`mail`, `calendar`, `meet`) rather
than kind-matching names. Seeded native Markdown lives under a declared
`notes` reference binding; FolderConnector must not observe native Markdown
roots. After init, `source list --json` must serialize typed bindings with no
irrelevant or null `role`/`path`/`account`/`gmail` fields. After phase2 it
exercises `workspace plan`/`workspace apply` with explicit revision and request
id, replays apply to assert `replayed: true`, runs `integrations reconcile` with
explicit revision and request id, replays reconcile to prove no additional
ledger run receipt, and asserts stale-revision and idempotency-conflict contract
responses where the harness can run hermetically. After phase2 it exercises
`retention preview`/`retention apply` for supported scopes on the fake ledger:
schema envelopes, deterministic preview, apply replay, stale-plan and
idempotency-conflict failures, raw-cache deletion with evidence/cursor
preservation, and unchanged source-remove and machine-forget preservation. It
asserts dedicated
`ledger.db` bytes. It asserts dedicated `external_document_evidence` and
`external_document_participants` tables, absence of generic `episodes` and
`recall_items` schema objects, typed `external_record` handles, and
account-derived Meet collection namespaces. Its final machine-forget
fixture verifies both Workspace configs and every evidence-table count remain
unchanged, indexes remain byte-identical, existing Google health rows become
`needs_auth` without losing connector-local `last_sync_at`, and connection
status retains the declaring Workspace list. Granola authorization tests only
the machine transport capability; it has no binding, evidence ledger, health,
or last-sync state. Authorized status requires coherent verified account metadata, OAuth
client identity, required scopes, and a usable refresh path; an orphaned secret
is reported as `needs_attention`.

The report also prints `catalyst mode: <hosted|local|none> (<reason>)`, sourced
from `margins --workspace <id> workspace status --json`. The sandbox clears
inherited provider environment variables before measuring this field, so the
line describes the sandbox's setup result rather than ambient host credentials.

## Agent-side contract: `margins recall` JSON

`margins recall --json` emits `margins.recall.v1` (without `--json`, recall prints readable results):

```text
schema_version: "margins.recall.v1"
status: string
reason: string
freshness: { status, stale, reason?, index: EvidenceFreshness, materialization[] }
results: [{ document_ref, source_kind?, evidence, content, similarity, via_catalyst_id?, via_catalyst_text? }]
top_contributing_catalysts: [{ id, text, entity, topic_name?, relevance_score, contribution_count }]
query, search_strategy, processing_time, total_results
```

Each result carries an opaque stable `document_ref` and a closed `evidence`
handle. There is no product result `file_path` or loose
`indexed_path` provenance. Failed refresh preserves last coherent evidence but
reports stale/error in `freshness`. Missing or invalid catalyst state remains
fail-closed; there is no semantic fallback.

Identity resolution is Workspace-wide: explicit people notes outrank session/
capture attendees, which outrank external-document identities. Names alone never
force a merge. Unified recall can return home
notes, reference notes, Gmail `thread_evidence`, Calendar
`calendar_event_evidence`, and Meet `external_document_evidence` rows
together; `recall --source <name>` restricts by declared binding name without
creating a second index. Native Markdown and Gmail product `document_ref`
values use the stable collection namespaces above, not binding display names.
Calendar uses the account-derived `calendar_<sha256>` namespace; Meet uses
`meet_<sha256>`.

## Lanes and ordering

This is the original implementation ordering, retained as a decision record.
The Workspace model supersedes references to content-folder state and universal
Episode projection; the completed lanes now conform to the boundary above.

1. **Framework + importer boundary** — keep revisioned connector
   plan/apply/reconcile state separate from bounded, ledger-free migration
   imports. This lane defines the contract in code; later lanes conform to it.
2. **Calendar connector** (after lane 1) — Google Calendar via the unified
   machine `margins connect google` credential and native Google driver.
   Calendar is the scoping source: events, attendees, and domain→org inference
   from Workspace authoritative `calendar_event_evidence`. It is what makes
   "prepare the next consequential meeting" computable.
3. **Email connector** (after lane 1) — use the native Gmail REST driver with
   explicit account routing and the machine OAuth capability from
   `margins connect google`. Every thread in the declared Gmail query/backfill
   collection is materialized in `thread_evidence`; participant evidence
   informs KnowledgePolicy curation and never narrows corpus membership.
   Calendar uses the same machine connection and native transport path.
4. **Google Meet transcripts connector** (after lane 1) — Meet auto-saves
   transcripts/recordings as Docs in Drive. Reconcile performs a complete
   provider snapshot with tombstones and
   materializes authoritative rows in `external_document_evidence` with
   role-blind `external_document_participants` when upstream metadata supplies
   observable people. The native driver queries Drive, Docs, and Meet v2; a Meet
   document can still correctly have zero participant associations when upstream
   metadata is unavailable. It does not project home Markdown; retrieval
   authority is the external-document tables.
5. **Granola importer** — survey selected exports or an authorized bounded MCP
   batch in memory, stage and validate minimal native notes, commit them into
   the vault without provenance or persistent migration state, then refresh
   Margins recall.
7. **Setup integration** — the single-folder path is `cd notes`, `margins init`,
   then `margins recall`; multi-source setup remains `workspace new`, declared
   `source add`, then explicit `connect google` and revisioned
   `workspace plan/apply` plus `integrations reconcile` in the workspace-setup
   skill. Only the initial home may be implicit; every additional Source is
   declared.

## Acceptance criteria (all lanes)

- Builds and tests pass on Linux (`cargo test` for the touched public crates);
  fixtures over real-shaped sample data, no live accounts required in CI.
- Idempotency: running `integrations reconcile` twice with unchanged Sources and
  a replayed request id produces zero new authoritative documents for
  Meet/Gmail/Calendar.
- Provenance: every connector result opens its Source. Authoritative external
  records carry typed `external_record` handles with optional href.
- Identity: fixture includes an ambiguous-name case; the connector omits or
  flags it rather than merging.
- Plan/apply/reconcile: no documented flow requires hand-editing TOML.
- The Granola importer produces only the minimal native note shape above,
  preserves existing files through collision allocation, and leaves no staging
  data after success or failure.
- Isolation: an unbound notes-bearing cwd may create the one implicit home only
  after the strict deny-list passes. `$HOME`, `/`, Margins/Enzyme state or
  config, temp paths, empty/non-note folders, and ancestors of declared homes
  refuse before writes; additional Sources are never inferred.
- Local-body boundary: no Workspace-state file contains the body of a declared
  native Markdown root; Enzyme indexes those files in place via
  `markdown_<sha256>/…` document refs.
- Policy: Mail/Calendar/Meet produce no required home note; connector-owned raw
  transport cache is never authority. Granola is outside this connector policy.
- Recall: reconcile automatically refreshes; one query can return home, reference,
  Gmail `thread_evidence`, Calendar `calendar_event_evidence`, and Meet
  `external_document_evidence` rows with correct Source provenance and typed
  external handles.

## Workspace-owned Google connection (resolved 2026-08-24; superseded 2026-08-25)

The managed Google home is `<MARGINS_HOME>/workspaces/<id>/google/`, superseding
the earlier machine-global `<MARGINS_HOME>/google` wording. Connect resolved the
Workspace, completed read-only consent, then declared exactly three fixed
Sources: `google-mail`, `google-calendar`, and `google-meet`. Status JSON reported
the Workspace id, authorized accounts, and attached Sources. Workspace-scoped
`disconnect google` removed those Source declarations and the authorization in
that Workspace home.

This ownership model remains as decision history only. It is not the active
implementation contract.

## Machine-level Google connection and portable consent (resolved 2026-08-25)

Each account has one machine connection at
`$MARGINS_HOME/google/<normalized-account>/`. Workspaces declare the account on
their `google-*` Sources with user-chosen `--name` values and the runner resolves
that account's machine home. `margins connect google [--account X]` consents once
per machine; every Workspace declaring X reuses it. Normal desktop storage uses
the native OS keyring; isolated/headless profiles must deliberately select the
0600 file backend. There is no legacy runner token migration and no portable
bundle import/export compatibility surface. `connect status` lists machine
connections and the sorted Workspace ids using each. Consent never creates or
resolves a Workspace and never declares Sources; `source add` is the declaration
boundary.

`margins disconnect google --account X` explicitly forgets only X's machine
credential (token, OAuth client secret, and managed home). It leaves every
Workspace source declaration, authoritative evidence row, raw cache, materialization
receipt, connector cursor, and recall index intact. Machine results identify the
retained declaring Workspaces. Existing Google connector materializations become
`needs_auth` while retaining the last successful refresh snapshot; a declaration
which has never refreshed has no connector ledger row. Machine connection status
still reports `connected: false` with the retained Workspace association.

Workspace binding changes use exact `source add ... --name NAME` and
`source remove NAME` only. Removing a binding excludes its evidence from
configured retrieval but does not purge ledger rows, raw cache, or indexes by
default. Removed Gmail bindings additionally make their stale materialization
receipts inert; removed Calendar bindings make old status, refresh, and
retrieval inert immediately while authoritative rows remain in the ledger.
Removed Meet bindings make their `external_document_evidence` rows inert.

There is no workspace-scoped `disconnect`, no `--forget`, no `--force`, and no
forced cross-workspace detachment. Connect never declares fixed
`google-mail`/`google-calendar`/`google-meet` source names; those are Source
kinds chosen by the user at declaration time.

### Desktop Calendar product reads (Phase 5)

Desktop capture-title suggestion and in-app Calendar context/meeting prep read
resolved Workspace authoritative `calendar_event_evidence` (and attendee
associations through `calendar_event_attendees`) for the selected Workspace.
They do not maintain a separate Calendar-only OAuth flow, direct Google
Calendar API authority, or machine-global Calendar subset selection. The unified
machine Google connection at `$MARGINS_HOME/google/<account>/` remains the
sole credential boundary; Workspace bindings declare which account and rolling
`CalendarCollectionSelector` apply. The former desktop Calendar-only OAuth,
Keychain credential service, settings, commands, and setup guide have been
removed.

Native Google credentials do not have a portable bundle format. There is no
import/export compatibility surface and no legacy token migration. The machine
connection does not contain Workspace configs, ledgers, indexes, raw caches,
downloaded models, `bootstrap.json`, or `llm-config-cache.json`. Bootstrap is a
machine/broker identity: copying it merges two physical machines into one broker
identity and lease domain, so the safe default is to create a new id. The hosted
credential cache is short-lived and identity-bound; setup reprovisions it.

Gmail cold ingestion runs in quota-bounded waves at no more than 150
`threads.get` calls/minute and retries quota/429 envelopes with bounded
exponential backoff and jitter. The greatest fetched `historyId` is stored in
the connector cursor. Gmail reconcile currently performs complete snapshot
refresh for the declared Gmail selector; warm incremental history refresh is
not implemented yet.

Consumer mailbox domains never become organization/account proposals. Their
addresses remain person proposals. Fixtures retain `historyId` and every
message/header/body field read by detection; the `from:me` fixture proves both
`user_replied > 0` and `user_sent > 0`.

Catalyst generation is narrower than embedding. Workspace recall embeds the
complete declared corpus locally. A non-empty `[policy].entities` is the exact
explicit Enzyme entity surface, minus explicit exclusions. When it is empty,
Margins supplies an automatic selection from noise-filtered correspondents,
people notes, and session attendees.

**Superseded 2026-08-25 by automatic correspondent entities; approval is now
operational activation only.** Automatic selection is made from noise-filtered
correspondents, people notes, and session attendees, minus explicit entity
exclusions; an explicit entity surface replaces it.

**Round-8 clarification (2026-08-25):** Gmail membership is workspace desired
state; Margins—not raw engine occurrence frequency—owns correspondent selection
through KnowledgePolicy.
Without explicit curation, the ephemeral config contains the bounded weighted
entity list minus explicit exclusions and connected-account identities. An
explicit entity list passes through with its profile and expansion options.
Automated/public-domain
and self link values are also written to `excluded_links`; their retained
threads may support selected humans but can never become catalyst topics.

PR #74 replaced the compatibility path with one generic thread-evidence
document per authoritative Gmail thread and rebuildable, role-blind
`participant_threads` associations. Margins supplies stable document refs,
entity links, observable importance, and authoritative provenance. Enzyme owns
generic occurrences, catalyst generation/invalidation, receipts, epochs,
grounding, and search ranking; it receives no raw Gmail payloads, mail roles,
message IDs, ownership spans, or mail-specific thresholds.

## Ownership gate (Phase 6)

| Concern | Margins | Connector | Enzyme |
| --- | --- | --- | --- |
| Connection UX, WorkspaceBinding, ledger transactions | owns | — | — |
| Authoritative records, refresh/tombstones/retention | owns | — | — |
| KnowledgePolicy, retrieval/provenance/hydration | owns | — | — |
| Provider transport/auth/cursors/parsing/quota/deletion | — | owns | — |
| Observable association signals | supplies | derives | — |
| Catalyst selection | owns policy inputs | never selects | generates |
| Generic ingestion/hashes/embeddings/occurrences | — | — | owns |
| Catalysts/receipts/epochs/grounding/ranking | — | — | owns |
| Engine inputs | stable document refs, body/title/time, role-blind entity links | — | receives only |

Connectors never pass provider raw payloads, retention rules, or source-specific
engine behavior into Enzyme. Margins never owns generic embedding or catalyst
generation. There is no compatibility path, migration shim, generic connector
registry, generic Episode register, message ownership spans,
raw payload ingestion into Enzyme, or vendored Enzyme edits in this phase.
Credential loss preserves bindings and evidence while marking refresh
unavailable or stale.

## Hosted catalyst setup contract (resolved 2026-08-24)

- Credential resolution is a setup concern. `margins setup`, successful Google
  connect, and desktop Included preparation may contact the free-config broker;
  recall lookup may not.
- CLI and desktop share `$MARGINS_HOME/bootstrap.json`. Desktop reconciles that
  file with its bootstrap-id Keychain item, preferring an existing file id. CLI
  reads/writes only the file and never touches Keychain.
- Setup publishes `$MARGINS_HOME/llm-config-cache.json` at mode 0600 with
  `api_key`, `base_url`, `model`, `expires_at`, `cached_at`, and the identity
  profile. Secrets are neither printed nor included in fixtures.
- Success pins `[llm].mode = "hosted"`. Broker failure deliberately pins
  `mode = "local"`; `margins setup` is the refresh/retry command.
- An Included key passed to desktop recall provisioning uses the explicit
  `OPENROUTER_API_KEY` seam. A pre-existing explicit environment key wins.
- Provider rejection deletes only the matching cached lease/bundle. Refresh is
  performed by setup, never by a recall query.

The read-only Workspace status contract includes `catalyst: { mode, reason }`.
It reports selection only and never serializes bundle fields. Official
`margins init` mirrors the same pair as `catalyst` and `reason` XML attributes;
`none` additionally emits the one-line `run margins setup` remediation.

## Recall-engine #13 compatibility note (2026-08-25)

The synced enzyme-rust PR #13 makes catalyst text canonical. Every stored chunk
contains a versioned JSON header, hypothesis prose, and fenced JSON-lines
receipts. `margins recall` passes the full stored chunk through in additive
result `via_catalyst_text` and contributor `text`, and the tree preserves its
receipt lines. Margins omits struct-level anchors because those fields, like the
`catalyst_receipts` and `catalyst_eras` tables, are derived and rebuildable from
the canonical text.

The engine's thin-evidence guard currently requires at least four distinct
documents and 128 substantive tokens per eligible entity/era. An entity below
either floor is recorded as skipped/suppressed and receives no catalyst. A small
curated workspace may therefore appear as `catalysts_pending` in Margins'
inventory-oriented status even though the engine correctly suppressed its thin
entity; fixtures must add evidence rather than lower the guard.

Identity-first retrieval prioritizes catalysts belonging to a person or
organization resolved from the query, then falls back globally. Gmail supports
that generic contract through Margins-curated entity links associated with the
thread document; it does not expose per-message ownership to the engine.
