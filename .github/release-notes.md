Margins 0.4.13 adds the project recording service to the official release
archives. The `margins` CLI and `margins-server` now ship together on Linux and
Apple Silicon macOS; the macOS archive also includes `margins-live`.

- A fresh bb Margins plugin installation can obtain both the CLI and project
  recorder from one version-pinned, checksum-verified archive.
- BB's connected-note thread reads the pinned meeting memo and transcript
  through its project's Margins service and can link the written Home Source
  note back to that session.
- The project service prepares transcription assets on its host and retains
  finalized audio for transcription while those assets become ready.
- Recall searches declared notes directly when a Workspace has no selected
  link entities, so setup can verify both a grounded question and an exact phrase.
- The browser recording, memo, transcript, and connected-note path passed an
  isolated BB end-to-end run with a spoken audio fixture.

The public open-core source remains available for audit and extension. Meeting
audio, transcripts, and Workspace notes remain under the user's control.
