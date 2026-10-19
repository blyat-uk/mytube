//! One video, read from its watch page with a plain GET -- no yt-dlp.
//!
//! The Add box previews a pasted video and adds it from this: one GET answers
//! in about a second where a yt-dlp probe runs an extraction, and it works
//! before the managed yt-dlp has been downloaded at all.
//! The probe stays as the fallback for a page that does not parse (a private
//! video the user's cookies can see, a page YouTube has reshaped).
//!
//! What the page carries, checked 2026-10-16 on `dI1xnt0sw5I` (public) and
//! `yi8Ehs1Mrwc` (members-only, fetched signed out):
//!
//! - the player response's `"videoDetails":{"videoId":…}`: `title`,
//!   `lengthSeconds` (a string), `channelId`, `author` -- present for a
//!   members-only video too, whose stream is withheld but whose details are not;
//! - its `"playabilityStatus"`, which for a members-only video signed out is
//!   `UNPLAYABLE` with an offer whose `offerId` is `sponsors_only_video` (the
//!   human-readable `reason` beside it is in whatever language YouTube guessed
//!   from the IP -- Bulgarian, on the machine this was checked from -- so it is
//!   never matched on);
//! - the ld+json `uploadDate` that [`upload_date::parse_upload_date`] reads;
//! - `playerMicroformatRenderer.canonicalUrl`, which is `/shorts/<id>` for a
//!   Short (`8Pae6Wy1abE`) and `/watch?v=<id>` otherwise;
//! - `videoDetails.shortDescription`, the description in full despite its
//!   name -- the same text the RSS feed carries (checked 2026-10-19), and what
//!   [`crate::part_links`] reads a series' other parts from.

use std::sync::OnceLock;

use serde_json::Value;

use crate::rss::USER_AGENT;
use crate::{upload_date, ytdlp};

/// Answers the EU consent interstitial. Without it a request from a European
/// address is 302'd to `consent.youtube.com` before it reaches anything, and
/// for `/shorts/` -- whose whole answer is a status and a redirect -- that
/// hides the answer entirely. Verified 2026-10-16: `/shorts/8Pae6Wy1abE`
/// answers 200 with it. It identifies nobody; it is the "reject all" answer.
pub(crate) const CONSENT_COOKIE: &str = "SOCS=CAI";

/// What MyTube needs from one watch page.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchPage {
    pub id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_title: Option<String>,
    /// `lengthSeconds`; `None` for `"0"`, which is what an upcoming premiere or
    /// a live stream reports.
    pub duration_secs: Option<i64>,
    /// The real upload instant, from the ld+json.
    pub published_at: Option<i64>,
    /// The page offers a channel membership in place of the video. Read
    /// signed out, so it says what the video is, never whether you may watch.
    pub members_only: bool,
    /// YouTube's own canonical URL for the video is `/shorts/<id>`. The
    /// `/shorts/` probe in [`is_short`] is the real test; this is what the page
    /// already says, for when that probe could not answer.
    pub canonical_short: bool,
    /// The description: `""` when the details say it is empty, `None` when
    /// they do not say. See [`parse_description`].
    pub description: Option<String>,
}

/// The JSON object starting right after `key` in `html`, where the object
/// itself must begin with `starts`.
///
/// `starts` is what tells the object wanted from its namesakes: the page holds
/// `"videoDetails":{"playerOverlayVideoDetailsRenderer":…}` as well as the
/// player response's `"videoDetails":{"videoId":…}`, and which comes first is
/// not something to rely on. A stream deserializer reads exactly one value and
/// stops, so the megabyte of page after it is never looked at.
fn object_at(html: &str, key: &str, starts: &str) -> Option<Value> {
    let at = html.find(&format!("{key}{starts}"))? + key.len();
    let value = serde_json::Deserializer::from_str(&html[at..])
        .into_iter::<Value>()
        .next()?
        .ok()?;
    value.is_object().then_some(value)
}

