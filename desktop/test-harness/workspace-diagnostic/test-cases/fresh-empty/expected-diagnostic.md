# Expected diagnostic for fresh-empty

## What Margins should notice
- The workspace has effectively zero usable note content (`workspace/` contains only `.gitkeep`).
- No meeting-note naming, date, frontmatter, or transcription signals are present.
- This is a strong “start now” scenario.

## Safe default recommendation
- Suggest creating one destination folder up front: `meeting-notes`.
- Use files named `YYYY-MM-DD meeting-title.md`.
- Keep notes in that location until a stable convention is intentionally established.

## Uncertainty / named caveats
- Ask only about a preferred location, but never delay first capture.

## What to avoid
- Do not suggest migrating, renaming, or reorganizing files.
- Do not imply required import steps before first capture.
- Do not recommend people folders or project-specific structure without evidence.

## Tone
- Use short, permissive copy: “You can start here now; nothing appears to force a vault-wide decision.”
