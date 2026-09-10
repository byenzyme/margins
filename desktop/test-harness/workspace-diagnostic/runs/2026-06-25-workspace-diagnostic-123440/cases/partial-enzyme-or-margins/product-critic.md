# Product Critic: partial-enzyme-or-margins

## Strongest thing the diagnostic did
This is the case where the Enzyme/cache lens matters most, and the skill handled
it correctly. It detected the `.margins` and `.enzyme` state, then refused to
touch it: do-not-do "repair or rewrite existing tool state files," "move notes
into .enzyme/.margins," "delete stale cache artifacts." Critically, it did NOT
treat rebuilding the cache or finishing the interrupted migration as a
prerequisite for first capture — exactly the read-before-write / cache-building-
is-separate boundary DESIGN_CONTEXT requires. Destination `notes/meeting-notes/`
matches expected.

## Most serious trust risk
The interrupted-migration story is real and rich (stale `1.2.0-beta` state,
partially-applied import, config/destination mismatch), but the user-facing copy
flattens it into the generic boilerplate plus the tag "stale-cache-metadata."
The user is told not to worry but not clearly told *what* the leftover state is
or that it is safe to leave alone / rebuild later as a separate action. Mechanism
terms (`.enzyme`, `.margins`, "cache") appear without outcome framing.

## Would the user feel ready to start capture?
Yes — and importantly without first "fixing" the half-migrated state.

## Is import/history understood as optional?
Yes; cache rebuild is correctly implied as deferrable, though not explicitly
named as a separate optional action.

## Surface / patch implicated
skill: translate stale cache/migration state into outcome language — "Margins
found leftover setup files from a previous run; they are safe to leave as-is, and
rebuilding the private search cache is a separate optional step." This also maps
to the app's "Build private search cache" surface for a later UX run.
