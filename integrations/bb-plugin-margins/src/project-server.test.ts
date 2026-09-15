import { afterEach, describe, expect, it, vi } from "vitest";
import { ProjectMarginsTransport, ProjectServerManager } from "./project-server.js";

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
      result: { workspace_id: "practice", instance_id: "instance-remote" },
    }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const handle = await new ProjectServerManager().ensure({
      projectId: "project",
      projectRoot: "/tmp/project",
      hostId: "host",
    }, "/tmp/plugin-data");

    expect(handle.baseUrl).toBe("https://margins.example.test");
    expect(handle.workspaceId).toBe("practice");
    expect(handle.instanceId).toBe("instance-remote");
    expect(handle.child).toBeUndefined();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://margins.example.test/v1/capabilities",
      expect.objectContaining({ headers: { authorization: "Bearer scoped-token" } }),
    );
  });

  it("builds a pinned handoff from typed metadata routes without fetching artifact or note bytes", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      let result: unknown;
      if (url.includes("/sessions?")) result = { sessions: [{ session_id: "rec-1", title: "Pinned" }], next_cursor: null };
      else if (url.endsWith("/transcript")) result = { terminal: true, live: false, updated_at_unix_ms: 12, body: "must not cross host contract" };
      else if (url.endsWith("/memo")) result = { revision: "memo-2", lines: [{ text: "private memo" }] };
      else if (url.endsWith("/artifacts")) result = [{ artifact_id: "artifact-1", kind: "transcript", retention_class: "session" }];
      else if (url.endsWith("/note-association")) result = { source_id: "notes", relative_path: "Meetings/pinned.md", revision: 3 };
      else throw new Error(`unexpected URL ${url}`);
      return new Response(JSON.stringify({ ok: true, result }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    const result = await new ProjectMarginsTransport(manager).connectedNoteContext({ projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1");
    expect(result).toMatchObject({ ok: true, context: { instanceId: "instance-remote", workspaceId: "practice", sessionId: "rec-1", memo: { lineCount: 1 }, noteAssociation: { relativePath: "Meetings/pinned.md" } } });
    expect(JSON.stringify(result)).not.toContain("private memo");
    expect(JSON.stringify(result)).not.toContain("must not cross host contract");
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/artifacts/artifact-1/content"))).toBe(false);
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
