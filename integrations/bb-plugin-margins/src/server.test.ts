import { createFakePluginHost, makePluginAgentConfigurationContext } from "@get-bb/plugin-sdk/testing";
import { describe, expect, it, vi } from "vitest";
import plugin from "./server.js";
import { meetingMentionId } from "./meeting-mention.js";

const browser = { clientId: "client-1", platform: "other" as const, secureContext: true, browserMicrophone: true, nativeMacCapture: false };
const mac = { ...browser, platform: "macos" as const };
const snapshot = { recordingId: "rec-1", sessionId: "rec-1", status: "recording" as const, notepad: { text: "", revision: "v1" } };

function harness(options: { heartbeatFails?: boolean; canonicalSessionId?: string; meetingList?: boolean; relay?: boolean; workspaceHostFails?: boolean; menuMeeting?: boolean; transcriptReady?: boolean; transcriptPartial?: boolean } = {}) {
  let stopped = false;
  let captureStatus: "recording" | "paused" = "recording";
  const spawn = vi.fn(async (_request: unknown) => ({ id: "thr-created" }));
  const host = createFakePluginHost({
    pluginId: "margins", agentSkillIds: ["watermark", "workspace-setup", "connected-note"],
    sdk: {
      threads: { get: async ({ threadId }: { threadId: string }) => ({ id: threadId, title: threadId === "thr-note" ? "Create connected meeting note" : null,
        projectId: threadId === "thr-other-project" ? "proj-2" : "proj-1" }) as never, spawn: spawn as never },
      projects: { get: async ({ projectId }: { projectId: string }) => ({
        id: projectId, kind: "standard", name: "Project",
        sources: [
          { id: "secondary", projectId, hostId: "wrong-host", path: "/tmp/worktree", isDefault: false, type: "local_path", createdAt: 1, updatedAt: 1 },
          { id: "primary", projectId, hostId: "project-host", path: projectId === "proj-2" ? "/srv/other" : "/srv/project", isDefault: true, type: "local_path", createdAt: 1, updatedAt: 1 },
        ], createdAt: 1, updatedAt: 1, gitRemoteUrl: null,
      }) as never },
    },
    experimental_callHostRpc: ({ method }) => {
      if (method === "workspaceOptions" && options.workspaceHostFails) throw new Error("Host artifact unavailable");
      if (method === "workspaceOptions") return { defaultWorkspaceId: "workspace-1", autoSelected: false,
        workspaces: [{ id: "workspace-1", name: "Notes" }, { id: "practice", name: "Practice" }] };
      if (method === "listWorkspaceMeetings" && options.menuMeeting) return { ok: true, meetings: [{
        sessionId: "mac-meeting-1", title: "Calendar review", startedAt: "2026-09-28T13:00:00Z",
        inputFinalized: false, notePath: null, threadIds: [], distilledMemoRevision: null,
      }] };
      if (method === "listWorkspaceMeetings" && options.meetingList) return { ok: true, meetings: [{
        sessionId: "meeting-1", title: null, startedAt: "2026-09-25T01:00:00Z", inputFinalized: true,
        notePath: "inbox/note.md", noteFile: { hostId: "project-host", path: "/tmp/vault/inbox/note.md" },
        threadIds: ["thr-note"], distilledMemoRevision: "memo-v1",
      }] };
      if (method === "readWorkspaceMeeting" && options.meetingList) return { ok: true, candidates: ["meeting-1"], meeting: {
        sessionId: "meeting-1", title: null, startedAt: "2026-09-25T01:00:00Z", inputFinalized: true,
        notepad: { text: "Memo", revision: "memo-v1" },
      } };
      if (method === "readWorkspaceMeeting") return { ok: true, candidates: ["rec-1"], meeting: { sessionId: "rec-1", title: "Customer call",
        startedAt: "2026-09-28T13:00:00Z", inputFinalized: true, notepad: { text: "Memo", revision: "memo-1" } } };
      if (method === "readWorkspaceTranscript") return { ok: true, body: "We chose October 5 for launch." };
      if (method === "noteDestination") return { ok: true, destination: "/srv/project/inbox", homeRoot: "/srv/project", homeSourceId: "home" };
      if (method === "linkWorkspaceNote") return { ok: true, revision: 1 };
      if (method === "captureAuthority") return { ok: true, instanceId: "instance-1", workspaceId: "workspace-1" };
      if (method === "relayWorkspaceHttp" && options.relay) return { status: 200, bodyBase64: Buffer.from('{"ok":true}').toString("base64") };
      if (method === "sessionExists") return { ok: true, found: true };
      if (method === "stop") { stopped = true; return { ok: true, snapshot: null }; }
      if (method === "pause") captureStatus = "paused";
      if (method === "resume") captureStatus = "recording";
      if (method === "uploadChunk") return { ok: true };
      if (method === "connectedNoteContext") return { ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-1", title: "Customer call",
        transcript: { available: options.transcriptReady !== false, terminal: options.transcriptReady !== false && !options.transcriptPartial, live: options.transcriptPartial === true, updatedAtUnixMs: 10 }, memo: { revision: "memo-1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "Pin exact session",
      } };
      if (method === "requestTranscription") return { ok: true, status: "queued", attempt: 1 };
      if (method === "heartbeat" && options.heartbeatFails) return { ok: false, error: { code: "offline", message: "offline", retryable: true } };
      return { ok: true, snapshot: stopped ? null : { ...snapshot, sessionId: options.canonicalSessionId || snapshot.sessionId, status: captureStatus } };
    },
  });
  plugin(host.bb);
  return { ...host, spawn };
}

