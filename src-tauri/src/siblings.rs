//! Recognising the parts of a multi-part upload from their titles alone.
//!
//! Channels that serialise a story across several videos mark the parts in no
//! agreed way: some prepend `(2)`, some append `- Part 2`, some write `Ep. 2`
//! or `#2`, and some change nothing at all and upload the identical title
//! twice. What every one of those has in common is that stripping the marker
//! leaves the same stem behind, so that stem -- the *core* -- is what siblings
//! are matched on.

use std::collections::HashMap;

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

/// How many titles [`normalize_memo`] remembers before starting over. Several
/// times a real library (3,400 videos), so in practice it never empties.
const MEMO_CAP: usize = 20_000;

/// [`normalize`], remembered by title.
///
/// The grouped feed rebuilds every channel's [`Atoms`] on each page it serves,
/// and running eight regexes over all 3,400 titles of a real library was about
/// half of that walk's time -- for titles that almost never change between one
/// page and the next. Keyed on the title alone because `normalize` is a pure
/// function of it, so there is nothing to invalidate: a retitled video simply
/// looks up a different key. Past [`MEMO_CAP`] the whole map is dropped rather
/// than evicted piecemeal; a cold walk is only the cost every walk used to pay.
fn normalize_memo(title: &str) -> Normalized {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Normalized>>> =
        std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().unwrap().get(title) {
        return hit.clone();
    }
    // Outside the lock: two walks racing on a new title both compute it, which
    // costs less than holding every other walk up behind the regexes.
    let fresh = normalize(title);
    let mut map = memo.lock().unwrap();
    if map.len() >= MEMO_CAP {
        map.clear();
    }
    map.insert(title.to_string(), fresh.clone());
    fresh
}

/// A shared leading run must be at least this many words, and cover at least
/// this share of the shorter title, before two differing titles are called
/// siblings. Tuned to reject anthology entries that merely open with the same
/// show name.
const MIN_PREFIX_WORDS: usize = 2;
const MIN_PREFIX_PERCENT: usize = 60;

/// Whether two normalised titles look like parts of the same upload.
///
/// A convenience over [`Stem`] for one-off comparisons; [`Atoms`] locates each
/// title's words once and compares the stems directly.
pub fn is_sibling(a: &Normalized, b: &Normalized) -> bool {
    Stem::new(a.clone()).matches(&Stem::new(b.clone()))
}

/// A [`Normalized`] title with its words located once.
///
/// Grouping compares every title on a channel against every other one it might
/// join, and splitting both cores into fresh word vectors on each of those
/// comparisons made allocation the bulk of the walk's cost. The words are byte
/// ranges into `core` rather than owned strings, since `tidy` has already
/// squeezed it to single spaces and a space is never part of a multi-byte char.
struct Stem {
    norm: Normalized,
    words: Vec<std::ops::Range<usize>>,
}

impl Stem {
    fn new(norm: Normalized) -> Self {
        let mut words = Vec::new();
        let mut start = 0;
        for (i, b) in norm.core.bytes().enumerate() {
            if b == b' ' {
                words.push(start..i);
                start = i + 1;
            }
        }
        words.push(start..norm.core.len());
        Self { norm, words }
    }

    fn word(&self, i: usize) -> &str {
        &self.norm.core[self.words[i].clone()]
    }

    /// The rule [`is_sibling`] names.
    fn matches(&self, other: &Stem) -> bool {
        let (a, b) = (&self.norm, &other.norm);
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
        let shorter = self.words.len().min(other.words.len());
        let shared = (0..shorter).take_while(|&i| self.word(i) == other.word(i)).count();
        shared >= MIN_PREFIX_WORDS && shared * 100 >= shorter * MIN_PREFIX_PERCENT
    }
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
    let words: Vec<Vec<&str>> = stripped.iter().map(|s| display_words(s)).collect();
    let titles: Vec<&[&str]> = words.iter().map(Vec::as_slice).collect();
    stem_of(&titles)
}

/// A title's words once [`strip_markers_for_display`] has squeezed it.
fn display_words(stripped: &str) -> Vec<&str> {
    stripped.split(' ').filter(|w| !w.is_empty()).collect()
}

/// How many words two stripped titles share from the start, compared by
/// [`word_key`].
fn shared_words(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| word_key(x) == word_key(y)).count()
}

