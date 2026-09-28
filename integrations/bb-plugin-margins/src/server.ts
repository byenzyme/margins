import type { BbPluginApi } from "@get-bb/plugin-sdk";
import { createHash, randomBytes } from "node:crypto";
import { z } from "zod";
import { parseMeetingMentionId } from "./meeting-mention.js";
import {
  PANEL_STATE_SCHEMA,
  captureRecordSchema,
  clientCapabilitiesSchema,
  hostSignals,
  marginsHostContract,
  marginsRpcContract,
  CAPTURE_DISCONNECT_GRACE_MS,
  type CaptureRecord,
  type ClientCapabilities,
  type HostError,
  type HostResult,
  type ConnectedNoteResult,
  type TranscriptionRequestResult,
  type PanelState,
  type ProjectTarget,
} from "./contracts.js";

const SESSION_PREFIX = "session:";
const RECORDING_PREFIX = "recording:";
const LIVE_PREFIX = "live:";
const LAST_SESSION_PREFIX = "last-session:";
const PROJECT_WORKSPACE_PREFIX = "project-workspace:";
const MEETING_ORIGIN_PREFIX = "meeting-origin:";
const MEETING_ARCHIVE_PREFIX = "meeting-archive:";
const MENU_GRANT_PREFIX = "menu-grant:";
const MENU_GRANT_EPOCH_PREFIX = "menu-grant-epoch:";
const MENU_GRANT_TTL_MS = 60 * 60 * 1_000;
const REALTIME_CHANNEL = "margins-recording";
export const DISCONNECT_GRACE_MS = CAPTURE_DISCONNECT_GRACE_MS;

function sessionKey(sessionId: string) { return `${SESSION_PREFIX}${sessionId}`; }
function recordingKey(recordingId: string) { return `${RECORDING_PREFIX}${recordingId}`; }
function liveKey(workspaceId: string) { return `${LIVE_PREFIX}${workspaceId}`; }
function lastSessionKey(workspaceId: string) { return `${LAST_SESSION_PREFIX}${workspaceId}`; }
function originKey(workspaceId: string, sessionId: string) { return `${MEETING_ORIGIN_PREFIX}${workspaceId}:${sessionId}`; }
function archiveKey(workspaceId: string, sessionId: string) { return `${MEETING_ARCHIVE_PREFIX}${workspaceId}:${sessionId}`; }
function meetingName(value: string | null | undefined) {
  const slug = (value || "meeting").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 90);
  return slug || "meeting";
}

function stateCopy(state: PanelState["state"], sourceLabel: string | null, error: HostError | null) {
  switch (state) {
    case "needs_setup": return {
      title: "Enable recording on this Mac",
      detail: "Margins can capture your microphone and conversations playing on this Mac. Recording starts only when you press Start, and stops and saves if this bb window stays disconnected.",
      primaryAction: "none" as const, primaryLabel: "Recording unavailable",
    };
    case "ready": return {
      title: "Record with your browser", detail: "Browser microphone only. To capture computer audio here, choose Connected Workspace in Margins Menu on your Mac.",
      primaryAction: "start" as const, primaryLabel: "Use browser microphone",
    };
    case "getting_ready": return {
      title: "Getting recording ready", detail: "Nothing is being recorded until your microphone and the project are both ready.",
      primaryAction: "none" as const, primaryLabel: "Getting ready",
    };
    case "recording": return {
      title: "Recording", detail: `${sourceLabel}. Audio and notes are being saved to your Workspace.`,
      primaryAction: "pause" as const, primaryLabel: "Pause",
    };
    case "paused": return {
      title: "Paused", detail: "Audio is paused. Your recording and notes received so far are safe in your Workspace.",
      primaryAction: "resume" as const, primaryLabel: "Resume",
    };
    case "recovering": return {
      title: "Reconnecting", detail: "Audio and notes already received are safe. Margins will stop and save if this bb window does not reconnect shortly.",
      primaryAction: "retry" as const, primaryLabel: "Reconnect",
    };
    case "saving": return {
      title: "Saving", detail: "Capture has stopped. Margins is finishing the audio already received by your Workspace.",
      primaryAction: "none" as const, primaryLabel: "Saving",
    };
    case "saved": return {
      title: "Meeting saved", detail: "Your recording and notes are safe in your Workspace. A transcript may take a little longer.",
      primaryAction: "none" as const, primaryLabel: "Meeting saved",
    };
    case "recording_elsewhere": return {
      title: "Recording from another bb window", detail: "This Workspace already has a recording. Return to the window with the microphone for recording controls.",
      primaryAction: "none" as const, primaryLabel: "Recording elsewhere",
    };
    case "needs_attention": return {
      title: "Recording needs attention", detail: error?.message || "Audio already received is safe. Try reconnecting from the bb window that started recording.",
      primaryAction: error?.retryable ? "retry" as const : "none" as const,
      primaryLabel: error?.retryable ? "Try again" : "Recording unavailable",
    };
    default: return {
      title: "Recording is not available here", detail: error?.message || "This browser cannot provide a microphone recording.",
      primaryAction: "none" as const, primaryLabel: "Recording unavailable",
    };
  }
}

