import { pathToFileURL } from "node:url";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { MarginsService } from "./service.mjs";

const sessionId = z.string().min(1).max(200).regex(/^[A-Za-z0-9_-]+$/);
const dataOutput = { data: z.unknown() };
const readOnly = { readOnlyHint: true, destructiveHint: false, openWorldHint: false };
const writable = { readOnlyHint: false, destructiveHint: true, openWorldHint: false };

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
    instructions: "Margins recordings belong to the configured Workspace. Read sessions and memo before editing. save_memo replaces the complete plain-text memo using its expected revision. The Mac menu app owns audio capture; these tools do not start or stop it.",
  });

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

  register(server, "read_transcript", {
    title: "Read a Margins transcript",
    description: "Read the transcript for one exact saved meeting ID from list_meetings.",
    inputSchema: { sessionId }, annotations: readOnly,
  }, (service, { sessionId }) => service.transcript(sessionId), serviceFactory);

  register(server, "read_memo", {
    title: "Read a Margins memo",
    description: "Read the editable memo and its revision for one exact meeting ID.",
    inputSchema: { sessionId }, annotations: readOnly,
  }, (service, { sessionId }) => service.memo(sessionId), serviceFactory);

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
