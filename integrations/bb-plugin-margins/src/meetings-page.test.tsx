// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MeetingsPage } from "./meetings-page.js";

const mocks = vi.hoisted(() => ({
  connectMenu: vi.fn(),
  probeMenu: vi.fn(async () => false),
  startBrowser: vi.fn(),
  call: vi.fn(async (method: string) => {
    if (method === "availableProjects") return { projects: [{ id: "proj-mac", name: "Mac" }] };
    if (method === "availableWorkspaces") return { workspaces: [{ id: "obsidian", name: "Obsidian" }], autoSelected: false };
    if (method === "getProjectPanelState") return { state: "ready", sessionId: null };
    if (method === "listWorkspaceMeetings") return { ok: true, meetings: [] };
    if (method === "issueMenuGrant") return {
      serviceUrl: "https://jpham-server.getbb.app/api/v1/plugins/margins/http/menu/relay",
      token: "grant", workspaceId: "obsidian", workspaceName: "Obsidian", instanceId: "one", expiresAt: Date.now() + 60_000,
    };
    throw new Error(`Unexpected RPC ${method}`);
  }),
}));

vi.mock("@get-bb/plugin-sdk/app", () => ({
  experimental_FileLink: () => null,
  useBbContext: () => ({ projectId: "proj-mac" }),
  useBbNavigate: () => ({}),
  useRealtime: () => undefined,
  useRpc: () => ({ call: mocks.call }),
}));
vi.mock("./browser-capture.js", () => ({
  browserCaptureOwner: { startFromProject: mocks.startBrowser },
  detectClientCapabilities: () => ({ clientId: "mac", platform: "macos", secureContext: true,
    browserMicrophone: true, nativeMacCapture: false }),
}));
vi.mock("./native-bridge-client.js", () => ({
  nativeBridgeOwner: { paired: false, status: null, probeMenu: mocks.probeMenu, connectMenu: mocks.connectMenu },
}));

afterEach(() => { vi.clearAllMocks(); sessionStorage.clear(); });

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
});
