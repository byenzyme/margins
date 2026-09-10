import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyWebMicrophoneFailure,
  selectWebRecorderMimeType,
  webMicrophoneConstraints,
  webMicrophonePolicyBlockedGuidance,
  webMicrophoneRecoveryGuidance,
} from "../src/lib/web-microphone-permission.ts";

function namedError(name: string): Error {
  const error = new Error(name);
  error.name = name;
  return error;
}

test("classifies permission, policy, device, hardware, and cancellation failures", () => {
  assert.equal(classifyWebMicrophoneFailure(namedError("NotAllowedError")).category, "permission-denied");
  assert.equal(classifyWebMicrophoneFailure(namedError("SecurityError")).category, "policy-blocked");
  assert.equal(classifyWebMicrophoneFailure(namedError("NotFoundError")).category, "no-device");
  assert.equal(classifyWebMicrophoneFailure(namedError("OverconstrainedError")).category, "selected-device-unavailable");
  assert.equal(classifyWebMicrophoneFailure(namedError("NotReadableError")).category, "hardware-unavailable");
  assert.equal(classifyWebMicrophoneFailure(namedError("TrackStartError")).category, "hardware-unavailable");
  assert.equal(classifyWebMicrophoneFailure(namedError("AbortError")).category, "aborted");
});

test("recorder MIME negotiation accepts only the server's WebM contract", () => {
  assert.equal(selectWebRecorderMimeType(type => type === "audio/webm"), "audio/webm");
  assert.equal(selectWebRecorderMimeType(type => type === "audio/mp4"), null);
  assert.equal(selectWebRecorderMimeType(type => type === "audio/ogg"), null);
});

test("policy-blocked guidance does not present ordinary site-denial recovery", () => {
  const guidance = webMicrophonePolicyBlockedGuidance({ ios: false, standalone: false });
  assert.match(guidance, /administrator|Permissions Policy/);
  assert.match(guidance, /top-level HTTPS/);
  assert.doesNotMatch(guidance, /address bar/);
});

test("a missing exact selection is distinguished from no microphone", () => {
  assert.equal(
    classifyWebMicrophoneFailure(namedError("NotFoundError"), { selectedDevice: true }).category,
    "selected-device-unavailable",
  );
  assert.deepEqual(webMicrophoneConstraints("origin-device-id"), {
    audio: { deviceId: { exact: "origin-device-id" } },
  });
  assert.deepEqual(webMicrophoneConstraints(null), { audio: true });
});

test("blocked guidance distinguishes iOS installed PWA, Safari tab, and desktop PWA", () => {
  assert.match(webMicrophoneRecoveryGuidance({ ios: true, standalone: true }), /iOS Settings[\s\S]*Safari/);
  assert.match(webMicrophoneRecoveryGuidance({ ios: true, standalone: false }), /Safari[\s\S]*Website Settings/);
  assert.match(webMicrophoneRecoveryGuidance({ ios: false, standalone: true }), /installed app menu/);
  assert.match(webMicrophoneRecoveryGuidance({ ios: false, standalone: false }), /address bar/);
});
