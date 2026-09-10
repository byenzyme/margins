# zoom-transcript-dump diff against expected

- Alignment: safe default is `notes/meetings/YYYY-MM-DD topic.md`; capture is not blocked; existing files remain untouched.
- Alignment: avoids migration, dedupe, people folders, and old-file renames.
- Added: Matches expected default and non-blocking posture. Calls out that scan output is especially misleading here because almost all evidence is non-Markdown transcript state.
- Possible gap for checker: concise diagnostic may need more case-specific evidence in final UI copy, but supporting pass files carry the evidence.
