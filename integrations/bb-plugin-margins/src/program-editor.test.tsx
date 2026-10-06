// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { codeThemeStyle, ProgramEditor } from "./program-editor.js";

const SAVED = 'workspace "notes" {\n  source markdown "home" { path "/notes" }\n  remember in folder "." create note\n}\n';
const PREVIEW = "11111111-2222-3333-4444-555555555555";
const mocks = vi.hoisted(() => ({
  saved: { workspaceId: "notes", programPath: "/m/configs/notes.enzyme", revision: "rev-1", program: "" },
  planBase: "rev-1",
  apply: { ok: true, revision: "rev-2" } as unknown,
  call: vi.fn(),
}));
const rpc = { call: mocks.call };
function defaultCall(method: string, input: { program?: string }) {
  if (method === "workspaceProgram") return Promise.resolve({ ...mocks.saved });
  if (method === "planWorkspaceProgram") {
    const program = input.program || "";
    if (program.includes("lern")) return Promise.resolve({ ok: false, error: { code: "workspace_desired_invalid",
      message: 'invalid desired program: 3:3: expected "}"; found "lern"', line: 3, column: 3 } });
    if (program.includes("Nope")) return Promise.resolve({ ok: false, error: { code: "workspace_desired_invalid",
      message: 'folder reading "Nope" does not exist under Home', line: null, column: null } });
    return Promise.resolve({ ok: true, previewId: PREVIEW, workspaceId: "notes", baseRevision: mocks.planBase,
      noop: program === mocks.saved.program, actions: [{ action: "set_policy", summary: "Attention policy: learn questions from folder:People" }],
      diff: "--- a/notes.enzyme\n+++ b/notes.enzyme\n@@ -3 +3,2 @@\n+  learn questions from folder \"People\"\n" });
  }
  if (method === "applyWorkspaceProgram") return Promise.resolve(mocks.apply);
  return Promise.reject(new Error(`Unexpected RPC ${method}`));
}

vi.mock("@get-bb/plugin-sdk/app", () => ({
  experimental_Diff: undefined,
  experimental_useCodeTheme: undefined,
  useRpc: () => rpc,
}));

function reset() {
  mocks.saved = { workspaceId: "notes", programPath: "/m/configs/notes.enzyme", revision: "rev-1", program: SAVED };
  mocks.planBase = "rev-1";
  mocks.apply = { ok: true, revision: "rev-2" };
  mocks.call.mockImplementation(defaultCall);
}
reset();
afterEach(() => { cleanup(); vi.clearAllMocks(); reset(); localStorage.clear(); });

async function openEditor() {
  render(<ProgramEditor projectId="proj-1" />);
  const area = await screen.findByLabelText("Workspace program", { selector: "textarea" }) as HTMLTextAreaElement;
  await waitFor(() => expect(area.value).toBe(mocks.saved.program));
  return area;
}
const withPeople = SAVED.replace('create note\n}', 'create note\n  learn questions from folder "People"\n}');

