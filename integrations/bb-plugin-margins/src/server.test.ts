import { createFakePluginHost } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it, vi } from "vitest";
import plugin from "./server.js";

const browser = { clientId: "client-1", platform: "other" as const, secureContext: true, browserMicrophone: true, nativeMacCapture: false };
const mac = { ...browser, platform: "macos" as const };
const snapshot = { recordingId: "rec-1", status: "recording" as const, notepad: { text: "", revision: "v1" } };

function harness(options: { heartbeatFails?: boolean } = {}) {
  let stopped = false;
  const host = createFakePluginHost({
    pluginId: "margins", agentSkillIds: ["watermark", "workspace-setup"],
    sdk: {
      threads: { get: async () => ({ id: "thr-1", projectId: "proj-1" }) as never },
      projects: { get: async () => ({
        id: "proj-1", kind: "standard", name: "Project",
        sources: [
          { id: "secondary", projectId: "proj-1", hostId: "wrong-host", path: "/tmp/worktree", isDefault: false, type: "local_path", createdAt: 1, updatedAt: 1 },
          { id: "primary", projectId: "proj-1", hostId: "project-host", path: "/srv/project", isDefault: true, type: "local_path", createdAt: 1, updatedAt: 1 },
        ], createdAt: 1, updatedAt: 1, gitRemoteUrl: null,
      }) as never },
    },
    experimental_callHostRpc: ({ method }) => {
      if (method === "captureAuthority") return { ok: true, instanceId: "instance-1", workspaceId: "workspace-1" };
      if (method === "sessionExists") return { ok: true, found: true };
      if (method === "stop") { stopped = true; return { ok: true, snapshot: null }; }
      if (method === "uploadChunk") return { ok: true };
      if (method === "connectedNoteContext") return { ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-1", title: "Customer call",
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 10 }, memo: { revision: "memo-1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "Pin exact session",
      } };
      if (method === "requestTranscription") return { ok: true, status: "queued", attempt: 1 };
      if (method === "heartbeat" && options.heartbeatFails) return { ok: false, error: { code: "offline", message: "offline", retryable: true } };
      return { ok: true, snapshot: stopped ? null : snapshot };
    },
  });
  plugin(host.bb);
  return host;
}

