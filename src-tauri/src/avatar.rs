//! Channel pictures: the small square avatar the Add dialog shows beside each
//! subscription, cached at `thumbs/channels/<channel_id>.jpg` and named by
//! `channels.thumb_path`.
//!
//! Nothing goes looking for them. The channel listing every poll already
//! reads carries the picture's URL once it is read as one document (see
//! `ytdlp::parse_listing`), so a picture costs one ~11 KB GET the first time a
//! channel is listed, and another only when the channel changes it or the
//! file goes missing. That is also how a library that predates pictures gets
//! all of them: its first poll after the update lists every subscription, and
//! there is no sweep of its own. A channel that is never listed -- an ad-hoc
//! uploader, a terminated account -- never gets one, and the dialog draws its
//! initials instead.

use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

use crate::config;
use crate::db::Db;
use crate::models::ChannelMeta;

/// What googleusercontent is asked for in place of the listing's own `=s0`
/// (the full-size original): 176 px square (`s176`), cropped to it (`c`), on a
/// white matte (`c0x00ffffff`), as a JPEG (`rj`). Verified 2026-10-16 on
/// `UC5qhtUpMhpBacu9sm3V2fag`: a 176x176 baseline JPEG of 11,312 bytes. That
/// is the size YouTube's own pages ask for, and more than the 40 px the dialog
/// draws at its largest needs even on a 4x display.
const SIZE_OPTIONS: &str = "s176-c-k-c0x00ffffff-no-rj";

/// The listing's picture URL with its size options replaced by
/// [`SIZE_OPTIONS`]. googleusercontent reads them from after the last `=` of
/// the path; a URL with none just gets them appended, which is the same URL
/// the replacement would have produced.
pub fn sized_url(url: &str) -> String {
    let base = match url.rfind('=') {
        Some(i) if !url[i..].contains('/') => &url[..i],
        _ => url,
    };
    format!("{base}={SIZE_OPTIONS}")
}

/// Where a channel's picture is kept: under `thumbs/`, because the asset
/// protocol's scope (`$CONFIG/mytube/thumbs/**`) is what lets the webview
/// read it, in a folder of its own beside the video thumbnails. `None` for an
/// id that is not a plain YouTube id: it is never turned into a path to write
/// or delete, whatever reached `remove_channel` or a Takeout CSV.
pub fn cached_path(channel_id: &str) -> Option<PathBuf> {
    let plain = !channel_id.is_empty()
        && channel_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| config::thumbs_dir().join("channels").join(format!("{channel_id}.jpg")))
}

/// Records what a listing said about `channel_id`, and fetches its picture
/// when the one on disk is missing or not the one listed. Called after every
/// successful listing a poll reads.
///
/// Never fails. A picture is not worth a poll's error, so whatever goes wrong
/// is one line in the journal, and the next listing simply tries again: the
/// row only records a picture once the file is in place.
pub async fn note(db: &Db, http: &reqwest::Client, channel_id: &str, meta: &ChannelMeta) {
    if let Err(err) = db.set_channel_meta(channel_id, meta) {
        eprintln!("mytube: could not record {channel_id}'s handle and subscribers: {err}");
    }
    let (Some(url), Some(path)) = (meta.avatar_url.as_deref(), cached_path(channel_id)) else {
        return;
    };
    if let Err(err) = refresh(db, http, channel_id, url, &path).await {
        eprintln!("mytube: picture for {channel_id} failed: {err}");
    }
}

/// Fetches `url` into `path` unless the row already records exactly that and
/// the file is still there. The second check is the filesystem's, so it is
/// asked on the blocking pool, and only when the database has no reason of its
/// own -- the common case of every poll after the first is one indexed read
/// and one stat.
async fn refresh(db: &Db, http: &reqwest::Client, channel_id: &str, url: &str, path: &Path)
    -> Result<()> {
    let recorded = path.to_string_lossy().into_owned();
    if !db.avatar_needs_fetch(channel_id, url, &recorded)? {
        let file = path.to_path_buf();
        if tokio::task::spawn_blocking(move || file.is_file()).await? {
            return Ok(());
        }
    }
    let bytes = download(http, &sized_url(url)).await?;
    let dest = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_atomically(&dest, &bytes)).await??;
    db.set_channel_avatar(channel_id, url, &recorded)
}

