import { ChevronDown, ChevronRight, Download, FileJson, RefreshCw, Search, X } from "lucide";
import type { CalendarEventSuggestion, EvidenceFreshness, GranolaImportStatus, ProjectSource, SessionInfo, Settings } from "../lib/tauri";
import { calendarFreshnessLabel } from "../lib/calendar-freshness";
import { normalizeSidebarDateFormat } from "../lib/date-format";
import { formatDuration, formatTime } from "../lib/format";
import { esc, iconSvg, js } from "../lib/html";
import { activeProject, normalizeProjects } from "../state/defaults";
import {
  captureCta,
  frontmatterDateLabel,
  importStatusMeta,
  isCaptureNote,
  isFailedImport,
  isImporting,
  type RowMeta,
  searchSessions,
  sessionFilterValues,
  sessionFrontmatterPeople,
  sessionFrontmatterSummary,
  sessionFrontmatterTags,
  sessionStatusGlyph,
  sessionTitle,
  sessionTitleParts,
  sidebarFilterOptions,
  sidebarSessionFilterCount,
  type SessionFilterKind,
  type SessionFilters,
} from "../lib/session-model";

export interface SidebarRenderContext {
  sidebarCollapsed: boolean;
  sessions: SessionInfo[];
  visibleSessions: SessionInfo[];
  projectSessions: Record<string, SessionInfo[]>;
  visibleProjectSessions: Record<string, SessionInfo[]>;
  activeSessionName: string | null;
  activeSessionStatus: SessionInfo["status"] | null;
  sidebarFilterOpen: boolean;
  sessionFilters: SessionFilters;
  /** Active-project session search is active; the project tree is swapped for the search view. */
  globalSearchActive: boolean;
  /** Current active-project search query. */
  globalSearchQuery: string;
  sidebarCollapsedSections: Set<string>;
  settings: Settings;
  /**
   * A project/vault is configured (path + notes folder) so a capture can start.
   * Deliberately independent of AI sign-in — audio + marks are local. AI is only
   * required later, at note-making time.
   */
  captureReady: boolean;
  calendarSuggestion: CalendarEventSuggestion | null;
  calendarSuggestionFreshness: EvidenceFreshness;
  sessionStatusLabel: (status: SessionInfo["status"]) => string;
  renderSessionStatus: (session: SessionInfo, label: string) => string;
  failedStartupSessionName?: string | null;
  noteErrorNames: Set<string>;
  /** Aria-live text for import progress; rendered into #sidebar-import-status. */
  importAnnouncement?: string;
  /** Session to flash with the success highlight after an import reconciles. */
  flashImportSessionName?: string | null;
  /** Session to flash with the success highlight after distillation completes. */
  flashDistilledSessionName?: string | null;
  /** A dropped/browsed import awaiting the configure step (speaker count + Transcribe). */
  pendingImport?: { paths: string[]; speakerCount: number } | null;
  /** Ensure-CLI-tools step running before the Setup-with-agent prompt copies. */
  agentSetupCliInstall?: { state: "idle" | "installing" | "ready" | "error"; projectId: string | null; message: string | null };
  /** Project ID currently waiting for agent detection after prompt was copied. */
  agentSetupWaitingProjectId: string | null;
  /** Cached markdown file counts per project ID; computed asynchronously after folder selection. */
  vaultNoteCounts: Map<string, number>;
  recallIndexingProjectIds: Set<string>;
  /** Projects whose notes are being re-synced (refresh button spinner). */
  refreshingProjectIds: Set<string>;
  copiedProjectPrompt: { projectId: string; mode: "assess" | "fix" | "setup" } | null;
  dismissedProjectSetupHints: Set<string>;
  granolaStatus: GranolaImportStatus;
  granolaBusy: "authorizing" | "importing" | null;
  /** Current import stage text, shown as the button tooltip while importing. */
  granolaImportStage?: string | null;
  /** Show the inline callout explaining how to get transcripts on plan-gated accounts. */
  showGranolaPlanCallout?: boolean;
}

export interface HomeIntroRenderContext {
  captureReady: boolean;
  noteChoiceMade: boolean;
  noteChoice: "write" | "capture" | null;
  contextReady: boolean;
  projectName: string;
}

