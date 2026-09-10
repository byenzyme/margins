export type MarginsMarkerKind = "USE" | "SOURCES";
export type MarginsTranscriptRef = { start_secs?: number; end_secs?: number; quote?: string };
export type MarginsGroundingUse = {
  kind: MarginsMarkerKind;
  section_id?: string;
  memo_ids?: string[];
  note_quote?: string;
  mode?: string;
  disposition?: string;
  transcript_refs?: MarginsTranscriptRef[];
  vault_refs?: Array<string | { path?: string; title?: string }>;
};
export type GroundedNoteBlock = { markdown: string; uses: MarginsGroundingUse[] };
export type GroundedNoteState = {
  blocks: GroundedNoteBlock[];
  pendingUses: MarginsGroundingUse[];
  lineBuffer: string;
  warnings: string[];
  consumedMemoIds: string[];
  suppressGroundingSection: boolean;
  frontmatterBuffer: string[] | null;
  // When set, the note existed before this stream started (reprocess). The
  // renderer keeps these blocks visible and swaps each one out for its
  // rewritten counterpart as that block finishes streaming.
  baseline?: GroundedNoteBlock[];
};

const MARGINS_MARKER_RE = /^<!--\s*MARGINS:(USE|SOURCES)\s+([\s\S]*?)\s*-->\s*$/;
const MARGINS_MARKER_PREFIX_RE = /^<!--\s*MARGINS:/;

export function stripMarginsMarkers(md: string): { cleanMarkdown: string; uses: MarginsGroundingUse[]; warnings: string[] } {
  const state = newGroundedNoteState();
  for (const line of md.split(/\r?\n/)) processGroundedNoteLine(state, line);
  return {
    cleanMarkdown: state.blocks.map(block => block.markdown).join("\n").trimStart(),
    uses: state.blocks.flatMap(block => block.uses),
    warnings: state.warnings,
  };
}

export function newGroundedNoteState(): GroundedNoteState {
  return { blocks: [], pendingUses: [], lineBuffer: "", warnings: [], consumedMemoIds: [], suppressGroundingSection: false, frontmatterBuffer: null };
}

export function memoIdForIndex(index: number): string {
  return `m${String(index + 1).padStart(3, "0")}`;
}

export function memoIndexFromId(id: string): number | null {
  const match = id.match(/^m(\d+)$/i);
  if (!match) return null;
  const idx = Number(match[1]) - 1;
  return Number.isFinite(idx) && idx >= 0 ? idx : null;
}

export function parseMarginsMarkerLine(line: string): MarginsGroundingUse | null | "malformed" {
  const match = line.trim().match(MARGINS_MARKER_RE);
  if (!match) return MARGINS_MARKER_PREFIX_RE.test(line.trim()) ? "malformed" : null;
  try {
    const parsed = JSON.parse(match[2]);
    return {
      kind: match[1] as MarginsMarkerKind,
      section_id: typeof parsed.section_id === "string" ? parsed.section_id : undefined,
      memo_ids: Array.isArray(parsed.memo_ids) ? parsed.memo_ids.filter((id: unknown) => typeof id === "string") : [],
      note_quote: typeof parsed.note_quote === "string" ? parsed.note_quote : undefined,
      mode: typeof parsed.mode === "string" ? parsed.mode : undefined,
      disposition: typeof parsed.disposition === "string" ? parsed.disposition : undefined,
      transcript_refs: Array.isArray(parsed.transcript_refs) ? parsed.transcript_refs : [],
      vault_refs: Array.isArray(parsed.vault_refs) ? parsed.vault_refs : [],
    };
  } catch {
    return "malformed";
  }
}

export function processGroundedNoteLine(state: GroundedNoteState, line: string) {
  const trimmed = line.trim();
  if (state.frontmatterBuffer) {
    state.frontmatterBuffer.push(line);
    if (trimmed === "---") {
      appendVisibleNoteLine(state, state.frontmatterBuffer.join("\n"));
      state.frontmatterBuffer = null;
    }
    return;
  }
  // Drop blank lines that arrive before any real content. A leading blank would
  // otherwise open an empty block, and that phantom block makes the opening
  // `---` below fail the frontmatter-open test — leaving the raw YAML to render
  // as visible prose above the note. Notes always begin with YAML frontmatter,
  // so nothing meaningful is lost by ignoring leading whitespace.
  if (state.blocks.length === 0 && state.pendingUses.length === 0 && !state.suppressGroundingSection && trimmed === "") {
    return;
  }
  if (state.blocks.length === 0 && state.pendingUses.length === 0 && trimmed === "---") {
    state.frontmatterBuffer = [line];
    return;
  }

  const startsGroundingSection = /^#{1,6}\s+grounding\b/i.test(trimmed) || /^grounding\s*:?$/i.test(trimmed);
  if (startsGroundingSection) {
    state.suppressGroundingSection = true;
    consumeMemoIdsFromText(state, line);
    return;
  }
  if (state.suppressGroundingSection) {
    if (/^#{1,6}\s+\S/.test(trimmed)) {
      state.suppressGroundingSection = false;
    } else {
      consumeMemoIdsFromText(state, line);
      return;
    }
  }

  const marker = parseMarginsMarkerLine(line);
  if (marker === "malformed") {
    state.warnings.push("Malformed MARGINS marker stripped from visible note.");
    return;
  }
  if (marker) {
    if (marker.kind === "USE") {
      state.pendingUses.push(marker);
      for (const id of marker.memo_ids || []) rememberConsumedMemoId(state, id);
    }
    return;
  }

  appendVisibleNoteLine(state, line);
}

