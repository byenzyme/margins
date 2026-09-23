# Margins for Codex

This local Codex plugin connects to one existing Margins Workspace service. It
lets Codex check transcription readiness, list meetings, read a transcript,
read a memo, and save the complete plain-text memo with a revision check. The
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

The Codex plugin source lives here under `integrations/margins-codex`. It is not
installed into a personal marketplace or published. A Codex session must load
this package in its environment before its tools appear; existing sessions do
not gain newly installed tools automatically.

## Verify

Run `npm test` for the MCP protocol and Workspace authorization fixture. The
tests use a fake service and do not change any real memo. For a read-only live
check, configure the variables above, then ask Codex to check the Margins
recording service and list recent meetings.

## Product boundary

This is the tool-backed Codex integration. The five tools work without a custom
component. An embedded editable memo widget is not included; current official
OpenAI documentation describes the MCP Apps component path for ChatGPT and
compatible hosts, and does not establish that this Codex surface renders it.
Pairing/setup UI, model downloads, and live recording controls remain future
integration work.