export function renderSidebar(ctx: SidebarRenderContext): string {
  const activeFilterCount = sidebarSessionFilterCount(ctx.sessionFilters);
  const projects = normalizeProjects(ctx.settings);
  const active = activeProject(ctx.settings);
  const projectConfigured = Boolean(active.path?.trim());
  const cta = captureCta({ captureReady: ctx.captureReady, projectConfigured, projectName: active.name });
  const newLabel = projectConfigured ? active.name : "Choose project";
  return `
    <margins id="workspace-sidebar" class="sidebar" data-tauri-drag-region>
      <div class="mobile-sidebar-header">
        <span>Navigation</span>
        <button class="icon-button mobile-sidebar-close" type="button" title="Close navigation" aria-label="Close navigation" onclick="window.__toggleSidebar()">${iconSvg(X, "ui-icon")}</button>
      </div>
      <div class="sidebar-content">
        <div class="sidebar-new ${ctx.captureReady && projects.length > 1 ? "has-switch" : ""}">
          <button class="sidebar-new-button" title="${esc(cta.title)}" ${cta.onclick ? `onclick="${cta.onclick}"` : ""} ${cta.disabled ? "disabled" : ""}>
            <span class="sidebar-new-plus" aria-hidden="true">+</span>
            <span class="sidebar-new-text">${projectConfigured ? `New in ` : ""}<span class="sidebar-new-project">${esc(newLabel)}</span></span>
          </button>
          ${ctx.captureReady && projects.length > 1 ? `
            <details class="sidebar-new-switch">
              <summary aria-label="Start a capture in a different project" title="Start a capture in a different project"></summary>
              <div class="sidebar-new-menu" role="menu">
                ${projects.map(project => renderNewProjectOption(project, project.id === active.id)).join("")}
              </div>
            </details>
          ` : ""}
          <button class="sidebar-new-search ${ctx.globalSearchActive ? "active" : ""}" title="Search notes in this project" aria-label="Search notes in this project" aria-pressed="${ctx.globalSearchActive}" onclick="window.__toggleGlobalSearch()">${iconSvg(Search, "ui-icon sidebar-search-toggle-icon")}</button>
        </div>

        ${ctx.globalSearchActive
          ? renderGlobalSearch(ctx, projects)
          : `
        <div class="sidebar-projects">
          <div class="sidebar-projects-head">
            <span>Projects</span>
            <button class="icon-button mini" title="Add project" aria-label="Add project" onclick="window.__addProject()">+</button>
          </div>
          ${projects.map(project => renderProjectTree(ctx, project, project.id === active.id, activeFilterCount)).join("")}
        </div>
        `}
      </div>
      <div class="sidebar-bottom">
        ${renderCalendarSidebarCard(ctx)}
        ${renderImportAffordance(ctx)}
      </div>
      <div class="sidebar-resizer" role="separator" aria-orientation="vertical" aria-label="Resize sidebar" title="Drag to resize sidebar" onpointerdown="window.__startSidebarResize(event)"></div>
    </margins>
  `;
}

/** Stable collapse key for a project's session sections. */
function projectCollapseKey(project: ProjectSource): string {
  return `project:${project.id}`;
}

function renderProjectTree(ctx: SidebarRenderContext, project: ProjectSource, selected: boolean, activeFilterCount: number): string {
  const collapsed = ctx.sidebarCollapsedSections.has(projectCollapseKey(project));
  const projectSessions = ctx.projectSessions[project.id] || [];
  const hasLoadedChildren = selected || projectSessions.length > 0;
  return `
    ${renderProjectRow(ctx, project, selected, collapsed)}
    ${!collapsed && hasLoadedChildren ? renderProjectChildren(ctx, project, selected, activeFilterCount) : ""}
  `;
}

function renderProjectRow(ctx: SidebarRenderContext, project: ProjectSource, selected: boolean, collapsed: boolean): string {
  const collapseKey = projectCollapseKey(project);
  return `
    <div class="sidebar-project ${selected ? "selected" : ""} ${collapsed ? "collapsed" : ""}">
      <div class="sidebar-project-row">
        <button class="sidebar-project-folder-btn" title="Change this project's folder" aria-label="Change folder for ${esc(project.name)}" onclick="window.__changeProjectFolder(${js(project.id)})">
          <span class="sidebar-project-folder" aria-hidden="true"></span>
        </button>
        <div class="sidebar-project-main sidebar-window-drag-control" role="button" tabindex="0" data-tauri-drag-region onclick="window.__selectProject(${js(project.id)})" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}" title="${esc(project.path)}">
          <span class="sidebar-project-name">${esc(project.name)}</span>
        </div>
        ${renderProjectRefreshButton(ctx, project)}
        <button class="sidebar-project-collapse" title="${collapsed ? "Show this project's notes" : "Collapse this project"}" aria-label="${collapsed ? "Expand project" : "Collapse project"}" aria-expanded="${!collapsed}" onclick="event.stopPropagation(); window.__toggleSidebarSection(${js(collapseKey)})">${iconSvg(collapsed ? ChevronRight : ChevronDown, "ui-icon sidebar-project-chevron-icon")}</button>
        ${renderProjectReadinessControl(ctx, project)}
      </div>
      <div class="sidebar-project-destination-row">
        ${renderProjectDestination(project)}
        ${selected ? renderProjectFilterButton(ctx) : ""}
      </div>
      ${selected ? renderProjectSetupHint(ctx, project) : ""}
    </div>
  `;
}

/** Hover-revealed cycle button that re-syncs a project's notes: purges sessions
 *  whose note file was deleted and removes them from the bar. Spins while busy. */
function renderProjectRefreshButton(ctx: SidebarRenderContext, project: ProjectSource): string {
  const refreshing = ctx.refreshingProjectIds.has(project.id);
  return `
    <button class="sidebar-project-refresh ${refreshing ? "refreshing" : ""}" title="Sync notes — remove any deleted notes from this project" aria-label="Sync notes for ${esc(project.name)}" ${refreshing ? "disabled" : ""} onclick="event.stopPropagation(); window.__refreshProjectNotes(${js(project.id)})">${iconSvg(RefreshCw, "ui-icon sidebar-project-refresh-icon")}</button>
  `;
}

