use anyhow::{anyhow, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::models::{Feed, FeedEntry};

/// The one place Shorts are identified.
///
/// YouTube's channel RSS gives every entry an `<link rel="alternate">`; Shorts
/// get a `/shorts/<id>` URL and normal videos get `/watch?v=<id>`. Verified
/// 15/15 against a channel's Shorts tab on 2026-08-29.
///
/// If YouTube ever stops emitting that URL, replace this function's body with
/// the verified fallback: `HEAD https://www.youtube.com/shorts/<id>` returns
/// 200 for a Short and 303 (to `/watch?v=`) for a normal video.
pub fn is_short(alternate_href: &str) -> bool {
    alternate_href.contains("/shorts/")
}

pub fn feed_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}")
}

fn local_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key.as_bytes())
        .map(|a| String::from_utf8_lossy(a.value.as_ref()).into_owned())
}

/// Pulls `UC…` out of a feed-level link such as
/// `https://www.youtube.com/channel/UCHnyfMqiRRG1u-2MsSQLbXA` or
/// `http://www.youtube.com/feeds/videos.xml?channel_id=UCHnyf…`.
fn channel_id_from_link(href: &str) -> Option<String> {
    let id = if let Some(rest) = href.split("/channel/").nth(1) {
        rest
    } else {
        href.split("channel_id=").nth(1)?
    };
    let id: String = id
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!id.is_empty()).then_some(id)
}

#[derive(Default)]
struct Partial {
    video_id: String,
    title: String,
    description: Option<String>,
    thumb_url: Option<String>,
    published: String,
    view_count: Option<i64>,
    href: String,
}

