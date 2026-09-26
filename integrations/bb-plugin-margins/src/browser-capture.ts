import type { PluginContentScriptContext } from "@get-bb/plugin-sdk/app";
import { fetchJsonWithDeadline } from "../../../desktop/src/lib/bounded-fetch.js";
import { WebDurableUploadQueue, bindDurableMediaRecorder, stopMediaRecorderWithDeadline } from "../../../desktop/src/lib/web-durable-upload.js";
import { acquireWebMicrophone, selectWebRecorderMimeType, webMicrophoneSupported } from "../../../desktop/src/lib/web-microphone-permission.js";
import { CAPTURE_DISCONNECT_GRACE_MS, type ClientCapabilities, type HostError, type PanelState } from "./contracts.js";

const CLIENT_KEY = "margins.bb.client.v1";
const CAPTURE_KEY = "margins.bb.capture.v1";

interface StoredCapture {
  sessionId: string;
  startedAtMs: number;
  nextSequence: number;
  paused: boolean;
  pendingControl?: { kind: "pause" | "resume" | "stop"; operationId: string };
  stopDrainError?: HostError;
}

interface LocalCapture extends StoredCapture {
  stream: MediaStream;
  recorder: MediaRecorder;
  uploads: WebDurableUploadQueue;
  heartbeat: ReturnType<typeof setInterval>;
  reachability: ReachabilityDeadline;
  heartbeatInFlight: boolean;
  generation: number;
  uploadErrors: Error[];
  levelMonitor?: { dispose(): void };
}

type Subscriber = () => void;

export class ReachabilityDeadline {
  private lastAcknowledgedAt = Date.now();
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly graceMs: number,
    private readonly expired: () => void,
  ) {}

  start() {
    this.schedule();
  }

  acknowledged() {
    this.lastAcknowledgedAt = Date.now();
    this.schedule();
  }

  failed() {
    this.schedule();
  }

  private schedule() {
    if (this.timer) clearTimeout(this.timer);
    const remaining = Math.max(0, this.lastAcknowledgedAt + this.graceMs - Date.now());
    this.timer = setTimeout(() => {
      this.timer = null;
      this.expired();
    }, remaining);
  }

  dispose() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}

type BrowserMedia = {
  recorder: MediaRecorder;
  uploads: Pick<WebDurableUploadQueue, "close">;
  stream: Pick<MediaStream, "getTracks">;
};

/** Synchronously detach the local producer, returning only the bounded final
 * recorder event. The caller can update UI before awaiting any transport. */
export function releaseBrowserDevice(input: Pick<BrowserMedia, "recorder" | "stream">): Promise<Error | null> {
  const recorderStopped = stopMediaRecorderWithDeadline(input.recorder).catch((cause) => (
    cause instanceof Error ? cause : new Error(String(cause))
  ));
  input.stream.getTracks().forEach((track) => track.stop());
  return recorderStopped;
}

/** Drain final recorder data and queued uploads before authority finalization.
 * Both waits are bounded by their respective transport primitives. */
export async function drainBrowserMedia(
  input: Pick<BrowserMedia, "uploads">,
  recorderStopped: Promise<Error | null>,
): Promise<{ errors: Error[] }> {
  const errors: Error[] = [];
  const recorderError = await recorderStopped;
  if (recorderError) errors.push(recorderError);
  await input.uploads.close().catch((cause) => {
    errors.push(cause instanceof Error ? cause : new Error(String(cause)));
  });
  return { errors };
}

export function releaseBrowserMedia(input: BrowserMedia): Promise<{ errors: Error[] }> {
  return drainBrowserMedia(input, releaseBrowserDevice(input));
}

export function hostAcknowledgedCapture(state: PanelState) {
  return state.error === null && (state.state === "recording" || state.state === "paused");
}

export function applyRecorderTransition(
  capture: { recorder: Pick<MediaRecorder, "pause" | "resume">; paused: boolean },
  state: PanelState,
  target: "paused" | "recording",
) {
  if (state.error !== null || state.state !== target) return false;
  if (target === "paused") capture.recorder.pause();
  else capture.recorder.resume();
  capture.paused = target === "paused";
  return true;
}