function renderProjectReadinessControl(ctx: SidebarRenderContext, project: ProjectSource): string {
  const tone = projectTone(project);
  const label = projectReadinessLabel(project);
  const recallIndexing = project.readiness === "ready" && ctx.recallIndexingProjectIds.has(project.id);
  if (project.readiness === "ready" && !recallIndexing) return "";
  if (project.readiness === "updating") {
    return `
      <span class="project-index-button updating" title="${esc(label)}">
        <span class="project-readiness-dot ${tone}" aria-hidden="true"></span>
        <span>Indexing...</span>
      </span>
    `;
  }
  if (recallIndexing) {
    return `<span class="project-readiness-dot ${tone} recall-indexing" title="Learning your notes" aria-label="Learning your notes"></span>`;
  }
  return `<span class="project-readiness-dot ${tone}" title="${esc(label)}" aria-label="${esc(label)}"></span>`;
}

function renderProjectSetupHint(ctx: SidebarRenderContext, project: ProjectSource): string {
  if (project.readiness === "ready" || ctx.dismissedProjectSetupHints.has(project.id)) return "";
  const copied = ctx.copiedProjectPrompt?.projectId === project.id ? ctx.copiedProjectPrompt.mode : null;
  const quickRunning = project.readiness === "updating";
  const hasFailed = project.readiness === "error";
  const agentMode = hasFailed ? "fix" : "setup";
  const agentCopied = copied === agentMode;
  const agentWaiting = ctx.agentSetupWaitingProjectId === project.id;
  const installingCli = ctx.agentSetupCliInstall?.state === "installing" && ctx.agentSetupCliInstall.projectId === project.id;
  const readyToCopy = ctx.agentSetupCliInstall?.state === "ready" && ctx.agentSetupCliInstall.projectId === project.id;
  const cliInstallError = ctx.agentSetupCliInstall?.state === "error" && ctx.agentSetupCliInstall.projectId === project.id
    ? ctx.agentSetupCliInstall.message
    : null;
  const agentLabel = installingCli
    ? "Installing CLI…"
    : agentCopied
      ? "Copied - paste in agent"
      : readyToCopy
        ? "Copy setup prompt"
        : hasFailed
          ? "Copy agent fix prompt"
          : "Agent setup";
  const quickLabel = hasFailed ? "Try quick setup again" : "Quick setup";
  const title = quickRunning ? "Learning your notes" : hasFailed ? "Note memory stalled" : "Help Margins learn your notes";
  const rawCount = ctx.vaultNoteCounts.get(project.id);
  const noteCountLabel = rawCount != null && rawCount > 0 ? (rawCount >= 1000 ? "999+ notes" : `${rawCount} notes`) : null;
  const message = cliInstallError
    ? `Could not prepare the setup prompt: ${cliInstallError}`
    : quickRunning
      ? "Building the local search that helps Margins find the right people, decisions, and follow-ups."
      : hasFailed
        ? "Recording still works. Try again when you want future notes to remember the right people, decisions, and follow-ups."
        : agentWaiting
          ? "Waiting for your agent — detected automatically."
          : "Connect your notes so writeups can remember the right people, decisions, and follow-ups.";
  return `
    <div class="project-setup-hint" role="status">
      <button class="project-setup-hint-dismiss" title="Dismiss setup hint" aria-label="Dismiss setup hint" onclick="event.stopPropagation(); window.__dismissProjectSetupHint(${js(project.id)})">×</button>
      <div class="project-setup-hint-copy">
        <div class="project-setup-hint-title">${esc(title)}${noteCountLabel ? ` <span class="project-setup-note-count">${esc(noteCountLabel)}</span>` : ""}</div>
        <div class="project-setup-hint-text">${esc(message)}</div>
      </div>
      <div class="project-setup-hint-actions">
        <button class="project-setup-hint-action primary ${agentCopied ? "copied" : ""}" title="${installingCli ? "Verifying the Margins CLI before copying the setup prompt." : agentCopied ? "Prompt copied. Paste it into your AI agent to finish setup." : "Recommended. Copies a prompt your AI agent can use to inspect your notes and set up stronger context."}" onclick="event.stopPropagation(); window.__copyProjectHelpPrompt(${js(project.id)}, '${agentMode}')" ${quickRunning || installingCli ? "disabled" : ""}>${installingCli ? `<span class="project-setup-spinner" aria-hidden="true"></span>` : ""}<span class="project-setup-hint-action-label">${esc(agentLabel)}</span>${!hasFailed && !agentCopied && !installingCli ? `<span class="project-setup-hint-badge">Recommended</span>` : ""}</button>
        <button class="project-setup-hint-action" title="Fast default setup with Margins. Good if you want the lighter path." onclick="event.stopPropagation(); window.__quickSetupProject(${js(project.id)})" ${quickRunning || installingCli ? "disabled" : ""}>${esc(quickRunning ? "Setting up..." : quickLabel)}</button>
      </div>
    </div>
  `;
}

