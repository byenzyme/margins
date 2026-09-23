import { pathToFileURL } from "node:url";
import { readFileSync } from "node:fs";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { MarginsService } from "./service.mjs";

const sessionId = z.string().min(1).max(200).regex(/^[A-Za-z0-9_-]+$/);
const dataOutput = { data: z.unknown() };
const readOnly = { readOnlyHint: true, destructiveHint: false, openWorldHint: false };
const writable = { readOnlyHint: false, destructiveHint: true, openWorldHint: false };
const memoPadUri = "ui://margins/memo-pad-v2.html";
const memoPadHtml = readFileSync(new URL("../web/memo-pad.html", import.meta.url), "utf8");
const liveMeetingUri = "ui://margins/live-meeting-v1.html";
const liveMeetingHtml = readFileSync(new URL("../web/live-meeting.html", import.meta.url), "utf8");

function result(data) {
  return { structuredContent: { data }, content: [{ type: "text", text: JSON.stringify(data) }] };
}

function register(server, name, config, execute, serviceFactory) {
  server.registerTool(name, { ...config, outputSchema: dataOutput }, async (input) => {
    try { return result(await execute(serviceFactory(), input)); }
    catch (error) {
      return { isError: true, content: [{ type: "text", text: error instanceof Error ? error.message : "Margins request failed" }] };
    }
  });
}

export function createServer(serviceFactory = () => MarginsService.fromEnv()) {
  const server = new McpServer({ name: "margins-codex", version: "0.1.0" }, {
    instructions: "Margins recordings belong to the configured Workspace. Read sessions and memo before editing. Use open_memo_pad for an editable saved-meeting memo or open_live_meeting for a chat-bound status view of one exact session when the host supports MCP Apps UI. The remote native capture path publishes transcript words after finalization, not during recording. save_memo replaces the complete plain-text memo using its expected revision. The Mac menu app owns audio capture; these tools do not start or stop it.",
  });

  server.registerResource("margins-memo-pad", memoPadUri, {}, async () => ({
    contents: [{ uri: memoPadUri, mimeType: "text/html;profile=mcp-app", text: memoPadHtml,
      _meta: { ui: { prefersBorder: true } } }],
  }));
  server.registerResource("margins-live-meeting", liveMeetingUri, {}, async () => ({
    contents: [{ uri: liveMeetingUri, mimeType: "text/html;profile=mcp-app", text: liveMeetingHtml,
      _meta: { ui: { prefersBorder: true } } }],
  }));

  register(server, "recording_service_status", {
    title: "Check Margins recording service",
    description: "Check the connected Workspace identity and whether server-side ASR is available.",
    inputSchema: {}, annotations: readOnly,
  }, (service) => service.capabilities(), serviceFactory);

  register(server, "list_meetings", {
    title: "List Margins meetings",
    description: "Find recent recordings in the configured Margins Workspace.",
    inputSchema: { limit: z.number().int().min(1).max(100).optional() }, annotations: readOnly,
  }, (service, { limit }) => service.sessions(limit ?? 20), serviceFactory);

  register(server, "find_current_meeting", {
    title: "Find current Margins meeting",
    description: "Find an open capture in the configured Workspace, including before its first segment closes. Returns its ID when unambiguous, otherwise null with candidate IDs. This is not a recording-health signal.",
    inputSchema: {}, annotations: readOnly,
  }, (service) => service.currentSession(), serviceFactory);

  register(server, "read_transcript", {
    title: "Read a Margins transcript",
    description: "Read the transcript for one exact saved meeting ID from list_meetings.",
    inputSchema: { sessionId }, annotations: readOnly,
  }, (service, { sessionId }) => service.transcript(sessionId), serviceFactory);

  register(server, "read_live_meeting", {
    title: "Read Margins meeting status",
    description: "Read capture-finalization status and available transcript for one exact meeting ID. During native remote recording, audio uploads live but transcription starts after Stop.",
    inputSchema: { sessionId }, annotations: readOnly,
  }, (service, { sessionId }) => service.liveMeeting(sessionId), serviceFactory);

  register(server, "open_live_meeting", {
    title: "Open Margins live meeting view",
    description: "Show a chat-bound, refreshing status and transcript view for one exact meeting ID. First use list_meetings to select the intended session. Recording remains controlled by the Mac menu app; remote transcript words appear after Stop.",
    inputSchema: { sessionId }, annotations: readOnly,
    _meta: { ui: { resourceUri: liveMeetingUri }, "openai/outputTemplate": liveMeetingUri },
  }, (service, { sessionId }) => service.liveMeeting(sessionId), serviceFactory);

  register(server, "read_memo", {
    title: "Read a Margins memo",
    description: "Read the editable memo and its revision for one exact meeting ID.",
    inputSchema: { sessionId }, annotations: readOnly,
  }, (service, { sessionId }) => service.memo(sessionId), serviceFactory);

  register(server, "open_memo_pad", {
    title: "Open Margins memo pad",
    description: "Show the editable memo pad for one exact saved meeting ID. For hosts without MCP Apps UI, returns the memo and revision as data.",
    inputSchema: { sessionId }, annotations: readOnly,
    _meta: { ui: { resourceUri: memoPadUri }, "openai/outputTemplate": memoPadUri },
  }, async (service, { sessionId }) => {
    const summary = await service.summary(sessionId);
    if (!summary) throw new Error("Meeting not found among the 100 most recent sessions");
    if (!summary.input_finalized) throw new Error("The memo pad currently supports saved meetings only");
    const memo = await service.memo(sessionId);
    return { ...memo, session_id: sessionId, capture_duration_ms: summary.capture_duration_ms ?? 0 };
  }, serviceFactory);

  register(server, "save_memo", {
    title: "Save a Margins memo",
    description: "Replace the complete plain-text memo for one exact meeting ID. Pass the revision returned by read_memo; a stale revision is rejected. For a live meeting, pass its observed elapsed milliseconds.",
    inputSchema: {
      sessionId,
      expectedRevision: z.string().min(1),
      text: z.string().max(100_000),
      observedAtMs: z.number().int().min(0).optional(),
      paused: z.boolean().optional(),
    }, annotations: writable,
  }, async (service, { sessionId, expectedRevision, text, observedAtMs, paused }) => {
    let offset = observedAtMs;
    let memoPaused = paused ?? false;
    if (offset === undefined) {
      const summary = await service.summary(sessionId);
      if (!summary) throw new Error("Meeting not found among the 100 most recent sessions");
      if (!summary.input_finalized) throw new Error("A live memo needs observedAtMs from the recording timeline");
      offset = summary.capture_duration_ms ?? 0;
      memoPaused = true;
    }
    return service.saveMemo({ sessionId, expectedRevision, text, observedAtMs: offset, paused: memoPaused });
  }, serviceFactory);

  return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const server = createServer();
  await server.connect(new StdioServerTransport());
}
