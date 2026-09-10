import assert from "node:assert/strict";
import test from "node:test";

import { mergeRefreshedSessions, preserveLiveCaptureRow } from "../src/lib/live-capture-row.ts";
import type { SessionInfo } from "../src/lib/tauri.ts";

function session(name: string, status: SessionInfo["status"]): SessionInfo {
  return {
    name,
    project_id: "p1",
    start_time: "2026-07-28T10:00:00Z",
    notes_path: `${name}.md`,
    segment_count: 0,
    duration_secs: 0,
    memo_line_count: 0,
    status,
    vault_note_path: null,
  };
}

test("stamps a warming-up capture back to recording when the backend lists it as settled", () => {
  const listed = [session("older-note", "synthesized"), session("new-capture", "unprocessed")];
  const merged = preserveLiveCaptureRow(listed, "new-capture", null, 3);
  assert.equal(merged[1].status, "recording");
  assert.equal(merged[1].memo_line_count, 3);
  assert.equal(merged[0].status, "synthesized");
  // Input list is not mutated.
  assert.equal(listed[1].status, "unprocessed");
});

test("re-inserts the optimistic row when the backend does not list the capture yet", () => {
  const optimistic = session("new-capture", "recording");
  const merged = preserveLiveCaptureRow([session("older-note", "synthesized")], "new-capture", optimistic, 2);
  assert.equal(merged[0].name, "new-capture");
  assert.equal(merged[0].status, "recording");
  assert.equal(merged[0].memo_line_count, 2);
  assert.equal(merged.length, 2);
});

test("no capture in flight leaves the list untouched", () => {
  const listed = [session("older-note", "synthesized")];
  assert.equal(preserveLiveCaptureRow(listed, null, null, 0), listed);
  assert.equal(preserveLiveCaptureRow(listed, "  ", null, 0), listed);
});

test("missing capture with no fallback row leaves the list untouched", () => {
  const listed = [session("older-note", "synthesized")];
  assert.equal(preserveLiveCaptureRow(listed, "new-capture", null, 0), listed);
});

const noCapture = { captureName: null, captureFallback: null, memoLineCount: 0 };

test("import error rows survive a refresh that does not list them", () => {
  const errorRow = {
    ...session("failed-import", "unprocessed"),
    import_status: "error" as const,
    import_error: "boom",
    import_source_path: "/tmp/a.wav",
  };
  const merged = mergeRefreshedSessions([session("older-note", "synthesized")], [errorRow], {
    ...noCapture,
    processingName: null,
  });
  assert.equal(merged[0].name, "failed-import");
  assert.equal(merged[0].import_status, "error");
  assert.equal(merged[0].import_error, "boom");
});

test("in-flight import keeps its progress badge on the listed backend row", () => {
  const current = [{ ...session("importing", "processing"), import_status: "transcribing" as const, import_source_path: "/tmp/a.wav" }];
  const listed = [session("importing", "unprocessed")];
  const merged = mergeRefreshedSessions(listed, current, { ...noCapture, processingName: "importing" });
  assert.equal(merged[0].import_status, "transcribing");
  // The optimistic processing stamp also survives the refresh.
  assert.equal(merged[0].status, "processing");
});

test("finished import drops its badge once processing has cleared", () => {
  const current = [{ ...session("importing", "processing"), import_status: "note" as const, import_source_path: "/tmp/a.wav" }];
  const listed = [session("importing", "synthesized")];
  const merged = mergeRefreshedSessions(listed, current, { ...noCapture, processingName: null });
  assert.equal(merged[0].import_status, undefined);
  assert.equal(merged[0].status, "synthesized");
});

test("queued import placeholders are re-inserted", () => {
  const queued = { ...session("later-import", "unprocessed"), import_status: "queued" as const, import_source_path: "/tmp/b.wav" };
  const merged = mergeRefreshedSessions([session("older-note", "synthesized")], [queued], {
    ...noCapture,
    processingName: "some-other-import",
  });
  assert.equal(merged[0].name, "later-import");
  assert.equal(merged[0].import_status, "queued");
});

test("reprocess stamp is kept only while the backend still says unprocessed", () => {
  const current = [session("note-a", "processing")];
  const stillStale = mergeRefreshedSessions([session("note-a", "unprocessed")], current, { ...noCapture, processingName: "note-a" });
  assert.equal(stillStale[0].status, "processing");
  const settled = mergeRefreshedSessions([session("note-a", "synthesized")], current, { ...noCapture, processingName: "note-a" });
  assert.equal(settled[0].status, "synthesized");
  const failed = mergeRefreshedSessions([session("note-a", "failed")], current, { ...noCapture, processingName: "note-a" });
  assert.equal(failed[0].status, "failed");
});
