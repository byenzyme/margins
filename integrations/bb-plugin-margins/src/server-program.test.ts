import { createFakePluginHost } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it } from "vitest";
import plugin from "./server.js";

function harness(options: { defaultWorkspaceId?: string | null; hostOffline?: boolean } = {}) {
  const calls: Array<{ method: string; input: unknown; hostId: string }> = [];
  const host = createFakePluginHost({
    pluginId: "margins", agentSkillIds: ["watermark", "workspace-setup", "connected-note"],
    sdk: {
      projects: { get: async ({ projectId }: { projectId: string }) => ({
        id: projectId, kind: "standard", name: "Project",
        sources: [{ id: "primary", projectId, hostId: "project-host", path: "/srv/project", isDefault: true, type: "local_path", createdAt: 1, updatedAt: 1 }],
        createdAt: 1, updatedAt: 1, gitRemoteUrl: null,
      }) as never },
    },
    experimental_callHostRpc: ({ method, input, hostId }) => {
      calls.push({ method, input, hostId });
      if (options.hostOffline && method !== "workspaceOptions") throw new Error("Host worker offline");
      if (method === "workspaceOptions") return { defaultWorkspaceId: options.defaultWorkspaceId === undefined ? "notes" : options.defaultWorkspaceId,
        autoSelected: false, workspaces: [{ id: "notes", name: "Notes" }] };
      if (method === "readWorkspaceProgram") return { workspaceId: "notes", programPath: "/m/configs/notes.enzyme", revision: "rev-1", program: "workspace \"notes\" {}\n" };
      if (method === "planWorkspaceProgram") return (input as { program: string }).program.includes("lern")
        ? { ok: false, error: { code: "workspace_desired_invalid", message: "3:3: expected \"}\"", line: 3, column: 3 } }
        : { ok: true, previewId: "11111111-2222-3333-4444-555555555555", workspaceId: "notes", baseRevision: "rev-1", noop: false,
          actions: [{ action: "update_program", summary: "Update program statements" }], diff: "--- a\n+++ b\n" };
      if (method === "applyWorkspaceProgram") return { ok: false, error: { code: "stale", message: "changed", line: null, column: null, actualRevision: "rev-9" } };
      throw new Error(`unexpected ${method}`);
    },
  });
  plugin(host.bb);
  return { ...host, calls };
}

describe("Workspace program RPC", () => {
  it("reads, plans, and applies the project's Workspace program on the project's machine", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("workspaceProgram", { projectId: "proj-1" })).resolves.toEqual({
      workspaceId: "notes", programPath: "/m/configs/notes.enzyme", revision: "rev-1", program: "workspace \"notes\" {}\n" });
    await expect(host.harness.behavior.callRpc("planWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes", program: "x" }))
      .resolves.toMatchObject({ ok: true, baseRevision: "rev-1", actions: [{ summary: "Update program statements" }] });
    await expect(host.harness.behavior.callRpc("planWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes", program: "lern" }))
      .resolves.toEqual({ ok: false, error: { code: "workspace_desired_invalid", message: "3:3: expected \"}\"", line: 3, column: 3 } });
    await expect(host.harness.behavior.callRpc("applyWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes",
      previewId: "11111111-2222-3333-4444-555555555555" })).resolves.toMatchObject({ ok: false, error: { code: "stale", actualRevision: "rev-9" } });
    const routed = host.calls.filter((call) => call.method !== "workspaceOptions");
    expect(routed.map((call) => call.method)).toEqual(["readWorkspaceProgram", "planWorkspaceProgram", "planWorkspaceProgram", "applyWorkspaceProgram"]);
    expect(routed.every((call) => call.hostId === "project-host")).toBe(true);
    expect(routed[0].input).toEqual({ workspaceId: "notes" });
    expect(routed[1].input).toEqual({ workspaceId: "notes", program: "x" });
  });

  it("refuses a plan for a Workspace the project no longer uses, and needs a Workspace", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("planWorkspaceProgram", { projectId: "proj-1", workspaceId: "other", program: "x" }))
      .rejects.toThrow("different Margins Workspace");
    expect(host.calls.some((call) => call.method === "planWorkspaceProgram")).toBe(false);
    await expect(harness({ defaultWorkspaceId: null }).harness.behavior.callRpc("workspaceProgram", { projectId: "proj-1" }))
      .rejects.toThrow("Set up a Margins Workspace");
  });

  it("surfaces an unreachable project machine instead of an invalid result", async () => {
    await expect(harness({ hostOffline: true }).harness.behavior.callRpc("workspaceProgram", { projectId: "proj-1" }))
      .rejects.toThrow("offline");
  });

  it("bounds the program text the editor may send", async () => {
    await expect(harness().harness.behavior.callRpc("planWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes", program: "x".repeat(300 * 1024) }))
      .rejects.toThrow();
  });
});
