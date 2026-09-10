# Integrations ledger as a recall SQLite source

Status: built into the Workspace model, 2026-08-24; Phase 7 removed the generic
Episode register and `recall_items` view.

## Minimal contract

Workspace recall and context authority is native Markdown plus dedicated
evidence tables for the four external connector kinds. There is no generic
Episode register, `recall_items` view, or Episode-framework projection path.

| Source | Authority |
| --- | --- |
| Native Markdown | Enzyme direct ingestion from declared `notes` roots |
| Gmail | `thread_evidence` plus rebuildable `participant_threads` |
| Calendar | `calendar_event_evidence` plus rebuildable `calendar_event_attendees` |
| Google Meet | `external_document_evidence` plus rebuildable `external_document_participants` |
| Granola | `external_document_evidence` plus rebuildable `external_document_participants` |

Recall does not need to understand connector-specific JSON beyond these
boundaries. Raw cache is never evidence; each dedicated table is supplied
through its generic evidence-document adapter.

**Superseded 2026-08-25 by automatic correspondent entities.** Here and below,
the curated set is now the selected set of automatic or explicitly pinned
entities minus explicit exclusions.

The ledger schema is recreation-only; there is no migration or compatibility
path for removed Episode-framework objects.

## Automatic Workspace runtime configuration

Durable Workspace state is:

```text
$MARGINS_HOME/workspaces/<id>/config.toml
$MARGINS_HOME/workspaces/<id>/index.db
$MARGINS_HOME/workspaces/<id>/ledger.db
```

`margins init` and the refresh at the end of a successful reconcile generate an
ephemeral recall-engine home. Margins binds each declared native Markdown root
with `EnzymePaths::bound_workspace_with_home`, supplying a deterministic
`document_ref_prefix` of `markdown_<sha256>/` derived from the absolute root
path. It automatically supplies a peer SQLite Source for each declared
Mail/Calendar binding, filtered by connector and account. Gmail binds
`thread_evidence` under the account-derived `gmail_<sha256>` collection key;
Calendar binds `calendar_event_evidence` under the account-derived
`calendar_<sha256>` collection key. Google Meet binds
`external_document_evidence` under the account-derived `meet_<sha256>`
collection key. Granola binds `external_document_evidence` under the
account-derived `granola_<sha256>` collection key. Conceptually, one generated
Calendar block is:

```toml
[workspaces.<id>.sources.calendar_<account_sha256>]
db = "/absolute/margins-home/workspaces/<id>/ledger.db"
query = "SELECT source_id AS id, 'sqlite:calendar_<account_sha256>/' || lower(hex(CAST(source_id AS BLOB))) AS document_ref, CAST(strftime('%s', occurred_from) AS INTEGER) * 1000 AS occurred_at_ms, title, body_text AS body, <ordered attendee JSON array> AS participants FROM calendar_event_evidence WHERE connector_id = 'gcal' AND source_account = 'person@example.com' AND tombstoned_at IS NULL AND occurred_from BETWEEN <rolling-start> AND <rolling-end>"
roles = { id = ["id"], document_ref = "document_ref", who = { format = "json_array", column = "participants" }, when = "occurred_at_ms", what = ["title", "body"] }
timestamp = { unit = "ms" }
```

Gmail uses an analogous generated query over `thread_evidence` with
`participant_threads` for composite participant roles, keyed by
`gmail_<sha256>` rather than the binding display name. Meet and Granola use
analogous generated queries over `external_document_evidence` with
`external_document_participants`, keyed by `meet_<sha256>` and
`granola_<sha256>` respectively. Calendar attendee associations are supplied
separately through `calendar_event_attendees`. Users and agents never
hand-edit any generated block.
`margins recall` remains read-only and never repairs an index. The generated
runtime disappears after refresh; the machine-global Margins config keeps only
machine concerns.

## Native Markdown roots

