# Margins menu bar recorder

The menu offers one first-run destination choice: **On this Mac** or
**Connected Workspace**. It remembers that choice in its own preferences.
The everyday surface has one primary action, **Record meeting** or **Finish
meeting**; Pause, Resume, Change destination, and Quit are in **More**.

**On this Mac** talks to the `margins-live` loopback API. Its CoreML worker
transcribes locally and saves to the selected Mac profile's Margins store. The
menu starts a configured `margins-live` app if that local service is not yet
running. This path does not send audio to Linux. The local model must already be
installed; a clean-install model downloader is still needed before shipping.

**Connected Workspace** launches a `native-bridge` helper and sends microphone
and system audio to the selected Linux Workspace. The menu checks that pairing
returns the requested Workspace. The Linux service stores the recording and
runs its configured ASR. A CoreML-enabled bridge can currently publish
provisional words during recording; that is a development behavior, not an
additional Mac transcription choice shown in this menu.

The menu app controls recording but does not acquire audio permission itself.
The signed capture process needs its own Microphone and System Audio grants.
The menu does not run a second `margins transcribe` pass after Stop.

## Scoped Mac build

Run `./build-and-run.sh` to build and launch the menu-only app. Set
`MARGINS_MENU_TEST_WINDOW=1` to compile the same controls as a normal window for
accessibility automation. `--build-only` compiles and signs without launching.

For a scoped test, set `MARGINS_MENU_SETTINGS_DOMAIN` to a unique preferences
suite. `MARGINS_MENU_BRIDGE_APP` and `MARGINS_MENU_LIVE_APP` select signed test
bundles; a packaged app can instead include them at
`Contents/Helpers/Margins Capture.app` and `Contents/Helpers/Margins Live.app`.
`MARGINS_MENU_REMOTE` and `MARGINS_MENU_WORKSPACE` prefill the connection fields.
The optional SSH binary/data-dir and home overrides are forwarded only to the
bridge. Set `MARGINS_MENU_TRANSFER_DIR` for an isolated recoverable transfer
spool, and `MARGINS_MENU_MIC_DEVICE` to select an exact input device without
changing the macOS default. Set `MARGINS_MENU_LOCAL_DISCOVERY`,
`MARGINS_MENU_LOCAL_HOME`, `MARGINS_MENU_LOCAL_PROFILE`, and
`MARGINS_MENU_LOCAL_WORK_DIR` for a separate local runtime profile.

The helper uses a private one-time pairing file. The menu removes it after
pairing and keeps the bearer in memory. Disconnecting after a saved recording
removes scoped temporary audio copies; the remote transfer spool persists until
the service acknowledges it.
