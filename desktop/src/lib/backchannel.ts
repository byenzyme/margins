export type BackchannelMemo = { time: string; text: string; windowRange?: string };
export type BackchannelTranscript = { speaker: string; text: string };
export type BackchannelWindow = {
  range: string;
  label: string;
  memos: BackchannelMemo[];
  transcripts: BackchannelTranscript[];
};

export interface BackchannelSummary {
  duration: string;
  segments: string;
  memoCount: number;
  transcriptCount: number;
  audioWindows: BackchannelWindow[];
  unwindowedMemos: BackchannelMemo[];
  memos: BackchannelMemo[];
}

export function parseBackchannel(md: string): BackchannelSummary {
  const duration = md.match(/\*\*Duration\*\*:\s*([^\n]+)/)?.[1]?.trim() || "";
  const segments = md.match(/\*\*Segments\*\*:\s*([^\n]+)/)?.[1]?.trim() || "";
  const metaMemoCount = Number(md.match(/Memo lines:\s*(\d+)/i)?.[1] || NaN);
  const metaTranscriptCount = Number(md.match(/Transcript entries:\s*(\d+)/i)?.[1] || NaN);

  const windows: BackchannelWindow[] = [];
  const memos: BackchannelMemo[] = [];
  let current: BackchannelWindow | null = null;
  let sawRealTimeline = false;

  for (const rawLine of md.split("\n")) {
    const line = rawLine.trim();
    if (!line) continue;

    const windowHeading = line.match(/^###\s+(.+)/);
    if (windowHeading) {
      sawRealTimeline = true;
      current = { range: windowHeading[1], label: "Audio captured in this window", memos: [], transcripts: [] };
      windows.push(current);
      continue;
    }

    const realMemo = line.match(/^\*\*\[([^\]]*memo)\]\*\*\s*(.*)/i);
    if (realMemo) {
      const memo = { time: realMemo[1], text: realMemo[2], windowRange: current?.range };
      memos.push(memo);
      current?.memos.push(memo);
      continue;
    }

    const transcript = line.match(/^>\s*\[transcript\s+([^\]]+)\]\s*(.*)/i);
    if (transcript && current) {
      current.label = "Speech detected in captured audio";
      const text = transcript[2]?.trim();
      if (text) current.transcripts.push({ speaker: transcript[1], text });
      continue;
    }
  }

  if (!sawRealTimeline) {
    let section: { title: string; text: string[] } | null = null;
    const flush = () => {
      if (!section) return;
      const isMemo = section.title.toLowerCase().includes("memo");
      const text = section.text.join(" ").trim();
      const time = section.title.match(/^\S+/)?.[0] || section.title.replace(/\s*memo\b/i, "").trim();
      if (isMemo) {
        const memo = { time, text };
        memos.push(memo);
        windows.push({ range: time, label: "Memo captured", memos: [memo], transcripts: [] });
      } else {
        const text = section.text.join(" ").replace(/^>\s*/, "").replace(/^\[transcript\s+[^\]]+\]\s*/i, "").trim();
        windows.push({
          range: time,
          label: "Speech detected in captured audio",
          memos: [],
          transcripts: text ? [{ speaker: section.title.replace(/^\S+\s*/, "") || "Transcript", text }] : [],
        });
      }
    };

    for (const rawLine of md.split("\n")) {
      const line = rawLine.trim();
      if (!line || line.startsWith("# ")) continue;
      if (line.startsWith("## ")) {
        flush();
        section = { title: line.slice(3), text: [] };
      } else if (section) {
        section.text.push(line);
      }
    }
    flush();
  }

  const windowed = new Set(windows.flatMap(win => win.memos));
  const unwindowedMemos = memos.filter(memo => !windowed.has(memo));

  return {
    duration,
    segments,
    memoCount: Number.isFinite(metaMemoCount) ? metaMemoCount : memos.length,
    transcriptCount: Number.isFinite(metaTranscriptCount) ? metaTranscriptCount : windows.filter(w => w.label.includes("Speech")).length,
    audioWindows: windows.length > 0 ? windows : [{ range: "Session", label: "Audio captured", memos: [], transcripts: [] }],
    unwindowedMemos,
    memos,
  };
}
