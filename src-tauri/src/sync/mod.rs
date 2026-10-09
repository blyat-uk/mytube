//! Opt-in cloud sync. See docs/superpowers/specs/2026-10-09-cloud-sync-design.md.
//!
//! Two layers. The lower one -- [`create`], [`join`], [`run_once`],
//! [`status`] -- knows nothing of Tauri: it takes an [`Env`] (a library, an
//! HTTP client, a server address and a state-file path) and does one whole
//! operation, so tests can drive two libraries against a real server with no
//! window and no event loop. The upper one is [`Syncer`], managed as Tauri
//! state: the clock, the one-request-at-a-time lock, the pace, the events and
//! the follow-up work (thumbnails, polls) an apply leaves behind.
//!
//! The pace is the whole cost model. The server's edge allows one request per
//! 10 s per IP and the free plan must never be outgrown, so each machine sends
//! at most one request a minute, and only when it has something to push or has
//! been asked to pull. A hidden, idle machine sends nothing.

pub mod client;
pub mod code;
pub mod crypto;
pub mod snapshot;
pub mod state;
pub mod verify;

#[cfg(test)]
mod e2e;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tauri::{AppHandle, Emitter};

use crate::config;
use crate::db::Db;
use crate::models::{SyncPeer, SyncStatus};
use crate::poll::{self, AppState};
use client::ClientError;
use code::SyncCode;
use crypto::{Encoded, OpenError};
use snapshot::{ApplyReport, Snapshot};
use state::{Peer, SyncState};

/// Carries [`SyncStatus`], after every attempt and every change of state.
pub const STATUS_EVENT: &str = "sync://status";

/// No payload. The library changed under every view, so `App.tsx` refetches.
/// Deliberately not `poll://finished`, which would also toast "N new videos"
/// and move the tray badge for rows that are not new to the user at all.
pub const FINISHED_EVENT: &str = "sync://finished";

pub const EXPIRED: &str = "This sync code is no longer in use: nothing synced with it for 90 days.";
pub const NEWER: &str = "Another computer runs a newer MyTube. Update this one to keep syncing.";

/// One request a minute, at most, per machine.
pub const PACE: Duration = Duration::from_secs(60);

/// The longest a `Retry-After` is honoured for. The pace lives in memory only,
/// so a restart would undo a longer one anyway; this just keeps a garbled
/// header from silencing sync for the rest of the session.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

// ------------------------------------------------------------- the rule

/// What one wake of the clock does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Off, expired, or nothing to push and nobody asked for a pull.
    Skip,
    /// Something to do, but the last request was under a minute ago.
    Wait(Instant),
    /// One `/v1/sync`, with the blob only when there is a change to push.
    Exchange { send_blob: bool },
}

/// The clock's whole rule, with nothing to read but its arguments.
pub fn decide(
    now: Instant,
    next_allowed: Instant,
    enabled: bool,
    expired: bool,
    dirty: bool,
    pull_wanted: bool,
) -> Decision {
    if !enabled || expired {
        return Decision::Skip;
    }
    if !dirty && !pull_wanted {
        return Decision::Skip;
    }
    if now < next_allowed {
        return Decision::Wait(next_allowed);
    }
    Decision::Exchange { send_blob: dirty }
}

/// When the next request may go, after one went at `now`: a minute, or longer
/// if the edge asked for longer.
pub fn next_allowed_after(now: Instant, retry_after: Option<Duration>) -> Instant {
    now + retry_after.unwrap_or_default().min(MAX_RETRY_AFTER).max(PACE)
}

// ------------------------------------------------------------- the library

/// The two halves of `snapshot`'s contract, as sync needs them. `Db` is the
/// only real implementation; the trait exists so the exchange can be tested
/// against a library that is not a database.
pub trait Library: Send + Sync + 'static {
    fn sync_snapshot(&self, machine_name: &str) -> Result<Snapshot>;
    fn apply_sync(&self, snapshots: &[Snapshot]) -> Result<ApplyReport>;
}

impl Library for Db {
    fn sync_snapshot(&self, machine_name: &str) -> Result<Snapshot> {
        Db::sync_snapshot(self, machine_name)
    }
    fn apply_sync(&self, snapshots: &[Snapshot]) -> Result<ApplyReport> {
        Db::apply_sync(self, snapshots)
    }
}

/// Everything one sync operation needs, none of it Tauri's.
#[derive(Clone)]
pub struct Env {
    pub lib: Arc<dyn Library>,
    pub http: reqwest::Client,
    /// From [`client::base_url`]: no trailing slash.
    pub base: String,
    /// `sync.json`. A parameter, not `state::path()`, because
    /// `MYTUBE_CONFIG_DIR` is process-wide and two test libraries need two files.
    pub state_path: PathBuf,
    pub machine_name: String,
}

/// This computer's name as the others will show it: the hostname, without the
/// `.local` macOS appends to every one.
pub fn machine_name() -> String {
    machine_name_from(&gethostname::gethostname().to_string_lossy())
}

fn machine_name_from(raw: &str) -> String {
    let raw = raw.trim();
    let name = match raw.len().checked_sub(".local".len()) {
        Some(cut) if raw.is_char_boundary(cut) && raw[cut..].eq_ignore_ascii_case(".local") => &raw[..cut],
        _ => raw,
    };
    if name.is_empty() { "This computer".to_string() } else { name.to_string() }
}

// ------------------------------------------------------------- failures

/// Why an operation did not finish.
#[derive(Debug)]
pub enum Failure {
    /// The request went out and the server said no, or never answered.
    Client(ClientError),
    /// Nothing was sent: the library could not be read, a slot could not be
    /// sealed, `sync.json` could not be written.
    Local(anyhow::Error),
    /// The request went through -- anything pushed is on the server -- but
    /// what came back could not be applied. `have` did not advance, so the
    /// same slots come back on the next pull.
    Apply(anyhow::Error),
}

