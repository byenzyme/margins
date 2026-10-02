import assert from "node:assert/strict";
import test from "node:test";

import {
  bindDurableMediaRecorder,
  stopMediaRecorderWithDeadline,
  WebDurableUploadQueue,
} from "../src/lib/web-durable-upload.ts";

class FakeSequencedWebmServer {
  private expected = 0;
  private receipts = new Map<number, string>();
  readonly appended: number[] = [];

  accept(sequence: number, body: string): void {
    if (sequence < this.expected) {
      if (this.receipts.get(sequence) === body) return;
      throw new Error(`WebM chunk sequence ${sequence} was already committed with different content`);
    }
    if (sequence !== this.expected) {
      throw new Error(`Out-of-order WebM chunk ${sequence}; expected ${this.expected}`);
    }
    this.receipts.set(sequence, body);
    this.appended.push(sequence);
    this.expected += 1;
  }
}

test("MediaRecorder dataavailable chunks upload in order before Finish", async () => {
  const uploaded: string[] = [];
  const sequences: number[] = [];
  const queue = new WebDurableUploadQueue(async (chunk, sequence) => {
    const value = await chunk.text();
    if (value === "one") await new Promise(resolve => setTimeout(resolve, 5));
    uploaded.push(value);
    sequences.push(sequence);
  }, error => assert.fail(error.message), { uploadTimeoutMs: 100, closeDeadlineMs: 100 });

  const recorder = { ondataavailable: null as ((event: { data: Blob }) => void) | null };
  bindDurableMediaRecorder(recorder, queue);
  recorder.ondataavailable?.({ data: new Blob(["one"]) });
  recorder.ondataavailable?.({ data: new Blob(["two"]) });
  assert.equal(queue.pendingCount, 2);
  await queue.close();
  assert.deepEqual(uploaded, ["one", "two"]);
  assert.deepEqual(sequences, [0, 1]);
  assert.equal(queue.pendingCount, 0);
});

test("never-settling durable upload is aborted and cannot hang Finish", async () => {
  const failures: string[] = [];
  let aborted = false;
  const queue = new WebDurableUploadQueue((_chunk, _sequence, signal) => new Promise<void>(() => {
    signal.addEventListener("abort", () => { aborted = true; }, { once: true });
  }), error => failures.push(error.message), { maxAttempts: 1, uploadTimeoutMs: 10, closeDeadlineMs: 30 });

  queue.enqueue(new Blob(["stuck"]));
  const started = Date.now();
  await assert.rejects(queue.close(), /incomplete|timed out/);
  assert.ok(Date.now() - started < 200, "Finish drain must be bounded");
  assert.equal(aborted, true);
  assert.ok(failures.some(message => message.includes("timed out")));
  assert.equal(queue.pendingCount, 1);
});

test("fake server rejects a later upload while a timed-out earlier request is still late", async () => {
  const attempted: number[] = [];
  const failures: string[] = [];
  const server = new FakeSequencedWebmServer();
  const queue = new WebDurableUploadQueue(async (chunk, sequence) => {
    attempted.push(sequence);
    const body = await chunk.text();
    if (sequence === 0) {
      setTimeout(() => server.accept(sequence, body), 15);
      await new Promise<void>(() => {});
    }
    server.accept(sequence, body);
  }, error => failures.push(error.message), { maxAttempts: 1, uploadTimeoutMs: 5, closeDeadlineMs: 50 });
  queue.enqueue(new Blob(["late-zero"]));
  queue.enqueue(new Blob(["one"]));
  await assert.rejects(queue.close(), /incomplete|timed out|Out-of-order/);
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(attempted, [0]);
  assert.deepEqual(server.appended, [0]);
  assert.equal(queue.pendingCount, 2);
});

test("fake server accepts only an identical retry for an already durable sequence", () => {
  const server = new FakeSequencedWebmServer();
  server.accept(0, "chunk-zero");
  server.accept(0, "chunk-zero");
  assert.throws(() => server.accept(0, "different"), /already committed with different content/);
  assert.throws(() => server.accept(2, "future"), /Out-of-order WebM chunk/);
  assert.deepEqual(server.appended, [0]);
});

test("lost ACK retries the exact sequence and stores one durable chunk", async () => {
  const server = new FakeSequencedWebmServer();
  const attempts: number[] = [];
  const queue = new WebDurableUploadQueue(async (chunk, sequence) => {
    attempts.push(sequence);
    server.accept(sequence, await chunk.text());
    if (attempts.length === 1) throw new Error("ack lost after commit");
  }, error => assert.fail(error.message), { maxAttempts: 2, uploadTimeoutMs: 100, closeDeadlineMs: 100 });
  queue.enqueue(new Blob(["only-once"]));
  await queue.close();
  assert.deepEqual(attempts, [0, 0]);
  assert.deepEqual(server.appended, [0]);
});

test("failed durable chunk blocks later uploads until its retained Blob succeeds", async () => {
  const uploaded: string[] = [];
  const failures: string[] = [];
  let fail = true;
  const queue = new WebDurableUploadQueue(async chunk => {
    const value = await chunk.text();
    if (value === "bad" && fail) throw new Error("HTTP 200 ok:false");
    uploaded.push(value);
  }, error => failures.push(error.message), { maxAttempts: 2, uploadTimeoutMs: 100, closeDeadlineMs: 100 });

  queue.enqueue(new Blob(["good-a"]));
  queue.enqueue(new Blob(["bad"]));
  queue.enqueue(new Blob(["good-b"]));
  await assert.rejects(queue.close(), /HTTP 200 ok:false/);
  assert.deepEqual(uploaded, ["good-a"]);
  assert.equal(queue.pendingCount, 2);
  fail = false;
  await queue.retryPending();
  assert.deepEqual(uploaded, ["good-a", "bad", "good-b"]);
  assert.deepEqual(failures, ["HTTP 200 ok:false"]);
});

test("durable queue applies bounded backpressure", async () => {
  const failures: string[] = [];
  let release!: () => void;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const queue = new WebDurableUploadQueue(() => blocked, error => failures.push(error.message), {
    maxQueuedChunks: 1,
    uploadTimeoutMs: 500,
    closeDeadlineMs: 500,
  });
  queue.enqueue(new Blob(["active"]));
  queue.enqueue(new Blob(["queued"]));
  queue.enqueue(new Blob(["dropped"]));
  assert.ok(failures.some(message => message.includes("backpressure limit")));
  release();
  await assert.rejects(queue.close(), /recording is incomplete/);
  assert.equal(queue.expectedNextSequence, 3);
});

test("Finish bounds a missing MediaRecorder stop event and keeps queued final data", async () => {
  const uploaded: string[] = [];
  const failures: Error[] = [];
  const queue = new WebDurableUploadQueue(async chunk => {
    uploaded.push(await chunk.text());
  }, error => failures.push(error), { closeDeadlineMs: 50 });
  const recorder = {
    ondataavailable: null as ((event: { data: Blob }) => unknown) | null,
    onstop: null as (() => unknown) | null,
    stop() {
      this.ondataavailable?.({ data: new Blob(["final"]) });
      // Deliberately omit onstop, matching a suspended/broken browser event.
    },
  };
  bindDurableMediaRecorder(recorder, queue);

  const stopError = await stopMediaRecorderWithDeadline(recorder as unknown as MediaRecorder, 5);
  if (stopError) failures.push(stopError);
  await queue.close();

  assert.deepEqual(uploaded, ["final"]);
  assert.equal(failures.length, 1);
  assert.match(failures[0].message, /stop event exceeded/);
});
