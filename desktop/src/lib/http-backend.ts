/**
 * HTTP backend for the Margins web/server deployment path.
 *
 * When the Rust server serves the built frontend it injects:
 *   window.__MARGINS_TOKEN__ = "<token>"
 *
 * Every Tauri command is mirrored here as a POST /api/invoke/<command> call.
 * Events are bridged via a single WebSocket at /ws/events.
 *
 * Web audio capture: getUserMedia → MediaRecorder → POST /api/audio/chunk
 */

import type {
  Settings,
  ProjectSource,
  ProjectRegistrationResult,
  AiStatus,
  IncludedAiStatus,
  AiReadiness,
  ResolutionPreview,
  VaultValidation,
  CalendarSuggestionResult,
  DeviceInfo,
  AudioTestResult,
  CaptureDeviceEvent,
  SystemAudioTestResult,
  RecordingStatus,
  HostedCaptureProtocol,
  WebRecordingRecoveryStatus,
  MemoLine,
  BackchannelSuggestionEvent,
  PrepHydrationEvent,
  HydratePrepRequest,
  ProcessingEvent,
  RecallIndexingEvent,
  SpeechModelPrepResult,
  SpeechModelProgressEvent,
  SpeechModelProbe,
  InstallCliResult,
  EnsureCliToolsResult,
  SessionInfo,
  GranolaImportSurvey,
  GranolaImportOptions,
  GranolaImportResult,
  GranolaImportStatus,
  GranolaRemoteImportResult,
  GranolaImportProgressEvent,
  FileDropPayload,
  UpdateTitleResult,
} from "./tauri";
import {
  acquireWebMicrophone,
  classifyWebMicrophoneFailure,
  enumerateWebMicrophones,
  selectWebRecorderMimeType,
  WebMicrophoneFailure,
  webMicrophonePermission,
  webMicrophoneSupported,
} from "./web-microphone-permission";
import { WebCaptureOperationController } from "./web-capture-operation";
import { createWebMicrophoneMeter, type WebMicrophoneMeter } from "./web-microphone-meter";
import { createWebLivePcmCapture, type WebLivePcmCapture } from "./web-live-pcm";
import { requireSuccessfulWebUpload } from "./web-upload-envelope";
import { convergeHostedPauseState, hostedPostureReconciliationAction } from "./web-capture-lifecycle";
import { decorateHostedRecordingStatus } from "./web-capture-status";
import {
  bindDurableMediaRecorder,
  stopMediaRecorderWithDeadline,
  WebDurableUploadQueue,
} from "./web-durable-upload";
import {
  missingHostedRecoveryAuthorityIds,
  reconcileHostedAuthorityFailure,
  selectWebRecoveryAuthority,
} from "./web-recovery-authority";
import {
  hostedRecordingLookupWasMissing,
  resolveAmbiguousHostedDiscard,
  resolveAmbiguousHostedStop,
} from "./hosted-command-reconciliation";
import {
  hostedCaptureProtocolHeaders,
  HOSTED_CAPTURE_PROTOCOL_VERSION,
  validateHostedCaptureProtocol,
} from "./hosted-capture-protocol";
import { runHostedStartPreflight } from "./hosted-start-preflight";

// ---------------------------------------------------------------------------
// Auth token
// ---------------------------------------------------------------------------

declare global {
  interface Window {
    __MARGINS_TOKEN__?: string;
  }
}

function getToken(): string | undefined {
  return typeof window !== "undefined" ? window.__MARGINS_TOKEN__ : undefined;
}

function isRealHttpHarness(): boolean {
  return typeof window !== "undefined" && new URLSearchParams(window.location.search).get("backend") === "http";
}

// ---------------------------------------------------------------------------
// Generic HTTP invoke helper
// ---------------------------------------------------------------------------

export async function invokeHttp<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const token = getToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...hostedCaptureProtocolHeaders(),
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  const res = await fetch(`/api/invoke/${command}`, {
    method: "POST",
    headers,
    body: JSON.stringify(args ?? {}),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`HTTP ${res.status} from ${command}: ${text}`);
  }
  const envelope = await res.json() as { ok: boolean; result?: T; error?: string };
  if (!envelope.ok) {
    throw new Error(envelope.error ?? `Command ${command} returned ok:false`);
  }
  return envelope.result as T;
}

// ---------------------------------------------------------------------------
// WebSocket event bridge
// ---------------------------------------------------------------------------

type EventHandler<T = unknown> = (payload: T) => void;

// Per-event listener registries
const eventListeners = new Map<string, Set<EventHandler>>();

function listenersFor(event: string): Set<EventHandler> {
  let set = eventListeners.get(event);
  if (!set) {
    set = new Set();
    eventListeners.set(event, set);
  }
  return set;
}

let ws: WebSocket | null = null;
let wsOpenPromise: Promise<void> | null = null;
let wsReconnectDelay = 500;
const WS_MAX_DELAY = 10_000;

function ensureWs(): Promise<void> {
  if (ws?.readyState === WebSocket.OPEN) return Promise.resolve();
  if (wsOpenPromise) return wsOpenPromise;
  const token = getToken();
  const url = token
    ? `/ws/events?token=${encodeURIComponent(token)}`
    : "/ws/events";

  // Use wss: when the page is served over https:
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const fullUrl = `${proto}//${window.location.host}${url}`;

  let resolveOpen!: () => void;
  let rejectOpen!: (reason: Error) => void;
  const openPromise = new Promise<void>((resolve, reject) => {
    resolveOpen = resolve;
    rejectOpen = reject;
  });
  wsOpenPromise = openPromise;

  let socket: WebSocket;
  try {
    socket = new WebSocket(fullUrl);
  } catch (err) {
    wsOpenPromise = null;
    const error = err instanceof Error ? err : new Error(String(err));
    rejectOpen(error);
    return openPromise;
  }

  socket.onopen = () => {
    ws = socket;
    wsReconnectDelay = 500; // reset backoff
    const ready = resolveOpen;
    if (wsOpenPromise === openPromise) wsOpenPromise = null;
    ready();
  };

  socket.onmessage = (msg) => {
    try {
      const frame = JSON.parse(msg.data as string) as { event: string; payload: unknown };
      const handlers = eventListeners.get(frame.event);
      if (handlers) {
        for (const h of handlers) h(frame.payload);
      }
    } catch {
      // malformed frame; ignore
    }
  };

  socket.onerror = () => {
    // onclose will fire next and handle reconnect
  };

  socket.onclose = () => {
    if (ws === socket) ws = null;
    if (wsOpenPromise === openPromise) {
      wsOpenPromise = null;
      rejectOpen(new Error("Processing event WebSocket closed before it opened."));
    }
    const delay = wsReconnectDelay;
    wsReconnectDelay = Math.min(wsReconnectDelay * 2, WS_MAX_DELAY);
    setTimeout(() => { void ensureWs().catch(() => {}); }, delay);
  };

  return openPromise;
}