impl Failure {
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Client(e) => e.retry_after(),
            _ => None,
        }
    }

    /// Whether a pull that failed this way should be asked for again. An
    /// outage passes; an expired group, a full one, or a server that refuses
    /// this build's requests will say the same thing every minute.
    fn worth_retrying(&self) -> bool {
        match self {
            Self::Client(e) => matches!(
                e,
                ClientError::Network(_) | ClientError::RateLimited { .. } | ClientError::Server(_)
            ),
            Self::Local(_) => false,
            Self::Apply(_) => true,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(e) => write!(f, "{e}"),
            Self::Local(e) => write!(f, "{e:#}"),
            Self::Apply(e) => write!(f, "Could not apply what the other computers synced: {e:#}"),
        }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Self::Local(e)
    }
}

// ------------------------------------------------------------- the operations

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| anyhow::anyhow!("sync task failed: {e}"))?
}

async fn load(env: &Env) -> Result<Option<SyncState>> {
    let path = env.state_path.clone();
    blocking(move || state::load_from(&path)).await
}

async fn save(env: &Env, st: &SyncState) -> Result<()> {
    let (path, st) = (env.state_path.clone(), st.clone());
    blocking(move || state::save_to(&path, &st)).await
}

/// This machine's snapshot, serialised once and hashed, on the blocking pool:
/// it reads every touched row under the connection lock.
async fn local(env: &Env) -> Result<Encoded> {
    let (lib, name) = (env.lib.clone(), env.machine_name.clone());
    blocking(move || crypto::encode(&lib.sync_snapshot(&name)?)).await
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One `/v1/sync`: pushes `local` when `send_blob`, opens what comes back,
/// applies it, and folds the answer into `st` -- whatever the outcome, so the
/// caller only has to save. Never stamps anything with the local clock but
/// `last_sync_at`.
async fn exchange(
    env: &Env,
    st: &mut SyncState,
    local: &Encoded,
    send_blob: bool,
) -> Result<ApplyReport, Failure> {
    let keys = match SyncCode::parse(&st.code) {
        Ok(code) => code.keys(),
        Err(e) => {
            st.last_error = Some(format!("The saved sync code is unreadable: {e:#}"));
            return Err(Failure::Local(e));
        }
    };
    let blob = if send_blob {
        match crypto::seal(&local.json, &keys.enc, &st.machine_id) {
            Ok(b) => Some(b),
            Err(e) => {
                st.last_error = Some(format!("{e:#}"));
                return Err(Failure::Local(e));
            }
        }
    } else {
        None
    };

    let resp = match client::sync(&env.http, &env.base, &keys.bearer, &st.machine_id, &st.have, blob.as_deref())
        .await
    {
        Ok(r) => r,
        Err(ClientError::NoGroup) => {
            st.expired = true;
            st.last_error = Some(EXPIRED.into());
            return Err(Failure::Client(ClientError::NoGroup));
        }
        Err(e) => {
            st.last_error = Some(e.to_string());
            return Err(Failure::Client(e));
        }
    };
    if send_blob {
        st.pushed_hash = Some(local.hash.clone());
    } else if !resp.machines.contains(&st.machine_id) {
        // The server dropped this machine's slot -- ninety days with no write
        // while the others kept the group alive -- so what was "already pushed"
        // is on the server no longer. Forgetting the hash makes the next wake
        // push it again; otherwise this machine's own edits would live nowhere
        // but here, and the others would prune it from their lists.
        st.pushed_hash = None;
    }

    // Folded into copies, committed only once the apply has succeeded: a
    // `have` that ran ahead of the library would never fetch those slots again.
    let mut have = st.have.clone();
    let mut peers = st.peers.clone();
    let mut snaps = Vec::new();
    let mut newer = false;
    for slot in resp.slots {
        if slot.machine == st.machine_id {
            continue;
        }
        match crypto::open(&slot.blob, &keys.enc, &slot.machine) {
            Ok(snap) => {
                have.insert(slot.machine.clone(), slot.rev);
                peers.insert(
                    slot.machine,
                    Peer { name: snap.machine_name.clone(), written_at: Some(slot.written_at) },
                );
                snaps.push(snap);
            }
            // Skipped for good: the same rev would fail the same way, so
            // `have` moves past it. Only that machine's next write is read.
            Err(OpenError::Unreadable(e)) => {
                eprintln!("mytube: sync: skipping slot {}: {e:#}", slot.machine);
                have.insert(slot.machine.clone(), slot.rev);
                if let Some(p) = peers.get_mut(&slot.machine) {
                    p.written_at = Some(slot.written_at);
                }
            }
            // Not skipped for good: once this machine is updated it can read
            // the slot, so `have` stays put and the slot comes back each pull.
            Err(OpenError::Newer { machine_name, .. }) => {
                newer = true;
                let name = machine_name
                    .or_else(|| peers.get(&slot.machine).map(|p| p.name.clone()))
                    .unwrap_or_else(|| "Another computer".into());
                peers.insert(slot.machine, Peer { name, written_at: Some(slot.written_at) });
            }
        }
    }
    // A machine whose slot expired, or that rejoined under a new id, is gone
    // from `machines`; keeping it would only grow `have` towards the cap.
    let me = st.machine_id.clone();
    let present = |m: &String| *m != me && resp.machines.contains(m);
    have.retain(|m, _| present(m));
    peers.retain(|m, _| present(m));
    st.peers = peers;

    let report = if snaps.is_empty() {
        Ok(ApplyReport::default())
    } else {
        let lib = env.lib.clone();
        blocking(move || lib.apply_sync(&snaps)).await
    };
    match report {
        Ok(report) => {
            st.have = have;
            st.last_sync_at = Some(unix_now());
            st.expired = false;
            st.last_error = newer.then(|| NEWER.to_string());
            Ok(report)
        }
        Err(e) => {
            let failure = Failure::Apply(e);
            st.last_error = Some(failure.to_string());
            Err(failure)
        }
    }
}

/// What one wake of the clock came to.
#[derive(Debug)]
pub enum Outcome {
    /// No `sync.json`: sync is off.
    Off,
    /// Nothing was sent: `Skip` or `Wait`.
    Idle(Decision),
    /// One request went out (or was about to, for a local failure); `sync.json`
    /// has been saved with the result.
    Sent(Result<ApplyReport, Failure>),
}

/// One wake of the clock without the clock: load `sync.json`, measure the
/// library, decide, exchange, save. No pacing and no events -- [`Syncer`]
/// adds those -- so a test can call it directly.
///
/// `pull_wanted` is taken, not just read, before the request goes out, so a
/// Sync now that arrives while it is in flight survives it; a failure worth
/// retrying puts it back.
pub async fn run_once(env: &Env, next_allowed: Instant, pull_wanted: &AtomicBool) -> Result<Outcome> {
    let Some(mut st) = load(env).await? else { return Ok(Outcome::Off) };
    let now = Instant::now();
    // The gates alone first, as though there were something to send: the
    // snapshot reads every touched row, which is not worth doing for a request
    // that could not go out anyway.
    if let gated @ (Decision::Skip | Decision::Wait(_)) =
        decide(now, next_allowed, true, st.expired, true, true)
    {
        return Ok(Outcome::Idle(gated));
    }
    let local = local(env).await?;
    let dirty = st.pushed_hash.as_deref() != Some(local.hash.as_str());
    let pull = pull_wanted.load(Ordering::SeqCst);
    let Decision::Exchange { send_blob } = decide(now, next_allowed, true, st.expired, dirty, pull) else {
        return Ok(Outcome::Idle(Decision::Skip));
    };

    let pulled = pull_wanted.swap(false, Ordering::SeqCst);
    let before = st.clone();
    let result = exchange(env, &mut st, &local, send_blob).await;
    if let Err(f) = &result {
        if pulled && f.worth_retrying() {
            pull_wanted.store(true, Ordering::SeqCst);
        }
    }
    // An outage fails the same way every minute; rewriting an identical file
    // each time would be wear for nothing.
    if st != before {
        save(env, &st).await?;
    }
    Ok(Outcome::Sent(result))
}

/// Creates a group holding this machine's snapshot, under a fresh code, and
/// saves the new identity -- replacing any old one only now that the new group
/// exists. Turn on, and Reset code when sync was already on.
pub async fn create(env: &Env, turnstile_token: &str) -> Result<SyncState, Failure> {
    let code = SyncCode::generate()?;
    let machine_id = code::new_machine_id()?;
    let keys = code.keys();
    let local = local(env).await?;
    let blob = crypto::seal(&local.json, &keys.enc, &machine_id)?;
    client::create_group(&env.http, &env.base, &keys.bearer, turnstile_token, &machine_id, &blob)
        .await
        .map_err(Failure::Client)?;
    let mut st = SyncState::new(code.to_string(), machine_id);
    st.pushed_hash = Some(local.hash);
    st.last_sync_at = Some(unix_now());
    save(env, &st).await?;
    Ok(st)
}

/// Joins the group behind `code` with a fresh machine id: one `/v1/sync` that
/// pushes this library and pulls everyone else's. A refusal (`no_group`,
/// `full`, an outage) saves nothing, so a mistyped code leaves sync exactly as
/// it was.
pub async fn join(env: &Env, code: &str) -> Result<ApplyReport, Failure> {
    let code = SyncCode::parse(code)?;
    let mut st = SyncState::new(code.to_string(), code::new_machine_id()?);
    let local = local(env).await?;
    match exchange(env, &mut st, &local, true).await {
        Ok(report) => {
            save(env, &st).await?;
            Ok(report)
        }
        // This machine's slot exists now, so the identity is kept; the empty
        // `have` makes the next pull fetch everything again.
        Err(Failure::Apply(e)) => {
            save(env, &st).await?;
            Err(Failure::Apply(e))
        }
        Err(other) => Err(other),
    }
}

/// Turn off: forget the code. The library is not touched, and nothing is sent
/// -- there is no leave endpoint, so this machine's slot simply expires.
pub async fn leave(env: &Env) -> Result<()> {
    let path = env.state_path.clone();
    blocking(move || state::remove_at(&path)).await
}

/// The status as the Settings section draws it.
pub fn status_of(st: Option<&SyncState>, machine_name: &str, current_hash: Option<&str>) -> SyncStatus {
    let Some(st) = st else {
        return SyncStatus { machine_name: machine_name.to_string(), ..Default::default() };
    };
    let mut peers: Vec<SyncPeer> = st
        .peers
        .values()
        .map(|p| SyncPeer { name: p.name.clone(), last_write_at: p.written_at })
        .collect();
    peers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(b.last_write_at.cmp(&a.last_write_at)));
    SyncStatus {
        enabled: true,
        code: Some(st.code.clone()),
        machine_name: machine_name.to_string(),
        peers,
        last_sync_at: st.last_sync_at,
        pending: current_hash.is_some_and(|h| st.pushed_hash.as_deref() != Some(h)),
        error: st.last_error.clone(),
        expired: st.expired,
    }
}

