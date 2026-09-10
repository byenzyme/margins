# Product Critic: frontmatter-obsidian

## Strongest thing the diagnostic did
Explicitly listed "normalize all frontmatter before first capture" and "retag
existing notes" as do-not-do. With malformed YAML present, the temptation to
"fix the schema first" is exactly the trap; the skill refused it and kept
`notes/meetings/` as the additive destination. This is the indexability lens
used correctly: weak frontmatter is named without being treated as a repair
prerequisite.

## Most serious trust risk
Uncertainty copy is jargon-y ("metadata signals are yaml-frontmatter,
malformed-frontmatter, wikilinks"). The word "malformed" leaking to a user
edges toward the vault-health tone DESIGN warns against, even though the
recommended action is correctly hands-off.

## Would the user feel ready to start capture?
Yes. No cleanup gate.

## Is import/history understood as optional?
Yes (boilerplate). The `imports/zoom` transcript and email-sync are listed as
evidence but not specifically addressed.

## Surface / patch implicated
skill: drop the raw "malformed-frontmatter" token from user-facing copy;
express weak frontmatter as "some notes have inconsistent metadata, which is
fine — nothing will be changed."
