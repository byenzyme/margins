# Product Critic: foreign-domain-vault

## Strongest thing the diagnostic did
Nailed the hardest judgment in the suite: do-not-do "assume this is a meeting
notes vault" and "impose people-based taxonomy" — despite the bait of
`contacts/people.json` and a file literally named `board-meeting-notes.md`. It
did not over-read the "board"/"meeting" filename words into a meeting workflow.
This is the correct foreign-domain restraint.

## Most serious trust risk
The destination is `notes/meetings/`, which is fine and matches the fixture, but
DESIGN_CONTEXT's stated preference for foreign-domain folders is an *isolated /
clearly-namespaced* meeting folder (e.g. "Margins meetings/") so new notes do not
look native to an unrelated personal/admin folder. `notes/meetings/` is
adequate but slightly less isolating than the design ideal. Minor; the expected
diagnostic agrees with `notes/meetings/`, so this is not a failure.

## Would the user feel ready to start capture?
Yes, and the adversarial "why are you making meeting folders in my finance/
recipe files" concern is pre-empted by the isolation framing.

## Is import/history understood as optional?
Yes — correctly framed as start-fresh; the pasted-calendar snippet is optional.

## Surface / patch implicated
skill (minor): for foreign-domain folders, prefer a clearly Margins-namespaced
destination to reinforce that captures are a new, separate layer.