/// [`display_stem`]'s rule over titles already stripped and split into words,
/// so [`series_stem`] can weigh many names while stripping each only once.
fn stem_of(titles: &[&[&str]]) -> Option<String> {
    let words = *titles.first()?;

    let mut shared = words.len();
    for theirs in &titles[1..] {
        shared = shared.min(shared_words(words, theirs));
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
    let identical = shared == words.len() && titles.iter().all(|o| o.len() == words.len());
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

/// [`display_stem`] for parts that may each have carried several names:
/// `parts[0]` is the card's leader, and each part lists its names with its
/// current title first.
///
/// A part renamed to chase the algorithm stays on its card through its old
/// name, but `EVERYTHING CHANGED` shares nothing with the parts beside it, so
/// the current titles alone name nothing and the card would fall back to its
/// leader's title. The series still has a name -- `The Blackwood Tapes Pt 2`
/// carries it -- and finding it means picking one name per part: the pick that
/// leaves the longest stem, counted in words.
///
/// Trying every combination would be exponential in the parts, and the stem
/// never needs it. [`display_stem`] reads only three things: the first title's
/// words, how far the run each other title shares with them reaches, and
/// whether every title is that run exactly. So once the leader's name is
/// fixed, every other part is chosen on its own -- the name sharing the
/// longest run with the leader's, and between equals one that *is* the
/// leader's, since only a card whose parts all strip to one title keeps a
/// separator inside its name (`Hardcore | The Blackwood Tapes`). That pick is
/// the best there is for that leader name, not an approximation of it, which
/// leaves only the leader's own few names to try: one pass over every other
/// part's names for each.
///
/// Every tie goes to the earlier name, current titles first. Another part's
/// former name is picked only when it fits the leader's name better than the
/// current one does, and the leader's only when it gives a longer stem -- so
/// with one name per part this is exactly [`display_stem`] over the current
/// titles, and a rename that changes nothing about the stem changes nothing
/// about the card. The words, and so the casing, are the leader's chosen name's.
///
/// A part with no names is skipped, as though the card did not hold it: it has
/// nothing to say about what the others share, and refusing the whole card
/// over it would throw away a name they agree on. Every row has a current
/// title, so this only guards a caller's slip.
pub fn series_stem(parts: &[Vec<&str>]) -> Option<String> {
    let stripped: Vec<Vec<String>> = parts
        .iter()
        .filter(|names| !names.is_empty())
        .map(|names| names.iter().map(|t| strip_markers_for_display(t)).collect())
        .collect();
    let words: Vec<Vec<Vec<&str>>> = stripped
        .iter()
        .map(|names| names.iter().map(|s| display_words(s)).collect())
        .collect();
    let (leader, rest) = words.split_first()?;

    let mut best: Option<(usize, String)> = None;
    for lead in leader {
        let mut chosen: Vec<&[&str]> = Vec::with_capacity(words.len());
        chosen.push(lead);
        chosen.extend(rest.iter().map(|names| closest_name(lead, names)));
        let Some(stem) = stem_of(&chosen) else { continue };
        // Words, not tokens: a standalone `|` names nothing.
        let len = stem.split(' ').filter(|w| !word_key(w).is_empty()).count();
        if best.as_ref().map_or(true, |(most, _)| len > *most) {
            best = Some((len, stem));
        }
    }
    best.map(|(_, stem)| stem)
}

/// The one of a part's names that leaves the most of `lead` standing as the
/// series name: the longest shared run first, then a name that is `lead`
/// exactly, then the earliest. `names` is never empty -- [`series_stem`] has
/// already dropped a part with none.
fn closest_name<'n, 'w>(lead: &[&str], names: &'n [Vec<&'w str>]) -> &'n [&'w str] {
    let fit = |name: &[&str]| {
        let shared = shared_words(lead, name);
        (shared, shared == lead.len() && name.len() == lead.len())
    };
    let mut best = names[0].as_slice();
    let mut best_fit = fit(best);
    for name in &names[1..] {
        let f = fit(name);
        if f > best_fit {
            (best, best_fit) = (name, f);
        }
    }
    best
}

/// One video as the grouping rule sees it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub title: String,
    /// Every name the video carried before `title` (`video_titles`, v12).
    /// Empty for nearly every row; a renamed video matches on all of them.
    pub former_titles: Vec<String>,
    /// Key shared with every video marked its sibling by hand. `None` -- the
    /// ordinary case -- leaves the title matcher to work alone.
    pub group: Option<String>,
}

