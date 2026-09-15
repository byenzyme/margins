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
      if (method === "stop") { stopped = true; return { ok: true, snapshot: null }; }
      if (method === "uploadChunk") return { ok: true };
      if (method === "heartbeat" && options.heartbeatFails) return { ok: false, error: { code: "offline", message: "offline", retryable: true } };
      return { ok: true, snapshot: stopped ? null : snapshot };
    },
  });
  plugin(host.bb);
  return host;
}

describe("Margins project recording server", () => {
  it("is honest about Mac setup and offers browser microphone recording elsewhere", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: mac })).resolves.toMatchObject({ state: "needs_setup", title: "Enable recording on this Mac", sourceLabel: null, primaryAction: "none" });
    await expect(host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1", client: browser })).resolves.toMatchObject({ state: "ready", sourceLabel: "Microphone only", storageLabel: "Saved to this bb project" });
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
    await expect(host.bb.storage.kv.get("saved:proj-1")).resolves.toMatchObject({ savedAtUnixMs: expect.any(Number) });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "stop")).toHaveLength(1);
  });
});
