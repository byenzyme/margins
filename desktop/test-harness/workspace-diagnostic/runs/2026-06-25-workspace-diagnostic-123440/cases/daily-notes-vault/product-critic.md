# Product Critic: daily-notes-vault

## Strongest thing the diagnostic did
Chose to piggyback on the existing `Daily/` rhythm rather than forcing a new
meeting folder — matching expected and respecting the dominant convention.

## Most serious trust risk
Two evidence inconsistencies undercut confidence:
1. do-not-do says "force a meeting folder that doesn't exist" — but a
   `meetings/` folder DOES exist (`meetings/2026-06-11 product sync.md`,
   `meetings/notes-raw.txt`). The competing convention was not even acknowledged.
2. "History And Import: There is no strong import requirement here" while a raw
   transcript (`meetings/notes-raw.txt`) is present.
The expected diagnostic's key caveat — "do not treat every Daily file as a
meeting" — is also not surfaced. The destination is right; the reasoning is thin.

## Would the user feel ready to start capture?
Yes.

## Is import/history understood as optional?
Yes, but the "no import requirement" claim is slightly inaccurate given the
transcript.

## Surface / patch implicated
skill: when two plausible destinations exist (Daily/ vs meetings/), name the
competing convention and the tradeoff instead of silently collapsing to one.
This is the ambiguity case LOOP_SPEC explicitly calls out.
