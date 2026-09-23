import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { JSDOM } from "jsdom";

const html = readFileSync(new URL("../web/live-meeting.html", import.meta.url), "utf8");
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test("chat-bound live view initializes, polls one session, and waits honestly for remote ASR", async () => {
  const calls = [];
  let poll;
  const dom = new JSDOM(html, { runScripts: "dangerously", url: "https://widget.invalid", beforeParse(window) {
    window.postMessage = message => calls.push(message);
    window.setInterval = callback => { poll = callback; return 1; };
  } });
  const { window } = dom;
  const get = id => window.document.getElementById(id);
  const reply = (request, result) => window.dispatchEvent(new window.MessageEvent("message", {
    source: window, data: { jsonrpc: "2.0", id: request.id, result },
  }));
  const notify = data => window.dispatchEvent(new window.MessageEvent("message", {
    source: window, data: { jsonrpc: "2.0", method: "ui/notifications/tool-result", params: { structuredContent: { data } } },
  }));
  try {
    assert.equal(calls[0].method, "ui/initialize");
    reply(calls[0], { hostCapabilities: {} });
    await tick();
    assert.equal(calls[1].method, "ui/notifications/initialized");
    notify({ session_id: "remote-live", input_finalized: false, transcript: null });
    assert.equal(get("status").textContent, "Capture not finalized");
    assert.match(get("detail").textContent, /after Stop/);
    poll();
    const refresh = calls.find(call => call.method === "tools/call");
    assert.deepEqual(JSON.parse(JSON.stringify(refresh.params)), { name: "read_live_meeting", arguments: { sessionId: "remote-live" } });
    reply(refresh, { structuredContent: { data: { session_id: "remote-live", input_finalized: true, transcript: { body: "Purple lantern seven", terminal: true } } } });
    await tick();
    assert.equal(get("status").textContent, "Transcript ready");
    assert.equal(get("transcript").textContent, "Purple lantern seven");
    notify({ session_id: "another-meeting", input_finalized: true, transcript: null });
    assert.equal(get("session").textContent, "remote-live");
    assert.match(get("error").textContent, /Meeting changed/);
  } finally { window.close(); }
});
