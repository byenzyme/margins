// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MeetingsPage } from "./meetings-page.js";

const mocks = vi.hoisted(() => ({
  connectMenu: vi.fn(),
  verify: vi.fn(),
  control: vi.fn(),
  refreshNative: vi.fn(),
  navigate: vi.fn(),
  meetings: [] as Array<{ sessionId: string; title: string; startedAt: string; inputFinalized: boolean;
    notePath: null; threadIds: string[]; distilledMemoRevision: null }>,
  native: { paired: false, status: null as { state: string; sessionId: string | null;
    microphoneSamples: number; micPeak: number } | null, connectionError: null as string | null },
  probeMenu: vi.fn(async () => false),
  startBrowser: vi.fn(),
  call: vi.fn(async (method: string, input?: { sessionId?: string }) => {
    if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }] };
    if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], autoSelected: false };
    if (method === "getProjectPanelState") return { state: "ready", sessionId: null };
    if (method === "listWorkspaceMeetings") return { ok: true, meetings: [...mocks.meetings] };
    if (method === "readWorkspaceMeeting") {
      const item = mocks.meetings.find((meeting) => meeting.sessionId === input?.sessionId);
      return { ok: true, meeting: item && { ...item, notepad: { revision: "rev", text: "" } } };
    }
    if (method === "captureAuthority") return { ok: true, instanceId: "one", workspaceId: "obsidian" };
    if (method === "issueMenuGrant") return {
      serviceUrl: "https://jpham-server.getbb.app/api/v1/plugins/margins/http/menu/relay",
      token: "grant", workspaceId: "obsidian", workspaceName: "Obsidian", instanceId: "one", expiresAt: Date.now() + 60_000,
    };
    throw new Error(`Unexpected RPC ${method}`);
  }),
  rpc: null as unknown,
}));
mocks.rpc = { call: mocks.call };

vi.mock("@get-bb/plugin-sdk/app", () => ({
  experimental_FileLink: () => null,
  useBbContext: () => ({ projectId: "proj-mac" }),
  useBbNavigate: () => ({ toPluginPanel: mocks.navigate }),
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
  vi.clearAllMocks();
  mocks.native.paired = false;
  mocks.native.status = null;
  mocks.native.connectionError = null;
  mocks.meetings = [];
  sessionStorage.clear();
});

describe("Meetings Mac recorder choice", () => {
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

  it("opens the new memo pad once the native recorder creates its session", async () => {
    const oldMeeting = { sessionId: "old", title: "Earlier meeting", startedAt: "2026-09-27T00:00:00Z",
      inputFinalized: true, notePath: null, threadIds: [], distilledMemoRevision: null };
    const newMeeting = { ...oldMeeting, sessionId: "new", title: "New meeting", inputFinalized: false };
    mocks.meetings = [oldMeeting];
    mocks.native.paired = true;
    mocks.refreshNative.mockImplementation(async () => mocks.native.status);
    const view = render(<MeetingsPage subPath="proj-mac/old" />);
    await screen.findByRole("heading", { name: "Earlier meeting" });
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await screen.findByRole("heading", { name: "Starting meeting…" });
    await waitFor(() => expect(mocks.control).toHaveBeenCalledWith("start"));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("heading", { name: "Earlier meeting" })).toBeNull();
    mocks.meetings = [newMeeting, oldMeeting];
    mocks.native.status = { state: "recording", sessionId: "new", microphoneSamples: 16000, micPeak: 0.1 };
    await screen.findByRole("heading", { name: "New meeting" });
    expect(screen.getByRole("textbox", { name: "Meeting memo pad" })).toHaveProperty("value", "");
    expect(mocks.navigate).toHaveBeenCalledWith("meetings", { subPath: "proj-mac/new" });
    view.unmount();
  });
});
