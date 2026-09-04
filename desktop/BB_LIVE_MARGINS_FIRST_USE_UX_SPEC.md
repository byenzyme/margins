# Margins in bb: first use and handoff UX

Status: proposed implementation brief, 2026-09-04.

This spec narrows first use to the intention the user already has: record this
meeting. Runtime installation, startup, host routing, and readiness are
supporting work. They must not become a setup product of their own.

It also closes the current post-stop gap. Capture and distillation remain
separate operations, but the user should be able to move from one to the other
without learning Margins' internal lifecycle.

## User promise

A meeting-rushed user can open **Margins live**, press **Start recording**,
approve any required macOS prompt, and return attention to the meeting. When
they stop, Margins says what is safe and offers one optional path to turn the
latest capture into a connected note.

The only privacy model needed in the capture path is:

> Audio and transcript stay on this Mac. Meeting context enters the thread only
> when you add `@Margins`.

The note-making boundary is disclosed later, at the point where it becomes
relevant.

## Product boundaries

- Keep one thread-side panel. Do not add a setup wizard, nav page, menu-bar UI,
  or permanent post-meeting dashboard.
- Do not ask for a host, meeting name, workspace, note destination, model,
  output format, or source selection on the happy capture path.
- Default to the thread environment's connected Mac. Ask the user to choose a
  recording Mac only after that default is unavailable or genuinely ambiguous.
- Derive the meeting name from the thread as today.
- Capture must not depend on note-making AI, a configured Workspace, or cloud
  authentication.
- Workspace setup and distillation remain separate contracts. They may feel
  like one continuous user request, but setup must finish and be reviewed
  before connected-note distillation begins.
- Do not automatically send meeting material to an agent or model. `@Margins`
  and **Make connected note** are explicit user actions.

## Canonical user-visible lifecycle

```text
Ready
  -> Preparing recording
  -> Live <-> Paused
  -> Saving meeting
  -> Meeting saved
  -> Ready

Any failed transition -> Needs attention -> retry the intended action
```

Runtime states such as missing, installed, stopped, starting, and outdated are
internal substates of **Preparing recording**. They should appear only when
they explain real waiting or a recovery action.

### 1. Ready

The normal and first-run panel has one primary action:

**Start recording**

Before the first successful recording only, show the two-line privacy promise
near the action. If the plugin knows it must install the local recorder, one
short subordinate sentence may say:

> First use prepares a private recorder on this Mac.

Do not lead with **Install Margins** or **Start Margins**. Pressing **Start
recording** is the intent that authorizes deterministic preparation. If a
future download is large enough to require separate informed consent, disclose
its size at this point without introducing configuration choices.

### 2. Preparing recording

The click must acknowledge immediately, before host/runtime work completes.
Replace the primary action with a quiet non-interactive state such as:

- **Preparing recorder** while verifying/installing/starting the runtime;
- **Waiting for microphone access** only after macOS permission is actually
  requested; or
- **Starting recording** after the runtime accepts the operation.

Use truthful stages, not a percentage or an indefinitely spinning primary
button. The stage may change as deterministic work advances. Preserve the
user’s place and do not open another panel.

If the runtime snapshot itself says `starting`, every panel renders
**Preparing recording** and keeps polling. It offers neither recording controls
nor `@Margins` until the snapshot reaches Recording or Paused.

Behind this transition the plugin may:

1. resolve the thread's recording host;
2. verify or install the pinned signed runtime;
3. start it and wait for private discovery;
4. request capture from that runtime; and
5. attach the resulting meeting to the thread.

If a prior recording is already live on the selected Mac, do not start a
second one. Attach it and enter **Live**.

### 3. Permission boundary

The signed runtime owns macOS microphone and system-audio consent. The panel
should prepare the user, while the operating system owns the actual prompt.

Permission recovery must distinguish:

- microphone access;
- system-audio access;
- plugin/runtime authentication;
- unsupported capture capability; and
- an offline recording Mac.

Do not map all of these to **Microphone access needed**. Each failure names the
blocked capability, says whether a recording was created, and provides one
recommended action.

An unsupported host has no retry control because retrying cannot change its
capability. Its only recovery is to open the thread on an Apple silicon Mac.

### 4. Live and Paused

Use the compact waveform/control row and continuously editable notepad already
implemented. Setup and privacy explanations disappear after recording starts.

- The notepad remains the dominant surface.
- The waveform is a compact state mark, not fabricated audio amplitude. Keep it
  still unless the runtime later supplies a real level signal; state changes may
  use one short acknowledgment that also respects reduced motion.
- Margins assigns hidden meeting timestamps and saves after a short pause.
- Before Pause, Stop, or `@Margins`, flush any dirty notepad text or make the
  pending save part of that operation. A user must not lose the last sentence
  by acting faster than the autosave timer.
- **Add @Margins** continues to insert the current meeting into the composer;
  it does not send.

### 5. Saving meeting

Stop is a transition, not an immediate return to Ready.

1. Flush the current notepad revision.
2. Ask the runtime to stop and finalize.
3. Preserve the returned stopped meeting identity.
4. Show what is already safe before mentioning remaining background work.

If final transcription/alignment continues after audio capture stops, say so
truthfully. Never imply the recording or notes were lost because later
processing failed.

