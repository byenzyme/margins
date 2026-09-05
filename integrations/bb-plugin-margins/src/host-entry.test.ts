import { experimental_createHostEntryHarness } from "@get-bb/plugin-sdk/testing/host";
import { describe, expect, it, vi } from "vitest";
import { createMarginsHostEntry } from "./host-entry.js";
import { desktopSnapshot } from "./fixtures.js";
import type { MarginsLiveTransport } from "./live-transport.js";
import type { HostOperationResult } from "./contracts.js";

function transport(): MarginsLiveTransport {
  const ok = (status: Parameters<typeof desktopSnapshot>[0]): HostOperationResult => ({
    ok: true,
    snapshot: desktopSnapshot(status),
  });
  return {
    readSnapshot: vi.fn(async () => ok("recording")),
    ensureRuntime: vi.fn(async () => ok("idle")),
    start: vi.fn(async () => ok("recording")),
    pause: vi.fn(async () => ok("paused")),
    resume: vi.fn(async () => ok("recording")),
    stop: vi.fn(async () => ({
      ...ok("finalizing"),
      stopped_session_id: "customer-call",
    })),
    updateNotepad: vi.fn(async () => ok("recording")),
  };
}

describe("Margins host entry", () => {
  it("validates the host RPC contract and emits invalidation signals", async () => {
    const fakeTransport = transport();
    const harness = experimental_createHostEntryHarness(
      createMarginsHostEntry({ transport: fakeTransport }),
    );

    await expect(
      harness.experimental_call("start", {
        operationId: "op-start",
        name: "Customer call",
      }),
    ).resolves.toMatchObject({
      ok: true,
      snapshot: { session: { session_id: "customer-call", status: "recording" } },
    });
    await expect(
      harness.experimental_call("pause", {
        operationId: "op-pause",
        sessionId: "customer-call",
        expectedGeneration: 2,
      }),
    ).resolves.toMatchObject({
      ok: true,
      snapshot: { session: { status: "paused" } },
    });
    await expect(
      harness.experimental_call("updateNotepad", {
        operationId: "op-notepad",
        sessionId: "customer-call",
        expectedGeneration: 2,
        expectedNotepadRevision: "v1-fixture",
        text: "One\nTwo",
      }),
    ).resolves.toMatchObject({ ok: true });
    await expect(
      harness.experimental_call("stop", {
        operationId: "op-stop",
        sessionId: "customer-call",
        expectedGeneration: 2,
      }),
    ).resolves.toMatchObject({ ok: true, stopped_session_id: "customer-call" });

    expect(fakeTransport.start).toHaveBeenCalledWith(
      expect.objectContaining({ operationId: "op-start" }),
    );
    expect(harness.experimental_getSignals()).toEqual([
      { signal: "changed", payload: { reason: "start" } },
      { signal: "changed", payload: { reason: "pause" } },
      { signal: "changed", payload: { reason: "notepad" } },
      { signal: "changed", payload: { reason: "stop" } },
    ]);

    await harness.experimental_dispose();
  });
});