function renderProjectFilterButton(ctx: SidebarRenderContext): string {
  const activeFilterCount = sidebarSessionFilterCount(ctx.sessionFilters);
  return `
    <button class="sidebar-filter-button ${activeFilterCount ? "active" : ""} ${ctx.sidebarFilterOpen ? "open" : ""}" title="Filter captures and saved notes" onclick="window.__toggleSessionFilters()" aria-label="Filter">
      <span class="sidebar-filter-glyph" aria-hidden="true"></span>${activeFilterCount ? `<span class="sidebar-filter-count">${activeFilterCount}</span>` : ""}
    </button>
  `;
}

// Destination is picker-only: clicking opens the native folder dialog rooted at
// the project. The root leaf shows grayed; the chosen subfolder in primary text.
// When unset, captures land at the root (no "/", no prompt).
function renderProjectDestination(project: ProjectSource): string {
  const rootLeaf = projectRootLeaf(project);
  const subpath = (project.inbox_folder || "").trim().replace(/^\/+|\/+$/g, "");
  return `
    <button class="sidebar-project-destination" title="Choose where captures are saved" onclick="window.__pickProjectDestination(${js(project.id)})">
      <span class="sidebar-project-destination-root">${esc(rootLeaf)}${subpath ? "/" : ""}</span>${subpath ? `<span class="sidebar-project-destination-sub">${esc(subpath)}</span>` : ""}
    </button>
  `;
}

function renderNewProjectOption(project: ProjectSource, selected: boolean): string {
  const tone = projectTone(project);
  const showReadiness = tone !== "ready";
  return `
    <button class="sidebar-new-menu-item ${selected ? "selected" : ""}" role="menuitem" onclick="window.__startMeetingInProject(${js(project.id)})" title="Start a capture in ${esc(project.name)}">
      ${showReadiness ? `<span class="project-readiness-dot ${tone}" title="${esc(projectReadinessLabel(project))}" aria-label="${esc(projectReadinessLabel(project))}"></span>` : ""}
      <span class="sidebar-new-menu-text">
        <span class="sidebar-new-menu-name">${esc(project.name)}</span>
        <span class="sidebar-new-menu-dest">${esc(projectDestinationLabel(project))}</span>
      </span>
    </button>
  `;
}

function renderProjectChildren(ctx: SidebarRenderContext, project: ProjectSource, selected: boolean, activeFilterCount: number): string {
  const projectSessions = ctx.projectSessions[project.id] || [];
  const visibleSessions = ctx.visibleProjectSessions[project.id] || [];
  return `
    <div class="sidebar-project-children">
      ${selected && ctx.sidebarFilterOpen ? renderSidebarFilterPanel(ctx) : ""}
      <div class="sidebar-sessions sidebar-sessions-tree">
        ${selected ? renderImportSlot(ctx) : ""}
        ${renderSessionList(ctx, projectSessions, visibleSessions, `project:${project.id}`)}
      </div>
    </div>
  `;
}

// Active-project session search view. Swaps in for the project tree while the
// search icon is toggled on. Only the active project's sessions are loaded
// client-side, so the scope line names what is searchable.
function renderGlobalSearch(ctx: SidebarRenderContext, projects: ProjectSource[]): string {
  const active = activeProject(ctx.settings);
  const query = ctx.globalSearchQuery.trim();
  const matches = query ? searchSessions(ctx.sessions, ctx.globalSearchQuery) : ctx.sessions;
  const otherProjects = projects.filter(p => p.id !== active.id).length;
  const scope = otherProjects > 0
    ? `Searching ${esc(active.name)} · open another project to search it`
    : `Searching ${esc(active.name)}`;
  return `
    <div class="sidebar-search-view">
      <div class="sidebar-search-bar">
        <div class="sidebar-search">
          <span class="sidebar-search-icon" aria-hidden="true">⌕</span>
          <input
            type="text"
            class="sidebar-search-input"
            id="sidebar-global-search-input"
            placeholder="Search this project"
            value="${esc(ctx.globalSearchQuery)}"
            oninput="window.__setGlobalSearch(this.value)"
            spellcheck="false"
            autocomplete="off"
          />
          ${ctx.globalSearchQuery ? `<button class="sidebar-search-clear" title="Clear search" onclick="window.__clearGlobalSearch()">×</button>` : ""}
        </div>
        <button class="sidebar-search-close" title="Back to projects" aria-label="Back to projects" onclick="window.__toggleGlobalSearch()">×</button>
      </div>
      <div class="sidebar-search-scope">${scope}</div>
      <div class="sidebar-sessions sidebar-search-results">
        ${matches.length === 0
          ? `<div class="sidebar-empty">${query ? "No notes match." : "No notes yet."}</div>`
          : `
            <div class="sidebar-search-group">
              <div class="sidebar-search-group-head">
                <span class="sidebar-search-group-name">${esc(active.name)}</span>
                <span class="sidebar-search-group-count">${matches.length}</span>
              </div>
              ${matches.map(s => renderSessionRow(ctx, s)).join("")}
            </div>
          `}
      </div>
    </div>
  `;
}

function projectRootLeaf(project: ProjectSource): string {
  const root = project.path.replace(/\/+$/g, "");
  return root.split(/[\\/]+/).filter(Boolean).pop() || root;
}

