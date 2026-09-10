import type { SessionInfo } from "./tauri";
import { formatSidebarDate, type SidebarDateFormat } from "./date-format";

export type SessionFilterKind = "tag" | "person" | "type" | "source";
export type SessionFilters = { tags: string[]; people: string[]; types: string[]; sources: string[] };

export function emptySessionFilters(): SessionFilters {
  return { tags: [], people: [], types: [], sources: [] };
}

/**
 * The user-driven descriptive part of a capture's title (primary color in the
 * sidebar). Empty when a capture has no name beyond its timestamp slug — the
 * timestamp itself carries no description.
 */
export function sessionDescription(s: SessionInfo): string {
  const explicit = s.frontmatter_title?.trim() || s.title?.trim() || s.calendar_event_title?.trim();
  if (explicit) return explicit;
  if (isTimestampSessionName(s.name)) return "";
  return titleFromSlug(s.name);
}

export function sessionTitle(s: SessionInfo): string {
  return sessionDescription(s) || "Untitled capture";
}

export interface SessionTitleParts {
  /** Formatted capture timestamp — rendered in a secondary color. */
  date: string;
  /** User/calendar descriptive title — rendered in the primary color. */
  description: string;
}

/** Split a session into its secondary (date) and primary (description) display parts. */
export function sessionTitleParts(s: SessionInfo, dateFormat: SidebarDateFormat): SessionTitleParts {
  return {
    date: formatSidebarDate(sessionCreatedSortKey(s), dateFormat),
    description: sessionDescription(s),
  };
}

function isTimestampSessionName(name: string): boolean {
  // Default `YYYY-MM-DD-HH-MM` slugs, optionally with seconds and/or a -N
  // uniqueness suffix (e.g. calendar-derived `...-HH-MM-SS` or collision `-2`).
  return /^\d{4}-\d{2}-\d{2}-\d{2}-\d{2}(?:-\d{2})?(?:-\d+)?$/.test(name);
}

function titleFromSlug(name: string): string {
  const words = name.replace(/[-_]+/g, " ").trim();
  if (!words) return "Untitled capture";
  return words.replace(/\b\w/g, char => char.toUpperCase());
}

export function isCaptureNote(s: SessionInfo): boolean {
  return s.source === "capture_note";
}

export function isImporting(s: SessionInfo): boolean {
  return s.import_status === "queued" || s.import_status === "transcribing" || s.import_status === "note";
}

export function isFailedImport(s: SessionInfo): boolean {
  return s.import_status === "error";
}

/**
 * A sidebar row's secondary line, split so the view can weight the two halves
 * differently: a quiet leading state phrase the glyph already hints at, and the
 * concrete, scannable detail (date, duration, people, marks) that deserves the
 * room. `status` is null when the glyph alone carries the state.
 */
export interface RowMeta {
  status: string | null;
  detail: string;
}

/** Sidebar row meta for an in-flight or failed audio import. */
export function importStatusMeta(s: SessionInfo): RowMeta | null {
  switch (s.import_status) {
    case "queued": return { status: "Queued", detail: "waiting" };
    case "transcribing": return { status: "Importing", detail: "transcribing…" };
    case "note": return { status: "Importing", detail: "writing note…" };
    case "error": return { status: "Import failed", detail: s.import_error || "click to retry" };
    default: return null;
  }
}

export function sessionSourceFilterValue(s: SessionInfo): string {
  return isCaptureNote(s) ? "Saved notes" : "Live captures";
}

export function sortSessionsForSidebar(items: SessionInfo[]): SessionInfo[] {
  return [...items].sort((a, b) => {
    const aKey = sessionCreatedSortKey(a);
    const bKey = sessionCreatedSortKey(b);
    if (aKey !== bKey) return bKey.localeCompare(aKey);
    return sessionTitle(a).localeCompare(sessionTitle(b));
  });
}

export function sessionCreatedSortKey(s: SessionInfo): string {
  return s.frontmatter_created_sort || s.start_time || "";
}

export function filterSessionsForSidebar(items: SessionInfo[], filters: SessionFilters): SessionInfo[] {
  return items.filter(s => {
    const tags = sessionFrontmatterTags(s);
    const people = sessionFrontmatterPeople(s);
    const type = sessionFrontmatterType(s);
    const source = sessionSourceFilterValue(s);
    if (filters.tags.length > 0 && !filters.tags.some(tag => tags.includes(tag))) return false;
    if (filters.people.length > 0 && !filters.people.some(person => people.includes(person))) return false;
    if (filters.types.length > 0 && (!type || !filters.types.includes(type))) return false;
    if (filters.sources.length > 0 && !filters.sources.includes(source)) return false;
    return true;
  });
}

