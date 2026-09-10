# Expected diagnostic

## What Margins should notice
- This is a repo-shaped workspace driven by RFCs and decision records.
- `rfc/` files are the strongest signal (`0001-api-contract.md`, `0002-retry-policy.md`, `ADR-0002-api-limits.md`), with owner/status metadata.
- `decisions/DEC-026-security-thread.md` is an explicit decision artifact tied to the RFCs.
- `notes/meeting-sync-jan.md` and `notes/raw-thread.txt` provide ad-hoc meeting snippets but are not in a dedicated meeting folder.
- There are explicit cross-references between notes, RFCs, and decisions (`DEC-026`, `ADR-0002`).
- Some documents mix dated and non-dated conventions; `2026` appears in titles and metadata, not everywhere.

## What uncertainty to name
- RFC/ADR files and decision records are strong process docs, not necessarily a recurring meeting capture workflow.
- Meeting-like content exists, but conventions are incomplete and split across `notes/`, `rfc/`, and `decisions/`.

## Safest default to recommend
Add new meeting notes to a separate, reversible folder near docs: `docs/meeting-notes`.

- Suggested first capture path: `docs/meeting-notes/YYYY-MM-DD decision-sync.md`
- Keep existing RFC and decision records untouched; do not reorganize these artifacts.

## What to avoid
- do not convert `DEC-*` or `rfc/*` files into meeting notes
- do not move or rename historical RFC/decision files
- do not assume all existing participants are recurring meeting participants

## Suggested language
- “You can start now. I’ll keep new notes in `docs/meeting-notes` and won’t touch your RFC or decision records.”
- “Importing older meeting fragments is optional before you begin your first new note.”