function projectDestinationLabel(project: ProjectSource): string {
  const folder = (project.inbox_folder || "").trim().replace(/^\/+|\/+$/g, "");
  const leaf = projectRootLeaf(project);
  return folder ? `${leaf}/${folder}` : leaf;
}

function projectTone(project: ProjectSource): string {
  if (project.readiness === "error") return "error";
  if (project.readiness === "updating") return "updating";
  if (project.readiness === "ready") return "ready";
  return "needs-setup";
}

function projectReadinessLabel(project: ProjectSource): string {
  if (project.readiness === "ready") return "Ready";
  if (project.readiness === "updating") return "Learning your notes... your notes aren't changed.";
  if (project.readiness === "error") return "Note memory needs attention — your notes weren't changed.";
  return "Note memory can be set up later";
}

const IMPORT_SPEAKER_OPTIONS: { value: number; label: string }[] = [
  { value: 1, label: "Single" },
  { value: 2, label: "2" },
  { value: 3, label: "3" },
  { value: 4, label: "4+" },
];

const SIDEBAR_HISTORY_COLLAPSED_LIMIT = 10;
const SIDEBAR_HISTORY_EXPANDED_LIMIT = 30;

function importSpeakerTitle(value: number): string {
  if (value === 1) return "Keep as a single voice — no speaker separation";
  if (value === 4) return "Separate into 4 or more speakers (caps at 8)";
  return `Separate into ${value} speakers (diarization)`;
}

function importBasename(path: string): string {
  return path.split(/[\\/]+/).filter(Boolean).pop() ?? path;
}

// Hover/focus-revealed browse fallback (keyboard-reachable) plus the aria-live
// status node. The speaker control now lives only in the post-drop configure
// card (renderImportSlot), not here.
function renderImportAffordance(ctx: SidebarRenderContext): string {
  return `
    <div class="sidebar-import">
      <button class="sidebar-import-action" aria-label="Import audio" title="Import audio" data-tooltip="Import audio" onclick="window.__importAudioBrowse()">
        ${iconSvg(Download, "ui-icon sidebar-import-icon")}
        <span class="visually-hidden">Import audio</span>
      </button>
      ${renderGranolaAction(ctx)}
    </div>
    ${ctx.showGranolaPlanCallout ? renderGranolaPlanCallout() : ""}
    <div id="sidebar-import-status" class="visually-hidden" aria-live="polite">${esc(ctx.importAnnouncement ?? "")}</div>
  `;
}

function renderGranolaPlanCallout(): string {
  return `
    <div class="granola-plan-callout" role="note">
      <span class="granola-plan-callout-text">Granola's API doesn't include transcripts on your plan. Drag a Granola export (JSON) onto the sidebar to import full transcripts.</span>
      <button class="granola-plan-callout-dismiss" aria-label="Dismiss" onclick="window.__dismissGranolaPlanCallout()">×</button>
    </div>
  `;
}

// One button, three moods: authorize, import, and busy. Drag-drop of
// exported Granola files remains available as a fallback via the drop box.
function renderGranolaAction(ctx: SidebarRenderContext): string {
  const busy = ctx.granolaBusy;
  const authorized = ctx.granolaStatus.authorized;
  const label = busy === "authorizing"
    ? "Authorizing Granola import..."
    : busy === "importing"
      ? (ctx.granolaImportStage || "Importing from Granola...")
      : authorized
        ? "Import from Granola"
        : "Authorize Granola import";
  const title = busy
    ? label
    : authorized
      ? `Import meetings from Granola${ctx.granolaStatus.accounts.length ? ` (${ctx.granolaStatus.accounts.join(", ")})` : ""}`
      : "Authorize a one-time Granola import into this project";
  return `
    <button class="sidebar-import-action sidebar-granola-action ${busy ? "busy" : ""} ${authorized ? "authorized" : ""}" aria-label="${esc(label)}" title="${esc(title)}" data-tooltip="${esc(label)}" onclick="window.__granolaImport()" ${busy ? "disabled" : ""}>
      ${iconSvg(FileJson, "ui-icon sidebar-import-icon")}
      <span class="visually-hidden">${esc(label)}</span>
    </button>
  `;
}

// The single, consistent top slot of the session list: the configure card while
// an import is pending, otherwise an always-in-DOM drop box (hidden by default,
// revealed via CSS under .sidebar.is-drop-target / .is-drop-invalid). The drop
// box is omitted while a configure card is present so they never collide.
function renderImportSlot(ctx: SidebarRenderContext): string {
  if (ctx.pendingImport) return renderImportConfigure(ctx.pendingImport);
  return `
    <div class="sidebar-drop-box" aria-hidden="true">
      <span class="sidebar-drop-box-glyph">⤓</span>
      <span class="sidebar-drop-box-label sidebar-drop-box-label-valid">Drop audio or Granola export</span>
      <span class="sidebar-drop-box-label sidebar-drop-box-label-audio">Drop audio to transcribe</span>
      <span class="sidebar-drop-box-label sidebar-drop-box-label-granola">Drop Granola export to import</span>
      <span class="sidebar-drop-box-label sidebar-drop-box-label-invalid">Audio, JSON, JSONL, or CSV only</span>
    </div>
  `;
}

