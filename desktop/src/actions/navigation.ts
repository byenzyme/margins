import type { AiStatus, CalendarEventSuggestion, DeviceInfo, EvidenceFreshness, SessionInfo } from "../lib/tauri";
import { getAiStatus, getCalendarEventSuggestion, listSessions, pickAudioFiles } from "../lib/tauri";
import {
  applyTheme,
  clampSidebarWidth,
  persistSidebarWidth,
  THEME_STORAGE_KEY,
  type ThemeMode,
} from "../lib/preferences";
import {
  emptySessionFilters,
  sessionFilterValues,
  type SessionFilterKind,
  type SessionFilters,
} from "../lib/session-model";
import type { View } from "./types";
import { isMobileNavigationViewport, sidebarToggleLabel } from "../lib/mobile-navigation";

let navigationRequestSeq = 0;

export interface NavigationActionsContext {
  get currentView(): View;
  set currentView(value: View);
  get settingsOverlayOpen(): boolean;
  set settingsOverlayOpen(value: boolean);
  get devices(): DeviceInfo[];
  set devices(value: DeviceInfo[]);
  get aiStatus(): AiStatus;
  set aiStatus(value: AiStatus);
  get sessions(): SessionInfo[];
  set sessions(value: SessionInfo[]);
  get calendarSuggestion(): CalendarEventSuggestion | null;
  set calendarSuggestion(value: CalendarEventSuggestion | null);
  get calendarSuggestionFreshness(): EvidenceFreshness;
  set calendarSuggestionFreshness(value: EvidenceFreshness);
  get recordingMounted(): boolean;
  set recordingMounted(value: boolean);
  get sidebarCollapsed(): boolean;
  set sidebarCollapsed(value: boolean);
  get sidebarWidth(): number;
  set sidebarWidth(value: number);
  get themeMode(): ThemeMode;
  set themeMode(value: ThemeMode);
  get sidebarFilterOpen(): boolean;
  set sidebarFilterOpen(value: boolean);
  get sessionFilters(): SessionFilters;
  set sessionFilters(value: SessionFilters);
  get globalSearchActive(): boolean;
  set globalSearchActive(value: boolean);
  get globalSearchQuery(): string;
  set globalSearchQuery(value: string);
  get sidebarCollapsedSections(): Set<string>;
  set sidebarCollapsedSections(value: Set<string>);
  beginPendingImport: (paths: string[]) => void;
  exitTransientPrep: () => void;
  render: () => void;
}

