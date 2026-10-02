import { experimental_createHostEntryHarness } from "@get-bb/plugin-sdk/testing/host";
import { describe, expect, it, vi } from "vitest";
import { createMarginsHostEntry } from "./host-entry.js";
import type { ProjectMarginsTransport } from "./project-server.js";

const target = { projectId: "proj-1", hostId: "host-1", projectRoot: "/srv/project" };
const snapshot = { recordingId: "rec-1", sessionId: "rec-1", status: "recording" as const,
  nextSequence: 0, notepad: { text: "", revision: "v1" } };

describe("Margins project host entry", () => {
  it("routes capture to the explicit project root and emits project invalidations", async () => {
    const transport = {
      authority: vi.fn(async () => ({ ok: true as const, instanceId: "instance-1", workspaceId: "workspace-1" })),
      sessionExists: vi.fn(async () => ({ ok: true as const, found: true })),
      start: vi.fn(async () => ({ ok: true as const, snapshot })),
      read: vi.fn(async () => ({ ok: true as const, snapshot })),
      mutate: vi.fn(async () => ({ ok: true as const, snapshot })),
      stop: vi.fn(async () => ({ ok: true as const, snapshot: null })),
      finishIncomplete: vi.fn(async () => ({ ok: true as const, snapshot: null })),
      upload: vi.fn(async () => ({ ok: true as const })),
      connectedNoteContext: vi.fn(async () => ({ ok: true as const, context: {
        schema: "margins.bb.connected-note-context.v1" as const, instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-1", title: null,
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 1 }, memo: { revision: "memo-1", lineCount: 0 }, artifacts: [], noteAssociation: null, instructions: "pin",
      } })),
      requestTranscription: vi.fn(async () => ({ ok: true as const, status: "queued" as const, attempt: 1 })),
      dispose: vi.fn(async () => undefined),
    } as unknown as ProjectMarginsTransport;
    const harness = experimental_createHostEntryHarness(createMarginsHostEntry(transport));
    await expect(harness.experimental_call("startBrowserCapture", { target, ownerId: "owner-1", name: "customer-call", startedAtUnixMs: 1_000 })).resolves.toMatchObject({ ok: true, snapshot: { recordingId: "rec-1" } });
    await expect(harness.experimental_call("pause", { target, recordingId: "rec-1", ownerId: "owner-1", expectedNextSequence: 0,
      segmentEndedUnixMs: 1_500, recoveredAfterReload: true })).resolves.toMatchObject({ ok: true });
    expect(transport.mutate).toHaveBeenCalledWith(target, expect.any(String), "rec-1", "owner-1", "pause_recording", 0, 1_500, true);
    await expect(harness.experimental_call("uploadChunk", { target, recordingId: "rec-1", ownerId: "owner-1",
      sequence: 0, bytesBase64: "YQ==", capturedStartUnixMs: 1_000, capturedEndUnixMs: 1_500 })).resolves.toMatchObject({ ok: true });
    expect(transport.upload).toHaveBeenCalledWith(target, expect.any(String), "rec-1", "owner-1", 0, "YQ==", 1_000, 1_500);
    await expect(harness.experimental_call("connectedNoteContext", { target, recordingId: "rec-1" })).resolves.toMatchObject({ ok: true, context: { sessionId: "rec-1" } });
    await expect(harness.experimental_call("requestTranscription", { target, recordingId: "rec-1" })).resolves.toMatchObject({ ok: true, status: "queued" });
    expect(harness.experimental_getRetainedWorkerLeaseCount()).toBe(1);
    expect(transport.start).toHaveBeenCalledWith(target, expect.any(String), "owner-1", "customer-call", 1_000);
    expect(harness.experimental_getSignals()).toEqual([
      { signal: "changed", payload: { projectId: "proj-1", reason: "start" } },
      { signal: "changed", payload: { projectId: "proj-1", reason: "pause" } },
    ]);
    await harness.experimental_dispose();
    expect(harness.experimental_getRetainedWorkerLeaseCount()).toBe(0);
  });
});
