# Product Critic: mixed-messy-power-vault

## Strongest thing the diagnostic did
Treated a genuinely messy, multi-root vault as usable rather than broken — the
exact DESIGN failure mode ("Margins is judging my messy folder") is avoided. It
picked `notes/captures/` (an active capture folder) over the strategy, exports,
and transcript roots, matching expected, and its do-not-do list protects
strategic docs and legacy transcripts.

## Most serious trust risk
The stale `imports/legacy-capture-index.md` is listed as evidence, but the
expected diagnostic's specific caveat — "treat the legacy import manifest as
history, not a current capture rule" — is not stated. A power user could wonder
whether Margins will follow that stale index. The destination choice is right; the
reasoning behind ignoring the legacy index is implicit.

## Would the user feel ready to start capture?
Yes, without feeling judged.

## Is import/history understood as optional?
Yes, generically.

## Surface / patch implicated
skill: when a stale import/capture manifest is present, explicitly say it is
treated as history and will not drive the destination choice.