/**
 * Subscribe to a server-sent event by name. Returns an unlisten function
 * matching the Tauri listen semantics used in tauri.ts.
 */
export function listen<T>(event: string, handler: EventHandler<T>): () => void {
  void ensureWs().catch((err) => {
    console.warn("[http-backend] event WebSocket connection failed:", err);
  });
  const set = listenersFor(event) as Set<EventHandler<T>>;
  set.add(handler);
  return () => set.delete(handler);
}

/** Register first, then resolve only once the server event socket is OPEN. */
async function listenWhenOpen<T>(event: string, handler: EventHandler<T>): Promise<() => void> {
  const unlisten = listen(event, handler);
  try {
    await ensureWs();
    return unlisten;
  } catch (err) {
    unlisten();
    throw err;
  }
}

// ---------------------------------------------------------------------------
// Web audio capture state
// ---------------------------------------------------------------------------

interface AudioCaptureState {
  sessionName: string;
  recordingId: string;
  ownerId: string;
  stream: MediaStream;
  recorder: MediaRecorder;
  meter: WebMicrophoneMeter | null;
  livePcm: WebLivePcmCapture | null;
  durableUploads: WebDurableUploadQueue;
  uploadErrors: Error[];
  pcmDiagnostics: { error: Error | null };
  healthGraceStartedAtMs: number;
  heartbeatTimer: ReturnType<typeof setInterval> | null;
}

let captureState: AudioCaptureState | null = null;
interface RecoveryAuthority {
  sessionName: string;
  recordingId: string;
  ownerId: string;
  healthGraceStartedAtMs: number;
  heartbeatTimer: ReturnType<typeof setInterval> | null;
}
const recoveryAuthorities = new Map<string, RecoveryAuthority>();
const pendingRecoveryOwnerIds = new Map<string, string>();
let hostedFinalizationWarning: string | null = null;

function invalidateHostedAuthority(recordingId: string, ownerId: string, error: unknown): void {
  const recovery = recoveryAuthorities.get(recordingId);
  if (recovery?.ownerId === ownerId) clearRecoveryAuthority(recovery);
  pendingRecoveryOwnerIds.delete(recordingId);
  const local = captureState;
  if (local?.recordingId === recordingId && local.ownerId === ownerId) {
    if (local.heartbeatTimer) clearInterval(local.heartbeatTimer);
    local.heartbeatTimer = null;
    const failure = new Error(`Hosted capture authority was lost: ${error instanceof Error ? error.message : error}`);
    if (!local.uploadErrors.some(existing => existing.message === failure.message)) {
      local.uploadErrors.push(failure);
    }
  }
}

async function reconcileHeartbeatFailure(
  recordingId: string,
  ownerId: string,
  initialError: unknown,
): Promise<void> {
  const outcome = await reconcileHostedAuthorityFailure(
    initialError,
    async () => {
      // A server restart reconstructs recoveries without persisted owner
      // secrets. Reclaim the exact ID with this tab's existing capability and
      // re-hydrate before allowing later memo mutation or Finish.
      await invokeHttp("claim_web_recording_recovery", { recordingId, ownerId });
      await invokeHttp("hydrate_web_recording_memo", { recordingId, ownerId });
    },
    error => invalidateHostedAuthority(recordingId, ownerId, error),
  );
  if (outcome === "reclaimed") {
    console.warn(`[http-backend] reclaimed hosted capture ${recordingId} after server restart`);
  } else if (outcome === "retry") {
    console.warn("[http-backend] capture heartbeat failed:", initialError);
  }
}

function startOwnerHeartbeat(recordingId: string, ownerId: string): ReturnType<typeof setInterval> {
  let inFlight = false;
  const beat = () => {
    if (inFlight) return;
    inFlight = true;
    void invokeHttp("heartbeat_web_recording", { recordingId, ownerId })
      .catch(error => reconcileHeartbeatFailure(recordingId, ownerId, error))
      .finally(() => { inFlight = false; });
  };
  const timer = setInterval(beat, 2_000);
  beat();
  return timer;
}

function installRecoveryAuthority(sessionName: string, recordingId: string, ownerId: string): void {
  const existing = recoveryAuthorities.get(recordingId);
  if (existing?.heartbeatTimer) clearInterval(existing.heartbeatTimer);
  const authority: RecoveryAuthority = {
    sessionName,
    recordingId,
    ownerId,
    healthGraceStartedAtMs: Date.now(),
    heartbeatTimer: null,
  };
  recoveryAuthorities.set(recordingId, authority);
  authority.heartbeatTimer = startOwnerHeartbeat(recordingId, ownerId);
}

function clearRecoveryAuthority(authority: RecoveryAuthority): void {
  if (authority?.heartbeatTimer) clearInterval(authority.heartbeatTimer);
  if (recoveryAuthorities.get(authority.recordingId) === authority) {
    recoveryAuthorities.delete(authority.recordingId);
  }
}

