# Diff Against Expected

## Alignment
- Workspace class matches expected: `foreign-domain-vault`.
- Safe default matches expected: `notes/meetings/YYYY-MM-DD meeting-title.md`.
- Non-mutation boundary matches expected: existing files are left untouched.
- Capture availability matches expected: user can start now.

## Evidence Used
- `travel/board-meeting-notes.md`
- `travel/itinerary-2026-08-dublin.md`
- `finance/tax/receipts-notes.md`
- `health/personal-docs.txt`
- `misc/pasted-from-calendar.txt`
- `README.txt`

## Notable Differences
- This run is a best-candidate diagnostic, not the output of a dedicated workspace-diagnostic skill. The current Margins skills are capture/distillation oriented, so the artifact records that limitation.
- The generated diagnostic keeps import/cache discussion short unless the fixture evidence requires it.

## Risk Against Expected
Low for this skill-only evaluation. Main residual risk is that a future app surface could make scan/cache/import actions feel mandatory even when the diagnostic text says they are optional.
