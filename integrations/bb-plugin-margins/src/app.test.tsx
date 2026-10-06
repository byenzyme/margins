// @vitest-environment jsdom
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { loadPluginApp, renderSlot } from "@get-bb/plugin-sdk/testing/app";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserCaptureOwner } from "./browser-capture.js";
import { nativeBridgeOwner, type NativeStatus } from "./native-bridge-client.js";
import type { PanelState } from "./contracts.js";

const app = await loadPluginApp(() => import("../app.js"));
function panel(changes: Partial<PanelState> = {}): PanelState {
  return {
    schema: "margins.bb.recording.panel.v2",
    state: "recording", title: "Recording", detail: "Microphone only. Audio and notes are being saved to this bb project.",
    sourceLabel: "Microphone only", storageLabel: "Saved to your Workspace",
    primaryAction: "pause", primaryLabel: "Pause", canStop: true, canEditNotepad: true,
    ownsRecording: true, recordingId: "rec-1", sessionId: "rec-1", notepad: { text: "Pricing", revision: "v1" },
    lastSessionId: null, error: null, ...changes,
  };
}

afterEach(() => { vi.restoreAllMocks(); sessionStorage.clear(); });

describe("Margins recording panel", () => {
  it("uses bridge duration for a 48 kHz mic and keeps old-helper sample fallback", () => {
    const status = { state: "recording", sessionId: "native-1", microphoneSamples: 528_000,
      microphoneDurationMs: 11_000, micPeak: 0.2 } as NativeStatus;
    const current = vi.spyOn(nativeBridgeOwner, "status", "get").mockReturnValue(status);
    const overlay = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    const screen = within(overlay.container);
    expect(screen.getByText("Recording · 0:11")).toBeTruthy();
    overlay.lifecycle.unmount();
    current.mockReturnValue({ ...status, microphoneDurationMs: undefined, microphoneSamples: 16_000 });
    const legacy = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    expect(within(legacy.container).getByText("Recording · 0:01")).toBeTruthy();
    legacy.lifecycle.unmount();
  });
  it("registers Meetings navigation, a sidebar level, persistent status, and one compact thread panel", () => {
    expect(app.contentScripts).toHaveLength(1);
    expect(app.appOverlays).toMatchObject([{ id: "recording-status" }]);
    expect(app.threadPanelActions).toMatchObject([{ id: "live", title: "Margins", layout: "flush" }]);
    expect(app.navPanels).toMatchObject([{ id: "meetings", title: "Meetings", icon: "Mic" }]);
    expect(app.navPanels[0]?.experimental_sidebarAccessory).toBeDefined();
    expect(app.settingsSections).toMatchObject([{ id: "recording" }]);
    expect(app.messageActions).toEqual([]);
  });

  it("shows the machine default and both Workspace destinations in settings", async () => {
    const projectWorkspace = vi.fn(() => ({ workspaceId: null }));
    const slot = renderSlot(app.settingsSections[0]!, {}, { rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      projectWorkspace,
      availableWorkspaces: () => ({ defaultWorkspaceId: "obsidian", resolvedWorkspaceId: "obsidian",
        autoSelected: false, workspaces: [{ id: "obsidian", name: "Obsidian" }] }),
      workspacePaths: () => ({ workspaceId: "obsidian", notes: "/vault/inbox", recordings: "/home/me/.margins/captures" }),
    } });
    const screen = within(slot.container);
    await screen.findByText("/vault/inbox");
    expect(screen.getByText("/home/me/.margins/captures")).toBeTruthy();
    expect(screen.getByRole("option", { name: "Machine default (Obsidian)" })).toBeTruthy();
    expect(projectWorkspace).toHaveBeenCalledWith({ projectId: "project-1" });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(projectWorkspace).toHaveBeenCalledWith({ projectId: "project-1", workspaceId: "" }));
    fireEvent.click(screen.getByRole("button", { name: "Edit program" }));
    expect(slot.inspection.navigateCalls).toContainEqual({ method: "toPluginPanel", path: "meetings", options: { subPath: "project-1/@program" } });
    slot.lifecycle.unmount();
  });

  it("offers known Workspaces in a picker when the machine has no default", async () => {
    const choose = vi.fn(() => ({ workspaceId: "vault" }));
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1" }, { rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: null, autoSelected: false,
        workspaces: [{ id: "vault", name: "Notes" }, { id: "other", name: "Other" }] }),
      getProjectPanelState: () => panel({ state: "unavailable", error: { code: "workspace_required", message: "Choose a Workspace", retryable: false } }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [] }),
      projectWorkspace: choose,
    } });
    const screen = within(slot.container);
    const picker = await screen.findByRole("combobox", { name: "Margins Workspace" });
    expect(within(picker).getAllByRole("option")).toHaveLength(2);
    expect(screen.queryByRole("textbox", { name: "Workspace id" })).toBeNull();
    fireEvent.change(picker, { target: { value: "other" } });
    fireEvent.click(screen.getByRole("button", { name: "Use Workspace" }));
    await waitFor(() => expect(choose).toHaveBeenCalledWith({ projectId: "project-1", workspaceId: "other" }));
    slot.lifecycle.unmount();
  });

  it("keeps Pause and Stop reachable after the recording panel unmounts", async () => {
    vi.spyOn(browserCaptureOwner, "recordingId", "get").mockReturnValue("rec-owner");
    vi.spyOn(browserCaptureOwner, "active", "get").mockReturnValue(true);
    vi.spyOn(browserCaptureOwner, "panel").mockReturnValue(panel());
    const pause = vi.spyOn(browserCaptureOwner, "pause").mockResolvedValue(panel({ state: "paused", primaryAction: "resume" }));
    const overlay = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    const screen = within(overlay.container);
    fireEvent.click(screen.getByRole("button", { name: "Pause recording" }));
    await waitFor(() => expect(pause).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: /Microphone only/ }));
    expect(overlay.inspection.navigateCalls).toContainEqual({ method: "toPluginPanel", path: "meetings", options: { subPath: "" } });
    overlay.lifecycle.unmount();
  });

  it("shows Saving immediately while a stop request is in flight", async () => {
    vi.spyOn(browserCaptureOwner, "recordingId", "get").mockReturnValue("rec-owner");
    vi.spyOn(browserCaptureOwner, "active", "get").mockReturnValue(true);
    vi.spyOn(browserCaptureOwner, "panel").mockReturnValue(panel());
    const stop = vi.spyOn(browserCaptureOwner, "stop").mockImplementation(() => new Promise(() => {}));
    const overlay = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    const screen = within(overlay.container);
    fireEvent.click(screen.getByRole("button", { name: "Stop and save recording" }));
    expect(stop).toHaveBeenCalledOnce();
    expect(screen.getByText("Saving recording…")).toBeDefined();
    expect(screen.queryByRole("button", { name: "Stop and save recording" })).toBeNull();
    overlay.lifecycle.unmount();
  });

  it("offers Finish with what was saved when the missing browser chunk cannot be replayed", async () => {
    vi.spyOn(browserCaptureOwner, "recordingId", "get").mockReturnValue("rec-owner");
    vi.spyOn(browserCaptureOwner, "hasPendingStop", "get").mockReturnValue(true);
    vi.spyOn(browserCaptureOwner, "canFinishIncomplete", "get").mockReturnValue(true);
    vi.spyOn(browserCaptureOwner, "panel").mockReturnValue(panel({ state: "needs_attention",
      primaryAction: "finish_incomplete", primaryLabel: "Finish with what was saved",
      error: { code: "browser_chunk_gap", message: "Audio sequence 1 was lost", retryable: false } }));
    const finish = vi.spyOn(browserCaptureOwner, "finishIncomplete").mockResolvedValue(panel({
      state: "saved", recordingId: null, ownsRecording: false, error: null,
    }));
    const overlay = renderSlot(app.appOverlays[0]!, {}, { context: { threadId: "thr-other" } });
    const screen = within(overlay.container);
    expect(screen.getByText("Audio sequence 1 was lost")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Finish with what was saved" }));
    await waitFor(() => expect(finish).toHaveBeenCalledOnce());
    overlay.lifecycle.unmount();
  });

  it("switches meeting memo pads without controlling the live capture", async () => {
    const live = { sessionId: "live-1", title: "Planning", startedAt: "2026-09-25T01:00:00Z", inputFinalized: false,
      notepad: { text: "Live memo", revision: "live-r1" }, notePath: null };
    const ended = { sessionId: "ended-1", title: "Review", startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notepad: { text: "Ended memo", revision: "ended-r1" }, notePath: null };
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1" }, { rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "recording", recordingId: "live-1" }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [live, ended] }),
      readWorkspaceMeeting: (input: unknown) => ({ ok: true, candidates: [], meeting: (input as { sessionId: string }).sessionId === live.sessionId ? live : ended }),
    } });
    const screen = within(slot.container);
    expect((await screen.findByRole("textbox", { name: "Meeting memo pad" }) as HTMLTextAreaElement).value).toBe("Live memo");
    expect(screen.getByText("Ready to refine")).toBeDefined();
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop and save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await waitFor(() => expect((screen.getByRole("textbox", { name: "Meeting memo pad" }) as HTMLTextAreaElement).value).toBe("Ended memo"));
    expect(slot.inspection.rpcCalls.some((call) => ["pauseBrowserCapture", "stopBrowserCapture"].includes(call.method))).toBe(false);
    slot.lifecycle.unmount();
  });

  it("starts a note thread for an ended meeting in its project", async () => {
    const ended = { sessionId: "ended-2", title: "Review", startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notepad: { text: "Decision", revision: "memo-v3" }, notePath: null, threadIds: [], distilledMemoRevision: null };
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1/ended-2" }, { context: { projectId: "project-1" }, rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "ready", recordingId: null }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [ended] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting: ended }),
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance", workspaceId: "vault", sessionId: "ended-2", title: "Review",
        transcript: { available: false, terminal: false, live: false, updatedAtUnixMs: 0 },
        memo: { revision: "memo-v3", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "",
      } }),
      transcribePinnedSession: () => ({ ok: true, status: "queued", attempt: 1 }),
      startConnectedNoteThread: () => ({ threadId: "thr-created" }),
    } });
    const screen = within(slot.container);
    const pad = await screen.findByRole("textbox", { name: "Meeting memo pad" });
    const makeNote = screen.getByRole("button", { name: "Make note →" });
    expect(makeNote.closest("footer")?.querySelector(".margins-meeting-trail")).not.toBeNull();
    expect(pad.compareDocumentPosition(makeNote) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(makeNote);
    await waitFor(() => expect(slot.inspection.navigateCalls.some((call) => call.method === "toThread" && call.threadId === "thr-created")).toBe(true));
    expect(slot.inspection.rpcCalls).toContainEqual(expect.objectContaining({ method: "startConnectedNoteThread",
      input: { projectId: "project-1", sessionId: "ended-2" } }));
    expect(slot.inspection.rpcCalls.some((call) => call.method === "transcribePinnedSession")).toBe(true);
    expect(slot.inspection.composer.text).toBe("");
    slot.lifecycle.unmount();
  });

  it("keeps a distilled memo editable and offers a new thread for a changed revision", async () => {
    const linked = { sessionId: "ended-linked", title: "Review", startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notepad: { text: "Original decision", revision: "memo-v1" }, notePath: "inbox/review.md", threadIds: ["thr-original"], distilledMemoRevision: "memo-v1" };
    const revised = { ...linked, notepad: { text: "Revised decision", revision: "memo-v2" } };
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1/ended-linked" }, { context: { projectId: "project-1" }, rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "ready", recordingId: null }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [linked] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting: linked }),
      saveWorkspaceMemo: () => ({ ok: true, candidates: [], meeting: revised }),
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance", workspaceId: "vault", sessionId: linked.sessionId, title: linked.title,
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 1 }, memo: { revision: "memo-v2", lineCount: 1 },
        artifacts: [], noteAssociation: { sourceId: "home", relativePath: linked.notePath, revision: 1 }, instructions: "",
      } }),
      startConnectedNoteThread: () => ({ threadId: "thr-update" }),
    } });
    const screen = within(slot.container);
    const pad = await screen.findByRole("textbox", { name: "Meeting memo pad" }) as HTMLTextAreaElement;
    expect(pad.readOnly).toBe(false);
    expect(screen.queryByRole("button", { name: "Update note →" })).toBeNull();
    fireEvent.change(pad, { target: { value: "Revised decision" } });
    fireEvent.click(await screen.findByRole("button", { name: "Update note →" }));
    await waitFor(() => expect(slot.inspection.navigateCalls.some((call) => call.method === "toThread" && call.threadId === "thr-update")).toBe(true));
    expect(slot.inspection.rpcCalls).toContainEqual(expect.objectContaining({ method: "saveWorkspaceMemo",
      input: expect.objectContaining({ expectedRevision: "memo-v1", text: "Revised decision" }) }));
    expect(slot.inspection.rpcCalls).toContainEqual(expect.objectContaining({ method: "startConnectedNoteThread",
      input: { projectId: "project-1", sessionId: "ended-linked" } }));
    slot.lifecycle.unmount();
  });

  it("opens distilled note and thread links by title beside the status", async () => {
    const notePath = "inbox/2026-09-25-quiet-meetings-with-accessibility.md";
    const noteFile = { hostId: "host-1", path: "/tmp/vault/inbox/2026-09-25-quiet-meetings-with-accessibility.md" };
    const linked = { sessionId: "ended-linked", title: null, startedAt: "2026-09-25T01:00:00Z", inputFinalized: true,
      notepad: { text: "Decision", revision: "memo-v1" }, notePath, noteFile,
      threadIds: ["thr-distilled"], threadLinks: [{ id: "thr-distilled", title: "Create connected meeting note" }], distilledMemoRevision: "memo-v1" };
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1/ended-linked" }, { context: { projectId: "project-1" }, openFilePreview: () => true, rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "ready", recordingId: null }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [linked] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting: linked }),
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance", workspaceId: "vault", sessionId: linked.sessionId, title: null,
        transcript: { available: true, terminal: true, live: false, updatedAtUnixMs: 1 }, memo: { revision: "memo-v1", lineCount: 1 },
        artifacts: [], noteAssociation: { sourceId: "home", relativePath: notePath, revision: 1 }, instructions: "",
      } }),
    } });
    const screen = within(slot.container);
    const links = await screen.findByRole("navigation", { name: "Meeting links" });
    fireEvent.click(within(links).getByRole("link", { name: "Quiet meetings with accessibility" }));
    fireEvent.click(within(links).getByRole("button", { name: "Create connected meeting note" }));
    expect(slot.inspection.navigateCalls).toContainEqual({ method: "experimental_openFilePreview",
      options: { target: { kind: "host", ...noteFile }, location: null } });
    expect(slot.inspection.navigateCalls).toContainEqual({ method: "toThread", threadId: "thr-distilled" });
    expect(slot.container.textContent).not.toContain("inbox/");
    expect(slot.container.textContent).not.toContain("thr-distilled");
    slot.lifecycle.unmount();
  });

  it("keeps a transcription failure on Meetings instead of putting its error in a composer draft", async () => {
    const ended = { sessionId: "ended-error", title: null, startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notepad: { text: "Decision", revision: "memo-v1" }, notePath: null, threadIds: [], distilledMemoRevision: null };
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1/ended-error" }, { context: { projectId: "project-1" }, rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "ready", recordingId: null }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [ended] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting: ended }),
      connectedNoteContext: () => ({ ok: true, context: {
        schema: "margins.bb.connected-note-context.v1", instanceId: "instance", workspaceId: "vault", sessionId: ended.sessionId, title: null,
        transcript: { available: false, terminal: false, live: false, updatedAtUnixMs: 0 },
        memo: { revision: "memo-v1", lineCount: 1 }, artifacts: [], noteAssociation: null, instructions: "",
      } }),
      transcribePinnedSession: () => ({ ok: false, error: { code: "transcription_unavailable", message: "upstream (503)", retryable: true } }),
    } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make note →" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("upstream (503)"));
    expect(screen.getByText("Transcript unavailable")).toBeDefined();
    expect(screen.getByRole("button", { name: "Retry" })).toBeDefined();
    expect(slot.inspection.navigateCalls.some((call) => call.method === "toCompose")).toBe(false);
    slot.lifecycle.unmount();
  });

  it("saves a later memo edit made while an earlier revision is in flight", async () => {
    const meeting = { sessionId: "ended-editing", title: "Review", startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notepad: { text: "Original", revision: "memo-v1" }, notePath: null, threadIds: [], distilledMemoRevision: null };
    let resolveFirst!: (result: unknown) => void;
    const firstSave = new Promise((resolve) => { resolveFirst = resolve; });
    const saveWorkspaceMemo = vi.fn().mockImplementationOnce(() => firstSave).mockImplementationOnce(() => ({
      ok: true, candidates: [], meeting: { ...meeting, notepad: { text: "Second edit", revision: "memo-v3" } },
    }));
    const slot = renderSlot(app.navPanels[0]!, { subPath: "project-1/ended-editing" }, { rpc: {
      availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }),
      availableWorkspaces: () => ({ defaultWorkspaceId: "vault", autoSelected: false, workspaces: [{ id: "vault", name: "Notes" }] }),
      getProjectPanelState: () => panel({ state: "ready", recordingId: null }),
      listWorkspaceMeetings: () => ({ ok: true, meetings: [meeting] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting }),
      saveWorkspaceMemo,
    } });
    const screen = within(slot.container);
    const pad = await screen.findByRole("textbox", { name: "Meeting memo pad" }) as HTMLTextAreaElement;
    fireEvent.change(pad, { target: { value: "First edit" } });
    fireEvent.blur(pad);
    await waitFor(() => expect(saveWorkspaceMemo).toHaveBeenCalledTimes(1));
    fireEvent.change(pad, { target: { value: "Second edit" } });
    resolveFirst({ ok: true, candidates: [], meeting: { ...meeting, notepad: { text: "First edit", revision: "memo-v2" } } });
    await waitFor(() => expect(saveWorkspaceMemo).toHaveBeenCalledTimes(2));
    expect(saveWorkspaceMemo).toHaveBeenLastCalledWith(expect.objectContaining({ expectedRevision: "memo-v2", text: "Second edit" }));
    await waitFor(() => expect((pad as HTMLTextAreaElement).value).toBe("Second edit"));
    slot.lifecycle.unmount();
  });

  it("shows the linked source memo in its distillation thread tab", async () => {
    const summary = { sessionId: "ended-2", title: "Review", startedAt: "2026-09-24T01:00:00Z", inputFinalized: true,
      notePath: "inbox/review.md", threadIds: ["thr-distill"], distilledMemoRevision: "memo-v3" };
    const slot = renderSlot(app.threadPanelActions[0]!, { threadId: "thr-distill", params: null }, { context: { projectId: "project-1" }, rpc: {
      listWorkspaceMeetings: () => ({ ok: true, meetings: [summary] }),
      readWorkspaceMeeting: () => ({ ok: true, candidates: [], meeting: { ...summary, notepad: { text: "Decision", revision: "memo-v3" } } }),
    } });
    const screen = within(slot.container);
    expect((await screen.findByRole("textbox", { name: "Source meeting memo pad" }) as HTMLTextAreaElement).value).toBe("Decision");
    expect(screen.getByText("Note: inbox/review.md")).toBeDefined();
    slot.lifecycle.unmount();
  });


});