// Post-drop configure card. The only place the speaker selector now appears;
// it is wired to __setPendingSpeakerCount (the pending import's count), NOT the
// saved Settings default. Multi-file drops share one count for the whole batch.
function renderImportConfigure(pending: { paths: string[]; speakerCount: number }): string {
  const selected = Math.max(1, Math.min(4, pending.speakerCount));
  const names = pending.paths.map(importBasename);
  const primary = names[0] ?? "audio file";
  const fileLabel = names.length > 1 ? `${primary} +${names.length - 1} more` : primary;
  const fileTitle = names.join("\n");
  const speakerButtons = IMPORT_SPEAKER_OPTIONS.map(opt =>
    `<button class="sidebar-import-speaker ${opt.value === selected ? "selected" : ""}" aria-pressed="${opt.value === selected}" title="${esc(importSpeakerTitle(opt.value))}" onclick="window.__setPendingSpeakerCount(${opt.value})">${esc(opt.label)}</button>`,
  ).join("");
  return `
    <div class="sidebar-import-configure" role="group" aria-label="Configure audio import">
      <div class="sidebar-import-configure-file" title="${esc(fileTitle)}">
        <span class="sidebar-import-configure-glyph" aria-hidden="true">♫</span>
        <span class="sidebar-import-configure-name">${esc(fileLabel)}</span>
      </div>
      <div class="sidebar-import-speakers" role="group" aria-label="Speaker count for this import">
        <span class="sidebar-import-speakers-label">Speakers</span>
        ${speakerButtons}
      </div>
      <div class="sidebar-import-configure-actions">
        <button class="sidebar-import-configure-cancel" onclick="window.__cancelPendingImport()">Cancel</button>
        <button class="sidebar-import-configure-go" onclick="window.__confirmPendingImport()">Transcribe</button>
      </div>
    </div>
  `;
}

export function renderHomeIntro(ctx: HomeIntroRenderContext): string {
  const captureReady = ctx.captureReady;
  const captureLabel = captureReady ? `New capture in ${ctx.projectName}` : "Choose folder";
  return `
    <div class="home-intro-panel ${ctx.noteChoiceMade ? "choice-made" : ""}">
      <div class="margo home-intro-margo" aria-hidden="true">
        <img src="/margo/welcome.png" alt="" width="132" />
      </div>
      <h1>For the things you notice but don't&nbsp;say.</h1>
      <div class="home-intro-points">
        <div><strong>A tap, mid-conversation</strong><span>Enter marks the thought without breaking your attention.</span></div>
        <div><strong>A cue while it's live</strong><span>Margins quietly surfaces the question you're not asking.</span></div>
        <div><strong>A note that connects</strong><span>Afterward, your marks are woven into the transcript and the notes you already keep.</span></div>
      </div>
      <p class="home-intro-safety">Audio stays on this Mac. Nothing records until you say so.</p>
      ${ctx.noteChoiceMade ? `
        <div class="home-intro-actions centered">
          <button class="primary" onclick="${captureReady ? "window.__startDefaultMeeting()" : "window.__addProject()"}">${esc(captureLabel)}</button>
        </div>
      ` : captureReady ? `
        <div class="home-intro-actions">
          <button class="primary" onclick="window.__chooseFirstCaptureMode('write')">Start your first capture</button>
        </div>
        <button class="link-button home-intro-alt" onclick="window.__chooseFirstCaptureMode('capture')">Just the transcript</button>
      ` : `
        <p class="home-context-note">First, choose a folder where your notes will live.</p>
        <div class="home-intro-actions">
          <button class="primary" onclick="window.__addProject()">Choose folder</button>
        </div>
      `}
    </div>
  `;
}

function renderSessionList(ctx: SidebarRenderContext, sessions = ctx.sessions, visibleSessions = ctx.visibleSessions, sectionKey = "active"): string {
  if (sessions.length === 0) {
    return `<div class="sidebar-empty">No captures yet.</div>`;
  }
  if (visibleSessions.length === 0) {
    const filtersActive = sidebarSessionFilterCount(ctx.sessionFilters) > 0;
    return `<div class="sidebar-empty">No captures match.${filtersActive ? `<br/><button class="link-button" onclick="window.__clearSessionFilters()">Clear filters</button>` : ""}</div>`;
  }
  if (visibleSessions.length <= SIDEBAR_HISTORY_COLLAPSED_LIMIT) {
    return `<div class="sidebar-section-rows">${visibleSessions.map(s => renderSessionRow(ctx, s)).join("")}</div>`;
  }

  const key = `history:${sectionKey}`;
  const expanded = ctx.sidebarCollapsedSections.has(key);
  const limit = expanded ? SIDEBAR_HISTORY_EXPANDED_LIMIT : SIDEBAR_HISTORY_COLLAPSED_LIMIT;
  const visible = visibleSessions.slice(0, limit);
  const hidden = visibleSessions.length - visible.length;
  const buttonLabel = expanded
    ? "Show fewer"
    : visibleSessions.length > SIDEBAR_HISTORY_EXPANDED_LIMIT
      ? `Show 20 more`
      : `Show ${hidden} more`;
  const buttonTitle = expanded
    ? `Collapse list to ${SIDEBAR_HISTORY_COLLAPSED_LIMIT}`
    : `Show up to ${Math.min(visibleSessions.length, SIDEBAR_HISTORY_EXPANDED_LIMIT)} items`;
  return `
    <div class="sidebar-section-rows">
      ${visible.map((s, index) => renderSessionRow(ctx, s, { secondary: index >= SIDEBAR_HISTORY_COLLAPSED_LIMIT })).join("")}
      <button class="sidebar-history-more" title="${esc(buttonTitle)}" aria-expanded="${expanded}" onclick="window.__toggleSidebarSection(${js(key)})">
        <span>${esc(buttonLabel)}</span>
        ${hidden > 0 && expanded ? `<span class="sidebar-history-more-count">${hidden} hidden</span>` : ""}
      </button>
    </div>
  `;
}

