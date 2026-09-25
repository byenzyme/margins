import { experimental_createHostEntryHarness } from "@get-bb/plugin-sdk/testing/host";
import { describe, expect, it, vi } from "vitest";
import { createMarginsHostEntry } from "./host-entry.js";
import type { ProjectMarginsTransport } from "./project-server.js";

const target = { projectId: "proj-1", hostId: "host-1", projectRoot: "/srv/project" };
const snapshot = { recordingId: "rec-1", sessionId: "rec-1", status: "recording" as const, notepad: { text: "", revision: "v1" } };

describe("Margins project host entry", () => {
  it("routes capture to the explicit project root and emits project invalidations", async () => {
    const transport = {
      authority: vi.fn(async () => ({ ok: true as const, instanceId: "instance-1", workspaceId: "workspace-1" })),
      sessionExists: vi.fn(async () => ({ ok: true as const, found: true })),
      start: vi.fn(async () => ({ ok: true as const, snapshot })),
      read: vi.fn(async () => ({ ok: true as const, snapshot })),
      mutate: vi.fn(async () => ({ ok: true as const, snapshot })),
      stop: vi.fn(async () => ({ ok: true as const, snapshot: null })),
      updateNotepad: vi.fn(async () => ({ ok: true as const, snapshot })),
      upload: vi.fn(async () => ({ ok: true as const })),
      connectedNoteContext: vi.fn(async () => ({ ok: true as const, context: {
        schema: "margins.bb.connected-note-context.v1" as const, instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-1", title: null,
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 1 }, memo: { revision: "memo-1", lineCount: 0 }, artifacts: [], noteAssociation: null, instructions: "pin",
      } })),
      requestTranscription: vi.fn(async () => ({ ok: true as const, status: "queued" as const, attempt: 1 })),
      dispose: vi.fn(async () => undefined),
    } as unknown as ProjectMarginsTransport;
    const harness = experimental_createHostEntryHarness(createMarginsHostEntry(transport));
    await expect(harness.experimental_call("startBrowserCapture", { target, ownerId: "owner-1", name: "customer-call" })).resolves.toMatchObject({ ok: true, snapshot: { recordingId: "rec-1" } });
    await expect(harness.experimental_call("pause", { target, recordingId: "rec-1", ownerId: "owner-1" })).resolves.toMatchObject({ ok: true });
    await expect(harness.experimental_call("connectedNoteContext", { target, recordingId: "rec-1" })).resolves.toMatchObject({ ok: true, context: { sessionId: "rec-1" } });
    await expect(harness.experimental_call("requestTranscription", { target, recordingId: "rec-1" })).resolves.toMatchObject({ ok: true, status: "queued" });
    expect(harness.experimental_getRetainedWorkerLeaseCount()).toBe(1);
    expect(transport.start).toHaveBeenCalledWith(target, expect.any(String), "owner-1", "customer-call");
    expect(harness.experimental_getSignals()).toEqual([
      { signal: "changed", payload: { projectId: "proj-1", reason: "start" } },
      { signal: "changed", payload: { projectId: "proj-1", reason: "pause" } },
    ]);
    await harness.experimental_dispose();
    expect(harness.experimental_getRetainedWorkerLeaseCount()).toBe(0);
  });
});
