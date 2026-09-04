# Integrations layer: design rationale and simplification notes

Companion to `INTEGRATIONS_CONNECTOR_CONTRACT.md` (the *what*). This document
records the *why* — the product and technical reasoning behind the integrations
database and its surrounding layer, the trade-offs taken under time pressure,
the places we already know are heavier than they need to be, and the evidence
from real-machine validation. It exists so a later review agent can revise the
design with the original intent in hand rather than reverse-engineering it.

Written 2026-08-24 by the orchestrating agent after the initial build (PRs
#19–#37) and four rounds of macOS validation on the owner's real machine.

---

## 1. What this layer is for (product frame)

The ICP is an owner-led retained-service firm — a boutique AI implementation,
research, or strategy agency with 5–20 active client relationships. The product
object is the **client relationship**; meetings, email, calendar, and documents
are *evidence* about it. The integrations layer exists for one reason: **value
must be apparent within minutes of install, from history the user already has**,
because a recorder proves itself only after weeks of new meetings, and no
principal waits weeks.

Two consequences shaped everything below:

1. **Connectors feed exactly two consumers.** (a) The relationship re-entry
   packet — "what is this conversation part of" — assembled from a bounded
   attendee/account neighborhood with a source manifest and temporal cutoff.
   (b) The held-set transition surface — the imported corpus arriving as a
   handful of proactive, source-backed utterances ("Earl Lee — new thread since
   you last spoke — receipt"). Anything ingested that cannot become a receipt
   behind an utterance or a packet claim is wasted scope. There is deliberately
   no browsing UI, no unified inbox, no dashboard.
2. **The agent orchestrates; the CLI executes.** The CLI never asks a question
   and never applies judgment. It exposes *evidence* (reply counts, automated-
   mail signals, date ranges) so an orchestrating agent — Claude Code, a custom
   stack, eventually a hosted agent — applies thresholds in conversation with
   the user. Heuristics are proposals and flags, never hard filters. The user
   never runs `survey/approve/pull` by hand; an agent skill
   (`margins guide onboarding`) does.

## 2. Why a SQLite ledger *and* selective markdown projection

The original implementation kept two representations of every imported item.
That universal-projection rule is **superseded by the Workspace model**. The
built design keeps machine truth at `$MARGINS_HOME/workspaces/<id>/ledger.db`
and projects only human-readable remote kinds into the declared writable home
notes Source at `<home>/Margins/Transcripts/`. Mail and Calendar remain
ledger-only; notes and reference folders remain native files. See the policy
table below. The reasons for retaining selective projection still matter:

- **Trust is won by inspectability.** A principal must be able to open and read
  durable human artifacts with ordinary file operations. Markdown in their
  home notes Source is the most legible representation for transcripts and
  authored knowledge. Hosted sources stay authoritative; Margins is a private
  context plane, never a system of record.
- **Recall now spans native Markdown and SQLite Sources.** Home and reference
  notes are indexed natively; `ledger.db` supplies Mail and Calendar through
  `recall_items`. The original “recall only reads Markdown” constraint is
  superseded by the SQLite-source seam documented in
  `INTEGRATIONS_RECALL_SQLITE_SOURCE.md`.
- **Idempotency, tombstones, and re-projection need a ledger.** Composite
  primary keys (`connector_id, source_account, source_id`) plus `content_hash`
  make "pull twice = zero new episodes" cheap; `tombstoned_at` survives a
  deleted note; `raw_items` lets a connector re-project without refetching
  when the projection format changes. None of that belongs in frontmatter.
- **Deleting a projected note must not corrupt anything.** The ledger is
  authoritative for ingestion state; the note is a view. `reproject_episode`
  rebuilds a note from `raw_items` where retained.

The Workspace still coordinates a ledger, selected notes, and one recall index,
but it no longer duplicates every remote or local item as Markdown.

Per-kind policy is declared once in code and connectors cannot override it:

| Source kind | raw payload cache | project to home | indexed how |
|---|---|---|---|
| `notes` (home/reference) | no | n/a | native Markdown, same exclusions |
| `captures` | no | n/a | native/session registry |
| `google-mail` | yes, quota-bound | no | ledger `recall_items` |
| `google-calendar` | yes, quota-bound | no | ledger `recall_items` |
| `google-meet` | yes, quota-bound transport | yes | projected home note |
| `granola` | yes, source-native transport | optional managed view | ledger external-document evidence |

## 3. Schema rationale (Workspace schema v1)

- **`connectors`** (`connector_id, account` PK): approved plan (JSON), cursor,
  last sync, scope boundary, health. Approved plans are persisted so `pull`
  can refuse to run against anything but the exact plan a human/agent
  approved from a survey. This is the "setup proposes, user approves, nothing
  hand-edited" doctrine made mechanical.
- **`episodes`** (`connector_id, source_account, source_id` PK): kind,
  occurred_at, content_hash, provenance, nullable `indexed_path`, imported_at,
  tombstoned_at. `indexed_path` is an original file for native registry rows,
  a projected home note for human-readable remote kinds, and `NULL` for
  ledger-only evidence. `projection_json` may also be SQL `NULL` or JSON
  `null`; context must render those rows from `recall_items`/raw payloads.
  Provenance is stored *per episode*, not derived, because every visible claim
  downstream must open its source.
- **`raw_items`** (same PK): fetched-but-not-projected rows. Email uses it
  heavily (fetch a page of threads, project only the allowlisted subset);
  folder/calendar barely need it. Optional by design.
- **`runs`**: per-pull manifests — counts, errors, and *candidate transitions*.
  Transition candidates are computed at pull time and recorded here so the
  transition surface has a wiring point even before the proactive UI exists.

Why SQLite specifically: it matches the rest of Margins (recall index,
sessions), it is inspectable with standard tools, and the pattern is the same
one gmail-to-sqlite-style tools use locally. No server and no compatibility
migration: the Workspace schema is the clean replacement.

## 4. Identity doctrine, and why it is enforced in the ledger

The most expensive class of error is merging two people. The rules, in
priority order, are: resolve by email → by exact full name → by alias; never
merge on a bare first name (the "two Kevins" rule); when ambiguous, omit or
flag, never guess. These are enforced at two points, deliberately redundant:

- **Projection**: ambiguous identities are written as raw evidence
  (`people_evidence`) but never become wikilinks/backlinks — even if a
  connector supplies one.
- **Transition candidates**: require unambiguous structured evidence (unique
  full name + valid email, or a wikilink); explicitly ambiguous identities,
  and full names tied to multiple distinct emails in one pull, are excluded.
A first version of the transition relaxation admitted ambiguous identities;
the Granola characterization test caught it. Reviewers should treat these
tests as load-bearing product policy, not incidental coverage.

## 5. Evidence, not filters (email in particular)

Real-mailbox validation drove three iterations:

1. Wholesale approval of surveyed senders imported 96 episodes, 49 of them CI
   failure mail. → Approval now *refuses* email with no explicit
   `--people/--domains`; the agent must choose.
2. A `likely-automated` flag existed but flagged only when *every*
   observation was automated; on a real mailbox 0 of 28 obvious automated
   senders were flagged. → Reply-weighted scoring: a user reply ranks a sender
   first and vetoes the flag; sender/domain patterns (`noreply.`, `promo.`,
   `services.`, newsletter platforms) suffice to flag. Result: 24/28 flagged,
   0/20 false positives; the last four were a missing `services.` label, since
   added.
3. Agents needed the raw numbers, not our verdict. → `survey --json` exposes
   per-proposal `{threads, user_replied, user_sent, last_interaction,
   automated_signals}` so an agent can apply "replied at least once" itself.

The principle for reviewers: **ranking and flags are defaults to propose;
counts are the contract.** Do not add hard filters to the CLI.

## 6. Native Google as transport

Google access is a narrow Rust driver owned by Margins. `margins connect google`
owns the user-facing OAuth ceremony and credential persistence; Workspace config
owns desired state; connectors own ledger transactions and tombstones; the
native driver owns installed-app OAuth integration, HTTPS transport, upstream
cursors, pagination, quota retry, parsing, and deletion detection.

The driver intentionally does not expose a generic provider registry, runtime
Discovery behavior, subprocess helpers, or PATH-dependent commands. It calls the
exact REST surfaces Margins uses today: Gmail threads/history, Calendar
events/sync tokens, Drive file discovery/export, Docs content where required,
and Meet conference/transcript/participant metadata. Existing connector parsing
and materialization code remains the authority for what enters the ledger.

Credentials are machine OAuth capability only. Account-scoped state lives under
`MARGINS_HOME/google/account`; refresh/access tokens never enter argv, logs,
Workspace config, ledger rows, fixtures, or output. The default backend uses
the OS keyring where usable. Headless Linux can opt into a deliberate 0600 file
backend whose limitation is explicit: it is portable and inspectable, but its
secrecy is bounded by the local account and filesystem permissions.

The user-facing surface remains `margins connect google`: browser plus loopback
callback by default, and an explicit `--headless` mode that prints the consent
URL to stderr before consent, emits deterministic nonsecret JSON to stdout, and
accepts the final address-bar URL through a no-echo local prompt. Neither mode
asks a user to paste OAuth URLs or tokens into an agent transcript. The ready gate verifies the
stored account and granted read-only Gmail, Calendar, Drive, Docs, and Meet
capability through API probes before the connection is considered ready.

The cost that remains is Google app verification. Brand verification (name,
logo, homepage/privacy/terms on a Search-Console-verified domain) is free and
fast, but it is a prerequisite for showing the logo at all, and Publish is
disabled until privacy/terms URLs exist. Gmail read scopes are "restricted":
they need declared scopes with justification, Limited Use disclosures, and an
end-to-end demo video, on a roughly six-week review. The CASA security
assessment applies when restricted data is accessed from or through a
third-party server; Margins is device-local and does not transmit Gmail data to
Enzyme servers, so confirm the current Google wording before assuming either
outcome. Pilots ship unverified with a one-line explanation of the warning and
verification follows when account counts justify it.

### Declared OAuth scopes

Exactly what `margins connect google` requests for Gmail, Calendar, Drive, Docs,
and Meet; nothing broader:

| Google class | Scope |
|---|---|
| Non-sensitive | account email identity |
| Sensitive | `calendar.readonly`, `documents.readonly`, `meetings.space.readonly` |
| Restricted | `gmail.readonly`, `drive.readonly` |

Two restricted scopes means restricted-scope review regardless; `drive.readonly`
is used to discover Meet transcript Docs in the "Meet Recordings" folder. A
reviewer could check whether `drive.metadata.readonly` + `documents.readonly`
suffices for that discovery — it would drop Drive out of the restricted tier
and shrink the review surface.

## 7. Known tensions and simplification candidates

Ranked by how much a reviewer could simplify with the least product risk.

1. **Survey mutation is now deliberately raw-only.** Gmail survey performs a
   bounded, idempotent ingest into the selected Workspace `ledger.db`;
   proposal evidence is queried from `raw_items`, and pull
   projects the approved subset from the same rows. This replaces the former
   "survey never writes" invariant with the narrower and testable rule
   "survey never projects human-facing home notes." The cheap
   same-day summary cache remains under `MARGINS_HOME`, while raw rows are the
   source truth. The path-evidence guard described here is superseded by the
   Workspace selector: `--workspace <id>`, `MARGINS_WORKSPACE`, or a persisted
   declared Source binding. Superseding the original arbitrary-cwd refusal, an
   unbound notes-bearing cwd may now create the one implicit home Workspace
   after a strict deny-list passes; `$HOME`, `/`, Margins/Enzyme config or state,
   temp paths, non-note folders, and ancestors of declared homes still refuse
   before the ledger can open. Every Source beyond home remains declared. This
   trade makes survey/pull agreement and
   re-projection without network access structural rather than best-effort.
2. **Three stores to keep consistent** (ledger, projected notes, recall
   index). Today consistency is by construction — projection writes the note
   and registers it in one transaction; recall re-indexes on refresh. A
   reviewer could ask whether the recall index should read the ledger directly
   for Episodes and skip projection for kinds nobody reads (calendar events are
   the obvious case: 1,524 projected notes for a year of events is noise in a
   home notes Source). **Resolved:** project only human-readable kinds (Meet
   transcripts and already-native Granola notes); Mail and Calendar stay
   ledger-only and enter recall through `recall_items`.
3. **Per-connector approved plans are richer than needed.** Email carries a
   people/domain allowlist plus generated Gmail queries; calendar carries a
   time window; folder carries include/exclude globs. The shape is
   heterogeneous JSON. A reviewer might unify to `{scope: {...}, window:
   {from,to}}` per connector and drop connector-specific plan validation.
4. **`raw_items` retention is unbounded.** Mail and Calendar keep fetched rows
   row. That is what makes re-projection free, but there is no retention
   policy. Add one (e.g. keep raw rows only for projected episodes plus N days)
   or accept the growth and say so.
5. **Transition candidates are a print statement.** They are computed at pull
   time, stored in `runs`, and printed. The proactive surface that should
   consume them (see the 2026-08-17 transition-surfaces doctrine: named person
   + concrete action + why-now + receipt, 1–3 per import, no dashboards) does
   not exist yet. Reviewers should resist turning the candidates into a
   ledger-backed "inbox"; the doctrine explicitly refuses that.
6. **The e2e harness's isolation check is file-based.** It cannot see the
   macOS Keychain (hence the forced file keyring in the harness) and it once
   printed a stale verdict. The connect-google managed home lands inside the
   sandbox automatically, which removes the keyring dance for real users but
   the harness should still assert the managed home's path.

9. **Fixtures cannot see fetch-shape regressions.** PR #42 made the Gmail
   survey ~3× faster by fetching a smaller thread payload — and silently
   zeroed `user_replied`/`user_sent` on a real mailbox (0/159 vs 43/199 the
   run before), because the detector read per-message sender fields the
   smaller payload no longer carried. The recorded fixtures passed. Rule for
   reviewers: any change to what a connector fetches needs (a) a test that
   the fixture payload contains every field a downstream detector reads, and
   (b) a real-mailbox spot-check of the evidence counts before merge.

### Resolved on 2026-08-24 (late): ledger-first survey and ledger-only recall

- **§7.1 resolved differently than first proposed.** Rather than making survey
  cheaper, the owner redirected to *ledger-first*: `survey` is now a bounded,
  idempotent ingest (`from:me` correspondence sample ∪ recent-inbox coverage
  sample, thread-keyed) into `raw_items`; evidence is computed from stored
  rows; `pull` projects the approved subset from the store without
  refetching; a warm re-survey makes zero thread fetches. The contract
  invariant became "ingest into the ledger is allowed; projection into home
  *notes* waits for approval and occurs only for policy-approved kinds" (PRs
  #50, #51). Why: three real-mailbox
  regressions in a row (reply detection, window, sampling) were all patched
  inside a transient in-memory survey that duplicated ingest.
- **§7.2 resolved: ledger-only kinds are indexed directly.** A `recall_items`
  view (episodes ∪ unprojected raw threads, one item per thread, projected
  threads anti-joined) is declared as a recall SQLite source via the PR #18
  provider, with no recall-engine changes (PRs #49, #51,
  `INTEGRATIONS_RECALL_SQLITE_SOURCE.md`). Raw email and calendar events no
  longer need markdown projection to be retrievable. Workspace setup and
  pull→refresh now generate this SQLite Source configuration automatically.
- **Sampling lesson.** In a >100-threads/day inbox, "most recent N threads"
  spans hours and never reaches anything the owner replied to; correspondence
  evidence must be sampled where the owner wrote (`from:me`).
  **Superseded 2026-08-25 by automatic correspondent entities; retained as the
  original cost recommendation.**
- **Cost policy (recommendation, not yet enforced): embed broadly, catalyze
  narrowly.** Embedding is local and cheap (ESE, the fast local embedding model in
  enzyme-rust — not the catalyst model; a year of email is tens of minutes
  once, then incremental; ~150–300 MB index), so the whole
  ledger should be indexed for retrieval, with backfill as a resumable
  background job by month. Catalysts are LLM-generated and budgeted per
  *entity* across eras on an epoch cadence, so cost scales with entity count,
  not corpus size — the risk is entity explosion and noise. Catalyst entities
  should come only from approved/curated people and accounts; raw email
  contributes evidence to those entities' timelines but never creates
  entities on its own.
- **Cost policy: embed broadly, catalyze automatically within bounded entity
  budgets.** Embedding is local and cheap (ESE, the fast local embedding model in
  enzyme-rust — not the catalyst model; a year of email is tens of minutes
  once, then incremental; ~150–300 MB index), so the whole
  ledger should be indexed for retrieval, with backfill as a resumable
  background job by month. Catalysts are LLM-generated and budgeted per
  *entity* across eras on an epoch cadence, so cost scales with entity count,
  not corpus size — the risk is entity explosion and noise. Correspondent
  people (email-keyed) and non-public domains become entities automatically
  from ledger `who` occurrences. Ranking is lexicographic reply/sent/thread
  evidence, encoded as `1,000,000 * min(user_replied, 999) + 1,000 *
  min(user_sent, 999) + min(thread_count, 999)` and then recency; occurrence
  volume remains available to the same bounded catalyst path used by Markdown
  entities. Existing thread-level
  automated signals and sender/domain heuristics suppress likely automation,
  with the existing real-reply veto. Approval is ingest/projection scope only.

### Gmail quota math (docs checked 2026-08-24)

Google's published Gmail API limits: **6,000 quota units per minute per user
per project** (~100/s) and 80M per day per project. Method costs:
`threads.get` 40, `threads.list` 10, `messages.get` 20, `messages.list` 5,
`history.list` 2. Consequences:

- A thread fetch is the expensive unit: at most ~150 `threads.get` per minute
  per user. The 200-thread survey ceiling is ~8k units — over one minute of
  quota — so the fetcher must back off on 429s rather than assume waves of
  eight are free.
- Full-history backfill of a >100-threads/day inbox (~40k threads/year) is
  ~1.6M units: roughly **4.5 hours of quota-bound time**, 2% of the daily
  project cap. Feasible only as a slow, resumable background job.
- **Curate senders first, then sample deep** is cheap: one `from:me`
  discovery pass (~100–200 thread gets, ≈4–8k units, about a minute), then per
  curated sender one `threads.list` (10) plus K thread gets (40 each). For
  S = 50 senders and K = 10 threads: ~20k units ≈ 3.5 minutes; K = 25: ~50k ≈
  8.5 minutes. That yields most of the relationship value at a few percent of
  the full-backfill cost, and catalyst cost stays S × per-entity budget
  regardless of K.
- After the first ingest, `history.list` (2 units) makes incremental sync
  almost free: fetch only changed threads.

### Storage model as built (verified 2026-08-24)

All durable machine state for one practice is now rooted at:

```text
$MARGINS_HOME/workspaces/<id>/
├── config.toml  # named Sources and Workspace policy
├── index.db     # unified recall index
└── ledger.db    # connector reports, approvals, cursors, raw cache, episodes
```

The machine-global `$MARGINS_HOME/config.toml` retains machine concerns such
as `[llm]` and `[update]`; it does not declare Workspace Sources. Content
folders never receive Margins machine state. A Workspace config resembles:

```toml
id = "customer-practice"

[policy]
excluded_folders = ["templates", "archive"]
excluded_tags = ["generated"]

[sources.home]
kind = "notes"
role = "home"
path = "/Users/example/Notes"

[sources.research]
kind = "notes"
role = "reference"
path = "/Users/example/Research"

[sources.mail]
kind = "google-mail"
account = "person@example.com"
```

Exactly one notes Source has `role = home`; all other Sources are read-only.
`ledger.db` is the connector
source of truth: connectors upsert into `raw_items`/`episodes`, and the store
recreates `recall_items` on open (episodes ∪ unprojected raw threads, one row
per thread). `init` and successful pull refresh the same `index.db`; pull no
longer leaves recall stale.

PR #59 makes refresh durability explicit. Before recall-engine can prune or
rewrite any indexed document, Margins opens and exhausts every configured
SQLite Source query, including the ledger view and the temporary reference
database. A missing column, broken view, or unreadable row therefore aborts
the refresh before index mutation. The previous `index.db` remains searchable
and is reported as stale until a later successful `init`. When a multi-
connector pull has mixed results, Margins refreshes the successful connectors'
ledger writes first and only then reports `integration_failed` with the
per-connector results; the command's failure exit remains unchanged.

The fresh-onboarding harness follows the same phase boundary. `init` and
phase1 establish and survey `config.toml`, but do not imply that a connector
has been approved or pulled. Consequently `verify-isolation` and `report`
require only `config.toml` before phase2. Phase2 persists a sandbox marker on
completion; only after that marker exists must both `ledger.db` and `index.db`
exist.

Recall refresh creates an ephemeral engine home. It binds the declared home
notes Source through `EnzymePaths::bound_workspace_with_home`, automatically
adds `ledger.db` as a SQLite Source, and discards the generated runtime after
refresh. Search, context, cues, and catalyst generation read `index.db`, not
the ledger at query time. Full-snapshot reconciliation remains O(rows); it is
fine at tens of thousands of threads and should be revisited past that.

**Superseded 2026-08-25 by enzyme-rust seams PR #14.** Additional reference
notes Sources used an engine-free include-roots bridge:
Margins runs the engine `FileDiscovery` with the same exclusions, places the
discovered bodies in a temporary SQLite file, indexes it as a named Source,
then deletes the runtime. No Workspace-state file contains a local Markdown
body. The preferred upstream seam is a public array of named filesystem
Sources carrying `{name, root, exclusions, writable}` into discovery metadata;
that would remove the temporary SQLite bridge without changing behavior. The
synced `NotesRootConfig` seam now does exactly that: home and reference Sources
are named Markdown roots with per-root exclusions and writability. Margins no
longer creates a notes-root symlink or copies reference bodies into temporary
SQLite.

### Ledger scope rule (owner direction, 2026-08-24; now enforced)

The ledger is **not** a home for local Markdown. Its scope, per Source kind:

| Source class | ledger registry/state | `raw_items` cache | projected home note |
|---|---|---|---|
| Mail, Calendar | yes | **yes**, quota-bound | **no**, ledger-only through `recall_items` |
| Meet | yes | **yes**, quota-bound transport | yes, `<home>/Margins/Transcripts/` |
| Granola | yes, registry | no | no copy; its native note is the artifact |
| Home/reference notes and folders | yes, reference-only registry as needed | no | no copy; index the original file |
| Captures | session/native registry | no | no integration copy |

Invariant tests require that no Workspace-state file contains a local Markdown
body, ledger-only kinds produce no note, and remote raw cache exists only for
the quota-bound kinds declared in the code policy table.

The ledger deliberately has no migration or backward-compatibility path.
Every change to a `CREATE` statement must bump `SCHEMA_VERSION` and re-pin the
full-schema fingerprint; a Linux test fails if either half is omitted. On
open, the store reads `user_version` before installing the recall view or
running any application query. An incompatible file is refused cleanly with
the exact user-facing message `unsupported ledger schema version N; delete and
recreate the workspace ledger`. Survey, approve, pull, status, init, and recall
all preserve that refusal rather than leaking a later raw SQLite error.

## 8. What real-machine validation established (evidence, not claims)

- **Isolation incident (`$HOME`, 2026-08-24):** a real command run without a
  declared Workspace treated the owner's home directory as content and wrote
  state there. That write is the evidence behind the implicit-home deny-list:
  never `$HOME`, `/`, Margins or Enzyme config/state (including the Workspace
  state root and descendants), system or configured temp paths, a folder with
  no Markdown note evidence, or an ancestor of an existing declared home.
  Safe single-folder cwd creation is intentionally narrower than general cwd
  inference; explicit `workspace new --home` remains the escape hatch.
  Its one creation receipt is stderr-only so JSON and other machine-readable
  command stdout remains valid on the first run.

- Scratch flow (folder connector only, before the Workspace rename): empty
  content folder → survey → approve → pull
  → transition → context packet in **12.4 s**; with a real Gmail survey in the
  loop, **~67 s**. The five-minute goal is met by a wide margin once Google is
  connected.
- Idempotency, tombstones, provenance-opens-source, and the two-Kevins refusal
  all verified on macOS with unmodified builds.
- **Schema/refresh failure evidence (macOS, 2026-08-24):** a ledger created by
  the previous build had schema version 1 but lacked the newly queried
  `account_scope` column. `init` reached the raw SQLite error and the failed
  rebuild wiped a previously usable 404-document recall index to zero
  documents. This incident is the reason schema refusal and SQLite Source
  preflight are fail-before-mutation invariants rather than best effort.
- Scoped email import on the owner's real mailbox: **1 genuine Episode**
  (a real correspondence thread including the owner's reply) and a meaningful
  transition — versus 96 noisy episodes when approval was wholesale.
- Fresh-user Google onboarding through the pre-native helper flow:
  **~40 min human wall-clock**, dominated by Google Cloud console steps — the
  number `margins connect google` must collapse.

- **Seamless onboarding (after `margins connect google`, 2026-08-24):** fresh
  sandbox, official build with the embedded Enzyme-owned client. Human step —
  one command, three Google screens (account chooser, unverified-app
  Advanced → continue, read-only consent) — took **~4m50s** (estimated from
  artifact timestamps; the owner described it as fast) versus **40m43s** for
  the old helper-native flow: 88% shorter. Machine side: 1,538 episodes and 6
  transitions pulled in ~10 s under the then-current universal projection
  policy. That projection count is historical and superseded by the per-kind
  Workspace policy. Isolation
  verified for files *and* the macOS Keychain (no new items), with independent
  out-of-sandbox checks of the real Google account and Keychain. Remaining
  friction: ~40 s Gmail survey (run twice by the harness); many correspondents
  tie on evidence at default survey depth (one reply each), leaving the
  ranking underdetermined; the helper post-consent page still shows (branding
  work in progress).

- **Branded loopback consent (PRs #41/#45, verified 2026-08-24):** `margins
  connect google` now runs Margins' own 127.0.0.1 listener and serves an
  Enzyme-branded, fully self-contained page; the owner confirmed a real Google
  consent landed on Margins' page. Isolation (files, Keychain, real Google
  state) held. The callback leaves no trace by design, so page evidence is
  observational.

- **Ledger-first survey on the real mailbox (2026-08-24, late; pre-#52 main):**
  287 proposals, **102 with replies (35.5%)**, max `user_replied` 92, `from:me`
  sample reaching back to 2024-02 — versus 43/199 before the regressions and
  0/163 after them. Cold survey 102 s with 202 Gmail calls (quota-bound: 200
  thread gets × 40 units exceeds 6,000 units/min); warm re-survey 14 s with 2
  calls and zero thread gets; scoped pull yielded **7 episodes and 3 candidate
  transitions** from the ledger in 0.04 s with no refetch. Recall over the
  ledger finds an unprojected raw thread at
  similarity 1.0 with Gmail provenance and no markdown note; index 9.3 MiB for
  1,729 documents, refresh 1.07 s. Defects found: a public mail domain
  (`domain:gmail.com`) ranked first as if it were an organization; the SQLite
  source block was not auto-provisioned in the workspace config — routed to
  the Workspace lanes. A second isolation incident (a manual command run
  without a declared
  Workspace resolved `$HOME` as the content root despite the PR #38 guard,
  because a home directory can carry note-like evidence) is the concrete
  justification for PR #52 deleting cwd/`$HOME` inference entirely.

### Owner decisions, 2026-08-25: fail closed; portable consent

**Fail closed everywhere.** The direct-search fallback is removed from every
shipped path (`init`, `recall`, cues, prep); the engine's
`direct_search` survives only as a benchmark/test API. A usable generator is a
valid hosted bundle or a configured local model; with neither, commands error
explicitly. Curated entities whose catalysts have not been generated yet
return an explicit non-success ("catalysts pending"), never a silent empty
success, and `pull`/`init` trigger generation so first value does not wait on
a nightly cadence. Ramifications, accepted knowingly:

- Ledger-only content (raw email threads, calendar events in `recall_items`)
  is no longer reachable by raw content search; it reaches the user only as
  evidence inside catalysts of curated entities. "Catalyze narrowly" becomes
  the retrieval boundary, not just the cost boundary.

  **Superseded 2026-08-25 by automatic correspondent entities.** Selected
  entities whose catalysts have not been generated retain the same pending
  contract. Ledger-only evidence now attaches to automatic or explicitly curated entities,
  and "Catalyze
  narrowly" becomes
  the retrieval boundary, not just the cost boundary.
- Offline operation depends on the local model being installed at setup;
  hosted-only machines have no retrieval without the broker.
- Desktop surfaces (live cues, prep hydration) must render an explicit
  "recall unavailable" state; silence is no longer a valid degradation.
- Public copy claiming offline operation must be conditioned on the
  local-model default.
- The CI harness needs recorded fixtures or a tiny local model to keep
  exercising recall without live Google calls.

**Consent reuse.** Google connections are machine-level identity: one consent
per machine + account, declared by workspaces rather than owned by them. The
managed home is `$MARGINS_HOME/google/<normalized-account>/`, not one unkeyed
shared directory, and Workspace `source.account` declarations select it. Normal
desktop storage uses the native OS keyring. Isolated/headless runs must
deliberately select the 0600 file backend with `--headless`; Margins does not silently downgrade storage and
does not provide legacy runner migration or portable import/export compatibility.

`bootstrap.json` does not port by default: doing so deliberately aliases two
machines to one broker identity and lease. `llm-config-cache.json` never ports;
it is expiring hosted state and `margins setup` reprovisions it. Workspace
configuration, ledgers, indexes, raw caches, model downloads, and Keychain
implementation metadata are also outside the connection bundle.

- **Connection reuse + portability on the owner's real account (2026-08-25,
  Mac round 5, build 7e4278cf7):** legacy consent relocated to the machine
  level; a second workspace declaring the same account surveyed over the
  shared connection with no new consent (290 proposals, 101 with replies,
  zero public-domain proposals); encrypted export (3 KB, no plaintext) →
  import into a fresh home connected without a browser; wrong passphrase
  fails cleanly; re-import is a no-op; `--forget` refused while declared;
  Keychain and `~/.margins` untouched. Trade-off surfaced: strict 150
  thread-gets/min pacing makes a cold 200-thread survey 102 s versus 22.8 s
  unpaced with no 429 observed — adaptive pacing (burst first, pace after a
  quota envelope) is recommended and awaits the owner's decision. Hosted
  bundle confirmation on the Mac remains blocked by the broker's per-IP
  daily new-lease cap, exhausted by validation sandboxes minting fresh
  bootstrap ids; validation now reuses one id.

- **Hosted catalyst path end to end on the owner's Mac (2026-08-25, round 6b,
  build 6ef29bf2f):** after the broker's shared admission cap was raised
  (temporary) `setup --only catalyst` provisioned the hosted bundle with no
  user step; `init` reported `catalyst="hosted" reason="hosted_bundle_ready"`
  (9.4 s including generation); recall used `search_strategy=catalyze` with
  attributed catalysts (32 ms); the same query with the broker URL pointed at
  a dead port produced byte-identical output (query path never fetches);
  removing the bundle produced the exact fail-closed line with rc 1; Keychain
  and `~/.margins` unchanged. Server-side follow-ups: issued leases carry
  `expires_at = null` (add expiry so clients rotate on time); 429 responses
  carry no `Retry-After`; the per-IP daily cap at 50 is a temporary raise.

- **Catalyst coherence on real correspondents (2026-08-25, round 7 baseline,
  manual top-3, hosted):** mechanics pass (15 episodes, 14 catalysts in ~6 s
  during pull→refresh), coherence
  fails for identifiable reasons: shared threads attribute to a single `who`,
  so one person got zero catalysts and 5/7 of another's were about third
  parties (per-participant attribution needed); catalysts persist no evidence
  anchors (metadata is entity_type + empty era), so hypotheses are not
  auditable — an engine gap; eras/epochs are not written for pull-triggered
  generation; thin evidence (forwards, newsletters) yields generic
  relationship boilerplate — an evidence-threshold/prompt-guard gap in the
  engine; duplicate `sqlite:mail` refs in results. Negative invariants held
  (no newsletter catalysts, no uncurated entities). Attribution/eras/duplicates
  go to the automatic-entities lane; anchors and the thin-evidence guard go
  upstream to enzyme-rust, after which Margins re-syncs the subtree.

  **Superseded 2026-08-25 by automatic correspondent entities and the corrected
  kernel-path diagnosis below.** The same baseline now
  fails for identifiable reasons. This automatic-entities lane resolves shared
  attribution by mapping every retained participant to a composite `who` role
  on one thread document, and consequently removes duplicate `sqlite:mail`
  refs. It also supersedes manual top-3 selection with weighted automatic
  correspondents while preserving the negative automation/public-domain
  invariants. Catalysts still persist no evidence anchors, so hypotheses are
  not auditable. The era finding is broader than the baseline suggested: the
  kernel workspace-provision path used by both `init` and pull-triggered refresh
  invokes `CatalystPipeline` without writing `catalyst_epochs`, leaving empty
  eras and no per-era allocation rows on either path. Evidence anchors,
  `allocate_catalysts_to_eras`/epoch persistence, and the thin-evidence prompt
  guard remain upstream enzyme-rust dependencies; Margins must not invent a
  parallel allocation path before re-syncing that subtree.

### Engine vs. Margins: where each piece belongs (owner question, 2026-08-25)

Product-side by design (Margins): connectors and the native Google transport, ledger and
survey/approve/pull, Workspace/Source model and config, Google connection
identity and portability, hosted credential provisioning at setup, fail-closed
policy, skills/harness/onboarding, pin/exclude policy.

Engine-side by design (enzyme-rust, vendored as a pristine subtree): document
representation, entity occurrences and eras, catalyst generation and epochs,
evidence anchors and grounding guards (upstream lane in flight).

**Superseded 2026-08-25 by enzyme-rust seams PR #14 and subtree sync #2.**
Engine seams previously bridged in Margins — each was a workaround for a missing
API that makes subtree syncs more brittle, to be moved upstream as a single
"seams" change set after the anchors/guard PR lands:

1. **Multiple notes roots** — Margins binds through an ephemeral symlink and
   materializes reference folders into a throwaway SQLite source; the engine
   should expose a public named multi-root API (name/root/exclusions/writable).
2. **Multi-participant attribution** — thread participants are encoded as
   fixed `who` columns (32 person + 16 domain) because the SQLite source's
   `who` role is scalar per column; upstream, `who` should accept a
   multi-valued (JSON array) column. Generic for any communication source.
3. **Source-provided entity weight** — markdown entities are weighted by
   mention count inside the engine; email evidence (replied/sent) is pre-ranked
   by Margins and frozen into the explicit ephemeral selection after the round-8
   mailbox showed raw link frequency selecting CI noise instead. A `weight`
   role on the source contract could eventually let the engine own ranking
   uniformly without surrendering Margins' connector-specific noise/self gate.
4. **Per-source refresh watermarks** — implemented Margins-side (#56) beside
   the engine's own fingerprint reconciliation; belongs next to it upstream.

The synchronized engine now owns all four seams. Margins declares named
`NotesRootConfig` roots directly; supplies one JSON-array `who` column and a
numeric `roles.weight` for ledger rows; and asks `source_refresh_staleness` for
the engine-owned watermark result (`last_refresh_ms`, `stale`, and
`stale_reason`). The fixed 32-person/16-domain cap, `cap-truncated` state,
reference-body materialization, symlinked virtual home, and parallel file-vs-
watermark comparison were deleted. Explicit entity curation/exclusions and the connector's
noise-filtered automatic entity list remain the authoritative product policy; additive
occurrence weights now drive engine ranking and era allocation within it.

- **Round 8 (automatic entities, PR #65, 2026-08-25): who selects the
  entities matters more than how they are weighted.** Mechanics passed
  (approval scopes ingest only; pull without refetch; hosted init), but
  coherence was worse than the manual baseline: the
  engine chose link entities by raw occurrence frequency, so newsletters,
  Stripe, GitHub CI (51 threads, 0 replies, already flagged automated) and the
  owner's own address received catalysts while the highest-weighted real
  correspondents received none, and noise catalysts contaminated retrieval.
  Margins had computed the right weights and noise flags but left selection
  to the engine and did not pass exclusions. Rule adopted: **Margins owns
  entity selection** — top-N by weight after noise suppression when no explicit
  curation exists, minus excludes and self — and pushes noise/self into the engine's
  exclusions, with a debug line listing selected entities/weights and a
  fixture asserting the exact list. Shared-thread attribution remains
  partly an engine behaviour (per-entity generation from shared documents).

- **Round 9 (2026-08-25, #66): a selection that can silently be empty is not
  a selection.** On the real workspace the debug line read `selected=[]`:
  the correspondent table was empty over 209 raw rows ingested by earlier
  builds, so Margins wrote no explicit entities and the engine fell back to
  its coverage selection, regenerating the same noise. The fixture passed
  only because it ingests fresh rows — a test that cannot see a ledger
  written by a previous build. Rules adopted: derive correspondents from
  data every retained row carries (`payload_json` headers), test against a
  ledger produced by the previous build, treat an empty selection as a hard
  error, and write exclusions from sources independent of the selection
  (survey automation flags, the public-domain rule, connected identities) so
  noise can never be resurrected by a fallback path.

- **Recurring bug class — Gmail address canonicalization at the wrong layer
  (2026-08-25, second occurrence):** PR #46 fixed reply detection by
  canonicalizing addresses (dots, case, plus-tags) for *comparison*; PR #66
  then canonicalized the account *before an exact `raw_items.source_account`
  lookup* and silently missed every stored row. Rule: ledger keys are stored
  and looked up raw; canonical forms exist only for identity comparison and
  aggregation. A selection that finds zero correspondent evidence must fail
  closed with an explicit sentinel rather than fall through to an engine
  default.

- **Round-9 implementation resolution (2026-08-25):** selection now keeps the
  exact connected-account spelling for ledger lookup and derives participant,
  reply, recency, and noise evidence from retained message headers; optional
  catalyst helper fields are not required. Cached survey automation flags,
  public-mail rules, and connected-account spellings independently feed
  `excluded_links`. A valid all-noise empty policy carries an internal
  non-resolving sentinel so coverage cannot activate; retained rows with zero
  decoded correspondent evidence fail before generation. The debug line
  reports raw-item, decoded-thread, and considered-correspondent counts.

- **Round 10 (2026-08-25, #67): entity selection fixed on the real
  mailbox.** 209/209 legacy threads decoded; 30/30 entities selected with the
  intended weights; 46 suppressed; zero noise, self, or public-domain
  entities; every survey top-10 correspondent selected and 8 of 10 given
  fresh catalysts (17 entities × 7 catalysts, 17 hosted calls, generated
  during pull); an exact-subject recall for one person returned 8/8 results
  backed by that person's catalysts. Residual: 13 of 30 selected entities
  materialized no catalysts because the `recall_items` *episode* branch
  exposes a single `who`, so participants on threads that approval
  projected lose their occurrences (one person sat entirely on projected
  shared threads) — Margins-side, with a fixture requirement. Engine-side
  items (anchors, eras, shared-document generation, thin-evidence
  boilerplate) await the enzyme-rust PR. Operational note: an isolated
  Cargo target on the Mac cost ~12 minutes of llama.cpp CMake per round.

- **Round-10 implementation resolution (2026-08-25):** selection and
  occurrence projection are separate correctness boundaries. The Source now
  preserves one retrieval document per thread while filling its fixed `who`
  columns from the episode projection's complete `people` array and participant
  domains when legacy raw helper arrays are absent. Provisioning reports the
  selected/materialized boundary and whether each missing catalyst lacked an
  occurrence or failed generation. A fixture projects the three-person shared
  thread, removes its new helper fields to reproduce the retained legacy shape,
  and requires DK and the domain to materialize.

- **Round-10 upstream residual:** per-entity generation over a shared document
  can still produce hypotheses chiefly about the other attendees. Exact-subject
  queries likewise admit catalysts for other people and organizations on that
  thread. Margins can preserve attribution and selection, but entity-focused
  evidence slicing/prompting inside shared documents belongs to the engine
  catalyst lane.

- **Round 11 (2026-08-25, #68): the Margins-side entity pipeline is
  validated on the real mailbox.** **Narrowly superseded later on 2026-08-25:
  the ten `no-occurrence` values exposed one remaining raw-only legacy
  compatibility gap, resolved below.** 30/30 entities selected with intended
  weights; 20/30 materialized with the 10 misses explicitly reported as
  no-occurrence; every survey top-10 correspondent has catalysts (including
  the person who sat only on projected shared threads); zero noise, self, or
  public-domain entities; 20 hosted calls for 20 materialized entities,
  generation during pull and idempotent init. What remains has the same shape
  in every round
  since 7 and is untouched by Margins fixes, i.e. engine-side: per-entity
  generation over shared documents yields hypotheses about the *other*
  participants; retrieval routes a person query through other entities'
  catalysts; no evidence anchors; empty eras/epochs; relational boilerplate
  on thin evidence. These are the asks on the upstream enzyme-rust PR
  (participant-scoped generation with the entity as subject; identity-first
  retrieval routing; anchors; provision-path epochs; evidence threshold and
  grounding guard). No further Margins-side coherence lane is warranted.

- **Round-11 raw-only compatibility resolution (2026-08-25):** the ten missing
  refs were outside every approved projection domain. Selection reconstructed
  them from pre-#65 headers, but their unprojected rows had only scalar
  `catalyst_who`/`catalyst_account`, so non-leading co-participants never became
  occurrences. Provisioning now derives bounded per-thread arrays ephemerally
  with the same Rust parser and noise/self rules, embeds them in the temporary
  Source query, and leaves the ledger untouched. No refetch or migration is
  required. A raw-only pre-#65 fixture selects and materializes all 30 refs,
  including three people and three domains on one shared document. Values past
  the existing 32-person/16-domain bounds are separately diagnosed as
  `cap-truncated`; selection is not silently reordered around the cap.

### Text is canonical; tables are index (owner principle, 2026-08-25)

The ledger/markdown split ("SQLite is machine state, markdown is the
inspectable truth") generalizes to every content kind the engine holds,
including catalysts. A catalyst is a self-contained text chunk: a small
stable header (entity, era, generation run/epoch, generated_at), the
hypothesis prose, and a structured **receipts block** listing its evidence
anchors (source ref, occurrence ids, timestamp, chunk index, short quote). An
agent or a person can audit the chunk with nothing but the chunk. The engine
parses header and receipts into anchor/era rows for machine operations
(pruning on tombstoned sources, occurrence joins, era filtering, budgets,
identity-first routing, the grounding guard) — those rows are derived and
must be regenerable from text (drop-and-rebuild yields identical rows).
Embedding covers the hypothesis prose only, so receipts do not distort
similarity; results return the full chunk. Why: schema evolution is a poor
fit for content that must stay flexible across future source kinds, whereas
provenance that travels inside the chunk is uniform, portable, and readable
by the agents that consume it. Applied to enzyme-rust PR #13 before merge;
the same rule governs how new sources should carry provenance text.

- **Engine PR #13 final shape (2026-08-25, sync `a5a4df2e`):** catalyst chunk =
  versioned JSON header (entity, era, run, epoch, timestamp, participant
  scope, era allocations) + hypothesis prose + fenced JSON-lines receipts;
  `catalyst_receipts`/`catalyst_eras` are derived indexes with a tested
  drop-and-rebuild; embeddings cover prose only; search returns the full
  chunk; provision/init/pull write epoch, era, receipts; thin-evidence
  threshold 4 documents / 128 substantive tokens; participant-scoped
  generation and identity-first routing. Speaker spans are a versioned text
  contract, `enzyme-speaker-span.v1`: each message is rendered as
  `## YYYY-MM-DD HH:MM:SS UTC — email1, email2` (column-zero `## `, U+2014 em
  dash, `, ` separator), identity tokens equal the link-entity key
  (ASCII case-insensitive, no alias resolution — Margins renders stable
  emails), every listed participant owns the span, non-matching spans are
  excluded, and the previous `### <date> — <sender>` form fails closed rather
  than exposing the whole thread. Margins renders to this contract before
  round 12.

### Engine seams landed upstream (enzyme-rust PR #14, 2026-08-25, sync `60c53fe1`)

The four seams plus a fifth surfaced by round 12 are now engine APIs, all
additive: `NotesRootConfig` (name/path/exclusions/writable) with
`FileDiscovery::from_notes_roots`; `SqliteWhoRole` accepting scalar, list,
JSON-array, or delimited columns; an optional `weight` role folded into
occurrence weight (`entity_occurrences.weight`, default 1.0);
`source_refresh_staleness` exposed per source; and link grounding that scans
every chunk of a document for `enzyme-speaker-span.v1` spans, so a
participant who appears only on later messages is no longer skipped as
no-context. Consequence for Margins (next subtree sync): delete the ephemeral
notes-root symlink and reference-folder materialization, the fixed 32+16
`who` columns, client-side weight/era-budget normalization, the parallel
watermark table from #56, and any first-message-only participant workaround.
Vendoring becomes plain sync.

- **Per-kind grounding floors (PR #72, 2026-08-25):** round 12 showed the
  engine's note-calibrated floor (4 documents / 128 substantive tokens per
  entity-era) suppressing all 30 real correspondents once evidence was
  scoped to each person's own spans — short replies are the norm in email.
  Floors are now a Margins policy per source kind, enforced unchanged by the
  engine: mail 2 documents / 48 own-span tokens, notes 4/128, overridable in
  the workspace `[policy]` and printed in the selection debug line. The
  grounding guard (every hypothesis cites a receipt inside the entity's own
  spans) stays fully enforced so lower floors cannot reopen boilerplate;
  fixtures cover a grounded cc-only 2-document entity and a 1-document skip
  with its reason; skip reasons are visible in status.
**Implemented by subtree sync #2.** Those bridges are now deleted. Mail rows
carry the complete participant/domain set in one JSON array and a
reply/sent/thread-derived additive weight. The every-chunk speaker-span scan is
used without a Margins occurrence-position workaround, including participants
who first appear in later messages.

### Consolidation record (2026-08-25)

**Preserve as built:** Workspace/Source model and state layout, implicit
workspace under a deny-list with isolation tests; ledger schema discipline;
fail-closed catalyst-only recall with generator-free evidence reads; hosted
credential bundle provisioned at setup with Margins-worded broker failures;
machine-level Google connections with a portable encrypted bundle;
automatic correspondent entities with an exact explicit curated-entity override and exclusions as
the override contract; `enzyme-speaker-span.v1` rendering; per-kind grounding
floors in `[policy]`; the local gate chain with design-doc and boundary
tests; a *minimal* email evidence adapter (source, provenance URL,
thread/message ids, timestamp/order, participants, message boundary,
body_text, JSON-array who, optional weight).

**Collapse / never resurrect:** every compensatory bridge deleted by the
seams sync (symlink binding, reference-folder materialization, fixed 32+16
`who` columns, client-side ranking/era normalization, the parallel watermark
table, first-slot participant fallbacks); manual people-approval for entity
existence (approval is ingest scope only); any "richer mail projection"
direction; the two-pass floor provisioning once the engine accepts
per-source floors.

**Hand to a top-level redesign (owner-opened, not agent-spawned):** whether
the email evidence adapter becomes a first-class engine source contract
(spans already are); per-source floors in one provisioning pass; verifying
shared-document generation quality and identity-first routing now that they
are engine-owned; the retention/deletion story for ledger-only content under
fail-closed; adaptive vs strict Gmail pacing; broker `expires_at` and the
temporary per-IP cap.

- **Binary provenance is a gate (lesson from round 13, 2026-08-25).** A Mac
  validation run reported a ledger refusal with wording that exists nowhere
  on the commit under test: the worker had run a stale binary. The
  continuation was retracted before any quota was spent. Rule: the CLI
  exposes its exact build commit in status/capabilities output; the harness
  records the binary's provenance at init and every phase asserts it equals
  the expected commit before any assertion runs; validation briefs state the
  expected commit and reports quote the provenance line. Findings from a
  binary whose provenance does not match are not evidence.

- **Round 13 (2026-08-25, exact `024fcc7c3`, binary provenance verified):
  the engine change set holds on the real mailbox.** All 59 catalyst chunks
  carry the versioned header and receipts fence; 137 parsed receipts equal
  137 derived rows with zero ownership or source-ref mismatches; eras are
  non-empty (18 across 2 epochs); Bob's hypotheses are 4/4 about Bob (1/7 in
  round 11, 0/0 in round 12) because every-chunk span scanning now credits
  his cc-only messages; DK 1/1; Steven 5/7 specific with no hypothesis about
  another participant; 17 of 30 selected entities materialized, the other 13
  correctly skipped as thin evidence at one document; zero noise entities;
  Keychain and real home untouched. Remaining questions are design, not
  defects: identity-first routing keys on identity-resolving queries and not
  on subject phrases (explicit `--person` scope or query-time identity
  resolution); a single long document (e.g. 1 doc / 513 tokens) is skipped
  by the 2-document floor — a token-only path may be warranted; receipt
  excerpt formatting sometimes omits the leading heading; the grounding
  guard tolerates question-form relational leaps. An earlier round-13
  attempt ran a binary built from the wrong commit; provenance is now a gate.

### Design disposition after round 13 (design thread, 2026-08-25)

Round 13 is accepted as the baseline for the *current* engine path. A clean
rewrite runs under a separate orchestrator (`thr_zc2dnzmrak`) that removes
participant-span ownership and mail-specific two-pass floors in favour of a
participant-to-thread sampling model; findings from this program are not
routed there unless the owner chooses. Dispositions: explicit person scope
is the authoritative retrieval operation and query-time identity resolution
may only be a visible, high-confidence convenience (never silently inferred
from subject phrases) — rewrite-era work, not added now; staleness fields
(`last_refresh_ms`, `stale`, `stale_reason`) may be added to workspace
status JSON; receipt-heading formatting stays engine-side with no Margins
workaround; question-form relational leaps are prompt/eval/taste work; no
token-only exception to the 2-document mail floor. After the provenance
hygiene PR, the current-path program is complete.

## 9. Open questions for a reviewer

- Should calendar events be ledger-only? **Resolved yes** in the Workspace
  policy; retained here because it was the question that forced selective
  projection. See §7.2.
- Is the survey cache worth its invariant risk, or should survey be made cheap?
- Where does the account object live — as a generalization of the relationship
  trajectory in the product store, or as a new ledger table? The stage-two
  account model (people/roles, dated episodes, commitments, decisions,
  tensions, next event) is unbuilt; the ledger's `episodes` table is its
  natural evidence source but should not become it.
- What is the retention/deletion story a client-confidentiality-sensitive
  principal will ask for? Today: projected human artifacts and ledger
  tombstones are distinct, while quota-bound `raw_items` persists. That is
  probably not the complete answer they want.
- When the Margins-owned OAuth client ships, does the per-connector "account"
  concept collapse to one Google connection with service toggles? Probably yes;
  the three Google connectors currently carry separate approval plans.

## 10. Resolved 2026-08-24: Workspace Google ownership and narrow catalysts (superseded in ownership only)

`margins connect google` now resolves the selected Workspace first and owns all
transport state at `$MARGINS_HOME/workspaces/<id>/google/`. A successful consent
declares the fixed `google-mail`, `google-calendar`, and `google-meet` Sources
for that account. Status is per Workspace. Disconnect removes those Source
declarations and the Workspace-owned authorization, but deliberately retains
ledger connector/raw/Episode rows: they remain reconciliation evidence that can
be tombstoned, while removal from the declared Source set excludes them from the
next recall refresh. This supersedes the earlier `$MARGINS_HOME/google` path.

Superseded 2026-08-25: authorization is machine-level per account at
`$MARGINS_HOME/google/<account>/`. Consent no longer resolves or mutates a
Workspace: `source add` separately owns every account declaration. The Source
declarations and ledger behavior above remain Workspace-owned. Ordinary
disconnect resolves only an explicitly selected or already cwd-bound Workspace
and detaches its declarations; explicit `--forget` owns credential deletion and
refuses to break declaring Workspaces without force.

The measured 200-thread cold survey is now enforced as bounded waves: eight
concurrent `threads.get` calls followed by a 3.2-second wave interval, exactly
150 gets/minute at 40 units each. Quota/HTTP 429 envelopes receive at most five
exponential retries (1–32 seconds plus bounded deterministic jitter). The first
ledger ingest stores the greatest returned Gmail `historyId`; subsequent surveys
use Gmail history from the native driver, fetch only changed threads, and fall
back to the bounded dual search only for an explicitly stale checkpoint.

Public mailbox providers are person identity evidence, never account identity:
Gmail/Googlemail, Outlook/Hotmail/Live, Yahoo country domains, iCloud/Me/Mac,
Proton, AOL, and the other enumerated consumer providers cannot produce a
`domain:` proposal or inferred organization. A person address remains eligible.

Catalyst selection is frozen by the Workspace wrapper rather than left to
coverage selection. The generated ephemeral recall config contains only
approved email people/account scopes plus people notes and recorded-session
attendees. All ledger rows remain embedded by ESE. Structured SQLite `who`
roles attach raw evidence to curated entities' timelines, but a sender seen
only in raw mail is absent from catalyst selection.

**Superseded 2026-08-25 by automatic correspondent entities; approval is scope,
not entity existence.** Selection is now protected from
unbounded coverage selection. With no explicit entity curation, the generated
ephemeral recall config contains automatic noise-filtered correspondents and
non-public domains, people notes, and recorded-session attendees. A non-empty
`[policy].entities` replaces that automatic selection exactly. Both paths honor
`[policy].excluded_entities`. Approval is not consulted. All ledger rows remain
embedded by ESE. One thread stays one retrieval document while composite
SQLite `who` columns attach every retained from/to/cc/bcc correspondent to that
document; shared threads therefore contribute an occurrence to each person
without duplicate result refs. Noise-only identities do not receive catalysts,
but their text remains evidence on a real participant's timeline when shared.

## 11. Resolved 2026-08-24: setup-owned hosted catalyst credentials

Hosted catalyst generation is provisioned before recall indexing, never while a
query is running. `margins setup` and a successful `margins connect google`
ensure one machine identity, fetch the broker once, publish the complete
credential bundle, and pin `[llm].mode = "hosted"`. Running `margins setup`
again is the explicit credential refresh path. A failed broker setup pins
`mode = "local"`; this is a deliberate offline fallback and prevents `auto`
from discovering a provider during later indexing or lookup.

The hosted setup copy is a Margins product boundary. User-facing setup,
connect, init, recall, and status output names only hosted or local catalysts
and actionable Margins commands. It never exposes the broker hostname,
provider brands, credential-environment switches, engine login instructions,
or connector transport names. Broker response details remain available only
when `MARGINS_RECALL_DEBUG` is explicitly enabled. Setup classifies anonymous
bootstrap refusal, network failure, server failure, malformed responses, and
rate limiting into stable one-line Margins reasons. A rate limit receives at
most two retries, honors a numeric `Retry-After` for no more than 60 seconds
with bounded jitter, and then selects the local catalyst fallback.

`margins setup` is an ordered, best-effort step runner rather than a download
transaction. It attempts (1) hosted catalyst identity and credential bundle,
(2) embedded agent skills, (3) local speech assets, and then (4) the local
catalyst model when the selected policy requires it. Under the default policy,
the fourth step runs only when hosted provisioning failed or was offline. Every
selected step emits one `ok` or `failed` status with a one-line reason; a
failure never skips a later selected step. Exit status is decided only after
all selected work and the final read-only catalyst mode/reason report.
This preserves hosted credentials even when native speech preparation fails.

`--only catalyst|skills|speech` is repeatable, and `--skip speech` provides a
credential/agent lane that does not touch speech assets. The local-model policy
is explicit: `--local-model fallback` is the default and installs the local
model only after hosted provisioning fails; `--local-model always` additionally
installs it after hosted success. Therefore a default hosted-success machine
currently has no offline catalyst retrieval if hosted access later disappears.
`--only catalyst` under the default policy does not touch any model path when
hosted provisioning succeeds; when it fails, the owner rule applies and setup
chooses/provisions local catalyst generation. Plain `margins setup` selects
every area.

**Decided (owner, 2026-08-25): the setup default is hosted free-config, not the
local model.** `--local-model fallback` remains the default and the local model
is installed only when hosted provisioning fails; `--local-model always` is an
explicit opt-in and will not become the default. Consequences the copy must
keep stating: offline catalyst retrieval exists only on machines where the
local fallback was actually selected (or `always` was chosen), and any public
"works offline" claim is conditioned on that selection rather than promised
for every install. This closes the open question raised in the 2026-08-25
setup-steps work.

Step status and process status answer different questions. Hosted, skills, or
speech failures remain visible on their step lines, but setup exits zero when
the final classifier reports a usable hosted generator or a successfully
prepared local fallback. It exits non-zero only when no usable generator
remains. The agent handoff is output separately from step failures. It presents
the single-folder path as `cd` into notes followed by `margins init`, while
naming `--workspace` / `MARGINS_WORKSPACE` for the multi-source path. An unbound
cwd is accepted only through the implicit-home deny-list rather than treated as
a general content root.

The machine identity is `$MARGINS_HOME/bootstrap.json` with exactly
`{"bootstrap_id": <uuid>}`. The desktop first adopts an existing file identity;
otherwise it mirrors the `margins.included-openrouter` / `bootstrap-id`
Keychain value into that file. Standalone CLI setup sends that identity to the
shared `/llm/free-config` route as `X-Enzyme-Bootstrap-Id`; desktop Included
final-note preparation separately retains `/margins/free-config` with
`X-Margins-Bootstrap-Id`. The CLI never writes Keychain.

`$MARGINS_HOME/llm-config-cache.json` is mode 0600 and contains exactly the
recall-engine cache fields: `api_key`, `base_url`, `model`, nullable
`expires_at`, `cached_at`, and `profile`. The broker provisions the first four;
setup records `cached_at`; and Margins derives `profile` as
`enzyme-llm-v1:bootstrap:<sha256(bootstrap_id)>`. Desktop Included setup writes
the same shape from its already-acquired lease and pins hosted mode. A rejected
provider key invalidates the matching Keychain lease and file bundle; a later
setup/Included preparation reprovisions it rather than recall lookup doing so.

Desktop recall provisioning may inject an already-cached Included lease as
`OPENROUTER_API_KEY` for the duration of the write-locked index build. Existing
`OPENAI_API_KEY` or `OPENROUTER_API_KEY` values win and are never overwritten.
The recall search path only opens `index.db`; it does not read bootstrap
identity, mint/refresh leases, or contact the free-config broker.

> **Superseded 2026-08-25:** product provisioning no longer injects or honors
> environment keys. A hosted generator is usable only from a valid,
> non-expired setup bundle with `[llm] mode = "hosted"`; local mode is usable
> only when its configured model is installed.

## 12. Catalyst mode is an observable setup result (2026-08-24)

Machines that have not rerun setup can still carry `[llm] mode = "auto"`.
Because query and generation paths are forbidden from discovering or minting
credentials, `auto` now means no selected catalyst until setup makes a deliberate
hosted or local choice. That state must not be hidden behind debug logging.

The portable, read-only catalyst classifier reports only `hosted`, `local`, or
`none` plus a stable reason code. It never returns credential fields, contacts
the broker, or discovers model files. `margins init` publishes that selection as
`catalyst` and `reason` XML attributes and prints one `run margins setup` hint
when the selection is `none`. Workspace status publishes the same pair as a
`catalyst` JSON object. The fresh-onboarding scorecard records the final pair so
sandbox evidence explains whether catalyst generation was actually selected.

> **Superseded 2026-08-25:** `init` no longer publishes success with
> `catalyst=none`. It fails with the setup hint. Workspace status retains the
> machine selection and adds catalyst inventory/readiness counts.

## 13. Ranked search fails closed (2026-08-25)

"Direct search" means ranked retrieval that adds raw literal or embedding hits
to catalyst results, generator-free `direct_no_generator` /
`direct_after_generator_error` modes, or provisioning retry without a
generator. Those paths are removed from the shipped `init`, `recall`, live-cue,
and prep flows. The engine API remains only for its benchmarks and tests.

A usable generator is exactly one of: a valid, non-expired hosted bundle with
hosted mode selected, or local mode with the configured local model installed.
Otherwise ranked product recall reports: `Recall unavailable: no usable
generator is configured. Run margins setup.` Live cues and prep carry an
explicit unavailable reason plus the `run margins setup` hint.

Ledger-only mail/calendar content is not raw-searchable. It can reach ranked
recall only as evidence attached to catalysts for curated entities. Offline
ranked recall therefore depends on the local model being installed. `init` and
successful pulls run bounded, resumable per-entity catalyst generation; an
incomplete inventory reports `status=catalysts_pending` with curated-entity,
catalyst, and pending-item counts until generation completes.

**Superseded 2026-08-25 by automatic correspondent entities.** In this paragraph,
"curated entities" now means selected automatic or explicitly configured entities,
minus explicit exclusions; the fail-closed and offline contracts are unchanged.

## 14. Enzyme #13 uses text-canonical catalysts (2026-08-25)

The vendored recall engine is synced to enzyme-rust PR #13. A catalyst's stored
text is now the canonical record: a versioned JSON header, hypothesis prose, and
a fenced JSON-lines receipts block. `margins recall` passes the complete stored
chunk through as each result's additive `via_catalyst_text` field and as each
top contributor's `text`. The tree prints that same chunk without rebuilding or
reflowing its receipt lines. Margins deliberately omits the engine's struct-level
anchor projection; provenance remains inline in the canonical text.

The `catalyst_receipts` and `catalyst_eras` tables are derived, rebuildable
indexes, not independent sources of truth. Generation also fails closed on thin
evidence: each eligible entity/era currently needs at least four distinct
documents and 128 substantive tokens. The engine records a below-threshold
entity as skipped/suppressed and creates no catalyst. Margins' coarser inventory
surface can consequently report `catalysts_pending` when a curated small
workspace has no materialized catalyst; this does not authorize lowering the
guard. Tests that assert catalysts must supply representative eligible evidence.

Retrieval is identity-first: a query that resolves to a person or organization
prioritizes that identity's catalysts before global catalyst fallback. Margins
continues to provide the explicit entity selection and ledger `who` occurrence
columns used by this routing and participant scoping.

**Speaker-span convention filled 2026-08-25.** Contract
`enzyme-speaker-span.v1` renders every mail message as
`## YYYY-MM-DD HH:MM:SS UTC — participant@example.com, other@example.com`, at
column zero, followed by a blank line and the message body. Margins uses
normalized stable email link keys from `from`, `to`, and `cc`, separated by
exactly comma-space; every listed key owns the span. Matching is ASCII
case-insensitive only. Nonmatching spans are excluded and an occurrence with no
matching span is omitted. The former `### <date> — <sender>` representation now
fails closed. Legacy ledger rows receive this rendering through ephemeral Source
enrichment and are not rewritten.

## 15. Grounding floors belong to source semantics (2026-08-25)

Round-12 hosted evidence separated retrieval quality from generation
eligibility: correspondent selection was correct 30/30, yet the global notes
floor suppressed 30/30 generations (`no_context` 16, fewer than four documents
13, fewer than 128 tokens one). Email is a collection of narrow, shared spans;
requiring four whole note-like documents over-penalizes valid own-span evidence.
Workspace policy therefore defaults mail to 2 documents/48 substantive tokens
and notes to 4/128, with positive per-kind overrides. The grounding guard still
requires every hypothesis to cite a receipt inside the entity's own spans.

The current engine accepts only scalar floors, so one Margins refresh is two
curated, pass-scoped epochs against the same index. Markdown runs first at
4/128; automatic mail links run second at 2/48. Both epochs finish coherently,
catalysts survive the other pass's authoritative selection, and the derived
receipt/era indexes rebuild once at the end. An engine-side per-entity floor map
would collapse this to one epoch and is the explicit upstream ask.

Operational truth comes from `catalyst_generation_skips`: status reports latest
reason-code counts and correspondent `non_materialized` labels use those codes
before Margins-local classifications. This avoids the round-12 contradiction
where an entity appeared cap-truncated despite a recorded `thin_evidence` skip.

The Bob `no_context` cases were an upstream occurrence-scoping gap, not an
identity mismatch. His exact link key appeared in valid `enzyme-speaker-span.v1`
headings, but the SQLite source assigned every participant the same first-chunk
occurrence context. Margins has no role field for per-participant chunk indexes
or occurrence IDs. Expanding ownership in Margins would be unsafe, so this PR
does not add a first-message or occurrence-position workaround. Enzyme-rust
seams PR #14 resolves the gap by searching every document chunk for speaker
spans; that behavior arrives with subtree sync #2. A CC-only regression keeps
the Margins renderer and lowered-floor grounding boundary covered.
