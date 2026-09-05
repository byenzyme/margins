# Margins in bb: live meeting companion

Status: plugin, CLI-owned recorder, and release packaging implemented; native
macOS recording verification and release remain, 2026-09-04.

## The product idea

Margins should be quietly present while a meeting is happening. A local
Margins runtime owns listening, transcription, timing, and storage. bb is where
the meeting can sit beside the work it is shaping.

This is intentionally smaller than a meeting archive or knowledge system. A
meeting starts as private, unfinished material: a rolling transcript and a few
human marks. A bb thread is work: a question, investigation, decision, or change
that may draw on that material. The plugin lets the user cross that boundary on
purpose with `@Margins`; it does not turn every sentence into durable knowledge
or every note into a task.

The intended everyday shape is a CLI-installed background runtime with bb as
its visible control surface. The full Margins app may remain useful for deeper
review and settings, but neither a full window nor a menu-bar app is required
to keep a bb meeting alive.

## What the user sees

The panel is ordered around five immediate needs: confidence that recording is
on, explicit control over it, a place to catch a thought, a way to see what was
saved, and a deliberate handoff into the thread. Everything else is supporting
machinery.

The plugin contributes one thread-side panel called **Margins live**. It shows:

- one compact voice-style row that makes ready, listening, paused, and saving
  states recognizable at a glance;
- literal start, pause, resume, and stop controls;
- one continuously editable **Notepad** that saves quietly as the user types; and
- **Add @Margins**, which inserts `@Margins` into the thread composer without
  sending the message.

Host routing, elapsed time, transcript freshness, and raw transcript text stay
out of the normal panel. They support the integration but are not things a user
needs to monitor during a call. Relevant failures still appear where the
waveform and controls normally sit.

There is no Margins navigation area, meeting history, review queue, message
action, task importer, or general-purpose agent tool in this version.

On bb mobile, this same responsive plugin panel runs inside bb's native WebView
shell. It still controls the Margins runtime on the thread's recording host;
opening it on a phone does not move capture to the phone or request the phone's
microphone. Visible controls stay compact, while coarse-pointer hit targets stay
large enough to tap reliably.

The target main button follows the current state:

| State | Main action |
| --- | --- |
| Ready, including first use or a stopped/missing runtime | **Start recording** prepares what is needed and begins capture |
| Preparing recording | No second action; show one truthful, non-animated stage |
| Listening | **Pause** |
| Paused | **Resume** |
| Saving meeting | No action while the runtime finalizes |
| Meeting saved | **Make connected note**, or quiet **Not now** |
| Needs attention | **Try again** for the intended action, with one specific recovery |

Runtime detection, installation, startup, and health checks are deterministic
plugin behavior. An agent can explain or recover from an unusual setup problem,
but it is not in the normal control loop. The optional conversation about which
notes Margins may read and where it may write remains skill-led.

## Process ownership

There is one recorder and one clock: the running Margins runtime process.

```text
bb panel or @Margins
  -> bb plugin server
  -> bb host worker on the chosen Mac
  -> private loopback API in the running Margins runtime
  -> capture, rolling transcription, memo, and session storage
```

The implementation has a transport-neutral `margins-live-runtime` port with
`snapshot` and one small command enum. The loopback HTTP server depends on this
port, not on Tauri. `margins-live` is the windowless runtime shipped beside the
normal `margins` command; it delegates to the existing native recorder through
`NativeLiveRuntime`. The desktop app uses that same adapter as a compatibility
path, without becoming the architectural owner.

The plugin never starts a second recorder, reads Margins database files, parses
private transcript files, or calls raw Tauri command names. The browser never
receives the local token and never chooses an arbitrary host for a request.

The current thread environment's Mac is used by default. A recording Mac can be
set explicitly when the thread's worktree lives elsewhere. All host calls carry
an explicit bb host ID and fail closed if that host is offline.

## Installing and starting Margins

The target install is owned by the plugin's host worker:

- bb installs and distributes the JavaScript plugin and its host worker;
- the host worker looks for live protocol V1 and targets one exact release;
- one **Start recording** intent checks for a live meeting, and when needed
  downloads the named release, checks the digest published by GitHub before
  unpacking it, keeps `margins-live` in plugin-owned storage, starts it, begins
  capture, and attaches the resulting meeting;
- the archive's normal `margins` command is copied to `~/.local/bin` only when
  that path is free or already managed by the plugin; and
- Margins owns microphone/system-audio permissions, runtime updates, and its
  private data.

