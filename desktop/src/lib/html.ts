import type { IconNode } from "lucide";

export function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;");
}

export function js(s: string): string {
  return esc(JSON.stringify(s));
}

export function iconSvg(icon: IconNode, className = "ui-icon"): string {
  const nodes = icon.map(([tag, attrs]) => {
    const attrString = Object.entries(attrs)
      .map(([key, value]) => `${key}="${String(value).replace(/\"/g, "&quot;")}"`)
      .join(" ");
    return `<${tag} ${attrString}></${tag}>`;
  }).join("");
  return `<svg class="${className}" aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${nodes}</svg>`;
}