/// The picture's bytes. Anything but an image is refused, so an error page
/// served with a 200 never lands on disk wearing a `.jpg`.
async fn download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = http.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let image = resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|t| t.to_str().ok())
        .is_some_and(|t| t.starts_with("image/"));
    if !image {
        bail!("the server sent something other than a picture");
    }
    let bytes = resp.bytes().await?;
    if bytes.is_empty() {
        bail!("the server sent an empty picture");
    }
    Ok(bytes.to_vec())
}

/// Writes through a temporary file beside `dest` and a rename over it, so the
/// webview never reads half a picture and a failed write leaves the old one
/// whole. Synced before the rename: a torn file that survived a power cut
/// would be taken for the current picture, since its URL is recorded, and
/// never fetched again.
///
/// The temporary name is unique per write, because two listings of one
/// channel can land at once -- a new subscription's backfill beside the poll
/// that follows it.
fn write_atomically(dest: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = dest.parent().ok_or_else(|| anyhow!("{} has no folder", dest.display()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.partial",
        dest.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Deletes a removed channel's picture. Best effort: one left behind costs
/// 11 KB, and a channel added again later is fetched afresh whatever is on
/// disk, since its new row records no picture.
pub fn forget(channel_id: &str) {
    let Some(path) = cached_path(channel_id) else { return };
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("mytube: could not delete {}: {err}", path.display());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Channel;
    use crate::sync::client::tests::{fake_server, Reply};
    use std::time::Duration;

    #[test]
    fn a_listed_picture_is_asked_for_at_176_px() {
        let want = "https://yt3.googleusercontent.com/luE0EB=s176-c-k-c0x00ffffff-no-rj";
        // What `avatar_uncropped` is, as listed.
        assert_eq!(sized_url("https://yt3.googleusercontent.com/luE0EB=s0"), want);
        // The square one under a bare number, with options of its own.
        assert_eq!(sized_url("https://yt3.googleusercontent.com/luE0EB=s900-c-k-c0x00ffffff-no-rj"),
                   want);
        // No options at all.
        assert_eq!(sized_url("https://yt3.googleusercontent.com/luE0EB"), want);
        // An `=` that is not in the last path segment is not an option.
        assert_eq!(sized_url("https://h/a=b/pic"), format!("https://h/a=b/pic={SIZE_OPTIONS}"));
    }

    #[test]
    fn a_channel_id_that_is_not_plain_never_becomes_a_path() {
        let p = cached_path("UC5qhtUpMhpBacu9sm3V2fag").unwrap();
        assert_eq!(p, config::thumbs_dir().join("channels").join("UC5qhtUpMhpBacu9sm3V2fag.jpg"));
        assert!(cached_path("UC-_09az").is_some());
        for bad in ["", "../UC1", "UC1/../../x", "UC 1", "UC1.jpg", "C:\\UC1"] {
            assert!(cached_path(bad).is_none(), "{bad:?}");
        }
    }

    fn library(channel_id: &str) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_channel(&Channel {
            id: channel_id.into(), title: "One".into(), handle: None,
            url: format!("https://www.youtube.com/channel/{channel_id}"), thumb_path: None,
            subscribed: true, member: false, added_at: 0, last_polled_at: None,
            terminated: false, auto_download: false,
        }).unwrap();
        db
    }

    fn picture(body: &str) -> Reply {
        Reply { status: 200, headers: vec![("Content-Type", "image/jpeg".into())],
                body: body.into() }
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap()
    }

    fn stored(db: &Db) -> Option<String> { db.get_channel("UC1").unwrap().unwrap().thumb_path }

    /// The bookkeeping end to end, against a local server: fetched once,
    /// recorded, left alone while nothing changes, and fetched again when the
    /// file goes missing or the channel lists a new picture.
    #[tokio::test]
    async fn a_picture_is_fetched_once_and_again_only_when_it_changes_or_goes_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("channels").join("UC1.jpg");
        let (base, seen) = fake_server(vec![picture("one"), picture("two"), picture("three")]);
        let (first, second) = (format!("{base}/pic=s0"), format!("{base}/new=s0"));
        let db = library("UC1");

        refresh(&db, &http(), "UC1", &first, &path).await.unwrap();
        assert_eq!(seen.recv().unwrap().path, format!("/pic={SIZE_OPTIONS}"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        assert_eq!(stored(&db).as_deref(), Some(&*path.to_string_lossy()));

        refresh(&db, &http(), "UC1", &first, &path).await.unwrap();
        assert!(seen.try_recv().is_err(), "the same picture, already on disk: no request");

        std::fs::remove_file(&path).unwrap();
        refresh(&db, &http(), "UC1", &first, &path).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two", "a missing file is fetched");

        refresh(&db, &http(), "UC1", &second, &path).await.unwrap();
        assert_eq!(seen.recv().unwrap().path, format!("/pic={SIZE_OPTIONS}"));
        assert_eq!(seen.recv().unwrap().path, format!("/new={SIZE_OPTIONS}"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "three", "a new picture replaces it");

        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["UC1.jpg"], "no temporary file is left beside it");
    }

    /// A fetch that fails records nothing, so the next listing asks again, and
    /// an error page served with a 200 never lands on disk as a picture.
    #[tokio::test]
    async fn a_failed_fetch_records_nothing_and_keeps_the_old_picture() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("channels").join("UC1.jpg");
        let page = Reply { status: 200, headers: vec![("Content-Type", "text/html".into())],
                           body: "<html>consent</html>".into() };
        let (base, _seen) = fake_server(vec![
            page, Reply { status: 404, headers: vec![], body: String::new() }, picture("one"),
            picture(""),
        ]);
        let url = format!("{base}/pic=s0");
        let db = library("UC1");

        assert!(refresh(&db, &http(), "UC1", &url, &path).await.is_err());
        assert!(refresh(&db, &http(), "UC1", &url, &path).await.is_err());
        assert_eq!(stored(&db), None);
        assert!(!path.exists());

        refresh(&db, &http(), "UC1", &url, &path).await.unwrap();
        let newer = format!("{base}/new=s0");
        let err = refresh(&db, &http(), "UC1", &newer, &path).await.unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one", "the old picture stays whole");
        assert!(db.avatar_needs_fetch("UC1", &newer, &path.to_string_lossy()).unwrap(),
                "and the row still asks for the new one");
    }

    /// The whole path against YouTube: a real listing names the channel's
    /// picture, subscriber count and handle, and the picture it names arrives
    /// as a small JPEG. Hits the network, so it is opt-in:
    ///
    ///     cargo test a_real_listing_names_its_channels_picture -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires network"]
    async fn a_real_listing_names_its_channels_picture() {
        const CHANNEL: &str = "UC5qhtUpMhpBacu9sm3V2fag";
        let tools = crate::tools::Tools::new(crate::tools::bin_dir(), reqwest::Client::new());
        let inv = tools.ytdlp_without_cookies(&config::load().unwrap_or_default()).await
            .expect("a yt-dlp");
        let listing = crate::ytdlp::flat_playlist(&inv, CHANNEL, 3).await.expect("a listing");
        println!("{:?}, {} entries", listing.channel, listing.entries.len());

        let url = listing.channel.avatar_url.clone().expect("the listing names a picture");
        assert!(listing.channel.follower_count.is_some_and(|n| n > 0), "and a subscriber count");
        assert!(listing.channel.handle.as_deref().is_some_and(|h| h.starts_with('@')));
        assert!(!listing.entries.is_empty(), "and still its entries");

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("channels").join(format!("{CHANNEL}.jpg"));
        let db = library(CHANNEL);
        refresh(&db, &http(), CHANNEL, &url, &path).await.expect("the picture downloads");
        let bytes = std::fs::read(&path).unwrap();
        println!("{} -> {} bytes", sized_url(&url), bytes.len());
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "a JPEG");
        assert!(bytes.len() < 64 * 1024, "the small one, not the original");
    }
}
