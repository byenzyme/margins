# google-drive-export diff against expected

- Alignment: safe default is `notes/meeting-notes/YYYY-MM-DD meeting-title.md`; capture is not blocked; existing files remain untouched.
- Alignment: avoids migration, dedupe, people folders, and old-file renames.
- Added: Matches expected default and avoidance. Adds explicit scan/indexability gap: Enzyme scan sees Markdown but misses import-state files that explain why cleanup should be optional.
- Possible gap for checker: concise diagnostic may need more case-specific evidence in final UI copy, but supporting pass files carry the evidence.