Workspace desired state declares native Markdown under `[bindings.<name>]` with
concrete `notes` variants (`path` + `role`). Margins ingests every declared
root directly through Enzyme; there is no FolderConnector production authority
for those bindings.

Each root receives a deterministic `document_ref_prefix` of
`markdown_<sha256>/`, derived only from the declared absolute path. Binding
display names never enter engine document refs. A single implicit home receives
the same prefix treatment as additional reference roots. Changing a root path is
a new collection; relocation-preserving identity is out of scope for this
slice.

Enzyme-rust seams PR #14 landed multi-root native ingestion. Margins declares
the home and every reference root as named Markdown roots with `path`,
`exclusions`, and `writable`; it creates no reference SQLite bridge file and no
symlinked virtual home. Result remapping restores binding name, kind, stable
`document_ref`, and the closed `evidence` handle for unified recall and
`--source <name>`. Enzyme's internal `file_path` name is not a product path and
is never published in recall JSON.

## Incremental reconciliation, identity, and provenance

The SQLite provider deliberately reads the complete configured query on every
refresh. It sorts rows deterministically, derives the recall source ref from
the configured source name plus typed `id`, fingerprints rendered content and
metadata, skips unchanged documents, rebuilds changed documents, and prunes ids
that disappeared. This is full-snapshot change detection rather than a fragile
`fetched_at > cursor` scheme; backdated edits and tombstones are therefore
correct. The expected fixture is small enough that query cost is negligible.

Recall identity for native Markdown is `markdown_<sha256>/…` from the declared
absolute root path. Gmail recall identity is `sqlite:gmail_<sha256>/…` from the
normalized account only; binding rename and selector changes preserve thread
refs. Calendar recall identity is `sqlite:calendar_<sha256>/…` from the
normalized account only; binding rename and selector changes preserve event
refs. Meet recall identity is `sqlite:meet_<sha256>/…` from the normalized
account only. Granola recall identity is `sqlite:granola_<sha256>/…` from the
normalized account only. The ledger's connector/account/source identity is also
present in the indexed provenance JSON, so a hit can be traced even if
source-ref encoding changes. Gmail, Calendar, Meet, and Granola provenance are
carried separately by their authoritative evidence adapters.

Stable identity does not imply fresh desired state. The Gmail snapshot
transaction receipts the exact query/backfill selector separately; a selector
change retains the old indexed threads and hashes while recall, integration
status, and workspace source status report
`stale`/`refresh_required` until a successful refresh under the new selector.
Binding display-name changes do not invalidate that account-scoped receipt.

One product-wrapper fix was required: Workspace staleness compares Markdown
files only with filesystem-backed recall documents. Structured SQLite sources
reconcile themselves during refresh; counting `sqlite:` documents as Markdown
made every successfully refreshed mixed index immediately appear stale. No
recall-engine internal was changed.

## Projection policy

- Ledger-only by default: connector payloads and other high-volume/re-fetchable
  records remain in their dedicated evidence tables. Calendar events are
  materialized in `calendar_event_evidence` with no home projection. Gmail
  threads are materialized in `thread_evidence` with no home projection. Meet
  and Granola meeting records are materialized in `external_document_evidence`
  with no required home projection.
- Project as notes: deliberate summaries, decisions, commitments, and
  user-edited artifacts. These are durable knowledge objects with a useful
  Markdown lifecycle (links, edits, sharing, publishing).
- Optional managed projection: Granola may write tagged home Markdown
  (`margins-managed-projection`) under configured projection folders; that
  projection is excluded from native-Markdown ingestion. Connector observations
  never auto-create native person or organization notes; entity promotion is a
  Margins KnowledgePolicy operation. Meet currently writes no projection.
  Projection is never a prerequisite for retrieval.

## Fixture evidence

