//! The other parts of a series, as an uploader lists them in a description.
//!
//! Recap channels have taken to giving every part of one story a new
//! clickbait title, which leaves the title matcher in [`crate::siblings`]
//! nothing to match on -- but the same uploaders still list the other parts
//! in each description, and that list is what this module reads.
//!
//! **No "part" wording is required of a link.** Measured on a real library
//! (2026-10-19, 1,601 descriptions): of 2,466 links to another video of the
//! same channel, every one was a part link, in a dozen layouts -- `Part 3
//! https://…`, `PART1:https://…`, `1—2：https://…`, `Parrt 33 https://…`, the
//! label after the link, bare links under a `Part 1` or `Parts:` heading.
//! Recognising "part" in all its spellings would only ever lose some of
//! them; what keeps a link honest is structural, and lives in
//! [`crate::siblings::Atoms`]: it counts only when it lands on another video
//! of the same channel that is already in the library, and a video claimed
//! by two separate shows is linked to neither.
//!
//! The one wording rule is an exclusion: a link whose label says it is a
//! promotion ("my latest video", "other series") is not a part. None of the
//! real library's descriptions says so today; it is a cheap net for the one
//! obvious way an unrelated video would be pulled into a series.

use std::collections::HashSet;
use std::sync::OnceLock;

/// A YouTube *video* URL, with or without its scheme, capturing the id and
/// whatever id character follows it -- the `regex` crate has no lookahead,
/// so an id running on past 11 characters is rejected by the caller instead.
/// Channel, playlist and handle URLs never match: only these paths carry a
/// video id.
fn link_pattern() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(?:https?://)?(?:[a-z0-9-]+\.)*(?:youtube\.com/(?:watch\?(?:[^\s#]*?&)?v=|live/|shorts/|embed/)|youtu\.be/)([a-z0-9_-]{11})([a-z0-9_-]?)",
        )
        .expect("literal pattern compiles")
    })
}

/// Labels that say a link is *not* the next part: a plug for something else
/// the channel made. Whole words only, so `Unpopularity` is not `popular`.
/// `previous` and `next` are deliberately absent -- that is how parts point
/// at each other.
fn promotion_pattern() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?:latest|newest|new videos?|new series|other series|another series|other videos|more videos|recommend(?:ed|ations?)?|popular|favou?rites?|trailers?|teasers?|shorts|playlists?)\b",
        )
        .expect("literal pattern compiles")
    })
}

/// The video ids on one line, in order, with the line's own text once they
/// are taken out.
fn links_on(line: &str) -> (Vec<&str>, String) {
    let mut ids = Vec::new();
    let mut rest = String::with_capacity(line.len());
    let mut last = 0;
    for c in link_pattern().captures_iter(line) {
        let whole = c.get(0).expect("group 0 always matches");
        // `notyoutube.com` holds `youtube.com`; a host only counts when it
        // starts the word.
        let glued = line[..whole.start()]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '.' || ch == '-');
        if glued || !c[2].is_empty() {
            continue;
        }
        rest.push_str(&line[last..whole.start()]);
        rest.push(' ');
        last = whole.end();
        ids.push(c.get(1).expect("the id group matches").as_str());
    }
    rest.push_str(&line[last..]);
    (ids, rest)
}

/// The ids of every other video `description` links to, in the order they
/// first appear.
///
/// A link is judged by its label: its own line's text when that line has
/// any, else the nearest line above that is neither blank nor a link of its
/// own -- the heading of the list it sits in (`Parts:`, `Recommended:`). A
/// heading that reads as a list's label (it ends in a colon) is judged for
/// labelled lines too, so `Other series:` over `Solo Leveling: https://…`
/// lines excludes them all; a title line above the parts that merely says
/// "newest" excludes nothing.
pub fn extract(description: &str, own_id: &str) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    let mut heading = "";
    for line in description.lines() {
        let (ids, rest) = links_on(line);
        if ids.is_empty() {
            if !line.trim().is_empty() {
                heading = line;
            }
            continue;
        }
        let labelled = rest.chars().any(char::is_alphanumeric);
        let list_label = heading.trim_end().ends_with([':', '：']);
        let promoted = if labelled {
            promotion_pattern().is_match(&rest)
                || (list_label && promotion_pattern().is_match(heading))
        } else {
            promotion_pattern().is_match(heading)
        };
        if promoted {
            continue;
        }
        for id in ids {
            if id != own_id && seen.insert(id) {
                out.push(id.to_string());
            }
        }
    }
    out
}