function renderSessionRow(ctx: SidebarRenderContext, s: SessionInfo, opts: { secondary?: boolean } = {}): string {
  const failedStartup = s.name === ctx.failedStartupSessionName;
  const failedImport = isFailedImport(s);
  const failedNote = s.status === "failed" || ctx.noteErrorNames.has(s.name);
  const importing = isImporting(s);
  const { glyph: baseGlyph, tone: baseTone } = sessionStatusGlyph(s);
  const glyph = failedImport || failedNote ? "!" : failedStartup ? "◐" : baseGlyph;
  const tone = failedImport || failedNote ? "danger" : failedStartup ? "incomplete" : baseTone;
  const glyphHtml = glyph === null
    ? ""
    : `<span class="sidebar-row-glyph tone-${tone}" aria-hidden="true">${glyph}</span>`;
  const captureNote = isCaptureNote(s);
  const title = sessionTitle(s);
  const { date: titleDate, description: titleDescription } = sessionTitleParts(s, normalizeSidebarDateFormat(ctx.settings.sidebar_date_format));
  const isActive = s.name === ctx.activeSessionName;
  const time = frontmatterDateLabel(s) || formatTime(s.start_time);
  const importMeta = importStatusMeta(s);
  const baseMeta = sidebarRowMeta(s, time, captureNote);
  const meta: RowMeta = failedNote
    ? { status: "Note failed", detail: "Retry" }
    : importMeta ?? (failedStartup ? { status: "Setup needed", detail: time } : baseMeta);
  const displayMeta = rowMetaWithDate(meta, titleDate, titleDescription, time);
  // Attention rows route to their recovery action; everything else opens.
  const onClick = failedImport
    ? `window.__retryImport(${js(s.name)})`
    : failedNote
      ? `window.__processSession(${js(s.name)})`
    : `window.__openSession(${js(s.name)}, ${js(s.status)}, ${js(s.project_id || "")})`;
  const classes = [
    "sidebar-row",
    isActive ? "active" : "",
    captureNote ? "is-note" : "",
    importing ? "is-importing" : "",
    failedImport ? "is-import-error" : "",
    failedNote ? "is-note-error" : "",
    opts.secondary ? "is-secondary" : "",
    s.name === ctx.flashImportSessionName ? "just-imported" : "",
    s.name === ctx.flashDistilledSessionName ? "just-distilled" : "",
    `tone-${tone}`,
  ].filter(Boolean).join(" ");
  const tooltipLabel = [titleDate, titleDescription].filter(Boolean).join(" · ") || title;
  const failureMessage = failedImport ? s.import_error : failedNote ? s.failure_message : null;
  const rowTooltip = failureMessage ? `${tooltipLabel} — ${failureMessage}` : tooltipLabel;
  const titleHtml = titleDescription
    ? `<span class="sidebar-row-name">${esc(titleDescription)}</span>`
    : titleDate
      ? `<span class="sidebar-row-name">${esc(titleDate)}</span>`
      : esc(title);
  return `
    <div class="${classes} sidebar-window-drag-control" role="button" tabindex="0" data-tauri-drag-region title="${esc(rowTooltip)}" onclick="${onClick}" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}">
      ${glyphHtml}
      <span class="sidebar-row-body">
        <span class="sidebar-row-title ${titleDescription ? "has-name" : ""}">${titleHtml}</span>
        <span class="sidebar-row-meta">${renderRowMeta(displayMeta)}</span>
      </span>
    </div>
  `;
}

// Two-weight meta line: the status phrase recedes behind the concrete detail.
// Status and detail are joined
// by a dimmed middot only when both are present.
function renderRowMeta(meta: RowMeta): string {
  const status = meta.status ? `<span class="sidebar-row-status">${esc(meta.status)}</span>` : "";
  const detail = meta.detail ? `<span class="sidebar-row-detail">${esc(meta.detail)}</span>` : "";
  const sep = status && detail ? `<span class="sidebar-row-meta-sep" aria-hidden="true">·</span>` : "";
  return `${status}${sep}${detail}`;
}

