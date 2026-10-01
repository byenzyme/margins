// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MeetingsPage } from "./meetings-page.js";

const mocks = vi.hoisted(() => ({
  connectMenu: vi.fn(),
  verify: vi.fn(),
  control: vi.fn(),
  refreshNative: vi.fn(),
  navigate: vi.fn(),
  navigateThread: vi.fn(),
  meetings: [] as Array<{ sessionId: string; title: string; startedAt: string; inputFinalized: boolean;
    notePath: null; threadIds: string[]; distilledMemoRevision: null; archived?: boolean }>,
  native: { paired: false, status: null as { state: string; sessionId: string | null;
    microphoneSamples: number; micPeak: number } | null, connectionError: null as string | null },
  probeMenu: vi.fn(async () => false),
  startBrowser: vi.fn(),
  call: vi.fn(async (method: string, input?: { sessionId?: string; text?: string }) => {
    if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }] };
    if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], autoSelected: false };
    if (method === "getProjectPanelState") return { state: "ready", sessionId: null };
    if (method === "listWorkspaceMeetings") return { ok: true, meetings: [...mocks.meetings] };
    if (method === "readWorkspaceMeeting") {
      const item = mocks.meetings.find((meeting) => meeting.sessionId === input?.sessionId);
      return { ok: true, meeting: item && { ...item, notepad: { revision: "rev", text: "" } } };
    }
    if (method === "saveWorkspaceMemo") {
      const item = mocks.meetings.find((meeting) => meeting.sessionId === input?.sessionId);
      return { ok: true, meeting: item && { ...item, notepad: { revision: "rev2", text: input?.text || "" } } };
    }
    if (method === "captureAuthority") return { ok: true, instanceId: "one", workspaceId: "obsidian" };
    if (method === "connectedNoteContext") return { ok: true, context: { sessionId: input?.sessionId,
      workspaceId: "obsidian", memo: { revision: "rev" }, transcript: { available: true }, noteAssociation: null } };
    if (method === "startConnectedNoteThread") return { threadId: "thr-note" };
    if (method === "issueMenuGrant") return {
      serviceUrl: "https://jpham-server.getbb.app/api/v1/plugins/margins/http/menu/relay",
      token: "grant", workspaceId: "obsidian", workspaceName: "Obsidian", instanceId: "one", expiresAt: Date.now() + 60_000,
    };
    throw new Error(`Unexpected RPC ${method}`);
  }),
  rpc: null as unknown,
}));
mocks.rpc = { call: mocks.call };
const defaultCall = mocks.call.getMockImplementation()!;

