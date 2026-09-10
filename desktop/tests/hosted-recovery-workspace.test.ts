import assert from "node:assert/strict";
import test from "node:test";

import {
  executeHostedRecoveryMutation,
  hostedMemoSyncAllowed,
  HostedRecoveryWorkspaceStore,
} from "../src/lib/hosted-recovery-workspace.ts";

interface Workspace {
  recordingId: string;
  sessionName: string;
  memo: string[];
  editorSelection: { activeId: string; start: number; end: number };
  visible: boolean;
  controllable: boolean;
}

const recoveryA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const activeB = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

for (const mutationName of ["Finish", "Discard"] as const) {
  test(`${mutationName} handler restores same-name active B before its next sync tick`, async () => {
    const store = new HostedRecoveryWorkspaceStore<Workspace, { memo: string[] }>();
    const bSnapshot: Workspace = {
      recordingId: activeB,
      sessionName: "same-name",
      memo: ["B live memo"],
      editorSelection: { activeId: "memo-input-0", start: 2, end: 7 },
      visible: true,
      controllable: true,
    };
    store.rememberActive(activeB, structuredClone(bSnapshot));
    let visibleWorkspace: Workspace = {
      ...structuredClone(bSnapshot),
      recordingId: recoveryA,
      memo: ["A recovery memo"],
    };
    const mutations: string[] = [];

    const completion = await executeHostedRecoveryMutation({
      completedRecordingId: recoveryA,
      mutate: async () => {
        mutations.push(mutationName);
        if (mutationName === "Finish") {
          store.queueFinished(recoveryA, { memo: [...visibleWorkspace.memo] });
        }
        return "same-name";
      },
      restoreActive: async () => {
        visibleWorkspace = store.takeActive(activeB)!;
        return visibleWorkspace;
      },
    });

    assert.deepEqual(mutations, [mutationName]);
    assert.equal(completion.active.recordingId, activeB);
    assert.deepEqual(visibleWorkspace.memo, ["B live memo"]);
    assert.deepEqual(visibleWorkspace.editorSelection, bSnapshot.editorSelection);
    assert.equal(visibleWorkspace.visible, true);
    assert.equal(visibleWorkspace.controllable, true);

    const syncTickAllowed = hostedMemoSyncAllowed({
      recordingId: visibleWorkspace.recordingId,
      selectedRecoveryId: null,
      selectedRecoveryHydrated: false,
      locallyOwned: true,
    });
    assert.equal(syncTickAllowed, true);
    const uploadedMemo = syncTickAllowed ? [...visibleWorkspace.memo] : [];
    assert.deepEqual(uploadedMemo, ["B live memo"]);
    assert.notDeepEqual(uploadedMemo, ["A recovery memo"]);
    if (mutationName === "Finish") {
      assert.deepEqual(store.finished(recoveryA)?.memo, ["A recovery memo"]);
    }
  });
}

test("a selected recovery suspends active B sync and requires exact hydrated ownership", () => {
  assert.equal(hostedMemoSyncAllowed({
    recordingId: activeB,
    selectedRecoveryId: recoveryA,
    selectedRecoveryHydrated: true,
    locallyOwned: true,
  }), false);
  assert.equal(hostedMemoSyncAllowed({
    recordingId: recoveryA,
    selectedRecoveryId: recoveryA,
    selectedRecoveryHydrated: false,
    locallyOwned: true,
  }), false);
  assert.equal(hostedMemoSyncAllowed({
    recordingId: recoveryA,
    selectedRecoveryId: recoveryA,
    selectedRecoveryHydrated: true,
    locallyOwned: true,
  }), true);
});