function id() { return crypto.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`; }
function readStored(): StoredCapture | null {
  try {
    const value = JSON.parse(sessionStorage.getItem(CAPTURE_KEY) || "null") as (Partial<StoredCapture> & { recordingId?: string }) | null;
    const sessionId = value?.sessionId || value?.recordingId;
    return value && typeof sessionId === "string" && Number.isInteger(value.nextSequence)
      ? { ...value, sessionId, startedAtMs: typeof value.startedAtMs === "number" ? value.startedAtMs : Date.now(),
          paused: value.paused === true, nextSequence: value.nextSequence! } : null;
  }
  catch { return null; }
}
function writeStored(value: StoredCapture | null) {
  try {
    if (value) {
      const stored: StoredCapture = {
        sessionId: value.sessionId,
        startedAtMs: value.startedAtMs,
        nextSequence: value.nextSequence,
        paused: value.paused,
        ...(value.pendingControl ? { pendingControl: value.pendingControl } : {}),
        ...(value.stopDrainError ? { stopDrainError: value.stopDrainError } : {}),
      };
      sessionStorage.setItem(CAPTURE_KEY, JSON.stringify(stored));
    }
    else sessionStorage.removeItem(CAPTURE_KEY);
  } catch { /* a locked-down browser can still record for the current page */ }
}
function clientId() {
  try {
    const prior = sessionStorage.getItem(CLIENT_KEY);
    if (prior) return prior;
    const next = id(); sessionStorage.setItem(CLIENT_KEY, next); return next;
  } catch { return id(); }
}

export function detectClientCapabilities(): ClientCapabilities {
  const mobile = /Android|iPad|iPhone|iPod|Mobile/i.test(navigator.userAgent)
    || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  const mac = !mobile && /Macintosh|Mac OS X/i.test(navigator.userAgent);
  return {
    clientId: clientId(), platform: mobile ? "mobile" : mac ? "macos" : "other",
    secureContext: window.isSecureContext === true,
    browserMicrophone: window.isSecureContext === true && webMicrophoneSupported(),
    // The current bb SDK has no browser-local native bridge. This must stay
    // false until bb supplies an actual capability object for this window.
    nativeMacCapture: false,
  };
}

async function rpc<T>(pluginId: string, method: string, input: object): Promise<T> {
  const value = await fetchJsonWithDeadline<{ ok: boolean; result?: T; error?: { message?: string } }>(
    `/api/v1/plugins/${encodeURIComponent(pluginId)}/rpc/${method}`,
    {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
    },
  );
  if (!value.ok) throw new Error(value.error?.message || `Margins ${method} failed`);
  return value.result as T;
}

function base64(bytes: Uint8Array) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(bytes.length, offset + 0x8000)));
  }
  return btoa(binary);
}

export interface BrowserCaptureDependencies {
  rpc<T>(pluginId: string, method: string, input: object): Promise<T>;
  acquireMicrophone(): Promise<MediaStream>;
  createRecorder(stream: MediaStream, mime: string | null): MediaRecorder;
  supportsMime(candidate: string): boolean;
  heartbeatMs: number;
  disconnectGraceMs: number;
}

const defaultDependencies: BrowserCaptureDependencies = {
  rpc,
  acquireMicrophone: acquireWebMicrophone,
  createRecorder(stream, mime) {
    return mime ? new MediaRecorder(stream, { mimeType: mime }) : new MediaRecorder(stream);
  },
  supportsMime: candidate => MediaRecorder.isTypeSupported(candidate),
  heartbeatMs: 2_000,
  disconnectGraceMs: CAPTURE_DISCONNECT_GRACE_MS,
};

export class BrowserCaptureOwner {
  private pluginId: string | null = null;
  private capture: LocalCapture | null = null;
  private readonly subscribers = new Set<Subscriber>();
  private recoveryStarted = false;
  private heartbeatRecovering = false;
  private endedRecordingId: string | null = null;
  private generation = 0;
  private refreshGeneration = 0;
  private refreshInFlight: Promise<PanelState> | null = null;
  private readonly panels = new Map<string, PanelState>();
  private readonly routePanels = new Map<string, PanelState>();
  private latestPanel: PanelState | null = null;
  private inputLevel = 0;
  private listeningSince = 0;
  private heardAudio = false;
  private readonly drafts = new Map<string, string>();

  constructor(private readonly dependencies: BrowserCaptureDependencies = defaultDependencies) {}

  install(context: PluginContentScriptContext) {
    this.pluginId = context.pluginId;
    const stored = readStored();
    if (stored) void this.recover(stored);
    return () => this.disconnect();
  }

  subscribe(fn: Subscriber) { this.subscribers.add(fn); return () => this.subscribers.delete(fn); }
  private emit() { for (const fn of this.subscribers) fn(); }
  get active() { return this.capture !== null; }
  get recovering() { return this.heartbeatRecovering; }
  get level() { return this.inputLevel; }
  get noAudioWarning() {
    return Boolean(this.capture && !this.capture.paused && this.listeningSince
      && !this.heardAudio && Date.now() - this.listeningSince >= 3_000);
  }
  get hasPendingStop() { return readStored()?.pendingControl?.kind === "stop"; }
  get recordingId() { return this.capture?.sessionId ?? readStored()?.sessionId ?? this.endedRecordingId; }
  get elapsedMs() { return this.capture ? Date.now() - this.capture.startedAtMs : 0; }
  panel(threadId?: string) {
    if (threadId && this.routePanels.has(threadId)) return this.routePanels.get(threadId)!;
    const sessionId = this.recordingId;
    return sessionId ? this.panels.get(sessionId) ?? this.latestPanel : this.latestPanel;
  }
  acceptPanel(routeOrSessionId: string, state: PanelState) {
    this.routePanels.set(routeOrSessionId, state);
    const sessionId = state.recordingId || (routeOrSessionId === this.recordingId ? routeOrSessionId : null);
    if (sessionId) {
      this.panels.set(sessionId, state);
      for (const [route, panel] of this.routePanels) {
        if (panel.recordingId === sessionId) this.routePanels.set(route, state);
      }
    }
    if (!this.recordingId || sessionId === this.recordingId) this.latestPanel = state;
    this.emit();
    return state;
  }
  draft(recordingId: string) { return this.drafts.get(recordingId); }
  retainDraft(recordingId: string, text: string) { this.drafts.set(recordingId, text); }
  clearDraft(recordingId: string) { this.drafts.delete(recordingId); }

  /** One bounded refresh at a time. Generations prevent an old response from
   * replacing a newer control result or a newly selected session. */
  refresh(threadId: string, load: () => Promise<PanelState>): Promise<PanelState> {
    if (this.refreshInFlight) {
      return this.refreshInFlight.then(() => this.refresh(threadId, load));
    }
    const generation = ++this.refreshGeneration;
    const request = load().then(state => {
      if (generation === this.refreshGeneration) this.acceptPanel(threadId, state);
      return state;
    }).finally(() => {
      if (this.refreshInFlight === request) this.refreshInFlight = null;
    });
    this.refreshInFlight = request;
    return request;
  }

  private fenceRefreshes() { ++this.refreshGeneration; }

  private monitorLevel(stream: MediaStream): { dispose(): void } | undefined {
    if (typeof AudioContext === "undefined") return undefined;
    try {
      const context = new AudioContext();
      const source = context.createMediaStreamSource(stream);
      const analyser = context.createAnalyser();
      analyser.fftSize = 512;
      const silent = context.createGain();
      silent.gain.value = 0;
      source.connect(analyser);
      analyser.connect(silent);
      silent.connect(context.destination);
      const values = new Uint8Array(analyser.fftSize);
      const timer = setInterval(() => {
        analyser.getByteTimeDomainData(values);
        const sum = values.reduce((total, value) => total + ((value - 128) / 128) ** 2, 0);
        this.inputLevel = Math.min(1, Math.sqrt(sum / values.length) * 5);
        if (this.inputLevel > 0.015) this.heardAudio = true;
        this.emit();
      }, 90);
      return { dispose: () => { clearInterval(timer); this.inputLevel = 0; void context.close(); this.emit(); } };
    } catch { return undefined; }
  }

  private async createLocal(stored: StoredCapture, stream: MediaStream) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    const mime = selectWebRecorderMimeType(this.dependencies.supportsMime);
    const recorder = this.dependencies.createRecorder(stream, mime);
    const uploadErrors: Error[] = [];
    const uploads = new WebDurableUploadQueue(async (chunk, sequence, signal) => {
      const bytes = new Uint8Array(await chunk.arrayBuffer());
      const value = await fetchJsonWithDeadline<{ ok: boolean; error?: string | { message?: string } }>(
        `/api/v1/plugins/${encodeURIComponent(this.pluginId!)}/http/capture/chunk`,
        {
        method: "POST", signal, headers: { "content-type": "application/json" },
        body: JSON.stringify({
          sessionId: stored.sessionId, client: detectClientCapabilities(),
          sequence, bytesBase64: base64(bytes),
        }),
        },
      );
      if (!value.ok) throw new Error(typeof value.error === "string" ? value.error : value.error?.message || "Audio upload failed");
      const nextSequence = sequence + 1;
      stored.nextSequence = nextSequence;
      const current = this.capture;
      if (current?.sessionId === stored.sessionId) current.nextSequence = nextSequence;
      // A final upload can settle after local Stop has persisted its operation
      // identity. Advance only the sequence cursor; never replace that pending
      // control or its incomplete-drain evidence with the pre-Stop object.
      const persisted = readStored();
      writeStored(persisted?.sessionId === stored.sessionId
        ? { ...persisted, nextSequence }
        : stored);
    }, (error) => {
      uploadErrors.push(error);
      this.emit();
      const current = this.capture;
      if (current?.sessionId === stored.sessionId) void this.stopAfterDisconnect(current);
    }, { initialSequence: stored.nextSequence });
    bindDurableMediaRecorder(recorder, uploads);
    const generation = ++this.generation;
    const reachability = new ReachabilityDeadline(this.dependencies.disconnectGraceMs, () => {
      const current = this.capture;
      if (current?.sessionId === stored.sessionId) void this.stopAfterDisconnect(current);
    });
    const heartbeat = setInterval(() => {
      const current = this.capture;
      if (!this.pluginId || !current || current.generation !== generation || current.heartbeatInFlight) return;
      current.heartbeatInFlight = true;
      void this.dependencies.rpc<PanelState>(this.pluginId, "heartbeat", {
        sessionId: stored.sessionId, client: detectClientCapabilities(),
        operationId: id(),
      }).then((state) => {
        if (this.capture?.generation !== generation) return;
        if (!hostAcknowledgedCapture(state)) throw new Error(state.error?.message || "The project recording is not reachable");
        this.heartbeatRecovering = false;
        reachability.acknowledged();
        this.emit();
      }).catch(() => {
        if (this.capture?.generation !== generation) return;
        this.heartbeatRecovering = true;
        reachability.failed();
        this.emit();
      }).finally(() => {
        if (this.capture?.generation === generation) this.capture.heartbeatInFlight = false;
      });
    }, this.dependencies.heartbeatMs);
    recorder.start(3_000);
    if (stored.paused) recorder.pause();
    this.listeningSince = stored.paused ? 0 : Date.now();
    this.heardAudio = false;
    this.capture = { ...stored, stream, recorder, uploads, heartbeat, reachability, heartbeatInFlight: false, generation, uploadErrors,
      levelMonitor: this.monitorLevel(stream) };
    reachability.start();
    this.heartbeatRecovering = false;
    writeStored(stored);
    this.emit();
  }

  private async recover(stored: StoredCapture) {
    if (this.recoveryStarted || this.capture) return;
    this.recoveryStarted = true;
    try {
      if (stored.pendingControl?.kind === "stop" && this.pluginId) {
        await this.reconcileStop(stored);
      } else {
        await this.createLocal(stored, await this.dependencies.acquireMicrophone());
      }
    }
    catch { /* the server-side grace period safely finishes what was received */ }
    finally { this.recoveryStarted = false; }
  }

  async start(threadId: string, title?: string) {
    return this.begin("beginBrowserCapture", { threadId }, title);
  }

  async startFromProject(projectId: string, title?: string) {
    return this.begin("beginProjectCapture", { projectId }, title);
  }

  private async begin(method: "beginBrowserCapture" | "beginProjectCapture", target: { threadId: string } | { projectId: string }, title?: string) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    if (this.capture) throw new Error("This bb window is already recording");
    this.endedRecordingId = null;
    // Permission and recorder construction happen before a durable project
    // session is allocated, so denial cannot leave an empty meeting behind.
    const stream = await this.dependencies.acquireMicrophone();
    const mime = selectWebRecorderMimeType(this.dependencies.supportsMime);
    try { this.dependencies.createRecorder(stream, mime); }
    catch (cause) { stream.getTracks().forEach((track) => track.stop()); throw cause; }
    const ownerId = id();
    try {
      const state = await this.dependencies.rpc<PanelState>(this.pluginId, method, {
        ...target, client: detectClientCapabilities(), ownerId, ...(title ? { title } : {}),
      });
      if (!state.recordingId || !state.ownsRecording) throw new Error(state.detail);
      await this.createLocal({ sessionId: state.recordingId, startedAtMs: Date.now(), nextSequence: 0, paused: false }, stream);
      this.acceptPanel(state.recordingId, state);
      return state;
    } catch (cause) {
      stream.getTracks().forEach((track) => track.stop());
      throw cause;
    }
  }

  async pause() {
    const current = this.capture; if (!current || !this.pluginId) return;
    if (!current.paused) current.recorder.pause();
    current.paused = true;
    const pendingControl = current.pendingControl?.kind === "pause"
      ? current.pendingControl
      : { kind: "pause" as const, operationId: id() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.emit();
    this.fenceRefreshes();
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "pause", {
      sessionId: current.sessionId, client: detectClientCapabilities(),
      operationId: pendingControl.operationId,
    });
    if (this.capture === current && state.error === null && state.state === "paused") {
      current.pendingControl = undefined;
      writeStored(current);
      this.acceptPanel(current.sessionId, state);
    }
    return state;
  }
  async resume() {
    const current = this.capture; if (!current || !this.pluginId) return;
    if (current.paused) current.recorder.resume();
    current.paused = false;
    this.listeningSince = Date.now();
    this.heardAudio = false;
    const pendingControl = current.pendingControl?.kind === "resume"
      ? current.pendingControl
      : { kind: "resume" as const, operationId: id() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.emit();
    this.fenceRefreshes();
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "resume", {
      sessionId: current.sessionId, client: detectClientCapabilities(),
      operationId: pendingControl.operationId,
    });
    if (this.capture === current && state.error === null && state.state === "recording") {
      current.pendingControl = undefined;
      writeStored(current);
      this.acceptPanel(current.sessionId, state);
    }
    return state;
  }
  async stop() {
    const current = this.capture; if (!current || !this.pluginId) return;
    this.capture = null;
    ++this.generation;
    this.fenceRefreshes();
    this.heartbeatRecovering = false;
    current.levelMonitor?.dispose();
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    const pendingControl = current.pendingControl?.kind === "stop"
      ? current.pendingControl
      : { kind: "stop" as const, operationId: id() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.showStopping(current);
    const recorderStopped = releaseBrowserDevice(current);
    try {
      const drain = await drainBrowserMedia(current, recorderStopped);
      const errors = [...current.uploadErrors, ...drain.errors];
      if (errors.length > 0) {
        current.stopDrainError = {
          code: "browser_audio_drain_incomplete",
          message: errors[0]!.message,
          retryable: true,
        };
        writeStored(current);
      }
      // The host owns finalization, so it must not close chunk admission until
      // the browser's bounded final-data drain has settled.
      return await this.reconcileStop(current);
    } finally {
      this.emit();
    }
  }

  async retryPendingStop() {
    const stored = readStored();
    if (!stored || stored.pendingControl?.kind !== "stop") return;
    return this.reconcileStop(stored);
  }

  private async reconcileStop(stored: StoredCapture): Promise<PanelState> {
    if (!this.pluginId || stored.pendingControl?.kind !== "stop") throw new Error("No pending Stop to reconcile");
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "stop", {
      sessionId: stored.sessionId, client: detectClientCapabilities(),
      operationId: stored.pendingControl.operationId,
    });
    const latest = readStored();
    const recovery = latest?.sessionId === stored.sessionId
      && latest.pendingControl?.kind === "stop"
      && latest.pendingControl.operationId === stored.pendingControl.operationId
      ? {
          ...stored,
          nextSequence: Math.max(stored.nextSequence, latest.nextSequence),
          stopDrainError: stored.stopDrainError ?? latest.stopDrainError,
        }
      : stored;
    if (state.error === null && state.state === "saved" && !recovery.stopDrainError) {
      this.endedRecordingId = recovery.sessionId;
      writeStored(null);
      this.acceptPanel(recovery.sessionId, state);
      return state;
    }
    const error = recovery.stopDrainError ?? state.error ?? {
      code: "authority_stop_unconfirmed",
      message: "Margins did not confirm that the recording finished. Try again with the retained Stop operation.",
      retryable: true,
    };
    const incomplete: PanelState = {
      ...state,
      state: "needs_attention",
      title: "Recording needs attention",
      detail: error.message,
      primaryAction: error.retryable ? "retry" : "none",
      primaryLabel: error.retryable ? "Try again" : "Recording unavailable",
      canStop: false,
      canEditNotepad: false,
      ownsRecording: true,
      recordingId: recovery.sessionId,
      error,
    };
    writeStored(recovery);
    this.acceptPanel(recovery.sessionId, incomplete);
    return incomplete;
  }

  private showStopping(stored: StoredCapture) {
    const prior = this.panels.get(stored.sessionId);
    if (!prior) { this.emit(); return; }
    this.acceptPanel(stored.sessionId, {
      ...prior,
      state: "saving",
      title: "Saving",
      detail: "Capture has stopped. Margins is finishing the audio already received by this bb project.",
      primaryAction: "none",
      primaryLabel: "Saving",
      canStop: false,
      canEditNotepad: false,
      ownsRecording: true,
      recordingId: stored.sessionId,
      error: null,
    });
  }

  private async stopAfterDisconnect(current: LocalCapture) {
    if (this.capture !== current) return;
    this.capture = null;
    ++this.generation;
    this.heartbeatRecovering = true;
    this.endedRecordingId = current.sessionId;
    current.levelMonitor?.dispose();
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    current.pendingControl = current.pendingControl?.kind === "stop"
      ? current.pendingControl
      : { kind: "stop", operationId: id() };
    writeStored(current);
    this.emit();
    const drain = await drainBrowserMedia(current, releaseBrowserDevice(current));
    const errors = [...current.uploadErrors, ...drain.errors];
    if (errors.length > 0) {
      current.stopDrainError = {
        code: "browser_audio_drain_incomplete",
        message: errors[0]!.message,
        retryable: true,
      };
      writeStored(current);
    }
    // The default transport is bounded. A failed acknowledgement leaves the
    // exact Stop identity in session storage for explicit reconciliation.
    await this.reconcileStop(current).catch(() => this.emit());
  }

  private disconnect() {
    const current = this.capture;
    if (!current) return;
    this.capture = null;
    ++this.generation;
    this.heartbeatRecovering = true;
    current.levelMonitor?.dispose();
    clearInterval(current.heartbeat);
    current.reachability.dispose();
    void releaseBrowserMedia(current);
    this.emit();
    // Keep sessionStorage: a refresh can reacquire the microphone and continue
    // within the grace period. No explicit stop bypasses the server-side grace.
  }
}

export const browserCaptureOwner = new BrowserCaptureOwner();
