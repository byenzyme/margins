# bb-plugin-margins

This is a small bb companion for Margins. It lets a bb thread talk to the local
Margins runtime on the selected recording Mac.

It does not read Margins database files, browse old meetings, or store transcript
text in bb. Margins stays the owner of recording, audio permissions, the session
clock, memo timestamps, transcript storage, and the live local API.

The install shape is one deterministic **Start recording** action in bb. If the
recorder is missing or stopped, the host worker prepares it as part of that
intent: it downloads one named Margins release, checks the release digest before
unpacking it, and keeps the windowless recorder in plugin-owned storage.
The same archive contains the normal `margins` command. The plugin puts that
command in `~/.local/bin` when the path is free or already belongs to this
plugin; it never overwrites an unrelated command.

## What appears in bb

- One thread side-panel action: **Margins live**.
- One compact listening row with a small waveform and recording controls, plus one
  continuously editable **Notepad**.
- One `@Margins` mention provider when live meeting context is available.
- Two bundled skills:
  - `watermark` for live, transcript-grounded feedback.
  - `workspace-setup` for optional Margins Workspace setup.

There are no nav pages, history views, message actions, review actions, broad
retrieval tools, or native agent tools in this MVP.

## How it connects

The bb server resolves the current thread to its environment host, or uses the
optional `marginsHostId` setting. Browser code never chooses arbitrary host ids.
The server then calls this plugin's bb host worker on that explicit host.

The host worker discovers Margins from the private discovery file written by the
process that owns capture:

```text
~/Library/Application Support/margins/desktop-live.v1.json
```

For non-default profiles, the directory name follows Margins' profile slug, for
example `margins-first-run-test`. Tests and development can override discovery
with `MARGINS_LIVE_DISCOVERY_FILE`. A direct development connection requires
both `MARGINS_LIVE_API_URL` and `MARGINS_LIVE_API_TOKEN`; one is never combined
with credentials from the private discovery file.

The host worker calls only the small local live API:

```text
GET  /v1/live/snapshot
POST /v1/live/start
POST /v1/live/pause
POST /v1/live/resume
POST /v1/live/stop
POST /v1/live/memo
POST /v1/live/notepad
```

Those requests use the V1 JSON contract from `margins-meeting-protocol`:
`protocol_version`, `operation_id`, `session_id`, `expected_generation`,
`rolling_transcript`, `memo_lines`, and `notepad_revision`. The notepad update
sends the complete visible text with the last revision it saw. Margins keeps
line timestamps private, preserves them for unchanged lines, marks edited lines,
and timestamps new lines against its own meeting clock. The legacy `DesktopLive*V1` type and
filename names describe the first adapter, not a requirement for a desktop UI.
The HTTP server itself calls the transport-neutral `LiveRuntime` seam in
`margins-live-runtime`. The signed archive contains `margins-live`, a small
background process which uses the existing native Margins recorder without
opening a desktop window. The desktop app can still serve the same contract as
a compatibility adapter.

The first release target is Apple Silicon macOS. Until that release exists,
preparation reports that an update is needed instead of falling back to
an unpinned download. Development can point `MARGINS_LIVE_RUNTIME_PATH` at a
compatible runtime; it must answer `capabilities` with live protocol V1.
The probe also requires `editable_notepad: true`, so an older append-only V1
runtime is treated as needing an update instead of failing after the panel opens.

## Storage

Plugin storage holds only thread attachment pointers, a short-lived stopped-meeting
handoff receipt, and preferences. A pointer
contains the bb thread id, selected host id, Margins session id, attach time, and
current generation. The receipt keeps the stopped session ID only long enough to
survive refresh and offer **Make connected note**. It never stores transcript text, memo text, audio, file
paths, credentials, participant names, or meeting history.

## Development

Run package commands only in this directory:

```bash
npm install
npm run typecheck
npm test
npm run build
```

Do not install or reload this package into a live bb instance during repo tests.
