import assert from "node:assert/strict";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { MarginsService } from "../src/service.mjs";
import { createServer } from "../src/server.mjs";

const sessionId = "remote-2026-09-23-test";

function fixture() {
  const calls = [];
  const fetchImpl = async (url, options) => {
    const parsed = new URL(url);
    const body = options.body && JSON.parse(options.body);
    calls.push({ path: parsed.pathname + parsed.search, options, body });
    assert.equal(options.headers.authorization, "Bearer scoped-token");
    let result;
    if (parsed.pathname === "/v1/capabilities") {
      result = { workspace_id: "journal", instance_id: "instance", asr_available: true };
    } else {
      assert.equal(options.headers["X-Margins-Instance-Id"], "instance");
      if (parsed.pathname === "/v1/workspaces/journal/sessions") {
        result = { sessions: [{ session_id: sessionId, input_finalized: true, capture_duration_ms: 1200 }], next_cursor: null };
      } else if (parsed.pathname === "/v1/workspaces/journal/active-sessions") {
        result = { sessions: [{ session_id: sessionId, input_finalized: false, segment_count: 0 }], next_cursor: null };
      } else if (parsed.pathname === "/v1/workspaces/journal/current") {
        result = sessionId;
      } else if (parsed.pathname === `/v1/workspaces/journal/sessions/${sessionId}`) {
        result = { session_id: sessionId, input_finalized: true, capture_duration_ms: 1200 };
      } else if (parsed.pathname.endsWith("/memo") && options.method === "GET") {
        result = { session_id: sessionId, revision: "rev-1", lines: [{ text: "first" }] };
      } else if (parsed.pathname.endsWith("/memo") && options.method === "PUT") {
        assert.equal(body.expected_revision, "rev-1");
        assert.equal(body.observed_at_ms, 1200);
        assert.equal(body.paused, true);
        assert.equal(body.text, "edited");
        result = { session_id: sessionId, revision: "rev-2", lines: [{ text: "edited" }] };
      } else if (parsed.pathname.endsWith("/transcript")) {
        result = { session_id: sessionId, body: "system and mic words", terminal: true };
      } else throw new Error(`unexpected fixture path: ${parsed.pathname}`);
    }
    return new Response(JSON.stringify({ ok: true, result }), { headers: { "content-type": "application/json" } });
  };
  return { service: new MarginsService({ url: "http://127.0.0.1:18765", workspace: "journal", token: "scoped-token", fetchImpl }), calls };
}

test("service accepts only HTTPS or loopback and checks Workspace identity", async () => {
  assert.throws(() => new MarginsService({ url: "http://example.com", workspace: "journal", token: "x" }), /HTTPS or loopback/);
  assert.throws(() => new MarginsService({ url: "https://example.com/other", workspace: "journal", token: "x" }), /only an origin/);
  const service = new MarginsService({
    url: "http://localhost:1", workspace: "wrong", token: "x",
    fetchImpl: async () => new Response(JSON.stringify({ ok: true, result: { workspace_id: "journal", instance_id: "instance" } })),
  });
  await assert.rejects(service.capabilities(), /does not match/);
});

test("Codex tools read one Workspace and save a revisioned memo", async () => {
  const { service, calls } = fixture();
  const server = createServer(() => service);
  const client = new Client({ name: "margins-test", version: "0.1.0" }, { capabilities: {} });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  try {
    const names = (await client.listTools()).tools.map((tool) => tool.name);
    assert.deepEqual(names.sort(), ["find_current_meeting", "list_meetings", "open_live_meeting", "open_memo_pad", "read_live_meeting", "read_memo", "read_transcript", "recording_service_status", "save_memo"].sort());
    const openTool = (await client.listTools()).tools.find(tool => tool.name === "open_memo_pad");
    assert.equal(openTool._meta.ui.resourceUri, "ui://margins/memo-pad-v2.html");
    const widget = await client.readResource({ uri: openTool._meta.ui.resourceUri });
    assert.equal(widget.contents[0].mimeType, "text/html;profile=mcp-app");
    assert.match(widget.contents[0].text, /Save memo/);
    const meetings = await client.callTool({ name: "list_meetings", arguments: {} });
    assert.equal(meetings.structuredContent.data.sessions[0].session_id, sessionId);
    const current = await client.callTool({ name: "find_current_meeting", arguments: {} });
    assert.equal(current.structuredContent.data.current_session_id, sessionId);
    const live = await client.callTool({ name: "open_live_meeting", arguments: { sessionId } });
    assert.equal(live.structuredContent.data.session_id, sessionId);
    const liveTool = (await client.listTools()).tools.find(tool => tool.name === "open_live_meeting");
    assert.equal(liveTool._meta.ui.resourceUri, "ui://margins/live-meeting-v1.html");
    const liveResource = await client.readResource({ uri: liveTool._meta.ui.resourceUri });
    assert.equal(liveResource.contents[0].mimeType, "text/html;profile=mcp-app");
    const memo = await client.callTool({ name: "read_memo", arguments: { sessionId } });
    assert.equal(memo.structuredContent.data.revision, "rev-1");
    const opened = await client.callTool({ name: "open_memo_pad", arguments: { sessionId } });
    assert.equal(opened.structuredContent.data.revision, "rev-1");
    const saved = await client.callTool({ name: "save_memo", arguments: { sessionId, expectedRevision: "rev-1", text: "edited" } });
    assert.equal(saved.structuredContent.data.revision, "rev-2");
    assert.equal(calls.filter((call) => call.options.method === "PUT").length, 1);
    const transcript = await client.callTool({ name: "read_transcript", arguments: { sessionId } });
    assert.equal(transcript.structuredContent.data.terminal, true);
  } finally {
    await client.close();
    await server.close();
  }
});

test("live meeting reports a pending transcript without claiming terminal text", async () => {
  const service = new MarginsService({ url: "http://127.0.0.1:18765", workspace: "journal", token: "scoped-token" });
  service.exactSummary = async () => ({ session_id: sessionId, input_finalized: false, segment_count: 1, processing_state: "none" });
  service.transcript = async () => { throw new Error("No aligned transcript or capture context found"); };
  const state = await service.liveMeeting(sessionId);
  assert.equal(state.input_finalized, false);
  assert.equal(state.segment_count, 1);
  assert.equal(state.transcript, null);
});
