//! The feed's search box: words, plus a little regex and exclusion that
//! nothing on screen advertises.
//!
//! Pure: no I/O. [`Query::parse`] runs once per statement -- `db.rs` exposes
//! it to SQLite as `search_match`, which keeps the parse as auxiliary data --
//! and [`Query::matches`] once per row.
//!
//! - Whitespace separates terms, and a video must match every one, in any
//!   order. Whitespace inside `(…)`, `[…]` or `"…"` does not split, so
//!   `(part one|finale)` stays one term.
//! - `-term` excludes whatever the term matches. A lone `-` is nothing: it is
//!   either half-typed or a separator pasted from a title.
//! - `"…"` is a literal phrase -- no regex, spaces kept -- for the rare search
//!   that means its brackets (`"[4K]"`).
//! - Anything else is tried as a case-insensitive regex, and taken literally if
//!   it does not compile (a half-typed `(one|tw`) or compiles into something
//!   nobody types on purpose (`C++`, `why?`; see [`reads_as_regex`]). A term
//!   that is a regex *also* matches its own text literally, because titles are
//!   full of characters a regex reads differently: `$100` is an anchor
//!   mid-pattern and matches nothing, `1+1` never matches "1+1", `^_^` cannot
//!   match at all. Ordinary words have no such characters, so for them this
//!   changes nothing.
//! - A term matches the video's title or its channel's name, either one, as
//!   the plain search always has.

use regex::Regex;

/// A parsed search: every term must hold.
pub struct Query {
    terms: Vec<Term>,
}

struct Term {
    exclude: bool,
    re: Regex,
}

/// One whitespace-separated piece of the input, before it becomes a pattern.
#[derive(Debug, PartialEq)]
struct Token {
    exclude: bool,
    quoted: bool,
    text: String,
}

impl Query {
    pub fn parse(input: &str) -> Query {
        let terms = split(input)
            .into_iter()
            .map(|t| Term { exclude: t.exclude, re: pattern(&t.text, t.quoted) })
            .collect();
        Query { terms }
    }

    /// Whether a video with this title, on a channel of this name, is shown.
    /// A query with no terms shows everything.
    pub fn matches(&self, title: &str, channel: &str) -> bool {
        self.terms.iter().all(|t| (t.re.is_match(title) || t.re.is_match(channel)) != t.exclude)
    }
}

/// The term as a regex, case-insensitive. A quoted term, or one that does not
/// compile, is its text escaped; one that compiles is that regex *or* its text
/// escaped (see the module docs for why both).
fn pattern(text: &str, quoted: bool) -> Regex {
    let literal = regex::escape(text);
    // `text` is checked alone first: wrapped in `(?:…)` an unbalanced term
    // could pair its stray parenthesis with the wrapper's and compile into
    // something nobody typed.
    if !quoted && literal != text && reads_as_regex(text) && Regex::new(text).is_ok() {
        if let Ok(re) = Regex::new(&format!("(?i)(?:{text})|{literal}")) {
            return re;
        }
    }
    Regex::new(&format!("(?i){literal}")).expect("an escaped literal compiles")
}

/// Whether a term was meant as a regex, before asking whether it compiles. Two
/// shapes compile and are still never written as one: a quantifier on a
/// quantifier (`C++`, `Notepad++`), which the regex crate takes as a repetition
/// of a repetition and so matches any "c" at all; and a trailing `?` (`why?`),
/// which only makes the last character optional -- for a match anywhere in a
/// title, `why?` is `wh`. A lazy `+?`, `*?` or `??` before more pattern is a
/// real regex and passes.
fn reads_as_regex(text: &str) -> bool {
    #[derive(PartialEq)]
    enum Prev { Atom, Greedy, Lazy }
    let (mut prev, mut in_class) = (Prev::Atom, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        prev = match c {
            '\\' => { chars.next(); Prev::Atom }
            ']' if in_class => { in_class = false; Prev::Atom }
            _ if in_class => Prev::Atom,
            '[' => { in_class = true; Prev::Atom }
            '*' | '+' if prev != Prev::Atom => return false,
            '*' | '+' => Prev::Greedy,
            '?' if prev == Prev::Lazy => return false,
            '?' if prev == Prev::Greedy => Prev::Lazy,
            '?' => Prev::Greedy,
            _ => Prev::Atom,
        };
    }
    !(prev != Prev::Atom && text.ends_with('?'))
}

