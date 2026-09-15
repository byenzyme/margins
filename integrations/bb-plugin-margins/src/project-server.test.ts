import { afterEach, describe, expect, it, vi } from "vitest";
import { ProjectServerManager } from "./project-server.js";

const saved = {
  url: process.env.MARGINS_BB_REMOTE_URL,
  token: process.env.MARGINS_BB_REMOTE_TOKEN,
  workspace: process.env.MARGINS_BB_REMOTE_WORKSPACE,
};

afterEach(() => {
  vi.unstubAllGlobals();
  for (const [name, value] of Object.entries({
    MARGINS_BB_REMOTE_URL: saved.url,
    MARGINS_BB_REMOTE_TOKEN: saved.token,
    MARGINS_BB_REMOTE_WORKSPACE: saved.workspace,
  })) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
});

describe("ProjectServerManager remote adapter", () => {
  it("uses a negotiated HTTPS Workspace without starting a local runtime", async () => {
    process.env.MARGINS_BB_REMOTE_URL = "https://margins.example.test/";
    process.env.MARGINS_BB_REMOTE_TOKEN = "scoped-token";
    process.env.MARGINS_BB_REMOTE_WORKSPACE = "practice";
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      ok: true,
      result: { workspace_id: "practice" },
    }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const handle = await new ProjectServerManager().ensure({
      projectId: "project",
      projectRoot: "/tmp/project",
      hostId: "host",
    }, "/tmp/plugin-data");

    expect(handle.baseUrl).toBe("https://margins.example.test");
    expect(handle.workspaceId).toBe("practice");
    expect(handle.child).toBeUndefined();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://margins.example.test/v1/capabilities",
      expect.objectContaining({ headers: { authorization: "Bearer scoped-token" } }),
    );
  });

  it("rejects incomplete or non-TLS remote configuration before transport", async () => {
    process.env.MARGINS_BB_REMOTE_URL = "http://remote.example.test";
    process.env.MARGINS_BB_REMOTE_TOKEN = "scoped-token";
    process.env.MARGINS_BB_REMOTE_WORKSPACE = "practice";
    await expect(new ProjectServerManager().ensure({
      projectId: "project",
      projectRoot: "/tmp/project",
      hostId: "host",
    }, "/tmp/plugin-data")).rejects.toThrow("HTTPS or loopback");
  });
});
