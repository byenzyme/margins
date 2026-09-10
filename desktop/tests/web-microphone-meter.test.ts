import assert from "node:assert/strict";
import test from "node:test";

import { createWebMicrophoneMeter } from "../src/lib/web-microphone-meter.ts";

test("web microphone meter stays unknown until a real sample and releases its lifecycle", async () => {
  let frameCallback: FrameRequestCallback | null = null;
  let nextSamples = [0, 0.25, -0.1, 0];
  let sourceDisconnected = 0;
  let analyserDisconnected = 0;
  let suspended = 0;
  let resumed = 0;
  let closed = 0;
  let cancelled = 0;

  const analyser = {
    fftSize: 4,
    getFloatTimeDomainData(samples: Float32Array) {
      samples.set(nextSamples);
    },
    disconnect() { analyserDisconnected += 1; },
  };
  const source = {
    connect() {},
    disconnect() { sourceDisconnected += 1; },
  };
  const context = {
    state: "running",
    createAnalyser: () => analyser,
    createMediaStreamSource: () => source,
    async resume() { resumed += 1; context.state = "running"; },
    async suspend() { suspended += 1; context.state = "suspended"; },
    async close() { closed += 1; context.state = "closed"; },
  };

  const meter = await createWebMicrophoneMeter({} as MediaStream, {
    createAudioContext: () => context,
    requestFrame(callback) { frameCallback = callback; return 17; },
    cancelFrame() { cancelled += 1; frameCallback = null; },
  });
  assert.ok(meter);
  assert.deepEqual(meter.snapshot(), { level: null, rms: null, audioFrameCount: 0 });

  const firstFrame = frameCallback;
  assert.ok(firstFrame);
  firstFrame(0);
  const firstSnapshot = meter.snapshot();
  assert.equal(firstSnapshot.level, 0.25);
  assert.equal(firstSnapshot.audioFrameCount, 1024);
  // RMS over the zero-padded 1024-sample analyser window.
  assert.ok(firstSnapshot.rms !== null && Math.abs(firstSnapshot.rms - Math.sqrt(0.0725 / 1024)) < 1e-6);

  await meter.pause();
  assert.deepEqual(meter.snapshot(), { level: null, rms: null, audioFrameCount: 1024 });
  assert.equal(suspended, 1);
  assert.equal(cancelled, 1);

  await meter.resume();
  assert.equal(resumed, 1);
  assert.deepEqual(meter.snapshot(), { level: null, rms: null, audioFrameCount: 1024 });
  nextSamples = [0.00002, 0, 0, 0];
  const resumedFrame = frameCallback;
  assert.ok(resumedFrame);
  resumedFrame(1);
  const resumedSnapshot = meter.snapshot();
  assert.equal(resumedSnapshot.audioFrameCount, 2048);
  assert.ok(resumedSnapshot.level !== null && Math.abs(resumedSnapshot.level - 0.00002) < 1e-9);

  await meter.close();
  await meter.close();
  assert.equal(sourceDisconnected, 1);
  assert.equal(analyserDisconnected, 1);
  assert.equal(closed, 1);
  assert.equal(cancelled, 2);
});

test("failed Web Audio setup closes partial meter resources and stays unavailable", async () => {
  let sourceDisconnected = 0;
  let analyserDisconnected = 0;
  let closed = 0;
  const context = {
    state: "suspended",
    createAnalyser: () => ({
      fftSize: 4,
      getFloatTimeDomainData() {},
      disconnect() { analyserDisconnected += 1; },
    }),
    createMediaStreamSource: () => ({
      connect() {},
      disconnect() { sourceDisconnected += 1; },
    }),
    async resume() { throw new Error("audio context unavailable"); },
    async suspend() {},
    async close() { closed += 1; context.state = "closed"; },
  };

  const meter = await createWebMicrophoneMeter({} as MediaStream, {
    createAudioContext: () => context,
    requestFrame() { throw new Error("must not schedule"); },
    cancelFrame() {},
  });

  assert.equal(meter, null);
  assert.equal(sourceDisconnected, 1);
  assert.equal(analyserDisconnected, 1);
  assert.equal(closed, 1);
});

test("failed resume leaves microphone level unknown and does not restart sampling", async () => {
  let scheduled = 0;
  const context = {
    state: "running",
    createAnalyser: () => ({
      fftSize: 4,
      getFloatTimeDomainData() {},
      disconnect() {},
    }),
    createMediaStreamSource: () => ({ connect() {}, disconnect() {} }),
    async resume() { throw new Error("resume blocked"); },
    async suspend() { context.state = "suspended"; },
    async close() { context.state = "closed"; },
  };

  const meter = await createWebMicrophoneMeter({} as MediaStream, {
    createAudioContext: () => context,
    requestFrame() { scheduled += 1; return scheduled; },
    cancelFrame() {},
  });
  assert.ok(meter);
  assert.equal(scheduled, 1);

  await meter.pause();
  await meter.resume();
  assert.deepEqual(meter.snapshot(), { level: null, rms: null, audioFrameCount: 0 });
  assert.equal(scheduled, 1);
  await meter.close();
});