// When the title already shows the description, surface the capture date as the
// detail anchor and drop a now-duplicate time token from the detail.
function rowMetaWithDate(meta: RowMeta, titleDate: string, titleDescription: string, time: string): RowMeta {
  if (!titleDate || !titleDescription) return meta;
  const cleaned = meta.detail
    .replace(new RegExp(`(^| · )${escapeRegExp(time)}( · |$)`), (match, before, after) => {
      if (before && after) return " · ";
      return "";
    })
    .replace(/^ · | · $/g, "")
    .trim();
  return { status: meta.status, detail: cleaned ? `${titleDate} · ${cleaned}` : titleDate };
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function sidebarRowMeta(s: SessionInfo, time: string, captureNote: boolean): RowMeta {
  if (captureNote) {
    const summary = sessionFrontmatterSummary(s, 3);
    if (summary.length) return { status: null, detail: summary.join(" · ") };
    if (s.vault_note_path) return { status: null, detail: compactPath(s.vault_note_path) };
    return { status: null, detail: `${time} · indexed note` };
  }
  // Label work that's still in flight; stay silent on settled states. Repeating
  // "Ready for note" down the list was noise.
  if (s.status === "recording") return { status: "Capturing now", detail: time };
  if (s.status === "processing") return { status: "Writing note", detail: time };
  if (s.status === "unprocessed") {
    const marks = s.memo_line_count ? `${s.memo_line_count} mark${s.memo_line_count === 1 ? "" : "s"}` : "";
    return { status: null, detail: [formatDuration(s.duration_secs), marks].filter(Boolean).join(" · ") };
  }
  // Saved notes are settled; the date + people/duration get the full width.
  const people = sessionFrontmatterPeople(s);
  return { status: null, detail: people.length ? people.slice(0, 2).join(", ") : formatDuration(s.duration_secs) };
}

function compactPath(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.slice(-2).join("/");
}

function renderCalendarSidebarCard(ctx: SidebarRenderContext): string {
  if (ctx.calendarSuggestion) {
    const start = ctx.calendarSuggestion.start
      ? new Date(ctx.calendarSuggestion.start).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
      : "";
    const people = ctx.calendarSuggestion.people.slice(0, 2).map(esc).join(", ");
    const freshness = calendarFreshnessLabel(ctx.calendarSuggestionFreshness);
    const freshnessRow = freshness
      ? `<div class="sidebar-calendar-meta sidebar-calendar-freshness">${esc(freshness)}</div>`
      : "";
    if (!ctx.captureReady) {
      return `
        <div class="sidebar-calendar-card">
          <div class="sidebar-calendar-label">${start ? `Calendar · ${esc(start)}` : "Calendar event"}</div>
          <div class="sidebar-calendar-title">${esc(ctx.calendarSuggestion.title)}</div>
          ${freshnessRow}
          <div class="sidebar-calendar-meta">${people || "No other attendees"}</div>
          <button class="primary" onclick="window.__nav('home')">Set up project</button>
        </div>
      `;
    }
    return `
      <div class="sidebar-calendar-card">
        <div class="sidebar-calendar-label">${start ? `Calendar · ${esc(start)}` : "Calendar event"}</div>
        <div class="sidebar-calendar-title">${esc(ctx.calendarSuggestion.title)}</div>
        ${freshnessRow}
        <div class="sidebar-calendar-meta">${people || "No other attendees"}</div>
        <div class="sidebar-calendar-actions">
          <button class="primary sidebar-prep-btn" onclick="window.__openCaptureFromCalendar()">Prepare for meeting</button>
        </div>
      </div>
    `;
  }
  return "";
}

function renderSidebarFilterPanel(ctx: SidebarRenderContext): string {
  return `
    <div class="sidebar-filter-panel" role="region" aria-label="Capture filters">
      <div class="sidebar-filter-head">
        <div>Filters</div>
        ${sidebarSessionFilterCount(ctx.sessionFilters) ? `<button class="link-button" onclick="window.__clearSessionFilters()">Clear</button>` : ""}
      </div>
      ${renderSidebarFilterGroup(ctx, "Source", "source", sidebarFilterOptions(ctx.sessions, "source"))}
      ${renderSidebarFilterGroup(ctx, "Tags", "tag", sidebarFilterOptions(ctx.sessions, "tag"))}
      ${renderSidebarFilterGroup(ctx, "People", "person", sidebarFilterOptions(ctx.sessions, "person"))}
      ${renderSidebarFilterGroup(ctx, "Type", "type", sidebarFilterOptions(ctx.sessions, "type"))}
      <div class="sidebar-filter-hint">Source distinguishes live captures from saved notes. Other filters read saved-note frontmatter.</div>
    </div>
  `;
}

function renderSidebarFilterGroup(ctx: SidebarRenderContext, label: string, kind: SessionFilterKind, options: { value: string; count: number }[]): string {
  if (options.length === 0) return "";
  return `
    <div class="sidebar-filter-group">
      <div class="sidebar-filter-label">${esc(label)}</div>
      <div class="sidebar-filter-chips">
        ${options.map(option => {
          const selected = sessionFilterValues(ctx.sessionFilters, kind).includes(option.value);
          return `<button class="filter-chip ${selected ? "selected" : ""}" onclick="window.__toggleSessionFilter(${js(kind)}, ${js(option.value)})">${esc(option.value)} <span>${option.count}</span></button>`;
        }).join("")}
      </div>
    </div>
  `;
}

// Re-exported for backward compatibility; tags shown only on hover in the new row design.
export function _sidebarLegacyTags(s: SessionInfo): string[] {
  return sessionFrontmatterTags(s).slice(0, 2);
}
