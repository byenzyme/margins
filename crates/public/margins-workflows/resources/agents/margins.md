<!-- BEGIN MARGINS AGENT INSTRUCTIONS -->
## Margins Meeting Artifacts

This directory is a Margins vault (a folder with `.margins/`). Use the `margins` CLI to inspect meeting artifacts before guessing paths or searching the whole vault.

Useful commands:
```bash
margins init            # establish or refresh this vault and its recall index
margins new
margins
margins current
margins ls
margins recent
margins transcript <meeting-id>
margins transcribe <audio-file> --name <session-name> --memo <memo.md> --speakers 1
margins import granola <export.json-or-csv>
margins recall "<query>"
```

Vault routing:
- The vault is discovered git-style: the CLI walks up from the current folder for a `.margins/` directory.
- Pass `--project <path>` to target a different vault without cd-ing into it.
- Do not create `.margins` folders manually; use `margins init` (establish) or `margins new` (record).

Recording lifecycle:
- `margins new` starts a separate meeting, generates its stable id, makes it current, and opens the recorder.
- Bare `margins` (or `margins attach`) returns to the current meeting and records another segment. Do not invent suffixed names for interruptions or multipart calls.
- `margins attach <meeting-id>` makes an older meeting current before recording another segment.
- Starting a new meeting replaces the current pointer; it never deletes the previous meeting.

Note refinement workflow:
- Run `margins recent` to identify the meeting and pick its stable meeting id.
- Run `margins transcript <meeting-id>` for the complete transcript: every utterance with speaker and timestamp, merged with the memo timeline. The root element's `view` attribute says whether you got the full reconstruction (`full`) or only a memo-aligned artifact (`aligned`, which can omit stretches where no memo was taken).
- Inspect `<saved_note_path>` in the transcript metadata for the saved/distilled note path when present, then patch that note directly.
- Treat memo lines as attention signals, not as the full content of the meeting.
<!-- END MARGINS AGENT INSTRUCTIONS -->