/// The description from `video_id`'s watch page, untrimmed. `None` when the
/// page has no player details -- a consent wall, a bot check, a removed
/// video -- or details without the key, or details about another video: all
/// of them "nothing was read", never "no description", and the caller must
/// not store any of them as one, since a stored description is never asked
/// for again. `Some("")` only when the key is there and empty.
pub fn parse_description(html: &str, video_id: &str) -> Option<String> {
    let details = object_at(html, r#""videoDetails":"#, r#"{"videoId""#)?;
    if str_field(&details, "videoId").as_deref() != Some(video_id) {
        return None;
    }
    description_of(&details)
}

fn description_of(details: &Value) -> Option<String> {
    details.get("shortDescription").and_then(Value::as_str).map(str::to_string)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads a watch page. `None` when it carries no player details at all -- a
/// private, removed or nonexistent video, or a page YouTube has reshaped --
/// which callers take as "ask yt-dlp instead".
pub fn parse(html: &str) -> Option<WatchPage> {
    let details = object_at(html, r#""videoDetails":"#, r#"{"videoId""#)?;
    let id = str_field(&details, "videoId")?;
    let title = str_field(&details, "title")?;
    let duration_secs = details
        .get("lengthSeconds")
        .and_then(|x| x.as_str().and_then(|s| s.parse::<i64>().ok()).or_else(|| x.as_i64()))
        .filter(|d| *d > 0);

    // The offer's wording is localised; its id is not. Searched for anywhere
    // in the status rather than at one path, because the renderer that carries
    // it (`playerLegacyDesktopYpcOfferRenderer` today) is the part most likely
    // to be renamed.
    let members_only = object_at(html, r#""playabilityStatus":"#, r#"{"status""#)
        .is_some_and(|s| s.to_string().contains(r#""sponsors_only_video""#));

    let canonical_short = object_at(html, r#""playerMicroformatRenderer":"#, "{")
        .and_then(|m| str_field(&m, "canonicalUrl"))
        .is_some_and(|u| u.contains("/shorts/"));

    Some(WatchPage {
        id,
        title,
        channel_id: str_field(&details, "channelId"),
        channel_title: str_field(&details, "author"),
        duration_secs,
        published_at: upload_date::parse_upload_date(html),
        members_only,
        canonical_short,
        description: description_of(&details),
    })
}

/// GETs and parses one watch page. `None` on any failure, including a page
/// that turns out to be about a different video than the one asked for.
pub async fn fetch(http: &reqwest::Client, video_id: &str) -> Option<WatchPage> {
    let resp = http
        .get(ytdlp::watch_url(video_id))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::COOKIE, CONSENT_COOKIE)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    parse(&resp.text().await.ok()?).filter(|p| p.id == video_id)
}

/// A client that never follows a redirect: [`is_short`]'s whole answer *is*
/// whether YouTube redirects, and a client that followed it would see a 200
/// either way and call every video a Short. Kept private to this module so no
/// caller can hand `is_short` the wrong one.
fn no_redirect_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
}

/// What the `/shorts/` endpoint's answer means: `200` serves the Short, a
/// redirect to the video's watch page says it is an ordinary video, and
/// anything else -- a 404 for an id that does not exist, a redirect to the
/// consent page, a 429 -- says nothing either way.
pub fn shorts_verdict(status: u16, location: Option<&str>) -> Option<bool> {
    match status {
        200 => Some(true),
        300..=399 if location.is_some_and(|l| l.contains("/watch?v=")) => Some(false),
        _ => None,
    }
}

/// Whether YouTube serves this video as a Short. Verified 2026-10-16:
/// `/shorts/8Pae6Wy1abE` (a Short) answers `200`, `/shorts/dI1xnt0sw5I` (a
/// 5-hour video) and `/shorts/yi8Ehs1Mrwc` (members-only) answer `303` to
/// `/watch?v=<id>`, and a nonexistent id `404`.
///
/// `None` is "could not tell", which callers take as not a Short: refusing a
/// real video because a request failed is the worse mistake, and every other
/// entry point refuses Shorts by their URL anyway. The body of a Short's page
/// is never read; only the status line matters.
pub async fn is_short(video_id: &str) -> Option<bool> {
    let resp = no_redirect_client()?
        .get(format!("https://www.youtube.com/shorts/{video_id}"))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::COOKIE, CONSENT_COOKIE)
        .send()
        .await
        .ok()?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok());
    shorts_verdict(resp.status().as_u16(), location)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pieces of a real watch page (`dI1xnt0sw5I`, 2026-10-16) this module
    /// reads, in the order the page has them, with everything between cut.
    /// The overlay `videoDetails` comes *first* here, on purpose: it must not
    /// be mistaken for the player's.
    const PUBLIC: &str = concat!(
        r#"<html><head><script type="application/ld+json" nonce="x">{"@context":"https://schema.org","@type":"VideoObject","#,
        r#""@id":"https://www.youtube.com/watch?v=dI1xnt0sw5I","name":"He Touched MAX LEVEL for One Night… Then Woke Up at Level 1 With 100x MANA","#,
        r#""thumbnailUrl":"https://i.ytimg.com/vi/dI1xnt0sw5I/maxresdefault.jpg","uploadDate":"2026-10-15T17:06:01-07:00","#,
        r#""comment":[{"@type":"https://schema.org/Comment","dateCreated":"2026-10-15T19:04:31-07:00","text":"Hope it’s good"}]}</script>"#,
        r#"<link rel="canonical" href="https://www.youtube.com/watch?v=dI1xnt0sw5I"></head><body>"#,
        r#"<script>var x = {"videoDetails":{"playerOverlayVideoDetailsRenderer":{"title":{"simpleText":"Overlay"}}}};</script>"#,
        r#"<script nonce="x">var ytInitialPlayerResponse = {"responseContext":{},"#,
        r#""playabilityStatus":{"status":"OK","playableInEmbed":true,"contextParams":"Q0FFU0FnZ0M="},"#,
        r#""streamingData":{"expiresInSeconds":"21540"},"#,
        r#""videoDetails":{"videoId":"dI1xnt0sw5I","title":"He Touched MAX LEVEL for One Night… Then Woke Up at Level 1 With 100x MANA","#,
        r#""lengthSeconds":"18961","keywords":["usa","manga explained in english"],"channelId":"UCksnvnKYJ7f5_LrJcg7ZxaQ","#,
        r#""isOwnerViewing":false,"shortDescription":"Braces in prose } { are fine inside a string.","isCrawlable":true,"#,
        r#""thumbnail":{"thumbnails":[{"url":"https://i.ytimg.com/vi/dI1xnt0sw5I/maxresdefault.jpg","width":1920,"height":1080}]},"#,
        r#""allowRatings":true,"viewCount":"3281","author":"Arashi Recap","isPrivate":false,"isLiveContent":false},"#,
        r#""microformat":{"playerMicroformatRenderer":{"lengthSeconds":"18961","externalChannelId":"UCksnvnKYJ7f5_LrJcg7ZxaQ","#,
        r#""uploadDate":"2026-10-15T17:06:01-07:00","isShortsEligible":false,"externalVideoId":"dI1xnt0sw5I","#,
        r#""canonicalUrl":"https://www.youtube.com/watch?v=dI1xnt0sw5I"}}};var meta = 1;</script></body></html>"#,
    );

    /// `yi8Ehs1Mrwc`, a members-only upload, fetched signed out: no stream,
    /// details intact, and the offer standing in for the player.
    const MEMBERS: &str = concat!(
        r#"<script type="application/ld+json">{"@type":"VideoObject","uploadDate":"2026-10-13T03:42:18-07:00"}</script>"#,
        r#"var ytInitialPlayerResponse = {"responseContext":{},"#,
        r#""playabilityStatus":{"status":"UNPLAYABLE","reason":"Станете член на канала, за да получите достъп","#,
        r#""errorScreen":{"playerLegacyDesktopYpcOfferRenderer":{"itemTitle":"Съдържание само за членове","#,
        r#""offerId":"sponsors_only_video"}}},"#,
        r#""videoDetails":{"videoId":"yi8Ehs1Mrwc","title":"(14) Everyone Is Obsessed With Cultivation, While I Accidentally Become Invincible as a Dad!","#,
        r#""lengthSeconds":"8062","channelId":"UC5qhtUpMhpBacu9sm3V2fag","author":"AnimeCapped Manga","isPrivate":false},"#,
        r#""microformat":{"playerMicroformatRenderer":{"canonicalUrl":"https://www.youtube.com/watch?v=yi8Ehs1Mrwc"}}};"#,
    );

    /// `8Pae6Wy1abE`, a Short: an ordinary watch page in every respect except
    /// the canonical URL.
    const SHORT: &str = concat!(
        r#"var ytInitialPlayerResponse = {"playabilityStatus":{"status":"OK"},"#,
        r#""videoDetails":{"videoId":"8Pae6Wy1abE","title":"A Short","lengthSeconds":"42","channelId":"UCBR8-60-B28hp2BmDPdntcQ","author":"YouTube"},"#,
        r#""microformat":{"playerMicroformatRenderer":{"isShortsEligible":true,"canonicalUrl":"https://www.youtube.com/shorts/8Pae6Wy1abE"}}};"#,
    );

    #[test]
    fn reads_the_players_details_not_the_overlays() {
        let p = parse(PUBLIC).expect("parses");
        assert_eq!(p.id, "dI1xnt0sw5I");
        assert_eq!(p.title, "He Touched MAX LEVEL for One Night… Then Woke Up at Level 1 With 100x MANA");
        assert_eq!(p.channel_id.as_deref(), Some("UCksnvnKYJ7f5_LrJcg7ZxaQ"));
        assert_eq!(p.channel_title.as_deref(), Some("Arashi Recap"));
        assert_eq!(p.duration_secs, Some(18961), "lengthSeconds is a string on the page");
        assert!(!p.members_only);
        assert!(!p.canonical_short);
    }

    #[test]
    fn the_upload_date_comes_from_the_ld_json_not_a_comment() {
        let p = parse(PUBLIC).unwrap();
        // 17:06:01 -07:00 is 00:06:01 UTC the next day; the comment's
        // `dateCreated` two hours later must not be what is read.
        assert_eq!(
            chrono::DateTime::from_timestamp(p.published_at.unwrap(), 0).unwrap().to_rfc3339(),
            "2026-10-16T00:06:01+00:00"
        );
    }

    #[test]
    fn a_members_only_video_is_recognised_by_its_offer_whatever_the_language() {
        let p = parse(MEMBERS).expect("details survive the paywall");
        assert!(p.members_only);
        assert_eq!(p.duration_secs, Some(8062));
        assert_eq!(p.channel_id.as_deref(), Some("UC5qhtUpMhpBacu9sm3V2fag"));
        assert!(p.published_at.is_some());
    }

    #[test]
    fn a_short_names_itself_in_its_canonical_url() {
        let p = parse(SHORT).unwrap();
        assert!(p.canonical_short);
        assert!(!p.members_only);
    }

    #[test]
    fn a_page_without_player_details_does_not_parse() {
        // What a removed or private video serves: a status, no details.
        let gone = r#"var ytInitialPlayerResponse = {"playabilityStatus":{"status":"ERROR","reason":"Video unavailable"}};"#;
        assert_eq!(parse(gone), None);
        assert_eq!(parse("<html>consent wall</html>"), None);
        // Details present but empty are no better than none.
        let empty = r#""videoDetails":{"videoId":"dI1xnt0sw5I","title":"  "}"#;
        assert_eq!(parse(empty), None);
    }

    #[test]
    fn a_zero_length_is_unknown_not_zero() {
        let upcoming = r#""videoDetails":{"videoId":"abcdefghijk","title":"Premiere","lengthSeconds":"0","isUpcoming":true}"#;
        let p = parse(upcoming).unwrap();
        assert_eq!(p.duration_secs, None);
        assert_eq!(p.channel_id, None, "a missing field is None, never an empty string");
        assert_eq!(p.published_at, None);
    }

    #[test]
    fn the_shorts_endpoint_answers_by_status_and_destination() {
        assert_eq!(shorts_verdict(200, None), Some(true));
        assert_eq!(shorts_verdict(303, Some("https://www.youtube.com/watch?v=dI1xnt0sw5I")), Some(false));
        // Relative, as some redirects are.
        assert_eq!(shorts_verdict(302, Some("/watch?v=dI1xnt0sw5I")), Some(false));
        // The consent wall is a redirect too, and says nothing about the video.
        assert_eq!(shorts_verdict(302, Some("https://consent.youtube.com/m?continue=…")), None);
        assert_eq!(shorts_verdict(303, None), None);
        assert_eq!(shorts_verdict(404, None), None);
        assert_eq!(shorts_verdict(429, None), None);
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap()
    }

    /// The real request path -- gzip, the consent cookie, the parse over a
    /// whole live page -- which the fixtures above cannot cover.
    ///
    ///     cargo test watch_page -- --ignored
    #[tokio::test]
    #[ignore = "requires network"]
    async fn reads_a_real_watch_page_and_it_is_not_a_short() {
        let p = fetch(&http(), "dI1xnt0sw5I").await.expect("page parsed");
        eprintln!("{p:?}");
        assert_eq!(p.channel_id.as_deref(), Some("UCksnvnKYJ7f5_LrJcg7ZxaQ"));
        assert_eq!(p.channel_title.as_deref(), Some("Arashi Recap"));
        assert_eq!(p.duration_secs, Some(18961));
        assert!(p.published_at.is_some_and(|ts| !upload_date::is_bucketed(ts)));
        assert!(!p.members_only);
        assert!(!p.canonical_short);
        assert_eq!(is_short("dI1xnt0sw5I").await, Some(false));
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn a_real_short_is_recognised() {
        assert_eq!(is_short("8Pae6Wy1abE").await, Some(true));
        let p = fetch(&http(), "8Pae6Wy1abE").await.expect("page parsed");
        assert!(p.canonical_short, "the page agrees with the endpoint");
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn a_real_members_only_page_reads_signed_out() {
        let p = fetch(&http(), "yi8Ehs1Mrwc").await.expect("page parsed");
        assert!(p.members_only);
        assert_eq!(is_short("yi8Ehs1Mrwc").await, Some(false));
    }

    #[test]
    fn parse_reads_short_description() {
        let p = parse(PUBLIC).expect("parses");
        assert_eq!(p.description.as_deref(), Some("Braces in prose } { are fine inside a string."));
        let escaped = r#""videoDetails":{"videoId":"abcdefghijk","title":"T","shortDescription":"Part 1 https://youtu.be/AAAAAAAAAAA\nPart 2 \"two\""}"#;
        assert_eq!(parse_description(escaped, "abcdefghijk").as_deref(),
                   Some("Part 1 https://youtu.be/AAAAAAAAAAA\nPart 2 \"two\""));
    }

    #[test]
    fn page_without_details_has_no_description() {
        // A consent wall or a bot check: nothing read, so nothing stored.
        assert_eq!(parse_description("<html>Before you continue to YouTube</html>", "x"), None);
        // Details without the key at all are a page reshaped, not a video
        // with no description: nothing read either.
        assert_eq!(parse_description(MEMBERS, "yi8Ehs1Mrwc"), None);
        assert_eq!(parse(MEMBERS).unwrap().description, None);
        // The key, empty: read, and there is none.
        let empty = r#""videoDetails":{"videoId":"abcdefghijk","title":"T","shortDescription":""}"#;
        assert_eq!(parse_description(empty, "abcdefghijk").as_deref(), Some(""));
        // A page about some other video says nothing about this one.
        assert_eq!(parse_description(empty, "zzzzzzzzzzz"), None);
    }
}
