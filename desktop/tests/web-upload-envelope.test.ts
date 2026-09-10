import assert from "node:assert/strict";
import test from "node:test";

import { requireSuccessfulWebUpload } from "../src/lib/web-upload-envelope.ts";

test("WebM upload accepts only an explicit durable success envelope", async () => {
  await requireSuccessfulWebUpload(
    new Response(JSON.stringify({ ok: true }), { status: 200 }),
    "Audio chunk upload",
  );
});

for (const error of [
  "Capture authority for 'meeting' belongs to another browser operation",
  "Failed to write audio chunk: disk full",
  "Web recording file is already closed",
]) {
  test(`WebM HTTP 200 rejection reaches uploadErrors/UI: ${error}`, async () => {
    await assert.rejects(
      requireSuccessfulWebUpload(
        new Response(JSON.stringify({ ok: false, error }), { status: 200 }),
        "Audio chunk upload",
      ),
      new RegExp(error.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    );
  });
}

test("WebM upload rejects a malformed HTTP 200 envelope", async () => {
  await assert.rejects(
    requireSuccessfulWebUpload(new Response("not json", { status: 200 }), "Audio chunk upload"),
    /invalid success envelope/,
  );
});
