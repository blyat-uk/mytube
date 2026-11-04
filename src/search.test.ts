import { describe, it, expect } from "vitest";
import { highlighter, parseSearch } from "./search";

/** The titles a query keeps -- the same cases search.rs's own tests run, so
 *  the box means one thing on both tabs. */
const hits = (query: string, titles: string[]) => {
  const shows = parseSearch(query);
  return titles.filter((t) => shows([t]));
};

describe("parseSearch, as search.rs reads a query", () => {
  it("matches a plain word anywhere, in any case", () => {
    expect(hits("cotton", ["100% COTTON", "wool"])).toEqual(["100% COTTON"]);
    expect(hits("émile", ["ÉMILE ZOLA", "emile"])).toEqual(["ÉMILE ZOLA"]);
  });

  it("needs every word, in any order", () => {
    expect(hits("fox red", ["red fox jumps", "fox, red", "red hen"])).toEqual(["red fox jumps", "fox, red"]);
  });

  it("reads regexes, keeping spaces inside a group or class", () => {
    expect(hits("term.*other", ["term then other", "other term"])).toEqual(["term then other"]);
    expect(hits("(part one|finale)", ["the part one", "the finale", "part two"])).toEqual(["the part one", "the finale"]);
    expect(hits("part[ ]one", ["the part one", "partone"])).toEqual(["the part one"]);
  });

  it("excludes with a minus, and ignores a lone one", () => {
    const titles = ["this one", "this butnotthisone", "that"];
    expect(hits("this -butnotthisone", titles)).toEqual(["this one"]);
    expect(hits("-this", titles)).toEqual(["that"]);
    expect(hits("part - two", ["part two", "part - two", "two"])).toEqual(["part two", "part - two"]);
  });

  it("takes a quoted phrase literally, an unclosed one to the end", () => {
    const titles = ["[4K] walking tour", "4k walk", "walking in k"];
    expect(hits('"[4K]"', titles)).toEqual(["[4K] walking tour"]);
    expect(hits('-"[4K]"', titles)).toEqual(["4k walk", "walking in k"]);
    expect(hits('"red fox', ["a red fox", "fox red"])).toEqual(["a red fox"]);
  });

  it("finds a regex's own text, and takes what nobody writes as one literally", () => {
    expect(hits("$100", ["win $100 now", "100 days"])).toEqual(["win $100 now"]);
    expect(hits("(one|tw", ["(one|two)", "one"])).toEqual(["(one|two)"]);
    expect(hits("C++", ["learn C++ fast", "C fast"])).toEqual(["learn C++ fast"]);
    expect(hits("why?", ["But why?", "what now"])).toEqual(["But why?"]);
    expect(hits("colou?r", ["color", "colour", "colr"])).toEqual(["color", "colour"]);
  });

  it("lets a term match any one field, and an empty query match everything", () => {
    const shows = parseSearch("alpha -video");
    expect(shows(["anything", "Alpha Channel"])).toBe(true);
    expect(shows(["a video", "Alpha Channel"])).toBe(false);
    expect(parseSearch("   ")(["a", "b"])).toBe(true);
  });
});

describe("highlighter", () => {
  it("marks what each kept term matches, merging overlaps", () => {
    const mark = highlighter("tapes black -nope");
    expect(mark("The Blackwood Tapes")).toEqual([
      { text: "The ", hit: false }, { text: "Black", hit: true }, { text: "wood ", hit: false },
      { text: "Tapes", hit: true },
    ]);
    expect(highlighter("ab abc")("xabcx")).toEqual([
      { text: "x", hit: false }, { text: "abc", hit: true }, { text: "x", hit: false },
    ]);
  });

  it("marks nothing without a search", () => {
    expect(highlighter("")("Anything")).toEqual([{ text: "Anything", hit: false }]);
  });
});
