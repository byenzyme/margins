export function formatDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function formatElapsed(secs: number): string {
  const total = Math.floor(secs);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${pad(h)}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

export function pad(n: number): string {
  return n.toString().padStart(2, "0");
}

export function formatTime(isoStr: string): string {
  try {
    return new Date(isoStr).toLocaleTimeString("en-US", {
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return isoStr;
  }
}

// Perceptual 0..1 loudness from a linear peak, using the same dB curve as the
// level meters so orb/meter readings stay consistent.
export function levelToNormalized(peak: number | null | undefined): number {
  if (peak == null || peak <= 0) return 0;
  const db = 20 * Math.log10(peak);
  return Math.max(0, Math.min(1, (db + 72) / 72));
}

export function levelBars(peak: number | null | undefined, count: number): string {
  if (peak == null || peak <= 0) {
    return Array(count).fill('<div class="bar"></div>').join("");
  }
  const normalized = levelToNormalized(peak);
  const filled = Math.round(normalized * count);

  return Array.from({ length: count }, (_, i) => {
    if (i >= filled) return '<div class="bar"></div>';
    if (i >= count * 0.85) return '<div class="bar clip"></div>';
    if (i >= count * 0.7) return '<div class="bar hot"></div>';
    return '<div class="bar active"></div>';
  }).join("");
}