The Linux integration test `integrations_recall_sqlite_source` uses recorded
Google fixtures end to end: reconcile/ingest, automatic Workspace provisioning,
then catalyst-backed recall through the fixture generator.
The fixture produces three thread-keyed raw Gmail rows materialized into
`thread_evidence`. The deterministic generator proves that a curated entity's
catalyst can surface that materialized row as evidence, with a catalyst id on
the hit; the thread has no Markdown projection and retains Gmail thread
provenance. The test does not establish raw-content searchability, which shipped
product paths no longer provide.

On the shared Linux debug lane used for this prototype, two fixture runs
measured 8 ms reconcile transport, 19 ms ingest, 705–781 ms index/embedding, and a
229,376-byte (224 KiB) `index.db`. These are diagnostic fixture numbers, not
performance budgets; the test prints fresh values with `--nocapture`.

## Follow-ups and limits

- Automatic provisioning is resolved: Workspace setup, `init`, and successful
  reconcile generate the ledger block and reconcile refreshes recall before returning.
- Connector-native JSON is transport/cache input, not evidence. Attachments and
  externally stored bodies need an explicit authoritative materialization policy.
- Materialized source documents are embedded into the recall index, so avoiding
  Markdown projection does not avoid index storage. Explicit retention preview/
  apply prunes authoritative evidence and triggers an Enzyme refresh for scopes
  `materialization` and `all`; raw cache is separately purgeable under
  `raw-cache` or `all`.

## Retention and purge (Phase 9)

Retention is never automatic. Workspace config may declare optional
`[retention]` age cutoffs (`raw_cache_max_age_days`, `tombstone_max_age_days`);
absent fields retain until explicit purge. Age eligibility applies only to the
`expired` scope during preview and never deletes active authoritative evidence.

```bash
margins --workspace <id> retention preview \
  --connector ID --account ACCOUNT \
  --scope expired|raw-cache|tombstones|materialization|all --json
margins --workspace <id> retention apply \
  --plan FILE --if-revision REV --request-id ID --json
```

Closed connector ids are `email`, `gcal`, `google_meet`, and `granola`; accounts
are normalized. No active binding is required, so removed collections remain
purgeable. Preview emits `margins.retention.preview.v1`; apply emits
`margins.retention.apply.v1`; failures remain `margins.error.v1`.

| scope | ledger effect | index effect |
| --- | --- | --- |
| `raw-cache` | deletes `raw_items` only | none |
| `tombstones` | physically deletes tombstoned authoritative rows | refresh only if preview marked `index_refresh_required` |
| `materialization` | deletes active+tombstoned authoritative evidence and associations, connector state/cursor/health, curation observations; preserves raw cache and historical run/reconcile/purge receipts | official binary refreshes generic Enzyme sources |
| `all` | `materialization` plus `raw-cache` | same as materialization |
| `expired` | rows eligible under `[retention]` cutoffs only | per preview |

There is no native Markdown deletion, no Granola projection-file deletion,
no provider deletion, no message-level Gmail target, no receipt deletion, and
no automatic purge. Source removal removes only the selected binding while
preserving evidence, raw cache, and indexes; credential loss preserves bindings
and all retained data.

After `materialization` or `all`, the official runtime regenerates the ephemeral
recall-engine configuration and prunes derived docs/chunks/occurrences/entities/
catalysts from `$MARGINS_HOME/workspaces/<id>/index.db`. There is no Enzyme
retention policy and no upstream seam.

## Gmail catalyst policy after PR #74

Each Gmail thread is one authoritative `thread_evidence` record and one generic
evidence document with a stable `document_ref`. `participant_threads` is a
rebuildable, role-blind association between normalized participant identities
and that thread; it does not preserve per-message attribution or message IDs as
engine inputs. Raw connector cache is never evidence and cannot
create an occurrence or resurrect a missing thread.

The connector derives observable participant, activity, recency, and noise
signals. Margins KnowledgePolicy uses those signals with pins, exclusions, and
budgets to choose curated entities. Connected-account
identities, likely automation, and public mailbox domains remain suppressed
according to Margins policy unless explicit product policy says otherwise.