vi.mock("@get-bb/plugin-sdk/app", () => ({
  experimental_FileLink: () => null,
  useBbContext: () => ({ projectId: "proj-mac" }),
  useBbNavigate: () => ({ toPluginPanel: mocks.navigate, toThread: mocks.navigateThread }),
  useComposer: () => ({ text: "", setText: vi.fn(), insertMention: vi.fn() }),
  useRealtime: () => undefined,
  useRpc: () => mocks.rpc,
}));
vi.mock("./browser-capture.js", () => ({
  browserCaptureOwner: { startFromProject: mocks.startBrowser },
  detectClientCapabilities: () => ({ clientId: "mac", platform: "macos", secureContext: true,
    browserMicrophone: true, nativeMacCapture: false }),
}));
vi.mock("./native-bridge-client.js", () => ({
  nativeBridgeOwner: Object.assign(mocks.native, { subscribe: () => () => undefined,
    probeMenu: mocks.probeMenu, connectMenu: mocks.connectMenu, verify: mocks.verify,
    control: mocks.control, refresh: mocks.refreshNative }),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  mocks.refreshNative.mockReset();
  mocks.call.mockImplementation(defaultCall);
  mocks.native.paired = false;
  mocks.native.status = null;
  mocks.native.connectionError = null;
  mocks.meetings = [];
  sessionStorage.clear();
});

describe("Meetings Mac recorder choice", () => {
  it("starts a note thread in the meeting's recorded project and opens it", async () => {
    mocks.meetings = [{ sessionId: "saved", title: "Customer call", startedAt: "2026-09-28T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null,
      originProjectId: "proj-origin" } as (typeof mocks.meetings)[number]];
    render(<MeetingsPage subPath="proj-mac/saved" />);
    fireEvent.click(await screen.findByRole("button", { name: "Make note →" }));
    await waitFor(() => expect(mocks.call).toHaveBeenCalledWith("startConnectedNoteThread",
      { projectId: "proj-origin", sessionId: "saved" }));
    expect(mocks.navigateThread).toHaveBeenCalledWith("thr-note");
  });
  it("does not start a second capture while a Menu meeting is already recording", async () => {
    mocks.native.paired = true;
    mocks.native.status = { state: "recording", sessionId: "menu-live", microphoneSamples: 16_000, micPeak: 0.2 };
    mocks.refreshNative.mockResolvedValue(mocks.native.status);
    mocks.meetings = [{ sessionId: "saved", title: "Earlier meeting", startedAt: "2026-09-28T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null }];
    render(<MeetingsPage subPath="proj-mac/saved" />);
    fireEvent.click(await screen.findByRole("button", { name: "Start" }));
    await screen.findByText("Margins Menu is already recording. Waiting for its meeting to appear here…");
    expect(mocks.control).not.toHaveBeenCalledWith("start");
    expect(screen.queryByText("New meeting")).toBeNull();
  });

  it("opens the memo for a meeting started in Margins Menu", async () => {
    mocks.native.paired = true;
    mocks.native.status = { state: "recording", sessionId: "menu-live", microphoneSamples: 16_000, micPeak: 0.2 };
    mocks.meetings = [{ sessionId: "menu-live", title: "Weekly review", startedAt: new Date().toISOString(),
      inputFinalized: false, notePath: null, threadIds: [], distilledMemoRevision: null }];
    render(<MeetingsPage subPath="proj-mac" />);
    await screen.findByText("Taking notes on the meeting already recording in Margins Menu.");
    expect(mocks.navigate).toHaveBeenCalledWith("meetings", { subPath: "proj-mac/menu-live" });
    expect(mocks.call).not.toHaveBeenCalledWith("recordMeetingOrigin", expect.anything());
  });

  it("renames, views transcript, archives, restores, and confirms permanent discard", async () => {
    const item = { sessionId: "complete", title: "Planning", startedAt: "2026-09-27T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null,
      durationMs: 77_000, audioSource: "Microphone + computer audio", workspaceName: "Obsidian",
      originProjectName: "Mac", archived: false };
    mocks.meetings = [item];
    const originalCall = mocks.call.getMockImplementation()!;
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; title?: string; archived?: boolean }) => {
      if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }] };
      if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], autoSelected: false };
      if (method === "getProjectPanelState") return { state: "ready", sessionId: null };
      if (method === "listWorkspaceMeetings") return { ok: true, meetings: [...mocks.meetings] };
      if (method === "readWorkspaceMeeting") return { ok: true, meeting: { ...mocks.meetings[0], notepad: { revision: "rev", text: "Decisions" } } };
      if (method === "connectedNoteContext") return { ok: true, context: { transcript: { available: true } } };
      if (method === "readWorkspaceTranscript") return { ok: true, body: "We chose the launch date." };
      if (method === "renameWorkspaceMeeting") { mocks.meetings[0]!.title = input!.title!; return { ok: true }; }
      if (method === "archiveWorkspaceMeeting") { mocks.meetings[0]!.archived = input!.archived!; return { ok: true }; }
      if (method === "discardWorkspaceMeeting") { mocks.meetings = []; return { ok: true }; }
      throw new Error(`Unexpected RPC ${method}`);
    }) as unknown as typeof originalCall);
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<MeetingsPage subPath="proj-mac/complete" />);
    await screen.findByDisplayValue("Decisions");
    expect(screen.getByText("Saved · 1:17 · Obsidian · from Mac")).toBeDefined();
    expect(screen.getByRole("button", { name: "Connect" })).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(mocks.call).toHaveBeenCalledWith("issueMenuGrant", expect.objectContaining({ projectId: "proj-mac" })));
    expect(mocks.startBrowser).not.toHaveBeenCalled();
    expect(screen.queryByText("Transcript ready")).toBeNull();
    expect(screen.queryByText("Choose project in composer")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Meeting title" }), { target: { value: "Launch review" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("heading", { name: "Launch review" });
    fireEvent.click(screen.getByRole("button", { name: "View transcript" }));
    await screen.findByText("We chose the launch date.");
    fireEvent.click(await screen.findByRole("button", { name: "More" }));
    fireEvent.click(screen.getByRole("button", { name: "Archive" }));
    await screen.findByRole("button", { name: "Archived (1)" });
    fireEvent.click(screen.getByRole("button", { name: "Archived (1)" }));
    fireEvent.click(screen.getByRole("button", { name: "Launch review" }));
    fireEvent.click(await screen.findByRole("button", { name: "More" }));
    fireEvent.click(screen.getByRole("button", { name: "Restore to recent" }));
    await screen.findByRole("heading", { name: "Launch review" });
    fireEvent.click(screen.getByRole("button", { name: "More" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard permanently…" }));
    await screen.findByRole("heading", { name: "No meetings yet" });
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining("Any linked note and bb thread remain"));
    confirm.mockRestore();
    mocks.call.mockImplementation(originalCall);
  });
  it("keeps opened memos visible when switching and shows a loading pad for uncached meetings", async () => {
    const first = { sessionId: "first", title: "First", startedAt: "2026-09-27T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null };
    const second = { ...first, sessionId: "second", title: "Second" };
    mocks.meetings = [first, second];
    const originalCall = mocks.call.getMockImplementation()!;
    let finishSecond!: () => void;
    mocks.call.mockImplementation(async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }] };
      if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], autoSelected: false };
      if (method === "getProjectPanelState") return { state: "ready", sessionId: null };
      if (method === "listWorkspaceMeetings") return { ok: true, meetings: [first, second] };
      if (method === "readWorkspaceMeeting") {
        if (input?.sessionId === "second") return new Promise((resolve) => {
          finishSecond = () => resolve({ ok: true, meeting: { ...second, notepad: { revision: "rev", text: "Second memo" } } });
        });
        return { ok: true, meeting: { ...first, notepad: { revision: "rev", text: "First memo" } } };
      }
      throw new Error(`Unexpected RPC ${method}`);
    });
    const view = render(<MeetingsPage subPath="proj-mac/first" />);
    await screen.findByDisplayValue("First memo");
    fireEvent.click(screen.getByRole("button", { name: "Second" }));
    await screen.findByRole("heading", { name: "Second" });
    expect(screen.getByText("Opening memo…")).toBeDefined();
    expect(screen.queryByText("No meetings yet")).toBeNull();
    finishSecond();
    await screen.findByDisplayValue("Second memo");
    fireEvent.click(screen.getByRole("button", { name: "First" }));
    await screen.findByDisplayValue("First memo");
    view.unmount();
    const reopened = render(<MeetingsPage subPath="proj-mac/first" />);
    expect(screen.getByDisplayValue("First memo")).toBeDefined();
    reopened.unmount();
    mocks.call.mockImplementation(originalCall);
  });
  it("keeps Connect visible and does not silently choose the browser mic after a loopback failure", async () => {
    mocks.connectMenu.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    const confirm = vi.spyOn(window, "confirm");
    const view = render(<MeetingsPage subPath="proj-mac" />);
    await screen.findByRole("button", { name: "Connect" });
    fireEvent.click(screen.getByRole("button", { name: "Start meeting" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("allow bb to access local devices"));
    expect(mocks.connectMenu).toHaveBeenCalledOnce();
    expect(mocks.startBrowser).not.toHaveBeenCalled();
    expect(confirm).not.toHaveBeenCalled();
    view.unmount();
    confirm.mockRestore();
  });

  it("renews a stale Menu pairing before starting, without switching to the browser mic", async () => {
    mocks.native.paired = true;
    mocks.native.connectionError = "Failed to fetch";
    mocks.verify.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    const view = render(<MeetingsPage subPath="proj-mac" />);
    await screen.findByRole("button", { name: "Reconnect" });
    fireEvent.click(screen.getByRole("button", { name: "Start meeting" }));
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith("start"));
    expect(mocks.connectMenu).toHaveBeenCalledOnce();
    expect(mocks.verify).toHaveBeenCalledTimes(2);
    expect(mocks.startBrowser).not.toHaveBeenCalled();
    view.unmount();
  });

  it("opens an editable pad immediately and binds it before native audio is ready", async () => {
    const oldMeeting = { sessionId: "old", title: "Earlier meeting", startedAt: "2026-09-27T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null };
    const newMeeting = { ...oldMeeting, sessionId: "new", title: "New meeting", startedAt: new Date().toISOString(), inputFinalized: false };
    mocks.meetings = [oldMeeting];
    mocks.native.paired = true;
    mocks.refreshNative.mockImplementation(async () => mocks.native.status);
    let finishVerify!: () => void;
    mocks.verify.mockImplementationOnce(() => new Promise<void>((resolve) => { finishVerify = resolve; }));
    const view = render(<MeetingsPage subPath="proj-mac/old" />);
    await screen.findByRole("heading", { name: "Earlier meeting" });
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await screen.findByRole("heading", { name: "New meeting" });
    fireEvent.change(screen.getByRole("textbox", { name: "Meeting memo pad" }), { target: { value: "Remember the decision" } });
    expect(mocks.control).not.toHaveBeenCalled();
    finishVerify();
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith("start"));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("heading", { name: "Earlier meeting" })).toBeNull();
    mocks.meetings = [newMeeting, oldMeeting];
    mocks.native.status = { state: "getting_ready", sessionId: null, microphoneSamples: 0, micPeak: 0 };
    await screen.findByRole("heading", { name: "New meeting" });
    await waitFor(() => expect(mocks.navigate).toHaveBeenCalledWith("meetings", { subPath: "proj-mac/new" }));
    await waitFor(() => expect(mocks.call).toHaveBeenCalledWith("saveWorkspaceMemo", expect.objectContaining({
      sessionId: "new", expectedRevision: "rev", text: "Remember the decision",
    })));
    expect(screen.getByRole("textbox", { name: "Meeting memo pad" })).toHaveProperty("value", "Remember the decision");
    expect(mocks.navigate).toHaveBeenCalledWith("meetings", { subPath: "proj-mac/new" });
    view.unmount();
  });

  it("keeps the unsent memo visible when native Start fails", async () => {
    mocks.native.paired = true;
    let failStart!: () => void;
    mocks.control.mockImplementationOnce(() => new Promise<void>((_resolve, reject) => {
      failStart = () => reject(new Error("Microphone unavailable"));
    }));
    const view = render(<MeetingsPage subPath="proj-mac" />);
    fireEvent.click(await screen.findByRole("button", { name: "Start meeting" }));
    const pad = await screen.findByRole("textbox", { name: "Meeting memo pad" });
    fireEvent.change(pad, { target: { value: "Keep this thought" } });
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith("start"));
    failStart();
    await screen.findByRole("alert", { name: "" });
    expect(screen.getByRole("textbox", { name: "Meeting memo pad" })).toHaveProperty("value", "Keep this thought");
    expect(screen.getByRole("button", { name: "Retry Start" })).toBeDefined();
    view.unmount();
  });
});
