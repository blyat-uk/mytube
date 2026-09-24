//! The three external programs MyTube runs -- yt-dlp, ffmpeg (with ffprobe) and
//! deno -- found, downloaded, verified and kept up to date.
//!
//! - `sources.rs`: which build to fetch for this OS/arch, and how to read the
//!   publishers' checksum files and redirects (with what was observed live).
//! - `locate.rs`: the resolution order -- override, managed, system -- and
//!   each tool's `--version`.
//! - `install.rs`: stream, hash, verify, extract, rename into place.
//!
//! Resolution is cached: every download and every probe asks for yt-dlp, and a
//! PyInstaller `--version` costs a second or more, so the tools are looked up
//! once and again only after an install, an update, a change to one of the
//! settings.json override keys, or a resolved file disappearing.

mod install;
mod locate;
mod sources;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, OwnedRwLockReadGuard, RwLock};

use crate::config::Settings;
use crate::models::{ToolKind, ToolProgress, ToolSource, ToolState, ToolStatus};
use crate::ytdlp::{Cookies, Runner};

use install::{Checksum, Fetch, Manifest, Plan, Unpack};
use locate::{Resolution, Search};
use sources::{FfmpegSource, Hosts, Target};

/// How stale the managed yt-dlp's last update check may get.
const UPDATE_EVERY_SECS: i64 = 24 * 60 * 60;

/// A verified update that finds yt-dlp in use tries the swap again this
/// often...
const SWAP_RETRY: Duration = Duration::from_secs(15);
/// ...for this long, before it is given up until the next check. On a
/// machine that is always downloading, giving up at the first busy lease
/// meant every day's 40 MB was fetched, verified and thrown away, and the
/// update never landed at all; a queue drains between jobs often enough for
/// one of these tries to find the lease free.
const SWAP_PATIENCE: Duration = Duration::from_secs(30 * 60);

/// Where managed copies live: `dirs::data_local_dir()/mytube/bin`. Not
/// `config_dir()`, which on Windows is the roaming profile.
pub fn bin_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mytube")
        .join("bin")
}

/// A resolved yt-dlp, ready to run, plus a read lease on the managed binaries.
/// Hold it for the whole life of the process it spawns: the updater only swaps
/// yt-dlp while no lease is out.
pub struct Invocation {
    pub runner: Runner,
    pub _lease: OwnedRwLockReadGuard<()>,
}

fn idx(kind: ToolKind) -> usize {
    match kind {
        ToolKind::Ytdlp => 0,
        ToolKind::Ffmpeg => 1,
        ToolKind::Deno => 2,
    }
}

fn override_of(s: &Settings, kind: ToolKind) -> &str {
    match kind {
        ToolKind::Ytdlp => &s.ytdlp_path,
        ToolKind::Ffmpeg => &s.ffmpeg_path,
        ToolKind::Deno => &s.deno_path,
    }
}

fn channel_of(s: &Settings) -> &'static str {
    if s.ytdlp_channel == "stable" {
        "stable"
    } else {
        "nightly"
    }
}

