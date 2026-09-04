// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { loadPluginApp, renderSlot } from "@get-bb/plugin-sdk/testing/app";
import { desktopSnapshot } from "./fixtures.js";
import type { PanelState } from "./contracts.js";

const app = await loadPluginApp(() => import("../app.js"));

function panel(state: Partial<PanelState> = {}): PanelState {
  return {
    schema: "margins.bb.live.panel.v1",
    threadId: "thr-1",
    state: "recording",
    title: "Listening",
    detail: "Margins is listening to this meeting.",
    primaryAction: "pause",
    primaryLabel: "Pause",
    canEditNotepad: true,
    canStop: true,
    canDetach: true,
    firstUse: false,
    preparationNeeded: false,
    installUrl: "https://github.com/byenzyme/margins/releases",
    host: { id: "host-1", name: "Recording Mac", status: "connected" },
    attachment: {
      threadId: "thr-1",
      hostId: "host-1",
      meetingId: "customer-call",
      attachedAtUnixMs: 1_800_000_000_000,
      detachedAtUnixMs: null,
      generation: 2,
    },
    savedMeeting: null,
    snapshot: desktopSnapshot("recording"),
    error: null,
    mention: { available: true, itemId: "mention-id" },
    ...state,
  } as PanelState;
}

describe("Margins app panel", () => {
  it("registers only the compact thread panel action", () => {
    expect(app.threadPanelActions).toMatchObject([
      {
        id: "margins-live",
        title: "Margins live",
        layout: "flush",
      },
    ]);
    expect(app.navPanels).toEqual([]);
    expect(app.messageActions).toEqual([]);
  });

  it("shows recording, the notepad, and the @Margins handoff without system details", async () => {
    const slot = renderSlot(
      app.threadPanelActions[0]!,
      { threadId: "thr-1", params: null },
      {
        rpc: {
          getPanelState: () => panel(),
          updateNotepad: () => panel(),
          pauseMeeting: () => panel({ state: "paused", primaryLabel: "Resume" }),
        },
      },
    );

    const screen = within(slot.container);
    await screen.findByText("Listening");
    expect(slot.container.querySelector('.margins-waveform[data-state="recording"]')).not.toBeNull();
    expect(slot.container.querySelector(".margins-recorder-row")).not.toBeNull();
    expect(
      screen.getByRole("button", { name: "Pause" }).classList.contains("margins-icon-control"),
    ).toBe(true);
    expect(
      screen
        .getByRole("button", { name: "Stop recording" })
        .classList.contains("margins-icon-control"),
    ).toBe(true);
    expect(screen.getByDisplayValue("Follow up on pricing")).toBeDefined();
    expect(screen.queryByText(/Sam: hello/)).toBeNull();
    expect(screen.queryByText("Host")).toBeNull();
    expect(screen.queryByText("Time")).toBeNull();
    expect(screen.queryByText("Live transcript")).toBeNull();
    expect(screen.queryByRole("button", { name: "Add note" })).toBeNull();
    expect(slot.container.querySelectorAll(".margins-notepad-editor")).toHaveLength(1);

    fireEvent.click(screen.getByRole("button", { name: "Add @Margins to message" }));
    await waitFor(() => {
      expect(slot.inspection.composer.mentions).toEqual([
        { provider: "margins", id: "mention-id", label: "@Margins" },
      ]);
    });

    const notepad = screen.getByPlaceholderText("Jot down what matters…");
    fireEvent.change(notepad, {
      target: { value: "Follow up on pricing\nSend the revised deck" },
    });
    fireEvent.blur(notepad);
    await waitFor(() => {
      expect(slot.inspection.rpcCalls).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ method: "updateNotepad" }),
        ]),
      );
    });

    expect(slot.inspection.rpcCalls).toEqual(
      expect.arrayContaining([
        {
          method: "updateNotepad",
          input: {
            threadId: "thr-1",
            expectedNotepadRevision: "v1-fixture",
            text: "Follow up on pricing\nSend the revised deck",
          },
        },
      ]),
    );
    slot.lifecycle.unmount();
  });

  it("acknowledges one Start recording intent immediately and shows first-use privacy", async () => {
    const ready = panel({
      state: "ready",
      title: "Ready",
      detail: "Nothing is recorded until you start.",
      primaryAction: "start",
      primaryLabel: "Start recording",
      canEditNotepad: false,
      canStop: false,
      canDetach: false,
      firstUse: true,
      preparationNeeded: true,
      attachment: null,
      snapshot: desktopSnapshot("idle"),
      mention: { available: false, itemId: null },
    });
    let finishStart!: (value: PanelState) => void;
    const startResult = new Promise<PanelState>((resolve) => {
      finishStart = resolve;
    });
    const slot = renderSlot(
      app.threadPanelActions[0]!,
      { threadId: "thr-1", params: null },
      {
        rpc: {
          getPanelState: () => ready,
          startMeeting: () => startResult,
        },
      },
    );

    const screen = within(slot.container);
    await screen.findByText("Nothing is recorded until you start.");
    expect(screen.getByText("Audio and transcript stay on this Mac.")).toBeDefined();
    expect(
      screen.getByText("Meeting context enters the thread only when you add @Margins."),
    ).toBeDefined();
    expect(screen.getByText("First use prepares a private recorder on this Mac.")).toBeDefined();
    expect(
      (screen.getByPlaceholderText("Start recording to use the notepad") as HTMLTextAreaElement)
        .disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => expect(screen.getAllByText("Preparing recording")).toHaveLength(2));
    expect(slot.inspection.rpcCalls).toContainEqual({
      method: "startMeeting",
      input: { threadId: "thr-1" },
    });
    expect(slot.inspection.rpcCalls.filter((call) => call.method === "startMeeting")).toHaveLength(1);
    expect(screen.queryByText("Install Margins")).toBeNull();
    expect(screen.queryByText("Start Margins")).toBeNull();
    finishStart(panel());
    await screen.findByText("Listening");
    expect((screen.getByPlaceholderText("Jot down what matters…") as HTMLTextAreaElement).disabled).toBe(false);
    slot.lifecycle.unmount();
  });

  it("keeps polling while the runtime is preparing without offering live actions", async () => {
    const preparing = panel({
      state: "preparing",
      title: "Preparing recording",
      detail: "Margins is starting the private recorder on this Mac.",
      primaryAction: "none",
      primaryLabel: "Preparing recording",
      canEditNotepad: false,
      canStop: false,
      mention: { available: false, itemId: null },
      snapshot: desktopSnapshot("starting"),
    });
    let reads = 0;
    const slot = renderSlot(
      app.threadPanelActions[0]!,
      { threadId: "thr-1", params: null },
      {
        rpc: {
          getPanelState: () => {
            reads += 1;
            return reads === 1 ? preparing : panel();
          },
        },
      },
    );

    const screen = within(slot.container);
    await screen.findByText("Preparing recording");
    expect(screen.queryByRole("button", { name: "Start recording" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop recording" })).toBeNull();
    expect((screen.getByRole("button", { name: "Add @Margins to message" }) as HTMLButtonElement).disabled).toBe(true);

    await waitFor(() => expect(screen.getByText("Listening")).toBeDefined(), { timeout: 4_500 });
    expect(reads).toBeGreaterThanOrEqual(2);
    slot.lifecycle.unmount();
  });

  it("shows unsupported capture as guidance without a retry control", async () => {
    const unsupported = panel({
      state: "unsupported_platform",
      title: "Recording is not supported here",
      detail:
        "Recording could not start. No recording was created. Open this thread on an Apple silicon Mac.",
      primaryAction: "none",
      primaryLabel: "Recording unavailable",
      canEditNotepad: false,
      canStop: false,
      canDetach: false,
      attachment: null,
      snapshot: null,
      mention: { available: false, itemId: null },
    });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => unsupported },
    });
    const screen = within(slot.container);
    await screen.findByText("Recording is not supported here");
    expect(screen.getByText(/Open this thread on an Apple silicon Mac/)).toBeDefined();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Recording unavailable" })).toBeNull();
    slot.lifecycle.unmount();
  });

  it("flushes the latest notepad text before Pause", async () => {
    const saved = panel({
      snapshot: {
        ...desktopSnapshot("recording"),
        memo_lines: [{ index: 0, at_ms: 12_345, text: "A final sentence" }],
        notepad_revision: "v2-fixture",
      },
    });
    let finishSave!: (value: PanelState) => void;
    const saveResult = new Promise<PanelState>((resolve) => {
      finishSave = resolve;
    });
    const slot = renderSlot(
      app.threadPanelActions[0]!,
      { threadId: "thr-1", params: null },
      {
        rpc: {
          getPanelState: () => panel(),
          updateNotepad: () => saveResult,
          pauseMeeting: () =>
            panel({ state: "paused", primaryAction: "resume", primaryLabel: "Resume" }),
        },
      },
    );

    const screen = within(slot.container);
    const editor = await screen.findByPlaceholderText("Jot down what matters…");
    fireEvent.change(editor, { target: { value: "A final sentence" } });
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() =>
      expect(slot.inspection.rpcCalls.map((call) => call.method)).toEqual([
        "getPanelState",
        "updateNotepad",
      ]),
    );
    finishSave(saved);
    await waitFor(() => {
      expect(slot.inspection.rpcCalls.map((call) => call.method)).toEqual([
        "getPanelState",
        "updateNotepad",
        "pauseMeeting",
      ]);
    });
    slot.lifecycle.unmount();
  });

  it("flushes the latest notepad text before Stop", async () => {
    const saved = panel({
      snapshot: {
        ...desktopSnapshot("recording"),
        memo_lines: [{ index: 0, at_ms: 12_345, text: "Last thought" }],
        notepad_revision: "v2-fixture",
      },
    });
    const stopped = panel({
      state: "meeting_saved",
      title: "Meeting saved on this Mac",
      detail: "Your recording and notes are safe.",
      primaryAction: "none",
      primaryLabel: "Meeting saved",
      canEditNotepad: false,
      canStop: false,
      canDetach: false,
      attachment: null,
      savedMeeting: {
        threadId: "thr-1",
        hostId: "host-1",
        meetingId: "customer-call",
        savedAtUnixMs: Date.now(),
      },
      snapshot: null,
      mention: { available: false, itemId: null },
    });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: {
        getPanelState: () => panel(),
        updateNotepad: () => saved,
        stopMeeting: () => stopped,
      },
    });
    const screen = within(slot.container);
    fireEvent.change(await screen.findByPlaceholderText("Jot down what matters…"), {
      target: { value: "Last thought" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Stop recording" }));
    await screen.findByText("Meeting saved on this Mac");
    expect(slot.inspection.rpcCalls.map((call) => call.method)).toEqual([
      "getPanelState",
      "updateNotepad",
      "stopMeeting",
    ]);
    slot.lifecycle.unmount();
  });

  it("flushes before adding live @Margins context", async () => {
    const saved = panel({
      snapshot: {
        ...desktopSnapshot("recording"),
        memo_lines: [{ index: 0, at_ms: 12_345, text: "Use this context" }],
        notepad_revision: "v2-fixture",
      },
    });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: () => saved },
    });
    const screen = within(slot.container);
    fireEvent.change(await screen.findByPlaceholderText("Jot down what matters…"), {
      target: { value: "Use this context" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add @Margins to message" }));
    await waitFor(() => expect(slot.inspection.composer.mentions).toHaveLength(1));
    expect(slot.inspection.rpcCalls.map((call) => call.method)).toEqual([
      "getPanelState",
      "updateNotepad",
    ]);
    slot.lifecycle.unmount();
  });

  it("turns a saved meeting into a focused composer draft without sending", async () => {
    const saved = panel({
      state: "meeting_saved",
      title: "Meeting saved on this Mac",
      detail:
        "Your recording and notes are safe. Margins can turn them into a connected note when you are ready.",
      primaryAction: "none",
      primaryLabel: "Meeting saved",
      canEditNotepad: false,
      canStop: false,
      canDetach: false,
      attachment: null,
      savedMeeting: {
        threadId: "thr-1",
        hostId: "host-1",
        meetingId: "customer-call",
        savedAtUnixMs: Date.now(),
      },
      snapshot: null,
      mention: { available: false, itemId: null },
    });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => saved, dismissSavedMeeting: () => panel({ state: "ready" }) },
    });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make connected note" }));
    expect(slot.inspection.composer.text).toBe(
      "Turn the Margins meeting I just recorded into a connected note.",
    );
    expect(slot.inspection.composer.text).not.toContain("customer-call");
    expect(slot.inspection.composer.focusCount).toBeGreaterThan(0);
    expect(slot.inspection.rpcCalls).toEqual([{ method: "getPanelState", input: { threadId: "thr-1" } }]);
    expect(screen.queryByRole("button", { name: "Add @Margins to message" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() =>
      expect(slot.inspection.rpcCalls).toContainEqual({
        method: "dismissSavedMeeting",
        input: { threadId: "thr-1" },
      }),
    );
    slot.lifecycle.unmount();
  });
});
