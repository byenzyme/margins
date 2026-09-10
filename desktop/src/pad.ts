import { invoke } from "@tauri-apps/api/core";
import { getSessionMemo } from "./lib/tauri";
import { parsePadMemo } from "./lib/pad-memo";

const query = new URLSearchParams(window.location.search);
const sessionName = query.get("session")?.trim() ?? "";
const projectId = query.get("project")?.trim() || null;
const title = document.getElementById("pad-title")!;
const lines = document.getElementById("pad-lines")!;
const form = document.getElementById("pad-compose") as HTMLFormElement;
const input = document.getElementById("pad-input") as HTMLTextAreaElement;
const status = document.getElementById("pad-status")!;
let refreshTimer: number | undefined;
let saving = false;

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, character => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "\"": "&quot;",
    "'": "&#39;",
  })[character]!);
}

async function refresh(): Promise<void> {
  if (!sessionName || saving) return;
  try {
    const memo = await getSessionMemo(sessionName, projectId);
    const parsed = parsePadMemo(memo);
    lines.innerHTML = parsed.length > 0
      ? parsed.map(line => `<article class="pad-line"><time>${escapeHtml(line.timestamp)}</time><p>${escapeHtml(line.text)}</p></article>`).join("")
      : `<p class="pad-empty">Marks added here or in Margins will appear together.</p>`;
  } catch (error) {
    status.textContent = String(error);
  }
}

form.addEventListener("submit", async event => {
  event.preventDefault();
  const text = input.value.trim();
  if (!text || saving || !sessionName) return;
  saving = true;
  input.disabled = true;
  status.textContent = "Saving…";
  try {
    await invoke("append_active_pad_line", { sessionName, text });
    input.value = "";
    status.textContent = "Saved";
    await refresh();
  } catch (error) {
    status.textContent = String(error);
  } finally {
    saving = false;
    input.disabled = false;
    input.focus();
  }
});

input.addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    form.requestSubmit();
  }
});

document.getElementById("pad-open")!.addEventListener("click", () => {
  void invoke("show_main_window");
});

document.getElementById("pad-close")!.addEventListener("click", async () => {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().close();
});

window.addEventListener("beforeunload", () => {
  if (refreshTimer !== undefined) window.clearInterval(refreshTimer);
});

title.textContent = sessionName || "Active capture";
document.title = sessionName ? `${sessionName} — Pad` : "Pad";
void refresh();
refreshTimer = window.setInterval(() => void refresh(), 1500);
