import { afterEach, describe, expect, it, vi } from "vitest";
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { marginsHome, ProjectMarginsTransport, ProjectServerManager, readAsrRuntimeConfig, resolveWorkspaceId, workspaceInstanceDir, workspaceOptions, workspacePaths } from "./project-server.js";

const saved = {
  url: process.env.MARGINS_BB_REMOTE_URL,
  token: process.env.MARGINS_BB_REMOTE_TOKEN,
  workspace: process.env.MARGINS_BB_REMOTE_WORKSPACE,
  home: process.env.MARGINS_HOME,
  cli: process.env.MARGINS_CLI_BIN,
};

afterEach(() => {
  vi.unstubAllGlobals();
  for (const [name, value] of Object.entries({
    MARGINS_BB_REMOTE_URL: saved.url,
    MARGINS_BB_REMOTE_TOKEN: saved.token,
    MARGINS_BB_REMOTE_WORKSPACE: saved.workspace,
    MARGINS_HOME: saved.home,
    MARGINS_CLI_BIN: saved.cli,
  })) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
});

describe("ProjectServerManager remote adapter", () => {
  it("reads recording and note destinations from the selected Workspace CLI", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-workspace-paths-"));
    try {
      const cli = join(root, "margins-test-cli");
      await writeFile(cli, `#!/usr/bin/env node\nif (process.argv.includes("destination")) console.log(JSON.stringify({destination:"/vault/inbox"}));\nelse console.log(JSON.stringify([{name:"captures",kind:"captures",path:"/data/captures"}]));\n`);
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      process.env.MARGINS_HOME = root;
      await expect(workspacePaths("practice")).resolves.toEqual({ notes: "/vault/inbox", recordings: "/data/captures" });
      await expect(workspacePaths("../other")).rejects.toThrow("Invalid Margins Workspace id");
    } finally { await rm(root, { recursive: true, force: true }); }
  });
  it("uses the Workspace service for completed meeting actions", async () => {
    const manager = { ensure: vi.fn(async () => ({ baseUrl: "https://margins.example.test", token: "scoped-token",
      workspaceId: "practice", instanceId: "instance-remote" })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async (input: string | URL | Request, _options?: RequestInit) => {
      const url = String(input);
      const result = url.endsWith("/transcript") ? { body: "Transcript text" } : { updated: true };
      return new Response(JSON.stringify({ ok: true, result }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const transport = new ProjectMarginsTransport(manager);
    const target = { projectId: "project", projectRoot: "/tmp/project", hostId: "host" };
    await expect(transport.renameWorkspaceMeeting(target, "/tmp/data", "meeting-1", "Planning"))
      .resolves.toEqual({ ok: true });
    await expect(transport.readWorkspaceTranscript(target, "/tmp/data", "meeting-1"))
      .resolves.toEqual({ ok: true, body: "Transcript text" });
    await expect(transport.discardWorkspaceMeeting(target, "/tmp/data", "meeting-1"))
      .resolves.toEqual({ ok: true });
    expect(fetchMock.mock.calls.map(([url, options]) => [String(url), options?.method])).toEqual([
      ["https://margins.example.test/v1/workspaces/practice/sessions/meeting-1/title", "PUT"],
      ["https://margins.example.test/v1/workspaces/practice/sessions/meeting-1/transcript", "GET"],
      ["https://margins.example.test/v1/workspaces/practice/sessions/meeting-1", "DELETE"],
    ]);
  });
  it("forwards capture upload through the selected Workspace service only", async () => {
    const manager = { ensure: vi.fn(async () => ({ baseUrl: "http://127.0.0.1:8787", token: "service-token",
      workspaceId: "practice", instanceId: "instance-1" })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async () => new Response('{"ok":true}', { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    const transport = new ProjectMarginsTransport(manager);
    const target = { projectId: "project", projectRoot: "/tmp/project", hostId: "host", workspaceId: "practice" };
    const bodyBase64 = Buffer.from('{"commands":[]}').toString("base64");
    await expect(transport.relayWorkspaceHttp(target, "/tmp/data", { method: "POST",
      path: "v1/workspaces/practice/audio-chunks", bodyBase64, contentType: "application/json" }))
      .resolves.toMatchObject({ status: 200 });
    expect(fetchMock).toHaveBeenCalledWith("http://127.0.0.1:8787/v1/workspaces/practice/audio-chunks",
      expect.objectContaining({ method: "POST", headers: expect.objectContaining({ authorization: "Bearer service-token" }) }));
    await expect(transport.relayWorkspaceHttp(target, "/tmp/data", { method: "GET",
      path: "v1/workspaces/practice/sessions?limit=1", bodyBase64: "" })).resolves.toMatchObject({ status: 200 });
    await expect(transport.relayWorkspaceHttp(target, "/tmp/data", { method: "GET",
      path: "v1/workspaces/other/current", bodyBase64: "" })).rejects.toThrow(/outside its Workspace/);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
  it("previews only existing notes confined to the selected Workspace Home", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-note-preview-"));
    try {
      const vault = join(root, "vault");
      await mkdir(join(vault, "inbox"), { recursive: true });
      await writeFile(join(vault, "inbox/note.md"), "# Linked note\n");
      await writeFile(join(root, "outside.md"), "# Outside\n");
      await symlink(join(root, "outside.md"), join(vault, "inbox/outside.md"));
      const cli = join(root, "margins-test-cli");
      await writeFile(cli, `#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify({home_root:${JSON.stringify(vault)}}));\n`);
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      const manager = { ensure: vi.fn(async () => ({ baseUrl: "http://127.0.0.1:8787", token: "token", workspaceId: "practice", instanceId: "local", child: {} })) } as unknown as ProjectServerManager;
      vi.stubGlobal("fetch", vi.fn(async (input: string | URL | Request) => {
        const url = String(input);
        const id = url.includes("/sessions/inside") ? "inside" : "outside";
        const result = url.endsWith("sessions?limit=50") ? { sessions: [{ session_id: "inside" }, { session_id: "outside" }] }
          : url.endsWith("/note-association") ? { relative_path: `inbox/${id === "inside" ? "note" : "outside"}.md`, bb_thread_ids: [] }
            : { session_id: id, title: null, started_at: "2026-09-25T01:00:00Z", input_finalized: true };
        return new Response(JSON.stringify({ ok: true, result }));
      }));
      const listed = await new ProjectMarginsTransport(manager).listWorkspaceMeetings({ projectId: "project", projectRoot: root, hostId: "host" }, root);
      expect(listed).toMatchObject({ ok: true, meetings: [
        { sessionId: "inside", noteFile: { hostId: "host", path: join(vault, "inbox/note.md") } },
        { sessionId: "outside", noteFile: null },
      ] });
    } finally { await rm(root, { recursive: true, force: true }); }
  });
  it("resolves two projects to the same machine default and keeps their overrides separate", async () => {
    const home = await mkdtemp(join(tmpdir(), "margins-bb-workspace-"));
    try {
      process.env.MARGINS_HOME = home;
      const cli = join(home, "margins-fixture");
      await writeFile(cli, '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify({default_workspace:"vault",workspaces:[{id:"vault",name:"Notes"},{id:"other",name:null}]}));\n');
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      const first = { projectId: "p1", projectRoot: "/code/one", hostId: "h" };
      const second = { projectId: "p2", projectRoot: "/code/two", hostId: "h" };
      expect(marginsHome()).toBe(home);
      await expect(resolveWorkspaceId(first)).resolves.toBe("vault");
      await expect(resolveWorkspaceId(second)).resolves.toBe("vault");
      await expect(resolveWorkspaceId({ ...second, workspaceId: "other" })).resolves.toBe("other");
    } finally { await rm(home, { recursive: true, force: true }); }
  });
  it("sets the sole Workspace as machine default on first use", async () => {
    const home = await mkdtemp(join(tmpdir(), "margins-bb-host-data-"));
    try {
      process.env.MARGINS_HOME = home;
      const cli = join(home, "margins-fixture");
      await writeFile(cli, `#!/usr/bin/env node\nconst fs=require('node:fs');const p=require('node:path');const file=p.join(process.env.MARGINS_HOME,'selected');if(process.argv[3]==='default'){fs.writeFileSync(file,process.argv[5]);process.stdout.write('{}');}else{process.stdout.write(JSON.stringify({default_workspace:fs.existsSync(file)?fs.readFileSync(file,'utf8'):null,workspaces:[{id:'vault',name:'Notes'}]}));}\n`);
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      await expect(workspaceOptions()).resolves.toMatchObject({ defaultWorkspaceId: "vault", autoSelected: true });
      await expect(workspaceOptions()).resolves.toMatchObject({ defaultWorkspaceId: "vault", autoSelected: false });
    } finally { await rm(home, { recursive: true, force: true }); }
  });
  it("reopens the newest saved Workspace meeting after recording ends", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      let result: unknown;
      if (url.endsWith("/active-sessions")) result = { sessions: [] };
      else if (url.endsWith("/sessions?limit=1")) result = { sessions: [{ session_id: "remote-newest" }], next_cursor: null };
      else if (url.endsWith("/sessions/remote-newest")) result = {
        session_id: "remote-newest", title: "Saved menu meeting", started_at: "2026-09-24T03:00:00Z", input_finalized: true,
      };
      else if (url.endsWith("/sessions/remote-newest/memo")) result = { revision: "rev-new", lines: [{ text: "BB note" }] };
      else throw new Error(`unexpected URL ${url}`);
      return new Response(JSON.stringify({ ok: true, result }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const meeting = await new ProjectMarginsTransport(manager).readWorkspaceMeeting({ projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data");
    expect(meeting).toMatchObject({ ok: true, meeting: { sessionId: "remote-newest", inputFinalized: true, notepad: { text: "BB note", revision: "rev-new" } } });
  });

  it("joins one active menu meeting and saves its revisioned memo in that Workspace", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    let memo = { revision: "rev-1", lines: [{ text: "First point" }] };
    const fetchMock = vi.fn(async (input: string | URL | Request, options?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/active-sessions")) return new Response(JSON.stringify({ ok: true, result: { sessions: [{ session_id: "remote-menu-1" }] } }));
      if (url.endsWith("/sessions/remote-menu-1")) return new Response(JSON.stringify({ ok: true, result: {
        session_id: "remote-menu-1", title: "Planning", started_at: "2026-09-24T03:00:00Z",
        input_finalized: false, capture_duration_ms: null,
      } }));
      if (url.endsWith("/sessions/remote-menu-1/memo")) {
        if (options?.method === "PUT") {
          const body = JSON.parse(String(options.body));
          expect(body).toMatchObject({ expected_revision: "rev-1", text: "First point\nSecond point", paused: false });
          expect(body.observed_at_ms).toBeGreaterThanOrEqual(0);
          expect(body.request_id).toBeTruthy();
          memo = { revision: "rev-2", lines: [{ text: "First point" }, { text: "Second point" }] };
        }
        return new Response(JSON.stringify({ ok: true, result: memo }));
      }
      throw new Error(`unexpected URL ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const transport = new ProjectMarginsTransport(manager);
    const target = { projectId: "project", projectRoot: "/tmp/project", hostId: "host" };
    const active = await transport.readWorkspaceMeeting(target, "/tmp/data");
    expect(active).toMatchObject({ ok: true, meeting: { sessionId: "remote-menu-1", notepad: { text: "First point", revision: "rev-1" } } });
    const savedMemo = await transport.saveWorkspaceMemo(target, "/tmp/data", "remote-menu-1", "rev-1", "First point\nSecond point");
    expect(savedMemo).toMatchObject({ ok: true, meeting: { sessionId: "remote-menu-1", notepad: { revision: "rev-2", text: "First point\nSecond point" } } });
    expect(fetchMock.mock.calls.every(([, options]) => (options?.headers as Record<string, string>)?.["X-Margins-Instance-Id"] === "instance-remote")).toBe(true);
  });

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
      workspaceId: "practice",
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

  it("shares one Workspace server and Meetings list across bb projects", async () => {
    process.env.MARGINS_BB_REMOTE_URL = "https://margins.example.test";
    process.env.MARGINS_BB_REMOTE_TOKEN = "scoped-token";
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const path = new URL(String(input)).pathname;
      const result = path === "/v1/capabilities" ? { workspace_id: "practice", instance_id: "instance-remote" }
        : path.endsWith("/sessions") ? { sessions: [{ session_id: "meeting-1" }] }
          : path.endsWith("/note-association") ? null
            : { session_id: "meeting-1", title: "Shared meeting", started_at: "2026-09-25T01:00:00Z", input_finalized: true };
      return new Response(JSON.stringify({ ok: true, result }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const manager = new ProjectServerManager();
    const transport = new ProjectMarginsTransport(manager);
    const first = { projectId: "first", projectRoot: "/code/one", hostId: "host", workspaceId: "practice" };
    const second = { projectId: "second", projectRoot: "/code/two", hostId: "host", workspaceId: "practice" };
    const [firstHandle, secondHandle] = await Promise.all([
      manager.ensure(first, "/plugin-data"), manager.ensure(second, "/plugin-data"),
    ]);
    expect(firstHandle).toBe(secondHandle);
    expect(fetchMock.mock.calls.filter(([url]) => String(url).endsWith("/v1/capabilities"))).toHaveLength(1);
    const firstList = await transport.listWorkspaceMeetings(first, "/plugin-data");
    const secondList = await transport.listWorkspaceMeetings(second, "/plugin-data");
    expect(firstList).toMatchObject({ ok: true, meetings: [{ sessionId: "meeting-1", title: "Shared meeting" }] });
    expect(secondList).toEqual(firstList);
    expect(workspaceInstanceDir("/plugin-data", "practice")).toBe("/plugin-data/workspace-servers/practice");
  });

  it("starts one local process and reads the same meetings from either project", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-shared-server-"));
    const manager = new ProjectServerManager();
    try {
      process.env.MARGINS_HOME = join(root, "home");
      await mkdir(process.env.MARGINS_HOME);
      const serverPath = join(root, "fixture-server");
      const modelDir = join(root, "model");
      const ortLibraryPath = join(root, "libonnxruntime.so");
      await mkdir(modelDir);
      await writeFile(ortLibraryPath, "fixture");
      await writeFile(serverPath, `#!/usr/bin/env node
const fs=require('node:fs');const http=require('node:http');const path=require('node:path');
const data=process.env.MARGINS_DATA_DIR;fs.mkdirSync(data,{recursive:true});
fs.appendFileSync(${JSON.stringify(join(root, "spawns"))},String(process.pid)+'\\n');
fs.writeFileSync(path.join(data,'token'),'fixture-token');
http.createServer((req,res)=>{if(req.url==='/health'){res.writeHead(200);res.end('ok');return;}
const pathname=new URL(req.url,'http://localhost').pathname;
const result=pathname.endsWith('/sessions')?{sessions:[{session_id:'meeting-1'}]}:
pathname.endsWith('/note-association')?null:
{session_id:'meeting-1',title:'Shared meeting',started_at:'2026-09-25T01:00:00Z',input_finalized:true};
res.setHeader('Content-Type','application/json');res.end(JSON.stringify({ok:true,result}));
}).listen(Number(process.env.MARGINS_PORT),'127.0.0.1');
`);
      await chmod(serverPath, 0o755);
      await writeFile(join(root, "asr-runtime.json"), JSON.stringify({ serverPath, modelDir, ortLibraryPath }));
      const first = { projectId: "first", projectRoot: join(root, "one"), hostId: "host", workspaceId: "practice" };
      const second = { projectId: "second", projectRoot: join(root, "two"), hostId: "host", workspaceId: "practice" };
      const transport = new ProjectMarginsTransport(manager);
      const [firstList, secondList] = await Promise.all([
        transport.listWorkspaceMeetings(first, root), transport.listWorkspaceMeetings(second, root),
      ]);
      expect(firstList).toMatchObject({ ok: true, meetings: [{ sessionId: "meeting-1" }] });
      expect(secondList).toEqual(firstList);
      expect((await readFile(join(root, "spawns"), "utf8")).trim().split("\n")).toHaveLength(1);
      expect(workspaceInstanceDir(root, "practice")).toBe(join(root, "workspace-servers", "practice"));
    } finally {
      await manager.dispose();
      await rm(root, { recursive: true, force: true });
    }
  });

  it("builds a pinned handoff from typed metadata routes without fetching artifact or note bytes", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      let result: unknown;
      if (url.includes("/sessions?")) result = { sessions: [{ session_id: "rec-1", title: "Pinned" }], next_cursor: null };
      else if (url.endsWith("/transcript")) result = { terminal: true, live: false, updated_at_unix_ms: 12, decoded_until_ms: 1000, body: "must not cross host contract" };
      else if (url.endsWith("/memo")) result = { revision: "memo-2", lines: [{ text: "private memo" }] };
      else if (url.endsWith("/artifacts")) result = [{ artifact_id: "artifact-1", kind: "transcript", retention_class: "session" }];
      else if (url.endsWith("/note-association")) result = { source_id: "notes", relative_path: "Meetings/pinned.md", revision: 3 };
      else throw new Error(`unexpected URL ${url}`);
      return new Response(JSON.stringify({ ok: true, result }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    const result = await new ProjectMarginsTransport(manager).connectedNoteContext({ projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1");
    expect(result).toMatchObject({ ok: true, context: { instanceId: "instance-remote", workspaceId: "practice", sessionId: "rec-1", transcript: { available: true, terminal: true }, memo: { lineCount: 1 }, noteAssociation: { relativePath: "Meetings/pinned.md" } } });
    expect(JSON.stringify(result)).not.toContain("private memo");
    expect(JSON.stringify(result)).not.toContain("must not cross host contract");
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/artifacts/artifact-1/content"))).toBe(false);
  });

  it("requests durable transcription when a terminal live checkpoint contains no decoded audio", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    vi.stubGlobal("fetch", vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      let result: unknown;
      if (url.includes("/sessions?")) result = { sessions: [{ session_id: "rec-1", title: "Saved capture" }] };
      else if (url.endsWith("/transcript")) result = { terminal: true, live: false, updated_at_unix_ms: 12,
        decoded_until_ms: 0, body: "# Capture Context\n## Timeline\n[00:05] memo: Decision" };
      else if (url.endsWith("/memo")) result = { revision: "memo-1", lines: [{ text: "Decision" }] };
      else if (url.endsWith("/artifacts")) result = [];
      else if (url.endsWith("/note-association")) result = null;
      else throw new Error(`unexpected URL ${url}`);
      return new Response(JSON.stringify({ ok: true, result }), { status: 200 });
    }));
    const result = await new ProjectMarginsTransport(manager).connectedNoteContext(
      { projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1",
    );
    expect(result).toMatchObject({ ok: true, context: { transcript: { available: false } } });
  });

  it("reports a saved capture awaiting transcription without exposing note bytes", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    vi.stubGlobal("fetch", vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      if (url.endsWith("/transcript")) return new Response(JSON.stringify({
        ok: false,
        error: { code: "invalid_request", message: "No aligned transcript or capture context found for 'rec-1'." },
      }), { status: 422 });
      let result: unknown;
      if (url.includes("/sessions?")) result = { sessions: [{ session_id: "rec-1", title: "Saved capture" }] };
      else if (url.endsWith("/memo")) result = { revision: "memo-1", lines: [] };
      else if (url.endsWith("/artifacts")) result = [];
      else if (url.endsWith("/note-association")) result = null;
      else throw new Error(`unexpected URL ${url}`);
      return new Response(JSON.stringify({ ok: true, result }), { status: 200 });
    }));
    const result = await new ProjectMarginsTransport(manager).connectedNoteContext(
      { projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1",
    );
    expect(result).toMatchObject({ ok: true, context: { sessionId: "rec-1", transcript: { available: false, terminal: false, live: false } } });
    expect(JSON.stringify(result)).toContain("obtain or wait for transcription of this exact session");
  });

  it("does not mistake another transcript failure for pending transcription", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    vi.stubGlobal("fetch", vi.fn(async (input: string | URL | Request) => {
      if (String(input).endsWith("/transcript")) return new Response(JSON.stringify({
        ok: false, error: { code: "invalid_request", message: "Transcript rejected for another reason" },
      }), { status: 422 });
      return new Response(JSON.stringify({ ok: true, result: {} }), { status: 200 });
    }));
    const result = await new ProjectMarginsTransport(manager).connectedNoteContext(
      { projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1",
    );
    expect(result).toMatchObject({ ok: false, error: { code: "connected_note_context_unavailable" } });
  });

  it("requests transcription for an exact saved session over the selected Workspace", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "https://margins.example.test", token: "scoped-token", workspaceId: "practice", instanceId: "instance-remote",
    })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ ok: true, result: {
      status: "queued", attempt: 1,
    } }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    const result = await new ProjectMarginsTransport(manager).requestTranscription(
      { projectId: "project", projectRoot: "/tmp/project", hostId: "host" }, "/tmp/data", "rec-1",
    );
    expect(result).toEqual({ ok: true, status: "queued", attempt: 1 });
    expect(fetchMock).toHaveBeenCalledWith(
      "https://margins.example.test/v1/workspaces/practice/sessions/rec-1/jobs/transcribe",
      expect.objectContaining({ method: "POST", headers: expect.objectContaining({ authorization: "Bearer scoped-token" }) }),
    );
  });

  it("transcribes a local browser capture from its finalized WAV without a remote producer job", async () => {
    const manager = { ensure: vi.fn(async () => ({
      baseUrl: "http://127.0.0.1:8787", token: "scoped-token", workspaceId: "practice", instanceId: "instance-local", child: {},
    })) } as unknown as ProjectServerManager;
    const fetchMock = vi.fn(async (input: string | URL | Request) => {
      const url = String(input);
      if (url.endsWith("/browser/sessions")) return new Response(JSON.stringify({ ok: true, result: {
        recordingId: "browser-1", sessionId: "meeting-1", status: "recording", notepad: { text: "", revision: "memo-1" },
      } }));
      if (url.endsWith("/api/invoke/transcribe_hosted_browser_session")) return new Response(JSON.stringify({ ok: true, result: "1 transcript entry" }));
      throw new Error(`unexpected URL ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const transport = new ProjectMarginsTransport(manager);
    const target = { projectId: "project", projectRoot: "/tmp/project", hostId: "host" };
    expect(await transport.start(target, "/tmp/data", "owner-1", "meeting-1")).toMatchObject({ ok: true });
    expect(await transport.requestTranscription(target, "/tmp/data", "meeting-1"))
      .toMatchObject({ ok: true, status: "complete" });
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/jobs/transcribe"))).toBe(false);
  });

  it("rejects incomplete or non-TLS remote configuration before transport", async () => {
    process.env.MARGINS_BB_REMOTE_URL = "http://remote.example.test";
    process.env.MARGINS_BB_REMOTE_TOKEN = "scoped-token";
    process.env.MARGINS_BB_REMOTE_WORKSPACE = "practice";
    await expect(new ProjectServerManager().ensure({
      projectId: "project",
      projectRoot: "/tmp/project",
      hostId: "host",
      workspaceId: "practice",
    }, "/tmp/plugin-data")).rejects.toThrow("HTTPS or loopback");
  });
});

describe("scoped ASR runtime configuration", () => {
  it("selects only complete absolute paths inside the plugin data scope", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-bb-asr-config-"));
    try {
      const serverPath = join(root, "margins-server");
      const modelDir = join(root, "model");
      const ortLibraryPath = join(root, "libonnxruntime.so");
      await mkdir(modelDir);
      await writeFile(serverPath, "binary");
      await chmod(serverPath, 0o755);
      await writeFile(ortLibraryPath, "library");
      await writeFile(join(root, "asr-runtime.json"), JSON.stringify({ serverPath, modelDir, ortLibraryPath }));
      await expect(readAsrRuntimeConfig(root)).resolves.toEqual({ serverPath, modelDir, ortLibraryPath });
      await writeFile(join(root, "asr-runtime.json"), JSON.stringify({ serverPath: "../margins-server", modelDir, ortLibraryPath }));
      await expect(readAsrRuntimeConfig(root)).rejects.toThrow("absolute path");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
