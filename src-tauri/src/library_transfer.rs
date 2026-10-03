//! The wiring between `transfer` (the archive format and every filesystem step)
//! and `db` (the SQL) — the half of an export or import that neither module
//! can own, since `transfer` never opens a database and `db` never touches a
//! file.
//!
//! It has two callers that must never disagree: the Settings dialog, through
//! `commands.rs`, which adds Tauri's events on top, and the terminal, through
//! `cli`, which has no Tauri runtime at all. So everything here is synchronous
//! and reports progress through a plain callback, and the rules that are easy
//! to get subtly wrong live here once — that a Replace measures what it would
//! remove against the archive's whole roster, and that an import's report is
//! two halves only these two modules can fill.

use crate::config::Settings;
use crate::db::Db;
use crate::models::{ArchiveSummary, ImportMode, ImportReport, TransferEstimate};
use crate::transfer;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;

/// What an export would weigh, for the "Include thumbnails" question.
pub fn estimate(db: &Db) -> Result<TransferEstimate> {
    let channels = db.export_channels()?;
    let videos = db.export_videos()?;
    // `transfer` owns the sizing because it owns what actually goes in the
    // zip: a thumbnail is counted only if the file is really there to be
    // copied -- one stat per thumbnail.
    Ok(transfer::estimate(&channels, &videos))
}