describe("Margins project recording server", () => {
  it("gives project agents pinned BB meeting reads and a note link tool", async () => {
    const host = harness();
    const config = await host.harness.behavior.resolveAgentConfiguration(makePluginAgentConfigurationContext({
      project: { id: "proj-1", kind: "standard", name: "Project" },
    }));
    expect(config?.tools.map((tool) => tool.name)).toEqual(["margins_bb_meeting_read", "margins_bb_note_link"]);
    const pin = { workspaceId: "workspace-1", sessionId: "rec-1", memoRevision: "memo-1" };
    const context = await host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, part: "context" });
    expect(context).toContain('"homeSourceId":"home"');
    const memo = await host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, part: "memo" });
    expect(memo).toContain('"body":"Memo"');
    const transcript = await host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, part: "transcript" });
    expect(transcript).toContain("We chose October 5 for launch.");
    await expect(host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, workspaceId: "other", part: "memo" }))
      .rejects.toThrow("different Workspace");
    await expect(host.harness.behavior.callAgentTool("margins_bb_note_link", { ...pin, sourceId: "home", relativePath: "inbox/note.md", expectedRevision: 0 }))
      .resolves.toContain('"revision":1');
  });
  it("reports a Workspace host failure without masking it as RPC output validation", async () => {
    const host = harness({ workspaceHostFails: true });
    await expect(host.harness.behavior.callRpc("availableWorkspaces", { projectId: "proj-1" }))
      .rejects.toThrow("Host artifact unavailable");
  });
  it("resolves a pinned meeting pill to hidden agent context and rejects a stale memo", async () => {
    const host = harness();
    const provider = host.harness.registrations.mentionProviders.find((item) => item.id === "margins")!;
    const id = meetingMentionId({ projectId: "proj-1", workspaceId: "workspace-1", sessionId: "rec-1",
      memoRevision: "memo-1", note: "create" });
    await expect(provider.resolve(id)).resolves.toEqual({ context: `<margins-context-v1>\n${JSON.stringify({
      workspaceId: "workspace-1", sessionId: "rec-1", memoRevision: "memo-1", bbProjectId: "proj-1",
      transcript: "ready", note: "create",
    })}\n</margins-context-v1>` });
    await expect(provider.resolve(meetingMentionId({ projectId: "proj-1", workspaceId: "workspace-1", sessionId: "rec-1",
      memoRevision: "old", note: "create" }))).rejects.toThrow("This meeting changed");
  });
  it("starts a connected-note thread in the meeting project with a pinned mention", async () => {
    const host = harness();
    await expect(host.harness.behavior.callRpc("startConnectedNoteThread", { projectId: "proj-1", sessionId: "rec-1" }))
      .resolves.toEqual({ threadId: "thr-created" });
    expect(host.spawn).toHaveBeenCalledWith(expect.objectContaining({
      projectId: "proj-1", environment: { type: "project-default" },
      input: [expect.objectContaining({ type: "text", mentions: [expect.objectContaining({
        resource: expect.objectContaining({ kind: "plugin", pluginId: "margins" }),
      })] })],
    }));
    const request = host.spawn.mock.calls[0]![0] as { input: [{ mentions: [{ resource: { itemId: string } }] }] };
    expect(request.input[0].mentions[0].resource.itemId).toBe(`margins:${meetingMentionId({
      projectId: "proj-1", workspaceId: "workspace-1", sessionId: "rec-1", memoRevision: "memo-1", note: "create",
    })}`);
    await host.bb.storage.kv.set("meeting-origin:workspace-1:rec-1", "proj-1");
    await expect(host.harness.behavior.callRpc("startConnectedNoteThread", { projectId: "proj-2", sessionId: "rec-1" }))
      .rejects.toThrow("belongs to another BB project");
    expect(host.spawn).toHaveBeenCalledTimes(1);
  });
  it("starts a note thread only from Make note, ignoring old pending keys", async () => {
    const host = harness();
    await host.bb.storage.kv.set("auto-note-pending:workspace-1:rec-1", { projectId: "proj-1", workspaceId: "workspace-1", sessionId: "rec-1" });
    await host.harness.behavior.callRpc("recordMeetingOrigin", { projectId: "proj-1", sessionId: "rec-1" });
    await host.harness.behavior.callRpc("listWorkspaceMeetings", { projectId: "proj-1" });
    expect(host.spawn).not.toHaveBeenCalled();
    await expect(host.harness.behavior.callRpc("startConnectedNoteThread", { projectId: "proj-1", sessionId: "rec-1" }))
      .resolves.toEqual({ threadId: "thr-created" });
    expect(host.spawn).toHaveBeenCalledOnce();
  });
  it("starts the requested note thread before transcription, but refuses transcript reads", async () => {
    const host = harness({ transcriptReady: false });
    await expect(host.harness.behavior.callRpc("startConnectedNoteThread", { projectId: "proj-1", sessionId: "rec-1" }))
      .resolves.toEqual({ threadId: "thr-created" });
    expect(host.spawn).toHaveBeenCalledOnce();
    const pin = { workspaceId: "workspace-1", sessionId: "rec-1", memoRevision: "memo-1" };
    await expect(host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, part: "transcript" }))
      .rejects.toThrow("transcript is not complete yet");
  });
  it("refuses partial transcript reads from a note thread started by Make note", async () => {
    const host = harness({ transcriptPartial: true });
    await expect(host.harness.behavior.callRpc("startConnectedNoteThread", { projectId: "proj-1", sessionId: "rec-1" }))
      .resolves.toEqual({ threadId: "thr-created" });
    const pin = { workspaceId: "workspace-1", sessionId: "rec-1", memoRevision: "memo-1" };
    await expect(host.harness.behavior.callAgentTool("margins_bb_meeting_read", { ...pin, part: "transcript" }))
      .rejects.toThrow("transcript is not complete yet");
  });
  it("issues a Workspace-bound Menu grant and revokes its relay access", async () => {
    const host = harness({ relay: true, menuMeeting: true });
    const grant = await host.harness.behavior.callRpc("issueMenuGrant", { projectId: "proj-1", origin: "https://jpham-server.getbb.app" }) as { token: string; serviceUrl: string; workspaceId: string };
    expect(grant).toMatchObject({ workspaceId: "workspace-1",
      serviceUrl: "https://jpham-server.getbb.app/api/v1/plugins/margins/http/menu/relay" });
    const auth = { authorization: `Bearer ${grant.token}` };
    const verified = await host.harness.behavior.fetchHttp("POST", "/menu/verify", { headers: auth });
    await expect(verified.json()).resolves.toMatchObject({ ok: true, workspaceId: "workspace-1" });
    const relayed = await host.harness.behavior.fetchHttp("POST", "/menu/relay", { headers: { ...auth, "content-type": "application/json" },
      body: JSON.stringify({ method: "GET", path: "v1/workspaces/workspace-1/current", bodyBase64: "" }) });
    await expect(relayed.json()).resolves.toMatchObject({ status: 200 });
    const created = await host.harness.behavior.fetchHttp("POST", "/menu/relay", { headers: { ...auth, "content-type": "application/json" },
      body: JSON.stringify({ method: "POST", path: "v1/workspaces/workspace-1/sessions",
        bodyBase64: Buffer.from(JSON.stringify({ session_id: "mac-meeting-1" })).toString("base64") }) });
    await expect(created.json()).resolves.toMatchObject({ status: 200 });
    await expect(host.bb.storage.kv.get("meeting-origin:workspace-1:mac-meeting-1")).resolves.toBe("proj-1");
    await expect(host.bb.storage.kv.get("auto-note-pending:workspace-1:mac-meeting-1")).resolves.toBeUndefined();
    await expect(host.harness.behavior.callRpc("listWorkspaceMeetings", { projectId: "proj-1" })).resolves.toMatchObject({
      ok: true, meetings: [{ sessionId: "mac-meeting-1", inputFinalized: false, originProjectId: "proj-1" }],
    });
    expect(host.harness.inspection.experimental_hostRpcCalls.some(call => call.method === "startBrowserCapture")).toBe(false);
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "relayWorkspaceHttp", input: expect.objectContaining({ target: expect.objectContaining({ workspaceId: "workspace-1" }) }) }),
    ]));
    await host.harness.behavior.callRpc("revokeMenuGrants", { projectId: "proj-2" });
    const rejected = await host.harness.behavior.fetchHttp("POST", "/menu/verify", { headers: auth });
    expect(rejected.status).toBe(401);
  });
  it("uses the bb page's HTTPS origin while refusing insecure remote origins", async () => {
    const host = harness();
    const grant = await host.harness.behavior.callRpc("issueMenuGrant", {
      projectId: "proj-1", origin: "https://jpham-server.taildd119e.ts.net:8443",
    }) as { serviceUrl: string };
    expect(grant.serviceUrl).toBe("https://jpham-server.taildd119e.ts.net:8443/api/v1/plugins/margins/http/menu/relay");
    await expect(host.harness.behavior.callRpc("issueMenuGrant", {
      projectId: "proj-1", origin: "http://100.107.201.27:38886",
    })).rejects.toThrow("HTTPS or localhost");
  });
  it("resolves linked thread titles while retaining the host note preview target", async () => {
    const host = harness({ meetingList: true });
    await expect(host.harness.behavior.callRpc("listWorkspaceMeetings", { projectId: "proj-1" })).resolves.toMatchObject({ ok: true,
      meetings: [{ noteFile: { hostId: "project-host", path: "/tmp/vault/inbox/note.md" },
        threadLinks: [{ id: "thr-note", title: "Create connected meeting note" }] }],
    });
  });
  it("shares archive state and recording provenance across projects in one Workspace", async () => {
    const host = harness({ meetingList: true });
    await expect(host.harness.behavior.callRpc("recordMeetingOrigin", { projectId: "proj-1", sessionId: "meeting-1" })).resolves.toEqual({ ok: true });
    await expect(host.harness.behavior.callRpc("archiveWorkspaceMeeting", { projectId: "proj-1", sessionId: "meeting-1", archived: true })).resolves.toEqual({ ok: true });
    await expect(host.harness.behavior.callRpc("listWorkspaceMeetings", { projectId: "proj-2" })).resolves.toMatchObject({ ok: true,
      meetings: [{ sessionId: "meeting-1", workspaceId: "workspace-1", originProjectId: "proj-1", archived: true }],
    });
    await expect(host.harness.behavior.callRpc("archiveWorkspaceMeeting", { projectId: "proj-2", sessionId: "meeting-1", archived: false })).resolves.toEqual({ ok: true });
    await expect(host.harness.behavior.callRpc("listWorkspaceMeetings", { projectId: "proj-1" })).resolves.toMatchObject({ ok: true,
      meetings: [{ sessionId: "meeting-1", archived: false }],
    });
  });
  it("keys ownership by the Workspace session while routing audio by browser recording id", async () => {
    const host = harness({ canonicalSessionId: "meeting-2" });
    const started = await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner" });
    expect(started).toMatchObject({ recordingId: "rec-1", sessionId: "meeting-2" });
    await expect(host.bb.storage.kv.get("session:meeting-2")).resolves.toMatchObject({ sessionId: "meeting-2", recordingId: "rec-1" });
    await expect(host.bb.storage.kv.get("live:workspace-1")).resolves.toBe("meeting-2");
    await expect(host.bb.storage.kv.get("recording:rec-1")).resolves.toBe("meeting-2");
    await host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-canonical", expectedNextSequence: 0 });
    await expect(host.bb.storage.kv.get("last-session:workspace-1")).resolves.toBe("meeting-2");
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "stop", input: expect.objectContaining({ recordingId: "rec-1" }) }),
    ]));
  });
  it("routes controls by session across project views", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner" });
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1", ownsRecording: true });
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-2", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1", ownsRecording: true });
    expect(host.harness.inspection.experimental_hostRpcCalls).toEqual(expect.arrayContaining([
      expect.objectContaining({ method: "readCapture", input: expect.objectContaining({ target: expect.objectContaining({ projectId: "proj-1", projectRoot: "/srv/project" }) }) }),
    ]));
    await expect(host.harness.behavior.callRpc("pause", { sessionId: "rec-1", client: browser, operationId: "pause-1", expectedNextSequence: 0 })).resolves.toMatchObject({ state: "paused" });
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: browser })).resolves.toMatchObject({ state: "paused" });
    await expect(host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-1", expectedNextSequence: 0 })).resolves.toMatchObject({ state: "saved" });
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
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: mac })).resolves.toMatchObject({ state: "ready", sourceLabel: "Microphone only", primaryAction: "start" });
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: { ...mac, browserMicrophone: false } })).resolves.toMatchObject({ state: "needs_setup", sourceLabel: null });
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: browser })).resolves.toMatchObject({ state: "ready", sourceLabel: "Microphone only", storageLabel: "Saves to your Workspace", primaryLabel: "Use browser microphone", detail: expect.stringContaining("choose Connected Workspace") });
    await expect(host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: mac, ownerId: "mac-owner" })).resolves.toMatchObject({ state: "recording", sourceLabel: "Microphone only" });
  });

  it("routes capture to the stable primary project folder and stores no note or audio bodies", async () => {
    const host = harness();
    const started = await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner-secret", title: "Customer call" });
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
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: browser })).resolves.toMatchObject({ state: "recording", recordingId: "rec-1" });
  });

  it("makes controls client-owned but lets another project client see that recording exists", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner-secret" });
    const other = { ...browser, clientId: "client-2" };
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: other })).resolves.toMatchObject({ state: "recording_elsewhere", canStop: false, canEditNotepad: false });
  });

  it("does not renew the disconnect lease when the project rejects a heartbeat", async () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    const host = harness({ heartbeatFails: true });
    await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner-secret" });
    now.mockReturnValue(9_000);
    await expect(host.harness.behavior.callRpc("heartbeat", { sessionId: "rec-1", client: browser, operationId: "heartbeat-1" })).resolves.toMatchObject({ state: "needs_attention" });
    await expect(host.bb.storage.kv.get("session:rec-1")).resolves.toMatchObject({ lastHeartbeatUnixMs: 1_000 });
    now.mockRestore();
  });

  it("reconciles a repeated Stop from its durable control receipt", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner-secret" });
    const input = { sessionId: "rec-1", client: browser, operationId: "stop-1", expectedNextSequence: 0 };
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.harness.behavior.callRpc("stop", input)).resolves.toMatchObject({ state: "saved" });
    await expect(host.bb.storage.kv.get("session:rec-1")).resolves.toBeUndefined();
    await expect(host.bb.storage.kv.get("live:workspace-1")).resolves.toBeUndefined();
    await expect(host.bb.storage.kv.get("last-session:workspace-1")).resolves.toBe("rec-1");
    await expect(host.harness.behavior.callRpc("getProjectPanelState", { projectId: "proj-1", client: browser })).resolves.toMatchObject({ state: "ready", lastSessionId: "rec-1" });
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "stop")).toHaveLength(1);
  });

  it("resolves connected-note context for an explicitly selected ended session", async () => {
    const host = harness();
    await host.harness.behavior.callRpc("beginProjectCapture", { projectId: "proj-1", client: browser, ownerId: "owner-secret" });
    await host.harness.behavior.callRpc("stop", { sessionId: "rec-1", client: browser, operationId: "stop-pinned", expectedNextSequence: 0 });
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
    await expect(host.bb.storage.kv.get("meeting-origin:workspace-1:mac-session-1")).resolves.toBeUndefined();
    expect(host.harness.inspection.experimental_hostRpcCalls.filter(call => call.method === "sessionExists")).toHaveLength(1);
  });
});