/// What `--ffmpeg-location` should say for a resolved ffmpeg.
///
/// A managed or system copy is called `ffmpeg` with `ffprobe` beside it, and
/// its directory is what yt-dlp has always been given. An override is a file
/// the person named, and it need not be called `ffmpeg` at all
/// (`ffmpeg-7.exe`, `ffmpeg.git`): handed its directory, yt-dlp would look for
/// `ffmpeg` in there and run some other build or none. So an override passes
/// the binary itself, which `--ffmpeg-location` accepts -- yt-dlp then runs
/// exactly that file and finds ffprobe beside it by the same name with
/// `ffmpeg` swapped for `ffprobe` (checked in `postprocessor/ffmpeg.py`,
/// `_determine_executables`).
fn ffmpeg_location(f: &locate::Found) -> Option<PathBuf> {
    if f.source == ToolSource::Override {
        Some(f.path.clone())
    } else {
        f.path.parent().map(Path::to_path_buf)
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Whether the managed yt-dlp is due an update check: never checked, checked
/// over a day ago, or installed from a channel other than the one now chosen.
fn update_due(record: Option<&install::Record>, channel: &str, now: i64) -> bool {
    let Some(r) = record else { return true };
    if r.channel.as_deref() != Some(channel) {
        return true;
    }
    match r.last_check {
        Some(t) => now - t >= UPDATE_EVERY_SECS,
        None => true,
    }
}

/// What one tool is doing right now, beyond where it resolved.
#[derive(Debug, Clone, Default)]
struct Activity {
    /// `Installing` / `Updating` while that is under way.
    busy: Option<ToolState>,
    /// The last install or update failure, until one succeeds.
    error: Option<String>,
    /// `error` is a failed *update* of a copy that still works. The status
    /// then says `ready` and carries the message as a note: nothing is
    /// broken, the newest build just has not arrived.
    update_failed: bool,
}

struct Cache {
    /// The three override keys the resolution was made under.
    key: [String; 3],
    res: [Resolution; 3],
}

struct State {
    cache: Option<Cache>,
    activity: [Activity; 3],
    manifest: Manifest,
}

enum Outcome {
    Done,
    UpToDate,
    Skipped,
    /// A yt-dlp process held a lease, so nothing was swapped.
    Busy,
}

pub struct Tools {
    bin: PathBuf,
    http: reqwest::Client,
    target: Target,
    hosts: Hosts,
    search: Search,
    app: OnceLock<AppHandle>,
    /// Read by every yt-dlp run for its whole life; written only for the
    /// instant an update renames a new binary over the old one.
    lease: Arc<RwLock<()>>,
    /// One install/update pass at a time. `try_lock`ed, never waited on: a
    /// second `ensure_all` while one is running has nothing to add.
    work: Mutex<()>,
    state: Mutex<State>,
    /// How often and for how long a staged update waits for the lease
    /// ([`SWAP_RETRY`], [`SWAP_PATIENCE`]); shortened by the tests.
    swap_retry: Duration,
    swap_patience: Duration,
}

impl Tools {
    pub fn new(bin_dir: PathBuf, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self::with_parts(bin_dir, http, Target::host(), Hosts::real(), Search::system()))
    }

    fn with_parts(bin: PathBuf, http: reqwest::Client, target: Target, hosts: Hosts, search: Search) -> Self {
        install::sweep(&bin);
        let manifest = Manifest::load(&bin);
        Tools {
            bin,
            http,
            target,
            hosts,
            search,
            app: OnceLock::new(),
            lease: Arc::new(RwLock::new(())),
            work: Mutex::new(()),
            state: Mutex::new(State { cache: None, activity: Default::default(), manifest }),
            swap_retry: SWAP_RETRY,
            swap_patience: SWAP_PATIENCE,
        }
    }

    /// Lets status and progress reach the UI. Before this is called, nothing is
    /// emitted (the CLI self-test never calls it).
    pub fn set_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// Resolves yt-dlp, ffmpeg and deno for `s` and takes a lease. An `Err`
    /// reads "yt-dlp is not available yet: <why>".
    pub async fn ytdlp(&self, s: &Settings) -> Result<Invocation> {
        self.invocation(s, crate::detect::resolve_cookies(s)).await
    }

    /// [`Tools::ytdlp`] with the cookie source already decided -- the part
    /// that is this module's, and what the tests drive.
    async fn invocation(&self, s: &Settings, cookies: Cookies) -> Result<Invocation> {
        // Lease first, so an update cannot swap the binary between resolving
        // its path and the caller spawning it.
        let lease = self.lease.clone().read_owned().await;
        let res = self.resolved(s).await;
        let program = match &res[0] {
            Resolution::Found(f) => f.path.clone(),
            Resolution::BadOverride(why) => bail!("yt-dlp is not available yet: {why}"),
            Resolution::Missing => {
                let a = self.state.lock().await.activity[0].clone();
                let why = match (a.busy, a.error) {
                    (Some(_), _) => "it is still being downloaded".to_string(),
                    (None, Some(e)) => format!("downloading it failed: {e}"),
                    (None, None) => "it is not installed, and MyTube has not downloaded it yet".to_string(),
                };
                bail!("yt-dlp is not available yet: {why}");
            }
        };
        let ffmpeg_location = res[1].found().and_then(ffmpeg_location);
        let deno = res[2].found().map(|f| f.path.clone());
        Ok(Invocation { runner: Runner { program, ffmpeg_location, deno, cookies }, _lease: lease })
    }

    /// The cached resolution for `s`, redone when the override keys changed
    /// or a file it named has gone.
    async fn resolved(&self, s: &Settings) -> [Resolution; 3] {
        let key = ToolKind::ALL.map(|k| override_of(s, k).trim().to_string());
        let mut st = self.state.lock().await;
        if let Some(c) = &st.cache {
            let intact = c.res.iter().all(|r| r.found().is_none_or(|f| f.path.is_file()));
            if c.key == key && intact {
                return c.res.clone();
            }
        }
        let mut res: [Resolution; 3] = Default::default();
        for kind in ToolKind::ALL {
            res[idx(kind)] = locate::resolve(kind, &key[idx(kind)], &self.bin, self.target, &self.search).await;
        }
        st.cache = Some(Cache { key, res: res.clone() });
        res
    }

    async fn invalidate(&self) {
        self.state.lock().await.cache = None;
    }

    pub async fn status(&self, s: &Settings) -> Vec<ToolStatus> {
        let res = self.resolved(s).await;
        let st = self.state.lock().await;
        ToolKind::ALL
            .iter()
            .map(|&kind| {
                let i = idx(kind);
                let a = &st.activity[i];
                let managed_record = st.manifest.get(kind);
                let (path, version, source, bad) = match &res[i] {
                    Resolution::Found(f) => {
                        let version = f.version.clone().or_else(|| {
                            (f.source == ToolSource::Managed)
                                .then(|| managed_record.and_then(|r| r.version.clone()))
                                .flatten()
                        });
                        (Some(f.path.display().to_string()), version, f.source, None)
                    }
                    Resolution::BadOverride(why) => (
                        Some(override_of(s, kind).trim().to_string()),
                        None,
                        ToolSource::Override,
                        Some(why.clone()),
                    ),
                    Resolution::Missing => (None, None, ToolSource::Missing, None),
                };
                let error = a.error.clone().or(bad);
                // A failed update check on a copy that resolved and runs is a
                // note, not a fault: downloads go on working with it.
                let usable_anyway = a.update_failed && matches!(res[i], Resolution::Found(_));
                let state = a.busy.unwrap_or(if error.is_some() && !usable_anyway {
                    ToolState::Error
                } else {
                    ToolState::Ready
                });
                let last_check = (kind == ToolKind::Ytdlp && source == ToolSource::Managed)
                    .then(|| managed_record.and_then(|r| r.last_check))
                    .flatten();
                ToolStatus { kind, path, version, source, state, error, last_check }
            })
            .collect()
    }

    async fn emit_status(&self, s: &Settings) {
        if let Some(app) = self.app.get() {
            let statuses = self.status(s).await;
            let _ = app.emit("tools://status", statuses);
        }
    }

    fn emit_progress(&self, p: ToolProgress) {
        if let Some(app) = self.app.get() {
            let _ = app.emit("tools://progress", p);
        }
    }

    async fn set_busy(&self, kind: ToolKind, busy: Option<ToolState>) {
        self.state.lock().await.activity[idx(kind)].busy = busy;
    }

    async fn set_error(&self, kind: ToolKind, error: Option<String>) {
        let a = &mut self.state.lock().await.activity[idx(kind)];
        a.error = error;
        a.update_failed = false;
    }

    /// Downloads whatever resolves to nothing. Never fails: errors land in the
    /// status. Called at startup and on every poll tick.
    pub async fn ensure_all(&self, s: &Settings) {
        let Ok(_work) = self.work.try_lock() else { return };
        self.ensure_locked(s).await;
    }

    /// Whether `kind` needs a first install. yt-dlp does whenever there is
    /// no managed copy -- a system one only tides things over until the
    /// managed, kept-current one lands. ffmpeg and deno only when nothing at
    /// all resolved. An override is never second-guessed.
    async fn needs_install(&self, kind: ToolKind, s: &Settings) -> bool {
        if !override_of(s, kind).trim().is_empty() {
            return false;
        }
        match kind {
            ToolKind::Ytdlp => locate::managed(kind, &self.bin, self.target).is_none(),
            _ => matches!(self.resolved(s).await[idx(kind)], Resolution::Missing),
        }
    }

    async fn ensure_locked(&self, s: &Settings) {
        for kind in ToolKind::ALL {
            if self.needs_install(kind, s).await {
                let _ = self.install(kind, s, ToolState::Installing, None, None).await;
            } else if kind != ToolKind::Ytdlp {
                // Resolved some other way since (a system package, an
                // override): an old download failure no longer applies.
                // yt-dlp's error can be an update's, which only a successful
                // check clears.
                self.set_error(kind, None).await;
            }
        }
        self.emit_status(s).await;
    }

    /// The daily yt-dlp update check (managed copy only, `ytdlp_auto_update`).
    ///
    /// A verified download that finds yt-dlp in use waits for it, trying the
    /// swap every [`SWAP_RETRY`] for up to [`SWAP_PATIENCE`]. Only `work` is
    /// held meanwhile, which polls and downloads never touch -- they take
    /// read leases, and nothing here holds the lease between tries -- and
    /// which keeps a second install or update from starting alongside.
    pub async fn maybe_update(&self, s: &Settings) {
        let Ok(_work) = self.work.try_lock() else { return };
        if let Outcome::Busy = self.update_ytdlp(s, false, self.swap_patience).await {
            eprintln!(
                "[mytube] yt-dlp: the update stayed busy for {} min; trying again at the next check",
                self.swap_patience.as_secs() / 60
            );
        }
    }

    /// "Check for updates" / "Retry": forces the check and retries failed
    /// installs. `Err("busy …")` while any lease is held.
    pub async fn update_now(&self, s: &Settings) -> Result<Vec<ToolStatus>> {
        let Ok(_work) = self.work.try_lock() else {
            bail!("busy: MyTube is already installing or updating a tool");
        };
        match self.lease.clone().try_write_owned() {
            Ok(g) => drop(g),
            Err(_) => bail!("busy: a download or refresh is using yt-dlp; try again when it finishes"),
        }
        self.ensure_locked(s).await;
        // No patience here: the button is waiting on this answer, and "busy,
        // try again" is the honest one when a job started mid-download.
        if let Outcome::Busy = self.update_ytdlp(s, true, Duration::ZERO).await {
            bail!("busy: a download or refresh started using yt-dlp; try again when it finishes");
        }
        Ok(self.status(s).await)
    }

    /// Checks the chosen channel's latest yt-dlp and installs it if it is not
    /// the one we have. `force` skips the 24 h gate and the auto-update
    /// setting (the "Check for updates" button). Only ever the managed copy.
    /// `patience` is how long a staged download may wait for the lease.
    async fn update_ytdlp(&self, s: &Settings, force: bool, patience: Duration) -> Outcome {
        const K: ToolKind = ToolKind::Ytdlp;
        if !s.ytdlp_path.trim().is_empty() || locate::managed(K, &self.bin, self.target).is_none() {
            return Outcome::Skipped;
        }
        let channel = channel_of(s);
        if !force {
            let record = self.state.lock().await.manifest.get(K).cloned();
            if !s.ytdlp_auto_update || !update_due(record.as_ref(), channel, now()) {
                return Outcome::Skipped;
            }
        }
        // Nothing to gain from downloading 40 MB that could not be swapped in.
        match self.lease.clone().try_write_owned() {
            Ok(g) => drop(g),
            Err(_) => return Outcome::Busy,
        }

        self.set_busy(K, Some(ToolState::Updating)).await;
        self.emit_status(s).await;
        let result = async {
            let tag = self.latest_tag(sources::ytdlp_repo(channel)).await?;
            let current = self.state.lock().await.manifest.get(K).cloned().unwrap_or_default();
            if current.version.as_deref() == Some(tag.as_str()) && current.channel.as_deref() == Some(channel) {
                let mut st = self.state.lock().await;
                st.manifest.entry(K).last_check = Some(now());
                st.manifest.save(&self.bin)?;
                return Ok(Outcome::UpToDate);
            }
            self.install_inner(K, s, Some(tag), Some(patience)).await
        }
        .await;
        self.finish(K, s, result, true).await
    }

    /// A first install (or an update when `guarded`), start to finish, with
    /// the status and error kept current around it.
    async fn install(
        &self,
        kind: ToolKind,
        s: &Settings,
        busy: ToolState,
        tag: Option<String>,
        guarded: Option<Duration>,
    ) -> Outcome {
        self.set_busy(kind, Some(busy)).await;
        self.emit_status(s).await;
        let result = self.install_inner(kind, s, tag, guarded).await;
        self.finish(kind, s, result, false).await
    }

    /// Records how an install or update ended. `update` marks the update of
    /// a copy that is already installed, whose failure the status shows as a
    /// note on a `ready` tool rather than as an error.
    async fn finish(&self, kind: ToolKind, s: &Settings, result: Result<Outcome>, update: bool) -> Outcome {
        let outcome = match result {
            Ok(o) => {
                if !matches!(o, Outcome::Busy) {
                    self.set_error(kind, None).await;
                }
                o
            }
            Err(e) => {
                let msg = if update { format!("Update check failed: {e:#}") } else { format!("{e:#}") };
                eprintln!("[mytube] {}: {msg}", kind.label());
                let a = &mut self.state.lock().await.activity[idx(kind)];
                a.error = Some(msg);
                a.update_failed = update;
                Outcome::Skipped
            }
        };
        self.set_busy(kind, None).await;
        self.emit_status(s).await;
        outcome
    }

    /// `guarded`: `Some(patience)` swaps only while no yt-dlp holds a lease,
    /// waiting up to `patience` for one to be free; `None` places at once (a
    /// first install, which nothing can be running).
    async fn install_inner(
        &self,
        kind: ToolKind,
        s: &Settings,
        tag: Option<String>,
        guarded: Option<Duration>,
    ) -> Result<Outcome> {
        let plan = self.plan(kind, s, tag).await?;
        let progress = |p: ToolProgress| self.emit_progress(p);
        let staged = install::stage(&self.http, &self.bin, &plan, &progress).await?;
        let placed = if let Some(patience) = guarded {
            // The swap itself: only while no yt-dlp is running.
            let Some(guard) = self.write_lease_within(patience).await else {
                staged.discard();
                return Ok(Outcome::Busy);
            };
            let placed = staged.place(&self.bin);
            drop(guard);
            placed?
        } else {
            staged.place(&self.bin)?
        };
        self.invalidate().await;
        let version = match plan.version {
            Some(v) => Some(v),
            None => locate::version_of(kind, &placed[0]).await,
        };
        let mut st = self.state.lock().await;
        let t = now();
        let r = st.manifest.entry(kind);
        r.version = version;
        r.installed_at = Some(t);
        if kind == ToolKind::Ytdlp {
            r.last_check = Some(t);
            r.channel = Some(channel_of(s).to_string());
        }
        st.manifest.save(&self.bin).context("writing tools.json")?;
        Ok(Outcome::Done)
    }

    /// The write lease, tried now and then every `swap_retry` until
    /// `patience` has passed. Never *waited* on: a queued `write().await`
    /// would make every new read lease -- every probe, download and listing --
    /// queue behind it until the running jobs finished, which is exactly the
    /// stall the lease design exists to avoid. The sleep is async, and no
    /// lease is held between tries.
    async fn write_lease_within(&self, patience: Duration) -> Option<tokio::sync::OwnedRwLockWriteGuard<()>> {
        let started = std::time::Instant::now();
        loop {
            if let Ok(g) = self.lease.clone().try_write_owned() {
                return Some(g);
            }
            let left = patience.saturating_sub(started.elapsed());
            if left.is_zero() {
                return None;
            }
            tokio::time::sleep(self.swap_retry.min(left)).await;
        }
    }

    /// The tag `releases/latest` redirects to. A GET rather than a HEAD (see
    /// sources.rs on martin-riedl's HEAD flakiness; GitHub is fine either
    /// way) whose body is never read -- only where it landed matters.
    async fn latest_tag(&self, repo: &str) -> Result<String> {
        let url = sources::latest_url(&self.hosts, repo);
        let resp = self.http.get(&url).send().await.with_context(|| format!("fetching {url}"))?;
        if !resp.status().is_success() {
            bail!("{url} answered {}", resp.status());
        }
        sources::tag_from_release_url(resp.url().as_str())
            .ok_or_else(|| anyhow!("{url} did not lead to a release tag (landed on {})", resp.url()))
    }

    async fn plan(&self, kind: ToolKind, s: &Settings, tag: Option<String>) -> Result<Plan> {
        let a = sources::assets(self.target)?;
        let h = &self.hosts;
        let t = self.target;
        let plan = match kind {
            ToolKind::Ytdlp => {
                let repo = sources::ytdlp_repo(channel_of(s));
                let tag = match tag {
                    Some(t) => t,
                    None => self.latest_tag(repo).await?,
                };
                Plan {
                    kind,
                    fetches: vec![Fetch {
                        url: sources::release_asset_url(h, repo, &tag, a.ytdlp),
                        checksum: Checksum::InList {
                            url: sources::release_asset_url(h, repo, &tag, "SHA2-256SUMS"),
                            name: a.ytdlp.to_string(),
                        },
                        unpack: Unpack::Bare { dest: t.exe("yt-dlp") },
                    }],
                    version: Some(tag),
                }
            }
            ToolKind::Deno => {
                let tag = self.latest_tag(sources::DENO_REPO).await?;
                let url = sources::release_asset_url(
                    h,
                    sources::DENO_REPO,
                    &tag,
                    &format!("deno-{}.zip", a.deno_triple),
                );
                Plan {
                    kind,
                    fetches: vec![Fetch {
                        checksum: Checksum::Single { url: format!("{url}.sha256sum") },
                        url,
                        unpack: Unpack::Zip { take: vec![(t.exe("deno"), t.exe("deno"))] },
                    }],
                    version: Some(tag.trim_start_matches('v').to_string()),
                }
            }
            ToolKind::Ffmpeg => {
                let take = |name: &str| (t.exe(name), t.exe(name));
                let fetches = match a.ffmpeg {
                    FfmpegSource::Builds { asset } => {
                        let take = vec![take("ffmpeg"), take("ffprobe")];
                        vec![Fetch {
                            url: sources::release_asset_url(h, sources::FFMPEG_BUILDS_REPO, "latest", asset),
                            checksum: Checksum::InList {
                                url: sources::release_asset_url(
                                    h,
                                    sources::FFMPEG_BUILDS_REPO,
                                    "latest",
                                    "checksums.sha256",
                                ),
                                name: asset.to_string(),
                            },
                            unpack: if asset.ends_with(".zip") {
                                Unpack::Zip { take }
                            } else {
                                Unpack::TarXz { take }
                            },
                        }]
                    }
                    FfmpegSource::MartinRiedl { arch } => ["ffmpeg", "ffprobe"]
                        .into_iter()
                        .map(|tool| Fetch {
                            url: sources::martin_riedl_url(h, arch, tool),
                            checksum: Checksum::BesideFinal { suffix: ".sha256" },
                            unpack: Unpack::Zip { take: vec![take(tool)] },
                        })
                        .collect(),
                };
                // `latest` is a moving tag; the installed binary says what it
                // is. FFmpeg-Builds replaces its assets in place when it
                // rebuilds, so a download caught mid-republish can fail its
                // checksum against a list from the other side of the swap.
                // That installs nothing and the next poll tick tries again.
                Plan { kind, version: None, fetches }
            }
        };
        Ok(plan)
    }
}

/// `mytube --self-test <dir>`: provisions all three tools fresh from their
/// real sources into `dir`, ignoring any system copy, checks each runs, and
/// that yt-dlp reports deno as a JS runtime. Returns the report text.
pub async fn self_test(dir: &Path) -> Result<String> {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).with_context(|| format!("creating {}", bin.display()))?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let target = Target::host();
    let tools = Tools::with_parts(bin.clone(), http, target, Hosts::real(), Search::none());
    // Defaults: no overrides, the nightly channel.
    let s = Settings::default();
    let mut report = vec![format!(
        "mytube {} self-test on {}/{}, tools in {}",
        env!("CARGO_PKG_VERSION"),
        target.os,
        target.arch,
        bin.display()
    )];

    for kind in ToolKind::ALL {
        let started = std::time::Instant::now();
        tools
            .install_inner(kind, &s, None, None)
            .await
            .with_context(|| format!("installing {}", kind.label()))?;
        report.push(format!("installed {} in {:.1}s", kind.label(), started.elapsed().as_secs_f64()));
    }

    let res = tools.resolved(&s).await;
    for kind in ToolKind::ALL {
        let f = res[idx(kind)]
            .found()
            .filter(|f| f.source == ToolSource::Managed)
            .ok_or_else(|| anyhow!("{} did not resolve to the managed copy", kind.label()))?;
        let v = f
            .version
            .as_deref()
            .ok_or_else(|| anyhow!("{} at {} did not report a version", kind.label(), f.path.display()))?;
        report.push(format!("{} {v}  {}", kind.label(), f.path.display()));
    }
    let ffprobe = bin.join(target.exe("ffprobe"));
    let out = locate::output_of(crate::proc::command(&ffprobe).arg("-version"), Duration::from_secs(60))
        .await
        .context("running ffprobe -version")?;
    let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string();
    if !out.status.success() || !first.starts_with("ffprobe version ") {
        bail!("ffprobe did not run: {first}");
    }
    report.push(first.split(" Copyright").next().unwrap_or(&first).to_string());

    let ytdlp = &res[0].found().expect("checked above").path;
    let deno = &res[2].found().expect("checked above").path;
    let verbose = js_runtime_probe(ytdlp, &bin, deno).await?;
    let runtimes = sources::js_runtimes_line(&verbose)
        .ok_or_else(|| anyhow!("yt-dlp -v printed no \"JS runtimes\" line:\n{verbose}"))?;
    if !sources::deno_is_a_js_runtime(&verbose) {
        bail!("yt-dlp does not see deno as a JS runtime (JS runtimes: {runtimes})");
    }
    report.push(format!("yt-dlp JS runtimes: {runtimes}"));
    if let Some(exe) = verbose.lines().find_map(|l| l.trim().strip_prefix("[debug] exe versions:")) {
        report.push(format!("yt-dlp exe versions:{exe}"));
    }
    report.push("OK".into());
    Ok(report.join("\n"))
}

