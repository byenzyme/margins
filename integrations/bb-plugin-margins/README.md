# Margins for bb

Margins adds a recording panel and an editable meeting notepad to bb. The
browser can supply microphone audio, or Margins Menu on a Mac can record both
microphone and computer audio. Meetings are saved in the selected Margins
Workspace's capture store. A project's Workspace override takes precedence over
the machine default; without either, the panel asks for a Workspace.
Meetings saved **On this Mac** use a separate local store and do not appear in
this bb project automatically.

## What works in this version

- Browser, Mac PWA, and mobile microphone recording, labelled **Microphone only**.
- Paired Mac-local `margins native-bridge` recording with separate microphone and
  computer-audio lanes delivered to the configured Linux Margins Workspace.
- **Use browser microphone** starts the browser-only fallback. For both audio
  lanes, choose **Connected Workspace** in Margins Menu and record there; bb
  joins its active meeting without starting a second capture.
- Small pause/stop controls and one full, editable notepad.
- Recording ownership survives panel close and thread navigation because a bb
  content script owns the browser stream. An app overlay provides persistent
  controls for browser and paired Mac recordings.
- Capture controls and heartbeat route by meeting session id. Switching threads
  or bb projects that select the same Workspace does not change the live meeting.
- A 25-second reconnect grace. After it expires, the project machine stops and
  saves the audio it received.
- A menu-recorded meeting in the same Workspace can be joined from the bb
  panel. Its active session appears with one revisioned notepad; bb does not
  start another browser recording for that meeting.
- Saved recordings are ordinary Margins sessions. An ASR-enabled project service
  transcribes finalized audio there. **Make connected note** requests
  transcription for an older saved session when needed; Margins still owns
  diarization, distillation, and retention.
- **Make connected note** starts a bb thread in the meeting's recorded project
  (or the project selected for older meetings without provenance) and opens it.
  The thread receives a pinned meeting reference with the current memo revision.
- Newly started recordings are queued for one automatic note thread after the
  session is finalized and its transcript is ready. The queue survives plugin
  reloads and keeps separate meetings independent; older saved sessions are not
  backfilled automatically.
- Bundled `watermark`, `workspace-setup`, and `connected-note` skills. BB agent
  tools read a pinned meeting's memo and transcript through its project host,
  then link an existing note in the Workspace Home Source.

The panel does not show a live transcript, host picker, elapsed-time dashboard,
meeting library, or processing controls. Plugin storage holds only routing and
heartbeat facts. It never holds audio, transcript, or notepad bodies.
The normal panel keeps the native port and pairing code inside **Connect a Mac
recorder manually**. Saved meetings show their date and one connected-note
action tied to that meeting.

## Installation shape

On first Workspace discovery, the plugin’s project-host worker installs one version-pinned,
digest-verified Margins release when needed. That release contains:

- `margins`, the normal CLI;
- `margins-server`, the project-side recording service.

The plugin pins `v0.4.11`, whose Linux and Apple Silicon macOS archives include
both executables. That release must be published before a clean installation
can start the project service. Development builds can use
`MARGINS_CLI_BIN` and `MARGINS_PROJECT_SERVER_PATH` as explicit overrides.

The service is launched with the bb project's default source path as
`MARGINS_WORK_DIR` and an explicit `MARGINS_WORKSPACE`. It uses that Workspace's
capture store. Set the machine default with `margins workspace default --set
<id>`, or enter a project override in the panel. For local development,
`MARGINS_PROJECT_SERVER_PATH` may point at an already-built `margins-server`.

The released project service prepares transcription on the **project host** in
the background at first launch. A Mac host downloads the shared FluidAudio
CoreML model cache; a Linux host downloads the pinned Parakeet ONNX model and
ONNX Runtime into its user cache. Meetings shows setup progress and a Retry
button on failure. Recording can start while setup runs: finalized audio waits
in the durable transcription queue, then transcribes when the model is ready.
The Mac running Margins Menu only needs its recording bridge when the project
service is on a remote Linux host; the remote host performs ASR.

