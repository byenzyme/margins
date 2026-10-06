// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MeetingsPage, rememberStopAck } from "./meetings-page.js";

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
  discardRetainedAudio: vi.fn(),
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
  realtime: null as null | (() => void),
}));
mocks.rpc = { call: mocks.call };
const defaultCall = mocks.call.getMockImplementation()!;

vi.mock("@get-bb/plugin-sdk/app", () => ({
  experimental_Diff: undefined,
  experimental_FileLink: () => null,
  experimental_useCodeTheme: undefined,
  useBbContext: () => ({ projectId: "proj-mac" }),
  useBbNavigate: () => ({ toPluginPanel: mocks.navigate, toThread: mocks.navigateThread }),
  useComposer: () => ({ text: "", setText: vi.fn(), insertMention: vi.fn() }),
  useRealtime: (_channel: string, callback: () => void) => { mocks.realtime = callback; },
  useRpc: () => mocks.rpc,
}));
vi.mock("./browser-capture.js", () => ({
  browserCaptureOwner: { startFromProject: mocks.startBrowser, discardRetainedAudio: mocks.discardRetainedAudio, panel: () => null,
    subscribe: () => () => undefined, hasPendingStop: false, recordingId: null },
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
  mocks.navigate.mockReset();
  mocks.call.mockImplementation(defaultCall);
  mocks.native.paired = false;
  mocks.native.status = null;
  mocks.native.connectionError = null;
  mocks.meetings = [];
  sessionStorage.clear();
});

describe("Meetings Mac recorder choice", () => {
  it("shows the saved duration in the stop acknowledgment", async () => {
    mocks.meetings = [{ sessionId: "native-saved", title: "Call", startedAt: "2026-09-28T00:00:00Z",
      inputFinalized: true, durationMs: 11_000, notePath: null, threadIds: [], distilledMemoRevision: null }] as never;
    rememberStopAck("native-saved", "0:27");
    render(<MeetingsPage subPath="proj-mac/native-saved" />);
    expect(await screen.findByText("Saved · 0:11 recorded")).toBeTruthy();
  });
  it("labels a saved recording and its transcript when audio ranges are missing", async () => {
    mocks.meetings = [{ sessionId: "partial", title: "Partial call", startedAt: "2026-09-28T00:00:00Z",
      inputFinalized: true, captureIncomplete: true, captureGaps: [{ segmentId: "browser-000000",
        startSequence: 1, endExclusive: 3, reason: "upload_missing" }, { segmentId: "browser-000001",
        startSequence: 0, endExclusive: 0, reason: "browser_reload" }],
      notePath: null, threadIds: [], distilledMemoRevision: null }] as never;
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string }) => {
      if (method === "readWorkspaceTranscript") return { ok: true, body: "[00:00] First saved words" };
      return defaultCall(method, input);
    }) as typeof mocks.call);
    render(<MeetingsPage subPath="proj-mac/partial" />);
    expect(await screen.findByText(/Audio gaps: browser-000000: 1–2/)).toBeTruthy();
    expect(screen.getByText(/browser-000001: capture interrupted/)).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "View transcript" }));
    expect(await screen.findByText("Incomplete transcript: some recorded audio is missing.")).toBeTruthy();
  });
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
    expect(mocks.discardRetainedAudio).toHaveBeenCalledWith("complete");
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

  it("tells the user not to speak until the Mac recorder has captured audio", async () => {
    const native = mocks.native as typeof mocks.native & { subscribe: (listener: () => void) => () => void };
    const subscribe = native.subscribe;
    const listeners: Array<() => void> = [];
    native.subscribe = (listener) => { listeners.push(listener); return () => undefined; };
    const publish = (status: NonNullable<typeof mocks.native.status>) => {
      mocks.native.status = status;
      act(() => listeners.forEach((listener) => listener()));
    };
    try {
      mocks.native.paired = true;
      mocks.native.status = { state: "ready", sessionId: null, microphoneSamples: 0, micPeak: 0 };
      mocks.refreshNative.mockImplementation(async () => mocks.native.status);
      const view = render(<MeetingsPage subPath="proj-mac" />);
      fireEvent.click(await screen.findByRole("button", { name: "Start meeting" }));
      await waitFor(() => expect(mocks.control).toHaveBeenCalledWith("start"));
      publish({ state: "getting_ready", sessionId: null, microphoneSamples: 0, micPeak: 0 });
      expect(screen.getByRole("status").textContent).toBe("Starting… don't speak yet");
      // Devices are open and buffering while the session is still being reserved.
      publish({ state: "recording", sessionId: null, microphoneSamples: 480, micPeak: 0.1 });
      expect(screen.getByRole("status").textContent).toBe("Recording");
      view.unmount();
    } finally { native.subscribe = subscribe; }
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
  it("opens the Workspace program from the Meetings sidebar and from a deep link", async () => {
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], resolvedWorkspaceId: "obsidian", autoSelected: false };
      if (method === "workspaceProgram") return { workspaceId: "obsidian", workspaceName: "Obsidian", programPath: "/m/configs/obsidian.enzyme", revision: "r1", program: 'workspace "obsidian" {}\n' };
      if (method === "planWorkspaceProgram") return { ok: true, previewId: "p", workspaceId: "obsidian", baseRevision: "r1", noop: true, actions: [], diff: "" };
      return defaultCall(method, input);
    }) as unknown as typeof defaultCall);
    const view = render(<MeetingsPage subPath="proj-mac" />);
    fireEvent.click(await screen.findByRole("button", { name: "Workspace program" }));
    expect(await screen.findByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    expect(mocks.navigate).toHaveBeenLastCalledWith("meetings", { subPath: "proj-mac/@program" });
    fireEvent.click(screen.getByRole("button", { name: "Close program editor" }));
    await waitFor(() => expect(screen.queryByTitle("/m/configs/obsidian.enzyme")).toBeNull());
    view.unmount();

    render(<MeetingsPage subPath="proj-mac/@program" />);
    expect(await screen.findByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    expect(mocks.call.mock.calls.some(([method, input]) => method === "readWorkspaceMeeting" && (input as { sessionId?: string })?.sessionId === "@program")).toBe(false);
  });

  it("offers Edit program in the setup result", async () => {
    let setUp = false;
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "availableWorkspaces") return setUp
        ? { workspaces: [{ id: "notes", name: "notes" }], resolvedWorkspaceId: "notes", autoSelected: false }
        : { workspaces: [], resolvedWorkspaceId: null, autoSelected: false };
      if (method === "getProjectPanelState") return setUp ? { state: "ready", sessionId: null } : { state: "unavailable", sessionId: null };
      if (method === "previewWorkspaceSetup") return { previewId: "p", workspaceId: "notes", homeRoot: "/notes", destination: "/notes/Meetings",
        programPath: "/m/configs/notes.enzyme", readings: ["folder:People"], skippedReadings: [], actions: [] };
      if (method === "applyWorkspaceSetup") { setUp = true; return { workspaceId: "notes", destination: "/notes/Meetings" }; }
      if (method === "speechSetup") return null;
      if (method === "workspaceProgram") return { workspaceId: "notes", workspaceName: "notes", programPath: "/m/configs/notes.enzyme", revision: "r1", program: 'workspace "notes" {}\n' };
      if (method === "planWorkspaceProgram") return { ok: true, previewId: "p", workspaceId: "notes", baseRevision: "r1", noop: true, actions: [], diff: "" };
      return defaultCall(method, input);
    }) as unknown as typeof defaultCall);
    render(<MeetingsPage subPath="proj-mac" />);
    fireEvent.click(await screen.findByRole("button", { name: "Continue →" }));
    fireEvent.click(await screen.findByRole("button", { name: "Use this Workspace" }));
    fireEvent.click(await screen.findByRole("button", { name: "Edit program" }));
    expect(await screen.findByLabelText("Workspace program", { selector: "textarea" })).toHaveProperty("value", 'workspace "notes" {}\n');
  });
  const programCalls = (async (method: string, input?: { sessionId?: string; text?: string }) => {
    if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], resolvedWorkspaceId: "obsidian", autoSelected: false };
    if (method === "workspaceProgram") return { workspaceId: "obsidian", workspaceName: "Obsidian", programPath: "/m/configs/obsidian.enzyme", revision: "r1", program: 'workspace "obsidian" {}\n' };
    if (method === "planWorkspaceProgram") return { ok: true, previewId: "p", workspaceId: "obsidian", baseRevision: "r1", noop: true, actions: [], diff: "" };
    return defaultCall(method, input);
  }) as unknown as typeof defaultCall;
  /** Mirrors bb: navigation re-renders the panel with each segment percent-encoded. */
  function renderLikeBb(initial: string) {
    const encode = (subPath: string) => subPath.split("/").map(encodeURIComponent).join("/");
    const view = render(<MeetingsPage subPath={encode(initial)} />);
    mocks.navigate.mockImplementation((_panel: string, options: { subPath: string }) =>
      view.rerender(<MeetingsPage subPath={encode(options.subPath)} />));
    return view;
  }
  const saved = (sessionId: string, title: string) => ({ sessionId, title, startedAt: "2026-10-06T10:00:00Z",
    inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null });

  it("keeps the program editor open under bb's encoded panel route", async () => {
    mocks.meetings = [saved("saved-1", "Review")];
    mocks.call.mockImplementation(programCalls);
    const view = renderLikeBb("proj-mac/saved-1");
    await screen.findByRole("heading", { name: "Review" });
    fireEvent.click(screen.getByRole("button", { name: "Workspace program" }));
    expect(await screen.findByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.getByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    expect(mocks.call).not.toHaveBeenCalledWith("saveWorkspaceMemo", expect.anything());
    view.unmount();

    mocks.call.mockClear();
    render(<MeetingsPage subPath="proj-mac/%40program" />);
    expect(await screen.findByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    expect(mocks.call.mock.calls.some(([method, input]) => method === "readWorkspaceMeeting"
      && /program/.test((input as { sessionId?: string })?.sessionId || ""))).toBe(false);
  });

  it("opens the program editor when a dirty memo's meeting was discarded, keeping the text", async () => {
    mocks.meetings = [saved("gone", "Discarded call")];
    let discarded = false;
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "saveWorkspaceMemo" && discarded) return { ok: false, error: { code: "not_found", message: "session not found", retryable: false } };
      return programCalls(method, input);
    }) as unknown as typeof defaultCall);
    renderLikeBb("proj-mac/gone");
    const pad = await screen.findByLabelText("Meeting memo pad");
    discarded = true;
    fireEvent.change(pad, { target: { value: "Follow up with Ana" } });
    fireEvent.click(screen.getByRole("button", { name: "Workspace program" }));
    expect(await screen.findByTitle("/m/configs/obsidian.enzyme")).toBeTruthy();
    expect(screen.getByText(/could not be saved \(session not found\)/)).toBeTruthy();
    expect(JSON.parse(sessionStorage.getItem("margins.bb.unsaved-memo.proj-mac/gone")!))
      .toEqual({ text: "Follow up with Ana", revision: "rev" });
    expect(screen.getByLabelText("Unsaved memo text")).toHaveProperty("value", "Follow up with Ana");
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    fireEvent.click(screen.getByRole("button", { name: "Copy text" }));
    expect(writeText).toHaveBeenCalledWith("Follow up with Ana");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
  });

  it("keeps a memo typed just before a refresh follows a newly live meeting", async () => {
    mocks.meetings = [saved("fresh-open", "Fresh")];
    renderLikeBb("proj-mac/fresh-open");
    const pad = await screen.findByLabelText("Meeting memo pad");
    mocks.meetings = [...mocks.meetings, { ...saved("live-new", "Live"), inputFinalized: false }];
    fireEvent.change(pad, { target: { value: "Typed before the refresh" } });
    await act(async () => { mocks.realtime!(); });
    await waitFor(() => expect(mocks.navigate).toHaveBeenLastCalledWith("meetings", { subPath: "proj-mac/live-new" }));
    expect(mocks.call).toHaveBeenCalledWith("saveWorkspaceMemo",
      { projectId: "proj-mac", sessionId: "fresh-open", expectedRevision: "rev", text: "Typed before the refresh" });
  });

  it("offers the saved memo when a restored memo conflicts", async () => {
    mocks.meetings = [saved("conflicted", "Conflicted")];
    sessionStorage.setItem("margins.bb.unsaved-memo.proj-mac/conflicted", JSON.stringify({ text: "Mine", revision: "old" }));
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "readWorkspaceMeeting") return { ok: true, meeting: { ...saved("conflicted", "Conflicted"), notepad: { revision: "rev", text: "Theirs" } } };
      if (method === "saveWorkspaceMemo") return { ok: false, error: { code: "revision_conflict", message: "memo changed elsewhere", retryable: false } };
      return defaultCall(method, input);
    }) as unknown as typeof defaultCall);
    renderLikeBb("proj-mac/conflicted");
    await waitFor(() => expect(screen.getByLabelText("Meeting memo pad")).toHaveProperty("value", "Mine"));
    fireEvent.click(await screen.findByRole("button", { name: "Load saved version" }));
    await waitFor(() => expect(screen.getByLabelText("Meeting memo pad")).toHaveProperty("value", "Theirs"));
    expect(screen.getByLabelText("Unsaved memo text")).toHaveProperty("value", "Mine");
    expect(sessionStorage.getItem("margins.bb.unsaved-memo.proj-mac/conflicted")).toBeNull();
  });

  it("switches meetings and projects past a failed memo save, then restores the text on return", async () => {
    mocks.meetings = [saved("first", "First"), saved("second", "Second")];
    let failing = true;
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string; expectedRevision?: string }) => {
      if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }, { id: "proj-other", name: "Other" }] };
      if (method === "saveWorkspaceMemo" && failing) return { ok: false, error: { code: "unavailable", message: "Margins server restarted", retryable: true } };
      return defaultCall(method, input);
    }) as unknown as typeof defaultCall);
    renderLikeBb("proj-mac/first");
    fireEvent.change(await screen.findByLabelText("Meeting memo pad"), { target: { value: "Unsaved idea" } });
    fireEvent.click(screen.getByRole("button", { name: "Second" }));
    await screen.findByRole("heading", { name: "Second" });
    expect(screen.getByText(/could not be saved \(Margins server restarted\)/)).toBeTruthy();
    expect(screen.getByLabelText("Meeting memo pad")).toHaveProperty("value", "");

    failing = false;
    fireEvent.click(screen.getByRole("button", { name: "First" }));
    await waitFor(() => expect(screen.getByLabelText("Meeting memo pad")).toHaveProperty("value", "Unsaved idea"));
    await waitFor(() => expect(mocks.call).toHaveBeenCalledWith("saveWorkspaceMemo",
      { projectId: "proj-mac", sessionId: "first", expectedRevision: "rev", text: "Unsaved idea" }));
    await waitFor(() => expect(sessionStorage.getItem("margins.bb.unsaved-memo.proj-mac/first")).toBeNull());
    expect(screen.queryByText(/could not be saved/)).toBeNull();

    failing = true;
    fireEvent.change(screen.getByLabelText("Meeting memo pad"), { target: { value: "Another idea" } });
    fireEvent.change(screen.getByLabelText("bb project for Meetings"), { target: { value: "proj-other" } });
    await waitFor(() => expect(mocks.navigate).toHaveBeenLastCalledWith("meetings", { subPath: "proj-other" }));
    expect(sessionStorage.getItem("margins.bb.unsaved-memo.proj-mac/first")).toContain("Another idea");
  });

  it("keeps a live meeting one click away while the program editor is open", async () => {
    mocks.meetings = [{ sessionId: "live-1", title: "Standup", startedAt: "2026-10-06T10:00:00Z", inputFinalized: false,
      notePath: null, threadIds: [], distilledMemoRevision: null }];
    mocks.call.mockImplementation((async (method: string, input?: { sessionId?: string; text?: string }) => {
      if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], resolvedWorkspaceId: "obsidian", autoSelected: false };
      if (method === "workspaceProgram") return { workspaceId: "obsidian", workspaceName: "Obsidian", programPath: "/m/configs/obsidian.enzyme", revision: "r1", program: "x" };
      if (method === "planWorkspaceProgram") return { ok: true, previewId: "p", workspaceId: "obsidian", baseRevision: "r1", noop: true, actions: [], diff: "" };
      return defaultCall(method, input);
    }) as unknown as typeof defaultCall);
    render(<MeetingsPage subPath="proj-mac/live-1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Workspace program" }));
    await screen.findByTitle("/m/configs/obsidian.enzyme");
    expect(screen.getByText(/Recording in progress/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Standup" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Back to the meeting" }));
    await waitFor(() => expect(screen.queryByTitle("/m/configs/obsidian.enzyme")).toBeNull());
    expect(mocks.navigate).toHaveBeenLastCalledWith("meetings", { subPath: "proj-mac/live-1" });
  });
});
