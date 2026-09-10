// Run with:
//   node --test --experimental-strip-types src/lib/lifecycle-state.status.test.ts
//
// The note-writing status line is a stable label surface. `note_stream` events
// carry the streamed note body itself in `message` (raw chunks like "---" and
// YAML frontmatter); that content must never become the job status, or it
// flashes above the heading as the note streams.
import { test } from "node:test";
import assert from "node:assert/strict";

import { reduceSessionLifecycle, deriveSessionJobState } from "./lifecycle-state.ts";
import type { ProcessingEvent } from "./tauri.ts";

function noteStreamEvent(message: string): ProcessingEvent {
  return { stage: "note_stream", message, progress: null, session: "s", track: "note", phase: "writing", emitted_at_ms: 1 };
}

test("note_stream chunks never surface as the job status", () => {
  let state = reduceSessionLifecycle(undefined, noteStreamEvent("---"), "s");
  assert.equal(deriveSessionJobState(state).message, "Writing note.");

  state = reduceSessionLifecycle(state, noteStreamEvent("created: \"[[2026-07-13]]\""), "s");
  assert.equal(deriveSessionJobState(state).message, "Writing note.");

  state = reduceSessionLifecycle(state, noteStreamEvent("This capture contains only the Harvard sentences"), "s");
  const job = deriveSessionJobState(state);
  assert.equal(job.status, "processing");
  assert.equal(job.message, "Writing note.");
});

test("meaningful note-track status messages are still surfaced", () => {
  // A real status update (not a streamed body chunk) must keep its message.
  const event: ProcessingEvent = {
    stage: "synthesize", message: "Preparing compact vault context…", progress: null,
    session: "s", track: "note", phase: "preparing_context", emitted_at_ms: 1,
  };
  const state = reduceSessionLifecycle(undefined, event, "s");
  assert.equal(deriveSessionJobState(state).message, "Preparing compact vault context…");
});
