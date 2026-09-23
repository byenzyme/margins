# Margins in bb: recording beside the work

Status: product and technical contract for the bb plugin, revised 2026-09-06.

## The promise

Margins gives a person one quiet place in bb to record a conversation and keep
the notes they type while it is happening. The device running the current bb
PWA supplies the audio. The stable folder behind the bb project owns the saved
meeting.

The panel should answer five questions without exposing implementation:

1. What will this device capture?
2. Is recording ready, live, paused, or being saved?
3. Where will the recording and notes be kept?
4. Can I keep typing without managing timestamps?
5. What is the one safe next action?

The normal object language is **recording**, **notepad**, **transcript**, and
**connected note**. Process, host, worker, binary, token, lease, database,
entitlement, and session ID are implementation terms and do not appear in the
ordinary interface.

## What the user sees

Margins contributes one thread-side panel. The compact top row contains a
truthful source label and small start/pause/stop controls. The rest of the panel
is one editable notepad. There is no live transcript wall, machine picker,
elapsed-time dashboard, meeting library, or processing UI in this surface.

The source label is literal:

- **Microphone + computer audio** only after recording on this Mac has been
  enabled and that capability is present in the current bb window.
- **Microphone only** when the current browser supplies a microphone stream.
- No source label that implies capture while capability is unknown.

Audio and notes are saved to the bb project folder on its connected project
machine. The first-use and live UI says this plainly. It never says that audio
stays on the Mac or phone.

The notepad is a single plain-text editor. Newlines are the user's lightweight
marks. Margins associates lines with the meeting clock and preserves those
anchors without drawing timestamps in this panel. Typing is saved quietly;
pause and stop flush the latest edit before changing recording state.

## User-visible states

```text
Checking
  -> Needs setup (Mac only, until local native capture is actually available)
  -> Ready — Microphone + computer audio
  -> Ready — Microphone only
  -> Getting ready
  -> Recording <-> Paused
  -> Saving
  -> Saved

Any connected operation may move to:
  Recovering connection -> prior state
  Recovering connection -> Saving -> Saved
  Needs attention -> one truthful recovery
```

State claims are conservative:

- **Getting ready** begins immediately after Start and ends only after the
  browser/native producer and project-side session both exist.
- **Recording** means this bb window owns a live audio producer and the project
  side has acknowledged the recording.
- **Paused** means both producer and project state are paused.
- **Saving** means capture has stopped locally and the project side is
  finalizing what it received.
- **Saved** means the existing Margins audio session is durable in the project.
  A transcript may still be absent and is not promised here.
- **Recovering connection** begins after acknowledgements or heartbeat fail. A
  brief reconnect does not manufacture a second recording.
- **Needs attention** says what could not happen, what is safe, and one next
  step. An unsupported path has no retry button.

## First use on a Mac

The Mac PWA always offers browser **Microphone only** when its secure-context
media API is available. For **Microphone + computer audio**, the person starts
`margins native-bridge` on that same Mac with a fixed remote Linux Workspace
and exact BB origin, then enters its one-time pairing code in the panel. The
bridge listens only on loopback and accepts only that Origin and Host. The BB
plugin compares the helper's instance and Workspace identity with the project
recording destination before enabling native Start. No enrolled bb host or
thread environment substitutes for the current Mac.

The BB app overlay keeps native and browser capture controls reachable after
the thread panel closes. Native capture uses the existing two-lane recorder and
durable remote transfer spool. The helper reports separate microphone/system
accepted and dropped samples, system frames, and silence so the UI can make a
source claim grounded in actual capture. A Mac device pass is still required
before shipping the native source as verified.

## Browser and mobile recording

In a secure context with `getUserMedia` and a supported `MediaRecorder`, the
current bb window can record **Microphone only**. The browser asks for
permission before a project session is allocated. Audio chunks are ordered,
acknowledged, and forwarded to the Margins service on the project's primary
source machine. A phone never claims to capture audio playing inside another
app.

The frontend recording owner is a trusted bb plugin content script. That
lifecycle is once per browser tab/window and survives thread navigation and
panel unmount. React panel state is a view of that owner; mounting a panel does
not create a recorder, and unmounting it does not imply that recording stopped.

The owner keeps a stable client/capture identity across a page refresh in the
same tab. It sends a short heartbeat while active. The project-side lease gives
a refresh or brief network blip 25 seconds to recover. After that, Margins
stops and safely finalizes the audio chunks and notepad content already received.

## Project storage

The plugin resolves the thread's `projectId`, calls `bb.sdk.projects.get`, and
selects the one source with `isDefault: true`. That source's `hostId` and `path`
are the capture destination. The environment worktree is never a fallback.

