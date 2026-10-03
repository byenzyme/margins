Margins 0.4.15 ships the CLI/TUI and bb plugin with a standalone project server.
The release archives contain `margins` and `margins-server`.

- BB browser recordings recover across reloads and network drops. Missing audio
  is marked incomplete rather than treated as a complete meeting: choose
  **Finish with what was saved**, or let the server finish an abandoned capture.
  Memo and transcript timing now share one clock.
- The TUI streams audio safely during capture, making Stop fast and limiting a
  crash to at most about five seconds of uncommitted audio with bounded temporary
  storage. BB and TUI memo edits merge or show a conflict instead of silently
  overwriting each other. A second recorder cannot take over the same session.
- Speech uses one transcription path. `--speakers` works for stereo recordings,
  and CoreML model downloads and cache repair are safer.
- The desktop app and `margins-live` no longer ship. Margins Menu records only
  in BB project mode.

Known limits: a page reload can lose browser chunks that the server has not
acknowledged; the recording shows those gaps. Linux live transcription is not
available.
