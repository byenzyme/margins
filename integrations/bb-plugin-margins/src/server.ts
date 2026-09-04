import type { BbPluginApi } from "@get-bb/plugin-sdk";
import {
  attachmentSchema,
  liveErrorSchema,
  savedMeetingSchema,
  marginsHostContract,
  marginsRpcContract,
  hostSignals,
  PANEL_STATE_SCHEMA,
  WATERMARK_CONTEXT_SCHEMA,
  type HostOperationResult,
  type LiveError,
  type LiveSnapshot,
  type PanelState,
  type PluginState,
  type PrimaryAction,
  type SavedMeeting,
  type ThreadAttachment,
  type WatermarkPacket,
} from "./contracts.js";

const ATTACHMENT_PREFIX = "attachment:";
const SAVED_MEETING_PREFIX = "saved-meeting:";
const CAPTURE_SEEN_PREFIX = "capture-seen:";
const LAST_ERROR_PREFIX = "last-error:";
const REALTIME_CHANNEL = "margins-live";
const DEFAULT_TRANSCRIPT_WINDOW_CHARS = 12_000;
const MAX_TRANSCRIPT_WINDOW_CHARS = 64_000;
const INSTALL_URL = "https://github.com/byenzyme/margins/releases";

interface ResolvedHost {
  id: string;
  name: string;
  status: string;
}

interface ServerSettings {
  marginsHostId: string;
  transcriptWindowChars: string;
}

function attachmentKey(threadId: string) {
  return `${ATTACHMENT_PREFIX}${threadId}`;
}

function savedMeetingKey(threadId: string) {
  return `${SAVED_MEETING_PREFIX}${threadId}`;
}

function captureSeenKey(threadId: string) {
  return `${CAPTURE_SEEN_PREFIX}${threadId}`;
}

function lastErrorKey(threadId: string) {
  return `${LAST_ERROR_PREFIX}${threadId}`;
}

