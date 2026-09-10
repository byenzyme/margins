import assert from "node:assert/strict";
import test from "node:test";

import { createWebLivePcmCapture } from "../src/lib/web-live-pcm.ts";

test("web live PCM batches contiguous mono samples and follows capture lifecycle", async () => {
  let processAudio: ((event: { inputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array } }) => void) | null = null;
  let sourceDisconnected = 0;
  let processorDisconnected = 0;
  let suspended = 0;
  let resumed = 0;
  let closed = 0;
  const uploads: Array<{ session: string; sampleRate: number; samples: number[] }> = [];

  const processor = {
    get onaudioprocess() { return processAudio; },
    set onaudioprocess(callback) { processAudio = callback; },
    connect() {},
    disconnect() { processorDisconnected += 1; },
  };
  const source = {
    connect() {},
    disconnect() { sourceDisconnected += 1; },
  };
  const context = {
    sampleRate: 8,
    state: "running",
    destination: {},
    createMediaStreamSource: () => source,
    createScriptProcessor: () => processor,
    async resume() { resumed += 1; context.state = "running"; },
    async suspend() { suspended += 1; context.state = "suspended"; },
    async close() { closed += 1; context.state = "closed"; },
  };
  const upload = async (session: string, sampleRate: number, samples: Float32Array) => {
    uploads.push({ session, sampleRate, samples: Array.from(samples) });
  };

  const capture = await createWebLivePcmCapture({} as MediaStream, "meeting-1", upload, {
    createAudioContext: () => context,
    upload,
  });
  assert.ok(capture);
  assert.ok(processAudio);

  const emit = (samples: number[]) => processAudio?.({
    inputBuffer: {
      numberOfChannels: 1,
      getChannelData: () => Float32Array.from(samples),
    },
  });
  emit([0.1, 0.2]);
  assert.equal(uploads.length, 0);
  emit([0.3, 0.4]);
  await Promise.resolve();
  assert.equal(uploads.length, 1);
  assert.equal(uploads[0].session, "meeting-1");
  assert.equal(uploads[0].sampleRate, 8);
  assert.deepEqual(uploads[0].samples.map(value => Number(value.toFixed(3))), [0.1, 0.2, 0.3, 0.4]);

  emit([0.5]);
  await capture.pause();
  await Promise.resolve();
  assert.equal(suspended, 1);
  assert.deepEqual(uploads[1].samples, [0.5]);
  emit([0.6, 0.7, 0.8, 0.9]);
  assert.equal(uploads.length, 2, "paused capture must not enqueue samples");

  await capture.resume();
  assert.equal(resumed, 1);
  emit([0.6, 0.7]);
  await capture.close();
  await capture.close();
  assert.deepEqual(uploads[2].samples.map(value => Number(value.toFixed(3))), [0.6, 0.7]);
  assert.equal(sourceDisconnected, 1);
  assert.equal(processorDisconnected, 1);
  assert.equal(closed, 1);
  assert.equal(processAudio, null);
});

test("web live PCM uploads batches in capture order", async () => {
  let processAudio: ((event: { inputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array } }) => void) | null = null;
  const completions: Array<() => void> = [];
  const started: number[][] = [];
  const processor = {
    get onaudioprocess() { return processAudio; },
    set onaudioprocess(callback) { processAudio = callback; },
    connect() {},
    disconnect() {},
  };
  const context = {
    sampleRate: 4,
    state: "running",
    destination: {},
    createMediaStreamSource: () => ({ connect() {}, disconnect() {} }),
    createScriptProcessor: () => processor,
    async resume() {},
    async suspend() { context.state = "suspended"; },
    async close() { context.state = "closed"; },
  };
  const upload = async (_session: string, _sampleRate: number, samples: Float32Array) => {
    started.push(Array.from(samples));
    await new Promise<void>(resolve => completions.push(resolve));
  };
  const capture = await createWebLivePcmCapture({} as MediaStream, "ordered", upload, {
    createAudioContext: () => context,
    upload,
  });
  assert.ok(capture);
  const emit = (samples: number[]) => processAudio?.({
    inputBuffer: {
      numberOfChannels: 1,
      getChannelData: () => Float32Array.from(samples),
    },
  });

  emit([1, 2]);
  emit([3, 4]);
  await Promise.resolve();
  assert.deepEqual(started, [[1, 2]], "second request must wait for the first");
  completions.shift()?.();
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(started, [[1, 2], [3, 4]]);
  completions.shift()?.();
  await capture.close();
});