/// [`status_of`] with the state file and the library read for it.
pub async fn status(env: &Env) -> SyncStatus {
    match load(env).await {
        Ok(None) => status_of(None, &env.machine_name, None),
        Ok(Some(st)) => {
            // A library that cannot be read shows as nothing pending rather
            // than as an error of its own: the next exchange reports it.
            let hash = local(env).await.ok().map(|l| l.hash);
            status_of(Some(&st), &env.machine_name, hash.as_deref())
        }
        Err(e) => SyncStatus {
            machine_name: env.machine_name.clone(),
            error: Some(format!("{e:#}")),
            ..Default::default()
        },
    }
}

// ------------------------------------------------------------- the clock

/// Sync as Tauri state. Not called `Sync`, which would shadow the marker trait
/// in every module that names it.
pub struct Syncer {
    /// Wakes the clock early: Sync now, window focus, a finished poll.
    wake: tokio::sync::Notify,
    /// Startup, and every Sync now, ask for one pull. Cleared by a pull that
    /// worked; see [`run_once`] for how one in flight is not lost.
    pull_wanted: AtomicBool,
    /// No request before this. Memory only: a restart starts afresh, and the
    /// edge's own limit is 10 s, so nothing is lost by forgetting it.
    next_allowed: std::sync::Mutex<Instant>,
    /// Held by everything that sends, for the whole load → request → save, so
    /// the clock can never save over a Turn on, a Join or a Turn off that
    /// landed while its request was out.
    send: tokio::sync::Mutex<()>,
    state_path: PathBuf,
    machine_name: String,
}

