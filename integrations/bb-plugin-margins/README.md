# Margins for bb

Margins adds a small recording panel and an editable meeting notepad to bb.
The current bb window supplies the audio; the recording and notes are saved in
the project’s primary folder under its existing `.margins` store.

## What works in this version

- Browser and mobile microphone recording, labelled **Microphone only**.
- One Start action: permission is requested before a project meeting exists.
- Small pause/stop controls and one full, editable notepad.
- Recording ownership survives panel close and thread navigation because a bb
  content script, rather than the panel component, owns the browser stream.
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

## Honest current limits

The plugin's current bb SDK does not yet expose either:

1. a browser-window-local native audio capability, required to enable
   microphone + computer audio on the Mac actually running the PWA; or
2. an app-global, client-scoped status/control contribution, required to keep
   recording controls visibly reachable while the side panel is closed.

The plugin therefore does not pretend a thread host is the current Mac. Browser
microphone capture can continue with the panel closed. A newer experimental bb
app-overlay surface may provide the persistent return path once the plugin moves
to that SDK and its lifecycle has been verified.

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
