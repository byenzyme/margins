# Expected diagnostic

## Import history (migration evidence)
- Treat this as an imported Google Drive export rather than a curated vault:
  - `google-drive-export.log` shows an export run and explicit warnings, including duplicate filename handling and malformed-heading conversion issues.
  - `import/drive-manifest.json` explicitly records `duplicate_title: client-kickoff` and `empty_file: Untitled document.md`, confirming export churn.
  - `workspace/cache-state.json` is partial and reports parse errors for both a roadmap file and an older text dump.
- Duplicate exports are present for the same call:
  - `MyDrive-Shared/Meetings/client-kickoff-2026-06-21.md`
  - `MyDrive-Shared/Meetings/client-kickoff-2026-06-21 copy.md`
- Half-converted Google Docs leftovers appear in `MyDrive-Shared/Projects/Atlas/Untitled document.md`.
- Meeting materials are distributed across export folders and top-level notes, with mixed polish levels:
  - polished note in `notes/`
  - raw/pasted notes in `MyDrive-Shared/...`

## Starting capture now
- Safe default: create new meeting notes in `notes/meeting-notes/`.
- Suggested filename pattern: `YYYY-MM-DD meeting-title.md`.
- Keep old exports untouched; only offer any de-dup/import cleanup as an optional one-time next step.
- If a one-time cleanup is desired later, run import normalization after the first capture.

## Explicitly avoid
- Do not move or rename existing files.
- Do not create people/project folders from inferred participants.
- Do not assume `MyDrive-Shared/Projects` is a canonical meeting log.

## Tone
- Use action-first language: “You can start now. Use `notes/meeting-notes/` for the next capture.”
- Keep uncertainty short and evidence-based.