impl Syncer {
    pub fn new(state_path: PathBuf) -> Self {
        Self {
            wake: tokio::sync::Notify::new(),
            pull_wanted: AtomicBool::new(true),
            next_allowed: std::sync::Mutex::new(Instant::now()),
            send: tokio::sync::Mutex::new(()),
            state_path,
            machine_name: machine_name(),
        }
    }

    /// The server address is re-read each time, like the poll interval, so a
    /// hand edit to `sync_server` needs no restart.
    async fn env(&self, app_state: &AppState) -> Env {
        let server = tokio::task::spawn_blocking(|| config::load().map(|s| s.sync_server))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        Env {
            lib: app_state.db.clone(),
            http: app_state.http.clone(),
            base: client::base_url(&server),
            state_path: self.state_path.clone(),
            machine_name: self.machine_name.clone(),
        }
    }

    fn next_allowed(&self) -> Instant {
        *self.next_allowed.lock().unwrap()
    }

    fn pace(&self, retry_after: Option<Duration>) {
        *self.next_allowed.lock().unwrap() = next_allowed_after(Instant::now(), retry_after);
    }

    /// One wake. Returns when it would next like to run, if sooner than the
    /// timer: the end of a `Wait`.
    async fn tick(&self, app: &AppHandle, app_state: &Arc<AppState>) -> Option<Instant> {
        let send = self.send.lock().await;
        let env = self.env(app_state).await;
        let outcome = match run_once(&env, self.next_allowed(), &self.pull_wanted).await {
            Ok(o) => o,
            Err(e) => {
                eprintln!("mytube: sync: {e:#}");
                return None;
            }
        };
        let result = match outcome {
            Outcome::Off | Outcome::Idle(Decision::Skip) => return None,
            Outcome::Idle(Decision::Wait(until)) => return Some(until),
            Outcome::Idle(Decision::Exchange { .. }) => return None,
            Outcome::Sent(result) => result,
        };
        self.pace(result.as_ref().err().and_then(Failure::retry_after));
        drop(send);
        match &result {
            Ok(report) => follow_up(app, app_state, report),
            Err(f) => eprintln!("mytube: sync failed: {}", detail(f)),
        }
        emit_status(app, &env).await;
        None
    }

    pub async fn status(&self, app_state: &AppState) -> SyncStatus {
        status(&self.env(app_state).await).await
    }

    /// Turn on, or Reset code when already on. Turnstile is waited for
    /// *without* the request lock -- a person is solving it, and the clock may
    /// keep exchanging meanwhile -- and the lock is taken only to send and save.
    pub async fn turn_on(&self, app: &AppHandle, app_state: &Arc<AppState>) -> Result<SyncStatus, String> {
        let env = self.env(app_state).await;
        let token = verify::token(app, &env.base).await.map_err(|e| format!("{e:#}"))?;
        let result = {
            let _send = self.send.lock().await;
            let r = create(&env, &token).await;
            self.pace(r.as_ref().err().and_then(Failure::retry_after));
            r
        };
        let status = emit_status(app, &env).await;
        match result {
            Ok(_) => Ok(status),
            Err(f) => Err(f.to_string()),
        }
    }

    /// Join another computer's group. Not held to the minute's pace -- the user
    /// is waiting on it -- but it does start the next minute.
    pub async fn join(&self, app: &AppHandle, app_state: &Arc<AppState>, code: String) -> Result<SyncStatus, String> {
        let env = self.env(app_state).await;
        let result = {
            let _send = self.send.lock().await;
            let r = join(&env, &code).await;
            // A code that does not parse never reached the server.
            if !matches!(r, Err(Failure::Local(_))) {
                self.pace(r.as_ref().err().and_then(Failure::retry_after));
            }
            r
        };
        let outcome = match result {
            Ok(report) => {
                follow_up(app, app_state, &report);
                Ok(())
            }
            // Joined, but the pull could not be applied: the state says so, and
            // the clock tries the pull again in a minute.
            Err(Failure::Apply(_)) => {
                self.pull_wanted.store(true, Ordering::SeqCst);
                self.wake.notify_one();
                Ok(())
            }
            Err(f) => Err(f.to_string()),
        };
        let status = emit_status(app, &env).await;
        outcome.map(|()| status)
    }

    /// Turn off. Under the request lock, so an exchange in flight cannot save
    /// the file back after it is gone.
    pub async fn leave(&self, app: &AppHandle, app_state: &AppState) -> Result<SyncStatus, String> {
        let env = self.env(app_state).await;
        {
            let _send = self.send.lock().await;
            leave(&env).await.map_err(|e| format!("{e:#}"))?;
        }
        Ok(emit_status(app, &env).await)
    }

    /// Asks for one pull and wakes the clock, which still keeps the pace. A
    /// no-op while sync is off, so nothing is left wanted for whenever it is
    /// turned on.
    pub async fn sync_now(&self) {
        let path = self.state_path.clone();
        let on = tokio::task::spawn_blocking(move || path.exists()).await.unwrap_or(false);
        if on {
            self.pull_wanted.store(true, Ordering::SeqCst);
            self.wake.notify_one();
        }
    }
}

/// The clock: one task for the life of the app, woken by a minute's timer or
/// by [`Syncer::sync_now`]. There is no request on Quit -- the next start's
/// pull, or the next minute on any other machine, catches up.
pub async fn run_clock(app: AppHandle, app_state: Arc<AppState>, syncer: Arc<Syncer>) {
    loop {
        let wake_at = syncer.tick(&app, &app_state).await;
        let nap = wake_at
            .map(|t| t.saturating_duration_since(Instant::now()))
            .unwrap_or(PACE)
            .min(PACE);
        tokio::select! {
            _ = tokio::time::sleep(nap) => {}
            _ = syncer.wake.notified() => {}
        }
    }
}