/// Writes every channel and video row in `db` to `dest`. `settings` is this
/// machine's: its portable subset travels, and its `download_dir` is the root
/// each download's path is recorded against.
pub fn export(
    db: &Db,
    settings: &Settings,
    dest: &Path,
    include_thumbs: bool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<()> {
    let channels = db.export_channels()?;
    let videos = db.export_videos()?;
    transfer::write_archive(dest, &channels, &videos, settings, include_thumbs, progress)
}

/// What an archive holds, measured against this library. Writes nothing.
pub fn inspect(db: &Db, path: &Path) -> Result<ArchiveSummary> {
    // Every channel, not just the subscribed ones: an ad-hoc uploader already
    // here is "already here", and saying otherwise would offer to re-add it.
    let known: HashSet<String> = db.export_channels()?.into_iter().map(|c| c.id).collect();
    let mut summary = transfer::read_summary(path, &known)?;

    // What a Replace would remove, measured here because only the database can
    // answer it. Against the archive's whole roster, never a ticked subset:
    // unticking a row means "skip it", so the number shown beside the
    // checklist must not move as the user works down it.
    let roster: Vec<String> = summary.channels.iter().map(|c| c.channel_id.clone()).collect();
    let (local_only_channels, local_only_videos) = db.absent_from(&roster)?;
    summary.local_only_channels = local_only_channels;
    summary.local_only_videos = local_only_videos;
    Ok(summary)
}

/// Imports the `picked` channels of the archive at `path`.
pub fn import(
    db: &Db,
    path: &Path,
    picked: &[String],
    mode: ImportMode,
    apply_settings: bool,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<ImportReport> {
    import_in(db, path, picked, mode, apply_settings, &transfer::Dirs::live(), progress)
}

fn import_in(
    db: &Db,
    path: &Path,
    picked: &[String],
    mode: ImportMode,
    apply_settings: bool,
    dirs: &transfer::Dirs,
    progress: &dyn Fn(usize, usize, &str),
) -> Result<ImportReport> {
    // Everything outside the database: settings, thumbnails, path re-rooting.
    let prepared = transfer::prepare_import_in(path, picked, apply_settings, dirs, progress)?;

    // And now the part that has to be atomic. One `Db` call, one transaction:
    // the connection Mutex is not reentrant, so a loop over several public
    // methods was never available, and a failure here must leave the library
    // exactly as it was.
    let mut report = db.apply_import(
        &prepared.channels,
        &prepared.videos,
        mode,
        &prepared.archive_channel_ids,
    )?;

    // `db` knows the row counts; `transfer` knows everything that happened
    // outside SQL. Neither can fill the other's half.
    report.downloads_relinked = prepared.report.downloads_relinked;
    report.thumbs_written = prepared.report.thumbs_written;
    report.settings_applied = prepared.report.settings_applied;
    report.download_dir_kept = prepared.report.download_dir_kept;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Channel, DownloadState, Video, VideoStatus};
    use std::path::PathBuf;

    fn noop(_: usize, _: usize, _: &str) {}

    fn channel(id: &str) -> Channel {
        Channel {
            id: id.into(),
            title: format!("Channel {id}"),
            handle: None,
            url: format!("https://www.youtube.com/channel/{id}"),
            thumb_path: None,
            subscribed: true,
            member: false,
            added_at: 1_000,
            last_polled_at: None,
            terminated: false,
            auto_download: false,
        }
    }

    fn video(id: &str, channel_id: &str) -> Video {
        Video {
            id: id.into(),
            channel_id: channel_id.into(),
            channel_title: String::new(),
            title: format!("Title of {id}"),
            description: None,
            thumb_url: None,
            thumb_path: None,
            published_at: Some(1_700_000_000),
            sort_at: Some(1_700_000_000),
            feed_rank: 0,
            added_manually: false,
            duration_secs: Some(600),
            view_count: None,
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

    fn library(channels: &[Channel], videos: &[Video]) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.apply_import(channels, videos, ImportMode::Merge, &[]).unwrap();
        db
    }

    fn settings_in(dir: &Path) -> Settings {
        let mut s = Settings::default();
        s.download_dir = dir.to_string_lossy().into_owned();
        s
    }

    fn dirs_in(root: &Path) -> transfer::Dirs {
        transfer::Dirs { thumbs: root.join("thumbs"), settings: root.join("settings.json") }
    }

    #[test]
    fn estimate_counts_the_whole_library() {
        let db = library(&[channel("A"), channel("B")], &[video("a1", "A"), video("b1", "B"), video("b2", "B")]);
        let est = estimate(&db).unwrap();
        assert_eq!((est.channel_count, est.video_count), (2, 3));
    }

    #[test]
    fn an_export_carries_every_row_and_this_machines_download_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let db = library(&[channel("A")], &[video("a1", "A"), video("a2", "A")]);
        let dest = tmp.path().join("out.zip");
        export(&db, &settings_in(tmp.path()), &dest, false, &noop).unwrap();

        let s = transfer::read_summary(&dest, &HashSet::new()).unwrap();
        assert_eq!((s.channels.len(), s.video_count), (1, 2));
        assert_eq!(s.download_dir, tmp.path().to_string_lossy());
    }

    #[test]
    fn inspect_measures_a_replace_against_the_whole_archive_and_flags_what_is_here() {
        let tmp = tempfile::tempdir().unwrap();
        // The archive: one channel this machine has, one it does not.
        let archive = tmp.path().join("a.zip");
        transfer::write_archive(
            &archive,
            &[channel("Shared"), channel("New")],
            &[video("s1", "Shared"), video("n1", "New")],
            &settings_in(tmp.path()),
            false,
            &noop,
        )
        .unwrap();
        // This machine: the shared channel, plus one the archive never mentions.
        let db = library(
            &[channel("Shared"), channel("LocalOnly")],
            &[video("s1", "Shared"), video("l1", "LocalOnly"), video("l2", "LocalOnly")],
        );

        let s = inspect(&db, &archive).unwrap();
        assert_eq!((s.local_only_channels, s.local_only_videos), (1, 2));
        let here = |id: &str| s.channels.iter().find(|c| c.channel_id == id).unwrap().already_here;
        assert!(here("Shared"));
        assert!(!here("New"));
    }

    #[test]
    fn an_import_reports_both_the_rows_and_what_landed_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let theirs = tmp.path().join("theirs");
        let thumb: PathBuf = theirs.join("a1.jpg");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(&thumb, b"\xff\xd8\xff").unwrap();
        let mut a1 = video("a1", "A");
        a1.thumb_path = Some(thumb.to_string_lossy().into_owned());
        let archive = tmp.path().join("a.zip");
        transfer::write_archive(
            &archive,
            &[channel("A"), channel("B")],
            &[a1, video("a2", "A"), video("b1", "B")],
            &settings_in(tmp.path()),
            true,
            &noop,
        )
        .unwrap();

        let ours = tmp.path().join("ours");
        let db = Db::open_in_memory().unwrap();
        let report = import_in(
            &db,
            &archive,
            &["A".to_string()],
            ImportMode::Merge,
            true,
            &dirs_in(&ours),
            &noop,
        )
        .unwrap();

        // From the database's half...
        assert_eq!((report.channels_added, report.videos_added), (1, 2), "only the picked channel");
        // ...and from `transfer`'s.
        assert_eq!(report.thumbs_written, 1);
        assert!(report.settings_applied);
        assert!(ours.join("thumbs/a1.jpg").is_file());
        assert!(db.get_channel("B").unwrap().is_none(), "an unpicked channel is skipped");
    }

    #[test]
    fn a_replace_removes_only_what_the_archive_never_mentions() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("a.zip");
        transfer::write_archive(
            &archive,
            &[channel("A"), channel("B")],
            &[video("a1", "A"), video("b1", "B")],
            &settings_in(tmp.path()),
            false,
            &noop,
        )
        .unwrap();
        let db = library(
            &[channel("B"), channel("Gone")],
            &[video("b1", "B"), video("g1", "Gone")],
        );

        // B is in the archive but left unticked: skipped, never removed.
        let report = import_in(
            &db,
            &archive,
            &["A".to_string()],
            ImportMode::Replace,
            false,
            &dirs_in(&tmp.path().join("ours")),
            &noop,
        )
        .unwrap();
        assert_eq!((report.channels_removed, report.videos_removed), (1, 1));
        assert!(db.get_channel("B").unwrap().is_some());
        assert!(db.get_channel("Gone").unwrap().is_none());
    }
}
