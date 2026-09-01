//! Recognising the parts of a multi-part upload from their titles alone.
//!
//! Channels that serialise a story across several videos mark the parts in no
//! agreed way: some prepend `(2)`, some append `- Part 2`, some write `Ep. 2`
//! or `#2`, and some change nothing at all and upload the identical title
//! twice. What every one of those has in common is that stripping the marker
//! leaves the same stem behind, so that stem -- the *core* -- is what siblings
//! are matched on.

/// A title reduced to the stem it shares with its siblings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    /// Lowercased, marker-free, punctuation-free, single-spaced.
    pub core: String,
    /// Whether a part marker was found and removed.
    pub had_marker: bool,
}

/// Part-number keywords, longest first so the alternation cannot settle for a
/// prefix (`ch` where `chapter` was meant).
const KW: &str = r"(?:episode|chapter|volume|part|book|vol|pt|ep|ch)";

/// Spelled-out part numbers, longest first (`nine` must not win over
/// `nineteen`). Twenty parts is well past what any real series reaches.
const NUMWORD: &str = r"(?:seventeen|eighteen|thirteen|fourteen|nineteen|fifteen|sixteen|twenty|eleven|twelve|three|seven|eight|four|five|nine|one|two|six|ten)";

/// Roman numerals as an explicit list rather than a general pattern: a real
/// roman regex also matches ordinary words spelled from those letters -- `mix`
/// is a valid numeral, and stripping it from `Song (Mix)` would be wrong.
const ROMAN: &str = r"(?:xviii|viii|xiii|xvii|iii|vii|xii|xiv|xvi|xix|ii|iv|vi|ix|xi|xv|xx|i|v|x)";

/// The same list without bare `i`, `v` and `x`, which carry no part-number
/// sense on their own inside brackets.
const ROMAN_BARE: &str = r"(?:xviii|viii|xiii|xvii|iii|vii|xii|xiv|xvi|xix|ii|iv|vi|ix|xi|xv|xx)";

/// Every way a part number is written, in the order they must be removed:
/// bracketed and keyword forms first, so that the bare leading and trailing
/// number patterns only ever see what those left behind.
fn marker_sources() -> Vec<String> {
    let count = r"(?:\s*(?:of|/)\s*\d+)?";
    // A single upload often covers several parts at once: `1-3`, `1+2`.
    let range = r"(?:\s*[-\u{2013}\u{2014}+]\s*\d+)?";
    vec![
        // (Part 2), [Ep. 3], {Chapter IV}
        format!(r"[\(\[\{{]\s*{KW}\s*\.?\s*(?:\d+{range}|{NUMWORD}|{ROMAN}){count}\s*[\)\]\}}]"),
        // (2), [3], (2/7), (IV)
        format!(r"[\(\[\{{]\s*(?:\d+{range}|{ROMAN_BARE}){count}\s*[\)\]\}}]"),
        // Part 2, Pt.2, Chapter 122-123, Episode 2 of 7
        format!(r"\b{KW}\s*\.?\s*\d+{range}\b{count}"),
        // Part Two, Part III -- a word or numeral needs a real separator,
        // otherwise `Chi` reads as chapter one.
        format!(r"\b{KW}(?:\s*\.\s*|\s+)(?:{NUMWORD}|{ROMAN})\b"),
        // #4, but only at one end: mid-title it is a ranking (`the #1 genius`).
        r"(?:^\s*#\s*\d+|#\s*\d+\s*$)".to_string(),
        // Leading `2 - `, `2. `, `2) `. Capped at three digits so a title
        // opening with a year (`1984: A Novel`) keeps it.
        r"^\s*\d{1,3}\s*[-\u{2013}\u{2014}.:)\]]\s*".to_string(),
        // Trailing ` 2`, ` - 2`, ` 1-3`. Three digits for the same reason.
        format!(r"[\s\-\u{{2013}}\u{{2014}}:|]+\d{{1,3}}{range}\s*$"),
    ]
}

/// The patterns compiled for [`normalize`], whose input is already lowercased.
fn marker_patterns() -> &'static [regex::Regex] {
    static RES: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    RES.get_or_init(|| {
        marker_sources()
            .iter()
            .map(|p| regex::Regex::new(p).expect("literal pattern compiles"))
            .collect()
    })
}

/// The same patterns made case-insensitive, for stripping a marker from a
/// title that has *not* been lowercased -- see [`display_stem`].
fn display_markers() -> &'static [regex::Regex] {
    static RES: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    RES.get_or_init(|| {
        marker_sources()
            .iter()
            .map(|p| regex::Regex::new(&format!("(?i){p}")).expect("literal pattern compiles"))
            .collect()
    })
}