/// What an apply leaves to do: thumbnails for the new rows, a poll for the new
/// subscriptions, and a refetch if anything moved.
fn follow_up(app: &AppHandle, app_state: &Arc<AppState>, report: &ApplyReport) {
    if !report.videos_added.is_empty() {
        let st = app_state.clone();
        let ids = report.videos_added.clone();
        tauri::async_runtime::spawn(async move {
            let jobs = ids.into_iter().map(|id| {
                let url = crate::ytdlp::thumb_url_for(&id);
                (id, url)
            });
            poll::cache_thumbs(&st, jobs).await;
        });
    }
    if !report.channels_added.is_empty() {
        let (st, app) = (app_state.clone(), app.clone());
        let ids = report.channels_added.clone();
        tauri::async_runtime::spawn(async move {
            let db = st.db.clone();
            let channels = tokio::task::spawn_blocking(move || {
                ids.iter().filter_map(|id| db.get_channel(id).ok().flatten()).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            // `poll_channels` try-locks: if a poll is already running this is
            // skipped, and the next interval poll and `deepen` catch up.
            if !channels.is_empty() {
                poll::poll_channels(&st, &app, channels).await;
            }
        });
    }
    if report.changed {
        let _ = app.emit(FINISHED_EVENT, ());
    }
}

async fn emit_status(app: &AppHandle, env: &Env) -> SyncStatus {
    let s = status(env).await;
    let _ = app.emit(STATUS_EVENT, &s);
    s
}

/// The journal's version of a failure: the user's sentence hides the cause.
fn detail(f: &Failure) -> String {
    match f {
        Failure::Client(ClientError::Network(d)) => format!("network: {d}"),
        Failure::Client(ClientError::Server(s)) => format!("server answered {s}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::client::tests::{fake_server, reply, Reply};
    use crate::sync::snapshot::{SyncChannel, SNAPSHOT_FORMAT};
    use std::sync::Mutex;

    // ---- the rule

    #[test]
    fn decide_skips_when_off_or_expired_whatever_else_is_true() {
        let t = Instant::now();
        assert_eq!(decide(t, t, false, false, true, true), Decision::Skip);
        assert_eq!(decide(t, t, true, true, true, true), Decision::Skip);
        assert_eq!(decide(t, t + PACE, true, true, true, true), Decision::Skip);
    }

    #[test]
    fn decide_skips_an_idle_machine_even_when_it_could_send() {
        let t = Instant::now();
        assert_eq!(decide(t, t, true, false, false, false), Decision::Skip);
        assert_eq!(decide(t + PACE, t, true, false, false, false), Decision::Skip);
    }

    #[test]
    fn decide_waits_out_the_minute() {
        let t = Instant::now();
        let next = t + Duration::from_secs(30);
        assert_eq!(decide(t, next, true, false, true, false), Decision::Wait(next));
        assert_eq!(decide(t, next, true, false, false, true), Decision::Wait(next));
    }

    #[test]
    fn decide_pushes_when_dirty_and_pulls_alone_when_only_asked() {
        let t = Instant::now();
        assert_eq!(decide(t, t, true, false, true, false), Decision::Exchange { send_blob: true });
        assert_eq!(decide(t, t, true, false, true, true), Decision::Exchange { send_blob: true });
        assert_eq!(decide(t, t, true, false, false, true), Decision::Exchange { send_blob: false });
        assert_eq!(
            decide(t + Duration::from_secs(5), t, true, false, false, true),
            Decision::Exchange { send_blob: false }
        );
    }

    #[test]
    fn the_next_request_waits_a_minute_or_what_the_edge_asked() {
        let t = Instant::now();
        assert_eq!(next_allowed_after(t, None), t + PACE);
        assert_eq!(next_allowed_after(t, Some(Duration::from_secs(10))), t + PACE);
        assert_eq!(next_allowed_after(t, Some(Duration::from_secs(300))), t + Duration::from_secs(300));
        assert_eq!(next_allowed_after(t, Some(Duration::from_secs(86_400))), t + MAX_RETRY_AFTER);
    }

    #[test]
    fn the_machine_name_loses_its_dot_local() {
        assert_eq!(machine_name_from("Annas-MacBook-Pro.local"), "Annas-MacBook-Pro");
        assert_eq!(machine_name_from("studio.LOCAL\n"), "studio");
        assert_eq!(machine_name_from("desk"), "desk");
        assert_eq!(machine_name_from("desk.lan"), "desk.lan");
        assert_eq!(machine_name_from(".local"), "This computer");
        assert_eq!(machine_name_from(""), "This computer");
        assert!(!machine_name().is_empty());
    }

    // ---- the status

    #[test]
    fn status_when_off_names_only_this_computer() {
        let s = status_of(None, "desk", Some("abc"));
        assert_eq!(s, SyncStatus { machine_name: "desk".into(), ..Default::default() });
    }

    #[test]
    fn status_when_on_reports_the_code_peers_and_what_is_pending() {
        let mut st = SyncState::new("MYT1-CODE".into(), "me".into());
        st.pushed_hash = Some("old".into());
        st.last_sync_at = Some(5);
        st.peers.insert("m2".into(), Peer { name: "laptop".into(), written_at: Some(9) });
        st.peers.insert("m1".into(), Peer { name: "Desk".into(), written_at: None });
        let s = status_of(Some(&st), "studio", Some("new"));
        assert!(s.enabled && s.pending && !s.expired);
        assert_eq!(s.code.as_deref(), Some("MYT1-CODE"));
        assert_eq!(s.last_sync_at, Some(5));
        assert_eq!(
            s.peers,
            vec![
                SyncPeer { name: "Desk".into(), last_write_at: None },
                SyncPeer { name: "laptop".into(), last_write_at: Some(9) },
            ]
        );
        assert!(!status_of(Some(&st), "studio", Some("old")).pending);
        assert!(!status_of(Some(&st), "studio", None).pending, "an unreadable library is not pending");
    }

    #[test]
    fn status_serialises_camel_case_for_the_frontend() {
        let mut st = SyncState::new("C".into(), "me".into());
        st.peers.insert("m".into(), Peer { name: "laptop".into(), written_at: Some(9) });
        let v = serde_json::to_value(status_of(Some(&st), "desk", None)).unwrap();
        for key in ["enabled", "code", "machineName", "peers", "lastSyncAt", "pending", "error", "expired"] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
        assert_eq!(v["peers"][0], serde_json::json!({"name": "laptop", "lastWriteAt": 9}));
    }

    // ---- the operations, against a fake server and a fake library

    /// A library that is a snapshot in memory, recording what it is asked to
    /// apply.
    struct FakeLib {
        snapshot: Mutex<Snapshot>,
        applied: Mutex<Vec<Vec<Snapshot>>>,
        fail_apply: AtomicBool,
        fail_snapshot: AtomicBool,
    }

    impl Library for FakeLib {
        fn sync_snapshot(&self, machine_name: &str) -> Result<Snapshot> {
            if self.fail_snapshot.load(Ordering::SeqCst) {
                anyhow::bail!("library unreadable");
            }
            let mut s = self.snapshot.lock().unwrap().clone();
            s.machine_name = machine_name.to_string();
            Ok(s)
        }
        fn apply_sync(&self, snapshots: &[Snapshot]) -> Result<ApplyReport> {
            if self.fail_apply.load(Ordering::SeqCst) {
                anyhow::bail!("disk full");
            }
            self.applied.lock().unwrap().push(snapshots.to_vec());
            Ok(ApplyReport { channels_added: vec!["UC9".into()], videos_added: vec![], changed: true })
        }
    }

    fn snap(channel: &str) -> Snapshot {
        Snapshot {
            format: SNAPSHOT_FORMAT,
            machine_name: String::new(),
            channels: vec![SyncChannel {
                id: channel.into(),
                title: channel.into(),
                handle: None,
                url: format!("https://www.youtube.com/channel/{channel}"),
                subscribed: true,
                subscribed_changed_at: Some(1),
                member: false,
                member_changed_at: None,
            }],
            videos: vec![],
            tombstones: vec![],
        }
    }

    struct Rig {
        env: Env,
        lib: Arc<FakeLib>,
        _dir: tempfile::TempDir,
    }

    fn rig(base: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let lib = Arc::new(FakeLib {
            snapshot: Mutex::new(snap("UC1")),
            applied: Mutex::new(vec![]),
            fail_apply: AtomicBool::new(false),
            fail_snapshot: AtomicBool::new(false),
        });
        let env = Env {
            lib: lib.clone(),
            http: reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap(),
            base: base.to_string(),
            state_path: dir.path().join("sync.json"),
            machine_name: "desk".into(),
        };
        Rig { env, lib, _dir: dir }
    }

    const CODE: [u8; 16] = [42; 16];
    const ME: &str = "MEMEMEMEMEMEMEMEMEMEME";
    const PEER: &str = "PEERPEERPEERPEERPEERPE";
    const GONE: &str = "GONEGONEGONEGONEGONEGO";

    fn enc() -> [u8; 32] {
        SyncCode::from_bytes(CODE).keys().enc
    }

    fn blob_from(machine: &str, mut s: Snapshot, name: &str) -> String {
        s.machine_name = name.into();
        crypto::seal(&crypto::encode(&s).unwrap().json, &enc(), machine).unwrap()
    }

    fn on(rig: &Rig) -> SyncState {
        let mut st = SyncState::new(SyncCode::from_bytes(CODE).to_string(), ME.into());
        st.have.insert(GONE.into(), 3);
        st.peers.insert(GONE.into(), Peer { name: "old".into(), written_at: Some(1) });
        state::save_to(&rig.env.state_path, &st).unwrap();
        st
    }

    fn saved(rig: &Rig) -> Option<SyncState> {
        state::load_from(&rig.env.state_path).unwrap()
    }

    fn sync_reply(slots: serde_json::Value) -> Reply {
        let body = serde_json::json!({"rev": 7, "machines": [ME, PEER], "slots": slots});
        reply(200, &body.to_string())
    }

    #[tokio::test]
    async fn a_dirty_machine_pushes_applies_and_records_what_it_read() {
        let (base, seen) = fake_server(vec![sync_reply(serde_json::json!([
            {"machine": PEER, "rev": 6, "written_at": 1_700_000_000, "blob": blob_from(PEER, snap("UC2"), "laptop")}
        ]))]);
        let rig = rig(&base);
        on(&rig);
        let pull = AtomicBool::new(false);
        let Outcome::Sent(Ok(report)) = run_once(&rig.env, Instant::now(), &pull).await.unwrap() else {
            panic!("expected a successful exchange");
        };
        assert!(report.changed);

        let req = seen.recv().unwrap();
        assert_eq!(req.body["machine"], ME);
        assert!(req.body["blob"].is_string(), "a dirty machine pushes");
        assert_eq!(req.body["have"], serde_json::json!({GONE: 3}));

        let applied = rig.lib.applied.lock().unwrap().clone();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0][0].channels[0].id, "UC2");
        assert_eq!(applied[0][0].machine_name, "laptop");

        let st = saved(&rig).unwrap();
        // Pruned to the group's machines: GONE is no longer in `machines`.
        assert_eq!(st.have, [(PEER.to_string(), 6)].into());
        assert_eq!(st.peers, [(PEER.to_string(), Peer { name: "laptop".into(), written_at: Some(1_700_000_000) })].into());
        let hash = crypto::encode(&rig.lib.sync_snapshot("desk").unwrap()).unwrap().hash;
        assert_eq!(st.pushed_hash.as_deref(), Some(hash.as_str()));
        assert!(st.last_sync_at.is_some());
        assert_eq!(st.last_error, None);
    }

    #[tokio::test]
    async fn a_hidden_idle_machine_sends_nothing() {
        // No replies at all: any request would fail and show up as Sent.
        let (base, _seen) = fake_server(vec![]);
        let rig = rig(&base);
        let mut st = on(&rig);
        st.pushed_hash = Some(crypto::encode(&rig.lib.sync_snapshot("desk").unwrap()).unwrap().hash);
        state::save_to(&rig.env.state_path, &st).unwrap();
        let out = run_once(&rig.env, Instant::now(), &AtomicBool::new(false)).await.unwrap();
        assert!(matches!(out, Outcome::Idle(Decision::Skip)), "{out:?}");
    }

    #[tokio::test]
    async fn off_and_expired_send_nothing_and_a_wait_is_reported() {
        let (base, _seen) = fake_server(vec![]);
        let rig = rig(&base);
        let pull = AtomicBool::new(true);
        assert!(matches!(run_once(&rig.env, Instant::now(), &pull).await.unwrap(), Outcome::Off));

        let mut st = on(&rig);
        let later = Instant::now() + PACE;
        assert!(matches!(
            run_once(&rig.env, later, &pull).await.unwrap(),
            Outcome::Idle(Decision::Wait(t)) if t == later
        ));
        st.expired = true;
        state::save_to(&rig.env.state_path, &st).unwrap();
        assert!(matches!(run_once(&rig.env, Instant::now(), &pull).await.unwrap(), Outcome::Idle(Decision::Skip)));
        assert!(pull.load(Ordering::SeqCst), "nothing sent, so the pull is still wanted");
    }

    #[tokio::test]
    async fn a_clean_machine_asked_to_pull_sends_no_blob_and_clears_the_wish() {
        let (base, seen) = fake_server(vec![sync_reply(serde_json::json!([]))]);
        let rig = rig(&base);
        let mut st = on(&rig);
        let hash = crypto::encode(&rig.lib.sync_snapshot("desk").unwrap()).unwrap().hash;
        st.pushed_hash = Some(hash.clone());
        state::save_to(&rig.env.state_path, &st).unwrap();
        let pull = AtomicBool::new(true);
        let out = run_once(&rig.env, Instant::now(), &pull).await.unwrap();
        assert!(matches!(out, Outcome::Sent(Ok(ref r)) if !r.changed), "{out:?}");
        assert!(!seen.recv().unwrap().body.as_object().unwrap().contains_key("blob"));
        assert!(!pull.load(Ordering::SeqCst));
        assert!(rig.lib.applied.lock().unwrap().is_empty(), "nothing came back, nothing applied");
        assert_eq!(saved(&rig).unwrap().pushed_hash, Some(hash));
    }

    #[tokio::test]
    async fn a_pull_that_finds_this_machines_slot_gone_pushes_again_next_time() {
        // Ninety days without a write cost this machine its slot, while the
        // others kept the group alive.
        let body = serde_json::json!({"rev": 40, "machines": [PEER], "slots": []});
        let (base, _seen) = fake_server(vec![reply(200, &body.to_string())]);
        let rig = rig(&base);
        let mut st = on(&rig);
        st.pushed_hash = Some(crypto::encode(&rig.lib.sync_snapshot("desk").unwrap()).unwrap().hash);
        state::save_to(&rig.env.state_path, &st).unwrap();
        run_once(&rig.env, Instant::now(), &AtomicBool::new(true)).await.unwrap();
        assert_eq!(saved(&rig).unwrap().pushed_hash, None);
        let next = run_once(&rig.env, Instant::now() + PACE, &AtomicBool::new(false)).await.unwrap();
        assert!(matches!(next, Outcome::Idle(Decision::Wait(_))), "dirty again, so waiting to push: {next:?}");
    }

    #[tokio::test]
    async fn an_unreadable_slot_is_skipped_but_read_past() {
        let other_key_blob = crypto::seal(&crypto::encode(&snap("UC2")).unwrap().json, &[0; 32], PEER).unwrap();
        let (base, _seen) = fake_server(vec![sync_reply(serde_json::json!([
            {"machine": PEER, "rev": 4, "written_at": 10, "blob": other_key_blob}
        ]))]);
        let rig = rig(&base);
        on(&rig);
        let out = run_once(&rig.env, Instant::now(), &AtomicBool::new(true)).await.unwrap();
        assert!(matches!(out, Outcome::Sent(Ok(_))), "{out:?}");
        assert!(rig.lib.applied.lock().unwrap().is_empty());
        let st = saved(&rig).unwrap();
        assert_eq!(st.have.get(PEER), Some(&4));
        assert_eq!(st.last_error, None);
    }

    #[tokio::test]
    async fn a_newer_slot_is_skipped_without_reading_past_it_and_says_so() {
        let newer = br#"{"format":99,"machine_name":"future","whatever":true}"#;
        let blob = crypto::seal(newer, &enc(), PEER).unwrap();
        let (base, _seen) = fake_server(vec![sync_reply(serde_json::json!([
            {"machine": PEER, "rev": 4, "written_at": 10, "blob": blob}
        ]))]);
        let rig = rig(&base);
        on(&rig);
        run_once(&rig.env, Instant::now(), &AtomicBool::new(true)).await.unwrap();
        let st = saved(&rig).unwrap();
        assert_eq!(st.have.get(PEER), None, "read again once this machine is updated");
        assert_eq!(st.last_error.as_deref(), Some(NEWER));
        assert_eq!(st.peers[PEER].name, "future");
        assert!(rig.lib.applied.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_apply_keeps_have_where_it_was_and_wants_the_pull_again() {
        let (base, _seen) = fake_server(vec![sync_reply(serde_json::json!([
            {"machine": PEER, "rev": 6, "written_at": 10, "blob": blob_from(PEER, snap("UC2"), "laptop")}
        ]))]);
        let rig = rig(&base);
        on(&rig);
        rig.lib.fail_apply.store(true, Ordering::SeqCst);
        let pull = AtomicBool::new(true);
        let out = run_once(&rig.env, Instant::now(), &pull).await.unwrap();
        assert!(matches!(out, Outcome::Sent(Err(Failure::Apply(_)))), "{out:?}");
        assert!(pull.load(Ordering::SeqCst));
        let st = saved(&rig).unwrap();
        assert_eq!(st.have.get(PEER), None);
        assert!(st.pushed_hash.is_some(), "the push itself landed");
        assert!(st.last_error.unwrap().contains("disk full"));
    }

    #[tokio::test]
    async fn no_group_marks_the_code_expired() {
        let (base, _seen) = fake_server(vec![reply(404, r#"{"error":"no_group"}"#)]);
        let rig = rig(&base);
        on(&rig);
        let pull = AtomicBool::new(true);
        let out = run_once(&rig.env, Instant::now(), &pull).await.unwrap();
        assert!(matches!(out, Outcome::Sent(Err(Failure::Client(ClientError::NoGroup)))));
        let st = saved(&rig).unwrap();
        assert!(st.expired);
        assert_eq!(st.last_error.as_deref(), Some(EXPIRED));
        assert!(!pull.load(Ordering::SeqCst), "an expired group is not asked again");
        assert!(status_of(Some(&st), "desk", None).expired);
    }

    #[tokio::test]
    async fn an_outage_keeps_the_pull_wanted_and_a_full_group_does_not() {
        let (base, _seen) = fake_server(vec![reply(503, ""), reply(409, r#"{"error":"full"}"#)]);
        let rig = rig(&base);
        let mut st = on(&rig);
        st.pushed_hash = Some(crypto::encode(&rig.lib.sync_snapshot("desk").unwrap()).unwrap().hash);
        state::save_to(&rig.env.state_path, &st).unwrap();

        let pull = AtomicBool::new(true);
        run_once(&rig.env, Instant::now(), &pull).await.unwrap();
        assert!(pull.load(Ordering::SeqCst));
        assert_eq!(saved(&rig).unwrap().last_error.as_deref(), Some(client::SERVER));

        run_once(&rig.env, Instant::now(), &pull).await.unwrap();
        assert!(!pull.load(Ordering::SeqCst));
        let st = saved(&rig).unwrap();
        assert_eq!(st.last_error.as_deref(), Some(client::FULL));
        assert!(!st.expired);
    }

    #[tokio::test]
    async fn a_library_that_cannot_be_read_sends_nothing() {
        let (base, _seen) = fake_server(vec![]);
        let rig = rig(&base);
        on(&rig);
        rig.lib.fail_snapshot.store(true, Ordering::SeqCst);
        assert!(run_once(&rig.env, Instant::now(), &AtomicBool::new(true)).await.is_err());
        assert!(!status(&rig.env).await.pending);
    }

    #[tokio::test]
    async fn create_saves_a_new_identity_only_when_the_group_exists() {
        let (base, seen) = fake_server(vec![
            reply(403, r#"{"error":"verification_failed"}"#),
            reply(201, r#"{"rev":1}"#),
        ]);
        let rig = rig(&base);
        let old = on(&rig);

        let err = create(&rig.env, "bad-token").await.unwrap_err();
        assert_eq!(err.to_string(), client::VERIFICATION_FAILED);
        assert_eq!(saved(&rig), Some(old.clone()), "a failed reset keeps the old code");
        let _ = seen.recv().unwrap();

        let st = create(&rig.env, "XXXX.DUMMY.TOKEN.XXXX").await.unwrap();
        let req = seen.recv().unwrap();
        assert_eq!(req.path, "/v1/group");
        assert_eq!(req.body["turnstileToken"], "XXXX.DUMMY.TOKEN.XXXX");
        assert_eq!(req.body["machine"], st.machine_id.as_str());
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {}", SyncCode::parse(&st.code).unwrap().keys().bearer).as_str())
        );
        // What was pushed opens with the new code under the new machine id.
        let pushed = crypto::open(req.body["blob"].as_str().unwrap(), &SyncCode::parse(&st.code).unwrap().keys().enc, &st.machine_id).unwrap();
        assert_eq!(pushed.machine_name, "desk");

        assert_ne!(st.code, old.code);
        assert_ne!(st.machine_id, old.machine_id);
        assert!(st.have.is_empty() && st.peers.is_empty() && !st.expired);
        assert!(st.pushed_hash.is_some());
        assert_eq!(saved(&rig), Some(st));
    }

    #[tokio::test]
    async fn join_saves_nothing_when_refused() {
        let (base, _seen) = fake_server(vec![
            reply(404, r#"{"error":"no_group"}"#),
            reply(409, r#"{"error":"full"}"#),
        ]);
        let rig = rig(&base);
        let code = SyncCode::from_bytes(CODE).to_string();
        assert_eq!(join(&rig.env, &code).await.unwrap_err().to_string(), client::NO_GROUP);
        assert_eq!(saved(&rig), None);
        assert_eq!(join(&rig.env, &code).await.unwrap_err().to_string(), client::FULL);
        assert_eq!(saved(&rig), None);
        // A code that does not parse never reaches the server.
        assert!(matches!(join(&rig.env, "MYT1-nope").await, Err(Failure::Local(_))));
    }

    #[tokio::test]
    async fn join_pushes_with_an_empty_have_and_saves_what_it_read() {
        let (base, seen) = fake_server(vec![sync_reply(serde_json::json!([
            {"machine": PEER, "rev": 2, "written_at": 10, "blob": blob_from(PEER, snap("UC2"), "laptop")}
        ]))]);
        let rig = rig(&base);
        let report = join(&rig.env, &SyncCode::from_bytes(CODE).to_string().to_lowercase()).await.unwrap();
        assert!(report.changed);
        let req = seen.recv().unwrap();
        assert_eq!(req.body["have"], serde_json::json!({}));
        assert!(req.body["blob"].is_string());
        let st = saved(&rig).unwrap();
        assert_eq!(st.code, SyncCode::from_bytes(CODE).to_string(), "saved in its canonical form");
        assert_eq!(req.body["machine"], st.machine_id.as_str());
        assert_eq!(st.have, [(PEER.to_string(), 2)].into());
        assert_eq!(st.peers[PEER].name, "laptop");
        assert!(st.pushed_hash.is_some());
    }

    #[tokio::test]
    async fn leave_forgets_the_code_and_nothing_else() {
        let (base, _seen) = fake_server(vec![]);
        let rig = rig(&base);
        on(&rig);
        leave(&rig.env).await.unwrap();
        assert_eq!(saved(&rig), None);
        assert!(!status(&rig.env).await.enabled);
        leave(&rig.env).await.unwrap();
    }
}