function appendVisibleNoteLine(state: GroundedNoteState, line: string) {
  const trimmed = line.trim();
  const startsMarkdownSection = /^#{1,6}\s+\S/.test(trimmed);
  const previousBlock = state.blocks[state.blocks.length - 1];
  if (
    startsMarkdownSection
    && previousBlock
    && previousBlock.markdown.trim().length > 0
  ) {
    state.blocks.push({ markdown: line, uses: state.pendingUses.splice(0) });
  } else if (state.pendingUses.length > 0 || state.blocks.length === 0) {
    state.blocks.push({ markdown: line, uses: state.pendingUses.splice(0) });
  } else {
    state.blocks[state.blocks.length - 1].markdown += `\n${line}`;
  }
}

export function mergeGroundingUsesIntoState(
  state: GroundedNoteState,
  uses: MarginsGroundingUse[],
): GroundedNoteState {
  if (!uses.length) return state;
  const next: GroundedNoteState = {
    ...state,
    blocks: state.blocks.map(block => ({ ...block, uses: [...block.uses] })),
    consumedMemoIds: [...state.consumedMemoIds],
    warnings: [...state.warnings],
    pendingUses: [...state.pendingUses],
  };
  const hasExistingUses = next.blocks.some(block => block.uses.length > 0);
  if (hasExistingUses) return next;

  const targetBlocks = next.blocks
    .map((block, index) => ({ block, index }))
    .filter(item => /^#{1,6}\s+\S/m.test(item.block.markdown));
  const fallbackBlocks = next.blocks.map((block, index) => ({ block, index }));
  const candidates = targetBlocks.length ? targetBlocks : fallbackBlocks;
  if (!candidates.length) return next;

  for (let i = 0; i < uses.length; i++) {
    const use = uses[i];
    const targetIndex = bestBlockIndexForUse(candidates, use, i);
    next.blocks[targetIndex].uses.push(use);
    for (const id of use.memo_ids || []) rememberConsumedMemoId(next, id);
  }
  return next;
}

export function mergeVaultRefsIntoState(
  state: GroundedNoteState,
  refs: Array<string | { path?: string; title?: string }>,
): GroundedNoteState {
  if (!refs.length) return state;
  const next: GroundedNoteState = {
    ...state,
    blocks: state.blocks.map(block => ({
      ...block,
      uses: block.uses.map(use => ({ ...use, vault_refs: [...(use.vault_refs || [])] })),
    })),
    consumedMemoIds: [...state.consumedMemoIds],
    warnings: [...state.warnings],
    pendingUses: [...state.pendingUses],
  };
  if (!next.blocks.length) return next;

  const existing = new Set(
    next.blocks
      .flatMap(block => block.uses)
      .flatMap(use => use.vault_refs || [])
      .map(vaultRefLabel)
      .filter(Boolean),
  );
  const missing = refs.filter(ref => {
    const label = vaultRefLabel(ref);
    return label && !existing.has(label);
  });
  if (!missing.length) return next;

  const firstGroundedBlock = next.blocks.find(block => block.uses.length > 0);
  if (firstGroundedBlock) {
    firstGroundedBlock.uses[0].vault_refs = [...(firstGroundedBlock.uses[0].vault_refs || []), ...missing];
    return next;
  }

  const firstSection = next.blocks.find(block => /^#{1,6}\s+\S/m.test(block.markdown)) || next.blocks[0];
  firstSection.uses.push({ kind: "USE", vault_refs: missing });
  return next;
}

function vaultRefLabel(ref: string | { path?: string; title?: string }): string {
  const raw = typeof ref === "string" ? ref : ref.title || ref.path || "";
  return raw.replace(/\.md$/i, "").toLowerCase().trim();
}

function bestBlockIndexForUse(
  candidates: { block: GroundedNoteBlock; index: number }[],
  use: MarginsGroundingUse,
  ordinal: number,
): number {
  const section = normalizeGroundingText(use.section_id || "");
  if (section) {
    const sectionWords = section.split(" ").filter(word => word.length > 2);
    const matched = candidates.find(item => {
      const text = normalizeGroundingText(item.block.markdown);
      return sectionWords.some(word => text.includes(word));
    });
    if (matched) return matched.index;
  }
  return candidates[Math.min(ordinal, candidates.length - 1)].index;
}

function normalizeGroundingText(value: string): string {
  return value.toLowerCase().replace(/[-_]+/g, " ").replace(/\s+/g, " ").trim();
}

export function rememberConsumedMemoId(state: GroundedNoteState, id: string) {
  if (!/^m\d+$/i.test(id)) return;
  const canonical = id.toLowerCase();
  if (!state.consumedMemoIds.includes(canonical)) state.consumedMemoIds.push(canonical);
}

export function consumeMemoIdsFromText(state: GroundedNoteState, text: string) {
  for (const match of text.matchAll(/\bm\d{3,}\b/gi)) rememberConsumedMemoId(state, match[0]);
}

export function consumeNoteStreamChunkInState(state: GroundedNoteState, chunk: string): boolean {
  state.lineBuffer += chunk;
  let processedVisibleLine = false;
  let newlineIndex = state.lineBuffer.search(/\r?\n/);
  while (newlineIndex >= 0) {
    const line = state.lineBuffer.slice(0, newlineIndex);
    const newlineLength = state.lineBuffer[newlineIndex] === "\r" && state.lineBuffer[newlineIndex + 1] === "\n" ? 2 : 1;
    state.lineBuffer = state.lineBuffer.slice(newlineIndex + newlineLength);
    const beforeBlocks = state.blocks.length;
    const beforeLast = state.blocks[beforeBlocks - 1]?.markdown.length || 0;
    processGroundedNoteLine(state, line);
    const afterBlocks = state.blocks.length;
    const afterLast = state.blocks[afterBlocks - 1]?.markdown.length || 0;
    if (afterBlocks !== beforeBlocks || afterLast !== beforeLast) processedVisibleLine = true;
    newlineIndex = state.lineBuffer.search(/\r?\n/);
  }
  return processedVisibleLine || safeVisibleLineBuffer(state).length > 0;
}

// The in-progress (not-yet-terminated) line, but only when it is safe to paint
// as prose. Returns "" while anything hidden could still be mid-flight, so the
// streaming renderer can preview it without ever flashing YAML frontmatter, a
// half-parsed MARGINS marker, or broken Markdown.
export function safeVisibleLineBuffer(state: GroundedNoteState): string {
  const partial = state.lineBuffer.trimStart();
  const boldDelimiterCount = partial.match(/\*\*/g)?.length || 0;
  const backtickCount = partial.match(/`/g)?.length || 0;
  const safe = state.frontmatterBuffer === null
    && !state.suppressGroundingSection
    && state.blocks.length > 0
    && partial.length >= 24
    // A marker may be split at any byte boundary. Once `<` begins a partial
    // line, keep buffering until newline so hidden JSON can never flash.
    && !partial.startsWith("<")
    // Let Markdown delimiters settle before previewing. Otherwise an opening
    // `**` can paint as raw punctuation for one frame before its closing chunk.
    && boldDelimiterCount % 2 === 0
    && backtickCount % 2 === 0;
  return safe ? partial : "";
}

// Blocks as they will look once the current in-progress line lands, so the
// streaming renderer can show settled-looking prose in place instead of holding
// it back until the line's terminating newline arrives. The partial is run
// through the same line processor the real newline will use, so the previewed
// block is byte-identical to its finalized form — no reflow when it commits.
// Returns the committed blocks unchanged when the partial isn't safe to show.
export function previewBlocksWithLineBuffer(state: GroundedNoteState): GroundedNoteBlock[] {
  if (!safeVisibleLineBuffer(state)) return state.blocks;
  const clone: GroundedNoteState = {
    ...state,
    blocks: state.blocks.map(block => ({ ...block, uses: [...block.uses] })),
    pendingUses: [...state.pendingUses],
    consumedMemoIds: [...state.consumedMemoIds],
    warnings: [...state.warnings],
    frontmatterBuffer: state.frontmatterBuffer ? [...state.frontmatterBuffer] : null,
  };
  processGroundedNoteLine(clone, state.lineBuffer);
  return clone.blocks;
}

export function groundedStateFromMarkdown(md: string): GroundedNoteState {
  const state = newGroundedNoteState();
  for (const line of md.split(/\r?\n/)) processGroundedNoteLine(state, line);
  return state;
}

export function cleanVisibleMarkdown(md: string): string {
  return stripMarginsMarkers(md).cleanMarkdown;
}
