import assert from "node:assert/strict";
import test from "node:test";

import { HostedRecoveryBrowserState, sameHostedRecording } from "../src/lib/hosted-recovery-browser-state.ts";
import type { RecordingStatus, WebRecordingRecoveryStatus } from "../src/lib/tauri.ts";

function recovery(recording_id: string, session_name = "same-name"): WebRecordingRecoveryStatus {
  return {
    recording_id,
    session_name,
    elapsed_secs: 10,
    finalization_error: "failed",
    owner_lease_active: false,
    recovery_phase: "failed",
  };
}

function status(recordingId: string, sessionName = "same-name"): RecordingStatus {
  return { is_recording: true, paused: false, session_name: sessionName, web_recording_id: recordingId } as RecordingStatus;
}

test("active B and same-name retained A remain separate through discovery and selection", () => {
  const state = new HostedRecoveryBrowserState();
  state.discover([recovery("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")]);
  assert.equal(state.activeStatus(status("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")), null);
  const activeB = status("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
  assert.equal(state.activeStatus(activeB)?.web_recording_id, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

  state.select("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
  assert.equal(state.selectedRecordingId(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
  assert.equal(state.acceptsSelectedPoll("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", activeB), false);
  assert.equal(sameHostedRecording(status("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), activeB), false);
});

test("multiple failures keep exact selection and hydration authority", () => {
  const state = new HostedRecoveryBrowserState();
  const a = recovery("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "duplicate");
  const b = recovery("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "duplicate");
  state.discover([a, b]);
  state.select(b.recording_id);
  state.markHydrated(b.recording_id);

  assert.equal(state.selectedMemoReady(a.recording_id), false);
  assert.equal(state.selectedMemoReady(b.recording_id), true);
  assert.equal(state.acceptsSelectedPoll(b.recording_id, status(a.recording_id, "duplicate")), false);

  state.discover([a, b]);
  assert.equal(state.selectedRecordingId(), b.recording_id);
  state.discover([a]);
  assert.equal(state.selectedRecordingId(), null);
});

for (const mutation of ["Finish", "Discard"] as const) {
  test(`${mutation} selected recovery A immediately restores same-name active B`, () => {
    const state = new HostedRecoveryBrowserState();
    const a = recovery("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "same-name");
    const b = status("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "same-name");
    state.discover([a]);
    state.select(a.recording_id);
    state.markHydrated(a.recording_id);

    const restored = state.completeRecovery(a.recording_id, [], b);
    assert.equal(state.selectedRecordingId(), null);
    assert.equal(restored?.web_recording_id, b.web_recording_id);
  });
}
