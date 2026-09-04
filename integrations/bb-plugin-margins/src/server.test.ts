import type { BbPluginApi } from "@get-bb/plugin-sdk";
import { createFakePluginHost } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it } from "vitest";
import type { HostOperationResult } from "./contracts.js";
import { desktopSnapshot } from "./fixtures.js";
import plugin from "./server.js";

type HostRecord = Awaited<ReturnType<BbPluginApi["sdk"]["hosts"]["list"]>>[number];

function hostRecord(id: string, status: HostRecord["status"] = "connected"): HostRecord {
  return {
    id,
    name: "Recording Mac",
    type: "persistent",
    status,
    maxPermissionMode: "full",
    lastSeenAt: null,
    lastRejectedProtocolVersion: null,
    createdAt: 1,
    updatedAt: 1,
  };
}

function createServerHarness(
  status: HostRecord["status"] = "connected",
  resultForMethod: (method: string) => HostOperationResult = (method) => ({
    ok: true,
    snapshot: desktopSnapshot(method === "pause" ? "paused" : "recording"),
  }),
) {
  return createFakePluginHost({
    pluginId: "margins",
    agentSkillIds: ["watermark", "workspace-setup"],
    sdk: {
      hosts: { list: async () => [hostRecord("host-1", status)] },
      threads: {
        get: async () =>
          ({
            id: "thr-1",
            projectId: "proj-1",
            environmentId: "env-1",
          }) as never,
      },
      environments: {
        get: async () => ({ id: "env-1", hostId: "host-1" }) as never,
      },
    },
    experimental_callHostRpc: ({ method }) => resultForMethod(method),
  });
}