/// One channel's videos, bucketed into the indivisible units the grouping rule
/// works in.
///
/// Titles are all the matcher above has, and they run out exactly where a
/// YouTuber renames a follow-up to chase the algorithm: `The Blackwood Tapes
/// Pt 3` and `EVERYTHING CHANGED` are the same show and share nothing. Marking
/// them siblings by hand records what the titles cannot.
///
/// An *atom* is a set of videos that belong on one card whatever the titles
/// say: a manual group, or -- for all but a handful of rows -- a single video
/// on its own. Two rules follow from a manual link being a promise:
///
/// - **An atom is never split**, and an atom pulled into a group arrives whole.
///   Half a promise is worse than none.
/// - **Only the leader's atom seeds.** Every title in it is matched against
///   every other atom, so marking a renamed part teaches the matcher that name;
///   but an atom that joins does *not* then widen the net further. Without that
///   ceiling one loose fuzzy match would weld two unrelated series into a card
///   there is no good way to take apart.
///
/// With no keys anywhere every atom is a single video and the result is exactly
/// what the matcher did before manual marking existed.
///
/// A video renamed after upload is the other way two names come to share a
/// card, and it is treated like a hand-marked atom: every name it ever carried
/// is matched, so the old one keeps the bond a clickbait rename would otherwise
/// break, and a renamed video drawn into a card seeds with its *other* names --
/// never the ones it joined through -- so a part uploaded later under the new
/// name still brings the whole series, while the old name reaches no further
/// than it did before the rename. YouTube vouches for those names belonging
/// together -- it is one video -- which is a stronger promise than any fuzzy
/// match. A video with one name still seeds nothing when it joins; the
/// ceiling is unchanged. Nor does a
/// rename that leaves the stem as it was -- `Part 1` added once part two came
/// out, a title re-cased -- or one to a name that matches nothing: a second
/// name has to be able to reach titles the first cannot before it vouches
/// for anything.
pub struct Atoms {
    ids: Vec<String>,
    /// Each entry's distinct names, its current title first unless that is
    /// one of the inert ones dropped in [`Atoms::new`]. Two names with one
    /// core (a rename that only changed case, punctuation or the part marker)
    /// are one stem, and a name that can match nothing is none, so neither
    /// costs a comparison nor counts as a second name for seeding. An
    /// untitled video has no stem at all.
    stems: Vec<Vec<Stem>>,
    /// Which atom each entry landed in.
    atom_of: Vec<usize>,
    /// Entry indices per atom, in the order the entries came in.
    atoms: Vec<Vec<usize>>,
    by_id: HashMap<String, usize>,
}

