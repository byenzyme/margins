import type { BbPluginApi } from "@get-bb/plugin-sdk";
import { z } from "zod";
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

const CAPTURE_PREFIX = "capture:";
const LAST_SESSION_PREFIX = "last-session:";
const REALTIME_CHANNEL = "margins-recording";
export const DISCONNECT_GRACE_MS = CAPTURE_DISCONNECT_GRACE_MS;

function captureKey(projectId: string) { return `${CAPTURE_PREFIX}${projectId}`; }
function lastSessionKey(projectId: string) { return `${LAST_SESSION_PREFIX}${projectId}`; }
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
      title: "Ready to record", detail: `${sourceLabel}. Audio and notes will be saved to this bb project.`,
      primaryAction: "start" as const, primaryLabel: "Start recording",
    };
    case "getting_ready": return {
      title: "Getting recording ready", detail: "Nothing is being recorded until your microphone and the project are both ready.",
      primaryAction: "none" as const, primaryLabel: "Getting ready",
    };
    case "recording": return {
      title: "Recording", detail: `${sourceLabel}. Audio and notes are being saved to this bb project.`,
      primaryAction: "pause" as const, primaryLabel: "Pause",
    };
    case "paused": return {
      title: "Paused", detail: "Audio is paused. Your recording and notes received so far are safe in this bb project.",
      primaryAction: "resume" as const, primaryLabel: "Resume",
    };
    case "recovering": return {
      title: "Reconnecting", detail: "Audio and notes already received are safe. Margins will stop and save if this bb window does not reconnect shortly.",
      primaryAction: "retry" as const, primaryLabel: "Reconnect",
    };
    case "saving": return {
      title: "Saving", detail: "Capture has stopped. Margins is finishing the audio already received by this bb project.",
      primaryAction: "none" as const, primaryLabel: "Saving",
    };
    case "saved": return {
      title: "Meeting saved", detail: "Your recording and notes are safe in this bb project. A transcript may take a little longer.",
      primaryAction: "none" as const, primaryLabel: "Meeting saved",
    };
    case "recording_elsewhere": return {
      title: "Recording from another bb window", detail: "This project already has a recording. Return to that window for recording controls.",
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

  host.experimental_onSignal("changed", ({ payload }) => bb.realtime.publish(REALTIME_CHANNEL, payload));
  host.experimental_onWorkerExit(({ hostId }) => bb.realtime.publish(REALTIME_CHANNEL, { hostId, reason: "project-recorder-offline" }));

  async function targetForThread(threadId: string): Promise<ProjectTarget> {
    const thread = await bb.sdk.threads.get({ threadId }) as unknown as { projectId: string };
    const project = await bb.sdk.projects.get({ projectId: thread.projectId });
    if (project.kind === "personal") throw new Error("Choose a project with a stable folder before recording.");
    const source = project.sources.find((candidate) => candidate.isDefault);
    if (!source) throw new Error("This project does not have a primary folder for recordings.");
    return { projectId: project.id, hostId: source.hostId, projectRoot: source.path };
  }

  async function readCapture(projectId: string): Promise<CaptureRecord | null> {
    const parsed = captureRecordSchema.safeParse(await bb.storage.kv.get(captureKey(projectId)));
    return parsed.success ? parsed.data : null;
  }
  async function saveCapture(value: CaptureRecord) { await bb.storage.kv.set(captureKey(value.projectId), value); }
  async function clearCapture(projectId: string) { await bb.storage.kv.delete(captureKey(projectId)); }
  async function readLastSession(projectId: string) {
    const value = await bb.storage.kv.get(lastSessionKey(projectId));
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
      sourceLabel, storageLabel: projectId ? "Saved to this bb project" : null,
      canStop: owns && ["recording", "paused", "recovering"].includes(state),
      canEditNotepad: owns && ["recording", "paused", "recovering"].includes(state),
      ownsRecording: owns, recordingId: options.capture?.recordingId || null,
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
    const capture = await readCapture(target.projectId);
    const lastSessionId = await readLastSession(target.projectId);
    if (capture) {
      const owns = capture.clientId === client.clientId;
      if (!owns) return basePanel(target.projectId, "recording_elsewhere", client, { capture, lastSessionId });
      const result = await callHost(target, "readCapture", { target, recordingId: capture.recordingId, ownerId: capture.ownerId }) as HostResult;
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

  async function operate(threadId: string, client: ClientCapabilities, recordingId: string, operationId: string, operation: "heartbeat" | "pause" | "resume" | "stop") {
    const target = await targetForThread(threadId);
    const receiptKey = `control-receipt:${target.projectId}:${operationId}`;
    if (operation !== "heartbeat") {
      const prior = await bb.storage.kv.get(receiptKey) as { recordingId?: unknown; operation?: unknown; clientId?: unknown } | null;
      if (prior) {
        if (prior.recordingId !== recordingId || prior.operation !== operation || prior.clientId !== client.clientId) {
          return basePanel(target.projectId, "needs_attention", client, {
            capture: null,
            error: { code: "operation_conflict", message: "This control operation id was already used for different content.", retryable: false },
          });
        }
        if (operation === "stop") {
          await bb.storage.kv.set(lastSessionKey(target.projectId), recordingId);
          await clearCapture(target.projectId);
        }
        return operation === "stop"
          ? basePanel(target.projectId, "saved", client, { lastSessionId: recordingId })
          : getPanelState(threadId, client);
      }
    }
    const capture = await readCapture(target.projectId);
    if (!capture || capture.recordingId !== recordingId || capture.clientId !== client.clientId) return getPanelState(threadId, client);
    const result = await callHost(target, operation, { target, recordingId, ownerId: capture.ownerId }) as HostResult;
    if (result.ok) {
      if (operation === "stop") {
        await bb.storage.kv.set(lastSessionKey(target.projectId), recordingId);
      } else {
        capture.lastHeartbeatUnixMs = operation === "heartbeat" ? Date.now() : capture.lastHeartbeatUnixMs;
        await saveCapture(capture);
      }
      if (operation !== "heartbeat") {
        await bb.storage.kv.set(receiptKey, { recordingId, operation, clientId: client.clientId });
      }
      if (operation === "stop") await clearCapture(target.projectId);
    }
    bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: operation });
    if (!result.ok) return basePanel(target.projectId, "needs_attention", client, { capture, error: result.error });
    if (operation === "stop") return basePanel(target.projectId, "saved", client, { lastSessionId: recordingId });
    return getPanelState(threadId, client);
  }

  bb.rpc.register(marginsRpcContract, {
    async captureAuthority({ threadId }) {
      try {
        const target = await targetForThread(threadId);
        return await callHost(target, "captureAuthority", { target });
      } catch (cause) {
        return { ok: false as const, error: { code: "project_folder_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: false } };
      }
    },
    async pinNativeSession({ threadId, sessionId, instanceId, workspaceId }) {
      try {
        const target = await targetForThread(threadId);
        const authority = await callHost(target, "captureAuthority", { target });
        if (!authority.ok) return authority;
        if (authority.instanceId !== instanceId || authority.workspaceId !== workspaceId) {
          return { ok: false as const, error: { code: "destination_changed", message: "The Mac recording belongs to a different Margins instance or Workspace.", retryable: false } };
        }
        const found = await callHost(target, "sessionExists", { target, recordingId: sessionId }) as { ok: true; found: boolean } | { ok: false; error: HostError };
        if (!found.ok) return { ok: false as const, error: found.error };
        if (!found.found) return { ok: false as const, error: { code: "native_session_not_found", message: "The saved Mac session is not visible in this BB project's Margins Workspace yet.", retryable: true } };
        await bb.storage.kv.set(lastSessionKey(target.projectId), sessionId);
        bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: "stop" });
        return { ok: true as const };
      } catch (cause) {
        return { ok: false as const, error: { code: "native_session_unavailable", message: cause instanceof Error ? cause.message : String(cause), retryable: true } };
      }
    },
    getPanelState: ({ threadId, client }) => getPanelState(threadId, client),
    async beginBrowserCapture({ threadId, client, ownerId, title }) {
      const target = await targetForThread(threadId);
      if (client.nativeMacCapture || !client.secureContext || !client.browserMicrophone) return getPanelState(threadId, client);
      return locked(target.projectId, async () => {
        if (await readCapture(target.projectId)) return getPanelState(threadId, client);
        const result = await callHost(target, "startBrowserCapture", { target, ownerId, name: meetingName(title) }) as HostResult;
        if (!result.ok || !result.snapshot) return basePanel(target.projectId, "needs_attention", client, { error: result.ok ? null : result.error });
        await saveCapture({
          projectId: target.projectId, hostId: target.hostId, projectRoot: target.projectRoot,
          recordingId: result.snapshot.recordingId,
          clientId: client.clientId, ownerId, lastHeartbeatUnixMs: Date.now(),
        });
        bb.realtime.publish(REALTIME_CHANNEL, { projectId: target.projectId, reason: "start" });
        return getPanelState(threadId, client);
      });
    },
    heartbeat: ({ threadId, client, recordingId, operationId }) => operate(threadId, client, recordingId, operationId, "heartbeat"),
    pause: ({ threadId, client, recordingId, operationId }) => operate(threadId, client, recordingId, operationId, "pause"),
    resume: ({ threadId, client, recordingId, operationId }) => operate(threadId, client, recordingId, operationId, "resume"),
    stop: ({ threadId, client, recordingId, operationId }) => operate(threadId, client, recordingId, operationId, "stop"),
    async updateNotepad({ threadId, client, recordingId, expectedRevision, text }) {
      const target = await targetForThread(threadId);
      const capture = await readCapture(target.projectId);
      if (!capture || capture.recordingId !== recordingId || capture.clientId !== client.clientId) return getPanelState(threadId, client);
      const result = await callHost(target, "updateNotepad", { target, recordingId, ownerId: capture.ownerId, expectedRevision, text }) as HostResult;
      if (!result.ok) return basePanel(target.projectId, "needs_attention", client, { capture, error: result.error });
      return getPanelState(threadId, client);
    },
    async connectedNoteContext({ threadId, sessionId }): Promise<ConnectedNoteResult> {
      const target = await targetForThread(threadId);
      const pinned = await readLastSession(target.projectId);
      if (pinned !== sessionId) {
        return { ok: false as const, error: { code: "session_pin_stale", message: "The selected meeting is no longer this project's pinned latest session. Refresh before creating the note.", retryable: true } };
      }
      return callHost(target, "connectedNoteContext", { target, recordingId: sessionId }) as Promise<ConnectedNoteResult>;
    },
    async transcribePinnedSession({ threadId, sessionId }): Promise<TranscriptionRequestResult> {
      const target = await targetForThread(threadId);
      const pinned = await readLastSession(target.projectId);
      if (pinned !== sessionId) {
        return { ok: false, error: { code: "session_pin_stale", message: "Refresh before transcribing this meeting.", retryable: true } };
      }
      return callHost(target, "requestTranscription", { target, recordingId: sessionId }) as Promise<TranscriptionRequestResult>;
    },
  });

  const chunkSchema = z.object({
    threadId: z.string().min(1), client: clientCapabilitiesSchema,
    recordingId: z.string().min(1), sequence: z.number().int().nonnegative(), bytesBase64: z.string().max(2_000_000),
  }).strict();
  bb.http.route("POST", "/capture/chunk", async (context) => {
    const parsed = chunkSchema.safeParse(await context.req.json().catch(() => null));
    if (!parsed.success) return context.json({ ok: false, error: "Invalid audio chunk" }, 400);
    const target = await targetForThread(parsed.data.threadId);
    const capture = await readCapture(target.projectId);
    if (!capture || capture.recordingId !== parsed.data.recordingId || capture.clientId !== parsed.data.client.clientId) {
      return context.json({ ok: false, error: "This bb window does not own the recording" }, 409);
    }
    const result = await callHost(target, "uploadChunk", {
      target, recordingId: capture.recordingId, ownerId: capture.ownerId,
      sequence: parsed.data.sequence, bytesBase64: parsed.data.bytesBase64,
    });
    return context.json(result, result.ok ? 200 : 502);
  });

  bb.ui.registerMentionProvider({
    id: "margins", label: "Margins", triggers: ["@"],
    async search() { return []; },
    async resolve() { throw new Error("Live Margins context is not available until the project recorder has produced a transcript."); },
  });
  bb.agents.configure((context) => context.project.kind === "personal" ? { tools: [], skills: [] } : {
    tools: [], skills: ["watermark", "workspace-setup"],
    instructions: "Margins recordings and notes live in the project's .margins folder. Use the Margins skills when the user asks about a recorded meeting; do not treat raw notes as settled knowledge.",
  });
}