function requestId(prefix: string, threadId: string) {
  return `${prefix}:${threadId}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
}

function parseWindowChars(raw: string) {
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return DEFAULT_TRANSCRIPT_WINDOW_CHARS;
  }
  return Math.min(parsed, MAX_TRANSCRIPT_WINDOW_CHARS);
}

function meetingName(value: string | null | undefined) {
  const slug = (value ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 90)
    .replace(/-+$/g, "");
  return slug || "meeting";
}

function stateCopy(
  state: PluginState,
  error: LiveError | null,
): Pick<PanelState, "title" | "detail" | "primaryAction" | "primaryLabel"> {
  if (state === "host_offline") {
    return {
      title: "Recording Mac is offline",
      detail: "Recording could not start. No recording was created. Reconnect this Mac, then try again.",
      primaryAction: "start",
      primaryLabel: "Try again",
    };
  }
  if (state === "unsupported_platform") {
    return {
      title: "Recording is not supported here",
      detail: "Recording could not start. No recording was created. Open this thread on an Apple silicon Mac.",
      primaryAction: "none",
      primaryLabel: "Recording unavailable",
    };
  }
  if (state === "runtime_auth_error") {
    return {
      title: "Margins could not connect securely",
      detail: "Recording could not start. No recording was created. Restart Margins, then try again.",
      primaryAction: "start",
      primaryLabel: "Try again",
    };
  }
  if (state === "microphone_permission") {
    return {
      title: "Microphone access is off",
      detail: "Recording could not start. No recording was created. Allow Microphone access in System Settings, then try again.",
      primaryAction: "start",
      primaryLabel: "Try again",
    };
  }
  if (state === "system_audio_permission") {
    return {
      title: "System audio access is off",
      detail: "Recording could not start. No recording was created. Allow System Audio Recording in System Settings, then try again.",
      primaryAction: "start",
      primaryLabel: "Try again",
    };
  }
  if (state === "runtime_error") {
    return {
      title: "Margins needs attention",
      detail: `Recording could not start. No recording was created. ${error?.message ?? "Try again."}`,
      primaryAction: "start",
      primaryLabel: "Try again",
    };
  }
  if (state === "preparing") {
    return {
      title: "Preparing recording",
      detail: "Margins is starting the private recorder on this Mac.",
      primaryAction: "none",
      primaryLabel: "Preparing recording",
    };
  }
  if (state === "recording") {
    return {
      title: "Listening",
      detail: "Margins is listening to this meeting.",
      primaryAction: "pause",
      primaryLabel: "Pause",
    };
  }
  if (state === "paused") {
    return {
      title: "Paused",
      detail: "Recording is paused.",
      primaryAction: "resume",
      primaryLabel: "Resume",
    };
  }
  if (state === "finalizing") {
    return {
      title: "Saving",
      detail: "Margins is finishing this recording.",
      primaryAction: "refresh",
      primaryLabel: "Refresh",
    };
  }
  if (state === "meeting_saved") {
    return {
      title: "Meeting saved on this Mac",
      detail: "Your recording and notes are safe. Margins can turn them into a connected note when you are ready.",
      primaryAction: "none",
      primaryLabel: "Meeting saved",
    };
  }
  if (state === "recoverable_error") {
    return {
      title: "Recording was interrupted",
      detail: error?.message ?? "Try again when you're ready.",
      primaryAction: "refresh",
      primaryLabel: "Try again",
    };
  }
  return {
    title: "Ready",
    detail: "Nothing is recorded until you start.",
    primaryAction: "start",
    primaryLabel: "Start recording",
  };
}

function isLiveStatus(status: string | undefined) {
  return (
    status === "starting" ||
    status === "recording" ||
    status === "paused" ||
    status === "finalizing" ||
    status === "needs_attention"
  );
}

function isLiveSnapshot(snapshot: LiveSnapshot | null) {
  return snapshot?.session ? isLiveStatus(snapshot.session.status) : false;
}

function mapSnapshotState(snapshot: LiveSnapshot | null): PluginState {
  const state = snapshot?.session?.status ?? "idle";
  if (state === "starting") return "preparing";
  if (state === "recording") return "recording";
  if (state === "paused") return "paused";
  if (state === "finalizing") return "finalizing";
  if (state === "needs_attention") {
    return "recoverable_error";
  }
  return "ready";
}

function mapErrorState(error: LiveError): PluginState {
  if (error.state) return error.state;
  const message = error.message.toLowerCase();
  if (error.code === "unsupported" || error.code === "capability_unavailable") {
    return "unsupported_platform";
  }
  if (error.code === "unauthorized" || error.code === "forbidden") return "runtime_auth_error";
  if (error.code.includes("microphone") || message.includes("microphone")) {
    return "microphone_permission";
  }
  if (
    error.code.includes("system_audio") ||
    message.includes("system audio") ||
    message.includes("screen recording")
  ) {
    return "system_audio_permission";
  }
  if (error.code === "permission_denied") return "microphone_permission";
  if (
    error.code === "margins_not_found" ||
    error.code === "not_found" ||
    error.code === "margins_closed" ||
    error.code === "connection_refused" ||
    error.code === "update_needed" ||
    error.code === "version_mismatch"
  ) {
    return "runtime_error";
  }
  if (error.code === "not_running") return "ready";
  return "recoverable_error";
}

function isPreparationError(error: LiveError) {
  return [
    "margins_not_found",
    "not_found",
    "margins_closed",
    "connection_refused",
    "update_needed",
    "version_mismatch",
    "not_running",
  ].includes(error.code);
}

function createEmptySnapshot(): LiveSnapshot {
  return {
    protocol_version: 1,
    server_unix_ms: Date.now(),
    session: null,
    health: {
      capture_phase: "idle",
      tap_status: "unknown",
      system_audio_expected: false,
      system_audio_observed: false,
      transcript_freshness: null,
    },
    rolling_transcript: [],
    memo_lines: [],
    notepad_revision: "empty",
  };
}

function encodeMentionId(input: {
  threadId: string;
  projectId: string | null;
  hostId: string;
  meetingId: string;
}) {
  return Buffer.from(JSON.stringify(input), "utf8").toString("base64url");
}

function decodeMentionId(
  itemId: string,
): { threadId: string; projectId: string | null; hostId: string; meetingId: string } | null {
  try {
    const raw = JSON.parse(Buffer.from(itemId, "base64url").toString("utf8")) as Record<
      string,
      unknown
    > | null;
    if (
      raw &&
      typeof raw.threadId === "string" &&
      typeof raw.hostId === "string" &&
      typeof raw.meetingId === "string" &&
      (typeof raw.projectId === "string" || raw.projectId === null)
    ) {
      return {
        threadId: raw.threadId,
        projectId: raw.projectId,
        hostId: raw.hostId,
        meetingId: raw.meetingId,
      };
    }
  } catch {
    return null;
  }
  return null;
}

function panelState(input: {
  threadId: string;
  host: ResolvedHost | null;
  attachment: ThreadAttachment | null;
  savedMeeting: SavedMeeting | null;
  firstUse: boolean;
  preparationNeeded?: boolean;
  snapshot: LiveSnapshot | null;
  error: LiveError | null;
  state: PluginState;
  mentionItemId: string | null;
}): PanelState {
  const copy = stateCopy(input.state, input.error);
  if (input.state === "ready" && isLiveSnapshot(input.snapshot) && input.attachment === null) {
    copy.detail = "A recording is already live. Start recording will use it.";
  }
  if (input.state === "recoverable_error") {
    copy.detail = input.attachment
      ? "Recording needs attention. Notes already saved are safe. Try again."
      : "Recording could not start. No recording was created. Try again.";
  }
  const primaryAction: PrimaryAction = copy.primaryAction;
  return {
    schema: PANEL_STATE_SCHEMA,
    threadId: input.threadId,
    state: input.state,
    title: copy.title,
    detail: copy.detail,
    primaryAction,
    primaryLabel: copy.primaryLabel,
    canEditNotepad:
      input.attachment !== null && (input.state === "recording" || input.state === "paused"),
    canStop: input.attachment !== null && (input.state === "recording" || input.state === "paused"),
    canDetach: input.attachment !== null,
    firstUse: input.firstUse,
    preparationNeeded: input.preparationNeeded ?? false,
    installUrl: input.error?.installUrl ?? INSTALL_URL,
    host: input.host,
    attachment: input.attachment,
    savedMeeting: input.savedMeeting,
    snapshot: input.snapshot,
    error: input.error,
    mention: {
      available: input.mentionItemId !== null,
      itemId: input.mentionItemId,
    },
  };
}

function formatMs(ms: number | null) {
  if (ms === null) return "unknown";
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

function buildWatermarkPacket(snapshot: LiveSnapshot, maxTranscriptChars: number): WatermarkPacket {
  if (!snapshot.session) {
    throw new Error("No live Margins meeting is available.");
  }
  const transcriptBody = snapshot.rolling_transcript
    .map((line) => line.text)
    .join("\n")
    .slice(-maxTranscriptChars);
  const freshness = snapshot.health.transcript_freshness ?? null;
  const now = Date.now();
  const transcriptUpdatedAt = freshness?.updated_at_unix_ms ?? null;
  const stale = transcriptUpdatedAt === null || now - transcriptUpdatedAt > 45_000;
  const warnings = snapshot.health.tap_warning ? [snapshot.health.tap_warning] : [];
  const context = [
    `Margins live meeting: ${snapshot.session.session_id}`,
    `Recording: ${snapshot.session.status}`,
    `Transcript: heard through ${formatMs(
      freshness?.decoded_until_ms ?? null,
    )}, settled through ${formatMs(freshness?.committed_until_ms ?? null)}`,
    isLiveSnapshot(snapshot)
      ? "Transcript is live; newest words may still change."
      : "Transcript is not currently live.",
    warnings.length > 0 ? `Warnings: ${warnings.join("; ")}` : "Warnings: none",
    "",
    "Recent notes:",
    ...(snapshot.memo_lines.length > 0
      ? snapshot.memo_lines.map((memo) => {
          const mark =
            memo.at_ms !== null && memo.at_ms !== undefined ? formatMs(memo.at_ms) : "Note";
          return `- ${mark}: ${memo.text}`;
        })
      : ["- none"]),
    "",
    "Transcript:",
    transcriptBody,
  ].join("\n");

  return {
    schema: WATERMARK_CONTEXT_SCHEMA,
    meeting: {
      id: snapshot.session.session_id,
      live: isLiveSnapshot(snapshot),
      status: snapshot.session.status,
    },
    freshness: {
      decodedUntilMs: freshness?.decoded_until_ms ?? null,
      committedUntilMs: freshness?.committed_until_ms ?? null,
      transcriptUpdatedAtUnixMs: transcriptUpdatedAt,
      resolvedAtUnixMs: now,
      stale,
      staleReason: stale ? "Transcript has not updated recently." : null,
    },
    captureHealth: {
      phase: snapshot.health.capture_phase,
      paused: snapshot.session.status === "paused",
      tapStatus: snapshot.health.tap_status,
      systemAudioExpected: snapshot.health.system_audio_expected,
      systemAudioObserved: snapshot.health.system_audio_observed,
      warnings,
    },
    bounds: {
      maxTranscriptChars,
      includedTranscriptChars: transcriptBody.length,
      memoItemsIncluded: snapshot.memo_lines.length,
    },
    context,
  };
}

function contextText(packet: WatermarkPacket) {
  return [
    `<MarginsContext schema="${packet.schema}">`,
    JSON.stringify(packet),
    "</MarginsContext>",
    "",
    packet.context,
  ].join("\n");
}

export default async function marginsPlugin(bb: BbPluginApi): Promise<void> {
  const settings = bb.settings.define({
    marginsHostId: {
      type: "string",
      label: "Recording Mac",
      description: "Leave this blank to use the Mac attached to the current thread.",
      default: "",
    },
    transcriptWindowChars: {
      type: "string",
      label: "Recent conversation",
      description: "How much recent transcript @Margins can use.",
      default: String(DEFAULT_TRANSCRIPT_WINDOW_CHARS),
    },
  });

  const hostClient = bb.hosts.experimental_client({
    contract: marginsHostContract,
    experimental_signals: hostSignals,
  });

  hostClient.experimental_onSignal("changed", ({ hostId }) => {
    bb.realtime.publish(REALTIME_CHANNEL, { hostId, reason: "changed" });
  });
  hostClient.experimental_onWorkerExit(({ hostId }) => {
    bb.realtime.publish(REALTIME_CHANNEL, { hostId, reason: "host-worker-exit" });
  });

  async function readSettings(): Promise<ServerSettings> {
    return (await settings.get()) as ServerSettings;
  }

  async function readAttachment(threadId: string): Promise<ThreadAttachment | null> {
    const raw = await bb.storage.kv.get<unknown>(attachmentKey(threadId));
    const parsed = attachmentSchema.safeParse(raw);
    if (!parsed.success) return null;
    return parsed.data;
  }

  async function saveAttachment(attachment: ThreadAttachment): Promise<void> {
    await bb.storage.kv.set(attachmentKey(attachment.threadId), attachment);
  }

  async function clearAttachment(threadId: string): Promise<void> {
    await bb.storage.kv.delete(attachmentKey(threadId));
  }

  async function readSavedMeeting(threadId: string): Promise<SavedMeeting | null> {
    const raw = await bb.storage.kv.get<unknown>(savedMeetingKey(threadId));
    const parsed = savedMeetingSchema.safeParse(raw);
    return parsed.success ? parsed.data : null;
  }

  async function saveStoppedMeeting(saved: SavedMeeting): Promise<void> {
    await bb.storage.kv.set(savedMeetingKey(saved.threadId), saved);
  }

  async function clearSavedMeeting(threadId: string): Promise<void> {
    await bb.storage.kv.delete(savedMeetingKey(threadId));
  }

  async function hasSeenCapture(threadId: string): Promise<boolean> {
    return (await bb.storage.kv.get<boolean>(captureSeenKey(threadId))) === true;
  }

  async function markCaptureSeen(threadId: string): Promise<void> {
    await bb.storage.kv.set(captureSeenKey(threadId), true);
  }

  async function readLastError(threadId: string): Promise<LiveError | null> {
    const raw = await bb.storage.kv.get<unknown>(lastErrorKey(threadId));
    const parsed = liveErrorSchema.safeParse(raw);
    return parsed.success ? parsed.data : null;
  }

  async function saveLastError(threadId: string, error: LiveError): Promise<void> {
    await bb.storage.kv.set(lastErrorKey(threadId), error);
  }

  async function clearLastError(threadId: string): Promise<void> {
    await bb.storage.kv.delete(lastErrorKey(threadId));
  }

  async function resolveHost(threadId: string): Promise<ResolvedHost | null> {
    const config = await readSettings();
    let hostId = config.marginsHostId.trim();
    if (!hostId) {
      const thread = (await bb.sdk.threads.get({ threadId })) as unknown as {
        environmentId?: string | null;
      };
      if (thread.environmentId) {
        const environment = (await bb.sdk.environments.get({
          environmentId: thread.environmentId,
        })) as unknown as {
          hostId?: string | null;
          host?: { id?: string | null };
        };
        hostId = environment.hostId ?? environment.host?.id ?? "";
      }
    }
    if (!hostId) return null;

    return findHost(hostId);
  }

  async function findHost(hostId: string): Promise<ResolvedHost> {
    const hosts = (await bb.sdk.hosts.list()) as Array<{
      id: string;
      name: string;
      status: string;
    }>;
    const host = hosts.find((candidate) => candidate.id === hostId);
    return {
      id: hostId,
      name: host?.name || hostId,
      status: host?.status || "disconnected",
    };
  }

  async function callHost(
    host: ResolvedHost,
    method: keyof typeof marginsHostContract,
    input: Record<string, unknown>,
  ): Promise<HostOperationResult> {
    if (host.status !== "connected") {
      return {
        ok: false,
        error: {
          code: "host_offline",
          message: "Pick the Mac that is recording.",
          retryable: true,
          state: "host_offline",
        },
      };
    }
    try {
      return await hostClient.call(method, input as never, { hostId: host.id });
    } catch {
      return {
        ok: false,
        error: {
          code: "host_unreachable",
          message: `bb could not reach Margins on ${host.name}.`,
          retryable: true,
          state: "recoverable_error",
        },
      };
    }
  }

  async function readSnapshotForThread(threadId: string): Promise<{
    host: ResolvedHost | null;
    attachment: ThreadAttachment | null;
    result: HostOperationResult;
  }> {
    const attachment = await readAttachment(threadId);
    const host = attachment ? await findHost(attachment.hostId) : await resolveHost(threadId);
    if (!host) {
      return {
        host: null,
        attachment,
        result: {
          ok: false,
          error: {
            code: "host_offline",
            message: "Pick the Mac that is recording.",
            retryable: true,
            state: "host_offline",
          },
        },
      };
    }
    const result = await callHost(host, "readSnapshot", {
      sessionId: attachment?.meetingId ?? "current",
    });
    return { host, attachment, result };
  }

  function panelForResult(
    threadId: string,
    host: ResolvedHost | null,
    attachment: ThreadAttachment | null,
    result: HostOperationResult,
    options: {
      savedMeeting?: SavedMeeting | null;
      firstUse?: boolean;
      passive?: boolean;
    } = {},
  ): PanelState {
    const snapshot = result.ok ? result.snapshot : null;
    const hiddenPreparationError =
      !result.ok && options.passive === true && isPreparationError(result.error);
    const error = result.ok || hiddenPreparationError ? null : result.error;
    const snapshotState = result.ok ? mapSnapshotState(result.snapshot) : "runtime_error";
    const state = options.savedMeeting
      ? "meeting_saved"
      : result.ok
        ? snapshotState !== "preparing" && isLiveSnapshot(result.snapshot) && attachment === null
          ? "ready"
          : snapshotState
        : hiddenPreparationError
          ? "ready"
          : mapErrorState(result.error);
    const meetingId = result.ok
      ? (result.snapshot.session?.session_id ?? attachment?.meetingId ?? null)
      : (attachment?.meetingId ?? null);
    const mentionItemId =
      meetingId !== null &&
      host !== null &&
      attachment !== null &&
      (snapshot?.session?.status === "recording" || snapshot?.session?.status === "paused") &&
      state !== "host_offline"
        ? encodeMentionId({
            threadId,
            projectId: null,
            hostId: host.id,
            meetingId,
          })
        : null;
    return panelState({
      threadId,
      host,
      attachment,
      savedMeeting: options.savedMeeting ?? null,
      firstUse: options.firstUse ?? false,
      preparationNeeded: hiddenPreparationError,
      snapshot:
        state === "meeting_saved"
          ? null
          : result.ok
            ? result.snapshot
            : state === "ready"
              ? createEmptySnapshot()
              : null,
      error,
      state,
      mentionItemId: state === "meeting_saved" ? null : mentionItemId,
    });
  }

  async function getPanelState(threadId: string): Promise<PanelState> {
    const [savedMeeting, seenCapture, lastError] = await Promise.all([
      readSavedMeeting(threadId),
      hasSeenCapture(threadId),
      readLastError(threadId),
    ]);
    if (savedMeeting) {
      const host = await findHost(savedMeeting.hostId);
      return panelForResult(
        threadId,
        host,
        null,
        { ok: true, snapshot: createEmptySnapshot() },
        { savedMeeting, firstUse: false, passive: true },
      );
    }
    const { host, attachment, result } = await readSnapshotForThread(threadId);
    if (result.ok && isLiveSnapshot(result.snapshot)) {
      await clearLastError(threadId);
    } else if (lastError) {
      return panelForResult(
        threadId,
        host,
        attachment,
        { ok: false, error: lastError },
        { firstUse: !seenCapture },
      );
    }
    return panelForResult(threadId, host, attachment, result, {
      firstUse: !seenCapture,
      passive: true,
    });
  }

  async function attachSnapshot(threadId: string, host: ResolvedHost, snapshot: LiveSnapshot) {
    const session = snapshot.session;
    if (!session || !isLiveStatus(session.status)) {
      throw new Error("No live Margins meeting is available.");
    }
    await saveAttachment({
      threadId,
      hostId: host.id,
      meetingId: session.session_id,
      attachedAtUnixMs: Date.now(),
      detachedAtUnixMs: null,
      generation: session.generation,
    });
    await Promise.all([
      clearSavedMeeting(threadId),
      clearLastError(threadId),
      markCaptureSeen(threadId),
    ]);
  }

  async function runAttachedOperation(
    threadId: string,
    operation: "pause" | "resume" | "stop",
  ): Promise<PanelState> {
    const attachment = await readAttachment(threadId);
    if (!attachment) return getPanelState(threadId);
    const routedHost = await findHost(attachment.hostId);
    const result = await callHost(routedHost, operation, {
      operationId: requestId(operation, threadId),
      sessionId: attachment.meetingId,
      expectedGeneration: attachment.generation,
    });
    if (result.ok && operation === "stop") {
      const meetingId = result.stopped_session_id || attachment.meetingId;
      await Promise.all([
        saveStoppedMeeting({
          threadId,
          hostId: attachment.hostId,
          meetingId,
          savedAtUnixMs: Date.now(),
        }),
        clearAttachment(threadId),
        markCaptureSeen(threadId),
      ]);
    } else if (result.ok && result.snapshot.session) {
      await saveAttachment({
        ...attachment,
        generation: result.snapshot.session.generation,
      });
    }
    bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: operation });
    if (!result.ok) {
      await saveLastError(threadId, result.error);
      return panelForResult(threadId, routedHost, attachment, result, {
        firstUse: !(await hasSeenCapture(threadId)),
      });
    }
    return getPanelState(threadId);
  }

  bb.rpc.register(marginsRpcContract, {
    getPanelState({ threadId }) {
      return getPanelState(threadId);
    },
    async startMeeting({ threadId, title }) {
      await clearLastError(threadId);
      const host = await resolveHost(threadId);
      if (!host) return getPanelState(threadId);
      const firstUse = !(await hasSeenCapture(threadId));
      const existing = await callHost(host, "readSnapshot", { sessionId: "current" });
      if (existing.ok && isLiveSnapshot(existing.snapshot)) {
        await attachSnapshot(threadId, host, existing.snapshot);
        bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "attach" });
        return getPanelState(threadId);
      }
      if (!existing.ok && !isPreparationError(existing.error)) {
        await saveLastError(threadId, existing.error);
        return panelForResult(threadId, host, null, existing, { firstUse });
      }

      let ready = existing;
      if (!existing.ok) {
        ready = await callHost(host, "ensureRuntime", {});
        bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "ensure-runtime" });
        if (!ready.ok) {
          await saveLastError(threadId, ready.error);
          return panelForResult(threadId, host, null, ready, { firstUse });
        }
        if (isLiveSnapshot(ready.snapshot)) {
          await attachSnapshot(threadId, host, ready.snapshot);
          bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "attach" });
          return getPanelState(threadId);
        }
      }

      const thread = (await bb.sdk.threads.get({ threadId })) as unknown as {
        title?: string | null;
      };
      const result = await callHost(host, "start", {
        operationId: requestId("start", threadId),
        name: meetingName(title?.trim() || thread.title?.trim()),
      });
      if (!result.ok) {
        await saveLastError(threadId, result.error);
        bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "start" });
        return panelForResult(threadId, host, null, result, { firstUse });
      }
      await attachSnapshot(threadId, host, result.snapshot);
      bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "start" });
      return getPanelState(threadId);
    },
    pauseMeeting({ threadId }) {
      return runAttachedOperation(threadId, "pause");
    },
    resumeMeeting({ threadId }) {
      return runAttachedOperation(threadId, "resume");
    },
    stopMeeting({ threadId }) {
      return runAttachedOperation(threadId, "stop");
    },
    async updateNotepad({ threadId, expectedNotepadRevision, text }) {
      const attachment = await readAttachment(threadId);
      if (!attachment) return getPanelState(threadId);
      const routedHost = await findHost(attachment.hostId);
      const result = await callHost(routedHost, "updateNotepad", {
        operationId: requestId("notepad", threadId),
        sessionId: attachment.meetingId,
        expectedGeneration: attachment.generation,
        expectedNotepadRevision,
        text,
      });
      if (!result.ok) {
        const latest = await callHost(routedHost, "readSnapshot", {
          sessionId: attachment.meetingId,
        });
        if (latest.ok) {
          return {
            ...panelForResult(threadId, routedHost, attachment, latest, {
              firstUse: !(await hasSeenCapture(threadId)),
            }),
            error: result.error,
          };
        }
        return panelForResult(threadId, routedHost, attachment, result, {
          firstUse: !(await hasSeenCapture(threadId)),
        });
      }
      if (result.ok && result.snapshot.session) {
        await saveAttachment({
          ...attachment,
          generation: result.snapshot.session.generation,
        });
      }
      bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "notepad" });
      return panelForResult(threadId, routedHost, attachment, result, {
        firstUse: !(await hasSeenCapture(threadId)),
      });
    },
    async dismissSavedMeeting({ threadId }) {
      await clearSavedMeeting(threadId);
      await clearLastError(threadId);
      bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "dismiss-saved" });
      return getPanelState(threadId);
    },
    async detachThread({ threadId }) {
      await clearAttachment(threadId);
      bb.realtime.publish(REALTIME_CHANNEL, { threadId, reason: "detach" });
      return getPanelState(threadId);
    },
  });

  bb.ui.registerMentionProvider({
    id: "margins",
    label: "Margins",
    triggers: ["@"],
    async search({ query, threadId, projectId }) {
      if (!threadId) return [];
      const needle = query.trim().toLowerCase();
      if (needle && !"margins".startsWith(needle)) return [];
      const state = await getPanelState(threadId);
      const meetingId = state.snapshot?.session?.session_id ?? state.attachment?.meetingId ?? null;
      if (!state.host || !meetingId || !state.mention.available) return [];
      return [
        {
          id: encodeMentionId({
            threadId,
            projectId,
            hostId: state.host.id,
            meetingId,
          }),
          title: "@Margins",
          subtitle:
            state.state === "paused" ? "Recording is paused" : "Use what Margins is hearing now",
        },
      ];
    },
    async resolve(itemId) {
      const decoded = decodeMentionId(itemId);
      if (!decoded) throw new Error("Margins context was not available.");
      const config = await readSettings();
      const maxTranscriptChars = parseWindowChars(config.transcriptWindowChars);
      const result = await callHost(await findHost(decoded.hostId), "readSnapshot", {
        sessionId: decoded.meetingId,
      });
      if (!result.ok) throw new Error(result.error.message);
      const packet = buildWatermarkPacket(result.snapshot, maxTranscriptChars);
      return { context: contextText(packet) };
    },
  });

  bb.agents.configure((context) => {
    if (
      context.project.kind === "personal" ||
      (context.origin.kind === "fork" && context.origin.pluginId === "side-chat")
    ) {
      return { tools: [], skills: [] };
    }
    return {
      tools: [],
      skills: ["watermark", "workspace-setup"],
      instructions:
        "When the user includes @Margins, use that injected live meeting context as the freshest source. Do not search old notes for live meeting feedback unless the user asks.",
    };
  });
}