describe("Margins project recording server", () => {
  it("stores a project Workspace override and can return to the machine default", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("projectWorkspace", { threadId: "thr-1" })).resolves.toEqual({ workspaceId: null });
    await expect(host.harness.behavior.callRpc("projectWorkspace", { threadId: "thr-1", workspaceId: "practice" })).resolves.toEqual({ workspaceId: "practice" });
    await expect(host.bb.storage.kv.get("project-workspace:proj-1")).resolves.toBe("practice");
    await expect(host.harness.behavior.callRpc("projectWorkspace", { threadId: "thr-1", workspaceId: "" })).resolves.toEqual({ workspaceId: null });
  });
  it("offers microphone-only recording on a Mac until native capture is available", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: mac })).resolves.toMatchObject({ state: "ready", sourceLabel: "Microphone only", primaryAction: "start" });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: { ...mac, browserMicrophone: false } })).resolves.toMatchObject({ state: "needs_setup", sourceLabel: null });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "ready", sourceLabel: "Microphone only", storageLabel: "Saves to your Workspace", primaryLabel: "Use browser microphone", detail: expect.stringContaining("choose Connected Workspace") });
    await expect(host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: mac, ownerId: "mac-owner" })).resolves.toMatchObject({ state: "recording", sourceLabel: "Microphone only" });
  });

  it("routes capture to the stable primary project folder and stores no note or audio bodies", async () => {
    const host = harness();
    const started = await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret", title: "Customer call" });
    expect(started).toMatchObject({ state: "recording", recordingId: "rec-1", ownsRecording: true });
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "startBrowserCapture", hostId: "project-host", input: expect.objectContaining({ target: { projectId: "proj-1", hostId: "project-host", projectRoot: "/srv/project" } }) }),
    ]));
    const stored = await host.bb.storage.kv.get<Record<string, unknown>>("capture:proj-1");
    expect(stored).toMatchObject({ projectRoot: "/srv/project", recordingId: "rec-1", clientId: "client-1" });
    expect(JSON.stringify(stored)).not.toMatch(/transcript|bytesBase64|notepad/);

    // A reload during a recording may encounter the wider v1 pointer. Its
    // obsolete routing hints are ignored rather than orphaning the capture.
    await host.bb.storage.kv.set("capture:proj-1", { ...stored, meetingId: "old-id", source: "browser_microphone", status: "recording", startedAtUnixMs: 1 });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1" });
  });

  it("makes controls client-owned but lets another project client see that recording exists", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    const other = { ...browser, clientId: "client-2" };
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: other })).resolves.toMatchObject({ state: "recording_elsewhere", canStop: false, canEditNotepad: false });
  });

  it("does not renew the disconnect lease when the project rejects a heartbeat", async () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    const host = harness({ heartbeatFails: true });
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    now.mockReturnValue(9_000);
    await expect(host.harness.behavior.callRpc("heartbeat", { threadId: "thr-1", client: browser, recordingId: "rec-1", operationId: "heartbeat-1" })).resolves.toMatchObject({ state: "needs_attention" });
    await expect(host.bb.storage.kv.get("capture:proj-1")).resolves.toMatchObject({ lastHeartbeatUnixMs: 1_000 });
    now.mockRestore();
  });

  it("reconciles a repeated Stop from its durable control receipt", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    const input = { threadId: "thr-1", client: browser, recordingId: "rec-1", operationId: "stop-1" };
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.bb.storage.kv.get("saved:proj-1")).resolves.toBeUndefined();
    await expect(host.bb.storage.kv.get("last-session:proj-1")).resolves.toBe("rec-1");
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "ready", lastSessionId: "rec-1" });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "stop")).toHaveLength(1);
  });

  it("pins connected-note context to the exact last session across remounts", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    await host.harness.behavior.callRpc("stop", { threadId: "thr-1", client: browser, recordingId: "rec-1", operationId: "stop-pinned" });
    await expect(host.harness.behavior.callRpc("connectedNoteContext", { threadId: "thr-1", sessionId: "rec-1" })).resolves.toMatchObject({ ok: true, context: { sessionId: "rec-1" } });
    await expect(host.harness.behavior.callRpc("connectedNoteContext", { threadId: "thr-1", sessionId: "stale" })).resolves.toMatchObject({ ok: false, error: { code: "session_pin_stale" } });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "connectedNoteContext")).toHaveLength(1);
    await expect(host.harness.behavior.callRpc("transcribePinnedSession", { threadId: "thr-1", sessionId: "stale" })).resolves.toMatchObject({ ok: false, error: { code: "session_pin_stale" } });
    await expect(host.harness.behavior.callRpc("transcribePinnedSession", { threadId: "thr-1", sessionId: "rec-1" })).resolves.toMatchObject({ ok: true, status: "queued", attempt: 1 });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "requestTranscription")).toHaveLength(1);
  });

  it("pins a saved Mac session only in the project's verified destination", async () => {
    const host = harness();
    const wrong = { threadId: "thr-1", sessionId: "mac-session-1", instanceId: "other", workspaceId: "workspace-1" };
    await expect(host.harness.behavior.callRpc("pinNativeSession", wrong)).resolves.toMatchObject({ ok: false, error: { code: "destination_changed" } });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "sessionExists")).toHaveLength(0);
    await expect(host.harness.behavior.callRpc("pinNativeSession", { ...wrong, instanceId: "instance-1" })).resolves.toMatchObject({ ok: true });
    await expect(host.bb.storage.kv.get("last-session:proj-1")).resolves.toBe("mac-session-1");
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "sessionExists")).toHaveLength(1);
  });
});
