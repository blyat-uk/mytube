//! The seam between `transfer` and `db`, which neither module's own tests can
//! reach: `transfer` never opens a database and `db` never touches a file, so
//! an archive that writes cleanly and a merge that applies cleanly can still
//! fail to mean the same thing. This is the only test that carries a real
//! library out through a zip and back into a different machine's database.
//!
//! `XDG_CONFIG_HOME` is redirected before anything else runs: `prepare_import`
//! applies settings and extracts thumbnails for real, and pointing it at the
//! developer's own `~/.config/mytube` would rewrite the library under test.

use mytube_lib::{config, db::Db, models::*, transfer};
use std::path::{Path, PathBuf};

fn chan(id: &str, title: &str) -> Channel {
    Channel {
        id: id.into(),
        title: title.into(),
        handle: None,
        url: format!("https://www.youtube.com/channel/{id}"),
        thumb_path: None,
        subscribed: true,
        member: false,
        added_at: 1_000,
        last_polled_at: None,
    }
}

fn video(id: &str, channel_id: &str, channel_title: &str, title: &str) -> Video {
    Video {
        id: id.into(),
        channel_id: channel_id.into(),
        channel_title: channel_title.into(),
        title: title.into(),
        description: None,
        thumb_url: Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")),
        thumb_path: None,
        published_at: Some(1_700_000_000),
        sort_at: Some(1_700_000_000),
        feed_rank: 0,
        added_manually: false,
        duration_secs: Some(600),
        view_count: Some(42),
        status: VideoStatus::Ready,
        hidden: false,
        watched: false,
        watched_at: None,
        download_state: DownloadState::None,
        download_error: None,
        file_path: None,
        downloaded_at: None,
        first_seen_at: 1_000,
        sibling_group: None,
    }
}