That preparation must be deterministic and consented by **Start recording**: no generated shell,
no `curl | sh`, no unpinned latest-version lookup during execution, and no agent
deciding where to put a binary. The plugin may use bb's normal confirmation and
progress surfaces.

The first packaged target is Apple Silicon macOS. The plugin pins the intended
first runtime release (`v0.4.9`). Before that release is published, setup says
an update is needed; it never substitutes a floating latest release. Other
platforms get an honest unsupported result until native capture is packaged for
them.

## The private local connection

At startup, the process that owns capture binds an unused `127.0.0.1` port and
writes `desktop-live.v1.json` inside its profile data directory. The file
contains:

- protocol version and profile name;
- the process ID and loopback base URL;
- a new random bearer token; and
- the six supported route paths.

The discovery file is written atomically with user-only permissions on Unix and
removed when the runtime exits when possible. A restart replaces the port and
token. The filename and `DesktopLive*V1` Rust type names are V1 compatibility
names from the first adapter; they do not make the desktop app the architectural
owner. This is separate from the hosted `margins-server` service.

Development may override the discovery file path. A direct URL override is used
only when a token override is also present, so a URL cannot borrow the token
from the private discovery file.

## Local live API

The local capture process exposes only these routes:

```text
GET  /v1/live/snapshot
POST /v1/live/start
POST /v1/live/pause
POST /v1/live/resume
POST /v1/live/stop
POST /v1/live/notepad
```

The wire types live in `margins-meeting-protocol`; the callable process seam
lives in `margins-live-runtime`. A snapshot contains:

- the current session ID, state, elapsed time, and generation;
- capture phase, system-audio health, warnings, and transcript freshness;
- at most 80 recent transcript lines / 20,000 characters; and
- the complete visible notepad as memo lines, plus an opaque notepad revision.

It contains no raw audio, local file paths, credentials, note history, bb thread
IDs, or distillation state.

Start, pause, resume, stop, and notepad-update requests carry an operation ID. Operations
that address a meeting also carry the session ID and may carry the generation
last seen by the plugin. Margins rejects requests aimed at a different or newer
recording. Successful operations return the latest snapshot, so the panel does
not have to guess what happened.

The bb editor sends the full plain-text notepad and the revision it last read.
It never sends timestamps. Margins reconciles that text with its timestamped
memo: unchanged lines keep their anchors, changed lines get an edit time, joined
lines keep the surviving line's anchor, and new lines use the current recording
time. Lines added while paused keep Margins' existing pause-block meaning. A
stale revision is rejected so another bb window cannot be overwritten silently.
None of these times are drawn in the bb panel.

`TimedMemoDocument` in `margins-core` owns those rules, the durable line shape,
plain-text reconciliation, revision, and Markdown parse/export. The terminal
notepad uses the same document for character edits, line splits, and joins. The
bb plugin does not recreate timestamp behavior in TypeScript; it only sends
plain text and renders the snapshot Margins returns.

## Thread attachment

Starting from a thread records a small pointer in plugin storage:

```json
{
  "threadId": "thr_...",
  "hostId": "host_...",
  "meetingId": "2026-09-04-10-00-00",
  "attachedAtUnixMs": 1788525600000,
  "generation": 3
}
```

That pointer is routing information, not meeting content. bb does not store the
transcript or memo bodies. If Start finds an already-running meeting, it attaches
that meeting instead of starting a second one.

Stop stores a separate, equally small handoff receipt containing the returned
`stopped_session_id`, host ID, thread ID, and save time. The live attachment is
then cleared. This receipt survives panel refreshes only so **Meeting saved** can
preserve the handoff to the exact capture. The composer uses natural language
about the meeting just recorded, and latest-session resolution finds it after
the user sends; raw meeting IDs and slugs never appear in user-visible copy. The
receipt contains no meeting content and is deleted by **Not now** or replaced
when a later recording succeeds.

## `@Margins` and the watermark skill

`@Margins` is the one everyday way meeting material enters an agent turn.

Mention search offers one item only when the attached meeting is Recording or
Paused. It stays hidden while the runtime is still starting or finalizing. bb's
mention API gives `resolve` only the chosen item ID, so the plugin encodes the
thread, meeting, and recording-host identity into that opaque ID. When the
message is sent, resolution reads a fresh snapshot from that exact host and
meeting.

The injected context includes recent transcript lines, notes the user saved, capture
health, and clear freshness language. It is limited to the current meeting and
is visible to the agent, not copied into plugin storage.