/// Splits the input into terms. Brackets and parentheses are tracked only well
/// enough to keep a regex's spaces inside it; one left unbalanced runs to the
/// end of the input and, failing to compile, is searched for literally.
fn split(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() { break; }

        let exclude = chars.next_if_eq(&'-').is_some();
        let quoted = chars.next_if_eq(&'"').is_some();
        let mut text = String::new();
        if quoted {
            // An unclosed quote takes the rest of the input, as typing one would.
            for c in chars.by_ref() {
                if c == '"' { break; }
                text.push(c);
            }
        } else {
            let (mut parens, mut in_class) = (0usize, false);
            while let Some(c) = chars.next_if(|&c| in_class || parens > 0 || !c.is_whitespace()) {
                text.push(c);
                match c {
                    '\\' => text.extend(chars.next()),
                    '[' if !in_class => in_class = true,
                    ']' if in_class => in_class = false,
                    '(' if !in_class => parens += 1,
                    ')' if !in_class => parens = parens.saturating_sub(1),
                    _ => {}
                }
            }
        }
        if !text.is_empty() {
            out.push(Token { exclude, quoted, text });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which of `titles` (all on a channel called "Chan") the query keeps.
    fn hits<'a>(query: &str, titles: &[&'a str]) -> Vec<&'a str> {
        let q = Query::parse(query);
        titles.iter().copied().filter(|t| q.matches(t, "Chan")).collect()
    }

    #[test]
    fn a_plain_word_matches_anywhere_in_any_case() {
        assert_eq!(hits("cotton", &["100% COTTON", "wool"]), ["100% COTTON"]);
        // Unicode case folding, which LIKE never did.
        assert_eq!(hits("émile", &["ÉMILE ZOLA", "emile"]), ["ÉMILE ZOLA"]);
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        let titles = ["red fox jumps", "fox, red", "red hen"];
        assert_eq!(hits("fox red", &titles), ["red fox jumps", "fox, red"]);
    }

    #[test]
    fn a_wildcard_regex_spans_the_gap() {
        let titles = ["term then other", "other then term", "term"];
        assert_eq!(hits("term.*other", &titles), ["term then other"]);
    }

    #[test]
    fn an_alternation_matches_any_branch() {
        let titles = ["one fish", "two fish", "three fish", "four fish"];
        assert_eq!(hits("(one|two|three)", &titles), ["one fish", "two fish", "three fish"]);
        assert_eq!(hits("one|two", &titles), ["one fish", "two fish"]);
    }

    #[test]
    fn spaces_inside_a_group_or_class_do_not_split_it() {
        let titles = ["the part one", "the finale", "part of one"];
        assert_eq!(hits("(part one|finale)", &titles), ["the part one", "the finale"]);
        assert_eq!(hits("part[ ]one", &titles), ["the part one"]);
        // A parenthesis inside a class does not open a group.
        assert_eq!(hits("[(] x", &["(a x", "a x"]), ["(a x"]);
    }

    #[test]
    fn a_minus_excludes_what_it_matches() {
        let titles = ["this one", "this and butnotthisone", "that"];
        assert_eq!(hits("this -butnotthisone", &titles), ["this one"]);
        assert_eq!(hits("-this", &titles), ["that"]);
        assert_eq!(hits("this -(one|and)", &titles), Vec::<&str>::new());
    }

    #[test]
    fn a_lone_minus_is_ignored() {
        assert_eq!(hits("part - two", &["part two", "part - two", "two"]), ["part two", "part - two"]);
        assert_eq!(hits("-", &["a", "b"]), ["a", "b"]);
    }

    #[test]
    fn a_quoted_phrase_is_literal() {
        let titles = ["[4K] walking tour", "4k walk", "walking in k"];
        assert_eq!(hits("\"[4K]\"", &titles), ["[4K] walking tour"]);
        assert_eq!(hits("\"tour walking\"", &["walking tour", "tour walking"]), ["tour walking"]);
        assert_eq!(hits("-\"[4K]\"", &titles), ["4k walk", "walking in k"]);
        // Unclosed: the rest of the input is the phrase.
        assert_eq!(hits("\"red fox", &["a red fox", "fox red"]), ["a red fox"]);
    }

    #[test]
    fn a_regex_still_finds_its_own_text() {
        assert_eq!(hits("$100", &["win $100 now", "100 days"]), ["win $100 now"]);
        assert_eq!(hits("1+1", &["1+1=2", "11"]), ["1+1=2", "11"]);
        assert_eq!(hits("^_^", &["happy ^_^", "sad"]), ["happy ^_^"]);
    }

    #[test]
    fn a_term_that_does_not_compile_is_literal() {
        assert_eq!(hits("(one|tw", &["(one|two)", "one"]), ["(one|two)"]);
        // Unbalanced, the wrapper must not close it: `a)|(?:b` alone is no regex.
        assert_eq!(hits("a)|(b", &["a", "b", "x a)|(b"]), ["x a)|(b"]);
    }

    #[test]
    fn a_term_no_one_writes_as_a_regex_is_literal() {
        assert_eq!(hits("C++", &["learn C++ fast", "C fast"]), ["learn C++ fast"]);
        assert_eq!(hits("notepad++", &["Notepad++ tips", "notes"]), ["Notepad++ tips"]);
        assert_eq!(hits("why?", &["But why?", "what now"]), ["But why?"]);
        assert_eq!(hits("what??", &["what?? no", "what now"]), ["what?? no"]);
        // Inside a class or escaped, those characters are no quantifier.
        assert_eq!(hits("[+*]x", &["a+x", "ax"]), ["a+x"]);
        assert_eq!(hits(r"c\+\+", &["C++", "C"]), ["C++"]);
        // A lazy quantifier, and an optional one mid-term, are regex.
        assert_eq!(hits("a.+?z", &["a to z", "az"]), ["a to z"]);
        assert_eq!(hits("colou?r", &["color", "colour", "colr"]), ["color", "colour"]);
    }

    #[test]
    fn a_term_may_match_the_channel_instead() {
        let q = Query::parse("alpha");
        assert!(q.matches("anything", "Alpha Channel"));
        let q = Query::parse("video -alpha");
        assert!(!q.matches("a video", "Alpha Channel"));
        assert!(q.matches("a video", "Beta"));
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert!(Query::parse("").matches("a", "b"));
        assert!(Query::parse("   ").matches("a", "b"));
        assert!(Query::parse("\"\"").matches("a", "b"));
    }

    #[test]
    fn splitting_keeps_escapes_and_marks_kinds() {
        let t = |exclude, quoted, text: &str| Token { exclude, quoted, text: text.into() };
        assert_eq!(split(r"a\ b -c -"), [t(false, false, r"a\ b"), t(true, false, "c")]);
        assert_eq!(split("-\"x y\" (p q"), [t(true, true, "x y"), t(false, false, "(p q")]);
    }
}
