import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { JSDOM } from "jsdom";

const html = readFileSync(new URL("../web/memo-pad.html", import.meta.url), "utf8");
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

function host({ toolOutput } = {}) {
  const calls = [];
  const dom = new JSDOM(html, { runScripts: "dangerously", url: "https://widget.invalid",
    beforeParse(window) {
      window.postMessage = message => { calls.push(message); };
      if (toolOutput) window.openai = { toolOutput };
    },
  });
  const { window } = dom;
  const notify = data => window.dispatchEvent(new window.MessageEvent("message", {
    source: window, data: { jsonrpc: "2.0", method: "ui/notifications/tool-result", params: { structuredContent: { data } } },
  }));
  const reply = (request, result) => window.dispatchEvent(new window.MessageEvent("message", {
    source: window, data: { jsonrpc: "2.0", id: request.id, result },
  }));
  const get = id => window.document.getElementById(id);
  const toolCalls = () => calls.filter(call => call.method === "tools/call");
  return { dom, window, calls, notify, reply, get, toolCalls };
}

test("memo widget initializes before receiving its first tool result", async () => {
  const h = host();
  try {
    assert.equal(h.calls[0].method, "ui/initialize");
    assert.equal(h.calls[0].params.protocolVersion, "2026-01-26");
    h.reply(h.calls[0], { hostCapabilities: {}, hostInfo: { name: "test", version: "1" } });
    await tick();
    assert.equal(h.calls[1].method, "ui/notifications/initialized");
    h.notify({ session_id: "remote-test", revision: "rev-1", lines: [{ text: "first" }] });
    assert.equal(h.get("memo").disabled, false);
    assert.equal(h.get("memo").value, "first");
  } finally { h.window.close(); }
});

test("memo widget hydrates preloaded ChatGPT tool output", async () => {
  const h = host({ toolOutput: { data: { session_id: "remote-test", revision: "rev-1", lines: [{ text: "preloaded" }] } } });
  try {
    h.reply(h.calls[0], { hostCapabilities: {} });
    await tick();
    assert.equal(h.get("memo").value, "preloaded");
    assert.equal(h.get("memo").disabled, false);
  } finally { h.window.close(); }
});

test("memo widget saves through MCP bridge with exact meeting and revision", async () => {
  const h = host();
  try {
    h.notify({ session_id: "remote-test", revision: "rev-1", lines: [{ text: "first" }] });
    assert.equal(h.get("memo").value, "first");
    assert.equal(h.get("save").disabled, true);
    h.get("memo").value = "edited";
    h.get("memo").dispatchEvent(new h.window.Event("input"));
    h.get("save").click();
    assert.equal(h.toolCalls()[0].method, "tools/call");
    assert.deepEqual(JSON.parse(JSON.stringify(h.toolCalls()[0].params)), {
      name: "save_memo", arguments: { sessionId: "remote-test", expectedRevision: "rev-1", text: "edited" },
    });
    h.reply(h.toolCalls()[0], { structuredContent: { data: { session_id: "remote-test", revision: "rev-2", lines: [{ text: "edited" }] } } });
    await tick();
    assert.equal(h.get("revision").textContent, "Revision rev-2");
    assert.equal(h.get("status").textContent, "Saved");
  } finally { h.window.close(); }
});

test("memo widget keeps draft on conflict and requires confirmation before reload", async () => {
  const h = host();
  try {
    h.notify({ session_id: "remote-test", revision: "rev-1", lines: [{ text: "first" }] });
    h.get("memo").value = "my draft";
    h.get("memo").dispatchEvent(new h.window.Event("input"));
    h.get("save").click();
    h.reply(h.toolCalls()[0], { isError: true, content: [{ text: "stale revision" }] });
    await tick();
    assert.equal(h.get("memo").value, "my draft");
    assert.match(h.get("status").textContent, /stale revision/);
    h.window.confirm = () => false;
    h.get("reload").click();
    assert.equal(h.toolCalls().length, 1);
    h.window.confirm = () => true;
    h.get("reload").click();
    assert.deepEqual(JSON.parse(JSON.stringify(h.toolCalls()[1].params)), { name: "read_memo", arguments: { sessionId: "remote-test" } });
    h.reply(h.toolCalls()[1], { structuredContent: { data: { session_id: "remote-test", revision: "rev-2", lines: [{ text: "server edit" }] } } });
    await tick();
    assert.equal(h.get("memo").value, "server edit");
  } finally { h.window.close(); }
});
