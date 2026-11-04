/**
 * The search box's language, for searches the frontend runs over rows it
 * already holds -- the Downloads tab, whose list is loaded whole. A port of
 * `src-tauri/src/search.rs`, which the feed's search runs through, so one box
 * means the same thing on both tabs:
 *
 * - Whitespace separates terms, and a row must match every one, in any order.
 *   Whitespace inside `(…)`, `[…]` or `"…"` does not split.
 * - `-term` excludes whatever the term matches. A lone `-` is nothing.
 * - `"…"` is a literal phrase.
 * - Anything else is tried as a case-insensitive regex, and taken literally if
 *   it does not compile or reads as something nobody types on purpose (`C++`,
 *   `why?`). A term that is a regex also matches its own text literally.
 * - A term matches any one of the row's fields.
 */

interface Token { exclude: boolean; quoted: boolean; text: string }

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Splits the input into terms; see `split` in search.rs. */
function split(input: string): Token[] {
  const out: Token[] = [];
  const chars = [...input];
  let i = 0;
  const ws = (c: string | undefined) => c !== undefined && /\s/.test(c);
  while (true) {
    while (ws(chars[i])) i++;
    if (i >= chars.length) break;
    const exclude = chars[i] === "-";
    if (exclude) i++;
    const quoted = chars[i] === '"';
    if (quoted) i++;
    let text = "";
    if (quoted) {
      // An unclosed quote takes the rest of the input, as typing one would.
      while (i < chars.length) {
        const c = chars[i++];
        if (c === '"') break;
        text += c;
      }
    } else {
      let parens = 0;
      let inClass = false;
      while (i < chars.length && (inClass || parens > 0 || !ws(chars[i]))) {
        const c = chars[i++];
        text += c;
        if (c === "\\") {
          if (i < chars.length) text += chars[i++];
        } else if (c === "[" && !inClass) inClass = true;
        else if (c === "]" && inClass) inClass = false;
        else if (c === "(" && !inClass) parens++;
        else if (c === ")" && !inClass) parens = Math.max(0, parens - 1);
      }
    }
    if (text) out.push({ exclude, quoted, text });
  }
  return out;
}

/** Whether a term was meant as a regex; see `reads_as_regex` in search.rs. */
function readsAsRegex(text: string): boolean {
  type Prev = "atom" | "greedy" | "lazy";
  let prev: Prev = "atom";
  let inClass = false;
  const chars = [...text];
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i];
    if (c === "\\") { i++; prev = "atom"; continue; }
    if (inClass) { if (c === "]") inClass = false; prev = "atom"; continue; }
    if (c === "[") { inClass = true; prev = "atom"; continue; }
    if (c === "*" || c === "+") {
      if (prev !== "atom") return false;
      prev = "greedy";
      continue;
    }
    if (c === "?") {
      if (prev === "lazy") return false;
      prev = prev === "greedy" ? "lazy" : "greedy";
      continue;
    }
    prev = "atom";
  }
  return !(prev !== "atom" && text.endsWith("?"));
}

function pattern(text: string, quoted: boolean): RegExp {
  const literal = escape(text);
  if (!quoted && literal !== text && readsAsRegex(text)) {
    try {
      new RegExp(text);
      return new RegExp(`(?:${text})|${literal}`, "i");
    } catch {
      // Not a regex after all: searched for as written.
    }
  }
  return new RegExp(literal, "i");
}

/** A parsed search, ready to test rows against. Parse once, match many. */
export function parseSearch(input: string): (fields: readonly string[]) => boolean {
  const terms = split(input).map((t) => ({ exclude: t.exclude, re: pattern(t.text, t.quoted) }));
  return (fields) => terms.every((t) => fields.some((f) => t.re.test(f)) !== t.exclude);
}

/** A stretch of text, and whether a search term matched it. */
export interface Hit { text: string; hit: boolean }

/**
 * Splits `text` into what the search's terms match and what they do not, for
 * drawing the matches. Excluded terms match nothing on a row that is shown,
 * so only the others mark anything; overlapping matches merge.
 */
export function highlighter(input: string): (text: string) => Hit[] {
  const res = split(input)
    .filter((t) => !t.exclude)
    .map((t) => new RegExp(pattern(t.text, t.quoted).source, "gi"));
  return (text) => {
    if (res.length === 0 || !text) return [{ text, hit: false }];
    const spans: [number, number][] = [];
    for (const re of res) {
      re.lastIndex = 0;
      for (let m = re.exec(text); m; m = re.exec(text)) {
        if (m[0].length === 0) { re.lastIndex++; continue; }
        spans.push([m.index, m.index + m[0].length]);
      }
    }
    if (spans.length === 0) return [{ text, hit: false }];
    spans.sort((a, b) => a[0] - b[0]);
    const merged: [number, number][] = [];
    for (const s of spans) {
      const last = merged[merged.length - 1];
      if (last && s[0] <= last[1]) last[1] = Math.max(last[1], s[1]);
      else merged.push([s[0], s[1]]);
    }
    const out: Hit[] = [];
    let at = 0;
    for (const [a, b] of merged) {
      if (a > at) out.push({ text: text.slice(at, a), hit: false });
      out.push({ text: text.slice(a, b), hit: true });
      at = b;
    }
    if (at < text.length) out.push({ text: text.slice(at), hit: false });
    return out;
  };
}
