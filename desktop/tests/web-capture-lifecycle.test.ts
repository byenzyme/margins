import assert from "node:assert/strict";
import test from "node:test";

import {
  convergeHostedPauseState,
  hostedPostureReconciliationAction,
} from "../src/lib/web-capture-lifecycle.ts";

test("failed server pause resumes browser capture", async () => {
  const transitions: string[] = [];
  await assert.rejects(convergeHostedPauseState(true, {
    async pauseLocal() { transitions.push("local-pause"); },
    async resumeLocal() { transitions.push("local-resume"); },
  }, async () => {
    transitions.push("server-pause");
    throw new Error("owner rejected");
  }), /owner rejected/);
  assert.deepEqual(transitions, ["local-pause", "server-pause", "local-resume"]);
});

test("failed server resume restores browser pause", async () => {
  const transitions: string[] = [];
  await assert.rejects(convergeHostedPauseState(false, {
    async pauseLocal() { transitions.push("local-pause"); },
    async resumeLocal() { transitions.push("local-resume"); },
  }, async () => {
    transitions.push("server-resume");
    throw new Error("closed");
  }), /closed/);
  assert.deepEqual(transitions, ["local-resume", "server-resume", "local-pause"]);
});

test("successful pause transition is not rolled back", async () => {
  const transitions: string[] = [];
  const result = await convergeHostedPauseState(true, {
    async pauseLocal() { transitions.push("local-pause"); },
    async resumeLocal() { transitions.push("local-resume"); },
  }, async () => {
    transitions.push("server-pause");
    return "paused";
  });
  assert.equal(result, "paused");
  assert.deepEqual(transitions, ["local-pause", "server-pause"]);
});

test("applied pause with a lost response converges from authoritative ID status", async () => {
  const transitions: string[] = [];
  const result = await convergeHostedPauseState(true, {
    async pauseLocal() { transitions.push("local-pause"); },
    async resumeLocal() { transitions.push("local-resume"); },
  }, async () => {
    transitions.push("server-applied-response-lost");
    throw new Error("connection reset");
  }, async () => {
    transitions.push("read-recording-id");
    return { paused: true, recordingId: "opaque-a" };
  });
  assert.equal(result.recordingId, "opaque-a");
  assert.deepEqual(transitions, ["local-pause", "server-applied-response-lost", "read-recording-id"]);
});

test("authoritative opposite posture rolls local state back after ambiguous resume", async () => {
  const transitions: string[] = [];
  await assert.rejects(convergeHostedPauseState(false, {
    async pauseLocal() { transitions.push("local-pause"); },
    async resumeLocal() { transitions.push("local-resume"); },
  }, async () => {
    transitions.push("server-response-lost");
    throw new Error("connection reset");
  }, async () => {
    transitions.push("read-recording-id");
    return { paused: true };
  }), /connection reset/);
  assert.deepEqual(transitions, ["local-resume", "server-response-lost", "read-recording-id", "local-pause"]);
});

test("later polling repairs MediaRecorder and PCM posture after double-failure ambiguity", () => {
  assert.equal(hostedPostureReconciliationAction(true, "recording"), "pause");
  assert.equal(hostedPostureReconciliationAction(false, "paused"), "resume");
  assert.equal(hostedPostureReconciliationAction(true, "paused"), null);
  assert.equal(hostedPostureReconciliationAction(false, "inactive"), null);
});
