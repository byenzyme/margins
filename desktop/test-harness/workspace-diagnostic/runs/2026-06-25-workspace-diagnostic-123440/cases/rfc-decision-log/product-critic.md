# Product Critic: rfc-decision-log

## Strongest thing the diagnostic did
The do-not-do list is precise and process-aware: "rename RFC names" and "merge
decision log and meeting notes automatically." It recognized that RFC/ADR and
`DEC-*` files are process artifacts that must not be absorbed into a meeting
workflow, and routed new notes to a separate `docs/meeting-notes/` — matching
expected exactly.

## Most serious trust risk
Confidence is marked "high," but the expected diagnostic's key nuance is that
RFC/decision records are strong process docs and NOT evidence of a recurring
meeting-capture workflow. The candidate's uncertainty section is the generic
convention-tag list and does not convey "there is meeting-like content but no
established meeting cadence." Slight overconfidence relative to the evidence.

## Would the user feel ready to start capture?
Yes.

## Is import/history understood as optional?
Yes, generically.

## Surface / patch implicated
skill: when strong non-meeting process structure dominates, lower confidence and
state the "process docs != meeting workflow" distinction explicitly.