function sourceFor(client: ClientCapabilities) {
  if (client.platform === "macos" && client.nativeMacCapture) return "Microphone + computer audio";
  return client.secureContext && client.browserMicrophone ? "Microphone only" : null;
}

export default function marginsPlugin(bb: BbPluginApi) {
  const host = bb.hosts.experimental_client({ contract: marginsHostContract, experimental_signals: hostSignals });
  const startLocks = new Map<string, Promise<void>>();
  const menuGrantSchema = z.object({ target: z.object({ projectId: z.string(), hostId: z.string(), projectRoot: z.string(), workspaceId: z.string() }).strict(),
    origin: z.string(), instanceId: z.string(), workspaceId: z.string(), epoch: z.number().int(), expiresAt: z.number().int() }).strict();
  const grantKey = (token: string) => `${MENU_GRANT_PREFIX}${createHash("sha256").update(token).digest("hex")}`;
  const grantEpoch = async (workspaceId: string) => Number(await bb.storage.kv.get(`${MENU_GRANT_EPOCH_PREFIX}${workspaceId}`) || 0);
  async function readMenuGrant(token: string) {
    if (!/^[A-Za-z0-9_-]{40,60}$/.test(token)) return null;
    const parsed = menuGrantSchema.safeParse(await bb.storage.kv.get(grantKey(token)));
    if (!parsed.success || parsed.data.expiresAt < Date.now()
      || parsed.data.epoch !== await grantEpoch(parsed.data.workspaceId)) return null;
    return parsed.data;
  }

  host.experimental_onSignal("changed", ({ payload }) => bb.realtime.publish(REALTIME_CHANNEL, payload));
  host.experimental_onWorkerExit(({ hostId }) => bb.realtime.publish(REALTIME_CHANNEL, { hostId, reason: "project-recorder-offline" }));

  async function targetForThread(threadId: string): Promise<ProjectTarget> {
    const thread = await bb.sdk.threads.get({ threadId }) as unknown as { projectId: string };
    return targetForProject(thread.projectId);
  }

  async function targetForProject(projectId: string): Promise<ProjectTarget> {
    const project = await bb.sdk.projects.get({ projectId });
    if (project.kind === "personal") throw new Error("Choose a project with a stable folder before recording.");
    const source = project.sources.find((candidate) => candidate.isDefault);
    if (!source) throw new Error("This project does not have a primary folder for recordings.");
    const workspaceId = await bb.storage.kv.get(`${PROJECT_WORKSPACE_PREFIX}${project.id}`);
    return { projectId: project.id, hostId: source.hostId, projectRoot: source.path,
      ...(typeof workspaceId === "string" && workspaceId ? { workspaceId } : {}) };
  }

  async function targetForSelection(input: { threadId?: string; projectId?: string }): Promise<ProjectTarget> {
    if (input.projectId) return targetForProject(input.projectId);
    if (input.threadId) return targetForThread(input.threadId);
    throw new Error("Choose a bb project for this meeting.");
  }

  async function readCapture(sessionId: string): Promise<CaptureRecord | null> {
    const canonical = await bb.storage.kv.get(recordingKey(sessionId));
    const parsed = captureRecordSchema.safeParse(await bb.storage.kv.get(sessionKey(typeof canonical === "string" ? canonical : sessionId)));
    return parsed.success ? parsed.data : null;
  }
  async function readLiveCapture(workspaceId: string) {
    const sessionId = await bb.storage.kv.get(liveKey(workspaceId));
    return typeof sessionId === "string" ? readCapture(sessionId) : null;
  }
  async function saveCapture(value: CaptureRecord) {
    await bb.storage.kv.set(sessionKey(value.sessionId), value);
    await bb.storage.kv.set(recordingKey(value.recordingId), value.sessionId);
    await bb.storage.kv.set(liveKey(value.workspaceId), value.sessionId);
  }
  async function clearCapture(value: CaptureRecord) {
    await bb.storage.kv.delete(sessionKey(value.sessionId));
    await bb.storage.kv.delete(recordingKey(value.recordingId));
    if (await bb.storage.kv.get(liveKey(value.workspaceId)) === value.sessionId) {
      await bb.storage.kv.delete(liveKey(value.workspaceId));
    }
  }
  async function readLastSession(workspaceId: string) {
    const value = await bb.storage.kv.get(lastSessionKey(workspaceId));
    return typeof value === "string" && value.length > 0 ? value : null;
  }
  async function callHost(target: ProjectTarget, method: keyof typeof marginsHostContract, input: object): Promise<any> {
    try {
      return await host.call(method as never, input as never, { hostId: target.hostId });
    } catch (cause) {
      return { ok: false, error: { code: "project_machine_offline", message: "Margins could not reach this bb project's machine. Audio already received there is safe; reconnect the project machine and try again.", retryable: true } };
    }
  }

  function basePanel(projectId: string | null, state: PanelState["state"], client: ClientCapabilities, options: {
    capture?: CaptureRecord | null; notepad?: PanelState["notepad"];
    error?: HostError | null; lastSessionId?: string | null;
  } = {}): PanelState {
    const sourceLabel = sourceFor(client);
    const copy = stateCopy(state, sourceLabel, options.error || null);
    const owns = Boolean(options.capture && options.capture.clientId === client.clientId);
    return {
      schema: PANEL_STATE_SCHEMA, state, ...copy,
      sourceLabel, storageLabel: projectId ? (state === "saved" ? "Saved to your Workspace" : "Saves to your Workspace") : null,
      canStop: owns && ["recording", "paused", "recovering"].includes(state),
      canEditNotepad: owns && ["recording", "paused", "recovering"].includes(state),
      ownsRecording: owns, recordingId: options.capture?.recordingId || null,
      sessionId: options.capture?.sessionId || options.lastSessionId || null,
      notepad: options.notepad || null,
      lastSessionId: options.lastSessionId || null,
      error: options.error || null,
    };
  }

  async function getPanelState(threadId: string, client: ClientCapabilities): Promise<PanelState> {
    let target: ProjectTarget;
    try { target = await targetForThread(threadId); }
    catch (cause) {
      const error = { code: "project_folder_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: false };
      return basePanel(null, "unavailable", client, { error });
    }
    return getPanelStateForTarget(target, client);
  }

  async function getPanelStateForTarget(target: ProjectTarget, client: ClientCapabilities): Promise<PanelState> {
    const authority = await callHost(target, "captureAuthority", { target });
    if (!authority.ok) return basePanel(target.projectId, "unavailable", client, { error: authority.error });
    const workspaceId = authority.workspaceId as string;
    const capture = await readLiveCapture(workspaceId);
    const lastSessionId = await readLastSession(workspaceId);
    if (capture) {
      const owns = capture.clientId === client.clientId;
      if (!owns) return basePanel(target.projectId, "recording_elsewhere", client, { capture, lastSessionId });
      const captureTarget: ProjectTarget = { projectId: capture.projectId, hostId: capture.hostId,
        projectRoot: capture.projectRoot, workspaceId: capture.workspaceId };
      const result = await callHost(captureTarget, "readCapture", { target: captureTarget, recordingId: capture.recordingId, ownerId: capture.ownerId }) as HostResult;
      if (!result.ok) {
        const withinGrace = Date.now() - capture.lastHeartbeatUnixMs <= DISCONNECT_GRACE_MS;
        return basePanel(target.projectId, withinGrace ? "recovering" : "needs_attention", client, { capture, lastSessionId, error: result.error });
      }
      if (!result.snapshot) return basePanel(target.projectId, "saving", client, { capture, lastSessionId });
      const state = result.snapshot.status === "paused" ? "paused" : result.snapshot.status === "saving" ? "saving" : "recording";
      return basePanel(target.projectId, state, client, { capture, lastSessionId, notepad: result.snapshot.notepad });
    }
    if (client.platform === "macos" && !sourceFor(client)) return basePanel(target.projectId, "needs_setup", client);
    if (!sourceFor(client)) return basePanel(target.projectId, "unavailable", client);
    return basePanel(target.projectId, "ready", client, { lastSessionId });
  }

  async function locked<T>(projectId: string, action: () => Promise<T>): Promise<T> {
    const prior = startLocks.get(projectId) || Promise.resolve();
    let release!: () => void;
    const next = new Promise<void>((resolve) => { release = resolve; });
    const queued = prior.then(() => next);
    startLocks.set(projectId, queued);
    await prior;
    try { return await action(); }
    finally { release(); if (startLocks.get(projectId) === queued) startLocks.delete(projectId); }
  }

  async function operate(sessionId: string, client: ClientCapabilities, operationId: string, operation: "heartbeat" | "pause" | "resume" | "stop") {
    const receiptKey = `control-receipt:${sessionId}:${operationId}`;
    const capture = await readCapture(sessionId);
    if (operation !== "heartbeat") {
      const prior = await bb.storage.kv.get(receiptKey) as { sessionId?: unknown; canonicalSessionId?: unknown; operation?: unknown; clientId?: unknown; workspaceId?: unknown; projectId?: unknown } | null;
      if (prior) {
        if (prior.sessionId !== sessionId || prior.operation !== operation || prior.clientId !== client.clientId) {
          return basePanel(capture?.projectId || null, "needs_attention", client, {
            capture: null,
            error: { code: "operation_conflict", message: "This control operation id was already used for different content.", retryable: false },
          });
        }
        if (operation === "stop") {
          if (typeof prior.workspaceId === "string") await bb.storage.kv.set(lastSessionKey(prior.workspaceId), capture?.sessionId || String(prior.canonicalSessionId || sessionId));
          if (capture) await clearCapture(capture);
        }
        return operation === "stop"
          ? basePanel(typeof prior.projectId === "string" ? prior.projectId : null, "saved", client, { lastSessionId: capture?.sessionId || String(prior.canonicalSessionId || sessionId) })
          : capture ? panelForCapture(capture, client) : basePanel(null, "unavailable", client);
      }
    }
    if (!capture || capture.clientId !== client.clientId) return basePanel(null, "unavailable", client, {
      error: { code: "session_not_owned", message: "This meeting is not owned by this browser.", retryable: false },
    });
    const target: ProjectTarget = { projectId: capture.projectId, hostId: capture.hostId,
      projectRoot: capture.projectRoot, workspaceId: capture.workspaceId };
    const result = await callHost(target, operation, { target, recordingId: capture.recordingId, ownerId: capture.ownerId }) as HostResult;
    if (result.ok) {
      if (operation === "stop") {
        await bb.storage.kv.set(lastSessionKey(capture.workspaceId), capture.sessionId);
      } else {
        capture.lastHeartbeatUnixMs = operation === "heartbeat" ? Date.now() : capture.lastHeartbeatUnixMs;
        await saveCapture(capture);
      }
      if (operation !== "heartbeat") {
        await bb.storage.kv.set(receiptKey, { sessionId, canonicalSessionId: capture.sessionId, operation, clientId: client.clientId,
          workspaceId: capture.workspaceId, projectId: capture.projectId });
      }
      if (operation === "stop") await clearCapture(capture);
    }
    bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: operation });
    if (!result.ok) return basePanel(target.projectId, "needs_attention", client, { capture, error: result.error });
    if (operation === "stop") return basePanel(target.projectId, "saved", client, { lastSessionId: capture.sessionId });
    return panelForCapture(capture, client);
  }

  async function panelForCapture(capture: CaptureRecord, client: ClientCapabilities): Promise<PanelState> {
    const target: ProjectTarget = { projectId: capture.projectId, hostId: capture.hostId,
      projectRoot: capture.projectRoot, workspaceId: capture.workspaceId };
    const result = await callHost(target, "readCapture", { target, recordingId: capture.recordingId, ownerId: capture.ownerId }) as HostResult;
    if (!result.ok) return basePanel(capture.projectId, "needs_attention", client, { capture, error: result.error });
    if (!result.snapshot) return basePanel(capture.projectId, "saving", client, { capture });
    return basePanel(capture.projectId, result.snapshot.status, client, { capture, notepad: result.snapshot.notepad });
  }

  async function beginForTarget(target: ProjectTarget, client: ClientCapabilities, ownerId: string, title?: string): Promise<PanelState> {
    if (client.nativeMacCapture || !client.secureContext || !client.browserMicrophone) return getPanelStateForTarget(target, client);
    const authority = await callHost(target, "captureAuthority", { target });
    if (!authority.ok) return basePanel(target.projectId, "unavailable", client, { error: authority.error });
    const workspaceId = authority.workspaceId as string;
    return locked(workspaceId, async () => {
      if (await readLiveCapture(workspaceId)) return getPanelStateForTarget(target, client);
      const result = await callHost(target, "startBrowserCapture", { target, ownerId, name: meetingName(title) }) as HostResult;
      if (!result.ok || !result.snapshot) return basePanel(target.projectId, "needs_attention", client, { error: result.ok ? null : result.error });
      await saveCapture({
        projectId: target.projectId, hostId: target.hostId, projectRoot: target.projectRoot,
        workspaceId, sessionId: result.snapshot.sessionId, recordingId: result.snapshot.recordingId, clientId: client.clientId,
        ownerId, lastHeartbeatUnixMs: Date.now(),
      });
      await bb.storage.kv.set(originKey(workspaceId, result.snapshot.sessionId), target.projectId);
      bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: "start" });
      return getPanelStateForTarget(target, client);
    });
  }

  bb.rpc.register(marginsRpcContract, {
    async availableWorkspaces({ projectId }) {
      const target = await targetForProject(projectId);
      const options = await callHost(target, "workspaceOptions", {});
      const resolvedWorkspaceId = target.workspaceId || options.defaultWorkspaceId || null;
      return { ...options, resolvedWorkspaceId };
    },
    async availableProjects() {
      const projects = await bb.sdk.projects.list();
      return { projects: projects.filter((project) => project.kind !== "personal")
        .map((project) => ({ id: project.id, name: project.name })) };
    },
    async getProjectPanelState({ projectId, client }) {
      try { return await getPanelStateForTarget(await targetForProject(projectId), client); }
      catch (cause) { return basePanel(null, "unavailable", client, {
        error: { code: "project_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: false },
      }); }
    },
    async beginProjectCapture({ projectId, client, ownerId, title }) {
      return beginForTarget(await targetForProject(projectId), client, ownerId, title);
    },
    async listWorkspaceMeetings({ projectId }) {
      const target = await targetForProject(projectId);
      const listed = await callHost(target, "listWorkspaceMeetings", { target });
      if (!listed.ok) return listed;
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) return authority;
      const options = await callHost(target, "workspaceOptions", {});
      const workspaceName = options.workspaces?.find((item: { id: string }) => item.id === authority.workspaceId)?.name || authority.workspaceId;
      return { ok: true as const, meetings: await Promise.all(listed.meetings.map(async (meeting: { sessionId: string; threadIds?: string[] }) => {
        const originProjectId = await bb.storage.kv.get(originKey(authority.workspaceId, meeting.sessionId));
        let originProjectName: string | null = null;
        if (typeof originProjectId === "string") {
          try { originProjectName = (await bb.sdk.projects.get({ projectId: originProjectId })).name; }
          catch { originProjectName = null; }
        }
        return { ...meeting, workspaceId: authority.workspaceId, workspaceName,
          originProjectId: typeof originProjectId === "string" ? originProjectId : null, originProjectName,
          archived: await bb.storage.kv.get(archiveKey(authority.workspaceId, meeting.sessionId)) === true,
          threadLinks: await Promise.all((meeting.threadIds || []).map(async (id) => {
          try {
            const thread = await bb.sdk.threads.get({ threadId: id }) as { title?: string | null };
            return { id, title: thread.title?.trim().slice(0, 100) || "Meeting note thread" };
          } catch { return { id, title: "Meeting note thread" }; }
        })),
        };
      })) };
    },
    async projectWorkspace({ threadId, projectId, workspaceId }) {
      const target = await targetForSelection({ threadId, projectId });
      if (workspaceId !== undefined) {
        const selected = workspaceId.trim();
        if (selected && !/^[a-z0-9][a-z0-9-]*$/.test(selected)) throw new Error("Invalid Workspace id");
        if (selected) {
          const options = await callHost(target, "workspaceOptions", {});
          if (!options.workspaces.some((workspace: { id: string }) => workspace.id === selected)) {
            throw new Error("Choose a Workspace from the list.");
          }
        }
        if (selected) await bb.storage.kv.set(`${PROJECT_WORKSPACE_PREFIX}${target.projectId}`, selected);
        else await bb.storage.kv.delete(`${PROJECT_WORKSPACE_PREFIX}${target.projectId}`);
      }
      const value = await bb.storage.kv.get(`${PROJECT_WORKSPACE_PREFIX}${target.projectId}`);
      return { workspaceId: typeof value === "string" && value ? value : null };
    },
    async readWorkspaceMeeting({ threadId, projectId, sessionId }) {
      const target = await targetForSelection({ threadId, projectId });
      const result = await callHost(target, "readWorkspaceMeeting", { target, sessionId });
      if (!sessionId && result.ok && result.meeting) {
        const authority = await callHost(target, "captureAuthority", { target });
        if (authority.ok) await bb.storage.kv.set(lastSessionKey(authority.workspaceId), result.meeting.sessionId);
      }
      return result;
    },
    async saveWorkspaceMemo({ threadId, projectId, sessionId, expectedRevision, text }) {
      const target = await targetForSelection({ threadId, projectId });
      return callHost(target, "saveWorkspaceMemo", { target, sessionId, expectedRevision, text });
    },
    async renameWorkspaceMeeting({ projectId, sessionId, title }) {
      const target = await targetForProject(projectId);
      return callHost(target, "renameWorkspaceMeeting", { target, sessionId, title });
    },
    async archiveWorkspaceMeeting({ projectId, sessionId, archived }) {
      const target = await targetForProject(projectId);
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) return authority;
      const read = await callHost(target, "readWorkspaceMeeting", { target, sessionId });
      if (!read.ok) return read;
      if (!read.meeting?.inputFinalized) return { ok: false as const,
        error: { code: "meeting_not_finished", message: "Stop and save this meeting before archiving it.", retryable: false } };
      if (archived) await bb.storage.kv.set(archiveKey(authority.workspaceId, sessionId), true);
      else await bb.storage.kv.delete(archiveKey(authority.workspaceId, sessionId));
      return { ok: true as const };
    },
    async recordMeetingOrigin({ projectId, sessionId }) {
      const target = await targetForProject(projectId);
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) return authority;
      const found = await callHost(target, "sessionExists", { target, recordingId: sessionId });
      if (!found.ok) return found;
      if (!found.found) return { ok: false as const,
        error: { code: "meeting_not_found", message: "Meeting has not appeared in this Workspace yet.", retryable: true } };
      const key = originKey(authority.workspaceId, sessionId);
      if (!await bb.storage.kv.get(key)) await bb.storage.kv.set(key, projectId);
      return { ok: true as const };
    },
    async discardWorkspaceMeeting({ projectId, sessionId }) {
      const target = await targetForProject(projectId);
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) return authority;
      const result = await callHost(target, "discardWorkspaceMeeting", { target, sessionId });
      if (!result.ok) return result;
      await bb.storage.kv.delete(archiveKey(authority.workspaceId, sessionId));
      await bb.storage.kv.delete(originKey(authority.workspaceId, sessionId));
      if (await bb.storage.kv.get(lastSessionKey(authority.workspaceId)) === sessionId) await bb.storage.kv.delete(lastSessionKey(authority.workspaceId));
      return { ok: true as const };
    },
    async readWorkspaceTranscript({ projectId, sessionId }) {
      const target = await targetForProject(projectId);
      return callHost(target, "readWorkspaceTranscript", { target, sessionId });
    },
    async captureAuthority({ threadId, projectId }) {
      try {
        const target = await targetForSelection({ threadId, projectId });
        return await callHost(target, "captureAuthority", { target });
      } catch (cause) {
        return { ok: false as const, error: { code: "project_folder_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: false } };
      }
    },
    async issueMenuGrant({ projectId, origin }) {
      const parsed = new URL(origin);
      if (parsed.origin !== origin || !(parsed.protocol === "https:"
        || parsed.protocol === "http:" && ["127.0.0.1", "localhost", "::1"].includes(parsed.hostname))) {
        throw new Error("Open bb over HTTPS or localhost to connect Margins Menu");
      }
      const target = await targetForProject(projectId);
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) throw new Error(authority.error.message);
      const options = await callHost(target, "workspaceOptions", {});
      const workspaceName = options.workspaces?.find((item: { id: string }) => item.id === authority.workspaceId)?.name || authority.workspaceId;
      const token = randomBytes(32).toString("base64url");
      const expiresAt = Date.now() + MENU_GRANT_TTL_MS;
      await bb.storage.kv.set(grantKey(token), { target: { ...target, workspaceId: authority.workspaceId },
        origin, instanceId: authority.instanceId, workspaceId: authority.workspaceId,
        epoch: await grantEpoch(authority.workspaceId), expiresAt });
      return { serviceUrl: `${origin}/api/v1/plugins/margins/http/menu/relay`, token,
        workspaceId: authority.workspaceId, workspaceName, instanceId: authority.instanceId, expiresAt };
    },
    async revokeMenuGrants({ projectId }) {
      const target = await targetForProject(projectId);
      const authority = await callHost(target, "captureAuthority", { target });
      if (!authority.ok) throw new Error(authority.error.message);
      await bb.storage.kv.set(`${MENU_GRANT_EPOCH_PREFIX}${authority.workspaceId}`, await grantEpoch(authority.workspaceId) + 1);
      return { revoked: true };
    },
    async pinNativeSession({ threadId, projectId, sessionId, instanceId, workspaceId }) {
      try {
        const target = await targetForSelection({ threadId, projectId });
        const authority = await callHost(target, "captureAuthority", { target });
        if (!authority.ok) return authority;
        if (authority.instanceId !== instanceId || authority.workspaceId !== workspaceId) {
          return { ok: false as const, error: { code: "destination_changed", message: "The Mac recording belongs to a different Margins instance or Workspace.", retryable: false } };
        }
        const found = await callHost(target, "sessionExists", { target, recordingId: sessionId }) as { ok: true; found: boolean } | { ok: false; error: HostError };
        if (!found.ok) return { ok: false as const, error: found.error };
        if (!found.found) return { ok: false as const, error: { code: "native_session_not_found", message: "The saved Mac session is not visible in this BB project's Margins Workspace yet.", retryable: true } };
        await bb.storage.kv.set(lastSessionKey(authority.workspaceId), sessionId);
        const origin = originKey(authority.workspaceId, sessionId);
        if (!await bb.storage.kv.get(origin)) await bb.storage.kv.set(origin, target.projectId);
        bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: "stop" });
        return { ok: true as const };
      } catch (cause) {
        return { ok: false as const, error: { code: "native_session_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: true } };
      }
    },
    getPanelState: ({ threadId, client }) => getPanelState(threadId, client),
    async beginBrowserCapture({ threadId, client, ownerId, title }) {
      return beginForTarget(await targetForThread(threadId), client, ownerId, title);
    },
    heartbeat: ({ sessionId, client, operationId }) => operate(sessionId, client, operationId, "heartbeat"),
    pause: ({ sessionId, client, operationId }) => operate(sessionId, client, operationId, "pause"),
    resume: ({ sessionId, client, operationId }) => operate(sessionId, client, operationId, "resume"),
    stop: ({ sessionId, client, operationId }) => operate(sessionId, client, operationId, "stop"),
    async updateNotepad({ sessionId, client, expectedRevision, text }) {
      const capture = await readCapture(sessionId);
      if (!capture || capture.clientId !== client.clientId) return basePanel(null, "unavailable", client);
      const target: ProjectTarget = { projectId: capture.projectId, hostId: capture.hostId,
        projectRoot: capture.projectRoot, workspaceId: capture.workspaceId };
      const result = await callHost(target, "updateNotepad", { target, recordingId: capture.recordingId, ownerId: capture.ownerId, expectedRevision, text }) as HostResult;
      if (!result.ok) return basePanel(target.projectId, "needs_attention", client, { capture, error: result.error });
      return panelForCapture(capture, client);
    },
    async connectedNoteContext({ threadId, projectId, sessionId }): Promise<ConnectedNoteResult> {
      const target = await targetForSelection({ threadId, projectId });
      return callHost(target, "connectedNoteContext", { target, recordingId: sessionId }) as Promise<ConnectedNoteResult>;
    },
    async transcribePinnedSession({ threadId, projectId, sessionId }): Promise<TranscriptionRequestResult> {
      const target = await targetForSelection({ threadId, projectId });
      return callHost(target, "requestTranscription", { target, recordingId: sessionId }) as Promise<TranscriptionRequestResult>;
    },
  });

  const chunkSchema = z.object({
    sessionId: z.string().min(1), client: clientCapabilitiesSchema,
    sequence: z.number().int().nonnegative(), bytesBase64: z.string().max(2_000_000),
  }).strict();
  bb.http.route("POST", "/capture/chunk", async (context) => {
    const parsed = chunkSchema.safeParse(await context.req.json().catch(() => null));
    if (!parsed.success) return context.json({ ok: false, error: "Invalid audio chunk" }, 400);
    const capture = await readCapture(parsed.data.sessionId);
    if (!capture || capture.clientId !== parsed.data.client.clientId) {
      return context.json({ ok: false, error: "This bb window does not own the recording" }, 409);
    }
    const target: ProjectTarget = { projectId: capture.projectId, hostId: capture.hostId,
      projectRoot: capture.projectRoot, workspaceId: capture.workspaceId };
    const result = await callHost(target, "uploadChunk", {
      target, recordingId: capture.recordingId, ownerId: capture.ownerId,
      sequence: parsed.data.sequence, bytesBase64: parsed.data.bytesBase64,
    });
    return context.json(result, result.ok ? 200 : 502);
  });

  bb.http.route("POST", "/menu/verify", async (context) => {
    const token = context.req.header("authorization")?.replace(/^Bearer /, "") || "";
    const grant = await readMenuGrant(token);
    return context.json(grant ? { ok: true, workspaceId: grant.workspaceId,
      instanceId: grant.instanceId, origin: grant.origin, expiresAt: grant.expiresAt }
      : { ok: false, error: "Capture grant expired or revoked" }, grant ? 200 : 401);
  }, { auth: "none" });

  bb.http.route("POST", "/menu/renew", async (context) => {
    const token = context.req.header("authorization")?.replace(/^Bearer /, "") || "";
    const grant = await readMenuGrant(token);
    if (!grant) return context.json({ ok: false, error: "Capture grant expired or revoked" }, 401);
    const expiresAt = Date.now() + MENU_GRANT_TTL_MS;
    await bb.storage.kv.set(grantKey(token), { ...grant, expiresAt });
    return context.json({ ok: true, expiresAt });
  }, { auth: "none" });

  bb.http.route("POST", "/menu/relay", async (context) => {
    const token = context.req.header("authorization")?.replace(/^Bearer /, "") || "";
    const grant = await readMenuGrant(token);
    if (!grant) return context.json({ ok: false, error: "Capture grant expired or revoked" }, 401);
    const raw = await context.req.text();
    if (raw.length > 2_100_000) return context.json({ ok: false, error: "Capture payload too large" }, 413);
    let body: unknown;
    try { body = JSON.parse(raw || "null"); }
    catch { return context.json({ ok: false, error: "Invalid capture relay JSON" }, 400); }
    const parsed = z.object({ method: z.enum(["GET", "POST", "PUT"]), path: z.string().min(1).max(300),
      bodyBase64: z.string().max(2_000_000), contentType: z.string().max(120).optional(),
      producerToken: z.string().max(300).optional(), instanceId: z.string().max(300).optional() }).strict()
      .safeParse(body);
    if (!parsed.success) return context.json({ ok: false, error: "Invalid capture relay request" }, 400);
    try {
      const result = await callHost(grant.target, "relayWorkspaceHttp", { target: grant.target, ...parsed.data });
      if (!Number.isInteger(result.status) || typeof result.bodyBase64 !== "string") {
        return context.json({ ok: false, error: "Capture host unavailable" }, 502);
      }
      if (result.status >= 200 && result.status < 300 && parsed.data.method === "POST"
        && parsed.data.path.replace(/^\//, "") === `v1/workspaces/${grant.workspaceId}/sessions`) {
        try {
          const command = JSON.parse(Buffer.from(parsed.data.bodyBase64, "base64").toString("utf8")) as { session_id?: unknown };
          if (typeof command.session_id === "string" && /^[A-Za-z0-9_-]{1,200}$/.test(command.session_id)) {
            const key = originKey(grant.workspaceId, command.session_id);
            if (!await bb.storage.kv.get(key)) await bb.storage.kv.set(key, grant.target.projectId);
          }
        } catch { /* A malformed capture command is reported by the Workspace service. */ }
      }
      return context.json(result);
    } catch { return context.json({ ok: false, error: "Capture relay unavailable" }, 502); }
  }, { auth: "none" });

  bb.ui.registerMentionProvider({
    id: "margins", label: "Margins", triggers: ["@"],
    async search() { return []; },
    async resolve(itemId) {
      const pinned = parseMeetingMentionId(itemId);
      const target = await targetForProject(pinned.projectId);
      const result = await callHost(target, "connectedNoteContext", { target, recordingId: pinned.sessionId });
      if (!result.ok) throw new Error("Meeting unavailable. Open Make note again.");
      const current = result.context;
      if (current.workspaceId !== pinned.workspaceId || current.sessionId !== pinned.sessionId
        || current.memo.revision !== pinned.memoRevision
        || Boolean(current.noteAssociation) !== (pinned.note === "update")) {
        throw new Error("This meeting changed. Open Make note again to use the latest memo.");
      }
      return { context: `<margins-context-v1>\n${JSON.stringify({
        workspaceId: pinned.workspaceId, sessionId: pinned.sessionId, memoRevision: pinned.memoRevision,
        bbProjectId: pinned.projectId, transcript: current.transcript.available ? "ready" : "pending",
        note: pinned.note,
      })}\n</margins-context-v1>` };
    },
  });
  bb.agents.configure((context) => context.project.kind === "personal" ? { tools: [], skills: [] } : {
    tools: [], skills: ["watermark", "workspace-setup"],
    instructions: "Margins recordings live in the resolved Margins Workspace. Use the Margins skills and the Workspace destination read for notes; do not infer a project .margins folder or treat raw notes as settled knowledge.",
  });
}