/// `2 of 7` and `2/7` outside brackets, where a bare pair of numbers is only a
/// part marker if it counts up to a plausible total -- `24/7` and `100/100` are
/// prose, and wrongly marking them lets the fuzzy rule loose on the title.
fn bare_count() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // Case-insensitive so the display path, which never lowercases, shares it.
        regex::Regex::new(r"(?i)\b(\d{1,3})\s*(?:of|/)\s*(\d{1,3})\b")
            .expect("literal pattern compiles")
    })
}

/// Strips every `N of M` that counts up, reporting whether it stripped any.
fn strip_bare_counts(s: &str) -> (String, bool) {
    let mut hit = false;
    let out = bare_count().replace_all(s, |c: &regex::Captures| {
        let part = c[1].parse::<u32>().unwrap_or(0);
        let total = c[2].parse::<u32>().unwrap_or(0);
        if part >= 1 && part < total {
            hit = true;
            " ".to_string()
        } else {
            c[0].to_string()
        }
    });
    (out.into_owned(), hit)
}

/// Drops everything that is not a letter, a digit or a space, then squeezes the
/// result down to single spaces. Apostrophes vanish rather than splitting a
/// word, so `don't` stays one token.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\'' | '\u{2019}' => {}
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reduces a title to the stem it would share with its siblings.
pub fn normalize(title: &str) -> Normalized {
    let lowered = title.to_lowercase();
    let (mut s, mut had_marker) = strip_bare_counts(&lowered);
    for re in marker_patterns() {
        if re.is_match(&s) {
            had_marker = true;
            s = re.replace_all(&s, " ").into_owned();
        }
    }
    Normalized { core: tidy(&s), had_marker }
}

/// A shared leading run must be at least this many words, and cover at least
/// this share of the shorter title, before two differing titles are called
/// siblings. Tuned to reject anthology entries that merely open with the same
/// show name.
const MIN_PREFIX_WORDS: usize = 2;
const MIN_PREFIX_PERCENT: usize = 60;

/// Whether two normalised titles look like parts of the same upload.
pub fn is_sibling(a: &Normalized, b: &Normalized) -> bool {
    if a.core.is_empty() || b.core.is_empty() {
        // Titles that are nothing but a part number (`(1)`, `(2)`) still belong
        // together, but an untitled video matches nothing.
        return a.core.is_empty() && b.core.is_empty() && a.had_marker && b.had_marker;
    }
    if a.core == b.core {
        return true;
    }
    // Beyond an exact stem match, only a title that actually carries a part
    // marker may match on a shared opening -- one side is enough, since part
    // one is so often unnumbered. Without that guard, every anthology sharing a
    // show name would read as one enormous series.
    if !(a.had_marker || b.had_marker) {
        return false;
    }
    let wa: Vec<&str> = a.core.split(' ').collect();
    let wb: Vec<&str> = b.core.split(' ').collect();
    let shared = wa.iter().zip(&wb).take_while(|(x, y)| x == y).count();
    let shorter = wa.len().min(wb.len());
    shared >= MIN_PREFIX_WORDS && shared * 100 >= shorter * MIN_PREFIX_PERCENT
}

