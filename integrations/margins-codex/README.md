# Margins for Codex

This local Codex plugin connects to one existing Margins Workspace service. It
lets Codex check transcription readiness, list meetings, read a transcript,
open an editable memo pad, follow one meeting's status and transcript in a
chat-bound view, and save the complete plain-text memo with a revision check. The
Mac menu app still owns microphone and computer-audio recording. The plugin
does not read audio or hold macOS capture permission.

## Connect one Workspace

Install dependencies in this directory with `npm ci`. Set these variables in
the environment that starts Codex:

- `MARGINS_CODEX_URL`: HTTPS service origin, or loopback HTTP origin.
- `MARGINS_CODEX_WORKSPACE`: the exact Workspace ID served at that origin.
- `MARGINS_CODEX_TOKEN_FILE`: path to an existing scoped service bearer file.
  `MARGINS_CODEX_TOKEN` is also accepted for an environment-managed credential.

The plugin manifest passes only these variable names to its local MCP server;
it contains no credentials. The server checks the service's instance and
Workspace before reading or writing a session. A memo save requires the exact
revision returned by `read_memo`. For a saved meeting, the service supplies the
capture duration as the memo timestamp. A live memo edit requires an explicit
`observedAtMs` from the recording timeline.

The Codex plugin source lives here under `integrations/margins-codex`. A Codex
session must load this package in its environment before its tools appear;
existing sessions do not gain newly installed tools automatically.

Call `open_memo_pad` with a saved meeting ID to show the editor in an MCP Apps
compatible host. The widget uses `save_memo` over the host's MCP bridge, so the
service token stays in the server process. A stale revision leaves the draft in
place; Reload asks before discarding it. Hosts that do not render MCP Apps UI
still receive the memo and revision in the tool result and can use `save_memo`.
Live meeting memo editing remains available through `save_memo` with a timeline
offset, but the widget currently opens saved meetings only.

Use `find_current_meeting` or `list_meetings` to find the intended session ID,
then `open_live_meeting` to keep a view pinned to that one session. It polls the
Workspace for capture-finalization status and any available transcript. The
floating-view button requests picture-in-picture when the host supports it.
`find_current_meeting` reads Workspace-wide open producer reservations; when
several exist, it returns candidate IDs and leaves selection to the user.
An open reservation is not proof that its Mac recorder is still healthy.
This is a read-only view: the Mac menu app still starts, pauses, and stops
recording. With Mac CoreML assets available, the native helper may publish
provisional words while audio uploads. Linux begins ONNX transcription after
Stop and replaces that draft with the final transcript. If CoreML is unavailable,
words may first appear after Stop. The view does not show Mac
microphone/system sample counters because those live in the Mac bridge process.

## Verify

Run `npm test` for the MCP protocol, Workspace authorization, and widgets'
interaction fixtures. The tests use a fake service and do not change any real memo. For a read-only live
check, configure the variables above, then ask Codex to check the Margins
recording service and list recent meetings.

## Product boundary

The MCP tools work without a custom component. The widgets use
the MCP Apps UI resource protocol. Whether a particular Codex client renders
that resource must be checked in that client. Pairing/setup UI, model
downloads, live recording controls, and rolling transcript production remain
future integration work.
