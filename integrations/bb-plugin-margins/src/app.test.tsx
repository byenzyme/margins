// @vitest-environment jsdom
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { loadPluginApp, renderSlot } from "@get-bb/plugin-sdk/testing/app";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserCaptureOwner } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";

const app = await loadPluginApp(() => import("../app.js"));
function panel(changes: Partial<PanelState> = {}): PanelState {
  return {
    schema: "margins.bb.recording.panel.v2",
    state: "recording", title: "Recording", detail: "Microphone only. Audio and notes are being saved to this bb project.",
    sourceLabel: "Microphone only", storageLabel: "Saved to this bb project",
    primaryAction: "pause", primaryLabel: "Pause", canStop: true, canEditNotepad: true,
    ownsRecording: true, recordingId: "rec-1", notepad: { text: "Pricing", revision: "v1" },
    lastSessionId: null, error: null, ...changes,
  };
}

afterEach(() => vi.restoreAllMocks());

describe("Margins recording panel", () => {
  it("registers a route-independent owner and only one compact thread panel", () => {
    expect(app.contentScripts).toHaveLength(1);
    expect(app.threadPanelActions).toMatchObject([{ id: "live", title: "Margins", layout: "flush" }]);
    expect(app.navPanels).toEqual([]);
    expect(app.messageActions).toEqual([]);
  });

  it("shows the truthful microphone source, project storage, and one editable notepad", async () => {
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: () => panel({ notepad: { text: "Pricing\nNext", revision: "v2" } }) },
    });
    const screen = within(slot.container);
    await screen.findByText("Recording");
    expect(screen.getByText("Microphone only")).toBeDefined();
    expect(screen.getByText("Saved to this bb project")).toBeDefined();
    expect(screen.queryByText(/live transcript/i)).toBeNull();
    const note = screen.getByRole("textbox", { name: "Meeting notepad" });
    fireEvent.change(note, { target: { value: "Pricing\nNext" } });
    await waitFor(() => expect(slot.inspection.rpcCalls.some((call) => call.method === "updateNotepad")).toBe(true));
    slot.lifecycle.unmount();
  });

  it("asks for microphone before Start resolves and never exposes setup internals", async () => {
    vi.spyOn(browserCaptureOwner, "start").mockResolvedValue(panel());
    const ready = panel({ state: "ready", title: "Ready to record", primaryAction: "start", primaryLabel: "Start recording", canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: { getPanelState: () => ready } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Start recording" }));
    expect(screen.getByRole("button", { name: "Start recording" }).textContent).toContain("Start recording");
    expect(screen.getByText("Getting recording ready")).toBeDefined();
    await waitFor(() => expect(browserCaptureOwner.start).toHaveBeenCalledWith("thr-1", undefined));
    expect(slot.container.textContent).not.toMatch(/daemon|helper|runtime|host worker|TCC|binary/i);
    slot.lifecycle.unmount();
  });

  it("puts a natural connected-note request in the composer without sending", async () => {
    const saved = panel({ state: "saved", title: "Meeting saved", primaryAction: "none", primaryLabel: "Meeting saved", canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null, lastSessionId: "rec-pinned" });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => saved,
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-pinned", title: "Pinned",
        transcript: { terminal: true, live: false, updatedAtUnixMs: 2 }, memo: { revision: "memo-1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "Pin exact session",
      } }),
    }, composer: { text: "Keep this draft." } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make connected note" }));
    await waitFor(() => expect(slot.inspection.composer.text).toContain('"sessionId":"rec-pinned"'));
    expect(slot.inspection.composer.text).toContain("Read and write note files only through the project's native filesystem Source");
    expect(slot.inspection.composer.focusCount).toBe(1);
    expect(slot.container.textContent).not.toContain("rec-pinned");
    slot.lifecycle.unmount();
  });

  it("keeps an unsaved notepad edit when the panel is collapsed and reopened", async () => {
    const failedUpdate = vi.fn(async () => { throw new Error("offline"); });
    const first = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: failedUpdate },
    });
    const firstScreen = within(first.container);
    const note = await firstScreen.findByRole("textbox", { name: "Meeting notepad" });
    fireEvent.change(note, { target: { value: "Pricing\nKeep this locally" } });
    await waitFor(() => expect(failedUpdate).toHaveBeenCalled());
    first.lifecycle.unmount();

    const reopened = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: () => panel() },
    });
    expect((await within(reopened.container).findByRole("textbox", { name: "Meeting notepad" }) as HTMLTextAreaElement).value).toBe("Pricing\nKeep this locally");
    reopened.lifecycle.unmount();
  });

  it("does not put Pause behind a stalled notepad save", async () => {
    const pause = vi.spyOn(browserCaptureOwner, "pause").mockResolvedValue(panel({ state: "paused" }));
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-pause", params: null }, {
      rpc: { getPanelState: () => panel({ recordingId: "rec-pause" }), updateNotepad: () => new Promise(() => {}) },
    });
    const screen = within(slot.container);
    fireEvent.change(await screen.findByRole("textbox", { name: "Meeting notepad" }), { target: { value: "unsaved" } });
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pause).toHaveBeenCalledOnce());
    slot.lifecycle.unmount();
  });

  it("does not put Stop behind a stalled notepad save", async () => {
    const stop = vi.spyOn(browserCaptureOwner, "stop").mockResolvedValue(panel({ state: "saved" }));
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-stop", params: null }, {
      rpc: { getPanelState: () => panel({ recordingId: "rec-stop" }), updateNotepad: () => new Promise(() => {}) },
    });
    const screen = within(slot.container);
    fireEvent.change(await screen.findByRole("textbox", { name: "Meeting notepad" }), { target: { value: "unsaved" } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and save" }));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    slot.lifecycle.unmount();
  });
});
