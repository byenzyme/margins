# codebase-plans-folder diff against expected

- Alignment: safe default is `docs/meetings/YYYY-MM-DD meeting-title.md`; capture is not blocked; existing files remain untouched.
- Alignment: avoids migration, dedupe, people folders, and old-file renames.
- Added: Matches expected destination. Adds caveat that scan evidence alone was insufficient, so representative file reads were required.
- Possible gap for checker: concise diagnostic may need more case-specific evidence in final UI copy, but supporting pass files carry the evidence.
