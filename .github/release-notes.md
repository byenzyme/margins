Margins 0.4.14 fixes first-run BB recording while transcription models are still
being installed on the project host. The official Linux and Apple Silicon macOS
archives continue to include `margins` and `margins-server`; the macOS archive
also includes `margins-live`.

- Linux's managed Parakeet setup now selects the model kind accepted by the
  live and offline transcription loaders.
- A browser meeting can start and save durable audio while the managed speech
  model downloads. Offline transcription can run after setup finishes.
- The BB Meetings panel keeps Transcribe unavailable while speech setup is
  preparing. On BB 0.41, the panel remains available without the newer overlay
  slot, with recording controls shown inside Meetings.
- On Apple Silicon, the project server can transcribe saved meetings with
  CoreML, and a stale derived preprocessor cache falls back to the downloaded
  original model. The saved-session Mac retry reached a terminal transcript and
  linked a connected note.
- The BB plugin pins this release and installs the checksum-verified runtime
  on its project host. Its empty-cache E2E lane covers recording and spoken
  transcription from a fresh model directory.

Meeting audio, transcripts, and Workspace notes remain under the user's control.