#[test]
fn a_library_survives_the_whole_journey_to_another_machine() {
    let tmp = tempfile::tempdir().unwrap();
    // Two machines, so two config dirs -- the whole point of the exercise. They
    // share `downloads`, standing in for the mount both machines can see, which
    // is what gives re-rooting something to find.
    let ours = tmp.path().join("ours");
    let theirs = tmp.path().join("theirs");
    let downloads = tmp.path().join("Videos");
    std::fs::create_dir_all(&downloads).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &ours);
    config::ensure_dirs().unwrap();

    // --- the exporting machine -------------------------------------------
    let src = Db::open_in_memory().unwrap();

    let mut adhoc = chan("UCadhoc", "Ad-hoc uploader");
    adhoc.subscribed = false; // travels only as a parent row
    let mut member = chan("UCmember", "Members channel");
    member.member = true;

    let mut watched = video("vid_watched", "UC1", "One", "Watched one");
    watched.watched = true;
    watched.watched_at = Some(1_700_000_500);
    let mut hidden = video("vid_hidden", "UC1", "One", "Hidden one");
    hidden.hidden = true;
    let mut manual = video("vid_manual", "UCadhoc", "Ad-hoc uploader", "Added by hand");
    manual.added_manually = true;

    // A finished download, with a file that really exists under `downloads`.
    let rel = PathBuf::from("One").join("Downloaded one [vid_dl].mkv");
    let abs = downloads.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, b"not really a video").unwrap();
    let mut done = video("vid_dl", "UC1", "One", "Downloaded one");
    done.download_state = DownloadState::Done;
    done.file_path = Some(abs.to_string_lossy().into_owned());
    done.downloaded_at = Some(1_700_000_900);

    // A download that will NOT be found on the other machine.
    let mut gone = video("vid_gone", "UC1", "One", "Missing one");
    gone.download_state = DownloadState::Done;
    gone.file_path = Some(downloads.join("One/Nowhere [vid_gone].mkv").to_string_lossy().into_owned());
    gone.downloaded_at = Some(1_700_000_950);

    // Two hand-marked siblings — the state no poll can ever rebuild.
    let mut p1 = video("vid_p1", "UCmember", "Members channel", "EVERYTHING CHANGED");
    let mut p2 = video("vid_p2", "UCmember", "Members channel", "The Blackwood Tapes Pt 3");
    p1.sibling_group = Some("1757000000000-vid_p1".into());
    p2.sibling_group = Some("1757000000000-vid_p1".into());

    src.apply_import(
        &[chan("UC1", "One"), adhoc, member],
        &[watched, hidden, manual, done, gone, p1, p2],
        ImportMode::Merge,
        &[],
    )
    .unwrap();

    // A cached thumbnail to ride along in the zip.
    let thumb = config::thumbs_dir().join("vid_watched.jpg");
    std::fs::write(&thumb, b"\xff\xd8\xff-jpeg-ish").unwrap();
    src.set_thumb_path("vid_watched", &thumb.to_string_lossy()).unwrap();

    let mut settings = config::load().unwrap();
    settings.download_dir = downloads.to_string_lossy().into_owned();
    // Import only adopts a player that would start something on the machine
    // it lands on, so the test uses one every machine has: this test binary.
    let player = format!("\"{}\" --fullscreen", std::env::current_exe().unwrap().display());
    settings.player_command = player.clone();
    settings.backfill_count = 77;
    settings.card_size = 400;
    config::save(&settings).unwrap();

    let channels = src.export_channels().unwrap();
    let videos = src.export_videos().unwrap();
    let archive = tmp.path().join("mytube-export.zip");
    transfer::write_archive(&archive, &channels, &videos, &settings, true, &|_, _, _| {}).unwrap();

    // --- reading it, before committing to anything ------------------------
    let summary = transfer::read_summary(&archive, &Default::default()).unwrap();
    assert_eq!(summary.format, transfer::FORMAT);
    assert_eq!(summary.channels.len(), 3, "every channel is offered, ad-hoc included");
    assert_eq!(summary.video_count, 7);
    assert!(summary.includes_thumbs && summary.thumb_count == 1);
    assert!(
        summary.channels.iter().any(|c| c.channel_id == "UCadhoc" && !c.subscribed),
        "the ad-hoc uploader travels, tagged as unsubscribed"
    );
    assert!(summary.channels.iter().any(|c| c.channel_id == "UCmember" && c.member));

    // --- the receiving machine -------------------------------------------
    // From here on this process *is* the other computer: an empty thumbnail
    // cache and default settings, which is what makes `thumbs_written` and the
    // settings assertions below mean anything.
    std::env::set_var("XDG_CONFIG_HOME", &theirs);
    config::ensure_dirs().unwrap();
    assert!(
        !config::thumbs_dir().join("vid_watched.jpg").exists(),
        "the receiving machine starts with nothing cached"
    );
    let picked: Vec<String> = summary.channels.iter().map(|c| c.channel_id.clone()).collect();
    let prepared =
        transfer::prepare_import(&archive, &picked, true, &|_, _, _| {}).unwrap();

    assert_eq!(prepared.archive_channel_ids.len(), 3);
    assert_eq!(prepared.report.downloads_relinked, 1, "only the file that is here relinks");
    assert_eq!(prepared.report.thumbs_written, 1);
    assert!(prepared.report.settings_applied);
    assert!(!prepared.report.download_dir_kept, "the archive's download dir exists here");

    let dst = Db::open_in_memory().unwrap();
    let report = dst
        .apply_import(&prepared.channels, &prepared.videos, ImportMode::Merge, &prepared.archive_channel_ids)
        .unwrap();
    assert_eq!(report.channels_added, 3);
    assert_eq!(report.videos_added, 7);

    // --- and now: does it mean the same thing? ---------------------------
    let got = |id: &str| dst.get_video(id).unwrap().expect("row crossed");

    let w = got("vid_watched");
    assert!(w.watched && w.watched_at == Some(1_700_000_500), "watched state crossed");
    assert_eq!(
        w.thumb_path,
        Some(config::thumbs_dir().join("vid_watched.jpg").to_string_lossy().into_owned()),
        "the thumb points at THIS machine's cache, not the exporting machine's"
    );
    assert!(Path::new(w.thumb_path.as_ref().unwrap()).exists(), "and the file is really there");

    assert!(got("vid_hidden").hidden);
    assert!(got("vid_manual").added_manually);
    assert_eq!(got("vid_p1").sibling_group, got("vid_p2").sibling_group);
    assert!(got("vid_p1").sibling_group.is_some(), "hand-marked siblings survived");

    let d = got("vid_dl");
    assert_eq!(d.download_state, DownloadState::Done);
    assert_eq!(d.file_path, Some(abs.to_string_lossy().into_owned()), "re-rooted onto a real file");
    assert_eq!(d.downloaded_at, Some(1_700_000_900));

    let g = got("vid_gone");
    assert_eq!(g.download_state, DownloadState::None, "a file that is not here is not claimed");
    assert_eq!(g.file_path, None);
    assert_eq!(g.downloaded_at, None);

    let member_row = dst.get_channel("UCmember").unwrap().unwrap();
    assert!(member_row.member, "the membership crossed");
    assert!(!dst.get_channel("UCadhoc").unwrap().unwrap().subscribed, "ad-hoc stays unsubscribed");

    let landed = config::load().unwrap();
    assert_eq!(landed.player_command, player);
    assert_eq!(landed.backfill_count, 77);
    assert_eq!(landed.card_size, 400);

    // Importing the very same archive again must be a no-op.
    let again = transfer::prepare_import(&archive, &picked, true, &|_, _, _| {}).unwrap();
    let second = dst
        .apply_import(&again.channels, &again.videos, ImportMode::Merge, &again.archive_channel_ids)
        .unwrap();
    assert_eq!(second, ImportReport::default(), "a second import changes nothing: {second:?}");
}
