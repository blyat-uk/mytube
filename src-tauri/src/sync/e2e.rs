//! Two machines, one real sync server.
//!
//! Two tests. `the_wire_protocol_…` runs two in-memory snapshots through the
//! server and checks every request this client makes against what the Worker
//! actually accepts. `two_libraries_converge_…` does the same with two real
//! `Db`s and asserts their snapshots end up identical -- the whole feature,
//! end to end.
//!
//! Ignored by default: they need a server, and run only when
//! `MYTUBE_SYNC_TEST_SERVER` names one -- a `wrangler dev` with Cloudflare's
//! always-pass Turnstile test secret, under which any token passes:
//!
//! ```sh
//! (cd sync-server && bunx wrangler dev --port 8787)
//! MYTUBE_SYNC_TEST_SERVER=http://127.0.0.1:8787 \
//!   cargo test --manifest-path src-tauri/Cargo.toml sync::e2e -- --ignored
//! ```
//!
//! Everything goes through the same `create` / `join` / `run_once` the app's
//! clock and commands use, minus the window and the pace (`wrangler dev` has
//! no edge rate limit to respect).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::code::SyncCode;
use super::snapshot::{ApplyReport, Snapshot, SyncChannel, SNAPSHOT_FORMAT};
use super::{create, join, run_once, state, Env, Library, Outcome};
use crate::db::Db;
use crate::models::{Channel, NewVideo, VideoStatus};

const SERVER_ENV: &str = "MYTUBE_SYNC_TEST_SERVER";

/// Cloudflare's documented dummy token, which the always-pass test secret
/// accepts.
const DUMMY_TOKEN: &str = "XXXX.DUMMY.TOKEN.XXXX";

fn channel(id: &str, subscribed: bool) -> Channel {
    Channel {
        id: id.into(),
        title: format!("Channel {id}"),
        handle: None,
        url: format!("https://www.youtube.com/channel/{id}"),
        thumb_path: None,
        subscribed,
        member: false,
        added_at: 1_700_000_000,
        last_polled_at: None,
        terminated: false,
        auto_download: false,
    }
}

fn video(id: &str, channel_id: &str, manual: bool) -> NewVideo {
    NewVideo {
        id: id.into(),
        channel_id: channel_id.into(),
        title: format!("Video {id}"),
        description: None,
        thumb_url: Some(crate::ytdlp::thumb_url_for(id)),
        published_at: Some(1_700_000_000),
        sort_at: Some(1_700_000_000),
        feed_rank: 0,
        added_manually: manual,
        duration_secs: Some(600),
        view_count: None,
        status: VideoStatus::Ready,
    }
}

fn machine(base: &str, dir: &std::path::Path, name: &str) -> (Env, Arc<Db>) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let env = Env {
        lib: db.clone(),
        http: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap(),
        base: super::client::base_url(base),
        state_path: dir.join(name).join("sync.json"),
        machine_name: name.into(),
    };
    (env, db)
}

/// One pull-wanted wake of the clock, which must have gone out and worked.
async fn exchange(env: &Env) {
    match run_once(env, Instant::now(), &AtomicBool::new(true)).await.unwrap() {
        Outcome::Sent(Ok(_)) => {}
        Outcome::Sent(Err(e)) => panic!("{}: exchange failed: {e}", env.machine_name),
        other => panic!("{}: nothing was sent: {other:?}", env.machine_name),
    }
}

async fn rounds(a: &Env, b: &Env, n: usize) {
    for _ in 0..n {
        exchange(a).await;
        exchange(b).await;
    }
}

/// A library that is one snapshot in memory, recording what it is handed.
/// Lets the wire protocol be checked against a real server without a database.
struct MemLib {
    snapshot: std::sync::Mutex<Snapshot>,
    applied: std::sync::Mutex<Vec<Snapshot>>,
}

