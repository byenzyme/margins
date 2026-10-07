# Margins.app menu bar recorder

The menu records only into a bb-connected Workspace through `native-bridge`.

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
Workspace and access without typing an SSH URL or Workspace ID. Typed
connection settings remain under **Manual connection (advanced)**. If Meetings
asks for a Workspace, set one up in bb first.
The everyday surface has one primary action, **Record meeting** or **Finish
meeting**; Pause, Resume, Change destination, and Quit are in **More**.

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

Record opens the microphone and system audio at once and holds that audio
while the Workspace session is created, so nothing said after the click is
lost; the menu shows **Starting… don't speak yet** until the microphone
delivers audio. Pause and Finish act immediately, even before the session
exists. If the session cannot be created (for example, bb is unreachable), the
audio captured so far is kept in `~/.margins/unsent/` (or `$MARGINS_HOME/unsent/`)
as an owner-only WAV, and the error names the file. Import it with
`margins transcribe <file>`, then delete it.

For bb-connected recording, macOS lists the bundled **Margins Capture** helper
under System Settings → Privacy & Security → Microphone after its first access
request. The helper should not be dragged into the Settings list. When packaging
a Developer ID build, run `sign-bundled-recorders.sh '/path/to/Margins Menu.app' '<Developer ID identity>'`
after bundling the capture helper and before notarization. It signs the helper with
the Hardened Runtime microphone entitlement and reseals the Menu bundle. Without
that entitlement, the helper can report denied access without ever appearing
in the Microphone list.
The capture executable's embedded bundle ID must match its helper app's bundle
ID; the signing script checks this before replacing signatures.
When connected to bb, the Menu shows microphone authorization and offers
**Allow microphone** before starting a meeting. That action asks the helper for
permission without opening a capture session. A denied grant links to the Mac
Microphone settings pane.

## Scoped Mac build

Run `./build-and-run.sh` to build and launch the menu-only app. Set
`MARGINS_MENU_TEST_WINDOW=1` to compile the same controls as a normal window for
accessibility automation. `--build-only` compiles and signs without launching.

The official Apple Silicon release builds `Margins.app` from this source and
bundles the signed `margins` capture helper. The GitHub release publishes a
notarized, stapled `Margins-X.Y.Z-macos-arm64.zip`. `build-and-run.sh` remains
the local test build and does not create a distributable app.

For a scoped test, set `MARGINS_MENU_SETTINGS_DOMAIN` to a unique preferences
suite. `MARGINS_MENU_BRIDGE_APP` selects a signed test bundle; a packaged app can
instead include it at `Contents/Helpers/Margins Capture.app`.
`MARGINS_MENU_REMOTE` and `MARGINS_MENU_WORKSPACE` prefill the connection fields.
The optional SSH binary/data-dir and home overrides are forwarded only to the
bridge. Set `MARGINS_MENU_TRANSFER_DIR` for an isolated recoverable transfer
spool, and `MARGINS_MENU_MIC_DEVICE` to select an exact input device without
changing the macOS default.

The helper uses a private one-time pairing file. The menu removes it after
pairing and keeps the bearer in memory. Disconnecting after a saved recording
removes scoped temporary audio copies; the remote transfer spool persists until
the service acknowledges it.