```text
current bb window
  -> browser or Mac-local audio producer
  -> authenticated bb plugin route
  -> Margins service on project.sources[isDefault].hostId
  -> project.sources[isDefault].path/.margins
```

The Margins service reuses the existing hosted capture implementation:

- MediaRecorder WebM/Opus chunks and ordered receipts;
- capture ownership and recovery;
- normal Margins session and segment records;
- `.margins/recordings` audio materialization;
- existing memo format and timing model;
- existing finalization and later CLI transcription paths.

The plugin database stores only client/capture routing and heartbeat state. It
does not store audio, transcript, or notepad bodies. The capture Mac's normal
Margins store is not used. Another bb client attached to the same project sees
the same project sessions through the ordinary Margins store.

Personal/projectless threads and projects without a stable default local-path
source cannot start recording in this version; the UI names that limitation
instead of selecting an unrelated machine or folder.

## The notepad and knowledge boundary

The notepad is evidence attached to one saved recording. It is not automatically
treated as durable knowledge, agent memory, a task list, or a conclusion. The
existing Margins session format retains the notes and their hidden time anchors.

After recording, **Make connected note** may place a plain-language request in
the bb composer and focus it. It does not send the message. The Margins
distillation skill resolves the latest project meeting when the user eventually
sends. Raw recording IDs never enter visible copy.

This preserves the distinction:

- a recording and notepad are what happened and what caught the user's attention;
- a bb thread is work that may use that evidence;
- a connected note is a later, reviewed act of knowledge-making.

## `@Margins`

`@Margins` remains the deliberate way to add bounded meeting evidence to a bb
message. It is offered only when a fresh, project-side live-context read exists.
The control/status response deliberately contains no rolling transcript. Mention
resolution performs a separate bounded context read at send time.

If the project service has not produced live transcript context, the mention is
not offered. Recording readiness is never inferred from mention availability.
The bundled `watermark` skill interprets injected context; it is not an agent
tool and it does not control recording.

## Persistent status while the panel is closed

The content-script lifecycle is sufficient to own browser capture across thread
navigation. Newer bb SDKs now include an experimental app-overlay surface with
the correct once-per-window lifetime. The plugin does not adopt it as part of
the capture-core reduction: doing so changes the visible product and raises the
minimum bb version, so it should be a small, separately verified UI migration.

That migration should provide:

- one state/icon visible on desktop and mobile while the panel is closed;
- an activation callback that opens the owning project's Margins controls;
- lifecycle tied to one PWA window, not one thread component;
- support for recording, paused, saving, recovering, and needs-attention states.

Until the plugin moves to that SDK, closing the panel is supported for capture
continuity but the user must return to the thread/panel to see controls. This
is an explicit product limitation, not represented as complete UX.

## Installation and process boundaries

The plugin may install a version-pinned Margins release on the project source
machine as part of Start. The release contains the normal `margins` CLI and the
project-side hosted capture service. Downloads are named, digest-verified, and
never overwrite an unrelated command.

The Mac panel currently shows the explicit `native-bridge` command and pairing
step. This is the available bridge while the BB SDK has no window-local native
audio API. The project-side release still needs to publish both `margins` and
`margins-server` for automatic installation on a fresh project machine.

The already-built `margins-live` Mac executable and its permission metadata are
retained as a possible implementation of the future client-local capture seam.
It is no longer a valid project store or a bb host-routing target by itself.

## Retention and processing

This plugin creates ordinary Margins sessions and does not add another
retention policy. The existing CLI owns transcription, retranscription,
diarization, artifact formats, and cleanup. The plugin has no Retranscribe or
speaker UI.

The existing cleanup-policy implementation needs a separate audit: UI offers
immediate, seven-day, and forever choices, while some deletion paths may not
consult that policy. This plugin must not promise timed deletion until that
core behavior is verified or fixed.

## Acceptance checks

- UI tests assert source and project-storage wording from a user's perspective.
- Browser capability tests never call a source ready until permission and both
  ends of capture exist.
- Content-script tests prove panel unmount does not dispose capture and app
  teardown does stop local tracks.
- Server tests prove the default project source is used and environment
  worktrees are ignored.
- Lease tests prove reconnect inside 25 seconds resumes ownership and expiry
  stops/finalizes once.
- Host/service tests prove chunks remain ordered and existing session/memo
  commands are used.
- The control snapshot contains no transcript body.
- Plugin storage tests prove no audio, transcript, or notepad text is persisted.
- Build/typecheck/package pass against the installed bb SDK declarations.
- A native Mac pass remains required before **Microphone + computer audio** can
  ship as ready.
