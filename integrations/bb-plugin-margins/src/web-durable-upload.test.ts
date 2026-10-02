import { describe, expect, it, vi } from "vitest";
import { bindDurableMediaRecorder, IrrecoverableAudioLossError, WebDurableUploadQueue } from "../../../desktop/src/lib/web-durable-upload.js";

describe("browser durable upload queue", () => {
  it("uses a byte budget so a 24-second outage does not discard eight small chunks", async () => {
    let available = false;
    const uploaded: number[] = [];
    const failure = vi.fn();
    const queue = new WebDurableUploadQueue(async (_chunk, sequence) => {
      if (!available) throw new Error("project temporarily offline");
      uploaded.push(sequence);
    }, failure, { maxQueuedBytes: 1_024, maxAttempts: 1,
      uploadTimeoutMs: 50, closeDeadlineMs: 100 });
    for (let sequence = 0; sequence < 10; sequence += 1) queue.enqueue(new Blob(["audio"]));
    expect(queue.pendingCount).toBe(10);
    expect(queue.hasIrrecoverableLoss).toBe(false);
    await vi.waitFor(() => expect(failure).toHaveBeenCalledOnce());
    available = true;
    await queue.retryPending();
    expect(uploaded).toEqual(Array.from({ length: 10 }, (_, index) => index));
  });

  it("reports terminal loss only when the byte budget is exceeded", async () => {
    const queue = new WebDurableUploadQueue(async () => undefined, vi.fn(), {
      maxQueuedBytes: 4, maxAttempts: 1,
    });
    queue.enqueue(new Blob(["12345"]));
    expect(queue.hasIrrecoverableLoss).toBe(true);
    expect(queue.expectedNextSequence).toBe(1);
    await expect(queue.close()).rejects.toBeInstanceOf(IrrecoverableAudioLossError);
  });

  it("replays an acknowledged Blob with the same capture timestamps", async () => {
    const delivered: Array<{ sequence: number; start?: number; end?: number }> = [];
    const queue = new WebDurableUploadQueue(async (_chunk, sequence, _signal, timing) => {
      delivered.push({ sequence, start: timing?.capturedStartUnixMs, end: timing?.capturedEndUnixMs });
    }, vi.fn());
    queue.enqueue(new Blob(["a"]), { capturedStartUnixMs: 1_000, capturedEndUnixMs: 4_000 });
    await queue.close();
    await queue.resend(0);
    expect(delivered).toEqual([
      { sequence: 0, start: 1_000, end: 4_000 },
      { sequence: 0, start: 1_000, end: 4_000 },
    ]);
  });

  it("bounds successive blobs by their actual wall-clock capture times", async () => {
    const now = vi.spyOn(Date, "now");
    const intervals: Array<[number, number]> = [];
    const queue = new WebDurableUploadQueue(async (_chunk, _sequence, _signal, timing) => {
      intervals.push([timing!.capturedStartUnixMs, timing!.capturedEndUnixMs]);
    }, vi.fn());
    const recorder = { ondataavailable: null as ((event: { data: Blob }) => void) | null };
    bindDurableMediaRecorder(recorder, queue, 1_000);
    now.mockReturnValue(1_800);
    recorder.ondataavailable?.({ data: new Blob(["first"]) });
    now.mockReturnValue(2_150);
    recorder.ondataavailable?.({ data: new Blob(["short final blob"]) });
    await queue.close();
    expect(intervals).toEqual([[1_000, 1_800], [1_800, 2_150]]);
    now.mockRestore();
  });
});
