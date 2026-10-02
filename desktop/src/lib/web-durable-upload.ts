export interface WebDurableUploadOptions {
  maxQueuedChunks?: number;
  maxQueuedBytes?: number;
  uploadTimeoutMs?: number;
  closeDeadlineMs?: number;
  initialSequence?: number;
  onSequenceAssigned?: (nextSequence: number) => void;
  maxAttempts?: number;
  maxReplayBytes?: number;
}

export interface WebChunkTiming {
  capturedStartUnixMs: number;
  capturedEndUnixMs: number;
}

export type DurableChunkUpload = (chunk: Blob, sequence: number, signal: AbortSignal, timing?: WebChunkTiming) => Promise<void>;

export class IrrecoverableAudioLossError extends Error {}

export function bindDurableMediaRecorder<T extends { data: Blob }>(
  recorder: { ondataavailable: ((event: T) => unknown) | null },
  uploads: WebDurableUploadQueue,
  segmentStartedUnixMs = Date.now(),
): void {
  let nextStartUnixMs = segmentStartedUnixMs;
  recorder.ondataavailable = event => {
    const endUnixMs = Math.max(nextStartUnixMs, Date.now());
    uploads.enqueue(event.data, { capturedStartUnixMs: nextStartUnixMs, capturedEndUnixMs: endUnixMs });
    nextStartUnixMs = endUnixMs;
  };
}

/** Ask MediaRecorder for its final chunk without allowing a missing `stop`
 * event to hold Finish open forever. A timeout leaves recording completeness
 * unknown and must be surfaced to the caller. */
export function stopMediaRecorderWithDeadline(
  recorder: Pick<MediaRecorder, "onstop" | "stop">,
  timeoutMs = 5_000,
): Promise<Error | null> {
  return new Promise((resolve) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const finish = (error: Error | null) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      resolve(error);
    };
    const deadline = Math.max(1, timeoutMs);
    recorder.onstop = () => finish(null);
    timer = setTimeout(() => finish(new Error(
      `MediaRecorder stop event exceeded ${deadline}ms; recording may be incomplete.`,
    )), deadline);
    try {
      recorder.stop();
    } catch (error) {
      finish(asError(error));
    }
  });
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

/** Ordered, bounded transport for MediaRecorder chunks. WebM blobs must never
 * overtake one another because the server appends them to one durable file. */
export class WebDurableUploadQueue {
  private readonly upload: DurableChunkUpload;
  private readonly onFailure: (error: Error) => void;
  private readonly queue: Array<{ chunk: Blob; sequence: number; timing?: WebChunkTiming }> = [];
  private readonly maxQueuedChunks: number;
  private readonly maxQueuedBytes: number;
  private queuedBytes = 0;
  private readonly uploadTimeoutMs: number;
  private readonly closeDeadlineMs: number;
  private readonly maxAttempts: number;
  private readonly maxReplayBytes: number;
  private readonly onSequenceAssigned?: (nextSequence: number) => void;
  private readonly replay = new Map<number, { chunk: Blob; timing?: WebChunkTiming }>();
  private replayBytes = 0;
  private worker: Promise<void> | null = null;
  private active: AbortController | null = null;
  private accepting = true;
  private nextSequence: number;
  private blocked = false;
  private lostChunk = false;
  private lastFailure: Error | null = null;

  constructor(
    upload: DurableChunkUpload,
    onFailure: (error: Error) => void,
    options: WebDurableUploadOptions = {},
  ) {
    this.upload = upload;
    this.onFailure = onFailure;
    this.maxQueuedChunks = Math.max(1, options.maxQueuedChunks ?? Number.MAX_SAFE_INTEGER);
    this.maxQueuedBytes = Math.max(1, options.maxQueuedBytes ?? 64 * 1024 * 1024);
    this.uploadTimeoutMs = Math.max(1, options.uploadTimeoutMs ?? 10_000);
    this.closeDeadlineMs = Math.max(1, options.closeDeadlineMs ?? 12_000);
    this.maxAttempts = Math.max(1, Math.trunc(options.maxAttempts ?? 3));
    this.maxReplayBytes = Math.max(0, Math.trunc(options.maxReplayBytes ?? 64 * 1024 * 1024));
    this.onSequenceAssigned = options.onSequenceAssigned;
    this.nextSequence = Math.max(0, Math.trunc(options.initialSequence ?? 0));
  }

  get pendingCount(): number {
    // The active item remains at queue[0] until its retry identity is settled.
    return this.queue.length;
  }

  get expectedNextSequence(): number { return this.nextSequence; }

  get recoverablePendingCount(): number { return this.queue.length; }
  get hasIrrecoverableLoss(): boolean { return this.lostChunk; }
  canReplay(sequence: number): boolean { return this.replay.has(sequence); }

  /** Replay an acknowledged chunk when Stop reports that its sequence is
   * missing. The server must accept an identical duplicate idempotently. */
  async resend(sequence: number): Promise<void> {
    const item = this.replay.get(sequence);
    if (!item) throw new IrrecoverableAudioLossError(`Browser audio sequence ${sequence} is no longer available to retry; recording is incomplete`);
    let lastError: Error | null = null;
    for (let attempt = 0; attempt < this.maxAttempts; attempt += 1) {
      try {
        await this.uploadWithTimeout(item.chunk, sequence, item.timing);
        return;
      } catch (error) {
        lastError = asError(error);
      }
    }
    throw lastError ?? new Error(`Browser audio sequence ${sequence} could not be retried`);
  }