describe("Margins server entry", () => {
  it("resolves thread environment host and exposes one live panel RPC", async () => {
    const host = createServerHarness();
    await plugin(host.bb);

    await expect(
      host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      schema: "margins.bb.live.panel.v1",
      state: "ready",
      primaryAction: "start",
      primaryLabel: "Start recording",
      host: { id: "host-1" },
      snapshot: {
        session: { session_id: "customer-call" },
        rolling_transcript: expect.arrayContaining([
          expect.objectContaining({ text: "[00:10] Sam: hello" }),
        ]),
      },
    });

    expect(host.harness.inspection.experimental_hostRpcCalls).toMatchObject([
      {
        method: "readSnapshot",
        input: { sessionId: "current" },
        hostId: "host-1",
      },
    ]);
  });

  it("starts a meeting, stores only the attachment pointer, and pauses by generation", async () => {
    let status: "idle" | "recording" | "paused" = "idle";
    const host = createServerHarness("connected", (method) => {
      if (method === "start") status = "recording";
      if (method === "pause") status = "paused";
      return { ok: true, snapshot: desktopSnapshot(status) };
    });
    await plugin(host.bb);

    await host.harness.behavior.callRpc("startMeeting", {
      threadId: "thr-1",
      title: "Customer call",
    });
    await expect(host.bb.storage.kv.get("attachment:thr-1")).resolves.toMatchObject({
      threadId: "thr-1",
      hostId: "host-1",
      meetingId: "customer-call",
      generation: 2,
      detachedAtUnixMs: null,
    });

    await host.harness.behavior.callRpc("pauseMeeting", { threadId: "thr-1" });
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          method: "start",
          input: expect.objectContaining({
            operationId: expect.stringMatching(/^start:thr-1:/),
            name: "customer-call",
          }),
          hostId: "host-1",
        }),
        expect.objectContaining({
          method: "pause",
          input: expect.objectContaining({
            operationId: expect.stringMatching(/^pause:thr-1:/),
            sessionId: "customer-call",
            expectedGeneration: 2,
          }),
          hostId: "host-1",
        }),
      ]),
    );
  });

  it("routes the whole notepad with the revision Margins last returned", async () => {
    let status: "idle" | "recording" = "idle";
    const host = createServerHarness("connected", (method) => {
      if (method === "start") status = "recording";
      return { ok: true, snapshot: desktopSnapshot(status) };
    });
    await plugin(host.bb);
    await host.harness.behavior.callRpc("startMeeting", {
      threadId: "thr-1",
      title: "Customer call",
    });

    await expect(
      host.harness.behavior.callRpc("updateNotepad", {
        threadId: "thr-1",
        expectedNotepadRevision: "v1-fixture",
        text: "One\nTwo",
      }),
    ).resolves.toMatchObject({ canEditNotepad: true });

    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          method: "updateNotepad",
          input: expect.objectContaining({
            operationId: expect.stringMatching(/^notepad:thr-1:/),
            sessionId: "customer-call",
            expectedGeneration: 2,
            expectedNotepadRevision: "v1-fixture",
            text: "One\nTwo",
          }),
          hostId: "host-1",
        }),
      ]),
    );
  });

  it("keeps @Margins hidden when the selected host is offline", async () => {
    const host = createServerHarness("disconnected");
    await plugin(host.bb);
    const provider = host.harness.inspection.registrations.mentionProviders[0]!;

    await expect(
      provider.search({
        trigger: "@",
        query: "margins",
        projectId: "proj-1",
        threadId: "thr-1",
      }),
    ).resolves.toEqual([]);
  });

  it("keeps @Margins hidden after an attached meeting has stopped", async () => {
    const host = createServerHarness("connected", () => ({
      ok: true,
      snapshot: desktopSnapshot("idle"),
    }));
    await plugin(host.bb);
    await host.bb.storage.kv.set("attachment:thr-1", {
      threadId: "thr-1",
      hostId: "host-1",
      meetingId: "customer-call",
      attachedAtUnixMs: Date.now(),
      detachedAtUnixMs: null,
      generation: 2,
    });
    const provider = host.harness.inspection.registrations.mentionProviders[0]!;

    await expect(
      provider.search({
        trigger: "@",
        query: "margins",
        projectId: "proj-1",
        threadId: "thr-1",
      }),
    ).resolves.toEqual([]);
    await expect(
      host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ mention: { available: false, itemId: null } });
  });

  it("shows a starting runtime as preparing and keeps @Margins hidden", async () => {
    const host = createServerHarness("connected", () => ({
      ok: true,
      snapshot: desktopSnapshot("starting"),
    }));
    await plugin(host.bb);

    await expect(
      host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      state: "preparing",
      title: "Preparing recording",
      primaryAction: "none",
      canStop: false,
      mention: { available: false, itemId: null },
    });

    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      state: "preparing",
      attachment: { meetingId: "customer-call" },
      mention: { available: false, itemId: null },
    });
    const provider = host.harness.inspection.registrations.mentionProviders[0]!;
    await expect(
      provider.search({
        trigger: "@",
        query: "margins",
        projectId: "proj-1",
        threadId: "thr-1",
      }),
    ).resolves.toEqual([]);
  });

  it("prepares a missing runtime and starts from the one Start action", async () => {
    let ready = false;
    let recording = false;
    const host = createServerHarness("connected", (method) => {
      if (method === "readSnapshot" && !ready) {
        return {
          ok: false,
          error: {
            code: "margins_not_found",
            message: "Margins is not installed on this Mac.",
            retryable: true,
            state: "runtime_error",
          },
        };
      }
      if (method === "ensureRuntime") ready = true;
      if (method === "start") recording = true;
      return { ok: true, snapshot: desktopSnapshot(recording ? "recording" : "idle") };
    });
    await plugin(host.bb);

    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "recording", primaryLabel: "Pause" });
    expect(host.harness.inspection.experimental_hostRpcCalls.map((call) => call.method)).toEqual([
      "readSnapshot",
      "ensureRuntime",
      "start",
      "readSnapshot",
    ]);
  });

  it("starts a stopped runtime behind the same Start action", async () => {
    let ready = false;
    let recording = false;
    const host = createServerHarness("connected", (method) => {
      if (method === "readSnapshot" && !ready) {
        return {
          ok: false,
          error: {
            code: "margins_closed",
            message: "Margins is not running.",
            retryable: true,
            state: "runtime_error",
          },
        };
      }
      if (method === "ensureRuntime") ready = true;
      if (method === "start") recording = true;
      return { ok: true, snapshot: desktopSnapshot(recording ? "recording" : "idle") };
    });
    await plugin(host.bb);
    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "recording" });
    expect(host.harness.inspection.experimental_hostRpcCalls.map((call) => call.method)).toContain(
      "ensureRuntime",
    );
  });

  it("attaches an existing live meeting instead of starting another", async () => {
    const host = createServerHarness();
    await plugin(host.bb);
    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "recording", attachment: { meetingId: "customer-call" } });
    expect(host.harness.inspection.experimental_hostRpcCalls.map((call) => call.method)).toEqual([
      "readSnapshot",
      "readSnapshot",
    ]);
  });

  it.each([
    ["microphone_permission_denied", "Microphone permission was denied.", "microphone_permission"],
    [
      "system_audio_permission_denied",
      "System audio permission was denied.",
      "system_audio_permission",
    ],
  ] as const)("keeps %s distinct", async (code, message, expectedState) => {
    const host = createServerHarness("connected", (method) =>
      method === "start"
        ? { ok: false, error: { code, message, retryable: true } }
        : { ok: true, snapshot: desktopSnapshot("idle") },
    );
    await plugin(host.bb);
    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: expectedState, primaryLabel: "Try again" });
    await expect(
      host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: expectedState });
  });

  it("keeps runtime authentication separate from audio permission", async () => {
    const host = createServerHarness("connected", () => ({
      ok: false,
      error: {
        code: "unauthorized",
        message: "Private connection expired.",
        retryable: false,
        state: "runtime_auth_error",
      },
    }));
    await plugin(host.bb);
    await expect(
      host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "runtime_auth_error" });
  });

  it("distinguishes unsupported capture from an offline host", async () => {
    const unsupported = createServerHarness("connected", (method) =>
      method === "readSnapshot"
        ? {
            ok: false,
            error: { code: "margins_not_found", message: "Missing", retryable: true },
          }
        : {
            ok: false,
            error: {
              code: "unsupported",
              message: "Apple silicon is required.",
              retryable: false,
              state: "unsupported_platform",
            },
          },
    );
    await plugin(unsupported.bb);
    await expect(
      unsupported.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      state: "unsupported_platform",
      primaryAction: "none",
      primaryLabel: "Recording unavailable",
    });

    const offline = createServerHarness("disconnected");
    await plugin(offline.bb);
    await expect(
      offline.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "host_offline" });
  });

  it("persists the stopped meeting identity through refresh", async () => {
    const host = createServerHarness("connected", (method) =>
      method === "stop"
        ? {
            ok: true,
            snapshot: desktopSnapshot("idle"),
            stopped_session_id: "saved-customer-call",
          }
        : { ok: true, snapshot: desktopSnapshot("recording") },
    );
    await plugin(host.bb);
    await host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" });
    await expect(
      host.harness.behavior.callRpc("stopMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      state: "meeting_saved",
      savedMeeting: { meetingId: "saved-customer-call" },
      mention: { available: false },
    });
    await expect(
      host.harness.behavior.callRpc("getPanelState", { threadId: "thr-1" }),
    ).resolves.toMatchObject({
      state: "meeting_saved",
      savedMeeting: { meetingId: "saved-customer-call" },
    });
    await expect(host.bb.storage.kv.get("attachment:thr-1")).resolves.toBeUndefined();
    await expect(
      host.harness.behavior.callRpc("dismissSavedMeeting", { threadId: "thr-1" }),
    ).resolves.toMatchObject({ state: "ready", savedMeeting: null });
    await expect(host.bb.storage.kv.get("saved-meeting:thr-1")).resolves.toBeUndefined();
  });

  it("resolves @Margins from a fresh host snapshot at send time", async () => {
    const host = createServerHarness();
    await plugin(host.bb);
    await host.harness.behavior.callRpc("startMeeting", { threadId: "thr-1" });
    const provider = host.harness.inspection.registrations.mentionProviders[0]!;

    const items = await provider.search({
      trigger: "@",
      query: "margins",
      projectId: "proj-1",
      threadId: "thr-1",
    });
    expect(items).toHaveLength(1);
    await expect(provider.resolve(items[0]!.id)).resolves.toMatchObject({
      context: expect.stringContaining('"schema":"margins.watermark.context.v1"'),
    });
    expect(host.harness.inspection.experimental_hostRpcCalls.at(-1)).toMatchObject({
      method: "readSnapshot",
      input: { sessionId: "customer-call" },
      hostId: "host-1",
    });
  });

  it("offers the two focused skills but no native tool", async () => {
    const host = createServerHarness();
    await plugin(host.bb);

    expect(host.harness.inspection.registrations.agentTools).toEqual([]);
    await expect(
      host.harness.behavior.resolveAgentConfiguration({
        thread: { id: "thr-1", title: "Thread" } as never,
        project: { id: "proj-1", name: "Project", kind: "standard" } as never,
        environment: { id: "env-1" } as never,
        host: { id: "host-1", name: "Recording Mac" } as never,
        provider: {
          id: "codex",
          model: "gpt-5.5",
          capabilities: { supportsNativeUserQuestion: false },
        },
        origin: { kind: null, pluginId: null },
      }),
    ).resolves.toMatchObject({
      skills: ["watermark", "workspace-setup"],
      tools: [],
    });
  });
});
