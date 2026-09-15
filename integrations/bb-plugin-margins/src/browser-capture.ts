import type { PluginContentScriptContext } from "@get-bb/plugin-sdk/app";
import { fetchJsonWithDeadline } from "../../../desktop/src/lib/bounded-fetch.js";
import { WebDurableUploadQueue, bindDurableMediaRecorder, stopMediaRecorderWithDeadline } from "../../../desktop/src/lib/web-durable-upload.js";
import { acquireWebMicrophone, selectWebRecorderMimeType, webMicrophoneSupported } from "../../../desktop/src/lib/web-microphone-permission.js";
import { CAPTURE_DISCONNECT_GRACE_MS, type ClientCapabilities, type PanelState } from "./contracts.js";

const CLIENT_KEY = "margins.bb.client.v1";
const CAPTURE_KEY = "margins.bb.capture.v1";

interface StoredCapture {
  threadId: string;
  recordingId: string;
  nextSequence: number;
  paused: boolean;
  pendingControl?: { kind: "pause" | "resume" | "stop"; operationId: string };
}

interface LocalCapture extends StoredCapture {
  stream: MediaStream;
  recorder: MediaRecorder;
  uploads: WebDurableUploadQueue;
  heartbeat: ReturnType<typeof setInterval>;
  reachability: ReachabilityDeadline;
  heartbeatInFlight: boolean;
  generation: number;
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

export async function releaseBrowserMedia(input: {
  recorder: MediaRecorder;
  uploads: Pick<WebDurableUploadQueue, "close">;
  stream: Pick<MediaStream, "getTracks">;
}) {
  // Calling stop first gives MediaRecorder a chance to emit its final chunk.
  // Tracks are released immediately; neither the stop event nor a failed upload
  // is allowed to leave the browser microphone active.
  const recorderStopped = stopMediaRecorderWithDeadline(input.recorder).catch(() => null);
  input.stream.getTracks().forEach((track) => track.stop());
  await recorderStopped;
  await input.uploads.close().catch(() => undefined);
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
  try { return JSON.parse(sessionStorage.getItem(CAPTURE_KEY) || "null") as StoredCapture | null; }
  catch { return null; }
}
function writeStored(value: StoredCapture | null) {
  try {
    if (value) sessionStorage.setItem(CAPTURE_KEY, JSON.stringify(value));
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
  private readonly drafts = new Map<string, string>();

  constructor(private readonly dependencies: BrowserCaptureDependencies = defaultDependencies) {}

  install(context: PluginContentScriptContext) {
    this.pluginId = context.pluginId;
    const stored = readStored();
    if (stored && detectClientCapabilities().platform !== "macos") void this.recover(stored);
    return () => this.disconnect();
  }

  subscribe(fn: Subscriber) { this.subscribers.add(fn); return () => this.subscribers.delete(fn); }
  private emit() { for (const fn of this.subscribers) fn(); }
  get active() { return this.capture !== null; }
  get recovering() { return this.heartbeatRecovering; }
  get recordingId() { return this.capture?.recordingId ?? readStored()?.recordingId ?? this.endedRecordingId; }
  panel(threadId: string) { return this.panels.get(threadId) ?? null; }
  acceptPanel(threadId: string, state: PanelState) {
    this.panels.set(threadId, state);
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

  private async createLocal(stored: StoredCapture, stream: MediaStream) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    const mime = selectWebRecorderMimeType(this.dependencies.supportsMime);
    const recorder = this.dependencies.createRecorder(stream, mime);
    const uploads = new WebDurableUploadQueue(async (chunk, sequence, signal) => {
      const bytes = new Uint8Array(await chunk.arrayBuffer());
      const value = await fetchJsonWithDeadline<{ ok: boolean; error?: string | { message?: string } }>(
        `/api/v1/plugins/${encodeURIComponent(this.pluginId!)}/http/capture/chunk`,
        {
        method: "POST", signal, headers: { "content-type": "application/json" },
        body: JSON.stringify({
          threadId: stored.threadId, client: detectClientCapabilities(), recordingId: stored.recordingId,
          sequence, bytesBase64: base64(bytes),
        }),
        },
      );
      if (!value.ok) throw new Error(typeof value.error === "string" ? value.error : value.error?.message || "Audio upload failed");
      stored.nextSequence = sequence + 1;
      writeStored(stored);
    }, () => {
      this.emit();
      const current = this.capture;
      if (current?.recordingId === stored.recordingId) void this.stopAfterDisconnect(current);
    }, { initialSequence: stored.nextSequence });
    bindDurableMediaRecorder(recorder, uploads);
    const generation = ++this.generation;
    const reachability = new ReachabilityDeadline(this.dependencies.disconnectGraceMs, () => {
      const current = this.capture;
      if (current?.recordingId === stored.recordingId) void this.stopAfterDisconnect(current);
    });
    const heartbeat = setInterval(() => {
      const current = this.capture;
      if (!this.pluginId || !current || current.generation !== generation || current.heartbeatInFlight) return;
      current.heartbeatInFlight = true;
      void this.dependencies.rpc<PanelState>(this.pluginId, "heartbeat", {
        threadId: stored.threadId, client: detectClientCapabilities(), recordingId: stored.recordingId,
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
    this.capture = { ...stored, stream, recorder, uploads, heartbeat, reachability, heartbeatInFlight: false, generation };
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
        await this.dependencies.rpc<PanelState>(this.pluginId, "stop", {
          threadId: stored.threadId, client: detectClientCapabilities(), recordingId: stored.recordingId,
          operationId: stored.pendingControl.operationId,
        });
        writeStored(null);
      } else {
        await this.createLocal(stored, await this.dependencies.acquireMicrophone());
      }
    }
    catch { /* the server-side grace period safely finishes what was received */ }
    finally { this.recoveryStarted = false; }
  }

  async start(threadId: string, title?: string) {
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
      const state = await this.dependencies.rpc<PanelState>(this.pluginId, "beginBrowserCapture", {
        threadId, client: detectClientCapabilities(), ownerId, ...(title ? { title } : {}),
      });
      if (!state.recordingId || !state.ownsRecording) throw new Error(state.detail);
      await this.createLocal({ threadId, recordingId: state.recordingId, nextSequence: 0, paused: false }, stream);
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
      threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId,
      operationId: pendingControl.operationId,
    });
    if (this.capture === current && state.error === null && state.state === "paused") {
      current.pendingControl = undefined;
      writeStored(current);
    }
    return state;
  }
  async resume() {
    const current = this.capture; if (!current || !this.pluginId) return;
    if (current.paused) current.recorder.resume();
    current.paused = false;
    const pendingControl = current.pendingControl?.kind === "resume"
      ? current.pendingControl
      : { kind: "resume" as const, operationId: id() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.emit();
    this.fenceRefreshes();
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "resume", {
      threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId,
      operationId: pendingControl.operationId,
    });
    if (this.capture === current && state.error === null && state.state === "recording") {
      current.pendingControl = undefined;
      writeStored(current);
    }
    return state;
  }
  async stop() {
    const current = this.capture; if (!current || !this.pluginId) return;
    this.capture = null;
    ++this.generation;
    this.fenceRefreshes();
    this.heartbeatRecovering = false;
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    const pendingControl = current.pendingControl?.kind === "stop"
      ? current.pendingControl
      : { kind: "stop" as const, operationId: id() };
    current.pendingControl = pendingControl;
    writeStored(current);
    const release = releaseBrowserMedia(current);
    const command = this.dependencies.rpc<PanelState>(this.pluginId, "stop", {
      threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId,
      operationId: pendingControl.operationId,
    });
    try {
      const [, state] = await Promise.all([release, command]);
      writeStored(null);
      return state;
    } finally {
      this.emit();
    }
  }

  private async stopAfterDisconnect(current: LocalCapture) {
    if (this.capture !== current) return;
    this.capture = null;
    ++this.generation;
    this.heartbeatRecovering = true;
    this.endedRecordingId = current.recordingId;
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    writeStored(null);
    this.emit();
    await releaseBrowserMedia(current);
    // The project-side lease independently stops and saves the session. Do not
    // turn an unreachable stop request into a second, unbounded retry loop here.
  }

  private disconnect() {
    const current = this.capture;
    if (!current) return;
    this.capture = null;
    ++this.generation;
    this.heartbeatRecovering = true;
    clearInterval(current.heartbeat);
    current.reachability.dispose();
    void releaseBrowserMedia(current);
    this.emit();
    // Keep sessionStorage: a refresh can reacquire the microphone and continue
    // within the grace period. No explicit stop bypasses the server-side grace.
  }
}

export const browserCaptureOwner = new BrowserCaptureOwner();
