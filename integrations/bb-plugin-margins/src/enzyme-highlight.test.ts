import { describe, expect, it } from "vitest";
import { offsetOf, programErrorLocation, tokenizeEnzyme } from "./enzyme-highlight.js";

const classes = (text: string) => tokenizeEnzyme(text).filter((token) => token.cls).map((token) => [token.text, token.cls]);

describe("Enzyme program highlighting", () => {
  it("classifies keywords, strings, numbers, comments, braces, source kinds, and profiles", () => {
    const program = [
      "// Margins meetings",
      'profile people { seek "who" notice ["roles", "ties"] }',
      "source kind google-mail {",
      '  query """SELECT id',
      '  FROM mail WHERE body = "x" """',
      "}",
      'workspace "notes" {',
      '  source markdown "home" { path "/notes" }',
      '  question budget 12',
      '  learn questions from folder "People" including linked pages about relationships',
      '  learn questions from folder "Projects" about profile { seek "d" notice ["x"] }',
      "  let mine = profile { seek \"s\" notice [\"n\"] }",
      "}",
    ].join("\n");
    const tokens = classes(program);
    expect(tokens).toContainEqual(["// Margins meetings", "comment"]);
    expect(tokens).toContainEqual(["people", "profile"]);
    expect(tokens).toContainEqual(["google-mail", "kind"]);
    expect(tokens).toContainEqual(["markdown", "kind"]);
    expect(tokens).toContainEqual(["relationships", "profile"]);
    expect(tokens).toContainEqual(["mine", "profile"]);
    expect(tokens).toContainEqual(['"""SELECT id\n  FROM mail WHERE body = "x" """', "string"]);
    expect(tokens).toContainEqual(['"notes"', "string"]);
    expect(tokens).toContainEqual(["12", "number"]);
    expect(tokens).toContainEqual(["{", "punctuation"]);
    expect(tokens).toContainEqual(["[", "punctuation"]);
    for (const word of ["workspace", "source", "kind", "learn", "questions", "from", "folder", "including", "about", "budget", "let"]) {
      expect(tokens, word).toContainEqual([word, "keyword"]);
    }
    // `about profile { … }` is an inline profile, not a profile named "profile".
    expect(tokens.filter(([text]) => text === "profile").every(([, cls]) => cls === "keyword")).toBe(true);
    // `source "chat"` in a reading names a declared source; nothing after it is a kind.
    expect(classes('learn questions from source "chat" including who links')).toContainEqual(["including", "keyword"]);
  });

  it("reproduces the text exactly and keeps highlighting text that is still being typed", () => {
    const partial = 'workspace "notes" {\n  source markdown "home { path\n  query """SELECT\n';
    expect(tokenizeEnzyme(partial).map((token) => token.text).join("")).toBe(partial);
    const tokens = classes(partial);
    expect(tokens).toContainEqual(['"home { path', "string"]);
    expect(tokens).toContainEqual(['"""SELECT\n', "string"]);
    expect(tokens).toContainEqual(["query", "keyword"]);
    expect(classes('path "a\\"b" x')).toEqual([["path", "keyword"], ['"a\\"b"', "string"], ["x", "keyword"]]);
  });

  it("reads enzyme-spec error locations and maps them to offsets", () => {
    expect(programErrorLocation('invalid desired program: 3:3: expected "}"; found "lern"')).toEqual({ line: 3, column: 3 });
    expect(programErrorLocation("2:14: unclosed string")).toEqual({ line: 2, column: 14 });
    expect(programErrorLocation('workspace "notes" must declare where Margins writes notes')).toBeNull();
    const text = "ab\ncdef\ng";
    expect(offsetOf(text, { line: 2, column: 3 })).toBe(5);
    expect(offsetOf(text, { line: 2, column: 99 })).toBe(7);
    expect(offsetOf(text, { line: 9, column: 1 })).toBe(text.length);
  });
});