impl Atoms {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            if let Some(k) = &e.group {
                buckets.entry(k.clone()).or_default().push(i);
            }
        }

        let n = entries.len();
        let mut atom_of = vec![usize::MAX; n];
        let mut atoms: Vec<Vec<usize>> = Vec::new();
        for members in buckets.into_values() {
            // A key left holding one live member -- the rest deleted since --
            // is not a group, and honouring it would pin a lone card in place.
            if members.len() < 2 {
                continue;
            }
            for &i in &members {
                atom_of[i] = atoms.len();
            }
            atoms.push(members);
        }

        let (mut ids, mut stems) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut by_id = HashMap::with_capacity(n);
        for (i, e) in entries.into_iter().enumerate() {
            if atom_of[i] == usize::MAX {
                atom_of[i] = atoms.len();
                atoms.push(vec![i]);
            }
            // Folded on the core alone, keeping a marker if either name had one:
            // `Stem::matches` reads the marker only to let a *different* core
            // in, so a marked stem matches everything its unmarked twin does,
            // and the pair is one name. Keyed on the whole `Normalized`, the
            // `Part 1` a first part so often gains once part two is up would
            // make it two names, and a seed that reaches nothing new.
            let mut names: Vec<Stem> = Vec::with_capacity(1 + e.former_titles.len());
            for title in std::iter::once(&e.title).chain(&e.former_titles) {
                let norm = normalize_memo(title);
                match names.iter_mut().find(|s| s.norm.core == norm.core) {
                    Some(same) => same.norm.had_marker |= norm.had_marker,
                    None => names.push(Stem::new(norm)),
                }
            }
            // An empty stem with no marker matches no title at all (see
            // `Stem::matches`), so a rename to nothing but emoji is not a
            // second name either. Before former titles this was never more
            // than one inert stem per untitled video; dropping it changes no
            // match.
            names.retain(|s| !s.norm.core.is_empty() || s.norm.had_marker);
            stems.push(names);
            by_id.insert(e.id.clone(), i);
            ids.push(e.id);
        }
        Self { ids, stems, atom_of, atoms, by_id }
    }

    /// Ids of every video belonging on the card `anchor` leads. The anchor
    /// comes first -- callers treat it as the leader -- and the rest follow in
    /// entry order, so the answer never depends on how the atoms happened to be
    /// bucketed.
    ///
    /// Empty when the anchor is not among the entries at all, which is not the
    /// same as a video with no siblings: that one comes back alone.
    pub fn group_led_by(&self, anchor: &str) -> Vec<&str> {
        let Some(&ai) = self.by_id.get(anchor) else { return Vec::new() };
        let home = self.atom_of[ai];

        let mut joined = vec![false; self.atoms.len()];
        joined[home] = true;
        // Which atoms' titles seed the search: the leader's, plus every
        // hand-marked atom drawn in after it. Marking a renamed part records
        // *both* its names, and both keep working from then on -- so pulling in
        // that pair has to bring what its other title reaches, or the series it
        // was marked to join would still arrive in pieces.
        //
        // A renamed video is the same case with YouTube doing the vouching,
        // with one difference: it seeds with its *other* names, never the ones
        // it joined through. Those it shares with what was already here, and
        // seeding with them is exactly what a one-name joiner is refused --
        // were it allowed, a rename to anything at all would let the old name
        // start pulling in titles it never reached before the rename. So a
        // part joining through its old name brings what its new name reaches
        // (a later upload under the new name), and one joining through its new
        // name brings what its old name reaches (the parts uploaded before),
        // and a rename changes nothing else about any card.
        //
        // A singleton with one name that joins adds nothing. That is the
        // ceiling: only names someone vouched for can widen a group, so one
        // loose fuzzy match cannot cascade through a channel.
        let mut seeds = vec![Seed { atom: home, spent: Vec::new() }];
        let mut next = 0;
        while next < seeds.len() {
            let seed = seeds[next].clone();
            next += 1;
            for a in 0..self.atoms.len() {
                if joined[a] || !self.seed_matches(&seed, a) {
                    continue;
                }
                joined[a] = true;
                if self.atoms[a].len() > 1 {
                    seeds.push(Seed { atom: a, spent: Vec::new() });
                    continue;
                }
                let names = &self.stems[self.atoms[a][0]];
                if names.len() > 1 {
                    let spent: Vec<bool> = names
                        .iter()
                        .map(|name| self.live_names(&seed).any(|s| s.matches(name)))
                        .collect();
                    if spent.contains(&false) {
                        seeds.push(Seed { atom: a, spent });
                    }
                }
            }
        }

        let mut out = vec![self.ids[ai].as_str()];
        for i in 0..self.ids.len() {
            if i != ai && joined[self.atom_of[i]] {
                out.push(self.ids[i].as_str());
            }
        }
        out
    }

    /// The names a seed searches with: every name of every video in its atom,
    /// less those a renamed video joined through.
    fn live_names<'s>(&'s self, seed: &'s Seed) -> impl Iterator<Item = &'s Stem> + 's {
        self.atoms[seed.atom].iter().flat_map(move |&i| {
            self.stems[i]
                .iter()
                .enumerate()
                .filter(move |(k, _)| !seed.spent.get(*k).copied().unwrap_or(false))
                .map(|(_, stem)| stem)
        })
    }

    /// Whether any name the seed searches with reads as a part of the same
    /// upload as any name in the atom. The atom's side is compared whole: a
    /// manual group's titles are precisely the extra seeds the user supplied,
    /// and a renamed video's former names are the ones YouTube no longer shows.
    fn seed_matches(&self, seed: &Seed, atom: usize) -> bool {
        self.live_names(seed).any(|s| {
            self.atoms[atom].iter().any(|&j| self.stems[j].iter().any(|name| s.matches(name)))
        })
    }
}

/// An atom [`Atoms::group_led_by`] searches from.
#[derive(Clone)]
struct Seed {
    atom: usize,
    /// For a renamed video drawn in, which of its names (by index into its
    /// stems) it joined through and so does not seed with. Empty for the
    /// leader's atom and a hand-marked one, which seed with every name.
    spent: Vec<bool>,
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

    /* ---------------- atoms: what a manual mark changes ---------------- */