impl MemLib {
    fn new(channel_id: &str) -> Arc<Self> {
        Arc::new(Self { snapshot: std::sync::Mutex::new(mem_snapshot(channel_id)), applied: Default::default() })
    }
    /// The machine names of every snapshot applied so far, in order.
    fn applied_from(&self) -> Vec<String> {
        self.applied.lock().unwrap().iter().map(|s| s.machine_name.clone()).collect()
    }
}

fn mem_snapshot(channel_id: &str) -> Snapshot {
    Snapshot {
        format: SNAPSHOT_FORMAT,
        machine_name: String::new(),
        channels: vec![SyncChannel {
            id: channel_id.into(),
            title: channel_id.into(),
            handle: None,
            url: format!("https://www.youtube.com/channel/{channel_id}"),
            subscribed: true,
            subscribed_changed_at: Some(1_700_000_000),
            member: false,
            member_changed_at: None,
        }],
        videos: vec![],
        tombstones: vec![],
    }
}

impl Library for MemLib {
    fn sync_snapshot(&self, machine_name: &str) -> anyhow::Result<Snapshot> {
        let mut s = self.snapshot.lock().unwrap().clone();
        s.machine_name = machine_name.into();
        Ok(s)
    }
    fn apply_sync(&self, snapshots: &[Snapshot]) -> anyhow::Result<ApplyReport> {
        self.applied.lock().unwrap().extend_from_slice(snapshots);
        Ok(ApplyReport { changed: !snapshots.is_empty(), ..Default::default() })
    }
}

