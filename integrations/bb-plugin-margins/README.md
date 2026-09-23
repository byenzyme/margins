# Margins for bb

Margins adds a small recording panel and an editable meeting notepad to bb.
The current bb window supplies the audio; the recording and notes are saved in
the project’s primary folder under its existing `.margins` store.

## What works in this version

- Browser, Mac PWA, and mobile microphone recording, labelled **Microphone only**.
- Paired Mac-local `margins native-bridge` recording with separate microphone and
  computer-audio lanes delivered to the configured Linux Margins Workspace.
- One Start action: permission is requested before a project meeting exists.
- Small pause/stop controls and one full, editable notepad.
- Recording ownership survives panel close and thread navigation because a bb
  content script owns the browser stream. An app overlay provides persistent
  controls for browser and paired Mac recordings.
- A 25-second reconnect grace. After it expires, the project machine stops and
  saves the audio it received.
- Saved recordings are ordinary Margins sessions. The normal `margins` CLI
  continues to own transcription, diarization, distillation, and retention.
- **Make connected note** fills and focuses the bb composer without sending.
- Bundled `watermark` and `workspace-setup` skills; no recording agent tool.

The panel does not show a live transcript, host picker, elapsed-time dashboard,
meeting library, or processing controls. Plugin storage holds only routing and
heartbeat facts. It never holds audio, transcript, or notepad bodies.

## Installation shape

On first Start, the plugin’s project-host worker installs one version-pinned,
digest-verified Margins release when needed. That release contains:

- `margins`, the normal CLI;
- `margins-server`, the project-side recording service.

The service is launched with the bb project’s default source path as
`MARGINS_WORK_DIR`, so it writes to `<project>/.margins`, never an environment
worktree or the recording device’s personal Margins store. For local development,
`MARGINS_PROJECT_SERVER_PATH` may point at an already-built `margins-server`.

## Mac PWA recording

The Mac PWA can record its microphone directly. For microphone and computer
audio, run an audio-capture-enabled Margins build **on the Mac running the browser**:

```sh
margins native-bridge --remote ssh://<configured-Linux-alias> \
  --workspace <Workspace-id> --origin https://<exact-bb-origin> --port 18765
```

Enter the one-time code printed by the bridge in the Margins panel. The plugin
compares the bridge's instance and Workspace identity with the BB project's
recording service before enabling Start. Configure the project host and Mac
bridge to use the same provisioned Linux service when using a separate authority:
`MARGINS_BB_REMOTE_URL`, `MARGINS_BB_REMOTE_TOKEN`, and
`MARGINS_BB_REMOTE_WORKSPACE` select it on the project host. The Mac bridge uses
the normal `ssh://` alias or HTTPS credentials from `MARGINS_REMOTE_TOKEN`.
The bridge only accepts requests from the exact BB origin on loopback and never
lets browser requests change its remote destination. The token stays in the
browser tab's session storage. Stopping retains the native transfer spool until
the remote server acknowledges it.

Mac native recording still requires a Mac build and physical device test; the
Linux build and protocol tests cannot prove TCC, system audio, or browser
private-network behavior. Native notepad editing is not exposed through this
bridge yet. The plugin's release-pinned installation also requires a published
release archive containing both `margins` and `margins-server`. The currently
published CLI does not contain the new `native-bridge` command; use the built
`margins-private` executable for development until a matching release ships.

`@Margins` is also withheld until the project recording service exposes a
bounded live-context read. The control snapshot deliberately carries no rolling
transcript.

## Development

Run inside this directory:

```bash
npm install
npm run typecheck
npm test
npm run build
```

Do not install or reload this package into a live bb instance during repo tests.
