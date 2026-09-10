# Expected diagnostic

This workspace looks like an interrupted migration: both `.enzyme` and `.margins`
artifacts are present, plus manually edited notes and one stale transcript import.
Behavior should be conservative and additive.

## Import history (stale migration state)
- `.enzyme/state.json` is old (`1.2.0-beta`) and points to a stale `active_file` path (`notes/meetin-notes/...`).
- `.enzyme/cache-state.json` is stale with unresolved file mentions in `unparsed_files`.
- `.margins/cache-state.json` is interrupted with `run_id` `margins-2026-06-02` and a path mismatch error.
- `.margins/import-state.json` is `partially-applied`, with queued and already applied notes.
- `.margins/config.json` capture destination differs from `.margins/cache-state.json` default destination and `.enzyme` root.
- No DB hint or DB migration marker is present in either cache payload; state looks interrupted rather than authoritative.

## Starting capture now
- Continue in `notes/meeting-notes/` for new captures.
- Use `YYYY-MM-DD meeting-title.md` and keep the process additive.
- Ask about import cleanup only after at least one successful new capture.

## Explicitly avoid
- Do not auto-repair or rewrite `.enzyme`/`.margins` metadata.
- Do not move existing notes to satisfy a folder convention.
- Do not claim a definitive canonical convention yet.

## Tone
- clear and permissive: “You can start now. New notes should go in `notes/meeting-notes/` while you keep legacy state untouched.”
