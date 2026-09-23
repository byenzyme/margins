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

For a same-capture CoreML check, set absolute `MARGINS_MENU_TRANSCRIBE_BIN`,
`MARGINS_MENU_TRANSCRIBE_VAULT`, and `MARGINS_MENU_TRANSCRIBE_HOME` paths. After
the project capture reaches **saved**, click **Transcribe Mac copy**. This runs
`margins --local transcribe` on each retained stereo WAV in the scoped vault,
with a separate profile and no note generator. Keep the menu connected until
the Mac transcript is complete; Disconnect removes the temporary WAV copies.

The BB project service starts transcription after remote finalization. The Mac
CoreML and Linux ONNX jobs can therefore overlap; **Transcribe Mac copy** does
not enforce which job starts first. The retained stereo WAV and remote Opus
lanes come from the same native capture, though their encodings differ.
