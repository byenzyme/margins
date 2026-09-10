export interface PadMemoLine {
  text: string;
  timestamp: string;
  blockOrdinal: number | null;
}

export function parsePadMemo(markdown: string): PadMemoLine[] {
  const lines: PadMemoLine[] = [];
  for (const raw of markdown.split(/\r?\n/)) {
    const block = raw.match(/^\[block (\d+)\]\s+(.+)$/);
    if (block) {
      lines.push({ text: block[2].trim(), timestamp: "Paused", blockOrdinal: Number(block[1]) });
      continue;
    }
    const timed = raw.match(/^\[(\d{2}:\d{2}(?::\d{2})?)(?:\s+~\d{2}:\d{2}(?::\d{2})?)?\]\s+(.*)$/);
    if (!timed) continue;
    const text = timed[2].replace(/^\(audio not live yet\)\s*/, "").trim();
    if (text) lines.push({ text, timestamp: timed[1], blockOrdinal: null });
  }
  return lines;
}