/// yt-dlp's verbose header, stdout and stderr together. `-v --version` exits
/// before the header is printed, so this passes no URL at all: the header
/// comes out, then "You must provide at least one URL" and exit status 2,
/// which is expected and ignored. `--ignore-config` keeps a user's own
/// yt-dlp config (cookies, output templates) out of it.
async fn js_runtime_probe(ytdlp: &Path, ffmpeg_dir: &Path, deno: &Path) -> Result<String> {
    let mut deno_arg = std::ffi::OsString::from("deno:");
    deno_arg.push(deno);
    let mut cmd = crate::proc::command(ytdlp);
    cmd.arg("--ignore-config")
        .arg("-v")
        .arg("--ffmpeg-location")
        .arg(ffmpeg_dir)
        .arg("--js-runtimes")
        .arg(deno_arg);
    let out = locate::output_of(&mut cmd, Duration::from_secs(120)).await.context("running yt-dlp -v")?;
    Ok(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[cfg(test)]
mod tests {
    use super::install::testserver::{start, Routes};
    use super::*;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap()
    }

    fn linux() -> Target {
        Target { os: "linux", arch: "x86_64" }
    }

    fn tools_at(bin: &Path, base: &str, search: Search) -> Tools {
        let hosts = Hosts { github: base.into(), martin_riedl: base.into() };
        Tools::with_parts(bin.to_path_buf(), client(), linux(), hosts, search)
    }

    /// A server that refuses everything: "offline", as far as the tools know.
    async fn offline() -> String {
        start(Routes::default()).await
    }

    #[test]
    fn an_update_is_due_daily_and_on_a_channel_switch() {
        let t = 1_790_000_000;
        let rec = |last: Option<i64>, ch: &str| install::Record {
            version: Some("v".into()),
            installed_at: Some(t),
            last_check: last,
            channel: Some(ch.into()),
        };
        assert!(update_due(None, "nightly", t));
        assert!(!update_due(Some(&rec(Some(t - 3600), "nightly")), "nightly", t));
        assert!(update_due(Some(&rec(Some(t - UPDATE_EVERY_SECS), "nightly")), "nightly", t));
        assert!(update_due(Some(&rec(Some(t - 60), "nightly")), "stable", t));
        assert!(update_due(Some(&rec(None, "nightly")), "nightly", t));
    }

    #[test]
    fn an_ffmpeg_override_is_located_by_its_file_and_anything_else_by_its_directory() {
        let found = |source| locate::Found { path: PathBuf::from("/opt/ff/ffmpeg-7"), source, version: None };
        assert_eq!(ffmpeg_location(&found(ToolSource::Override)), Some(PathBuf::from("/opt/ff/ffmpeg-7")));
        for source in [ToolSource::Managed, ToolSource::System] {
            assert_eq!(ffmpeg_location(&found(source)), Some(PathBuf::from("/opt/ff")));
        }
    }

    #[tokio::test]
    async fn an_unknown_target_is_an_error_in_the_status_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let hosts = Hosts { github: offline().await, martin_riedl: String::new() };
        let tools = Tools::with_parts(
            tmp.path().to_path_buf(),
            client(),
            Target { os: "plan9", arch: "mips" },
            hosts,
            Search::only(vec![]),
        );
        let s = Settings::default();
        tools.ensure_all(&s).await;
        for st in tools.status(&s).await {
            assert_eq!(st.state, ToolState::Error, "{st:?}");
            assert_eq!(st.error.as_deref(), Some("no managed build for plan9/mips"));
            assert_eq!(st.source, ToolSource::Missing);
        }
    }

    #[tokio::test]
    async fn offline_failures_land_in_the_status_and_in_the_ytdlp_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tools_at(tmp.path(), &offline().await, Search::only(vec![]));
        let s = Settings::default();
        tools.ensure_all(&s).await;
        let statuses = tools.status(&s).await;
        assert_eq!(statuses.len(), 3);
        for st in &statuses {
            assert_eq!(st.state, ToolState::Error, "{st:?}");
            assert!(st.error.as_deref().unwrap().contains("404"), "{st:?}");
        }
        let err = tools.invocation(&s, Cookies::None).await.err().unwrap().to_string();
        assert!(err.starts_with("yt-dlp is not available yet: downloading it failed:"), "{err}");
    }

    #[tokio::test]
    async fn a_second_ensure_all_while_one_runs_returns_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tools_at(tmp.path(), &offline().await, Search::only(vec![]));
        let _running = tools.work.try_lock().unwrap();
        let s = Settings::default();
        tokio::time::timeout(Duration::from_secs(2), tools.ensure_all(&s)).await.unwrap();
        // It did nothing: no attempt, so no error recorded.
        assert!(tools.status(&s).await.iter().all(|st| st.error.is_none()));
        let err = tools.update_now(&s).await.err().unwrap().to_string();
        assert!(err.starts_with("busy"), "{err}");
    }

    #[tokio::test]
    async fn a_bad_override_is_named_in_the_status_and_the_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tools = tools_at(tmp.path(), &offline().await, Search::only(vec![]));
        let s = Settings { ytdlp_path: "/nowhere/yt-dlp".into(), ..Settings::default() };
        let err = tools.invocation(&s, Cookies::None).await.err().unwrap().to_string();
        assert!(err.starts_with("yt-dlp is not available yet: ytdlp_path in settings.json"), "{err}");
        let st = &tools.status(&s).await[0];
        assert_eq!((st.source, st.state, st.path.as_deref()), (ToolSource::Override, ToolState::Error, Some("/nowhere/yt-dlp")));
        // An override is never replaced by a download.
        assert!(!tools.needs_install(ToolKind::Ytdlp, &s).await);
    }

    #[cfg(unix)]
    mod unix {
        use super::super::install::sha256_hex;
        use super::super::install::testserver::Reply;
        use super::super::locate::fake;
        use super::*;
        use std::io::Write;

        fn script(body: &str) -> Vec<u8> {
            format!("#!/bin/sh\n{body}\n").into_bytes()
        }

        #[tokio::test]
        async fn offline_with_no_managed_copy_falls_back_to_the_system_ytdlp() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let sys = tmp.path().join("sys");
            fake::all(&sys, "2026.08.19");
            let tools = tools_at(&bin, &offline().await, Search::only(vec![sys.clone()]));
            let s = Settings::default();
            tools.ensure_all(&s).await;

            let inv = tools.invocation(&s, Cookies::Browser("firefox".into())).await.unwrap();
            assert_eq!(inv.runner.program, sys.join("yt-dlp"));
            assert_eq!(inv.runner.ffmpeg_location.as_deref(), Some(sys.as_path()));
            assert_eq!(inv.runner.deno, Some(sys.join("deno")));
            assert_eq!(inv.runner.cookies, Cookies::Browser("firefox".into()));
            let st = &tools.status(&s).await[0];
            assert_eq!(st.source, ToolSource::System);
            // The failed managed download still shows, so Retry is offered.
            assert_eq!(st.state, ToolState::Error);
        }

        #[tokio::test]
        async fn the_lease_makes_update_now_busy() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            fake::all(&bin, "2026.09.16");
            let tools = tools_at(&bin, &offline().await, Search::only(vec![]));
            let s = Settings::default();
            let running = tools.invocation(&s, Cookies::None).await.unwrap();
            let err = tools.update_now(&s).await.expect_err("busy").to_string();
            assert!(err.starts_with("busy"), "{err}");
            // And the daily check quietly waits for the next tick.
            tools.maybe_update(&s).await;
            assert!(tools.status(&s).await[0].error.is_none());
            drop(running);
            // With the lease back, update_now gets as far as the (offline) network.
            let statuses = tools.update_now(&s).await.unwrap();
            assert!(statuses[0].error.as_deref().unwrap().contains("404"), "{statuses:?}");
            // And that failure is a note on a yt-dlp that still works, not a fault.
            assert_eq!(statuses[0].state, ToolState::Ready, "{statuses:?}");
            assert!(statuses[0].error.as_deref().unwrap().starts_with("Update check failed: "), "{statuses:?}");
        }

        #[tokio::test]
        async fn an_ffmpeg_override_is_handed_to_yt_dlp_as_the_binary_itself() {
            // Named something other than `ffmpeg`: given only its directory,
            // yt-dlp would look for `ffmpeg` in there and never find it.
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let own = tmp.path().join("own");
            fake::all(&bin, "2026.09.16");
            let mine = fake::tool(&own, "ffmpeg-7", "ffmpeg version 7.1 Copyright");
            let tools = tools_at(&bin, &offline().await, Search::only(vec![]));
            let s = Settings { ffmpeg_path: mine.display().to_string(), ..Settings::default() };
            let inv = tools.invocation(&s, Cookies::None).await.unwrap();
            assert_eq!(inv.runner.ffmpeg_location, Some(mine));
            // Without the override it is the managed copy's directory, as ever.
            drop(inv);
            let inv = tools.invocation(&Settings::default(), Cookies::None).await.unwrap();
            assert_eq!(inv.runner.ffmpeg_location.as_deref(), Some(bin.as_path()));
        }

        #[tokio::test]
        async fn resolution_is_cached_until_an_override_changes() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let other = tmp.path().join("other");
            fake::all(&bin, "one");
            let alt = fake::tool(&other, "yt-dlp", "two");
            let tools = tools_at(&bin, &offline().await, Search::only(vec![]));
            let s = Settings::default();
            assert_eq!(tools.status(&s).await[0].version.as_deref(), Some("one"));
            // Same settings: the answer is the cached one, not a fresh --version.
            std::fs::write(bin.join("yt-dlp"), script("echo changed")).unwrap();
            assert_eq!(tools.status(&s).await[0].version.as_deref(), Some("one"));
            let s2 = Settings { ytdlp_path: alt.display().to_string(), ..Settings::default() };
            let st = &tools.status(&s2).await[0];
            assert_eq!((st.source, st.version.as_deref()), (ToolSource::Override, Some("two")));
            // A resolved file that vanished is looked up again.
            std::fs::remove_file(&alt).unwrap();
            assert_eq!(tools.status(&s2).await[0].state, ToolState::Error);
        }

        fn zip_of(name: &str, data: &[u8]) -> Vec<u8> {
            let mut buf = std::io::Cursor::new(Vec::new());
            {
                let mut w = zip::ZipWriter::new(&mut buf);
                w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
                w.finish().unwrap();
            }
            buf.into_inner()
        }

        fn tar_xz_of(members: &[(&str, &[u8])]) -> Vec<u8> {
            let mut tar = tar::Builder::new(Vec::new());
            for (name, data) in members {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                tar.append_data(&mut h, name, *data).unwrap();
            }
            let raw = tar.into_inner().unwrap();
            let mut w = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
            w.write_all(&raw).unwrap();
            w.finish().unwrap()
        }

        /// A fake GitHub serving every source the linux/x86_64 row names, at
        /// the paths the real one uses.
        fn publish_ytdlp(routes: &Routes, repo: &str, tag: &str, body: &[u8]) {
            routes.set(&format!("/{repo}/releases/latest"), Reply::Redirect(format!("/{repo}/releases/tag/{tag}")));
            routes.set(&format!("/{repo}/releases/tag/{tag}"), Reply::Body(b"<html>release</html>".to_vec()));
            routes.set(
                &format!("/{repo}/releases/download/{tag}/SHA2-256SUMS"),
                Reply::Body(format!("{}  yt-dlp\n{}  yt-dlp_linux\n", sha256_hex(b"other"), sha256_hex(body)).into_bytes()),
            );
            routes.set(&format!("/{repo}/releases/download/{tag}/yt-dlp_linux"), Reply::Body(body.to_vec()));
        }

        fn publish_all(routes: &Routes) {
            publish_ytdlp(routes, "yt-dlp/yt-dlp-nightly-builds", "2026.09.16.232951", &script("echo 2026.09.16.232951"));
            let deno = zip_of("deno", &script("echo 'deno 2.9.7 (stable, release, x86_64-unknown-linux-gnu)'"));
            routes.set("/denoland/deno/releases/latest", Reply::Redirect("/denoland/deno/releases/tag/v2.9.7".into()));
            routes.set("/denoland/deno/releases/tag/v2.9.7", Reply::Body(b"<html/>".to_vec()));
            let d = "/denoland/deno/releases/download/v2.9.7/deno-x86_64-unknown-linux-gnu.zip";
            routes.set(&format!("{d}.sha256sum"), Reply::Body(format!("{}  deno-x86_64-unknown-linux-gnu.zip\n", sha256_hex(&deno)).into_bytes()));
            routes.set(d, Reply::Body(deno));
            let ff = tar_xz_of(&[
                ("ffmpeg-master-latest-linux64-gpl/bin/ffmpeg", &script("echo 'ffmpeg version N-1-gabc Copyright'")),
                ("ffmpeg-master-latest-linux64-gpl/bin/ffprobe", &script("echo 'ffprobe version N-1-gabc Copyright'")),
            ]);
            let f = "/yt-dlp/FFmpeg-Builds/releases/download/latest";
            routes.set(
                &format!("{f}/checksums.sha256"),
                Reply::Body(format!("{}  ffmpeg-master-latest-linux64-gpl.tar.xz\n", sha256_hex(&ff)).into_bytes()),
            );
            routes.set(&format!("{f}/ffmpeg-master-latest-linux64-gpl.tar.xz"), Reply::Body(ff));
        }

        #[tokio::test]
        async fn ensure_all_installs_everything_missing_and_ytdlp_then_resolves_it() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let tools = tools_at(&bin, &base, Search::only(vec![]));
            let s = Settings::default();
            tools.ensure_all(&s).await;

            let statuses = tools.status(&s).await;
            for st in &statuses {
                assert_eq!((st.source, st.state, st.error.as_deref()), (ToolSource::Managed, ToolState::Ready, None), "{st:?}");
            }
            assert_eq!(statuses[0].version.as_deref(), Some("2026.09.16.232951"));
            assert_eq!(statuses[1].version.as_deref(), Some("N-1-gabc"));
            assert_eq!(statuses[2].version.as_deref(), Some("2.9.7"));
            assert!(statuses[0].last_check.is_some());

            let inv = tools.invocation(&s, Cookies::None).await.unwrap();
            assert_eq!(inv.runner.program, bin.join("yt-dlp"));
            assert_eq!(inv.runner.ffmpeg_location.as_deref(), Some(bin.as_path()));
            assert_eq!(inv.runner.deno, Some(bin.join("deno")));

            let m = Manifest::load(&bin);
            assert_eq!(m.ytdlp.as_ref().unwrap().channel.as_deref(), Some("nightly"));
            assert_eq!(m.deno.as_ref().unwrap().version.as_deref(), Some("2.9.7"));
            assert_eq!(m.ffmpeg.as_ref().unwrap().version.as_deref(), Some("N-1-gabc"));
        }

        #[tokio::test]
        async fn a_system_ffmpeg_and_deno_are_not_downloaded_but_ytdlp_is() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let sys = tmp.path().join("sys");
            fake::all(&sys, "2026.08.19");
            let tools = tools_at(&bin, &base, Search::only(vec![sys.clone()]));
            let s = Settings::default();
            tools.ensure_all(&s).await;
            let st = tools.status(&s).await;
            assert_eq!(st.iter().map(|t| t.source).collect::<Vec<_>>(), vec![ToolSource::Managed, ToolSource::System, ToolSource::System]);
            assert!(!bin.join("ffmpeg").exists() && !bin.join("deno").exists());
        }

        #[tokio::test]
        async fn a_channel_switch_installs_that_channels_build() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            publish_ytdlp(&routes, "yt-dlp/yt-dlp", "2026.08.19", &script("echo 2026.08.19"));
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let tools = tools_at(&bin, &base, Search::only(vec![]));
            tools.ensure_all(&Settings::default()).await;

            let stable = Settings { ytdlp_channel: "stable".into(), ..Settings::default() };
            tools.maybe_update(&stable).await;
            let st = &tools.status(&stable).await[0];
            assert_eq!((st.version.as_deref(), st.state), (Some("2026.08.19"), ToolState::Ready), "{st:?}");
            assert_eq!(Manifest::load(&bin).ytdlp.unwrap().channel.as_deref(), Some("stable"));
            assert!(!bin.join("yt-dlp.old").exists());

            // Auto-update off: even a channel switch waits for the button.
            let back = Settings { ytdlp_auto_update: false, ..Settings::default() };
            tools.maybe_update(&back).await;
            assert_eq!(tools.status(&back).await[0].version.as_deref(), Some("2026.08.19"));
            tools.update_now(&back).await.unwrap();
            assert_eq!(tools.status(&back).await[0].version.as_deref(), Some("2026.09.16.232951"));
        }

        #[tokio::test]
        async fn the_same_latest_tag_only_bumps_last_check() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let tools = tools_at(&bin, &base, Search::only(vec![]));
            let s = Settings::default();
            tools.ensure_all(&s).await;
            // Pretend the last check was two days ago.
            {
                let mut st = tools.state.lock().await;
                st.manifest.entry(ToolKind::Ytdlp).last_check = Some(now() - 2 * UPDATE_EVERY_SECS);
                st.manifest.save(&bin).unwrap();
            }
            let installed_at = Manifest::load(&bin).ytdlp.unwrap().installed_at;
            // Were it to download, it would find this and fail the checksum.
            routes.set(
                "/yt-dlp/yt-dlp-nightly-builds/releases/download/2026.09.16.232951/yt-dlp_linux",
                Reply::Body(b"tampered".to_vec()),
            );
            tools.maybe_update(&s).await;
            let rec = Manifest::load(&bin).ytdlp.unwrap();
            assert!(rec.last_check.unwrap() >= now() - 5);
            assert_eq!(rec.installed_at, installed_at);
            assert!(tools.status(&s).await[0].error.is_none());
        }

        #[tokio::test]
        async fn a_verified_update_that_finds_yt_dlp_busy_waits_and_then_swaps() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let mut tools = tools_at(&bin, &base, Search::only(vec![]));
            tools.swap_retry = Duration::from_millis(50);
            tools.swap_patience = Duration::from_secs(20);
            let s = Settings::default();
            tools.ensure_all(&s).await;
            let before = std::fs::read(bin.join("yt-dlp")).unwrap();
            {
                let mut st = tools.state.lock().await;
                st.manifest.entry(ToolKind::Ytdlp).last_check = Some(now() - 2 * UPDATE_EVERY_SECS);
                st.manifest.save(&bin).unwrap();
            }
            // A newer build that takes ~1.6 s to arrive, so a download can
            // start in the middle of it and hold its lease past the swap.
            let newer = script("echo 2026.09.20");
            publish_ytdlp(&routes, "yt-dlp/yt-dlp-nightly-builds", "2026.09.20", &newer);
            routes.set(
                "/yt-dlp/yt-dlp-nightly-builds/releases/download/2026.09.20/yt-dlp_linux",
                Reply::Slow(newer.clone()),
            );

            let download = async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let inv = tools.invocation(&s, Cookies::None).await.unwrap();
                // The update's download has finished well before this; its
                // swap has been finding the lease taken.
                tokio::time::sleep(Duration::from_millis(2700)).await;
                let during = std::fs::read(bin.join("yt-dlp")).unwrap();
                let state = tools.status(&s).await[0].state;
                drop(inv);
                (during, state)
            };
            let ((), (during, state)) = tokio::join!(tools.maybe_update(&s), download);

            assert_eq!(during, before, "never swapped under a live job");
            assert_eq!(state, ToolState::Updating, "the wait shows as updating");
            assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), newer, "swapped once the lease was free");
            let st = &tools.status(&s).await[0];
            assert_eq!((st.version.as_deref(), st.state, st.error.as_deref()), (Some("2026.09.20"), ToolState::Ready, None));
            assert!(!bin.join(install::STAGING).read_dir().map(|mut d| d.next().is_some()).unwrap_or(false));
        }

        #[tokio::test]
        async fn a_staged_update_is_given_up_once_its_patience_runs_out() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let mut tools = tools_at(&bin, &base, Search::only(vec![]));
            tools.swap_retry = Duration::from_millis(50);
            tools.swap_patience = Duration::from_millis(400);
            let s = Settings::default();
            tools.ensure_all(&s).await;
            let before = std::fs::read(bin.join("yt-dlp")).unwrap();
            {
                let mut st = tools.state.lock().await;
                st.manifest.entry(ToolKind::Ytdlp).last_check = Some(now() - 2 * UPDATE_EVERY_SECS);
                st.manifest.save(&bin).unwrap();
            }
            let newer = script("echo 2026.09.20");
            publish_ytdlp(&routes, "yt-dlp/yt-dlp-nightly-builds", "2026.09.20", &newer);
            routes.set(
                "/yt-dlp/yt-dlp-nightly-builds/releases/download/2026.09.20/yt-dlp_linux",
                Reply::Slow(newer),
            );
            let download = async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let inv = tools.invocation(&s, Cookies::None).await.unwrap();
                tokio::time::sleep(Duration::from_millis(3000)).await;
                drop(inv);
            };
            tokio::join!(tools.maybe_update(&s), download);
            assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), before);
            let st = &tools.status(&s).await[0];
            assert_eq!((st.state, st.error.as_deref()), (ToolState::Ready, None), "busy is not a failure");
            assert!(!bin.join(install::STAGING).read_dir().map(|mut d| d.next().is_some()).unwrap_or(false));
        }

        #[tokio::test]
        async fn a_failed_update_keeps_the_working_binary() {
            let routes = Routes::default();
            let base = start(routes.clone()).await;
            publish_all(&routes);
            let tmp = tempfile::tempdir().unwrap();
            let bin = tmp.path().join("bin");
            let tools = tools_at(&bin, &base, Search::only(vec![]));
            let s = Settings::default();
            tools.ensure_all(&s).await;
            let before = std::fs::read(bin.join("yt-dlp")).unwrap();

            let newer = script("echo 2026.09.20");
            publish_ytdlp(&routes, "yt-dlp/yt-dlp-nightly-builds", "2026.09.20", &newer);
            routes.set("/yt-dlp/yt-dlp-nightly-builds/releases/download/2026.09.20/yt-dlp_linux", Reply::Truncated(newer));
            tools.update_now(&s).await.unwrap();

            assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), before);
            let st = &tools.status(&s).await[0];
            // The old binary works, so the tool is ready; the failure rides
            // along as a note for the Settings view to show quietly.
            assert_eq!((st.source, st.state), (ToolSource::Managed, ToolState::Ready), "{st:?}");
            assert!(st.error.as_deref().unwrap().starts_with("Update check failed: "), "{st:?}");
            assert_eq!(st.version.as_deref(), Some("2026.09.16.232951"));
            assert!(tools.invocation(&s, Cookies::None).await.is_ok());
            assert!(!bin.join(install::STAGING).read_dir().map(|mut d| d.next().is_some()).unwrap_or(false));
        }
    }

    /// Real downloads from the real sources, for this host. Slow (~250 MB).
    #[tokio::test]
    #[ignore]
    async fn each_tool_installs_for_this_host_and_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let tools = Tools::with_parts(bin.clone(), client(), Target::host(), Hosts::real(), Search::none());
        let s = Settings::default();
        tools.ensure_all(&s).await;
        for st in tools.status(&s).await {
            assert_eq!((st.source, st.state), (ToolSource::Managed, ToolState::Ready), "{st:?}");
            assert!(st.version.is_some(), "{st:?}");
            println!("{st:?}");
        }
        // A second pass has nothing to do and changes nothing.
        let before = std::fs::metadata(bin.join(Target::host().exe("yt-dlp"))).unwrap().modified().unwrap();
        tools.ensure_all(&s).await;
        let after = std::fs::metadata(bin.join(Target::host().exe("yt-dlp"))).unwrap().modified().unwrap();
        assert_eq!(before, after);
        // And the update check agrees the fresh install is current.
        tools.update_now(&s).await.unwrap();
        assert!(tools.status(&s).await[0].error.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn the_self_test_passes_on_this_host() {
        let tmp = tempfile::tempdir().unwrap();
        let report = self_test(tmp.path()).await.unwrap();
        println!("{report}");
        assert!(report.ends_with("OK"));
    }
}
