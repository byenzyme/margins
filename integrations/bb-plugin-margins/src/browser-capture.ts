import type { PluginContentScriptContext } from "@get-bb/plugin-sdk/app";
import { fetchJsonWithDeadline } from "../../../desktop/src/lib/bounded-fetch.js";
import { IrrecoverableAudioLossError, WebDurableUploadQueue, bindDurableMediaRecorder, stopMediaRecorderWithDeadline, type DurableChunkUpload, type WebChunkTiming, type WebDurableUploadOptions } from "../../../desktop/src/lib/web-durable-upload.js";
import { acquireWebMicrophone, selectWebRecorderMimeType, webMicrophoneSupported } from "../../../desktop/src/lib/web-microphone-permission.js";
import { BrowserChunkStore } from "./browser-chunk-store.js";
import { CAPTURE_DISCONNECT_GRACE_MS, type ClientCapabilities, type HostError, type PanelState } from "./contracts.js";

const CLIENT_KEY = "margins.bb.client.v1";
const CAPTURE_KEY = "margins.bb.capture.v1";

interface StoredCapture {
  sessionId: string;
  projectId?: string;
  startedAtMs: number;
  nextSequence: number;
  expectedNextSequence?: number;
  paused: boolean;
  pendingControl?: { kind: "pause" | "resume" | "stop"; operationId: string; segmentTimeUnixMs?: number };
  stopDrainError?: HostError;
  stopGapError?: HostError;
  stopGapSequence?: number;
  finishIncompleteRequested?: boolean;
}

interface LocalCapture extends StoredCapture {
  stream: MediaStream;
  recorder: MediaRecorder;
  recorderStopped: boolean;
  recorderStop: Promise<Error | null> | null;
  recorderMime: string | null;
  uploads: WebDurableUploadQueue;
  heartbeat: ReturnType<typeof setInterval>;
  reachability: ReachabilityDeadline;
  heartbeatInFlight: boolean;
  generation: number;
  uploadErrors: Error[];
  levelMonitor?: { dispose(): void };
}

type Subscriber = () => void;

/** Copies each admitted chunk into reload-durable storage at the moment its
 * sequence is assigned. Chunks the queue refuses (backpressure) are not
 * persisted: their sequence is already counted as lost. */
class PersistentUploadQueue extends WebDurableUploadQueue {
  constructor(
    upload: DurableChunkUpload,
    onFailure: (error: Error) => void,
    options: WebDurableUploadOptions,
    private readonly persist: (sequence: number, chunk: Blob, timing: WebChunkTiming) => void,
  ) {
    super(upload, onFailure, options);
  }

  override enqueue(chunk: Blob, timing?: WebChunkTiming): void {
    const sequence = this.expectedNextSequence;
    const queued = this.pendingCount;
    super.enqueue(chunk, timing);
    // enqueue() is synchronous and the worker cannot shift before its first
    // await, so growth means this exact Blob was admitted at `sequence`.
    if (timing && this.pendingCount > queued) this.persist(sequence, chunk, timing);
  }
}

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
function releaseBrowserDevice(input: Pick<BrowserMedia, "recorder" | "stream">): Promise<Error | null> {
  const recorderStopped = stopMediaRecorderWithDeadline(input.recorder).catch((cause) => (
    cause instanceof Error ? cause : new Error(String(cause))
  ));
  input.stream.getTracks().forEach((track) => track.stop());
  return recorderStopped;
}

/** Drain final recorder data and queued uploads before authority finalization.
 * Both waits are bounded by their respective transport primitives. */
async function drainBrowserMedia(
  input: Pick<BrowserMedia, "uploads">,
  recorderStopped: Promise<Error | null>,
): Promise<{ errors: Error[]; recorderError: Error | null }> {
  const errors: Error[] = [];
  const recorderError = await recorderStopped;
  if (recorderError) errors.push(recorderError);
  await input.uploads.close().catch((cause) => {
    errors.push(cause instanceof Error ? cause : new Error(String(cause)));
  });
  return { errors, recorderError };
}

export function releaseBrowserMedia(input: BrowserMedia): Promise<{ errors: Error[] }> {
  return drainBrowserMedia(input, releaseBrowserDevice(input));
}

export function hostAcknowledgedCapture(state: PanelState) {
  return state.error === null && (state.state === "recording" || state.state === "paused");
}

