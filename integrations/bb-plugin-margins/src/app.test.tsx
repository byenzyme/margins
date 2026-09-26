// @vitest-environment jsdom
import { fireEvent, waitFor, within } from "@testing-library/dom";
import { loadPluginApp, renderSlot } from "@get-bb/plugin-sdk/testing/app";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserCaptureOwner } from "./browser-capture.js";
import type { PanelState } from "./contracts.js";

const app = await loadPluginApp(() => import("../app.js"));
const { MarginsPanel } = await import("./app.js");
const legacyPanel = { ...app.threadPanelActions[0]!, component: MarginsPanel };
const noWorkspaceMeeting = () => ({ ok: true as const, meeting: null, candidates: [] });
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
  it("registers Meetings navigation, a sidebar level, persistent status, and one compact thread panel", () => {
    expect(app.contentScripts).toHaveLength(1);
    expect(app.appOverlays).toMatchObject([{ id: "recording-status" }]);
    expect(app.threadPanelActions).toMatchObject([{ id: "live", title: "Margins", layout: "flush" }]);
    expect(app.navPanels).toMatchObject([{ id: "meetings", title: "Meetings", icon: "Mic" }]);
    expect(app.navPanels[0]?.experimental_sidebarAccessory).toBeDefined();
    expect(app.settingsSections).toMatchObject([{ id: "recording" }]);
    expect(app.messageActions).toEqual([]);
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

  it("prefills a new-thread composer for an ended meeting without sending", async () => {
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
    } });
    const screen = within(slot.container);
    fireEvent.click(await screen.findByRole("button", { name: "Make note →" }));
    await waitFor(() => expect(slot.inspection.navigateCalls.some((call) => call.method === "toCompose")).toBe(true));
    const compose = slot.inspection.navigateCalls.find((call) => call.method === "toCompose");
    expect(compose).toMatchObject({ options: { focusPrompt: true } });
    const prompt = (compose as { options: { initialPrompt: string } }).options.initialPrompt;
    expect(prompt).toMatch(/^Make a connected note from my Review meeting on [A-Za-z]+ \d+, \d{4}\.\n\n<margins-context-v1>\n/);
    const context = JSON.parse(prompt.match(/<margins-context-v1>\n(.+)\n<\/margins-context-v1>/)?.[1] || "null");
    expect(context).toEqual({ workspaceId: "vault", sessionId: "ended-2", memoRevision: "memo-v3",
      bbProjectId: "project-1", transcript: "pending", note: "create" });
    expect(slot.inspection.rpcCalls.some((call) => call.method === "transcribePinnedSession")).toBe(true);
    expect(slot.inspection.rpcCalls.some((call) => call.method === "threads.spawn")).toBe(false);
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
    } });
    const screen = within(slot.container);
    const pad = await screen.findByRole("textbox", { name: "Meeting memo pad" }) as HTMLTextAreaElement;
    expect(pad.readOnly).toBe(false);
    expect(screen.queryByRole("button", { name: "Update note →" })).toBeNull();
    fireEvent.change(pad, { target: { value: "Revised decision" } });
    fireEvent.click(await screen.findByRole("button", { name: "Update note →" }));
    await waitFor(() => expect(slot.inspection.navigateCalls.some((call) => call.method === "toCompose")).toBe(true));
    expect(slot.inspection.rpcCalls).toContainEqual(expect.objectContaining({ method: "saveWorkspaceMemo",
      input: expect.objectContaining({ expectedRevision: "memo-v1", text: "Revised decision" }) }));
    expect(JSON.stringify(slot.inspection.navigateCalls)).toContain("memo-v2");
    expect(JSON.stringify(slot.inspection.navigateCalls)).toContain("Update the connected note from my Review meeting");
    expect(JSON.stringify(slot.inspection.navigateCalls)).toContain('\\"note\\":\\"update\\"');
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

  it("shows the truthful microphone source, project storage, and one editable notepad", async () => {
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: () => panel({ notepad: { text: "Pricing\nNext", revision: "v2" } }) },
    });
    const screen = within(slot.container);
    await screen.findByText("Recording");
    expect(screen.getByText("Microphone only")).toBeDefined();
    expect(screen.getByText("Saved to your Workspace")).toBeDefined();
    expect(screen.queryByText(/live transcript/i)).toBeNull();
    const note = screen.getByRole("textbox", { name: "Meeting notepad" });
    fireEvent.change(note, { target: { value: "Pricing\nNext" } });
    await waitFor(() => expect(slot.inspection.rpcCalls.some((call) => call.method === "updateNotepad")).toBe(true));
    slot.lifecycle.unmount();
  });

  it("asks for microphone before Start resolves and never exposes setup internals", async () => {
    vi.spyOn(browserCaptureOwner, "start").mockResolvedValue(panel());
    const ready = panel({ state: "ready", title: "Ready to record", primaryAction: "start", primaryLabel: "Start recording", canStop: false, canEditNotepad: false, ownsRecording: false, recordingId: null, notepad: null });
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: { getPanelState: () => ready, readWorkspaceMeeting: noWorkspaceMeeting } });
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => ready, readWorkspaceMeeting: noWorkspaceMeeting },
    });
    const screen = within(slot.container);
    await screen.findByRole("button", { name: "Use browser microphone" });
    expect(slot.container.querySelector("details.margins-native-manual")).toBeNull();
    slot.lifecycle.unmount();
    const settings = renderSlot(app.settingsSections[0]!, {}, { rpc: { availableProjects: () => ({ projects: [{ id: "project-1", name: "Project" }] }) } });
    await waitFor(() => expect(settings.container.querySelector("details.margins-native-manual")).not.toBeNull());
    const manual = settings.container.querySelector("details.margins-native-manual") as HTMLDetailsElement;
    expect(manual.open).toBe(false);
    expect(manual.textContent).toContain("Connect a Mac recorder manually");
    settings.lifecycle.unmount();
  });

  it("keeps the latest saved note visible while a new browser recording remains available", async () => {
    const ready = panel({ state: "ready", title: "Record with your browser", primaryAction: "start",
      primaryLabel: "Use browser microphone", canStop: false, canEditNotepad: false,
      ownsRecording: false, recordingId: null, notepad: null, lastSessionId: "older-session" });
    const meeting = { sessionId: "newest-session", title: "Planning", startedAt: "2026-09-24T03:00:00Z",
      inputFinalized: true, notepad: { text: "Saved BB note", revision: "rev-new" } };
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: {
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
    const first = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, {
      rpc: { getPanelState: () => panel(), updateNotepad: failedUpdate },
    });
    const firstScreen = within(first.container);
    const note = await firstScreen.findByRole("textbox", { name: "Meeting notepad" });
    fireEvent.change(note, { target: { value: "Pricing\nKeep this locally" } });
    await waitFor(() => expect(failedUpdate).toHaveBeenCalled());
    first.lifecycle.unmount();

    const reopened = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-1", params: null }, { rpc: {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-pause", params: null }, {
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
    const slot = renderSlot(legacyPanel, { threadId: "thr-stop", params: null }, {
      rpc: { getPanelState: () => panel({ recordingId: "rec-stop" }), updateNotepad: () => new Promise(() => {}) },
    });
    const screen = within(slot.container);
    fireEvent.change(await screen.findByRole("textbox", { name: "Meeting notepad" }), { target: { value: "unsaved" } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and save" }));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    slot.lifecycle.unmount();
  });
});
