/**
 * Highlighting for Enzyme workspace programs (`.enzyme`).
 *
 * The lexical rules mirror enzyme-spec's `lex` (v0.2.0): `//` line comments,
 * `"""` multiline strings, JSON-escaped `"` strings, the punctuation
 * `{}[]=,%`, and words of letters, digits, `_` and `-` (digit-led words may
 * also contain `.`). Every name a user chooses is a quoted string, so a bare
 * word is language vocabulary. That lets the highlighter classify words by
 * position instead of keeping a copy of the keyword list that could drift
 * from the grammar: the word after `source` (or `source kind`) names a source
 * kind, and the word after `about`, `let`, or a top-level `profile` names a
 * profile. Unlike the parser, the lexer here never fails: unfinished strings
 * run to the end of their line (or of the text, for `"""`), so text being
 * typed still highlights.
 */

export type EnzymeTokenClass = "comment" | "string" | "number" | "punctuation" | "keyword" | "kind" | "profile";

export interface EnzymeToken {
  text: string;
  /** null for whitespace and characters the language does not accept. */
  cls: EnzymeTokenClass | null;
  /** Offset of the token's first character in the program text. */
  start: number;
}

const PUNCTUATION = "{}[]=,%";
const isWordStart = (c: string) => /[A-Za-z0-9_]/.test(c);

export function tokenizeEnzyme(text: string): EnzymeToken[] {
  const out: EnzymeToken[] = [];
  const words: EnzymeToken[] = [];
  const push = (start: number, end: number, cls: EnzymeTokenClass | null) => {
    const token = { text: text.slice(start, end), cls, start };
    out.push(token);
    return token;
  };
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    const start = i;
    if (/\s/.test(c)) {
      while (i < text.length && /\s/.test(text[i])) i += 1;
      push(start, i, null);
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i += 1;
      push(start, i, "comment");
    } else if (text.startsWith('"""', i)) {
      const close = text.indexOf('"""', i + 3);
      i = close < 0 ? text.length : close + 3;
      push(start, i, "string");
      words.push({ text: "", cls: "string", start });
    } else if (c === '"') {
      i += 1;
      while (i < text.length && text[i] !== "\n") {
        if (text[i] === "\\" && i + 1 < text.length && text[i + 1] !== "\n") { i += 2; continue; }
        if (text[i] === '"') { i += 1; break; }
        i += 1;
      }
      push(start, i, "string");
      words.push({ text: "", cls: "string", start });
    } else if (PUNCTUATION.includes(c)) {
      i += 1;
      words.push(push(start, i, "punctuation"));
    } else if (isWordStart(c)) {
      const extra = /[0-9]/.test(c) ? /[A-Za-z0-9_.-]/ : /[A-Za-z0-9_-]/;
      while (i < text.length && extra.test(text[i])) i += 1;
      const token = push(start, i, /[0-9]/.test(c) ? "number" : wordClass(words, text.slice(start, i)));
      words.push(token);
    } else {
      i += 1;
      push(start, i, null);
    }
  }
  return out;
}

/** The class of a bare word, from the significant tokens before it. */
function wordClass(previous: EnzymeToken[], word: string): EnzymeTokenClass {
  const back = (n: number) => {
    const token = previous[previous.length - n];
    return token && token.cls !== "string" ? token.text : null;
  };
  if (back(1) === "source" && word !== "kind") return "kind";
  if (back(1) === "kind" && back(2) === "source") return "kind";
  if (back(1) === "about" && word !== "profile") return "profile";
  if (back(1) === "let" || back(1) === "profile") return "profile";
  return "keyword";
}

export interface ProgramErrorLocation {
  line: number;
  column: number;
}

/**
 * enzyme-spec prefixes lexer and parser errors with `line:column:` (1-based,
 * columns in characters). Resolve-time and Margins policy errors carry no
 * location, so this returns null for them.
 */
export function programErrorLocation(message: string): ProgramErrorLocation | null {
  const match = /(?:^|[\s:])(\d+):(\d+): /.exec(message);
  if (!match) return null;
  const line = Number(match[1]);
  const column = Number(match[2]);
  return line > 0 && column > 0 ? { line, column } : null;
}

/** The character offset of a 1-based line and column, clamped to the text. */
export function offsetOf(text: string, location: ProgramErrorLocation): number {
  let offset = 0;
  for (let line = 1; line < location.line; line += 1) {
    const next = text.indexOf("\n", offset);
    if (next < 0) return text.length;
    offset = next + 1;
  }
  const end = text.indexOf("\n", offset);
  return Math.min(offset + location.column - 1, end < 0 ? text.length : end);
}
