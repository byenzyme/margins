import assert from "node:assert/strict";
import test from "node:test";

import {
  hostedCaptureProtocolHeaders,
  HOSTED_CAPTURE_PROTOCOL_VERSION,
  validateHostedCaptureProtocol,
} from "../src/lib/hosted-capture-protocol.ts";

test("hosted boot negotiates an exact protocol and sends the version header", () => {
  const protocol = validateHostedCaptureProtocol({
    version: HOSTED_CAPTURE_PROTOCOL_VERSION,
    minimumClientVersion: HOSTED_CAPTURE_PROTOCOL_VERSION,
  });
  assert.equal(protocol.version, HOSTED_CAPTURE_PROTOCOL_VERSION);
  assert.deepEqual(hostedCaptureProtocolHeaders(), {
    "X-Margins-Capture-Protocol": String(HOSTED_CAPTURE_PROTOCOL_VERSION),
  });
});

test("mixed browser/server capture versions require a reload", () => {
  assert.throws(() => validateHostedCaptureProtocol({
    version: 99,
    minimumClientVersion: 99,
  }), /protocol mismatch.*Reload/i);
});