Margins supplies Enzyme with generic evidence documents, stable entity links,
importance, and authoritative provenance handles. Enzyme owns generic hashes,
embeddings, occurrences, catalyst generation/invalidation, receipts, epochs,
grounding, and search ranking. It receives no Gmail roles, message IDs,
ownership spans, raw payloads, retention rules, or mail-specific floors. One
shared thread can associate with several participant entities while remaining
exactly one document and one result reference.

## Calendar evidence policy (Phase 5)

Each authoritative calendar event is one `calendar_event_evidence` record and
one generic evidence document with a stable `document_ref` in the
account-derived `calendar_<sha256>` namespace. `calendar_event_attendees` is a
rebuildable, role-blind association between normalized attendee identities and
that event; source-native observable metadata may be retained, but attendee
sampling/ranking cannot change corpus membership. Raw connector cache is never
evidence.

The rolling boundary lives only in workspace desired state
(`CalendarCollectionSelector`: `lookback_days` and `lookahead_days`). Reconcile
derives the request boundary from the active binding selector. Explicit
cancellation tombstones the authoritative event.
Failed refresh preserves the last coherent materialization and reports
stale/error machine-readably.

For Granola supplemental refresh, missing notes/transcript fields are filled
only from the prior authoritative ledger row. Projection Markdown is never read
as provider evidence, and transport warnings mark the materialization stale.

Product evidence is `EvidenceHandle::ExternalRecord { connector_id: "gcal",
source_account, source_id, href? }`. Desktop capture-title suggestion and
Calendar context/meeting prep consume the same Workspace authoritative rows; they
do not use a separate Calendar-only OAuth/API path or machine-global Calendar
subset authority.

## Meet and Granola evidence policy (Phase 6)

Each authoritative Meet or Granola document is one `external_document_evidence`
record and one generic evidence document with a stable `document_ref` in the
account-derived `meet_<sha256>` or `granola_<sha256>` namespace.
`external_document_participants` is a rebuildable, role-blind association
between normalized participant identities and that document; source-native
observable metadata may be retained, but participant sampling/ranking cannot
change corpus membership. Connector-specific raw cache is optional and never
evidence.

Meet reconcile is a complete provider snapshot with explicit tombstones.
Workspace `margins sync` and explicit `integrations reconcile --connector
granola --account EMAIL ...` run the portable authenticated MCP refresh for the
bound account. Generic `integrations reconcile` has no implicit Granola
transport and a broad reconcile skips Granola. Explicit `margins import granola
PATH --account EMAIL` remains the separate supplemental file-import path.
Granola supplemental refresh
does not claim complete snapshot deletion semantics comparable to Meet
tombstones.

Product evidence is `EvidenceHandle::ExternalRecord { connector_id:
"google_meet" | "granola", source_account, source_id, href? }`. Optional
Granola managed projection Markdown is tagged
`margins-managed-projection` and excluded from native-Markdown ingestion; Meet
currently writes none. Persistent Granola desktop/CLI connection status is
transport/auth only and has no machine-global last-sync authority over Workspace
evidence freshness. It is connected only when verified account metadata, client
identity, required scopes, and a usable refresh path are coherent; an orphaned
credential is `needs_attention`.
The managed-projection tag stays excluded after binding removal so retained
projection files cannot resurrect excluded external evidence as native notes.

## Hosted generation boundary (resolved 2026-08-24)

The Workspace wrapper's generator receives credentials established by machine
setup. The persisted bundle lives at `$MARGINS_HOME/llm-config-cache.json`; its
identity lives at `$MARGINS_HOME/bootstrap.json`; and machine `config.toml`
explicitly selects `hosted` or the deliberate `local` fallback. Desktop may
temporarily expose an already-cached Included key through `OPENROUTER_API_KEY`
during index provisioning, preserving the engine's explicit-env precedence.

None of these operations occurs in `recall()`: lookup opens an existing index
and has a fixture assertion that even a configured loopback broker receives no
connection. Credential expiry/rejection is repaired by rerunning setup, not by
search silently fetching a replacement.

