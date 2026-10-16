use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::LazyLock;

use crate::rss::USER_AGENT;

static RE_BARE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^UC[\w-]{22}$").unwrap());
static RE_CHANNEL_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"youtube\.com/channel/(UC[\w-]{22})").unwrap());
static RE_CANONICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<link[^>]+rel="canonical"[^>]+href="https://www\.youtube\.com/channel/(UC[\w-]{22})""#).unwrap()
});
static RE_OG_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta[^>]+property="og:url"[^>]+content="https://www\.youtube\.com/channel/(UC[\w-]{22})""#).unwrap()
});
static RE_EXTERNAL_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""externalId"\s*:\s*"(UC[\w-]{22})""#).unwrap());

static RE_WATCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[?&]v=([\w-]{11})").unwrap());
static RE_SHORT_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"youtube\.com/shorts/([\w-]{11})").unwrap());
static RE_SHORT_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:youtu\.be/|youtube\.com/(?:live|embed|v)/)([\w-]{11})").unwrap()
});
static RE_BARE_VIDEO_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w-]{11}$").unwrap());
static RE_LIST_PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[?&]list=([\w-]+)").unwrap());
/// A playlist id typed or pasted on its own: `PL…` (a user's playlist --
/// `PLEgOOfzzbNNk`, a real one, is only 13 characters), `UU…` (a channel's
/// uploads), `OLAK5uy_…` (an album), and `RD…` so that a bare mix id is refused
/// rather than mistaken for a channel. At least 12 characters, so an
/// 11-character video id that happens to start `PL` stays a video; and none of
/// these prefixes is `UC`, so a 24-character channel id stays a channel.
static RE_BARE_PLAYLIST_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:PL|UU|OLAK5uy_|RD)[\w-]{10,}$").unwrap());

/// Why a mix cannot be added. One sentence wherever a mix is refused, so the
/// Add box says the same thing whichever form it was pasted in.
pub const MIX_REFUSAL: &str =
    "That is a YouTube Mix. Mixes are generated on the fly for each viewer and cannot be added.";

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelInput { Id(String), FetchUrl(String) }

/// What the Add box was handed. A `watch?v=` URL means "this video", not
/// "subscribe to this channel" -- subscribing uses a channel URL or @handle --
/// even when it also carries a `list=`: the link names one video, and
/// [`playlist_id_of`] offers the playlist beside it.
#[derive(Debug, Clone, PartialEq)]
pub enum AddTarget { Channel(ChannelInput), Video(String), Short(String), Playlist(String) }

pub fn parse_add_input(input: &str) -> Result<AddTarget> {
    let t = input.trim();
    if t.is_empty() {
        return Err(anyhow!("Enter a channel URL, @handle, channel ID, or a video or playlist URL"));
    }
    // Shorts are refused everywhere, including here.
    if let Some(c) = RE_SHORT_URL.captures(t) { return Ok(AddTarget::Short(c[1].to_string())); }
    if let Some(c) = RE_WATCH.captures(t) { return Ok(AddTarget::Video(c[1].to_string())); }
    // `embed/videoseries?list=…` is an embedded *playlist*, and "videoseries"
    // is exactly eleven word characters -- it would otherwise read as a video id.
    if let Some(c) = RE_SHORT_PATH.captures(t).filter(|c| &c[1] != "videoseries") {
        return Ok(AddTarget::Video(c[1].to_string()));
    }
    // A YouTube link with a `list=` and no video is the playlist itself:
    // `/playlist?list=`, `/watch?list=` (which plays it from the top), the
    // embed above, music.youtube.com's albums.
    if t.contains("youtube.com/") || t.contains("youtu.be/") {
        if let Some(c) = RE_LIST_PARAM.captures(t) { return playlist_target(&c[1]); }
    }
    // A bare id is a video only if it is not a 24-char channel id.
    if RE_BARE_VIDEO_ID.is_match(t) && !RE_BARE_ID.is_match(t) {
        return Ok(AddTarget::Video(t.to_string()));
    }
    if RE_BARE_PLAYLIST_ID.is_match(t) { return playlist_target(t); }
    Ok(AddTarget::Channel(parse_channel_input(t)?))
}