The bundled `watermark` skill tells the agent how to answer from that context:
lead with the useful read, stay short enough for a live conversation, say when
the transcript is behind or still changing, and offer one good next move. It
does not fetch data itself and is not a hidden tool.

There is no `margins_status`, `margins_get_context`, `margins_add_memo`,
`margins_submit_receipt`, or `margins_watermark` agent tool in this version.
Distillation remains a separate skill and workflow.

## Optional notes setup

The bundled `workspace-setup` skill is separate from live recording. It is used
only when the user asks Margins to understand a notes practice. It follows
`margins guide workspace-setup` and helps the user decide, in ordinary language:

- which notes Margins may search;
- where an approved connected note may be written; and
- how to prove setup by finding something already in those notes.

It must not start processing the current meeting or draft a connected note as
part of setup.

## Realtime and failure behavior

Host signals and bb realtime messages mean only “something changed.” They do
not carry transcript or memo content. The panel always refetches a snapshot and
also polls while a meeting is active.

Missing, stopped, and outdated runtime states are internal preparation details,
not separate happy-path buttons. The panel distinguishes user-actionable failures:

- recording Mac offline;
- unsupported capture platform;
- plugin/runtime authentication failure;
- microphone permission;
- system-audio permission; and
- another interruption that can be retried.

Each failure says what could not happen, whether a recording or already-saved
notes are safe, and one next action. Before Pause, Stop, or Add `@Margins`, the
panel first commits any dirty notepad revision so the action cannot outrun the
500 ms background save.

A missing runtime on one Mac does not disable the plugin everywhere.

## Runtime shell

The runtime has no required product UI of its own. It may show the smallest
native permission prompt the operating system requires, but recording controls,
the listening waveform, and the notepad belong in bb for this integration.

The earlier menu-bar implementation has been removed. The desktop process is a
temporary composition adapter, not a commitment to a tray-resident Margins app.

## Acceptance checks

The vertical slice is ready when:

- plugin app, server, and host bundles build with the current bb SDK;
- plugin tests cover the panel, server routing/storage, mention resolution,
  host RPC, discovery/auth requests, pinned release checks, and runtime start;
- Rust tests cover the public JSON shapes, auth, route behavior, recent-line
  limits, operation replay, generation/revision checks, discovery permissions,
  and shared terminal/bb notepad timing;
- a Rust-emitted golden V1 fixture is accepted by the plugin's Zod schemas;
- portable Margins tests pass without native audio or macOS frameworks;
- no plugin test installs or reloads the live bb instance;
- the runtime trait compiles independently of Tauri and native audio; and
- the remaining macOS pass verifies the `margins-live` binary, audio permission
  flow, recording transitions, signing, and notarization without a menu-bar UI.

## Verification on 2026-09-04

The existing Margins setup path was exercised against the real headless backend
and frontend with `agent-browser`. The **Agent setup** action installed the
normal `margins` command into a hermetic home and produced the expected
`margins guide workspace-setup` prompt without changing the developer's existing
command.

The plugin was installed into a separate bb development instance and exercised
in real desktop and 390 px thread panels. One **Start recording** click moved
immediately through **Preparing recording** to **Listening**. A fixture permission
denial remained visible as **Needs attention** through realtime/refetch, retry
reached Listening, and text entered immediately before Stop was stored before
the stop request. Stop returned and persisted its session identity as **Meeting
saved**. **Make connected note** placed an explicit-session request in the
focused composer without sending it. Refresh preserved both the saved handoff
and the unsent composer draft.

A separate isolated run removed the direct development API and let the bb host
worker start a compatible detached `margins-live` fixture. The worker checked
the runtime capability, discovered its private V1 endpoint, and returned the
panel to **Margins is ready**. This verifies process startup and discovery, not
the unreleased `v0.4.9` download or native audio capture.

## Deliberate limits and follow-ups

- The first plugin uses the thread environment's Mac by default and accepts a
  recording-host ID in settings. A friendly connected-Mac picker is the clearest
  next bb UI improvement.
- `margins-live` now composes the native recorder as a windowless background
  process. The remaining product gate is a signed, notarized Apple Silicon
  release plus a native microphone/system-audio and permission smoke pass.
- The live snapshot is intentionally recent and fixed-size. Browsing old
  meetings and creating connected notes remain Margins workflows, not plugin
  panel features.
- The companion plugin currently lives in this repository under
  `integrations/bb-plugin-margins`; packaging and marketplace publication are
  separate release work.