/// A word reduced to what makes it comparable: its letters and digits,
/// lowercased. Dropping punctuation is what lets `Tapes:` and `Tapes` count as
/// the same word, so a shared opening survives the different separators that
/// different channels put after it.
fn word_key(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// A title as it should be *shown* once its part marker is gone: the patterns
/// [`normalize`] uses, applied to the original text so its casing and its
/// punctuation both survive.
///
/// Matching case-insensitively on the original, rather than mapping match
/// ranges back from a lowercased copy, is deliberate: `to_lowercase()` can
/// change a string's length (Turkish dotted capital I becomes two chars) and
/// every range past that point would then name the wrong bytes.
fn strip_markers_for_display(title: &str) -> String {
    let (mut s, _) = strip_bare_counts(title);
    for re in display_markers() {
        s = re.replace_all(&s, " ").into_owned();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A name for the series these titles form: the words they all share from the
/// start, once their part markers are stripped.
///
/// This is the display counterpart to [`normalize`], whose `core` is lowercased
/// and stripped of punctuation and so can never be shown. `None` when the
/// titles share nothing usable -- titles that are *only* a part number leave an
/// empty stem behind -- and the caller then falls back to a title of its own
/// choosing.
pub fn display_stem(titles: &[&str]) -> Option<String> {
    let stripped: Vec<String> = titles.iter().map(|t| strip_markers_for_display(t)).collect();
    let words: Vec<&str> = stripped.first()?.split(' ').filter(|w| !w.is_empty()).collect();

    let mut shared = words.len();
    for other in stripped.iter().skip(1) {
        let theirs: Vec<&str> = other.split(' ').filter(|w| !w.is_empty()).collect();
        shared = shared.min(
            words.iter().zip(&theirs).take_while(|(a, b)| word_key(a) == word_key(b)).count(),
        );
    }
    if shared == 0 {
        return None;
    }

    // When every part strips down to the same title, that title *is* the series
    // name and whatever punctuation sits inside it belongs to the name. When
    // they differ, the shared run has to stop at the first standalone
    // separator: that is where the removed marker stood, so everything past it
    // -- `The Blackwood Tapes : The` out of two differing subtitles -- belongs
    // to one part rather than to the series.
    let identical = shared == words.len()
        && stripped
            .iter()
            .all(|o| o.split(' ').filter(|w| !w.is_empty()).count() == words.len());
    if !identical {
        if let Some(cut) = words[..shared].iter().position(|w| word_key(w).is_empty()) {
            shared = cut;
        }
    }
    if shared == 0 {
        return None;
    }

    // The words are the first title's, so the stem keeps that title's casing --
    // and, at its end, whatever separator ran into the marker that was removed.
    let stem = words[..shared].join(" ");
    let stem = stem
        .trim_end_matches(|c: char| {
            matches!(c, ':' | '-' | '\u{2013}' | '\u{2014}' | '|' | ',' | '\u{00b7}' | '.' | ' ')
        })
        .trim();
    if stem.is_empty() { None } else { Some(stem.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sib(a: &str, b: &str) -> bool {
        is_sibling(&normalize(a), &normalize(b))
    }

    #[test]
    fn an_unchanged_title_uploaded_twice_is_a_sibling() {
        // The plainest case: the channel numbers nothing at all.
        assert!(sib("The Blackwood Tapes", "The Blackwood Tapes"));
    }

    #[test]
    fn a_prepended_number_is_stripped() {
        assert!(sib("(2) The Blackwood Tapes", "The Blackwood Tapes"));
        assert!(sib("(2) The Blackwood Tapes", "(7) The Blackwood Tapes"));
        assert!(sib("[3] The Blackwood Tapes", "The Blackwood Tapes"));
    }

    #[test]
    fn an_appended_marker_is_stripped() {
        assert!(sib("The Blackwood Tapes - Part 7", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes Part 7", "The Blackwood Tapes Part 1"));
        assert!(sib("The Blackwood Tapes (Part 7)", "The Blackwood Tapes"));
    }

    #[test]
    fn the_common_keyword_spellings_are_all_markers() {
        for marked in [
            "The Blackwood Tapes pt 2",
            "The Blackwood Tapes pt. 2",
            "The Blackwood Tapes Pt.2",
            "The Blackwood Tapes ep 2",
            "The Blackwood Tapes Ep. 2",
            "The Blackwood Tapes Episode 2",
            "The Blackwood Tapes Chapter 2",
            "The Blackwood Tapes Ch. 2",
            "The Blackwood Tapes Vol 2",
            "The Blackwood Tapes Volume 2",
            "The Blackwood Tapes Book 2",
        ] {
            assert!(sib(marked, "The Blackwood Tapes"), "{marked}");
        }
    }

    #[test]
    fn a_hash_number_is_a_marker() {
        assert!(sib("The Blackwood Tapes #4", "The Blackwood Tapes"));
        assert!(sib("#4 The Blackwood Tapes", "The Blackwood Tapes"));
    }

    #[test]
    fn a_count_of_total_is_a_marker() {
        assert!(sib("The Blackwood Tapes 2 of 7", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes (2/7)", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes Part 2 of 7", "The Blackwood Tapes"));
    }

    #[test]
    fn a_spelled_out_part_number_is_a_marker() {
        assert!(sib("The Blackwood Tapes Part Two", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes, Part Thirteen", "The Blackwood Tapes"));
    }

    #[test]
    fn a_roman_part_number_is_a_marker() {
        assert!(sib("The Blackwood Tapes Part III", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes (IV)", "The Blackwood Tapes"));
    }

    #[test]
    fn a_leading_number_with_a_separator_is_a_marker() {
        assert!(sib("2 - The Blackwood Tapes", "The Blackwood Tapes"));
        assert!(sib("2. The Blackwood Tapes", "The Blackwood Tapes"));
        assert!(sib("2) The Blackwood Tapes", "The Blackwood Tapes"));
    }

    #[test]
    fn a_trailing_bare_number_is_a_marker() {
        assert!(sib("The Blackwood Tapes 2", "The Blackwood Tapes"));
        assert!(sib("The Blackwood Tapes - 2", "The Blackwood Tapes"));
    }

    #[test]
    fn punctuation_and_case_do_not_separate_siblings() {
        assert!(sib("THE BLACKWOOD TAPES!", "the blackwood tapes"));
        assert!(sib("The Blackwood Tapes: Part 2", "The  Blackwood   Tapes"));
    }

    #[test]
    fn a_marked_series_matches_across_different_subtitles() {
        // The common shape: a shared stem, a part marker, then a per-part
        // subtitle that differs every time.
        assert!(sib(
            "The Blackwood Tapes Part 1: The Arrival",
            "The Blackwood Tapes Part 2: The Cellar",
        ));
    }

    #[test]
    fn an_unmarked_first_part_still_matches_a_marked_one() {
        // Part one very often carries no number at all.
        assert!(sib(
            "The Blackwood Tapes: The Arrival",
            "The Blackwood Tapes Part 2: The Cellar",
        ));
    }

    #[test]
    fn an_anthology_with_a_shared_show_name_is_not_a_series() {
        // No part marker anywhere, so a shared opening is just branding.
        assert!(!sib("Creepy Tales: The Diner", "Creepy Tales: The Manor"));
    }

    #[test]
    fn a_short_shared_opening_is_not_enough() {
        // Two words shared, but they cover less than the stem should.
        assert!(!sib(
            "Top 10 Horror Stories Part 1",
            "Top 10 Comedy Stories Part 1",
        ));
    }

    #[test]
    fn different_stories_with_the_same_marker_are_not_siblings() {
        assert!(!sib("The Blackwood Tapes Part 1", "The Ashford Letters Part 1"));
    }

    #[test]
    fn unrelated_titles_are_not_siblings() {
        assert!(!sib("How to Bake Bread", "How to Bake Cakes"));
        assert!(!sib("The Blackwood Tapes", "Something Else Entirely"));
    }

    #[test]
    fn a_title_that_is_only_a_part_number_matches_its_own_kind() {
        assert!(sib("(1)", "(2)"));
        // ...but an untitled video is not everyone's sibling.
        assert!(!sib("", ""));
        assert!(!sib("", "(2)"));
    }

    /// Prints what the matcher would actually do to the real library, so the
    /// thresholds above can be judged against real titles rather than invented
    /// ones. Read-only, and opt-in:
    ///
    ///     cargo test cluster_the_real_library -- --ignored --nocapture
    ///
    /// Set `MYTUBE_DB` to point at a copy if the app is running, since SQLite
    /// cannot open a live WAL database read-only.
    #[test]
    #[ignore = "reads the real library"]
    fn cluster_the_real_library() {
        let path = std::env::var("MYTUBE_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            dirs::config_dir().unwrap().join("mytube").join("mytube.db")
        });
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("library opens");

        let mut st = conn
            .prepare(
                "SELECT c.title, v.title FROM videos v
                 JOIN channels c ON c.id = v.channel_id
                 WHERE v.status='ready' ORDER BY c.title, v.id",
            )
            .unwrap();
        let mut by_channel: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap();
        for row in rows {
            let (channel, title) = row.unwrap();
            by_channel.entry(channel).or_default().push(title);
        }

        let (mut videos, mut grouped, mut groups, mut fuzzy_groups) = (0usize, 0usize, 0usize, 0usize);
        let mut missed: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<Vec<usize>> = Default::default();
        for (channel, titles) in &by_channel {
            videos += titles.len();
            let stems: Vec<Normalized> = titles.iter().map(|t| normalize(t)).collect();
            for i in 0..stems.len() {
                // Exactly what the feature does: one anchor against the channel.
                let set: Vec<usize> = (0..stems.len())
                    .filter(|&j| is_sibling(&stems[i], &stems[j]))
                    .collect();
                if set.len() < 2 {
                    // A marked title with no siblings is a miss worth eyeballing.
                    if stems[i].had_marker {
                        missed.push(format!("[{channel}] {}", titles[i]));
                    }
                    continue;
                }
                if !seen.insert(set.clone()) {
                    continue;
                }
                groups += 1;
                grouped += set.len();
                // Groups whose cores differ leaned on the fuzzy prefix rule --
                // the only rule that can invent a series that is not there.
                let fuzzy = set.iter().any(|&j| stems[j].core != stems[i].core);
                if fuzzy { fuzzy_groups += 1; }
                println!("\n[{channel}] {} parts{}", set.len(), if fuzzy { "  (fuzzy)" } else { "" });
                for j in set {
                    println!("    {}", titles[j]);
                }
            }
        }
        println!("\n=== marked but unmatched ({}) ===", missed.len());
        for m in &missed {
            println!("    {m}");
        }
        println!(
            "\n=== {groups} series ({fuzzy_groups} needing the fuzzy rule) across {} channels; \
             {grouped} of {videos} videos grouped ===",
            by_channel.len()
        );
    }

    #[test]
    fn a_series_name_is_what_the_parts_share() {
        assert_eq!(
            display_stem(&["The Blackwood Tapes", "The Blackwood Tapes - Part 7"]).as_deref(),
            Some("The Blackwood Tapes"),
        );
        assert_eq!(
            display_stem(&["(2) The Blackwood Tapes", "(7) The Blackwood Tapes"]).as_deref(),
            Some("The Blackwood Tapes"),
        );
    }

    #[test]
    fn a_series_name_stops_where_the_marker_stood() {
        // Per-part subtitles differ, so the name is the opening they share --
        // not `The Blackwood Tapes : The`, which is where a plain longest
        // common prefix would land, both parts happening to open with "The".
        assert_eq!(
            display_stem(&[
                "The Blackwood Tapes Part 1: The Arrival",
                "The Blackwood Tapes Part 2: The Cellar",
            ])
            .as_deref(),
            Some("The Blackwood Tapes"),
        );
    }

    #[test]
    fn punctuation_inside_a_shared_name_is_part_of_it() {
        // Every part strips to the same title, so nothing here came from a
        // marker and the separator is the channel's own.
        assert_eq!(
            display_stem(&["Hardcore | The Blackwood Tapes 1", "Hardcore | The Blackwood Tapes 2"])
                .as_deref(),
            Some("Hardcore | The Blackwood Tapes"),
        );
    }

    #[test]
    fn a_series_name_keeps_the_first_titles_casing() {
        assert_eq!(
            display_stem(&["THE BLACKWOOD TAPES Part 2", "The Blackwood Tapes"]).as_deref(),
            Some("THE BLACKWOOD TAPES"),
        );
    }

    #[test]
    fn parts_that_are_nothing_but_a_number_have_no_name() {
        assert_eq!(display_stem(&["(1)", "(2)"]), None);
        assert_eq!(display_stem(&[]), None);
    }

    #[test]
    fn a_range_of_parts_is_one_marker() {
        // Real titles: `... Mountain 1—3`, `Chapter 122-123 | ...`, `Recap part 1+2`.
        assert_eq!(normalize("The Blackwood Tapes 1\u{2014}3").core, "the blackwood tapes");
        assert_eq!(normalize("The Blackwood Tapes 1-2").core, "the blackwood tapes");
        assert_eq!(normalize("Chapter 122-123 | The Secret").core, "the secret");
        assert_eq!(normalize("The Blackwood Tapes part 1+2").core, "the blackwood tapes");
    }

    #[test]
    fn an_idiom_that_looks_like_a_fraction_is_not_a_marker() {
        // `24/7` and `100/100` are prose. A part marker counts up: 2 of 7.
        assert!(!normalize("My Skeletons Farm 24/7 to Build an Empire").had_marker);
        assert!(!normalize("His System Gave Him 100/100 Laning").had_marker);
        assert!(normalize("The Blackwood Tapes 2/7").had_marker);
    }

    #[test]
    fn a_hash_number_counts_only_at_either_end() {
        assert!(normalize("#4 The Blackwood Tapes").had_marker);
        assert!(normalize("The Blackwood Tapes #4").had_marker);
        // Mid-title it is a ranking, not a part: `Became The #1 Genius`.
        assert!(!normalize("The Worst Student Became The #1 Genius Overnight").had_marker);
    }

    #[test]
    fn a_marker_records_that_it_was_found() {
        assert!(normalize("The Blackwood Tapes Part 2").had_marker);
        assert!(!normalize("The Blackwood Tapes").had_marker);
    }

    #[test]
    fn the_core_is_the_stem_without_the_marker() {
        assert_eq!(normalize("(2) The Blackwood Tapes!").core, "the blackwood tapes");
    }
}