  /** Release replay bytes after the authority has durably finalized Stop. */
  releaseRetained(): void {
    this.replay.clear();
    this.replayBytes = 0;
  }

  enqueue(chunk: Blob, timing?: WebChunkTiming): void {
    if (!this.accepting || chunk.size === 0) return;
    if (this.queue.length >= this.maxQueuedChunks || this.queuedBytes + chunk.size > this.maxQueuedBytes) {
      // The caller must still fence Stop against this sequence. The Blob was
      // not retained, so this capture cannot be finalized as complete.
      this.nextSequence += 1;
      this.onSequenceAssigned?.(this.nextSequence);
      this.lostChunk = true;
      this.onFailure(new IrrecoverableAudioLossError(
        `Durable audio upload backpressure limit (${this.maxQueuedBytes} bytes) reached; a WebM chunk was not delivered.`,
      ));
      return;
    }
    this.queue.push({ chunk, sequence: this.nextSequence++, timing });
    this.onSequenceAssigned?.(this.nextSequence);
    this.queuedBytes += chunk.size;
    this.startWorker();
  }

  async close(): Promise<void> {
    this.accepting = false;
    await this.retryPending();
  }

  /** Retry retained, unacknowledged Blobs during a pending Stop. */
  async retryPending(): Promise<void> {
    if (this.lostChunk) throw new IrrecoverableAudioLossError("A WebM chunk exceeded the upload queue limit; recording is incomplete");
    if (this.blocked && this.worker) await this.worker;
    this.blocked = false;
    this.lastFailure = null;
    this.startWorker();
    const worker = this.worker ?? Promise.resolve();
    let timer: ReturnType<typeof setTimeout> | null = null;
    await Promise.race<void>([
      worker,
      new Promise<void>((_resolve, reject) => {
        timer = setTimeout(() => {
          const error = new Error(
            `Durable audio upload drain exceeded ${this.closeDeadlineMs}ms; recording is incomplete.`,
          );
          this.active?.abort(error);
          this.blocked = true;
          this.lastFailure = error;
          this.onFailure(error);
          reject(error);
        }, this.closeDeadlineMs);
      }),
    ]).finally(() => {
      if (timer) clearTimeout(timer);
    });
    if (this.blocked || this.queue.length > 0) {
      throw this.lastFailure ?? new Error("Durable audio upload is incomplete");
    }
  }

  private startWorker(): void {
    if (this.worker || this.blocked || this.queue.length === 0) return;
    const worker = this.drain().finally(() => {
      if (this.worker === worker) this.worker = null;
      if (!this.blocked && this.queue.length > 0) this.startWorker();
    });
    this.worker = worker;
    void worker.catch(() => undefined);
  }

  private async drain(): Promise<void> {
    while (this.queue.length > 0) {
      const item = this.queue[0]!;
      let lastError: Error | null = null;
      for (let attempt = 0; attempt < this.maxAttempts; attempt += 1) {
        try {
          await this.uploadWithTimeout(item.chunk, item.sequence, item.timing);
          lastError = null;
          break;
        } catch (error) {
          lastError = asError(error);
          // close() may have reached its deadline and aborted this attempt.
          // Leave this Blob and every later sequence queued for an explicit
          // retry; do not start another attempt after the Stop deadline.
          if (this.blocked) return;
        }
      }
      if (lastError) {
        this.blocked = true;
        this.lastFailure = lastError;
        this.onFailure(lastError);
        return;
      }
      this.queue.shift();
      this.queuedBytes -= item.chunk.size;
      this.retainForReplay(item);
    }
  }

  private retainForReplay(item: { chunk: Blob; sequence: number; timing?: WebChunkTiming }): void {
    // Server ACKs are durable, so old replay copies may be evicted without
    // affecting the normal path. A later gap on an evicted sequence remains
    // visibly incomplete rather than finalizing with missing audio.
    if (item.chunk.size > this.maxReplayBytes) return;
    while (this.replayBytes + item.chunk.size > this.maxReplayBytes) {
      const oldest = this.replay.keys().next().value;
      if (oldest === undefined) break;
      this.replayBytes -= this.replay.get(oldest)!.chunk.size;
      this.replay.delete(oldest);
    }
    this.replay.set(item.sequence, { chunk: item.chunk, timing: item.timing });
    this.replayBytes += item.chunk.size;
  }

  private async uploadWithTimeout(chunk: Blob, sequence: number, timing?: WebChunkTiming): Promise<void> {
    const controller = new AbortController();
    this.active = controller;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let raw: Promise<void>;
    try {
      raw = this.upload(chunk, sequence, controller.signal, timing);
    } catch (error) {
      raw = Promise.reject(error);
    }
    void raw.catch(() => undefined);
    const aborted = new Promise<never>((_, reject) => {
      controller.signal.addEventListener("abort", () => reject(asError(controller.signal.reason)), { once: true });
    });
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        const error = new Error(`Durable audio upload timed out after ${this.uploadTimeoutMs}ms`);
        controller.abort(error);
        reject(error);
      }, this.uploadTimeoutMs);
    });
    try {
      await Promise.race([raw, aborted, timeout]);
    } finally {
      if (timer) clearTimeout(timer);
      if (this.active === controller) this.active = null;
    }
  }
}