function id() { return crypto.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`; }
function secureOwnerId() {
  if (!crypto.randomUUID) throw new Error("Secure browser capture identity is unavailable");
  return crypto.randomUUID();
}
function readStored(): StoredCapture | null {
  try {
    const value = JSON.parse(sessionStorage.getItem(CAPTURE_KEY) || "null") as (Partial<StoredCapture> & { recordingId?: string }) | null;
    const sessionId = value?.sessionId || value?.recordingId;
    return value && typeof sessionId === "string" && Number.isInteger(value.nextSequence)
      ? { ...value, sessionId, startedAtMs: typeof value.startedAtMs === "number" ? value.startedAtMs : Date.now(),
          paused: value.paused === true, nextSequence: value.nextSequence!,
          expectedNextSequence: Number.isInteger(value.expectedNextSequence) ? value.expectedNextSequence : value.nextSequence! } : null;
  }
  catch { return null; }
}
function writeStored(value: StoredCapture | null) {
  try {
    if (value) {
      const stored: StoredCapture = {
        sessionId: value.sessionId,
        ...(value.projectId ? { projectId: value.projectId } : {}),
        startedAtMs: value.startedAtMs,
        nextSequence: value.nextSequence,
        ...(value.expectedNextSequence !== undefined ? { expectedNextSequence: value.expectedNextSequence } : {}),
        paused: value.paused,
        ...(value.pendingControl ? { pendingControl: value.pendingControl } : {}),
        ...(value.stopDrainError ? { stopDrainError: value.stopDrainError } : {}),
        ...(value.stopGapError ? { stopGapError: value.stopGapError } : {}),
        ...(value.stopGapSequence !== undefined ? { stopGapSequence: value.stopGapSequence } : {}),
        ...(value.finishIncompleteRequested ? { finishIncompleteRequested: true } : {}),
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

function missingBrowserAudioSequence(error: HostError): number | null {
  if (error.code !== "browser_chunk_gap") return null;
  const match = /^Missing browser audio sequence (\d+); retry the chunk before (?:Stop|Pause)\.$/.exec(error.message);
  const sequence = Number(match?.[1]);
  return match && Number.isSafeInteger(sequence) ? sequence : null;
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
  // A cold project recorder may initialize the ASR model during the first
  // capture request. Keep the ordinary control calls on their short deadline.
  const timeoutMs = method === "beginProjectCapture" ? 30_000 : 10_000;
  const value = await fetchJsonWithDeadline<{ ok: boolean; result?: T; error?: { message?: string } }>(
    `/api/v1/plugins/${encodeURIComponent(pluginId)}/rpc/${method}`,
    {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(input),
    },
    { timeoutMs },
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
  /** Reload-durable retention for unacknowledged chunks. Absent or null keeps
   * memory-only retention. */
  chunkStore?: BrowserChunkStore | null;
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
  chunkStore: new BrowserChunkStore(),
};

export class BrowserCaptureOwner {
  private pluginId: string | null = null;
  private capture: LocalCapture | null = null;
  private pendingCapture: LocalCapture | null = null;
  private readonly subscribers = new Set<Subscriber>();
  private recoveryStarted = false;
  private heartbeatRecovering = false;
  private endedRecordingId: string | null = null;
  private generation = 0;
  private readonly panels = new Map<string, PanelState>();
  private readonly routePanels = new Map<string, PanelState>();
  private latestPanel: PanelState | null = null;
  private inputLevel = 0;
  private listeningSince = 0;
  private heardAudio = false;

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
  get canFinishIncomplete() {
    const stored = readStored();
    return stored?.pendingControl?.kind === "stop"
      && (stored.finishIncompleteRequested || stored.stopDrainError?.retryable === false
        || stored.stopGapError?.retryable === false);
  }
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

  private async createLocal(stored: StoredCapture, stream: MediaStream, segmentStartedUnixMs = Date.now()) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    const mime = selectWebRecorderMimeType(this.dependencies.supportsMime);
    const recorder = this.dependencies.createRecorder(stream, mime);
    const uploadErrors: Error[] = [];
    const chunkStore = this.dependencies.chunkStore;
    const uploads = new PersistentUploadQueue(async (chunk, sequence, signal, timing) => {
      if (!timing) throw new Error("Browser audio capture timestamps are missing");
      await this.postChunk(stored.sessionId, new Uint8Array(await chunk.arrayBuffer()), sequence, timing, signal);
      void chunkStore?.acknowledge(stored.sessionId, sequence);
      const nextSequence = Math.max(stored.nextSequence, sequence + 1);
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
      if (current?.sessionId === stored.sessionId && !current.paused) void this.stopAfterDisconnect(current);
    }, { initialSequence: stored.nextSequence, onSequenceAssigned: (nextSequence) => {
      stored.expectedNextSequence = nextSequence;
      const current = this.capture ?? this.pendingCapture;
      if (current?.sessionId === stored.sessionId) current.expectedNextSequence = nextSequence;
      const persisted = readStored();
      writeStored(persisted?.sessionId === stored.sessionId
        ? { ...persisted, expectedNextSequence: nextSequence }
        : stored);
    } }, (sequence, chunk, timing) => {
      void chunkStore?.persist(stored.sessionId, sequence, chunk, timing);
    });
    bindDurableMediaRecorder(recorder, uploads, segmentStartedUnixMs);
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
    if (!stored.paused) recorder.start(3_000);
    this.listeningSince = stored.paused ? 0 : Date.now();
    this.heardAudio = false;
    this.capture = { ...stored, stream, recorder, recorderStopped: stored.paused, recorderStop: null,
      recorderMime: mime, uploads, heartbeat, reachability, heartbeatInFlight: false, generation, uploadErrors,
      levelMonitor: this.monitorLevel(stream) };
    reachability.start();
    this.heartbeatRecovering = false;
    writeStored(stored);
    this.emit();
  }

  private async postChunk(sessionId: string, bytes: Uint8Array, sequence: number, timing: WebChunkTiming, signal?: AbortSignal) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    const value = await fetchJsonWithDeadline<{ ok: boolean; error?: string | { message?: string } }>(
      `/api/v1/plugins/${encodeURIComponent(this.pluginId)}/http/capture/chunk`,
      {
      method: "POST", signal, headers: { "content-type": "application/json" },
      body: JSON.stringify({
        sessionId, client: detectClientCapabilities(),
        sequence, bytesBase64: base64(bytes),
        capturedStartUnixMs: timing.capturedStartUnixMs,
        capturedEndUnixMs: timing.capturedEndUnixMs,
      }),
      },
    );
    if (!value.ok) throw new Error(typeof value.error === "string" ? value.error : value.error?.message || "Audio upload failed");
  }

  /** After a reload the in-memory queue is gone, but chunks persisted before
   * their acknowledgement can still be delivered. Identical bytes for an
   * already-durable sequence are accepted idempotently by the server. Stops at
   * the first failure so later bytes stay retained for another attempt; the
   * server's sequence fence decides what is actually missing. */
  private async flushPersisted(sessionId: string): Promise<{ found: Set<number>; delivered: Set<number> }> {
    const found = new Set<number>();
    const delivered = new Set<number>();
    const store = this.dependencies.chunkStore;
    if (!store) return { found, delivered };
    const chunks = await store.pending(sessionId);
    for (const chunk of chunks) found.add(chunk.sequence);
    for (const chunk of chunks) {
      if (!await this.uploadPersisted(chunk.sessionId, chunk.sequence, chunk)) break;
      delivered.add(chunk.sequence);
    }
    return { found, delivered };
  }

  private async uploadPersisted(sessionId: string, sequence: number, chunk: { bytes: ArrayBuffer; timing: WebChunkTiming }) {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        await this.postChunk(sessionId, new Uint8Array(chunk.bytes), sequence, chunk.timing);
        await this.dependencies.chunkStore?.acknowledge(sessionId, sequence);
        return true;
      } catch { /* retry once; failure leaves the bytes retained */ }
    }
    return false;
  }

  /** Replay one server-reported missing sequence from memory or, after a
   * reload, from persisted storage. */
  private async resendGap(sessionId: string, sequence: number, pending: LocalCapture | null) {
    if (pending?.uploads.canReplay(sequence)) return pending.uploads.resend(sequence);
    const persisted = (await this.dependencies.chunkStore?.pending(sessionId))?.find((chunk) => chunk.sequence === sequence);
    if (!persisted) {
      if (pending) return pending.uploads.resend(sequence);
      throw new IrrecoverableAudioLossError("The missing chunk is unavailable in this browser; recording is incomplete.");
    }
    if (!await this.uploadPersisted(sessionId, sequence, persisted)) {
      throw new Error(`Browser audio sequence ${sequence} could not be retried`);
    }
  }

  private async hasPersisted(sessionId: string, sequence: number) {
    const store = this.dependencies.chunkStore;
    if (!store) return false;
    return (await store.pending(sessionId)).some((chunk) => chunk.sequence === sequence);
  }

  /** A finalized recording no longer needs any retained bytes. */
  private forgetPersisted(sessionId: string) {
    void this.dependencies.chunkStore?.forgetSession(sessionId);
  }

  /** MediaRecorder emits a complete, independently decodable WebM stream only
   * after stop. Keep the stop promise so Pause and Stop cannot stop it twice. */
  private stopSegment(current: LocalCapture): Promise<Error | null> {
    if (current.recorderStopped) return current.recorderStop ?? Promise.resolve(null);
    current.recorderStopped = true;
    current.recorderStop = stopMediaRecorderWithDeadline(current.recorder);
    return current.recorderStop;
  }

  private settledWhileAway(current: LocalCapture, state: PanelState): PanelState {
    this.capture = null;
    ++this.generation;
    this.heartbeatRecovering = false;
    current.levelMonitor?.dispose();
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    void this.stopSegment(current);
    current.stream.getTracks().forEach((track) => track.stop());
    void current.uploads.close().catch(() => undefined);
    current.uploads.releaseRetained();
    this.forgetPersisted(current.sessionId);
    this.endedRecordingId = current.sessionId;
    writeStored(null);
    this.acceptPanel(current.sessionId, state);
    return state;
  }

  private async recover(stored: StoredCapture) {
    if (this.recoveryStarted || this.capture) return;
    this.recoveryStarted = true;
    try {
      if (stored.pendingControl?.kind === "stop" && this.pluginId) {
        if (stored.finishIncompleteRequested) await this.finishIncomplete();
        else await this.retryPendingStop();
      } else {
        if (!this.pluginId) return;
        const client = detectClientCapabilities();
        const snapshot = await this.dependencies.rpc<PanelState>(this.pluginId, "readCapture", {
          sessionId: stored.sessionId, client, operationId: id(),
        });
        if (snapshot.state === "saving" || snapshot.state === "saved") {
          this.forgetPersisted(stored.sessionId);
          this.endedRecordingId = stored.sessionId;
          writeStored(null);
          this.acceptPanel(stored.sessionId, snapshot);
          return;
        }
        if (snapshot.error || !["recording", "paused"].includes(snapshot.state)
          || !Number.isSafeInteger(snapshot.nextSequence)) {
          throw new Error(snapshot.error?.message || "Browser capture recovery snapshot is unavailable");
        }
        const serverNextSequence = snapshot.nextSequence!;
        const localExpected = stored.expectedNextSequence ?? stored.nextSequence;
        const resumedSequence = Math.max(localExpected, serverNextSequence);
        stored.nextSequence = resumedSequence;
        stored.expectedNextSequence = resumedSequence;
        const shouldRecord = stored.pendingControl?.kind === "resume" || !stored.paused;
        const stream = await this.dependencies.acquireMicrophone();
        try {
          // Deliver bytes retained across the reload into the segment they
          // belong to before rotating it; only what is still missing after
          // this becomes a declared gap.
          await this.flushPersisted(stored.sessionId);
          if (snapshot.state === "recording") {
            const paused = await this.dependencies.rpc<PanelState>(this.pluginId, "pause", {
              sessionId: stored.sessionId, client, operationId: id(),
              expectedNextSequence: resumedSequence, segmentEndedUnixMs: Date.now(), recoveredAfterReload: true,
            });
            if (paused.error || paused.state !== "paused") {
              throw new Error(paused.error?.message || "Could not rotate the browser audio segment after reload");
            }
          }
          let segmentStartedUnixMs = Date.now();
          if (shouldRecord) {
            segmentStartedUnixMs = Date.now();
            const resumed = await this.dependencies.rpc<PanelState>(this.pluginId, "resume", {
              sessionId: stored.sessionId, client, operationId: id(), segmentStartedUnixMs,
            });
            if (resumed.error || resumed.state !== "recording") {
              throw new Error(resumed.error?.message || "Could not resume the browser audio segment after reload");
            }
          }
          stored.paused = !shouldRecord;
          stored.pendingControl = undefined;
          writeStored(stored);
          await this.createLocal(stored, stream, segmentStartedUnixMs);
          this.acceptPanel(stored.sessionId, snapshot.state === "paused" && !shouldRecord ? snapshot
            : { ...snapshot, state: shouldRecord ? "recording" : "paused",
              primaryAction: shouldRecord ? "pause" : "resume", primaryLabel: shouldRecord ? "Pause" : "Resume" });
        } catch (cause) {
          stream.getTracks().forEach((track) => track.stop());
          throw cause;
        }
      }
    }
    catch { /* the server-side owner lease finishes abandoned audio as incomplete */ }
    finally { this.recoveryStarted = false; }
  }

  async startFromProject(projectId: string, title?: string) {
    return this.begin(projectId, title);
  }

  private async begin(projectId: string, title?: string) {
    if (!this.pluginId) throw new Error("Margins is still loading");
    if (this.capture) throw new Error("This bb window is already recording");
    this.endedRecordingId = null;
    // Permission and recorder construction happen before a durable project
    // session is allocated, so denial cannot leave an empty meeting behind.
    const stream = await this.dependencies.acquireMicrophone();
    const mime = selectWebRecorderMimeType(this.dependencies.supportsMime);
    try { this.dependencies.createRecorder(stream, mime); }
    catch (cause) { stream.getTracks().forEach((track) => track.stop()); throw cause; }
    const ownerId = secureOwnerId();
    const startedAtMs = Date.now();
    try {
      const state = await this.dependencies.rpc<PanelState>(this.pluginId, "beginProjectCapture", {
        projectId, client: detectClientCapabilities(), ownerId, startedAtUnixMs: startedAtMs,
        ...(title ? { title } : {}),
      });
      if (!state.recordingId || !state.ownsRecording) throw new Error(state.detail);
      await this.createLocal({ sessionId: state.recordingId, projectId, startedAtMs,
        nextSequence: state.nextSequence ?? 0, expectedNextSequence: state.nextSequence ?? 0, paused: false }, stream, startedAtMs);
      this.acceptPanel(state.recordingId, state);
      return state;
    } catch (cause) {
      stream.getTracks().forEach((track) => track.stop());
      throw cause;
    }
  }

  async pause() {
    const current = this.capture; if (!current || !this.pluginId) return;
    if (current.paused && !current.pendingControl) return this.panels.get(current.sessionId) ?? this.latestPanel ?? undefined;
    current.paused = true;
    const pendingControl = current.pendingControl?.kind === "pause"
      ? current.pendingControl
      : { kind: "pause" as const, operationId: id(), segmentTimeUnixMs: Date.now() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.emit();
    const recorderError = await this.stopSegment(current);
    if (this.capture !== current) return;
    if (recorderError) return this.incompleteControlPanel(current, recorderError, "pause");
    try {
      await current.uploads.retryPending();
    } catch (cause) {
      return this.incompleteControlPanel(current, cause instanceof Error ? cause : new Error(String(cause)), "pause");
    }
    if (this.capture !== current) return;
    current.expectedNextSequence = current.uploads.expectedNextSequence;
    writeStored(current);
    const replayed = new Set<number>();
    try {
      while (this.capture === current) {
        const state = await this.dependencies.rpc<PanelState>(this.pluginId, "pause", {
          sessionId: current.sessionId, client: detectClientCapabilities(),
          operationId: pendingControl.operationId,
          expectedNextSequence: current.expectedNextSequence,
          segmentEndedUnixMs: pendingControl.segmentTimeUnixMs,
        });
        if (this.capture !== current) return state;
        if (state.error === null && state.state === "paused") {
          current.pendingControl = undefined;
          writeStored(current);
          this.acceptPanel(current.sessionId, state);
          return state;
        }
        if (state.state === "saved" && state.lastSessionId === current.sessionId) {
          return this.settledWhileAway(current, state);
        }
        const gap = state.error ? missingBrowserAudioSequence(state.error) : null;
        if (gap === null || replayed.has(gap)) {
          return this.incompleteControlPanel(current, new Error(state.error?.message || "Pause was not acknowledged"), "pause");
        }
        replayed.add(gap);
        await current.uploads.resend(gap);
      }
    } catch (cause) {
      if (this.capture !== current) return;
      return this.incompleteControlPanel(current, cause instanceof Error ? cause : new Error(String(cause)), "pause");
    }
    return;
  }
  async resume() {
    const current = this.capture; if (!current || !this.pluginId) return;
    if (!current.paused) return this.panels.get(current.sessionId) ?? this.latestPanel ?? undefined;
    if (current.pendingControl?.kind === "pause") {
      const paused = await this.pause();
      if (!paused || paused.error !== null || paused.state !== "paused") return paused;
    }
    const pendingControl = current.pendingControl?.kind === "resume"
      ? current.pendingControl
      : { kind: "resume" as const, operationId: id(), segmentTimeUnixMs: Date.now() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.emit();
    try {
      const state = await this.dependencies.rpc<PanelState>(this.pluginId, "resume", {
        sessionId: current.sessionId, client: detectClientCapabilities(),
        operationId: pendingControl.operationId,
        segmentStartedUnixMs: pendingControl.segmentTimeUnixMs,
      });
      if (this.capture !== current) return state;
      if (state.state === "saved" && state.lastSessionId === current.sessionId) {
        return this.settledWhileAway(current, state);
      }
      if (state.error !== null || state.state !== "recording") {
        return this.incompleteControlPanel(current, new Error(state.error?.message || "Resume was not acknowledged"), "resume");
      }
      const recorder = this.dependencies.createRecorder(current.stream, current.recorderMime);
      bindDurableMediaRecorder(recorder, current.uploads, pendingControl.segmentTimeUnixMs);
      recorder.start(3_000);
      current.recorder = recorder;
      current.recorderStopped = false;
      current.recorderStop = null;
      current.paused = false;
      this.listeningSince = Date.now();
      this.heardAudio = false;
      current.pendingControl = undefined;
      writeStored(current);
      this.acceptPanel(current.sessionId, state);
      return state;
    } catch (cause) {
      if (this.capture !== current) return;
      return this.incompleteControlPanel(current, cause instanceof Error ? cause : new Error(String(cause)), "resume");
    }
  }

  private incompleteControlPanel(current: LocalCapture, cause: Error, action: "pause" | "resume"): PanelState {
    const error: HostError = { code: "browser_audio_drain_incomplete", message: cause.message, retryable: true };
    const prior = this.panels.get(current.sessionId) ?? this.latestPanel;
    const panel: PanelState = {
      ...(prior ?? {
        schema: "margins.bb.recording.panel.v2", sourceLabel: null, storageLabel: null,
        sessionId: current.sessionId, notepad: null, lastSessionId: null,
      }),
      state: "needs_attention", title: "Recording incomplete", detail: error.message,
      primaryAction: action, primaryLabel: action === "pause" ? "Retry pause" : "Retry recording", canStop: true,
      canEditNotepad: false, ownsRecording: true, recordingId: current.sessionId, error,
    };
    this.acceptPanel(current.sessionId, panel);
    return panel;
  }
  async stop() {
    const current = this.capture; if (!current || !this.pluginId) return;
    this.capture = null;
    this.pendingCapture = current;
    ++this.generation;
    this.heartbeatRecovering = false;
    current.levelMonitor?.dispose();
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    const pendingControl = current.pendingControl?.kind === "stop"
      ? current.pendingControl
      : { kind: "stop" as const, operationId: id(), segmentTimeUnixMs: Date.now() };
    current.pendingControl = pendingControl;
    writeStored(current);
    this.showStopping(current);
    const recorderStopped = this.stopSegment(current);
    current.stream.getTracks().forEach((track) => track.stop());
    try {
      const drain = await drainBrowserMedia(current, recorderStopped);
      current.expectedNextSequence = current.uploads.expectedNextSequence;
      const errors = drain.errors;
      if (errors.length > 0) {
        current.stopDrainError = {
          code: "browser_audio_drain_incomplete",
          message: errors[0]!.message,
          retryable: !drain.recorderError && !(errors[0] instanceof IrrecoverableAudioLossError) && !current.uploads.hasIrrecoverableLoss
            && current.uploads.recoverablePendingCount > 0,
        };
        writeStored(current);
        return this.incompleteStopPanel(current, current.stopDrainError);
      }
      // The host owns finalization, so it must not close chunk admission until
      // the browser's bounded final-data drain has settled.
      const result = await this.reconcileStop(current);
      return result;
    } finally {
      this.emit();
    }
  }

  async retryPendingStop() {
    const stored = readStored();
    if (!stored || stored.pendingControl?.kind !== "stop") return;
    const pending = this.pendingCapture?.sessionId === stored.sessionId ? this.pendingCapture : null;
    // Without this page's queue (after a reload), deliver what IndexedDB kept.
    const persisted = pending ? null : await this.flushPersisted(stored.sessionId);
    if (stored.stopDrainError) {
      if (!stored.stopDrainError.retryable) return this.incompleteStopPanel(stored, stored.stopDrainError);
      if (persisted && persisted.found.size > 0) {
        // The server's sequence fence in Stop decides whether anything is
        // still missing; a reported gap stays retryable while its bytes remain.
        stored.stopDrainError = undefined;
        writeStored(stored);
      } else {
        if (!pending || pending.uploads.recoverablePendingCount === 0) {
          const error = { ...stored.stopDrainError, retryable: false,
            message: `${stored.stopDrainError.message} The missing audio is unavailable in this browser.`,
          };
          stored.stopDrainError = error;
          writeStored(stored);
          return this.incompleteStopPanel(stored, error);
        }
        try {
          await pending.uploads.retryPending();
        } catch (cause) {
          const error = { code: "browser_audio_drain_incomplete", message: cause instanceof Error ? cause.message : String(cause),
            retryable: !(cause instanceof IrrecoverableAudioLossError) && !pending.uploads.hasIrrecoverableLoss };
          stored.stopDrainError = error;
          writeStored(stored);
          return this.incompleteStopPanel(stored, error);
        }
        stored.nextSequence = Math.max(stored.nextSequence, pending.nextSequence);
        stored.stopDrainError = undefined;
        writeStored(stored);
      }
    }
    if (stored.stopGapError) {
      if (stored.stopGapSequence === undefined
        || !pending && !persisted?.delivered.has(stored.stopGapSequence)
          && !await this.hasPersisted(stored.sessionId, stored.stopGapSequence)) {
        const error = {
          ...stored.stopGapError,
          message: `${stored.stopGapError.message} The missing chunk is unavailable in this browser; recording is incomplete.`,
          retryable: false,
        };
        stored.stopGapError = error;
        writeStored(stored);
        return this.incompleteStopPanel(stored, error);
      }
      try {
        if (!persisted?.delivered.has(stored.stopGapSequence)) {
          await this.resendGap(stored.sessionId, stored.stopGapSequence, pending);
        }
      } catch (cause) {
        const message = cause instanceof Error ? cause.message : String(cause);
        const error = { code: "browser_chunk_gap", message,
          retryable: !(cause instanceof IrrecoverableAudioLossError),
        };
        stored.stopGapError = error;
        writeStored(stored);
        return this.incompleteStopPanel(stored, error);
      }
      stored.stopGapError = undefined;
      stored.stopGapSequence = undefined;
      writeStored(stored);
    }
    const result = await this.reconcileStop(stored);
    return result;
  }

  async finishIncomplete() {
    const stored = readStored();
    if (!stored || stored.pendingControl?.kind !== "stop" || !this.pluginId || !this.canFinishIncomplete) return;
    stored.finishIncompleteRequested = true;
    writeStored(stored);
    this.showStopping(stored);
    try {
      const state = await this.dependencies.rpc<PanelState>(this.pluginId, "finishIncomplete", {
        sessionId: stored.sessionId, client: detectClientCapabilities(),
        operationId: stored.pendingControl.operationId,
        expectedNextSequence: stored.expectedNextSequence ?? stored.nextSequence,
      });
      if (state.error?.code === "session_not_owned") {
        const saved = await this.savedAfterLease(stored);
        if (saved) return saved;
      }
      if (state.error || state.state !== "saved") {
        return this.incompleteStopPanel(stored, state.error ?? {
          code: "incomplete_finish_unconfirmed", message: "Margins has not confirmed the incomplete recording was saved. Try again.",
          retryable: true,
        });
      }
      this.forgetPersisted(stored.sessionId);
      this.endedRecordingId = stored.sessionId;
      writeStored(null);
      if (this.pendingCapture?.sessionId === stored.sessionId) {
        this.pendingCapture.uploads.releaseRetained();
        this.pendingCapture = null;
      }
      this.acceptPanel(stored.sessionId, state);
      return state;
    } catch (cause) {
      return this.incompleteStopPanel(stored, { code: "incomplete_finish_unconfirmed",
        message: cause instanceof Error ? cause.message : String(cause), retryable: true });
    }
  }

  private incompleteStopPanel(stored: StoredCapture, error: HostError): PanelState {
    const prior = this.panels.get(stored.sessionId) ?? this.latestPanel;
    const incomplete: PanelState = {
      schema: prior?.schema ?? "margins.bb.recording.panel.v2",
      state: "needs_attention",
      title: "Recording incomplete",
      detail: error.message,
      sourceLabel: prior?.sourceLabel ?? null,
      storageLabel: prior?.storageLabel ?? null,
      primaryAction: error.retryable && !stored.finishIncompleteRequested ? "retry" : "finish_incomplete",
      primaryLabel: error.retryable && !stored.finishIncompleteRequested ? "Try again" : "Finish with what was saved",
      canStop: false,
      canEditNotepad: false,
      ownsRecording: true,
      recordingId: stored.sessionId,
      sessionId: stored.sessionId,
      notepad: prior?.notepad ?? null,
      lastSessionId: prior?.lastSessionId ?? null,
      error,
    };
    this.acceptPanel(stored.sessionId, incomplete);
    return incomplete;
  }

  private async savedAfterLease(stored: StoredCapture): Promise<PanelState | null> {
    if (!this.pluginId || !stored.projectId) return null;
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "getProjectPanelState", {
      projectId: stored.projectId, client: detectClientCapabilities(),
    });
    if (state.state !== "saved" || state.lastSessionId !== stored.sessionId) return null;
    this.forgetPersisted(stored.sessionId);
    this.endedRecordingId = stored.sessionId;
    writeStored(null);
    if (this.pendingCapture?.sessionId === stored.sessionId) {
      this.pendingCapture.uploads.releaseRetained();
      this.pendingCapture = null;
    }
    this.acceptPanel(stored.sessionId, state);
    return state;
  }

  private async reconcileStop(stored: StoredCapture): Promise<PanelState> {
    if (!this.pluginId || stored.pendingControl?.kind !== "stop") throw new Error("No pending Stop to reconcile");
    const state = await this.dependencies.rpc<PanelState>(this.pluginId, "stop", {
      sessionId: stored.sessionId, client: detectClientCapabilities(),
      operationId: stored.pendingControl.operationId,
      expectedNextSequence: stored.expectedNextSequence ?? stored.nextSequence,
      ...(stored.pendingControl.segmentTimeUnixMs !== undefined
        ? { segmentEndedUnixMs: stored.pendingControl.segmentTimeUnixMs } : {}),
    });
    if (state.error?.code === "session_not_owned") {
      const saved = await this.savedAfterLease(stored);
      if (saved) return saved;
    }
    const latest = readStored();
    const recovery = latest?.sessionId === stored.sessionId
      && latest.pendingControl?.kind === "stop"
      && latest.pendingControl.operationId === stored.pendingControl.operationId
      ? {
          ...stored,
          nextSequence: Math.max(stored.nextSequence, latest.nextSequence),
          expectedNextSequence: Math.max(stored.expectedNextSequence ?? stored.nextSequence,
            latest.expectedNextSequence ?? latest.nextSequence),
          stopDrainError: stored.stopDrainError ?? latest.stopDrainError,
          stopGapError: stored.stopGapError ?? latest.stopGapError,
          stopGapSequence: stored.stopGapSequence ?? latest.stopGapSequence,
        }
      : stored;
    if (state.error === null && state.state === "saved" && !recovery.stopDrainError && !recovery.stopGapError) {
      this.forgetPersisted(recovery.sessionId);
      this.endedRecordingId = recovery.sessionId;
      writeStored(null);
      if (this.pendingCapture?.sessionId === recovery.sessionId) {
        this.pendingCapture.uploads.releaseRetained();
        this.pendingCapture = null;
      }
      this.acceptPanel(recovery.sessionId, state);
      return state;
    }
    if (state.error?.code === "browser_chunk_gap") {
      recovery.stopGapError = state.error;
      recovery.stopGapSequence = missingBrowserAudioSequence(state.error) ?? undefined;
    }
    const rawError = recovery.stopDrainError ?? state.error ?? {
      code: "authority_stop_unconfirmed",
      message: "Margins did not confirm that the recording finished. Try again with the retained Stop operation.",
      retryable: true,
    };
    const gapReplayable = recovery.stopGapSequence === undefined
      || this.pendingCapture?.uploads.canReplay(recovery.stopGapSequence)
      || await this.hasPersisted(recovery.sessionId, recovery.stopGapSequence);
    const error: HostError = state.error?.code === "browser_audio_empty" || !gapReplayable
      ? { ...rawError, retryable: false } : rawError;
    if (state.error?.code === "browser_audio_empty") recovery.stopDrainError = error;
    if (recovery.stopGapError) recovery.stopGapError = { ...recovery.stopGapError, retryable: error.retryable };
    const canFinish = recovery.stopDrainError?.retryable === false || recovery.stopGapError?.retryable === false;
    const incomplete: PanelState = {
      ...state,
      state: "needs_attention",
      title: "Recording needs attention",
      detail: error.message,
      primaryAction: error.retryable ? "retry" : canFinish ? "finish_incomplete" : "none",
      primaryLabel: error.retryable ? "Try again" : canFinish ? "Finish with what was saved" : "Recording unavailable",
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
    this.pendingCapture = current;
    ++this.generation;
    this.heartbeatRecovering = true;
    this.endedRecordingId = current.sessionId;
    current.levelMonitor?.dispose();
    current.reachability.dispose();
    clearInterval(current.heartbeat);
    current.pendingControl = current.pendingControl?.kind === "stop"
      ? current.pendingControl
      : { kind: "stop", operationId: id(), segmentTimeUnixMs: Date.now() };
    writeStored(current);
    this.emit();
    const recorderStopped = this.stopSegment(current);
    current.stream.getTracks().forEach((track) => track.stop());
    const drain = await drainBrowserMedia(current, recorderStopped);
    current.expectedNextSequence = current.uploads.expectedNextSequence;
    const errors = drain.errors;
    if (errors.length > 0) {
      current.stopDrainError = {
        code: "browser_audio_drain_incomplete",
        message: errors[0]!.message,
        retryable: !drain.recorderError && !(errors[0] instanceof IrrecoverableAudioLossError) && !current.uploads.hasIrrecoverableLoss
          && current.uploads.recoverablePendingCount > 0,
      };
      writeStored(current);
      this.incompleteStopPanel(current, current.stopDrainError);
      return;
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
    const recorderStopped = this.stopSegment(current);
    current.stream.getTracks().forEach((track) => track.stop());
    void drainBrowserMedia(current, recorderStopped);
    this.emit();
    // Keep sessionStorage: a refresh can reacquire the microphone and continue
    // within the grace period. No explicit stop bypasses the server-side grace.
  }
}

export const browserCaptureOwner = new BrowserCaptureOwner();