export function sidebarSessionFilterCount(filters: SessionFilters): number {
  return filters.tags.length + filters.people.length + filters.types.length + filters.sources.length;
}

export function sidebarFilterOptions(items: SessionInfo[], kind: SessionFilterKind): { value: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const session of items) {
    const values = kind === "tag"
      ? sessionFrontmatterTags(session)
      : kind === "person"
        ? sessionFrontmatterPeople(session)
        : kind === "source"
          ? [sessionSourceFilterValue(session)]
          : sessionFrontmatterType(session)
            ? [sessionFrontmatterType(session)!]
            : [];
    for (const value of new Set(values.filter(Boolean))) counts.set(value, (counts.get(value) || 0) + 1);
  }
  return [...counts.entries()]
    .map(([value, count]) => ({ value, count }))
    .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value));
}

export function sessionFilterValues(filters: SessionFilters, kind: SessionFilterKind): string[] {
  if (kind === "tag") return filters.tags;
  if (kind === "person") return filters.people;
  if (kind === "source") return filters.sources;
  return filters.types;
}

export function sessionFrontmatterTags(s: SessionInfo): string[] {
  return (s.frontmatter_tags || []).map(v => v.trim()).filter(Boolean);
}

export function sessionFrontmatterPeople(s: SessionInfo): string[] {
  const frontmatterPeople = (s.frontmatter_people || []).map(v => v.trim()).filter(Boolean);
  return frontmatterPeople.length > 0 ? frontmatterPeople : (s.people || []).map(v => v.trim()).filter(Boolean);
}

export function sessionFrontmatterType(s: SessionInfo): string | null {
  return s.frontmatter_reflection_type?.trim() || null;
}

export function sessionFrontmatterSummary(s: SessionInfo, limit = 3): string[] {
  const items: string[] = [];
  const date = frontmatterDateLabel(s);
  const type = sessionFrontmatterType(s);
  if (date) items.push(date);
  if (type) items.push(type);
  for (const person of sessionFrontmatterPeople(s)) items.push(person);
  for (const tag of sessionFrontmatterTags(s).map(tag => `#${tag.replace(/^#/, "")}`)) items.push(tag);
  return [...new Set(items)].slice(0, limit);
}

export function frontmatterDateLabel(s: SessionInfo): string | null {
  const raw = s.frontmatter_created?.trim();
  if (!raw) return null;
  const match = raw.match(/\d{4}-\d{2}-\d{2}/);
  return match?.[0] || raw.replace(/^\[\[/, "").replace(/\]\]$/, "").replace(/^['\"]|['\"]$/g, "");
}

export function defaultCollapsedSidebarKeys(): string[] {
  return [];
}

export function searchSessions(items: SessionInfo[], query: string): SessionInfo[] {
  const q = query.trim().toLowerCase();
  if (!q) return items;
  return items.filter(s => {
    const haystack = [
      sessionTitle(s),
      s.name,
      ...(s.people || []),
      ...sessionFrontmatterPeople(s),
      ...sessionFrontmatterTags(s),
      sessionFrontmatterType(s) || "",
    ].join(" ").toLowerCase();
    return haystack.includes(q);
  });
}

export interface CaptureCta {
  title: string;
  ariaLabel: string;
  onclick: string;
  disabled: boolean;
}

export function captureCta(opts: {
  captureReady: boolean;
  projectConfigured: boolean;
  projectName: string;
}): CaptureCta {
  const title = opts.captureReady
    ? `Start a new capture in ${opts.projectName} (⌘N)`
    : opts.projectConfigured
      ? "Capture is not ready yet"
      : "Choose a project before capture";
  const onclick = opts.captureReady
    ? "window.__startDefaultMeeting()"
    : opts.projectConfigured
      ? ""
      : "window.__addProject()";
  return {
    title,
    ariaLabel: opts.captureReady ? "New capture" : title,
    onclick,
    disabled: opts.projectConfigured && !opts.captureReady,
  };
}

export function sessionStatusGlyph(s: SessionInfo): { glyph: string | null; tone: "recording" | "processing" | "incomplete" | "danger" | "none" } {
  if (isCaptureNote(s)) return { glyph: null, tone: "none" };
  if (isImporting(s)) return { glyph: "", tone: "processing" };
  switch (s.status) {
    case "recording": return { glyph: "●", tone: "recording" };
    case "processing": return { glyph: "", tone: "processing" };
    case "failed": return { glyph: "!", tone: "danger" };
    case "synthesized": return { glyph: null, tone: "none" };
    case "unprocessed": return { glyph: "◐", tone: "incomplete" };
    default: return { glyph: null, tone: "none" };
  }
}