test("failed PCM setup releases partial Web Audio resources", async () => {
  let sourceDisconnected = 0;
  let processorDisconnected = 0;
  let closed = 0;
  const context = {
    sampleRate: 48_000,
    state: "suspended",
    destination: {},
    createMediaStreamSource: () => ({
      connect() {},
      disconnect() { sourceDisconnected += 1; },
    }),
    createScriptProcessor: () => ({
      onaudioprocess: null,
      connect() {},
      disconnect() { processorDisconnected += 1; },
    }),
    async resume() { throw new Error("blocked"); },
    async suspend() {},
    async close() { closed += 1; context.state = "closed"; },
  };
  const upload = async () => {};

  const failures: string[] = [];
  const capture = await createWebLivePcmCapture({} as MediaStream, "meeting-2", upload, {
    createAudioContext: () => context,
    upload,
  }, error => failures.push(error.message));
  assert.equal(capture, null);
  assert.equal(sourceDisconnected, 1);
  assert.equal(processorDisconnected, 1);
  assert.equal(closed, 1);
  assert.deepEqual(failures, ["blocked"]);
});

test("failed ordered PCM upload is reported and later batches stay ordered", async () => {
  let processAudio: ((event: { inputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array } }) => void) | null = null;
  const started: number[][] = [];
  const failures: string[] = [];
  const processor = {
    get onaudioprocess() { return processAudio; },
    set onaudioprocess(callback) { processAudio = callback; },
    connect() {},
    disconnect() {},
  };
  const context = {
    sampleRate: 4,
    state: "running",
    destination: {},
    createMediaStreamSource: () => ({ connect() {}, disconnect() {} }),
    createScriptProcessor: () => processor,
    async resume() {},
    async suspend() { context.state = "suspended"; },
    async close() { context.state = "closed"; },
  };
  let attempt = 0;
  const upload = async (_session: string, _sampleRate: number, samples: Float32Array) => {
    started.push(Array.from(samples));
    attempt += 1;
    if (attempt === 1) throw new Error("HTTP 503");
  };
  const capture = await createWebLivePcmCapture({} as MediaStream, "ordered-errors", upload, {
    createAudioContext: () => context,
    upload,
  }, error => failures.push(error.message));
  assert.ok(capture);
  const emit = (samples: number[]) => processAudio?.({
    inputBuffer: { numberOfChannels: 1, getChannelData: () => Float32Array.from(samples) },
  });

  emit([1, 2]);
  emit([3, 4]);
  await new Promise<void>(resolve => setImmediate(resolve));
  await new Promise<void>(resolve => setImmediate(resolve));
  await capture.close();

  assert.deepEqual(started, [[1, 2], [3, 4]]);
  assert.deepEqual(failures, ["HTTP 503"]);
});

test("never-settling PCM upload times out, bounded queue recovers, and close cannot block durable audio", async () => {
  let processAudio: ((event: { inputBuffer: { numberOfChannels: number; getChannelData(channel: number): Float32Array } }) => void) | null = null;
  const started: number[][] = [];
  const failures: string[] = [];
  const processor = {
    get onaudioprocess() { return processAudio; },
    set onaudioprocess(callback) { processAudio = callback; },
    connect() {},
    disconnect() {},
  };
  const context = {
    sampleRate: 4,
    state: "running",
    destination: {},
    createMediaStreamSource: () => ({ connect() {}, disconnect() {} }),
    createScriptProcessor: () => processor,
    async resume() {},
    async suspend() { context.state = "suspended"; },
    async close() { await new Promise<void>(() => {}); },
  };
  let attempt = 0;
  const upload = async (_session: string, _sampleRate: number, samples: Float32Array) => {
    started.push(Array.from(samples));
    attempt += 1;
    if (attempt === 1) await new Promise<void>(() => {});
  };
  const capture = await createWebLivePcmCapture({} as MediaStream, "bounded", upload, {
    createAudioContext: () => context,
    upload,
  }, error => failures.push(error.message), {
    maxQueuedBatches: 2,
    uploadTimeoutMs: 5,
    closeDeadlineMs: 5,
  });
  assert.ok(capture);
  const emit = (samples: number[]) => processAudio?.({
    inputBuffer: { numberOfChannels: 1, getChannelData: () => Float32Array.from(samples) },
  });

  emit([1, 2]);
  emit([3, 4]);
  emit([5, 6]);
  emit([7, 8]);
  assert.equal(started.length, 1);
  assert.match(failures[0] || "", /backpressure limit/);
  await new Promise<void>(resolve => setTimeout(resolve, 15));
  assert.deepEqual(started, [[1, 2], [3, 4], [5, 6]], "accepted batches recover in order after timeout");
  assert.ok(failures.some(message => /timed out/.test(message)));

  const closeStarted = Date.now();
  await capture.close();
  assert.ok(Date.now() - closeStarted < 100, "supplementary PCM close must be deadline bounded");
});
