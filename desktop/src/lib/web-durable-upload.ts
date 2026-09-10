export interface WebDurableUploadOptions {
  maxQueuedChunks?: number;
  uploadTimeoutMs?: number;
  closeDeadlineMs?: number;
}

export type DurableChunkUpload = (chunk: Blob, sequence: number, signal: AbortSignal) => Promise<void>;

export function bindDurableMediaRecorder<T extends { data: Blob }>(
  recorder: { ondataavailable: ((event: T) => unknown) | null },
  uploads: WebDurableUploadQueue,
): void {
  recorder.ondataavailable = event => uploads.enqueue(event.data);
}

/** Ask MediaRecorder for its final chunk without allowing a missing `stop`
 * event to hold Finish open forever. A timeout is returned as transport
 * damage so the caller can still finalize the server-received subset. */
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
      `MediaRecorder stop event exceeded ${deadline}ms; Finish continued with the server-received subset.`,
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
  private readonly queue: Array<{ chunk: Blob; sequence: number }> = [];
  private readonly maxQueuedChunks: number;
  private readonly uploadTimeoutMs: number;
  private readonly closeDeadlineMs: number;
  private worker: Promise<void> | null = null;
  private active: AbortController | null = null;
  private accepting = true;
  private nextSequence = 0;

  constructor(
    upload: DurableChunkUpload,
    onFailure: (error: Error) => void,
    options: WebDurableUploadOptions = {},
  ) {
    this.upload = upload;
    this.onFailure = onFailure;
    this.maxQueuedChunks = Math.max(1, options.maxQueuedChunks ?? 8);
    this.uploadTimeoutMs = Math.max(1, options.uploadTimeoutMs ?? 10_000);
    this.closeDeadlineMs = Math.max(1, options.closeDeadlineMs ?? 12_000);
  }

  get pendingCount(): number {
    return this.queue.length + (this.active ? 1 : 0);
  }

  enqueue(chunk: Blob): void {
    if (!this.accepting || chunk.size === 0) return;
    if (this.queue.length >= this.maxQueuedChunks) {
      this.onFailure(new Error(
        `Durable audio upload backpressure limit (${this.maxQueuedChunks} chunks) reached; a WebM chunk was not delivered.`,
      ));
      return;
    }
    this.queue.push({ chunk, sequence: this.nextSequence++ });
    this.startWorker();
  }

  async close(): Promise<void> {
    this.accepting = false;
    const worker = this.worker ?? Promise.resolve();
    let timer: ReturnType<typeof setTimeout> | null = null;
    await Promise.race([
      worker,
      new Promise<void>((resolve) => {
        timer = setTimeout(() => {
          this.queue.length = 0;
          const error = new Error(
            `Durable audio upload drain exceeded ${this.closeDeadlineMs}ms; Finish continued with the server-received subset.`,
          );
          this.active?.abort(error);
          this.onFailure(error);
          resolve();
        }, this.closeDeadlineMs);
      }),
    ]).finally(() => {
      if (timer) clearTimeout(timer);
    });
  }

  private startWorker(): void {
    if (this.worker) return;
    const worker = this.drain().finally(() => {
      if (this.worker === worker) this.worker = null;
      if (this.accepting && this.queue.length > 0) this.startWorker();
    });
    this.worker = worker;
    void worker.catch(() => undefined);
  }

  private async drain(): Promise<void> {
    while (this.queue.length > 0) {
      const { chunk, sequence } = this.queue.shift()!;
      try {
        await this.uploadWithTimeout(chunk, sequence);
      } catch (error) {
        this.onFailure(asError(error));
      }
    }
  }

  private async uploadWithTimeout(chunk: Blob, sequence: number): Promise<void> {
    const controller = new AbortController();
    this.active = controller;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let raw: Promise<void>;
    try {
      raw = this.upload(chunk, sequence, controller.signal);
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
