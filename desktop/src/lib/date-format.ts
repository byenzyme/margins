// Sidebar/header display date formatting. Distinct from the filesystem-facing
// `note_filename_template` / `created_date_format` (chrono strftime, rendered in
// Rust): this is a small set of friendly presets for how a capture's timestamp
// reads in the UI. The filename slug keeps its own template.

export type SidebarDateFormat = "compact" | "iso" | "us" | "date-only";

export const DEFAULT_SIDEBAR_DATE_FORMAT: SidebarDateFormat = "compact";

export const SIDEBAR_DATE_FORMAT_OPTIONS: { value: SidebarDateFormat; label: string; example: string }[] = [
  { value: "compact", label: "Compact", example: "Jun 22, 2:30 PM" },
  { value: "iso", label: "ISO (24-hour)", example: "2026-06-22 14:30" },
  { value: "us", label: "US numeric", example: "6/22/26, 2:30 PM" },
  { value: "date-only", label: "Date only", example: "Jun 22, 2026" },
];

export function normalizeSidebarDateFormat(value: string | null | undefined): SidebarDateFormat {
  return SIDEBAR_DATE_FORMAT_OPTIONS.some(o => o.value === value)
    ? (value as SidebarDateFormat)
    : DEFAULT_SIDEBAR_DATE_FORMAT;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Format an ISO timestamp for the sidebar/header. Returns "" for empty/invalid input. */
export function formatSidebarDate(iso: string | null | undefined, format: SidebarDateFormat = DEFAULT_SIDEBAR_DATE_FORMAT): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (!Number.isFinite(d.getTime())) return "";
  switch (normalizeSidebarDateFormat(format)) {
    case "iso":
      return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
    case "us":
      return d.toLocaleString(undefined, {
        month: "numeric", day: "numeric", year: "2-digit",
        hour: "numeric", minute: "2-digit",
      });
    case "date-only":
      return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
    case "compact":
    default: {
      const date = d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
      const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
      return `${date}, ${time}`;
    }
  }
}