export function registerNavigationActions(ctx: NavigationActionsContext) {
  window.__nav = async (view: View) => {
    const requestSeq = ++navigationRequestSeq;

    if (view === "settings") {
      ctx.settingsOverlayOpen = true;
      if (ctx.currentView === "recording") ctx.recordingMounted = false;
      if (ctx.currentView === "settings") ctx.currentView = "home";
      ctx.render();

      const aiStatus = await getAiStatus().catch(() => ctx.aiStatus);
      if (requestSeq !== navigationRequestSeq) return;
      ctx.aiStatus = aiStatus;

      ctx.render();
      return;
    }

    if (view === "home") {
      ctx.exitTransientPrep();
      ctx.settingsOverlayOpen = false;
      if (ctx.currentView === "recording") ctx.recordingMounted = false;
      ctx.currentView = view;
      ctx.render();

      const sessions = await listSessions().catch(() => []);
      if (requestSeq !== navigationRequestSeq) return;
      ctx.sessions = sessions;

      const calendarResult = await getCalendarEventSuggestion().catch(() => null);
      if (requestSeq !== navigationRequestSeq) return;
      if (calendarResult) {
        ctx.calendarSuggestion = calendarResult.suggestion;
        ctx.calendarSuggestionFreshness = calendarResult.freshness;
      }
      ctx.render();
      return;
    }
    if (ctx.currentView === "recording" && view !== "recording") {
      ctx.recordingMounted = false;
    }
    ctx.currentView = view;
    ctx.render();
  };

  window.__importAudioBrowse = async () => {
    let paths: string[] = [];
    try {
      paths = await pickAudioFiles();
    } catch (e) {
      console.warn("Audio file picker failed", e);
      return;
    }
    if (paths.length > 0) ctx.beginPendingImport(paths);
  };

  window.__closeSettings = async () => {
    ctx.settingsOverlayOpen = false;
    if (ctx.currentView === "settings") ctx.currentView = "home";
    if (ctx.currentView === "recording") ctx.recordingMounted = false;
    ctx.render();
  };

  window.__toggleSidebar = () => {
    ctx.sidebarCollapsed = !ctx.sidebarCollapsed;
    const shell = document.querySelector<HTMLElement>(".workspace-shell");
    const topbar = document.querySelector<HTMLElement>(".window-topbar");
    const toggle = document.querySelector<HTMLElement>(".window-topbar-toggle");
    if (shell && topbar && toggle) {
      shell.classList.toggle("sidebar-collapsed", ctx.sidebarCollapsed);
      const label = sidebarToggleLabel(ctx.sidebarCollapsed, isMobileNavigationViewport());
      toggle.setAttribute("title", label);
      toggle.setAttribute("aria-label", label);
      toggle.setAttribute("aria-expanded", String(!ctx.sidebarCollapsed));
      window.setTimeout(() => ctx.render(), 260);
    } else {
      ctx.render();
    }
  };

  window.__startSidebarResize = (event: PointerEvent) => {
    if (ctx.sidebarCollapsed) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = ctx.sidebarWidth;
    const shell = document.querySelector<HTMLElement>(".workspace-shell");
    const topbar = document.querySelector<HTMLElement>(".window-topbar");
    shell?.classList.add("is-resizing");
    topbar?.classList.add("is-resizing");
    const onMove = (moveEvent: PointerEvent) => {
      ctx.sidebarWidth = clampSidebarWidth(startWidth + moveEvent.clientX - startX);
      shell?.style.setProperty("--sidebar-width", `${ctx.sidebarWidth}px`);
    };
    const onUp = () => {
      persistSidebarWidth(ctx.sidebarWidth);
      shell?.classList.remove("is-resizing");
      topbar?.classList.remove("is-resizing");
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
    };
    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp, { once: true });
  };

  window.__toggleTheme = () => {
    ctx.themeMode = ctx.themeMode === "dark" ? "light" : "dark";
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, ctx.themeMode);
    } catch {
      // Ignore storage errors; the in-memory theme still updates.
    }
    applyTheme(ctx.themeMode);
    ctx.render();
  };

  window.__toggleSessionFilters = () => {
    ctx.sidebarFilterOpen = !ctx.sidebarFilterOpen;
    ctx.render();
  };

  window.__toggleSessionFilter = (kind: SessionFilterKind, value: string) => {
    const values = sessionFilterValues(ctx.sessionFilters, kind);
    const next = values.includes(value) ? values.filter(v => v !== value) : [...values, value];
    if (kind === "tag") ctx.sessionFilters = { ...ctx.sessionFilters, tags: next };
    else if (kind === "person") ctx.sessionFilters = { ...ctx.sessionFilters, people: next };
    else if (kind === "source") ctx.sessionFilters = { ...ctx.sessionFilters, sources: next };
    else ctx.sessionFilters = { ...ctx.sessionFilters, types: next };
    ctx.render();
  };

  window.__clearSessionFilters = () => {
    ctx.sessionFilters = emptySessionFilters();
    ctx.render();
  };

  window.__toggleGlobalSearch = () => {
    ctx.globalSearchActive = !ctx.globalSearchActive;
    if (!ctx.globalSearchActive) ctx.globalSearchQuery = "";
    ctx.render();
  };

  window.__setGlobalSearch = (value: string) => {
    ctx.globalSearchQuery = value;
    ctx.render();
  };

  window.__clearGlobalSearch = () => {
    ctx.globalSearchQuery = "";
    ctx.render();
  };

  window.__toggleSidebarSection = (id: string) => {
    const next = new Set(ctx.sidebarCollapsedSections);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    ctx.sidebarCollapsedSections = next;
    ctx.render();
  };
}
