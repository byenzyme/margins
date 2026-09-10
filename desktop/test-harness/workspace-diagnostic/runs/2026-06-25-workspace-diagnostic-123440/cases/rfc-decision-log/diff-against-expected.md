# Diff Against Expected

## Alignment
- Workspace class matches expected: `codebase-with-docs`.
- Safe default matches expected: `docs/meeting-notes/YYYY-MM-DD meeting-title.md`.
- Non-mutation boundary matches expected: existing files are left untouched.
- Capture availability matches expected: user can start now.

## Evidence Used
- `notes/2026-03-11-architecture-review.md`
- `decisions/DEC-026-security-thread.md`
- `rfc/0001-api-contract.md`
- `rfc/0002-retry-policy.md`
- `rfc/rfc-appendix.txt`
- `notes/meeting-sync-jan.md`

## Notable Differences
- This run is a best-candidate diagnostic, not the output of a dedicated workspace-diagnostic skill. The current Margins skills are capture/distillation oriented, so the artifact records that limitation.
- The generated diagnostic keeps import/cache discussion short unless the fixture evidence requires it.

## Risk Against Expected
Low for this skill-only evaluation. Main residual risk is that a future app surface could make scan/cache/import actions feel mandatory even when the diagnostic text says they are optional.