describe("Workspace program editor", () => {
  it("shows the program path with highlighted tokens and line numbers", async () => {
    const { container } = render(<ProgramEditor projectId="proj-1" />);
    await screen.findByText("/m/configs/notes.enzyme");
    const token = (cls: string) => Array.from(container.querySelectorAll(`pre [data-token="${cls}"]`)).map((node) => node.textContent);
    await waitFor(() => expect(token("kind")).toEqual(["markdown"]));
    expect(token("keyword")).toEqual(expect.arrayContaining(["workspace", "source", "path", "remember", "in", "folder", "create", "note"]));
    expect(token("string")).toEqual(['"notes"', '"home"', '"/notes"', '"."']);
    expect(token("punctuation")).toEqual(["{", "{", "}", "}"]);
    expect(Array.from(container.querySelectorAll(".margins-code-gutter span")).map((node) => node.textContent)).toEqual(["1", "2", "3", "4", "5"]);
    // An unchanged program validates as a no-op and cannot be reviewed.
    expect(await screen.findByText("No changes")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Review changes" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Revert to saved" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("places a parse error on its line and token, and a location-less error under the editor", async () => {
    const area = await openEditor();
    const container = document.body;
    fireEvent.change(area, { target: { value: 'workspace "notes" {\n  source markdown "home" { path "/notes" }\n  lern questions\n}\n' } });
    expect(await screen.findByText("Line 3, column 3")).toBeTruthy();
    expect(container.querySelector(".margins-code-gutter span.is-error")?.textContent).toBe("3");
    expect(container.querySelector("pre .enz-error")?.textContent).toBe("lern");
    expect((container.querySelector(".margins-code-error-line") as HTMLElement).style.getPropertyValue("--enz-line")).toBe("2");
    expect(container.querySelector(".margins-code-error-callout")?.textContent).toBe('invalid desired program: expected "}"; found "lern"');
    expect(area.getAttribute("aria-invalid")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Go to line 3" }));
    expect(area.selectionStart).toBe(area.value.indexOf("lern"));

    fireEvent.change(area, { target: { value: SAVED.replace('"."', '"Nope"') } });
    expect(await screen.findByText('folder reading "Nope" does not exist under Home')).toBeTruthy();
    expect(screen.getByText("Not valid")).toBeTruthy();
    expect(container.querySelector(".margins-code-gutter span.is-error")).toBeNull();
    expect(mocks.call.mock.calls.filter(([method]) => method === "applyWorkspaceProgram")).toHaveLength(0);
  });

  it("reviews the plan's summaries and diff and applies exactly that plan", async () => {
    const area = await openEditor();
    fireEvent.change(area, { target: { value: withPeople } });
    expect(await screen.findByText("Valid · 1 change")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review changes" }));
    const review = screen.getByRole("dialog", { name: "Review program changes" });
    expect(review.textContent).toContain("Attention policy: learn questions from folder:People");
    expect(review.querySelector("pre")?.textContent).toContain('+  learn questions from folder "People"');
    mocks.saved = { ...mocks.saved, revision: "rev-2", program: withPeople };
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByText("Saved to /m/configs/notes.enzyme.")).toBeTruthy();
    expect(mocks.call).toHaveBeenCalledWith("applyWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes", previewId: PREVIEW });
    expect(mocks.call).toHaveBeenCalledWith("planWorkspaceProgram", { projectId: "proj-1", workspaceId: "notes", program: withPeople });
    expect(localStorage.getItem("margins.program-draft.notes")).toBeNull();
  });

  it("refuses a stale plan, keeps the user's text, and offers a reload with a way back", async () => {
    const area = await openEditor();
    fireEvent.change(area, { target: { value: withPeople } });
    await screen.findByText("Valid · 1 change");
    fireEvent.click(screen.getByRole("button", { name: "Review changes" }));
    // Someone runs `margins workspace edit` meanwhile.
    const external = SAVED.replace('"."', '"Inbox"');
    mocks.saved = { ...mocks.saved, revision: "rev-9", program: external };
    mocks.apply = { ok: false, error: { code: "stale", message: "changed", line: null, column: null, actualRevision: "rev-9" } };
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect((await screen.findByRole("alert")).textContent).toContain("changed outside this editor");
    expect(area.value).toBe(withPeople);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect((screen.getByRole("button", { name: "Review changes" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Load the saved version" }));
    await waitFor(() => expect(area.value).toBe(external));
    fireEvent.click(screen.getByRole("button", { name: "Restore it" }));
    expect(area.value).toBe(withPeople);
    // The restored text is still based on the old revision, so planning flags it again.
    mocks.planBase = "rev-9";
    expect((await screen.findByRole("alert")).textContent).toContain("changed outside this editor");
    fireEvent.click(screen.getByRole("button", { name: "Review my edits against the saved version" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(area.value).toBe(withPeople);
    await waitFor(() => expect((screen.getByRole("button", { name: "Review changes" }) as HTMLButtonElement).disabled).toBe(false));
  });

  it("restores an unsaved draft and reverts to the saved program with an undo", async () => {
    localStorage.setItem("margins.program-draft.notes", JSON.stringify({ text: withPeople, baseRevision: "rev-1" }));
    render(<ProgramEditor projectId="proj-1" />);
    const area = await screen.findByLabelText("Workspace program", { selector: "textarea" }) as HTMLTextAreaElement;
    await waitFor(() => expect(area.value).toBe(withPeople));
    expect(screen.getByText("Restored your unsaved edits.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Revert to saved" }));
    await waitFor(() => expect(area.value).toBe(SAVED));
    await waitFor(() => expect(localStorage.getItem("margins.program-draft.notes")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Restore it" }));
    expect(area.value).toBe(withPeople);
  });

  it("indents with Tab instead of leaving the editor", async () => {
    const area = await openEditor();
    area.setSelectionRange(0, 0);
    fireEvent.keyDown(area, { key: "Tab" });
    expect(area.value.startsWith("  workspace")).toBe(true);
  });
});

describe("bb code theme", () => {
  it("paints token classes from the live theme's TextMate rules", () => {
    const style = codeThemeStyle({ mode: "dark", name: "t", theme: { name: "t", type: "dark", fg: "#eeeeee", bg: "#111111",
      colors: { "editor.background": "#101010", "editorLineNumber.foreground": "#555555" },
      tokenColors: [
        { settings: { foreground: "#ffffff" } },
        { scope: ["keyword", "storage.type"], settings: { foreground: "#aa00ff" } },
        { scope: "keyword.control", settings: { foreground: "#bb00ff" } },
        { scope: "string, string.quoted.double", settings: { foreground: "#00aa00" } },
        { scope: "meta.embedded string", settings: { foreground: "#ff0000" } },
        { scope: "comment", settings: { foreground: "#777777", fontStyle: "italic" } },
        { scope: "entity.name.type", settings: { foreground: "#00aaff" } },
      ] } }) as Record<string, string>;
    expect(style).toMatchObject({ "--enz-bg": "#101010", "--enz-fg": "#eeeeee", "--enz-gutter": "#555555",
      "--enz-keyword": "#bb00ff", "--enz-string": "#00aa00", "--enz-comment": "#777777", "--enz-kind": "#00aaff" });
    expect(style["--enz-number"]).toBeUndefined();
    expect(codeThemeStyle(null)).toEqual({});
  });
});
