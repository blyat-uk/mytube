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

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelInput { Id(String), FetchUrl(String) }

/// What the Add box was handed. A `watch?v=` URL means "download this video",
/// not "subscribe to this channel" — subscribing uses a channel URL or @handle.
#[derive(Debug, Clone, PartialEq)]
pub enum AddTarget { Channel(ChannelInput), Video(String), Short(String) }

pub fn parse_add_input(input: &str) -> Result<AddTarget> {
    let t = input.trim();
    if t.is_empty() {
        return Err(anyhow!("Enter a channel URL, @handle, channel ID, or video URL"));
    }
    // Shorts are refused everywhere, including here.
    if let Some(c) = RE_SHORT_URL.captures(t) { return Ok(AddTarget::Short(c[1].to_string())); }
    if let Some(c) = RE_WATCH.captures(t) { return Ok(AddTarget::Video(c[1].to_string())); }
    if let Some(c) = RE_SHORT_PATH.captures(t) { return Ok(AddTarget::Video(c[1].to_string())); }
    // A bare id is a video only if it is not a 24-char channel id.
    if RE_BARE_VIDEO_ID.is_match(t) && !RE_BARE_ID.is_match(t) {
        return Ok(AddTarget::Video(t.to_string()));
    }
    Ok(AddTarget::Channel(parse_channel_input(t)?))
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
}
