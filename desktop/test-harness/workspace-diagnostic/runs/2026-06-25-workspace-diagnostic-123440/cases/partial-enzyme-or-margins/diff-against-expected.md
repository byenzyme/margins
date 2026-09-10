# Diff Against Expected

## Alignment
- Workspace class matches expected: `partial-enzyme-or-margins`.
- Safe default matches expected: `notes/meeting-notes/YYYY-MM-DD meeting-title.md`.
- Non-mutation boundary matches expected: existing files are left untouched.
- Capture availability matches expected: user can start now.

## Evidence Used
- `notes/Captures/2026-06-08-customer-checkin.md`
- `notes/legacy/2026-05-20-kickoff.md`
- `notes/meeting-notes/2026-06-01 meeting sync.md`
- `.margins/cache-state.json`
- `.margins/import-state.json`
- `.enzyme/cache-state.json`

## Notable Differences
- This run is a best-candidate diagnostic, not the output of a dedicated workspace-diagnostic skill. The current Margins skills are capture/distillation oriented, so the artifact records that limitation.
- The generated diagnostic keeps import/cache discussion short unless the fixture evidence requires it.

## Risk Against Expected
Low for this skill-only evaluation. Main residual risk is that a future app surface could make scan/cache/import actions feel mandatory even when the diagnostic text says they are optional.
