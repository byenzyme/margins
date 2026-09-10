# Expected diagnostic for append-only-dated-log

## What Margins should notice
- There is a recurring `meeting-log/` home for meeting artifacts, but with mixed formats.
- Most meeting artifacts include dates in filenames and/or headings, with a duplicate pair on `2026-06-14` plus a transcript-style drop-in.
- Folder mix includes one non-meeting planning file (`plans/sprint-notes.txt`) and one legacy log-like file (`append-only.md`) with loose metadata.

## Uncertainty to state
- Naming is mostly consistent but duplicates and transcript fragments suggest manual, imperfect filing, so avoid strict-schema claims.
- Mention that old duplicates should be left as history unless user asks for cleanup.

## Safe default recommendation
- Keep capturing in `meeting-log` because it is already the least risky choice.
- For each new note: `YYYY-MM-DD topic.md`.
- Keep one-note-per-meeting style (`YYYY-MM-DD topic.md`) for consistency with existing dated files.

## What to avoid
- Do not rename/rehome the existing dated log.
- Do not replace their append pattern with people/project trees.
- Do not request an initial import before continuing capture.

## Tone expectation
- Reassuring and low-friction: “You already have a workable pattern; we’ll stick with it and keep it reversible.”
