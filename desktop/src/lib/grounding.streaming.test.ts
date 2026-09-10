// Streaming-note parser guarantees. Run with:
//   node --test --experimental-strip-types src/lib/grounding.streaming.test.ts
//
// These cover the failure that flashed raw YAML frontmatter / streamed
// fragments above the note heading: the note body must show only settled,
// safe prose while a note streams in, never partial frontmatter or markers.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  newGroundedNoteState,
  consumeNoteStreamChunkInState,
  previewBlocksWithLineBuffer,
  safeVisibleLineBuffer,
} from "./grounding.ts";
import { prepareMarkdownForRender } from "./markdown.ts";

// The exact chunk boundaries the note writer streamed in the real 1280x900
// capture (first chunk is the bare frontmatter fence with no newline).
const REAL_CHUNKS = [
  "---",
  "\ncreated: \"[[2026-07-13]]\"\ntags: []\npeople: []\n---",
  "\n# Harvard sentences test capture\n\nThis capture contains",
  " only the Harvard sentences — a standard phonetically balanced",
  " speech corpus used to calibrate and test audio",
  " transcription systems. No real conversation took place.\n\n",
  "The aligned timeline records a single mic channel reading.\n",
];

function visibleFrom(blocks: { markdown: string }[]): string {
  return blocks.map(b => prepareMarkdownForRender(b.markdown)).join("\n");
}

function looksLikeMetadata(text: string): boolean {
  return /created:|margins_session|^tags:|^people:|<!--\s*MARGINS/m.test(text);
}

// Replays chunks and asserts no metadata ever surfaces in the previewed body,
// across an arbitrary re-chunking of the same bytes.
function assertNoLeak(label: string, chunks: string[]) {
  const state = newGroundedNoteState();
  for (const chunk of chunks) {
    consumeNoteStreamChunkInState(state, chunk);
    const preview = visibleFrom(previewBlocksWithLineBuffer(state));
    assert.ok(!looksLikeMetadata(preview), `${label}: metadata leaked into preview: ${JSON.stringify(preview.slice(0, 120))}`);
  }
  const committed = visibleFrom(state.blocks);
  assert.ok(!looksLikeMetadata(committed), `${label}: metadata leaked into committed blocks`);
  assert.ok(committed.includes("Harvard sentences test capture"), `${label}: heading missing`);
}

test("frontmatter never renders as visible prose (real chunking)", () => {
  assertNoLeak("real", REAL_CHUNKS);
});

test("frontmatter never leaks under adversarial re-chunking", () => {
  assertNoLeak("char-by-char", REAL_CHUNKS.join("").split(""));
  // A stray leading blank line before the fence must not open a phantom block
  // that defeats frontmatter detection.
  assertNoLeak("leading-blank", ["\n\n", ...REAL_CHUNKS]);
});

test("safe partial prose previews before its line terminates", () => {
  const state = newGroundedNoteState();
  // Stream up to (but not including) the newline that ends the first paragraph.
  for (const chunk of REAL_CHUNKS.slice(0, 5)) consumeNoteStreamChunkInState(state, chunk);
  assert.equal(state.lineBuffer.includes("\n"), false, "precondition: first paragraph line is still open");
  const preview = visibleFrom(previewBlocksWithLineBuffer(state));
  assert.match(preview, /This capture contains only the Harvard sentences/, "in-progress prose should preview");
  // The committed blocks (no preview) would still be withholding that prose.
  const committed = visibleFrom(state.blocks);
  assert.ok(!committed.includes("phonetically balanced"), "committed blocks still lag behind the live line");
});

test("previewed block is byte-identical to its finalized form (no reflow)", () => {
  const state = newGroundedNoteState();
  for (const chunk of REAL_CHUNKS.slice(0, 5)) consumeNoteStreamChunkInState(state, chunk);
  const previewedMarkdown = previewBlocksWithLineBuffer(state).map(b => b.markdown);
  // Terminate the current line with no new content: the committed blocks must
  // then equal what the preview already showed — i.e. committing causes no
  // reflow of what the reader was looking at.
  consumeNoteStreamChunkInState(state, "\n");
  const committedMarkdown = state.blocks.map(b => b.markdown);
  assert.deepEqual(committedMarkdown, previewedMarkdown);
});

test("partial MARGINS markers and grounding sections never preview", () => {
  const state = newGroundedNoteState();
  consumeNoteStreamChunkInState(state, "---\ncreated: \"x\"\n---\n# Heading\n\nA sufficiently long paragraph of real prose to be eligible for preview.\n");
  // A marker split mid-flight: the `<` opener must suppress any preview.
  consumeNoteStreamChunkInState(state, "<!-- MARGINS:USE {\"section_id\":\"heading\",\"mem");
  assert.equal(safeVisibleLineBuffer(state), "", "an open marker must never be previewable");
  const preview = visibleFrom(previewBlocksWithLineBuffer(state));
  assert.ok(!looksLikeMetadata(preview), "no marker/metadata in preview");
  assert.match(preview, /sufficiently long paragraph/, "committed prose still shows");
});