/// The stored form of [`extract`]'s answer: ids joined by single spaces.
pub fn encode(ids: &[String]) -> String {
    ids.join(" ")
}

/// [`encode`] undone. `None` -- a description nobody has parsed -- links
/// nothing, like an empty one.
pub fn decode(stored: Option<&str>) -> Vec<String> {
    stored.map(|s| s.split_whitespace().map(str::to_string).collect()).unwrap_or_default()
}

/// What `videos.part_links` holds for a row whose description is
/// `description`.
pub fn derive(description: Option<&str>, own_id: &str) -> Option<String> {
    description.map(|d| encode(&extract(d, own_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `6MmIixGviow`'s description as fetched 2026-10-19, minus the
    /// paragraphs of prose after the list.
    const EXAMPLE: &str = "(10) Others Fight MONSTERS, He Gets RICH Drawing TALISMANS!

Part 1 https://youtu.be/Ww2TIZtkUGc
Part 2 https://youtu.be/SUV1zsevGH4
Part 3 https://youtu.be/zxqx2NT5EG8
Part 4 https://youtu.be/z2_NL8rYCTo
Part 5 https://youtu.be/yqgFAIBHoNE
Part 6 https://youtu.be/m7f5JOrCDLo
Part 7 https://youtu.be/bg9MmGlZrAw
Part 8 https://youtu.be/WWfHInBa2Fw
Part 9 https://youtu.be/W8SdOlqw7-c

By 40 I had stopped. SUBSCRIBE for more! -N

Join our membership https://www.youtube.com/channel/UC5qhtUpMhpBacu9sm3V2fag/join
";

    #[test]
    fn reads_the_example_description() {
        assert_eq!(
            extract(EXAMPLE, "6MmIixGviow"),
            ids(&["Ww2TIZtkUGc", "SUV1zsevGH4", "zxqx2NT5EG8", "z2_NL8rYCTo", "yqgFAIBHoNE",
                  "m7f5JOrCDLo", "bg9MmGlZrAw", "WWfHInBa2Fw", "W8SdOlqw7-c"]),
        );
    }

    #[test]
    fn layouts_without_part_on_the_line() {
        let list = "Parts:\n- https://youtu.be/AAAAAAAAAAA\n- https://youtu.be/BBBBBBBBBBB\n";
        assert_eq!(extract(list, "x"), ids(&["AAAAAAAAAAA", "BBBBBBBBBBB"]));
        for line in [
            "PART1:https://youtu.be/AAAAAAAAAAA",
            "1—2：https://youtu.be/AAAAAAAAAAA",
            "https://youtu.be/AAAAAAAAAAA    Part One",
            "Parrt 33 https://youtu.be/AAAAAAAAAAA",
            "PART 1\nhttps://youtu.be/AAAAAAAAAAA",
        ] {
            assert_eq!(extract(line, "x"), ids(&["AAAAAAAAAAA"]), "{line}");
        }
    }

    #[test]
    fn url_shapes() {
        for text in [
            "https://www.youtube.com/watch?v=AAAAAAAAAAA&t=10s",
            "youtube.com/watch?feature=share&v=AAAAAAAAAAA",
            "https://m.youtube.com/watch?v=AAAAAAAAAAA",
            "https://www.youtube.com/live/AAAAAAAAAAA",
            "https://youtube.com/shorts/AAAAAAAAAAA",
            "https://www.youtube.com/embed/AAAAAAAAAAA",
            "https://youtu.be/AAAAAAAAAAA?si=x1y2",
            "(https://youtu.be/AAAAAAAAAAA)",
            "HTTPS://YOUTU.BE/AAAAAAAAAAA",
        ] {
            assert_eq!(extract(text, "x"), ids(&["AAAAAAAAAAA"]), "{text}");
        }
        for text in [
            "https://www.youtube.com/playlist?list=PLabcdefghijk",
            "https://www.youtube.com/@AnimeCapped",
            "https://www.youtube.com/channel/UC5qhtUpMhpBacu9sm3V2fag",
            "https://youtu.be/AAAAAAAAAAAB",
            "https://example.com/watch?v=AAAAAAAAAAA",
        ] {
            assert!(extract(text, "x").is_empty(), "{text}");
        }
    }

    #[test]
    fn self_link_is_dropped() {
        let text = "Part 2 https://youtu.be/AAAAAAAAAAA\nThis one https://youtu.be/SELF0000000";
        assert_eq!(extract(text, "SELF0000000"), ids(&["AAAAAAAAAAA"]));
    }

    #[test]
    fn duplicates_collapse() {
        let text = "https://youtu.be/AAAAAAAAAAA\nhttps://www.youtube.com/watch?v=AAAAAAAAAAA";
        assert_eq!(extract(text, "x"), ids(&["AAAAAAAAAAA"]));
    }

    #[test]
    fn promotion_words_exclude() {
        assert!(extract("Watch my latest video: https://youtu.be/AAAAAAAAAAA", "x").is_empty());
        assert!(extract("Other series:\nhttps://youtu.be/AAAAAAAAAAA", "x").is_empty());
        assert!(extract("Check out my NEW SERIES https://youtu.be/AAAAAAAAAAA", "x").is_empty());
        // The exclusion reaches a list under its heading and no further.
        let mixed = "Part 1 https://youtu.be/AAAAAAAAAAA\n\nRecommended:\n\
                     https://youtu.be/BBBBBBBBBBB\nhttps://youtu.be/CCCCCCCCCCC\n";
        assert_eq!(extract(mixed, "x"), ids(&["AAAAAAAAAAA"]));
        // `previous` and `next` are how parts point at each other.
        assert_eq!(extract("Previous part: https://youtu.be/AAAAAAAAAAA", "x"),
                   ids(&["AAAAAAAAAAA"]));
        assert_eq!(extract("Next: https://youtu.be/AAAAAAAAAAA", "x"), ids(&["AAAAAAAAAAA"]));
        // A promotion word inside another word is not one.
        assert_eq!(extract("Unpopularity arc, part 2 https://youtu.be/AAAAAAAAAAA", "x"),
                   ids(&["AAAAAAAAAAA"]));
    }

    #[test]
    fn a_promotion_heading_excludes_its_labelled_list() {
        let other = "Check out my other series:\nSolo Leveling Recap: https://youtu.be/AAAAAAAAAAA\n\
                     Omniscient Reader: https://youtu.be/BBBBBBBBBBB\n";
        assert!(extract(other, "x").is_empty());
        assert!(extract("Other series:\n1. https://youtu.be/AAAAAAAAAAA", "x").is_empty());
        // A heading only counts as one when it reads as a list's label: a
        // title line above the parts that happens to say "newest" is not.
        assert_eq!(extract("The Newest Hero Rises\nPart 1 https://youtu.be/AAAAAAAAAAA", "x"),
                   ids(&["AAAAAAAAAAA"]));
    }

    #[test]
    fn round_trip() {
        let v = ids(&["AAAAAAAAAAA", "BBBBBBBBBBB"]);
        assert_eq!(decode(Some(&encode(&v))), v);
        assert!(decode(Some("")).is_empty());
        assert!(decode(None).is_empty());
        assert_eq!(derive(None, "x"), None);
        assert_eq!(derive(Some(""), "x"), Some(String::new()));
        assert_eq!(derive(Some("Part 1 https://youtu.be/AAAAAAAAAAA"), "x"),
                   Some("AAAAAAAAAAA".to_string()));
    }
}
