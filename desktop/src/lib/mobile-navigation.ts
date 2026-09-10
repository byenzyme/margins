export const MOBILE_NAVIGATION_QUERY = "(max-width: 720px)";

interface MatchMediaWindow {
  matchMedia?: (query: string) => Pick<MediaQueryList, "matches">;
}

export function isMobileNavigationViewport(
  host: MatchMediaWindow | undefined = typeof window === "undefined" ? undefined : window,
): boolean {
  return host?.matchMedia?.(MOBILE_NAVIGATION_QUERY).matches === true;
}

export function sidebarToggleLabel(collapsed: boolean, mobile: boolean): string {
  if (mobile) return collapsed ? "Open navigation" : "Close navigation";
  return collapsed ? "Expand sidebar" : "Collapse sidebar";
}
