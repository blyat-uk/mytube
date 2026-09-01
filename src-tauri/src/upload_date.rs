//! Real upload dates, straight from a video's watch page.
//!
//! A channel listing never carries a true upload date. yt-dlp's
//! `youtubetab:approximate_date` (see [`crate::ytdlp::flat_playlist_args`])
//! turns YouTube's relative wording -- "1 month ago", "3 weeks ago" -- into a
//! midnight-UTC timestamp measured back from the moment of the poll, which is
//! enough to order a feed but is not a date the video was uploaded on. In one
//! real library 364 unrelated videos all landed on the same value.
//!
//! The watch page embeds the exact instant in its ld+json, so one plain GET
//! per video recovers it: no API key, no cookies, no yt-dlp extraction.

use futures::stream::{self, StreamExt};
use std::collections::HashMap;

use crate::rss::USER_AGENT;

/// Matches the ld+json instant. `publishDate` is the fallback: the two agree
/// on every page checked, but only `uploadDate` is guaranteed by schema.org.
fn date_pattern() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#""(?:uploadDate|publishDate)"\s*:\s*"([^"]+)""#)
            .expect("literal pattern compiles")
    })
}

/// True when a stored date is an `approximate_date` bucket rather than a real
/// one: those land on midnight UTC exactly, while a date sourced from RSS or
/// from a watch page carries a time of day.
///
/// A real upload that genuinely happened at 00:00:00 UTC is indistinguishable
/// and would be refetched needlessly. That costs one request and rewrites the
/// same value, so the ambiguity is harmless.
pub fn is_bucketed(ts: i64) -> bool {
    ts % 86_400 == 0
}

/// Pulls the upload instant out of a watch page, as a Unix timestamp.
///
/// YouTube writes the uploader's local offset (`...T11:03:09-07:00`), so the
/// value is normalised to UTC -- which is what yt-dlp's `%(upload_date)s`
/// reports, and therefore what already-downloaded filenames were built from.
pub fn parse_upload_date(html: &str) -> Option<i64> {
    let raw = date_pattern().captures(html)?.get(1)?.as_str();

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp());
    }
    // Some pages carry a bare `YYYY-MM-DD`; treat it as midnight UTC.
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

async fn fetch_one(http: &reqwest::Client, video_id: &str) -> Option<i64> {
    let url = crate::ytdlp::watch_url(video_id);
    let resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    parse_upload_date(&resp.text().await.ok()?)
}

/// Matches the concurrency already used for feeds and thumbnails.
const CONCURRENCY: usize = 8;

/// Resolves as many ids as it can. Ids whose page fails or parses to nothing
/// are simply absent from the map, leaving the caller's existing value alone.
pub async fn fetch_many<I>(http: &reqwest::Client, ids: I) -> HashMap<String, i64>
where
    I: IntoIterator<Item = String>,
{
    stream::iter(ids)
        .map(|id| async move { fetch_one(http, &id).await.map(|ts| (id, ts)) })
        .buffer_unordered(CONCURRENCY)
        .filter_map(|found| async move { found })
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape a real watch page carries, offset and all.
    const PAGE: &str = r#"...{"uploadDate":"2026-07-18T11:03:09-07:00","other":1}..."#;

    #[test]
    fn reads_the_upload_instant_from_a_watch_page() {
        let ts = parse_upload_date(PAGE).expect("date found");
        // 11:03:09 -07:00 is 18:03:09 UTC on the same day.
        assert_eq!(
            chrono::DateTime::from_timestamp(ts, 0).unwrap().to_rfc3339(),
            "2026-07-18T18:03:09+00:00"
        );
    }

    #[test]
    fn normalises_to_the_utc_day_that_yt_dlp_reports() {
        // Late-evening Pacific upload: the UTC day is the NEXT one, and
        // `%(upload_date)s` -- so the downloaded filename -- says the next day.
        let page = r#"{"uploadDate":"2026-07-18T20:00:00-07:00"}"#;
        let ts = parse_upload_date(page).unwrap();
        let day = chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%d");
        assert_eq!(day.to_string(), "2026-07-19");
    }

    #[test]
    fn falls_back_to_publish_date() {
        let page = r#"{"publishDate":"2026-01-02T03:04:05+00:00"}"#;
        assert!(parse_upload_date(page).is_some());
    }

    #[test]
    fn accepts_a_bare_calendar_date() {
        let ts = parse_upload_date(r#"{"uploadDate":"2026-07-18"}"#).unwrap();
        assert_eq!(ts % 86_400, 0, "a bare date is midnight UTC");
        assert_eq!(
            chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%d").to_string(),
            "2026-07-18"
        );
    }

    #[test]
    fn a_page_without_a_date_yields_nothing() {
        assert_eq!(parse_upload_date("<html>no dates here</html>"), None);
        assert_eq!(parse_upload_date(r#"{"uploadDate":"not a date"}"#), None);
    }

    #[test]
    fn midnight_exactly_is_the_approximate_date_signature() {
        // 2026-07-30 00:00:00 UTC -- the bucket 364 videos shared.
        assert!(is_bucketed(1_785_369_600));
    }

    #[test]
    fn a_timestamp_with_a_time_of_day_is_a_real_date() {
        assert!(!is_bucketed(1_785_369_600 + 43_200));
    }

    /// Exercises the real request path -- gzip, redirects, user agent -- which
    /// the parsing tests above cannot cover. Hits YouTube, so it is opt-in:
    ///
    ///     cargo test -- --ignored
    #[tokio::test]
    #[ignore = "requires network"]
    async fn fetches_a_real_upload_date_over_http() {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();

        // Uploaded 2026-07-18; the channel listing buckets it at 2026-07-30.
        let ts = fetch_one(&http, "sxKEb786yiU").await.expect("date resolved");

        assert!(!is_bucketed(ts), "a real date carries a time of day");
        assert_eq!(
            chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%d").to_string(),
            "2026-07-18"
        );
    }
}
