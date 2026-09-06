import { createFakePluginHost } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it, vi } from "vitest";
import { DISCONNECT_GRACE_MS } from "./server.js";
import plugin from "./server.js";

const browser = { clientId: "client-1", platform: "other" as const, secureContext: true, browserMicrophone: true, nativeMacCapture: false };
const mac = { ...browser, platform: "macos" as const };
const snapshot = { recordingId: "rec-1", meetingId: "customer-call", status: "recording" as const, elapsedMs: 1_000, notepad: { text: "", revision: "v1" }, transcriptAvailable: false };

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
    await expect(host.harness.behavior.callRpc("heartbeat", { threadId: "thr-1", client: browser, recordingId: "rec-1" })).resolves.toMatchObject({ state: "needs_attention" });
    await expect(host.bb.storage.kv.get("capture:proj-1")).resolves.toMatchObject({ lastHeartbeatUnixMs: 1_000 });
    now.mockRestore();
  });

  it("stops and records a durable handoff after the disconnect grace", async () => {
    vi.useFakeTimers();
    const host = harness();
    await host.harness.behavior.callRpc("beginBrowserCapture", { threadId: "thr-1", client: browser, ownerId: "owner-secret" });
    const running = host.harness.runService("capture-disconnect-safety");
    await vi.advanceTimersByTimeAsync(DISCONNECT_GRACE_MS + 4_000);
    await expect(host.bb.storage.kv.get("saved:proj-1")).resolves.toMatchObject({ meetingId: "customer-call" });
    await host.harness.lifecycle.dispose();
    await running;
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([expect.objectContaining({ method: "stop", hostId: "project-host" })]));
    vi.useRealTimers();
  });
});
