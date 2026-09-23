# Margins menu bar test client

This scoped macOS menu bar app controls existing Margins recorders. **On this
Mac** uses the private `margins-live` loopback API, including its CoreML live
transcription. **BB project** launches a configured `native-bridge` helper and
sends separate microphone and system lanes to the selected Linux Workspace,
where the project service owns ONNX transcription and storage. The menu app
does not capture audio or hold an audio permission itself.

Build into this directory's ignored `.build/` and launch:

```sh
./build-and-run.sh
```

For a scoped remote test, set `MARGINS_MENU_BRIDGE_APP` to the exact test helper
`.app`, `MARGINS_MENU_REMOTE` to its SSH alias or HTTPS authority, and
`MARGINS_MENU_WORKSPACE` to the provisioned Workspace. The optional
`MARGINS_MENU_SSH_REMOTE_BINARY`, `MARGINS_MENU_SSH_REMOTE_DATA_DIR`, and
`MARGINS_MENU_HOME` variables are forwarded only to the launched helper. The
helper needs its own microphone and system-audio permissions. The menu app
uses a private temporary pairing file, removes it after pairing, and keeps the
bearer in memory. Its scoped local WAV copies are removed when Disconnect is
clicked, after the recording is saved. Use `MARGINS_MENU_LOCAL_DISCOVERY` for a
nondefault `margins-live` profile.

The BB project service starts transcription after remote finalization. A local
recording and a remote recording are separate sessions; this client does not
claim a byte-for-byte ASR comparison across them. For an exact comparison,
the bridge's `--local-audio-dir` option retains a copy of the native capture
before its recovery audio is reclaimed; that copy can be transcribed on Mac
while the project service transcribes its remote lanes.
