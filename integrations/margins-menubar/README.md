# Margins menu bar recorder

Open **Meetings** in bb on a Mac and click **Connect**. bb sends the resolved
Workspace and a revocable capture grant directly to the Menu over loopback; no
SSH alias, Workspace id, or pairing code is needed. For a `getbb.app` origin,
the Mac must already be enrolled as a bb machine. The Menu reads that machine's
local Connect credential only to cross the bb edge; other HTTPS bb origins are
verified directly with the grant. The native helper receives only the Workspace
grant and talks to the Menu's local relay. Recording from either bb or
the Menu then appears in the same Workspace. bb settings can revoke the grant.
The grant stays in Menu memory, renews while connected, and expires after the
Menu disconnects or bb revokes access.

The first-run menu points bb users to **Meetings → Connect**; it sends the
Workspace and access without typing an SSH URL or Workspace ID. **On this Mac**
is a separate local recording choice. Typed connection settings remain under
**Manual connection (advanced)**. The menu remembers the chosen destination in
its own preferences. If Meetings asks for a Workspace, set one up in bb first.
The everyday surface has one primary action, **Record meeting** or **Finish
meeting**; Pause, Resume, Change destination, and Quit are in **More**.

**On this Mac** talks to the `margins-live` loopback API. Its CoreML worker
transcribes locally and saves to the selected Mac profile's Margins store. The
menu starts a configured `margins-live` app if that local service is not yet
running. This path does not send audio to Linux. The local model must already be
installed; a clean-install model downloader is still needed before shipping.

**Connected Workspace** launches a `native-bridge` helper and sends microphone
and system audio to the selected Workspace through bb's scoped HTTP relay.
Remote bb uses the project host's Workspace service; local bb uses the same
machine's service and store. The menu checks that pairing returns the requested
Workspace. The service stores the recording and runs its configured ASR. A CoreML-enabled bridge can currently publish
provisional words during recording; that is a development behavior, not an
additional Mac transcription choice shown in this menu.

The menu app controls recording but does not acquire audio permission itself.
The signed capture process needs its own Microphone and System Audio grants.
The menu does not run a second `margins transcribe` pass after Stop.

For bb-connected recording, macOS lists the bundled **Margins Capture** helper
under System Settings → Privacy & Security → Microphone after its first access
request. **On this Mac** uses the separate **Margins Live** helper. Neither
helper should be dragged into the Settings list. When packaging a Developer ID
build, run `sign-bundled-recorders.sh '/path/to/Margins Menu.app' '<Developer ID identity>'`
after bundling both helpers and before notarization. It signs the helpers with
the Hardened Runtime microphone entitlement and reseals the Menu bundle. Without
that entitlement, the helper can report denied access without ever appearing
in the Microphone list.

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
