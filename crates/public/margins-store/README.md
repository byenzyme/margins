# margins-store

`margins-store` owns Margins' portable SQLite persistence and storage-only
session index queries. It depends only on `margins-core` within the repository;
it has no desktop, capture, Tauri, CPAL, or platform dependency.

The `legacy` module preserves the existing `sessions.sqlite` tables, JSON
metadata import, path-based functions, tombstones, artifact registry,
grounding, and vault-note registry. The root `margins::session` module is a thin
re-export of that API, so existing CLI and desktop callers continue to read and
write the same database.

`SqliteSessionRepository` implements `margins_core::SessionRepository` with
durable optimistic revisions. The original tables are not renamed, removed, or
rewritten. Three additive, backwards-compatible sidecar tables retain repository
revision state, the rich segment contract that the legacy schema cannot
represent, and stable artifact IDs. Older Margins builds ignore those tables
and continue to use the original rows.

Legacy segments created before the sidecar contract existed do not contain
audio format, frame count, drop diagnostics, a stable segment ID, or timeline
qualification. The repository returns `corrupt_data` for a full aggregate read
of such a session instead of inventing those values. Those sessions remain
fully available through `legacy` and `list_session_index`. A future media-aware
backfill may populate contract metadata only after it can prove those fields
from source artifacts without changing baseline behavior.

The richer desktop note index remains in the transitional root crate because
it scans Markdown/frontmatter, reconciles file links, and mutates stale note
registrations. Moving that workflow into storage would violate the public
store's dependency direction. `list_session_index` is the portable,
storage-only query intended for a standalone CLI.