/// A mix's `RD…` id is a seed, not a list: YouTube generates the videos for
/// whoever opens it, differently each time, so there is nothing fixed to add.
fn is_mix(list_id: &str) -> bool {
    list_id.starts_with("RD")
}

fn playlist_target(list_id: &str) -> Result<AddTarget> {
    if is_mix(list_id) {
        return Err(anyhow!(MIX_REFUSAL));
    }
    Ok(AddTarget::Playlist(list_id.to_string()))
}

/// The playlist a video link was opened from -- `watch?v=…&list=PL…`,
/// `youtu.be/…?list=…` -- so the Add box can offer the whole playlist instead.
/// A mix is not one: on a video link its `list=` is simply ignored, since the
/// video itself is still perfectly addable.
pub fn playlist_id_of(input: &str) -> Option<String> {
    RE_LIST_PARAM
        .captures(input.trim())
        .map(|c| c[1].to_string())
        .filter(|id| !is_mix(id))
}

/// The `@handle` a channel input names, spelled as YouTube reports it in a
/// channel's `uploader_id` (`@AnimeCapped25`): from `@AnimeCapped25` itself or
/// any `youtube.com/@AnimeCapped25/…` link. A handle with non-Latin letters
/// arrives percent-encoded in a copied URL and is decoded, so it compares with
/// the stored one. `None` for everything else, including the `/c/` and
/// `/user/` forms, which only a page fetch can resolve.
pub fn handle_of(input: &str) -> Option<String> {
    let ChannelInput::FetchUrl(url) = parse_channel_input(input).ok()? else { return None };
    let rest = url.split_once("youtube.com/@")?.1;
    let name = rest.split(['/', '?', '#']).next().filter(|n| !n.is_empty())?;
    Some(format!("@{}", percent_decode(name)))
}