Google consent has a separate portability boundary. Workspaces retain their
own binding declarations and ledgers, but resolve `account` on Google bindings
through the machine connection at `$MARGINS_HOME/google/<account>/`. The encrypted Google
export carries only OAuth client credentials and that account's token; it does
not carry this Workspace's ledger/index or catalyst selection state.

`bootstrap.json` remains the local machine's broker identity. Porting it would
merge machine identities and leases, so connection import leaves it alone.
`llm-config-cache.json` is never portable: it expires, is bound to setup's
machine identity, and is reprovisioned by `margins setup`.

## Catalyst selection status (2026-08-24)

Recall's setup-only credential boundary makes catalyst availability a durable
machine status rather than something query-time resolution may repair. The
shared status vocabulary is therefore exposed in both initialization XML and
Workspace status JSON:

- `hosted / explicit_env` or `hosted / hosted_bundle_ready`;
- `local / local_selected`;
- `none / setup_required`, or a precise unreadable, invalid, missing, or expired
  configuration/bundle reason.

This inspection is read-only and secret-free. In particular, it does not log or
serialize the cached API key and does not probe the hosted endpoint. A `none`
selection adds the user-facing hint `Catalyst generation unavailable — run
margins setup.` while direct indexing remains available.

## Enzyme #13 text-canonical catalyst contract (2026-08-25)

The recall-engine subtree is synced to enzyme-rust PR #13. Catalyst text is the
canonical record: a versioned JSON header, hypothesis prose, and a fenced
JSON-lines receipts block. Recall JSON passes that complete stored chunk through
unchanged as additive result `via_catalyst_text` and contributor `text`; the tree
prints the same chunk while preserving receipt line boundaries. Margins omits
the engine's struct-level anchor projection instead of independently rendering
derived anchors.

`catalyst_receipts` and `catalyst_eras` are rebuildable indexes parsed from the
stored chunks. The grounding guard currently requires at least four distinct
documents and 128 substantive tokens for an entity/era. Below-threshold evidence
is recorded by the engine as skipped/suppressed and produces no catalyst. Because
Margins readiness is an inventory summary, a curated entity with no materialized
catalyst can surface as `catalysts_pending`; small fixtures and workspaces must
add evidence rather than weaken the threshold.

Search is identity-first: an entity-resolving query prioritizes that identity's
catalysts before global fallback. Margins supplies the curated entity list;
Calendar entity links come from the role-blind `calendar_event_attendees`
evidence adapter; Gmail entity links come from the role-blind
`participant_threads` evidence adapter; Meet and Granola entity links come
from the role-blind `external_document_participants` evidence adapter.

Source freshness is likewise engine-owned after sync #2. Recall reads
`source_refresh_staleness` for every configured SQLite Source and reports the
engine's successful-refresh watermark, `stale` bit, and `stale_reason`; it no
longer compares ledger/WAL mtimes against a parallel Margins interpretation of
the `source_refreshes` table.

## Product recall JSON envelope (`margins.recall.v1`)

Piped or otherwise non-interactive `margins recall` is a Margins-owned envelope, not raw engine
output. Required top-level fields are `schema_version`, retrieval `status`,
`reason`, and `freshness`. `freshness` contains `status`, `stale`, optional
`reason`, a separate `index` object, and per-source `materialization[]` entries
each with `source`, `status`, `stale`, optional `reason`, and optional
`last_successful_refresh`.

Each result has an opaque stable `document_ref` and closed `evidence`:

- `evidence.kind = native_markdown` is the only variant with `path`.
- `evidence.kind = external_record` has `connector_id`, `source_account`,
  `source_id`, optional `href`, and never a path.

There is no product result `file_path` or loose `indexed_path` provenance.
Gmail reconcile marks successful authoritative materialization fresh; a
failed refresh records error while preserving the prior thread snapshot.
Materialization freshness and Enzyme index freshness are separate.
