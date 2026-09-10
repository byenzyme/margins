/**
 * Browser microphone PCM bridge for the hosted Parakeet worker.
 *
 * MediaRecorder remains the durable WebM recording path. This bridge taps the
 * same MediaStream through Web Audio and sends contiguous mono Float32 PCM to
 * the server's low-latency transcript endpoint.
 */

interface PcmInputBuffer {
  numberOfChannels: number;
  getChannelData(channel: number): Float32Array;
}

interface PcmProcessEvent {
  inputBuffer: PcmInputBuffer;
}

interface PcmProcessor {
  onaudioprocess: ((event: PcmProcessEvent) => void) | null;
  connect(destination: unknown): unknown;
  disconnect(): void;
}

interface PcmSource {
  connect(destination: PcmProcessor): unknown;
  disconnect(): void;
}

interface PcmAudioContext {
  readonly sampleRate: number;
  state: string;
  readonly destination: unknown;
  createMediaStreamSource(stream: MediaStream): PcmSource;
  createScriptProcessor(bufferSize: number, inputChannels: number, outputChannels: number): PcmProcessor;
  resume(): Promise<void>;
  suspend(): Promise<void>;
  close(): Promise<void>;
}

export interface WebLivePcmDependencies {
  createAudioContext(): PcmAudioContext;
  upload(sessionName: string, sampleRate: number, samples: Float32Array, signal: AbortSignal): Promise<void>;
}

export interface WebLivePcmOptions {
  maxQueuedBatches?: number;
  uploadTimeoutMs?: number;
  closeDeadlineMs?: number;
}

export type WebLivePcmFailureHandler = (error: Error) => void;

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function browserDependencies(
  upload: WebLivePcmDependencies["upload"],
): WebLivePcmDependencies | null {
  if (typeof AudioContext === "undefined") return null;
  return {
    createAudioContext: () => new AudioContext() as unknown as PcmAudioContext,
    upload,
  };
}

export class WebLivePcmCapture {
  private readonly context: PcmAudioContext;
  private readonly source: PcmSource;
  private readonly processor: PcmProcessor;
  private readonly sessionName: string;
  private readonly upload: WebLivePcmDependencies["upload"];
  private readonly batchSamples: number;
  private readonly queuedFrames: Float32Array[] = [];
  private queuedSampleCount = 0;
  private readonly uploadQueue: Float32Array[] = [];
  private readonly maxQueuedBatches: number;
  private readonly uploadTimeoutMs: number;
  private readonly closeDeadlineMs: number;
  private uploadWorker: Promise<void> | null = null;
  private activeUpload: AbortController | null = null;
  private readonly onFailure: WebLivePcmFailureHandler;
  private running = true;
  private closed = false;

  constructor(
    context: PcmAudioContext,
    source: PcmSource,
    processor: PcmProcessor,
    sessionName: string,
    upload: WebLivePcmDependencies["upload"],
    onFailure: WebLivePcmFailureHandler,
    options: WebLivePcmOptions,
  ) {
    this.context = context;
    this.source = source;
    this.processor = processor;
    this.sessionName = sessionName;
    this.upload = upload;
    this.onFailure = onFailure;
    this.maxQueuedBatches = Math.max(1, options.maxQueuedBatches ?? 4);
    this.uploadTimeoutMs = Math.max(1, options.uploadTimeoutMs ?? 5_000);
    this.closeDeadlineMs = Math.max(1, options.closeDeadlineMs ?? 1_000);
    // Half-second batches keep request overhead bounded while staying well
    // below the Parakeet worker's five-second rolling decode cadence.
    this.batchSamples = Math.max(1, Math.floor(context.sampleRate / 2));
    this.processor.onaudioprocess = this.handleAudioProcess;
  }

  async pause(): Promise<void> {
    if (this.closed || !this.running) return;
    this.running = false;
    this.flush();
    if (this.context.state !== "closed") await this.context.suspend().catch(() => undefined);
  }

  async resume(): Promise<void> {
    if (this.closed || this.running) return;
    if (this.context.state !== "running") await this.context.resume().catch(() => undefined);
    this.running = this.context.state === "running";
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.running = false;
    this.processor.onaudioprocess = null;
    this.flush();
    try { this.source.disconnect(); } catch { /* already disconnected */ }
    try { this.processor.disconnect(); } catch { /* already disconnected */ }
    const contextClose = this.context.state !== "closed"
      ? this.context.close().catch(() => undefined)
      : Promise.resolve();
    const worker = this.uploadWorker ?? Promise.resolve();
    let deadline: ReturnType<typeof setTimeout> | null = null;
    await Promise.race([
      Promise.allSettled([contextClose, worker]).then(() => undefined),
      new Promise<void>((resolve) => {
        deadline = setTimeout(() => {
          this.uploadQueue.length = 0;
          this.activeUpload?.abort(new Error("Live PCM close deadline exceeded"));
          this.reportFailure(new Error("Live PCM bridge stopped waiting for uploads so durable audio could finish."));
          resolve();
        }, this.closeDeadlineMs);
      }),
    ]).finally(() => {
      if (deadline) clearTimeout(deadline);
    });
  }

