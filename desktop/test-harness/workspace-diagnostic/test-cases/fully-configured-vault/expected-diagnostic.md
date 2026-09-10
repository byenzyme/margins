# Expected diagnostic

This vault is already well set up. Margins and Enzyme are both configured, the
`meetings/` folder contains consistently structured notes, people wikilinks
resolve to a `people/` folder, and `.enzyme/state.json` reports a healthy index
with catalysts present. The right response is to say so plainly and confirm the
user can keep going as-is.

## No restructuring needed

The diagnostic must NOT suggest any of the following:
- moving or renaming existing notes
- adding or normalizing frontmatter on existing files
- creating new folders
- changing the capture destination (it is already `meetings`)
- re-running `enzyme init` (already initialized at version 0.5.9, above the 0.5.8 contract floor)
- importing or converting any files

If any of these are suggested, the run has over-diagnosed a healthy vault.

## Four-tier map — expected output

Show the four-tier map with all content in the "already usable" tier:

- **Already usable:** `meetings/` — 5 notes with consistent `date`, `title`,
  `participants`, and `tags` frontmatter; wikilinks to `people/` folder that
  exists and is populated; Enzyme index healthy (8 docs, 6 catalysts).
- **Weakly indexable:** nothing.
- **Would improve retrieval:** nothing (or at most a very minor note — e.g., no
  notes yet for 2026-07, which is expected and not a problem).
- **Can wait:** nothing that needs attention before the next capture.

## Proof note

The diagnostic must still produce a distilled example note from the user's own
content — one of the five `meetings/` notes is the right source. Show the
output. This is the "wow moment" even when no restructuring is needed.

## Tone and register

- Plain language, no CLI jargon.
- Opening line should be a positive confirmation: something like "Your notes
  folder is already set up well — Margins can start capturing here immediately."
- Do not manufacture findings to fill the template. If there is nothing to flag,
  say so.
- Brief is better. A healthy vault should get a shorter diagnostic than a messy
  one.

## Starting capture now

Confirm the destination is `meetings/` and the filename pattern is
`YYYY-MM-DD topic.md`. No other action is needed before capture.

## Safe next steps

The only optional next step worth mentioning is running a sample search now that
the index is built (e.g., `enzyme -p . catalyze "Q3 planning"`) to show the
user what retrieval looks like. This is optional and read-only.
