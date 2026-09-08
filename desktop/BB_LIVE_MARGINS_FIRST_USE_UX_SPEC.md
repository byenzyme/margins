# Margins in bb: first use, capture, and handoff

Status: user-journey brief, revised 2026-09-06.

## User promise

A person about to join a meeting can open Margins in bb, understand exactly
what this device will hear and where it will be saved, then start with one
action. They can leave the panel or change threads without ending capture. If
this bb window disappears long enough, Margins stops and saves what reached the
project.

## The first screen

The panel leads with the source and destination, then the action.

For a browser or phone that can record:

> **Ready · Microphone only**
> Recording and notes are saved in this bb project.
> **Start**

For a Mac that has not been enabled:

> **Enable recording on this Mac**
> Margins can hear you and conversations playing on this Mac. Recording starts
> only when you press Start. If this bb window stays disconnected, Margins stops
> and saves to this project.

No first-use screen mentions installation or process topology. If the current
bb build cannot connect to Mac-local capture, it says setup is not available
yet and offers no control that can only fail.

## State and action flow

| State | What it means | Primary action |
| --- | --- | --- |
| Checking | Client and project capability are being read | none |
| Needs setup | Mac capture is not yet enabled in this bb window | Enable recording on this Mac, only when supported |
| Ready | The named source can start and the project destination exists | Start |
| Getting ready | Permission, capture producer, or project session is not complete | none; Cancel only when real cancellation is supported |
| Recording | Audio is flowing from this window and acknowledged by the project | Pause |
| Paused | Audio production and project state are paused | Resume |
| Recovering connection | The owner is inside the 25-second reconnect grace | none; keep the user's text |
| Saving | Local capture has ended; received data is being finalized | none |
| Saved | The existing Margins audio meeting is durable | Make connected note or Not now |
| Needs attention | A specific transition failed | one truthful recovery, or none if unsupported |

Start acknowledges immediately with **Getting ready**, but it never claims
recording until both ends exist. No spinner, fake waveform, percentage, or
elapsed timer is required. A static waveform glyph may identify audio; only a
real amplitude meter may move.

## During the meeting

The recording controls stay small. The notepad owns most of the panel and keeps
focus while status refreshes. Every visible line is editable. Newlines become
quiet time anchors in the existing Margins memo model; timestamps are not shown.

The latest notepad text is flushed before Pause, Stop, and Add `@Margins`.
Conflicts preserve the local draft and ask the user to reload or retry; polling
never replaces text under the cursor.

Collapsing the panel or opening another thread does not end capture because the
bb-window content script, not the React panel, owns the audio stream. Closing or
reloading the whole window stops local tracks. A reload using the same tab
identity can reclaim the project-side capture inside the 25-second grace.

## Disconnect

The first missed heartbeat changes the owning client to **Recovering
connection**. Already acknowledged audio and notes are safe on the project
machine. If the connection returns inside 25 seconds, capture continues with
the same identity and no duplicate meeting.

After the grace expires, the project side finalizes exactly once. A later client
sees the saved meeting rather than a still-recording fiction. If finalization
fails, the failure says that received audio is retained and gives one retry.

## Stop and handoff

Stop performs these observable steps:

1. flush the current notepad text;
2. end the device audio producer and drain its acknowledged chunks;
3. show **Saving** while the project creates the normal Margins audio session;
4. show **Saved** only after that session is durable.

The saved state says:

> Recording and notes are saved in this bb project. A transcript may appear
> after Margins processes the audio.

**Make connected note** appends this natural request to the current composer
and focuses it:

> Turn the Margins meeting I just recorded into a connected note.

It does not send. The later skill resolves the latest meeting in the project.
Raw IDs, paths, and processing terms stay out of user-visible copy.

## Privacy and consent

Required truthful statements:

- capture starts only after Start;
- the visible source label names what the current device can hear;
- browser/mobile mode is microphone-only;
- audio and notes are sent to and saved on the bb project's source machine;
- meeting evidence enters an agent turn only after the user adds `@Margins` or
  sends a connected-note request.

Forbidden statements include “stays on this Mac” and any implication that a
phone captures another app's internal audio.

## Mobile

On supported mobile browsers the same panel says **Microphone only**. Controls
remain visually small but have at least 40px coarse-pointer hit targets. The
notepad remains the dominant surface. Background capture is bounded by browser
and PWA lifecycle rules: the UI must not promise continued recording after the
browser suspends or kills the page.

Newer bb SDKs provide an experimental app-overlay contribution that can host a
client-global recording status. The plugin still needs a separately tested SDK
migration before it can claim that return path. Capture continuity and control
discoverability remain separate until then.

## Falsifiable UX claims

- A meeting-rushed browser user can identify source, destination, and Start in
  under three seconds because they are the only top-row claims and action.
  False if setup or architecture appears first.
- An in-meeting user can keep typing through status refreshes because the
  notepad dominates and polling preserves draft, selection, and focus. False if
  text jumps or controls compete with the editor.
- A privacy-conscious user can explain where audio goes because the panel says
  it is saved to this bb project. False if any copy says it stays on the device.
- A returning user can trust Saved because it is shown only after existing
  Margins session finalization succeeds. False if Stop immediately collapses to
  Ready or promises a transcript.

Known risk: the current plugin is pinned below the SDK version that adds an
experimental app overlay. The closed-panel journey remains incomplete until
that surface is adopted and verified on desktop and mobile.