/// `%E3%81%82` → `あ`. A malformed escape is kept as it is and bytes that do
/// not form UTF-8 are replaced, so this never fails: it only has to make a
/// copied URL comparable with the handle as YouTube reports it.
fn percent_decode(s: &str) -> String {
    let hex = |b: u8| (b as char).to_digit(16);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_channel_input(input: &str) -> Result<ChannelInput> {
    let t = input.trim();
    if t.is_empty() { return Err(anyhow!("Enter a channel URL, @handle, or channel ID")); }

    if RE_BARE_ID.is_match(t) { return Ok(ChannelInput::Id(t.to_string())); }
    if let Some(c) = RE_CHANNEL_PATH.captures(t) {
        return Ok(ChannelInput::Id(c[1].to_string()));
    }
    if let Some(handle) = t.strip_prefix('@') {
        if !handle.is_empty() && !handle.contains('/') {
            return Ok(ChannelInput::FetchUrl(format!("https://www.youtube.com/@{handle}")));
        }
    }

    let normalized = if t.starts_with("http://") || t.starts_with("https://") {
        t.replacen("http://", "https://", 1)
    } else {
        format!("https://{t}")
    };
    let host_ok = ["https://www.youtube.com/", "https://youtube.com/",
                   "https://m.youtube.com/", "https://youtu.be/"]
        .iter().any(|p| normalized.starts_with(p));
    if !host_ok {
        return Err(anyhow!("Not a YouTube URL: {t}"));
    }
    let normalized = normalized
        .replacen("https://youtube.com/", "https://www.youtube.com/", 1)
        .replacen("https://m.youtube.com/", "https://www.youtube.com/", 1);
    Ok(ChannelInput::FetchUrl(normalized))
}

/// Extracts a channel ID from page HTML.
///
/// Order matters. A bare `"channelId":"UC..."` regex is deliberately NOT used:
/// on a real Veritasium page it matched a nested recommendation blob and
/// returned the wrong channel entirely.
pub fn extract_channel_id(html: &str) -> Option<String> {
    for re in [&*RE_CANONICAL, &*RE_OG_URL, &*RE_EXTERNAL_ID] {
        if let Some(c) = re.captures(html) {
            return Some(c[1].to_string());
        }
    }
    None
}

pub async fn resolve(client: &reqwest::Client, input: &str) -> Result<String> {
    match parse_channel_input(input)? {
        ChannelInput::Id(id) => Ok(id),
        ChannelInput::FetchUrl(url) => {
            let resp = client.get(&url)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
                .send().await?;
            if !resp.status().is_success() {
                return Err(anyhow!("{url} returned HTTP {}", resp.status()));
            }
            let html = resp.text().await?;
            extract_channel_id(&html)
                .ok_or_else(|| anyhow!("Could not find a channel ID at {url}"))
        }
    }
}

pub fn parse_takeout_csv(data: &str) -> Result<Vec<(String, String)>> {
    let cleaned = data.strip_prefix('\u{feff}').unwrap_or(data);
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(cleaned.as_bytes());

    let headers = rdr.headers()?.clone();
    let find = |want: &str| headers.iter().position(|h| h.trim().eq_ignore_ascii_case(want));
    let id_col = find("Channel Id").unwrap_or(0);
    let title_col = find("Channel Title").unwrap_or(2);

    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = match rec { Ok(r) => r, Err(_) => continue };
        let id = rec.get(id_col).unwrap_or("").trim().to_string();
        if !RE_BARE_ID.is_match(&id) { continue; }
        let title = rec.get(title_col).unwrap_or("").trim().to_string();
        out.push((id, title));
    }
    // Alphabetical by title so a several-hundred-row import checklist is
    // scannable. Case-insensitive, with the id as a tiebreaker so the order is
    // stable for channels sharing a name (or missing one).
    out.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "UCHnyfMqiRRG1u-2MsSQLbXA";

    #[test]
    fn bare_ids_and_channel_urls_need_no_fetch() {
        for input in [REAL,
                      "https://www.youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA",
                      "youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA/videos"] {
            match parse_channel_input(input).unwrap() {
                ChannelInput::Id(id) => assert_eq!(id, REAL),
                other => panic!("expected Id for {input}, got {other:?}"),
            }
        }
    }

    #[test]
    fn handles_and_legacy_urls_become_fetchable_urls() {
        let cases = [
            ("@veritasium",                            "https://www.youtube.com/@veritasium"),
            ("https://www.youtube.com/@veritasium",    "https://www.youtube.com/@veritasium"),
            ("youtube.com/@veritasium/videos",         "https://www.youtube.com/@veritasium/videos"),
            ("https://www.youtube.com/c/Veritasium",   "https://www.youtube.com/c/Veritasium"),
            ("https://www.youtube.com/user/1veritasium","https://www.youtube.com/user/1veritasium"),
            ("https://youtu.be/J1WoNuemKOg",           "https://youtu.be/J1WoNuemKOg"),
            ("https://www.youtube.com/watch?v=J1WoNuemKOg",
             "https://www.youtube.com/watch?v=J1WoNuemKOg"),
        ];
        for (input, want) in cases {
            match parse_channel_input(input).unwrap() {
                ChannelInput::FetchUrl(u) => assert_eq!(u, want, "for input {input}"),
                other => panic!("expected FetchUrl for {input}, got {other:?}"),
            }
        }
    }

    #[test]
    fn video_urls_route_to_a_video_not_a_channel_subscription() {
        for input in ["https://www.youtube.com/watch?v=J1WoNuemKOg",
                      "https://youtu.be/J1WoNuemKOg",
                      "youtube.com/watch?v=J1WoNuemKOg&t=30s",
                      "https://www.youtube.com/live/J1WoNuemKOg",
                      "J1WoNuemKOg"] {
            match parse_add_input(input).unwrap() {
                AddTarget::Video(id) => assert_eq!(id, "J1WoNuemKOg", "for {input}"),
                other => panic!("expected Video for {input}, got {other:?}"),
            }
        }
    }

    #[test]
    fn channel_inputs_still_route_to_a_channel() {
        for input in [REAL, "@veritasium", "https://www.youtube.com/@veritasium",
                      "https://www.youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA"] {
            assert!(matches!(parse_add_input(input).unwrap(), AddTarget::Channel(_)),
                    "{input} should be a channel");
        }
    }

    #[test]
    fn shorts_urls_are_refused_at_the_add_box() {
        match parse_add_input("https://www.youtube.com/shorts/b3KpFdb1pW8").unwrap() {
            AddTarget::Short(id) => assert_eq!(id, "b3KpFdb1pW8"),
            other => panic!("expected Short, got {other:?}"),
        }
    }

    #[test]
    fn a_24_char_uc_id_is_a_channel_not_an_11_char_video_id() {
        assert!(matches!(parse_add_input(REAL).unwrap(), AddTarget::Channel(_)));
    }

    #[test]
    fn junk_input_is_rejected() {
        for bad in ["", "   ", "not a url", "https://example.com/whatever"] {
            assert!(parse_channel_input(bad).is_err(), "{bad} should be rejected");
            assert!(parse_add_input(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn canonical_link_is_preferred() {
        let html = format!(r#"<link rel="canonical" href="https://www.youtube.com/channel/{REAL}">"#);
        assert_eq!(extract_channel_id(&html).as_deref(), Some(REAL));
    }

    #[test]
    fn og_url_and_external_id_work_as_fallbacks() {
        let og = format!(r#"<meta property="og:url" content="https://www.youtube.com/channel/{REAL}">"#);
        assert_eq!(extract_channel_id(&og).as_deref(), Some(REAL));
        let ext = format!(r#"{{"externalId":"{REAL}","x":1}}"#);
        assert_eq!(extract_channel_id(&ext).as_deref(), Some(REAL));
    }

    /// Regression for a verified real-world failure: scraping the first
    /// `"channelId"` in Veritasium's page HTML returned UCin0m13qWv3-051xlWlHamA,
    /// a completely different channel from a recommendation blob.
    #[test]
    fn the_channel_id_blob_must_never_be_used() {
        let html = format!(
            r#"{{"channelId":"UCin0m13qWv3-051xlWlHamA"}} ... <link rel="canonical" href="https://www.youtube.com/channel/{REAL}">"#);
        assert_eq!(extract_channel_id(&html).as_deref(), Some(REAL),
                   "canonical must win over a stray channelId blob");

        let only_blob = r#"{"channelId":"UCin0m13qWv3-051xlWlHamA"}"#;
        assert_eq!(extract_channel_id(only_blob), None,
                   "a bare channelId blob must not be trusted as a result");
    }

    #[test]
    fn html_without_any_id_yields_none() {
        assert_eq!(extract_channel_id("<html><body>nothing</body></html>"), None);
    }

    #[test]
    fn takeout_csv_is_parsed() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UCHnyfMqiRRG1u-2MsSQLbXA,http://www.youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA,Veritasium\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,http://www.youtube.com/channel/UCXuqSBlHAE6Xw-yeJA0Tunw,\"Linus Tech Tips, LLC\"\n";
        let rows = parse_takeout_csv(csv).unwrap();
        assert_eq!(rows.len(), 2);
        // Rows come back alphabetically, not in file order.
        assert_eq!(rows[0].1, "Linus Tech Tips, LLC", "quoted commas must survive");
        assert_eq!(rows[1], (REAL.to_string(), "Veritasium".to_string()));
    }

    #[test]
    fn takeout_rows_come_back_alphabetically() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,u,zebra channel\n\
                   UCHnyfMqiRRG1u-2MsSQLbXA,u,Veritasium\n\
                   UCBJycsmduvYEL83R_U4JriQ,u,apple channel\n\
                   UC4QobU6STFB0P71PMvOGN5A,u,Beta\n";
        let titles: Vec<String> =
            parse_takeout_csv(csv).unwrap().into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            titles,
            vec!["apple channel", "Beta", "Veritasium", "zebra channel"],
            "sorting must ignore case, not push lowercase names to the end"
        );
    }

    #[test]
    fn alphabetical_order_is_stable_for_duplicate_titles() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
                   UCXuqSBlHAE6Xw-yeJA0Tunw,u,Same\n\
                   UCHnyfMqiRRG1u-2MsSQLbXA,u,Same\n";
        let ids: Vec<String> =
            parse_takeout_csv(csv).unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["UCHnyfMqiRRG1u-2MsSQLbXA", "UCXuqSBlHAE6Xw-yeJA0Tunw"]);
    }

    #[test]
    fn takeout_csv_tolerates_a_bom_and_skips_malformed_rows() {
        let csv = "\u{feff}Channel Id,Channel Url,Channel Title\n\
                   UCHnyfMqiRRG1u-2MsSQLbXA,url,Veritasium\n\
                   notachannelid,url,Bogus\n";
        let rows = parse_takeout_csv(csv).unwrap();
        assert_eq!(rows.len(), 1, "rows without a valid UC id are skipped");
    }

    // ---- playlists ----

    /// The real test playlist (15 parts, AnimeCapped Manga). Only 13
    /// characters, which is why a bare id is not required to be longer.
    const LIST: &str = "PLEgOOfzzbNNk";

    fn playlist(input: &str) -> String {
        match parse_add_input(input) {
            Ok(AddTarget::Playlist(id)) => id,
            other => panic!("expected Playlist for {input}, got {other:?}"),
        }
    }

    #[test]
    fn playlist_links_in_every_host_form_are_playlists() {
        for input in [
            "https://www.youtube.com/playlist?list=PLEgOOfzzbNNk",
            "http://youtube.com/playlist?list=PLEgOOfzzbNNk",
            "youtube.com/playlist?list=PLEgOOfzzbNNk",
            "https://m.youtube.com/playlist?list=PLEgOOfzzbNNk",
            "https://www.youtube.com/playlist?si=abc123&list=PLEgOOfzzbNNk",
            "https://www.youtube.com/playlist?list=PLEgOOfzzbNNk&si=abc123",
            "  https://www.youtube.com/playlist?list=PLEgOOfzzbNNk  ",
            // A watch link without a video plays the playlist from the top.
            "https://www.youtube.com/watch?list=PLEgOOfzzbNNk",
            // An embedded playlist: "videoseries" must not pass for a video id.
            "https://www.youtube.com/embed/videoseries?list=PLEgOOfzzbNNk",
        ] {
            assert_eq!(playlist(input), LIST, "for {input}");
        }
        assert_eq!(
            playlist("https://music.youtube.com/playlist?list=OLAK5uy_kXk1cmrHJyX2yEK5DdDpmtZiN2Y0hBO2c"),
            "OLAK5uy_kXk1cmrHJyX2yEK5DdDpmtZiN2Y0hBO2c"
        );
        // Watch later and Liked videos are playlists too; the listing reads
        // them with the user's cookies.
        assert_eq!(playlist("https://www.youtube.com/playlist?list=WL"), "WL");
    }

    #[test]
    fn bare_playlist_ids_are_playlists() {
        for id in [LIST,
                   "PLBCF2DAC6FFB574DE",
                   "PL590L5WQmH8fJ54F369BLDSqIwcs-TCfs",
                   "UU5qhtUpMhpBacu9sm3V2fag",
                   "OLAK5uy_kXk1cmrHJyX2yEK5DdDpmtZiN2Y0hBO2c"] {
            assert_eq!(playlist(id), id);
        }
    }

    #[test]
    fn a_bare_playlist_id_never_takes_a_video_or_channel_id() {
        // Eleven characters is a video id, whatever it starts with.
        assert_eq!(parse_add_input("PLaYlIsT123").unwrap(), AddTarget::Video("PLaYlIsT123".into()));
        assert_eq!(parse_add_input("RDxxxxxxxxx").unwrap(), AddTarget::Video("RDxxxxxxxxx".into()));
        // A channel's own id is UC…, its uploads playlist UU…: one letter apart.
        assert!(matches!(parse_add_input(REAL).unwrap(), AddTarget::Channel(ChannelInput::Id(_))));
    }

    #[test]
    fn a_video_link_inside_a_playlist_stays_a_video_and_names_its_playlist() {
        for input in [
            "https://www.youtube.com/watch?v=J1WoNuemKOg&list=PLEgOOfzzbNNk",
            "https://www.youtube.com/watch?list=PLEgOOfzzbNNk&v=J1WoNuemKOg&index=3",
            "https://youtu.be/J1WoNuemKOg?list=PLEgOOfzzbNNk",
            "https://youtu.be/J1WoNuemKOg?si=x&list=PLEgOOfzzbNNk&t=4",
        ] {
            assert_eq!(parse_add_input(input).unwrap(), AddTarget::Video("J1WoNuemKOg".into()), "{input}");
            assert_eq!(playlist_id_of(input).as_deref(), Some(LIST), "{input}");
        }
        assert_eq!(playlist_id_of("https://www.youtube.com/watch?v=J1WoNuemKOg"), None);
        assert_eq!(playlist_id_of("J1WoNuemKOg"), None);
    }

    #[test]
    fn a_mix_is_refused_as_a_playlist_and_ignored_beside_a_video() {
        for input in ["https://www.youtube.com/playlist?list=RDJ1WoNuemKOg",
                      "https://www.youtube.com/watch?list=RDMM",
                      "RDJ1WoNuemKOg"] {
            let err = parse_add_input(input).expect_err(input).to_string();
            assert_eq!(err, MIX_REFUSAL, "{input}");
        }
        let radio = "https://www.youtube.com/watch?v=J1WoNuemKOg&list=RDJ1WoNuemKOg&start_radio=1";
        assert_eq!(parse_add_input(radio).unwrap(), AddTarget::Video("J1WoNuemKOg".into()));
        assert_eq!(playlist_id_of(radio), None, "a mix is never offered as the playlist");
    }

    #[test]
    fn shorts_still_win_over_anything_else_in_the_link() {
        assert!(matches!(
            parse_add_input("https://www.youtube.com/shorts/b3KpFdb1pW8?list=PLEgOOfzzbNNk").unwrap(),
            AddTarget::Short(_)
        ));
    }

    #[test]
    fn handles_are_read_from_every_handle_form() {
        for input in ["@AnimeCapped25",
                      "https://www.youtube.com/@AnimeCapped25",
                      "youtube.com/@AnimeCapped25/videos",
                      "https://m.youtube.com/@AnimeCapped25?si=x",
                      "https://www.youtube.com/@AnimeCapped25#about"] {
            assert_eq!(handle_of(input).as_deref(), Some("@AnimeCapped25"), "{input}");
        }
        // Copied from an address bar, a non-Latin handle is percent-encoded.
        assert_eq!(handle_of("https://www.youtube.com/@%E3%81%82%E3%81%84").as_deref(), Some("@あい"));
        // A stray % that is not an escape survives as itself.
        assert_eq!(handle_of("@100%real").as_deref(), Some("@100%real"));
    }

    #[test]
    fn only_a_handle_is_a_handle() {
        for input in [REAL,
                      "https://www.youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA",
                      "https://www.youtube.com/c/Veritasium",
                      "https://www.youtube.com/user/1veritasium",
                      "https://www.youtube.com/@",
                      "not a url"] {
            assert_eq!(handle_of(input), None, "{input}");
        }
    }
}