    /// `(id, title, manual group key)`.
    type Row<'a> = (&'a str, &'a str, Option<&'a str>);

    fn led_by(rows: &[Row], anchor: &str) -> Vec<String> {
        let named: Vec<Named> =
            rows.iter().map(|(id, title, group)| (*id, vec![*title], *group)).collect();
        renamed_led_by(&named, anchor)
    }

    /// `(id, every name it carried with the current title first, manual group key)`.
    type Named<'a> = (&'a str, Vec<&'a str>, Option<&'a str>);

    /// [`led_by`] for rows that may have been renamed.
    fn renamed_led_by(rows: &[Named], anchor: &str) -> Vec<String> {
        let entries = rows
            .iter()
            .map(|(id, names, group)| Entry {
                id: (*id).into(),
                title: names[0].into(),
                former_titles: names[1..].iter().map(|t| t.to_string()).collect(),
                group: group.map(str::to_string),
            })
            .collect();
        let mut got: Vec<String> =
            Atoms::new(entries).group_led_by(anchor).into_iter().map(String::from).collect();
        // The anchor leads; the rest are a set as far as the callers care.
        if !got.is_empty() {
            got[1..].sort();
        }
        got
    }

    #[test]
    fn without_any_marks_a_group_is_what_the_titles_share() {
        let rows: &[Row] = &[
            ("a", "The Blackwood Tapes", None),
            ("b", "The Blackwood Tapes Part 2", None),
            ("c", "Something Else Entirely", None),
        ];
        assert_eq!(led_by(rows, "a"), ["a", "b"]);
        assert_eq!(led_by(rows, "c"), ["c"]);
    }

    #[test]
    fn a_hand_marked_pair_groups_though_the_titles_share_nothing() {
        // The case the whole feature exists for: a follow-up renamed to chase
        // the algorithm, which no amount of pattern tuning could ever join.
        let rows: &[Row] = &[
            ("p3", "The Blackwood Tapes Pt 3", Some("g1")),
            ("ec", "EVERYTHING CHANGED", Some("g1")),
        ];
        assert_eq!(led_by(rows, "p3"), ["p3", "ec"]);
        assert_eq!(led_by(rows, "ec"), ["ec", "p3"]);
    }

    #[test]
    fn every_marked_title_seeds_the_automatic_matcher() {
        // `w2` matches neither the anchor nor the anchor's own title -- only the
        // other title in its atom -- and still joins. That is the point of
        // marking: the name you recorded keeps working on its own afterwards.
        let rows: &[Row] = &[
            ("p3", "The Blackwood Tapes Pt 3", Some("g1")),
            ("w1", "Whispers In The Dark", Some("g1")),
            ("w2", "Whispers In The Dark Part 2", None),
        ];
        assert_eq!(led_by(rows, "p3"), ["p3", "w1", "w2"]);
    }

    #[test]
    fn an_auto_joined_title_does_not_seed_further_matches() {
        // `b` joins through the marked title `a`. `c` matches `b` but not `a`,
        // and stays out: only what was marked by hand widens the net, so one
        // loose match cannot cascade through a channel.
        let rows: &[Row] = &[
            ("a", "Nightfall Ashen Court Chronicle Reborn Again", Some("g1")),
            ("z", "An Unrelated Title", Some("g1")),
            ("b", "Nightfall Ashen Court Part 2", None),
            ("c", "Nightfall Ashen Court Of Dawn And Dusk Part 4", None),
        ];
        assert!(is_sibling(
            &normalize("Nightfall Ashen Court Part 2"),
            &normalize("Nightfall Ashen Court Of Dawn And Dusk Part 4"),
        ));
        assert_eq!(led_by(rows, "a"), ["a", "b", "z"]);
    }

    #[test]
    fn a_marked_atom_drawn_in_keeps_seeding() {
        // The realistic shape: parts 1 and 2 match each other, part 2 was
        // marked with the renamed part 3, and part 4 arrived under the new name.
        // Whichever end leads, one card has to hold all four -- a mark that
        // still left the series in pieces would not have done its job.
        let rows: &[Row] = &[
            ("p1", "The Blackwood Tapes", None),
            ("p2", "The Blackwood Tapes Part 2", Some("g1")),
            ("ec", "EVERYTHING CHANGED", Some("g1")),
            ("ec2", "EVERYTHING CHANGED Part 2", None),
        ];
        assert_eq!(led_by(rows, "ec2"), ["ec2", "ec", "p1", "p2"]);
        assert_eq!(led_by(rows, "p1"), ["p1", "ec", "ec2", "p2"]);
    }

    #[test]
    fn a_matched_atom_joins_whole() {
        // `b` matches the anchor and `x` does not, but they were marked
        // together -- so `x` comes too. A manual link promises those two share
        // a card, and half a promise is worse than none.
        let rows: &[Row] = &[
            ("a", "The Blackwood Tapes", None),
            ("b", "The Blackwood Tapes Part 2", Some("g1")),
            ("x", "Nothing Like The Others", Some("g1")),
        ];
        assert_eq!(led_by(rows, "a"), ["a", "b", "x"]);
    }

    #[test]
    fn a_key_left_with_one_member_is_not_a_group() {
        // Its siblings deleted since. The row must fall back to plain title
        // matching rather than sitting pinned to a group of one.
        let rows: &[Row] = &[
            ("a", "The Blackwood Tapes", Some("g1")),
            ("b", "The Blackwood Tapes Part 2", None),
        ];
        assert_eq!(led_by(rows, "a"), ["a", "b"]);
    }

    #[test]
    fn an_anchor_that_is_not_here_has_no_group() {
        assert!(led_by(&[("a", "The Blackwood Tapes", None)], "nope").is_empty());
    }

    /* ---------------- former titles: a rename keeps its bonds ---------------- */

    #[test]
    fn a_part_renamed_to_clickbait_stays_on_its_card() {
        // The rename that used to drop a part off its series: nothing in
        // `EVERYTHING CHANGED` reaches the other parts, and nothing has to,
        // because the name it was uploaded under still does. Whichever part
        // leads, the card is the same three.
        assert!(!sib("EVERYTHING CHANGED", "The Blackwood Tapes"));
        let rows: &[Named] = &[
            ("p1", vec!["The Blackwood Tapes"], None),
            ("p2", vec!["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"], None),
            ("p3", vec!["The Blackwood Tapes Part 3"], None),
            ("x", vec!["Something Else Entirely"], None),
        ];
        assert_eq!(renamed_led_by(rows, "p1"), ["p1", "p2", "p3"]);
        assert_eq!(renamed_led_by(rows, "p2"), ["p2", "p1", "p3"]);
        assert_eq!(renamed_led_by(rows, "p3"), ["p3", "p1", "p2"]);
    }

    #[test]
    fn a_part_uploaded_under_the_new_name_brings_the_whole_series() {
        // `a_marked_atom_drawn_in_keeps_seeding` without the mark: YouTube
        // vouches that both names are one video, so the renamed part seeds
        // with both and the follow-up under the new name finds its way to part
        // one -- from either end of the chain.
        let rows: &[Named] = &[
            ("p1", vec!["The Blackwood Tapes"], None),
            ("p2", vec!["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"], None),
            ("ec2", vec!["EVERYTHING CHANGED Part 2"], None),
        ];
        assert!(!sib("EVERYTHING CHANGED Part 2", "The Blackwood Tapes"));
        assert_eq!(renamed_led_by(rows, "ec2"), ["ec2", "p1", "p2"]);
        assert_eq!(renamed_led_by(rows, "p1"), ["p1", "ec2", "p2"]);
    }

    #[test]
    fn every_name_the_leader_carried_seeds() {
        // Renamed twice. Its current title reaches nothing, its first name
        // reaches the parts it was uploaded beside, and the name it wore in
        // between reaches a follow-up uploaded while it wore it.
        let rows: &[Named] = &[
            ("p3", vec!["SHOCKING", "EVERYTHING CHANGED", "The Blackwood Tapes Pt 3"], None),
            ("p1", vec!["The Blackwood Tapes"], None),
            ("p2", vec!["The Blackwood Tapes Part 2"], None),
            ("ec2", vec!["EVERYTHING CHANGED Part 2"], None),
            ("x", vec!["Something Else Entirely"], None),
        ];
        assert_eq!(renamed_led_by(rows, "p3"), ["p3", "ec2", "p1", "p2"]);
    }

    #[test]
    fn a_cosmetic_rename_does_not_seed_further_matches() {
        // `an_auto_joined_title_does_not_seed_further_matches` with `b`
        // re-cased and re-punctuated since upload. Both names normalise alike,
        // so `b` still has one name, and one name that joins seeds nothing.
        let rows: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], Some("g1")),
            ("z", vec!["An Unrelated Title"], Some("g1")),
            ("b", vec!["NIGHTFALL ASHEN COURT, PART 2!", "Nightfall Ashen Court Part 2"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b", "z"]);
    }

    #[test]
    fn numbering_a_part_after_the_fact_is_not_a_second_name() {
        // Part one gets its `Part 1` once part two exists. Same stem, one with
        // a marker and one without -- and a marked stem already matches
        // everything its unmarked twin does, so the old name adds nothing to
        // match on. Counted as a second name it would make `b` a seed all the
        // same, and `c`, which matches only `b`, would cascade in past the
        // ceiling.
        let rows: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], Some("g1")),
            ("z", vec!["An Unrelated Title"], Some("g1")),
            ("b", vec!["Nightfall Ashen Court Part 1", "Nightfall Ashen Court"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert!(sib(
            "Nightfall Ashen Court Part 1",
            "Nightfall Ashen Court Of Dawn And Dusk Part 4",
        ));
        assert!(!sib(
            "Nightfall Ashen Court Chronicle Reborn Again",
            "Nightfall Ashen Court Of Dawn And Dusk Part 4",
        ));
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b", "z"]);
        // ...and the other way round, the marker dropped by the rename.
        let rows: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], Some("g1")),
            ("z", vec!["An Unrelated Title"], Some("g1")),
            ("b", vec!["Nightfall Ashen Court", "Nightfall Ashen Court Part 1"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b", "z"]);
    }

    #[test]
    fn a_name_with_nothing_to_match_on_is_not_a_second_name() {
        // Renamed to nothing but emoji, which normalise to an empty, unmarked
        // stem that matches no title at all. `b` keeps its old name to match
        // on -- it still joins -- but it has one name that can match anything,
        // so like any one-name joiner it seeds nothing.
        let rows: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], Some("g1")),
            ("z", vec!["An Unrelated Title"], Some("g1")),
            ("b", vec!["\u{1f631}\u{1f631}\u{1f631}", "Nightfall Ashen Court Part 2"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b", "z"]);
        assert_eq!(renamed_led_by(rows, "b"), ["b", "a", "c", "z"]);
    }

    #[test]
    fn a_one_name_joiner_still_seeds_nothing_past_a_renamed_part() {
        // `r` is renamed, so it seeds, and `b` joins through its new name. `c`
        // matches `b` -- `b` carries a marker, `r`'s new name does not -- and
        // nothing else; `b` has one name, so `c` stays out. A renamed part
        // widens the net by its own names and not one step further.
        let rows: &[Named] = &[
            ("a", vec!["The Blackwood Tapes"], None),
            ("r", vec!["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"], None),
            ("b", vec!["Everything Changed Part 2"], None),
            ("c", vec!["Everything Changed Forever"], None),
        ];
        assert!(sib("Everything Changed Part 2", "Everything Changed Forever"));
        assert!(!sib("EVERYTHING CHANGED", "Everything Changed Forever"));
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b", "r"]);
    }

    #[test]
    fn a_renamed_part_does_not_seed_with_the_name_it_joined_through() {
        // `a` reaches `b` only through `b`'s old name, which also reaches `c`
        // while `a`'s own title does not. Before the rename `b` was a one-name
        // joiner and `c` stayed out; renaming `b` to something matching nothing
        // must not change that -- the old name it joined through is spent, and
        // the new one reaches nothing. Seeding with every name would let any
        // rename at all turn an old title into a seed.
        let rows: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], None),
            ("b", vec!["SHOCKING REVEAL", "Nightfall Ashen Court Part 2"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert!(sib("Nightfall Ashen Court Part 2", "Nightfall Ashen Court Of Dawn And Dusk Part 4"));
        assert!(!sib(
            "Nightfall Ashen Court Chronicle Reborn Again",
            "Nightfall Ashen Court Of Dawn And Dusk Part 4",
        ));
        assert_eq!(renamed_led_by(rows, "a"), ["a", "b"]);

        // Unrenamed, the same rows group exactly the same way.
        let before: &[Named] = &[
            ("a", vec!["Nightfall Ashen Court Chronicle Reborn Again"], None),
            ("b", vec!["Nightfall Ashen Court Part 2"], None),
            ("c", vec!["Nightfall Ashen Court Of Dawn And Dusk Part 4"], None),
        ];
        assert_eq!(renamed_led_by(before, "a"), ["a", "b"]);
    }

    #[test]
    fn a_renamed_part_in_a_marked_atom_seeds_with_every_name() {
        // `p1` reaches the atom only through `p2`'s old name; the atom still
        // arrives whole, and then seeds with every name of every member: `p2`'s
        // new one brings `ec2`, `x`'s brings `w2`.
        let rows: &[Named] = &[
            ("p1", vec!["The Blackwood Tapes"], None),
            ("p2", vec!["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"], Some("g1")),
            ("x", vec!["Nothing Like The Others"], Some("g1")),
            ("ec2", vec!["EVERYTHING CHANGED Part 2"], None),
            ("w2", vec!["Nothing Like The Others Part 2"], None),
            ("y", vec!["Something Else Entirely"], None),
        ];
        assert_eq!(renamed_led_by(rows, "p1"), ["p1", "ec2", "p2", "w2", "x"]);
        assert_eq!(renamed_led_by(rows, "ec2"), ["ec2", "p1", "p2", "w2", "x"]);
    }

    /* ---------------- series_stem: naming a card of renamed parts ---------------- */

    fn stem_of_parts(parts: &[&[&str]]) -> Option<String> {
        let parts: Vec<Vec<&str>> = parts.iter().map(|p| p.to_vec()).collect();
        series_stem(&parts)
    }

    #[test]
    fn with_one_name_a_part_a_series_name_is_display_stem() {
        // Byte for byte: the card of a series nobody renamed must not change.
        let cases: &[&[&str]] = &[
            &["The Blackwood Tapes", "The Blackwood Tapes - Part 7"],
            &["(2) The Blackwood Tapes", "(7) The Blackwood Tapes"],
            &["The Blackwood Tapes Part 1: The Arrival", "The Blackwood Tapes Part 2: The Cellar"],
            &["Hardcore | The Blackwood Tapes 1", "Hardcore | The Blackwood Tapes 2"],
            &["Hardcore | The Blackwood Tapes 1", "Hardcore | The Blackwood Tapes: Finale"],
            &["THE BLACKWOOD TAPES Part 2", "The Blackwood Tapes"],
            &["The Blackwood Tapes", "EVERYTHING CHANGED", "The Blackwood Tapes Part 3"],
            &["(1)", "(2)"],
            &["The Blackwood Tapes Part 2"],
            &[],
        ];
        for titles in cases {
            let one_each: Vec<&[&str]> = titles.iter().map(std::slice::from_ref).collect();
            assert_eq!(stem_of_parts(&one_each), display_stem(titles), "{titles:?}");
        }
    }

    #[test]
    fn a_renamed_part_is_named_by_the_name_it_was_uploaded_under() {
        assert_eq!(
            display_stem(&[
                "The Blackwood Tapes",
                "EVERYTHING CHANGED",
                "The Blackwood Tapes Part 3",
            ]),
            None,
        );
        assert_eq!(
            stem_of_parts(&[
                &["The Blackwood Tapes"],
                &["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"],
                &["The Blackwood Tapes Part 3"],
            ])
            .as_deref(),
            Some("The Blackwood Tapes"),
        );
        // A renamed leader too, whose words -- and casing -- name the card.
        assert_eq!(
            stem_of_parts(&[
                &["EVERYTHING CHANGED", "THE BLACKWOOD TAPES Pt 3"],
                &["The Blackwood Tapes"],
                &["The Blackwood Tapes Part 2"],
            ])
            .as_deref(),
            Some("THE BLACKWOOD TAPES"),
        );
    }

    #[test]
    fn a_rename_that_changes_nothing_about_the_stem_keeps_the_current_name() {
        // Both of the leader's names give three words. The current one wins,
        // so a re-cased title moves nothing on its card.
        assert_eq!(
            stem_of_parts(&[
                &["The Blackwood Tapes Part 2", "THE BLACKWOOD TAPES PART 2"],
                &["the blackwood tapes"],
            ])
            .as_deref(),
            Some("The Blackwood Tapes"),
        );
        assert_eq!(
            stem_of_parts(&[
                &["THE BLACKWOOD TAPES Part 2", "The Blackwood Tapes Part 2"],
                &["the blackwood tapes"],
            ])
            .as_deref(),
            Some("THE BLACKWOOD TAPES"),
        );
        // A former name that shares less than the current one is never picked.
        assert_eq!(
            stem_of_parts(&[
                &["The Blackwood Tapes: The Arrival"],
                &["The Blackwood Tapes Part 2: The Cellar", "Blackwood 2"],
            ])
            .as_deref(),
            Some("The Blackwood Tapes"),
        );
    }

    #[test]
    fn a_former_name_that_matches_the_leader_exactly_keeps_its_separator() {
        // Both of part two's names share all five of the leader's words, but
        // only the old one *is* the leader's title, and only a card whose
        // parts strip to one title keeps the `|` inside its name. A pick on the
        // shared run alone would settle for the current name and `Hardcore`.
        assert_eq!(
            display_stem(&[
                "Hardcore | The Blackwood Tapes 1",
                "Hardcore | The Blackwood Tapes: Finale",
            ])
            .as_deref(),
            Some("Hardcore"),
        );
        assert_eq!(
            stem_of_parts(&[
                &["Hardcore | The Blackwood Tapes 1"],
                &["Hardcore | The Blackwood Tapes: Finale", "Hardcore | The Blackwood Tapes 2"],
            ])
            .as_deref(),
            Some("Hardcore | The Blackwood Tapes"),
        );
    }

    #[test]
    fn a_part_with_no_names_is_skipped() {
        assert_eq!(
            stem_of_parts(&[&[], &["The Blackwood Tapes"], &["The Blackwood Tapes Part 2"]])
                .as_deref(),
            Some("The Blackwood Tapes"),
        );
        assert_eq!(stem_of_parts(&[&[], &[]]), None);
        assert_eq!(stem_of_parts(&[]), None);
    }

    #[test]
    fn naming_a_long_renamed_series_is_not_a_search_over_every_combination() {
        // Forty parts of four names each is 4^40 combinations; this would never
        // finish if they were tried. The clickbait opens with the part's own
        // number, so no two parts share a word of it, and the one name that
        // fits is last every time.
        let names: Vec<[String; 4]> = (1..=40)
            .map(|n| {
                [
                    format!("{n} SHOCKING"),
                    format!("{n} Reasons Why"),
                    format!("{n}x Gone Wrong"),
                    format!("The Blackwood Tapes Part {n}"),
                ]
            })
            .collect();
        let parts: Vec<Vec<&str>> =
            names.iter().map(|n| n.iter().map(String::as_str).collect()).collect();
        assert_eq!(series_stem(&parts).as_deref(), Some("The Blackwood Tapes"));
    }
}