function newCaptureOwnerId(): string {
  return typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `capture-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function scopedAuthority(
  recordingId?: string | null,
  sessionName?: string | null,
): { sessionName: string; recordingId: string; ownerId: string } | null {
  if (captureState && (!recordingId || captureState.recordingId === recordingId)
      && (!sessionName || captureState.sessionName === sessionName)) return captureState;
  return selectWebRecoveryAuthority(recoveryAuthorities, recordingId, sessionName);
}

/** Latest browser-mic peak level (0..1) at rAF freshness, for UI that needs
 *  smoother sampling than the 250ms status poll. Null when no live meter. */
export function webMicrophoneLevel(): number | null {
  return captureState?.meter?.snapshot().level ?? null;
}

/** Latest browser-mic RMS energy (0..1) — smoother than peak, feeds the
 *  rolling waveform. Null when no live meter. */
export function webMicrophoneRms(): number | null {
  return captureState?.meter?.snapshot().rms ?? null;
}

function withWebMicrophoneTelemetry(status: RecordingStatus): RecordingStatus {
  const state = captureState;
  const localCapture = state?.recordingId === status.web_recording_id ? state : null;
  const sample = localCapture?.meter?.snapshot();
  const recovery = !localCapture && status.web_recording_id
    ? recoveryAuthorities.get(status.web_recording_id) ?? null
    : null;
  return decorateHostedRecordingStatus(status, localCapture ? {
    recordingId: localCapture.recordingId,
    sessionName: localCapture.sessionName,
    micLevel: sample?.level ?? null,
    micAudioFrameCount: sample?.audioFrameCount ?? 0,
    durableUploadError: localCapture.uploadErrors[localCapture.uploadErrors.length - 1]?.message ?? null,
    livePcmError: localCapture.pcmDiagnostics.error?.message ?? null,
    healthGraceStartedAtMs: localCapture.healthGraceStartedAtMs,
    authority: "capture",
  } : recovery ? {
    recordingId: recovery.recordingId,
    sessionName: recovery.sessionName,
    micLevel: null,
    micAudioFrameCount: 0,
    durableUploadError: null,
    livePcmError: null,
    healthGraceStartedAtMs: recovery.healthGraceStartedAtMs,
    authority: "recovery",
  } : null);
}

async function postAudioChunk(
  recordingId: string,
  ownerId: string,
  sequence: number,
  chunk: Blob,
  signal: AbortSignal,
): Promise<void> {
  const token = getToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/octet-stream",
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  headers["X-Margins-Capture-Owner"] = ownerId;
  headers["X-Margins-Recording-Id"] = recordingId;
  headers["X-Margins-Capture-Protocol"] = String(HOSTED_CAPTURE_PROTOCOL_VERSION);
  headers["X-Margins-Chunk-Sequence"] = String(sequence);
  const response = await fetch("/api/audio/chunk", {
    method: "POST",
    headers,
    body: chunk,
    signal,
  });
  await requireSuccessfulWebUpload(response, "Audio chunk upload");
}

async function postLivePcmChunk(
  recordingId: string,
  ownerId: string,
  sampleRate: number,
  samples: Float32Array,
  signal: AbortSignal,
): Promise<void> {
  const token = getToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/octet-stream",
  };
  if (token) headers["Authorization"] = `Bearer ${token}`;
  headers["X-Margins-Capture-Owner"] = ownerId;
  headers["X-Margins-Recording-Id"] = recordingId;
  headers["X-Margins-Capture-Protocol"] = String(HOSTED_CAPTURE_PROTOCOL_VERSION);
  const body = samples.buffer.slice(
    samples.byteOffset,
    samples.byteOffset + samples.byteLength,
  ) as ArrayBuffer;
  const response = await fetch(
    `/api/live-audio/pcm?channel=mic&sample_rate=${sampleRate}`,
    { method: "POST", headers, body, signal },
  );
  await requireSuccessfulWebUpload(response, "Live PCM upload");
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

export async function getSettings(): Promise<Settings> {
  return invokeHttp("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  return invokeHttp("update_settings", { settings });
}

export async function updateAudioSettings(settings: Pick<Settings, "audio_input_ready" | "system_audio_ready" | "input_device_mode" | "input_device_uid" | "input_device_name">): Promise<void> {
  return invokeHttp("update_audio_settings", {
    audioInputReady: settings.audio_input_ready,
    systemAudioReady: settings.system_audio_ready,
    inputDeviceMode: settings.input_device_mode,
    inputDeviceUid: settings.input_device_uid,
    inputDeviceName: settings.input_device_name,
  });
}

export async function registerProject(settings: Settings, project: ProjectSource): Promise<ProjectRegistrationResult> {
  return invokeHttp("register_project", { settings, project });
}

export async function updateProjectReadiness(id: string, readiness: "ready" | "needs_setup" | "updating" | "error"): Promise<Settings> {
  if (isRealHttpHarness()) {
    void id;
    void readiness;
    return getSettings();
  }
  return invokeHttp("update_project_readiness", { id, readiness });
}

export async function getAiStatus(): Promise<AiStatus> {
  return invokeHttp("get_ai_status");
}

export async function getIncludedAiStatus(): Promise<IncludedAiStatus> {
  return invokeHttp("get_included_ai_status");
}

export async function prepareIncludedAi(): Promise<IncludedAiStatus> {
  return invokeHttp("prepare_included_ai");
}

export async function signInChatgpt(): Promise<AiStatus> {
  return invokeHttp("sign_in_chatgpt");
}

export async function getAiReadiness(): Promise<AiReadiness> {
  return invokeHttp("get_ai_readiness");
}

export async function previewAiResolution(settings: Settings): Promise<ResolutionPreview> {
  return invokeHttp("preview_ai_resolution", { settings });
}

export async function validateVault(path: string): Promise<VaultValidation> {
  return invokeHttp("validate_vault", { path });
}

export async function ensureDefaultVault(path: string): Promise<VaultValidation> {
  return invokeHttp("ensure_default_vault", { path });
}

export async function indexVault(path: string): Promise<VaultValidation> {
  return invokeHttp("index_vault", { path });
}

export async function getCalendarEventSuggestion(): Promise<CalendarSuggestionResult> {
  return invokeHttp("get_calendar_event_suggestion");
}

export async function updateSessionPeople(name: string, people: string[]): Promise<string[]> {
  return invokeHttp("update_session_people", { name, people });
}

export async function updateSessionTitle(name: string, title: string): Promise<UpdateTitleResult> {
  return invokeHttp("update_session_title", { name, title });
}

/** selectVaultFolder is not available in the http backend (no native dialog). */
export async function selectVaultFolder(_defaultPath?: string | null): Promise<string | null> {
  // The server deployment has no native file dialog; return null so the caller
  // can fall back to a text-input UI.
  return null;
}

/** No native dialog on the server; the caller keeps the existing subfolder. */
export async function selectProjectSubfolder(_projectRoot: string): Promise<string | null> {
  return null;
}

export async function prepareSpeechModels(parakeetModelDir?: string | null): Promise<SpeechModelPrepResult> {
  return invokeHttp("prepare_speech_models", { parakeetModelDir: parakeetModelDir ?? null });
}

export async function probeSpeechModels(customPath?: string): Promise<SpeechModelProbe> {
  return invokeHttp("probe_speech_models", { customPath: customPath ?? null });
}

export async function cancelSpeechModelDownload(): Promise<void> {
  return invokeHttp("cancel_speech_model_download");
}

export async function clearSpeechModels(): Promise<string> {
  return invokeHttp("clear_speech_models");
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

export async function listDevices(): Promise<DeviceInfo[]> {
  const microphones = await enumerateWebMicrophones();
  return microphones.map((device, index) => ({
    uid: device.deviceId,
    name: device.label,
    is_default: index === 0,
  }));
}

async function measureWebMicrophonePeak(stream: MediaStream): Promise<number> {
  if (typeof AudioContext === "undefined") return 0;
  let context: AudioContext | null = null;
  try {
    context = new AudioContext();
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    context.createMediaStreamSource(stream).connect(analyser);
    if (context.state === "suspended") await context.resume();
    const samples = new Float32Array(analyser.fftSize);
    let peak = 0;
    for (let attempt = 0; attempt < 10; attempt++) {
      analyser.getFloatTimeDomainData(samples);
      for (const sample of samples) peak = Math.max(peak, Math.abs(sample));
      await new Promise(resolve => window.setTimeout(resolve, 50));
    }
    return peak;
  } catch {
    return 0;
  } finally {
    await context?.close().catch(() => undefined);
  }
}

export async function testAudioInput(deviceId?: string | null): Promise<AudioTestResult> {
  const stream = await acquireWebMicrophone(deviceId);
  webMicrophonePermission.noteGranted();
  try {
    const track = stream.getAudioTracks()[0];
    return {
      device_name: track?.label || "Browser microphone",
      peak: await measureWebMicrophonePeak(stream),
      drop_count: 0,
      ok: Boolean(track?.enabled),
    };
  } finally {
    stream.getTracks().forEach(track => track.stop());
  }
}

export async function testSystemAudioTap(): Promise<SystemAudioTestResult> {
  return invokeHttp("test_system_audio_tap");
}

export async function openPrivacyPane(pane: "microphone" | "system-audio"): Promise<void> {
  return invokeHttp("open_privacy_pane", { pane });
}

export async function restartApp(): Promise<void> {
  return invokeHttp("restart_app");
}

export async function installCliTool(): Promise<InstallCliResult> {
  return invokeHttp("install_cli_tool");
}

export async function ensureCliTools(): Promise<EnsureCliToolsResult> {
  return invokeHttp("ensure_cli_tools");
}

export async function installWorkspaceSkills(_projectPath: string): Promise<void> {
  // no-op: skill files ship bundled in native builds
}

// App update is not available in the http backend
export async function installAvailableAppUpdate(): Promise<boolean> {
  return false;
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

export async function listSessions(projectId?: string | null): Promise<SessionInfo[]> {
  return invokeHttp("list_sessions", { projectId: projectId ?? null });
}

export async function getProjectFilesFingerprint(projectId?: string | null): Promise<{ project_id: string | null; fingerprint: string }> {
  return invokeHttp("get_project_files_fingerprint", { projectId: projectId ?? null });
}

export async function deleteSession(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("delete_session", { name, projectId: projectId ?? null });
}

export async function reconcileProjectNotes(projectId?: string | null): Promise<SessionInfo[]> {
  return invokeHttp("reconcile_project_notes", { projectId: projectId ?? null });
}

// ---------------------------------------------------------------------------
// Audio import
// ---------------------------------------------------------------------------

export async function importAudioFile(path: string, maxSpeakers?: number | null, projectId?: string | null): Promise<string> {
  return invokeHttp("import_audio_file", { path, maxSpeakers: maxSpeakers ?? null, projectId: projectId ?? null });
}

export async function surveyGranolaImport(paths: string[], projectId?: string | null): Promise<GranolaImportSurvey> {
  return invokeHttp("survey_granola_import", { paths, projectId: projectId ?? null });
}

export async function importGranolaFiles(paths: string[], options: GranolaImportOptions, projectId?: string | null): Promise<GranolaImportResult> {
  return invokeHttp("import_granola_files", { paths, options, projectId: projectId ?? null });
}

export async function getGranolaImportStatus(): Promise<GranolaImportStatus> {
  return invokeHttp("get_granola_import_status");
}

export async function authorizeGranolaImport(): Promise<GranolaImportStatus> {
  return invokeHttp("authorize_granola_import");
}

export async function revokeGranolaImportAuthorization(account?: string | null): Promise<GranolaImportStatus> {
  return invokeHttp("revoke_granola_import_authorization", { account: account ?? null });
}

export async function importGranolaMcp(projectId?: string | null): Promise<GranolaRemoteImportResult> {
  return invokeHttp("import_granola_mcp", { projectId: projectId ?? null });
}

/** pickAudioFiles / pickGranolaImportFiles: no native dialog on http path. */
export async function pickAudioFiles(): Promise<string[]> {
  return [];
}

export async function pickGranolaImportFiles(): Promise<string[]> {
  return [];
}

/** onFileDrop: native drag-drop is not available on the http path. */
export async function onFileDrop(_callback: (payload: FileDropPayload) => void): Promise<() => void> {
  return () => {};
}

// ---------------------------------------------------------------------------
// Recording — web audio capture
// ---------------------------------------------------------------------------

const webCaptureOperations = new WebCaptureOperationController();

function stopStream(stream: MediaStream | null): void {
  stream?.getTracks().forEach(track => track.stop());
}

function cancelledWebCaptureFailure(): WebMicrophoneFailure {
  return new WebMicrophoneFailure("aborted", "Microphone setup cancelled.");
}

function preferredRecorderMimeType(): string | null {
  if (typeof MediaRecorder.isTypeSupported !== "function") return null;
  return selectWebRecorderMimeType(mimeType => MediaRecorder.isTypeSupported(mimeType));
}

interface WebRecordingStart {
  sessionName: string;
  recordingId: string;
}

function stopWebSession(recordingId: string, ownerId: string): Promise<string> {
  return invokeHttp<string>("stop_recording", { recordingId, ownerId });
}

function discardWebSession(recordingId: string, ownerId: string): Promise<string> {
  return invokeHttp<string>("discard_recording", { recordingId, ownerId });
}

async function discardAllocatedWebSession(
  sessionName: string,
  recordingId: string,
  ownerId: string,
): Promise<void> {
  try {
    await discardWebSession(recordingId, ownerId);
  } catch (error) {
    // Cancellation crossed server allocation but cleanup was ambiguous. Keep
    // the exact same capability alive so this tab can retry instead of
    // manufacturing an ownerless phantom recovery.
    installRecoveryAuthority(sessionName, recordingId, ownerId);
    throw error;
  }
}

export async function startRecording(
  name: string,
  deviceId?: string | null,
  projectId?: string | null,
): Promise<string> {
  hostedFinalizationWarning = null;
  // Browsers only expose getUserMedia in a secure context (HTTPS, or
  // localhost/127.0.0.1). Over plain http on a LAN/Tailscale IP,
  // navigator.mediaDevices is undefined, so surface the actual fix instead of
  // a generic permission error.
  if (!window.isSecureContext) {
    throw new WebMicrophoneFailure("insecure-context");
  }
  if (!webMicrophoneSupported()) throw new WebMicrophoneFailure("unsupported");

  const operation = webCaptureOperations.begin();
  const ownerId = newCaptureOwnerId();
  let stream: MediaStream | null = null;
  let meter: WebMicrophoneMeter | null = null;
  let livePcm: WebLivePcmCapture | null = null;
  let startupHeartbeat: ReturnType<typeof setInterval> | null = null;
  const pcmDiagnostics = { error: null as Error | null };
  let sessionName: string | null = null;
  let recordingId: string | null = null;
  let resolvePcmSession: (recordingId: string | null) => void = () => {};
  const pcmSession = new Promise<string | null>((resolve) => {
    resolvePcmSession = resolve;
  });
  try {
    // Register cancellation before this network preflight. A Cancel click must
    // prevent a slow response from proceeding to permission or allocation.
    const active = await runHostedStartPreflight(
      () => invokeHttp<RecordingStatus>("get_recording_status"),
      () => listHostedRecordingRecoveries(),
    );
    if (!webCaptureOperations.isCurrent(operation)) throw cancelledWebCaptureFailure();
    if (active?.session_name) {
      throw new Error(
        `Recording '${active.session_name}' is already active. Return to its owner tab, or explicitly recover it after capture transport stops.`,
      );
    }

    stream = await acquireWebMicrophone(deviceId);
    webMicrophonePermission.noteGranted();
    if (!webCaptureOperations.isCurrent(operation)) {
      stopStream(stream);
      stream = null;
      throw cancelledWebCaptureFailure();
    }

    const mimeType = preferredRecorderMimeType();
    let recorder: MediaRecorder;
    try {
      recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
    } catch (error) {
      throw classifyWebMicrophoneFailure(error, { selectedDevice: Boolean(deviceId) });
    }

    // Prime Web Audio while this call still has the microphone permission/user
    // activation context. Creating it after the server session request can
    // leave AudioContext suspended in browsers with strict autoplay policy.
    // Batches captured during server setup wait for the allocated session name.
    livePcm = await createWebLivePcmCapture(stream, "pending", async (_pending, sampleRate, samples, signal) => {
      const allocatedRecording = await pcmSession;
      if (!allocatedRecording) throw new Error("Recording ended before live PCM session allocation");
      await postLivePcmChunk(allocatedRecording, ownerId, sampleRate, samples, signal);
      pcmDiagnostics.error = null;
    }, undefined, error => { pcmDiagnostics.error = error; });
    // PCM setup awaits AudioContext.resume(), which can outlive a Cancel click.
    // Recheck before allocating any backend session or starting a heartbeat.
    if (!webCaptureOperations.isCurrent(operation)) {
      await livePcm?.close();
      livePcm = null;
      stopStream(stream);
      stream = null;
      throw cancelledWebCaptureFailure();
    }

    // The browser permission prompt and recorder validation both complete
    // before the server allocates a session, preventing denied/ignored prompts
    // from leaving orphan server state.
    try {
      const allocated = await invokeHttp<WebRecordingStart>("start_recording", {
        name,
        deviceIndex: null,
        projectId: projectId ?? null,
        ownerId,
      });
      sessionName = allocated.sessionName;
      recordingId = allocated.recordingId;
      resolvePcmSession(recordingId);
      startupHeartbeat = startOwnerHeartbeat(recordingId, ownerId);
    } catch (error) {
      resolvePcmSession(null);
      stopStream(stream);
      stream = null;
      throw error;
    }

    if (await webCaptureOperations.discardIfStale(
      operation,
      recordingId,
      staleRecordingId => discardAllocatedWebSession(sessionName!, staleRecordingId, ownerId).catch(() => undefined),
    )) {
      stopStream(stream);
      stream = null;
      throw cancelledWebCaptureFailure();
    }

    // Meter the same MediaStream that MediaRecorder uploads. The server cannot
    // inspect browser samples, so its nullable telemetry is overlaid locally
    // by every status path below.
    meter = await createWebMicrophoneMeter(stream);
    if (await webCaptureOperations.discardIfStale(
      operation,
      recordingId,
      staleRecordingId => discardAllocatedWebSession(sessionName!, staleRecordingId, ownerId).catch(() => undefined),
    )) {
      await meter?.close();
      meter = null;
      await livePcm?.close();
      livePcm = null;
      stopStream(stream);
      stream = null;
      throw cancelledWebCaptureFailure();
    }

    const state: AudioCaptureState = {
      sessionName,
      recordingId,
      ownerId,
      stream,
      recorder,
      meter,
      livePcm,
      durableUploads: null as unknown as WebDurableUploadQueue,
      uploadErrors: [],
      pcmDiagnostics,
      healthGraceStartedAtMs: Date.now(),
      heartbeatTimer: null,
    };

    // This read authorizes later memo replacement and proves an empty client
    // buffer is based on the current server state.
    await invokeHttp<MemoLine[]>("hydrate_web_recording_memo", { recordingId, ownerId });

    state.durableUploads = new WebDurableUploadQueue(
      (chunk, sequence, signal) => postAudioChunk(recordingId!, ownerId, sequence, chunk, signal),
      error => {
        state.uploadErrors.push(error);
        console.warn("[http-backend] audio chunk upload failed:", error);
      },
    );

    bindDurableMediaRecorder(recorder, state.durableUploads);

    try {
      recorder.start(3000); // 3-second timeslice
    } catch (error) {
      await meter?.close();
      meter = null;
      await livePcm?.close();
      livePcm = null;
      stopStream(stream);
      stream = null;
      await discardAllocatedWebSession(sessionName, recordingId, ownerId).catch(() => undefined);
      throw classifyWebMicrophoneFailure(error, { selectedDevice: Boolean(deviceId) });
    }

    captureState = state;
    state.healthGraceStartedAtMs = Date.now();
    state.heartbeatTimer = startupHeartbeat;
    startupHeartbeat = null;
    webCaptureOperations.complete(operation);
    return sessionName;
  } catch (error) {
    resolvePcmSession(null);
    if (startupHeartbeat) clearInterval(startupHeartbeat);
    if (recordingId && captureState?.recordingId !== recordingId) {
      await discardAllocatedWebSession(sessionName ?? name, recordingId, ownerId).catch(() => undefined);
    }
    await meter?.close();
    await livePcm?.close();
    stopStream(stream);
    const failure = error instanceof WebMicrophoneFailure
      ? error
      : (error instanceof DOMException ? classifyWebMicrophoneFailure(error, { selectedDevice: Boolean(deviceId) }) : null);
    if (failure) {
      webMicrophonePermission.noteFailure(failure);
      throw failure;
    }
    throw error;
  } finally {
    webCaptureOperations.complete(operation);
  }
}

export async function cancelRecordingStartup(): Promise<boolean> {
  return webCaptureOperations.cancelCurrent();
}

export async function stopRecording(requestedRecordingId?: string | null): Promise<string> {
  const state = captureState;
  if (!state || (requestedRecordingId && state.recordingId !== requestedRecordingId)) {
    const authority = scopedAuthority(requestedRecordingId);
    if (!authority) {
      throw new Error("This tab does not own the active hosted capture. Take control explicitly before finishing it.");
    }
    const result = await stopWebSession(authority.recordingId, authority.ownerId);
    const recovery = recoveryAuthorities.get(authority.recordingId);
    if (recovery) clearRecoveryAuthority(recovery);
    return result;
  }

  try {
    // MediaRecorder normally emits its final dataavailable before onstop. If
    // the browser never emits onstop, preserve the received subset and report
    // that loss instead of holding Finish open forever.
    const recorderStopError = await stopMediaRecorderWithDeadline(state.recorder);
    if (recorderStopError) state.uploadErrors.push(recorderStopError);
  } finally {
    if (state.heartbeatTimer) clearInterval(state.heartbeatTimer);
    state.heartbeatTimer = null;
    await state.meter?.close();
    await state.livePcm?.close();
    stopStream(state.stream);
    if (captureState === state) captureState = null;
  }

  // onstop follows the final dataavailable event. Drain every upload registered
  // by that point before telling the server to finalize the recording.
  await state.durableUploads.close();
  let result: string;
  try {
    result = await stopWebSession(state.recordingId, state.ownerId);
  } catch (err) {
    let exactStatusExists: boolean | null = null;
    try {
      await invokeHttp<RecordingStatus>("get_web_recording_status", { recordingId: state.recordingId });
      exactStatusExists = true;
    } catch (reconciliationError) {
      if (hostedRecordingLookupWasMissing(reconciliationError)) exactStatusExists = false;
    }
    if (resolveAmbiguousHostedStop(exactStatusExists) === "completed") {
      const staleRecovery = recoveryAuthorities.get(state.recordingId);
      if (staleRecovery) clearRecoveryAuthority(staleRecovery);
      hostedFinalizationWarning = "The server finished saving this recording, but its response was lost. The completed session was reconciled by recording ID.";
      return state.sessionName;
    }
    installRecoveryAuthority(state.sessionName, state.recordingId, state.ownerId);
    if (state.uploadErrors.length === 0) throw err;
    const stopError = err instanceof Error ? err.message : String(err);
    const uploadDetail = state.uploadErrors.map((uploadError) => uploadError.message).join("; ");
    throw new Error(`Recording finalization failed (${stopError}); audio upload failures: ${uploadDetail}`);
  }
  if (state.uploadErrors.length > 0) {
    const detail = state.uploadErrors.map((err) => err.message).join("; ");
    hostedFinalizationWarning = `Recording saved from the audio chunks that reached the server. ${state.uploadErrors.length} WebM chunk upload(s) failed: ${detail}`;
    console.warn(`[http-backend] ${hostedFinalizationWarning}`);
  }
  return result;
}

export function consumeHostedFinalizationWarning(): string | null {
  const warning = hostedFinalizationWarning;
  hostedFinalizationWarning = null;
  return warning;
}

export async function discardRecording(requestedRecordingId?: string | null): Promise<string> {
  const authority = scopedAuthority(requestedRecordingId);
  if (!authority) {
    throw new Error("This tab does not own the active hosted capture. Take control explicitly before discarding it.");
  }
  if (captureState && captureState.recordingId === authority.recordingId) {
    const state = captureState;
    captureState = null;
    if (state.heartbeatTimer) clearInterval(state.heartbeatTimer);
    try {
      state.recorder.stop();
    } catch {
      // ignore if already stopped
    }
    await state.meter?.close();
    await state.livePcm?.close();
    stopStream(state.stream);
    installRecoveryAuthority(state.sessionName, state.recordingId, state.ownerId);
  }
  try {
    const result = await discardWebSession(authority.recordingId, authority.ownerId);
    const recovery = recoveryAuthorities.get(authority.recordingId);
    if (recovery) clearRecoveryAuthority(recovery);
    return result;
  } catch (error) {
    let exactStatus: RecordingStatus | null = null;
    let exactStatusExists: boolean | null = null;
    try {
      exactStatus = await invokeHttp<RecordingStatus>("get_web_recording_status", {
        recordingId: authority.recordingId,
      });
      exactStatusExists = true;
    } catch (reconciliationError) {
      if (hostedRecordingLookupWasMissing(reconciliationError)) exactStatusExists = false;
    }
    const phase = exactStatus?.web_recoveries?.find(
      recovery => recovery.recording_id === authority.recordingId,
    )?.recovery_phase;
    const resolution = resolveAmbiguousHostedDiscard(exactStatusExists, phase);
    if (resolution === "completed") {
      const recovery = recoveryAuthorities.get(authority.recordingId);
      if (recovery) clearRecoveryAuthority(recovery);
      hostedFinalizationWarning = "The server discarded this recording, but its response was lost. Cleanup was reconciled by recording ID.";
      return authority.sessionName;
    }
    // `cleanup_pending` is transactional server state: retain the same
    // capability and heartbeat so the user can retry the exact discard.
    throw error;
  }
}

export async function switchRecordingDevice(deviceIndex?: number): Promise<RecordingStatus> {
  return invokeHttp("switch_recording_device", { deviceIndex: deviceIndex ?? null });
}

export async function restartSystemAudioCapture(): Promise<RecordingStatus> {
  return invokeHttp("restart_system_audio_capture");
}

export async function pauseRecording(): Promise<RecordingStatus> {
  const state = captureState;
  if (!state) throw new Error("Only the browser tab capturing audio can pause this recording.");
  const lifecycle = capturePauseLifecycle(state);
  return withWebMicrophoneTelemetry(await convergeHostedPauseState(
    true,
    lifecycle,
    () => invokeHttp<RecordingStatus>("pause_recording", {
      recordingId: state.recordingId,
      ownerId: state.ownerId,
    }),
    () => invokeHttp<RecordingStatus>("get_web_recording_status", { recordingId: state.recordingId }),
  ));
}

export async function resumeRecording(): Promise<RecordingStatus> {
  const state = captureState;
  if (!state) throw new Error("Only the browser tab capturing audio can resume this recording.");
  const status = await convergeHostedPauseState(
    false,
    capturePauseLifecycle(state),
    () => invokeHttp<RecordingStatus>("resume_recording", {
      recordingId: state.recordingId,
      ownerId: state.ownerId,
    }),
    () => invokeHttp<RecordingStatus>("get_web_recording_status", { recordingId: state.recordingId }),
  );
  state.healthGraceStartedAtMs = Date.now();
  return withWebMicrophoneTelemetry(status);
}

function capturePauseLifecycle(state: AudioCaptureState) {
  return {
    async pauseLocal() {
      state.recorder.pause();
      await state.meter?.pause();
      await state.livePcm?.pause();
    },
    async resumeLocal() {
      state.recorder.resume();
      await state.meter?.resume();
      await state.livePcm?.resume();
    },
  };
}

async function reconcileLocalCapturePosture(status: RecordingStatus): Promise<void> {
  const state = captureState;
  if (!state || status.web_recording_id !== state.recordingId || state.recorder.state === "inactive") return;
  const shouldPause = Boolean(status.paused);
  const action = hostedPostureReconciliationAction(shouldPause, state.recorder.state);
  if (!action) return;

  try {
    if (action === "pause") state.recorder.pause();
    else state.recorder.resume();
  } catch (error) {
    console.warn("[http-backend] could not reconcile MediaRecorder posture:", error);
  }
  try {
    if (shouldPause) await state.meter?.pause();
    else await state.meter?.resume();
  } catch (error) {
    console.warn("[http-backend] could not reconcile microphone meter posture:", error);
  }
  try {
    if (shouldPause) await state.livePcm?.pause();
    else await state.livePcm?.resume();
  } catch (error) {
    state.pcmDiagnostics.error = error instanceof Error ? error : new Error(String(error));
    console.warn("[http-backend] could not reconcile live PCM posture:", error);
  }
  if (!shouldPause) state.healthGraceStartedAtMs = Date.now();
}

export async function setLiveTranscriptionMode(mode: "stereo_split" | "mic_diarized"): Promise<RecordingStatus> {
  return invokeHttp("set_live_transcription_mode", { mode });
}

export async function getRecordingStatus(): Promise<RecordingStatus> {
  const status = await invokeHttp<RecordingStatus>("get_recording_status");
  await reconcileLocalCapturePosture(status);
  return withWebMicrophoneTelemetry(status);
}

export async function getHostedRecordingStatus(recordingId: string): Promise<RecordingStatus> {
  const status = await invokeHttp<RecordingStatus>("get_web_recording_status", { recordingId });
  await reconcileLocalCapturePosture(status);
  return withWebMicrophoneTelemetry(status);
}

export async function negotiateHostedCaptureProtocol(): Promise<HostedCaptureProtocol> {
  let protocol: HostedCaptureProtocol;
  try {
    protocol = await invokeHttp<HostedCaptureProtocol>("get_hosted_capture_protocol");
  } catch (error) {
    throw new Error(`Hosted capture protocol negotiation failed. Reload after the server and browser assets are upgraded together. ${error instanceof Error ? error.message : error}`);
  }
  return validateHostedCaptureProtocol(protocol);
}

export async function listHostedRecordingRecoveries(): Promise<WebRecordingRecoveryStatus[]> {
  const recoveries = await invokeHttp<WebRecordingRecoveryStatus[]>("list_web_recording_recoveries");
  for (const missingId of missingHostedRecoveryAuthorityIds(
    recoveryAuthorities.keys(),
    recoveries.map(recovery => recovery.recording_id),
  )) {
    const authority = recoveryAuthorities.get(missingId);
    if (authority) clearRecoveryAuthority(authority);
    pendingRecoveryOwnerIds.delete(missingId);
  }
  return recoveries;
}

export async function syncMemo(lines: MemoLine[], sessionName: string, recordingId?: string | null): Promise<void> {
  const authority = scopedAuthority(recordingId, sessionName);
  if (!authority) throw new Error("This tab cannot replace memo state for a capture it does not own.");
  return invokeHttp("sync_memo", { lines, sessionName, recordingId: authority.recordingId, ownerId: authority.ownerId });
}

export async function checkpointMemoLine(lines: MemoLine[], committedIndex: number, sessionName: string, recordingId?: string | null): Promise<void> {
  const authority = scopedAuthority(recordingId, sessionName);
  if (!authority) throw new Error("This tab cannot commit memo state for a capture it does not own.");
  return invokeHttp("checkpoint_memo_line", { lines, committedIndex, sessionName, recordingId: authority.recordingId, ownerId: authority.ownerId });
}

export async function hydrateRecordingMemo(sessionName: string, recordingId?: string | null): Promise<MemoLine[]> {
  const authority = scopedAuthority(recordingId, sessionName);
  if (!authority) throw new Error("Select a specific hosted recovery before hydrating its memo.");
  return invokeHttp("hydrate_web_recording_memo", {
    recordingId: authority.recordingId,
    ownerId: authority.ownerId,
  });
}

export async function claimInterruptedRecording(sessionName: string, recordingId: string): Promise<MemoLine[]> {
  if (recoveryAuthorities.has(recordingId)) {
    return hydrateRecordingMemo(sessionName, recordingId);
  }
  const ownerId = pendingRecoveryOwnerIds.get(recordingId) ?? newCaptureOwnerId();
  pendingRecoveryOwnerIds.set(recordingId, ownerId);
  await invokeHttp("claim_web_recording_recovery", { recordingId, ownerId });
  installRecoveryAuthority(sessionName, recordingId, ownerId);
  pendingRecoveryOwnerIds.delete(recordingId);
  // A hydration failure does not revoke the capability the server accepted.
  // The same tab can retry hydration/claim without generating a conflicting ID.
  return hydrateRecordingMemo(sessionName, recordingId);
}

export async function requestBackchannelForMemo(lines: MemoLine[], committedIndex: number, sessionName: string, recordingId?: string | null): Promise<void> {
  const authority = scopedAuthority(recordingId, sessionName);
  if (!authority) throw new Error("This tab cannot request a backchannel for a capture it does not own.");
  return invokeHttp("request_backchannel_for_memo", {
    lines,
    committedIndex,
    sessionName,
    recordingId: authority.recordingId,
    ownerId: authority.ownerId,
    clientSentUnixMs: Date.now(),
  });
}

export async function steerBackchannelForMemo(
  memoIndex: number,
  steering: string,
  previousSuggestion: string | null,
  sessionName: string,
  recordingId?: string | null,
): Promise<void> {
  const authority = scopedAuthority(recordingId, sessionName);
  if (!authority) throw new Error("This tab cannot steer a backchannel for a capture it does not own.");
  return invokeHttp("steer_backchannel_for_memo", {
    memoIndex,
    steering,
    previousSuggestion,
    sessionName,
    recordingId: authority.recordingId,
    ownerId: authority.ownerId,
  });
}

export async function hydratePrepSketch(req: HydratePrepRequest): Promise<void> {
  return invokeHttp("hydrate_prep_sketch", { ...req, clientSentUnixMs: Date.now() });
}

export async function steerPrepHydration(sessionName: string, blockOrdinal: number, instruction: string): Promise<void> {
  return invokeHttp("steer_prep_hydration", { sessionName, blockOrdinal, instruction });
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

export async function processSession(
  name: string,
  projectId?: string | null,
  overwriteExistingNote = false,
  maxSpeakers?: number | null,
  forceTranscribe = false,
): Promise<void> {
  return invokeHttp("process_session", { name, projectId: projectId ?? null, overwriteExistingNote, maxSpeakers: maxSpeakers ?? null, forceTranscribe });
}

export async function retrySession(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("retry_session", { name, projectId: projectId ?? null });
}

export async function cancelProcessSession(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("cancel_process_session", { name, projectId: projectId ?? null });
}

export async function clearSessionNoteError(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("clear_session_note_error", { name, projectId: projectId ?? null });
}

export async function saveDraftNote(name: string, projectId?: string | null, overwriteExistingNote = false): Promise<string> {
  return invokeHttp("save_draft_note", { name, projectId: projectId ?? null, overwriteExistingNote });
}

export async function discardNote(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("discard_note", { name, projectId: projectId ?? null });
}

export async function refineSession(name: string, message: string, projectId?: string | null): Promise<void> {
  return invokeHttp("refine_session", { name, message, projectId: projectId ?? null });
}

export async function reprocessSessionWithPeople(name: string, people: string[], projectId?: string | null): Promise<void> {
  return invokeHttp("reprocess_session_with_people", { name, people, projectId: projectId ?? null });
}

export async function getAlignedContent(name: string, projectId?: string | null): Promise<string> {
  return invokeHttp("get_aligned_content", { name, projectId: projectId ?? null });
}

export async function getSessionMemo(name: string, projectId?: string | null): Promise<string> {
  return invokeHttp("get_session_memo", { name, projectId: projectId ?? null });
}

export async function getVaultNote(name: string, projectId?: string | null): Promise<string> {
  return invokeHttp("get_vault_note", { name, projectId: projectId ?? null });
}

export async function getSessionGrounding(name: string, projectId?: string | null): Promise<unknown | null> {
  return invokeHttp("get_session_grounding", { name, projectId: projectId ?? null });
}

export async function getDistillTrace(name: string, projectId?: string | null): Promise<ProcessingEvent[]> {
  return invokeHttp("get_distill_trace", { name, projectId: projectId ?? null });
}

export async function openNote(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("open_note", { name, projectId: projectId ?? null });
}

export async function openNoteInObsidian(name: string, projectId?: string | null): Promise<void> {
  return invokeHttp("open_note_in_obsidian", { name, projectId: projectId ?? null });
}

export async function openNoteTargetInObsidian(target: string, projectId?: string | null): Promise<void> {
  return invokeHttp("open_note_target_in_obsidian", { target, projectId: projectId ?? null });
}

// ---------------------------------------------------------------------------
// Events (http → WebSocket bridge)
// ---------------------------------------------------------------------------

export function onProcessingProgress(
  callback: (event: ProcessingEvent) => void,
): Promise<() => void> {
  return listenWhenOpen<ProcessingEvent>("processing-progress", callback);
}

export function onRecallIndexing(
  callback: (event: RecallIndexingEvent) => void,
): () => void {
  return listen<RecallIndexingEvent>("recall-indexing", callback);
}

export function onSpeechModelProgress(
  callback: (event: SpeechModelProgressEvent) => void,
): () => void {
  return listen<SpeechModelProgressEvent>("speech-model-progress", callback);
}

export function onBackchannelSuggestion(
  callback: (event: BackchannelSuggestionEvent) => void,
): () => void {
  return listen<BackchannelSuggestionEvent>("backchannel-suggestion", callback);
}

export function onBackchannelError(
  callback: (event: BackchannelSuggestionEvent) => void,
): () => void {
  return listen<BackchannelSuggestionEvent>("backchannel-error", callback);
}

export function onGranolaImportProgress(
  callback: (event: GranolaImportProgressEvent) => void,
): () => void {
  return listen<GranolaImportProgressEvent>("granola-import-progress", callback);
}

export function onGlobalCaptureToggle(callback: () => void): () => void {
  return listen<unknown>("global-capture-toggle", () => callback());
}

export function onCaptureDeviceChanged(callback: (state: CaptureDeviceEvent) => void): () => void {
  return listen<CaptureDeviceEvent>("capture-device-changed", callback);
}

export function onSettingsChanged(callback: () => void): () => void {
  return listen<unknown>("settings-changed", () => callback());
}

export function onPrepHydration(
  callback: (event: PrepHydrationEvent) => void,
): () => void {
  return listen<PrepHydrationEvent>("prep-hydration", callback);
}