  private readonly handleAudioProcess = (event: PcmProcessEvent) => {
    if (!this.running || this.closed || event.inputBuffer.numberOfChannels < 1) return;
    const input = event.inputBuffer.getChannelData(0);
    if (input.length === 0) return;
    this.queuedFrames.push(new Float32Array(input));
    this.queuedSampleCount += input.length;
    if (this.queuedSampleCount >= this.batchSamples) this.flush();
  };

  private flush(): void {
    if (this.queuedSampleCount === 0) return;
    const samples = new Float32Array(this.queuedSampleCount);
    let offset = 0;
    for (const frame of this.queuedFrames) {
      samples.set(frame, offset);
      offset += frame.length;
    }
    this.queuedFrames.length = 0;
    this.queuedSampleCount = 0;

    if (this.uploadQueue.length >= this.maxQueuedBatches) {
      this.reportFailure(new Error(
        `Live PCM upload backpressure limit (${this.maxQueuedBatches} batches) reached; a supplementary batch was dropped.`,
      ));
      return;
    }
    this.uploadQueue.push(samples);
    this.startUploadWorker();
  }

  private startUploadWorker(): void {
    if (this.uploadWorker) return;
    const worker = this.drainUploads().finally(() => {
      if (this.uploadWorker === worker) this.uploadWorker = null;
      if (this.uploadQueue.length > 0 && !this.closed) this.startUploadWorker();
    });
    this.uploadWorker = worker;
    void worker.catch(() => undefined);
  }

  private async drainUploads(): Promise<void> {
    while (this.uploadQueue.length > 0) {
      const samples = this.uploadQueue.shift()!;
      try {
        await this.uploadWithDeadline(samples);
      } catch (error) {
        this.reportFailure(asError(error));
      }
    }
  }

  private async uploadWithDeadline(samples: Float32Array): Promise<void> {
    const controller = new AbortController();
    this.activeUpload = controller;
    let timeout: ReturnType<typeof setTimeout> | null = null;
    let rawUpload: Promise<void>;
    try {
      rawUpload = this.upload(
        this.sessionName,
        this.context.sampleRate,
        samples,
        controller.signal,
      );
    } catch (error) {
      rawUpload = Promise.reject(error);
    }
    // Suppress a late rejection from an implementation that ignored abort.
    void rawUpload.catch(() => undefined);
    const aborted = new Promise<never>((_, reject) => {
      controller.signal.addEventListener("abort", () => {
        reject(asError(controller.signal.reason || "Live PCM upload aborted"));
      }, { once: true });
    });
    const timedOut = new Promise<never>((_, reject) => {
      timeout = setTimeout(() => {
        const failure = new Error(`Live PCM upload timed out after ${this.uploadTimeoutMs}ms`);
        controller.abort(failure);
        reject(failure);
      }, this.uploadTimeoutMs);
    });
    try {
      await Promise.race([rawUpload, aborted, timedOut]);
    } finally {
      if (timeout) clearTimeout(timeout);
      if (this.activeUpload === controller) this.activeUpload = null;
    }
  }

  private reportFailure(failure: Error): void {
    // Live PCM is supplementary to durable MediaRecorder capture.
    this.onFailure(failure);
    console.warn("[http-backend] live PCM upload unavailable:", failure);
  }
}

export async function createWebLivePcmCapture(
  stream: MediaStream,
  sessionName: string,
  upload: WebLivePcmDependencies["upload"],
  dependencies: WebLivePcmDependencies | null = browserDependencies(upload),
  onFailure: WebLivePcmFailureHandler = () => {},
  options: WebLivePcmOptions = {},
): Promise<WebLivePcmCapture | null> {
  if (!dependencies) {
    onFailure(new Error("Web Audio is unavailable; live transcription cannot receive microphone PCM."));
    return null;
  }

  let context: PcmAudioContext | null = null;
  let source: PcmSource | null = null;
  let processor: PcmProcessor | null = null;
  try {
    context = dependencies.createAudioContext();
    source = context.createMediaStreamSource(stream);
    processor = context.createScriptProcessor(4096, 1, 1);
    source.connect(processor);
    // ScriptProcessor callbacks are only driven while connected to an output.
    // The callback never writes output samples, so this connection is silent.
    processor.connect(context.destination);
    if (context.state !== "running") await context.resume();
    if (context.state !== "running") throw new Error("Web Audio PCM bridge did not start");
    return new WebLivePcmCapture(
      context,
      source,
      processor,
      sessionName,
      dependencies.upload,
      onFailure,
      options,
    );
  } catch (error) {
    const failure = asError(error);
    onFailure(failure);
    console.warn("[http-backend] Web Audio PCM bridge could not start:", failure);
    try { source?.disconnect(); } catch { /* partially initialized */ }
    try { processor?.disconnect(); } catch { /* partially initialized */ }
    if (context?.state !== "closed") await context?.close().catch(() => undefined);
    return null;
  }
}
