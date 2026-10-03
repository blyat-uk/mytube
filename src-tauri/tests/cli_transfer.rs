//! `mytube export` and `mytube import` the way an SSH session runs them: the
//! real binary, with no display to open a window on and no terminal to ask
//! questions on. Each "machine" is a config directory of its own
//! (`MYTUBE_CONFIG_DIR`, set on the child only), so nothing here can touch the
//! developer's library and the tests can run side by side.
//!
//! What only this can show: that the subcommands are reached before Tauri or
//! GTK is (no `DISPLAY` and still a clean exit), that an unattended run takes
//! the dialog's defaults without asking, and that what one machine writes the
//! other reads back.
//!
//! Unix only: on Windows the subcommands refuse to run (see `cli::NOT_ON_WINDOWS`).

#![cfg(unix)]

use mytube_lib::{config, db::Db, models::*, transfer};
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn mytube(machine: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mytube"))
        .args(args)
        .current_dir(cwd)
        .env(config::CONFIG_DIR_ENV, machine)
        // An SSH session without -X: anything that reached for GTK would fail.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

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

/// The machine at home: two channels, three videos, one of them watched and
/// with a cached thumbnail, and a setting worth carrying.
fn home_library(home: &Path) {
    let db = Db::open(&home.join("mytube.db")).unwrap();
    let mut watched = video("v1", "UC1");
    watched.watched = true;
    watched.watched_at = Some(1_700_000_500);
    db.apply_import(
        &[chan("UC1", "One"), chan("UC2", "Two")],
        &[watched, video("v2", "UC1"), video("v3", "UC2")],
        ImportMode::Merge,
        &[],
    )
    .unwrap();
    let thumb = home.join("thumbs").join("v1.jpg");
    std::fs::create_dir_all(thumb.parent().unwrap()).unwrap();
    std::fs::write(&thumb, b"\xff\xd8\xff-jpeg-ish").unwrap();
    db.set_thumb_path("v1", &thumb.to_string_lossy()).unwrap();

    let mut settings = config::Settings::default();
    settings.backfill_count = 77;
    config::save_to(&home.join("settings.json"), &settings).unwrap();
}

#[test]
fn a_library_goes_from_home_to_the_laptop_with_nobody_at_a_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, laptop) = (tmp.path().join("home"), tmp.path().join("laptop"));
    home_library(&home);

    // `out/` does not exist yet; the trailing slash says it is a directory.
    let out = mytube(&home, tmp.path(), &["export", "out/"]);
    assert!(out.status.success(), "export failed: {}", text(&out.stderr));
    let archive = tmp.path().join("out").join(transfer::default_archive_name());
    assert!(archive.is_file(), "the dated archive is in out/");
    let said = text(&out.stdout);
    assert!(
        said.contains("Exported 2 channels and 3 videos with 1 thumbnail to"),
        "{said}"
    );
    // Nothing asked, no progress drawn, and not a word from GTK.
    assert_eq!(text(&out.stderr), "");

    // The laptop has never run MyTube: an import is allowed to set it up.
    let archive_arg = archive.to_string_lossy().into_owned();
    let inn = mytube(&laptop, tmp.path(), &["import", &archive_arg]);
    assert!(inn.status.success(), "import failed: {}", text(&inn.stderr));
    let said = text(&inn.stdout);
    assert!(said.contains("Imported 2 channels and 3 videos."), "{said}");
    assert!(said.contains("Applied the archive's settings."), "{said}");
    assert_eq!(text(&inn.stderr), "");

    let db = Db::open(&laptop.join("mytube.db")).unwrap();
    let v1 = db.get_video("v1").unwrap().expect("crossed");
    assert!(v1.watched, "watched state crossed");
    assert!(laptop.join("thumbs").join("v1.jpg").is_file(), "and its thumbnail");
    assert_eq!(config::load_from(&laptop.join("settings.json")).unwrap().backfill_count, 77);
}

#[test]
fn an_unattended_replace_needs_yes_and_without_it_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, laptop) = (tmp.path().join("home"), tmp.path().join("laptop"));
    home_library(&home);
    let out = mytube(&home, tmp.path(), &["export", "lib.zip", "--no-thumbs"]);
    assert!(out.status.success(), "{}", text(&out.stderr));

    // The laptop follows a channel home never had.
    Db::open(&laptop.join("mytube.db"))
        .unwrap()
        .apply_import(&[chan("UCtrip", "Trip")], &[video("t1", "UCtrip")], ImportMode::Merge, &[])
        .unwrap();

    let refused = mytube(&laptop, tmp.path(), &["import", "lib.zip", "--replace"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(text(&refused.stderr).contains("--yes"), "{}", text(&refused.stderr));
    let db = Db::open(&laptop.join("mytube.db")).unwrap();
    assert!(db.get_channel("UCtrip").unwrap().is_some(), "a refused replace removed nothing");
    assert!(db.get_channel("UC1").unwrap().is_none(), "nor imported anything");
    drop(db);

    let replaced = mytube(&laptop, tmp.path(), &["import", "lib.zip", "--replace", "--no-settings", "--yes"]);
    assert!(replaced.status.success(), "{}", text(&replaced.stderr));
    assert!(
        text(&replaced.stdout).contains("Removed 1 video row and 1 channel from the library. No files were deleted."),
        "{}",
        text(&replaced.stdout)
    );
    let db = Db::open(&laptop.join("mytube.db")).unwrap();
    assert!(db.get_channel("UCtrip").unwrap().is_none());
    assert!(db.get_channel("UC1").unwrap().is_some());
}

#[test]
fn exporting_from_a_machine_with_no_library_fails_and_creates_none() {
    let tmp = tempfile::tempdir().unwrap();
    let nowhere = tmp.path().join("never-ran");
    let out = mytube(&nowhere, tmp.path(), &["export"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no MyTube library"), "{}", text(&out.stderr));
    assert!(!nowhere.exists(), "an export never creates a library");
}

#[test]
fn help_and_mistakes_answer_in_text_and_never_with_a_window() {
    let tmp = tempfile::tempdir().unwrap();
    let help = mytube(tmp.path(), tmp.path(), &["--help"]);
    assert!(help.status.success());
    assert!(text(&help.stdout).contains("mytube import FILE"));

    let missing = mytube(tmp.path(), tmp.path(), &["import"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(text(&missing.stderr).contains("mytube import --help"));

    let not_an_archive = tmp.path().join("notes.txt");
    std::fs::write(&not_an_archive, "hello").unwrap();
    let bad = mytube(tmp.path(), tmp.path(), &["import", "notes.txt"]);
    assert_eq!(bad.status.code(), Some(1), "{}", text(&bad.stderr));
}
