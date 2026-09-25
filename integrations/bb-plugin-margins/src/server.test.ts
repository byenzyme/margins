import { createFakePluginHost } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it, vi } from "vitest";
import plugin from "./server.js";

const browser = { clientId: "client-1", platform: "other" as const, secureContext: true, browserMicrophone: true, nativeMacCapture: false };
const mac = { ...browser, platform: "macos" as const };
const snapshot = { recordingId: "rec-1", sessionId: "rec-1", status: "recording" as const, notepad: { text: "", revision: "v1" } };

function harness(options: { heartbeatFails?: boolean; canonicalSessionId?: string } = {}) {
  let stopped = false;
  let captureStatus: "recording" | "paused" = "recording";
  const host = createFakePluginHost({
    pluginId: "margins", agentSkillIds: ["watermark", "workspace-setup"],
    sdk: {
      threads: { get: async ({ threadId }: { threadId: string }) => ({ id: threadId, projectId: threadId === "thr-other-project" ? "proj-2" : "proj-1" }) as never },
      projects: { get: async ({ projectId }: { projectId: string }) => ({
        id: projectId, kind: "standard", name: "Project",
        sources: [
          { id: "secondary", projectId, hostId: "wrong-host", path: "/tmp/worktree", isDefault: false, type: "local_path", createdAt: 1, updatedAt: 1 },
          { id: "primary", projectId, hostId: "project-host", path: projectId === "proj-2" ? "/srv/other" : "/srv/project", isDefault: true, type: "local_path", createdAt: 1, updatedAt: 1 },
        ], createdAt: 1, updatedAt: 1, gitRemoteUrl: null,
      }) as never },
    },
    experimental_callHostRpc: ({ method }) => {
      if (method === "captureAuthority") return { ok: true, instanceId: "instance-1", workspaceId: "workspace-1" };
      if (method === "sessionExists") return { ok: true, found: true };
      if (method === "stop") { stopped = true; return { ok: true, snapshot: null }; }
      if (method === "pause") captureStatus = "paused";
      if (method === "resume") captureStatus = "recording";
      if (method === "uploadChunk") return { ok: true };
      if (method === "connectedNoteContext") return { ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-1", title: "Customer call",
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 10 }, memo: { revision: "memo-1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "Pin exact session",
      } };
      if (method === "requestTranscription") return { ok: true, status: "queued", attempt: 1 };
      if (method === "heartbeat" && options.heartbeatFails) return { ok: false, error: { code: "offline", message: "offline", retryable: true } };
      return { ok: true, snapshot: stopped ? null : { ...snapshot, sessionId: options.canonicalSessionId || snapshot.sessionId, status: captureStatus } };
    },
  });
  plugin(host.bb);
  return host;
}

describe("Margins project recording server", () => {
  it("keys ownership by the Workspace session while routing audio by browser recording id", async () => {
    const host = harness({ canonicalSessionId: "meeting-2" });
    const started = await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner" });
    expect(started).toMatchObject({ recordingId: "rec-1", sessionId: "meeting-2" });
    await expect(host.bb.storage.kv.get("session:meeting-2")).resolves.toMatchObject({ sessionId: "meeting-2", recordingId: "rec-1" });
    await expect(host.bb.storage.kv.get("live:workspace-1")).resolves.toBe("meeting-2");
    await expect(host.bb.storage.kv.get("recording:rec-1")).resolves.toBe("meeting-2");
    await host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-canonical" });
    await expect(host.bb.storage.kv.get("last-session:workspace-1")).resolves.toBe("meeting-2");
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "stop", input: expect.objectContaining({ recordingId: "rec-1" }) }),
    ]));
  });
  it("routes controls by session across thread changes", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner" });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-2", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1", ownsRecording: true });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-other-project", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1", ownsRecording: true });
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "readCapture", input: expect.objectContaining({ target: expect.objectContaining({ projectId: "proj-1", projectRoot: "/srv/project" }) }) }),
    ]));
    await expect(host.harness.behavior.callRpc("pause", { sessionId: "rec-1", client: browser, operationId: "pause-1" })).resolves.toMatchObject({ state: "paused" });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "paused" });
    await expect(host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-1" })).resolves.toMatchObject({ state: "saved" });
  });
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
    const stored = await host.bb.storage.kv.get<Record<string, unknown>>("session:rec-1");
    expect(stored).toMatchObject({ projectRoot: "/srv/project", workspaceId: "workspace-1", recordingId: "rec-1", clientId: "client-1" });
    await expect(host.bb.storage.kv.get("live:workspace-1")).resolves.toBe("rec-1");
    expect(JSON.stringify(stored)).not.toMatch(/transcript|bytesBase64|notepad/);

    // A reload during a recording may encounter the wider v1 pointer. Its
    // obsolete routing hints are ignored rather than orphaning the capture.
    await host.bb.storage.kv.set("session:rec-1", { ...stored, meetingId: "old-id", source: "browser_microphone", status: "recording", startedAtUnixMs: 1 });
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
    await expect(host.harness.behavior.callRpc("heartbeat", { sessionId: "rec-1", client: browser, operationId: "heartbeat-1" })).resolves.toMatchObject({ state: "needs_attention" });
    await expect(host.bb.storage.kv.get("session:rec-1")).resolves.toMatchObject({ lastHeartbeatUnixMs: 1_000 });
    now.mockRestore();
  });

  it("reconciles a repeated Stop from its durable control receipt", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    const input = { sessionId: "rec-1", client: browser, operationId: "stop-1" };
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.bb.storage.kv.get("session:rec-1")).resolves.toBeUndefined();
    await expect(host.bb.storage.kv.get("live:workspace-1")).resolves.toBeUndefined();
    await expect(host.bb.storage.kv.get("last-session:workspace-1")).resolves.toBe("rec-1");
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "ready", lastSessionId: "rec-1" });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "stop")).toHaveLength(1);
  });

  it("resolves connected-note context for an explicitly selected ended session", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    await host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-pinned" });
    await expect(host.harness.behavior.callRpc("connectedNoteContext", { threadId: "thr-1", sessionId: "rec-1" })).resolves.toMatchObject({ ok: true, context: { sessionId: "rec-1" } });
    await expect(host.harness.behavior.callRpc("connectedNoteContext", { threadId: "thr-1", sessionId: "older-session" })).resolves.toMatchObject({ ok: true });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "connectedNoteContext")).toHaveLength(2);
    await expect(host.harness.behavior.callRpc("transcribePinnedSession", { threadId: "thr-1", sessionId: "older-session" })).resolves.toMatchObject({ ok: true, status: "queued", attempt: 1 });
    await expect(host.harness.behavior.callRpc("transcribePinnedSession", { threadId: "thr-1", sessionId: "rec-1" })).resolves.toMatchObject({ ok: true, status: "queued", attempt: 1 });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "requestTranscription")).toHaveLength(2);
  });

  it("pins a saved Mac session only in the project's verified destination", async () => {
    const host = harness();
    const wrong = { threadId: "thr-1", sessionId: "mac-session-1", instanceId: "other", workspaceId: "workspace-1" };
    await expect(host.harness.behavior.callRpc("pinNativeSession", wrong)).resolves.toMatchObject({ ok: false, error: { code: "destination_changed" } });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "sessionExists")).toHaveLength(0);
    await expect(host.harness.behavior.callRpc("pinNativeSession", { ...wrong, instanceId: "instance-1" })).resolves.toMatchObject({ ok: true });
    await expect(host.bb.storage.kv.get("last-session:workspace-1")).resolves.toBe("mac-session-1");
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "sessionExists")).toHaveLength(1);
  });
});