fn mem_machine(base: &str, dir: &std::path::Path, name: &str, lib: Arc<MemLib>) -> Env {
    Env {
        lib,
        http: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap(),
        base: super::client::base_url(base),
        state_path: dir.join(name).join("sync.json"),
        machine_name: name.into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs MYTUBE_SYNC_TEST_SERVER: a wrangler dev with the Turnstile test keys"]
async fn the_wire_protocol_round_trips_through_a_real_server() {
    let Ok(base) = std::env::var(SERVER_ENV) else {
        eprintln!("{SERVER_ENV} is not set; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (lib_a, lib_b) = (MemLib::new("UCaaaaaaaaaaaaaaaaaaaaaa"), MemLib::new("UCbbbbbbbbbbbbbbbbbbbbbb"));
    let a = mem_machine(&base, dir.path(), "A", lib_a.clone());
    let b = mem_machine(&base, dir.path(), "B", lib_b.clone());

    // A mistyped code reaches no group, and leaves nothing behind.
    let stray = SyncCode::generate().unwrap().to_string();
    assert_eq!(join(&b, &stray).await.unwrap_err().to_string(), super::client::NO_GROUP);
    assert!(state::load_from(&b.state_path).unwrap().is_none());

    create(&a, DUMMY_TOKEN).await.unwrap();
    let code = state::load_from(&a.state_path).unwrap().unwrap().code;
    let joined = join(&b, &code).await.unwrap();
    assert!(joined.changed);
    assert_eq!(lib_b.applied_from(), ["A"], "B pulled A's slot on joining");

    // A clean A pulls without a blob and finds B's.
    exchange(&a).await;
    assert_eq!(lib_a.applied_from(), ["B"]);
    let st_a = state::load_from(&a.state_path).unwrap().unwrap();
    let st_b = state::load_from(&b.state_path).unwrap().unwrap();
    assert_eq!(st_a.peers[&st_b.machine_id].name, "B");
    assert!(st_a.peers[&st_b.machine_id].written_at.is_some());
    assert_eq!(st_b.peers[&st_a.machine_id].name, "A");

    // Nothing new: a pull brings nothing back, because `have` is current.
    exchange(&a).await;
    assert_eq!(lib_a.applied_from(), ["B"]);

    // B changes, pushes on its own (no pull asked), and A reads the new slot.
    lib_b.snapshot.lock().unwrap().channels[0].member = true;
    let pushed = run_once(&b, Instant::now(), &AtomicBool::new(false)).await.unwrap();
    assert!(matches!(pushed, Outcome::Sent(Ok(_))), "{pushed:?}");
    exchange(&a).await;
    assert_eq!(lib_a.applied_from(), ["B", "B"]);
    assert!(lib_a.applied.lock().unwrap()[1].channels[0].member);

    // A reset: a new group under a new code, with only A in it.
    create(&a, DUMMY_TOKEN).await.unwrap();
    let reset = state::load_from(&a.state_path).unwrap().unwrap();
    assert_ne!(reset.code, code);
    exchange(&a).await;
    assert_eq!(lib_a.applied_from(), ["B", "B"], "the new group holds nobody else");
}

fn watched(db: &Db, id: &str) -> bool {
    db.get_video(id).unwrap().unwrap_or_else(|| panic!("{id} missing")).watched
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs MYTUBE_SYNC_TEST_SERVER: a wrangler dev with the Turnstile test keys"]
async fn two_libraries_converge_through_a_real_server() {
    let Ok(base) = std::env::var(SERVER_ENV) else {
        eprintln!("{SERVER_ENV} is not set; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (a, db_a) = machine(&base, dir.path(), "A");
    let (b, db_b) = machine(&base, dir.path(), "B");

    // Different libraries before they ever meet.
    db_a.upsert_channel(&channel("UCaaaaaaaaaaaaaaaaaaaaaa", true)).unwrap();
    db_a.insert_video_if_new(&video("aaaaaaaaaa1", "UCaaaaaaaaaaaaaaaaaaaaaa", false)).unwrap();
    db_a.set_watched("aaaaaaaaaa1", true).unwrap();
    db_b.upsert_channel(&channel("UCbbbbbbbbbbbbbbbbbbbbbb", true)).unwrap();
    // An ad-hoc upload: its unsubscribed parent has to travel with it.
    db_b.upsert_channel(&channel("UCxxxxxxxxxxxxxxxxxxxxxx", false)).unwrap();
    db_b.insert_video_if_new(&video("bbbbbbbbbb1", "UCxxxxxxxxxxxxxxxxxxxxxx", true)).unwrap();

    create(&a, DUMMY_TOKEN).await.unwrap();
    let code = state::load_from(&a.state_path).unwrap().unwrap().code;
    join(&b, &code).await.unwrap();
    rounds(&a, &b, 2).await;

    assert!(db_b.get_channel("UCaaaaaaaaaaaaaaaaaaaaaa").unwrap().is_some_and(|c| c.subscribed));
    assert!(db_a.get_channel("UCbbbbbbbbbbbbbbbbbbbbbb").unwrap().is_some_and(|c| c.subscribed));
    assert!(watched(&db_b, "aaaaaaaaaa1"));
    assert!(db_a.get_video("bbbbbbbbbb1").unwrap().is_some());

    // Stamps are whole seconds and a tie goes to `true`, so the un-watch has
    // to land in a later second than the watch to win.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    db_b.set_watched("aaaaaaaaaa1", false).unwrap();
    db_a.set_hidden("bbbbbbbbbb1", true).unwrap();
    rounds(&a, &b, 3).await;

    assert!(!watched(&db_a, "aaaaaaaaaa1"), "B's un-watch reached A");
    assert!(db_b.get_video("bbbbbbbbbb1").unwrap().unwrap().hidden, "A's hide reached B");

    let snap_a = db_a.sync_snapshot("same").unwrap();
    let snap_b = db_b.sync_snapshot("same").unwrap();
    assert_eq!(snap_a, snap_b, "the two libraries did not converge");

    // Converged means quiet: neither machine has anything left to push.
    for env in [&a, &b] {
        let out = run_once(env, Instant::now(), &AtomicBool::new(false)).await.unwrap();
        assert!(matches!(out, Outcome::Idle(super::Decision::Skip)), "{}: {out:?}", env.machine_name);
    }
}
