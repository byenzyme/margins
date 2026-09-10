import assert from "node:assert/strict";
import test from "node:test";

import { WebCaptureOperationController } from "../src/lib/web-capture-operation.ts";

test("late cancelled start discards only its own session after a retry begins", async () => {
  const controller = new WebCaptureOperationController();
  const startA = controller.begin();
  assert.equal(controller.cancelCurrent(), true);
  const startB = controller.begin();
  const discarded: string[] = [];
  const discard = async (sessionName: string) => { discarded.push(sessionName); };

  assert.equal(await controller.discardIfStale(startA, "delayed-a", discard), true);
  assert.equal(await controller.discardIfStale(startB, "active-b", discard), false);
  assert.deepEqual(discarded, ["delayed-a"]);
  assert.equal(controller.isCurrent(startB), true);
});

test("Start Cancel Retry cancels slow preflight before permission allocation", async () => {
  const controller = new WebCaptureOperationController();
  const deferred = () => {
    let resolve!: () => void;
    const promise = new Promise<void>(done => { resolve = done; });
    return { promise, resolve };
  };
  const firstGate = deferred();
  const retryGate = deferred();
  let permissionAllocations = 0;
  const run = async (gate: Promise<void>) => {
    const operation = controller.begin();
    await gate;
    if (!controller.isCurrent(operation)) return false;
    permissionAllocations += 1;
    controller.complete(operation);
    return true;
  };

  const first = run(firstGate.promise);
  assert.equal(controller.cancelCurrent(), true);
  const retry = run(retryGate.promise);
  firstGate.resolve();
  assert.equal(await first, false);
  assert.equal(permissionAllocations, 0);
  retryGate.resolve();
  assert.equal(await retry, true);
  assert.equal(permissionAllocations, 1);
});

test("Cancel during awaited PCM setup prevents backend allocation and retry remains usable", async () => {
  const controller = new WebCaptureOperationController();
  let releasePcm!: () => void;
  const pcmSetup = new Promise<void>(resolve => { releasePcm = resolve; });
  let backendAllocations = 0;

  const start = async (setup: Promise<void>) => {
    const operation = controller.begin();
    await setup;
    if (!controller.isCurrent(operation)) return false;
    backendAllocations += 1;
    controller.complete(operation);
    return true;
  };

  const cancelled = start(pcmSetup);
  assert.equal(controller.cancelCurrent(), true);
  releasePcm();
  assert.equal(await cancelled, false);
  assert.equal(backendAllocations, 0);

  assert.equal(await start(Promise.resolve()), true);
  assert.equal(backendAllocations, 1);
});