pub fn parse_feed(xml: &str) -> Result<Feed> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut entry_depth = 0usize;
    let mut in_entry = false;
    let mut cur = Partial::default();
    let mut entries = Vec::new();
    let mut shorts_rejected = 0usize;
    let mut channel_id = String::new();
    // The feed's own `<link>` elements carry the full `UC…` id even when
    // `<yt:channelId>` does not, so it wins when both are present.
    let mut channel_id_from_links: Option<String> = None;
    let mut channel_title = String::new();
    let mut target: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                let name = local_name(&e);
                if name == "entry" && !in_entry {
                    in_entry = true;
                    entry_depth = depth;
                    cur = Partial::default();
                }
                target = if in_entry {
                    if depth == entry_depth + 1 {
                        match name.as_str() {
                            "videoId" | "title" | "published" => Some(name),
                            _ => None,
                        }
                    } else if depth == entry_depth + 2 && name == "description" {
                        Some("description".into())
                    } else {
                        None
                    }
                } else if depth == 2 {
                    match name.as_str() {
                        "title" => Some("feedTitle".into()),
                        "channelId" => Some("feedChannelId".into()),
                        _ => None,
                    }
                } else {
                    None
                };
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(&e);
                if in_entry {
                    if name == "link" && attr(&e, "rel").as_deref() == Some("alternate") {
                        if let Some(h) = attr(&e, "href") {
                            cur.href = h;
                        }
                    } else if name == "thumbnail" && cur.thumb_url.is_none() {
                        cur.thumb_url = attr(&e, "url");
                    } else if name == "statistics" {
                        cur.view_count = attr(&e, "views").and_then(|v| v.parse().ok());
                    }
                } else if name == "link" && depth == 1 && channel_id_from_links.is_none() {
                    channel_id_from_links = attr(&e, "href")
                        .as_deref()
                        .and_then(channel_id_from_link);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(k) = target.as_deref() {
                    let val = t.unescape().unwrap_or_default().into_owned();
                    match k {
                        "videoId" => cur.video_id = val,
                        "title" => cur.title = val,
                        "published" => cur.published = val,
                        "description" => cur.description = Some(val),
                        "feedTitle" => channel_title = val,
                        "feedChannelId" => channel_id = val,
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                target = None;
                if in_entry && e.local_name().as_ref() == b"entry" && depth == entry_depth {
                    in_entry = false;
                    if is_short(&cur.href) {
                        shorts_rejected += 1;
                    } else if !cur.video_id.is_empty() {
                        let published_at = chrono::DateTime::parse_from_rfc3339(&cur.published)
                            .map(|d| d.timestamp())
                            .unwrap_or(0);
                        entries.push(FeedEntry {
                            video_id: std::mem::take(&mut cur.video_id),
                            title: std::mem::take(&mut cur.title),
                            description: cur.description.take(),
                            thumb_url: cur.thumb_url.take(),
                            published_at,
                            view_count: cur.view_count,
                        });
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("malformed feed XML: {e}")),
            _ => {}
        }
        buf.clear();
    }

    if in_entry {
        return Err(anyhow!("malformed feed XML: unterminated <entry>"));
    }
    if let Some(id) = channel_id_from_links {
        channel_id = id;
    }
    Ok(Feed { channel_id, channel_title, entries, shorts_rejected })
}

pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

pub async fn fetch_feed(client: &reqwest::Client, channel_id: &str) -> Result<Feed> {
    let resp = client
        .get(feed_url(channel_id))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("feed for {channel_id} returned HTTP {}", resp.status()));
    }
    parse_feed(&resp.text().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/veritasium_feed.xml");

    #[test]
    fn shorts_are_detected_from_the_alternate_link() {
        assert!(is_short("https://www.youtube.com/shorts/b3KpFdb1pW8"));
        assert!(!is_short("https://www.youtube.com/watch?v=J1WoNuemKOg"));
    }

    #[test]
    fn every_short_in_the_real_feed_is_rejected() {
        let feed = parse_feed(FIXTURE).unwrap();
        assert_eq!(feed.shorts_rejected, 11, "fixture has 11 shorts of 15 entries");
        assert_eq!(feed.entries.len(), 4);
        let ids: Vec<&str> = feed.entries.iter().map(|e| e.video_id.as_str()).collect();
        assert_eq!(ids, vec!["J1WoNuemKOg", "wt4p2oalmRY", "tL9Lw250spc", "GK2pZ_oVU1o"]);
    }

    #[test]
    fn feed_level_metadata_is_extracted() {
        let feed = parse_feed(FIXTURE).unwrap();
        assert_eq!(feed.channel_id, "UCHnyfMqiRRG1u-2MsSQLbXA");
        assert_eq!(feed.channel_title, "Veritasium");
    }

    #[test]
    fn entry_fields_are_extracted_correctly() {
        let feed = parse_feed(FIXTURE).unwrap();
        let e = &feed.entries[0];
        assert_eq!(e.video_id, "J1WoNuemKOg");
        assert_eq!(e.title, "We sent a balloon to space to film the solar eclipse");
        assert!(e.thumb_url.as_deref().unwrap().contains("J1WoNuemKOg"));
        assert!(e.published_at > 1_700_000_000, "published_at should be a real unix ts");
        assert!(e.view_count.unwrap() > 0);
        assert!(e.description.as_deref().unwrap_or("").len() > 10);
    }

    #[test]
    fn entry_title_is_not_confused_with_media_title_or_feed_title() {
        let feed = parse_feed(FIXTURE).unwrap();
        assert!(feed.entries.iter().all(|e| e.title != "Veritasium"));
    }

    #[test]
    fn malformed_xml_returns_an_error_rather_than_panicking() {
        assert!(parse_feed("<feed><entry><unclosed>").is_err());
    }

    #[test]
    fn an_empty_but_valid_feed_yields_no_entries() {
        let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:yt="http://www.youtube.com/xml/schemas/2015">
  <yt:channelId>UCtest</yt:channelId><title>Empty</title></feed>"#;
        let feed = parse_feed(xml).unwrap();
        assert_eq!(feed.channel_title, "Empty");
        assert!(feed.entries.is_empty());
    }

    #[test]
    fn feed_url_is_built_from_the_channel_id() {
        assert_eq!(feed_url("UC1"),
            "https://www.youtube.com/feeds/videos.xml?channel_id=UC1");
    }
}