For an explicitly provisioned project host, `asr-runtime.json` in the plugin's
host data directory can select a server with the `hosted-web` feature and its
local Parakeet model and ONNX Runtime library instead of automatic setup. It contains absolute paths:

```json
{
  "serverPath": "/path/to/margins-server",
  "modelDir": "/path/to/parakeet-tdt-v2",
  "ortLibraryPath": "/path/to/libonnxruntime.so"
}
```

The project-host worker validates those paths before launch. Without this
configuration, it uses the version-pinned installation as before. The service
stores transcripts alongside the Workspace's sessions, not in plugin storage.

## Mac PWA recording

The Mac PWA can request microphone permission through the browser and record
that microphone directly. This does not require a Mac Margins CLI. The project
machine still needs a Margins recording service to receive and save the audio.
The browser-only mode is labelled **Microphone only**; it does not request a
screen share or claim to capture the Mac's output audio. Browser tab or screen
sharing can offer an audio track, but that depends on the user's selected
surface and browser support. Browser permission cannot grant audio access to a
separate native executable.

Margins Menu handles pairing when **Connected Workspace** is selected. For a
connected Workspace recording, choose **Microphone** in Margins Menu before
Start. The menu lists the Mac's available inputs and a System default choice;
the chosen input stays selected for future meetings on that Mac. bb shows the
selected microphone beside its connection status. If a pinned microphone is
unplugged, choose another input in the menu before recording. This choice does
not change the Mac's system default or the separate **On this Mac** recorder.

For a manual or older recorder, expand **Connect a Mac recorder manually** and run
an audio-capture-enabled Margins build **on the Mac running the browser**:

```sh
margins native-bridge --remote ssh://<configured-Linux-alias> \
  --workspace <Workspace-id> --origin https://<exact-bb-origin> --port 18765
```

Enter the one-time code printed by the bridge in the Margins panel. The plugin
compares the bridge's instance and Workspace identity with the BB project's
recording service before enabling Start. Configure the project host and Mac
bridge to use the same provisioned Linux service when using a separate authority:
`MARGINS_BB_REMOTE_URL` and `MARGINS_BB_REMOTE_TOKEN` select its endpoint on the
project host; the project or machine Workspace setting selects its Workspace. The Mac bridge uses
the normal `ssh://` alias or HTTPS credentials from `MARGINS_REMOTE_TOKEN`.
The bridge only accepts requests from the exact BB origin on loopback and never
lets browser requests change its remote destination. The token stays in the
browser tab's session storage. Stopping retains the native transfer spool until
the remote server acknowledges it.

The scoped Mac menu and authenticated Helium bb panel have passed an end-to-end
test: bb joined the active two-lane recording, saved its memo, and retained that
memo through Finish and Linux transcription. This was a development bundle,
not an installed-app first-run test. A menu-owned meeting's memo is editable in
bb through the Workspace service; the native bridge itself carries audio and
control only. The plugin's release-pinned installation also requires a published
release archive containing both `margins` and `margins-server`. The currently
published CLI does not contain the new `native-bridge` command; use the built
`margins-private` executable for development until a matching release ships.

`@Margins` is also withheld until the project recording service exposes a
bounded live-context read. The control snapshot deliberately carries no rolling
transcript.

## First-run Workspace setup

In Meetings, select the bb project containing your notes and choose its Home
folder. An Obsidian project can use its own root without typing a path. The
note destination defaults to `inbox`. Continue scans notes and uses hosted Jev
selection when available, then shows the exact Workspace plan before **Use this
Workspace** applies it and sets the machine default. A vault with no notes can
still be connected for recording. Existing People notes can inform recall; the
first confirmed participant can create a People folder during distillation.
SQLite sources require an explicit mapping later.

## Development

Run inside this directory:

```bash
npm install
npm run typecheck
npm test
npm run build
```

Do not install or reload this package into a live bb instance during repo tests.