### 6. Meeting saved

Show this transient completion state after a successful stop:

> **Meeting saved on this Mac**
>
> Your recording and notes are safe. Margins can turn them into a connected
> note when you are ready.

Primary action: **Make connected note**

Secondary action: **Not now** or a quiet route back to **Start recording**.

This is not a history or review surface. Persist only enough stopped-meeting
identity to survive a refresh and support the handoff. That identity remains
internal: raw meeting IDs and slugs never appear in user-visible panel or
composer copy. Once dismissed, or once a new recording begins, the panel can
return to Ready.

## Distillation handoff

**Make connected note** should place a plain-language request in the current
thread composer and focus it. The request refers naturally to the meeting the
user just recorded; Margins' canonical latest-session-first flow resolves the
actual session after the user sends it.

The action does not silently start an agent turn. Sending remains the user's
choice.

Suggested intent, not required literal copy:

> Turn the Margins meeting I just recorded into a connected note.

### First note only: setup through doing

If no reviewed Workspace exists, the setup skill performs read-only discovery
after the user sends the note-making request. In the background it may scan the
available notes, infer conventions, identify relevant prior notes, and propose
the smallest read/write boundary.

The user should review one compact proposal framed as outcomes:

> Margins may search these notes and write connected meeting notes here.

The reviewed workspace plan is applied unchanged. Only then does the separate
distillation workflow resolve the selected/latest meeting and make the note.

Subsequent meetings reuse that approved boundary without replaying onboarding.

### Note-making privacy boundary

At the first action that would send content to a note-making AI, disclose what
will move and why. This belongs here, not before local recording:

> To make the note, Margins sends the transcript, your notes, and selected
> related-note excerpts to your chosen note-making AI.

Local capture must remain usable if the user declines or has not configured
note-making AI.

## Deferred until needed

Keep these out of first recording:

- Workspace and vault configuration;
- source folders and write destination;
- recall proof;
- note templates, frontmatter, tags, and people metadata;
- model/provider sign-in for connected-note generation;
- advanced audio devices and transcription model selection;
- runtime version, install path, discovery endpoint, and host ID;
- changing the recording Mac when the default works.

Perform deterministic discovery, finalization, alignment, indexing, and
readiness checks in the background. Ask only at a consent boundary, an
ambiguous choice, or a failure that cannot be recovered safely.

## Failure principles

Every failure answers three questions:

1. What could not happen?
2. Was the recording/notepad preserved?
3. What is the one recommended next action?

Do not collapse unsupported platform, offline host, runtime authentication, and
macOS audio permission into the same state. Keep technical detail behind an
optional disclosure or plugin logs.

## Acceptance claims

1. **Meeting-rushed start**
   - Claim: a first-time user can begin capture from the panel without choosing
     configuration or understanding runtime installation.
   - Pass evidence: one visible primary **Start recording** action; the first
     click advances immediately into a truthful preparation state; success
     reaches Live without a second plugin button.
   - False if: the user must click **Install Margins**, **Start Margins**, pick a
     host, or open Settings before a normal first capture.

2. **Privacy comprehension**
   - Claim: a privacy-conscious user can distinguish local capture from context
     intentionally shared with bb/AI.
   - Pass evidence: the two-line local/`@Margins` boundary is visible before
     first capture, and the note-making AI boundary appears only at note-making.
   - False if: copy merely says "private" or implies that local capture needs
     cloud AI.

3. **Attention preservation**
   - Claim: installation/startup cannot steal the user's live moment.
   - Pass evidence: immediate action acknowledgment, monotonic human stages,
     no configuration decision, and no modal except the OS-owned permission
     prompt.
   - False if: a naked spinner persists, stages move backward without reason,
     or the user is sent to another surface.

4. **Notepad continuity**
   - Claim: the final text typed before Pause, Stop, or `@Margins` survives and
     is used by that action.
   - Pass evidence: tests exercise actions before the debounce expires and the
     resulting runtime snapshot/export contains the text.
   - False if: those actions observe only the prior saved revision.

5. **Post-stop continuity**
   - Claim: after Stop, the user can tell the meeting is safe and can initiate
     connected-note work without searching for it.
   - Pass evidence: a durable-through-refresh Meeting saved state uses the
     returned stopped meeting identity and offers Make connected note.
   - False if: the panel immediately returns to generic Ready or restores a
     live-only `@Margins` action.

## Required verification

- Update the product/technical spec so its state table and local API match the
  implemented notepad and first-use flow.
- Unit-test the single Start path across ready runtime, stopped runtime,
  missing runtime, existing live meeting, permission denial, unsupported host,
  and host offline.
- Unit-test dirty-notepad Pause, Stop, and `@Margins` behavior before debounce.
- Unit-test stopped identity and Meeting saved persistence through panel
  refetch/realtime invalidation.
- Exercise the complete isolated browser journey at desktop and phone width:
  first open -> Start recording -> preparation -> permission/recovery fixture ->
  Live -> type without waiting -> Stop -> Meeting saved -> Make connected note.
- Capture screenshots or visible-text evidence for first Ready, preparation,
  Live, Needs attention, and Meeting saved.
- Review the evidence with the falsifiable claims above. Record at least one
  risk or failure; do not let the implementation author self-certify taste.
