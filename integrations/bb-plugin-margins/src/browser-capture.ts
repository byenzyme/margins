import type { PluginContentScriptContext } from "@get-bb/plugin-sdk/app";
import { WebDurableUploadQueue, bindDurableMediaRecorder, stopMediaRecorderWithDeadline } from "../../../desktop/src/lib/web-durable-upload.js";
import { acquireWebMicrophone, selectWebRecorderMimeType, webMicrophoneSupported } from "../../../desktop/src/lib/web-microphone-permission.js";
import { CAPTURE_DISCONNECT_GRACE_MS, type ClientCapabilities, type PanelState } from "./contracts.js";

const CLIENT_KEY = "margins.bb.client.v1";
const CAPTURE_KEY = "margins.bb.capture.v1";

interface StoredCapture {
  threadId: string;
  recordingId: string;
  ownerId: string;
  nextSequence: number;
  paused: boolean;
}

interface LocalCapture extends StoredCapture {
  stream: MediaStream;
  recorder: MediaRecorder;
  uploads: WebDurableUploadQueue;
  heartbeat: ReturnType<typeof setInterval>;
  reachability: ReachabilityDeadline;
}

type Subscriber = () => void;

export class ReachabilityDeadline {
  private lastAcknowledgedAt = Date.now();
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly graceMs: number,
    private readonly expired: () => void,
  ) {}

  acknowledged() {
    this.lastAcknowledgedAt = Date.now();
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  failed() {
    if (this.timer) return;
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
  const response = await fetch(`/api/v1/plugins/${encodeURIComponent(pluginId)}/rpc/${method}`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
  });
  const value = await response.json() as { ok: boolean; result?: T; error?: { message?: string } };
  if (!response.ok || !value.ok) throw new Error(value.error?.message || `Margins ${method} failed`);
  return value.result as T;
}

function base64(bytes: Uint8Array) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(bytes.length, offset + 0x8000)));
  }
  return btoa(binary);
}

class BrowserCaptureOwner {
  private pluginId: string | null = null;
  private capture: LocalCapture | null = null;
  private readonly subscribers = new Set<Subscriber>();
  private recoveryStarted = false;
  private heartbeatRecovering = false;
  private endedRecordingId: string | null = null;

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

  private async createLocal(stored: StoredCapture, stream: MediaStream) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    const mime = selectWebRecorderMimeType((candidate) => MediaRecorder.isTypeSupported(candidate));
    const recorder = mime ? new MediaRecorder(stream, { mimeType: mime }) : new MediaRecorder(stream);
    const uploads = new WebDurableUploadQueue(async (chunk, sequence, signal) => {
      const bytes = new Uint8Array(await chunk.arrayBuffer());
      const response = await fetch(`/api/v1/plugins/${encodeURIComponent(this.pluginId!)}/http/capture/chunk`, {
        method: "POST", signal, headers: { "content-type": "application/json" },
        body: JSON.stringify({
          threadId: stored.threadId, client: detectClientCapabilities(), recordingId: stored.recordingId,
          sequence, bytesBase64: base64(bytes),
        }),
      });
      const value = await response.json() as { ok: boolean; error?: string | { message?: string } };
      if (!response.ok || !value.ok) throw new Error(typeof value.error === "string" ? value.error : value.error?.message || "Audio upload failed");
      stored.nextSequence = sequence + 1;
      writeStored(stored);
    }, () => this.emit(), { initialSequence: stored.nextSequence });
    bindDurableMediaRecorder(recorder, uploads);
    const reachability = new ReachabilityDeadline(CAPTURE_DISCONNECT_GRACE_MS, () => {
      const current = this.capture;
      if (current?.recordingId === stored.recordingId) void this.stopAfterDisconnect(current);
    });
    const heartbeat = setInterval(() => {
      if (!this.pluginId || !this.capture) return;
      void rpc<PanelState>(this.pluginId, "heartbeat", {
        threadId: stored.threadId, client: detectClientCapabilities(), recordingId: stored.recordingId,
      }).then((state) => {
        if (!hostAcknowledgedCapture(state)) throw new Error(state.error?.message || "The project recording is not reachable");
        this.heartbeatRecovering = false;
        reachability.acknowledged();
        this.emit();
      }).catch(() => {
        this.heartbeatRecovering = true;
        reachability.failed();
        this.emit();
      });
    }, 2_000);
    recorder.start(3_000);
    if (stored.paused) recorder.pause();
    this.capture = { ...stored, stream, recorder, uploads, heartbeat, reachability };
    this.heartbeatRecovering = false;
    writeStored(stored);
    this.emit();
  }

  private async recover(stored: StoredCapture) {
    if (this.recoveryStarted || this.capture) return;
    this.recoveryStarted = true;
    try { await this.createLocal(stored, await acquireWebMicrophone()); }
    catch { /* the server-side grace period safely finishes what was received */ }
    finally { this.recoveryStarted = false; }
  }

  async start(threadId: string, title?: string) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    if (this.capture) throw new Error("This bb window is already recording");
    this.endedRecordingId = null;
    // Permission and recorder construction happen before a durable project
    // session is allocated, so denial cannot leave an empty meeting behind.
    const stream = await acquireWebMicrophone();
    const mime = selectWebRecorderMimeType((candidate) => MediaRecorder.isTypeSupported(candidate));
    try { mime ? new MediaRecorder(stream, { mimeType: mime }) : new MediaRecorder(stream); }
    catch (cause) { stream.getTracks().forEach((track) => track.stop()); throw cause; }
    const ownerId = id();
    try {
      const state = await rpc<PanelState>(this.pluginId, "beginBrowserCapture", {
        threadId, client: detectClientCapabilities(), ownerId, ...(title ? { title } : {}),
      });
      if (!state.recordingId || !state.ownsRecording) throw new Error(state.detail);
      await this.createLocal({ threadId, recordingId: state.recordingId, ownerId, nextSequence: 0, paused: false }, stream);
      return state;
    } catch (cause) {
      stream.getTracks().forEach((track) => track.stop());
      throw cause;
    }
  }

  async pause() {
    const current = this.capture; if (!current || !this.pluginId) return;
    const state = await rpc<PanelState>(this.pluginId, "pause", { threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId });
    if (applyRecorderTransition(current, state, "paused")) writeStored(current);
    return state;
  }
  async resume() {
    const current = this.capture; if (!current || !this.pluginId) return;
    const state = await rpc<PanelState>(this.pluginId, "resume", { threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId });
    if (applyRecorderTransition(current, state, "recording")) writeStored(current);
    return state;
  }
  async stop() {
    const current = this.capture; if (!current || !this.pluginId) return;
    this.capture = null;
    this.heartbeatRecovering = false;
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    try {
      await releaseBrowserMedia(current);
      return await rpc<PanelState>(this.pluginId, "stop", { threadId: current.threadId, client: detectClientCapabilities(), recordingId: current.recordingId });
    } finally {
      writeStored(null);
      this.emit();
    }
  }

  private async stopAfterDisconnect(current: LocalCapture) {
    if (this.capture !== current) return;
    this.capture = null;
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
