// @vitest-environment jsdom
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { loadPluginApp, renderSlot } from "@get-bb/plugin-sdk/testing/app";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserCaptureOwner } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";

const app = await loadPluginApp(() => import("../app.js"));
const noWorkspaceMeeting = () => ({ ok: true as const, meeting: null, candidates: [] });
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
  it("registers a route-independent owner, persistent status, and one compact thread panel", () => {
    expect(app.contentScripts).toHaveLength(1);
    expect(app.appOverlays).toMatchObject([{ id: "recording-status" }]);
    expect(app.threadPanelActions).toMatchObject([{ id: "live", title: "Margins", layout: "flush" }]);
    expect(app.navPanels).toEqual([]);
    expect(app.messageActions).toEqual([]);
  });

  it("keeps Pause and Stop reachable after the recording panel unmounts", async () => {
    vi.spyOn(browserCaptureOwner, "threadId", "get").mockReturnValue("thr-owner");
    vi.spyOn(browserCaptureOwner, "active", "get").mockReturnValue(true);
    vi.spyOn(browserCaptureOwner, "panel").mockReturnValue(panel());
    const pause = vi.spyOn(browserCaptureOwner, "pause").mockResolvedValue(panel({ state: "paused", primaryAction: "resume" }));
    const overlay = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    const screen = within(overlay.container);
    fireEvent.click(screen.getByRole("button", { name: "Pause recording" }));
    await waitFor(() => expect(pause).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: /Recording/ }));
    expect(overlay.inspection.navigateCalls).toContainEqual({ method: "toThread", threadId: "thr-owner" });
    overlay.lifecycle.unmount();
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
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: { getPanelState: () => ready, readWorkspaceMeeting: noWorkspaceMeeting } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Start recording" }));
    expect(screen.getByRole("button", { name: "Start recording" }).textContent).toContain("Start recording");
    expect(screen.getByText("Getting recording ready")).toBeDefined();
    await waitFor(() => expect(browserCaptureOwner.start).toHaveBeenCalledWith("thr-1", undefined));
    expect(slot.container.textContent).not.toMatch(/daemon|helper|runtime|host worker|TCC|binary/i);
    slot.lifecycle.unmount();
  });

  it("keeps manual Mac pairing out of the ordinary browser recording flow", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh");
    const ready = panel({ state: "ready", title: "Record with your browser", primaryAction: "start",
      primaryLabel: "Use browser microphone", canStop: false, canEditNotepad: false,
      ownsRecording: false, recordingId: null, notepad: null });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => ready, readWorkspaceMeeting: noWorkspaceMeeting },
    });
    const screen = within(slot.container);
    await screen.findByRole("button", { name: "Use browser microphone" });
    const manual = slot.container.querySelector("details.margins-native-manual") as HTMLDetailsElement | null;
    expect(manual).not.toBeNull();
    expect(manual?.open).toBe(false);
    expect(manual?.textContent).toContain("Connect a Mac recorder manually");
    slot.lifecycle.unmount();
  });

  it("keeps the latest saved note visible while a new browser recording remains available", async () => {
    const ready = panel({ state: "ready", title: "Record with your browser", primaryAction: "start",
      primaryLabel: "Use browser microphone", canStop: false, canEditNotepad: false,
      ownsRecording: false, recordingId: null, notepad: null, lastSessionId: "older-session" });
    const meeting = { sessionId: "newest-session", title: "Planning", startedAt: "2026-09-24T03:00:00Z",
      inputFinalized: true, notepad: { text: "Saved BB note", revision: "rev-new" } };
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => ready,
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting }),
    } });
    const screen = within(slot.container);
    expect((await screen.findByRole("textbox", { name: "Workspace meeting notepad" }) as HTMLTextAreaElement).value).toBe("Saved BB note");
    expect(screen.getByText(/Latest saved meeting/)).toBeDefined();
    expect(slot.container.querySelector("time")?.getAttribute("datetime")).toBe(meeting.startedAt);
    expect(screen.getByRole("button", { name: "Use browser microphone" })).toBeDefined();
    expect(screen.getAllByRole("button", { name: "Make connected note" })).toHaveLength(1);
    slot.lifecycle.unmount();
  });

  it("puts a natural connected-note request in the composer without sending", async () => {
    const saved = panel({ state: "saved", title: "Meeting saved", primaryAction: "none", primaryLabel: "Meeting saved", canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null, lastSessionId: "rec-pinned" });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => saved,
      readWorkspaceMeeting: noWorkspaceMeeting,
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-pinned", title: "Pinned",
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 2 }, memo: { revision: "memo-1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "Pin exact session",
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

  it("requests remote transcription before drafting a note for an audio-only session", async () => {
    const saved = panel({ state: "saved", title: "Meeting saved", primaryAction: "none", primaryLabel: "Meeting saved", canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null, lastSessionId: "rec-pinned" });
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => saved,
      readWorkspaceMeeting: noWorkspaceMeeting,
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance-1", workspaceId: "workspace-1", sessionId: "rec-pinned", title: "Pinned",
        transcript: { available: false, terminal: false, live: false, updatedAtUnixMs: 0 }, memo: { revision: "memo-1", lineCount: 0 }, artifacts: [], noteAssociation: null, instructions: "Wait for transcription",
      } }),
      transcribePinnedSession: () => ({ ok: true, status: "queued", attempt: 1 }),
    } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make connected note" }));
    await waitFor(() => expect(slot.inspection.rpcCalls.some((call) => call.method === "transcribePinnedSession")).toBe(true));
    expect(slot.inspection.composer.text).toBe("");
    expect(screen.getByText(/Transcribing this meeting/)).toBeDefined();
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

  it("joins a menu-recorded Workspace meeting and saves its memo without starting browser capture", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh");
    const ready = panel({ state: "ready", title: "Ready to record", primaryAction: "start", primaryLabel: "Start recording",
      canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null });
    const meeting = { sessionId: "remote-menu-1", title: "Planning", startedAt: "2026-09-24T03:00:00Z",
      inputFinalized: false, notepad: { text: "First point", revision: "rev-1" } };
    const saveWorkspaceMemo = vi.fn(async () => ({ ok: true as const, candidates: [meeting.sessionId],
      meeting: { ...meeting, notepad: { text: "First point\nSecond point", revision: "rev-2" } } }));
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => ready,
      readWorkspaceMeeting: () => ({ ok: true, candidates: [meeting.sessionId], meeting }),
      saveWorkspaceMemo,
    } });
    const screen = within(slot.container);
    const note = await screen.findByRole("textbox", { name: "Workspace meeting notepad" });
    await waitFor(() => expect(slot.container.querySelector("details.margins-native-manual")).toBeNull());
    expect((note as HTMLTextAreaElement).value).toBe("First point");
    fireEvent.change(note, { target: { value: "First point\nSecond point" } });
    fireEvent.click(screen.getByRole("button", { name: "Save note" }));
    await waitFor(() => expect(saveWorkspaceMemo).toHaveBeenCalledWith({
      threadId: "thr-1", sessionId: "remote-menu-1", expectedRevision: "rev-1", text: "First point\nSecond point",
    }));
    expect(slot.inspection.rpcCalls.some((call) => call.method === "beginBrowserCapture")).toBe(false);
    slot.lifecycle.unmount();
  });

  it("drafts a connected note for the same saved menu session", async () => {
    const ready = panel({ state: "ready", title: "Ready to record", primaryAction: "start", primaryLabel: "Start recording",
      canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null });
    const meeting = { sessionId: "remote-menu-2", title: "Planning", startedAt: "2026-09-24T03:00:00Z",
      inputFinalized: true, notepad: { text: "Follow-up", revision: "rev-2" } };
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-1", params: null }, { rpc: {
      getPanelState: () => ready,
      readWorkspaceMeeting: () => ({ ok: true, candidates: [meeting.sessionId], meeting }),
      captureAuthority: () => ({ ok: true, instanceId: "instance", workspaceId: "workspace" }),
      pinNativeSession: () => ({ ok: true }),
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance", workspaceId: "workspace",
        sessionId: meeting.sessionId, title: meeting.title,
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 2 },
        memo: { revision: "rev-2", lineCount: 1 }, artifacts: [], noteAssociation: null,
        instructions: "Use this exact meeting",
      } }),
    } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make connected note" }));
    await waitFor(() => expect(slot.inspection.composer.text).toContain('"sessionId":"remote-menu-2"'));
    expect(slot.inspection.rpcCalls.some((call) => call.method === "pinNativeSession" &&
      (call.input as { sessionId?: string }).sessionId === "remote-menu-2")).toBe(true);
    slot.lifecycle.unmount();
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
